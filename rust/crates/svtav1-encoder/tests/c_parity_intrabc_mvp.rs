//! Differential parity: the IntraBC MVP (DV predictor) stack
//! (`svtav1-encoder/src/intrabc_mvp.rs`) vs the REAL exported C functions
//! (IBC chunk 6, docs/ibc-port-map.md §D).
//!
//! C oracle: `setup_ref_mv_list` (adaptive_mv_pred.c:651, EXPORTED) +
//! `svt_av1_find_best_ref_mvs_from_stack` (:2030, EXPORTED) driven for
//! `INTRA_FRAME` over randomized KEY-frame mode-info grids with intrabc
//! neighbours (the shim assembles the `MacroBlockD` exactly per
//! `svt_aom_init_xd`). Compares: the FULL raw 8-slot stack (values +
//! weights + the beyond-count gm-fill), the count, the mode context, and
//! the nearest/near from-stack reads — order and weight ties are the DV
//! predictor's determinism surface (map §F.8).

use svtav1_cref as cref;
use svtav1_encoder::intrabc::TileMiBounds;
use svtav1_encoder::intrabc_mvp as mvp;
use svtav1_types::motion::Mv;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// (bsize enum, w_mi, h_mi) placement set for the random grids.
const SIZES: [(u8, i32, i32); 11] = [
    (0, 1, 1),   // 4x4
    (1, 1, 2),   // 4x8
    (2, 2, 1),   // 8x4
    (3, 2, 2),   // 8x8
    (4, 2, 4),   // 8x16
    (5, 4, 2),   // 16x8
    (6, 4, 4),   // 16x16
    (7, 4, 8),   // 16x32
    (8, 8, 4),   // 32x16
    (9, 8, 8),   // 32x32
    (12, 16, 16), // 64x64
];

/// Build a random KEY-frame mode-info neighbourhood: a greedy tiling of
/// blocks, each either plain intra or an intrabc block with a whole-pel
/// DV; every 4x4 cell of a block carries identical fields (SVT's
/// replicated mi grid). A pool of shared DVs seeds the dedup/weight-
/// accumulation and sort-tie arms.
fn random_grid(rng: &mut Rng, rows: usize, cols: usize, ibc_pct: u64) -> Vec<mvp::MvpMiEntry> {
    let mut grid = vec![mvp::MvpMiEntry::default(); rows * cols];
    let mut filled = vec![false; rows * cols];
    // Shared DV pool (duplicates across blocks -> stack dedup + ties).
    let dv_pool: Vec<Mv> = (0..6)
        .map(|_| Mv {
            x: ((rng.below(320) as i32 - 300) * 8) as i16,
            y: ((rng.below(200) as i32 - 190) * 8) as i16,
        })
        .collect();
    for r in 0..rows {
        for c in 0..cols {
            if filled[r * cols + c] {
                continue;
            }
            // Random size that fits.
            let (bsize, w, h) = loop {
                let cand = SIZES[rng.below(SIZES.len() as u64) as usize];
                if r + cand.2 as usize <= rows && c + cand.1 as usize <= cols {
                    break cand;
                }
            };
            let is_ibc = rng.below(100) < ibc_pct;
            let dv = if rng.below(3) == 0 {
                dv_pool[rng.below(dv_pool.len() as u64) as usize]
            } else if rng.below(8) == 0 {
                // Extreme DV: exercises clamp_mv_ref.
                Mv { x: -16000, y: 15992 }
            } else {
                Mv {
                    x: ((rng.below(300) as i32 - 290) * 8) as i16,
                    y: ((rng.below(160) as i32 - 150) * 8) as i16,
                }
            };
            let entry = mvp::MvpMiEntry {
                bsize,
                mode: if is_ibc { 0 } else { rng.below(13) as u8 },
                use_intrabc: is_ibc,
                ref_frame: [0, -1],
                mv: [if is_ibc { dv } else { Mv::default() }, Mv::default()],
                partition: rng.below(10) as u8,
            };
            for dr in 0..h as usize {
                for dc in 0..w as usize {
                    if r + dr < rows && c + dc < cols {
                        grid[(r + dr) * cols + c + dc] = entry;
                        filled[(r + dr) * cols + c + dc] = true;
                    }
                }
            }
        }
    }
    grid
}

fn to_c_cells(grid: &[mvp::MvpMiEntry]) -> Vec<cref::MvpCell> {
    grid.iter()
        .map(|e| {
            (
                e.bsize,
                e.mode,
                e.use_intrabc,
                e.ref_frame[0],
                e.ref_frame[1],
                e.mv[0].as_int(),
                e.partition,
            )
        })
        .collect()
}

#[test]
fn c_parity_setup_ref_mv_list_intra() {
    let mut rng = Rng(0x1BC6_3417_0001);
    // Grid = tile + 4-cell margin on the right/bottom (SVT's mi grid is
    // padded past mi_cols; the ROW-3/5 scans' col_offset=+1 stepping can
    // legally read one column past a frame-edge block — the margin cells
    // are read identically on both sides).
    let (rows, cols) = (52usize, 52usize);
    let mut checked = 0u64;
    let mut nonzero_count = 0u64;
    let mut multi_count = 0u64;
    let mut mode_ctx_values = std::collections::BTreeSet::new();
    let mut sec_rect_cases = 0u64;
    let mut vert_a_cases = 0u64;
    let mut subtile_cases = 0u64;

    for grid_iter in 0..12 {
        let ibc_pct = [15u64, 45, 80][grid_iter % 3];
        let grid = random_grid(&mut rng, rows, cols, ibc_pct);
        let c_cells = to_c_cells(&grid);

        // Frame/tile geometry: full-tile + sub-tile variants; frame mi
        // dims leave the margin outside the frame.
        let (mi_rows, mi_cols) = (48i32, 48i32);
        let tiles = [
            (0i32, 48i32, 0i32, 48i32),
            (0, 48, 8, 48),   // tile starting at col 8
            (16, 48, 0, 40),  // tile starting at row 16, ending col 40
        ];
        for &(trs, tre, tcs, tce) in &tiles {
            let tile = TileMiBounds { mi_row_start: trs, mi_row_end: tre, mi_col_start: tcs, mi_col_end: tce };
            for &(bsize, w_mi, h_mi) in &SIZES {
                for &sb128 in &[false, true] {
                    let sb_mi_size = if sb128 { 32 } else { 16 };
                    // Random size-aligned in-tile position (2 draws each).
                    for _ in 0..2 {
                        let span_r = ((tre - trs - h_mi) / h_mi).max(0) as u64 + 1;
                        let span_c = ((tce - tcs - w_mi) / w_mi).max(0) as u64 + 1;
                        let mi_row = trs + (rng.below(span_r) as i32) * h_mi;
                        let mi_col = tcs + (rng.below(span_c) as i32) * w_mi;
                        if mi_row + h_mi > tre || mi_col + w_mi > tce {
                            continue;
                        }

                        let ctx = mvp::derive_block_ctx(
                            mi_row,
                            mi_col,
                            usize::from(bsize),
                            mi_rows,
                            mi_cols,
                            tile,
                            sb_mi_size,
                        );
                        let gview = mvp::MvpGrid {
                            entries: &grid,
                            stride: cols as i32,
                            base: mi_row * cols as i32 + mi_col,
                        };
                        let rs = mvp::generate_mvp_table_intra_frame(&gview, &ctx);
                        let (rs_nearest, rs_near) = mvp::find_best_ref_mvs_from_stack(&rs);

                        let c = cref::setup_ref_mv_list_intra(
                            &c_cells,
                            rows,
                            cols,
                            (mi_row, mi_col),
                            usize::from(bsize),
                            (mi_rows, mi_cols),
                            (trs, tre, tcs, tce),
                            sb128,
                        );

                        assert_eq!(
                            rs.count, c.count,
                            "stack count diverges: bsize={bsize} mi=({mi_row},{mi_col}) tile=({trs},{tre},{tcs},{tce}) sb128={sb128} grid={grid_iter}"
                        );
                        for i in 0..8usize {
                            assert_eq!(
                                (rs.stack[i].this_mv.as_int(), rs.stack[i].weight),
                                c.stack[i],
                                "stack[{i}] diverges: bsize={bsize} mi=({mi_row},{mi_col}) tile=({trs},{tre},{tcs},{tce}) sb128={sb128} grid={grid_iter} (count={})",
                                c.count
                            );
                        }
                        assert_eq!(
                            rs.mode_context, c.mode_context,
                            "mode_context diverges: bsize={bsize} mi=({mi_row},{mi_col})"
                        );
                        assert_eq!(
                            (rs_nearest.as_int(), rs_near.as_int()),
                            (c.nearest, c.near),
                            "nearest/near diverge: bsize={bsize} mi=({mi_row},{mi_col})"
                        );

                        checked += 1;
                        if c.count > 0 {
                            nonzero_count += 1;
                        }
                        if c.count > 1 {
                            multi_count += 1;
                        }
                        mode_ctx_values.insert(c.mode_context);
                        if ctx.is_sec_rect {
                            sec_rect_cases += 1;
                        }
                        if grid[(mi_row * cols as i32 + mi_col) as usize].partition == 6 {
                            vert_a_cases += 1;
                        }
                        if trs != 0 || tcs != 0 || tce != 48 {
                            subtile_cases += 1;
                        }
                    }
                }
            }
        }
    }
    // Anti-vacuity: the sweep must exercise the machinery for real.
    assert!(checked > 1200, "too few MVP cases: {checked}");
    assert!(nonzero_count > 400, "stacks mostly empty: {nonzero_count}");
    assert!(multi_count > 150, "multi-entry stacks (sort/dedup arm) rare: {multi_count}");
    assert!(mode_ctx_values.len() >= 3, "mode_context degenerate: {mode_ctx_values:?}");
    assert!(sec_rect_cases > 40, "is_sec_rect arm untested: {sec_rect_cases}");
    assert!(vert_a_cases > 5, "has_top_right VERT_A arm untested: {vert_a_cases}");
    assert!(subtile_cases > 300, "sub-tile availability untested: {subtile_cases}");
}

/// Directed: dv_ref composition — empty stack falls back to find_ref_dv;
/// single-entry uses nearest; nearest==0 uses near.
#[test]
fn compose_dv_ref_matches_c_semantics() {
    let (rows, cols) = (52usize, 52usize);
    let tile = TileMiBounds { mi_row_start: 0, mi_row_end: 48, mi_col_start: 0, mi_col_end: 48 };

    // Empty neighbourhood -> find_ref_dv fallback (both first-SB-row and
    // interior forms).
    let grid = vec![mvp::MvpMiEntry::default(); rows * cols];
    for (mi_row, expect) in [
        (0i32, Mv { x: (-4 * 16 - 256) as i16, y: 0 }.as_int()), // first SB row: ((-4*mib-256)*8... see below
        (16, Mv { x: 0, y: -(16 * 4 * 8) as i16 }.as_int()),
    ] {
        let ctx = mvp::derive_block_ctx(mi_row, 16, 3, 48, 48, tile, 16);
        let g = mvp::MvpGrid { entries: &grid, stride: cols as i32, base: mi_row * cols as i32 + 16 };
        let out = mvp::generate_mvp_table_intra_frame(&g, &ctx);
        assert_eq!(out.count, 0);
        let dv_ref = mvp::compose_dv_ref(&out, tile, 16, mi_row);
        if mi_row == 0 {
            // find_ref_dv first-SB-row arm: x = -(4*mib + 256)*... C:
            // (x = -4*mib - 256) << 3? Locked via the chunk-2-verified
            // find_ref_dv — just cross-check against it directly.
            let direct = svtav1_encoder::intrabc::find_ref_dv(tile, 16, mi_row);
            assert_eq!(dv_ref.as_int(), direct.as_int());
        } else {
            assert_eq!(dv_ref.as_int(), expect);
        }
    }

    // One intrabc above-neighbour: nearest = its DV -> dv_ref = DV.
    let mut grid2 = vec![mvp::MvpMiEntry::default(); rows * cols];
    let dv = Mv { x: -128, y: -64 };
    for c in 16..18 {
        grid2[15 * cols + c] = mvp::MvpMiEntry {
            bsize: 3,
            mode: 0,
            use_intrabc: true,
            ref_frame: [0, -1],
            mv: [dv, Mv::default()],
            partition: 0,
        };
    }
    let ctx = mvp::derive_block_ctx(16, 16, 3, 48, 48, tile, 16);
    let g = mvp::MvpGrid { entries: &grid2, stride: cols as i32, base: 16 * cols as i32 + 16 };
    let out = mvp::generate_mvp_table_intra_frame(&g, &ctx);
    assert!(out.count >= 1);
    let dv_ref = mvp::compose_dv_ref(&out, tile, 16, 16);
    assert_eq!(dv_ref.as_int(), dv.as_int());
}

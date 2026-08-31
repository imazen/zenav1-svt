//! Differential parity: the INTER MVP stack
//! (`svtav1-encoder/src/inter_mvp.rs`) vs the REAL exported C functions
//! (inter campaign chunk C2, `docs/INTER-ENCODE-PLAN.md` §2).
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4): the oracle is the
//! EXPORTED `setup_ref_mv_list` (adaptive_mv_pred.c:651) driven for a
//! GENERAL `MvReferenceFrame` — single refs 1..7 and compound types
//! 8..28 — over randomized inter mode-info grids, with the temporal-MVP
//! (MFMV) block LIVE (`use_ref_frame_mvs = 1`, a caller-supplied
//! `pcs->tpl_mvs`, real order hints on synthetic reference objects), plus
//! `svt_aom_gm_get_motion_vector_enc`, `svt_aom_compute_inter_mode_ctx_light`,
//! `svt_aom_get_av1_mv_pred_drl` and `svt_av1_find_best_ref_mvs_from_stack`.
//!
//! Compared: the FULL raw 8-slot stack (`this_mv`, `comp_mv`, weight —
//! including the beyond-count gm-fill), the count, the mode context (all
//! three of its fields: NEWMV, GLOBALMV and REFMV), the nearest/near
//! from-stack reads, and C's `mv_ref0[64]` scratch (the symmetric-refs
//! channel).

use svtav1_cref::inter_mvp as cinter;
use svtav1_encoder::inter_mvp as rmvp;
use svtav1_encoder::intrabc::TileMiBounds;
use svtav1_encoder::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
use svtav1_types::motion::{Mv, TransformationType, WarpedMotionParams};

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
    (0, 1, 1),    // 4x4
    (1, 1, 2),    // 4x8
    (2, 2, 1),    // 8x4
    (3, 2, 2),    // 8x8
    (4, 2, 4),    // 8x16
    (5, 4, 2),    // 16x8
    (6, 4, 4),    // 16x16
    (7, 4, 8),    // 16x32
    (8, 8, 4),    // 32x16
    (9, 8, 8),    // 32x32
    (12, 16, 16), // 64x64
];

/// Single-ref inter modes + the compound modes, so `have_newmv_in_inter_mode`
/// and `is_global_mv_block` both fire.
const INTER_MODES: [u8; 12] = [13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24];

const GRID_ROWS: usize = 52;
const GRID_COLS: usize = 52;
const MI_ROWS: i32 = 48;
const MI_COLS: i32 = 48;
/// `cm->mi_stride`, which is what `add_tpl_ref_mv` halves to index `tpl_mvs`.
const MI_STRIDE_FULL: i32 = GRID_COLS as i32;
const TPL_STRIDE: i32 = MI_STRIDE_FULL / 2;
/// C sizes `pcs->tpl_mvs` `((mi_rows + MAX_MIB_SIZE) >> 1) * (mi_stride >> 1)`.
const TPL_CELLS: usize = (((MI_ROWS + 32) >> 1) * TPL_STRIDE) as usize;

/// Build a random INTER mode-info neighbourhood: a greedy tiling where
/// each block is intra, intrabc, single-ref inter or compound inter, with
/// a shared MV pool so the stack's dedup / weight-accumulation / sort-tie
/// arms all fire.
fn random_grid(rng: &mut Rng, intra_pct: u64, compound_pct: u64) -> Vec<MvpMiEntry> {
    let rows = GRID_ROWS;
    let cols = GRID_COLS;
    let mut grid = vec![MvpMiEntry::default(); rows * cols];
    let mut filled = vec![false; rows * cols];
    let mv_pool: Vec<Mv> = (0..6)
        .map(|_| Mv {
            x: ((rng.below(400) as i32) - 200) as i16,
            y: ((rng.below(400) as i32) - 200) as i16,
        })
        .collect();
    for r in 0..rows {
        for c in 0..cols {
            if filled[r * cols + c] {
                continue;
            }
            let (bsize, w, h) = loop {
                let cand = SIZES[rng.below(SIZES.len() as u64) as usize];
                if r + cand.2 as usize <= rows && c + cand.1 as usize <= cols {
                    break cand;
                }
            };
            let roll = rng.below(100);
            let pick_mv = |rng: &mut Rng| -> Mv {
                if rng.below(3) == 0 {
                    mv_pool[rng.below(mv_pool.len() as u64) as usize]
                } else if rng.below(12) == 0 {
                    // Extreme MV: exercises clamp_mv_ref.
                    Mv {
                        x: -16000,
                        y: 15992,
                    }
                } else {
                    Mv {
                        x: ((rng.below(600) as i32) - 300) as i16,
                        y: ((rng.below(600) as i32) - 300) as i16,
                    }
                }
            };
            let entry = if roll < intra_pct {
                // Plain intra, or an intrabc block (is_inter_block true,
                // ref_frame[0] == INTRA_FRAME -> never matches an inter rf).
                let ibc = rng.below(4) == 0;
                MvpMiEntry {
                    bsize,
                    mode: if ibc { 0 } else { rng.below(13) as u8 },
                    use_intrabc: ibc,
                    ref_frame: [0, -1],
                    mv: [
                        if ibc { pick_mv(rng) } else { Mv::default() },
                        Mv::default(),
                    ],
                    partition: rng.below(10) as u8,
                }
            } else if roll < intra_pct + compound_pct {
                // Compound: pick a pair straight out of C's ref_frame_map
                // so the compound arm of add_ref_mv_candidate can match.
                let ct = 8 + rng.below(21) as i8;
                let rf = rmvp::av1_set_ref_frame(ct);
                MvpMiEntry {
                    bsize,
                    mode: INTER_MODES[rng.below(INTER_MODES.len() as u64) as usize],
                    use_intrabc: false,
                    ref_frame: rf,
                    mv: [pick_mv(rng), pick_mv(rng)],
                    partition: rng.below(10) as u8,
                }
            } else {
                MvpMiEntry {
                    bsize,
                    mode: INTER_MODES[rng.below(INTER_MODES.len() as u64) as usize],
                    use_intrabc: false,
                    ref_frame: [1 + rng.below(7) as i8, -1],
                    mv: [pick_mv(rng), pick_mv(rng)],
                    partition: rng.below(10) as u8,
                }
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

fn to_c_cells(grid: &[MvpMiEntry]) -> Vec<cinter::InterMvpCell> {
    grid.iter()
        .map(|e| {
            (
                e.bsize,
                e.mode,
                e.use_intrabc,
                e.ref_frame[0],
                e.ref_frame[1],
                e.mv[0].as_int(),
                e.mv[1].as_int(),
                e.partition,
            )
        })
        .collect()
}

/// A random temporal MV field: a mix of INVALID cells (the common case)
/// and live projections with assorted `ref_frame_offset`s (including 0,
/// which drives `div_mult[0] == 0`).
fn random_tpl(rng: &mut Rng, live_pct: u64) -> Vec<rmvp::TplMvRef> {
    (0..TPL_CELLS)
        .map(|_| {
            if rng.below(100) < live_pct {
                rmvp::TplMvRef {
                    mfmv0: Mv {
                        x: ((rng.below(2000) as i32) - 1000) as i16,
                        y: ((rng.below(2000) as i32) - 1000) as i16,
                    },
                    ref_frame_offset: rng.below(34) as u8,
                }
            } else {
                rmvp::TplMvRef::default()
            }
        })
        .collect()
}

fn to_c_tpl(tpl: &[rmvp::TplMvRef]) -> Vec<cinter::TplCell> {
    tpl.iter()
        .map(|t| (t.mfmv0.as_int(), t.ref_frame_offset))
        .collect()
}

/// Global-motion models: identity, translation, rotzoom, affine.
fn gm_model(kind: u64, rng: &mut Rng) -> WarpedMotionParams {
    let mut gm = WarpedMotionParams::default();
    match kind {
        0 => gm.wm_type = TransformationType::Identity,
        1 => {
            gm.wm_type = TransformationType::Translation;
            gm.wmmat[0] = ((rng.below(1 << 17) as i32) - (1 << 16)) & !0x7;
            gm.wmmat[1] = ((rng.below(1 << 17) as i32) - (1 << 16)) & !0x7;
        }
        2 => {
            gm.wm_type = TransformationType::RotZoom;
            gm.wmmat[0] = (rng.below(1 << 17) as i32) - (1 << 16);
            gm.wmmat[1] = (rng.below(1 << 17) as i32) - (1 << 16);
            gm.wmmat[2] = (1 << 16) + (rng.below(2048) as i32) - 1024;
            gm.wmmat[3] = (rng.below(2048) as i32) - 1024;
            gm.wmmat[4] = -gm.wmmat[3];
            gm.wmmat[5] = gm.wmmat[2];
        }
        _ => {
            gm.wm_type = TransformationType::Affine;
            for j in 0..6 {
                gm.wmmat[j] = (rng.below(1 << 17) as i32) - (1 << 16);
            }
            gm.wmmat[2] += 1 << 16;
            gm.wmmat[5] += 1 << 16;
        }
    }
    gm
}

fn env_to_c(env: &rmvp::InterMvpEnv) -> cinter::InterMvpEnvC {
    let mut gm_wmtype = [0i32; 8];
    let mut gm_wmmat = [[0i32; 6]; 8];
    for i in 0..8 {
        gm_wmtype[i] = env.global_motion[i].wm_type as i32;
        gm_wmmat[i] = env.global_motion[i].wmmat;
    }
    let mut sign_bias = [0i32; 8];
    for i in 0..8 {
        sign_bias[i] = env.ref_frame_sign_bias[i] as i32;
    }
    cinter::InterMvpEnvC {
        gm_wmtype,
        gm_wmmat,
        ref_frame_sign_bias: sign_bias,
        allow_high_precision_mv: env.allow_high_precision_mv,
        force_integer_mv: env.force_integer_mv,
        use_ref_frame_mvs: env.use_ref_frame_mvs,
        enable_order_hint: env.order_hint_info.enable_order_hint,
        order_hint_bits: env.order_hint_info.order_hint_bits as i32,
        cur_order_hint: env.cur_order_hint,
        ref_order_hint: env.ref_order_hint,
        mi_stride_full: MI_STRIDE_FULL,
        sb64_sq_no4xn_geom: env.sb64_sq_no4xn_geom,
        symmetric_refs: env.symmetric_refs,
    }
}

#[test]
fn c_parity_setup_ref_mv_list_inter() {
    let mut rng = Rng(0x1_2ED0_C2_0007);
    let mut checked = 0u64;
    let mut nonzero_count = 0u64;
    let mut full_stacks = 0u64;
    let mut compound_cases = 0u64;
    let mut mfmv_cases = 0u64;
    let mut mfmv_contributed = 0u64;
    let mut globalmv_bit_cases = 0u64;
    let mut mode_ctx_values = std::collections::BTreeSet::new();
    let mut nonzero_comp_mv = 0u64;

    // Ref-frame types to sweep: every single ref, plus a spread of
    // compound types over both the bidir block (8..19) and the unidir
    // block (20..28).
    let ref_types: [i8; 14] = [1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 16, 19, 20, 28];

    for grid_iter in 0..6 {
        let intra_pct = [10u64, 30, 55][grid_iter % 3];
        let compound_pct = [45u64, 25, 15][grid_iter % 3];
        let grid = random_grid(&mut rng, intra_pct, compound_pct);
        let c_cells = to_c_cells(&grid);
        let tpl = random_tpl(&mut rng, [60u64, 25, 90][grid_iter % 3]);
        let c_tpl = to_c_tpl(&tpl);

        // Frame-level environment for this grid.
        let mut global_motion = [WarpedMotionParams::default(); 8];
        for (i, slot) in global_motion.iter_mut().enumerate() {
            // ref 0 (INTRA_FRAME) stays IDENTITY, as SVT leaves it.
            *slot = if i == 0 {
                WarpedMotionParams::default()
            } else {
                gm_model(rng.below(4), &mut rng)
            };
        }
        let mut ref_frame_sign_bias = [0u32; 8];
        for slot in ref_frame_sign_bias.iter_mut().skip(1) {
            *slot = rng.below(2) as u32;
        }
        let mut ref_order_hint = [0i32; 8];
        for slot in ref_order_hint.iter_mut().skip(1) {
            *slot = rng.below(32) as i32;
        }

        for &use_mfmv in &[false, true] {
            for &allow_hp in &[false, true] {
                for &(sb64_geom, symmetric) in &[(false, false), (true, false), (false, true)] {
                    let env = rmvp::InterMvpEnv {
                        global_motion: &global_motion,
                        ref_frame_sign_bias,
                        allow_high_precision_mv: allow_hp,
                        force_integer_mv: false,
                        use_ref_frame_mvs: use_mfmv,
                        order_hint_info: rmvp::OrderHintInfo {
                            enable_order_hint: true,
                            order_hint_bits: 5,
                        },
                        cur_order_hint: 17,
                        ref_order_hint,
                        tpl_mvs: &tpl,
                        tpl_stride: TPL_STRIDE,
                        sb64_sq_no4xn_geom: sb64_geom,
                        symmetric_refs: symmetric,
                    };
                    let c_env = env_to_c(&env);

                    let tiles = [(0i32, 48i32, 0i32, 48i32), (16, 48, 8, 40)];
                    for &(trs, tre, tcs, tce) in &tiles {
                        let tile = TileMiBounds {
                            mi_row_start: trs,
                            mi_row_end: tre,
                            mi_col_start: tcs,
                            mi_col_end: tce,
                        };
                        for &(bsize, w_mi, h_mi) in &SIZES {
                            let sb128 = rng.below(2) == 0;
                            let sb_mi_size = if sb128 { 32 } else { 16 };
                            let span_r = ((tre - trs - h_mi) / h_mi).max(0) as u64 + 1;
                            let span_c = ((tce - tcs - w_mi) / w_mi).max(0) as u64 + 1;
                            let mi_row = trs + (rng.below(span_r) as i32) * h_mi;
                            let mi_col = tcs + (rng.below(span_c) as i32) * w_mi;
                            if mi_row + h_mi > tre || mi_col + w_mi > tce {
                                continue;
                            }
                            let ctx = derive_block_ctx(
                                mi_row,
                                mi_col,
                                usize::from(bsize),
                                MI_ROWS,
                                MI_COLS,
                                tile,
                                sb_mi_size,
                            );
                            let gview = MvpGrid {
                                entries: &grid,
                                stride: GRID_COLS as i32,
                                base: mi_row * GRID_COLS as i32 + mi_col,
                            };

                            for &ref_frame in &ref_types {
                                let gm_mv = rmvp::gm_mv_candidates_for(
                                    &env,
                                    ref_frame,
                                    usize::from(bsize),
                                    mi_col,
                                    mi_row,
                                );
                                let rs =
                                    rmvp::setup_ref_mv_list(&gview, &ctx, &env, ref_frame, gm_mv);
                                let (rs_nearest, rs_near) = rmvp::find_best_ref_mvs_from_stack(
                                    &rs, ref_frame, allow_hp, false,
                                );

                                let c = cinter::setup_ref_mv_list_inter(
                                    &c_cells,
                                    GRID_ROWS,
                                    GRID_COLS,
                                    (mi_row, mi_col),
                                    usize::from(bsize),
                                    (MI_ROWS, MI_COLS),
                                    (trs, tre, tcs, tce),
                                    sb128,
                                    ref_frame,
                                    &c_tpl,
                                    &c_env,
                                );

                                let where_ = format!(
                                    "rf={ref_frame} bsize={bsize} mi=({mi_row},{mi_col}) \
                                     tile=({trs},{tre},{tcs},{tce}) sb128={sb128} mfmv={use_mfmv} \
                                     hp={allow_hp} sb64geom={sb64_geom} sym={symmetric} \
                                     grid={grid_iter}"
                                );
                                assert_eq!(rs.count, c.count, "stack count diverges: {where_}");
                                for i in 0..8usize {
                                    assert_eq!(
                                        (
                                            rs.stack[i].this_mv.as_int(),
                                            rs.stack[i].comp_mv.as_int(),
                                            rs.stack[i].weight
                                        ),
                                        c.stack[i],
                                        "stack[{i}] diverges: {where_} (count={})",
                                        c.count
                                    );
                                }
                                assert_eq!(
                                    rs.mode_context, c.mode_context,
                                    "mode_context diverges: {where_}"
                                );
                                assert_eq!(
                                    (rs_nearest.as_int(), rs_near.as_int()),
                                    (c.nearest, c.near),
                                    "nearest/near diverge: {where_}"
                                );
                                for i in 0..64usize {
                                    assert_eq!(
                                        rs.mv_ref0[i].as_int(),
                                        c.mv_ref0[i],
                                        "mv_ref0[{i}] diverges: {where_}"
                                    );
                                }

                                checked += 1;
                                if c.count > 0 {
                                    nonzero_count += 1;
                                }
                                if c.count as usize >= 8 {
                                    full_stacks += 1;
                                }
                                if ref_frame >= 8 {
                                    compound_cases += 1;
                                    if c.stack[0].1 != 0 {
                                        nonzero_comp_mv += 1;
                                    }
                                }
                                if use_mfmv {
                                    mfmv_cases += 1;
                                    // Load-bearing check: re-run the SAME C
                                    // oracle with the MFMV block switched off
                                    // and count the cases where the temporal
                                    // candidates actually changed the answer.
                                    // "The code ran" is not evidence; "the
                                    // output moved" is.
                                    let mut off = c_env.clone();
                                    off.use_ref_frame_mvs = false;
                                    let c_off = cinter::setup_ref_mv_list_inter(
                                        &c_cells,
                                        GRID_ROWS,
                                        GRID_COLS,
                                        (mi_row, mi_col),
                                        usize::from(bsize),
                                        (MI_ROWS, MI_COLS),
                                        (trs, tre, tcs, tce),
                                        sb128,
                                        ref_frame,
                                        &c_tpl,
                                        &off,
                                    );
                                    if c_off.count != c.count
                                        || c_off.stack != c.stack
                                        || c_off.mode_context != c.mode_context
                                    {
                                        mfmv_contributed += 1;
                                    }
                                }
                                if c.mode_context & (1 << 3) != 0 {
                                    globalmv_bit_cases += 1;
                                }
                                mode_ctx_values.insert(c.mode_context);
                            }
                        }
                    }
                }
            }
        }
    }

    // Anti-vacuity: the sweep must actually reach every arm it claims to.
    assert!(checked > 4000, "too few inter MVP cases: {checked}");
    assert!(nonzero_count > 2000, "stacks mostly empty: {nonzero_count}");
    assert!(
        full_stacks > 50,
        "stack never saturated (MAX_REF_MV_STACK_SIZE arm untested): {full_stacks}"
    );
    assert!(
        compound_cases > 1000,
        "compound ref types barely swept: {compound_cases}"
    );
    assert!(
        nonzero_comp_mv > 100,
        "compound arm never produced a non-zero comp_mv: {nonzero_comp_mv}"
    );
    assert!(mfmv_cases > 1500, "MFMV barely swept: {mfmv_cases}");
    assert!(
        mfmv_contributed > 300,
        "the temporal-MVP walk never CHANGED the stack — add_tpl_ref_mv is untested \
         even though it ran: {mfmv_contributed}"
    );
    assert!(
        globalmv_bit_cases > 100,
        "the GLOBALMV mode-context bit never set: {globalmv_bit_cases}"
    );
    assert!(
        mode_ctx_values.len() >= 8,
        "mode_context degenerate: {mode_ctx_values:?}"
    );
}

#[test]
fn c_parity_gm_get_motion_vector_enc() {
    let mut rng = Rng(0xB0B0_0C2_0011);
    let mut checked = 0u64;
    let mut nonzero = 0u64;
    for kind in 0..4u64 {
        for _ in 0..400 {
            let gm = gm_model(kind, &mut rng);
            let bsize = SIZES[rng.below(SIZES.len() as u64) as usize].0;
            let mi_col = rng.below(200) as i32;
            let mi_row = rng.below(200) as i32;
            for &allow_hp in &[false, true] {
                for &is_int in &[false, true] {
                    let rs = rmvp::gm_get_motion_vector_enc(
                        &gm,
                        allow_hp,
                        usize::from(bsize),
                        mi_col,
                        mi_row,
                        is_int,
                    );
                    let c = cinter::gm_get_motion_vector_enc(
                        gm.wm_type as i32,
                        &gm.wmmat,
                        allow_hp,
                        usize::from(bsize),
                        mi_col,
                        mi_row,
                        is_int,
                    );
                    assert_eq!(
                        rs.as_int(),
                        c,
                        "gm_get_motion_vector_enc diverges: kind={kind} bsize={bsize} \
                         mi=({mi_row},{mi_col}) hp={allow_hp} int={is_int} wmmat={:?}",
                        gm.wmmat
                    );
                    checked += 1;
                    if c != 0 {
                        nonzero += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 5000, "too few gm cases: {checked}");
    assert!(nonzero > 2000, "gm MVs mostly zero: {nonzero}");
}

#[test]
fn c_parity_compute_inter_mode_ctx_light() {
    let mut rng = Rng(0x11FE_0C2_0023);
    let mut checked = 0u64;
    let mut values = std::collections::BTreeSet::new();
    // LPD1 assumes block size >= 8x8, so drop 4x4 / 4x8 / 8x4.
    let sizes: Vec<(u8, i32, i32)> = SIZES
        .iter()
        .copied()
        .filter(|s| s.1 >= 2 && s.2 >= 2)
        .collect();
    for grid_iter in 0..4 {
        let grid = random_grid(&mut rng, [15u64, 40, 60][grid_iter % 3], 30);
        let c_cells = to_c_cells(&grid);
        for &(trs, tre, tcs, tce) in &[(0i32, 48i32, 0i32, 48i32), (16, 48, 8, 40)] {
            let tile = TileMiBounds {
                mi_row_start: trs,
                mi_row_end: tre,
                mi_col_start: tcs,
                mi_col_end: tce,
            };
            for &(bsize, w_mi, h_mi) in &sizes {
                for &sb128 in &[false, true] {
                    let sb_mi_size = if sb128 { 32 } else { 16 };
                    for _ in 0..3 {
                        let span_r = ((tre - trs - h_mi) / h_mi).max(0) as u64 + 1;
                        let span_c = ((tce - tcs - w_mi) / w_mi).max(0) as u64 + 1;
                        let mi_row = trs + (rng.below(span_r) as i32) * h_mi;
                        let mi_col = tcs + (rng.below(span_c) as i32) * w_mi;
                        if mi_row + h_mi > tre || mi_col + w_mi > tce {
                            continue;
                        }
                        let ctx = derive_block_ctx(
                            mi_row,
                            mi_col,
                            usize::from(bsize),
                            MI_ROWS,
                            MI_COLS,
                            tile,
                            sb_mi_size,
                        );
                        let gview = MvpGrid {
                            entries: &grid,
                            stride: GRID_COLS as i32,
                            base: mi_row * GRID_COLS as i32 + mi_col,
                        };
                        for ref_frame in [1i8, 3, 5, 7, 8, 16, 28] {
                            let rs = rmvp::compute_inter_mode_ctx_light(&gview, &ctx, ref_frame);
                            let c = cinter::compute_inter_mode_ctx_light(
                                &c_cells,
                                GRID_ROWS,
                                GRID_COLS,
                                (mi_row, mi_col),
                                usize::from(bsize),
                                (MI_ROWS, MI_COLS),
                                (trs, tre, tcs, tce),
                                sb128,
                                ref_frame,
                            );
                            assert_eq!(
                                rs, c,
                                "inter_mode_ctx_light diverges: rf={ref_frame} bsize={bsize} \
                                 mi=({mi_row},{mi_col}) tile=({trs},{tre},{tcs},{tce}) \
                                 sb128={sb128} grid={grid_iter}"
                            );
                            checked += 1;
                            values.insert(c);
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 1000, "too few light-ctx cases: {checked}");
    assert!(values.len() >= 6, "light mode_ctx degenerate: {values:?}");
}

#[test]
fn c_parity_get_av1_mv_pred_drl() {
    let mut rng = Rng(0xD12_0C2_0037);
    let mut checked = 0u64;
    let mut compound_cases = 0u64;
    for _ in 0..3000 {
        // Random stack with a random count.
        let count = rng.below(9) as u8;
        let mut stack = [svtav1_types::motion::CandidateMv::default(); 8];
        let mut c_stack = [(0u32, 0u32, 0i32); 8];
        for i in 0..8usize {
            let this = Mv {
                x: ((rng.below(2000) as i32) - 1000) as i16,
                y: ((rng.below(2000) as i32) - 1000) as i16,
            };
            let comp = Mv {
                x: ((rng.below(2000) as i32) - 1000) as i16,
                y: ((rng.below(2000) as i32) - 1000) as i16,
            };
            let w = rng.below(2000) as i32;
            stack[i] = svtav1_types::motion::CandidateMv {
                this_mv: this,
                comp_mv: comp,
                weight: w,
            };
            c_stack[i] = (this.as_int(), comp.as_int(), w);
        }
        let rs_stack = rmvp::InterMvpStack {
            stack,
            count,
            mode_context: 0,
            mv_ref0: [Mv::default(); 64],
        };

        let is_compound = rng.below(2) == 0;
        // C asserts is_compound modes are the compound family; keep the
        // mode set consistent with is_compound so the LUT lookups are
        // meaningful (the LUTs return MB_MODE_COUNT for single modes).
        let mode = if is_compound {
            17 + rng.below(8) as u8
        } else {
            13 + rng.below(4) as u8
        };
        // C indexes ref_mv_stack[ref_mv_idx] with ref_mv_idx up to
        // 1 + drl_index (compound) — keep it in range as C's callers do.
        let drl_index = rng.below(3) as u8;
        let ref_frame: i8 = if is_compound {
            8 + rng.below(21) as i8
        } else {
            1 + rng.below(7) as i8
        };

        let initial = rmvp::DrlMvPred {
            nearestmv: [Mv { x: 11, y: -22 }, Mv { x: -33, y: 44 }],
            nearmv: [Mv { x: 55, y: -66 }, Mv { x: -77, y: 88 }],
            ref_mv: [Mv { x: 99, y: -111 }, Mv { x: -122, y: 133 }],
        };
        let rs = rmvp::get_av1_mv_pred_drl(
            &rs_stack,
            is_compound,
            mode,
            usize::from(drl_index),
            initial,
        );

        let mut io = [
            initial.nearestmv[0].as_int(),
            initial.nearestmv[1].as_int(),
            initial.nearmv[0].as_int(),
            initial.nearmv[1].as_int(),
            initial.ref_mv[0].as_int(),
            initial.ref_mv[1].as_int(),
        ];
        cinter::get_av1_mv_pred_drl(
            &c_stack,
            count,
            ref_frame,
            is_compound,
            mode,
            drl_index,
            &mut io,
        );

        let where_ = format!(
            "is_compound={is_compound} mode={mode} drl={drl_index} count={count} rf={ref_frame}"
        );
        assert_eq!(rs.nearestmv[0].as_int(), io[0], "nearestmv[0]: {where_}");
        assert_eq!(rs.nearestmv[1].as_int(), io[1], "nearestmv[1]: {where_}");
        assert_eq!(rs.nearmv[0].as_int(), io[2], "nearmv[0]: {where_}");
        assert_eq!(rs.nearmv[1].as_int(), io[3], "nearmv[1]: {where_}");
        assert_eq!(rs.ref_mv[0].as_int(), io[4], "ref_mv[0]: {where_}");
        assert_eq!(rs.ref_mv[1].as_int(), io[5], "ref_mv[1]: {where_}");
        checked += 1;
        if is_compound {
            compound_cases += 1;
        }
    }
    assert!(checked > 2500, "too few drl cases: {checked}");
    assert!(
        compound_cases > 1000,
        "drl compound arm rare: {compound_cases}"
    );
}

/// `av1_set_ref_frame` / `av1_ref_frame_type` round-trip over every
/// signalable type — the pair expansion the whole inter branch keys on.
#[test]
fn ref_frame_type_roundtrip() {
    for t in 0i8..29 {
        let rf = rmvp::av1_set_ref_frame(t);
        assert_eq!(
            rmvp::av1_ref_frame_type(rf),
            t,
            "av1_ref_frame_type(av1_set_ref_frame({t})) != {t} (rf={rf:?})"
        );
    }
}

/// Directed regression, **tier 1**: C's `has_top_right` mutates `bs` in
/// its 4x4-group loop and the `PARTITION_VERT_A` check at the end reads
/// the MUTATED value (adaptive_mv_pred.c:303-322).
///
/// This cell FAILED before the fix and passes after — the criterion in
/// `docs/WORKING-ON-THIS.md` §3. Observed failure: an 8x8 block at
/// `mi = (36, 10)` in a uniform 64x64-neighbour field, `ref_frame = 2`,
/// `partition = PARTITION_VERT_A`, 64x64 SB — port `ref_mv_stack[0].weight
/// = 672`, C `= 668` (the port kept a top-right candidate C drops). Only
/// `partition == 6` diverges; all nine other partition types agreed, which
/// is what localizes it to that branch.
#[test]
fn c_parity_has_top_right_vert_a_uses_mutated_bs() {
    let base = MvpMiEntry {
        bsize: 12, // BLOCK_64X64
        mode: 17,  // NEAREST_NEARESTMV
        use_intrabc: false,
        ref_frame: [2, 5],
        mv: [Mv { x: -84, y: 122 }, Mv { x: 260, y: 164 }],
        partition: 0,
    };
    let gm = [WarpedMotionParams::default(); 8];
    let tpl = vec![rmvp::TplMvRef::default(); TPL_CELLS];
    let c_tpl = to_c_tpl(&tpl);
    let tile = TileMiBounds {
        mi_row_start: 0,
        mi_row_end: MI_ROWS,
        mi_col_start: 0,
        mi_col_end: MI_COLS,
    };
    let env = rmvp::InterMvpEnv {
        global_motion: &gm,
        ref_frame_sign_bias: [0; 8],
        allow_high_precision_mv: false,
        force_integer_mv: false,
        use_ref_frame_mvs: false,
        order_hint_info: rmvp::OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: 5,
        },
        cur_order_hint: 17,
        ref_order_hint: [0; 8],
        tpl_mvs: &tpl,
        tpl_stride: TPL_STRIDE,
        sb64_sq_no4xn_geom: false,
        symmetric_refs: false,
    };
    let c_env = env_to_c(&env);

    let mut vert_a_differed_from_none = false;
    let mut weights = Vec::new();
    for part in 0u8..10 {
        let mut e = base;
        e.partition = part;
        let grid: Vec<MvpMiEntry> = vec![e; GRID_ROWS * GRID_COLS];
        let c_cells = to_c_cells(&grid);
        let ctx = derive_block_ctx(36, 10, 3, MI_ROWS, MI_COLS, tile, 16);
        let gview = MvpGrid {
            entries: &grid,
            stride: GRID_COLS as i32,
            base: 36 * GRID_COLS as i32 + 10,
        };
        let rs = rmvp::setup_ref_mv_list(&gview, &ctx, &env, 2, [Mv::default(); 2]);
        let c = cinter::setup_ref_mv_list_inter(
            &c_cells,
            GRID_ROWS,
            GRID_COLS,
            (36, 10),
            3,
            (MI_ROWS, MI_COLS),
            (0, MI_ROWS, 0, MI_COLS),
            false,
            2,
            &c_tpl,
            &c_env,
        );
        assert_eq!(rs.count, c.count, "count diverges at partition={part}");
        for i in 0..8usize {
            assert_eq!(
                (
                    rs.stack[i].this_mv.as_int(),
                    rs.stack[i].comp_mv.as_int(),
                    rs.stack[i].weight
                ),
                c.stack[i],
                "stack[{i}] diverges at partition={part}"
            );
        }
        assert_eq!(
            rs.mode_context, c.mode_context,
            "mode_context diverges at partition={part}"
        );
        weights.push(c.stack[0].2);
        if part == 6 {
            vert_a_differed_from_none = c.stack[0].2 != weights[0];
        }
    }
    // Anti-vacuity: this geometry MUST be one where the VERT_A branch
    // fires, otherwise the cell cannot witness the regression it exists
    // for.
    assert!(
        vert_a_differed_from_none,
        "geometry no longer reaches the has_top_right VERT_A branch \
         (weights per partition: {weights:?})"
    );
}

/// `svt_aom_mode_context_analyzer` (inter_prediction.c:2565, EXPORTED) —
/// the compound collapse of the packed mode context. Swept over every
/// context `setup_ref_mv_list` can produce (its NEWMV field is 0..5 and
/// its REFMV field 0..5) crossed with single and compound `rf` pairs.
#[test]
fn c_parity_mode_context_analyzer() {
    let mut checked = 0u64;
    let mut distinct = std::collections::BTreeSet::new();
    let mut collapsed = 0u64;
    for newmv in 0i16..6 {
        for refmv in 0i16..6 {
            for globalmv in 0i16..2 {
                let mode_context = newmv | (globalmv << 3) | (refmv << 4);
                for &rf in &[
                    [1i8, -1],
                    [7, -1],
                    [1, 5], // LAST + BWDREF (a real compound pair)
                    [4, 7], // GOLDEN + ALTREF
                    [1, 2], // LAST + LAST2 (unidir)
                    [6, 7], // ALTREF2 + ALTREF (unidir)
                ] {
                    let rs = rmvp::mode_context_analyzer(mode_context, rf);
                    let c = cinter::mode_context_analyzer(mode_context, rf);
                    assert_eq!(
                        rs, c,
                        "mode_context_analyzer diverges: ctx={mode_context} rf={rf:?}"
                    );
                    checked += 1;
                    distinct.insert(c);
                    if rf[1] > 0 {
                        collapsed += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 400, "too few analyzer cases: {checked}");
    assert!(collapsed > 250, "compound arm barely swept: {collapsed}");
    assert!(
        distinct.len() >= 8,
        "analyzer output degenerate: {distinct:?}"
    );
}

/// `svt_av1_count_overlappable_neighbors` (adaptive_mv_pred.c:1893,
/// EXPORTED) — the OBMC neighbour count, including the `mi_step == 1`
/// chroma-pair rewind that mutates the loop variable.
#[test]
fn c_parity_count_overlappable_neighbors() {
    let mut rng = Rng(0x0BC_0C2_0041);
    let mut checked = 0u64;
    let mut nonzero = 0u64;
    let mut zeroed_by_bsize = 0u64;
    let mut counts = std::collections::BTreeSet::new();
    for grid_iter in 0..4 {
        // A high 4xN population is what drives the mi_step == 1 rewind.
        let grid = random_grid(&mut rng, [20u64, 45, 65][grid_iter % 3], 30);
        let c_cells = to_c_cells(&grid);
        for &(trs, tre, tcs, tce) in &[(0i32, 48i32, 0i32, 48i32), (16, 48, 8, 40)] {
            let tile = TileMiBounds {
                mi_row_start: trs,
                mi_row_end: tre,
                mi_col_start: tcs,
                mi_col_end: tce,
            };
            for &(bsize, w_mi, h_mi) in &SIZES {
                for _ in 0..4 {
                    let span_r = ((tre - trs - h_mi) / h_mi).max(0) as u64 + 1;
                    let span_c = ((tce - tcs - w_mi) / w_mi).max(0) as u64 + 1;
                    let mi_row = trs + (rng.below(span_r) as i32) * h_mi;
                    let mi_col = tcs + (rng.below(span_c) as i32) * w_mi;
                    if mi_row + h_mi > tre || mi_col + w_mi > tce {
                        continue;
                    }
                    let ctx = derive_block_ctx(
                        mi_row,
                        mi_col,
                        usize::from(bsize),
                        MI_ROWS,
                        MI_COLS,
                        tile,
                        16,
                    );
                    let gview = MvpGrid {
                        entries: &grid,
                        stride: GRID_COLS as i32,
                        base: mi_row * GRID_COLS as i32 + mi_col,
                    };
                    let rs = rmvp::count_overlappable_neighbors(&gview, &ctx, usize::from(bsize));
                    let c = cinter::count_overlappable_neighbors(
                        &c_cells,
                        GRID_ROWS,
                        GRID_COLS,
                        (mi_row, mi_col),
                        usize::from(bsize),
                        (MI_ROWS, MI_COLS),
                        (trs, tre, tcs, tce),
                    );
                    assert_eq!(
                        rs, c,
                        "overlappable_neighbors diverges: bsize={bsize} \
                         mi=({mi_row},{mi_col}) tile=({trs},{tre},{tcs},{tce}) grid={grid_iter}"
                    );
                    checked += 1;
                    counts.insert(c);
                    if c > 0 {
                        nonzero += 1;
                    }
                    // 4xN / Nx4 are below the motion-variation threshold and
                    // must count ZERO regardless of the neighbourhood.
                    if bsize <= 2 {
                        assert_eq!(c, 0, "a sub-8px block must not be overlappable");
                        zeroed_by_bsize += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 300, "too few OBMC-count cases: {checked}");
    assert!(nonzero > 100, "counts mostly zero: {nonzero}");
    assert!(
        zeroed_by_bsize > 50,
        "the is_motion_variation_allowed_bsize early return was never taken: {zeroed_by_bsize}"
    );
    assert!(counts.len() >= 5, "counts degenerate: {counts:?}");
}

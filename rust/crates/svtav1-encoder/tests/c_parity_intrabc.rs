//! Differential parity: the IntraBC pure-math translations
//! (`svtav1-encoder/src/intrabc.rs`) vs the REAL exported C functions
//! (IBC chunk 2, docs/ibc-port-map.md §D).
//!
//! Every test drives the Rust port and the linked C SVT-AV1 code on
//! identical randomized + adversarial inputs and asserts exact equality.
//! C oracles (all EXPORTED T-symbols, called through thin struct-assembly
//! shims in svtav1-cref):
//!   - `svt_aom_is_dv_valid`                (adaptive_mv_pred.c:1908)
//!   - `svt_aom_find_ref_dv`                (inter_prediction.c:2390)
//!   - `svt_aom_get_qp_based_th_scaling_factors` (enc_mode_config.c:25)
//!   - `svt_aom_estimate_mv_rate`           (md_rate_estimation.c:458) —
//!     which drives the static `svt_av1_build_nmv_cost_table` +
//!     `build_nmv_component_cost_table` chain end-to-end
//!   - `svt_av1_mv_bit_cost` / `_light`     (rd_cost.c:70/:59)
//!   - `svt_aom_mv_err_cost` / `_light`     (av1me.c:141/:126)

use svtav1_cref as cref;
use svtav1_encoder::intrabc;
use svtav1_entropy::mv_coding::{
    CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_OFFSET_BITS, MvSubpelPrecision, NmvComponent,
    NmvContext,
};
use svtav1_types::motion::{FullMvLimits, Mv};

/// Deterministic xorshift64* PRNG (house pattern — c_parity.rs).
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
    fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u64) as i32
    }
}

// ---------------------------------------------------------------------------
// svt_aom_is_dv_valid
// ---------------------------------------------------------------------------

/// Exhaustive-ish randomized sweep of DV validity: every C BlockSize, both
/// SB sizes, in-tile positions, aligned + subpel DVs, tile bounds at frame
/// edges AND adversarial (huge/sentinel) bounds — the aom-rs KB-15 Root 1
/// input class (§C.3 of the map).
#[test]
fn c_parity_is_dv_valid() {
    use svtav1_tables::block::{
        BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE, NUM_4X4_BLOCKS_HIGH, NUM_4X4_BLOCKS_WIDE,
    };
    let mut rng = Rng(0x1BC0_D51D_0001);
    let mut checked = 0u64;
    let mut valid_seen = 0u64;
    // (sb_log2_mi, sb_px)
    for &(sb_log2, sb_px) in &[(4u32, 64i32), (5u32, 128i32)] {
        // Tile bound sets: typical full-frame (512px = 128 MI), small, large,
        // non-zero start (multi-tile), and an adversarial huge-end sentinel
        // (1<<16 — what an unclamped caller would pass; C and the port must
        // agree even there, garbage-in-garbage-out equally).
        let tiles = [
            (0, 128, 0, 128),
            (0, 32, 0, 32),
            (0, 512, 0, 512),
            (64, 128, 64, 192),
            (0, 1 << 16, 0, 1 << 16),
        ];
        for &(trs, tre, tcs, tce) in &tiles {
            for bsize in 0..22usize {
                let bw = i32::from(BLOCK_SIZE_WIDE[bsize]);
                let bh = i32::from(BLOCK_SIZE_HIGH[bsize]);
                let bw_mi = i32::from(NUM_4X4_BLOCKS_WIDE[bsize]);
                let bh_mi = i32::from(NUM_4X4_BLOCKS_HIGH[bsize]);
                for _ in 0..200 {
                    // Block position: MI-aligned to the block's own MI dims
                    // (C positions always are), inside the tile.
                    let span_r = (tre - trs - bh_mi).max(1);
                    let span_c = (tce - tcs - bw_mi).max(1);
                    let mi_row = trs + (rng.below(span_r as u64) as i32 / bh_mi) * bh_mi;
                    let mi_col = tcs + (rng.below(span_c as u64) as i32 / bw_mi) * bw_mi;
                    // DV: mix of whole-pel (multiple of 8) and subpel;
                    // magnitudes biased small-negative (the legal-ish zone)
                    // with occasional huge values.
                    let mag = |rng: &mut Rng| -> i32 {
                        match rng.below(8) {
                            0 => rng.range_i32(-16, 16) * 8,
                            1..=4 => rng.range_i32(-256, 64) * 8,
                            5 | 6 => rng.range_i32(-2048, 2048) * 8,
                            _ => rng.range_i32(-16384, 16383), // incl. subpel
                        }
                    };
                    let (dv_x, dv_y) = (mag(&mut rng), mag(&mut rng));
                    let dv = Mv {
                        x: dv_x as i16,
                        y: dv_y as i16,
                    };
                    let tile = intrabc::TileMiBounds {
                        mi_col_start: tcs,
                        mi_col_end: tce,
                        mi_row_start: trs,
                        mi_row_end: tre,
                    };
                    let rs = intrabc::is_dv_valid(
                        dv, mi_row, mi_col, bw, bh, bw_mi, bh_mi, tile, sb_log2, sb_px,
                    );
                    let c = cref::is_dv_valid(
                        (dv.x, dv.y),
                        mi_row,
                        mi_col,
                        bsize as i32,
                        sb_log2 as i32,
                        (trs, tre, tcs, tce),
                    );
                    assert_eq!(
                        rs, c,
                        "is_dv_valid diverges: dv=({dv_x},{dv_y}) mi=({mi_row},{mi_col}) \
                         bsize={bsize} sb_log2={sb_log2} tile=({trs},{tre},{tcs},{tce})"
                    );
                    checked += 1;
                    valid_seen += u64::from(c);
                }
            }
        }
    }
    // Anti-vacuity: the sweep must exercise BOTH verdicts substantially.
    assert!(checked >= 40_000, "sweep too small: {checked}");
    assert!(
        valid_seen > 500 && valid_seen < checked - 500,
        "sweep is vacuous: {valid_seen}/{checked} valid"
    );
}

/// Directed edge cases: the wavefront / already-coded-SB64 boundaries and
/// the sub-8x8 chroma-reference margin, swept densely around each boundary.
#[test]
fn c_parity_is_dv_valid_boundaries() {
    // 4x4 (bsize 0, sub-8x8 both dims), 4x8 (1), 8x4 (2), 8x8 (3).
    for &(bsize, bw, bh, bw_mi, bh_mi) in &[
        (0i32, 4, 4, 1, 1),
        (1, 4, 8, 1, 2),
        (2, 8, 4, 2, 1),
        (3, 8, 8, 2, 2),
        (12, 64, 64, 16, 16),
    ] {
        for &(sb_log2, sb_px) in &[(4u32, 64i32), (5u32, 128i32)] {
            let tile = intrabc::TileMiBounds {
                mi_col_start: 0,
                mi_col_end: 128,
                mi_row_start: 0,
                mi_row_end: 128,
            };
            // Dense scan: every whole-pel DV in a window covering the
            // delay/wavefront boundary for a mid-frame block.
            let (mi_row, mi_col) = (80, 80);
            for dy_px in -340..=8 {
                for dx_px in [-336, -320, -260, -256, -128, -64, -8, -4, 0, 4] {
                    let dv = Mv {
                        x: (dx_px * 8) as i16,
                        y: (dy_px * 8) as i16,
                    };
                    let rs = intrabc::is_dv_valid(
                        dv, mi_row, mi_col, bw, bh, bw_mi, bh_mi, tile, sb_log2, sb_px,
                    );
                    let c = cref::is_dv_valid(
                        (dv.x, dv.y),
                        mi_row,
                        mi_col,
                        bsize,
                        sb_log2 as i32,
                        (0, 128, 0, 128),
                    );
                    assert_eq!(
                        rs, c,
                        "boundary diverges: dv_px=({dx_px},{dy_px}) bsize={bsize} sb={sb_px}"
                    );
                }
            }
            // Sub-8x8 chroma margin: positions near the tile left/top edge.
            for mi in 0..6 {
                for edge_px in 0..12 {
                    let dv = Mv {
                        x: (-(edge_px) * 8) as i16,
                        y: -512,
                    };
                    let rs = intrabc::is_dv_valid(
                        dv, 64, mi, bw, bh, bw_mi, bh_mi, tile, sb_log2, sb_px,
                    );
                    let c = cref::is_dv_valid(
                        (dv.x, dv.y),
                        64,
                        mi,
                        bsize,
                        sb_log2 as i32,
                        (0, 128, 0, 128),
                    );
                    assert_eq!(rs, c, "chroma-margin diverges: mi_col={mi} edge={edge_px}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_find_ref_dv + resolve_dv_ref composition
// ---------------------------------------------------------------------------

#[test]
fn c_parity_find_ref_dv() {
    for tile_row_start in [0, 16, 32, 64] {
        for mib_size in [16, 32] {
            for mi_row in 0..96 {
                for mi_col in [0, 40] {
                    let tile = intrabc::TileMiBounds {
                        mi_col_start: 0,
                        mi_col_end: 128,
                        mi_row_start: tile_row_start,
                        mi_row_end: 128,
                    };
                    let rs = intrabc::find_ref_dv(tile, mib_size, mi_row);
                    let (cx, cy) = cref::find_ref_dv(tile_row_start, mib_size, mi_row, mi_col);
                    assert_eq!(
                        (rs.x, rs.y),
                        (cx, cy),
                        "find_ref_dv diverges: trs={tile_row_start} mib={mib_size} mi_row={mi_row}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// svt_aom_get_qp_based_th_scaling_factors
// ---------------------------------------------------------------------------

#[test]
fn c_parity_qp_based_th_scaling_factors() {
    for enable in [false, true] {
        for qp in 0..=70u32 {
            let rs = intrabc::qp_based_th_scaling_factors(enable, qp);
            let c = cref::qp_based_th_scaling_factors(enable, qp);
            assert_eq!(rs, c, "qp scaling diverges at enable={enable} qp={qp}");
        }
    }
}

// ---------------------------------------------------------------------------
// The nmv/dv cost-table build chain (svt_aom_estimate_mv_rate end-to-end)
// ---------------------------------------------------------------------------

/// Random valid CDF in C layout (strictly decreasing ICDF, structural 0,
/// random adaptation counter) — the c_parity.rs house generator.
fn random_cdf(rng: &mut Rng, out: &mut [u16]) {
    let nsymbs = out.len() - 1;
    loop {
        let mut cuts: Vec<u16> = (0..nsymbs - 1)
            .map(|_| 1 + rng.below(32766) as u16)
            .collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            out[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    out[nsymbs - 1] = 0;
    out[nsymbs] = rng.below(33) as u16;
}

fn random_nmv_component(rng: &mut Rng) -> NmvComponent {
    let mut c = NmvComponent {
        classes_cdf: [0; MV_CLASSES + 1],
        class0_fp_cdf: [[0; MV_FP_SIZE + 1]; CLASS0_SIZE],
        fp_cdf: [0; MV_FP_SIZE + 1],
        sign_cdf: [0; 3],
        class0_hp_cdf: [0; 3],
        hp_cdf: [0; 3],
        class0_cdf: [0; CLASS0_SIZE + 1],
        bits_cdf: [[0; 3]; MV_OFFSET_BITS],
    };
    random_cdf(rng, &mut c.classes_cdf);
    for row in &mut c.class0_fp_cdf {
        random_cdf(rng, row);
    }
    random_cdf(rng, &mut c.fp_cdf);
    random_cdf(rng, &mut c.sign_cdf);
    random_cdf(rng, &mut c.class0_hp_cdf);
    random_cdf(rng, &mut c.hp_cdf);
    random_cdf(rng, &mut c.class0_cdf);
    for row in &mut c.bits_cdf {
        random_cdf(rng, row);
    }
    c
}

fn random_nmv_context(rng: &mut Rng) -> NmvContext {
    let mut ctx = NmvContext::default();
    random_cdf(rng, &mut ctx.joints_cdf);
    ctx.comps = [random_nmv_component(rng), random_nmv_component(rng)];
    ctx
}

/// The 143-u16 flat serialization in C struct layout order (the SAME order
/// c_parity_mv.rs verified against the C `FcTable::Nmvc` byte extraction).
fn flatten_nmv(ctx: &NmvContext) -> Vec<u16> {
    let mut flat: Vec<u16> = Vec::with_capacity(cref::NMV_FLAT_LEN);
    flat.extend_from_slice(&ctx.joints_cdf);
    for comp in &ctx.comps {
        flat.extend_from_slice(&comp.classes_cdf);
        for fp in &comp.class0_fp_cdf {
            flat.extend_from_slice(fp);
        }
        flat.extend_from_slice(&comp.fp_cdf);
        flat.extend_from_slice(&comp.sign_cdf);
        flat.extend_from_slice(&comp.class0_hp_cdf);
        flat.extend_from_slice(&comp.hp_cdf);
        flat.extend_from_slice(&comp.class0_cdf);
        for b in &comp.bits_cdf {
            flat.extend_from_slice(b);
        }
    }
    assert_eq!(flat.len(), cref::NMV_FLAT_LEN);
    flat
}

const SENTINEL: i32 = -0x5EAD;

/// Full-table diff of the build chain at every precision arm C reaches:
/// nmvc at LOW (hp=0) / HIGH (hp=1), ndvc at NONE (the dv arm), across the
/// default context + randomized contexts.
#[test]
fn c_parity_build_nmv_cost_table() {
    let mut rng = Rng(0x1BC0_D51D_0002);
    for iter in 0..6 {
        let (nmvc, ndvc) = if iter == 0 {
            (NmvContext::default(), NmvContext::default())
        } else {
            (random_nmv_context(&mut rng), random_nmv_context(&mut rng))
        };
        let (nf, df) = (flatten_nmv(&nmvc), flatten_nmv(&ndvc));
        for hp in [false, true] {
            let c = cref::estimate_mv_rate(false, true, hp, Some(&nf), Some(&df), SENTINEL);
            let rs_nmv = intrabc::build_nmv_cost_table(
                &nmvc,
                if hp {
                    MvSubpelPrecision::High
                } else {
                    MvSubpelPrecision::Low
                },
            );
            let rs_dv = intrabc::build_nmv_cost_table(&ndvc, MvSubpelPrecision::None);
            assert_eq!(rs_nmv.joint_cost.as_slice(), &c.nmv_joint, "nmv joint (iter {iter} hp {hp})");
            assert_eq!(rs_dv.joint_cost.as_slice(), &c.dv_joint, "dv joint (iter {iter})");
            for comp in 0..2 {
                let c_nmv = &c.nmv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
                let c_dv = &c.dv_costs[comp * cref::MV_VALS..(comp + 1) * cref::MV_VALS];
                for v in -intrabc::MV_MAX..=intrabc::MV_MAX {
                    let idx = (intrabc::MV_MAX + v) as usize;
                    assert_eq!(
                        rs_nmv.comp_cost[comp].cost(v),
                        c_nmv[idx],
                        "nmv cost diverges comp={comp} v={v} iter={iter} hp={hp}"
                    );
                    assert_eq!(
                        rs_dv.comp_cost[comp].cost(v),
                        c_dv[idx],
                        "dv cost diverges comp={comp} v={v} iter={iter}"
                    );
                }
            }
        }
    }
}

/// The two fill-gating hazards of `svt_aom_estimate_mv_rate` (map §F.5):
/// the `approx_inter_rate` early return SKIPS the dv fill even when
/// allow_intrabc, and `!allow_intrabc` never fills dv.
#[test]
fn c_parity_estimate_mv_rate_gating() {
    // !allow_intrabc: dv tables untouched (sentinel survives).
    let c = cref::estimate_mv_rate(false, false, false, None, None, SENTINEL);
    assert!(c.dv_joint.iter().all(|&v| v == SENTINEL));
    assert!(c.dv_costs.iter().all(|&v| v == SENTINEL));
    // approx_inter_rate: early return BEFORE the dv arm — dv untouched
    // even with allow_intrabc=1; nmv zeroed.
    let c = cref::estimate_mv_rate(true, true, false, None, None, SENTINEL);
    assert!(c.dv_joint.iter().all(|&v| v == SENTINEL), "approx must skip dv fill");
    assert!(c.dv_costs.iter().all(|&v| v == SENTINEL), "approx must skip dv fill");
    assert!(c.nmv_joint.iter().all(|&v| v == 0));
    assert!(c.nmv_costs.iter().all(|&v| v == 0));
}

// ---------------------------------------------------------------------------
// svt_av1_mv_bit_cost / svt_aom_mv_err_cost (+ _light)
// ---------------------------------------------------------------------------

/// RD-time + search-time MV cost formulas vs C, over the same tables (C's
/// from the real build chain, the port's from its own builder — already
/// proven equal above, so any divergence here is in the formulas).
#[test]
fn c_parity_mv_costs() {
    let mut rng = Rng(0x1BC0_D51D_0003);
    for iter in 0..4 {
        let ctx = if iter == 0 {
            NmvContext::default()
        } else {
            random_nmv_context(&mut rng)
        };
        let flat = flatten_nmv(&ctx);
        // dv arm => precision NONE tables (the IBC configuration).
        let c = cref::estimate_mv_rate(false, true, false, None, Some(&flat), SENTINEL);
        let rs = intrabc::build_nmv_cost_table(&ctx, MvSubpelPrecision::None);
        let (c_dv0, c_dv1) = c.dv_costs.split_at(cref::MV_VALS);

        let mut cases: Vec<((i16, i16), (i16, i16))> = vec![
            ((0, 0), (0, 0)),
            ((-8, 0), (0, 0)),
            ((0, -8), (0, 0)),
            ((-2048, -512), (-8, 0)),
            ((16376, -16376), (0, 0)),
            // clip-edge: |diff| == MV_MAX = 16383 (the largest value both
            // sides index in-table; C's CLIP3 bound ±16384 reads one past
            // the populated table — unreachable for any real DV, excluded).
            ((16383, 0), (0, 0)),
            ((-16383, 0), (0, 0)),
            ((0, 16383), (0, 0)),
        ];
        for _ in 0..400 {
            let m = |rng: &mut Rng| rng.range_i32(-2048, 2048) as i16;
            cases.push(((m(&mut rng) , m(&mut rng)), (m(&mut rng), m(&mut rng))));
        }
        for &(mv, refmv) in &cases {
            let rmv = Mv { x: mv.0, y: mv.1 };
            let rref = Mv {
                x: refmv.0,
                y: refmv.1,
            };
            // RD-time: MV_COST_WEIGHT_SUB (the IBC weight) + the ordinary
            // inter weight 108 (guards the formula for any weight).
            for weight in [intrabc::MV_COST_WEIGHT_SUB, 108] {
                let rs_cost = intrabc::mv_bit_cost(rmv, rref, &rs, weight);
                let c_cost = cref::mv_bit_cost(mv, refmv, &c.dv_joint, c_dv0, c_dv1, weight);
                assert_eq!(rs_cost, c_cost, "mv_bit_cost diverges mv={mv:?} ref={refmv:?} w={weight}");
            }
            assert_eq!(
                intrabc::mv_bit_cost_light(rmv, rref),
                cref::mv_bit_cost_light(mv, refmv),
                "mv_bit_cost_light diverges mv={mv:?} ref={refmv:?}"
            );
            for epb in [1, 63, 1024, 7276] {
                let rs_cost = intrabc::mv_err_cost(rmv, rref, &rs, epb);
                let c_cost = cref::mv_err_cost(mv, refmv, &c.dv_joint, c_dv0, c_dv1, epb);
                assert_eq!(rs_cost, c_cost, "mv_err_cost diverges mv={mv:?} ref={refmv:?} epb={epb}");
            }
            assert_eq!(
                intrabc::mv_err_cost_light(rmv, rref),
                cref::mv_err_cost_light(mv, refmv),
                "mv_err_cost_light diverges mv={mv:?} ref={refmv:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// mvsad_err_cost (static in C — locked against its exported eighth-pel twin)
// ---------------------------------------------------------------------------

/// C `mvsad_err_cost` (av1me.c:150, static) is `svt_mv_cost(diff*8) *
/// sad_per_bit >> AV1_PROB_COST_SHIFT` over FULL-PEL inputs. There is no
/// exported symbol, but its table lookup is the same `svt_mv_cost` the
/// exported `svt_av1_mv_bit_cost` uses (weight path aside), so lock the
/// port's `mvsad_err_cost` against a reconstruction from C's tables: the
/// exact formula over C's own dv cost tables.
#[test]
fn c_parity_mvsad_err_cost_formula() {
    let mut rng = Rng(0x1BC0_D51D_0004);
    let ctx = random_nmv_context(&mut rng);
    let flat = flatten_nmv(&ctx);
    let c = cref::estimate_mv_rate(false, true, false, None, Some(&flat), SENTINEL);
    let rs = intrabc::build_nmv_cost_table(&ctx, MvSubpelPrecision::None);
    let (c_dv0, c_dv1) = c.dv_costs.split_at(cref::MV_VALS);
    for _ in 0..2000 {
        // Full-pel domain: |diff|*8 must stay <= MV_MAX = 16383 (a diff of
        // exactly 2048 full-pel = 16384 eighth-pel is C's one-past-the-table
        // CLIP3 edge — unreachable for any real DV, excluded; see the
        // MvComponentCost PORT-NOTE).
        let m = |rng: &mut Rng| rng.range_i32(-1000, 1000);
        let (mx, my, rx, ry) = (m(&mut rng), m(&mut rng), m(&mut rng), m(&mut rng));
        let spb = rng.range_i32(1, 255);
        let rs_cost = intrabc::mvsad_err_cost(mx, my, rx, ry, spb, false, &rs);
        // C formula transcription over C's own tables (diff*8 lookup):
        let (dx, dy) = ((mx - rx) * 8, (my - ry) * 8);
        let joint = match (dy == 0, dx == 0) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        };
        let cost = c.dv_joint[joint]
            + c_dv0[(intrabc::MV_MAX + dy.clamp(-intrabc::MV_MAX, intrabc::MV_MAX)) as usize]
            + c_dv1[(intrabc::MV_MAX + dx.clamp(-intrabc::MV_MAX, intrabc::MV_MAX)) as usize];
        let c_cost = ((cost as u32).wrapping_mul(spb as u32).wrapping_add(1 << 8) >> 9) as i32;
        assert_eq!(rs_cost, c_cost, "mvsad_err_cost diverges mv=({mx},{my}) ref=({rx},{ry}) spb={spb}");
        // approx arm.
        assert_eq!(
            intrabc::mvsad_err_cost(mx, my, rx, ry, spb, true, &rs),
            1296 + 50 * ((mx - rx).abs() * 8 + (my - ry).abs() * 8),
            "mvsad light arm"
        );
    }
}

// ---------------------------------------------------------------------------
// set_mv_search_range (av1me.c:98) — pure clamp math, transcription lock
// ---------------------------------------------------------------------------

/// `svt_av1_set_mv_search_range` is EXPORTED in C but takes MvLimits*; its
/// math is 12 lines of pure clamping — locked by transcription against the
/// C source text (av1me.c:98-123) over a randomized sweep.
#[test]
fn set_mv_search_range_matches_c_math() {
    let mut rng = Rng(0x1BC0_D51D_0005);
    for _ in 0..5000 {
        let init = FullMvLimits {
            col_min: rng.range_i32(-4096, 0),
            col_max: rng.range_i32(0, 4096),
            row_min: rng.range_i32(-4096, 0),
            row_max: rng.range_i32(0, 4096),
        };
        let mv = Mv {
            x: rng.range_i32(-16383, 16383) as i16,
            y: rng.range_i32(-16383, 16383) as i16,
        };
        let mut got = init;
        intrabc::set_mv_search_range(&mut got, mv);
        // C transcription:
        let (x, y) = (i32::from(mv.x), i32::from(mv.y));
        let mut col_min = (x >> 3) - intrabc::MAX_FULL_PEL_VAL + i32::from((x & 7) != 0);
        let mut row_min = (y >> 3) - intrabc::MAX_FULL_PEL_VAL + i32::from((y & 7) != 0);
        let mut col_max = (x >> 3) + intrabc::MAX_FULL_PEL_VAL;
        let mut row_max = (y >> 3) + intrabc::MAX_FULL_PEL_VAL;
        col_min = col_min.max((intrabc::MV_LOW >> 3) + 1);
        row_min = row_min.max((intrabc::MV_LOW >> 3) + 1);
        col_max = col_max.min((intrabc::MV_UPP >> 3) - 1);
        row_max = row_max.min((intrabc::MV_UPP >> 3) - 1);
        let want = (
            init.col_min.max(col_min),
            init.col_max.min(col_max),
            init.row_min.max(row_min),
            init.row_max.min(row_max),
        );
        assert_eq!(
            (got.col_min, got.col_max, got.row_min, got.row_max),
            want,
            "set_mv_search_range diverges for mv={mv:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// IbcCtrls level table — transcription lock (set_intrabc_level is static C)
// ---------------------------------------------------------------------------

/// `set_intrabc_level` (enc_mode_config.c:1657-1836) is static and writes a
/// PPCS field — not FFI-reachable without constructing a PPCS shell for a
/// side-effect-only fn. Lock the transcription instead: values hand-carried
/// from the C switch (independently re-read for this test), INCLUDING the
/// case-6/7 fall-through fields (mesh_* left unassigned in C — the port
/// zero-defaults them, valid for the single-KEY-frame scope; see for_level's
/// PORT-NOTE).
#[test]
fn ibc_ctrls_level_table_transcription_lock() {
    use intrabc::{IbcCtrls, MeshPattern};
    // (level, palette_hint, nsq, b4, max_hash, max_cand, thresh, mv_diff_th,
    //  patterns[0..2], qp_scaling, search_dir)
    struct Row {
        level: u8,
        hint: bool,
        nsq: bool,
        b4: bool,
        max_hash: u8,
        max_cand: u16,
        thresh: u64,
        mv_diff_th: i32,
        p0: (i32, i32),
        p1: (i32, i32),
        qp_scaling: bool,
        dir: u8,
    }
    let rows = [
        Row { level: 1, hint: false, nsq: false, b4: false, max_hash: 64, max_cand: 256, thresh: 1 << 20, mv_diff_th: -1, p0: (256, 1), p1: (256, 1), qp_scaling: false, dir: 0 },
        Row { level: 2, hint: true, nsq: false, b4: false, max_hash: 64, max_cand: 256, thresh: 1 << 20, mv_diff_th: -1, p0: (256, 8), p1: (64, 1), qp_scaling: false, dir: 0 },
        Row { level: 3, hint: true, nsq: true, b4: false, max_hash: 64, max_cand: 256, thresh: 1 << 20, mv_diff_th: 0, p0: (256, 8), p1: (64, 1), qp_scaling: true, dir: 0 },
        Row { level: 4, hint: true, nsq: true, b4: false, max_hash: 64, max_cand: 64, thresh: 1 << 24, mv_diff_th: 0, p0: (256, 8), p1: (32, 1), qp_scaling: true, dir: 0 },
        Row { level: 5, hint: true, nsq: true, b4: false, max_hash: 8, max_cand: 64, thresh: 1 << 24, mv_diff_th: 0, p0: (256, 8), p1: (32, 1), qp_scaling: true, dir: 0 },
        // Cases 6 / MAX(7): C assigns ONLY enabled/hint/nsq/b4/max_hash/
        // max_cand/thresh/search_dir; mesh fields fall through (port: zeroed).
        Row { level: 6, hint: true, nsq: true, b4: false, max_hash: 8, max_cand: 32, thresh: u64::MAX, mv_diff_th: 0, p0: (0, 0), p1: (0, 0), qp_scaling: false, dir: 0 },
        Row { level: 7, hint: true, nsq: true, b4: false, max_hash: 8, max_cand: 32, thresh: u64::MAX, mv_diff_th: 0, p0: (0, 0), p1: (0, 0), qp_scaling: false, dir: 1 },
    ];
    assert!(!IbcCtrls::for_level(0).enabled);
    for r in &rows {
        let c = IbcCtrls::for_level(r.level);
        assert!(c.enabled, "level {} enabled", r.level);
        assert_eq!(c.palette_hint, r.hint, "level {} hint", r.level);
        assert_eq!(c.nsq_parent_gating, r.nsq, "level {} nsq", r.level);
        assert_eq!(c.b4_parent_gating, r.b4, "level {} b4", r.level);
        assert_eq!(c.max_block_size_hash, r.max_hash, "level {} max_hash", r.level);
        assert_eq!(c.max_cand_per_bucket, r.max_cand, "level {} max_cand", r.level);
        assert_eq!(c.exhaustive_mesh_thresh, r.thresh, "level {} thresh", r.level);
        assert_eq!(c.mesh_search_mv_diff_threshold, r.mv_diff_th, "level {} mv_diff", r.level);
        assert_eq!(
            c.mesh_patterns,
            [
                MeshPattern { range: r.p0.0, interval: r.p0.1 },
                MeshPattern { range: r.p1.0, interval: r.p1.1 },
                MeshPattern::default(),
                MeshPattern::default(),
            ],
            "level {} patterns",
            r.level
        );
        assert_eq!(c.mesh_qp_scaling, r.qp_scaling, "level {} qp_scaling", r.level);
        assert_eq!(c.search_dir, r.dir, "level {} dir", r.level);
    }
    // The allintra level derivation (enc_mode_config.c:2344-2371).
    for (preset, level) in [(0u8, 3u8), (1, 4), (2, 5), (3, 6), (4, 7), (5, 0), (6, 0), (10, 0)] {
        assert_eq!(intrabc::allintra_intrabc_level(preset, true, true), level);
        assert_eq!(intrabc::allintra_intrabc_level(preset, false, true), 0);
        assert_eq!(intrabc::allintra_intrabc_level(preset, true, false), 0);
    }
}


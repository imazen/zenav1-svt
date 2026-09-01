//! Differential parity: the rate-control rate model, qdelta-by-rate search,
//! RD-multiplier frame-type arms and boost helpers
//! (`svtav1-encoder/src/port_rc_process.rs`) vs the REAL exported C symbols in
//! `Bin/Release/libSvtAv1Enc.a`.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4) for every function C
//! exports: `svt_av1_rc_bits_per_mb`, `svt_av1_compute_qdelta_by_rate`,
//! `svt_aom_compute_rd_mult_based_on_qindex`,
//! `svt_av1_get_cqp_kf_boost_from_r0`, `svt_av1_get_gfu_boost_from_r0_lap`,
//! `svt_av1_calculate_boost_bits`, and all seven exported const tables.
//! Nothing here compares a transcription against a second transcription.
//!
//! Two C functions in this port are `static` with no exported symbol —
//! `find_qindex_by_rate` and the three `def_*_rd_multiplier` arms. Neither
//! gets a hand-derived vector suite, because both are driven END TO END by a
//! tier-1 differential above them: `find_qindex_by_rate` is the second half of
//! `compute_qdelta_by_rate`, and the multiplier arms are the three branches of
//! `compute_rd_mult_based_on_qindex`, swept here over all seven update types.

use svtav1_cref::rate_control as cref_rc;
use svtav1_encoder::port_rc_process as rc;

/// The port's `convert_qindex_to_q` covers 8- and 10-bit (it panics on 12);
/// that is the whole port's envelope, so the sweeps use the same two.
const BIT_DEPTHS: [u8; 2] = [8, 10];

#[test]
fn rc_bits_per_mb_matches_c_over_full_qindex_sweep() {
    // C asserts `MIN_BPB_FACTOR <= correction_factor <= MAX_BPB_FACTOR`
    // (0.005 .. 1.5, rc_process.h); stay inside it so the oracle is driven on
    // its own contract.
    let factors = [0.005f64, 0.1, 0.5, 0.7, 1.0, 1.25, 1.5];
    let mut cells = 0usize;
    for &bd in &BIT_DEPTHS {
        for &frame_type in &[rc::KEY_FRAME, rc::INTER_FRAME] {
            for &sc in &[false, true] {
                for &cf in &factors {
                    for qindex in 0..=255i32 {
                        let got = rc::rc_bits_per_mb(frame_type, qindex, cf, bd, sc);
                        let want = cref_rc::rc_bits_per_mb(
                            frame_type,
                            qindex,
                            cf,
                            i32::from(bd),
                            i32::from(sc),
                        );
                        assert_eq!(
                            got, want,
                            "rc_bits_per_mb(ft={frame_type}, q={qindex}, cf={cf}, bd={bd}, sc={sc})"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cells, BIT_DEPTHS.len() * 2 * 2 * factors.len() * 256);
}

/// The port's `convert_qindex_to_q` and C's must agree before anything built
/// on them means much — a positive control on the shared primitive.
#[test]
fn convert_qindex_to_q_matches_c() {
    for &bd in &BIT_DEPTHS {
        for qindex in 0..=255i32 {
            let got = svtav1_encoder::rate_control::convert_qindex_to_q(qindex, bd);
            let want = cref_rc::convert_qindex_to_q(qindex, i32::from(bd));
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "convert_qindex_to_q(q={qindex}, bd={bd}): port {got} vs C {want}"
            );
        }
    }
}

#[test]
fn compute_qdelta_by_rate_matches_c() {
    // `best_quality` / `worst_quality` are `quantizer_to_qindex[min_qp]` /
    // `[max_qp]` in the real encoder (`svt_aom_set_rc_param` ->
    // `svt_av1_rc_init`). These bounds bracket that range plus the degenerate
    // best == worst case the binary search must still terminate on.
    let bounds = [(0i32, 255i32), (0, 63), (44, 255), (100, 180), (128, 128)];
    // 1.0 / 1.5 / 1.7 / 2.0 are the ratios `svt_av1_frame_type_qdelta` can
    // produce (RATE_FACTOR_DELTAS plus the GF_ARF_LOW += 0.2); the rest probe
    // around them.
    let ratios = [0.25f64, 0.5, 1.0, 1.5, 1.7, 2.0, 4.0];
    let mut cells = 0usize;
    for &bd in &BIT_DEPTHS {
        for &(best, worst) in &bounds {
            for &frame_type in &[rc::KEY_FRAME, rc::INTER_FRAME] {
                for &sc in &[false, true] {
                    for &ratio in &ratios {
                        for qindex in (0..=255i32).step_by(3) {
                            let got = rc::compute_qdelta_by_rate(
                                best, worst, frame_type, qindex, ratio, bd, sc,
                            );
                            let want = cref_rc::compute_qdelta_by_rate(
                                best,
                                worst,
                                frame_type,
                                qindex,
                                ratio,
                                i32::from(bd),
                                i32::from(sc),
                            );
                            assert_eq!(
                                got, want,
                                "compute_qdelta_by_rate(best={best}, worst={worst}, \
                                 ft={frame_type}, q={qindex}, ratio={ratio}, bd={bd}, sc={sc})"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells > 10_000, "sweep collapsed to {cells} cells");
}

/// `frame_type_qdelta` (rc_crf_cqp.c:157) is `static`, but it is a pure
/// wrapper over the exported `compute_qdelta_by_rate` and the exported
/// `svt_av1_rate_factor_deltas` table, so driving C's exported pair with the
/// ratio the wrapper computes is still a tier-1 statement about the result.
#[test]
fn frame_type_qdelta_matches_c_through_exported_pair() {
    let levels = [
        rc::RateFactorLevel::InterNormal,
        rc::RateFactorLevel::InterLow,
        rc::RateFactorLevel::InterHigh,
        rc::RateFactorLevel::GfArfLow,
        rc::RateFactorLevel::GfArfStd,
        rc::RateFactorLevel::KfStd,
    ];
    let c_deltas = cref_rc::rate_factor_deltas();
    for &bd in &BIT_DEPTHS {
        for &lvl in &levels {
            for &sc in &[false, true] {
                for q in (0..=255i32).step_by(2) {
                    // Rebuild the ratio from C's OWN exported table.
                    let mut ratio = c_deltas[lvl as usize];
                    if lvl == rc::RateFactorLevel::GfArfLow {
                        ratio -= (0.0 - 2.0) * 0.1;
                        if ratio < 1.0 {
                            ratio = 1.0;
                        }
                    }
                    let frame_type = if lvl == rc::RateFactorLevel::KfStd {
                        rc::KEY_FRAME
                    } else {
                        rc::INTER_FRAME
                    };
                    let want = cref_rc::compute_qdelta_by_rate(
                        0,
                        255,
                        frame_type,
                        q,
                        ratio,
                        i32::from(bd),
                        i32::from(sc),
                    );
                    let got = rc::frame_type_qdelta(0, 255, lvl, q, bd, sc);
                    assert_eq!(
                        got, want,
                        "frame_type_qdelta(lvl={lvl:?}, q={q}, bd={bd}, sc={sc})"
                    );
                }
            }
        }
    }
}

/// The three `def_*_rd_multiplier` arms, swept over EVERY update type — this
/// is what pins the GF/ARF and LF/inter arms the port did not have.
#[test]
fn compute_rd_mult_based_on_qindex_matches_c_all_update_types() {
    use rc::FrameUpdateType::*;
    let update_types = [
        KfUpdate,
        LfUpdate,
        GfUpdate,
        ArfUpdate,
        OverlayUpdate,
        IntnlOverlayUpdate,
        IntnlArfUpdate,
    ];
    for &bd in &BIT_DEPTHS {
        for &ut in &update_types {
            for qindex in 0..=255i32 {
                let got = rc::compute_rd_mult_based_on_qindex(bd, ut, qindex);
                let want =
                    svtav1_cref::compute_rd_mult_based_on_qindex(bd, ut as i32, qindex as u8);
                assert_eq!(
                    got, want,
                    "compute_rd_mult_based_on_qindex(bd={bd}, ut={ut:?}, q={qindex})"
                );
            }
        }
    }
}

/// A separation check: the non-KF arms must actually DIFFER from the KF arm
/// somewhere, or the sweep above would pass against a KF-only port too. This
/// is the anti-vacuity control for the whole `def_*_rd_multiplier` story.
#[test]
fn rd_mult_arms_are_distinguishable_from_the_kf_arm() {
    use rc::FrameUpdateType::*;
    let mut arf_differs = 0usize;
    let mut inter_differs = 0usize;
    for qindex in 0..=255i32 {
        let kf = svtav1_cref::compute_rd_mult_based_on_qindex(8, KfUpdate as i32, qindex as u8);
        let arf = svtav1_cref::compute_rd_mult_based_on_qindex(8, ArfUpdate as i32, qindex as u8);
        let lf = svtav1_cref::compute_rd_mult_based_on_qindex(8, LfUpdate as i32, qindex as u8);
        if arf != kf {
            arf_differs += 1;
        }
        if lf != kf {
            inter_differs += 1;
        }
    }
    assert!(
        arf_differs > 200,
        "ARF arm differs from KF at only {arf_differs} of 256 qindices — a KF-only \
         port would pass the sweep above"
    );
    assert!(
        inter_differs > 200,
        "LF/inter arm differs from KF at only {inter_differs} of 256 qindices"
    );
}

#[test]
fn get_cqp_kf_boost_from_r0_matches_c() {
    // r0 is the TPL rate ratio, normally in (0, 1]; 0.0 exercises the
    // R0_MIN_DIVISOR floor.
    let r0s = [
        0.0f64, 1e-9, 1e-6, 0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0, 1.5, 4.0,
        // TIE CELLS, deliberately constructed. With `frames_to_key == -1` the
        // factor is exactly 7.0, so the numerator is `3 * (75 + 17*7) = 582`
        // (res <= 720p) or `4 * 194 = 776`. 582/1164 and 776/1552 are exactly
        // 0.5 in f64, and 582/388 / 776/517.333.. land on 1.5. C's `rint`
        // rounds half-to-EVEN, Rust's `f64::round` rounds half-AWAY-from-zero,
        // so these cells are what separates the two. Verified by mutation:
        // swapping `round_ties_even` for `round` in the port makes this test
        // FAIL only because these values are here.
        1164.0, 1552.0, 388.0, 776.0, 2328.0, 3104.0,
    ];
    // -1 is C's "frames_to_key not available" sentinel.
    let ftks = [-1i32, 0, 1, 4, 9, 16, 17, 25, 60, 100, 101, 1000];
    for &r0 in &r0s {
        for &ftk in &ftks {
            for res in 0..=6i32 {
                let got = rc::get_cqp_kf_boost_from_r0(r0, ftk, res);
                let want = cref_rc::get_cqp_kf_boost_from_r0(r0, ftk, res);
                assert_eq!(got, want, "kf_boost(r0={r0}, ftk={ftk}, res={res})");
            }
        }
    }
}

#[test]
fn get_gfu_boost_from_r0_lap_matches_c() {
    // 600.0 / 1200.0 are TIE CELLS: with (min, max) = (4, 10) and
    // frames_to_key >= 100 the factor is exactly 300, so 300/600 == 0.5 and
    // 300/1200 == 0.25; 300/200 == 1.5. Same rint-vs-round separation as the
    // KF twin above, and the same mutation check backs it.
    let r0s = [
        0.0f64, 1e-6, 0.05, 0.2, 0.5, 0.8, 1.0, 2.0, 600.0, 1200.0, 200.0, 120.0,
    ];
    let ftks = [0i32, 1, 4, 9, 16, 25, 64, 100, 400, 1000];
    // The (min, max) pairs the callers use, plus an inverted pair so the
    // AOMMIN-then-AOMMAX order (which is NOT the same as a clamp when
    // min > max) is exercised.
    let bounds = [(4.0f64, 10.0f64), (2.0, 8.0), (10.0, 4.0), (0.0, 100.0)];
    for &(mn, mx) in &bounds {
        for &r0 in &r0s {
            for &ftk in &ftks {
                let got = rc::get_gfu_boost_from_r0_lap(mn, mx, r0, ftk);
                let want = cref_rc::get_gfu_boost_from_r0_lap(mn, mx, r0, ftk);
                assert_eq!(
                    got, want,
                    "gfu_boost(min={mn}, max={mx}, r0={r0}, ftk={ftk})"
                );
            }
        }
    }
}

#[test]
fn calculate_boost_bits_matches_c() {
    let frame_counts = [-1i32, 0, 1, 2, 4, 8, 16, 32, 64];
    let boosts = [0i32, 1, 100, 500, 1023, 1024, 2000, 5000, 20000, 100_000];
    let group_bits = [
        -1i64,
        0,
        1,
        1_000,
        100_000,
        10_000_000,
        1_000_000_000,
        4_000_000_000,
    ];
    for &fc in &frame_counts {
        for &b in &boosts {
            for &gb in &group_bits {
                let got = rc::calculate_boost_bits(fc, b, gb);
                let want = cref_rc::calculate_boost_bits(fc, b, gb);
                assert_eq!(
                    got, want,
                    "calculate_boost_bits(fc={fc}, boost={b}, gb={gb})"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The const tables — compared against the EXPORTED data symbols, not against a
// second transcription of the C source.
// ---------------------------------------------------------------------------

#[test]
fn const_tables_match_the_exported_c_symbols() {
    assert_eq!(
        rc::NON_BASE_QINDEX_WEIGHT_REF,
        cref_rc::non_base_qindex_weight_ref(),
        "svt_av1_non_base_qindex_weight_ref"
    );
    assert_eq!(
        rc::NON_BASE_QINDEX_WEIGHT_WQ,
        cref_rc::non_base_qindex_weight_wq(),
        "svt_av1_non_base_qindex_weight_wq"
    );
    assert_eq!(
        rc::TPL_HL_ISLICE_DIV_FACTOR,
        cref_rc::tpl_hl_islice_div_factor(),
        "svt_av1_tpl_hl_islice_div_factor"
    );
    assert_eq!(
        rc::TPL_HL_BASE_FRAME_DIV_FACTOR,
        cref_rc::tpl_hl_base_frame_div_factor(),
        "svt_av1_tpl_hl_base_frame_div_factor"
    );
    assert_eq!(rc::R0_WEIGHT, cref_rc::r0_weight(), "svt_av1_r0_weight");
    assert_eq!(
        rc::RATE_FACTOR_DELTAS,
        cref_rc::rate_factor_deltas(),
        "svt_av1_rate_factor_deltas"
    );
    let c_levels = cref_rc::rate_factor_levels();
    for (i, lvl) in rc::RATE_FACTOR_LEVELS.iter().enumerate() {
        assert_eq!(
            *lvl as i32, c_levels[i],
            "svt_av1_rate_factor_levels[{i}]: port {lvl:?} vs C {}",
            c_levels[i]
        );
    }
}

/// Anti-vacuity for the table test: the exported symbols must actually be
/// READ, not silently zero. A linker that dropped the data symbol would make
/// every table above compare against zeros, which would fail — but a table
/// that is legitimately all-100s (`non_base_qindex_weight_ref`) would still
/// pass against garbage that happened to match. Prove the C side is live by
/// asserting on the one entry that is deliberately NOT uniform.
#[test]
fn exported_table_read_is_not_vacuous() {
    let wq = cref_rc::non_base_qindex_weight_wq();
    assert_eq!(
        wq[2], 300,
        "svt_av1_non_base_qindex_weight_wq[2] read as {} — the exported data symbol \
         is not being read (a zeroed or wrong-symbol read would look like this)",
        wq[2]
    );
    let deltas = cref_rc::rate_factor_deltas();
    assert!(
        (deltas[4] - 2.0).abs() < 1e-12 && (deltas[3] - 1.5).abs() < 1e-12,
        "svt_av1_rate_factor_deltas read as {deltas:?}"
    );
}

// ---------------------------------------------------------------------------
// Sequence-level setup: `svt_aom_set_rc_param` + `svt_av1_rc_init`.
//
// Both are EXPORTED and both take a `SequenceControlSet*`; the shim
// `calloc`s a real one per call (never a `static` — that race was measured in
// this repo today) and drives the real symbol. Tier 1.
// ---------------------------------------------------------------------------

/// A tiny xorshift so the sweeps are reproducible without a dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn set_rc_param_matches_c() {
    let mut rng = Rng(0x5eed_1234_9abc_def0);
    // Dimensions: the C field is uint16_t, so stay inside it. The listed
    // widths bracket every `w % 16` residue class, which is what separates
    // `((w+15)/16) << 1` from `(2w+15)/16` on the downsample arm.
    let dims: Vec<u32> = (1..=17u32)
        .chain([31, 32, 33, 63, 64, 65, 127, 128, 176, 640, 1920, 3840, 7680])
        .collect();
    let mut cells = 0usize;
    for &w in &dims {
        for &h in &dims {
            for &fpd in &[false, true] {
                for rc_mode in 0..=2i32 {
                    for &gop_rc in &[false, true] {
                        let inp = svtav1_encoder::port_rc_process::SetRcParamInput {
                            first_pass_downsample: fpd,
                            max_input_luma_width: w,
                            max_input_luma_height: h,
                            encoder_bit_depth: [8, 10, 12][(rng.below(3)) as usize],
                            vbr_min_section_pct: rng.below(200) as i32,
                            vbr_max_section_pct: rng.below(2000) as i32,
                            rate_control_mode: rc_mode,
                            min_qp_allowed: rng.below(64) as i32,
                            max_qp_allowed: rng.below(64) as i32,
                            gop_constraint_rc: gop_rc,
                            over_shoot_pct: rng.below(1001) as i32,
                            under_shoot_pct: rng.below(101) as i32,
                            maximum_buffer_size_ms: rng.below(1_000_000) as i64,
                            starting_buffer_level_ms: rng.below(1_000_000) as i64,
                            optimal_buffer_level_ms: rng.below(1_000_000) as i64,
                            max_intra_bitrate_pct: rng.below(10_000) as u32,
                            max_inter_bitrate_pct: rng.below(10_000) as u32,
                            sframe_dist: rng.below(1000) as i32,
                            sframe_mode: rng.below(3) as i32,
                        };
                        let c_in = svtav1_cref::rate_control::SetRcParamIn {
                            first_pass_downsample: i32::from(inp.first_pass_downsample),
                            max_input_luma_width: inp.max_input_luma_width,
                            max_input_luma_height: inp.max_input_luma_height,
                            encoder_bit_depth: inp.encoder_bit_depth,
                            vbr_min_section_pct: inp.vbr_min_section_pct,
                            vbr_max_section_pct: inp.vbr_max_section_pct,
                            rate_control_mode: inp.rate_control_mode,
                            min_qp_allowed: inp.min_qp_allowed,
                            max_qp_allowed: inp.max_qp_allowed,
                            gop_constraint_rc: i32::from(inp.gop_constraint_rc),
                            over_shoot_pct: inp.over_shoot_pct,
                            under_shoot_pct: inp.under_shoot_pct,
                            maximum_buffer_size_ms: inp.maximum_buffer_size_ms,
                            starting_buffer_level_ms: inp.starting_buffer_level_ms,
                            optimal_buffer_level_ms: inp.optimal_buffer_level_ms,
                            max_intra_bitrate_pct: inp.max_intra_bitrate_pct,
                            max_inter_bitrate_pct: inp.max_inter_bitrate_pct,
                            sframe_dist: inp.sframe_dist,
                            sframe_mode: inp.sframe_mode,
                        };
                        let want = svtav1_cref::rate_control::set_rc_param(&c_in);
                        let got = svtav1_encoder::port_rc_process::set_rc_param(&inp);
                        assert_eq!(got.frame_width, want.frame_width, "frame_width {inp:?}");
                        assert_eq!(got.frame_height, want.frame_height, "frame_height {inp:?}");
                        assert_eq!(got.mb_cols, want.mb_cols, "mb_cols {inp:?}");
                        assert_eq!(got.mb_rows, want.mb_rows, "mb_rows {inp:?}");
                        assert_eq!(got.num_mbs, want.num_mbs, "num_mbs {inp:?}");
                        assert_eq!(got.bit_depth, want.bit_depth, "bit_depth {inp:?}");
                        assert_eq!(got.vbrmin_section, want.vbrmin_section, "vbrmin {inp:?}");
                        assert_eq!(got.vbrmax_section, want.vbrmax_section, "vbrmax {inp:?}");
                        assert_eq!(got.mode, want.mode, "mode {inp:?}");
                        assert_eq!(got.best_allowed_q, want.best_allowed_q, "best_q {inp:?}");
                        assert_eq!(got.worst_allowed_q, want.worst_allowed_q, "worst_q {inp:?}");
                        assert_eq!(got.over_shoot_pct, want.over_shoot_pct, "over {inp:?}");
                        assert_eq!(got.under_shoot_pct, want.under_shoot_pct, "under {inp:?}");
                        assert_eq!(
                            got.maximum_buffer_size_ms, want.maximum_buffer_size_ms,
                            "max_buf {inp:?}"
                        );
                        assert_eq!(
                            got.starting_buffer_level_ms, want.starting_buffer_level_ms,
                            "start_buf {inp:?}"
                        );
                        assert_eq!(
                            got.optimal_buffer_level_ms, want.optimal_buffer_level_ms,
                            "opt_buf {inp:?}"
                        );
                        assert_eq!(
                            got.max_intra_bitrate_pct, want.max_intra_bitrate_pct,
                            "intra_pct {inp:?}"
                        );
                        assert_eq!(
                            got.max_inter_bitrate_pct, want.max_inter_bitrate_pct,
                            "inter_pct {inp:?}"
                        );
                        assert_eq!(got.sframe_dist, want.sframe_dist, "sframe_dist {inp:?}");
                        assert_eq!(got.sframe_mode, want.sframe_mode, "sframe_mode {inp:?}");
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells > 5_000, "sweep collapsed to {cells} cells");
}

/// The downsample MB-count arm is exactly the "assume the index/order looks
/// like what it looks like" trap. Prove C really computes `((w+15)/16) << 1`
/// and NOT `(2w+15)/16`, by finding a width where the two disagree and
/// reading C's answer.
#[test]
fn set_rc_param_downsample_mb_cols_is_ceil_then_double() {
    // w = 17: ((17+15)/16)<<1 == 4, but (34+15)/16 == 3. They differ.
    let mut c_in = svtav1_cref::rate_control::SetRcParamIn {
        first_pass_downsample: 1,
        max_input_luma_width: 17,
        max_input_luma_height: 17,
        ..Default::default()
    };
    c_in.max_qp_allowed = 63;
    let want = svtav1_cref::rate_control::set_rc_param(&c_in);
    assert_eq!(
        want.mb_cols, 4,
        "C's downsample mb_cols for w=17 is {} — if this is 3, C ceil-divides \
         the DOUBLED width and the port's comment is wrong",
        want.mb_cols
    );
    assert_ne!(
        want.mb_cols,
        (2 * 17 + 15) / 16,
        "the two readings must differ here"
    );
}

#[test]
fn rc_init_matches_c() {
    let mut rng = Rng(0xabcd_0f0f_1234_5678);
    let mut cells = 0usize;
    for mode in 0..=2i32 {
        for hier in 0..=5i32 {
            for _ in 0..64 {
                let best = rng.below(256) as i32;
                let worst = rng.below(256) as i32;
                let inp = svtav1_encoder::port_rc_process::RcInitInput {
                    mode,
                    best_allowed_q: best,
                    worst_allowed_q: worst,
                    starting_buffer_level: rng.below(1_000_000_000) as i64,
                    avg_frame_bandwidth: rng.below(10_000_000) as i32,
                    hierarchical_levels: hier,
                };
                let c_in = svtav1_cref::rate_control::RcInitIn {
                    mode: inp.mode,
                    best_allowed_q: inp.best_allowed_q,
                    worst_allowed_q: inp.worst_allowed_q,
                    starting_buffer_level: inp.starting_buffer_level,
                    avg_frame_bandwidth: inp.avg_frame_bandwidth,
                    hierarchical_levels: inp.hierarchical_levels,
                    // Only read on the `mode != AOM_Q` tail
                    // (`svt_av1_new_framerate`); non-zero so C's own
                    // divide-by-frame-rate cannot trip on the zeroed set.
                    frame_rate_numerator: 60_000,
                    frame_rate_denominator: 1_000,
                };
                let want = svtav1_cref::rate_control::rc_init(&c_in);
                let got = svtav1_encoder::port_rc_process::rc_init(&inp);
                assert_eq!(
                    got.avg_frame_qindex_key, want.avg_frame_qindex_key,
                    "avg_frame_qindex[KEY] {inp:?}"
                );
                assert_eq!(
                    got.avg_frame_qindex_inter, want.avg_frame_qindex_inter,
                    "avg_frame_qindex[INTER] {inp:?}"
                );
                assert_eq!(got.last_q_key, want.last_q_key, "last_q[KEY] {inp:?}");
                assert_eq!(got.last_q_inter, want.last_q_inter, "last_q[INTER] {inp:?}");
                assert_eq!(got.buffer_level, want.buffer_level, "buffer_level {inp:?}");
                assert_eq!(
                    got.bits_off_target, want.bits_off_target,
                    "bits_off_target {inp:?}"
                );
                assert_eq!(
                    got.rolling_target_bits, want.rolling_target_bits,
                    "rolling_target {inp:?}"
                );
                assert_eq!(
                    got.rolling_actual_bits, want.rolling_actual_bits,
                    "rolling_actual {inp:?}"
                );
                assert_eq!(got.total_actual_bits, want.total_actual_bits);
                assert_eq!(got.total_target_bits, want.total_target_bits);
                assert_eq!(
                    got.frames_since_key, want.frames_since_key,
                    "frames_since_key {inp:?}"
                );
                assert_eq!(got.frames_since_cdf_update, want.frames_since_cdf_update);
                assert_eq!(got.this_key_frame_forced, want.this_key_frame_forced);
                assert_eq!(
                    got.rate_correction_factors, want.rate_correction_factors,
                    "rate_correction_factors {inp:?}"
                );
                assert_eq!(
                    got.baseline_gf_interval, want.baseline_gf_interval,
                    "baseline_gf_interval {inp:?}"
                );
                assert_eq!(
                    got.worst_quality, want.worst_quality,
                    "worst_quality {inp:?}"
                );
                assert_eq!(got.best_quality, want.best_quality, "best_quality {inp:?}");
                assert_eq!(got.cur_avg_base_me_dist, want.cur_avg_base_me_dist);
                assert_eq!(got.prev_avg_base_me_dist, want.prev_avg_base_me_dist);
                assert_eq!(got.avg_frame_low_motion, want.avg_frame_low_motion);
                cells += 1;
            }
        }
    }
    assert_eq!(cells, 3 * 6 * 64);
}

/// Anti-vacuity for the two above: prove the shim's output block is really
/// being written by C and is not just the zeroed `Default`. `frames_since_key`
/// is C's hardcoded 8 and `rate_correction_factors[KF_STD]` is the 1.0
/// override — both are non-zero for a reason, and both would be 0 if the FFI
/// out-parameter were not populated.
#[test]
fn rc_init_c_side_output_is_populated_not_zeroed() {
    let c_in = svtav1_cref::rate_control::RcInitIn {
        mode: 2, // AOM_Q
        best_allowed_q: 0,
        worst_allowed_q: 255,
        starting_buffer_level: 0,
        avg_frame_bandwidth: 0,
        hierarchical_levels: 4,
        frame_rate_numerator: 60_000,
        frame_rate_denominator: 1_000,
    };
    let out = svtav1_cref::rate_control::rc_init(&c_in);
    assert_eq!(
        out.frames_since_key, 8,
        "C's rc->frames_since_key read as {} — the shim out-parameter is not \
         being populated (everything would compare equal against a zeroed port)",
        out.frames_since_key
    );
    assert_eq!(
        out.rate_correction_factors[5], 1.0,
        "rate_correction_factors[KF_STD=5] read as {}",
        out.rate_correction_factors[5]
    );
    assert_eq!(out.rate_correction_factors[0], 0.7);
    assert_eq!(out.baseline_gf_interval, 16);
}

// ---------------------------------------------------------------------------
// The MD lambda chain: `svt_aom_compute_rd_mult`, `svt_aom_compute_fast_lambda`
// and `svt_aom_lambda_assign` (all EXPORTED), plus the `static const` SAD
// lambda tables they read.
// ---------------------------------------------------------------------------

/// All 768 entries of the three `av1_lambda_mode_decision*_bit_sad` tables,
/// against the REAL C arrays. They are `static const` in a header, so there is
/// no symbol to bind; `ref_rc_lambda_md_sad` indexes the C-side array.
#[test]
fn lambda_sad_tables_match_c() {
    use svtav1_encoder::port_rc_process::lambda_tables as t;
    for q in 0..256usize {
        assert_eq!(
            t::LAMBDA_MODE_DECISION_8BIT_SAD[q],
            cref_rc::lambda_md_sad(8, q as i32),
            "av1_lambda_mode_decision8_bit_sad[{q}]"
        );
        assert_eq!(
            t::LAMBDA_MODE_DECISION_10BIT_SAD[q],
            cref_rc::lambda_md_sad(10, q as i32),
            "av1lambda_mode_decision10_bit_sad[{q}]"
        );
        assert_eq!(
            t::LAMBDA_MODE_DECISION_12BIT_SAD[q],
            cref_rc::lambda_md_sad(12, q as i32),
            "av1lambda_mode_decision12_bit_sad[{q}]"
        );
    }
    // Anti-vacuity: the shim must be reading a real table, not returning 0.
    assert_eq!(
        cref_rc::lambda_md_sad(8, 0),
        86,
        "C's av1_lambda_mode_decision8_bit_sad[0] read as {} — the shim is not \
         indexing the real table",
        cref_rc::lambda_md_sad(8, 0)
    );
    assert_ne!(
        cref_rc::lambda_md_sad(8, 255),
        cref_rc::lambda_md_sad(10, 255)
    );
}

/// Build the port-side and C-side context pair from one description, so the
/// sweeps below cannot drift apart.
fn lambda_ctx(
    frame_type: i32,
    tl: u8,
    hier: u8,
    ut: rc::FrameUpdateType,
    alt: bool,
    rtc: bool,
    stats: bool,
    base_q: i32,
    dq: bool,
    r0dq: bool,
    scale: [i32; 7],
) -> (
    svtav1_encoder::port_rc_process::LambdaContext,
    svtav1_cref::rate_control::LambdaCtx,
) {
    let p = svtav1_encoder::port_rc_process::LambdaContext {
        frame_type,
        temporal_layer_index: tl,
        hierarchical_levels: hier,
        update_type: ut,
        alt_lambda_factors: alt,
        rtc,
        stats_based_sb_lambda_modulation: stats,
        base_q_idx: base_q,
        delta_q_present: dq,
        r0_delta_qp_md: r0dq,
        lambda_scale_factors: scale,
    };
    let c = svtav1_cref::rate_control::LambdaCtx {
        frame_type,
        temporal_layer_index: i32::from(tl),
        hierarchical_levels: i32::from(hier),
        update_type: ut as i32,
        alt_lambda_factors: i32::from(alt),
        rtc: i32::from(rtc),
        stats_based_sb_lambda_modulation: i32::from(stats),
        base_q_idx: base_q,
        delta_q_present: i32::from(dq),
        r0_delta_qp_md: i32::from(r0dq),
        lambda_scale_factors: scale,
    };
    (p, c)
}

const UPDATE_TYPES: [rc::FrameUpdateType; 7] = [
    rc::FrameUpdateType::KfUpdate,
    rc::FrameUpdateType::LfUpdate,
    rc::FrameUpdateType::GfUpdate,
    rc::FrameUpdateType::ArfUpdate,
    rc::FrameUpdateType::OverlayUpdate,
    rc::FrameUpdateType::IntnlOverlayUpdate,
    rc::FrameUpdateType::IntnlArfUpdate,
];

/// `svt_aom_compute_rd_mult` + `svt_aom_compute_fast_lambda`, swept over
/// every input `update_lambda` reads. `update_lambda` is `static` in C, so
/// this pair IS its oracle — and the sweep covers all four branches of its
/// stats-based block plus both `rd_frame_type_factor` rows and the alt table.
#[test]
fn compute_rd_mult_and_fast_lambda_match_c() {
    let mut rng = Rng(0x1357_9bdf_0246_8ace);
    let mut cells = 0usize;
    // Cover both `frame_type` values, every temporal layer vs hierarchical
    // level relation (which is what selects update_lambda's gf_update_type),
    // and every flag combination.
    for &bd in &[8u8, 10] {
        for &frame_type in &[rc::KEY_FRAME, rc::INTER_FRAME] {
            for hier in 0..=5u8 {
                for tl in 0..=5u8 {
                    for &alt in &[false, true] {
                        for &rtc_flag in &[false, true] {
                            for &stats in &[false, true] {
                                for &dq in &[false, true] {
                                    for &r0dq in &[false, true] {
                                        let ut = UPDATE_TYPES[(rng.below(7)) as usize];
                                        let base_q = rng.below(256) as i32;
                                        let (p, c) = lambda_ctx(
                                            frame_type, tl, hier, ut, alt, rtc_flag, stats, base_q,
                                            dq, r0dq, [128; 7],
                                        );
                                        // Walk q_index around base_q so every
                                        // qdiff threshold (-8, -4, 0, +4, +8)
                                        // is straddled.
                                        for delta in
                                            [-20i32, -9, -8, -5, -4, -1, 0, 1, 4, 5, 8, 9, 20]
                                        {
                                            let q = (base_q + delta).clamp(0, 255) as u8;
                                            for &me_q in &[q, base_q.clamp(0, 255) as u8] {
                                                let got_full =
                                                    svtav1_encoder::port_rc_process::compute_rd_mult(
                                                        &p, q, me_q, bd,
                                                    );
                                                let want_full =
                                                    svtav1_cref::rate_control::compute_rd_mult(
                                                        &c,
                                                        i32::from(q),
                                                        i32::from(me_q),
                                                        i32::from(bd),
                                                    );
                                                assert_eq!(
                                                    got_full, want_full,
                                                    "compute_rd_mult bd={bd} q={q} me_q={me_q} {p:?}"
                                                );
                                                let got_fast =
                                                    svtav1_encoder::port_rc_process::compute_fast_lambda(
                                                        &p, q, me_q, bd,
                                                    );
                                                let want_fast =
                                                    svtav1_cref::rate_control::compute_fast_lambda(
                                                        &c,
                                                        i32::from(q),
                                                        i32::from(me_q),
                                                        i32::from(bd),
                                                    );
                                                assert_eq!(
                                                    got_fast, want_fast,
                                                    "compute_fast_lambda bd={bd} q={q} me_q={me_q} {p:?}"
                                                );
                                                cells += 1;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cells > 100_000, "sweep collapsed to {cells} cells");
}

/// `svt_aom_lambda_assign` across all three bit depths, both
/// `multiply_lambda` values, and non-identity `lambda_scale_factors`.
#[test]
fn lambda_assign_matches_c() {
    let mut rng = Rng(0x2468_ace0_1357_9bdf);
    let mut cells = 0usize;
    // 8 and 10 only: the port's 12-bit `full_lambda` needs a 12-bit DC
    // quantizer table it does not have, and says so by panicking rather than
    // returning a plausible number. See `lambda_assign`'s docs.
    for &bd in &[8u8, 10] {
        for &mul in &[false, true] {
            for &ut in &UPDATE_TYPES {
                for &frame_type in &[rc::KEY_FRAME, rc::INTER_FRAME] {
                    for &stats in &[false, true] {
                        // 128 is the identity; the other values prove the
                        // `>> 7` scale is really applied and indexed by
                        // `update_type` (not by gf_update_type).
                        for &sf in &[128i32, 96, 160, 255] {
                            let mut scale = [128i32; 7];
                            scale[ut as usize] = sf;
                            let hier = rng.below(6) as u8;
                            let tl = rng.below(6) as u8;
                            let base_q = rng.below(256) as i32;
                            let (p, c) = lambda_ctx(
                                frame_type,
                                tl,
                                hier,
                                ut,
                                false,
                                false,
                                stats,
                                base_q,
                                rng.below(2) == 1,
                                rng.below(2) == 1,
                                scale,
                            );
                            for qp in (0..=255i32).step_by(7) {
                                let got = svtav1_encoder::port_rc_process::lambda_assign(
                                    &p, bd, qp as u8, mul,
                                );
                                let want = svtav1_cref::rate_control::lambda_assign(
                                    &c,
                                    i32::from(bd),
                                    qp,
                                    mul,
                                );
                                assert_eq!(
                                    got, want,
                                    "lambda_assign bd={bd} qp={qp} mul={mul} sf={sf} {p:?}"
                                );
                                cells += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cells > 5_000, "sweep collapsed to {cells} cells");
}

/// Anti-vacuity for the lambda sweeps: `update_lambda`'s `gf_update_type` is
/// DERIVED from frame_type/temporal layer and is NOT `ppcs->update_type`.
/// Prove C really behaves that way, by holding `update_type` fixed and moving
/// only the temporal layer — if C used `update_type` the results would be
/// identical and a port that confused the two would pass the sweep above.
#[test]
fn update_lambda_gf_type_is_derived_not_the_ppcs_update_type() {
    // update_type fixed at LF_UPDATE; only temporal_layer_index moves.
    let mk = |tl: u8| {
        lambda_ctx(
            rc::INTER_FRAME,
            tl,
            /* hierarchical_levels */ 4,
            rc::FrameUpdateType::LfUpdate,
            false,
            false,
            false,
            128,
            false,
            false,
            [128; 7],
        )
        .1
    };
    let at_tl0 = svtav1_cref::rate_control::compute_rd_mult(&mk(0), 128, 128, 8);
    let at_tl2 = svtav1_cref::rate_control::compute_rd_mult(&mk(2), 128, 128, 8);
    let at_tl4 = svtav1_cref::rate_control::compute_rd_mult(&mk(4), 128, 128, 8);
    assert_ne!(
        at_tl0, at_tl4,
        "C gave the same rdmult ({at_tl0}) at temporal_layer 0 and 4 with \
         update_type held at LF_UPDATE — then gf_update_type is NOT derived and \
         the port's comment is wrong"
    );
    // tl=0 -> ARF_UPDATE (factor 150), tl=2 < 4 -> INTNL_ARF (150),
    // tl=4 == max -> LF_UPDATE (180). So 0 and 2 agree, 4 differs.
    assert_eq!(
        at_tl0, at_tl2,
        "tl 0 and 2 must land on the same factor (150)"
    );
}

/// `svt_av1_new_framerate` (pass2_strategy.c:900) — EXPORTED, TIER 1 — and
/// through it the `static` `av1_rc_update_framerate` (:880) it calls
/// unconditionally. This is the step [`rc_init`] names as its own gap.
#[test]
fn new_framerate_matches_c() {
    let mut rng = Rng(0x0fed_cba9_8765_4321);
    // Frame rates around C's `< 0.1` cliff (which replaces the value with 30,
    // NOT with 0.1), plus the usual broadcast/film rates.
    let rates = [
        0.0f64, 0.001, 0.05, 0.0999, 0.1, 0.5, 1.0, 23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94,
        60.0, 120.0, 240.0, 1000.0,
    ];
    // num_mbs for 64x64 up to 8K, plus 0 and a value big enough that
    // `MBs * MAX_MB_RATE` overflows `int` (which C does in int arithmetic).
    let mb_counts = [0i32, 16, 396, 1620, 8160, 32_640, 129_600, 10_000_000];
    let vbrmax = [0i32, 50, 100, 200, 400, 2000];
    let mut cells = 0usize;
    for &fr in &rates {
        for &mbs in &mb_counts {
            for &vmax in &vbrmax {
                // Bounded by the ENCODER'S CONTRACT, not by taste.
                // `enc_settings.c:110` rejects `target_bit_rate > 100000000`,
                // and `c_parity_rc_qindex::target_bit_rate_contract_is_driven_not_transcribed`
                // proves that bound by driving the real
                // `svt_av1_enc_set_parameter` rather than transcribing the
                // constant — so this list cannot silently drift out of the
                // envelope C can actually be handed.
                //
                // The previous top cell was 4_000_000_000, forty times the
                // maximum, and it was the ONLY diverging cell: there C casts a
                // double past INT_MAX to int, which is UB, and the hardware
                // splits — `cvttsd2si` yields INT_MIN on x86, `fcvtzs` yields
                // INT_MAX on aarch64. The port saturates, so it matched
                // aarch64 and COULD NOT match x86; no port behaviour satisfies
                // that cell on both. Recorded as SUSPECTED-C-BUGS #17, which
                // keeps the measurement; removing it here is removing an
                // out-of-contract input, not an expectation.
                for &br in &[0u32, 1, 1_000, 500_000, 20_000_000, 100_000_000] {
                    let got = svtav1_encoder::port_rc_process::new_framerate(br, mbs, vmax, fr);
                    let want = svtav1_cref::rate_control::new_framerate(
                        &svtav1_cref::rate_control::NewFramerateIn {
                            target_bit_rate: br,
                            num_mbs: mbs,
                            vbrmax_section: vmax,
                            framerate: fr,
                        },
                    );
                    assert_eq!(
                        got.new_framerate.to_bits(),
                        want.new_framerate.to_bits(),
                        "new_framerate(br={br}, mbs={mbs}, vmax={vmax}, fr={fr})"
                    );
                    assert_eq!(
                        got.avg_frame_bandwidth, want.avg_frame_bandwidth,
                        "avg_frame_bandwidth(br={br}, mbs={mbs}, vmax={vmax}, fr={fr})"
                    );
                    assert_eq!(
                        got.max_frame_bandwidth, want.max_frame_bandwidth,
                        "max_frame_bandwidth(br={br}, mbs={mbs}, vmax={vmax}, fr={fr})"
                    );
                    cells += 1;
                }
            }
        }
    }
    let _ = rng.next();
    assert_eq!(cells, rates.len() * mb_counts.len() * vbrmax.len() * 6);
}

/// The `< 0.1` cliff, read off C rather than asserted from the port: a
/// sub-threshold frame rate becomes **30**, not 0.1, which is a 300x jump in
/// `avg_frame_bandwidth`. If a future edit "clamps to 0.1" instead, this goes
/// red with a number that explains itself.
#[test]
fn new_framerate_sub_threshold_jumps_to_30_not_to_the_threshold() {
    let want =
        svtav1_cref::rate_control::new_framerate(&svtav1_cref::rate_control::NewFramerateIn {
            target_bit_rate: 3_000_000,
            num_mbs: 1620,
            vbrmax_section: 100,
            framerate: 0.05,
        });
    assert_eq!(
        want.new_framerate, 30.0,
        "C replaced 0.05 with {}",
        want.new_framerate
    );
    // 3_000_000 / 30 == 100_000. At a 0.1 clamp it would be 30_000_000.
    assert_eq!(want.avg_frame_bandwidth, 100_000);
}

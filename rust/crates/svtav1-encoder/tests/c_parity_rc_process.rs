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

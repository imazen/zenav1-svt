//! `Codec/rc_vbr_cbr.c`'s scalar core (`svtav1-encoder/src/port_rc_vbr_cbr.rs`).
//!
//! **MIXED EVIDENCE, and which is which per test:**
//!
//! * TIER 1 — the eighteen `rc_tables.h` minq tables (4,608 entries) and the
//!   four `BOOST_*` thresholds. Both are `static const` / `#define` in C
//!   HEADERS with no exported symbol, so `ref_rc_minq_table` /
//!   `ref_rc_boost_threshold` read the REAL C arrays and macros and the
//!   port's parser-extracted copies are compared against them. Nothing here
//!   is a transcription agreeing with a transcription.
//! * TIER 4 — every FUNCTION. All of them are `static` in `rc_vbr_cbr.c` with
//!   no exported symbol (`nm -g` on `Bin/Release/libSvtAv1Enc.a` lists only
//!   `svt_aom_dynamic_resize_decision`, `svt_av1_rc_calc_qindex_rate_control`,
//!   `svt_av1_rc_postencode_update{,_gop_const}`,
//!   `svt_av1_rc_process_rate_allocation` and `svt_av1_resize_reset_rc` from
//!   that file, none of which is here). Expected values are literals derived
//!   by hand from the C statements quoted beside them.
//!
//! `find_qindex` and `get_bits_per_mb` sit between the two: their comparands
//! (`svt_av1_convert_qindex_to_q`, `svt_av1_rc_bits_per_mb`) ARE exported and
//! ARE pinned at tier 1 in `c_parity_rc_process.rs`, so only the search /
//! forwarding shape is tier 4 here.

use svtav1_cref::rate_control as cref_rc;
use svtav1_encoder::port_rc_vbr_cbr as vbr;

#[test]
fn minq_tables_match_the_real_c_arrays() {
    for (i, (name, table)) in vbr::minq_tables::ALL_MINQ_TABLES.iter().enumerate() {
        for q in 0..256usize {
            let want = cref_rc::minq_table(i as i32, q as i32);
            assert_eq!(table[q], want, "{name}[{q}]: port {} vs C {want}", table[q]);
        }
    }
    assert_eq!(vbr::minq_tables::ALL_MINQ_TABLES.len(), 18);
}

/// Anti-vacuity for the sweep above: the shim must be indexing real arrays,
/// not returning zeros (many minq entries legitimately ARE 0 at low qindex,
/// so an all-zero read would pass a large part of the sweep).
#[test]
fn minq_table_shim_reads_real_data() {
    // Out of range is the sentinel, so the shim's bounds check is live.
    assert_eq!(cref_rc::minq_table(-1, 0), i32::MIN);
    assert_eq!(cref_rc::minq_table(18, 0), i32::MIN);
    assert_eq!(cref_rc::minq_table(0, 256), i32::MIN);
    // The top of every table is non-zero and the families differ from each
    // other, so a constant-returning shim cannot pass this.
    let tops: Vec<i32> = (0..18).map(|t| cref_rc::minq_table(t, 255)).collect();
    assert!(tops.iter().all(|&v| v > 0), "minq tops read as {tops:?}");
    assert!(
        tops.iter().collect::<std::collections::HashSet<_>>().len() > 1,
        "every table's top entry is identical ({tops:?}) — the shim is not \
         selecting per table"
    );
}

#[test]
fn boost_thresholds_match_the_c_macros() {
    assert_eq!(vbr::BOOST_KF_LOW, cref_rc::boost_threshold(0));
    assert_eq!(vbr::BOOST_KF_HIGH, cref_rc::boost_threshold(1));
    assert_eq!(vbr::BOOST_GF_LOW_TPL_LA, cref_rc::boost_threshold(2));
    assert_eq!(vbr::BOOST_GF_HIGH_TPL_LA, cref_rc::boost_threshold(3));
    assert_eq!(
        cref_rc::boost_threshold(4),
        i32::MIN,
        "the sentinel must be live"
    );
}

/// `ASSIGN_MINQ_TABLE`'s two-axis selection (bit depth AND the C variable
/// name), checked against the C arrays through the shim so a swapped family
/// cannot pass.
#[test]
fn assign_minq_table_selects_the_right_c_array() {
    use svtav1_encoder::port_rc_vbr_cbr::MinqFamily::*;
    // (family, bit_depth) -> the shim's table index, from rc_shims.c's order.
    let cases = [
        (KfLowMotionCqp, 8u8, 0i32),
        (KfLowMotionCqp, 10, 1),
        (KfLowMotionCqp, 12, 2),
        (KfHighMotion, 8, 3),
        (ArfgfLowMotion, 8, 4),
        (ArfgfHighMotion, 8, 5),
        (Inter, 8, 6),
        (Rtc, 8, 7),
        (KfHighMotion, 10, 8),
        (ArfgfLowMotion, 10, 9),
        (ArfgfHighMotion, 10, 10),
        (Inter, 10, 11),
        (Rtc, 10, 12),
        (KfHighMotion, 12, 13),
        (ArfgfLowMotion, 12, 14),
        (ArfgfHighMotion, 12, 15),
        (Inter, 12, 16),
        (Rtc, 12, 17),
    ];
    for (family, bd, idx) in cases {
        let t = vbr::assign_minq_table(bd, family);
        for q in (0..256usize).step_by(17) {
            assert_eq!(
                t[q],
                cref_rc::minq_table(idx, q as i32),
                "assign_minq_table({bd}, {family:?})[{q}]"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// get_active_quality (rc_vbr_cbr.c:30) — TIER 4, hand-derived.
// ---------------------------------------------------------------------------

/// A pair of tiny synthetic tables so the interpolation arithmetic can be
/// checked without any real-table value getting in the way.
fn synth() -> ([i32; 256], [i32; 256]) {
    let mut low = [0i32; 256];
    let mut high = [0i32; 256];
    low[10] = 100;
    high[10] = 200;
    low[20] = 50;
    high[20] = 30; // deliberately INVERTED: qdiff is negative here.
    (low, high)
}

#[test]
fn active_quality_saturates_to_low_motion_above_high() {
    // C: `if (boost > high) return low_motion_minq[q];`
    let (lo, hi) = synth();
    assert_eq!(vbr::get_active_quality(10, 5001, 400, 5000, &lo, &hi), 100);
}

#[test]
fn active_quality_saturates_to_high_motion_below_low() {
    // C: `if (boost < low) return high_motion_minq[q];`
    let (lo, hi) = synth();
    assert_eq!(vbr::get_active_quality(10, 399, 400, 5000, &lo, &hi), 200);
}

#[test]
fn active_quality_at_the_bounds_picks_the_expected_end() {
    let (lo, hi) = synth();
    // boost == high: offset == 0, adjustment == (0*100 + 2300)/4600 == 0 -> low.
    assert_eq!(vbr::get_active_quality(10, 5000, 400, 5000, &lo, &hi), 100);
    // boost == low: offset == gap == 4600,
    // adjustment == (4600*100 + 2300)/4600 == 460_000+2300 = 462_300 / 4600
    //            == 100 (truncating) -> 100 + 100 == 200, the high table.
    assert_eq!(vbr::get_active_quality(10, 400, 400, 5000, &lo, &hi), 200);
}

#[test]
fn active_quality_interpolates_with_c_truncating_division() {
    let (lo, hi) = synth();
    // gap = 4600, boost = 2700 -> offset = 2300, qdiff = 100.
    // adjustment = (2300*100 + 2300) / 4600 = 232_300 / 4600 = 50 (50.5 trunc).
    // Result 100 + 50 = 150. A round-half-up would give 151.
    assert_eq!(vbr::get_active_quality(10, 2700, 400, 5000, &lo, &hi), 150);
}

#[test]
fn active_quality_handles_a_negative_qdiff_asymmetrically() {
    let (lo, hi) = synth();
    // q = 20: low 50, high 30 -> qdiff = -20.
    // gap = 4600, boost = 2700 -> offset = 2300.
    // adjustment = (2300 * -20 + 2300) / 4600 = (-46_000 + 2300) / 4600
    //            = -43_700 / 4600 = -9 (C truncates TOWARD ZERO: -9.5 -> -9,
    //            where a floor would give -10).
    // Result 50 + (-9) = 41.
    assert_eq!(vbr::get_active_quality(20, 2700, 400, 5000, &lo, &hi), 41);
}

/// The direction check, on the REAL tables: a HIGHER boost must select the
/// LOWER-motion (better, i.e. smaller) qindex. Easy to read backwards from the
/// `high - boost` offset, so it gets its own assertion.
#[test]
fn higher_boost_selects_the_low_motion_table() {
    let at_low_boost = vbr::get_kf_active_quality_tpl(vbr::BOOST_KF_LOW, 200, 8);
    let at_high_boost = vbr::get_kf_active_quality_tpl(vbr::BOOST_KF_HIGH, 200, 8);
    let lo = vbr::assign_minq_table(
        8,
        svtav1_encoder::port_rc_vbr_cbr::MinqFamily::KfLowMotionCqp,
    );
    let hi = vbr::assign_minq_table(8, svtav1_encoder::port_rc_vbr_cbr::MinqFamily::KfHighMotion);
    assert_eq!(
        at_high_boost, lo[200],
        "boost == high must give the LOW-motion entry"
    );
    assert_eq!(
        at_low_boost, hi[200],
        "boost == low must give the HIGH-motion entry"
    );
    assert!(
        lo[200] <= hi[200],
        "the real tables should have low-motion <= high-motion at q=200 \
         ({} vs {})",
        lo[200],
        hi[200]
    );
}

// ---------------------------------------------------------------------------
// The reverse searches (rc_vbr_cbr.c:1641, :1666) — TIER 4.
// ---------------------------------------------------------------------------

#[test]
fn kf_q_tpl_returns_the_start_q_when_already_within_4() {
    // C's loop condition is `abs(target - active) > 4 && ...`, so a target
    // within 4 of the starting active quality exits immediately.
    let start = 150;
    let active = vbr::get_kf_active_quality_tpl(1000, start as usize, 8);
    assert_eq!(vbr::get_kf_q_tpl(start, 1000, active, 8), start);
    assert_eq!(vbr::get_kf_q_tpl(start, 1000, active + 4, 8), start);
    assert_eq!(vbr::get_kf_q_tpl(start, 1000, active - 4, 8), start);
}

#[test]
fn kf_q_tpl_walks_toward_the_target_and_lands_within_4() {
    let start = 200;
    let boost = 1500;
    // Aim at the active quality of a much lower q; the search must walk down.
    let target = vbr::get_kf_active_quality_tpl(boost, 120, 8);
    let got = vbr::get_kf_q_tpl(start, boost, target, 8);
    assert!(
        got < start,
        "expected a downward walk from {start}, got {got}"
    );
    let landed = vbr::get_kf_active_quality_tpl(boost, got.clamp(0, 255) as usize, 8);
    assert!(
        (target - landed).abs() <= 4 || got <= 0,
        "landed at q={got} with active {landed} vs target {target}"
    );
}

#[test]
fn gfu_q_tpl_uses_the_arfgf_tables_not_the_kf_ones() {
    // Same start/boost/target through both entry points must differ, or the
    // ASSIGN_MINQ_TABLE family selection is not being honoured.
    let start = 180;
    let boost = 1000;
    let target = 90;
    let kf = vbr::get_kf_q_tpl(start, boost, target, 8);
    let gfu = vbr::get_gfu_q_tpl(start, boost, target, 8);
    assert_ne!(
        kf, gfu,
        "the KF and ARF/GF reverse searches returned the same q ({kf}) — the \
         minq family is not being selected"
    );
}

/// `prev_dif` is computed ONCE before C's loop and never updated, so the
/// second exit clause is "no worse than the FIRST difference", not "stopped
/// improving". On an unreachable target that clause stays true forever and C
/// walks `q` out of the 0..=255 table domain, indexing out of bounds on every
/// further iteration.
///
/// MEASURED: the port's first faithful transcription of this loop panicked on
/// exactly the call below, with an integer overflow, after ~4 s of walking.
/// That is the runaway, observed rather than argued. The port now stops at the
/// edge of the domain; this test pins that it terminates AND that it stops AT
/// the boundary, so a future "simplification" that drops the guard
/// reintroduces the hang and goes red instead of hanging CI.
#[test]
fn reverse_search_stops_at_the_domain_edge_on_an_unreachable_target() {
    // A target far above anything the tables can produce: the walk goes up.
    assert_eq!(
        vbr::get_kf_q_tpl(200, 1000, 100_000, 8),
        255,
        "expected the walk to stop at the top of the ladder"
    );
    // Far below: the walk goes down.
    assert_eq!(
        vbr::get_gfu_q_tpl(200, 1000, -100_000, 8),
        0,
        "expected the walk to stop at the bottom"
    );
    // A start outside the domain is brought in before the first read.
    assert_eq!(vbr::get_kf_q_tpl(1_000, 1000, 100_000, 8), 255);
    assert_eq!(vbr::get_kf_q_tpl(-1_000, 1000, -100_000, 8), 0);
}

// ---------------------------------------------------------------------------
// find_qindex (rc_vbr_cbr.c:1772) — the search shape is tier 4, the value it
// compares is tier-1 pinned elsewhere.
// ---------------------------------------------------------------------------

#[test]
fn find_qindex_is_the_inverse_of_convert_qindex_to_q() {
    use svtav1_encoder::rate_control::convert_qindex_to_q;
    for &bd in &[8u8, 10] {
        for q in 0..=255i32 {
            let target = convert_qindex_to_q(q, bd);
            let found = vbr::find_qindex(target, bd, 0, 255);
            // C returns the SMALLEST qindex whose q is >= desired_q. The
            // ladder has repeated values, so `found <= q` and the value at
            // `found` must equal the value at `q`.
            assert!(found <= q, "find_qindex overshot: {found} > {q} at bd={bd}");
            assert_eq!(
                convert_qindex_to_q(found, bd),
                target,
                "find_qindex({target}, {bd}) = {found}, whose q differs from q={q}'s"
            );
        }
    }
}

#[test]
fn find_qindex_respects_its_bounds_and_saturates_at_the_top() {
    // Above the top of the ladder there is no qindex with q >= desired, so C
    // walks `low` all the way to `worst_qindex` and returns it.
    assert_eq!(vbr::find_qindex(1.0e9, 8, 0, 255), 255);
    // Below the bottom, the first candidate already satisfies it.
    assert_eq!(vbr::find_qindex(-1.0, 8, 0, 255), 0);
    // Narrow bounds are honoured.
    assert_eq!(vbr::find_qindex(1.0e9, 8, 40, 60), 60);
    assert_eq!(vbr::find_qindex(-1.0, 8, 40, 60), 40);
    assert_eq!(vbr::find_qindex(0.0, 8, 128, 128), 128);
}

/// `find_qindex` and `find_qindex_by_rate` are near-identical binary searches
/// with the comparison INVERTED (q rises with qindex; bits fall with it).
/// Copying one into the other flips the answer, so prove they really do move
/// in opposite directions on the same ladder.
#[test]
fn find_qindex_and_find_qindex_by_rate_move_in_opposite_directions() {
    use svtav1_encoder::port_rc_process::{INTER_FRAME, find_qindex_by_rate, rc_bits_per_mb};
    use svtav1_encoder::rate_control::convert_qindex_to_q;
    // Raising the desired q must RAISE the returned qindex.
    let a = vbr::find_qindex(convert_qindex_to_q(60, 8), 8, 0, 255);
    let b = vbr::find_qindex(convert_qindex_to_q(180, 8), 8, 0, 255);
    assert!(a < b, "find_qindex is not monotone increasing ({a} vs {b})");
    // Raising the desired BITS must LOWER the returned qindex.
    let c = find_qindex_by_rate(
        rc_bits_per_mb(INTER_FRAME, 60, 1.0, 8, false),
        8,
        INTER_FRAME,
        false,
        0,
        255,
    );
    let d = find_qindex_by_rate(
        rc_bits_per_mb(INTER_FRAME, 180, 1.0, 8, false),
        8,
        INTER_FRAME,
        false,
        0,
        255,
    );
    assert!(c < d, "find_qindex_by_rate sanity ({c} vs {d})");
    // And the two searches are genuinely different functions on the same input
    // domain: feeding find_qindex the BITS number lands somewhere else.
    assert_ne!(
        vbr::find_qindex(
            f64::from(rc_bits_per_mb(INTER_FRAME, 60, 1.0, 8, false)),
            8,
            0,
            255
        ),
        c
    );
}

// ---------------------------------------------------------------------------
// The small clamps — TIER 4.
// ---------------------------------------------------------------------------

#[test]
fn clamp_iframe_target_size_skips_the_pct_clamp_at_zero() {
    // C: `if (rc_cfg->max_intra_bitrate_pct) { ... }` — 0 means "no clamp",
    // NOT "0% of the bandwidth".
    assert_eq!(
        vbr::clamp_iframe_target_size(1000, 1_000_000, 0, 999_999),
        999_999
    );
    // 50% of 1000 == 500, so the target is pulled down to 500.
    assert_eq!(
        vbr::clamp_iframe_target_size(1000, 1_000_000, 50, 999_999),
        500
    );
    // The pct clamp cannot RAISE the target.
    assert_eq!(
        vbr::clamp_iframe_target_size(1000, 1_000_000, 500, 100),
        100
    );
    // The max_frame_bandwidth clamp applies unconditionally afterwards.
    assert_eq!(vbr::clamp_iframe_target_size(1000, 300, 0, 999_999), 300);
    assert_eq!(vbr::clamp_iframe_target_size(1000, 300, 500, 999_999), 300);
}

#[test]
fn get_bits_per_mb_forwards_to_the_tier1_rate_model() {
    use svtav1_encoder::port_rc_process::{INTER_FRAME, KEY_FRAME, rc_bits_per_mb};
    for &ft in &[KEY_FRAME, INTER_FRAME] {
        for &sc in &[false, true] {
            for q in (0..=255i32).step_by(11) {
                assert_eq!(
                    vbr::get_bits_per_mb(ft, sc, 8, 0.7, q),
                    rc_bits_per_mb(ft, q, 0.7, 8, sc)
                );
            }
        }
    }
}

/// `adjust_q_cbr`'s content-change arm uses `tanh`, a libm call. Pin the sign
/// behaviour (which is what the surrounding C branch keys on) rather than an
/// exact value, and record that the exact value is host-libm-dependent.
#[test]
fn cbr_content_change_qdelta_signs_follow_the_distortion_ratio() {
    // delta < 0 (distortion falling) -> q_adj_factor < 1 -> a NEGATIVE qdelta,
    // which is what C's comment ("push Q downwards") describes.
    let falling = vbr::cbr_content_change_qdelta(500, 1000, 200, 8);
    assert!(falling < 0, "expected a downward qdelta, got {falling}");
    // delta == 0 -> factor exactly 1.0 -> no change.
    assert_eq!(vbr::cbr_content_change_qdelta(1000, 1000, 200, 8), 0);
    // delta > 0 -> factor > 1 -> a positive qdelta.
    let rising = vbr::cbr_content_change_qdelta(2000, 1000, 200, 8);
    assert!(rising > 0, "expected an upward qdelta, got {rising}");
}

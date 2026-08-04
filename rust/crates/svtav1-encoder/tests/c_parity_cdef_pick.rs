//! Differential parity: the CDEF strength-from-QP picker vs C float
//! semantics (`svt_pick_cdef_from_qp` replicated in the cref shim against
//! the library's real `svt_aom_ac_quant_qtx`).
//!
//! C has three arms (enc_cdef.c:837/:845/:853). Two are reachable on this
//! port's all-intra KEY-frame path and both are swept here over every
//! qindex at bd8 and bd10:
//!
//! - INTRA (`sc_class5 == 0`): four f32 polynomial fits + `roundf`.
//! - SCREEN (`sc_class5 == 1`): four DIFFERENT f64 fits with a TRUNCATING
//!   `(int32_t)` cast and no rounding at all.
//!
//! C selects between them with
//! `const uint8_t sc = allintra ? ppcs->sc_class5 : ppcs->sc_class1;`
//! (enc_cdef.c:916) on the `use_qp_strength` fast path (:912).
//!
//! These sweeps pin the Rust translation (evaluation order, float WIDTH,
//! rounding vs truncation, AC-table input) to the C result for every
//! reachable qindex.

use svtav1_cref as cref;
use svtav1_encoder::cdef::pick_cdef_params_key_frame;

#[test]
fn qp_strength_picker_matches_c_for_all_qindexes() {
    for qindex in 0..=255u8 {
        let ours = pick_cdef_params_key_frame(qindex, 8, false);
        let (cy, cuv) = cref::pick_cdef_from_qp_intra_8bit(qindex);
        assert_eq!(
            (ours.y_strength as i32, ours.uv_strength as i32),
            (cy, cuv),
            "picker diverges from C at qindex {qindex}"
        );
    }
}

/// bd10 differential (task #94): the same fit against the library's real
/// `svt_aom_ac_quant_qtx(qindex, 0, EB_TEN_BIT)` with C's `q >>= (bd - 8)`
/// normalization. Proves the port's bd10 CDEF-from-QP header derivation is
/// C-exact for every qindex — the frame-header component of bd10 identity.
#[test]
fn qp_strength_picker_matches_c_for_all_qindexes_bd10() {
    for qindex in 0..=255u8 {
        let ours = pick_cdef_params_key_frame(qindex, 10, false);
        let (cy, cuv) = cref::pick_cdef_from_qp_intra(qindex, 10);
        assert_eq!(
            (ours.y_strength as i32, ours.uv_strength as i32),
            (cy, cuv),
            "bd10 picker diverges from C at qindex {qindex}"
        );
    }
}

/// Screen-content arm (item G1.6), bd8 + bd10, every qindex.
///
/// This is the arm C takes on `sc_class5` frames at the `use_qp_strength`
/// presets (allintra M7+ by `cdef_search_level == 10`; screen detection is
/// force-disabled at M8+, enc_handle.c:4641-4651 — so M7 exactly at a
/// default config, M8-M13 under a screen-forcing tune). It uses four
/// different quadratics AND truncates where the intra arm rounds, so it is
/// a genuinely different frame-header value, not a rounding nicety.
#[test]
fn qp_strength_picker_screen_arm_matches_c_for_all_qindexes() {
    for bit_depth in [8u8, 10] {
        for qindex in 0..=255u8 {
            let ours = pick_cdef_params_key_frame(qindex, bit_depth, true);
            let (cy, cuv) = cref::pick_cdef_from_qp_screen(qindex, bit_depth);
            assert_eq!(
                (ours.y_strength as i32, ours.uv_strength as i32),
                (cy, cuv),
                "screen-arm picker diverges from C at qindex {qindex} bd{bit_depth}"
            );
        }
    }
}

/// ANTI-VACUITY guard for the sweep above: the screen arm must actually be
/// a different function from the intra arm, or the new test would keep
/// passing with the `is_screen_content` flag ignored.
///
/// MEASURED against the C shim: the two arms disagree at 252/256 bd8
/// qindexes and 251/256 bd10 qindexes. The floor asserted here (200) is far
/// below the measurement but far above anything a stale/ignored flag could
/// produce (which is 0), so the gate fails loudly if the arm selection is
/// dropped while staying robust to a future C fit-constant tweak.
#[test]
fn screen_and_intra_arms_disagree_on_most_qindexes() {
    for (bit_depth, measured) in [(8u8, 252usize), (10, 251)] {
        let mut c_differs = 0usize;
        let mut port_differs = 0usize;
        for qindex in 0..=255u8 {
            let (sy, suv) = cref::pick_cdef_from_qp_screen(qindex, bit_depth);
            let (iy, iuv) = cref::pick_cdef_from_qp(qindex, bit_depth, false, true);
            if (sy, suv) != (iy, iuv) {
                c_differs += 1;
            }
            let ps = pick_cdef_params_key_frame(qindex, bit_depth, true);
            let pi = pick_cdef_params_key_frame(qindex, bit_depth, false);
            if (ps.y_strength, ps.uv_strength) != (pi.y_strength, pi.uv_strength) {
                port_differs += 1;
            }
        }
        assert_eq!(
            c_differs, measured,
            "C arm-divergence count changed at bd{bit_depth} (was {measured})"
        );
        assert_eq!(
            port_differs, c_differs,
            "port must reproduce C's arm divergence exactly at bd{bit_depth}"
        );
        assert!(
            c_differs > 200,
            "the two arms must be materially different at bd{bit_depth}"
        );
    }
}

/// The screen arm's structural signature vs the intra arm, pinned against
/// C so a silent fallback to the intra fit cannot pass.
///
/// C-derived facts (all confirmed by the shim in this test):
/// - the screen arm has a NON-ZERO chroma floor at qindex 0 — its `uv_f2`
///   intercept is `+1.17022324`, which truncates to 1, where the intra
///   arm's `+0.00228092` rounds to 0;
/// - its `uv_f1` quadratic is negative enough that chroma collapses back
///   to 0 at qindex 255, where the intra arm still signals 3.
#[test]
fn screen_arm_structural_signature_matches_c() {
    for bit_depth in [8u8, 10] {
        // Chroma floor at the bottom of the qindex range.
        let (_, suv0) = cref::pick_cdef_from_qp_screen(0, bit_depth);
        let (_, iuv0) = cref::pick_cdef_from_qp(0, bit_depth, false, true);
        assert_eq!(suv0, 1, "screen chroma floor at qindex 0, bd{bit_depth}");
        assert_eq!(iuv0, 0, "intra chroma is 0 at qindex 0, bd{bit_depth}");

        // Chroma collapse at the top.
        let (sy255, suv255) = cref::pick_cdef_from_qp_screen(255, bit_depth);
        let (iy255, iuv255) = cref::pick_cdef_from_qp(255, bit_depth, false, true);
        assert_eq!((sy255, suv255), (60, 0), "screen top, bd{bit_depth}");
        assert_eq!((iy255, iuv255), (63, 3), "intra top, bd{bit_depth}");

        // ...and the port reproduces all four.
        let p0 = pick_cdef_params_key_frame(0, bit_depth, true);
        let p255 = pick_cdef_params_key_frame(255, bit_depth, true);
        assert_eq!(p0.uv_strength as i32, suv0);
        assert_eq!((p255.y_strength as i32, p255.uv_strength as i32), (60, 0));
    }
}

/// The inter arm (enc_cdef.c:845-852) exists in C but is unreachable from
/// this all-intra still encoder (`frame_type` is always KEY_FRAME, so C's
/// `is_intra` is always 1 and `pick_cdef_params_key_frame` has no inter
/// path). Pin the shim's inter arm as genuinely distinct so a future
/// inter port has a live oracle, and so this file documents WHY the port
/// only has two arms.
#[test]
fn inter_arm_is_distinct_and_unported() {
    let mut differs = 0usize;
    for qindex in 0..=255u8 {
        let inter = cref::pick_cdef_from_qp(qindex, 8, false, false);
        let intra = cref::pick_cdef_from_qp(qindex, 8, false, true);
        if inter != intra {
            differs += 1;
        }
    }
    assert!(
        differs > 200,
        "C's inter arm must be a materially different fit ({differs} qindexes differ)"
    );
}

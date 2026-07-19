//! Differential parity: the CDEF strength-from-QP picker vs C float
//! semantics (`svt_pick_cdef_from_qp` intra branch replicated in the cref
//! shim against the library's real `svt_aom_ac_quant_qtx`).
//!
//! The formula is four f32 polynomial fits + roundf; this pins the Rust f32
//! translation (evaluation order, rounding mode, AC-table input) to the C
//! result for every reachable qindex.

use svtav1_cref as cref;
use svtav1_encoder::cdef::pick_cdef_params_key_frame;

#[test]
fn qp_strength_picker_matches_c_for_all_qindexes() {
    for qindex in 0..=255u8 {
        let ours = pick_cdef_params_key_frame(qindex, 8);
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
        let ours = pick_cdef_params_key_frame(qindex, 10);
        let (cy, cuv) = cref::pick_cdef_from_qp_intra(qindex, 10);
        assert_eq!(
            (ours.y_strength as i32, ours.uv_strength as i32),
            (cy, cuv),
            "bd10 picker diverges from C at qindex {qindex}"
        );
    }
}

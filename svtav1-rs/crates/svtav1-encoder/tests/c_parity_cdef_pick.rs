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
        let ours = pick_cdef_params_key_frame(qindex);
        let (cy, cuv) = cref::pick_cdef_from_qp_intra_8bit(qindex);
        assert_eq!(
            (ours.y_strength as i32, ours.uv_strength as i32),
            (cy, cuv),
            "picker diverges from C at qindex {qindex}"
        );
    }
}

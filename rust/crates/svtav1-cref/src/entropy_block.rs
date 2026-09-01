//! FFI bindings for the PER-BLOCK bitstream-emission oracle — the pieces of
//! `write_modes_b` / `svt_aom_write_modes_sb` (`Source/Lib/Codec/entropy_coding.c`)
//! the wx-entropy lane ports.
//!
//! Backed by `shims/entropy_block_shims.c`. That file's header comment states,
//! per entry point, whether it is **tier 1** (a call into a real exported
//! symbol, i.e. the release archive's compiled code) or **tier 1-header** (a
//! `static INLINE` whose source text this TU compiles, which is the C source
//! but not the archive's copy of it). See `docs/WORKING-ON-THIS.md` §4.
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with a concurrent lane.

unsafe extern "C" {
    fn ref_eb_is_masked_compound_type(comp_type: i32) -> i32;
    fn ref_eb_wedge_params_bits(bsize: i32) -> i32;
    fn ref_eb_wedge_bits_lookup(bsize: i32) -> i32;
    fn ref_eb_is_interintra_wedge_used(bsize: i32) -> i32;
    fn ref_eb_is_comp_ref_allowed(bsize: i32) -> i32;
    fn ref_eb_is_interinter_compound_used(comp_type: i32, bsize: i32) -> i32;
    fn ref_eb_is_any_masked_compound_used(bsize: i32) -> i32;
}

/// C `svt_aom_is_masked_compound_type` (inter_prediction.c:34). Tier 1.
pub fn is_masked_compound_type(comp_type: i32) -> bool {
    unsafe { ref_eb_is_masked_compound_type(comp_type) != 0 }
}

/// C `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053). Tier 1.
pub fn wedge_params_bits(bsize: i32) -> i32 {
    unsafe { ref_eb_wedge_params_bits(bsize) }
}

/// C `svt_aom_get_wedge_bits_lookup` (inter_prediction.c:2019). Tier 1.
pub fn wedge_bits_lookup(bsize: i32) -> i32 {
    unsafe { ref_eb_wedge_bits_lookup(bsize) }
}

/// C `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015). Tier 1.
pub fn is_interintra_wedge_used(bsize: i32) -> bool {
    unsafe { ref_eb_is_interintra_wedge_used(bsize) != 0 }
}

/// C `is_comp_ref_allowed` (inter_prediction.h:284). Tier 1-header.
pub fn is_comp_ref_allowed(bsize: i32) -> bool {
    unsafe { ref_eb_is_comp_ref_allowed(bsize) != 0 }
}

/// C `is_interinter_compound_used` (inter_prediction.h:288). Tier 1-header.
pub fn is_interinter_compound_used(comp_type: i32, bsize: i32) -> bool {
    unsafe { ref_eb_is_interinter_compound_used(comp_type, bsize) != 0 }
}

/// C `is_any_masked_compound_used` (inter_prediction.h:303). Tier 1-header.
pub fn is_any_masked_compound_used(bsize: i32) -> bool {
    unsafe { ref_eb_is_any_masked_compound_used(bsize) != 0 }
}

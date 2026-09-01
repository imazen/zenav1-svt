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

/// The eleven `MacroBlockD` fields `set_mi_row_col` writes, plus the mi
/// offset it derives. A neighbour of `None` is C's `NULL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiRowCol {
    pub mb_to_top_edge: i32,
    pub mb_to_bottom_edge: i32,
    pub mb_to_left_edge: i32,
    pub mb_to_right_edge: i32,
    pub up_available: bool,
    pub left_available: bool,
    pub above_mi: Option<usize>,
    pub left_mi: Option<usize>,
    pub n8_w: u8,
    pub n8_h: u8,
    pub is_sec_rect: bool,
    pub mi_offset: usize,
}

unsafe extern "C" {
    fn ref_eb_set_mi_row_col(
        mi_row: i32,
        bh: i32,
        mi_col: i32,
        bw: i32,
        mi_stride: i32,
        mi_rows: i32,
        mi_cols: i32,
        tile_mi_row_start: i32,
        tile_mi_col_start: i32,
        out: *mut i32,
    ) -> i32;
}

/// C `set_mi_row_col` (entropy_coding.c:4681). Tier 1 — exported.
///
/// Returns `None` only if the shim could not allocate; that is an
/// environment failure, and a caller should treat it as one rather than as
/// a parity result.
#[allow(clippy::too_many_arguments)]
pub fn set_mi_row_col(
    mi_row: i32,
    bh: i32,
    mi_col: i32,
    bw: i32,
    mi_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    tile_mi_row_start: i32,
    tile_mi_col_start: i32,
) -> Option<MiRowCol> {
    let mut out = [0i32; 12];
    let rc = unsafe {
        ref_eb_set_mi_row_col(
            mi_row,
            bh,
            mi_col,
            bw,
            mi_stride,
            mi_rows,
            mi_cols,
            tile_mi_row_start,
            tile_mi_col_start,
            out.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return None;
    }
    Some(MiRowCol {
        mb_to_top_edge: out[0],
        mb_to_bottom_edge: out[1],
        mb_to_left_edge: out[2],
        mb_to_right_edge: out[3],
        up_available: out[4] != 0,
        left_available: out[5] != 0,
        above_mi: (out[6] >= 0).then(|| out[6] as usize),
        left_mi: (out[7] >= 0).then(|| out[7] as usize),
        n8_w: out[8] as u8,
        n8_h: out[9] as u8,
        is_sec_rect: out[10] != 0,
        mi_offset: out[11] as usize,
    })
}

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

// ---------------------------------------------------------------------------
// The small EXPORTED helpers of entropy_coding.c — all tier 1.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_eb_partition_cdf_length(bsize: i32) -> i32;
    fn ref_eb_allow_palette(allow_sc: i32, bsize: i32) -> i32;
    fn ref_eb_palette_bsize_ctx(bsize: i32) -> i32;
    fn ref_eb_write_uniform_cost(n: i32, v: i32) -> i32;
    fn ref_eb_count_primitive_quniform(n: i32, v: i32) -> i32;
    fn ref_eb_count_primitive_subexpfin(n: i32, k: i32, v: i32) -> i32;
    fn ref_eb_uleb_size_in_bytes(value: u64) -> u64;
    fn ref_eb_uleb_encode(value: u64, available: u64, out: *mut u8, out_size: *mut u64) -> i32;
    fn ref_eb_get_skip_context(
        above_valid: i32,
        above_skip: i32,
        left_valid: i32,
        left_skip: i32,
    ) -> i32;
    fn ref_eb_get_palette_mode_ctx(
        above_valid: i32,
        above_pal: i32,
        left_valid: i32,
        left_pal: i32,
    ) -> i32;
    fn ref_eb_get_kf_y_mode_ctx(
        up_available: i32,
        up_mode: i32,
        left_available: i32,
        left_mode: i32,
        out: *mut i32,
    );
    fn ref_eb_wb_run(ops: *const i32, n_ops: i32, buf: *mut u8, cap: i32, aligned: *mut i32)
    -> u32;
}

/// C `svt_aom_partition_cdf_length` (entropy_coding.c:922). Tier 1.
pub fn partition_cdf_length(bsize: i32) -> i32 {
    unsafe { ref_eb_partition_cdf_length(bsize) }
}

/// C `svt_aom_allow_palette` (entropy_coding.c:4223). Tier 1.
pub fn allow_palette(allow_screen_content_tools: bool, bsize: i32) -> bool {
    unsafe { ref_eb_allow_palette(i32::from(allow_screen_content_tools), bsize) != 0 }
}

/// C `svt_aom_get_palette_bsize_ctx` (entropy_coding.c:4228). Tier 1.
pub fn palette_bsize_ctx(bsize: i32) -> i32 {
    unsafe { ref_eb_palette_bsize_ctx(bsize) }
}

/// C `svt_aom_write_uniform_cost` (entropy_coding.c:4308). Tier 1.
pub fn write_uniform_cost(n: i32, v: i32) -> i32 {
    unsafe { ref_eb_write_uniform_cost(n, v) }
}

/// C `svt_aom_count_primitive_quniform` (entropy_coding.c:2896). Tier 1.
pub fn count_primitive_quniform(n: i32, v: i32) -> i32 {
    unsafe { ref_eb_count_primitive_quniform(n, v) }
}

/// C `svt_aom_count_primitive_subexpfin` (entropy_coding.c:2952). Tier 1.
pub fn count_primitive_subexpfin(n: i32, k: i32, v: i32) -> i32 {
    unsafe { ref_eb_count_primitive_subexpfin(n, k, v) }
}

/// C `svt_aom_uleb_size_in_bytes` (entropy_coding.c:1310). Tier 1.
pub fn uleb_size_in_bytes(value: u64) -> u64 {
    unsafe { ref_eb_uleb_size_in_bytes(value) }
}

/// C `svt_aom_uleb_encode` (entropy_coding.c:1318). Tier 1.
///
/// Returns `Err(rc)` with C's negative return code when C refuses, else the
/// coded bytes.
pub fn uleb_encode(value: u64, available: u64) -> Result<Vec<u8>, i32> {
    let mut buf = [0u8; 16];
    let mut size: u64 = 0;
    let rc = unsafe { ref_eb_uleb_encode(value, available, buf.as_mut_ptr(), &mut size) };
    if rc != 0 {
        return Err(rc);
    }
    Ok(buf[..size as usize].to_vec())
}

/// C `av1_get_skip_context` (entropy_coding.c:983). Tier 1.
///
/// `None` is C's NULL neighbour pointer.
pub fn get_skip_context(above_skip: Option<bool>, left_skip: Option<bool>) -> i32 {
    unsafe {
        ref_eb_get_skip_context(
            i32::from(above_skip.is_some()),
            i32::from(above_skip.unwrap_or(false)),
            i32::from(left_skip.is_some()),
            i32::from(left_skip.unwrap_or(false)),
        )
    }
}

/// C `svt_aom_get_palette_mode_ctx` (entropy_coding.c:4240). Tier 1.
pub fn get_palette_mode_ctx(above_pal: Option<u8>, left_pal: Option<u8>) -> i32 {
    unsafe {
        ref_eb_get_palette_mode_ctx(
            i32::from(above_pal.is_some()),
            i32::from(above_pal.unwrap_or(0)),
            i32::from(left_pal.is_some()),
            i32::from(left_pal.unwrap_or(0)),
        )
    }
}

/// C `svt_aom_get_kf_y_mode_ctx` (entropy_coding.c:1004). Tier 1.
///
/// Returns `(above_ctx, left_ctx)`. `None` is C's `!up_available` /
/// `!left_available`.
pub fn get_kf_y_mode_ctx(up_mode: Option<u8>, left_mode: Option<u8>) -> (u8, u8) {
    let mut out = [0i32; 2];
    unsafe {
        ref_eb_get_kf_y_mode_ctx(
            i32::from(up_mode.is_some()),
            i32::from(up_mode.unwrap_or(0)),
            i32::from(left_mode.is_some()),
            i32::from(left_mode.unwrap_or(0)),
            out.as_mut_ptr(),
        );
    }
    (out[0] as u8, out[1] as u8)
}

/// One scripted op for [`wb_run`].
#[derive(Clone, Copy, Debug)]
pub enum WbOp {
    /// C `svt_aom_wb_write_bit`.
    Bit(bool),
    /// C `svt_aom_wb_write_literal(data, bits)`.
    Literal { data: i32, bits: i32 },
    /// C `svt_aom_wb_write_inv_signed_literal(data, bits)`.
    InvSigned { data: i32, bits: i32 },
}

/// Drive C's `AomWriteBitBuffer` over a scripted op list. Tier 1.
///
/// Returns `(bytes, bytes_written, is_byte_aligned)` — the produced buffer
/// truncated to `svt_aom_wb_bytes_written`, that count, and
/// `svt_aom_wb_is_byte_aligned`.
pub fn wb_run(ops: &[WbOp], cap: usize) -> (Vec<u8>, u32, bool) {
    let mut flat: Vec<i32> = Vec::with_capacity(ops.len() * 3);
    for op in ops {
        match *op {
            WbOp::Bit(b) => flat.extend_from_slice(&[0, i32::from(b), 0]),
            WbOp::Literal { data, bits } => flat.extend_from_slice(&[1, data, bits]),
            WbOp::InvSigned { data, bits } => flat.extend_from_slice(&[2, data, bits]),
        }
    }
    let mut buf = vec![0u8; cap];
    let mut aligned = 0i32;
    let written = unsafe {
        ref_eb_wb_run(
            flat.as_ptr(),
            ops.len() as i32,
            buf.as_mut_ptr(),
            cap as i32,
            &mut aligned,
        )
    };
    buf.truncate(written as usize);
    (buf, written, aligned != 0)
}

// ---------------------------------------------------------------------------
// svt_aom_get_txb_ctx — tier 1, exported.
// ---------------------------------------------------------------------------

/// What C `svt_aom_get_txb_ctx` produced, plus the unit counts it derived on
/// the way (which the port's caller must reproduce as its slice lengths).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxbCtx {
    /// C `*txb_skip_ctx`.
    pub txb_skip_ctx: i32,
    /// C `*dc_sign_ctx`.
    pub dc_sign_ctx: i32,
    /// C `txb_w_unit` — `tx_size_wide_unit` clipped at the frame's right edge.
    pub txb_w_unit: i32,
    /// C `txb_h_unit`.
    pub txb_h_unit: i32,
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_eb_get_txb_ctx(
        plane: i32,
        tx_size: i32,
        plane_bsize: i32,
        aligned_width: i32,
        aligned_height: i32,
        blk_org_x: i32,
        blk_org_y: i32,
        top: *const u8,
        top_len: i32,
        left: *const u8,
        left_len: i32,
        out: *mut i32,
    ) -> i32;
}

/// C `svt_aom_get_txb_ctx` (entropy_coding.c:248). Tier 1 — exported.
///
/// `top` / `left` are the neighbour bytes AT the block origin; the shim
/// places them where C's `na_top_ptr_pu(na, blk_org_x)` will find them.
/// `None` only on an allocation failure inside the shim, which is an
/// environment failure rather than a parity result.
#[allow(clippy::too_many_arguments)]
pub fn get_txb_ctx(
    plane: i32,
    tx_size: i32,
    plane_bsize: i32,
    aligned_width: i32,
    aligned_height: i32,
    blk_org_x: i32,
    blk_org_y: i32,
    top: &[u8],
    left: &[u8],
) -> Option<TxbCtx> {
    let mut out = [0i32; 4];
    let rc = unsafe {
        ref_eb_get_txb_ctx(
            plane,
            tx_size,
            plane_bsize,
            aligned_width,
            aligned_height,
            blk_org_x,
            blk_org_y,
            top.as_ptr(),
            top.len() as i32,
            left.as_ptr(),
            left.len() as i32,
            out.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return None;
    }
    Some(TxbCtx {
        txb_skip_ctx: out[0],
        dc_sign_ctx: out[1],
        txb_w_unit: out[2],
        txb_h_unit: out[3],
    })
}

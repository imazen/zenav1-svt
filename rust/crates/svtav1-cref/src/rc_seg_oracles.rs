// ---- Segmentation oracles (G2.4, 2026-08-03) ----
//
// Every entry point below drives an EXPORTED C symbol
// (`nm libSvtAv1Enc.a` shows all of them as `T`), so these are the strongest
// evidence tier this project has. The vertical's `static` members
// (`get_variance_for_cu`, `roi_map_setup_segmentation`,
// `roi_map_apply_segmentation_based_quantization`, `encode_segmentation`,
// `svt_aom_get_segment_id`, `write_inter_segment_id`) have no symbol and are
// covered indirectly (through these callers) or by hand-derived vectors.

unsafe extern "C" {
    pub(super) fn ref_segmentation_feature_tables(
        bits: *mut i32,
        is_signed: *mut i32,
        maxv: *mut i32,
    );
    pub(super) fn ref_neg_interleave(x: i32, r: i32, max: i32) -> i32;
    pub(super) fn ref_calculate_segmentation_data(
        feature_enabled_flat: *const i16,
        last_active_seg_id: *mut u8,
        seg_id_pre_skip: *mut u8,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_setup_segmentation(
        aq_mode: u8,
        variance: *const u16,
        b64_total_count: u32,
        block_count: u32,
        enabled: *mut u8,
        update_map: *mut u8,
        temporal_update: *mut u8,
        update_data: *mut u8,
        feature_data_flat: *mut i16,
        feature_enabled_flat: *mut i16,
        last_active_seg_id: *mut u8,
        seg_id_pre_skip: *mut u8,
        variance_bin_edge: *mut i16,
    );
    pub(super) fn ref_apply_segmentation_based_quantization(
        variance_bin_edge: *const i16,
        feature_data_flat: *const i16,
        base_q_idx: i32,
        variance: *const u16,
        b64_total_count: u32,
        block_count: u32,
        sb_index: u32,
        bsize: i32,
        org_x: i32,
        org_y: i32,
    ) -> u8;
    pub(super) fn ref_get_spatial_seg_prediction(
        seg_map: *const u8,
        mi_cols: i32,
        mi_rows: i32,
        mi_row: i32,
        mi_col: i32,
        left_available: i32,
        up_available: i32,
        cdf_index: *mut i32,
    ) -> i32;
    pub(super) fn ref_update_segmentation_map(
        seg_map: *mut u8,
        mi_cols: i32,
        mi_rows: i32,
        bsize: i32,
        mi_row: i32,
        mi_col: i32,
        segment_id: u8,
    );
    pub(super) fn ref_wb_write_inv_signed_literal(data: i32, bits: i32, out_bits: *mut u8) -> u32;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_write_segment_id(
        seg_map: *mut u8,
        mi_cols: i32,
        mi_rows: i32,
        bsize: i32,
        mi_row: i32,
        mi_col: i32,
        left_available: i32,
        up_available: i32,
        last_active_seg_id: u8,
        segment_id: u8,
        skip_coeff: i32,
        out_bytes: *mut u8,
        out_cdf: *mut u16,
        out_segment_id: *mut u8,
    ) -> u32;
}

/// C `SEG_LVL_MAX` — the width of the three feature tables.
pub const SEG_LVL_MAX: usize = 8;
/// C `MAX_SEGMENTS`.
pub const MAX_SEGMENTS: usize = 8;

/// The three exported const tables from `segmentation_params.c:16-21`, read
/// out of the linked library: `(bits, signed, max)`.
pub fn segmentation_feature_tables() -> ([i32; SEG_LVL_MAX], [i32; SEG_LVL_MAX], [i32; SEG_LVL_MAX])
{
    let mut bits = [0i32; SEG_LVL_MAX];
    let mut sgn = [0i32; SEG_LVL_MAX];
    let mut maxv = [0i32; SEG_LVL_MAX];
    unsafe {
        ref_segmentation_feature_tables(bits.as_mut_ptr(), sgn.as_mut_ptr(), maxv.as_mut_ptr())
    };
    (bits, sgn, maxv)
}

/// C `svt_av1_neg_interleave` (entropy_coding.c:4825) — exported symbol.
pub fn neg_interleave(x: i32, reference: i32, max: i32) -> i32 {
    unsafe { ref_neg_interleave(x, reference, max) }
}

/// C `calculate_segmentation_data` (segmentation.c:249) — exported symbol.
/// Takes the INCOMING `(last_active_seg_id, seg_id_pre_skip)` so the
/// accumulate-don't-clear behaviour is observable, and returns the outgoing
/// pair.
pub fn calculate_segmentation_data(
    feature_enabled: &[[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    last_active_seg_id: u8,
    seg_id_pre_skip: u8,
) -> (u8, u8) {
    let mut last = last_active_seg_id;
    let mut pre = seg_id_pre_skip;
    unsafe {
        ref_calculate_segmentation_data(
            feature_enabled.as_flattened().as_ptr(),
            &mut last,
            &mut pre,
        )
    };
    (last, pre)
}

/// The `SegmentationParams` fields `svt_aom_setup_segmentation` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefSegmentationParams {
    pub enabled: bool,
    pub update_map: bool,
    pub temporal_update: bool,
    pub update_data: bool,
    pub feature_data: [[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    pub feature_enabled: [[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    pub last_active_seg_id: u8,
    pub seg_id_pre_skip: u8,
    pub variance_bin_edge: [i16; MAX_SEGMENTS],
}

/// C `svt_aom_setup_segmentation` (segmentation.c:228) — exported symbol,
/// non-ROI arm (the shim leaves `ppcs->roi_map_evt` NULL). Drives
/// `find_segment_qps` + `calculate_segmentation_data` end to end.
///
/// `variance` is `b64_total_count` rows of `block_count` samples, matching
/// the `EB_MALLOC_2D(variance, b64_total_count, block_count)` shape
/// (pcs.c:1280).
pub fn setup_segmentation(
    aq_mode: u8,
    variance: &[u16],
    b64_total_count: u32,
    block_count: u32,
) -> RefSegmentationParams {
    assert_eq!(variance.len(), (b64_total_count * block_count) as usize);
    let mut enabled = 0u8;
    let mut update_map = 0u8;
    let mut temporal_update = 0u8;
    let mut update_data = 0u8;
    let mut feature_data = [[0i16; SEG_LVL_MAX]; MAX_SEGMENTS];
    let mut feature_enabled = [[0i16; SEG_LVL_MAX]; MAX_SEGMENTS];
    let mut last_active_seg_id = 0u8;
    let mut seg_id_pre_skip = 0u8;
    let mut variance_bin_edge = [0i16; MAX_SEGMENTS];
    unsafe {
        ref_setup_segmentation(
            aq_mode,
            variance.as_ptr(),
            b64_total_count,
            block_count,
            &mut enabled,
            &mut update_map,
            &mut temporal_update,
            &mut update_data,
            feature_data.as_flattened_mut().as_mut_ptr(),
            feature_enabled.as_flattened_mut().as_mut_ptr(),
            &mut last_active_seg_id,
            &mut seg_id_pre_skip,
            variance_bin_edge.as_mut_ptr(),
        )
    };
    RefSegmentationParams {
        enabled: enabled != 0,
        update_map: update_map != 0,
        temporal_update: temporal_update != 0,
        update_data: update_data != 0,
        feature_data,
        feature_enabled,
        last_active_seg_id,
        seg_id_pre_skip,
        variance_bin_edge,
    }
}

/// C `svt_aom_apply_segmentation_based_quantization` (segmentation.c:136) —
/// exported symbol, non-ROI arm. Returns the assigned `segment_id`. This is
/// the only reachable driver for the `static` `get_variance_for_cu`.
///
/// `variance` is the WHOLE contiguous plane (`b64_total_count * block_count`
/// samples) laid out exactly as `EB_MALLOC_2D` does, because C's BLOCK_16X8
/// index arithmetic reads PAST the selected b64's row into the next one —
/// see the shim's comment and `svtav1_encoder::segmentation`'s doc.
/// `bsize` is the raw `BlockSize` enum value.
#[allow(clippy::too_many_arguments)]
pub fn apply_segmentation_based_quantization(
    variance_bin_edge: &[i16; MAX_SEGMENTS],
    feature_data: &[[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    base_q_idx: i32,
    variance: &[u16],
    b64_total_count: u32,
    block_count: u32,
    sb_index: u32,
    bsize: i32,
    org_x: i32,
    org_y: i32,
) -> u8 {
    assert_eq!(variance.len(), (b64_total_count * block_count) as usize);
    assert!(sb_index < b64_total_count);
    unsafe {
        ref_apply_segmentation_based_quantization(
            variance_bin_edge.as_ptr(),
            feature_data.as_flattened().as_ptr(),
            base_q_idx,
            variance.as_ptr(),
            b64_total_count,
            block_count,
            sb_index,
            bsize,
            org_x,
            org_y,
        )
    }
}

/// C `svt_av1_get_spatial_seg_prediction` (entropy_coding.c:4777) — exported
/// symbol. Returns `(prediction, cdf_index)`.
pub fn get_spatial_seg_prediction(
    seg_map: &[u8],
    mi_cols: i32,
    mi_rows: i32,
    mi_row: i32,
    mi_col: i32,
    left_available: bool,
    up_available: bool,
) -> (i32, i32) {
    assert_eq!(seg_map.len(), (mi_cols * mi_rows) as usize);
    let mut cdf_index = -1i32;
    let pred = unsafe {
        ref_get_spatial_seg_prediction(
            seg_map.as_ptr(),
            mi_cols,
            mi_rows,
            mi_row,
            mi_col,
            i32::from(left_available),
            i32::from(up_available),
            &mut cdf_index,
        )
    };
    (pred, cdf_index)
}

/// C `svt_aom_wb_write_inv_signed_literal` (entropy_coding.c:1377) —
/// exported symbol. Returns the emitted bits, MSB first, one `u8` per bit.
/// This is the only nontrivial primitive inside the `static`
/// `encode_segmentation`.
pub fn wb_write_inv_signed_literal(data: i32, bits: i32) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    let n = unsafe { ref_wb_write_inv_signed_literal(data, bits, out.as_mut_ptr()) };
    out.truncate(n as usize);
    out
}

/// What C's `write_segment_id` produced for one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefWriteSegmentId {
    /// Coded payload from the real range coder.
    pub bytes: Vec<u8>,
    /// The segmentation map after the call (write_segment_id stamps the
    /// block's mi footprint).
    pub seg_map: Vec<u8>,
    /// `FRAME_CONTEXT.seg.spatial_pred_seg_cdf` after adaptation, flattened.
    pub spatial_pred_seg_cdf: Vec<u16>,
    /// `mbmi->segment_id` after the call — C OVERWRITES it with the spatial
    /// prediction on the `skip_coeff` path.
    pub segment_id: u8,
}

/// C `write_segment_id` (entropy_coding.c:4867) — exported symbol, driven
/// through the real range coder and the real `g_fc.seg` CDFs.
///
/// Call [`fc_init`] first (and hold the caller's `fc_init` mutex): the CDFs
/// come from the process-global C frame context and this call ADAPTS them.
#[allow(clippy::too_many_arguments)]
pub fn write_segment_id(
    seg_map: &[u8],
    mi_cols: i32,
    mi_rows: i32,
    bsize: i32,
    mi_row: i32,
    mi_col: i32,
    left_available: bool,
    up_available: bool,
    last_active_seg_id: u8,
    segment_id: u8,
    skip_coeff: bool,
) -> RefWriteSegmentId {
    assert_eq!(seg_map.len(), (mi_cols * mi_rows) as usize);
    let mut map = seg_map.to_vec();
    let mut bytes = vec![0u8; 1024];
    // SPATIAL_PREDICTION_PROBS * CDF_SIZE(MAX_SEGMENTS) = 3 * 9
    let mut cdf = vec![0u16; 3 * (MAX_SEGMENTS + 1)];
    let mut out_id = 0u8;
    let n = unsafe {
        ref_write_segment_id(
            map.as_mut_ptr(),
            mi_cols,
            mi_rows,
            bsize,
            mi_row,
            mi_col,
            i32::from(left_available),
            i32::from(up_available),
            last_active_seg_id,
            segment_id,
            i32::from(skip_coeff),
            bytes.as_mut_ptr(),
            cdf.as_mut_ptr(),
            &mut out_id,
        )
    };
    bytes.truncate(n as usize);
    RefWriteSegmentId {
        bytes,
        seg_map: map,
        spatial_pred_seg_cdf: cdf,
        segment_id: out_id,
    }
}

/// C `svt_av1_update_segmentation_map` (entropy_coding.c:4847) — exported
/// symbol. Stamps in place and returns the mutated map.
pub fn update_segmentation_map(
    seg_map: &[u8],
    mi_cols: i32,
    mi_rows: i32,
    bsize: i32,
    mi_row: i32,
    mi_col: i32,
    segment_id: u8,
) -> Vec<u8> {
    assert_eq!(seg_map.len(), (mi_cols * mi_rows) as usize);
    let mut out = seg_map.to_vec();
    unsafe {
        ref_update_segmentation_map(
            out.as_mut_ptr(),
            mi_cols,
            mi_rows,
            bsize,
            mi_row,
            mi_col,
            segment_id,
        )
    };
    out
}

// ---- MV entropy encode oracle (AUDIT 2026-07-14) ----

unsafe extern "C" {
    pub(super) fn ref_get_mv_class(z: i32, offset: *mut i32) -> i32;
    pub(super) fn ref_encode_mv_seq(
        mv_y: *const i32,
        mv_x: *const i32,
        ref_y: *const i32,
        ref_x: *const i32,
        n: i32,
        precision: i32,
        out: *mut u8,
        cap: u32,
    ) -> u32;
}

// ---- Temporal-filter noise estimator oracle (AUDIT 2026-07-14) ----

unsafe extern "C" {
    pub(super) fn ref_estimate_noise_fp16(
        src: *const u8,
        width: u16,
        height: u16,
        y_stride: u16,
    ) -> i32;
}

/// Reference `svt_estimate_noise_fp16_c` (temporal_filtering.c): FP16
/// noise-level estimate of a luma plane (Sobel edge rejection + Laplacian),
/// or `-65536` (-1 in fp16) if too few smooth pixels.
pub fn estimate_noise_fp16(src: &[u8], width: usize, height: usize, y_stride: usize) -> i32 {
    assert!(src.len() >= (height - 1) * y_stride + width);
    unsafe { ref_estimate_noise_fp16(src.as_ptr(), width as u16, height as u16, y_stride as u16) }
}

/// Reference `svt_av1_get_mv_class(z)`: returns `(class, offset)`.
pub fn get_mv_class(z: i32) -> (i32, i32) {
    let mut offset = 0i32;
    let c = unsafe { ref_get_mv_class(z, &mut offset) };
    (c, offset)
}

/// Reference MV-difference entropy encode of a whole sequence through one
/// adapting `NmvContext` (default CDFs), in C encode order (vertical/Y first).
/// Faithful transcription of `svt_av1_encode_mv` + `encode_mv_component`
/// driving the real `svt_av1_get_mv_class` + `aom_write_symbol`. `precision`
/// is the `MvSubpelPrecision` int (-1 none, 0 low, 1 high). Returns the
/// finalized od_ec byte stream.
pub fn encode_mv_seq(mvs: &[(i16, i16)], refs: &[(i16, i16)], precision: i32) -> Vec<u8> {
    assert_eq!(mvs.len(), refs.len());
    let n = mvs.len();
    let mv_y: Vec<i32> = mvs.iter().map(|m| m.1 as i32).collect();
    let mv_x: Vec<i32> = mvs.iter().map(|m| m.0 as i32).collect();
    let ref_y: Vec<i32> = refs.iter().map(|m| m.1 as i32).collect();
    let ref_x: Vec<i32> = refs.iter().map(|m| m.0 as i32).collect();
    let mut out = vec![0u8; 4096];
    let nbytes = unsafe {
        ref_encode_mv_seq(
            mv_y.as_ptr(),
            mv_x.as_ptr(),
            ref_y.as_ptr(),
            ref_x.as_ptr(),
            n as i32,
            precision,
            out.as_mut_ptr(),
            out.len() as u32,
        )
    };
    assert!(
        nbytes as usize <= out.len(),
        "MV seq exceeded oracle buffer"
    );
    out.truncate(nbytes as usize);
    out
}

// ---- Scan orders + coefficient-context helpers ----

unsafe extern "C" {
    pub(super) fn ref_scan_len(tx_size: i32) -> i32;
    pub(super) fn ref_scan_copy(tx_size: i32, scan_class: i32, scan_out: *mut i16, len: i32);
    pub(super) fn ref_tx_type_to_scan_index(tx_type: i32) -> i32;
    pub(super) fn ref_get_br_ctx(levels: *const u8, c: i32, bwl: i32, tx_class: i32) -> i32;
    pub(super) fn ref_get_eob_pos_token(eob: i32, extra: *mut i32) -> i32;
    pub(super) fn ref_nz_map_ctx_offset(tx_size: i32, coeff_idx: i32) -> i32;
    pub(super) fn ref_txb_init_levels(coeff: *const i32, width: i32, height: i32, levels: *mut u8);
    pub(super) fn ref_get_nz_map_contexts(
        levels: *const u8,
        scan: *const i16,
        eob: u16,
        tx_size: i32,
        tx_class: i32,
        coeff_contexts: *mut i8,
    );
    #[cfg(target_arch = "x86_64")]
    pub(super) fn ref_get_nz_map_contexts_sse2(
        levels: *const u8,
        scan: *const i16,
        eob: u16,
        tx_size: i32,
        tx_class: i32,
        coeff_contexts: *mut i8,
    );
    pub(super) fn ref_get_txsize_entropy_ctx(tx_size: i32) -> i32;
    pub(super) fn ref_get_txb_bwl(tx_size: i32) -> i32;
    pub(super) fn ref_get_txb_wide(tx_size: i32) -> i32;
    pub(super) fn ref_get_txb_high(tx_size: i32) -> i32;
}

/// Number of coefficients scanned for `tx_size` (adjusted dimensions).
pub fn scan_len(tx_size: usize) -> usize {
    unsafe { ref_scan_len(tx_size as i32) as usize }
}

/// Copy the reference scan order for (tx_size, scan_class 0..3).
pub fn scan(tx_size: usize, scan_class: usize) -> Vec<i16> {
    let len = scan_len(tx_size);
    let mut v = vec![0i16; len];
    unsafe {
        ref_scan_copy(
            tx_size as i32,
            scan_class as i32,
            v.as_mut_ptr(),
            len as i32,
        )
    };
    v
}

pub fn tx_type_to_scan_index(tx_type: usize) -> usize {
    unsafe { ref_tx_type_to_scan_index(tx_type as i32) as usize }
}

pub fn get_br_ctx(levels: &[u8], c: usize, bwl: usize, tx_class: usize) -> i32 {
    unsafe { ref_get_br_ctx(levels.as_ptr(), c as i32, bwl as i32, tx_class as i32) }
}

pub fn get_eob_pos_token(eob: i32) -> (i32, i32) {
    let mut extra = 0i32;
    let t = unsafe { ref_get_eob_pos_token(eob, &mut extra) };
    (t, extra)
}

pub fn nz_map_ctx_offset(tx_size: usize, coeff_idx: usize) -> i32 {
    unsafe { ref_nz_map_ctx_offset(tx_size as i32, coeff_idx as i32) }
}

pub fn txb_init_levels(coeff: &[i32], width: usize, height: usize, levels: &mut [u8]) {
    unsafe {
        ref_txb_init_levels(
            coeff.as_ptr(),
            width as i32,
            height as i32,
            levels.as_mut_ptr(),
        )
    };
}

pub fn get_nz_map_contexts(
    levels: &[u8],
    scan: &[i16],
    eob: u16,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    unsafe {
        ref_get_nz_map_contexts(
            levels.as_ptr(),
            scan.as_ptr(),
            eob,
            tx_size as i32,
            tx_class as i32,
            coeff_contexts.as_mut_ptr(),
        )
    };
}

/// The production RTCD-default SIMD kernel `svt_av1_get_nz_map_contexts_sse2`:
/// fills the whole padded block in raster order, then stamps the eob position.
/// Byte-identical to [`get_nz_map_contexts`] at every scan position, but also
/// writes the non-scan positions (raster), so it is the reference for the
/// port's raster fill on the *full* buffer.
///
/// x86_64 ONLY — this wraps an SSE2 kernel that does not exist on other
/// architectures. It was declared unconditionally, so on arm64 the entire
/// `zenav1-svt-entropy` c_parity test binary failed to LINK (undefined
/// `_svt_av1_get_nz_map_contexts_sse2`) and NO entropy parity test ran there at
/// all. Gating it by architecture is what lets the rest of that suite run on
/// arm64; the callers are gated to match.
#[cfg(target_arch = "x86_64")]
pub fn get_nz_map_contexts_sse2(
    levels: &[u8],
    scan: &[i16],
    eob: u16,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    unsafe {
        ref_get_nz_map_contexts_sse2(
            levels.as_ptr(),
            scan.as_ptr(),
            eob,
            tx_size as i32,
            tx_class as i32,
            coeff_contexts.as_mut_ptr(),
        )
    };
}

pub fn txsize_entropy_ctx(tx_size: usize) -> usize {
    unsafe { ref_get_txsize_entropy_ctx(tx_size as i32) as usize }
}
pub fn txb_bwl(tx_size: usize) -> usize {
    unsafe { ref_get_txb_bwl(tx_size as i32) as usize }
}
pub fn txb_wide(tx_size: usize) -> usize {
    unsafe { ref_get_txb_wide(tx_size as i32) as usize }
}
pub fn txb_high(tx_size: usize) -> usize {
    unsafe { ref_get_txb_high(tx_size as i32) as usize }
}

// ---- AV1 quantizer step tables ----

unsafe extern "C" {
    pub(super) fn ref_dc_quant_qtx(qindex: i32) -> i16;
    pub(super) fn ref_ac_quant_qtx(qindex: i32) -> i16;
    pub(super) fn ref_dc_quant_qtx_bd(qindex: i32, bd: i32) -> i16;
    pub(super) fn ref_ac_quant_qtx_bd(qindex: i32, bd: i32) -> i16;
}

/// Reference `svt_aom_dc_quant_qtx(qindex, 0, 8-bit)`.
pub fn dc_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_dc_quant_qtx(qindex) }
}

/// Reference `svt_aom_ac_quant_qtx(qindex, 0, 8-bit)`.
pub fn ac_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_ac_quant_qtx(qindex) }
}

/// Reference `svt_aom_dc_quant_qtx(qindex, 0, bd)` for `bd` in {8, 10, 12}
/// (the `EbBitDepth` values). Backs the bd10 qlookup-table FFI check.
pub fn dc_quant_qtx_bd(qindex: i32, bd: i32) -> i16 {
    unsafe { ref_dc_quant_qtx_bd(qindex, bd) }
}

/// Reference `svt_aom_ac_quant_qtx(qindex, 0, bd)` for `bd` in {8, 10, 12}.
pub fn ac_quant_qtx_bd(qindex: i32, bd: i32) -> i16 {
    unsafe { ref_ac_quant_qtx_bd(qindex, bd) }
}

// ---- variance-boost helper wrappers (rc_aq.c, exported in both modes) ----

unsafe extern "C" {
    pub(super) fn svt_av1_convert_qindex_to_q_fp8(qindex: i32, bit_depth: i32) -> i32;
    pub(super) fn svt_av1_compute_qdelta_fp(
        qstart_fp8: i32,
        qtarget_fp8: i32,
        bit_depth: i32,
    ) -> i32;
}

/// C `svt_av1_convert_qindex_to_q_fp8`. `bit_depth` is the EbBitDepth enum
/// value (8/10/12).
pub fn convert_qindex_to_q_fp8(qindex: i32, bit_depth: i32) -> i32 {
    unsafe { svt_av1_convert_qindex_to_q_fp8(qindex, bit_depth) }
}

// ---- video-mode qindex derivation (rc_process.c, exported) ----
//
// These two are what the STILL path never calls: `cqp_qindex_calc` returns
// early when `scs->allintra`, so a video-mode key frame's qindex comes from
// here and a still frame's does not. Bound for the inter campaign's C1a.

unsafe extern "C" {
    pub(super) fn svt_av1_compute_qdelta(qstart: f64, qtarget: f64, bit_depth: i32) -> i32;
    pub(super) fn svt_av1_convert_qindex_to_q(qindex: i32, bit_depth: i32) -> f64;
    pub(super) fn svt_av1_get_q_index_from_qstep_ratio(
        leaf_qindex: i32,
        qstep_ratio: f64,
        bit_depth: i32,
    ) -> i32;
}

/// C `svt_av1_convert_qindex_to_q` (rc_process.c:186). `bit_depth` is the
/// EbBitDepth enum value (8/10/12).
#[must_use]
pub fn convert_qindex_to_q(qindex: i32, bit_depth: i32) -> f64 {
    unsafe { svt_av1_convert_qindex_to_q(qindex, bit_depth) }
}

/// C `svt_av1_compute_qdelta` (rc_process.c:201).
#[must_use]
pub fn compute_qdelta(qstart: f64, qtarget: f64, bit_depth: i32) -> i32 {
    unsafe { svt_av1_compute_qdelta(qstart, qtarget, bit_depth) }
}

/// C `svt_av1_get_q_index_from_qstep_ratio` (rc_process.c:322).
#[must_use]
pub fn get_q_index_from_qstep_ratio(leaf_qindex: i32, qstep_ratio: f64, bit_depth: i32) -> i32 {
    unsafe { svt_av1_get_q_index_from_qstep_ratio(leaf_qindex, qstep_ratio, bit_depth) }
}

/// C `svt_av1_compute_qdelta_fp`.
pub fn compute_qdelta_fp(qstart_fp8: i32, qtarget_fp8: i32, bit_depth: i32) -> i32 {
    unsafe { svt_av1_compute_qdelta_fp(qstart_fp8, qtarget_fp8, bit_depth) }
}

// ---- AC-bias wrappers (ac_bias.c, exported; feature code in both modes) ----

unsafe extern "C" {
    pub(super) fn ref_psy_distortion(
        input: *const u8,
        input_stride: u32,
        recon: *const u8,
        recon_stride: u32,
        width: u32,
        height: u32,
    ) -> u64;
    pub(super) fn svt_psy_adjust_rate_light(
        coeff: *const i32,
        coeff_bits: u64,
        width: u32,
        height: u32,
        ac_bias: f64,
    ) -> u64;
    pub(super) fn get_effective_ac_bias(
        ac_bias: f64,
        is_islice: bool,
        temporal_layer_index: u8,
    ) -> f64;
}

/// C `svt_psy_distortion` (8-bit).
pub fn psy_distortion(
    input: &[u8],
    input_stride: u32,
    recon: &[u8],
    recon_stride: u32,
    width: u32,
    height: u32,
) -> u64 {
    unsafe {
        ref_psy_distortion(
            input.as_ptr(),
            input_stride,
            recon.as_ptr(),
            recon_stride,
            width,
            height,
        )
    }
}

/// C `svt_psy_adjust_rate_light`.
pub fn psy_adjust_rate_light(
    coeff: &[i32],
    coeff_bits: u64,
    width: u32,
    height: u32,
    ac_bias: f64,
) -> u64 {
    unsafe { svt_psy_adjust_rate_light(coeff.as_ptr(), coeff_bits, width, height, ac_bias) }
}

/// C `get_effective_ac_bias`.
pub fn effective_ac_bias(ac_bias: f64, is_islice: bool, temporal_layer_index: u8) -> f64 {
    unsafe { get_effective_ac_bias(ac_bias, is_islice, temporal_layer_index) }
}

// ---- picture-analysis sub-sampled mean producers (pic_analysis_process.c) ----

unsafe extern "C" {
    pub(super) fn svt_compute_sub_mean_8x8_c(input_samples: *const u8, input_stride: u16) -> u64;
    pub(super) fn svt_aom_compute_sub_mean_squared_values_c(
        input_samples: *const u8,
        input_stride: u32,
        input_area_width: u32,
        input_area_height: u32,
    ) -> u64;
}

/// C `svt_compute_sub_mean_8x8_c` (fp8 sub-sampled 8x8 mean).
pub fn sub_mean_8x8(block: &[u8], stride: u16) -> u64 {
    unsafe { svt_compute_sub_mean_8x8_c(block.as_ptr(), stride) }
}

/// C `svt_aom_compute_sub_mean_squared_values_c` (fp16 sub-sampled mean of squares).
pub fn sub_mean_squared_8x8(block: &[u8], stride: u32) -> u64 {
    unsafe { svt_aom_compute_sub_mean_squared_values_c(block.as_ptr(), stride, 8, 8) }
}

unsafe extern "C" {
    pub(super) fn ref_normalize_sb_delta_q(
        base_q_idx: u8,
        delta_q_res: u8,
        qindexes: *mut u8,
        sb_count: u16,
    );
}

/// C `svt_av1_normalize_sb_delta_q` (rc_aq.c:830) driven over a shim-built
/// `PictureControlSet` shell. The ONE definition in the C tree (outside every
/// `#if SVT_HDR_MODE` block), so it is the oracle for both port arms.
///
/// `sb_qindex` is rewritten in place with the normalized values. `delta_q_res`
/// must be 2, 4 or 8 (C asserts it).
pub fn normalize_sb_delta_q(base_q_idx: u8, delta_q_res: u8, sb_qindex: &mut [u8]) {
    assert!(
        matches!(delta_q_res, 2 | 4 | 8),
        "C asserts delta_q_res in {{2,4,8}}"
    );
    let n = u16::try_from(sb_qindex.len()).expect("sb_count fits u16 (C sb_total_count is u16)");
    unsafe { ref_normalize_sb_delta_q(base_q_idx, delta_q_res, sb_qindex.as_mut_ptr(), n) }
}

unsafe extern "C" {
    pub(super) fn ref_noise_normalization(
        dequant_dc: i16,
        dequant_ac: i16,
        coeff: *const i32,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        eob: *mut u16,
        tx_size: i32,
        tx_type: i32,
        strength: u8,
    );
}

/// Fork `svt_av1_perform_noise_normalization` via a minimal-struct shim
/// (no QM). Buffers are packed rasters like the quantizer's.
#[allow(clippy::too_many_arguments)]
pub fn noise_normalization(
    dequant: [i16; 2],
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    tx_size: i32,
    tx_type: i32,
    strength: u8,
) {
    unsafe {
        ref_noise_normalization(
            dequant[0],
            dequant[1],
            coeff.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            eob,
            tx_size,
            tx_type,
            strength,
        )
    }
}

unsafe extern "C" {
    pub(super) fn ref_spatial_facade(
        input: *const u8,
        input_stride: u32,
        recon: *const u8,
        recon_stride: u32,
        width: u32,
        height: u32,
        mode: u8,
        uv_mode: u8,
        is_chroma: u8,
        is_interintra: u8,
        comp_type: u8,
        temporal_layer_index: u8,
        ac_bias: f64,
        tx_bias: u8,
    ) -> u64;
}

/// Fork mds0 distortion facade (`svt_spatial_full_distortion_kernel_facade`)
/// driven with a synthetic BlockModeInfo. 8-bit, offsets 0.
#[allow(clippy::too_many_arguments)]
pub fn spatial_facade(
    input: &[u8],
    input_stride: u32,
    recon: &[u8],
    recon_stride: u32,
    width: u32,
    height: u32,
    mode: u8,
    uv_mode: u8,
    is_chroma: bool,
    is_interintra: bool,
    comp_type: u8,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> u64 {
    unsafe {
        ref_spatial_facade(
            input.as_ptr(),
            input_stride,
            recon.as_ptr(),
            recon_stride,
            width,
            height,
            mode,
            uv_mode,
            u8::from(is_chroma),
            u8::from(is_interintra),
            comp_type,
            temporal_layer_index,
            ac_bias,
            tx_bias,
        )
    }
}

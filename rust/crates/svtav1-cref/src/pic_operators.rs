//! Reference bindings for `Codec/pic_operators.c`, `Codec/deblocking_common.c`
//! and the residual entry points of `Codec/intra_prediction.c`.
//!
//! Every symbol reached here is an exported symbol of `libSvtAv1Enc.a`
//! (verified with `nm -g`), so these drive the real C code — evidence tier 1
//! in `docs/WORKING-ON-THIS.md` §4. The `ref_*` wrappers live in
//! `shims/picops_dblk_shims.c`, which also performs the one-shot RTCD and
//! intra-predictor-table init the dispatched entry points need; see that
//! file's header for why (`svt_spatial_full_distortion_kernel` and
//! `svt_aom_eb_pred[][]` are null .bss slots on BOTH ISAs until init runs).

/// `MAX_SEGMENTS` (definitions.h:1689).
pub const MAX_SEGMENTS: usize = 8;
/// `SEG_LVL_MAX` (segmentation_params.h) — number of segment features.
pub const SEG_LVL_MAX: usize = 8;
/// `REF_FRAMES` (definitions.h).
pub const REF_FRAMES: usize = 8;
/// `MAX_MODE_LF_DELTAS` (definitions.h).
pub const MAX_MODE_LF_DELTAS: usize = 2;
/// `MAX_PLANES`.
pub const MAX_PLANES: usize = 3;

/// Flattened `LoopFilterInfoN::lvl[plane][seg][dir][ref][mode]`.
pub const LVL_LEN: usize = MAX_PLANES * MAX_SEGMENTS * 2 * REF_FRAMES * MAX_MODE_LF_DELTAS;

unsafe extern "C" {
    fn ref_picops_rtcd_ready() -> i32;
    fn ref_residual_kernel8bit(
        input: *const u8,
        input_stride: u32,
        pred: *const u8,
        pred_stride: u32,
        residual: *mut i16,
        residual_stride: u32,
        area_width: u32,
        area_height: u32,
    );
    fn ref_residual_kernel16bit(
        input: *const u16,
        input_stride: u32,
        pred: *const u16,
        pred_stride: u32,
        residual: *mut i16,
        residual_stride: u32,
        area_width: u32,
        area_height: u32,
    );
    fn ref_full_distortion_kernel32_bits(
        coeff: *const i32,
        recon_coeff: *const i32,
        stride: u32,
        area_width: u32,
        area_height: u32,
        out2: *mut u64,
    );
    fn ref_full_distortion_kernel_cbf_zero32_bits(
        coeff: *const i32,
        coeff_stride: u32,
        area_width: u32,
        area_height: u32,
        out2: *mut u64,
    );
    fn ref_picture_full_distortion32_bits_single(
        coeff: *const i32,
        recon_coeff: *const i32,
        stride: u32,
        bwidth: u32,
        bheight: u32,
        cnt_nz_coeff: u32,
        out2: *mut u64,
    );
    fn ref_spatial_full_distortion_kernel_c(
        input: *const u8,
        input_offset: u32,
        input_stride: u32,
        recon: *const u8,
        recon_offset: i32,
        recon_stride: u32,
        area_width: u32,
        area_height: u32,
    ) -> u64;
    #[allow(clippy::too_many_arguments)]
    fn ref_spatial_full_distortion_kernel_facade(
        input: *const u8,
        input_offset: u32,
        input_stride: u32,
        recon: *const u8,
        recon_offset: i32,
        recon_stride: u32,
        area_width: u32,
        area_height: u32,
        mode: i32,
        uv_mode: i32,
        is_interintra_used: u8,
        compound_type: i32,
        is_chroma: i32,
        temporal_layer_index: u8,
        ac_bias: f64,
        tx_bias: u8,
    ) -> u64;
    #[allow(clippy::too_many_arguments)]
    fn ref_picture_full_distortion32_bits_single_facade(
        coeff: *const i32,
        recon_coeff: *const i32,
        stride: u32,
        bwidth: u32,
        bheight: u32,
        area_width: u32,
        area_height: u32,
        cnt_nz_coeff: u32,
        mode: i32,
        uv_mode: i32,
        is_interintra_used: u8,
        compound_type: i32,
        is_chroma: i32,
        temporal_layer_index: u8,
        ac_bias: f64,
        tx_bias: u8,
        out2: *mut u64,
    );
    fn ref_update_sharpness(sharpness_lvl: i32, lvl: i32, out_lim: *mut u8, out_mblim: *mut u8);
    #[allow(clippy::too_many_arguments)]
    fn ref_get_filter_level_delta_lf(
        filt_lvl4: *const i32,
        sharpness: i32,
        mode_ref_delta_enabled: u8,
        ref_deltas: *const i8,
        mode_deltas: *const i8,
        segmentation_enabled: u8,
        seg_enabled: *const u8,
        seg_data: *const i32,
        delta_lf_multi: u8,
        dir_idx: i32,
        plane: i32,
        sb_delta_lf: *mut i32,
        seg_id: u8,
        pred_mode: i32,
        ref_frame_0: i32,
    ) -> u8;
    #[allow(clippy::too_many_arguments)]
    fn ref_loop_filter_frame_init(
        filt_lvl4: *const i32,
        sharpness: i32,
        mode_ref_delta_enabled: u8,
        ref_deltas: *const i8,
        mode_deltas: *const i8,
        segmentation_enabled: u8,
        seg_enabled: *const u8,
        seg_data: *const i32,
        plane_start: i32,
        plane_end: i32,
        out_lvl: *mut u8,
    );
    fn ref_generate_padding16_bit(
        buf: *mut u16,
        origin: u32,
        src_stride: u32,
        original_src_width: u32,
        original_src_height: u32,
        padding_width: u32,
        padding_height: u32,
    );
    fn ref_pad_input_picture_16bit(
        src: *mut u16,
        src_stride: u32,
        original_src_width: u32,
        original_src_height: u32,
        pad_right: u32,
        pad_bottom: u32,
    );
    fn ref_convert_8bit_to_16bit(
        src: *const u8,
        src_stride: u32,
        dst: *mut u16,
        dst_stride: u32,
        width: u32,
        height: u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_yv12_copy_plane8(
        plane: i32,
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        width: i32,
        height: i32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_yv12_copy_plane16(
        plane: i32,
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        width: i32,
        height: i32,
    );
    fn ref_intra_is_smooth(mode: i32, uv_mode: i32, plane: i32) -> i32;
    fn ref_intra_is_smooth_inter(mode: i32, uv_mode: i32, plane: i32, ref_frame_0: i32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_dr_predictor(
        dst: *mut u8,
        stride: i32,
        tx_size: i32,
        above_data: *const u8,
        left_data: *const u8,
        upsample_above: i32,
        upsample_left: i32,
        angle: i32,
        bw: i32,
        bh: i32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_intra_prediction_open_loop_mb(
        p_angle: i32,
        ois_intra_mode: u8,
        src_origin_x: u32,
        src_origin_y: u32,
        tx_size: i32,
        above_data: *const u8,
        left_data: *const u8,
        dst: *mut u8,
        stride: i32,
        bw: i32,
        bh: i32,
    ) -> i32;
}

/// Positive control for the dispatched entry points: are the RTCD slots and
/// intra-predictor tables this TU calls through actually bound?
///
/// `docs/WORKING-ON-THIS.md` §5 — a silent probe and a genuine absence are
/// indistinguishable, so the tier-1 tests assert this before believing a
/// match. A null slot on x86-64 is a jump to address 0, not a wrong answer.
#[must_use]
pub fn rtcd_ready() -> bool {
    unsafe { ref_picops_rtcd_ready() != 0 }
}

/// The reference `svt_residual_kernel8bit_c`.
#[allow(clippy::too_many_arguments)]
pub fn residual_kernel8bit(
    input: &[u8],
    input_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    residual: &mut [i16],
    residual_stride: usize,
    area_width: usize,
    area_height: usize,
) {
    assert_covers(input.len(), input_stride, area_width, area_height, "input");
    assert_covers(pred.len(), pred_stride, area_width, area_height, "pred");
    assert_covers(
        residual.len(),
        residual_stride,
        area_width,
        area_height,
        "residual",
    );
    unsafe {
        ref_residual_kernel8bit(
            input.as_ptr(),
            input_stride as u32,
            pred.as_ptr(),
            pred_stride as u32,
            residual.as_mut_ptr(),
            residual_stride as u32,
            area_width as u32,
            area_height as u32,
        );
    }
}

/// The reference `svt_residual_kernel16bit_c`.
#[allow(clippy::too_many_arguments)]
pub fn residual_kernel16bit(
    input: &[u16],
    input_stride: usize,
    pred: &[u16],
    pred_stride: usize,
    residual: &mut [i16],
    residual_stride: usize,
    area_width: usize,
    area_height: usize,
) {
    assert_covers(input.len(), input_stride, area_width, area_height, "input");
    assert_covers(pred.len(), pred_stride, area_width, area_height, "pred");
    assert_covers(
        residual.len(),
        residual_stride,
        area_width,
        area_height,
        "residual",
    );
    unsafe {
        ref_residual_kernel16bit(
            input.as_ptr(),
            input_stride as u32,
            pred.as_ptr(),
            pred_stride as u32,
            residual.as_mut_ptr(),
            residual_stride as u32,
            area_width as u32,
            area_height as u32,
        );
    }
}

/// The reference `svt_full_distortion_kernel32_bits_c`; returns
/// `(DIST_CALC_RESIDUAL, DIST_CALC_PREDICTION)`.
pub fn full_distortion_kernel32_bits(
    coeff: &[i32],
    recon_coeff: &[i32],
    stride: usize,
    area_width: usize,
    area_height: usize,
) -> (u64, u64) {
    assert_covers(coeff.len(), stride, area_width, area_height, "coeff");
    assert_covers(recon_coeff.len(), stride, area_width, area_height, "recon");
    let mut out = [0u64; 2];
    unsafe {
        ref_full_distortion_kernel32_bits(
            coeff.as_ptr(),
            recon_coeff.as_ptr(),
            stride as u32,
            area_width as u32,
            area_height as u32,
            out.as_mut_ptr(),
        );
    }
    (out[0], out[1])
}

/// The reference `svt_full_distortion_kernel_cbf_zero32_bits_c`.
pub fn full_distortion_kernel_cbf_zero32_bits(
    coeff: &[i32],
    coeff_stride: usize,
    area_width: usize,
    area_height: usize,
) -> (u64, u64) {
    assert_covers(coeff.len(), coeff_stride, area_width, area_height, "coeff");
    let mut out = [0u64; 2];
    unsafe {
        ref_full_distortion_kernel_cbf_zero32_bits(
            coeff.as_ptr(),
            coeff_stride as u32,
            area_width as u32,
            area_height as u32,
            out.as_mut_ptr(),
        );
    }
    (out[0], out[1])
}

/// The reference `svt_aom_picture_full_distortion32_bits_single` — the
/// DISPATCHED path (RTCD kernels, not the `_c` spellings).
pub fn picture_full_distortion32_bits_single(
    coeff: &[i32],
    recon_coeff: &[i32],
    stride: usize,
    bwidth: usize,
    bheight: usize,
    cnt_nz_coeff: u32,
) -> (u64, u64) {
    assert_covers(coeff.len(), stride, bwidth, bheight, "coeff");
    assert_covers(recon_coeff.len(), stride, bwidth, bheight, "recon");
    let mut out = [0u64; 2];
    unsafe {
        ref_picture_full_distortion32_bits_single(
            coeff.as_ptr(),
            recon_coeff.as_ptr(),
            stride as u32,
            bwidth as u32,
            bheight as u32,
            cnt_nz_coeff,
            out.as_mut_ptr(),
        );
    }
    (out[0], out[1])
}

/// The reference `svt_spatial_full_distortion_kernel_c` (C_DEFAULT).
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_kernel_c(
    input: &[u8],
    input_offset: usize,
    input_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
) -> u64 {
    assert!(input.len() >= input_offset + (area_height - 1) * input_stride + area_width);
    assert!(recon.len() >= recon_offset + (area_height - 1) * recon_stride + area_width);
    unsafe {
        ref_spatial_full_distortion_kernel_c(
            input.as_ptr(),
            input_offset as u32,
            input_stride as u32,
            recon.as_ptr(),
            recon_offset as i32,
            recon_stride as u32,
            area_width as u32,
            area_height as u32,
        )
    }
}

/// Mode-family inputs to the tx-bias facade, as raw C enum values.
#[derive(Debug, Clone, Copy)]
pub struct FacadeMode {
    /// `BlockModeInfo::mode` (PredictionMode).
    pub mode: i32,
    /// `BlockModeInfo::uv_mode` (UvPredictionMode).
    pub uv_mode: i32,
    /// `BlockModeInfo::is_interintra_used`.
    pub is_interintra_used: bool,
    /// `BlockModeInfo::interinter_comp.type` (CompoundType).
    pub compound_type: i32,
}

/// The reference `svt_spatial_full_distortion_kernel_facade` on the
/// `hbd_md = false` arm.
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_kernel_facade(
    input: &[u8],
    input_offset: usize,
    input_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
    mi: FacadeMode,
    is_chroma: bool,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> u64 {
    assert!(input.len() >= input_offset + (area_height - 1) * input_stride + area_width);
    assert!(recon.len() >= recon_offset + (area_height - 1) * recon_stride + area_width);
    unsafe {
        ref_spatial_full_distortion_kernel_facade(
            input.as_ptr(),
            input_offset as u32,
            input_stride as u32,
            recon.as_ptr(),
            recon_offset as i32,
            recon_stride as u32,
            area_width as u32,
            area_height as u32,
            mi.mode,
            mi.uv_mode,
            u8::from(mi.is_interintra_used),
            mi.compound_type,
            i32::from(is_chroma),
            temporal_layer_index,
            ac_bias,
            tx_bias,
        )
    }
}

/// The reference `svt_aom_picture_full_distortion32_bits_single_facade`.
#[allow(clippy::too_many_arguments)]
pub fn picture_full_distortion32_bits_single_facade(
    coeff: &[i32],
    recon_coeff: &[i32],
    stride: usize,
    bwidth: usize,
    bheight: usize,
    area_width: usize,
    area_height: usize,
    cnt_nz_coeff: u32,
    mi: FacadeMode,
    is_chroma: bool,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> (u64, u64) {
    assert_covers(coeff.len(), stride, bwidth, bheight, "coeff");
    assert_covers(recon_coeff.len(), stride, bwidth, bheight, "recon");
    let mut out = [0u64; 2];
    unsafe {
        ref_picture_full_distortion32_bits_single_facade(
            coeff.as_ptr(),
            recon_coeff.as_ptr(),
            stride as u32,
            bwidth as u32,
            bheight as u32,
            area_width as u32,
            area_height as u32,
            cnt_nz_coeff,
            mi.mode,
            mi.uv_mode,
            u8::from(mi.is_interintra_used),
            mi.compound_type,
            i32::from(is_chroma),
            temporal_layer_index,
            ac_bias,
            tx_bias,
            out.as_mut_ptr(),
        );
    }
    (out[0], out[1])
}

/// The reference `svt_aom_update_sharpness`, reported for one level as
/// `(lim, mblim)`.
pub fn update_sharpness(sharpness_lvl: i32, lvl: i32) -> (u8, u8) {
    let (mut lim, mut mblim) = (0u8, 0u8);
    unsafe { ref_update_sharpness(sharpness_lvl, lvl, &raw mut lim, &raw mut mblim) };
    (lim, mblim)
}

/// The frame-header state the two LF-level entry points read.
#[derive(Debug, Clone)]
pub struct LfFrameState {
    /// `filter_level[0]`, `filter_level[1]`, `filter_level_u`, `filter_level_v`.
    pub filter_levels: [i32; 4],
    /// `sharpness_level`.
    pub sharpness: i32,
    /// `mode_ref_delta_enabled`.
    pub mode_ref_delta_enabled: bool,
    /// `ref_deltas[REF_FRAMES]`.
    pub ref_deltas: [i8; REF_FRAMES],
    /// `mode_deltas[MAX_MODE_LF_DELTAS]`.
    pub mode_deltas: [i8; MAX_MODE_LF_DELTAS],
    /// `segmentation_params.segmentation_enabled`.
    pub segmentation_enabled: bool,
    /// `feature_enabled[seg][feature]`.
    pub seg_enabled: [[u8; SEG_LVL_MAX]; MAX_SEGMENTS],
    /// `feature_data[seg][feature]`.
    pub seg_data: [[i32; SEG_LVL_MAX]; MAX_SEGMENTS],
}

impl Default for LfFrameState {
    fn default() -> Self {
        Self {
            filter_levels: [0; 4],
            sharpness: 0,
            mode_ref_delta_enabled: false,
            ref_deltas: [0; REF_FRAMES],
            mode_deltas: [0; MAX_MODE_LF_DELTAS],
            segmentation_enabled: false,
            seg_enabled: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
            seg_data: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
        }
    }
}

impl LfFrameState {
    fn flat_seg(
        &self,
    ) -> (
        [u8; MAX_SEGMENTS * SEG_LVL_MAX],
        [i32; MAX_SEGMENTS * SEG_LVL_MAX],
    ) {
        let mut en = [0u8; MAX_SEGMENTS * SEG_LVL_MAX];
        let mut da = [0i32; MAX_SEGMENTS * SEG_LVL_MAX];
        for s in 0..MAX_SEGMENTS {
            for f in 0..SEG_LVL_MAX {
                en[s * SEG_LVL_MAX + f] = self.seg_enabled[s][f];
                da[s * SEG_LVL_MAX + f] = self.seg_data[s][f];
            }
        }
        (en, da)
    }
}

/// The reference `svt_aom_get_filter_level_delta_lf`.
#[allow(clippy::too_many_arguments)]
pub fn get_filter_level_delta_lf(
    state: &LfFrameState,
    delta_lf_multi: bool,
    dir_idx: i32,
    plane: i32,
    sb_delta_lf: &mut [i32; 4],
    seg_id: u8,
    pred_mode: i32,
    ref_frame_0: i32,
) -> u8 {
    let (en, da) = state.flat_seg();
    unsafe {
        ref_get_filter_level_delta_lf(
            state.filter_levels.as_ptr(),
            state.sharpness,
            u8::from(state.mode_ref_delta_enabled),
            state.ref_deltas.as_ptr(),
            state.mode_deltas.as_ptr(),
            u8::from(state.segmentation_enabled),
            en.as_ptr(),
            da.as_ptr(),
            u8::from(delta_lf_multi),
            dir_idx,
            plane,
            sb_delta_lf.as_mut_ptr(),
            seg_id,
            pred_mode,
            ref_frame_0,
        )
    }
}

/// The reference `svt_av1_loop_filter_frame_init`. `preset` seeds the `lvl`
/// table before the call so untouched cells stay distinguishable.
pub fn loop_filter_frame_init(
    state: &LfFrameState,
    plane_start: i32,
    plane_end: i32,
    preset: u8,
) -> Vec<u8> {
    let (en, da) = state.flat_seg();
    let mut out = vec![preset; LVL_LEN];
    unsafe {
        ref_loop_filter_frame_init(
            state.filter_levels.as_ptr(),
            state.sharpness,
            u8::from(state.mode_ref_delta_enabled),
            state.ref_deltas.as_ptr(),
            state.mode_deltas.as_ptr(),
            u8::from(state.segmentation_enabled),
            en.as_ptr(),
            da.as_ptr(),
            plane_start,
            plane_end,
            out.as_mut_ptr(),
        );
    }
    out
}

/// The reference `svt_aom_generate_padding16_bit`, applied in place to
/// `buf` with C's `src_pic` at `origin` (in u16 elements).
pub fn generate_padding16_bit(
    buf: &mut [u16],
    origin: usize,
    src_stride: usize,
    original_src_width: usize,
    original_src_height: usize,
    padding_width: usize,
    padding_height: usize,
) {
    unsafe {
        ref_generate_padding16_bit(
            buf.as_mut_ptr(),
            origin as u32,
            src_stride as u32,
            original_src_width as u32,
            original_src_height as u32,
            padding_width as u32,
            padding_height as u32,
        );
    }
}

/// The reference `svt_aom_pad_input_picture_16bit`, applied in place.
pub fn pad_input_picture_16bit(
    src: &mut [u16],
    src_stride: usize,
    original_src_width: usize,
    original_src_height: usize,
    pad_right: usize,
    pad_bottom: usize,
) {
    unsafe {
        ref_pad_input_picture_16bit(
            src.as_mut_ptr(),
            src_stride as u32,
            original_src_width as u32,
            original_src_height as u32,
            pad_right as u32,
            pad_bottom as u32,
        );
    }
}

/// The reference `svt_convert_8bit_to_16bit_c`.
pub fn convert_8bit_to_16bit(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    assert_covers(src.len(), src_stride, width, height, "src");
    assert_covers(dst.len(), dst_stride, width, height, "dst");
    unsafe {
        ref_convert_8bit_to_16bit(
            src.as_ptr(),
            src_stride as u32,
            dst.as_mut_ptr(),
            dst_stride as u32,
            width as u32,
            height as u32,
        );
    }
}

/// The reference `svt_aom_yv12_copy_y_c` (plane 0) / `_u_c` (1) / `_v_c` (2)
/// on the 8-bit arm.
pub fn yv12_copy_plane_8(
    plane: usize,
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    assert_covers(src.len(), src_stride, width, height, "src");
    assert_covers(dst.len(), dst_stride, width, height, "dst");
    unsafe {
        ref_yv12_copy_plane8(
            plane as i32,
            src.as_ptr(),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            width as i32,
            height as i32,
        );
    }
}

/// The 16-bit arm of the same three functions (C sets
/// `YV12_FLAG_HIGHBITDEPTH` in `flags`). Strides and width are in u16
/// elements, as C's `y_stride` / `y_width` are once it has taken the
/// `CONVERT_TO_SHORTPTR` branch. The shim stores the plane pointer through
/// `CONVERT_TO_BYTEPTR` — a `Yv12BufferConfig` holds a 16-bit plane as
/// `ptr >> 1`, not as the pointer.
pub fn yv12_copy_plane_16(
    plane: usize,
    src: &[u16],
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    width: usize,
    height: usize,
) {
    assert_covers(src.len(), src_stride, width, height, "src");
    assert_covers(dst.len(), dst_stride, width, height, "dst");
    unsafe {
        ref_yv12_copy_plane16(
            plane as i32,
            src.as_ptr(),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            width as i32,
            height as i32,
        );
    }
}

/// The reference `svt_aom_is_smooth` for an INTRA block.
pub fn intra_is_smooth(mode: i32, uv_mode: i32, plane: i32) -> bool {
    unsafe { ref_intra_is_smooth(mode, uv_mode, plane) != 0 }
}

/// The reference `svt_aom_is_smooth` with an explicit `ref_frame[0]`, which
/// is what `is_inter_block` reads.
pub fn intra_is_smooth_with_ref(mode: i32, uv_mode: i32, plane: i32, ref_frame_0: i32) -> bool {
    unsafe { ref_intra_is_smooth_inter(mode, uv_mode, plane, ref_frame_0) != 0 }
}

/// Length of the edged above/left buffers the two entry points below take,
/// matching C's own `above_data[…]` layout. The block origin is at
/// [`EDGE_ORIGIN`], so index `EDGE_ORIGIN - 1` is the `above_row[-1]`
/// corner sample every zone-2 predictor reads.
pub const EDGE_BUF_LEN: usize = 160;
/// Index of the block origin inside an [`EDGE_BUF_LEN`] buffer (C `+ 16`).
pub const EDGE_ORIGIN: usize = 16;

/// The reference `svt_aom_dr_predictor` — the DISPATCHED path (`angle == 90`
/// and `angle == 180` go through `svt_aom_eb_pred[][]`).
///
/// `above_data` / `left_data` are full [`EDGE_BUF_LEN`] buffers; the shim
/// re-stages them into 64-byte-aligned locals before calling C, so the SIMD
/// kernels get the alignment the encoder's own buffers give them.
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor(
    dst: &mut [u8],
    stride: usize,
    tx_size: i32,
    above_data: &[u8; EDGE_BUF_LEN],
    left_data: &[u8; EDGE_BUF_LEN],
    upsample_above: i32,
    upsample_left: i32,
    angle: i32,
    bw: usize,
    bh: usize,
) {
    assert!(dst.len() >= (bh - 1) * stride + bw);
    assert!(bw <= 64 && bh <= 64);
    unsafe {
        ref_dr_predictor(
            dst.as_mut_ptr(),
            stride as i32,
            tx_size,
            above_data.as_ptr(),
            left_data.as_ptr(),
            upsample_above,
            upsample_left,
            angle,
            bw as i32,
            bh as i32,
        );
    }
}

/// The reference `svt_aom_intra_prediction_open_loop_mb`.
///
/// `src_origin_x`/`src_origin_y` are only tested for `> 0` by C (they select
/// the DC variant), so any positive value stands for "neighbours available".
#[allow(clippy::too_many_arguments)]
pub fn intra_prediction_open_loop_mb(
    p_angle: i32,
    ois_intra_mode: u8,
    src_origin_x: u32,
    src_origin_y: u32,
    tx_size: i32,
    above_data: &[u8; EDGE_BUF_LEN],
    left_data: &[u8; EDGE_BUF_LEN],
    dst: &mut [u8],
    stride: usize,
    bw: usize,
    bh: usize,
) {
    assert!(dst.len() >= (bh - 1) * stride + bw);
    assert!(bw <= 64 && bh <= 64);
    unsafe {
        ref_intra_prediction_open_loop_mb(
            p_angle,
            ois_intra_mode,
            src_origin_x,
            src_origin_y,
            tx_size,
            above_data.as_ptr(),
            left_data.as_ptr(),
            dst.as_mut_ptr(),
            stride as i32,
            bw as i32,
            bh as i32,
        );
    }
}

#[track_caller]
fn assert_covers(len: usize, stride: usize, width: usize, height: usize, what: &str) {
    let need = if height == 0 {
        0
    } else {
        (height - 1) * stride + width
    };
    assert!(
        len >= need,
        "{what}: buffer of {len} cannot cover {width}x{height} at stride {stride} (needs {need})"
    );
}

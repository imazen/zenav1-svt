//! FFI bindings for the temporal-filtering oracle (lane wp-preanalysis).
//!
//! Backed by `shims/tf_shims.c`, which calls the REAL exported C symbols
//! `svt_aom_noise_log1p_fp16`, `tf_use_64x64_pred`,
//! `svt_aom_apply_filtering_central_c`,
//! `svt_aom_apply_filtering_central_highbd_c`,
//! `svt_aom_get_final_filtered_pixels_c`,
//! `svt_av1_apply_temporal_filter_planewise_medium_c` and
//! `svt_av1_apply_temporal_filter_planewise_medium_hbd_c`, plus the real
//! `OD_DIVU` macro over the exported `svt_aom_od_divu_small_consts` table —
//! evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).

/// The flat `MeContext` fields the TF kernels read. Layout must match
/// `TfCtxArgs` in `shims/tf_shims.c`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TfCtxArgs {
    pub tf_block_col: i32,
    pub tf_block_row: i32,
    pub tf_mv_dist_th: u32,
    pub tf_chroma: i32,
    pub tf_32x32_block_split_flag: [i32; 4],
    pub tf_16x16_mv_x: [i16; 16],
    pub tf_16x16_mv_y: [i16; 16],
    pub tf_16x16_block_error: [u64; 16],
    pub tf_32x32_mv_x: [i16; 4],
    pub tf_32x32_mv_y: [i16; 4],
    pub tf_32x32_block_error: [u64; 4],
    pub tf_decay_factor_fp16: [u32; 3],
    pub tf_64x64_block_error: u64,
    pub p_best_sad_64x64: u32,
    pub p_best_sad_32x32: [u32; 4],
    pub tf_use_pred_64x64_only_th: u8,
}

impl Default for TfCtxArgs {
    fn default() -> Self {
        Self {
            tf_block_col: 0,
            tf_block_row: 0,
            tf_mv_dist_th: 10,
            tf_chroma: 1,
            tf_32x32_block_split_flag: [0; 4],
            tf_16x16_mv_x: [0; 16],
            tf_16x16_mv_y: [0; 16],
            tf_16x16_block_error: [0; 16],
            tf_32x32_mv_x: [0; 4],
            tf_32x32_mv_y: [0; 4],
            tf_32x32_block_error: [0; 4],
            tf_decay_factor_fp16: [1 << 16; 3],
            tf_64x64_block_error: 0,
            p_best_sad_64x64: 0,
            p_best_sad_32x32: [0; 4],
            tf_use_pred_64x64_only_th: 0,
        }
    }
}

unsafe extern "C" {
    fn ref_tf_noise_log1p_fp16(noise_level_fp16: i32) -> i32;
    fn ref_tf_od_divu(x: u32, d: u32) -> u32;
    fn ref_tf_use_64x64_pred(a: *const TfCtxArgs) -> i8;
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_apply_filtering_central(
        tf_chroma: i32,
        src_y: *const u8,
        src_u: *const u8,
        src_v: *const u8,
        src_stride_y: u32,
        accum_y: *mut u32,
        accum_u: *mut u32,
        accum_v: *mut u32,
        count_y: *mut u16,
        count_u: *mut u16,
        count_v: *mut u16,
        blk_width: u16,
        blk_height: u16,
        ss_x: u32,
        ss_y: u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_apply_filtering_central_highbd(
        tf_chroma: i32,
        src_y: *const u16,
        src_u: *const u16,
        src_v: *const u16,
        src_stride_y: u32,
        accum_y: *mut u32,
        accum_u: *mut u32,
        accum_v: *mut u32,
        count_y: *mut u16,
        count_u: *mut u16,
        count_v: *mut u16,
        blk_width: u16,
        blk_height: u16,
        ss_x: u32,
        ss_y: u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_get_final_filtered_pixels(
        tf_chroma: i32,
        is_highbd: i32,
        sy: *mut u8,
        su: *mut u8,
        sv: *mut u8,
        hy: *mut u16,
        hu: *mut u16,
        hv: *mut u16,
        accum_y: *const u32,
        accum_u: *const u32,
        accum_v: *const u32,
        count_y: *const u16,
        count_u: *const u16,
        count_v: *const u16,
        stride: *const u32,
        blk_y_src_offset: i32,
        blk_ch_src_offset: i32,
        blk_width_ch: u16,
        blk_height_ch: u16,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_apply_planewise_medium(
        a: *const TfCtxArgs,
        y_src: *const u8,
        y_src_stride: i32,
        y_pre: *const u8,
        y_pre_stride: i32,
        u_src: *const u8,
        v_src: *const u8,
        uv_src_stride: i32,
        u_pre: *const u8,
        v_pre: *const u8,
        uv_pre_stride: i32,
        block_width: u32,
        block_height: u32,
        ss_x: i32,
        ss_y: i32,
        y_accum: *mut u32,
        y_count: *mut u16,
        u_accum: *mut u32,
        u_count: *mut u16,
        v_accum: *mut u32,
        v_count: *mut u16,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_apply_planewise_medium_hbd(
        a: *const TfCtxArgs,
        y_src: *const u16,
        y_src_stride: i32,
        y_pre: *const u16,
        y_pre_stride: i32,
        u_src: *const u16,
        v_src: *const u16,
        uv_src_stride: i32,
        u_pre: *const u16,
        v_pre: *const u16,
        uv_pre_stride: i32,
        block_width: u32,
        block_height: u32,
        ss_x: i32,
        ss_y: i32,
        y_accum: *mut u32,
        y_count: *mut u16,
        u_accum: *mut u32,
        u_count: *mut u16,
        v_accum: *mut u32,
        v_count: *mut u16,
        encoder_bit_depth: u32,
    );
}

/// `svt_aom_noise_log1p_fp16`.
pub fn noise_log1p_fp16(noise_level_fp16: i32) -> i32 {
    unsafe { ref_tf_noise_log1p_fp16(noise_level_fp16) }
}

/// The real `OD_DIVU` macro, over `svt_aom_od_divu_small_consts`.
pub fn od_divu(x: u32, d: u32) -> u32 {
    unsafe { ref_tf_od_divu(x, d) }
}

/// `tf_use_64x64_pred`.
pub fn use_64x64_pred(args: &TfCtxArgs) -> i8 {
    unsafe { ref_tf_use_64x64_pred(args) }
}

/// `svt_aom_apply_filtering_central_c`.
#[allow(clippy::too_many_arguments)]
pub fn apply_filtering_central(
    tf_chroma: bool,
    src: [&[u8]; 3],
    src_stride_y: u32,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
    blk_width: u16,
    blk_height: u16,
    ss_x: u32,
    ss_y: u32,
) {
    let (a0, rest) = accum.split_at_mut(1);
    let (a1, a2) = rest.split_at_mut(1);
    let (c0, crest) = count.split_at_mut(1);
    let (c1, c2) = crest.split_at_mut(1);
    unsafe {
        ref_tf_apply_filtering_central(
            i32::from(tf_chroma),
            src[0].as_ptr(),
            src[1].as_ptr(),
            src[2].as_ptr(),
            src_stride_y,
            a0[0].as_mut_ptr(),
            a1[0].as_mut_ptr(),
            a2[0].as_mut_ptr(),
            c0[0].as_mut_ptr(),
            c1[0].as_mut_ptr(),
            c2[0].as_mut_ptr(),
            blk_width,
            blk_height,
            ss_x,
            ss_y,
        );
    }
}

/// `svt_aom_apply_filtering_central_highbd_c`.
#[allow(clippy::too_many_arguments)]
pub fn apply_filtering_central_highbd(
    tf_chroma: bool,
    src: [&[u16]; 3],
    src_stride_y: u32,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
    blk_width: u16,
    blk_height: u16,
    ss_x: u32,
    ss_y: u32,
) {
    let (a0, rest) = accum.split_at_mut(1);
    let (a1, a2) = rest.split_at_mut(1);
    let (c0, crest) = count.split_at_mut(1);
    let (c1, c2) = crest.split_at_mut(1);
    unsafe {
        ref_tf_apply_filtering_central_highbd(
            i32::from(tf_chroma),
            src[0].as_ptr(),
            src[1].as_ptr(),
            src[2].as_ptr(),
            src_stride_y,
            a0[0].as_mut_ptr(),
            a1[0].as_mut_ptr(),
            a2[0].as_mut_ptr(),
            c0[0].as_mut_ptr(),
            c1[0].as_mut_ptr(),
            c2[0].as_mut_ptr(),
            blk_width,
            blk_height,
            ss_x,
            ss_y,
        );
    }
}

/// `svt_aom_get_final_filtered_pixels_c`, 8-bit arm.
#[allow(clippy::too_many_arguments)]
pub fn get_final_filtered_pixels(
    tf_chroma: bool,
    src_center: &mut [Vec<u8>; 3],
    accum: &[Vec<u32>; 3],
    count: &[Vec<u16>; 3],
    stride: [u32; 3],
    blk_y_src_offset: i32,
    blk_ch_src_offset: i32,
    blk_width_ch: u16,
    blk_height_ch: u16,
) {
    let (s0, rest) = src_center.split_at_mut(1);
    let (s1, s2) = rest.split_at_mut(1);
    unsafe {
        ref_tf_get_final_filtered_pixels(
            i32::from(tf_chroma),
            0,
            s0[0].as_mut_ptr(),
            s1[0].as_mut_ptr(),
            s2[0].as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            accum[0].as_ptr(),
            accum[1].as_ptr(),
            accum[2].as_ptr(),
            count[0].as_ptr(),
            count[1].as_ptr(),
            count[2].as_ptr(),
            stride.as_ptr(),
            blk_y_src_offset,
            blk_ch_src_offset,
            blk_width_ch,
            blk_height_ch,
        );
    }
}

/// `svt_aom_get_final_filtered_pixels_c`, 10-bit arm.
#[allow(clippy::too_many_arguments)]
pub fn get_final_filtered_pixels_highbd(
    tf_chroma: bool,
    altref: &mut [Vec<u16>; 3],
    accum: &[Vec<u32>; 3],
    count: &[Vec<u16>; 3],
    stride: [u32; 3],
    blk_y_src_offset: i32,
    blk_ch_src_offset: i32,
    blk_width_ch: u16,
    blk_height_ch: u16,
) {
    let (h0, rest) = altref.split_at_mut(1);
    let (h1, h2) = rest.split_at_mut(1);
    unsafe {
        ref_tf_get_final_filtered_pixels(
            i32::from(tf_chroma),
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            h0[0].as_mut_ptr(),
            h1[0].as_mut_ptr(),
            h2[0].as_mut_ptr(),
            accum[0].as_ptr(),
            accum[1].as_ptr(),
            accum[2].as_ptr(),
            count[0].as_ptr(),
            count[1].as_ptr(),
            count[2].as_ptr(),
            stride.as_ptr(),
            blk_y_src_offset,
            blk_ch_src_offset,
            blk_width_ch,
            blk_height_ch,
        );
    }
}

/// `svt_av1_apply_temporal_filter_planewise_medium_c`.
#[allow(clippy::too_many_arguments)]
pub fn apply_planewise_medium(
    args: &TfCtxArgs,
    y_src: &[u8],
    y_src_stride: i32,
    y_pre: &[u8],
    y_pre_stride: i32,
    u_src: &[u8],
    v_src: &[u8],
    uv_src_stride: i32,
    u_pre: &[u8],
    v_pre: &[u8],
    uv_pre_stride: i32,
    block_width: u32,
    block_height: u32,
    ss_x: i32,
    ss_y: i32,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
) {
    let (a0, rest) = accum.split_at_mut(1);
    let (a1, a2) = rest.split_at_mut(1);
    let (c0, crest) = count.split_at_mut(1);
    let (c1, c2) = crest.split_at_mut(1);
    unsafe {
        ref_tf_apply_planewise_medium(
            args,
            y_src.as_ptr(),
            y_src_stride,
            y_pre.as_ptr(),
            y_pre_stride,
            u_src.as_ptr(),
            v_src.as_ptr(),
            uv_src_stride,
            u_pre.as_ptr(),
            v_pre.as_ptr(),
            uv_pre_stride,
            block_width,
            block_height,
            ss_x,
            ss_y,
            a0[0].as_mut_ptr(),
            c0[0].as_mut_ptr(),
            a1[0].as_mut_ptr(),
            c1[0].as_mut_ptr(),
            a2[0].as_mut_ptr(),
            c2[0].as_mut_ptr(),
        );
    }
}

/// `svt_av1_apply_temporal_filter_planewise_medium_hbd_c`.
#[allow(clippy::too_many_arguments)]
pub fn apply_planewise_medium_hbd(
    args: &TfCtxArgs,
    y_src: &[u16],
    y_src_stride: i32,
    y_pre: &[u16],
    y_pre_stride: i32,
    u_src: &[u16],
    v_src: &[u16],
    uv_src_stride: i32,
    u_pre: &[u16],
    v_pre: &[u16],
    uv_pre_stride: i32,
    block_width: u32,
    block_height: u32,
    ss_x: i32,
    ss_y: i32,
    accum: &mut [Vec<u32>; 3],
    count: &mut [Vec<u16>; 3],
    encoder_bit_depth: u32,
) {
    let (a0, rest) = accum.split_at_mut(1);
    let (a1, a2) = rest.split_at_mut(1);
    let (c0, crest) = count.split_at_mut(1);
    let (c1, c2) = crest.split_at_mut(1);
    unsafe {
        ref_tf_apply_planewise_medium_hbd(
            args,
            y_src.as_ptr(),
            y_src_stride,
            y_pre.as_ptr(),
            y_pre_stride,
            u_src.as_ptr(),
            v_src.as_ptr(),
            uv_src_stride,
            u_pre.as_ptr(),
            v_pre.as_ptr(),
            uv_pre_stride,
            block_width,
            block_height,
            ss_x,
            ss_y,
            a0[0].as_mut_ptr(),
            c0[0].as_mut_ptr(),
            a1[0].as_mut_ptr(),
            c1[0].as_mut_ptr(),
            a2[0].as_mut_ptr(),
            c2[0].as_mut_ptr(),
            encoder_bit_depth,
        );
    }
}

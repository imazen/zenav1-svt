//! Reference bindings for the inter-prediction / motion-compensation surface
//! (`Source/Lib/Codec/inter_prediction.c`, `enc_inter_prediction.c`).
//!
//! These drive the REAL exported C kernels — evidence tier 1 in
//! `docs/WORKING-ON-THIS.md` §4. Everything here goes through
//! `shims/inter_pred_shims.c`, which builds the `InterpFilterParams` /
//! `ConvolveParams` structs the kernels take by pointer, so no C struct is
//! mirrored in Rust.

use core::ffi::c_int;

unsafe extern "C" {
    fn ref_interp_filter_kernel(filt: c_int, size: c_int, subpel: c_int, out: *mut i16);
    fn ref_interp_filter_taps(filt: c_int, size: c_int) -> c_int;
    fn ref_get_conv_params_no_round(
        do_average: c_int,
        dst_stride: c_int,
        is_compound: c_int,
        bd: c_int,
        out: *mut c_int,
    );
    fn ref_convolve_2d_sr(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        filt_x: c_int,
        fx_size: c_int,
        filt_y: c_int,
        fy_size: c_int,
        subpel_x_q4: i32,
        subpel_y_q4: i32,
        bd: c_int,
    );
    fn ref_convolve_x_sr(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        filt_x: c_int,
        fx_size: c_int,
        subpel_x_q4: i32,
        bd: c_int,
    );
    fn ref_convolve_y_sr(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        filt_y: c_int,
        fy_size: c_int,
        subpel_y_q4: i32,
        bd: c_int,
    );
    fn ref_convolve_2d_copy_sr(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        bd: c_int,
    );
    fn ref_jnt_convolve_2d(
        src: *const u8,
        src_stride: i32,
        dst8: *mut u8,
        dst8_stride: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: i32,
        h: i32,
        filt_x: c_int,
        fx_size: c_int,
        filt_y: c_int,
        fy_size: c_int,
        subpel_x_q4: i32,
        subpel_y_q4: i32,
        bd: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_jnt_convolve_x(
        src: *const u8,
        src_stride: i32,
        dst8: *mut u8,
        dst8_stride: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: i32,
        h: i32,
        filt_x: c_int,
        fx_size: c_int,
        subpel_x_q4: i32,
        bd: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_jnt_convolve_y(
        src: *const u8,
        src_stride: i32,
        dst8: *mut u8,
        dst8_stride: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: i32,
        h: i32,
        filt_y: c_int,
        fy_size: c_int,
        subpel_y_q4: i32,
        bd: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_jnt_convolve_2d_copy(
        src: *const u8,
        src_stride: i32,
        dst8: *mut u8,
        dst8_stride: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: i32,
        h: i32,
        bd: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_setup_scale_factors_for_frame(
        other_w: c_int,
        other_h: c_int,
        this_w: c_int,
        this_h: c_int,
        out: *mut c_int,
    );
    fn ref_av1_is_scaled(other_w: c_int, other_h: c_int, this_w: c_int, this_h: c_int) -> c_int;
    fn ref_dist_wtd_comp_weight_assign(
        enable_order_hint: c_int,
        order_hint_bits: c_int,
        cur_frame_index: c_int,
        bck_frame_index: c_int,
        fwd_frame_index: c_int,
        compound_idx: c_int,
        order_idx: c_int,
        is_compound: c_int,
        out: *mut c_int,
    );
    fn ref_get_relative_dist_enc(
        enable_order_hint: c_int,
        order_hint_bits: c_int,
        ref_hint: c_int,
        order_hint: c_int,
    ) -> c_int;
}

/// One 8-tap phase of the kernel `av1_get_interp_filter_params_with_block_size`
/// selects for `(filt, size)` — `filt` is `InterpFilter` (0 regular, 1 smooth,
/// 2 sharp, 3 bilinear) and `size` the block width (x) or height (y).
pub fn interp_filter_kernel(filt: i32, size: i32, subpel: i32) -> [i16; 8] {
    let mut out = [0i16; 8];
    unsafe { ref_interp_filter_kernel(filt, size, subpel, out.as_mut_ptr()) };
    out
}

/// `InterpFilterParams::taps` for the selected params.
pub fn interp_filter_taps(filt: i32, size: i32) -> i32 {
    unsafe { ref_interp_filter_taps(filt, size) }
}

/// `get_conv_params_no_round(...)` -> `(round_0, round_1)`.
pub fn conv_params_rounds(
    do_average: bool,
    dst_stride: i32,
    is_compound: bool,
    bd: i32,
) -> (i32, i32) {
    let mut out = [0i32; 2];
    unsafe {
        ref_get_conv_params_no_round(
            i32::from(do_average),
            dst_stride,
            i32::from(is_compound),
            bd,
            out.as_mut_ptr(),
        )
    };
    (out[0], out[1])
}

/// Reference `svt_av1_convolve_2d_sr_c` (inter_prediction.c:329).
///
/// `src_origin` is the index of the block's top-left pixel inside `src`; the
/// kernel reads 3 pixels left of it and 3 rows above, so the caller must pad.
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_sr(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    filt_y: i32,
    fy_size: i32,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_convolve_2d_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            filt_x,
            fx_size,
            filt_y,
            fy_size,
            subpel_x_q4,
            subpel_y_q4,
            8,
        );
    }
}

/// Reference `svt_av1_convolve_x_sr_c` (inter_prediction.c:402).
#[allow(clippy::too_many_arguments)]
pub fn convolve_x_sr(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    subpel_x_q4: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_convolve_x_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            filt_x,
            fx_size,
            subpel_x_q4,
            8,
        );
    }
}

/// Reference `svt_av1_convolve_y_sr_c` (inter_prediction.c:374).
#[allow(clippy::too_many_arguments)]
pub fn convolve_y_sr(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_y: i32,
    fy_size: i32,
    subpel_y_q4: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_convolve_y_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            filt_y,
            fy_size,
            subpel_y_q4,
            8,
        );
    }
}

/// Reference `svt_av1_convolve_2d_copy_sr_c` (inter_prediction.c:431).
pub fn convolve_2d_copy_sr(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_convolve_2d_copy_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            8,
        );
    }
}

/// Distance-weighting knobs shared by the four compound kernels.
#[derive(Clone, Copy, Debug)]
pub struct JntCfg {
    /// `conv_params->do_average`.
    pub do_average: bool,
    /// `conv_params->use_jnt_comp_avg`.
    pub use_jnt: bool,
    /// `conv_params->fwd_offset`.
    pub fwd: i32,
    /// `conv_params->bck_offset`.
    pub bck: i32,
}

/// Reference `svt_av1_jnt_convolve_2d_c` (inter_prediction.c:526).
#[allow(clippy::too_many_arguments)]
pub fn jnt_convolve_2d(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    filt_y: i32,
    fy_size: i32,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_jnt_convolve_2d(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            w as i32,
            h as i32,
            filt_x,
            fx_size,
            filt_y,
            fy_size,
            subpel_x_q4,
            subpel_y_q4,
            8,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_jnt_convolve_x_c` (inter_prediction.c:629).
#[allow(clippy::too_many_arguments)]
pub fn jnt_convolve_x(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    subpel_x_q4: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_jnt_convolve_x(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            w as i32,
            h as i32,
            filt_x,
            fx_size,
            subpel_x_q4,
            8,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_jnt_convolve_y_c` (inter_prediction.c:584).
#[allow(clippy::too_many_arguments)]
pub fn jnt_convolve_y(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    filt_y: i32,
    fy_size: i32,
    subpel_y_q4: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_jnt_convolve_y(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            w as i32,
            h as i32,
            filt_y,
            fy_size,
            subpel_y_q4,
            8,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_jnt_convolve_2d_copy_c` (inter_prediction.c:674).
#[allow(clippy::too_many_arguments)]
pub fn jnt_convolve_2d_copy(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_jnt_convolve_2d_copy(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            w as i32,
            h as i32,
            8,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_setup_scale_factors_for_frame` (inter_prediction.c:201)
/// -> `(x_scale_fp, y_scale_fp, x_step_q4, y_step_q4)`.
///
/// On the invalid-size early return C leaves `x_step_q4` / `y_step_q4`
/// untouched; the shim seeds them with `-1` so that case is recognisable.
pub fn setup_scale_factors_for_frame(
    other_w: i32,
    other_h: i32,
    this_w: i32,
    this_h: i32,
) -> (i32, i32, i32, i32) {
    let mut out = [0i32; 4];
    unsafe {
        ref_setup_scale_factors_for_frame(other_w, other_h, this_w, this_h, out.as_mut_ptr())
    };
    (out[0], out[1], out[2], out[3])
}

/// Reference `av1_is_scaled` (inter_prediction.h:165) on the factors that
/// `svt_av1_setup_scale_factors_for_frame` derives for these sizes.
pub fn av1_is_scaled(other_w: i32, other_h: i32, this_w: i32, this_h: i32) -> bool {
    unsafe { ref_av1_is_scaled(other_w, other_h, this_w, this_h) != 0 }
}

/// Reference `svt_av1_dist_wtd_comp_weight_assign` (inter_prediction.c:290)
/// -> `(fwd_offset, bck_offset, use_dist_wtd_comp_avg)`.
#[allow(clippy::too_many_arguments)]
pub fn dist_wtd_comp_weight_assign(
    enable_order_hint: bool,
    order_hint_bits: i32,
    cur_frame_index: i32,
    bck_frame_index: i32,
    fwd_frame_index: i32,
    compound_idx: i32,
    order_idx: i32,
    is_compound: bool,
) -> (i32, i32, i32) {
    let mut out = [0i32; 3];
    unsafe {
        ref_dist_wtd_comp_weight_assign(
            i32::from(enable_order_hint),
            order_hint_bits,
            cur_frame_index,
            bck_frame_index,
            fwd_frame_index,
            compound_idx,
            order_idx,
            i32::from(is_compound),
            out.as_mut_ptr(),
        )
    };
    (out[0], out[1], out[2])
}

/// Reference `svt_aom_get_relative_dist_enc` (inter_prediction.c:274).
pub fn get_relative_dist_enc(
    enable_order_hint: bool,
    order_hint_bits: i32,
    ref_hint: i32,
    order_hint: i32,
) -> i32 {
    unsafe {
        ref_get_relative_dist_enc(
            i32::from(enable_order_hint),
            order_hint_bits,
            ref_hint,
            order_hint,
        )
    }
}

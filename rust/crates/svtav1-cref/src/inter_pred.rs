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

// ---------------------------------------------------------------------------
// 10/12-bit (highbd) MC kernels.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_highbd_convolve_2d_sr(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
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
    fn ref_highbd_convolve_x_sr(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        w: i32,
        h: i32,
        filt_x: c_int,
        fx_size: c_int,
        subpel_x_q4: i32,
        bd: c_int,
    );
    fn ref_highbd_convolve_y_sr(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        w: i32,
        h: i32,
        filt_y: c_int,
        fy_size: c_int,
        subpel_y_q4: i32,
        bd: c_int,
    );
    fn ref_highbd_convolve_2d_copy_sr(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        w: i32,
        h: i32,
        bd: c_int,
    );
    fn ref_highbd_jnt_convolve_2d(
        src: *const u16,
        src_stride: i32,
        dst16: *mut u16,
        dst16_stride: i32,
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
    fn ref_highbd_jnt_convolve_x(
        src: *const u16,
        src_stride: i32,
        dst16: *mut u16,
        dst16_stride: i32,
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
    fn ref_highbd_jnt_convolve_y(
        src: *const u16,
        src_stride: i32,
        dst16: *mut u16,
        dst16_stride: i32,
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
    fn ref_highbd_jnt_convolve_2d_copy(
        src: *const u16,
        src_stride: i32,
        dst16: *mut u16,
        dst16_stride: i32,
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
}

/// Reference `svt_av1_highbd_convolve_2d_sr_c` (inter_prediction.c:784).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_2d_sr(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    filt_y: i32,
    fy_size: i32,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    bd: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_highbd_convolve_2d_sr(
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
            bd,
        );
    }
}

/// Reference `svt_av1_highbd_convolve_x_sr_c` (inter_prediction.c:731).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_x_sr(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    subpel_x_q4: i32,
    bd: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_highbd_convolve_x_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            filt_x,
            fx_size,
            subpel_x_q4,
            bd,
        );
    }
}

/// Reference `svt_av1_highbd_convolve_y_sr_c` (inter_prediction.c:758).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_y_sr(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    filt_y: i32,
    fy_size: i32,
    subpel_y_q4: i32,
    bd: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_highbd_convolve_y_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            filt_y,
            fy_size,
            subpel_y_q4,
            bd,
        );
    }
}

/// Reference `svt_av1_highbd_convolve_2d_copy_sr_c` (inter_prediction.c:713).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_2d_copy_sr(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    bd: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w);
    unsafe {
        ref_highbd_convolve_2d_copy_sr(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            bd,
        );
    }
}

/// Reference `svt_av1_highbd_jnt_convolve_2d_c` (inter_prediction.c:1034).
#[allow(clippy::too_many_arguments)]
pub fn highbd_jnt_convolve_2d(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
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
    bd: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_highbd_jnt_convolve_2d(
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
            bd,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_highbd_jnt_convolve_x_c` (inter_prediction.c:905).
#[allow(clippy::too_many_arguments)]
pub fn highbd_jnt_convolve_x(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    filt_x: i32,
    fx_size: i32,
    subpel_x_q4: i32,
    bd: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_highbd_jnt_convolve_x(
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
            bd,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_highbd_jnt_convolve_y_c` (inter_prediction.c:950).
#[allow(clippy::too_many_arguments)]
pub fn highbd_jnt_convolve_y(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    filt_y: i32,
    fy_size: i32,
    subpel_y_q4: i32,
    bd: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_highbd_jnt_convolve_y(
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
            bd,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

/// Reference `svt_av1_highbd_jnt_convolve_2d_copy_c` (inter_prediction.c:995).
#[allow(clippy::too_many_arguments)]
pub fn highbd_jnt_convolve_2d_copy(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    bd: i32,
    cfg: JntCfg,
) {
    assert!(conv_buf.len() >= (h - 1) * conv_stride + w);
    unsafe {
        ref_highbd_jnt_convolve_2d_copy(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            w as i32,
            h as i32,
            bd,
            i32::from(cfg.do_average),
            i32::from(cfg.use_jnt),
            cfg.fwd,
            cfg.bck,
        );
    }
}

// ---------------------------------------------------------------------------
// MC dispatchers (svt_inter_predictor and friends).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_convolve_tables_are_pure_c() -> c_int;
    fn ref_inter_predictor_pd0(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        xs: i32,
        ys: i32,
        subpel_x: i32,
        subpel_y: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        is_compound: c_int,
        do_average: c_int,
        bd: c_int,
    );
    fn ref_inter_predictor(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        xs: i32,
        ys: i32,
        subpel_x: i32,
        subpel_y: i32,
        other_w: c_int,
        other_h: c_int,
        this_w: c_int,
        this_h: c_int,
        w: i32,
        h: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
        interp_filters: u32,
        is_intrabc: c_int,
        bd: c_int,
    );
    fn ref_highbd_inter_predictor(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        xs: i32,
        ys: i32,
        subpel_x: i32,
        subpel_y: i32,
        other_w: c_int,
        other_h: c_int,
        this_w: c_int,
        this_h: c_int,
        w: i32,
        h: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
        interp_filters: u32,
        is_intrabc: c_int,
        bd: c_int,
    );
    fn ref_inter_predictor_light_pd1_8bit(
        src: *mut u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        interp_filters: u32,
        xs: i32,
        ys: i32,
        subpel_x: i32,
        subpel_y: i32,
        conv_buf: *mut u16,
        conv_stride: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_convolve_2d_for_intrabc(
        src: *const u8,
        src_stride: c_int,
        dst: *mut u8,
        dst_stride: c_int,
        w: c_int,
        h: c_int,
        subpel_x_q4: c_int,
        subpel_y_q4: c_int,
        bd: c_int,
    );
    fn ref_highbd_convolve_2d_for_intrabc(
        src: *const u16,
        src_stride: c_int,
        dst: *mut u16,
        dst_stride: c_int,
        w: c_int,
        h: c_int,
        subpel_x_q4: c_int,
        subpel_y_q4: c_int,
        bd: c_int,
    );
    fn ref_get_convolve_filter_params(interp_filters: u32, w: c_int, h: c_int, out: *mut c_int);
    fn ref_make_interp_filters(y_filter: c_int, x_filter: c_int) -> u32;
}

/// Whether the RTCD-filled `svt_aom_convolve` table holds the plain `_c`
/// kernels (as opposed to a SIMD tier). Recorded by the parity test so a
/// green run says WHICH C code it agreed with.
pub fn convolve_tables_are_pure_c() -> bool {
    unsafe { ref_convolve_tables_are_pure_c() != 0 }
}

/// The `SubpelParams` fields the dispatchers read.
#[derive(Clone, Copy, Debug)]
pub struct RefSubpel {
    /// `xs`.
    pub xs: i32,
    /// `ys`.
    pub ys: i32,
    /// `subpel_x`.
    pub subpel_x: i32,
    /// `subpel_y`.
    pub subpel_y: i32,
}

/// Reference `svt_inter_predictor_pd0` (inter_prediction.c:1256).
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor_pd0(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    sp: RefSubpel,
    is_compound: bool,
    do_average: bool,
) {
    unsafe {
        ref_inter_predictor_pd0(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            sp.xs,
            sp.ys,
            sp.subpel_x,
            sp.subpel_y,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            i32::from(is_compound),
            i32::from(do_average),
            8,
        );
    }
}

/// Compound knobs for the two full dispatchers.
#[derive(Clone, Copy, Debug)]
pub struct RefCompound {
    /// `conv_params->is_compound`.
    pub is_compound: bool,
    /// `conv_params->do_average`.
    pub do_average: bool,
    /// `conv_params->use_jnt_comp_avg`.
    pub use_jnt: bool,
    /// `conv_params->fwd_offset`.
    pub fwd: i32,
    /// `conv_params->bck_offset`.
    pub bck: i32,
}

/// Reference `svt_inter_predictor` (inter_prediction.c:1386). The four
/// `*_w`/`*_h` values build the `ScaleFactors` C asserts on (and then ignores).
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    sp: RefSubpel,
    frame_sizes: (i32, i32, i32, i32),
    w: usize,
    h: usize,
    comp: RefCompound,
    interp_filters: u32,
    is_intrabc: bool,
) {
    unsafe {
        ref_inter_predictor(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            sp.xs,
            sp.ys,
            sp.subpel_x,
            sp.subpel_y,
            frame_sizes.0,
            frame_sizes.1,
            frame_sizes.2,
            frame_sizes.3,
            w as i32,
            h as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            i32::from(comp.is_compound),
            i32::from(comp.do_average),
            i32::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
            interp_filters,
            i32::from(is_intrabc),
            8,
        );
    }
}

/// Reference `svt_highbd_inter_predictor` (inter_prediction.c:1444).
#[allow(clippy::too_many_arguments)]
pub fn highbd_inter_predictor(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    sp: RefSubpel,
    frame_sizes: (i32, i32, i32, i32),
    w: usize,
    h: usize,
    comp: RefCompound,
    interp_filters: u32,
    is_intrabc: bool,
    bd: i32,
) {
    unsafe {
        ref_highbd_inter_predictor(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            sp.xs,
            sp.ys,
            sp.subpel_x,
            sp.subpel_y,
            frame_sizes.0,
            frame_sizes.1,
            frame_sizes.2,
            frame_sizes.3,
            w as i32,
            h as i32,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            i32::from(comp.is_compound),
            i32::from(comp.do_average),
            i32::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
            interp_filters,
            i32::from(is_intrabc),
            bd,
        );
    }
}

/// Reference `svt_inter_predictor_light_pd1` (inter_prediction.c:1283) on its
/// 8-bit arm. The `bd > 8` arm needs a packed `src_2b` plane and is not bound.
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor_light_pd1_8bit(
    src: &mut [u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    interp_filters: u32,
    sp: RefSubpel,
    comp: RefCompound,
) {
    unsafe {
        ref_inter_predictor_light_pd1_8bit(
            src.as_mut_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            interp_filters,
            sp.xs,
            sp.ys,
            sp.subpel_x,
            sp.subpel_y,
            conv_buf.as_mut_ptr(),
            conv_stride as i32,
            i32::from(comp.is_compound),
            i32::from(comp.do_average),
            i32::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
        );
    }
}

/// Reference `convolve_2d_for_intrabc` (inter_prediction.c:1194).
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_for_intrabc(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
) {
    unsafe {
        ref_convolve_2d_for_intrabc(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            subpel_x_q4,
            subpel_y_q4,
            8,
        );
    }
}

/// Reference `highbd_convolve_2d_for_intrabc` (inter_prediction.c:1237).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_2d_for_intrabc(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_q4: i32,
    subpel_y_q4: i32,
    bd: i32,
) {
    unsafe {
        ref_highbd_convolve_2d_for_intrabc(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            subpel_x_q4,
            subpel_y_q4,
            bd,
        );
    }
}

/// Reference `av1_get_convolve_filter_params` (inter_prediction.h:139) ->
/// `(x_filter_index, y_filter_index)`.
pub fn get_convolve_filter_params(interp_filters: u32, w: i32, h: i32) -> (i32, i32) {
    let mut out = [0i32; 2];
    unsafe { ref_get_convolve_filter_params(interp_filters, w, h, out.as_mut_ptr()) };
    (out[0], out[1])
}

/// Reference `av1_make_interp_filters` (filter.h:64) — Y first, X second.
pub fn make_interp_filters(y_filter: i32, x_filter: i32) -> u32 {
    unsafe { ref_make_interp_filters(y_filter, x_filter) }
}

// ---------------------------------------------------------------------------
// Masked-compound / wedge-search primitives.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_is_masked_compound_type(t: c_int) -> c_int;
    fn ref_subtract_block(
        rows: c_int,
        cols: c_int,
        diff: *mut i16,
        diff_stride: c_int,
        src: *const u8,
        src_stride: c_int,
        pred: *const u8,
        pred_stride: c_int,
    );
    fn ref_highbd_subtract_block(
        rows: c_int,
        cols: c_int,
        diff: *mut i16,
        diff_stride: c_int,
        src: *const u16,
        src_stride: c_int,
        pred: *const u16,
        pred_stride: c_int,
        bd: c_int,
    );
    fn ref_sum_squares_i16(src: *const i16, n: u32) -> u64;
    fn ref_sse(
        a: *const u8,
        a_stride: c_int,
        b: *const u8,
        b_stride: c_int,
        w: c_int,
        h: c_int,
    ) -> i64;
    fn ref_highbd_sse(
        a: *const u16,
        a_stride: c_int,
        b: *const u16,
        b_stride: c_int,
        w: c_int,
        h: c_int,
    ) -> i64;
    fn ref_wedge_sse_from_residuals(r1: *const i16, d: *const i16, m: *const u8, n: c_int) -> u64;
    fn ref_wedge_sign_from_residuals(ds: *const i16, m: *const u8, n: c_int, limit: i64) -> c_int;
    fn ref_wedge_compute_delta_squares(d: *mut i16, a: *const i16, b: *const i16, n: c_int);
    fn ref_build_compound_diffwtd_mask(
        mask: *mut u8,
        mask_type: c_int,
        src0: *const u8,
        src0_stride: c_int,
        src1: *const u8,
        src1_stride: c_int,
        h: c_int,
        w: c_int,
    );
    fn ref_build_compound_diffwtd_mask_highbd(
        mask: *mut u8,
        mask_type: c_int,
        src0: *const u16,
        src0_stride: c_int,
        src1: *const u16,
        src1_stride: c_int,
        h: c_int,
        w: c_int,
        bd: c_int,
    );
    fn ref_highbd_blend_a64_hmask_16bit(
        dst: *mut u16,
        dst_stride: u32,
        src0: *const u16,
        src0_stride: u32,
        src1: *const u16,
        src1_stride: u32,
        mask: *const u8,
        w: c_int,
        h: c_int,
        bd: c_int,
    );
}

/// Reference `svt_aom_is_masked_compound_type` (inter_prediction.c:34).
pub fn is_masked_compound_type(t: i32) -> bool {
    unsafe { ref_is_masked_compound_type(t) != 0 }
}

/// Reference `svt_aom_subtract_block_c` (inter_prediction.c:55).
#[allow(clippy::too_many_arguments)]
pub fn subtract_block(
    rows: usize,
    cols: usize,
    diff: &mut [i16],
    diff_stride: usize,
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
) {
    assert!(diff.len() >= (rows - 1) * diff_stride + cols);
    unsafe {
        ref_subtract_block(
            rows as i32,
            cols as i32,
            diff.as_mut_ptr(),
            diff_stride as i32,
            src.as_ptr(),
            src_stride as i32,
            pred.as_ptr(),
            pred_stride as i32,
        );
    }
}

/// Reference `svt_aom_highbd_subtract_block_c` (inter_prediction.c:38).
#[allow(clippy::too_many_arguments)]
pub fn highbd_subtract_block(
    rows: usize,
    cols: usize,
    diff: &mut [i16],
    diff_stride: usize,
    src: &[u16],
    src_stride: usize,
    pred: &[u16],
    pred_stride: usize,
    bd: i32,
) {
    assert!(diff.len() >= (rows - 1) * diff_stride + cols);
    unsafe {
        ref_highbd_subtract_block(
            rows as i32,
            cols as i32,
            diff.as_mut_ptr(),
            diff_stride as i32,
            src.as_ptr(),
            src_stride as i32,
            pred.as_ptr(),
            pred_stride as i32,
            bd,
        );
    }
}

/// Reference `svt_aom_sum_squares_i16_c` (inter_prediction.c:2522).
pub fn sum_squares_i16(src: &[i16], n: usize) -> u64 {
    assert!(n > 0 && src.len() >= n);
    unsafe { ref_sum_squares_i16(src.as_ptr(), n as u32) }
}

/// Reference `svt_aom_sse_c` (enc_inter_prediction.c:612).
pub fn sse(a: &[u8], a_stride: usize, b: &[u8], b_stride: usize, w: usize, h: usize) -> i64 {
    assert!(a.len() >= (h - 1) * a_stride + w && b.len() >= (h - 1) * b_stride + w);
    unsafe {
        ref_sse(
            a.as_ptr(),
            a_stride as i32,
            b.as_ptr(),
            b_stride as i32,
            w as i32,
            h as i32,
        )
    }
}

/// Reference `svt_aom_highbd_sse_c` (enc_inter_prediction.c:597).
pub fn highbd_sse(
    a: &[u16],
    a_stride: usize,
    b: &[u16],
    b_stride: usize,
    w: usize,
    h: usize,
) -> i64 {
    assert!(a.len() >= (h - 1) * a_stride + w && b.len() >= (h - 1) * b_stride + w);
    unsafe {
        ref_highbd_sse(
            a.as_ptr(),
            a_stride as i32,
            b.as_ptr(),
            b_stride as i32,
            w as i32,
            h as i32,
        )
    }
}

/// Reference `svt_av1_wedge_sse_from_residuals_c` (inter_prediction.c:2457).
pub fn wedge_sse_from_residuals(r1: &[i16], d: &[i16], m: &[u8], n: usize) -> u64 {
    assert!(r1.len() >= n && d.len() >= n && m.len() >= n);
    unsafe { ref_wedge_sse_from_residuals(r1.as_ptr(), d.as_ptr(), m.as_ptr(), n as i32) }
}

/// Reference `svt_av1_wedge_sign_from_residuals_c` (enc_inter_prediction.c:414).
pub fn wedge_sign_from_residuals(ds: &[i16], m: &[u8], n: usize, limit: i64) -> bool {
    assert!(n > 0 && ds.len() >= n && m.len() >= n);
    unsafe { ref_wedge_sign_from_residuals(ds.as_ptr(), m.as_ptr(), n as i32, limit) != 0 }
}

/// Reference `svt_av1_wedge_compute_delta_squares_c` (enc_inter_prediction.c:375).
pub fn wedge_compute_delta_squares(d: &mut [i16], a: &[i16], b: &[i16], n: usize) {
    assert!(d.len() >= n && a.len() >= n && b.len() >= n);
    unsafe { ref_wedge_compute_delta_squares(d.as_mut_ptr(), a.as_ptr(), b.as_ptr(), n as i32) };
}

/// Reference `svt_av1_build_compound_diffwtd_mask_c` (inter_prediction.c:154).
#[allow(clippy::too_many_arguments)]
pub fn build_compound_diffwtd_mask(
    mask: &mut [u8],
    mask_type: i32,
    src0: &[u8],
    src0_stride: usize,
    src1: &[u8],
    src1_stride: usize,
    h: usize,
    w: usize,
) {
    assert!(mask.len() >= h * w);
    unsafe {
        ref_build_compound_diffwtd_mask(
            mask.as_mut_ptr(),
            mask_type,
            src0.as_ptr(),
            src0_stride as i32,
            src1.as_ptr(),
            src1_stride as i32,
            h as i32,
            w as i32,
        );
    }
}

/// Reference `svt_av1_build_compound_diffwtd_mask_highbd_c` (inter_prediction.c:139).
#[allow(clippy::too_many_arguments)]
pub fn build_compound_diffwtd_mask_highbd(
    mask: &mut [u8],
    mask_type: i32,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    bd: i32,
) {
    assert!(mask.len() >= h * w);
    unsafe {
        ref_build_compound_diffwtd_mask_highbd(
            mask.as_mut_ptr(),
            mask_type,
            src0.as_ptr(),
            src0_stride as i32,
            src1.as_ptr(),
            src1_stride as i32,
            h as i32,
            w as i32,
            bd,
        );
    }
}

/// Reference `svt_aom_highbd_blend_a64_hmask_16bit_c` (inter_prediction.c:2500).
#[allow(clippy::too_many_arguments)]
pub fn highbd_blend_a64_hmask_16bit(
    dst: &mut [u16],
    dst_stride: usize,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    mask: &[u8],
    w: usize,
    h: usize,
    bd: i32,
) {
    assert!(dst.len() >= (h - 1) * dst_stride + w && mask.len() >= w);
    unsafe {
        ref_highbd_blend_a64_hmask_16bit(
            dst.as_mut_ptr(),
            dst_stride as u32,
            src0.as_ptr(),
            src0_stride as u32,
            src1.as_ptr(),
            src1_stride as u32,
            mask.as_ptr(),
            w as i32,
            h as i32,
            bd,
        );
    }
}

// ---------------------------------------------------------------------------
// Wedge mask tables.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_is_interintra_wedge_used(bsize: c_int) -> c_int;
    fn ref_get_wedge_bits_lookup(bsize: c_int) -> c_int;
    fn ref_get_wedge_params_bits(bsize: c_int) -> c_int;
    fn ref_get_contiguous_soft_mask(
        wedge_index: c_int,
        wedge_sign: c_int,
        bsize: c_int,
        out: *mut u8,
        n: c_int,
    );
}

/// Reference `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015).
pub fn is_interintra_wedge_used(bsize: i32) -> bool {
    unsafe { ref_is_interintra_wedge_used(bsize) != 0 }
}

/// Reference `svt_aom_get_wedge_bits_lookup` (inter_prediction.c:2019).
pub fn get_wedge_bits_lookup(bsize: i32) -> i32 {
    unsafe { ref_get_wedge_bits_lookup(bsize) }
}

/// Reference `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053).
pub fn get_wedge_params_bits(bsize: i32) -> i32 {
    unsafe { ref_get_wedge_params_bits(bsize) }
}

/// Reference `svt_aom_get_contiguous_soft_mask` (inter_prediction.c:2023),
/// after `svt_av1_init_wedge_masks` has run. Returns `n` bytes of the mask.
pub fn get_contiguous_soft_mask(
    wedge_index: i32,
    wedge_sign: i32,
    bsize: i32,
    n: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; n];
    unsafe {
        ref_get_contiguous_soft_mask(wedge_index, wedge_sign, bsize, out.as_mut_ptr(), n as i32);
    }
    out
}

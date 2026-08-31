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

// ---------------------------------------------------------------------------
// Inter-intra.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_combine_interintra(
        mode: c_int,
        use_wedge: c_int,
        wedge_index: c_int,
        wedge_sign: c_int,
        bsize: c_int,
        plane_bsize: c_int,
        comppred: *mut u8,
        compstride: c_int,
        interpred: *const u8,
        interstride: c_int,
        intrapred: *const u8,
        intrastride: c_int,
    );
    fn ref_combine_interintra_highbd(
        mode: c_int,
        use_wedge: c_int,
        wedge_index: c_int,
        wedge_sign: c_int,
        bsize: c_int,
        plane_bsize: c_int,
        comppred: *mut u16,
        compstride: c_int,
        interpred: *const u16,
        interstride: c_int,
        intrapred: *const u16,
        intrastride: c_int,
        bd: c_int,
    );
}

/// The wedge selection shared by both `combine_interintra` bindings.
#[derive(Clone, Copy, Debug)]
pub struct IiWedge {
    /// `use_wedge_interintra`.
    pub use_wedge: bool,
    /// `wedge_index`.
    pub index: i32,
    /// `wedge_sign`.
    pub sign: i32,
}

/// Reference `svt_aom_combine_interintra` (inter_prediction.c:2468).
#[allow(clippy::too_many_arguments)]
pub fn combine_interintra(
    mode: i32,
    wedge: IiWedge,
    bsize: i32,
    plane_bsize: i32,
    comppred: &mut [u8],
    compstride: usize,
    interpred: &[u8],
    interstride: usize,
    intrapred: &[u8],
    intrastride: usize,
) {
    unsafe {
        ref_combine_interintra(
            mode,
            i32::from(wedge.use_wedge),
            wedge.index,
            wedge.sign,
            bsize,
            plane_bsize,
            comppred.as_mut_ptr(),
            compstride as i32,
            interpred.as_ptr(),
            interstride as i32,
            intrapred.as_ptr(),
            intrastride as i32,
        );
    }
}

/// Reference `svt_aom_combine_interintra_highbd` (inter_prediction.c:2298).
#[allow(clippy::too_many_arguments)]
pub fn combine_interintra_highbd(
    mode: i32,
    wedge: IiWedge,
    bsize: i32,
    plane_bsize: i32,
    comppred: &mut [u16],
    compstride: usize,
    interpred: &[u16],
    interstride: usize,
    intrapred: &[u16],
    intrastride: usize,
    bd: i32,
) {
    unsafe {
        ref_combine_interintra_highbd(
            mode,
            i32::from(wedge.use_wedge),
            wedge.index,
            wedge.sign,
            bsize,
            plane_bsize,
            comppred.as_mut_ptr(),
            compstride as i32,
            interpred.as_ptr(),
            interstride as i32,
            intrapred.as_ptr(),
            intrastride as i32,
            bd,
        );
    }
}

// ---------------------------------------------------------------------------
// Fast RD models.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_model_rd_from_var_lapndz(
        var: i64,
        n_log2: u32,
        qstep: u32,
        rate: *mut i32,
        dist: *mut i64,
    );
    fn ref_model_rd_from_sse(
        bsize: c_int,
        quantizer: c_int,
        bit_depth: c_int,
        sse: u64,
        simple: c_int,
        rate: *mut u32,
        dist: *mut u64,
    );
    fn ref_log2f_safe(x: u32) -> c_int;
    fn ref_get_msb(x: u32) -> c_int;
}

/// Reference `svt_av1_model_rd_from_var_lapndz` (enc_inter_prediction.c:1933).
pub fn model_rd_from_var_lapndz(var: i64, n_log2: u32, qstep: u32) -> (i32, i64) {
    let mut rate = 0i32;
    let mut dist = 0i64;
    unsafe { ref_model_rd_from_var_lapndz(var, n_log2, qstep, &mut rate, &mut dist) };
    (rate, dist)
}

/// Reference `model_rd_from_sse` (enc_inter_prediction.c:1954).
pub fn model_rd_from_sse(
    bsize: i32,
    quantizer: i32,
    bit_depth: i32,
    sse: u64,
    simple_model_rd_from_var: bool,
) -> (u32, u64) {
    let mut rate = 0u32;
    let mut dist = 0u64;
    unsafe {
        ref_model_rd_from_sse(
            bsize,
            quantizer,
            bit_depth,
            sse,
            i32::from(simple_model_rd_from_var),
            &mut rate,
            &mut dist,
        )
    };
    (rate, dist)
}

/// Reference `svt_log2f_safe` (definitions.h:612).
pub fn log2f_safe(x: u32) -> i32 {
    unsafe { ref_log2f_safe(x) }
}

/// Reference `get_msb` (definitions.h:617).
pub fn get_msb(x: u32) -> i32 {
    unsafe { ref_get_msb(x) }
}

// ---------------------------------------------------------------------------
// OBMC wsrc/mask producer.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_calc_target_weighted_pred_above(
        n4_w: c_int,
        rel_mi_col: c_int,
        nb_mi_width: c_int,
        mask_buf: *mut i32,
        wsrc_buf: *mut i32,
        tmp: *const u8,
        tmp_stride: c_int,
        overlap: c_int,
    );
    fn ref_calc_target_weighted_pred_left(
        n4_w: c_int,
        rel_mi_row: c_int,
        nb_mi_height: c_int,
        mask_buf: *mut i32,
        wsrc_buf: *mut i32,
        tmp: *const u8,
        tmp_stride: c_int,
        overlap: c_int,
    );
    fn ref_skip_u4x4_pred_in_obmc(bsize: c_int, dir: c_int, ssx: c_int, ssy: c_int) -> c_int;
    fn ref_get_plane_block_size(bsize: c_int, ssx: c_int, ssy: c_int) -> c_int;
    fn ref_get_obmc_mask(overlap: c_int, out: *mut u8);
}

/// Reference `svt_av1_calc_target_weighted_pred_above_c`
/// (enc_inter_prediction.c:1577).
#[allow(clippy::too_many_arguments)]
pub fn calc_target_weighted_pred_above(
    n4_w: usize,
    rel_mi_col: usize,
    nb_mi_width: usize,
    mask_buf: &mut [i32],
    wsrc_buf: &mut [i32],
    tmp: &[u8],
    tmp_stride: usize,
    overlap: usize,
) {
    unsafe {
        ref_calc_target_weighted_pred_above(
            n4_w as i32,
            rel_mi_col as i32,
            nb_mi_width as i32,
            mask_buf.as_mut_ptr(),
            wsrc_buf.as_mut_ptr(),
            tmp.as_ptr(),
            tmp_stride as i32,
            overlap as i32,
        );
    }
}

/// Reference `svt_av1_calc_target_weighted_pred_left_c`
/// (enc_inter_prediction.c:1605).
#[allow(clippy::too_many_arguments)]
pub fn calc_target_weighted_pred_left(
    n4_w: usize,
    rel_mi_row: usize,
    nb_mi_height: usize,
    mask_buf: &mut [i32],
    wsrc_buf: &mut [i32],
    tmp: &[u8],
    tmp_stride: usize,
    overlap: usize,
) {
    unsafe {
        ref_calc_target_weighted_pred_left(
            n4_w as i32,
            rel_mi_row as i32,
            nb_mi_height as i32,
            mask_buf.as_mut_ptr(),
            wsrc_buf.as_mut_ptr(),
            tmp.as_ptr(),
            tmp_stride as i32,
            overlap as i32,
        );
    }
}

/// Reference `svt_av1_skip_u4x4_pred_in_obmc` (inter_prediction.c:2403).
pub fn skip_u4x4_pred_in_obmc(bsize: i32, dir: i32, ssx: i32, ssy: i32) -> i32 {
    unsafe { ref_skip_u4x4_pred_in_obmc(bsize, dir, ssx, ssy) }
}

/// Reference `get_plane_block_size` (common_utils.h:135); `-1` is
/// `BLOCK_INVALID`.
pub fn get_plane_block_size(bsize: i32, ssx: i32, ssy: i32) -> i32 {
    unsafe { ref_get_plane_block_size(bsize, ssx, ssy) }
}

/// Reference `svt_av1_get_obmc_mask(overlap)`.
pub fn get_obmc_mask(overlap: usize) -> Vec<u8> {
    let mut out = vec![0u8; overlap];
    unsafe { ref_get_obmc_mask(overlap as i32, out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------
// Scaled-reference kernels.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_convolve_2d_scale_full(
        src: *const u8,
        src_stride: c_int,
        dst8: *mut u8,
        dst8_stride: c_int,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: c_int,
        h: c_int,
        filt_x: c_int,
        fx_size: c_int,
        filt_y: c_int,
        fy_size: c_int,
        subpel_x_qn: c_int,
        x_step_qn: c_int,
        subpel_y_qn: c_int,
        y_step_qn: c_int,
        bd: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
    fn ref_highbd_convolve_2d_scale_full(
        src: *const u16,
        src_stride: c_int,
        dst: *mut u16,
        dst_stride: c_int,
        conv_buf: *mut u16,
        conv_stride: c_int,
        w: c_int,
        h: c_int,
        filt_x: c_int,
        fx_size: c_int,
        filt_y: c_int,
        fy_size: c_int,
        subpel_x_qn: c_int,
        x_step_qn: c_int,
        subpel_y_qn: c_int,
        y_step_qn: c_int,
        bd: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
    );
}

/// The `(subpel, step)` phase pair each axis of a scaled convolve takes, in
/// the `SCALE_SUBPEL_BITS = 10` domain.
#[derive(Clone, Copy, Debug)]
pub struct ScalePhases {
    /// `subpel_x_qn`.
    pub subpel_x_qn: i32,
    /// `x_step_qn`.
    pub x_step_qn: i32,
    /// `subpel_y_qn`.
    pub subpel_y_qn: i32,
    /// `y_step_qn`.
    pub y_step_qn: i32,
}

/// Reference `svt_av1_convolve_2d_scale_c` (inter_prediction.c:448), with the
/// full ConvolveParams surface (compound and distance weights included).
///
/// The older [`crate::convolve_2d_scale`] binding pins EIGHTTAP_REGULAR and
/// single prediction only; this one is the general form.
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_scale_full(
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
    phases: ScalePhases,
    comp: RefCompound,
) {
    unsafe {
        ref_convolve_2d_scale_full(
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
            phases.subpel_x_qn,
            phases.x_step_qn,
            phases.subpel_y_qn,
            phases.y_step_qn,
            8,
            i32::from(comp.is_compound),
            i32::from(comp.do_average),
            i32::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
        );
    }
}

/// Reference `svt_av1_highbd_convolve_2d_scale_c` (inter_prediction.c:828).
#[allow(clippy::too_many_arguments)]
pub fn highbd_convolve_2d_scale_full(
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
    phases: ScalePhases,
    comp: RefCompound,
    bd: i32,
) {
    unsafe {
        ref_highbd_convolve_2d_scale_full(
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
            phases.subpel_x_qn,
            phases.x_step_qn,
            phases.subpel_y_qn,
            phases.y_step_qn,
            bd,
            i32::from(comp.is_compound),
            i32::from(comp.do_average),
            i32::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
        );
    }
}

// ---------------------------------------------------------------------------
// Subpel-param derivation, via the exported tf_inter_predictor.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_tf_inter_predictor(
        src: *mut u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        pre_y: c_int,
        pre_x: c_int,
        mv_x: c_int,
        mv_y: c_int,
        other_w: c_int,
        other_h: c_int,
        this_w: c_int,
        this_h: c_int,
        super_block_size: c_int,
        frame_width: c_int,
        frame_height: c_int,
        blk_width: c_int,
        blk_height: c_int,
        mb_to_left: c_int,
        mb_to_right: c_int,
        mb_to_top: c_int,
        mb_to_bottom: c_int,
        interp_filters: u32,
        bit_depth: c_int,
        subsampling_shift: c_int,
    );
}

/// The `MacroBlockD` edge distances the subpel derivation reads.
#[derive(Clone, Copy, Debug)]
pub struct RefMbEdges {
    /// `mb_to_left_edge`.
    pub to_left: i32,
    /// `mb_to_right_edge`.
    pub to_right: i32,
    /// `mb_to_top_edge`.
    pub to_top: i32,
    /// `mb_to_bottom_edge`.
    pub to_bottom: i32,
}

/// Reference `tf_inter_predictor` (enc_inter_prediction.c:2452).
///
/// This is the only EXPORTED caller of the `static` `compute_subpel_params`
/// whose arguments a shim can synthesise: it reads exactly
/// `scs->super_block_size` off the `SequenceControlSet` and the four
/// `mb_to_*_edge` fields off the `MacroBlockD`. Its output pixels therefore
/// pin both `compute_subpel_params` and `clamp_mv_to_umv_border_sb`.
///
/// `src` must be the base of the reference plane (the function offsets it by
/// the derived `pos_x`/`pos_y` itself), with enough margin around the block.
#[allow(clippy::too_many_arguments)]
pub fn tf_inter_predictor(
    src: &mut [u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    pre: (i32, i32),
    mv: (i32, i32),
    frame_sizes: (i32, i32, i32, i32),
    super_block_size: i32,
    frame_dims: (i32, i32),
    blk: (i32, i32),
    edges: RefMbEdges,
    interp_filters: u32,
    bit_depth: i32,
    subsampling_shift: i32,
) {
    unsafe {
        ref_tf_inter_predictor(
            src.as_mut_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            pre.0,
            pre.1,
            mv.0,
            mv.1,
            frame_sizes.0,
            frame_sizes.1,
            frame_sizes.2,
            frame_sizes.3,
            super_block_size,
            frame_dims.0,
            frame_dims.1,
            blk.0,
            blk.1,
            edges.to_left,
            edges.to_right,
            edges.to_top,
            edges.to_bottom,
            interp_filters,
            bit_depth,
            subsampling_shift,
        );
    }
}

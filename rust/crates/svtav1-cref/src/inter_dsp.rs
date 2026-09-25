use super::*;

// AUDIT 2026-07-14: inter / motion DSP oracles (sad, variance, convolve8).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_sad(w: i32, h: i32, src: *const u8, ss: i32, r: *const u8, rs: i32) -> u32;
    pub(super) fn ref_chroma_variance10(
        pred: *const u16,
        ps: i32,
        src: *const u16,
        ss: i32,
        w: i32,
        h: i32,
    ) -> u32;
    pub(super) fn ref_variance(
        w: i32,
        h: i32,
        a: *const u8,
        as_: i32,
        b: *const u8,
        bs: i32,
        sse: *mut u32,
    ) -> u32;
    pub(super) fn ref_convolve8_horiz(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        taps: *const i16,
        w: i32,
        h: i32,
    );
    pub(super) fn ref_convolve8_vert(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        taps: *const i16,
        w: i32,
        h: i32,
    );
}

/// Reference `svt_aom_sad{w}x{h}_c`: sum of abs differences over the block.
/// `src_origin`/`ref_origin` index the block top-left inside their buffers.
#[allow(clippy::too_many_arguments)]
pub fn sad(
    w: usize,
    h: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    r: &[u8],
    ref_origin: usize,
    ref_stride: usize,
) -> u32 {
    assert!(src_origin + (h - 1) * src_stride + w <= src.len());
    assert!(ref_origin + (h - 1) * ref_stride + w <= r.len());
    let out = unsafe {
        ref_sad(
            w as i32,
            h as i32,
            src.as_ptr().add(src_origin),
            src_stride as i32,
            r.as_ptr().add(ref_origin),
            ref_stride as i32,
        )
    };
    assert_ne!(out, 0xFFFF_FFFF, "cref: unsupported SAD size {w}x{h}");
    out
}

/// The actual sized 10-bit variance used by pristine independent UV search.
/// Argument order is prediction first, source second (signed rounding matters).
pub fn chroma_variance10(
    pred: &[u16],
    ps: usize,
    src: &[u16],
    ss: usize,
    w: usize,
    h: usize,
) -> u32 {
    assert!(w > 0 && h > 0 && w <= 64 && h <= 64);
    assert!((h - 1) * ps + w <= pred.len());
    assert!((h - 1) * ss + w <= src.len());
    let result = unsafe {
        ref_chroma_variance10(
            pred.as_ptr(),
            ps as i32,
            src.as_ptr(),
            ss as i32,
            w as i32,
            h as i32,
        )
    };
    assert_ne!(result, u32::MAX, "unsupported chroma variance geometry");
    result
}

/// Reference `svt_aom_variance{w}x{h}_c`: returns `(variance, sse)` where
/// `sse = sum((a-b)^2)` and `variance = sse - sum(a-b)^2 / (w*h)` (two blocks).
#[allow(clippy::too_many_arguments)]
pub fn variance(
    w: usize,
    h: usize,
    a: &[u8],
    a_origin: usize,
    a_stride: usize,
    b: &[u8],
    b_origin: usize,
    b_stride: usize,
) -> (u32, u32) {
    assert!(a_origin + (h - 1) * a_stride + w <= a.len());
    assert!(b_origin + (h - 1) * b_stride + w <= b.len());
    let mut sse = 0u32;
    let var = unsafe {
        ref_variance(
            w as i32,
            h as i32,
            a.as_ptr().add(a_origin),
            a_stride as i32,
            b.as_ptr().add(b_origin),
            b_stride as i32,
            &mut sse,
        )
    };
    assert_ne!(sse, 0xFFFF_FFFF, "cref: unsupported variance size {w}x{h}");
    (var, sse)
}

/// Reference `svt_aom_convolve8_horiz_c` with `x_step_q4=16` and the given
/// 8 taps. `src_origin` points at the C-convention origin (the kernel reads
/// `src[origin-3 ..= origin+w+3]` per row — 3 left / 4 right of the window).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_horiz(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    taps: &[i16; 8],
    w: usize,
    h: usize,
) {
    assert!(src_origin >= 3);
    assert!(src_origin + (h - 1) * src_stride + w + 4 <= src.len());
    assert!((h - 1) * dst_stride + w <= dst.len());
    unsafe {
        ref_convolve8_horiz(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            taps.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

/// Reference `svt_aom_convolve8_vert_c` with `y_step_q4=16` and the given
/// 8 taps. `src_origin` points at the C-convention origin (the kernel reads
/// rows `origin-3 ..= origin+h+3` — 3 above / 4 below the window).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_vert(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    taps: &[i16; 8],
    w: usize,
    h: usize,
) {
    assert!(src_origin >= 3 * src_stride);
    assert!(src_origin + (h + 3) * src_stride + w <= src.len());
    assert!((h - 1) * dst_stride + w <= dst.len());
    unsafe {
        ref_convolve8_vert(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            taps.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

unsafe extern "C" {
    pub(super) fn ref_obmc_mask(length: i32, out: *mut u8);
    pub(super) fn ref_obmc_blend_above(
        dst: *mut u8,
        dst_stride: i32,
        above: *const u8,
        above_stride: i32,
        w: i32,
        overlap: i32,
    );
    pub(super) fn ref_obmc_blend_left(
        dst: *mut u8,
        dst_stride: i32,
        left: *const u8,
        left_stride: i32,
        overlap: i32,
        h: i32,
    );
}

/// Reference `svt_av1_get_obmc_mask(length)` (length in {1,2,4,8,16,32}).
pub fn obmc_mask(length: usize) -> Vec<u8> {
    let mut out = vec![0u8; length];
    unsafe { ref_obmc_mask(length as i32, out.as_mut_ptr()) };
    out
}

/// Reference OBMC "above" blend: `svt_aom_blend_a64_vmask_c(dst, dst, above,
/// obmc_mask(overlap))` over the top `overlap` rows × `w` cols — the exact
/// `build_obmc_inter_pred_above` reconstruction blend (dst = current pred).
pub fn obmc_blend_above(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    above_stride: usize,
    w: usize,
    overlap: usize,
) {
    assert!((overlap - 1) * dst_stride + w <= dst.len());
    assert!((overlap - 1) * above_stride + w <= above.len());
    unsafe {
        ref_obmc_blend_above(
            dst.as_mut_ptr(),
            dst_stride as i32,
            above.as_ptr(),
            above_stride as i32,
            w as i32,
            overlap as i32,
        );
    }
}

/// Reference OBMC "left" blend: `svt_aom_blend_a64_hmask_c(dst, dst, left,
/// obmc_mask(overlap))` over `h` rows × the left `overlap` cols — the exact
/// `build_obmc_inter_pred_left` reconstruction blend (dst = current pred).
pub fn obmc_blend_left(
    dst: &mut [u8],
    dst_stride: usize,
    left: &[u8],
    left_stride: usize,
    overlap: usize,
    h: usize,
) {
    assert!((h - 1) * dst_stride + overlap <= dst.len());
    assert!((h - 1) * left_stride + overlap <= left.len());
    unsafe {
        ref_obmc_blend_left(
            dst.as_mut_ptr(),
            dst_stride as i32,
            left.as_ptr(),
            left_stride as i32,
            overlap as i32,
            h as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// AUDIT 2026-07-14: oracles for the dormant inter/scaling stubs
// (warp.rs / scale.rs / superres.rs are NOT ports of these — see the
// c_parity_{warp,scale,superres} suites which pin the divergence).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_warp_affine(
        mat: *const i32,
        r: *const u8,
        width: i32,
        height: i32,
        stride: i32,
        pred: *mut u8,
        p_col: i32,
        p_row: i32,
        p_width: i32,
        p_height: i32,
        p_stride: i32,
        alpha: i16,
        beta: i16,
        gamma: i16,
        delta: i16,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_convolve_2d_scale(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        subpel_x_qn: i32,
        x_step_qn: i32,
        subpel_y_qn: i32,
        y_step_qn: i32,
    );
    pub(super) fn ref_superres_filter_normative(phase: i32, out8: *mut i16);
    pub(super) fn ref_superres_upscale_row(
        input: *const u8,
        in_width: i32,
        output: *mut u8,
        out_width: i32,
    );
    pub(super) fn ref_superres_upscale_row_hbd(
        input: *mut u16,
        in_width: i32,
        output: *mut u16,
        out_width: i32,
        bd: i32,
    );
    pub(super) fn ref_resize_plane_horizontal(
        input: *const u8,
        height: i32,
        width: i32,
        in_stride: i32,
        output: *mut u8,
        width2: i32,
        out_stride: i32,
    ) -> i32;
    pub(super) fn ref_down2_symeven(input: *const u8, length: i32, output: *mut u8);
    pub(super) fn ref_interpolate_core(
        input: *const u8,
        in_length: i32,
        output: *mut u8,
        out_length: i32,
        bank: i32,
    );
}

/// Reference `svt_av1_resize_plane_horizontal` (resize.c:464) — the superres
/// SOURCE downscale: `width` -> `width2` at unchanged height. Drives C's
/// `static` `resize_multistep` (down2 steps + polyphase interpolate), so this
/// one oracle covers every arm of the ladder.
pub fn resize_plane_horizontal(
    input: &[u8],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u8],
    width2: usize,
    out_stride: usize,
) {
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height - 1) * out_stride + width2);
    let rc = unsafe {
        ref_resize_plane_horizontal(
            input.as_ptr(),
            height as i32,
            width as i32,
            in_stride as i32,
            output.as_mut_ptr(),
            width2 as i32,
            out_stride as i32,
        )
    };
    assert_eq!(rc, 0, "svt_av1_resize_plane_horizontal failed (rc {rc})");
}

/// Reference `svt_av1_down2_symeven_c` (resize.c:170) — exact 2:1 decimation.
pub fn down2_symeven(input: &[u8], length: usize, output: &mut [u8]) {
    assert!(input.len() >= length && output.len() >= length.div_ceil(2));
    unsafe { ref_down2_symeven(input.as_ptr(), length as i32, output.as_mut_ptr()) };
}

/// Reference `svt_av1_interpolate_core_c` (resize.c:287) with an explicit
/// filter bank (0 = filters1000 / normative, 1 = 875, 2 = 750, 3 = 625,
/// 4 = 500 — C's `choose_interp_filter` ladder).
pub fn interpolate_core(
    input: &[u8],
    in_length: usize,
    output: &mut [u8],
    out_length: usize,
    bank: i32,
) {
    assert!(input.len() >= in_length && output.len() >= out_length);
    unsafe {
        ref_interpolate_core(
            input.as_ptr(),
            in_length as i32,
            output.as_mut_ptr(),
            out_length as i32,
            bank,
        )
    };
}

/// Reference `svt_av1_warp_affine_c` (non-compound, 8-bit). `mat` is the 6-entry
/// Q16 affine model; `pred` is `p_height` rows × `p_stride`. See warped_motion.c.
#[allow(clippy::too_many_arguments)]
pub fn warp_affine(
    mat: &[i32; 6],
    r: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    pred: &mut [u8],
    p_col: i32,
    p_row: i32,
    p_width: usize,
    p_height: usize,
    p_stride: usize,
    shear: (i16, i16, i16, i16),
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_warp_affine(
            mat.as_ptr(),
            r.as_ptr(),
            width as i32,
            height as i32,
            stride as i32,
            pred.as_mut_ptr(),
            p_col,
            p_row,
            p_width as i32,
            p_height as i32,
            p_stride as i32,
            shear.0,
            shear.1,
            shear.2,
            shear.3,
        );
    }
}

/// Reference `svt_av1_convolve_2d_scale_c` (non-compound 8-bit, EIGHTTAP_REGULAR
/// both axes). Phases are in the `SCALE_SUBPEL_BITS = 10` domain. `src` must be
/// pre-offset so the kernel's fo_horiz/fo_vert (3) taps stay in bounds.
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_scale(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_qn: i32,
    x_step_qn: i32,
    subpel_y_qn: i32,
    y_step_qn: i32,
) {
    unsafe {
        ref_convolve_2d_scale(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            subpel_x_qn,
            x_step_qn,
            subpel_y_qn,
            y_step_qn,
        );
    }
}

/// Copy one 8-tap phase (0..=63) of the C normative resize filter
/// `svt_av1_resize_filter_normative`.
pub fn superres_filter_normative(phase: usize) -> [i16; 8] {
    let mut out = [0i16; 8];
    unsafe { ref_superres_filter_normative(phase as i32, out.as_mut_ptr()) };
    out
}

/// Reference one-row normative horizontal upscale (`upscale_normative_rect` ->
/// `av1_convolve_horiz_rs_c`). `input` indexes the first pixel of a row that has
/// >= 5 border bytes on each side (the rect replicates/restores them in place).
pub fn superres_upscale_row(
    input: &mut [u8],
    input_origin: usize,
    in_width: usize,
    output: &mut [u8],
    out_width: usize,
) {
    assert!(input_origin >= 5 && input_origin + in_width + 5 <= input.len());
    assert!(output.len() >= out_width);
    unsafe {
        ref_superres_upscale_row(
            input.as_ptr().add(input_origin),
            in_width as i32,
            output.as_mut_ptr(),
            out_width as i32,
        );
    }
}

/// Reference `highbd_upscale_normative_rect` ->
/// `av1_highbd_convolve_horiz_rs_c` (transcribed in the shim — both are
/// `static` in super_res.c). `input` indexes the first sample of a u16 row
/// that has >= 5 border samples on each side; the rect replicates the edge
/// samples into them and restores them in place, so the row content is
/// preserved across the call.
pub fn superres_upscale_row_hbd(
    input: &mut [u16],
    input_origin: usize,
    in_width: usize,
    output: &mut [u16],
    out_width: usize,
    bd: i32,
) {
    assert!(input_origin >= 5 && input_origin + in_width + 5 <= input.len());
    assert!(output.len() >= out_width);
    unsafe {
        ref_superres_upscale_row_hbd(
            input.as_mut_ptr().add(input_origin),
            in_width as i32,
            output.as_mut_ptr(),
            out_width as i32,
            bd,
        );
    }
}

/// Reference `svt_av1_upsample_intra_edge_c` with the block edge at
/// `p[origin..origin+sz]` (writes `p[origin-2..]`).
pub fn upsample_intra_edge(p: &mut [u8], origin: usize, sz: usize) {
    unsafe { svt_av1_upsample_intra_edge_c(p.as_mut_ptr().add(origin), sz as i32) }
}

/// Reference `svt_aom_intra_edge_filter_strength`.
pub fn intra_edge_filter_strength(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> i32 {
    unsafe { svt_aom_intra_edge_filter_strength(bs0, bs1, delta, filt_type) }
}

/// Reference `svt_aom_use_intra_edge_upsample`.
pub fn use_intra_edge_upsample(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> bool {
    unsafe { svt_aom_use_intra_edge_upsample(bs0, bs1, delta, filt_type) != 0 }
}

/// Reference dr predictor over origin-based edged buffers
/// (`above[origin+i]` = C `above_row[i]`), dispatching to the C
/// z1/z2/z3 kernels exactly like C `svt_aom_dr_predictor`.
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    bw: usize,
    bh: usize,
    angle: i32,
) {
    // C eb_dr_intra_derivative lookups (get_dx/get_dy, intra_prediction.c).
    const DR: [u16; 90] = [
        0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0,
        0, 132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0,
        51, 0, 0, 45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0,
        0, 0, 0, 11, 0, 0, 7, 0, 0, 3, 0, 0,
    ];
    let dx = if angle > 0 && angle < 90 {
        DR[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR[(180 - angle) as usize] as i32
    } else {
        1
    };
    let dy = if angle > 90 && angle < 180 {
        DR[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR[(270 - angle) as usize] as i32
    } else {
        1
    };
    unsafe {
        let a = above.as_ptr().add(origin);
        let l = left.as_ptr().add(origin);
        if angle > 0 && angle < 90 {
            svt_av1_dr_prediction_z1_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_above as i32,
                dx,
                dy,
            );
        } else if angle > 90 && angle < 180 {
            svt_av1_dr_prediction_z2_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_above as i32,
                upsample_left as i32,
                dx,
                dy,
            );
        } else if angle > 180 && angle < 270 {
            svt_av1_dr_prediction_z3_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_left as i32,
                dx,
                dy,
            );
        } else {
            panic!("dr_predictor_edged: exact 90/180 handled by V/H paths");
        }
    }
}

// ---------------------------------------------------------------------------

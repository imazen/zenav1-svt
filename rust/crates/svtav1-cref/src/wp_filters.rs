// wp-filters lane: Codec/{resize,restoration,restoration_pick,warped_motion,
// global_motion}.c oracles.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_get_frame_update_type(
        frame_type: i32,
        hierarchical_levels: i32,
        temporal_layer_index: i32,
    ) -> i32;
}

/// Reference `svt_aom_get_frame_update_type` (resize.c:1246). Returns a
/// `SvtAv1FrameUpdateType` discriminant: 0 = `KF_UPDATE`, 1 = `LF_UPDATE`,
/// 3 = `ARF_UPDATE`, 6 = `INTNL_ARF_UPDATE` (EbSvtAv1Enc.h:183).
///
/// `frame_type` is the AV1 `FrameType` (0 = KEY_FRAME, 1 = INTER_FRAME,
/// 2 = INTRA_ONLY_FRAME, 3 = S_FRAME).
pub fn get_frame_update_type(
    frame_type: i32,
    hierarchical_levels: i32,
    temporal_layer_index: i32,
) -> i32 {
    unsafe { ref_get_frame_update_type(frame_type, hierarchical_levels, temporal_layer_index) }
}

// --- warped_motion.c ---

unsafe extern "C" {
    pub(super) fn ref_warped_filter_row(phase: i32, out8: *mut i16);
    pub(super) fn ref_get_shear_params(mat6: *const i32, out4: *mut i16) -> i32;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_find_projection(
        np: i32,
        pts1: *const i32,
        pts2: *const i32,
        bsize: i32,
        mv_x: i16,
        mv_y: i16,
        mi_row: i32,
        mi_col: i32,
        out_mat6: *mut i32,
        out_shear4: *mut i16,
    ) -> i32;
    pub(super) fn ref_select_samples(
        mv_x: i16,
        mv_y: i16,
        pts: *mut i32,
        pts_inref: *mut i32,
        len: i32,
        bsize: i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_warp_plane(
        wm_io: *mut i32,
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
        subsampling_x: i32,
        subsampling_y: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_av1_warp_plane(
        wm_io: *mut i32,
        use_hbd: i32,
        bd: i32,
        ref8b: *const u8,
        ref2b: *const u8,
        width: i32,
        height: i32,
        stride: i32,
        pred: *mut u8,
        p_col: i32,
        p_row: i32,
        p_width: i32,
        p_height: i32,
        p_stride: i32,
        subsampling_x: i32,
        subsampling_y: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_warp_affine_sub(
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
        subsampling_x: i32,
        subsampling_y: i32,
        alpha: i16,
        beta: i16,
        gamma: i16,
        delta: i16,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_warp_affine_compound(
        mat: *const i32,
        r: *const u8,
        width: i32,
        height: i32,
        stride: i32,
        pred: *mut u8,
        dst: *mut u16,
        dst_stride: i32,
        do_average: i32,
        use_jnt_comp_avg: i32,
        fwd_offset: i32,
        bck_offset: i32,
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
    pub(super) fn ref_highbd_warp_affine(
        mat: *const i32,
        ref16: *const u16,
        width: i32,
        height: i32,
        stride: i32,
        pred: *mut u16,
        p_col: i32,
        p_row: i32,
        p_width: i32,
        p_height: i32,
        p_stride: i32,
        subsampling_x: i32,
        subsampling_y: i32,
        bd: i32,
        alpha: i16,
        beta: i16,
        gamma: i16,
        delta: i16,
    );
}

/// One phase of the real `svt_aom_warped_filter` (warped_motion.c:57).
/// `phase` is `0 ..= WARPEDPIXEL_PREC_SHIFTS * 3` (192).
pub fn warped_filter_row(phase: i32) -> [i16; 8] {
    assert!((0..=192).contains(&phase));
    let mut out = [0i16; 8];
    unsafe { ref_warped_filter_row(phase, out.as_mut_ptr()) };
    out
}

/// Reference `svt_get_shear_params` (warped_motion.c:907). Returns
/// `(allowed, [alpha, beta, gamma, delta])`.
pub fn get_shear_params(mat: &[i32; 6]) -> (bool, [i16; 4]) {
    let mut out = [0i16; 4];
    let ok = unsafe { ref_get_shear_params(mat.as_ptr(), out.as_mut_ptr()) };
    (ok != 0, out)
}

/// Reference `svt_find_projection` (warped_motion.c:473). Returns
/// `(failed, wmmat, [alpha, beta, gamma, delta])` — `failed` is C's own
/// nonzero-is-failure return.
#[allow(clippy::too_many_arguments)]
pub fn find_projection(
    pts1: &[i32],
    pts2: &[i32],
    bsize: i32,
    mv: (i16, i16),
    mi_row: i32,
    mi_col: i32,
) -> (bool, [i32; 6], [i16; 4]) {
    let np = pts1.len() / 2;
    assert_eq!(pts2.len(), pts1.len());
    // LEAST_SQUARES_SAMPLES_MAX is 8 in C; the shim's stack arrays are that
    // size, so refuse anything larger here rather than smashing them.
    assert!(np <= 8, "np {np} exceeds LEAST_SQUARES_SAMPLES_MAX");
    let mut mat = [0i32; 6];
    let mut shear = [0i16; 4];
    let failed = unsafe {
        ref_find_projection(
            np as i32,
            pts1.as_ptr(),
            pts2.as_ptr(),
            bsize,
            mv.0,
            mv.1,
            mi_row,
            mi_col,
            mat.as_mut_ptr(),
            shear.as_mut_ptr(),
        )
    };
    (failed != 0, mat, shear)
}

/// Reference `svt_aom_select_samples` (warped_motion.c:935). Compacts `pts`
/// and `pts_inref` in place and returns the retained count.
pub fn select_samples(
    mv: (i16, i16),
    pts: &mut [i32],
    pts_inref: &mut [i32],
    len: usize,
    bsize: i32,
) -> u8 {
    assert!(pts.len() >= 2 * len && pts_inref.len() >= 2 * len);
    let n = unsafe {
        ref_select_samples(
            mv.0,
            mv.1,
            pts.as_mut_ptr(),
            pts_inref.as_mut_ptr(),
            len as i32,
            bsize,
        )
    };
    n as u8
}

/// Reference `svt_warp_plane` (warped_motion.c:686), non-compound 8-bit.
/// `wm_io` is `[wmtype, mat0..mat5, alpha, beta, gamma, delta]` and is written
/// back so the ROTZOOM fix-up (which MUTATES the model) is observable.
#[allow(clippy::too_many_arguments)]
pub fn warp_plane(
    wm_io: &mut [i32; 11],
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
    subsampling_x: i32,
    subsampling_y: i32,
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_warp_plane(
            wm_io.as_mut_ptr(),
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
            subsampling_x,
            subsampling_y,
        );
    }
}

/// Reference `svt_av1_warp_plane` (warped_motion.c:868) with `use_hbd = 0` —
/// the bit-depth dispatcher's 8-bit arm.
#[allow(clippy::too_many_arguments)]
pub fn av1_warp_plane_lowbd(
    wm_io: &mut [i32; 11],
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
    subsampling_x: i32,
    subsampling_y: i32,
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_av1_warp_plane(
            wm_io.as_mut_ptr(),
            0,
            8,
            r.as_ptr(),
            core::ptr::null(),
            width as i32,
            height as i32,
            stride as i32,
            pred.as_mut_ptr(),
            p_col,
            p_row,
            p_width as i32,
            p_height as i32,
            p_stride as i32,
            subsampling_x,
            subsampling_y,
        );
    }
}

/// Reference `svt_av1_warp_affine_c` with explicit chroma subsampling — the
/// existing [`warp_affine`] hardwires `0, 0`.
#[allow(clippy::too_many_arguments)]
pub fn warp_affine_sub(
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
    subsampling_x: i32,
    subsampling_y: i32,
    shear: (i16, i16, i16, i16),
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_warp_affine_sub(
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
            subsampling_x,
            subsampling_y,
            shear.0,
            shear.1,
            shear.2,
            shear.3,
        );
    }
}

/// Reference `svt_av1_warp_affine_c`, COMPOUND arm. `dst` is the
/// `ConvBufType` accumulator; with `do_average` it is read and `pred` written.
#[allow(clippy::too_many_arguments)]
pub fn warp_affine_compound(
    mat: &[i32; 6],
    r: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    pred: &mut [u8],
    dst: &mut [u16],
    dst_stride: usize,
    do_average: bool,
    jnt: Option<(i32, i32)>,
    p_col: i32,
    p_row: i32,
    p_width: usize,
    p_height: usize,
    p_stride: usize,
    shear: (i16, i16, i16, i16),
) {
    assert!(r.len() >= height * stride);
    assert!(dst.len() >= (p_height - 1) * dst_stride + p_width);
    let (use_jnt, fwd, bck) = match jnt {
        Some((f, b)) => (1, f, b),
        None => (0, 0, 0),
    };
    unsafe {
        ref_warp_affine_compound(
            mat.as_ptr(),
            r.as_ptr(),
            width as i32,
            height as i32,
            stride as i32,
            pred.as_mut_ptr(),
            dst.as_mut_ptr(),
            dst_stride as i32,
            i32::from(do_average),
            use_jnt,
            fwd,
            bck,
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

/// Reference `svt_av1_highbd_warp_affine_c` (warped_motion.c:719),
/// non-compound. The shim splits the `u16` reference into SVT's 8+2 pair
/// (one byte per pixel in each plane, the low 2 bits in bits 7:6 of the 2b
/// plane) before calling, so the caller passes ordinary `u16` pixels.
#[allow(clippy::too_many_arguments)]
pub fn highbd_warp_affine(
    mat: &[i32; 6],
    r: &[u16],
    width: usize,
    height: usize,
    stride: usize,
    pred: &mut [u16],
    p_col: i32,
    p_row: i32,
    p_width: usize,
    p_height: usize,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    bd: i32,
    shear: (i16, i16, i16, i16),
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_highbd_warp_affine(
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
            subsampling_x,
            subsampling_y,
            bd,
            shear.0,
            shear.1,
            shear.2,
            shear.3,
        );
    }
}

// --- restoration.c: the self-guided (SGR) chain ---

unsafe extern "C" {
    pub(super) fn ref_decode_xq(xqd: *const i32, ep: i32, xq_out2: *mut i32);
    pub(super) fn ref_sgr_params(ep: i32, out4: *mut i32);
    pub(super) fn ref_sgr_x_by_xplus1(out256: *mut i32);
    pub(super) fn ref_sgr_one_by_x(out25: *mut i32);
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_selfguided_restoration_lbd(
        dgd8: *const u8,
        origin: i32,
        width: i32,
        height: i32,
        dgd_stride: i32,
        flt0: *mut i32,
        flt1: *mut i32,
        flt_stride: i32,
        sgr_params_idx: i32,
        bit_depth: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_selfguided_restoration_hbd(
        dgd16: *const u16,
        origin: i32,
        width: i32,
        height: i32,
        dgd_stride: i32,
        flt0: *mut i32,
        flt1: *mut i32,
        flt_stride: i32,
        sgr_params_idx: i32,
        bit_depth: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_apply_selfguided_restoration_lbd(
        dat8: *const u8,
        origin: i32,
        width: i32,
        height: i32,
        stride: i32,
        eps: i32,
        xqd: *const i32,
        dst8: *mut u8,
        dst_origin: i32,
        dst_stride: i32,
        bit_depth: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_apply_selfguided_restoration_hbd(
        dat16: *const u16,
        origin: i32,
        width: i32,
        height: i32,
        stride: i32,
        eps: i32,
        xqd: *const i32,
        dst16: *mut u16,
        dst_origin: i32,
        dst_stride: i32,
        bit_depth: i32,
    );
}

/// Reference `svt_decode_xq` (restoration.c:597) for one `ep` preset.
pub fn decode_xq(xqd: &[i32; 2], ep: i32) -> [i32; 2] {
    let mut out = [0i32; 2];
    unsafe { ref_decode_xq(xqd.as_ptr(), ep, out.as_mut_ptr()) };
    out
}

/// One entry of the real `svt_aom_eb_sgr_params` (restoration.c:31), as
/// `[r0, r1, s0, s1]`.
pub fn sgr_params(ep: i32) -> [i32; 4] {
    assert!((0..16).contains(&ep));
    let mut out = [0i32; 4];
    unsafe { ref_sgr_params(ep, out.as_mut_ptr()) };
    out
}

/// The real `svt_aom_eb_x_by_xplus1` table (restoration.c).
pub fn sgr_x_by_xplus1() -> [i32; 256] {
    let mut out = [0i32; 256];
    unsafe { ref_sgr_x_by_xplus1(out.as_mut_ptr()) };
    out
}

/// The real `svt_aom_eb_one_by_x` table (restoration.c).
pub fn sgr_one_by_x() -> [i32; 25] {
    let mut out = [0i32; 25];
    unsafe { ref_sgr_one_by_x(out.as_mut_ptr()) };
    out
}

/// Reference `svt_av1_selfguided_restoration_c` (restoration.c:886), 8-bit.
/// `origin` is the index of pixel `(0, 0)` in `dgd`; the kernel reads three
/// pixels outside the unit in every direction, so `dgd` must be extended.
#[allow(clippy::too_many_arguments)]
pub fn selfguided_restoration_lbd(
    dgd: &[u8],
    origin: usize,
    width: i32,
    height: i32,
    dgd_stride: usize,
    flt0: &mut [i32],
    flt1: &mut [i32],
    flt_stride: usize,
    sgr_params_idx: i32,
    bit_depth: i32,
) {
    unsafe {
        ref_selfguided_restoration_lbd(
            dgd.as_ptr(),
            origin as i32,
            width,
            height,
            dgd_stride as i32,
            flt0.as_mut_ptr(),
            flt1.as_mut_ptr(),
            flt_stride as i32,
            sgr_params_idx,
            bit_depth,
        );
    }
}

/// Reference `svt_av1_selfguided_restoration_c`, high bit depth. `dgd` is a
/// real `u16` plane (SVT's `CONVERT_TO_SHORTPTR` convention), NOT the 8+2
/// packed pair the warp kernels take.
#[allow(clippy::too_many_arguments)]
pub fn selfguided_restoration_hbd(
    dgd: &[u16],
    origin: usize,
    width: i32,
    height: i32,
    dgd_stride: usize,
    flt0: &mut [i32],
    flt1: &mut [i32],
    flt_stride: usize,
    sgr_params_idx: i32,
    bit_depth: i32,
) {
    unsafe {
        ref_selfguided_restoration_hbd(
            dgd.as_ptr(),
            origin as i32,
            width,
            height,
            dgd_stride as i32,
            flt0.as_mut_ptr(),
            flt1.as_mut_ptr(),
            flt_stride as i32,
            sgr_params_idx,
            bit_depth,
        );
    }
}

/// Reference `svt_apply_selfguided_restoration_c` (restoration.c:924), 8-bit.
#[allow(clippy::too_many_arguments)]
pub fn apply_selfguided_restoration_lbd(
    dat: &[u8],
    origin: usize,
    width: i32,
    height: i32,
    stride: usize,
    eps: i32,
    xqd: &[i32; 2],
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    bit_depth: i32,
) {
    unsafe {
        ref_apply_selfguided_restoration_lbd(
            dat.as_ptr(),
            origin as i32,
            width,
            height,
            stride as i32,
            eps,
            xqd.as_ptr(),
            dst.as_mut_ptr(),
            dst_origin as i32,
            dst_stride as i32,
            bit_depth,
        );
    }
}

/// Reference `svt_apply_selfguided_restoration_c`, high bit depth.
#[allow(clippy::too_many_arguments)]
pub fn apply_selfguided_restoration_hbd(
    dat: &[u16],
    origin: usize,
    width: i32,
    height: i32,
    stride: usize,
    eps: i32,
    xqd: &[i32; 2],
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bit_depth: i32,
) {
    unsafe {
        ref_apply_selfguided_restoration_hbd(
            dat.as_ptr(),
            origin as i32,
            width,
            height,
            stride as i32,
            eps,
            xqd.as_ptr(),
            dst.as_mut_ptr(),
            dst_origin as i32,
            dst_stride as i32,
            bit_depth,
        );
    }
}

// --- restoration_pick.c: the SGR search kernels ---

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_lowbd_pixel_proj_error(
        src8: *const u8,
        src_origin: i32,
        width: i32,
        height: i32,
        src_stride: i32,
        dat8: *const u8,
        dat_origin: i32,
        dat_stride: i32,
        flt0: *mut i32,
        flt0_stride: i32,
        flt1: *mut i32,
        flt1_stride: i32,
        xq: *const i32,
        ep: i32,
    ) -> i64;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_highbd_pixel_proj_error(
        src16: *const u16,
        src_origin: i32,
        width: i32,
        height: i32,
        src_stride: i32,
        dat16: *const u16,
        dat_origin: i32,
        dat_stride: i32,
        flt0: *mut i32,
        flt0_stride: i32,
        flt1: *mut i32,
        flt1_stride: i32,
        xq: *const i32,
        ep: i32,
    ) -> i64;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_get_proj_subspace(
        src8: *const u8,
        src_origin: i32,
        width: i32,
        height: i32,
        src_stride: i32,
        dat8: *const u8,
        dat_origin: i32,
        dat_stride: i32,
        flt0: *mut i32,
        flt0_stride: i32,
        flt1: *mut i32,
        flt1_stride: i32,
        ep: i32,
        xq_out2: *mut i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_get_proj_subspace_hbd(
        src16: *const u16,
        src_origin: i32,
        width: i32,
        height: i32,
        src_stride: i32,
        dat16: *const u16,
        dat_origin: i32,
        dat_stride: i32,
        flt0: *mut i32,
        flt0_stride: i32,
        flt1: *mut i32,
        flt1_stride: i32,
        ep: i32,
        xq_out2: *mut i32,
    );
}

/// Reference `svt_av1_lowbd_pixel_proj_error_c` (restoration_pick.c:161).
/// `xq` is the DECODED weight pair, not the signalled `xqd`.
#[allow(clippy::too_many_arguments)]
pub fn lowbd_pixel_proj_error(
    src: &[u8],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u8],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &mut [i32],
    flt0_stride: usize,
    flt1: &mut [i32],
    flt1_stride: usize,
    xq: &[i32; 2],
    ep: i32,
) -> i64 {
    unsafe {
        ref_lowbd_pixel_proj_error(
            src.as_ptr(),
            src_origin as i32,
            width as i32,
            height as i32,
            src_stride as i32,
            dat.as_ptr(),
            dat_origin as i32,
            dat_stride as i32,
            flt0.as_mut_ptr(),
            flt0_stride as i32,
            flt1.as_mut_ptr(),
            flt1_stride as i32,
            xq.as_ptr(),
            ep,
        )
    }
}

/// Reference `svt_av1_highbd_pixel_proj_error_c` (restoration_pick.c:228).
#[allow(clippy::too_many_arguments)]
pub fn highbd_pixel_proj_error(
    src: &[u16],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u16],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &mut [i32],
    flt0_stride: usize,
    flt1: &mut [i32],
    flt1_stride: usize,
    xq: &[i32; 2],
    ep: i32,
) -> i64 {
    unsafe {
        ref_highbd_pixel_proj_error(
            src.as_ptr(),
            src_origin as i32,
            width as i32,
            height as i32,
            src_stride as i32,
            dat.as_ptr(),
            dat_origin as i32,
            dat_stride as i32,
            flt0.as_mut_ptr(),
            flt0_stride as i32,
            flt1.as_mut_ptr(),
            flt1_stride as i32,
            xq.as_ptr(),
            ep,
        )
    }
}

/// Reference `svt_get_proj_subspace_c` (restoration_pick.c:422), 8-bit.
#[allow(clippy::too_many_arguments)]
pub fn get_proj_subspace(
    src: &[u8],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u8],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &mut [i32],
    flt0_stride: usize,
    flt1: &mut [i32],
    flt1_stride: usize,
    ep: i32,
) -> [i32; 2] {
    let mut out = [0i32; 2];
    unsafe {
        ref_get_proj_subspace(
            src.as_ptr(),
            src_origin as i32,
            width as i32,
            height as i32,
            src_stride as i32,
            dat.as_ptr(),
            dat_origin as i32,
            dat_stride as i32,
            flt0.as_mut_ptr(),
            flt0_stride as i32,
            flt1.as_mut_ptr(),
            flt1_stride as i32,
            ep,
            out.as_mut_ptr(),
        );
    }
    out
}

/// Reference `svt_get_proj_subspace_c`, high bit depth.
#[allow(clippy::too_many_arguments)]
pub fn get_proj_subspace_hbd(
    src: &[u16],
    src_origin: usize,
    width: usize,
    height: usize,
    src_stride: usize,
    dat: &[u16],
    dat_origin: usize,
    dat_stride: usize,
    flt0: &mut [i32],
    flt0_stride: usize,
    flt1: &mut [i32],
    flt1_stride: usize,
    ep: i32,
) -> [i32; 2] {
    let mut out = [0i32; 2];
    unsafe {
        ref_get_proj_subspace_hbd(
            src.as_ptr(),
            src_origin as i32,
            width as i32,
            height as i32,
            src_stride as i32,
            dat.as_ptr(),
            dat_origin as i32,
            dat_stride as i32,
            flt0.as_mut_ptr(),
            flt0_stride as i32,
            flt1.as_mut_ptr(),
            flt1_stride as i32,
            ep,
            out.as_mut_ptr(),
        );
    }
    out
}

// --- global_motion.c + enc_warped_motion.c: the GM model chain ---

unsafe extern "C" {
    pub(super) fn ref_convert_model_to_params(params6: *const f64, out7: *mut i32);
    pub(super) fn ref_gm_get_params_cost(gm7: *const i32, ref7: *const i32, allow_hp: i32) -> i32;
    pub(super) fn ref_is_enough_erroradvantage(
        best_erroradvantage: f64,
        params_cost: i32,
        erroradv_type: i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_warp_error(
        wm_io: *mut i32,
        r: *const u8,
        width: i32,
        height: i32,
        stride: i32,
        dst: *const u8,
        dst_origin: i32,
        p_col: i32,
        p_row: i32,
        p_width: i32,
        p_height: i32,
        p_stride: i32,
        subsampling_x: i32,
        subsampling_y: i32,
        chess_refn: i32,
        best_error: i64,
    ) -> i64;
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_refine_integerized_param(
        rfn_early_exit: i32,
        wm_io: *mut i32,
        wmtype: i32,
        r: *const u8,
        r_width: i32,
        r_height: i32,
        r_stride: i32,
        dst: *const u8,
        d_width: i32,
        d_height: i32,
        d_stride: i32,
        n_refinements: i32,
        chess_refn: i32,
        best_frame_error: i64,
        pic_sad: u32,
        params_cost: i32,
    ) -> i64;
}

/// Reference `svt_av1_convert_model_to_params` (global_motion.c:63). Returns
/// `(wmtype, wmmat)`.
pub fn convert_model_to_params(params: &[f64; 6]) -> (i32, [i32; 6]) {
    let mut out = [0i32; 7];
    unsafe { ref_convert_model_to_params(params.as_ptr(), out.as_mut_ptr()) };
    (out[0], [out[1], out[2], out[3], out[4], out[5], out[6]])
}

/// Reference `svt_aom_gm_get_params_cost` (global_me_cost.c:24). `gm` / `ref_gm`
/// are `[wmtype, wmmat0..wmmat5]`.
#[must_use]
pub fn gm_get_params_cost(gm: &[i32; 7], ref_gm: &[i32; 7], allow_hp: bool) -> i32 {
    unsafe { ref_gm_get_params_cost(gm.as_ptr(), ref_gm.as_ptr(), i32::from(allow_hp)) }
}

/// Reference `svt_av1_is_enough_erroradvantage` (global_motion.c:30).
pub fn is_enough_erroradvantage(
    best_erroradvantage: f64,
    params_cost: i32,
    erroradv_type: i32,
) -> bool {
    unsafe { ref_is_enough_erroradvantage(best_erroradvantage, params_cost, erroradv_type) != 0 }
}

/// Reference `svt_av1_warp_error` (enc_warped_motion.c:77). `wm_io` is
/// `[wmtype, mat0..mat5, alpha, beta, gamma, delta]`, written back so the
/// shear derivation the function performs is observable.
#[allow(clippy::too_many_arguments)]
pub fn warp_error(
    wm_io: &mut [i32; 11],
    r: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    dst: &[u8],
    dst_origin: usize,
    p_col: i32,
    p_row: i32,
    p_width: i32,
    p_height: i32,
    p_stride: usize,
    subsampling_x: i32,
    subsampling_y: i32,
    chess_refn: bool,
    best_error: i64,
) -> i64 {
    assert!(r.len() >= height * stride);
    unsafe {
        ref_warp_error(
            wm_io.as_mut_ptr(),
            r.as_ptr(),
            width as i32,
            height as i32,
            stride as i32,
            dst.as_ptr(),
            dst_origin as i32,
            p_col,
            p_row,
            p_width,
            p_height,
            p_stride as i32,
            subsampling_x,
            subsampling_y,
            i32::from(chess_refn),
            best_error,
        )
    }
}

/// Reference `svt_av1_refine_integerized_param` (global_motion.c:117).
/// `wm_io` is refined in place, same layout as [`warp_error`].
#[allow(clippy::too_many_arguments)]
pub fn refine_integerized_param(
    rfn_early_exit: bool,
    wm_io: &mut [i32; 11],
    wmtype: i32,
    r: &[u8],
    r_width: usize,
    r_height: usize,
    r_stride: usize,
    dst: &[u8],
    d_width: i32,
    d_height: i32,
    d_stride: usize,
    n_refinements: i32,
    chess_refn: bool,
    best_frame_error: i64,
    pic_sad: u32,
    params_cost: i32,
) -> i64 {
    assert!(r.len() >= r_height * r_stride);
    unsafe {
        ref_refine_integerized_param(
            i32::from(rfn_early_exit),
            wm_io.as_mut_ptr(),
            wmtype,
            r.as_ptr(),
            r_width as i32,
            r_height as i32,
            r_stride as i32,
            dst.as_ptr(),
            d_width,
            d_height,
            d_stride as i32,
            n_refinements,
            i32::from(chess_refn),
            best_frame_error,
            pic_sad,
            params_cost,
        )
    }
}

// --- resize.c: the high-bit-depth resize ladder ---

unsafe extern "C" {
    pub(super) fn ref_highbd_interpolate_core(
        input: *const u16,
        in_length: i32,
        output: *mut u16,
        out_length: i32,
        bd: i32,
        bank: i32,
    );
    pub(super) fn ref_highbd_down2_symeven(
        input: *const u16,
        length: i32,
        output: *mut u16,
        bd: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_highbd_resize_plane_horizontal(
        input: *const u16,
        height: i32,
        width: i32,
        in_stride: i32,
        output: *mut u16,
        width2: i32,
        out_stride: i32,
        bd: i32,
    ) -> i32;
}

/// Reference `svt_av1_highbd_interpolate_core_c` (resize.c:489) with an
/// explicit filter bank (0 = filters1000 / normative, 1 = 875, 2 = 750,
/// 3 = 625, 4 = 500 — C's `choose_interp_filter` ladder).
pub fn highbd_interpolate_core(
    input: &[u16],
    in_length: usize,
    output: &mut [u16],
    out_length: usize,
    bd: i32,
    bank: i32,
) {
    assert!(input.len() >= in_length && output.len() >= out_length);
    unsafe {
        ref_highbd_interpolate_core(
            input.as_ptr(),
            in_length as i32,
            output.as_mut_ptr(),
            out_length as i32,
            bd,
            bank,
        );
    }
}

/// Reference `svt_av1_highbd_down2_symeven_c` (resize.c:568).
pub fn highbd_down2_symeven(input: &[u16], length: usize, output: &mut [u16], bd: i32) {
    assert!(input.len() >= length && output.len() >= length.div_ceil(2));
    unsafe { ref_highbd_down2_symeven(input.as_ptr(), length as i32, output.as_mut_ptr(), bd) };
}

/// Reference `svt_av1_highbd_resize_plane_horizontal` (resize.c:761) — the
/// high-bit-depth superres SOURCE downscale, `width` -> `width2` at unchanged
/// height. Drives C's `static` `highbd_resize_multistep` (down2 steps +
/// polyphase interpolate), so this one oracle covers every arm of the ladder.
#[allow(clippy::too_many_arguments)]
pub fn highbd_resize_plane_horizontal(
    input: &[u16],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u16],
    width2: usize,
    out_stride: usize,
    bd: i32,
) {
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height - 1) * out_stride + width2);
    let rc = unsafe {
        ref_highbd_resize_plane_horizontal(
            input.as_ptr(),
            height as i32,
            width as i32,
            in_stride as i32,
            output.as_mut_ptr(),
            width2 as i32,
            out_stride as i32,
            bd,
        )
    };
    assert_eq!(
        rc, 0,
        "svt_av1_highbd_resize_plane_horizontal failed (rc {rc})"
    );
}

// --- ransac.c ---

unsafe extern "C" {
    pub(super) fn ref_ransac(
        pts4: *const i32,
        npoints: i32,
        ty: i32,
        num_desired_motions: i32,
        out_params: *mut f64,
        out_num_inliers: *mut i32,
        out_inliers: *mut i32,
    ) -> i32;
}

/// One model as `svt_aom_ransac` produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct RansacModelOut {
    pub params: [f64; 6],
    pub num_inliers: usize,
    /// Interleaved `[x0, y0, x1, y1, ...]`, `num_inliers` pairs.
    pub inliers: Vec<i32>,
}

/// Reference `svt_aom_ransac` (ransac.c:428). `points` is interleaved
/// `[x, y, rx, ry]` per correspondence; `ty` is the `TransformationType`
/// (1 = TRANSLATION, 2 = ROTZOOM, 3 = AFFINE — IDENTITY is rejected by C's
/// own assert). Returns `(ok, models)`.
pub fn ransac(points: &[i32], ty: i32, num_desired_motions: usize) -> (bool, Vec<RansacModelOut>) {
    assert_eq!(points.len() % 4, 0);
    let npoints = points.len() / 4;
    assert!((1..=3).contains(&ty));
    let mut params = vec![0f64; num_desired_motions * 6];
    let mut counts = vec![0i32; num_desired_motions];
    let mut inliers = vec![0i32; num_desired_motions * 2 * npoints.max(1)];
    let ok = unsafe {
        ref_ransac(
            points.as_ptr(),
            npoints as i32,
            ty,
            num_desired_motions as i32,
            params.as_mut_ptr(),
            counts.as_mut_ptr(),
            inliers.as_mut_ptr(),
        )
    };
    let models = (0..num_desired_motions)
        .map(|i| {
            let n = counts[i] as usize;
            let base = i * 2 * npoints.max(1);
            RansacModelOut {
                params: params[i * 6..i * 6 + 6].try_into().unwrap(),
                num_inliers: n,
                inliers: inliers[base..base + 2 * n].to_vec(),
            }
        })
        .collect();
    (ok != 0, models)
}

unsafe extern "C" {
    pub(super) fn ref_dc_intra_pred(
        dst: *mut u8,
        stride: isize,
        above: *const u8,
        left: *const u8,
        w: i32,
        h: i32,
        has_above: i32,
        has_left: i32,
    );
}

/// Call C's sized 8-bit DC predictor, retaining padding in the caller's buffer.
#[allow(clippy::too_many_arguments)]
pub fn dc_intra_pred(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    width: usize,
    height: usize,
    has_above: bool,
    has_left: bool,
) {
    assert!([4usize, 8, 16, 32, 64].contains(&width) && [4usize, 8, 16, 32, 64].contains(&height));
    assert!(width.max(height) <= 4 * width.min(height));
    assert!(stride <= isize::MAX as usize);
    let storage = height.checked_mul(stride).unwrap();
    let last = (height - 1)
        .checked_mul(stride)
        .unwrap()
        .checked_add(width)
        .unwrap();
    assert!(dst.len() >= storage.max(last));
    assert!(!has_above || above.len() >= width);
    assert!(!has_left || left.len() >= height);
    unsafe {
        ref_dc_intra_pred(
            dst.as_mut_ptr(),
            stride as isize,
            above.as_ptr(),
            left.as_ptr(),
            width as i32,
            height as i32,
            has_above as i32,
            has_left as i32,
        );
    }
}

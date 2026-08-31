//! FFI bindings for the open-loop ME oracle (inter campaign chunk C4,
//! `rust/docs/INTER-ENCODE-PLAN.md` §2).
//!
//! Backed by `shims/inter_me_shims.c`, which calls the REAL exported C symbols
//! `svt_aom_compute8x4_sad_kernel_c`, `svt_ext_sad_calculation_8x8_16x16_c`,
//! `svt_ext_sad_calculation_32x32_64x64_c`,
//! `svt_ext_all_sad_calculation_8x8_16x16_c`,
//! `svt_ext_eight_sad_calculation_32x32_64x64_c`,
//! `svt_nxm_sad_kernel_helper_c`, `svt_sad_loop_kernel_c`,
//! `svt_aom_get_scaled_picture_distance`, `hme_level_2` and `check_00_center`
//! — evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with the concurrent C0/C2/C3 lanes.

unsafe extern "C" {
    fn ref_me_compute8x4_sad(src: *const u8, src_stride: u32, r: *const u8, ref_stride: u32)
    -> u32;
    #[allow(clippy::too_many_arguments)]
    fn ref_me_ext_sad_8x8_16x16(
        src: *const u8,
        src_stride: u32,
        r: *const u8,
        ref_stride: u32,
        best_sad: *mut u32,
        best_mv: *mut u32,
        off8: u32,
        off16: u32,
        mv: u32,
        p_sad16x16: *mut u32,
        i16_: u32,
        p_sad8x8: *mut u32,
        i8_: u32,
        sub_sad: i32,
    );
    fn ref_me_ext_sad_32x32_64x64(
        p_sad16x16: *const u32,
        best_sad: *mut u32,
        best_mv: *mut u32,
        off32: u32,
        off64: u32,
        mv: u32,
        p_sad32x32: *mut u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_me_ext_all_sad_8x8_16x16(
        src: *const u8,
        src_stride: u32,
        r: *const u8,
        ref_stride: u32,
        mv: u32,
        best_sad: *mut u32,
        best_mv: *mut u32,
        off8: u32,
        off16: u32,
        p_eight_sad16x16: *mut u32,
        sub_sad: i32,
    );
    fn ref_me_ext_eight_sad_32x32_64x64(
        p_sad16x16: *const u32,
        best_sad: *mut u32,
        best_mv: *mut u32,
        off32: u32,
        off64: u32,
        mv: u32,
        p_sad32x32: *mut u32,
    );
    fn ref_me_nxm_sad(
        src: *const u8,
        src_stride: u32,
        r: *const u8,
        ref_stride: u32,
        height: u32,
        width: u32,
    ) -> u32;
    #[allow(clippy::too_many_arguments)]
    fn ref_me_sad_loop_kernel(
        src: *const u8,
        src_stride: u32,
        r: *const u8,
        ref_stride: u32,
        block_height: u32,
        block_width: u32,
        best_sad: *mut u64,
        x_sc: *mut i16,
        y_sc: *mut i16,
        src_stride_raw: u32,
        skip_search_line: u8,
        sa_w: i16,
        sa_h: i16,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_me_sad_loop_kernel_rtcd(
        src: *const u8,
        src_stride: u32,
        r: *const u8,
        ref_stride: u32,
        block_height: u32,
        block_width: u32,
        best_sad: *mut u64,
        x_sc: *mut i16,
        y_sc: *mut i16,
        src_stride_raw: u32,
        skip_search_line: u8,
        sa_w: i16,
        sa_h: i16,
    );
    fn ref_me_get_scaled_picture_distance(dist: u16) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn ref_me_hme_level_2(
        b64_src: *const u8,
        b64_src_stride: u32,
        hme_search_method: u8,
        ref_alloc: *const u8,
        ref_org: u32,
        ref_stride: u16,
        ref_w: u16,
        ref_h: u16,
        org_x: i16,
        org_y: i16,
        block_width: u32,
        block_height: u32,
        sa_width: i16,
        sa_height: i16,
        l1x: i16,
        l1y: i16,
        best_sad: *mut u64,
        sc_x: *mut i16,
        sc_y: *mut i16,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_me_check_00_center(
        b64_src: *const u8,
        b64_src_stride: u32,
        me_early_exit_th: u32,
        ref_alloc: *const u8,
        ref_org: u32,
        ref_stride: u16,
        ref_w: u16,
        ref_h: u16,
        sb_origin_x: u32,
        sb_origin_y: u32,
        sb_width: u32,
        sb_height: u32,
        x_sc: *mut i16,
        y_sc: *mut i16,
        zz_sad: u32,
    ) -> u32;
}

/// C `svt_aom_compute8x4_sad_kernel_c` (motion_estimation.c:43).
pub fn compute8x4_sad(src: &[u8], src_stride: usize, r: &[u8], ref_stride: usize) -> u32 {
    assert!(src.len() >= 3 * src_stride + 8 && r.len() >= 3 * ref_stride + 8);
    unsafe {
        ref_me_compute8x4_sad(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr(),
            ref_stride as u32,
        )
    }
}

/// C `svt_ext_sad_calculation_8x8_16x16_c` (motion_estimation.c:100).
#[allow(clippy::too_many_arguments)]
pub fn ext_sad_calculation_8x8_16x16(
    src: &[u8],
    src_stride: usize,
    r: &[u8],
    ref_stride: usize,
    best_sad: &mut [u32; 85],
    best_mv: &mut [u32; 85],
    off8: usize,
    off16: usize,
    mv: u32,
    p_sad16x16: &mut [u32; 16],
    i16_: usize,
    p_sad8x8: &mut [u32; 64],
    i8_: usize,
    sub_sad: bool,
) {
    unsafe {
        ref_me_ext_sad_8x8_16x16(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr(),
            ref_stride as u32,
            best_sad.as_mut_ptr(),
            best_mv.as_mut_ptr(),
            off8 as u32,
            off16 as u32,
            mv,
            p_sad16x16.as_mut_ptr(),
            i16_ as u32,
            p_sad8x8.as_mut_ptr(),
            i8_ as u32,
            i32::from(sub_sad),
        );
    }
}

/// C `svt_ext_sad_calculation_32x32_64x64_c` (motion_estimation.c:164).
pub fn ext_sad_calculation_32x32_64x64(
    p_sad16x16: &[u32; 16],
    best_sad: &mut [u32; 85],
    best_mv: &mut [u32; 85],
    off32: usize,
    off64: usize,
    mv: u32,
    p_sad32x32: &mut [u32; 4],
) {
    unsafe {
        ref_me_ext_sad_32x32_64x64(
            p_sad16x16.as_ptr(),
            best_sad.as_mut_ptr(),
            best_mv.as_mut_ptr(),
            off32 as u32,
            off64 as u32,
            mv,
            p_sad32x32.as_mut_ptr(),
        );
    }
}

/// C `svt_ext_all_sad_calculation_8x8_16x16_c` (motion_estimation.c:318).
#[allow(clippy::too_many_arguments)]
pub fn ext_all_sad_calculation_8x8_16x16(
    src: &[u8],
    src_stride: usize,
    r: &[u8],
    ref_stride: usize,
    mv: u32,
    best_sad: &mut [u32; 85],
    best_mv: &mut [u32; 85],
    off8: usize,
    off16: usize,
    p_eight_sad16x16: &mut [[u32; 8]; 16],
    sub_sad: bool,
) {
    unsafe {
        ref_me_ext_all_sad_8x8_16x16(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr(),
            ref_stride as u32,
            mv,
            best_sad.as_mut_ptr(),
            best_mv.as_mut_ptr(),
            off8 as u32,
            off16 as u32,
            p_eight_sad16x16.as_mut_ptr().cast::<u32>(),
            i32::from(sub_sad),
        );
    }
}

/// C `svt_ext_eight_sad_calculation_32x32_64x64_c` (motion_estimation.c:351).
pub fn ext_eight_sad_calculation_32x32_64x64(
    p_sad16x16: &[[u32; 8]; 16],
    best_sad: &mut [u32; 85],
    best_mv: &mut [u32; 85],
    off32: usize,
    off64: usize,
    mv: u32,
    p_sad32x32: &mut [[u32; 8]; 4],
) {
    unsafe {
        ref_me_ext_eight_sad_32x32_64x64(
            p_sad16x16.as_ptr().cast::<u32>(),
            best_sad.as_mut_ptr(),
            best_mv.as_mut_ptr(),
            off32 as u32,
            off64 as u32,
            mv,
            p_sad32x32.as_mut_ptr().cast::<u32>(),
        );
    }
}

/// C `svt_nxm_sad_kernel_helper_c` (C_DEFAULT/compute_sad_c.c:21).
pub fn nxm_sad(
    src: &[u8],
    src_stride: usize,
    r: &[u8],
    ref_stride: usize,
    height: usize,
    width: usize,
) -> u32 {
    assert!(src.len() >= (height - 1) * src_stride + width);
    assert!(r.len() >= (height - 1) * ref_stride + width);
    unsafe {
        ref_me_nxm_sad(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr(),
            ref_stride as u32,
            height as u32,
            width as u32,
        )
    }
}

/// `(best_sad, x_search_center, y_search_center)`.
pub type SadLoopOut = (u64, i16, i16);

/// C `svt_sad_loop_kernel_c` (C_DEFAULT/compute_sad_c.c:63).
#[allow(clippy::too_many_arguments)]
pub fn sad_loop_kernel(
    src: &[u8],
    src_stride: usize,
    r: &[u8],
    ref_base: usize,
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    src_stride_raw: usize,
    skip_search_line: u8,
    sa_w: i16,
    sa_h: i16,
) -> SadLoopOut {
    let mut best_sad = 0u64;
    let mut x = 0i16;
    let mut y = 0i16;
    unsafe {
        ref_me_sad_loop_kernel(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr().add(ref_base),
            ref_stride as u32,
            block_height as u32,
            block_width as u32,
            &mut best_sad,
            &mut x,
            &mut y,
            src_stride_raw as u32,
            skip_search_line,
            sa_w,
            sa_h,
        );
    }
    (best_sad, x, y)
}

/// The RTCD-dispatched `svt_sad_loop_kernel` — whatever SIMD tier this host
/// selects. Used as a positive control that the host's kernel agrees with the
/// `_c` one the port transcribes.
#[allow(clippy::too_many_arguments)]
pub fn sad_loop_kernel_rtcd(
    src: &[u8],
    src_stride: usize,
    r: &[u8],
    ref_base: usize,
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    src_stride_raw: usize,
    skip_search_line: u8,
    sa_w: i16,
    sa_h: i16,
) -> SadLoopOut {
    let mut best_sad = 0u64;
    let mut x = 0i16;
    let mut y = 0i16;
    unsafe {
        ref_me_sad_loop_kernel_rtcd(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr().add(ref_base),
            ref_stride as u32,
            block_height as u32,
            block_width as u32,
            &mut best_sad,
            &mut x,
            &mut y,
            src_stride_raw as u32,
            skip_search_line,
            sa_w,
            sa_h,
        );
    }
    (best_sad, x, y)
}

/// C `svt_aom_get_scaled_picture_distance` (motion_estimation.c:1152).
pub fn get_scaled_picture_distance(dist: u16) -> u16 {
    unsafe { ref_me_get_scaled_picture_distance(dist) }
}

/// C `hme_level_2` (motion_estimation.c:971). `ref_alloc` is the whole padded
/// allocation and `ref_org` the index of pixel (0,0) inside it.
#[allow(clippy::too_many_arguments)]
pub fn hme_level_2(
    b64_src: &[u8],
    b64_src_stride: usize,
    hme_search_method: u8,
    ref_alloc: &[u8],
    ref_org: usize,
    ref_stride: u16,
    ref_w: u16,
    ref_h: u16,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    sa_width: i16,
    sa_height: i16,
    l1x: i16,
    l1y: i16,
) -> SadLoopOut {
    let mut best_sad = 0u64;
    let mut x = 0i16;
    let mut y = 0i16;
    unsafe {
        ref_me_hme_level_2(
            b64_src.as_ptr(),
            b64_src_stride as u32,
            hme_search_method,
            ref_alloc.as_ptr(),
            ref_org as u32,
            ref_stride,
            ref_w,
            ref_h,
            org_x,
            org_y,
            block_width,
            block_height,
            sa_width,
            sa_height,
            l1x,
            l1y,
            &mut best_sad,
            &mut x,
            &mut y,
        );
    }
    (best_sad, x, y)
}

/// C `check_00_center` (motion_estimation.c:1060). Returns
/// `(hme_mv_sad, x_search_center, y_search_center)`.
#[allow(clippy::too_many_arguments)]
pub fn check_00_center(
    b64_src: &[u8],
    b64_src_stride: usize,
    me_early_exit_th: u32,
    ref_alloc: &[u8],
    ref_org: usize,
    ref_stride: u16,
    ref_w: u16,
    ref_h: u16,
    sb_origin_x: u32,
    sb_origin_y: u32,
    sb_width: u32,
    sb_height: u32,
    x_sc: i16,
    y_sc: i16,
    zz_sad: u32,
) -> (u32, i16, i16) {
    let mut x = x_sc;
    let mut y = y_sc;
    let r = unsafe {
        ref_me_check_00_center(
            b64_src.as_ptr(),
            b64_src_stride as u32,
            me_early_exit_th,
            ref_alloc.as_ptr(),
            ref_org as u32,
            ref_stride,
            ref_w,
            ref_h,
            sb_origin_x,
            sb_origin_y,
            sb_width,
            sb_height,
            &mut x,
            &mut y,
            zz_sad,
        )
    };
    (r, x, y)
}

// ---------------------------------------------------------------------------
// av1me.c — the OBMC search and the four C_DEFAULT kernels it drives.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_obmc_kernel(
        which: i32,
        width: i32,
        height: i32,
        pre: *const u8,
        pre_stride: i32,
        xoffset: i32,
        yoffset: i32,
        wsrc: *const i32,
        mask: *const i32,
        sse: *mut u32,
    ) -> u32;
    #[allow(clippy::too_many_arguments)]
    fn ref_upsampled_pred(
        comp_pred: *mut u8,
        width: i32,
        height: i32,
        subpel_x_q3: i32,
        subpel_y_q3: i32,
        ref_alloc: *const u8,
        ref_base: i32,
        ref_stride: i32,
        subpel_search: i32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_me_convolve8_horiz(
        src_alloc: *const u8,
        src_base: i32,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        kernel: *const i16,
        w: i32,
        h: i32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_me_convolve8_vert(
        src_alloc: *const u8,
        src_base: i32,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        kernel: *const i16,
        w: i32,
        h: i32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_obmc_full_pixel_search(
        pre_alloc: *const u8,
        pre_base: i32,
        pre_stride: i32,
        wsrc: *mut i32,
        mask: *mut i32,
        bsize: i32,
        mvp_x: i32,
        mvp_y: i32,
        sadpb: i32,
        ref_mv_x: i32,
        ref_mv_y: i32,
        col_min: i32,
        col_max: i32,
        row_min: i32,
        row_max: i32,
        mv_joint: *const i32,
        mv_cost0: *const i32,
        mv_cost1: *const i32,
        errorperbit: i32,
        approx_inter_rate: i32,
        fpel_range: i32,
        fpel_diag: i32,
        out_x: *mut i32,
        out_y: *mut i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_obmc_sub_pixel_tree_up(
        pre_alloc: *const u8,
        pre_base: i32,
        pre_stride: i32,
        wsrc: *mut i32,
        mask: *mut i32,
        bsize: i32,
        best_x: i32,
        best_y: i32,
        ref_mv_x: i32,
        ref_mv_y: i32,
        allow_hp: i32,
        errorperbit: i32,
        forced_stop: i32,
        iters_per_step: i32,
        col_min: i32,
        col_max: i32,
        row_min: i32,
        row_max: i32,
        mv_joint: *const i32,
        mv_cost0: *const i32,
        mv_cost1: *const i32,
        approx_inter_rate: i32,
        use_accurate_subpel_search: i32,
        out_x: *mut i32,
        out_y: *mut i32,
        out_distortion: *mut i32,
        out_sse: *mut u32,
    ) -> u32;
}

/// The MV cost tables C reads through `x->nmv_vec_cost` / `x->mv_cost_stack`.
/// `comp[i]` is indexed from `MV_MAX` in C, so each half must be
/// `2 * MV_MAX + 1` entries long.
pub struct RefMvCost<'a> {
    /// C `x->nmv_vec_cost` — 4 joint costs.
    pub joint: &'a [i32; 4],
    /// C `x->mv_cost_stack[0]` before the `+ MV_MAX` bias.
    pub comp0: &'a [i32],
    /// C `x->mv_cost_stack[1]` before the `+ MV_MAX` bias.
    pub comp1: &'a [i32],
}

/// C `svt_aom_obmc_sadWxH_c` (C_DEFAULT/sad_av1.c). Panics on a size the shim
/// does not instantiate rather than returning a sentinel nobody checks.
pub fn obmc_sad(
    pre: &[u8],
    pre_stride: usize,
    wsrc: &[i32],
    mask: &[i32],
    w: usize,
    h: usize,
) -> u32 {
    let mut sse = 0u32;
    let r = unsafe {
        ref_obmc_kernel(
            0,
            w as i32,
            h as i32,
            pre.as_ptr(),
            pre_stride as i32,
            0,
            0,
            wsrc.as_ptr(),
            mask.as_ptr(),
            &mut sse,
        )
    };
    assert_ne!(r, u32::MAX, "the C shim has no obmc_sad{w}x{h}");
    r
}

/// C `svt_aom_obmc_varianceWxH_c` (C_DEFAULT/variance.c). Returns
/// `(return_value, sse)`.
pub fn obmc_variance(
    pre: &[u8],
    pre_stride: usize,
    wsrc: &[i32],
    mask: &[i32],
    w: usize,
    h: usize,
) -> (u32, u32) {
    let mut sse = 0u32;
    let r = unsafe {
        ref_obmc_kernel(
            1,
            w as i32,
            h as i32,
            pre.as_ptr(),
            pre_stride as i32,
            0,
            0,
            wsrc.as_ptr(),
            mask.as_ptr(),
            &mut sse,
        )
    };
    assert_ne!(r, u32::MAX, "the C shim has no obmc_variance{w}x{h}");
    (r, sse)
}

/// C `svt_aom_obmc_sub_pixel_varianceWxH_c` (C_DEFAULT/variance.c). Returns
/// `(return_value, sse)`.
#[allow(clippy::too_many_arguments)]
pub fn obmc_sub_pixel_variance(
    pre: &[u8],
    pre_stride: usize,
    xoffset: usize,
    yoffset: usize,
    wsrc: &[i32],
    mask: &[i32],
    w: usize,
    h: usize,
) -> (u32, u32) {
    let mut sse = 0u32;
    let r = unsafe {
        ref_obmc_kernel(
            2,
            w as i32,
            h as i32,
            pre.as_ptr(),
            pre_stride as i32,
            xoffset as i32,
            yoffset as i32,
            wsrc.as_ptr(),
            mask.as_ptr(),
            &mut sse,
        )
    };
    assert_ne!(
        r,
        u32::MAX,
        "the C shim has no obmc_sub_pixel_variance{w}x{h}"
    );
    (r, sse)
}

/// C `svt_aom_upsampled_pred_c` (C_DEFAULT/variance.c:88).
#[allow(clippy::too_many_arguments)]
pub fn upsampled_pred(
    comp_pred: &mut [u8],
    width: usize,
    height: usize,
    subpel_x_q3: i32,
    subpel_y_q3: i32,
    ref_alloc: &[u8],
    ref_base: i64,
    ref_stride: usize,
    subpel_search: i32,
) {
    unsafe {
        ref_upsampled_pred(
            comp_pred.as_mut_ptr(),
            width as i32,
            height as i32,
            subpel_x_q3,
            subpel_y_q3,
            ref_alloc.as_ptr(),
            ref_base as i32,
            ref_stride as i32,
            subpel_search,
        );
    }
}

/// C `svt_aom_convolve8_horiz_c` (convolve.c:288).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_horiz(
    src_alloc: &[u8],
    src_base: i64,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    kernel: &[i16; 8],
    w: usize,
    h: usize,
) {
    unsafe {
        ref_me_convolve8_horiz(
            src_alloc.as_ptr(),
            src_base as i32,
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            kernel.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

/// C `svt_aom_convolve8_vert_c` (convolve.c:300).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_vert(
    src_alloc: &[u8],
    src_base: i64,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    kernel: &[i16; 8],
    w: usize,
    h: usize,
) {
    unsafe {
        ref_me_convolve8_vert(
            src_alloc.as_ptr(),
            src_base as i32,
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            kernel.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

/// The full-pel limits C keeps in `x->mv_limits`.
#[derive(Clone, Copy, Debug)]
pub struct RefMvLimits {
    /// C `col_min`.
    pub col_min: i32,
    /// C `col_max`.
    pub col_max: i32,
    /// C `row_min`.
    pub row_min: i32,
    /// C `row_max`.
    pub row_max: i32,
}

/// C `svt_av1_obmc_full_pixel_search` (av1me.c:673). Returns
/// `(cost, dst_mv_x, dst_mv_y)` with the MV in FULL-PEL.
#[allow(clippy::too_many_arguments)]
pub fn obmc_full_pixel_search(
    pre_alloc: &[u8],
    pre_base: i64,
    pre_stride: usize,
    wsrc: &mut [i32],
    mask: &mut [i32],
    bsize: i32,
    mvp: (i32, i32),
    sadpb: i32,
    ref_mv: (i32, i32),
    limits: RefMvLimits,
    cost: &RefMvCost,
    errorperbit: i32,
    approx_inter_rate: bool,
    fpel_range: i32,
    fpel_diag: bool,
) -> (i32, i32, i32) {
    let mut ox = 0i32;
    let mut oy = 0i32;
    let r = unsafe {
        ref_obmc_full_pixel_search(
            pre_alloc.as_ptr(),
            pre_base as i32,
            pre_stride as i32,
            wsrc.as_mut_ptr(),
            mask.as_mut_ptr(),
            bsize,
            mvp.0,
            mvp.1,
            sadpb,
            ref_mv.0,
            ref_mv.1,
            limits.col_min,
            limits.col_max,
            limits.row_min,
            limits.row_max,
            cost.joint.as_ptr(),
            cost.comp0.as_ptr(),
            cost.comp1.as_ptr(),
            errorperbit,
            i32::from(approx_inter_rate),
            fpel_range,
            i32::from(fpel_diag),
            &mut ox,
            &mut oy,
        )
    };
    (r, ox, oy)
}

/// C `svt_av1_find_best_obmc_sub_pixel_tree_up` (av1me.c:878). Returns
/// `(besterr, mv_x, mv_y, distortion, sse)` with the MV in EIGHTH-PEL.
#[allow(clippy::too_many_arguments)]
pub fn obmc_sub_pixel_tree_up(
    pre_alloc: &[u8],
    pre_base: i64,
    pre_stride: usize,
    wsrc: &mut [i32],
    mask: &mut [i32],
    bsize: i32,
    best_mv: (i32, i32),
    ref_mv: (i32, i32),
    allow_hp: bool,
    errorperbit: i32,
    forced_stop: i32,
    iters_per_step: i32,
    limits: RefMvLimits,
    cost: &RefMvCost,
    approx_inter_rate: bool,
    use_accurate_subpel_search: i32,
) -> (u32, i32, i32, i32, u32) {
    let mut ox = 0i32;
    let mut oy = 0i32;
    let mut dis = 0i32;
    let mut sse = 0u32;
    let r = unsafe {
        ref_obmc_sub_pixel_tree_up(
            pre_alloc.as_ptr(),
            pre_base as i32,
            pre_stride as i32,
            wsrc.as_mut_ptr(),
            mask.as_mut_ptr(),
            bsize,
            best_mv.0,
            best_mv.1,
            ref_mv.0,
            ref_mv.1,
            i32::from(allow_hp),
            errorperbit,
            forced_stop,
            iters_per_step,
            limits.col_min,
            limits.col_max,
            limits.row_min,
            limits.row_max,
            cost.joint.as_ptr(),
            cost.comp0.as_ptr(),
            cost.comp1.as_ptr(),
            i32::from(approx_inter_rate),
            use_accurate_subpel_search,
            &mut ox,
            &mut oy,
            &mut dis,
            &mut sse,
        )
    };
    (r, ox, oy, dis, sse)
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_obmc_kernel_rtcd(
        which: i32,
        bsize: i32,
        pre: *const u8,
        pre_stride: i32,
        xoffset: i32,
        yoffset: i32,
        wsrc: *const i32,
        mask: *const i32,
        sse: *mut u32,
    ) -> u32;
}

/// The RTCD-dispatched OBMC kernel for `bsize` — whatever SIMD tier this host
/// selects, i.e. what `svt_av1_find_best_obmc_sub_pixel_tree_up` actually
/// calls. `which`: 0 = `osdf`, 1 = `ovf`, 2 = `osvf`. Returns
/// `(return_value, sse)`.
#[allow(clippy::too_many_arguments)]
pub fn obmc_kernel_rtcd(
    which: i32,
    bsize: i32,
    pre: &[u8],
    pre_stride: usize,
    xoffset: usize,
    yoffset: usize,
    wsrc: &[i32],
    mask: &[i32],
) -> (u32, u32) {
    let mut sse = 0u32;
    let r = unsafe {
        ref_obmc_kernel_rtcd(
            which,
            bsize,
            pre.as_ptr(),
            pre_stride as i32,
            xoffset as i32,
            yoffset as i32,
            wsrc.as_ptr(),
            mask.as_ptr(),
            &mut sse,
        )
    };
    (r, sse)
}

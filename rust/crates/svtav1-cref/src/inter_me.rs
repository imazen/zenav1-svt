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

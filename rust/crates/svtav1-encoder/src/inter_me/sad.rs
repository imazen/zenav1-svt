//! The ME SAD-accumulation kernels — a 1:1 port of the top of
//! `Source/Lib/Codec/motion_estimation.c` plus the two `compute_sad_c.c`
//! loop kernels the open-loop search drives.
//!
//! | Rust | C | exported? |
//! |---|---|---|
//! | [`compute8x4_sad_kernel`] | `svt_aom_compute8x4_sad_kernel_c` (:43) | yes |
//! | [`compute8x8_sad_kernel`] | `compute8x8_sad_kernel_c` (:71) | no (`static`) |
//! | [`ext_sad_calculation_8x8_16x16`] | `svt_ext_sad_calculation_8x8_16x16_c` (:100) | yes |
//! | [`ext_sad_calculation_32x32_64x64`] | `svt_ext_sad_calculation_32x32_64x64_c` (:164) | yes |
//! | [`ext_eight_sad_calculation_8x8_16x16`] | `svt_ext_eight_sad_calculation_8x8_16x16` (:202) | no (`static`) |
//! | [`ext_all_sad_calculation_8x8_16x16`] | `svt_ext_all_sad_calculation_8x8_16x16_c` (:318) | yes |
//! | [`ext_eight_sad_calculation_32x32_64x64`] | `svt_ext_eight_sad_calculation_32x32_64x64_c` (:351) | yes |
//! | [`nxm_sad_kernel`] | `svt_nxm_sad_kernel_helper_c` (compute_sad_c.c:21) | yes |
//! | [`sad_loop_kernel`] | `svt_sad_loop_kernel_c` (compute_sad_c.c:63) | yes |
//!
//! **Pointer convention.** C passes bare `uint32_t*` aimed at
//! `p_sb_best_sad[list][ref] + ME_TIER_ZERO_PU_*`. Every call site aims the
//! SAD pointer and the MV pointer at the *same* index of their respective
//! 85-entry arrays, so the port takes the two arrays plus one shared offset
//! rather than four independent pointers. That is the only shape change; the
//! arithmetic is transcribed verbatim.

/// C `EB_ABS_DIFF` (utility.h:105) over two samples.
#[inline]
fn abs_diff(a: u8, b: u8) -> u32 {
    u32::from(a.abs_diff(b))
}

/// C `_MVXT` (definitions.h:2052).
#[inline]
pub fn mvxt(mv: u32) -> i16 {
    (mv & 0xFFFF) as u16 as i16
}

/// C `_MVYT` (definitions.h:2053).
#[inline]
pub fn mvyt(mv: u32) -> i16 {
    (mv >> 16) as u16 as i16
}

/// C's packed ME motion vector: `((uint32_t)y << 16) | (uint16_t)x`.
#[inline]
pub fn pack_mv(x: i16, y: i16) -> u32 {
    ((y as u16 as u32) << 16) | (x as u16 as u32)
}

/// C `svt_aom_compute8x4_sad_kernel_c` (motion_estimation.c:43).
pub fn compute8x4_sad_kernel(src: &[u8], src_stride: usize, rf: &[u8], ref_stride: usize) -> u32 {
    let mut sad = 0u32;
    for row in 0..4 {
        let s = &src[row * src_stride..];
        let r = &rf[row * ref_stride..];
        for i in 0..8 {
            sad += abs_diff(s[i], r[i]);
        }
    }
    sad
}

/// C `compute8x8_sad_kernel_c` (motion_estimation.c:71, `static`).
pub fn compute8x8_sad_kernel(src: &[u8], src_stride: usize, rf: &[u8], ref_stride: usize) -> u32 {
    let mut sad = 0u32;
    for row in 0..8 {
        let s = &src[row * src_stride..];
        let r = &rf[row * ref_stride..];
        for i in 0..8 {
            sad += abs_diff(s[i], r[i]);
        }
    }
    sad
}

/// C `svt_nxm_sad_kernel_helper_c` (C_DEFAULT/compute_sad_c.c:21) — the
/// generic NxM SAD behind the `svt_nxm_sad_kernel` RTCD pointer.
pub fn nxm_sad_kernel(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    height: usize,
    width: usize,
) -> u32 {
    let mut sad = 0u32;
    for y in 0..height {
        let s = &src[y * src_stride..];
        let r = &rf[y * ref_stride..];
        for x in 0..width {
            sad += abs_diff(s[x], r[x]);
        }
    }
    sad
}

/// Result of [`sad_loop_kernel`]: C writes these through three out-pointers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SadLoopResult {
    /// C `*best_sad`.
    pub best_sad: u64,
    /// C `*x_search_center`.
    pub x_search_center: i16,
    /// C `*y_search_center`.
    pub y_search_center: i16,
}

/// C `svt_sad_loop_kernel_c` (C_DEFAULT/compute_sad_c.c:63).
///
/// `ref_base` is the index inside `rf` of the search region's top-left sample
/// (C advances the `ref` pointer by `src_stride_raw` per search line). The
/// initial `*best_sad = 0xffffff` and the strict `<` update — so the FIRST
/// minimum wins ties — are both load-bearing and reproduced exactly.
///
/// The `skip_search_line` early-continue is gated on
/// `block_width == 16 && block_height <= 16` in C; the `(void)skip_search_line`
/// above it is dead in the shipping build (the parameter is read two lines
/// later) and is not a reason to drop the branch.
#[allow(clippy::too_many_arguments)]
pub fn sad_loop_kernel(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_base: i64,
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    src_stride_raw: usize,
    skip_search_line: u8,
    search_area_width: i16,
    search_area_height: i16,
) -> SadLoopResult {
    let mut out = SadLoopResult {
        best_sad: 0xffffff,
        x_search_center: 0,
        y_search_center: 0,
    };
    let mut rbase = ref_base;
    for y in 0..search_area_height {
        if block_width == 16 && block_height <= 16 && skip_search_line != 0 && (y & 1) == 0 {
            rbase += src_stride_raw as i64;
            continue;
        }
        for x in 0..search_area_width {
            let off = rbase + i64::from(x);
            let sad = u64::from(nxm_sad_kernel(
                src,
                src_stride,
                &rf[off as usize..],
                ref_stride,
                block_height,
                block_width,
            ));
            if sad < out.best_sad {
                out.best_sad = sad;
                out.x_search_center = x;
                out.y_search_center = y;
            }
        }
        rbase += src_stride_raw as i64;
    }
    out
}

/// C `svt_ext_sad_calculation_8x8_16x16_c` (motion_estimation.c:100).
///
/// `off8` / `off16` index `best_sad` **and** `best_mv` (C aims both pointers
/// at the same slot of `p_sb_best_sad` / `p_sb_best_mv`).
#[allow(clippy::too_many_arguments)]
pub fn ext_sad_calculation_8x8_16x16(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    best_sad: &mut [u32],
    best_mv: &mut [u32],
    off8: usize,
    off16: usize,
    mv: u32,
    p_sad16x16: &mut [u32],
    i16x16: usize,
    p_sad8x8: &mut [u32],
    i8x8: usize,
    sub_sad: bool,
) {
    let mut sad = [0u32; 4];
    if sub_sad {
        sad[0] = compute8x4_sad_kernel(src, 2 * src_stride, rf, 2 * ref_stride) << 1;
        sad[1] = compute8x4_sad_kernel(&src[8..], 2 * src_stride, &rf[8..], 2 * ref_stride) << 1;
        sad[2] = compute8x4_sad_kernel(
            &src[8 * src_stride..],
            2 * src_stride,
            &rf[8 * ref_stride..],
            2 * ref_stride,
        ) << 1;
        sad[3] = compute8x4_sad_kernel(
            &src[8 * src_stride + 8..],
            2 * src_stride,
            &rf[8 * ref_stride + 8..],
            2 * ref_stride,
        ) << 1;
    } else {
        sad[0] = compute8x8_sad_kernel(src, src_stride, rf, ref_stride);
        sad[1] = compute8x8_sad_kernel(&src[8..], src_stride, &rf[8..], ref_stride);
        sad[2] = compute8x8_sad_kernel(
            &src[8 * src_stride..],
            src_stride,
            &rf[8 * ref_stride..],
            ref_stride,
        );
        sad[3] = compute8x8_sad_kernel(
            &src[8 * src_stride + 8..],
            src_stride,
            &rf[8 * ref_stride + 8..],
            ref_stride,
        );
    }

    for k in 0..4 {
        if sad[k] < best_sad[off8 + k] {
            best_sad[off8 + k] = sad[k];
            best_mv[off8 + k] = mv;
        }
        p_sad8x8[i8x8 + k] = sad[k];
    }

    let sad16x16 = sad[0] + sad[1] + sad[2] + sad[3];
    if sad16x16 < best_sad[off16] {
        best_sad[off16] = sad16x16;
        best_mv[off16] = mv;
    }
    p_sad16x16[i16x16] = sad16x16;
}

/// C `svt_ext_sad_calculation_32x32_64x64_c` (motion_estimation.c:164).
#[allow(clippy::too_many_arguments)]
pub fn ext_sad_calculation_32x32_64x64(
    p_sad16x16: &[u32; 16],
    best_sad: &mut [u32],
    best_mv: &mut [u32],
    off32: usize,
    off64: usize,
    mv: u32,
    p_sad32x32: &mut [u32; 4],
) {
    let mut sad32 = [0u32; 4];
    for q in 0..4 {
        sad32[q] = p_sad16x16[4 * q] + p_sad16x16[4 * q + 1] + p_sad16x16[4 * q + 2] + p_sad16x16[4 * q + 3];
        p_sad32x32[q] = sad32[q];
        if sad32[q] < best_sad[off32 + q] {
            best_sad[off32 + q] = sad32[q];
            best_mv[off32 + q] = mv;
        }
    }
    let sad64x64 = sad32[0] + sad32[1] + sad32[2] + sad32[3];
    if sad64x64 < best_sad[off64] {
        best_sad[off64] = sad64x64;
        best_mv[off64] = mv;
    }
}

/// C `svt_ext_eight_sad_calculation_8x8_16x16` (motion_estimation.c:202,
/// `static`): eight consecutive horizontal search points at once.
///
/// C's `(void)p_eight_sad8x8` is faithful — the 8x8 eight-point array is
/// written by no C variant of this kernel, so the port takes no such
/// parameter.
#[allow(clippy::too_many_arguments)]
pub fn ext_eight_sad_calculation_8x8_16x16(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    mv: u32,
    start_16x16_pos: usize,
    best_sad: &mut [u32],
    best_mv: &mut [u32],
    base8: usize,
    base16: usize,
    p_eight_sad16x16: &mut [[u32; 8]; 16],
    sub_sad: bool,
) {
    let start_8x8_pos = 4 * start_16x16_pos;
    let o8 = base8 + start_8x8_pos;
    let o16 = base16 + start_16x16_pos;

    for search_index in 0..8usize {
        let mut sad = [0u32; 4];
        if sub_sad {
            let ss = src_stride << 1;
            let rs = ref_stride << 1;
            sad[0] = compute8x4_sad_kernel(src, ss, &rf[search_index..], rs) << 1;
            sad[1] = compute8x4_sad_kernel(&src[8..], ss, &rf[8 + search_index..], rs) << 1;
            sad[2] = compute8x4_sad_kernel(
                &src[src_stride << 3..],
                ss,
                &rf[(ref_stride << 3) + search_index..],
                rs,
            ) << 1;
            sad[3] = compute8x4_sad_kernel(
                &src[(src_stride << 3) + 8..],
                ss,
                &rf[(ref_stride << 3) + 8 + search_index..],
                rs,
            ) << 1;
        } else {
            sad[0] = compute8x8_sad_kernel(src, src_stride, &rf[search_index..], ref_stride);
            sad[1] = compute8x8_sad_kernel(&src[8..], src_stride, &rf[8 + search_index..], ref_stride);
            sad[2] = compute8x8_sad_kernel(
                &src[src_stride << 3..],
                src_stride,
                &rf[(ref_stride << 3) + search_index..],
                ref_stride,
            );
            sad[3] = compute8x8_sad_kernel(
                &src[(src_stride << 3) + 8..],
                src_stride,
                &rf[(ref_stride << 3) + 8 + search_index..],
                ref_stride,
            );
        }
        let packed = pack_mv(mvxt(mv) + search_index as i16, mvyt(mv));
        for k in 0..4 {
            if sad[k] < best_sad[o8 + k] {
                best_sad[o8 + k] = sad[k];
                best_mv[o8 + k] = packed;
            }
        }
        let sad16x16 = sad[0] + sad[1] + sad[2] + sad[3];
        p_eight_sad16x16[start_16x16_pos][search_index] = sad16x16;
        if sad16x16 < best_sad[o16] {
            best_sad[o16] = sad16x16;
            best_mv[o16] = packed;
        }
    }
}

/// C's z-order-to-raster 16x16 map inside
/// `svt_ext_all_sad_calculation_8x8_16x16_c` (motion_estimation.c:319).
const ALL_SAD_OFFSETS: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

/// C `svt_ext_all_sad_calculation_8x8_16x16_c` (motion_estimation.c:318).
#[allow(clippy::too_many_arguments)]
pub fn ext_all_sad_calculation_8x8_16x16(
    src: &[u8],
    src_stride: usize,
    rf: &[u8],
    ref_stride: usize,
    mv: u32,
    best_sad: &mut [u32],
    best_mv: &mut [u32],
    base8: usize,
    base16: usize,
    p_eight_sad16x16: &mut [[u32; 8]; 16],
    sub_sad: bool,
) {
    for y in 0..4usize {
        for x in 0..4usize {
            let block_index = 16 * y * src_stride + 16 * x;
            let search_position_index = 16 * y * ref_stride + 16 * x;
            ext_eight_sad_calculation_8x8_16x16(
                &src[block_index..],
                src_stride,
                &rf[search_position_index..],
                ref_stride,
                mv,
                ALL_SAD_OFFSETS[4 * y + x],
                best_sad,
                best_mv,
                base8,
                base16,
                p_eight_sad16x16,
                sub_sad,
            );
        }
    }
}

/// C `svt_ext_eight_sad_calculation_32x32_64x64_c` (motion_estimation.c:351).
#[allow(clippy::too_many_arguments)]
pub fn ext_eight_sad_calculation_32x32_64x64(
    p_sad16x16: &[[u32; 8]; 16],
    best_sad: &mut [u32],
    best_mv: &mut [u32],
    off32: usize,
    off64: usize,
    mv: u32,
    p_sad32x32: &mut [[u32; 8]; 4],
) {
    for search_index in 0..8usize {
        let mut sad32 = [0u32; 4];
        let packed = pack_mv(mvxt(mv) + search_index as i16, mvyt(mv));
        for q in 0..4 {
            sad32[q] = p_sad16x16[4 * q][search_index]
                + p_sad16x16[4 * q + 1][search_index]
                + p_sad16x16[4 * q + 2][search_index]
                + p_sad16x16[4 * q + 3][search_index];
            p_sad32x32[q][search_index] = sad32[q];
            if sad32[q] < best_sad[off32 + q] {
                best_sad[off32 + q] = sad32[q];
                best_mv[off32 + q] = packed;
            }
        }
        let sad64x64 = sad32[0] + sad32[1] + sad32[2] + sad32[3];
        if sad64x64 < best_sad[off64] {
            best_sad[off64] = sad64x64;
            best_mv[off64] = packed;
        }
    }
}

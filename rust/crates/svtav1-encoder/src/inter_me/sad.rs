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

use archmage::prelude::*;
use svtav1_dsp::me_sad::block_sad_scalar;
#[cfg(target_arch = "x86_64")]
use svtav1_dsp::me_sad::block_sad_v3;
#[cfg(target_arch = "aarch64")]
use svtav1_dsp::me_sad::{block_sad_arm_v2, block_sad_neon};

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
///
/// One-shot dispatch. The hot call sites are inside
/// [`ext_sad_calculation_8x8_16x16`] and
/// [`ext_all_sad_calculation_8x8_16x16`], which summon the token once and
/// route through the tier helper directly.
pub fn compute8x4_sad_kernel(src: &[u8], src_stride: usize, rf: &[u8], ref_stride: usize) -> u32 {
    svtav1_dsp::me_sad::block_sad(src, src_stride, rf, ref_stride, 8, 4)
}

/// C `compute8x8_sad_kernel_c` (motion_estimation.c:71, `static`).
pub fn compute8x8_sad_kernel(src: &[u8], src_stride: usize, rf: &[u8], ref_stride: usize) -> u32 {
    svtav1_dsp::me_sad::block_sad(src, src_stride, rf, ref_stride, 8, 8)
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
    // One-shot dispatch. The three call sites in `hme.rs` are single
    // evaluations, not search loops; the loop call site inside
    // [`sad_loop_kernel`] calls the tier helper directly instead (see the
    // `sad_loop_body!` variants below), so no target-feature boundary is
    // crossed per search position.
    svtav1_dsp::me_sad::block_sad(src, src_stride, rf, ref_stride, width, height)
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
/// The body of [`sad_loop_kernel`], stamped once per archmage tier so the
/// token is summoned ONCE outside the search loop rather than per search
/// position. `$kernel` is the matching `svtav1_dsp::me_sad` helper.
///
/// Every tier computes the same integer SAD, so the `best_sad` sequence — and
/// therefore the first-wins tie-break — is identical on every arm.
macro_rules! sad_loop_body {
    ($token:ident, $kernel:ident, $src:ident, $src_stride:ident, $rf:ident,
     $ref_base:ident, $ref_stride:ident, $block_height:ident, $block_width:ident,
     $src_stride_raw:ident, $skip_search_line:ident, $search_area_width:ident,
     $search_area_height:ident) => {{
        let mut out = SadLoopResult {
            best_sad: 0xffffff,
            x_search_center: 0,
            y_search_center: 0,
        };
        let mut rbase = $ref_base;
        for y in 0..$search_area_height {
            if $block_width == 16 && $block_height <= 16 && $skip_search_line != 0 && (y & 1) == 0 {
                rbase += $src_stride_raw as i64;
                continue;
            }
            for x in 0..$search_area_width {
                let off = rbase + i64::from(x);
                let sad = u64::from($kernel(
                    $token,
                    $src,
                    $src_stride,
                    &$rf[off as usize..],
                    $ref_stride,
                    $block_width,
                    $block_height,
                ));
                if sad < out.best_sad {
                    out.best_sad = sad;
                    out.x_search_center = x;
                    out.y_search_center = y;
                }
            }
            rbase += $src_stride_raw as i64;
        }
        out
    }};
}

macro_rules! sad_loop_variant {
    ($(#[$m:meta])* $name:ident, $tok:ident, $kernel:ident) => {
        $(#[$m])*
        #[allow(clippy::too_many_arguments)]
        fn $name(
            token: $tok,
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
            sad_loop_body!(
                token,
                $kernel,
                src,
                src_stride,
                rf,
                ref_base,
                ref_stride,
                block_height,
                block_width,
                src_stride_raw,
                skip_search_line,
                search_area_width,
                search_area_height
            )
        }
    };
}

sad_loop_variant!(sad_loop_dispatch_scalar, ScalarToken, block_sad_scalar);
#[cfg(target_arch = "aarch64")]
sad_loop_variant!(
    #[arcane]
    sad_loop_dispatch_neon,
    NeonToken,
    block_sad_neon
);
#[cfg(target_arch = "aarch64")]
sad_loop_variant!(
    #[arcane]
    sad_loop_dispatch_arm_v2,
    Arm64V2Token,
    block_sad_arm_v2
);
#[cfg(target_arch = "x86_64")]
sad_loop_variant!(
    #[arcane]
    sad_loop_dispatch_v3,
    Desktop64,
    block_sad_v3
);

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
///
/// The whole search loop lives inside the dispatched variant, so the token is
/// summoned once per CALL, not once per search position.
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
    incant!(
        sad_loop_dispatch(
            src,
            src_stride,
            rf,
            ref_base,
            ref_stride,
            block_height,
            block_width,
            src_stride_raw,
            skip_search_line,
            search_area_width,
            search_area_height
        ),
        [arm_v2, v3, neon, scalar]
    )
}

/// C `svt_ext_sad_calculation_8x8_16x16_c` (motion_estimation.c:100).
///
/// `off8` / `off16` index `best_sad` **and** `best_mv` (C aims both pointers
/// at the same slot of `p_sb_best_sad` / `p_sb_best_mv`).
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn ext_sad_calculation_8x8_16x16_generic<F4, F8>(
    sad8x4: &F4,
    sad8x8: &F8,
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
) where
    F4: Fn(&[u8], usize, &[u8], usize) -> u32,
    F8: Fn(&[u8], usize, &[u8], usize) -> u32,
{
    let mut sad = [0u32; 4];
    if sub_sad {
        sad[0] = sad8x4(src, 2 * src_stride, rf, 2 * ref_stride) << 1;
        sad[1] = sad8x4(&src[8..], 2 * src_stride, &rf[8..], 2 * ref_stride) << 1;
        sad[2] = sad8x4(
            &src[8 * src_stride..],
            2 * src_stride,
            &rf[8 * ref_stride..],
            2 * ref_stride,
        ) << 1;
        sad[3] = sad8x4(
            &src[8 * src_stride + 8..],
            2 * src_stride,
            &rf[8 * ref_stride + 8..],
            2 * ref_stride,
        ) << 1;
    } else {
        sad[0] = sad8x8(src, src_stride, rf, ref_stride);
        sad[1] = sad8x8(&src[8..], src_stride, &rf[8..], ref_stride);
        sad[2] = sad8x8(
            &src[8 * src_stride..],
            src_stride,
            &rf[8 * ref_stride..],
            ref_stride,
        );
        sad[3] = sad8x8(
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
        sad32[q] = p_sad16x16[4 * q]
            + p_sad16x16[4 * q + 1]
            + p_sad16x16[4 * q + 2]
            + p_sad16x16[4 * q + 3];
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
#[inline(always)]
fn ext_eight_sad_calculation_8x8_16x16_generic<F4, F8>(
    sad8x4: &F4,
    sad8x8: &F8,
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
) where
    F4: Fn(&[u8], usize, &[u8], usize) -> u32,
    F8: Fn(&[u8], usize, &[u8], usize) -> u32,
{
    let start_8x8_pos = 4 * start_16x16_pos;
    let o8 = base8 + start_8x8_pos;
    let o16 = base16 + start_16x16_pos;

    for search_index in 0..8usize {
        let mut sad = [0u32; 4];
        if sub_sad {
            let ss = src_stride << 1;
            let rs = ref_stride << 1;
            sad[0] = sad8x4(src, ss, &rf[search_index..], rs) << 1;
            sad[1] = sad8x4(&src[8..], ss, &rf[8 + search_index..], rs) << 1;
            sad[2] = sad8x4(
                &src[src_stride << 3..],
                ss,
                &rf[(ref_stride << 3) + search_index..],
                rs,
            ) << 1;
            sad[3] = sad8x4(
                &src[(src_stride << 3) + 8..],
                ss,
                &rf[(ref_stride << 3) + 8 + search_index..],
                rs,
            ) << 1;
        } else {
            sad[0] = sad8x8(src, src_stride, &rf[search_index..], ref_stride);
            sad[1] = sad8x8(&src[8..], src_stride, &rf[8 + search_index..], ref_stride);
            sad[2] = sad8x8(
                &src[src_stride << 3..],
                src_stride,
                &rf[(ref_stride << 3) + search_index..],
                ref_stride,
            );
            sad[3] = sad8x8(
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
#[inline(always)]
fn ext_all_sad_calculation_8x8_16x16_generic<F4, F8>(
    sad8x4: &F4,
    sad8x8: &F8,
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
) where
    F4: Fn(&[u8], usize, &[u8], usize) -> u32,
    F8: Fn(&[u8], usize, &[u8], usize) -> u32,
{
    for y in 0..4usize {
        for x in 0..4usize {
            let block_index = 16 * y * src_stride + 16 * x;
            let search_position_index = 16 * y * ref_stride + 16 * x;
            ext_eight_sad_calculation_8x8_16x16_generic(
                sad8x4,
                sad8x8,
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

// ---------------------------------------------------------------------------
// Tier wrappers for the three 8x8/16x16 SAD-accumulation kernels.
//
// The `*_generic` bodies above are `#[inline(always)]` and take the 8x4 / 8x8
// block SAD as closures. A closure defined inside an `#[arcane]` body inherits
// that body's `#[target_feature]`s, so the SIMD helper is called WITHOUT
// crossing a target-feature boundary — and the boundary is crossed once per
// public call rather than once per 8x8 block. `ext_all_sad_calculation_8x8_16x16`
// evaluates 512 8x8 SADs per call, so the amortisation there is 512:1.
//
// Every tier computes the same integer SAD (see `svtav1_dsp::me_sad`), so the
// comparison sequence, the tie-break and the written arrays are identical on
// every arm.
// ---------------------------------------------------------------------------

macro_rules! ext_sad_8x8_16x16_variant {
    ($(#[$m:meta])* $name:ident, $tok:ident, $k:ident) => {
        $(#[$m])*
        #[allow(clippy::too_many_arguments)]
        fn $name(
            token: $tok,
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
            let s4 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 4);
            let s8 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 8);
            ext_sad_calculation_8x8_16x16_generic(
                &s4, &s8, src, src_stride, rf, ref_stride, best_sad, best_mv, off8, off16, mv,
                p_sad16x16, i16x16, p_sad8x8, i8x8, sub_sad,
            );
        }
    };
}

macro_rules! ext_eight_sad_8x8_16x16_variant {
    ($(#[$m:meta])* $name:ident, $tok:ident, $k:ident) => {
        $(#[$m])*
        #[allow(clippy::too_many_arguments)]
        fn $name(
            token: $tok,
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
            let s4 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 4);
            let s8 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 8);
            ext_eight_sad_calculation_8x8_16x16_generic(
                &s4, &s8, src, src_stride, rf, ref_stride, mv, start_16x16_pos, best_sad, best_mv,
                base8, base16, p_eight_sad16x16, sub_sad,
            );
        }
    };
}

macro_rules! ext_all_sad_8x8_16x16_variant {
    ($(#[$m:meta])* $name:ident, $tok:ident, $k:ident) => {
        $(#[$m])*
        #[allow(clippy::too_many_arguments)]
        fn $name(
            token: $tok,
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
            let s4 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 4);
            let s8 = |a: &[u8], sa: usize, b: &[u8], sb: usize| $k(token, a, sa, b, sb, 8, 8);
            ext_all_sad_calculation_8x8_16x16_generic(
                &s4, &s8, src, src_stride, rf, ref_stride, mv, best_sad, best_mv, base8, base16,
                p_eight_sad16x16, sub_sad,
            );
        }
    };
}

ext_sad_8x8_16x16_variant!(ext_sad8_dispatch_scalar, ScalarToken, block_sad_scalar);
#[cfg(target_arch = "aarch64")]
ext_sad_8x8_16x16_variant!(
    #[arcane]
    ext_sad8_dispatch_neon,
    NeonToken,
    block_sad_neon
);
#[cfg(target_arch = "aarch64")]
ext_sad_8x8_16x16_variant!(
    #[arcane]
    ext_sad8_dispatch_arm_v2,
    Arm64V2Token,
    block_sad_arm_v2
);
#[cfg(target_arch = "x86_64")]
ext_sad_8x8_16x16_variant!(
    #[arcane]
    ext_sad8_dispatch_v3,
    Desktop64,
    block_sad_v3
);

ext_eight_sad_8x8_16x16_variant!(ext_eight8_dispatch_scalar, ScalarToken, block_sad_scalar);
#[cfg(target_arch = "aarch64")]
ext_eight_sad_8x8_16x16_variant!(
    #[arcane]
    ext_eight8_dispatch_neon,
    NeonToken,
    block_sad_neon
);
#[cfg(target_arch = "aarch64")]
ext_eight_sad_8x8_16x16_variant!(
    #[arcane]
    ext_eight8_dispatch_arm_v2,
    Arm64V2Token,
    block_sad_arm_v2
);
#[cfg(target_arch = "x86_64")]
ext_eight_sad_8x8_16x16_variant!(
    #[arcane]
    ext_eight8_dispatch_v3,
    Desktop64,
    block_sad_v3
);

ext_all_sad_8x8_16x16_variant!(ext_all8_dispatch_scalar, ScalarToken, block_sad_scalar);
#[cfg(target_arch = "aarch64")]
ext_all_sad_8x8_16x16_variant!(
    #[arcane]
    ext_all8_dispatch_neon,
    NeonToken,
    block_sad_neon
);
#[cfg(target_arch = "aarch64")]
ext_all_sad_8x8_16x16_variant!(
    #[arcane]
    ext_all8_dispatch_arm_v2,
    Arm64V2Token,
    block_sad_arm_v2
);
#[cfg(target_arch = "x86_64")]
ext_all_sad_8x8_16x16_variant!(
    #[arcane]
    ext_all8_dispatch_v3,
    Desktop64,
    block_sad_v3
);

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
    incant!(
        ext_sad8_dispatch(
            src, src_stride, rf, ref_stride, best_sad, best_mv, off8, off16, mv, p_sad16x16,
            i16x16, p_sad8x8, i8x8, sub_sad
        ),
        [arm_v2, v3, neon, scalar]
    )
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
    incant!(
        ext_eight8_dispatch(
            src,
            src_stride,
            rf,
            ref_stride,
            mv,
            start_16x16_pos,
            best_sad,
            best_mv,
            base8,
            base16,
            p_eight_sad16x16,
            sub_sad
        ),
        [arm_v2, v3, neon, scalar]
    )
}

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
    incant!(
        ext_all8_dispatch(
            src,
            src_stride,
            rf,
            ref_stride,
            mv,
            best_sad,
            best_mv,
            base8,
            base16,
            p_eight_sad16x16,
            sub_sad
        ),
        [arm_v2, v3, neon, scalar]
    )
}

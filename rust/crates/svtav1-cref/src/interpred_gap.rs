//! Reference bindings for the inter-prediction functions the wholesale-MC
//! lane left unported: `Source/Lib/C_DEFAULT/inter_prediction_c.c` and the
//! 10-bit arm of `Source/Lib/Codec/inter_prediction.c`.
//!
//! These drive the REAL exported C symbols — evidence tier 1 in
//! `docs/WORKING-ON-THIS.md` §4. Everything here goes through
//! `shims/interpred_gap_shims.c`, which builds the `ConvolveParams` the
//! kernels take by pointer, so no C struct is mirrored in Rust.

use core::ffi::c_int;

unsafe extern "C" {
    fn ref_build_compound_diffwtd_mask_d16_c(
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
    fn ref_build_compound_diffwtd_mask_d16_rtcd(
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
    fn ref_d16_diff_round(is_compound: c_int, bd: c_int) -> c_int;
}

/// How the C side should be reached: the scalar `_c` kernel, or this host's
/// RTCD-dispatched tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D16MaskTier {
    /// `svt_av1_build_compound_diffwtd_mask_d16_c` — the reference semantics.
    Scalar,
    /// `svt_av1_build_compound_diffwtd_mask_d16` — the function pointer RTCD
    /// installed for this host (NEON here, SSE4.1/AVX2 on x86-64).
    Dispatched,
}

/// Reference `svt_av1_build_compound_diffwtd_mask_d16_c`
/// (C_DEFAULT/inter_prediction_c.c:30), with `conv_params` built by
/// `get_conv_params_no_round(0, NULL, 0, is_compound = 1, bd)` — the only
/// shape either live caller passes.
#[allow(clippy::too_many_arguments)]
pub fn build_compound_diffwtd_mask_d16(
    tier: D16MaskTier,
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
    assert!(h > 0 && w > 0);
    assert!(mask.len() >= h * w);
    assert!(src0.len() >= (h - 1) * src0_stride + w);
    assert!(src1.len() >= (h - 1) * src1_stride + w);
    let f = match tier {
        D16MaskTier::Scalar => ref_build_compound_diffwtd_mask_d16_c,
        D16MaskTier::Dispatched => ref_build_compound_diffwtd_mask_d16_rtcd,
    };
    unsafe {
        f(
            mask.as_mut_ptr(),
            mask_type,
            src0.as_ptr(),
            src0_stride as c_int,
            src1.as_ptr(),
            src1_stride as c_int,
            h as c_int,
            w as c_int,
            bd,
        );
    }
}

/// The `round` the C kernel derives from `conv_params` and `bd`:
/// `2 * FILTER_BITS - round_0 - round_1 + (bd - 8)`.
pub fn d16_diff_round(is_compound: bool, bd: i32) -> i32 {
    unsafe { ref_d16_diff_round(i32::from(is_compound), bd) }
}

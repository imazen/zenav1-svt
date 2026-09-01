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

unsafe extern "C" {
    fn ref_pack_block(
        in8: *const u8,
        in8_stride: u32,
        inn: *const u8,
        inn_stride: u32,
        out16: *mut u16,
        out_stride: u32,
        width: u32,
        height: u32,
    );
    fn ref_enc_msb_pack2_d(
        in8: *const u8,
        in8_stride: u32,
        inn: *const u8,
        inn_stride: u32,
        out16: *mut u16,
        out_stride: u32,
        width: u32,
        height: u32,
    );
}

/// Which C entry to drive for the 8+2 -> 10-bit pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackEntry {
    /// `svt_aom_pack_block` (inter_prediction.c:26) — through
    /// `svt_aom_pack2d_src`'s width/height dispatch, so the SIMD arm runs
    /// whenever `width % 4 == 0 && height % 2 == 0`.
    Dispatched,
    /// `svt_enc_msb_pack2_d` (C_DEFAULT/pack_unpack_c.c:18) — the scalar
    /// reference, forced regardless of extent.
    Scalar,
}

/// Reference 8-bit + 2-bit -> 10-bit pack.
///
/// The argument order is `svt_aom_pack2d_src`'s (buffer then its own stride),
/// not `svt_enc_msb_pack2_d`'s interleaved one; the shim does the reorder.
#[allow(clippy::too_many_arguments)]
pub fn pack_block(
    entry: PackEntry,
    in8: &[u8],
    in8_stride: usize,
    inn: &[u8],
    inn_stride: usize,
    out16: &mut [u16],
    out_stride: usize,
    width: usize,
    height: usize,
) {
    assert!(width > 0 && height > 0);
    assert!(in8.len() >= (height - 1) * in8_stride + width);
    assert!(inn.len() >= (height - 1) * inn_stride + width);
    assert!(out16.len() >= (height - 1) * out_stride + width);
    let f = match entry {
        PackEntry::Dispatched => ref_pack_block,
        PackEntry::Scalar => ref_enc_msb_pack2_d,
    };
    unsafe {
        f(
            in8.as_ptr(),
            in8_stride as u32,
            inn.as_ptr(),
            inn_stride as u32,
            out16.as_mut_ptr(),
            out_stride as u32,
            width as u32,
            height as u32,
        );
    }
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_inter_predictor_light_pd1_hbd(
        src: *mut u8,
        src_2b: *mut u8,
        src_stride: i32,
        dst: *mut u16,
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
        bd: c_int,
    );
}

/// Reference `svt_inter_predictor_light_pd1` (inter_prediction.c:1283) on its
/// `bd > EB_EIGHT_BIT` arm — the one that packs `src` (8 MSB) + `src_2b`
/// (2 LSB) into a 10-bit scratch. The 8-bit arm is
/// `inter_pred::inter_predictor_light_pd1_8bit`.
///
/// `src_origin` must sit at least `INTERPOLATION_OFFSET` (8) rows and columns
/// inside both planes: C reads the window from
/// `src - 8 - 8 * src_stride`. BOTH planes are indexed at `src_stride`.
#[allow(clippy::too_many_arguments)]
pub fn inter_predictor_light_pd1_hbd(
    src: &mut [u8],
    src_2b: &mut [u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    w: usize,
    h: usize,
    interp_filters: u32,
    sp: crate::inter_pred::RefSubpel,
    comp: crate::inter_pred::RefCompound,
    bd: i32,
) {
    assert!(src_origin >= 8 * src_stride + 8, "no 8-pixel MC border");
    assert_eq!(
        src.len(),
        src_2b.len(),
        "planes share one stride and extent"
    );
    unsafe {
        ref_inter_predictor_light_pd1_hbd(
            src.as_mut_ptr().add(src_origin),
            src_2b.as_mut_ptr().add(src_origin),
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
            conv_stride as c_int,
            c_int::from(comp.is_compound),
            c_int::from(comp.do_average),
            c_int::from(comp.use_jnt),
            comp.fwd,
            comp.bck,
            bd,
        );
    }
}

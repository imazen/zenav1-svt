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

/// Everything `ref_enc_make_inter_predictor` needs that is not a buffer.
///
/// Grouped rather than passed as 30 positional arguments: the C entry takes 32
/// and mis-ordering two `int`s of the same type is a silent wrong answer.
#[derive(Debug, Clone, Copy)]
pub struct EncMakePredArgs {
    /// `pre_y` / `pre_x` — the block's position in the reference plane.
    pub pre_y: i32,
    /// See [`Self::pre_y`].
    pub pre_x: i32,
    /// The motion vector, eighth-pel.
    pub mv: (i32, i32),
    /// `svt_av1_setup_scale_factors_for_frame(other_w, other_h, this_w, this_h)`.
    pub scale: (i32, i32, i32, i32),
    /// `scs->super_block_size`.
    pub super_block_size: i32,
    /// `frame_width` / `frame_height`.
    pub frame: (i32, i32),
    /// `blk_width` / `blk_height`.
    pub blk: (usize, usize),
    /// `bsize`, and `xd->bsize`.
    pub bsize: i32,
    /// `xd->mb_to_{left,right,top,bottom}_edge`.
    pub edges: (i32, i32, i32, i32),
    /// The packed `InterpFilters` word.
    pub interp_filters: u32,
    /// `src_stride` / `dst_stride`.
    pub strides: (usize, usize),
    /// `conv_params->dst_stride`.
    pub conv_stride: usize,
    /// `conv_params` flags: `(is_compound, do_average, use_jnt, fwd, bck)`.
    pub compound: (bool, bool, bool, i32, i32),
    /// `plane`, `ss_y`, `ss_x`.
    pub plane: (usize, i32, i32),
    /// `bit_depth`.
    pub bit_depth: i32,
    /// `use_intrabc`.
    pub use_intrabc: bool,
    /// `is16bit`.
    pub is16bit: bool,
    /// `is_masked_compound`, and the `InterInterCompoundData` it selects:
    /// `(comp_type, wedge_index, wedge_sign, mask_type)`.
    pub masked: Option<(i32, i32, i32, i32)>,
}

/// `WarpedMotionParams` marshalled as
/// `[wmtype, mat0..mat5, alpha, beta, gamma, delta]`, written back so the
/// ROTZOOM fix-up C performs in place (warped_motion.c:834-837) is observable.
pub type WarpIo = [i32; 11];

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_enc_make_inter_predictor(
        src: *mut core::ffi::c_void,
        src_2b: *mut core::ffi::c_void,
        dst: *mut core::ffi::c_void,
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
        bsize: c_int,
        mb_to_left: c_int,
        mb_to_right: c_int,
        mb_to_top: c_int,
        mb_to_bottom: c_int,
        interp_filters: u32,
        src_stride: c_int,
        dst_stride: c_int,
        conv_buf: *mut u16,
        conv_stride: c_int,
        is_compound: c_int,
        do_average: c_int,
        use_jnt: c_int,
        fwd: c_int,
        bck: c_int,
        plane: c_int,
        ss_y: c_int,
        ss_x: c_int,
        bit_depth: c_int,
        use_intrabc: c_int,
        is16bit: c_int,
        is_masked_compound: c_int,
        comp_type: c_int,
        wedge_index: c_int,
        wedge_sign: c_int,
        mask_type: c_int,
        seg_mask: *mut u8,
        is_wm: c_int,
        wm_io: *mut i32,
    );
}

/// The reference planes, in whichever of C's three representations the caller
/// holds — the same three-way split `SrcPlanes` makes in the port.
pub enum RefSrc<'a> {
    /// `!is16bit`.
    Lbd(&'a mut [u8]),
    /// `is16bit` with a 2-bit plane: MSBs and LSBs, both at `src_stride`.
    Split {
        /// The eight most significant bits.
        msb: &'a mut [u8],
        /// The two least significant bits, in each byte's top two bits.
        lsb: &'a mut [u8],
    },
    /// `is16bit` with no 2-bit plane — an unpacked 10-bit plane.
    Hbd(&'a mut [u16]),
}

/// The prediction destination.
pub enum RefDst<'a> {
    /// 8-bit.
    Lbd(&'a mut [u8]),
    /// 16-bit.
    Hbd(&'a mut [u16]),
}

/// Reference `svt_aom_enc_make_inter_predictor` (enc_inter_prediction.c:2515).
///
/// `warp` selects `is_wm`: `Some(wm_io)` drives the two WARP leaves, `None`
/// the two non-warp ones.
///
/// `src_origin` is where the reference plane's (0, 0) sits in the slice, in
/// SAMPLES; the shim converts to whatever pointer C wants. C's own
/// `src_ptr + (pos_x + pos_y * src_stride) * (1 << is16bit)` offset is applied
/// INSIDE the C function, so it must not be pre-applied here.
#[allow(clippy::too_many_arguments)]
pub fn enc_make_inter_predictor(
    src: RefSrc<'_>,
    src_origin: usize,
    dst: RefDst<'_>,
    conv_buf: &mut [u16],
    seg_mask: &mut [u8],
    warp: Option<&mut WarpIo>,
    a: EncMakePredArgs,
) {
    let (src_p, src2_p) = match src {
        RefSrc::Lbd(p) => (
            p.as_mut_ptr().wrapping_add(src_origin).cast(),
            core::ptr::null_mut(),
        ),
        RefSrc::Split { msb, lsb } => (
            msb.as_mut_ptr().wrapping_add(src_origin).cast(),
            lsb.as_mut_ptr().wrapping_add(src_origin).cast(),
        ),
        RefSrc::Hbd(p) => (
            p.as_mut_ptr().wrapping_add(src_origin).cast(),
            core::ptr::null_mut(),
        ),
    };
    let dst_p: *mut core::ffi::c_void = match dst {
        RefDst::Lbd(d) => d.as_mut_ptr().cast(),
        RefDst::Hbd(d) => d.as_mut_ptr().cast(),
    };
    let (comp_type, wedge_index, wedge_sign, mask_type) = a.masked.unwrap_or((0, 0, 0, 0));
    unsafe {
        ref_enc_make_inter_predictor(
            src_p,
            src2_p,
            dst_p,
            a.pre_y,
            a.pre_x,
            a.mv.0,
            a.mv.1,
            a.scale.0,
            a.scale.1,
            a.scale.2,
            a.scale.3,
            a.super_block_size,
            a.frame.0,
            a.frame.1,
            a.blk.0 as c_int,
            a.blk.1 as c_int,
            a.bsize,
            a.edges.0,
            a.edges.1,
            a.edges.2,
            a.edges.3,
            a.interp_filters,
            a.strides.0 as c_int,
            a.strides.1 as c_int,
            conv_buf.as_mut_ptr(),
            a.conv_stride as c_int,
            c_int::from(a.compound.0),
            c_int::from(a.compound.1),
            c_int::from(a.compound.2),
            a.compound.3,
            a.compound.4,
            a.plane.0 as c_int,
            a.plane.1,
            a.plane.2,
            a.bit_depth,
            c_int::from(a.use_intrabc),
            c_int::from(a.is16bit),
            c_int::from(a.masked.is_some()),
            comp_type,
            wedge_index,
            wedge_sign,
            mask_type,
            seg_mask.as_mut_ptr(),
            c_int::from(warp.is_some()),
            match warp {
                Some(w) => w.as_mut_ptr(),
                None => core::ptr::null_mut(),
            },
        );
    }
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_tf_inter_predictor_hbd(
        src: *mut u16,
        src_stride: i32,
        dst: *mut u16,
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
        conv_buf: *mut u16,
        conv_stride: c_int,
        bit_depth: c_int,
        subsampling_shift: c_int,
    );
}

/// Reference `tf_inter_predictor` (enc_inter_prediction.c:2452) on its
/// `bit_depth > EB_EIGHT_BIT` arm.
///
/// `inter_pred::tf_inter_predictor` binds the same C function through `u8`
/// slices and can therefore only express the 8-bit arm; see the shim's
/// comment. `src_origin` is in SAMPLES.
#[allow(clippy::too_many_arguments)]
pub fn tf_inter_predictor_hbd(
    src: &mut [u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_stride: usize,
    conv_buf: &mut [u16],
    conv_stride: usize,
    pre: (i32, i32),
    mv: (i32, i32),
    scale: (i32, i32, i32, i32),
    super_block_size: i32,
    frame: (i32, i32),
    blk: (i32, i32),
    edges: (i32, i32, i32, i32),
    interp_filters: u32,
    bit_depth: i32,
    subsampling_shift: i32,
) {
    unsafe {
        ref_tf_inter_predictor_hbd(
            src.as_mut_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            pre.0,
            pre.1,
            mv.0,
            mv.1,
            scale.0,
            scale.1,
            scale.2,
            scale.3,
            super_block_size,
            frame.0,
            frame.1,
            blk.0,
            blk.1,
            edges.0,
            edges.1,
            edges.2,
            edges.3,
            interp_filters,
            conv_buf.as_mut_ptr(),
            conv_stride as c_int,
            bit_depth,
            subsampling_shift,
        );
    }
}

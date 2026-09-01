//! `svt_aom_enc_make_inter_predictor`, EXECUTABLE.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `svt_aom_enc_make_inter_predictor` (:2515) and
//! `av1_make_masked_scaled_inter_predictor` (:77).
//!
//! # What already existed, and what this adds
//!
//! [`crate::port_make_pred`] ports this function's DECISIONS — which leaf a
//! `(is_wm, is_masked_compound, is16bit)` triple selects, the packed-scratch
//! geometry, the `src` byte offset — as separate predicates, and says so.
//! What it does not do is RUN one: nothing in the port turned a motion vector
//! and a reference plane into prediction pixels through this entry. This
//! module is that, composed out of leaves that are each already tier-1 gated:
//! [`crate::port_subpel_params::compute_subpel_params`],
//! [`crate::port_pack::pack_block`],
//! [`crate::port_inter_predictor::inter_predictor`] /
//! `highbd_inter_predictor`,
//! [`crate::port_diffwtd_d16::build_compound_diffwtd_mask_d16`] and
//! [`crate::port_masked_blend::build_masked_compound_no_round`].
//!
//! # The three source representations are ONE `uint8_t*` in C
//!
//! C decides between them with two arguments that are not an enum:
//! `is16bit` and whether `src_ptr_2b` is NULL. The three live combinations are
//! [`SrcPlanes::Lbd`] (`!is16bit`), [`SrcPlanes::Split`]
//! (`is16bit && src_ptr_2b`) and [`SrcPlanes::Hbd`]
//! (`is16bit && !src_ptr_2b`, where C casts the `uint8_t*` to `uint16_t*`).
//! They differ in the POINTER ARITHMETIC too, which is the part that is easy
//! to get wrong: the byte offset is `pos_x + pos_y * src_stride` for `Lbd` and
//! `Split`, and that TIMES TWO for `Hbd` (`* (1 << is16bit)`, :2585) — because
//! only there is `src_stride` counted in bytes rather than samples. This port
//! indexes typed slices, so the `Hbd` doubling disappears and the other two
//! stay; that is the same address in every case.
//!
//! # All four of C's leaves are here
//!
//! **regular**, **masked-compound**
//! (`av1_make_masked_scaled_inter_predictor`), **warp** (`svt_av1_warp_plane`)
//! and **masked-warp** (`av1_make_masked_warp_inter_predictor`, :1633).
//!
//! The warp leaves needed a gap closed one level down first:
//! `crate::port_warp`'s high-bit-depth kernel took an already-unpacked `u16`
//! plane, while C's `svt_av1_highbd_warp_affine_c` reads `ref8b` + `ref2b`
//! per sample. `port_warp::HbdWarpRef` is that read as a view, so both forms
//! now reach the identical kernel.
//!
//! # The masked-warp leaf's absurd `p_stride`, which is deliberate
//!
//! C passes `MAX_SB_SQUARE` (16384) as the PREDICTION stride into a
//! `2 * MAX_SB_SQUARE`-BYTE scratch (:1673) — one row and the buffer is gone.
//! It is safe only because that leaf is always compound with
//! `do_average == 0`, and `svt_av1_highbd_warp_affine_c` writes `pred` in no
//! other case; everything real goes to `conv_params->dst`. This port passes
//! the same stride over an EMPTY scratch, so if the dead branch ever became
//! live it panics instead of corrupting memory. The leaf also derives its own
//! `ss_x` / `ss_y` as `plane == 0 ? 0 : 1` (:1657) rather than taking the
//! caller's — reproduced.
//!
//! # Evidence
//!
//! TIER 1 — `svt_aom_enc_make_inter_predictor` is an exported symbol (`nm`:
//! `T`), gated in `tests/c_parity_port_enc_make_pred.rs`.

use crate::port_convolve::{ConvolveParams, SrcView};
use crate::port_convolve_hbd::SrcView16;
use crate::port_diffwtd_d16::build_compound_diffwtd_mask_d16;
use crate::port_inter_predictor::{InterpFilters, highbd_inter_predictor, inter_predictor};
use crate::port_make_pred::{MakePredLeaf, make_pred_leaf, packed_src_geom};
use crate::port_masked_blend::{
    InterInterCompoundData, build_masked_compound_no_round, build_masked_compound_no_round_hbd,
};
use crate::port_masked_compound::{CompoundType, DiffwtdMaskType};
use crate::port_pack::pack_block;
use crate::port_scale_factors::ScaleFactors;
use crate::port_scale_factors::SubpelParams;
use crate::port_subpel_params::{MbEdges, Mv, RefGeometry, compute_subpel_params};
use crate::port_warp::{WarpConvolveParams, WarpPlaneIo, av1_warp_plane};
use crate::port_wedge_masks::WedgeMasks;
use svtav1_types::motion::WarpedMotionParams;

/// The reference plane(s), in whichever of C's three representations the
/// caller holds.
#[derive(Debug, Clone, Copy)]
pub enum SrcPlanes<'a> {
    /// `!is16bit` — one 8-bit plane.
    Lbd(&'a [u8]),
    /// `is16bit && src_ptr_2b != NULL` — SVT's split 8 MSB + 2 LSB pair, both
    /// at `src_stride`.
    Split {
        /// The eight most significant bits.
        msb: &'a [u8],
        /// The two least significant bits, in the TOP two bits of each byte.
        lsb: &'a [u8],
    },
    /// `is16bit && src_ptr_2b == NULL` — an already-unpacked 10-bit plane.
    Hbd(&'a [u16]),
}

impl SrcPlanes<'_> {
    /// C's `is16bit` flag, recovered from the representation.
    pub fn is16bit(&self) -> bool {
        !matches!(self, SrcPlanes::Lbd(_))
    }
}

/// The prediction destination, whose width matches the source's depth.
#[derive(Debug)]
pub enum DstPlane<'a> {
    /// 8-bit output (`!is16bit`).
    Lbd(&'a mut [u8]),
    /// 16-bit output (`is16bit`), which C reaches by casting `dst_ptr`.
    Hbd(&'a mut [u16]),
}

/// The masked-compound arm's extra inputs.
pub struct MaskedCompound<'a> {
    /// `interinter_comp`.
    pub comp: &'a InterInterCompoundData,
    /// `seg_mask` — written by this call for DIFFWTD on plane 0, read for
    /// WEDGE.
    pub seg_mask: &'a mut [u8],
    /// The wedge mask tables, for the WEDGE case.
    pub wedge: &'a WedgeMasks,
    /// `bsize` — indexes the wedge table and the sub-sampling test.
    pub bsize: usize,
    /// `comp_data->mask_type`. C keeps it on `InterInterCompoundData`; the
    /// port's [`InterInterCompoundData`] carries only the three fields the
    /// BLEND reads, so the DIFFWTD mask builder's input rides here.
    pub mask_type: DiffwtdMaskType,
}

/// What this entry refuses rather than approximating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakePredError {
    /// A WARP leaf was selected but no [`WarpedMotionParams`] came with it.
    /// C would dereference a NULL `wm_params` here.
    WarpParamsMissing(MakePredLeaf),
    /// The source representation and the destination depth disagree.
    DepthMismatch,
}

/// `svt_aom_enc_make_inter_predictor` (enc_inter_prediction.c:2515) — the
/// non-warp leaves.
///
/// `pre_y` / `pre_x` are the block's position in the reference plane, in this
/// plane's samples; `src_origin` is where that plane's (0, 0) sits in the
/// slice. On the masked arm C clears `do_average` (:2593) before recursing,
/// which this reproduces on its own copy of `conv_params` rather than mutating
/// the caller's.
#[allow(clippy::too_many_arguments)]
pub fn enc_make_inter_predictor(
    src: SrcPlanes<'_>,
    src_origin: usize,
    src_stride: usize,
    dst: DstPlane<'_>,
    dst_stride: usize,
    conv_buf: &mut [u16],
    pre_y: i32,
    pre_x: i32,
    mv: Mv,
    sf: &ScaleFactors,
    conv_params: &ConvolveParams,
    interp_filters: InterpFilters,
    masked: Option<MaskedCompound<'_>>,
    warp: Option<&mut WarpedMotionParams>,
    geom: RefGeometry,
    blk_width: usize,
    blk_height: usize,
    edges: &MbEdges,
    plane: usize,
    ss_y: i32,
    ss_x: i32,
    bit_depth: i32,
    use_intrabc: bool,
    is_wm: bool,
) -> Result<(), MakePredError> {
    let is16bit = src.is16bit();
    let leaf = make_pred_leaf(is_wm, masked.is_some(), is16bit);
    match (&src, &dst) {
        (SrcPlanes::Lbd(_), DstPlane::Lbd(_))
        | (SrcPlanes::Split { .. } | SrcPlanes::Hbd(_), DstPlane::Hbd(_)) => {}
        _ => return Err(MakePredError::DepthMismatch),
    }

    if is_wm {
        let Some(wm) = warp else {
            return Err(MakePredError::WarpParamsMissing(leaf));
        };
        return warp_leaf(
            src,
            src_origin,
            src_stride,
            dst,
            dst_stride,
            conv_buf,
            wm,
            conv_params,
            masked,
            geom,
            pre_y,
            pre_x,
            blk_width,
            blk_height,
            plane,
            ss_y,
            ss_x,
            bit_depth,
        );
    }

    let (subpel_params, pos_y, pos_x) = compute_subpel_params(
        geom,
        pre_y,
        pre_x,
        mv,
        sf,
        blk_width as i32,
        blk_height as i32,
        edges,
        ss_y,
        ss_x,
    );
    // `src_mod = src_ptr + pos_x + pos_y * src_stride`. C multiplies that by
    // `1 << is16bit` only when there is NO 2-bit plane, because only then is
    // `src_ptr` a byte pointer into `uint16_t` samples; indexing typed slices
    // makes all three cases this one expression.
    let src_index = src_origin as isize + pos_x as isize + pos_y as isize * src_stride as isize;
    let src_index = usize::try_from(src_index).expect("reference position before the plane");

    // The masked arm redirects the CONV_BUF at a private MAX_SB_SIZE-stride
    // scratch, predicts reference 1 into it, then blends. C's scratch is
    // `uint8_t tmp_buf[2 * MAX_SB_SQUARE]` reinterpreted as CONV_BUF_TYPE.
    let mut own_params = *conv_params;
    if masked.is_some() {
        own_params.do_average = false;
        own_params.dst_stride = TMP_BUF_STRIDE;
    }

    let ctx = PredictCtx {
        src,
        src_index,
        src_stride,
        dst_stride,
        subpel_params,
        blk_width,
        blk_height,
        conv_params: own_params,
        interp_filters,
        use_intrabc,
        bit_depth,
        sf,
    };

    let Some(m) = masked else {
        let mut out = dst;
        predict_one(&ctx, conv_buf, &mut out);
        return Ok(());
    };

    // Reference 1's prediction goes into the private CONV_BUF. C writes the
    // pixel output to `dst_ptr` here too and then overwrites it with the
    // blend, so the pixels of this pass are dead; a scratch of the same shape
    // keeps that explicit instead of writing and discarding into the caller's
    // buffer.
    let mut tmp_buf16 = alloc::vec![0u16; TMP_BUF_STRIDE * TMP_BUF_STRIDE];
    let mut scratch_l;
    let mut scratch_h;
    let mut throwaway = match &dst {
        DstPlane::Lbd(d) => {
            scratch_l = alloc::vec![0u8; d.len()];
            DstPlane::Lbd(&mut scratch_l)
        }
        DstPlane::Hbd(d) => {
            scratch_h = alloc::vec![0u16; d.len()];
            DstPlane::Hbd(&mut scratch_h)
        }
    };
    predict_one(&ctx, &mut tmp_buf16, &mut throwaway);
    drop(throwaway);

    // DIFFWTD's mask is derived from the two CONV_BUFs, on plane 0 only.
    if plane == 0 && m.comp.compound_type == CompoundType::DiffWtd {
        build_compound_diffwtd_mask_d16(
            m.seg_mask,
            m.mask_type,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            blk_height,
            blk_width,
            &own_params,
            bit_depth,
        );
    }
    match dst {
        DstPlane::Lbd(d) => build_masked_compound_no_round(
            d,
            dst_stride,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            m.comp,
            m.seg_mask,
            m.wedge,
            m.bsize,
            blk_height,
            blk_width,
            &own_params,
        ),
        DstPlane::Hbd(d) => build_masked_compound_no_round_hbd(
            d,
            dst_stride,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            m.comp,
            m.seg_mask,
            m.wedge,
            m.bsize,
            blk_height,
            blk_width,
            &own_params,
            bit_depth,
        ),
    }
    Ok(())
}

/// Everything the ordinary predictor pass needs, gathered so the masked arm
/// can run it twice without re-deriving anything.
struct PredictCtx<'a> {
    src: SrcPlanes<'a>,
    src_index: usize,
    src_stride: usize,
    dst_stride: usize,
    subpel_params: SubpelParams,
    blk_width: usize,
    blk_height: usize,
    conv_params: ConvolveParams,
    interp_filters: InterpFilters,
    use_intrabc: bool,
    bit_depth: i32,
    sf: &'a ScaleFactors,
}

/// One `svt_inter_predictor` / `svt_highbd_inter_predictor` pass, with the
/// `Split` representation packed on the way in.
fn predict_one(ctx: &PredictCtx<'_>, conv_buf: &mut [u16], out: &mut DstPlane<'_>) {
    match (ctx.src, out) {
        (SrcPlanes::Lbd(p), DstPlane::Lbd(d)) => inter_predictor(
            SrcView::new(p, ctx.src_index, ctx.src_stride),
            d,
            ctx.dst_stride,
            conv_buf,
            &ctx.subpel_params,
            ctx.blk_width,
            ctx.blk_height,
            &ctx.conv_params,
            ctx.interp_filters,
            ctx.use_intrabc,
        ),
        (SrcPlanes::Hbd(p), DstPlane::Hbd(d)) => highbd_inter_predictor(
            SrcView16::new(p, ctx.src_index, ctx.src_stride),
            d,
            ctx.dst_stride,
            conv_buf,
            &ctx.subpel_params,
            ctx.blk_width,
            ctx.blk_height,
            &ctx.conv_params,
            ctx.interp_filters,
            ctx.use_intrabc,
            ctx.bit_depth,
        ),
        (SrcPlanes::Split { msb, lsb }, DstPlane::Hbd(d)) => {
            let g = packed_src_geom(ctx.blk_width, ctx.blk_height, ctx.sf);
            let off = crate::port_pack::INTERPOLATION_OFFSET;
            let window = ctx.src_index - off - off * ctx.src_stride;
            let mut packed = alloc::vec![0u16; g.stride * g.height];
            pack_block(
                &msb[window..],
                ctx.src_stride,
                &lsb[window..],
                ctx.src_stride,
                &mut packed,
                g.stride,
                g.width,
                g.height,
            );
            highbd_inter_predictor(
                SrcView16::new(&packed, g.origin, g.stride),
                d,
                ctx.dst_stride,
                conv_buf,
                &ctx.subpel_params,
                ctx.blk_width,
                ctx.blk_height,
                &ctx.conv_params,
                ctx.interp_filters,
                ctx.use_intrabc,
                ctx.bit_depth,
            );
        }
        _ => unreachable!("depth pairing is validated at the entry point"),
    }
}

/// `MAX_SB_SIZE` — the masked arm's private CONV_BUF stride (:88).
pub const TMP_BUF_STRIDE: usize = 128;

/// `svt_aom_enc_make_inter_predictor`'s `is_wm` branch (:2523-2565) — the
/// plain warp leaf and `av1_make_masked_warp_inter_predictor` (:1633).
///
/// C hands `svt_av1_warp_plane` the reference's PLANE-LOCAL extent
/// (`frame_width >> ss_x`, `frame_height >> ss_y`) and the block's position as
/// `p_col` / `p_row`. There is no `compute_subpel_params` on this path and the
/// motion vector is unused — the affine model IS the motion.
#[allow(clippy::too_many_arguments)]
fn warp_leaf(
    src: SrcPlanes<'_>,
    src_origin: usize,
    src_stride: usize,
    dst: DstPlane<'_>,
    dst_stride: usize,
    conv_buf: &mut [u16],
    wm: &mut WarpedMotionParams,
    conv_params: &ConvolveParams,
    masked: Option<MaskedCompound<'_>>,
    geom: RefGeometry,
    pre_y: i32,
    pre_x: i32,
    blk_width: usize,
    blk_height: usize,
    plane: usize,
    ss_y: i32,
    ss_x: i32,
    bit_depth: i32,
) -> Result<(), MakePredError> {
    let buf_w = geom.frame_width >> ss_x;
    let buf_h = geom.frame_height >> ss_y;

    let Some(m) = masked else {
        // Plain warp: straight into the caller's `dst`, with `conv_params->dst`
        // (the CONV_BUF) live only when compound.
        let cp = to_warp_params(conv_params);
        let cb = conv_params.is_compound.then_some(&mut *conv_buf);
        let mut dst = dst;
        warp_into(
            src, src_origin, src_stride, &mut dst, dst_stride, cb, wm, buf_w, buf_h, pre_x, pre_y,
            blk_width, blk_height, ss_x, ss_y, bit_depth, &cp,
        );
        return Ok(());
    };

    // `av1_make_masked_warp_inter_predictor`: reference 1's warp goes into a
    // private CONV_BUF at MAX_SB_SIZE stride, then blends with the caller's.
    let mut own = *conv_params;
    own.do_average = false;
    own.dst_stride = TMP_BUF_STRIDE;
    let mut tmp_buf16 = alloc::vec![0u16; TMP_BUF_STRIDE * TMP_BUF_STRIDE];
    // The leaf derives its OWN subsampling (:1657), and its `p_stride` is
    // `MAX_SB_SQUARE` into a scratch one row long — dead because this leaf is
    // always compound with `do_average == 0`. An empty slice keeps it dead
    // LOUDLY: a live write panics rather than corrupting.
    let (mss_x, mss_y) = if plane == 0 { (0, 0) } else { (1, 1) };
    let mut dead_pred_l: [u8; 0] = [];
    let mut dead_pred_h: [u16; 0] = [];
    let cp = to_warp_params(&own);
    {
        let io = match src {
            SrcPlanes::Lbd(p) => WarpPlaneIo::Lowbd {
                reference: &p[src_origin..],
                pred: &mut dead_pred_l,
            },
            SrcPlanes::Hbd(p) => WarpPlaneIo::Highbd {
                reference: &p[src_origin..],
                pred: &mut dead_pred_h,
                bd: bit_depth,
            },
            SrcPlanes::Split { msb, lsb } => WarpPlaneIo::HighbdSplit {
                msb: &msb[src_origin..],
                lsb: &lsb[src_origin..],
                pred: &mut dead_pred_h,
                bd: bit_depth,
            },
        };
        av1_warp_plane(
            wm,
            io,
            buf_w,
            buf_h,
            src_stride,
            Some(&mut tmp_buf16),
            pre_x,
            pre_y,
            blk_width as i32,
            blk_height as i32,
            MAX_SB_SQUARE,
            mss_x,
            mss_y,
            &cp,
        );
    }

    if plane == 0 && m.comp.compound_type == CompoundType::DiffWtd {
        build_compound_diffwtd_mask_d16(
            m.seg_mask,
            m.mask_type,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            blk_height,
            blk_width,
            &own,
            bit_depth,
        );
    }
    match dst {
        DstPlane::Lbd(d) => build_masked_compound_no_round(
            d,
            dst_stride,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            m.comp,
            m.seg_mask,
            m.wedge,
            m.bsize,
            blk_height,
            blk_width,
            &own,
        ),
        DstPlane::Hbd(d) => build_masked_compound_no_round_hbd(
            d,
            dst_stride,
            conv_buf,
            conv_params.dst_stride,
            &tmp_buf16,
            TMP_BUF_STRIDE,
            m.comp,
            m.seg_mask,
            m.wedge,
            m.bsize,
            blk_height,
            blk_width,
            &own,
            bit_depth,
        ),
    }
    Ok(())
}

/// `MAX_SB_SQUARE` — the masked-warp leaf's (dead) prediction stride.
const MAX_SB_SQUARE: usize = TMP_BUF_STRIDE * TMP_BUF_STRIDE;

/// `ConvolveParams` -> `WarpConvolveParams`. C has ONE struct; the port has
/// two because the warp module predates the convolve one, and they carry the
/// identical eight fields.
fn to_warp_params(cp: &ConvolveParams) -> WarpConvolveParams {
    WarpConvolveParams {
        do_average: cp.do_average,
        dst_stride: cp.dst_stride,
        round_0: cp.round_0,
        round_1: cp.round_1,
        is_compound: cp.is_compound,
        use_jnt_comp_avg: cp.use_jnt_comp_avg,
        fwd_offset: cp.fwd_offset,
        bck_offset: cp.bck_offset,
    }
}

/// One `svt_av1_warp_plane` call over whichever representation the caller has.
#[allow(clippy::too_many_arguments)]
fn warp_into(
    src: SrcPlanes<'_>,
    src_origin: usize,
    src_stride: usize,
    dst: &mut DstPlane<'_>,
    dst_stride: usize,
    conv_buf: Option<&mut [u16]>,
    wm: &mut WarpedMotionParams,
    buf_w: i32,
    buf_h: i32,
    p_col: i32,
    p_row: i32,
    blk_width: usize,
    blk_height: usize,
    ss_x: i32,
    ss_y: i32,
    bit_depth: i32,
    cp: &WarpConvolveParams,
) {
    let io = match (src, dst) {
        (SrcPlanes::Lbd(p), DstPlane::Lbd(d)) => WarpPlaneIo::Lowbd {
            reference: &p[src_origin..],
            pred: d,
        },
        (SrcPlanes::Hbd(p), DstPlane::Hbd(d)) => WarpPlaneIo::Highbd {
            reference: &p[src_origin..],
            pred: d,
            bd: bit_depth,
        },
        (SrcPlanes::Split { msb, lsb }, DstPlane::Hbd(d)) => WarpPlaneIo::HighbdSplit {
            msb: &msb[src_origin..],
            lsb: &lsb[src_origin..],
            pred: d,
            bd: bit_depth,
        },
        _ => unreachable!("depth pairing is validated at the entry point"),
    };
    av1_warp_plane(
        wm,
        io,
        buf_w,
        buf_h,
        src_stride,
        conv_buf,
        p_col,
        p_row,
        blk_width as i32,
        blk_height as i32,
        dst_stride,
        ss_x,
        ss_y,
        cp,
    );
}

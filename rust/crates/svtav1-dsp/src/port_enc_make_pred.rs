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
//! # Scope, stated as a fraction
//!
//! Two of C's four leaves are here: **regular** and **masked-compound**
//! (`av1_make_masked_scaled_inter_predictor`). The two WARP leaves
//! (`svt_av1_warp_plane` and `av1_make_masked_warp_inter_predictor`, :1633)
//! return [`MakePredError::WarpNotWired`] rather than a plausible-but-wrong
//! prediction (`WORKING-ON-THIS.md` §6). They are blocked on a real gap one
//! level down: `crate::port_warp::av1_warp_plane`'s `WarpPlaneIo` has `Lowbd`
//! and `Highbd` arms but no SPLIT one, while C's `svt_av1_warp_plane`
//! (warped_motion.c:868) takes `ref_2b` and hands it to `highbd_warp_plane`.
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
use crate::port_wedge_masks::WedgeMasks;

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
    /// The caller asked for a WARP leaf. See this module's scope note.
    WarpNotWired(MakePredLeaf),
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
    if is_wm {
        return Err(MakePredError::WarpNotWired(leaf));
    }
    match (&src, &dst) {
        (SrcPlanes::Lbd(_), DstPlane::Lbd(_))
        | (SrcPlanes::Split { .. } | SrcPlanes::Hbd(_), DstPlane::Hbd(_)) => {}
        _ => return Err(MakePredError::DepthMismatch),
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

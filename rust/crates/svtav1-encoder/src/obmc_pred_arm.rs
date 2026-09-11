//! OBMC prediction for one inter block, and the scratch it runs on.
//!
//! C reference: `Source/Lib/Codec/enc_inter_prediction.c`
//! (`av1_inter_prediction_obmc` :2925, `svt_aom_precompute_obmc_data` :1816,
//! `build_prediction_by_above_preds` :1335, `build_prediction_by_left_preds`
//! :1380) over the buffers `Source/Lib/Codec/md_process.c:374-381` allocates.
//!
//! # What OBMC is, in one paragraph
//!
//! An OBMC block keeps its own motion vector and its own translation
//! prediction, and then BLENDS the edges of that prediction with what its
//! already-coded ABOVE and LEFT neighbours would have predicted for the same
//! pixels using THEIR motion. The blend depth is half the block, tapering. So
//! the work is: predict the block normally (the caller has already done that),
//! predict each overlapping neighbour's motion into scratch, and blend.
//!
//! # The memory model is C's, deliberately
//!
//! C allocates the OBMC scratch ONCE per `ModeDecisionContext` — i.e. once per
//! MD thread, not per frame and not per block — sized by SUPERBLOCK, and only
//! when `obmc_allowed` (`md_process.c:374`):
//!
//! ```text
//! obmc_buff_0/_1 : sb_size * sb_size * bits * MAX_PLANES  bytes
//! obmc_conv_buf  : sb_size * sb_size                      u16
//! wsrc_buf/mask_buf : sb_size * sb_size                   i32
//! ```
//!
//! [`ObmcBuffers`] is that, as a thread-local grown on first use and reused
//! for the life of the thread. Nothing here allocates per block or per
//! candidate: the neighbour walks and their plane geometry are fixed arrays
//! (`MAX_VISITED_NB`, `MAX_NB_PRED_GEOM`), which is what C's callback shape
//! amounts to.
//!
//! # And the neighbour predictions are computed ONCE PER BLOCK
//!
//! This is a PERFORMANCE property, not an allocation one, and it is the reason
//! C carries `obmc_neighbor_luma_pred_ready` / `_chroma_pred_ready` and the
//! `use_precomputed_obmc` flag at all. A neighbour's prediction depends on the
//! NEIGHBOUR's motion and the current block's geometry — never on the
//! candidate's own MV — so every OBMC candidate of one block reuses the same
//! two scratch buffers. [`ObmcBuffers::neighbours_ready_for`] is that cache,
//! keyed on the block origin and component mask exactly as C's pair of flags
//! is keyed on the block and the mask.

use alloc::vec::Vec;
use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes};
use svtav1_dsp::port_obmc_build::{above_preds_edge_adjust, left_preds_edge_adjust};
use svtav1_dsp::port_obmc_nb_pred::{
    NbSide, Neighbour, ObmcRefPic, ObmcScratch, build_prediction_by_nb_pred,
};
use svtav1_dsp::port_obmc_pred::{
    COMPONENT_CHROMA, COMPONENT_LUMA, MAX_VISITED_NB, NbMi, ObmcAdjacent, ObmcPlanes, PredEdges,
    VisitedNb, build_obmc_inter_prediction, foreach_overlappable_nb_above_into,
    foreach_overlappable_nb_left_into, nb_max_above, nb_max_left,
    setup_build_prediction_by_above_pred, setup_build_prediction_by_left_pred,
};
use svtav1_dsp::port_subpel_params::MbEdges;
use svtav1_types::block::BlockSize;

/// The longest mi span either OBMC walk reads: one superblock edge in mi
/// units (128 / 4 = 32) plus the one cell past it the 4-wide pairing rule can
/// reach (`idx = above_mi_col - mi_col + 1`).
const MAX_NB_SPAN: usize = 33;

/// A cell the walk treats as "not overlappable" — what an out-of-range or
/// intra neighbour contributes.
const NB_MI_INTRA: NbMi = NbMi {
    bsize: BlockSize::Block4x4,
    overlappable: false,
};

/// C `MAX_PLANES`.
const MAX_PLANES: usize = 3;

/// Which block and component mask the cached neighbour predictions belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NeighbourKey {
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    component_mask: u32,
}

/// C's per-`ModeDecisionContext` OBMC scratch (`md_process.c:374-381`).
///
/// Grown once, on the first OBMC block a thread sees, to the SUPERBLOCK size —
/// exactly C's sizing — and reused for the life of the thread. See the module
/// header for why this shape and not a per-block allocation.
pub(crate) struct ObmcBuffers {
    /// C `ctx->obmc_buff_0` — the ABOVE neighbours' predictions, three planes
    /// back to back at the block's own stride.
    buf0: Vec<u8>,
    /// C `ctx->obmc_buff_1` — the LEFT neighbours'.
    buf1: Vec<u8>,
    /// C `ctx->obmc_conv_buf`.
    conv: Vec<u16>,
    /// The block the contents of `buf0`/`buf1` belong to, or `None` when they
    /// hold another block's. C's `obmc_neighbor_{luma,chroma}_pred_ready`.
    ready: Option<NeighbourKey>,
    /// The superblock edge this was sized for; a larger one regrows.
    sb_size: usize,
}

impl Default for ObmcBuffers {
    fn default() -> Self {
        Self {
            buf0: Vec::new(),
            buf1: Vec::new(),
            conv: Vec::new(),
            ready: None,
            sb_size: 0,
        }
    }
}

impl ObmcBuffers {
    /// C `md_process.c:374-381`'s allocation, deferred to the first OBMC block
    /// this thread predicts. A frame that never reaches OBMC pays nothing,
    /// which is C's `if (obmc_allowed)` guard.
    fn ensure(&mut self, sb_size: usize) {
        if self.sb_size >= sb_size {
            return;
        }
        let n = sb_size * sb_size;
        // `bits` is 1 here: this arm is the 8-bit one. C sizes for
        // `hbd_md ? 2 : 1`, so a future 10-bit arm doubles these two.
        self.buf0.resize(n * MAX_PLANES, 0);
        self.buf1.resize(n * MAX_PLANES, 0);
        self.conv.resize(n, 0);
        self.sb_size = sb_size;
        // The contents are now meaningless for whatever block they described.
        self.ready = None;
    }

    /// True when `buf0`/`buf1` already hold THIS block's neighbour
    /// predictions, so the caller can skip straight to the blend. C's
    /// `use_precomputed_obmc` + the two `_pred_ready` flags.
    fn neighbours_ready_for(&self, key: NeighbourKey) -> bool {
        self.ready == Some(key)
    }
}

thread_local! {
    /// One per thread, like C's one per `ModeDecisionContext`.
    static OBMC_BUFFERS: core::cell::RefCell<ObmcBuffers> =
        core::cell::RefCell::new(ObmcBuffers::default());
}

/// One mi cell as the OBMC walk reads it: the neighbour's shape, whether it is
/// overlappable, and the motion to predict with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObmcNbCell {
    /// C `mbmi->bsize`.
    pub bsize: BlockSize,
    /// C `is_neighbor_overlappable` — `ref_frame[0] > INTRA_FRAME`.
    pub overlappable: bool,
    /// C `mbmi->block_mi.ref_frame[0]`.
    pub ref_frame: i8,
    /// C `mbmi->block_mi.mv[0]`.
    pub mv: svtav1_types::motion::Mv,
    /// C `mbmi->block_mi.interp_filters`.
    pub interp_filters: u32,
}

/// Everything the OBMC predictor needs that is not the block's own prediction.
pub(crate) struct ObmcCtx<'a> {
    /// The reference picture per `MvReferenceFrame`, as the block's own
    /// prediction reads it.
    pub padded_by_ref: &'a [Option<&'a crate::picture::PaddedRef>; 8],
    /// The mi row ABOVE the block, from `mi_col` rightwards.
    pub above_row: &'a [ObmcNbCell],
    /// The mi column LEFT of the block, from `mi_row` downwards.
    pub left_col: &'a [ObmcNbCell],
    /// `xd->up_available` / `xd->left_available`.
    pub up_available: bool,
    pub left_available: bool,
    /// `cm->mi_cols` / `cm->mi_rows`.
    pub mi_cols: usize,
    pub mi_rows: usize,
    /// `seq_header.sb_size` in pixels.
    pub sb_size: usize,
    /// The ALIGNED frame extent, which is what the MC clamp is taken against.
    pub frame_w: usize,
    pub frame_h: usize,
    /// The block's own `MbEdges`, before either walk rewrites a pair of them.
    pub edges: MbEdges,
}

/// Blend the ABOVE and LEFT neighbours' motion into a block's own prediction.
///
/// C `av1_inter_prediction_obmc` (:2925) with `use_precomputed_obmc` — the
/// caller has already written the block's translation prediction into
/// `y`/`u`/`v`, and this rewrites their edges in place.
///
/// `bw`/`bh` are the block's LUMA dimensions and `uv_stride` the chroma one;
/// chroma is skipped entirely when `u` is empty, which is the `component_mask`
/// C would pass as LUMA-only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_obmc_in_place(
    ctx: &ObmcCtx<'_>,
    bsize: BlockSize,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    y: &mut [u8],
    y_stride: usize,
    u: &mut [u8],
    v: &mut [u8],
    uv_stride: usize,
) {
    let has_uv = !u.is_empty();
    let component_mask = if has_uv {
        COMPONENT_LUMA | COMPONENT_CHROMA
    } else {
        COMPONENT_LUMA
    };
    let mi_row = (org_y / 4) as i32;
    let mi_col = (org_x / 4) as i32;
    let n4_w = bw / 4;
    let n4_h = bh / 4;

    // ---- which neighbours overlap (C's two `foreach_overlappable_nb_*`) ----
    let mut above_nbs = [VisitedNb {
        rel_mi: 0,
        nb_mi_size: 0,
        mi_index: 0,
    }; MAX_VISITED_NB];
    let mut left_nbs = above_nbs;
    // C reads the mi grid in place through `xd->mi`; this projects the two
    // spans it walks into fixed arrays rather than two per-block `Vec`s. The
    // longest span is one superblock edge in mi units plus the one cell the
    // 4-wide pairing rule can reach past it.
    let mut above_cells = [NB_MI_INTRA; MAX_NB_SPAN];
    let mut left_cells = [NB_MI_INTRA; MAX_NB_SPAN];
    let n_above_cells = ctx.above_row.len().min(MAX_NB_SPAN);
    let n_left_cells = ctx.left_col.len().min(MAX_NB_SPAN);
    for (dst, c) in above_cells
        .iter_mut()
        .zip(ctx.above_row.iter().take(n_above_cells))
    {
        *dst = NbMi {
            bsize: c.bsize,
            overlappable: c.overlappable,
        };
    }
    for (dst, c) in left_cells
        .iter_mut()
        .zip(ctx.left_col.iter().take(n_left_cells))
    {
        *dst = NbMi {
            bsize: c.bsize,
            overlappable: c.overlappable,
        };
    }
    let above_cells = &above_cells[..n_above_cells];
    let left_cells = &left_cells[..n_left_cells];
    let n_above = foreach_overlappable_nb_above_into(
        ctx.up_available,
        above_cells,
        mi_col as usize,
        n4_w,
        ctx.mi_cols,
        nb_max_above(bsize),
        &mut above_nbs,
    );
    let n_left = foreach_overlappable_nb_left_into(
        ctx.left_available,
        left_cells,
        mi_row as usize,
        n4_h,
        ctx.mi_rows,
        nb_max_left(bsize),
        &mut left_nbs,
    );
    if n_above == 0 && n_left == 0 {
        return;
    }

    let key = NeighbourKey {
        org_x,
        org_y,
        bw,
        bh,
        component_mask,
    };
    OBMC_BUFFERS.with(|cell| {
        let mut bufs = cell.borrow_mut();
        bufs.ensure(ctx.sb_size);

        // C `use_precomputed_obmc`: the neighbour predictions depend on the
        // NEIGHBOURS, never on this candidate's MV, so they are built once per
        // block and every OBMC candidate after the first reuses them.
        if !bufs.neighbours_ready_for(key) {
            build_neighbour_predictions(
                ctx,
                &mut bufs,
                bsize,
                mi_row,
                mi_col,
                bw,
                bh,
                &above_nbs[..n_above],
                &left_nbs[..n_left],
                component_mask,
            );
            bufs.ready = Some(key);
        }

        // ---- the blend (C `av1_build_obmc_inter_prediction`) ----
        let plane_len = bw * bh;
        let (b0, b1) = (&bufs.buf0, &bufs.buf1);
        let above = ObmcAdjacent {
            plane: [
                &b0[..plane_len],
                &b0[plane_len..plane_len * 2],
                &b0[plane_len * 2..plane_len * 3],
            ],
            stride: [bw, bw, bw],
        };
        let left = ObmcAdjacent {
            plane: [
                &b1[..plane_len],
                &b1[plane_len..plane_len * 2],
                &b1[plane_len * 2..plane_len * 3],
            ],
            stride: [bw, bw, bw],
        };
        let mut planes = ObmcPlanes {
            dst: [y, u, v],
            dst_stride: [y_stride, uv_stride, uv_stride],
        };
        build_obmc_inter_prediction(
            &mut planes,
            &above,
            &above_nbs[..n_above],
            &left,
            &left_nbs[..n_left],
            bsize,
            component_mask,
        );
    });
}

/// C `build_prediction_by_above_preds` (:1335) + `_left_preds` (:1380): every
/// overlapping neighbour's motion, predicted into the two scratch buffers.
#[allow(clippy::too_many_arguments)]
fn build_neighbour_predictions(
    ctx: &ObmcCtx<'_>,
    bufs: &mut ObmcBuffers,
    bsize: BlockSize,
    mi_row: i32,
    mi_col: i32,
    bw: usize,
    bh: usize,
    above_nbs: &[VisitedNb],
    left_nbs: &[VisitedNb],
    component_mask: u32,
) {
    let plane_len = bw * bh;
    for (side, nbs) in [(NbSide::Above, above_nbs), (NbSide::Left, left_nbs)] {
        for nb in nbs {
            // `mi_index`, NOT `rel_mi`: for a 4-wide neighbour C takes the
            // GEOMETRY from the start of the pair and the MODE INFO from the
            // pair's second half. See `VisitedNb::mi_index`.
            let cell = match side {
                NbSide::Above => ctx.above_row[nb.mi_index.min(ctx.above_row.len() - 1)],
                NbSide::Left => ctx.left_col[nb.mi_index.min(ctx.left_col.len() - 1)],
            };
            let Some(reference) = ctx.padded_by_ref[cell.ref_frame.max(0) as usize] else {
                // An overlappable neighbour names a reference this frame does
                // not carry. C cannot reach it — the mi grid and the reference
                // table are filled from the same list — so it is a wiring bug.
                panic!(
                    "an OBMC neighbour names reference {} with no DPB picture",
                    cell.ref_frame
                );
            };
            let Some((refu, refv)) = reference.uv.as_ref() else {
                continue;
            };

            // C rewrites the edges TWICE per walk, and missing either half
            // moves pixels:
            //
            //  1. ONCE per WALK, before any neighbour
            //     (`build_prediction_by_above_preds` :1344): the ABOVE walk
            //     widens `mb_to_bottom_edge` because its prediction is only
            //     HALF the block tall (capped at 32), and the LEFT walk widens
            //     `mb_to_right_edge` for the mirror reason. Without it the MC
            //     clamp cuts the neighbour prediction short.
            //  2. ONCE per NEIGHBOUR (`av1_setup_build_prediction_by_*_pred`),
            //     which rewrites the OTHER two edges from the walk's captured
            //     `mb_to_far_edge` — the block's own far edge, taken BEFORE
            //     this walk's adjustment.
            let mut e = PredEdges {
                to_left: ctx.edges.to_left,
                to_right: ctx.edges.to_right,
                to_top: ctx.edges.to_top,
                to_bottom: ctx.edges.to_bottom,
            };
            match side {
                NbSide::Above => {
                    e.to_bottom += above_preds_edge_adjust((bh / 4) as i32);
                    setup_build_prediction_by_above_pred(
                        &mut e,
                        mi_col,
                        nb.rel_mi as i32,
                        nb.nb_mi_size as i32,
                        (bw / 4) as i32,
                        ctx.edges.to_right,
                    );
                }
                NbSide::Left => {
                    e.to_right += left_preds_edge_adjust((bw / 4) as i32);
                    setup_build_prediction_by_left_pred(
                        &mut e,
                        mi_row,
                        nb.rel_mi as i32,
                        nb.nb_mi_size as i32,
                        (bh / 4) as i32,
                        ctx.edges.to_bottom,
                    );
                }
            }
            let edges = MbEdges {
                to_left: e.to_left,
                to_right: e.to_right,
                to_top: e.to_top,
                to_bottom: e.to_bottom,
            };

            let dst = match side {
                NbSide::Above => &mut bufs.buf0,
                NbSide::Left => &mut bufs.buf1,
            };
            let (p0, rest) = dst.split_at_mut(plane_len);
            let (p1, rest2) = rest.split_at_mut(plane_len);
            let (p2, _) = rest2.split_at_mut(plane_len);

            let _ = build_prediction_by_nb_pred(
                side,
                ObmcRefPic {
                    y: SrcPlanes::Lbd(&reference.y.buf),
                    u: SrcPlanes::Lbd(&refu.buf),
                    v: SrcPlanes::Lbd(&refv.buf),
                    stride: [reference.y.stride, refu.stride, refv.stride],
                    dims: (reference.y.width as i32, reference.y.height as i32),
                },
                [reference.y.origin, refu.origin, refv.origin],
                ObmcScratch {
                    y: DstPlane::Lbd(p0),
                    u: DstPlane::Lbd(p1),
                    v: DstPlane::Lbd(p2),
                    stride: [bw, bw, bw],
                },
                (ctx.frame_w as i32, ctx.frame_h as i32),
                ctx.sb_size,
                bsize,
                mi_row,
                mi_col,
                Neighbour {
                    mv: svtav1_dsp::port_subpel_params::Mv {
                        x: cell.mv.x,
                        y: cell.mv.y,
                    },
                    interp_filters: cell.interp_filters,
                    extent_mi: nb.nb_mi_size,
                    rel_mi: nb.rel_mi,
                },
                &edges,
                1,
                1,
                component_mask,
                &mut bufs.conv,
                8,
            );
        }
    }
}

#[cfg(test)]
mod scratch_tests {
    use super::*;

    /// The scratch is C's, sized by C's formula (`md_process.c:374-381`):
    /// `sb_size * sb_size * bits * MAX_PLANES` for each of the two neighbour
    /// buffers and `sb_size * sb_size` for the convolve scratch, with
    /// `bits = 1` on this 8-bit arm.
    ///
    /// It is pinned because the whole point of this shape is that it does NOT
    /// scale with the number of blocks or candidates: a regression to
    /// per-block buffers would still pass every byte gate.
    #[test]
    fn scratch_is_superblock_sized_and_grown_once() {
        let mut b = ObmcBuffers::default();
        assert_eq!(b.buf0.len(), 0, "nothing is allocated before the first use");

        b.ensure(64);
        assert_eq!(b.buf0.len(), 64 * 64 * MAX_PLANES);
        assert_eq!(b.buf1.len(), 64 * 64 * MAX_PLANES);
        assert_eq!(b.conv.len(), 64 * 64);
        let (p0, p1) = (b.buf0.as_ptr(), b.buf1.as_ptr());

        // A second block at the same superblock size REUSES the buffers —
        // this is the property that makes it C's memory model and not a
        // per-block allocation wearing a thread-local's clothes.
        b.ensure(64);
        assert_eq!(b.buf0.as_ptr(), p0, "re-entry reallocated buf0");
        assert_eq!(b.buf1.as_ptr(), p1, "re-entry reallocated buf1");

        // A LARGER superblock grows once, and invalidates the cached
        // neighbour predictions because they described a different block.
        b.ready = Some(NeighbourKey {
            org_x: 0,
            org_y: 0,
            bw: 16,
            bh: 16,
            component_mask: 7,
        });
        b.ensure(128);
        assert_eq!(b.buf0.len(), 128 * 128 * MAX_PLANES);
        assert!(b.ready.is_none(), "a regrow must drop the stale cache");
    }

    /// The per-block cache is keyed on the block AND the component mask, the
    /// two things C's `obmc_neighbor_{luma,chroma}_pred_ready` pair is keyed
    /// on. A stale hit would blend one block's edges with another's
    /// neighbours.
    #[test]
    fn the_neighbour_cache_is_keyed_on_the_block() {
        let mut b = ObmcBuffers::default();
        b.ensure(64);
        let key = NeighbourKey {
            org_x: 32,
            org_y: 16,
            bw: 16,
            bh: 16,
            component_mask: 7,
        };
        assert!(!b.neighbours_ready_for(key), "empty cache must miss");
        b.ready = Some(key);
        assert!(b.neighbours_ready_for(key), "same block must hit");
        for other in [
            NeighbourKey { org_x: 48, ..key },
            NeighbourKey { org_y: 32, ..key },
            NeighbourKey { bw: 8, ..key },
            NeighbourKey { bh: 8, ..key },
            NeighbourKey {
                component_mask: 1,
                ..key
            },
        ] {
            assert!(
                !b.neighbours_ready_for(other),
                "a different block/mask must MISS: {other:?}"
            );
        }
    }
}

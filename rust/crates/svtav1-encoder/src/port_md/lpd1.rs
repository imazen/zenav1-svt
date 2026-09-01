//! The LIGHT-PD1 decision gates of `Source/Lib/Codec/product_coding_loop.c`
//! plus the chroma-complexity detectors they share with regular PD1.
//!
//! | this module | C |
//! |---|---|
//! | [`chroma_complexity_check_pred`] | `:6013-6141` (**EXPORTED**) |
//! | [`chroma_complexity_check`] + its two arms | `:6143-6327` (`static`) |
//! | [`should_perform_tx`] | `lpd1_should_perform_tx` `:6329-6410` |
//! | [`blk_skip_luma_rd`] | `lpd1_blk_skip_luma_rd` `:6417-6441` |
//! | [`chroma_energy_skip`] | `lpd1_chroma_energy_skip` `:6453-6540` |
//! | [`globalmv_bypass_allowed`] | the predicate half of `lpd1_try_mds0_bypass` `:8939-8965` |
//!
//! # Why LPD1 matters for inter
//!
//! Light PD1 is the fast INTER coding path: it is entered only off an
//! I-slice (`lpd1_try_mds0_bypass` returns immediately on one, `:8942`) and
//! every gate here is either inter-only or reads inter neighbour state
//! (NEARESTMV / NEAREST_NEARESTMV modes, neighbour `skip` flags, the ME
//! MV). None of it had a counterpart in the port: the still-image funnel
//! never builds an LPD1 context.
//!
//! # Shape of the translation
//!
//! C signals these decisions by MUTATING the candidate buffer — zeroing
//! `y_has_coeff`, rewriting `eob`, overwriting the distortion array — and
//! returning a `bool` that means "and also do the rest of what I did".
//! Reproducing that here would need the whole buffer type. Instead each
//! function returns the DECISION as a type ([`SkipLumaOutcome`],
//! [`ChromaEnergyOutcome`]), and its doc comment lists the exact writes the
//! caller owes C. The arithmetic reaching the decision is unchanged.
//!
//! # Evidence
//!
//! [`chroma_complexity_check_pred`] is **tier 1** — `nm -g` reports
//! `T _chroma_complexity_check_pred` (no `svt_aom_` prefix, no header
//! prototype), and `tests/c_parity_pcl_lpd1.rs` drives the real symbol
//! through `shims/pcl_shims.c`. Everything else in this file is `static` in
//! C with no symbol and is **tier 4** — hand-derived vectors traced against
//! the C source (`docs/WORKING-ON-THIS.md` §4).
//!
//! # Reachability
//!
//! Nothing calls this yet — the public entry point still refuses inter
//! frames (`docs/WORKING-ON-THIS.md` §7).

use crate::md_subpel::{EB_AV1_VAR_OFFS, NUM_PELS_LOG2_LOOKUP};
use crate::port_enc_mode_config::encdec::Lpd1TxSkipDecisionCtrls;

/// C `COMPONENT_TYPE` (definitions.h:695-702), restricted to the four
/// values these detectors produce. `COMPONENT_ALL` / `COMPONENT_NONE`
/// exist in C but no path here can return them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ComponentType {
    /// `COMPONENT_LUMA` — no chroma plane is interesting.
    Luma = 0,
    /// `COMPONENT_CHROMA` — both Cb and Cr are.
    Chroma = 1,
    /// `COMPONENT_CHROMA_CB`
    Cb = 2,
    /// `COMPONENT_CHROMA_CR`
    Cr = 3,
}

/// A block-aligned view of one plane: `data` starts at the block's first
/// sample, `stride` is the picture stride.
///
/// The whole file works in these rather than in raw offsets, so every read
/// is a bounds-checked slice and the C `+ input_cb_origin_in_index` pointer
/// arithmetic happens once, in the caller.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

impl<'a> Plane<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], stride: usize) -> Self {
        Plane { data, stride }
    }
}

/// The three planes of one block, each already offset to its origin.
#[derive(Debug, Clone, Copy)]
pub struct BlockPlanes<'a> {
    pub y: Plane<'a>,
    pub u: Plane<'a>,
    pub v: Plane<'a>,
}

/// The chroma geometry these detectors read off `ctx->blk_geom`.
#[derive(Debug, Clone, Copy)]
pub struct UvGeom {
    /// `blk_geom->bwidth_uv`
    pub bwidth_uv: usize,
    /// `blk_geom->bheight_uv`
    pub bheight_uv: usize,
    /// `blk_geom->bsize_uv`, an index into
    /// [`NUM_PELS_LOG2_LOOKUP`] (C `eb_num_pels_log2_lookup`).
    pub bsize_uv: usize,
}

/// C `ctx->chroma_complexity` + `ctx->cfl_complexity`, the pair
/// [`chroma_complexity_check_pred`] reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromaState {
    pub chroma_complexity: ComponentType,
    pub cfl_complexity: ComponentType,
}

/// C `RDCOST` (rd_cost.h:36):
/// `ROUND_POWER_OF_TWO(R * RM, AV1_PROB_COST_SHIFT) + D * (1 << RDDIV_BITS)`
/// with `AV1_PROB_COST_SHIFT == 9` and `RDDIV_BITS == 7`.
#[inline]
fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + (1 << 8)) >> 9) + (dist << 7)
}

/// C `ROUND_POWER_OF_TWO(value, n)` (definitions.h:478).
#[inline]
fn round_power_of_two(value: u32, n: u32) -> u32 {
    (value + ((1u32 << n) >> 1)) >> n
}

/// C's `svt_nxm_sad_kernel(a, a_stride << shift, b, b_stride << shift,
/// h >> shift, w)` — every `1 << shift`-th row of the block.
///
/// Row subsampling is expressed as a stride multiplier here for the same
/// reason C writes it that way: the two buffers are subsampled in lockstep,
/// so the comparison stays over corresponding rows.
fn sad_subsampled(a: Plane<'_>, b: Plane<'_>, width: usize, height: usize, shift: u32) -> u32 {
    let rows = height >> shift;
    let (a_stride, b_stride) = (a.stride << shift, b.stride << shift);
    let mut sad = 0u32;
    for r in 0..rows {
        let ar = &a.data[r * a_stride..r * a_stride + width];
        let br = &b.data[r * b_stride..r * b_stride + width];
        for x in 0..width {
            sad += u32::from(ar[x].abs_diff(br[x]));
        }
    }
    sad
}

/// C's `fn_ptr->vf(plane, stride, svt_aom_eb_av1_var_offs, 0, &sse)`
/// normalised by `ROUND_POWER_OF_TWO(var, eb_num_pels_log2_lookup[bsize])`
/// — the per-pixel activity of a chroma plane against a flat 128 reference.
fn block_variance_vs_flat(p: Plane<'_>, geom: UvGeom) -> u32 {
    let var = svtav1_dsp::variance::variance_diff(
        p.data,
        p.stride,
        &EB_AV1_VAR_OFFS,
        0,
        geom.bwidth_uv,
        geom.bheight_uv,
    );
    round_power_of_two(var, u32::from(NUM_PELS_LOG2_LOOKUP[geom.bsize_uv]))
}

/// The `cb > th && cr > th` / `cb > th` / `cr > th` ladder both detectors
/// end with. `None` == neither plane cleared the threshold.
#[inline]
fn dominant(cb_over: bool, cr_over: bool) -> Option<ComponentType> {
    match (cb_over, cr_over) {
        (true, true) => Some(ComponentType::Chroma),
        (true, false) => Some(ComponentType::Cb),
        (false, true) => Some(ComponentType::Cr),
        (false, false) => None,
    }
}

/// C's "promote, never demote" merge at `:6079-6084` and `:6129-6135`: a
/// plane already flagged stays flagged, and flagging the other one
/// promotes to `COMPONENT_CHROMA`.
fn merge_component(prior: ComponentType, found: ComponentType) -> ComponentType {
    match found {
        ComponentType::Chroma => ComponentType::Chroma,
        ComponentType::Cb if prior == ComponentType::Cr => ComponentType::Chroma,
        ComponentType::Cr if prior == ComponentType::Cb => ComponentType::Chroma,
        other => other,
    }
}

// ---------------------------------------------------------------------------
// chroma_complexity_check_pred (:6013-6141, EXPORTED)
// ---------------------------------------------------------------------------

/// C `chroma_complexity_check_pred` (`:6013-6141`, EXPORTED), 8-bit arm.
///
/// Compares each chroma plane's PREDICTION error against twice the luma
/// plane's, over the chroma-sized region, and (when `use_var`) additionally
/// against a fixed activity threshold.
///
/// Three details a paraphrase loses:
///
/// * **It returns early on `COMPONENT_CHROMA`** (`:6015`) — once both
///   planes are flagged there is nothing left to learn, and in particular
///   `cfl_complexity` is NOT updated on that path.
/// * **The row subsampling is unconditional** here (`shift` from
///   `bheight_uv` alone, `:6021`), unlike [`chroma_complexity_check`] where
///   it depends on the detector level.
/// * **A plane already flagged is not re-measured** (`:6031`, `:6040`): its
///   distortion stays 0, which cannot exceed `y_dist`, so the ladder below
///   preserves the prior flag rather than re-deriving it.
///
/// `cfl_cplx_th` is C's `ctx->cfl_ctrls.cplx_th`.
#[must_use]
pub fn chroma_complexity_check_pred(
    state: ChromaState,
    geom: UvGeom,
    input: BlockPlanes<'_>,
    pred: BlockPlanes<'_>,
    use_var: bool,
    cfl_cplx_th: u32,
) -> ChromaState {
    if state.chroma_complexity == ComponentType::Chroma {
        return state;
    }
    let mut out = state;

    let shift: u32 = if geom.bheight_uv > 8 {
        2
    } else if geom.bheight_uv > 4 {
        1
    } else {
        0
    };
    let sad = |a: Plane<'_>, b: Plane<'_>| {
        u64::from(sad_subsampled(a, b, geom.bwidth_uv, geom.bheight_uv, shift))
    };
    let y_dist = sad(input.y, pred.y);
    let cb_dist = if matches!(
        state.chroma_complexity,
        ComponentType::Luma | ComponentType::Cr
    ) {
        sad(input.u, pred.u)
    } else {
        0
    };
    let cr_dist = if matches!(
        state.chroma_complexity,
        ComponentType::Luma | ComponentType::Cb
    ) {
        sad(input.v, pred.v)
    } else {
        0
    };
    // `:6075` — luma is doubled so a plane must be MUCH worse to count.
    let y_dist = y_dist << 1;

    if let Some(found) = dominant(cb_dist > y_dist, cr_dist > y_dist) {
        out.chroma_complexity = merge_component(state.chroma_complexity, found);
    }
    if cb_dist > y_dist || cr_dist > y_dist {
        out.cfl_complexity = ComponentType::Chroma;
    }

    if use_var {
        let var_cb = block_variance_vs_flat(input.u, geom);
        let var_cr = block_variance_vs_flat(input.v, geom);
        // `:6126` — C comments that this 150 could become a parameter; it
        // is a literal today, so it is one here too.
        const TH: u32 = 150;
        if let Some(found) = dominant(var_cb > TH, var_cr > TH) {
            out.chroma_complexity = merge_component(out.chroma_complexity, found);
        }
        if var_cb > cfl_cplx_th || var_cr > cfl_cplx_th {
            out.cfl_complexity = ComponentType::Chroma;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// chroma_complexity_check (:6143-6327)
// ---------------------------------------------------------------------------

/// The INTER arm of C `chroma_complexity_check` (`:6148-6277`), 8-bit.
///
/// `input` is the source block; `reference` is the motion-compensated
/// reference block, i.e. the caller has already applied
/// `blk_org + (mv >> 3)` to the luma plane and `(blk_org + (mv >> 3)) >> 1`
/// to both chroma planes — C's `src_y_offset` / `src_cb_offset` /
/// `src_cr_offset` (`:6162-6164`).
///
/// Two C behaviours reproduced ON PURPOSE:
///
/// * **The Cr SAD reads the Cb source index** (`:6234` / `:6256` both pass
///   `loc->input_cb_origin_in_index` for the V plane). On 4:2:0 the two
///   chroma planes share a geometry so the index is the same value, which
///   is why it is invisible; the caller must still hand the Cb-derived
///   origin for `input.v`.
/// * **All three distortions are measured over the CHROMA block size**
///   (`:6192` "so SADs are comparable"), so the luma SAD covers only the
///   top-left quarter of the luma block.
#[must_use]
pub fn chroma_complexity_check_inter(
    detector_level: u8,
    geom: UvGeom,
    input: BlockPlanes<'_>,
    reference: BlockPlanes<'_>,
) -> Option<ComponentType> {
    let shift: u32 = if detector_level >= 2 {
        if geom.bheight_uv > 8 {
            2
        } else if geom.bheight_uv > 4 {
            1
        } else {
            0
        }
    } else if geom.bheight_uv > 4 {
        1
    } else {
        0
    };
    let sad = |a: Plane<'_>, b: Plane<'_>| {
        u64::from(sad_subsampled(a, b, geom.bwidth_uv, geom.bheight_uv, shift))
    };
    let y_dist = sad(input.y, reference.y);
    let cb_dist = sad(input.u, reference.u);
    let cr_dist = sad(input.v, reference.v);
    // `:6263-6268` — the luma weight is the detector level, not a constant.
    let y_dist = if detector_level >= 2 {
        y_dist << 2
    } else {
        y_dist << 1
    };
    dominant(cb_dist > y_dist, cr_dist > y_dist)
}

/// The VARIANCE arm of C `chroma_complexity_check` (`:6281-6323`), 8-bit.
///
/// Note the threshold is level-keyed here (`75` at levels 0/1, `150`
/// above, `:6315`) whereas [`chroma_complexity_check_pred`]'s is a fixed
/// 150 — the two detectors are not the same test at a different input.
#[must_use]
pub fn chroma_complexity_check_variance(
    detector_level: u8,
    geom: UvGeom,
    input: BlockPlanes<'_>,
) -> Option<ComponentType> {
    let th: u32 = if detector_level <= 1 { 75 } else { 150 };
    let var_cb = block_variance_vs_flat(input.u, geom);
    let var_cr = block_variance_vs_flat(input.v, geom);
    dominant(var_cb > th, var_cr > th)
}

/// C `chroma_complexity_check` (`:6143-6327`), 8-bit.
///
/// The composition is not "inter arm or variance arm": on an INTER block at
/// detector level <= 2 BOTH run, the inter one first, and the variance one
/// is only reached when the inter one found nothing (`:6281`).
#[must_use]
pub fn chroma_complexity_check(
    is_inter: bool,
    detector_level: u8,
    geom: UvGeom,
    input: BlockPlanes<'_>,
    reference: Option<BlockPlanes<'_>>,
) -> ComponentType {
    if is_inter {
        let reference = reference.expect("an inter block must supply its reference block");
        if let Some(found) = chroma_complexity_check_inter(detector_level, geom, input, reference) {
            return found;
        }
    }
    if !is_inter || detector_level <= 2 {
        if let Some(found) = chroma_complexity_check_variance(detector_level, geom, input) {
            return found;
        }
    }
    ComponentType::Luma
}

// ---------------------------------------------------------------------------
// lpd1_should_perform_tx (:6329-6410)
// ---------------------------------------------------------------------------

/// The neighbour state C reads off `ctx->blk_ptr->av1xd` at `:6352-6360`.
///
/// `None` means "not both neighbours available" — C's
/// `xd->left_available && xd->up_available` gate, which skips the whole
/// neighbour bonus rather than treating a missing neighbour as unfavourable.
#[derive(Debug, Clone, Copy)]
pub struct Neighbours {
    /// Both neighbours carry `skip`.
    pub both_skip: bool,
    /// Both neighbours are `NEARESTMV` or `NEAREST_NEARESTMV`.
    pub both_nearest: bool,
}

/// The RD half of [`should_perform_tx`] (`:6378-6396`).
#[derive(Debug, Clone, Copy)]
pub struct SkipRdInputs {
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]` — the 8-bit lambda, ALWAYS,
    /// even under hbd (`:6379`, unlike [`blk_skip_luma_rd`] which picks by
    /// `hbd_md`).
    pub full_lambda: u64,
    /// C `cand_bf->fast_luma_rate`.
    pub fast_luma_rate: u64,
    /// C `md_rate_est_ctx->skip_fac_bits[skip_coeff_ctx][0]` (code coeffs).
    pub skip_fac_bits_coded: u64,
    /// C `md_rate_est_ctx->skip_fac_bits[skip_coeff_ctx][1]` (skip).
    pub skip_fac_bits_skip: u64,
    /// C `cand_bf->full_dist`.
    pub full_dist: u64,
}

/// C `INIT_BIT_EST` (product_coding_loop.c:46).
pub const INIT_BIT_EST: u64 = 6000;

/// C `lpd1_should_perform_tx` (`:6329-6410`).
///
/// Accumulates "skip evidence" as an integer score and returns whether the
/// transform should still run. Intra blocks and a zero
/// `skip_tx_score_th` short-circuit to `true` (`:6331-6337`).
///
/// **The score is a signed `int` in C and stays signed here.** The QP bias
/// at `:6399-6407` subtracts up to 200 and the score can legitimately go
/// negative before the final `score < skip_tx_score_th` compare; making it
/// unsigned would wrap and invert the decision on exactly the low-QP,
/// luma-dominant inputs the bias exists for.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn should_perform_tx(
    is_inter: bool,
    ctrls: &Lpd1TxSkipDecisionCtrls,
    luma_fast_dist: u64,
    bwidth: usize,
    bheight: usize,
    qp_index: u32,
    neighbours: Option<Neighbours>,
    rd: SkipRdInputs,
    is_luma_dominant_input: bool,
) -> bool {
    if !is_inter || ctrls.skip_tx_score_th == 0 {
        return true;
    }

    let mut score: i32 = 0;
    let th_normalizer = (bheight * bwidth) as u64 * u64::from(qp_index);

    // C multiplies a `uint64_t` distortion by 100 and lets it wrap; the
    // widest real `luma_fast_dist` is a 128x128 SSE (< 2^32), so the product
    // is nowhere near the wrap. `wrapping_mul` says "C's semantics" rather
    // than permitting a value that cannot occur.
    if luma_fast_dist.wrapping_mul(100) < u64::from(ctrls.dist_energy_th) * th_normalizer {
        score += 50;
        if let Some(n) = neighbours {
            if n.both_skip {
                score += 20;
            }
            if n.both_nearest {
                score += 15;
            }
            // C tests the two flags again rather than nesting, so a block
            // with both properties scores 20 + 15 + 15, not 15.
            if n.both_skip && n.both_nearest {
                score += 15;
            }
        }
    }

    if ctrls.rd_skip_th != 0 {
        let est_skip_cost = rdcost(
            rd.full_lambda,
            rd.fast_luma_rate + rd.skip_fac_bits_skip,
            rd.full_dist << 4,
        );
        let th = rdcost(
            rd.full_lambda,
            rd.fast_luma_rate + rd.skip_fac_bits_coded + INIT_BIT_EST,
            ((bheight * bwidth) as u64) << 4,
        );
        if est_skip_cost * 100 < u64::from(ctrls.rd_skip_th) * th {
            score += 150;
        }
    }

    // `:6399-6407` — at low QP the evidence is discounted, far harder when
    // the input is luma-dominant.
    const QP_BIAS: i32 = 20;
    let luma_weight = |dominant: i32, plain: i32| {
        QP_BIAS
            * if is_luma_dominant_input {
                dominant
            } else {
                plain
            }
    };
    if qp_index < 32 {
        score -= luma_weight(10, 1);
    } else if qp_index < 64 {
        score -= luma_weight(5, 0);
    } else if qp_index < 128 {
        score -= luma_weight(3, 0);
    }

    score < ctrls.skip_tx_score_th
}

// ---------------------------------------------------------------------------
// lpd1_blk_skip_luma_rd (:6417-6441)
// ---------------------------------------------------------------------------

/// What [`blk_skip_luma_rd`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipLumaOutcome {
    /// Keep the coded residual; the caller changes nothing.
    KeepResidual,
    /// SKIP won. The caller owes C's writes at `:6427-6437`: clear
    /// `y_has_coeff`, `eob.y[0]`, `quant_dc.y[0]`, `block_has_coeff` and
    /// `cnt_nz_coeff`; set the residual distortion to the PREDICTION
    /// distortion; force luma `transform_type` to `DCT_DCT`, and on an
    /// inter candidate `transform_type_uv` too. Chroma must then be
    /// bypassed (`perform_chroma = false`).
    CommitSkip,
}

/// C `lpd1_blk_skip_luma_rd` (`:6417-6441`).
///
/// `lambda` is `full_lambda_md[EB_10_BIT_MD]` under `hbd_md` and the 8-bit
/// one otherwise (`:6419`) — note this differs from
/// [`should_perform_tx`], which uses the 8-bit lambda unconditionally.
#[must_use]
pub fn blk_skip_luma_rd(
    lambda: u64,
    y_coeff_bits: u64,
    skip_fac_bits_coded: u64,
    skip_fac_bits_skip: u64,
    dist_residual: u64,
    dist_prediction: u64,
    blk_skip_luma_rd_pct: u64,
) -> SkipLumaOutcome {
    let non_skip_cost = rdcost(lambda, y_coeff_bits + skip_fac_bits_coded, dist_residual);
    let skip_cost = rdcost(lambda, skip_fac_bits_skip, dist_prediction);
    if skip_cost * blk_skip_luma_rd_pct < non_skip_cost * 100 {
        SkipLumaOutcome::CommitSkip
    } else {
        SkipLumaOutcome::KeepResidual
    }
}

// ---------------------------------------------------------------------------
// lpd1_chroma_energy_skip (:6453-6540)
// ---------------------------------------------------------------------------

/// What [`chroma_energy_skip`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaEnergyOutcome {
    /// Both planes are below the energy threshold AND luma coded nothing:
    /// the whole block is SKIP. The caller owes `u_has_coeff = v_has_coeff
    /// = 0` and, when the candidate allows it, `skip_mode = true`, then
    /// returns immediately.
    BlockSkip,
    /// Both planes are below threshold but luma has coefficients: drop both
    /// chroma transforms (clear `u`/`v` `has_coeff`, `eob`, `quant_dc`,
    /// both distortions and both coeff-bit counts for whichever planes were
    /// in the transform set), force `transform_type_uv = DCT_DCT` on an
    /// inter candidate, and set the component to
    /// [`ComponentType::Luma`].
    DropBothChroma,
    /// Only Cb cleared the threshold: drop Cb's transform, keep Cr, and set
    /// the component to [`ComponentType::Cr`].
    DropCb,
    /// Only Cr cleared: drop Cr's transform, keep Cb, component
    /// [`ComponentType::Cb`].
    DropCr,
    /// Neither plane cleared: nothing changes.
    KeepBoth,
}

/// C `lpd1_chroma_energy_skip` (`:6453-6540`), 8-bit.
///
/// A per-plane absolute prediction-error gate: a plane whose SAD against
/// its prediction is below `energy_th * uv_area / 8` carries too little
/// signal to be worth a transform.
///
/// The function is strictly ADDITIVE towards skipping (C says so at
/// `:6481`): a plane the chroma-complexity detector already dropped is
/// treated as having passed, so this can never re-introduce one. That is
/// why `component` is an input as well as an output.
///
/// The SAD walks every OTHER row (`u_stride << 1`, `bheight_uv >> 1`,
/// `:6465-6469`) — a fixed halving, not the level-keyed shift the
/// complexity detectors use.
#[must_use]
pub fn chroma_energy_skip(
    component: ComponentType,
    geom: UvGeom,
    input: BlockPlanes<'_>,
    pred: BlockPlanes<'_>,
    chroma_skip_energy_th: u32,
    luma_has_coeff: bool,
) -> ChromaEnergyOutcome {
    let cb_in_tx = matches!(component, ComponentType::Chroma | ComponentType::Cb);
    let cr_in_tx = matches!(component, ComponentType::Chroma | ComponentType::Cr);
    let blk_area_uv = (geom.bwidth_uv * geom.bheight_uv) as u32;
    // C computes this in `uint32_t` and then shifts; the product is the
    // widest term (th up to a few hundred times an area up to 4096), well
    // inside 32 bits for every shipped threshold.
    let th_total = chroma_skip_energy_th.wrapping_mul(blk_area_uv) >> 3;

    let sad = |a: Plane<'_>, b: Plane<'_>| sad_subsampled(a, b, geom.bwidth_uv, geom.bheight_uv, 1);
    // A plane not in the transform set counts as passed (`:6481-6482`).
    let cb_pass = !cb_in_tx || sad(input.u, pred.u) < th_total;
    let cr_pass = !cr_in_tx || sad(input.v, pred.v) < th_total;

    match (cb_pass, cr_pass) {
        (true, true) if !luma_has_coeff => ChromaEnergyOutcome::BlockSkip,
        (true, true) => ChromaEnergyOutcome::DropBothChroma,
        // C's `else if` chain re-tests `*_in_tx`, so a plane that "passed"
        // only because it was already dropped does not trigger a drop arm.
        (true, false) if cb_in_tx => ChromaEnergyOutcome::DropCb,
        (false, true) if cr_in_tx => ChromaEnergyOutcome::DropCr,
        _ => ChromaEnergyOutcome::KeepBoth,
    }
}

// ---------------------------------------------------------------------------
// lpd1_try_mds0_bypass, predicate half (:8939-8965)
// ---------------------------------------------------------------------------

/// C `lpd1_try_mds0_bypass`'s five early returns (`:8942-8965`).
///
/// The rest of that function is candidate INJECTION — it writes a synthetic
/// GLOBALMV candidate, runs the light-PD1 predictor over it and prices it —
/// which is pipeline plumbing this port structures differently. Only the
/// decision is translated; the caller performs the injection.
///
/// `pd0_mds0_best_cost` is the PD0 residual variance for this block, whose
/// `u32::MAX` sentinel (PD0 skipped) fails the comparison by construction —
/// which is C's intent (`:8961`) and survives here because the comparison
/// is done in `u64`.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn globalmv_bypass_allowed(
    is_i_slice: bool,
    globalmv_bypass_th: u32,
    shape_is_square: bool,
    use_ref_frame_mvs: bool,
    l0_gm_is_identity: bool,
    me_mv_is_zero: bool,
    pd0_mds0_best_cost: u32,
    bwidth: usize,
    bheight: usize,
) -> bool {
    if is_i_slice || globalmv_bypass_th == 0 || !shape_is_square || use_ref_frame_mvs {
        return false;
    }
    if !l0_gm_is_identity || !me_mv_is_zero {
        return false;
    }
    let blk_area = (bwidth * bheight) as u64;
    u64::from(pd0_mds0_best_cost) < u64::from(globalmv_bypass_th) * blk_area
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrls(score_th: i32, dist_th: u16, rd_th: u16) -> Lpd1TxSkipDecisionCtrls {
        Lpd1TxSkipDecisionCtrls {
            skip_tx_score_th: score_th,
            dist_energy_th: dist_th,
            rd_skip_th: rd_th,
        }
    }

    fn flat_rd() -> SkipRdInputs {
        SkipRdInputs {
            full_lambda: 0,
            fast_luma_rate: 0,
            skip_fac_bits_coded: 0,
            skip_fac_bits_skip: 0,
            full_dist: 0,
        }
    }

    /// Tier 4: traced against `product_coding_loop.c:6331-6337`.
    #[test]
    fn intra_and_disabled_thresholds_always_transform() {
        let c = ctrls(125, 30, 100);
        assert!(should_perform_tx(
            false,
            &c,
            0,
            16,
            16,
            100,
            None,
            flat_rd(),
            false
        ));
        let off = ctrls(0, 30, 100);
        assert!(should_perform_tx(
            true,
            &off,
            0,
            16,
            16,
            100,
            None,
            flat_rd(),
            false
        ));
    }

    /// The neighbour bonuses are additive, and both-flags scores 50.
    #[test]
    fn neighbour_bonuses_stack_to_fifty() {
        // dist gate passes (0 < anything), rd gate off, qp >= 128 so no bias.
        let c = ctrls(1000, 30, 0);
        let score_for = |n: Option<Neighbours>| {
            // Recover the score by sweeping the threshold: the function
            // returns `score < th`. The sweep starts at 1 because a
            // threshold of 0 short-circuits to `true` (`:6335`).
            (1..=1000i32)
                .find(|&th| {
                    should_perform_tx(true, &ctrls(th, 30, 0), 0, 16, 16, 200, n, flat_rd(), false)
                })
                .unwrap()
        };
        let _ = c;
        assert_eq!(score_for(None), 51, "50 + 1 for the strict `<`");
        assert_eq!(
            score_for(Some(Neighbours {
                both_skip: true,
                both_nearest: false
            })),
            71
        );
        assert_eq!(
            score_for(Some(Neighbours {
                both_skip: false,
                both_nearest: true
            })),
            66
        );
        assert_eq!(
            score_for(Some(Neighbours {
                both_skip: true,
                both_nearest: true
            })),
            101,
            "50 + 20 + 15 + 15"
        );
    }

    /// The QP bias can drive the score NEGATIVE — the case an unsigned
    /// score would wrap on, inverting the decision.
    #[test]
    fn low_qp_luma_dominant_bias_can_go_negative() {
        // dist_energy_th 0 makes the dist gate `x < 0`, always false, so
        // the score enters the bias at 0. rd gate off. score -> -200.
        let c = ctrls(-100, 0, 0);
        // score = -200 < -100 -> skip the transform.
        assert!(should_perform_tx(
            true,
            &c,
            1000,
            16,
            16,
            10,
            None,
            flat_rd(),
            true
        ));
        // With the same threshold but a non-luma-dominant input the bias is
        // only -20, which is NOT below -100.
        assert!(!should_perform_tx(
            true,
            &c,
            1000,
            16,
            16,
            10,
            None,
            flat_rd(),
            false
        ));
    }

    #[test]
    fn skip_luma_rd_prefers_skip_when_the_margin_holds() {
        // non_skip dist 1000, skip dist 1200, no rate. pct 100 -> compare
        // 1200*128*100 vs 1000*128*100: skip loses.
        assert_eq!(
            blk_skip_luma_rd(0, 0, 0, 0, 1000, 1200, 100),
            SkipLumaOutcome::KeepResidual
        );
        // pct 50 halves the skip cost -> 1200*128*50 < 1000*128*100.
        assert_eq!(
            blk_skip_luma_rd(0, 0, 0, 0, 1000, 1200, 50),
            SkipLumaOutcome::CommitSkip
        );
    }

    fn planes<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8], stride: usize) -> BlockPlanes<'a> {
        BlockPlanes {
            y: Plane::new(y, stride),
            u: Plane::new(u, stride),
            v: Plane::new(v, stride),
        }
    }

    #[test]
    fn chroma_energy_skip_never_reintroduces_a_dropped_plane() {
        let geom = UvGeom {
            bwidth_uv: 4,
            bheight_uv: 4,
            bsize_uv: 0,
        };
        let zeros = [0u8; 64];
        let ones = [255u8; 64];
        // Cr is already dropped (component == Cb): even though the V planes
        // differ wildly, only Cb is measured — and it matches, so it passes.
        let input = planes(&zeros, &zeros, &zeros, 4);
        let pred = planes(&zeros, &zeros, &ones, 4);
        assert_eq!(
            chroma_energy_skip(ComponentType::Cb, geom, input, pred, 8, true),
            ChromaEnergyOutcome::DropBothChroma
        );
        // With both planes in the set the V mismatch keeps Cr alive.
        assert_eq!(
            chroma_energy_skip(ComponentType::Chroma, geom, input, pred, 8, true),
            ChromaEnergyOutcome::DropCb
        );
    }

    #[test]
    fn chroma_energy_skip_commits_a_block_skip_only_without_luma_coeffs() {
        let geom = UvGeom {
            bwidth_uv: 4,
            bheight_uv: 4,
            bsize_uv: 0,
        };
        let zeros = [0u8; 64];
        let p = planes(&zeros, &zeros, &zeros, 4);
        assert_eq!(
            chroma_energy_skip(ComponentType::Chroma, geom, p, p, 8, false),
            ChromaEnergyOutcome::BlockSkip
        );
        assert_eq!(
            chroma_energy_skip(ComponentType::Chroma, geom, p, p, 8, true),
            ChromaEnergyOutcome::DropBothChroma
        );
    }

    #[test]
    fn merge_promotes_but_never_demotes() {
        assert_eq!(
            merge_component(ComponentType::Cr, ComponentType::Cb),
            ComponentType::Chroma
        );
        assert_eq!(
            merge_component(ComponentType::Cb, ComponentType::Cb),
            ComponentType::Cb
        );
        assert_eq!(
            merge_component(ComponentType::Luma, ComponentType::Chroma),
            ComponentType::Chroma
        );
    }

    #[test]
    fn globalmv_bypass_needs_every_precondition() {
        let ok = |th, i_slice, sq, refmvs, gm, mv, cost| {
            globalmv_bypass_allowed(i_slice, th, sq, refmvs, gm, mv, cost, 16, 16)
        };
        assert!(ok(10, false, true, false, true, true, 100));
        assert!(!ok(10, true, true, false, true, true, 100), "I slice");
        assert!(!ok(0, false, true, false, true, true, 100), "th 0");
        assert!(!ok(10, false, false, false, true, true, 100), "non-square");
        assert!(!ok(10, false, true, true, true, true, 100), "ref frame mvs");
        assert!(!ok(10, false, true, false, false, true, 100), "warped gm");
        assert!(
            !ok(10, false, true, false, true, false, 100),
            "nonzero me mv"
        );
        // 10 * 256 = 2560; the sentinel and anything at/above the bound fail.
        assert!(!ok(10, false, true, false, true, true, 2560));
        assert!(ok(10, false, true, false, true, true, 2559));
        assert!(!ok(10, false, true, false, true, true, u32::MAX));
    }

    #[test]
    fn variance_detector_threshold_is_level_keyed() {
        let geom = UvGeom {
            bwidth_uv: 8,
            bheight_uv: 8,
            bsize_uv: 0, // 4x4 log2 pels = 4; deliberately small so the
                         // normalisation leaves a large per-pixel value
        };
        // A checkerboard of 0 / 255 against the flat-128 reference has a
        // large variance; a constant-128 block has none.
        let mut checker = [0u8; 64];
        for (i, p) in checker.iter_mut().enumerate() {
            *p = if i % 2 == 0 { 0 } else { 255 };
        }
        let flat = [128u8; 64];
        let busy = planes(&flat, &checker, &checker, 8);
        let calm = planes(&flat, &flat, &flat, 8);
        assert_eq!(
            chroma_complexity_check_variance(0, geom, busy),
            Some(ComponentType::Chroma)
        );
        assert_eq!(chroma_complexity_check_variance(0, geom, calm), None);
        // Mixed: Cb busy, Cr calm.
        let mixed = BlockPlanes {
            y: Plane::new(&flat, 8),
            u: Plane::new(&checker, 8),
            v: Plane::new(&flat, 8),
        };
        assert_eq!(
            chroma_complexity_check_variance(0, geom, mixed),
            Some(ComponentType::Cb)
        );
    }
}

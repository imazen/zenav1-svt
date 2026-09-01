//! The MDS3 cost — `svt_aom_full_cost` (rd_cost.c:1349), its PD0 twin
//! (:1330), the coefficient-rate dispatcher `svt_aom_txb_estimate_coeff_bits`
//! (:1233) and the per-block context generation (:1475).
//!
//! # Why `full_cost` returns a decision rather than mutating a buffer
//!
//! C's `svt_aom_full_cost` is not a pure cost function: it can DECIDE that a
//! block is better coded as skip, and then rewrites the candidate buffer —
//! zeroing `y_has_coeff` / `u_has_coeff` / `v_has_coeff` / `cnt_nz_coeff`,
//! resetting `tx_depth` to 0, forcing every `transform_type` to `DCT_DCT`,
//! and clearing the eob and quant-DC arrays — before returning the cost
//! through a pointer. Two separate arms do this: the `blk_skip_decision`
//! arm (inter only) and the `skip_mode` arm.
//!
//! This port returns [`FullCostResult`], which carries the cost, the total
//! rate, the distortion actually used, and the two flags that say which
//! rewrite the caller must apply. The arithmetic is C's; what changes is that
//! the mutation is the CALLER's, so a cost query cannot silently invalidate a
//! candidate it was only meant to price.

use crate::entropy::context::SKIP_CONTEXTS;
use crate::entropy::context::SKIP_MODE_CONTEXTS;
use crate::port_rd_cost::rdcost;

/// C `svt_aom_full_cost`'s two skip tables (md_rate_estimation.h:58/120).
#[derive(Debug, Clone, Copy, Default)]
pub struct SkipFacBits {
    /// C `skip_fac_bits[SKIP_CONTEXTS][2]` — the coefficient-skip flag.
    pub skip: [[i32; 2]; SKIP_CONTEXTS],
    /// C `skip_mode_fac_bits[SKIP_CONTEXTS][2]` — the skip-MODE flag.
    pub skip_mode: [[i32; 2]; SKIP_MODE_CONTEXTS],
}

/// C `y_distortion[DIST_TOTAL][DIST_CALC_TOTAL]` for one plane: the SSD and
/// SSIM distortions, each in a `[non-skip, skip]` pair.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaneDist {
    /// C `[DIST_SSD][0]` — distortion if the residual IS coded.
    pub ssd_nonskip: u64,
    /// C `[DIST_SSD][1]` — distortion if the block is coded as skip.
    pub ssd_skip: u64,
    /// C `[DIST_SSIM][0]`.
    pub ssim_nonskip: u64,
    /// C `[DIST_SSIM][1]`.
    pub ssim_skip: u64,
}

/// The three planes' distortions.
#[derive(Debug, Clone, Copy, Default)]
pub struct FullDist {
    pub y: PlaneDist,
    pub cb: PlaneDist,
    pub cr: PlaneDist,
}

impl FullDist {
    fn ssd_nonskip(&self) -> u64 {
        self.y.ssd_nonskip + self.cb.ssd_nonskip + self.cr.ssd_nonskip
    }
    fn ssd_skip(&self) -> u64 {
        self.y.ssd_skip + self.cb.ssd_skip + self.cr.ssd_skip
    }
    fn ssim_nonskip(&self) -> u64 {
        self.y.ssim_nonskip + self.cb.ssim_nonskip + self.cr.ssim_nonskip
    }
    fn ssim_skip(&self) -> u64 {
        self.y.ssim_skip + self.cb.ssim_skip + self.cr.ssim_skip
    }
}

/// The `ModeDecisionContext` / candidate fields `svt_aom_full_cost` reads.
#[derive(Debug, Clone, Copy)]
pub struct FullCostInputs {
    /// C `ctx->skip_coeff_ctx`.
    pub skip_coeff_ctx: usize,
    /// C `ctx->skip_mode_ctx`.
    pub skip_mode_ctx: usize,
    /// C `ctx->tune_ssim_level > SSIM_LVL_0`.
    pub update_full_cost_ssim: bool,
    /// C `ctx->shut_fast_rate`.
    pub shut_fast_rate: bool,
    /// C `pcs->ppcs->frm_hdr.tx_mode == TX_MODE_SELECT`.
    pub tx_mode_select: bool,
    /// C `svt_av1_is_lossless_segment(pcs, blk_ptr->segment_id)`.
    pub lossless_segment: bool,
    /// C `ctx->blk_skip_decision`.
    pub blk_skip_decision: bool,
    /// C `cand_bf->block_has_coeff`.
    pub block_has_coeff: bool,
    /// C `is_inter_mode(cand_bf->cand->block_mi.mode)`.
    pub is_inter_mode: bool,
    /// C `cand_bf->cand->skip_mode_allowed`.
    pub skip_mode_allowed: bool,
    /// C `svt_aom_get_tx_size_bits(cand_bf, ctx, pcs, tx_depth, 1)` — the
    /// tx-size rate WITH coefficients. Only consulted when
    /// `block_has_coeff`; the caller computes it with
    /// [`crate::vartx::tx_size_bits_vartx`] (inter) or the
    /// `tx_size_fac_bits[cat][ctx][depth]` lookup (intra).
    pub non_skip_tx_size_bits: u64,
    /// C `svt_aom_get_tx_size_bits(..., 0)` — the tx-size rate when the
    /// block is signalled skip. C asserts this is 0 for every inter mode.
    pub skip_tx_size_bits: u64,
    /// C `cand_bf->fast_luma_rate` + `fast_chroma_rate`, from MDS0.
    pub fast_rate: u64,
}

/// What C did to the candidate buffer, returned instead of done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullCostResult {
    /// C `*(cand_bf->full_cost)`.
    pub cost: u64,
    /// C `cand_bf->total_rate`.
    pub total_rate: u64,
    /// C `cand_bf->full_dist` — a `uint32_t` in C, so the value is
    /// TRUNCATED on the way in. The port keeps the u64 here and reports the
    /// truncation separately in [`FullCostResult::full_dist_u32`].
    pub full_dist: u64,
    /// C `*(cand_bf->full_cost_ssim)`, `None` when `tune_ssim` is off.
    pub full_cost_ssim: Option<u64>,
    /// The `blk_skip_decision` arm fired: the caller must move the skip
    /// distortions into the non-skip slots and clear the coefficient state.
    pub forced_coeff_skip: bool,
    /// C `cand_bf->cand->block_mi.skip_mode` — the skip-MODE arm fired.
    pub skip_mode: bool,
}

impl FullCostResult {
    /// C's `cand_bf->full_dist = (uint32_t)mode_distortion`.
    #[inline]
    pub fn full_dist_u32(&self) -> u32 {
        self.full_dist as u32
    }
}

/// C `svt_aom_full_cost_pd0` (rd_cost.c:1330, EXPORTED).
///
/// PD0 uses `partition_fac_bits[0][PARTITION_NONE]` as a stand-in for the
/// real partition rate — C's own comment says so — and `skip_fac_bits[0][0]`
/// rather than the block's real skip context. Both approximations are
/// deliberate; reproducing them is what byte-parity requires.
pub fn full_cost_pd0(
    y_coeff_bits: u64,
    y_distortion_nonskip: u64,
    partition_none_fac_bits_ctx0: i32,
    skip_fac_bits_ctx0_nonskip: i32,
    lambda: u64,
) -> u64 {
    let coeff_rate = y_coeff_bits + skip_fac_bits_ctx0_nonskip as u64;
    rdcost(
        lambda,
        coeff_rate + partition_none_fac_bits_ctx0 as u64,
        y_distortion_nonskip,
    )
}

/// C `svt_aom_full_cost` (rd_cost.c:1349, EXPORTED).
///
/// Order matters and is C's: the block-skip decision runs FIRST (and can
/// rewrite the distortions it then uses), the coefficient rate is assembled
/// from whichever branch survived, and only then is the skip-MODE cost
/// compared. The skip-mode comparison is `<=`, not `<` — a tie goes to skip
/// mode.
pub fn full_cost(
    inputs: &FullCostInputs,
    dist: &FullDist,
    y_coeff_bits: u64,
    cb_coeff_bits: u64,
    cr_coeff_bits: u64,
    lambda: u64,
    t: &SkipFacBits,
) -> FullCostResult {
    // C computes both tx-size rates up front, gated on `!shut_fast_rate &&
    // tx_mode == TX_MODE_SELECT`; the non-skip one only when the block has
    // coefficients.
    let (non_skip_tx_size_bits, skip_tx_size_bits) =
        if !inputs.shut_fast_rate && inputs.tx_mode_select {
            (
                if inputs.block_has_coeff {
                    inputs.non_skip_tx_size_bits
                } else {
                    0
                },
                inputs.skip_tx_size_bits,
            )
        } else {
            (0, 0)
        };
    debug_assert!(!inputs.is_inter_mode || skip_tx_size_bits == 0);

    let skip_ctx = inputs.skip_coeff_ctx;
    let mut block_has_coeff = inputs.block_has_coeff;
    let mut forced_coeff_skip = false;
    let mut ssd_nonskip = dist.ssd_nonskip();
    let mut ssim_nonskip = dist.ssim_nonskip();

    // Arm 1 — `blk_skip_decision`: inter blocks only, and never on a lossless
    // segment (where dropping the residual is not free).
    if !inputs.lossless_segment
        && inputs.blk_skip_decision
        && block_has_coeff
        && inputs.is_inter_mode
    {
        let non_skip_cost = rdcost(
            lambda,
            y_coeff_bits
                + cb_coeff_bits
                + cr_coeff_bits
                + non_skip_tx_size_bits
                + t.skip[skip_ctx][0] as u64,
            ssd_nonskip,
        );
        let skip_cost = rdcost(
            lambda,
            t.skip[skip_ctx][1] as u64 + skip_tx_size_bits,
            dist.ssd_skip(),
        );
        if skip_cost < non_skip_cost {
            ssd_nonskip = dist.ssd_skip();
            ssim_nonskip = dist.ssim_skip();
            block_has_coeff = false;
            forced_coeff_skip = true;
        }
    }

    let coeff_rate = if block_has_coeff {
        y_coeff_bits
            + cb_coeff_bits
            + cr_coeff_bits
            + non_skip_tx_size_bits
            + t.skip[skip_ctx][0] as u64
    } else {
        t.skip[skip_ctx][1] as u64 + skip_tx_size_bits
    };

    let mut mode_rate = inputs.fast_rate + coeff_rate;
    let mut mode_distortion = ssd_nonskip;
    let mut mode_ssim_distortion = if inputs.update_full_cost_ssim {
        ssim_nonskip
    } else {
        0
    };
    let mut mode_cost = rdcost(lambda, mode_rate, mode_distortion);
    let mut skip_mode = false;

    // Arm 2 — skip MODE. Note this uses the ORIGINAL skip distortions, not
    // the ones arm 1 may have promoted.
    if inputs.skip_mode_allowed {
        let skip_mode_rate = t.skip_mode[inputs.skip_mode_ctx][1] as u64;
        let skip_mode_distortion = dist.ssd_skip();
        let skip_mode_ssim_distortion = if inputs.update_full_cost_ssim {
            dist.ssim_skip()
        } else {
            0
        };
        let skip_mode_cost = rdcost(lambda, skip_mode_rate, skip_mode_distortion);
        if skip_mode_cost <= mode_cost {
            mode_cost = skip_mode_cost;
            mode_rate = skip_mode_rate;
            mode_distortion = skip_mode_distortion;
            mode_ssim_distortion = skip_mode_ssim_distortion;
            skip_mode = true;
        }
    }

    FullCostResult {
        cost: mode_cost,
        total_rate: mode_rate,
        full_dist: mode_distortion,
        full_cost_ssim: if inputs.update_full_cost_ssim {
            Some(rdcost(lambda, mode_rate, mode_ssim_distortion))
        } else {
            None
        },
        forced_coeff_skip,
        skip_mode,
    }
}

// ---------------------------------------------------------------------------
// svt_aom_txb_estimate_coeff_bits (rd_cost.c:1233 / :1206)
// ---------------------------------------------------------------------------

/// C `COMPONENT_TYPE` (definitions.h) — which planes a coefficient-rate call
/// covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentType {
    /// C `COMPONENT_LUMA`.
    Luma,
    /// C `COMPONENT_CHROMA_CB`.
    ChromaCb,
    /// C `COMPONENT_CHROMA_CR`.
    ChromaCr,
    /// C `COMPONENT_CHROMA` — both chroma planes.
    Chroma,
    /// C `COMPONENT_ALL`.
    All,
}

impl ComponentType {
    #[inline]
    fn covers_luma(self) -> bool {
        matches!(self, ComponentType::Luma | ComponentType::All)
    }
    #[inline]
    fn covers_cb(self) -> bool {
        matches!(
            self,
            ComponentType::ChromaCb | ComponentType::Chroma | ComponentType::All
        )
    }
    #[inline]
    fn covers_cr(self) -> bool {
        matches!(
            self,
            ComponentType::ChromaCr | ComponentType::Chroma | ComponentType::All
        )
    }
}

/// One plane's coefficient-rate inputs.
pub struct TxbPlane<'a> {
    /// The quantized coefficients of this txb.
    pub qcoeff: &'a [i32],
    /// C's `y_eob` / `cb_eob` / `cr_eob`.
    pub eob: u16,
    /// C `txsize` (luma) or `txsize_uv` (chroma), as this port's tx index.
    pub tx_size: usize,
    /// C `tx_type` / `tx_type_uv`.
    pub tx_type: usize,
    /// C `ctx->{luma,cb,cr}_txb_skip_context`.
    pub txb_skip_ctx: usize,
    /// C `ctx->{luma,cb,cr}_dc_sign_context`.
    pub dc_sign_ctx: usize,
}

/// The three per-plane coefficient rates C writes through its out-pointers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TxbCoeffBits {
    pub y: u64,
    pub cb: u64,
    pub cr: u64,
}

/// C `svt_aom_txb_estimate_coeff_bits` (rd_cost.c:1233, EXPORTED).
///
/// A dispatcher: per plane, `svt_av1_cost_coeffs_txb` when the eob is
/// non-zero and `av1_cost_skip_txb` when it is. Two details are easy to lose:
///
/// * only the LUMA rate is shifted left by `mds_subres_step` — the sub-
///   resolution search halves the rows it actually transformed, and C scales
///   the luma rate back up to a full-resolution estimate. Chroma is not
///   scaled;
/// * a plane the `component_type` does not cover is left UNTOUCHED, not
///   zeroed. C writes through out-pointers and simply skips them, so a caller
///   that reuses a struct keeps the previous value. This port returns the
///   partial struct and makes the caller merge, which is the same contract
///   made visible.
#[allow(clippy::too_many_arguments)]
pub fn txb_estimate_coeff_bits(
    component_type: ComponentType,
    luma: Option<&TxbPlane<'_>>,
    cb: Option<&TxbPlane<'_>>,
    cr: Option<&TxbPlane<'_>>,
    intra_dir: usize,
    mds_subres_step: u32,
    rates: &crate::leaf_funnel::MdRates,
) -> TxbCoeffBits {
    let mut out = TxbCoeffBits::default();
    if let Some(p) = luma.filter(|_| component_type.covers_luma()) {
        out.y = plane_rate(p, 0, intra_dir, rates) << mds_subres_step;
    }
    if let Some(p) = cb.filter(|_| component_type.covers_cb()) {
        out.cb = plane_rate(p, 1, intra_dir, rates);
    }
    if let Some(p) = cr.filter(|_| component_type.covers_cr()) {
        out.cr = plane_rate(p, 1, intra_dir, rates);
    }
    out
}

/// C `svt_aom_txb_estimate_coeff_bits_pd0` (rd_cost.c:1206, EXPORTED).
///
/// The PD0 arm hardwires `PLANE_TYPE_Y`, `DCT_DCT` and ZERO contexts — C
/// passes literal `0`s for both the txb-skip and dc-sign contexts, which is
/// an approximation PD0 accepts, not an oversight.
pub fn txb_estimate_coeff_bits_pd0(
    qcoeff: &[i32],
    y_eob: u16,
    tx_size: usize,
    mds_subres_step: u32,
    rates: &crate::leaf_funnel::MdRates,
) -> u64 {
    let plane = TxbPlane {
        qcoeff,
        eob: y_eob,
        tx_size,
        // C passes DCT_DCT, whose index is 0.
        tx_type: 0,
        txb_skip_ctx: 0,
        dc_sign_ctx: 0,
    };
    if y_eob != 0 {
        plane_rate(&plane, 0, 0, rates) << mds_subres_step
    } else {
        // C's else arm does NOT shift.
        plane_rate(&plane, 0, 0, rates)
    }
}

fn plane_rate(
    p: &TxbPlane<'_>,
    plane_type: usize,
    intra_dir: usize,
    rates: &crate::leaf_funnel::MdRates,
) -> u64 {
    if p.eob != 0 {
        crate::leaf_funnel::cost_coeffs_txb(
            p.qcoeff,
            p.eob,
            p.tx_size,
            p.tx_type,
            plane_type,
            p.txb_skip_ctx,
            p.dc_sign_ctx,
            intra_dir,
            rates,
        ) as u64
    } else {
        crate::leaf_funnel::cost_skip_txb(p.tx_size, plane_type, p.txb_skip_ctx, rates) as u64
    }
}

// ---------------------------------------------------------------------------
// svt_aom_coding_loop_context_generation (rd_cost.c:1475)
// ---------------------------------------------------------------------------

/// C `block_signals_txsize` (rd_cost.c:1496): `bsize > BLOCK_4X4`.
#[inline]
pub fn block_signals_txsize(bsize: svtav1_types::block::BlockSize) -> bool {
    bsize.as_index() > svtav1_types::block::BlockSize::Block4x4.as_index()
}

/// The per-block contexts `svt_aom_coding_loop_context_generation` derives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockContexts {
    /// C `ctx->intra_luma_top_ctx` / `intra_luma_left_ctx` — key frames only;
    /// left at their previous value otherwise, which C never reads because
    /// `svt_aom_intra_fast_cost` gates them on `slice_type == I_SLICE`.
    pub intra_luma_top_ctx: usize,
    pub intra_luma_left_ctx: usize,
    /// C `ctx->is_inter_ctx`.
    pub is_inter_ctx: usize,
    /// C `ctx->skip_mode_ctx`.
    pub skip_mode_ctx: usize,
    /// C `ctx->skip_coeff_ctx`.
    pub skip_coeff_ctx: usize,
    /// Whether C called `svt_aom_collect_neighbors_ref_counts_new` — the
    /// reference counts are only refreshed when MD will consume them.
    pub collect_ref_counts: bool,
}

/// C `svt_aom_coding_loop_context_generation` (rd_cost.c:1475, EXPORTED).
///
/// Three gates, each of which suppresses work MD will not read:
///
/// * `shut_fast_rate` skips the mode contexts entirely;
/// * the reference counts are collected only on a non-key slice (or a key
///   slice with IntraBC) AND at `approx_inter_rate < 2` — the entropy coder
///   has its own call, so skipping here is not a bitstream change;
/// * `skip_coeff_ctx` is 0 unless `rate_est_ctrls.update_skip_coeff_ctx`.
#[allow(clippy::too_many_arguments)]
pub fn coding_loop_context_generation(
    is_key_slice: bool,
    allow_intrabc: bool,
    shut_fast_rate: bool,
    approx_inter_rate: u8,
    update_skip_coeff_ctx: bool,
    kf_y_mode_ctx: impl FnOnce() -> (usize, usize),
    intra_inter_ctx: impl FnOnce() -> usize,
    skip_mode_ctx: impl FnOnce() -> usize,
    skip_ctx: impl FnOnce() -> usize,
) -> BlockContexts {
    let mut out = BlockContexts::default();
    if !shut_fast_rate {
        if is_key_slice {
            let (top, left) = kf_y_mode_ctx();
            out.intra_luma_top_ctx = top;
            out.intra_luma_left_ctx = left;
        }
        out.is_inter_ctx = intra_inter_ctx();
        out.skip_mode_ctx = skip_mode_ctx();
    }
    out.collect_ref_counts = (!is_key_slice || allow_intrabc) && approx_inter_rate < 2;
    out.skip_coeff_ctx = if update_skip_coeff_ctx { skip_ctx() } else { 0 };
    out
}

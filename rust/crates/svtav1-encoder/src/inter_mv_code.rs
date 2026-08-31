//! Inter-frame motion-vector ENTROPY CODING and MV RATE — campaign chunk C3
//! (`docs/INTER-ENCODE-PLAN.md`).
//!
//! C reference: `Source/Lib/Codec/{entropy_coding,md_rate_estimation,rd_cost,
//! enc_mode_config,enc_dec_process,cabac_context_model,definitions,
//! inter_prediction,mode_decision}.{c,h}`.
//!
//! # What this module is for
//!
//! `entropy/mv_coding.rs` already carries the *symbol* writer
//! ([`crate::entropy::mv_coding::encode_mv_diff`] /`encode_mv_component`) and
//! `intrabc.rs` already carries the *cost-table build chain*
//! ([`crate::intrabc::build_nmv_cost_table`]) and the per-MV cost formulas
//! ([`crate::intrabc::mv_bit_cost`], [`crate::intrabc::mv_err_cost`]) — both
//! C-parity gated. What was MISSING, and is what this module adds, is
//! everything BETWEEN those two layers on the inter path:
//!
//! 1. The **precision derivation** `svt_av1_encode_mv` performs internally
//!    (`force_integer_mv` overrides `usehp` to `MV_SUBPEL_NONE`,
//!    entropy_coding.c:1498-1500). The existing `write_mv` adapter takes only
//!    `allow_hp: bool` and so cannot express the force-integer frame.
//! 2. The **per-inter-mode dispatch** that decides WHICH of a block's MVs are
//!    coded at all (entropy_coding.c:5216-5244) and, symmetrically, which are
//!    priced (rd_cost.c:1088-1128). NEWMV/NEW_NEWMV code `1 + is_compound`
//!    MVs; NEAREST_NEWMV/NEAR_NEWMV code only ref 1; NEW_NEARESTMV/NEW_NEARMV
//!    code only ref 0; every other inter mode codes none.
//! 3. **`svt_aom_estimate_mv_rate`** itself (md_rate_estimation.c:458-488) —
//!    the `approx_inter_rate` zero-fill early return, the hp/non-hp stack
//!    selection, and the `allow_intrabc` dv arm. `intrabc.rs` ported only the
//!    dv arm ([`crate::intrabc::build_dv_cost_tables`]).
//! 4. The **CDF adaptation** that goes with writing: `av1_update_mv_stats` /
//!    `update_mv_component_stats` (md_rate_estimation.c:650-705),
//!    `reset_nmv_counter` (cabac_context_model.c:1956) and `avg_nmv`
//!    (enc_dec_process.c:2567) — plus the `update_mv` cadence
//!    (`set_cdf_controls`, enc_mode_config.c:8468-8498) that says WHEN the MD
//!    rate tables are rebuilt from the adapting per-SB context.
//!
//! # The two adaptation paths, and why they are not the same thing
//!
//! There are two distinct "CDF update" mechanisms on the MV path and mixing
//! them up is exactly the class of bug the op-trace differ exists to catch:
//!
//! * **The writer's own adaptation.** `aom_write_symbol` updates the CDF it
//!   just coded through, in place, whenever `AomWriter::allow_update_cdf` is
//!   set — and that flag is frame-global (`ec_process.c:100`:
//!   `!large_scale_tile && !disable_cdf_update`), NOT per-symbol-class. So on
//!   any ordinary frame EVERY MV symbol adapts `fc->nmvc` as it is written,
//!   in the exact order joint → (vertical: sign, class, int bits, fr, hp) →
//!   (horizontal: same). This is what a decoder mirrors, so it is normative.
//! * **MD's shadow adaptation.** `svt_aom_update_stats` replays the same
//!   updates into the per-SB `pcs->ec_ctx_array[sb]` so mode decision can
//!   price later blocks against an evolving context. That one IS gated —
//!   `cdf_ctrl.update_mv` ([`cdf_update_mv`]) — and when it is off, the MD
//!   tables are copied once per frame instead of rebuilt per SB
//!   (`copy_mv_rate`, enc_dec_process.c:36-56). It is an RD-accuracy knob; it
//!   never changes what the writer emits for a given decision.
//!
//! [`update_mv_stats`] is the second one. It performs byte-identical updates
//! to the first (both are `update_cdf` over the same CDFs in the same order),
//! which is what lets `tests/c_parity_mv_code.rs` gate it against the CDF
//! state the REAL exported `svt_av1_encode_mv` leaves behind.
//!
//! # A C asymmetry worth not "fixing"
//!
//! Under `force_integer_mv` the WRITER emits at `MV_SUBPEL_NONE` (no
//! fractional, no hp symbols) but the RATE tables are still built at
//! `MV_SUBPEL_LOW_PRECISION`: `svt_aom_estimate_mv_rate` passes
//! `allow_high_precision_mv` (a 0/1 `uint8_t`) straight in as the precision
//! and never consults `force_integer_mv` (md_rate_estimation.c:474-478). So
//! MD prices fractional bits that will not be written. That is upstream
//! behaviour and byte-identity requires reproducing it — [`estimate_mv_rate`]
//! deliberately takes no `force_integer_mv` argument for this reason.
//!
//! # Reachability
//!
//! Nothing here is called yet: the public entry point still refuses inter
//! frames (`pipeline.rs`, the `if !is_key` guard) and wiring belongs to the
//! chunks that own `pipeline.rs` / `entropy/obu.rs`. Per
//! `docs/WORKING-ON-THIS.md` §7 a faithful translation with no caller stays
//! translated; the reachability note is here rather than a `#[allow(dead_code)]`.

use crate::entropy::mv_coding::{
    CLASS0_BITS, CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_JOINTS, MvSubpelPrecision, NmvComponent,
    NmvContext, encode_mv_diff, get_mv_class,
};
use crate::entropy::writer::AomWriter;
use crate::intrabc::{MvCostTables, build_nmv_cost_table, mv_table_cost};
use svtav1_types::motion::Mv;
use svtav1_types::prediction::PredictionMode;

// =============================================================================
// §1. Inter-mode predicates — which MVs a block codes
// =============================================================================

/// C `svt_aom_have_newmv_in_inter_mode` (mode_decision.c:257-260, EXPORTED).
#[inline]
pub fn have_newmv_in_inter_mode(mode: PredictionMode) -> bool {
    matches!(
        mode,
        PredictionMode::NewMv
            | PredictionMode::NewNewMv
            | PredictionMode::NearestNewMv
            | PredictionMode::NewNearestMv
            | PredictionMode::NearNewMv
            | PredictionMode::NewNearMv
    )
}

/// C `have_nearmv_in_inter_mode` (inter_prediction.h:416-418, `static INLINE`).
#[inline]
pub fn have_nearmv_in_inter_mode(mode: PredictionMode) -> bool {
    matches!(
        mode,
        PredictionMode::NearMv
            | PredictionMode::NearNearMv
            | PredictionMode::NearNewMv
            | PredictionMode::NewNearMv
    )
}

/// C `is_inter_compound_mode` (definitions.h:1622-1624, `static INLINE`):
/// `mode >= NEAREST_NEARESTMV && mode <= NEW_NEWMV`.
#[inline]
pub fn is_inter_compound_mode(mode: PredictionMode) -> bool {
    let v = mode as u8;
    v >= PredictionMode::NearestNearestMv as u8 && v <= PredictionMode::NewNewMv as u8
}

/// C `is_inter_singleref_mode` (definitions.h:1626-1628, `static INLINE`):
/// `mode >= SINGLE_INTER_MODE_START && mode < SINGLE_INTER_MODE_END`.
#[inline]
pub fn is_inter_singleref_mode(mode: PredictionMode) -> bool {
    let v = mode as u8;
    (PredictionMode::SINGLE_INTER_MODE_START..PredictionMode::SINGLE_INTER_MODE_END).contains(&v)
}

/// Which of a block's two reference MVs carry a coded MV difference.
///
/// This is the shape shared by the writer (`entropy_coding.c:5216-5244`) and
/// the rate estimator (`rd_cost.c:1088-1128`) — both switch on the inter mode
/// in exactly this order, and the two must agree or MD prices a bitstream it
/// is not going to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvCodePlan {
    /// No MV difference is coded (NEARESTMV / NEARMV / GLOBALMV and their
    /// compound forms).
    None,
    /// Only `mv[0]` / `pred_mv[0]` (NEW_NEARESTMV, NEW_NEARMV).
    Ref0,
    /// Only `mv[1]` / `pred_mv[1]` (NEAREST_NEWMV, NEAR_NEWMV).
    Ref1,
    /// `mv[0]` then `mv[1]` (NEW_NEWMV — C loops `ref < 1 + is_compound`).
    Both,
}

impl MvCodePlan {
    /// The reference indices this plan codes, in C's coding order.
    #[inline]
    pub fn refs(self) -> &'static [usize] {
        match self {
            MvCodePlan::None => &[],
            MvCodePlan::Ref0 => &[0],
            MvCodePlan::Ref1 => &[1],
            MvCodePlan::Both => &[0, 1],
        }
    }
}

/// C `entropy_coding.c:5216-5244` / `rd_cost.c:1088-1128`: which MVs an inter
/// block codes, from its mode alone.
///
/// C's writer branches `inter_mode == NEWMV || inter_mode == NEW_NEWMV` and
/// then loops `for (ref = 0; ref < 1 + is_compound; ++ref)`. NEWMV is a
/// single-ref mode so `is_compound` is 0 there and the loop runs once;
/// NEW_NEWMV is compound so it runs twice. That collapses to [`Self::Ref0`]
/// for NEWMV and [`Self::Both`] for NEW_NEWMV.
#[inline]
pub fn mv_code_plan(mode: PredictionMode) -> MvCodePlan {
    match mode {
        PredictionMode::NewMv => MvCodePlan::Ref0,
        PredictionMode::NewNewMv => MvCodePlan::Both,
        PredictionMode::NearestNewMv | PredictionMode::NearNewMv => MvCodePlan::Ref1,
        PredictionMode::NewNearestMv | PredictionMode::NewNearMv => MvCodePlan::Ref0,
        _ => MvCodePlan::None,
    }
}

// =============================================================================
// §2. Precision derivation + the writer
// =============================================================================

/// The precision `svt_av1_encode_mv` (entropy_coding.c:1492-1516) actually
/// codes at.
///
/// Every C call site passes `usehp = frm_hdr->allow_high_precision_mv`
/// (:5227/:5235/:5242) and the function itself overrides it to
/// `MV_SUBPEL_NONE` when `frm_hdr.force_integer_mv` is set (:1498-1500). The
/// same derivation appears on the stats side as
/// `allow_hp = force_integer_mv ? MV_SUBPEL_NONE : allow_high_precision_mv`
/// (md_rate_estimation.c:1029-1030).
#[inline]
pub fn mv_precision(allow_high_precision_mv: bool, force_integer_mv: bool) -> MvSubpelPrecision {
    if force_integer_mv {
        MvSubpelPrecision::None
    } else if allow_high_precision_mv {
        MvSubpelPrecision::High
    } else {
        MvSubpelPrecision::Low
    }
}

/// C `svt_av1_encode_mv` (entropy_coding.c:1492-1516): write one MV as a
/// difference from `ref_mv`, through the frame's adapting `nmvc`.
///
/// The vertical (row/y) component is coded first. `nmvc` MUST be the frame
/// context's single `NmvContext` threaded across every MV of the frame — a
/// fresh context per MV is not decodable, which is what
/// [`crate::entropy::mv_coding::write_mv`] does and why it is not used here.
#[inline]
pub fn encode_mv(
    w: &mut AomWriter,
    nmvc: &mut NmvContext,
    mv: Mv,
    ref_mv: Mv,
    precision: MvSubpelPrecision,
) {
    let diff_row = i32::from(mv.y) - i32::from(ref_mv.y);
    let diff_col = i32::from(mv.x) - i32::from(ref_mv.x);
    encode_mv_diff(w, nmvc, diff_row, diff_col, precision);
}

/// C `entropy_coding.c:5216-5244`: write the MV difference(s) an inter block
/// codes, in C's order, through one adapting `nmvc`.
///
/// `mvs` / `pred_mvs` are the block's `block_mi.mv[]` and `blk_ptr->predmv[]`.
/// Returns the plan actually taken so a caller can assert against its own
/// candidate bookkeeping.
pub fn write_inter_block_mvs(
    w: &mut AomWriter,
    nmvc: &mut NmvContext,
    mode: PredictionMode,
    mvs: &[Mv; 2],
    pred_mvs: &[Mv; 2],
    precision: MvSubpelPrecision,
) -> MvCodePlan {
    let plan = mv_code_plan(mode);
    for &r in plan.refs() {
        encode_mv(w, nmvc, mvs[r], pred_mvs[r], precision);
    }
    plan
}

// =============================================================================
// §3. MV rate — svt_aom_estimate_mv_rate and the per-mode dispatch
// =============================================================================

/// C `MV_COST_WEIGHT` (mode_decision.c:295, product_coding_loop.c:63,
/// rd_cost.c:26 — the same literal 108 in all three).
pub const MV_COST_WEIGHT: i32 = 108;

/// The nmv cost tables as `svt_aom_estimate_mv_rate` leaves them.
///
/// The `approx_inter_rate` arm is NOT "no tables": C `memset`s
/// `nmv_vec_cost` and `nmv_costs` to zero and repoints `nmvcoststack` at the
/// zeroed non-hp array (md_rate_estimation.c:459-465), so every subsequent
/// `svt_av1_mv_bit_cost` through them returns 0. [`Self::Zero`] models that
/// exactly rather than pretending the lookup is skipped.
#[derive(Debug, Clone)]
pub enum NmvRate {
    /// `approx_inter_rate`: joint and component tables are all zero.
    Zero,
    /// The built tables (`nmv_costs_hp` when `allow_high_precision_mv`, else
    /// `nmv_costs` — C keeps both arrays and selects with `nmvcoststack`).
    Tables(MvCostTables),
}

impl NmvRate {
    /// C `mv_cost` (rd_cost.c:53-58) over whichever table this arm selects.
    /// `diff_*` are eighth-pel MV differences.
    #[inline]
    pub fn table_cost(&self, diff_x: i32, diff_y: i32) -> i32 {
        match self {
            NmvRate::Zero => 0,
            NmvRate::Tables(t) => mv_table_cost(diff_x, diff_y, t),
        }
    }

    /// The built tables, or `None` on the `approx_inter_rate` arm.
    #[inline]
    pub fn tables(&self) -> Option<&MvCostTables> {
        match self {
            NmvRate::Zero => None,
            NmvRate::Tables(t) => Some(t),
        }
    }
}

/// Everything `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-488)
/// writes into `MdRateEstimationContext`.
#[derive(Debug, Clone)]
pub struct MvRateEstimate {
    /// `nmv_vec_cost` + the selected `nmvcoststack` tables.
    pub nmv: NmvRate,
    /// `dv_joint_cost` + `dv_cost`, built only when `allow_intrabc` AND the
    /// `approx_inter_rate` early return did not fire. `None` means C left the
    /// arrays UNTOUCHED (stale memory there, not zeros) — see
    /// `tests/c_parity_intrabc.rs::c_parity_estimate_mv_rate_gating`.
    pub dv: Option<MvCostTables>,
}

/// C `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-488).
///
/// Order matters and is reproduced literally:
/// 1. `approx_inter_rate` returns EARLY with nmv zeroed — before the dv arm,
///    so an `allow_intrabc` frame at `approx_inter_rate` gets NO dv tables.
/// 2. The nmv tables are built at `MvSubpelPrecision::High` when
///    `allow_high_precision_mv` else `Low` — never `None`, even under
///    `force_integer_mv` (see the module docs).
/// 3. The dv tables are built from `ndvc` at `MvSubpelPrecision::None`,
///    gated on `allow_intrabc`.
pub fn estimate_mv_rate(
    nmvc: &NmvContext,
    ndvc: &NmvContext,
    allow_high_precision_mv: bool,
    allow_intrabc: bool,
    approx_inter_rate: bool,
) -> MvRateEstimate {
    if approx_inter_rate {
        return MvRateEstimate {
            nmv: NmvRate::Zero,
            dv: None,
        };
    }
    let precision = if allow_high_precision_mv {
        MvSubpelPrecision::High
    } else {
        MvSubpelPrecision::Low
    };
    MvRateEstimate {
        nmv: NmvRate::Tables(build_nmv_cost_table(nmvc, precision)),
        dv: if allow_intrabc {
            Some(build_nmv_cost_table(ndvc, MvSubpelPrecision::None))
        } else {
            None
        },
    }
}

/// C `copy_mv_rate` (enc_dec_process.c:36-56) plus the cadence choice its two
/// call sites make (`:2802-2806` and `:2908-2912`): what MD's per-SB rate
/// tables hold.
///
/// When `cdf_ctrl.update_mv` is set ([`cdf_update_mv`]), C re-runs
/// `svt_aom_estimate_mv_rate` per superblock against that SB's own adapting
/// `ec_ctx_array[sb]`; when it is clear, C copies the FRAME-level tables once
/// (built at frame genesis from `pcs->md_frame_context`) and reuses them for
/// every SB. Passing the SB context in the second case would be wrong even
/// though it is the same context on the first SB.
///
/// C's copy is a `memcpy` of only the SELECTED hp/non-hp array plus, gated on
/// `allow_intrabc`, the dv pair — the unselected array is left stale. That is
/// unobservable here because [`MvRateEstimate`] owns exactly the selected
/// tables, so the copy is a clone.
pub fn sb_mv_rate(
    update_mv: bool,
    frame_rate: &MvRateEstimate,
    sb_nmvc: &NmvContext,
    sb_ndvc: &NmvContext,
    allow_high_precision_mv: bool,
    allow_intrabc: bool,
    approx_inter_rate: bool,
) -> MvRateEstimate {
    if update_mv {
        estimate_mv_rate(
            sb_nmvc,
            sb_ndvc,
            allow_high_precision_mv,
            allow_intrabc,
            approx_inter_rate,
        )
    } else {
        frame_rate.clone()
    }
}

/// C `svt_av1_mv_bit_cost` (rd_cost.c:70-78) over an [`NmvRate`], including
/// the `approx_inter_rate` zero-table arm.
///
/// C clamps the DIFFERENCE to `[MV_LOW, MV_UPP] = [-16384, 16384]` before the
/// table lookup; [`crate::intrabc::MvComponentCost::cost`] clamps one ULP
/// narrower to the table's populated `[-MV_MAX, MV_MAX]` so the port never
/// reads outside the array (C reads one element past it at exactly ±16384 —
/// see that type's PORT-NOTE). Any MV pair reachable from a legal frame stays
/// well inside both bounds.
#[inline]
pub fn mv_bit_cost(mv: Mv, ref_mv: Mv, rate: &NmvRate, weight: i32) -> i32 {
    let diff_x = i32::from(mv.x) - i32::from(ref_mv.x);
    let diff_y = i32::from(mv.y) - i32::from(ref_mv.y);
    // C: ROUND_POWER_OF_TWO(cost * weight, RDDIV_BITS = 7).
    let v = rate.table_cost(diff_x, diff_y) * weight;
    (v + (1 << 6)) >> 7
}

/// C `svt_aom_mv_err_cost` (av1me.c:141-149) over an [`NmvRate`] — the
/// SSD-domain MV cost the sub-pel search pays, `use_mvcost` arm.
///
/// The inter search reads its tables through `x->nmv_vec_cost` /
/// `x->mv_cost_stack`, which `svt_aom_md_init_xd`-time assignment points at
/// `md_rate_est_ctx->nmv_vec_cost` / `nmvcoststack` (mode_decision.c:2098-2099,
/// :2984-2985) — i.e. at the tables [`estimate_mv_rate`] builds, NOT at the dv
/// tables `intrabc.rs` gates. On the `approx_inter_rate` arm those tables are
/// zeroed, so the cost is 0; C reaches the same value through its own zero
/// fill rather than through the `_light` twin, which has a separate call site.
#[inline]
pub fn mv_err_cost(mv: Mv, ref_mv: Mv, rate: &NmvRate, error_per_bit: i32) -> i32 {
    match rate {
        // Zeroed tables: mv_cost() is 0, and ROUND_POWER_OF_TWO_64(0, k) == 0.
        NmvRate::Zero => 0,
        NmvRate::Tables(t) => crate::intrabc::mv_err_cost(mv, ref_mv, t, error_per_bit),
    }
}

/// C `svt_aom_mv_err_cost_light` (av1me.c:126-132) — the `approx_inter_rate`
/// fast path. Textually identical to [`mv_bit_cost_light`]; C duplicates the
/// body under two names for two call sites and this port keeps both so each
/// call site cites the function it actually mirrors.
#[inline]
pub fn mv_err_cost_light(mv: Mv, ref_mv: Mv) -> i32 {
    crate::intrabc::mv_err_cost_light(mv, ref_mv)
}

/// C `svt_av1_mv_bit_cost_light` (rd_cost.c:59-65) — the `approx_inter_rate`
/// fast path, table-independent.
#[inline]
pub fn mv_bit_cost_light(mv: Mv, ref_mv: Mv) -> i32 {
    const FACTOR: i32 = 50;
    let absdx = (i32::from(mv.x) - i32::from(ref_mv.x)).abs();
    let absdy = (i32::from(mv.y) - i32::from(ref_mv.y)).abs();
    1296 + FACTOR * (absdx + absdy)
}

/// C `svt_aom_inter_fast_cost`'s `mv_rate` term (rd_cost.c:1088-1128): the
/// total MV rate an inter candidate pays, dispatched on its mode.
///
/// Mirrors [`mv_code_plan`] exactly — that identity is the point: MD must
/// price precisely the MVs the writer will emit. Modes with no coded MV pay
/// zero (C leaves `mv_rate` at its `0` initializer and never enters the
/// `svt_aom_have_newmv_in_inter_mode` block).
pub fn inter_mv_rate(
    mode: PredictionMode,
    mvs: &[Mv; 2],
    pred_mvs: &[Mv; 2],
    rate: &NmvRate,
    weight: i32,
) -> i32 {
    mv_code_plan(mode)
        .refs()
        .iter()
        .map(|&r| mv_bit_cost(mvs[r], pred_mvs[r], rate, weight))
        .sum()
}

// =============================================================================
// §4. CDF adaptation for MVs
// =============================================================================

/// C `update_mv_component_stats` (md_rate_estimation.c:650-686): replay one
/// MV component's symbol sequence as `update_cdf` calls, in write order.
///
/// `comp` is the already-differenced component and must be nonzero (C
/// `assert(comp != 0)`; the joint type guarantees it).
pub fn update_mv_component_stats(
    comp: i32,
    mvcomp: &mut NmvComponent,
    precision: MvSubpelPrecision,
) {
    use crate::entropy::cdf::update_cdf;
    debug_assert!(comp != 0);
    let sign = comp < 0;
    let mag = comp.unsigned_abs() as i32;
    let (mv_class, offset) = get_mv_class(mag - 1);
    let d = offset >> 3;
    let fr = (offset >> 1) & 3;
    let hp = offset & 1;

    update_cdf(&mut mvcomp.sign_cdf, usize::from(sign), 2);
    update_cdf(&mut mvcomp.classes_cdf, mv_class as usize, MV_CLASSES);
    if mv_class == 0 {
        update_cdf(&mut mvcomp.class0_cdf, d as usize, CLASS0_SIZE);
    } else {
        let n = mv_class as i32 + CLASS0_BITS as i32 - 1;
        for i in 0..n {
            update_cdf(&mut mvcomp.bits_cdf[i as usize], ((d >> i) & 1) as usize, 2);
        }
    }
    if (precision as i32) > (MvSubpelPrecision::None as i32) {
        if mv_class == 0 {
            update_cdf(
                &mut mvcomp.class0_fp_cdf[d as usize],
                fr as usize,
                MV_FP_SIZE,
            );
        } else {
            update_cdf(&mut mvcomp.fp_cdf, fr as usize, MV_FP_SIZE);
        }
    }
    if (precision as i32) > (MvSubpelPrecision::Low as i32) {
        if mv_class == 0 {
            update_cdf(&mut mvcomp.class0_hp_cdf, hp as usize, 2);
        } else {
            update_cdf(&mut mvcomp.hp_cdf, hp as usize, 2);
        }
    }
}

/// C `av1_update_mv_stats` (md_rate_estimation.c:690-705): replay one MV's
/// whole symbol sequence — joint, then vertical, then horizontal — into
/// `nmvc`.
///
/// This is MD's shadow adaptation of the per-SB `ec_ctx_array[sb]`, driven
/// from `svt_aom_update_stats` (:1026-1046) and gated by
/// [`cdf_update_mv`]. Its update sequence is identical to the one
/// `aom_write_symbol` performs while WRITING the same MV, which is how
/// `tests/c_parity_mv_code.rs` gates it against the real
/// `svt_av1_encode_mv`.
pub fn update_mv_stats(nmvc: &mut NmvContext, mv: Mv, ref_mv: Mv, precision: MvSubpelPrecision) {
    use crate::entropy::cdf::update_cdf;
    let diff_x = i32::from(mv.x) - i32::from(ref_mv.x);
    let diff_y = i32::from(mv.y) - i32::from(ref_mv.y);
    // C `svt_av1_get_mv_joint` (rd_cost.c:47): tests y first.
    let j = if diff_y == 0 {
        if diff_x == 0 { 0usize } else { 1 }
    } else if diff_x == 0 {
        2
    } else {
        3
    };
    update_cdf(&mut nmvc.joints_cdf, j, MV_JOINTS);
    // mv_joint_vertical(j): j == HZVNZ(2) || j == HNZVNZ(3).
    if j == 2 || j == 3 {
        update_mv_component_stats(diff_y, &mut nmvc.comps[0], precision);
    }
    // mv_joint_horizontal(j): j == HNZVZ(1) || j == HNZVNZ(3).
    if j == 1 || j == 3 {
        update_mv_component_stats(diff_x, &mut nmvc.comps[1], precision);
    }
}

/// C `svt_aom_update_stats`'s MV arm (md_rate_estimation.c:1026-1046): the
/// per-block dispatch that feeds [`update_mv_stats`], in the same shape as
/// the writer's ([`write_inter_block_mvs`]).
pub fn update_inter_block_mv_stats(
    nmvc: &mut NmvContext,
    mode: PredictionMode,
    mvs: &[Mv; 2],
    pred_mvs: &[Mv; 2],
    precision: MvSubpelPrecision,
) -> MvCodePlan {
    let plan = mv_code_plan(mode);
    for &r in plan.refs() {
        update_mv_stats(nmvc, mvs[r], pred_mvs[r], precision);
    }
    plan
}

/// C `reset_nmv_counter` (cabac_context_model.c:1956-1969), reached through
/// the exported `svt_av1_reset_cdf_symbol_counters` (:1971).
///
/// `RESET_CDF_COUNTER(cdf, nsymbs)` zeroes the adaptation counter at
/// `cdf[nsymbs]` of every CDF in the named array and leaves the
/// probabilities alone. `NmvContext`'s per-field `nsymbs` are spelled out
/// here exactly as the C macro invocations spell them.
pub fn reset_nmv_counter(nmvc: &mut NmvContext) {
    nmvc.joints_cdf[MV_JOINTS] = 0;
    for comp in &mut nmvc.comps {
        comp.classes_cdf[MV_CLASSES] = 0;
        for row in &mut comp.class0_fp_cdf {
            row[MV_FP_SIZE] = 0;
        }
        comp.fp_cdf[MV_FP_SIZE] = 0;
        comp.sign_cdf[2] = 0;
        comp.class0_hp_cdf[2] = 0;
        comp.hp_cdf[2] = 0;
        comp.class0_cdf[CLASS0_SIZE] = 0;
        for row in &mut comp.bits_cdf {
            row[2] = 0;
        }
    }
}

/// C `avg_nmv` (enc_dec_process.c:2567-2579): weighted per-entry average of
/// two SBs' `NmvContext`s, in place into `left`.
///
/// Same field enumeration as [`reset_nmv_counter`]; the per-entry arithmetic
/// is [`crate::entropy::cdf::avg_cdf_entries`], which is already gated
/// against C's `avg_cdf_symbol`. C's `AVERAGE_CDF` covers every element of
/// each array including the counter slot, so a flat pass over the whole
/// field reproduces it (see that function's docs).
///
/// The `ndvc` twin already lives in
/// [`crate::entropy::context::FrameContext::avg_cdf_with`]; this is the
/// `nmvc` half, which has no home yet because `FrameContext` carries no
/// `nmvc` field (adding one belongs to the chunk that owns
/// `entropy/context.rs`).
pub fn avg_nmv(left: &mut NmvContext, tr: &NmvContext, wt_left: i32, wt_tr: i32) {
    use crate::entropy::cdf::avg_cdf_entries as avg;
    avg(&mut left.joints_cdf, &tr.joints_cdf, wt_left, wt_tr);
    for i in 0..2 {
        let (l, r) = (&mut left.comps[i], &tr.comps[i]);
        avg(&mut l.classes_cdf, &r.classes_cdf, wt_left, wt_tr);
        avg(
            l.class0_fp_cdf.as_flattened_mut(),
            r.class0_fp_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(&mut l.fp_cdf, &r.fp_cdf, wt_left, wt_tr);
        avg(&mut l.sign_cdf, &r.sign_cdf, wt_left, wt_tr);
        avg(&mut l.class0_hp_cdf, &r.class0_hp_cdf, wt_left, wt_tr);
        avg(&mut l.hp_cdf, &r.hp_cdf, wt_left, wt_tr);
        avg(&mut l.class0_cdf, &r.class0_cdf, wt_left, wt_tr);
        avg(
            l.bits_cdf.as_flattened_mut(),
            r.bits_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
    }
}

// =============================================================================
// §5. The update_mv cadence
// =============================================================================

/// C `set_cdf_controls`' `update_mv` arm (enc_mode_config.c:8468-8498).
///
/// Level 1 is the only level that adapts MVs, and an I-slice forces it off
/// unconditionally (:8496) — which is why the whole still-image envelope
/// never exercises MV CDF adaptation and why this chunk needed its own
/// oracle rather than an identity cell.
#[inline]
pub fn cdf_update_mv(update_cdf_level: u8, is_i_slice: bool) -> bool {
    let update_mv = update_cdf_level == 1;
    update_mv && !is_i_slice
}

/// C `svt_aom_get_update_cdf_level_default` (enc_mode_config.c:8510-8522).
#[inline]
pub fn update_cdf_level_default(enc_mode: i32, is_i_slice: bool, is_base: bool) -> u8 {
    if enc_mode <= 0 {
        1
    } else if enc_mode <= 3 {
        if is_base { 1 } else { 2 }
    } else if enc_mode <= 8 {
        u8::from(is_i_slice)
    } else {
        0
    }
}

/// C `svt_aom_get_update_cdf_level_rtc` (enc_mode_config.c:8524-8532).
#[inline]
pub fn update_cdf_level_rtc(enc_mode: i32, is_i_slice: bool) -> u8 {
    if enc_mode <= 8 {
        u8::from(is_i_slice)
    } else {
        0
    }
}

/// C `svt_aom_get_update_cdf_level_allintra` (enc_mode_config.c:8534-8545).
#[inline]
pub fn update_cdf_level_allintra(enc_mode: i32) -> u8 {
    if enc_mode <= 3 {
        1
    } else if enc_mode <= 6 {
        2
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_plan_matches_writer_and_rate_dispatch() {
        use PredictionMode as M;
        assert_eq!(mv_code_plan(M::NewMv), MvCodePlan::Ref0);
        assert_eq!(mv_code_plan(M::NewNewMv), MvCodePlan::Both);
        assert_eq!(mv_code_plan(M::NearestNewMv), MvCodePlan::Ref1);
        assert_eq!(mv_code_plan(M::NearNewMv), MvCodePlan::Ref1);
        assert_eq!(mv_code_plan(M::NewNearestMv), MvCodePlan::Ref0);
        assert_eq!(mv_code_plan(M::NewNearMv), MvCodePlan::Ref0);
        for m in [
            M::NearestMv,
            M::NearMv,
            M::GlobalMv,
            M::NearestNearestMv,
            M::NearNearMv,
            M::GlobalGlobalMv,
        ] {
            assert_eq!(mv_code_plan(m), MvCodePlan::None, "{m:?}");
        }
        // Every mode that codes an MV is exactly the have_newmv set.
        for raw in 13u8..=24 {
            let m = mode_from_raw(raw);
            assert_eq!(
                mv_code_plan(m) != MvCodePlan::None,
                have_newmv_in_inter_mode(m),
                "{m:?}"
            );
        }
    }

    fn mode_from_raw(v: u8) -> PredictionMode {
        use PredictionMode as M;
        match v {
            13 => M::NearestMv,
            14 => M::NearMv,
            15 => M::GlobalMv,
            16 => M::NewMv,
            17 => M::NearestNearestMv,
            18 => M::NearNearMv,
            19 => M::NearestNewMv,
            20 => M::NewNearestMv,
            21 => M::NearNewMv,
            22 => M::NewNearMv,
            23 => M::GlobalGlobalMv,
            24 => M::NewNewMv,
            _ => unreachable!(),
        }
    }

    #[test]
    fn precision_derivation() {
        assert_eq!(mv_precision(false, false), MvSubpelPrecision::Low);
        assert_eq!(mv_precision(true, false), MvSubpelPrecision::High);
        // force_integer_mv wins over allow_high_precision_mv.
        assert_eq!(mv_precision(true, true), MvSubpelPrecision::None);
        assert_eq!(mv_precision(false, true), MvSubpelPrecision::None);
    }

    #[test]
    fn approx_inter_rate_zeroes_every_mv_cost() {
        let ctx = NmvContext::default();
        let est = estimate_mv_rate(&ctx, &ctx, true, true, true);
        assert!(matches!(est.nmv, NmvRate::Zero));
        // approx returns BEFORE the dv arm even with allow_intrabc.
        assert!(est.dv.is_none());
        for &(mv, rf) in &[
            (Mv { x: 0, y: 0 }, Mv { x: 0, y: 0 }),
            (Mv { x: 1000, y: -7 }, Mv { x: -3, y: 9 }),
        ] {
            assert_eq!(mv_bit_cost(mv, rf, &est.nmv, MV_COST_WEIGHT), 0);
        }
    }

    #[test]
    fn reset_nmv_counter_only_touches_counters() {
        let mut ctx = NmvContext::default();
        ctx.joints_cdf[MV_JOINTS] = 7;
        ctx.comps[1].bits_cdf[3][2] = 31;
        ctx.comps[0].class0_fp_cdf[1][MV_FP_SIZE] = 12;
        let before_probs = ctx.comps[0].classes_cdf[0];
        reset_nmv_counter(&mut ctx);
        assert_eq!(ctx.joints_cdf[MV_JOINTS], 0);
        assert_eq!(ctx.comps[1].bits_cdf[3][2], 0);
        assert_eq!(ctx.comps[0].class0_fp_cdf[1][MV_FP_SIZE], 0);
        assert_eq!(ctx.comps[0].classes_cdf[0], before_probs);
    }

    #[test]
    fn cdf_update_mv_is_off_on_i_slices_at_every_level() {
        for level in 0u8..=3 {
            assert!(!cdf_update_mv(level, true), "level {level}");
        }
        assert!(cdf_update_mv(1, false));
        for level in [0u8, 2, 3] {
            assert!(!cdf_update_mv(level, false), "level {level}");
        }
    }
}

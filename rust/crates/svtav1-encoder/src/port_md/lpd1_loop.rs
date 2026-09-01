//! The LIGHT-PD1 mode-decision loop of
//! `Source/Lib/Codec/product_coding_loop.c`: the MDS0 fast cost, the
//! one-winner candidate walk, and the MDS3 staging switches.
//!
//! | this module | C |
//! |---|---|
//! | [`fast_loop_core_light_pd1`] | `:1009-1066` |
//! | [`md_stage_0_light_pd1`] | `:1525-1555` |
//! | [`Mds3LightSettings`] | `md_stage_3_light_pd1` `:7119-7135` |
//! | [`plan_chroma`], [`luma_tx_skipped`], [`luma_skip_rd_applies`], [`second_chroma_detector_runs`], [`no_chroma_epilogue`] | `full_loop_core_light_pd1` `:6541-6694`, **decisions only** |
//! | [`luma_eob_zero_takes_the_early_exit`], [`luma_tx_skipped`] | `perform_dct_dct_tx_light_pd1` `:5434-5560`, **decisions only** |
//! | [`lpd1_me_mv_index`], [`lpd1_me_mv_to_eighth_pel`], [`lpd1_skip_subpel`], [`MdMeDist`] | `read_refine_me_mvs_light_pd1` `:2737-2811`, **decisions only** |
//!
//! # Coverage, stated as a fraction
//!
//! `fast_loop_core_light_pd1`, `md_stage_0_light_pd1` and
//! `md_stage_3_light_pd1` are ported WHOLE (their non-arithmetic parts —
//! the predictor call and the fast-cost function — are the caller's, and
//! are parameters).
//!
//! `full_loop_core_light_pd1` and `perform_dct_dct_tx_light_pd1` are
//! **PARTIAL**: every branch condition, threshold and piece of chroma
//! bookkeeping is here, and NONE of the four operations they sequence is —
//! the residual/transform/quantise chain, the chroma full loop, the full
//! cost, and the LPD1 predictor. Those live in the DSP and transform layers
//! this port structures differently, so they are not transcribed from this
//! C file; what is missing from THIS module is their call ORDER, which is a
//! caller concern. Do not read the list above as "the LPD1 full loop is
//! ported".
//!
//! # Why this is separate from the regular loop
//!
//! Light PD1 is the fast INTER path and is a genuinely different algorithm,
//! not a configuration of the regular one:
//!
//! * It keeps **two** candidate buffers and evaluates ONE winner, so there
//!   is no per-class pool, no sort, and no
//!   [`super::nic_prune`] staging. [`md_stage_0_light_pd1`]'s ping-pong
//!   over those two buffers is the whole of its candidate management.
//! * Its fast cost is **luma-only, 8-bit only, and never SSD** (C says so
//!   at `:1008`), with two early exits that abandon a candidate outright by
//!   writing `MAX_MODE_COST`.
//! * MDS1 and MDS2 do not exist on this path: `md_stage_0_light_pd1` is
//!   followed directly by `md_stage_3_light_pd1`.
//!
//! # Evidence
//!
//! **Tier 4 throughout** — all three are `static` in C with no exported
//! symbol (`docs/WORKING-ON-THIS.md` §4). The distortion comes from the
//! port's already-gated [`svtav1_dsp::variance`] rather than a second
//! transcription of `variance_c`.
//!
//! # Reachability
//!
//! Nothing calls this yet — the public entry point still refuses inter
//! frames (`docs/WORKING-ON-THIS.md` §7).

use super::lpd1::{ComponentType, Plane};
use svtav1_types::motion::Mv;

/// C `MAX_MODE_COST` (coding_unit.h:37) — the "never pick this" sentinel a
/// candidate's fast cost is set to when an early exit abandons it.
pub const MAX_MODE_COST: u64 = 13_754_408_443_200 * 8;

/// C `CandEliminationCtlrs` (md_process.h:523-533), the fields the LPD1
/// fast loop reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CandEliminationCtrls {
    pub enabled: bool,
    /// `dc_only_th` — applied to a NON-DC intra candidate.
    pub dc_only_th: u32,
    /// `skip_dc_th` — applied to the DC candidate itself.
    pub skip_dc_th: u32,
}

/// The running MDS0 winner, or `None` for C's `(uint64_t)~0` "no candidate
/// scored yet" sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mds0Best {
    /// C `ctx->mds0_best_cost`.
    pub cost: u64,
    /// C `cand_bf_ptr_array[ctx->mds0_best_idx]->luma_fast_dist`.
    pub luma_fast_dist: u64,
}

/// What [`fast_loop_core_light_pd1`] produced for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLoopOutcome {
    /// C wrote `MAX_MODE_COST` into the candidate's fast cost and returned
    /// without predicting or scoring it. [`FastLoopOutcome::cost`] reports
    /// that sentinel so the caller's comparison is unchanged.
    Eliminated {
        /// Which of C's two early exits fired.
        reason: EliminationReason,
    },
    /// The candidate was predicted and scored.
    Scored(FastScore),
}

/// Which early exit abandoned a candidate. C makes no distinction — both
/// arms write the same sentinel — but naming them keeps a bisect honest
/// about WHICH gate dropped a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EliminationReason {
    /// `:1018-1029` — an intra candidate on a block the current best
    /// already predicts well.
    IntraOnAWellPredictedBlock,
    /// `:1049-1054` — the distortion ALONE already costs more than the
    /// current best total.
    DistortionAloneExceedsTheBest,
}

/// The numbers C writes onto the candidate buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastScore {
    /// C `*cand_bf->fast_cost`.
    pub fast_cost: u64,
    /// C `cand_bf->luma_fast_dist` — the RAW variance, NOT shifted.
    pub luma_fast_dist: u64,
    /// C `cand_bf->full_dist` — the SSE, kept because the TX-bypass gate
    /// reads it (`:1045`).
    pub full_dist: u64,
    /// C `cand_bf->fast_luma_rate`.
    pub fast_luma_rate: u64,
    /// C `cand_bf->fast_chroma_rate`.
    pub fast_chroma_rate: u64,
}

impl FastLoopOutcome {
    /// The fast cost the caller compares, sentinel included.
    #[must_use]
    pub fn cost(&self) -> u64 {
        match self {
            FastLoopOutcome::Eliminated { .. } => MAX_MODE_COST,
            FastLoopOutcome::Scored(s) => s.fast_cost,
        }
    }
}

/// C `RDCOST` (rd_cost.h:36).
#[inline]
fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + (1 << 8)) >> 9) + (dist << 7)
}

/// C `fast_loop_core_light_pd1` (`:1009-1066`).
///
/// `pred` must already hold the candidate's prediction — C runs
/// `product_prediction_fun_table_light_pd1` at `:1034`, between the two
/// early exits, and that predictor lives in the DSP layer. Splitting it out
/// is deliberate and is the ONE reordering here: the caller predicts only
/// after [`intra_candidate_eliminated`] says the first gate did not fire.
///
/// `fast_cost_fn` is C's `av1_product_fast_cost_func_table[is_inter]`,
/// which returns the whole fast COST given the shifted distortion; it is
/// not called at all when `shut_fast_rate` is set.
///
/// Three things a paraphrase loses:
///
/// * **The distortion is shifted left by 4 for the cost and NOT for the
///   stored value** (`:1044`). C's comment explains why: full lambda is
///   calibrated for a squared metric already shifted by 4, and variance is
///   not. Storing the shifted value would corrupt every later consumer of
///   `luma_fast_dist`, including [`super::lpd1::should_perform_tx`].
/// * **`full_dist` is the SSE, not the variance** (`:1046`) — a different
///   number from the same pass, and the TX-bypass gate reads it.
/// * **The second early exit compares a DISTORTION-ONLY cost** against the
///   best TOTAL cost (`:1050-1051`). That is valid because rate is
///   non-negative, and it is why the exit is safe before the rate is known.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn fast_loop_core_light_pd1(
    best_so_far: Option<Mds0Best>,
    block: (usize, usize),
    full_lambda: u64,
    shut_fast_rate: bool,
    src: Plane<'_>,
    pred: Plane<'_>,
    fast_cost_fn: impl FnOnce(u64) -> (u64, u64, u64),
) -> FastLoopOutcome {
    let (width, height) = block;

    // C's `vf(pred, pred_stride, src, src_stride, &sse)` — prediction
    // first. The variance is symmetric in its arguments, but the order is
    // kept so a reader can check it against C.
    let variance = svtav1_dsp::variance::variance_diff(
        pred.data,
        pred.stride,
        src.data,
        src.stride,
        width,
        height,
    );
    // C's `vf` writes the SSE through an out-parameter in the SAME pass.
    // The port takes a second pass rather than duplicating the kernel; the
    // values are identical by construction. C stores the SSE into a
    // `unsigned int` before widening it, and at the widest block (128x128,
    // max per-sample error 255) the SSE is 1.07e9 — inside 32 bits — so
    // the truncation is inert and is transcribed rather than relied on.
    let sse = svtav1_dsp::variance::sse(pred.data, pred.stride, src.data, src.stride, width, height)
        as u32;

    let luma_fast_dist = u64::from(variance);
    // `:1044` — shifted for the COST only.
    let shifted_dist = luma_fast_dist << 4;

    if let Some(best) = best_so_far {
        let distortion_cost = rdcost(full_lambda, 0, shifted_dist);
        if distortion_cost > best.cost {
            return FastLoopOutcome::Eliminated {
                reason: EliminationReason::DistortionAloneExceedsTheBest,
            };
        }
    }

    let (fast_cost, fast_luma_rate, fast_chroma_rate) = if shut_fast_rate {
        // `:1059` — the cost IS the shifted distortion, with no RDCOST
        // wrapper. Applying one here would scale it by 128.
        (shifted_dist, 0, 0)
    } else {
        fast_cost_fn(shifted_dist)
    };

    FastLoopOutcome::Scored(FastScore {
        fast_cost,
        luma_fast_dist,
        full_dist: u64::from(sse),
        fast_luma_rate,
        fast_chroma_rate,
    })
}

/// C's FIRST early exit in `fast_loop_core_light_pd1` (`:1017-1031`), split
/// out because it decides whether the prediction runs at all.
///
/// An INTRA candidate is dropped when the running best already predicts the
/// block well enough — measured as its `luma_fast_dist` against a threshold
/// scaled by the block AREA. The threshold is the harsher `skip_dc_th` for
/// the DC candidate itself and `dc_only_th` for every other intra mode,
/// which is the opposite of the naming's first reading: `dc_only_th` is the
/// threshold used when the mode is NOT DC.
#[must_use]
pub fn intra_candidate_eliminated(
    elim: &CandEliminationCtrls,
    best_so_far: Option<Mds0Best>,
    is_intra_mode: bool,
    is_dc_pred: bool,
    block: (usize, usize),
) -> bool {
    let Some(best) = best_so_far else {
        return false;
    };
    if !is_intra_mode || !elim.enabled {
        return false;
    }
    let th = if is_dc_pred {
        elim.skip_dc_th
    } else {
        elim.dc_only_th
    };
    // C multiplies a `uint32_t` threshold by the area in `uint32_t`
    // (`:1024`). At 128x128 an area of 16384 times a shipped threshold of a
    // few hundred stays inside 32 bits; the port widens to u64 so the
    // comparison against a u64 distortion needs no cast, which is
    // value-identical over that domain.
    let th = u64::from(th) * (block.0 * block.1) as u64;
    best.luma_fast_dist < th
}

/// The result of the LPD1 candidate walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mds0LightResult {
    /// C `ctx->mds0_best_idx` — a BUFFER index in `0..2`, not a candidate
    /// index.
    pub best_buffer_idx: usize,
    /// C `ctx->mds0_best_cost`.
    pub best_cost: u64,
    /// Which candidate ended up in the winning buffer.
    pub best_cand_idx: Option<usize>,
}

/// C `md_stage_0_light_pd1` (`:1525-1555`).
///
/// `score` receives `(cand_idx, buffer_idx)` and returns that candidate's
/// fast cost — it is C's `fast_loop_core_light_pd1` call plus the buffer
/// writes around it.
///
/// **The two-buffer ping-pong is the whole point and it is easy to get
/// wrong.** C toggles `cand_buff_idx` ONLY inside the improvement branch
/// (`:1552`). So a candidate that does not improve is overwritten in place
/// by the next one, and the buffer holding the current winner is never
/// touched again until something beats it. Toggling unconditionally — the
/// obvious "alternate buffers" reading — would let the second
/// non-improving candidate clobber the winner's buffer.
///
/// A corollary worth stating because it looks like a bug: after the LAST
/// improvement the index has already toggled AWAY from the winner, so
/// `mds0_best_idx` (captured before the toggle) and `cand_buff_idx` point
/// at different buffers at the end. That is correct — `mds0_best_idx` is
/// what MDS3 reads.
pub fn md_stage_0_light_pd1(
    n_candidates: usize,
    mut score: impl FnMut(usize, usize) -> u64,
) -> Mds0LightResult {
    let mut best_cost = u64::MAX;
    let mut best_buffer_idx = 0usize;
    let mut best_cand_idx = None;
    let mut cand_buff_idx = 0usize;

    for cand_idx in 0..n_candidates {
        let cost = score(cand_idx, cand_buff_idx);
        if cost < best_cost {
            best_cost = cost;
            best_buffer_idx = cand_buff_idx;
            best_cand_idx = Some(cand_idx);
            cand_buff_idx = 1 - cand_buff_idx;
        }
    }

    Mds0LightResult {
        best_buffer_idx,
        best_cost,
        best_cand_idx,
    }
}

/// The MD-staging switches C `md_stage_3_light_pd1` (`:7119-7135`) sets
/// before the single full loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mds3LightSettings {
    /// `ctx->mds_do_chroma` — always true; LPD1 assumes 4xN / Nx4 blocks
    /// are disabled, so `blk_geom->has_uv` is taken as given (`:7123`).
    pub mds_do_chroma: bool,
    /// `ctx->uv_intra_comp_only`
    pub uv_intra_comp_only: bool,
    /// `ctx->rdoq_ctrls.skip_uv`
    pub rdoq_skip_uv: bool,
    /// `ctx->rdoq_ctrls.dct_dct_only`
    pub rdoq_dct_dct_only: bool,
    /// `ctx->mds_do_rdoq`
    pub mds_do_rdoq: bool,
    /// `ctx->mds_fast_coeff_est_level`
    pub mds_fast_coeff_est_level: u8,
    /// `ctx->mds_subres_step`
    pub mds_subres_step: u8,
}

/// C `md_stage_3_light_pd1`'s settings block (`:7124-7133`).
///
/// The two RDOQ switches are CLEARED only when EncDec is bypassed
/// (`:7127-7130`): with no EncDec pass to re-run the transform, the
/// shortcuts EncDec would normally compensate for must not be taken. When
/// EncDec does run they keep whatever the picture-level derivation set,
/// which is why they are inputs.
#[must_use]
pub fn md_stage_3_light_pd1_settings(
    bypass_encdec: bool,
    prior_rdoq_skip_uv: bool,
    prior_rdoq_dct_dct_only: bool,
) -> Mds3LightSettings {
    let (rdoq_skip_uv, rdoq_dct_dct_only) = if bypass_encdec {
        (false, false)
    } else {
        (prior_rdoq_skip_uv, prior_rdoq_dct_dct_only)
    };
    Mds3LightSettings {
        mds_do_chroma: true,
        uv_intra_comp_only: true,
        rdoq_skip_uv,
        rdoq_dct_dct_only,
        mds_do_rdoq: true,
        mds_fast_coeff_est_level: 1,
        mds_subres_step: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tier 4: hand-derived against the C lines named in each comment. All
    /// three functions are `static` in `product_coding_loop.c`.
    fn plane(data: &[u8], stride: usize) -> Plane<'_> {
        Plane::new(data, stride)
    }

    /// `:1552` — the toggle is INSIDE the improvement branch. With costs
    /// 10, 20, 5 the winner must survive the middle candidate.
    #[test]
    fn the_buffer_pingpong_protects_the_running_winner() {
        let costs = [10u64, 20, 5];
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let r = md_stage_0_light_pd1(costs.len(), |c, b| {
            seen.push((c, b));
            costs[c]
        });
        // cand 0 -> buffer 0, improves, toggle to 1.
        // cand 1 -> buffer 1, does NOT improve, no toggle (buffer 0 keeps
        //           the winner).
        // cand 2 -> buffer 1 again, improves, best_idx 1.
        assert_eq!(seen, vec![(0, 0), (1, 1), (2, 1)]);
        assert_eq!(r.best_buffer_idx, 1);
        assert_eq!(r.best_cost, 5);
        assert_eq!(r.best_cand_idx, Some(2));
    }

    /// An unconditional toggle clobbers the winner as soon as TWO
    /// candidates in a row fail to improve: the second of them lands back
    /// on the winner's buffer. Stated as its own case because the
    /// difference is invisible on any sequence that keeps improving, and
    /// the first draft of this test used one that did (10, 20, 5) and
    /// passed under both readings.
    #[test]
    fn an_unconditional_toggle_would_clobber_the_winner() {
        let costs = [10u64, 20, 30];
        let mut buffers = [u64::MAX; 2];
        let r = md_stage_0_light_pd1(costs.len(), |c, b| {
            buffers[b] = costs[c];
            costs[c]
        });
        assert_eq!(buffers[r.best_buffer_idx], r.best_cost);
        // The alternate reading, simulated:
        let mut naive_buffers = [u64::MAX; 2];
        let mut naive_best = u64::MAX;
        let mut naive_idx = 0usize;
        for (c, &cost) in costs.iter().enumerate() {
            let b = c % 2;
            naive_buffers[b] = cost;
            if cost < naive_best {
                naive_best = cost;
                naive_idx = b;
            }
        }
        assert_eq!(naive_idx, 0, "the naive walk's winner is in buffer 0");
        assert_ne!(
            naive_buffers[naive_idx], naive_best,
            "and that buffer no longer holds the winner"
        );
    }

    /// Every candidate improving means the toggle runs every time.
    #[test]
    fn a_monotone_improving_sequence_alternates_buffers() {
        let costs = [40u64, 30, 20, 10];
        let mut seen = Vec::new();
        let r = md_stage_0_light_pd1(costs.len(), |c, b| {
            seen.push(b);
            costs[c]
        });
        assert_eq!(seen, vec![0, 1, 0, 1]);
        assert_eq!(r.best_buffer_idx, 1);
        assert_eq!(r.best_cand_idx, Some(3));
    }

    /// No candidates at all leaves the sentinel in place (C never enters
    /// the loop, and `mds0_best_cost` stays `(uint64_t)~0`).
    #[test]
    fn an_empty_candidate_list_keeps_the_sentinel() {
        let r = md_stage_0_light_pd1(0, |_, _| unreachable!());
        assert_eq!(r.best_cost, u64::MAX);
        assert_eq!(r.best_cand_idx, None);
    }

    /// `:1022-1023` — `dc_only_th` is the threshold for a NON-DC mode.
    #[test]
    fn the_dc_and_non_dc_thresholds_are_the_other_way_round() {
        let elim = CandEliminationCtrls {
            enabled: true,
            dc_only_th: 10,
            skip_dc_th: 100,
        };
        let best = Some(Mds0Best {
            cost: 1,
            luma_fast_dist: 500,
        });
        // area 64: non-DC th 640 > 500 -> eliminated; DC th 6400 > 500 too.
        assert!(intra_candidate_eliminated(&elim, best, true, false, (8, 8)));
        assert!(intra_candidate_eliminated(&elim, best, true, true, (8, 8)));
        // area 16: non-DC th 160 < 500 -> kept; DC th 1600 > 500 -> dropped.
        assert!(!intra_candidate_eliminated(
            &elim,
            best,
            true,
            false,
            (4, 4)
        ));
        assert!(intra_candidate_eliminated(&elim, best, true, true, (4, 4)));
    }

    /// `:1018` — inter candidates and a disabled control never eliminate,
    /// and neither does the first candidate (no best yet).
    #[test]
    fn the_intra_elimination_needs_all_three_preconditions() {
        let elim = CandEliminationCtrls {
            enabled: true,
            dc_only_th: 1_000_000,
            skip_dc_th: 1_000_000,
        };
        let best = Some(Mds0Best {
            cost: 1,
            luma_fast_dist: 1,
        });
        assert!(intra_candidate_eliminated(&elim, best, true, false, (8, 8)));
        assert!(!intra_candidate_eliminated(
            &elim,
            best,
            false,
            false,
            (8, 8)
        ));
        let off = CandEliminationCtrls {
            enabled: false,
            ..elim
        };
        assert!(!intra_candidate_eliminated(&off, best, true, false, (8, 8)));
        assert!(!intra_candidate_eliminated(
            &elim,
            None,
            true,
            false,
            (8, 8)
        ));
    }

    /// `:1044-1046` — the stored distortion is the RAW variance and
    /// `full_dist` is the SSE, which are different numbers.
    #[test]
    fn the_stored_distortion_is_unshifted_and_full_dist_is_the_sse() {
        // 4x4: prediction constant 100, source constant 110 -> every diff
        // is -10, so sse = 16 * 100 = 1600 and variance = 1600 - (-160)^2/16
        // = 1600 - 1600 = 0.
        let pred = [100u8; 16];
        let src = [110u8; 16];
        let out = fast_loop_core_light_pd1(
            None,
            (4, 4),
            0,
            true,
            plane(&src, 4),
            plane(&pred, 4),
            |_| unreachable!("shut_fast_rate skips the rate function"),
        );
        let FastLoopOutcome::Scored(s) = out else {
            panic!("must score");
        };
        assert_eq!(s.luma_fast_dist, 0, "constant offset has zero variance");
        assert_eq!(s.full_dist, 1600, "but a real SSE");
        assert_eq!(s.fast_cost, 0, "shut_fast_rate -> cost is the shifted dist");
    }

    /// `:1058-1061` — `shut_fast_rate` makes the cost the SHIFTED
    /// distortion directly, with no `RDCOST` wrapper.
    #[test]
    fn shut_fast_rate_uses_the_shifted_distortion_as_the_cost() {
        // 2x2 with a checkerboard: pred {0,255,255,0}, src all 0.
        let pred = [0u8, 255, 255, 0];
        let src = [0u8; 4];
        let out = fast_loop_core_light_pd1(
            None,
            (2, 2),
            123,
            true,
            plane(&src, 2),
            plane(&pred, 2),
            |_| unreachable!(),
        );
        let FastLoopOutcome::Scored(s) = out else {
            panic!("must score");
        };
        assert_eq!(s.fast_cost, s.luma_fast_dist << 4);
        assert_ne!(s.fast_cost, 0, "the control must not be vacuous");
    }

    /// `:1049-1054` — the distortion-only cost is compared against the
    /// running best TOTAL, and the eliminated candidate reports the
    /// sentinel.
    #[test]
    fn a_candidate_whose_distortion_alone_loses_is_abandoned() {
        let pred = [0u8, 255, 255, 0];
        let src = [0u8; 4];
        let best = Some(Mds0Best {
            cost: 1,
            luma_fast_dist: 0,
        });
        let out = fast_loop_core_light_pd1(
            best,
            (2, 2),
            0,
            false,
            plane(&src, 2),
            plane(&pred, 2),
            |_| unreachable!("an eliminated candidate is never priced"),
        );
        assert_eq!(
            out,
            FastLoopOutcome::Eliminated {
                reason: EliminationReason::DistortionAloneExceedsTheBest
            }
        );
        assert_eq!(out.cost(), MAX_MODE_COST);
        // A generous best keeps it.
        let generous = Some(Mds0Best {
            cost: u64::MAX / 2,
            luma_fast_dist: 0,
        });
        let kept = fast_loop_core_light_pd1(
            generous,
            (2, 2),
            0,
            false,
            plane(&src, 2),
            plane(&pred, 2),
            |d| (d + 7, 3, 0),
        );
        let FastLoopOutcome::Scored(s) = kept else {
            panic!("must score");
        };
        assert_eq!(s.fast_cost, (s.luma_fast_dist << 4) + 7);
        assert_eq!(s.fast_luma_rate, 3);
    }

    /// `:7127-7130` — the RDOQ switches are cleared ONLY under
    /// `bypass_encdec`.
    #[test]
    fn mds3_clears_the_rdoq_switches_only_when_encdec_is_bypassed() {
        let bypassed = md_stage_3_light_pd1_settings(true, true, true);
        assert!(!bypassed.rdoq_skip_uv && !bypassed.rdoq_dct_dct_only);
        let normal = md_stage_3_light_pd1_settings(false, true, true);
        assert!(normal.rdoq_skip_uv && normal.rdoq_dct_dct_only);
        // The rest is unconditional.
        for s in [bypassed, normal] {
            assert!(s.mds_do_chroma && s.uv_intra_comp_only && s.mds_do_rdoq);
            assert_eq!(s.mds_fast_coeff_est_level, 1);
            assert_eq!(s.mds_subres_step, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// full_loop_core_light_pd1 (:6541-6694) — the decision content
// ---------------------------------------------------------------------------

/// C's "the luma transform produced nothing" state, written in TWO places
/// with the SAME constants: the `perform_tx == false` arm of
/// `full_loop_core_light_pd1` (`:6569-6579`) and the `eob == 0` early
/// return of `perform_dct_dct_tx_light_pd1` (`:5497-5507`).
///
/// The `6000` is not a distortion or a rate estimate — it is C's flat
/// stand-in for the cost of signalling an all-zero luma block, the same
/// literal as `INIT_BIT_EST` (`:46`) though C does not use the macro here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LumaTxSkipped {
    /// `cand_bf->eob.y[0]`, `quant_dc.y[0]`, `y_has_coeff` — all cleared.
    pub y_has_coeff: bool,
    /// Both `y_full_distortion[DIST_SSD][..]` entries.
    pub y_dist_residual: u64,
    pub y_dist_prediction: u64,
    /// `*y_coeff_bits`.
    pub y_coeff_bits: u64,
    /// `transform_type[0]` is forced to DCT_DCT, and on an INTER candidate
    /// `transform_type_uv` follows luma.
    pub force_uv_dct_dct: bool,
}

/// The state C installs when the luma transform is skipped or produced no
/// coefficients (`:6569-6579` / `:5497-5507`).
#[must_use]
pub fn luma_tx_skipped(is_inter_mode: bool) -> LumaTxSkipped {
    LumaTxSkipped {
        y_has_coeff: false,
        y_dist_residual: 0,
        y_dist_prediction: 0,
        y_coeff_bits: 6000,
        force_uv_dct_dct: is_inter_mode,
    }
}

/// C `:5495-5496` — the `eob == 0` early return inside
/// `perform_dct_dct_tx_light_pd1` fires only at particular
/// coefficient-rate-estimation levels.
///
/// The level test is `>= 2 || == 0`, i.e. every level EXCEPT 1. Reading it
/// as "level >= 2" drops the `coeff_rate_est_lvl == 0` case, where C also
/// takes the exit.
#[must_use]
pub fn luma_eob_zero_takes_the_early_exit(eob_y: u16, coeff_rate_est_lvl: u8) -> bool {
    eob_y == 0 && (coeff_rate_est_lvl >= 2 || coeff_rate_est_lvl == 0)
}

/// C `:6588-6589` — the four preconditions on the luma-RD skip gate.
///
/// The gate itself is [`super::lpd1::blk_skip_luma_rd`]; this is the
/// guard that decides whether to consult it at all. It is INTER-only, and
/// it requires that the transform actually ran AND kept coefficients —
/// there is nothing to skip otherwise.
#[must_use]
pub fn luma_skip_rd_applies(
    blk_skip_luma_rd_pct: u64,
    perform_tx: bool,
    block_has_coeff: bool,
    is_inter_mode: bool,
) -> bool {
    blk_skip_luma_rd_pct != 0 && perform_tx && block_has_coeff && is_inter_mode
}

/// The chroma bookkeeping `full_loop_core_light_pd1` performs between the
/// luma transform and the chroma one (`:6583-6631`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromaPlan {
    /// C's local `perform_chroma`.
    pub perform_chroma: bool,
    /// C's local `chroma_component` — which planes the chroma transform
    /// runs over.
    pub chroma_component: ComponentType,
    /// C `ctx->chroma_complexity`.
    pub chroma_complexity: ComponentType,
    /// C `ctx->lpd1_chroma_comp` — which planes the PREDICTOR must
    /// produce, which is not the same set.
    pub lpd1_chroma_comp: ComponentType,
    /// Clear `u_has_coeff` / `eob.u[0]` / the Cb distortions and bits.
    pub zero_cb: bool,
    /// The Cr twin.
    pub zero_cr: bool,
}

/// C `full_loop_core_light_pd1`'s chroma decisions (`:6583-6631`).
///
/// `detect` is C's `chroma_complexity_check`
/// ([`super::lpd1::chroma_complexity_check`]); it is a closure because C
/// only calls it on the path where chroma was about to be skipped, and it
/// is expensive.
///
/// Four things a paraphrase loses:
///
/// * **`chroma_complexity` is only updated at detector levels 1..=3**
///   (`:6602`). At level 4 the detector still decides `chroma_component`
///   but `ctx->chroma_complexity` stays `COMPONENT_LUMA`, which changes
///   what the LATER `chroma_complexity_check_pred` call sees.
/// * **The zeroing rules are crossed.** `COMPONENT_CHROMA_CB` means "Cb is
///   the interesting plane", so it zeroes **Cr** (`:6610`), and vice versa
///   (`:6617`). Reading the constant as naming the plane to clear inverts
///   both.
/// * **`lpd1_chroma_comp` is not `chroma_component`.** It is what the
///   PREDICTOR must produce: `COMPONENT_CHROMA` whenever a recon is needed
///   (`:6625`), and also whenever chroma runs at a detector level <= 3
///   (`:6631`), because the later `chroma_complexity_check_pred` reads
///   prediction samples for both planes.
/// * **`perform_chroma` starts true when the luma block coded anything**,
///   and `zero_y_coeff_exit` is what makes an all-zero luma block skip
///   chroma at all (`:6587`).
#[must_use]
pub fn plan_chroma(
    luma_has_coeff: bool,
    zero_y_coeff_exit: bool,
    luma_skip_committed: bool,
    chroma_detector_level: u8,
    recon_needed: bool,
    detect: impl FnOnce() -> ComponentType,
) -> ChromaPlan {
    let mut perform_chroma = luma_has_coeff || !zero_y_coeff_exit;
    if luma_skip_committed {
        perform_chroma = false;
    }

    let mut chroma_component = ComponentType::Chroma;
    let mut chroma_complexity = ComponentType::Luma;
    let (mut zero_cb, mut zero_cr) = (false, false);

    if !perform_chroma {
        if chroma_detector_level != 0 {
            chroma_component = detect();
            if chroma_detector_level <= 3 {
                chroma_complexity = chroma_component;
            }
        } else {
            chroma_component = ComponentType::Luma;
        }
        perform_chroma = chroma_component != ComponentType::Luma;
        // C compares `chroma_component > COMPONENT_LUMA` on the raw enum,
        // and LUMA is 0 while every other value is positive, so the
        // inequality is exactly "not LUMA".
        if matches!(chroma_component, ComponentType::Cb | ComponentType::Luma) {
            zero_cr = true;
        }
        if matches!(chroma_component, ComponentType::Cr | ComponentType::Luma) {
            zero_cb = true;
        }
    }

    let mut lpd1_chroma_comp = if recon_needed {
        ComponentType::Chroma
    } else {
        chroma_component
    };
    if perform_chroma && !recon_needed && chroma_detector_level <= 3 {
        lpd1_chroma_comp = ComponentType::Chroma;
    }

    ChromaPlan {
        perform_chroma,
        chroma_component,
        chroma_complexity,
        lpd1_chroma_comp,
        zero_cb,
        zero_cr,
    }
}

/// C `:6652-6656` — whether the SECOND chroma detector
/// ([`super::lpd1::chroma_complexity_check_pred`], with `use_var = 0`)
/// runs before the chroma transform.
///
/// It is skipped once `chroma_complexity` already says `COMPONENT_CHROMA`
/// — there is nothing left to learn — and it needs one of the two
/// shortcut signals to be armed, because its only consumer is the
/// shortcut path.
#[must_use]
pub fn second_chroma_detector_runs(
    chroma_component: ComponentType,
    chroma_detector_level: u8,
    chroma_complexity: ComponentType,
    use_tx_shortcuts_mds3: bool,
    use_uv_shortcuts_on_y_coeffs: bool,
) -> bool {
    chroma_component != ComponentType::Luma
        && chroma_detector_level != 0
        && chroma_detector_level <= 3
        && chroma_complexity != ComponentType::Chroma
        && (use_tx_shortcuts_mds3 || use_uv_shortcuts_on_y_coeffs)
}

/// C `:6684-6693` — what happens when chroma is skipped entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoChromaEpilogue {
    /// Run the predictor anyway, purely to produce recon samples.
    pub predict_chroma_for_recon: bool,
    /// `u_has_coeff = v_has_coeff = 0`.
    pub clear_chroma_coeff: bool,
    /// `cand->block_mi.skip_mode = true` — only when the candidate allows
    /// it.
    pub set_skip_mode: bool,
}

/// C `full_loop_core_light_pd1`'s `else` branch (`:6684-6693`).
///
/// Note the predictor still runs when `lpd1_chroma_comp` is not
/// `COMPONENT_LUMA`: the block is not coding chroma, but the RECON needs
/// chroma samples, and [`plan_chroma`] has already set
/// `lpd1_chroma_comp` to `COMPONENT_CHROMA` in exactly that case.
#[must_use]
pub fn no_chroma_epilogue(
    lpd1_chroma_comp: ComponentType,
    skip_mode_allowed: bool,
) -> NoChromaEpilogue {
    NoChromaEpilogue {
        predict_chroma_for_recon: lpd1_chroma_comp != ComponentType::Luma,
        clear_chroma_coeff: true,
        set_skip_mode: skip_mode_allowed,
    }
}

#[cfg(test)]
mod full_loop_tests {
    use super::*;

    /// `:6569-6579` and `:5497-5507` install the same state.
    #[test]
    fn the_skipped_luma_state_is_the_same_in_both_places() {
        let inter = luma_tx_skipped(true);
        assert_eq!(inter.y_coeff_bits, 6000);
        assert!(!inter.y_has_coeff);
        assert!(inter.force_uv_dct_dct);
        // Intra keeps its own chroma transform type.
        assert!(!luma_tx_skipped(false).force_uv_dct_dct);
    }

    /// `:5496` — every level EXCEPT 1 takes the exit.
    #[test]
    fn the_eob_zero_exit_covers_level_zero_as_well_as_two_and_up() {
        for lvl in [0u8, 2, 3, 4] {
            assert!(luma_eob_zero_takes_the_early_exit(0, lvl), "lvl {lvl}");
        }
        assert!(!luma_eob_zero_takes_the_early_exit(0, 1));
        // A nonzero eob never takes it.
        assert!(!luma_eob_zero_takes_the_early_exit(1, 0));
    }

    /// `:6588-6589` — four preconditions, each necessary.
    #[test]
    fn the_luma_skip_rd_gate_needs_all_four_preconditions() {
        assert!(luma_skip_rd_applies(50, true, true, true));
        assert!(!luma_skip_rd_applies(0, true, true, true));
        assert!(!luma_skip_rd_applies(50, false, true, true));
        assert!(!luma_skip_rd_applies(50, true, false, true));
        assert!(!luma_skip_rd_applies(50, true, true, false), "intra");
    }

    /// `:6587` — a luma block with coefficients always runs chroma, and
    /// the detector is never consulted.
    #[test]
    fn a_coded_luma_block_runs_chroma_without_detection() {
        let p = plan_chroma(true, true, false, 3, false, || {
            unreachable!("the detector must not run")
        });
        assert!(p.perform_chroma);
        assert_eq!(p.chroma_component, ComponentType::Chroma);
        assert_eq!(p.chroma_complexity, ComponentType::Luma);
        assert!(!p.zero_cb && !p.zero_cr);
    }

    /// `:6587` — with `zero_y_coeff_exit` off, an all-zero luma block
    /// still runs chroma.
    #[test]
    fn zero_y_coeff_exit_is_what_makes_an_empty_luma_block_skip_chroma() {
        let kept = plan_chroma(false, false, false, 3, false, || unreachable!());
        assert!(kept.perform_chroma);
        let skipped = plan_chroma(false, true, false, 0, false, || unreachable!());
        assert!(!skipped.perform_chroma);
        assert_eq!(skipped.chroma_component, ComponentType::Luma);
        assert!(skipped.zero_cb && skipped.zero_cr);
    }

    /// `:6610-6623` — the zeroing is CROSSED: `Cb` means "keep Cb", so it
    /// clears Cr.
    #[test]
    fn the_zeroing_clears_the_plane_the_component_does_not_name() {
        let cb = plan_chroma(false, true, false, 1, false, || ComponentType::Cb);
        assert!(cb.perform_chroma);
        assert!(cb.zero_cr && !cb.zero_cb);
        let cr = plan_chroma(false, true, false, 1, false, || ComponentType::Cr);
        assert!(cr.zero_cb && !cr.zero_cr);
        let both = plan_chroma(false, true, false, 1, false, || ComponentType::Chroma);
        assert!(!both.zero_cb && !both.zero_cr);
    }

    /// `:6602` — `chroma_complexity` tracks the detector only at levels
    /// 1..=3.
    #[test]
    fn chroma_complexity_is_recorded_only_below_level_four() {
        for lvl in [1u8, 2, 3] {
            let p = plan_chroma(false, true, false, lvl, false, || ComponentType::Cb);
            assert_eq!(p.chroma_complexity, ComponentType::Cb, "lvl {lvl}");
        }
        let high = plan_chroma(false, true, false, 4, false, || ComponentType::Cb);
        assert_eq!(high.chroma_component, ComponentType::Cb);
        assert_eq!(
            high.chroma_complexity,
            ComponentType::Luma,
            "level 4 decides but does not record"
        );
    }

    /// `:6625` / `:6631` — the PREDICTOR's component set is not the
    /// transform's.
    #[test]
    fn the_predictor_component_set_is_wider_than_the_transform_one() {
        // Recon needed: always the full pair, whatever the transform does.
        let recon = plan_chroma(false, true, false, 1, true, || ComponentType::Cb);
        assert_eq!(recon.chroma_component, ComponentType::Cb);
        assert_eq!(recon.lpd1_chroma_comp, ComponentType::Chroma);
        // No recon, chroma runs, detector level <= 3: still the full pair,
        // because the second detector reads both planes' predictions.
        let widened = plan_chroma(false, true, false, 3, false, || ComponentType::Cb);
        assert_eq!(widened.lpd1_chroma_comp, ComponentType::Chroma);
        // No recon, chroma runs, level 4: the predictor follows the
        // transform.
        let narrow = plan_chroma(false, true, false, 4, false, || ComponentType::Cb);
        assert_eq!(narrow.lpd1_chroma_comp, ComponentType::Cb);
        // Chroma fully skipped and no recon: nothing to predict.
        let none = plan_chroma(false, true, false, 0, false, || unreachable!());
        assert_eq!(none.lpd1_chroma_comp, ComponentType::Luma);
    }

    /// `:6588-6592` — a committed luma skip overrides a coded luma block.
    #[test]
    fn a_committed_luma_skip_forces_the_chroma_decision_back_through_the_detector() {
        let p = plan_chroma(true, true, true, 1, false, || ComponentType::Luma);
        assert!(!p.perform_chroma);
        assert!(p.zero_cb && p.zero_cr);
    }

    /// `:6652-6654` — five conditions on the second detector.
    #[test]
    fn the_second_detector_needs_every_condition() {
        let ok = |comp, lvl, cplx, mds3, uv| second_chroma_detector_runs(comp, lvl, cplx, mds3, uv);
        assert!(ok(ComponentType::Cb, 2, ComponentType::Luma, true, false));
        assert!(ok(ComponentType::Cb, 2, ComponentType::Luma, false, true));
        assert!(!ok(ComponentType::Luma, 2, ComponentType::Luma, true, true));
        assert!(!ok(ComponentType::Cb, 0, ComponentType::Luma, true, true));
        assert!(!ok(ComponentType::Cb, 4, ComponentType::Luma, true, true));
        assert!(
            !ok(ComponentType::Cb, 2, ComponentType::Chroma, true, true),
            "already fully complex"
        );
        assert!(!ok(ComponentType::Cb, 2, ComponentType::Luma, false, false));
    }

    /// `:6684-6693` — chroma prediction still runs for the recon.
    #[test]
    fn the_no_chroma_epilogue_still_predicts_for_a_recon() {
        let with_recon = no_chroma_epilogue(ComponentType::Chroma, true);
        assert!(with_recon.predict_chroma_for_recon);
        assert!(with_recon.clear_chroma_coeff && with_recon.set_skip_mode);
        let without = no_chroma_epilogue(ComponentType::Luma, false);
        assert!(!without.predict_chroma_for_recon);
        assert!(
            !without.set_skip_mode,
            "skip_mode needs the candidate's consent"
        );
    }
}

// ---------------------------------------------------------------------------
// read_refine_me_mvs_light_pd1 (:2737-2811) — the decision content
// ---------------------------------------------------------------------------

/// C `read_refine_me_mvs_light_pd1`'s index into
/// `me_results->me_mv_array` (`:2767-2769`).
///
/// The base is `me_block_offset * max_refs + ref_idx`, and the LIST is
/// selected by adding `max_l0` for list 1 — the two lists are laid out
/// consecutively per block, not interleaved. Reading this as
/// `+ list * max_refs` (the obvious alternative) indexes another block's
/// row entirely.
#[must_use]
pub fn lpd1_me_mv_index(
    me_block_offset: usize,
    max_refs: usize,
    ref_idx: usize,
    list_idx: usize,
    max_l0: usize,
) -> usize {
    me_block_offset * max_refs + ref_idx + if list_idx != 0 { max_l0 } else { 0 }
}

/// C `:2770` — the ME array holds FULL-PEL motion vectors; MD works in
/// eighth-pel.
///
/// C stores the product back into an `int16_t`, so a full-pel component
/// beyond +-4095 truncates. That cannot happen for a real ME result
/// (`MV_UPP` is 1 << 14 in eighth-pel, i.e. 2048 full-pel), and the
/// wrapping multiply says "C's semantics" rather than permitting it.
#[must_use]
pub fn lpd1_me_mv_to_eighth_pel(full_pel: Mv) -> Mv {
    Mv {
        x: full_pel.x.wrapping_mul(8),
        y: full_pel.y.wrapping_mul(8),
    }
}

/// C `:2772-2774` — when the sub-pel refinement is skipped for this
/// candidate even though sub-pel search is enabled.
///
/// Two independent reasons: the block sits inside an intra-predicted
/// border and the neighbouring-mode reduction is on (C's comment notes
/// this is only safe because that path codes DC only, so no candidate
/// cost is needed), or the block is at or below the configured minimum
/// sub-pel size.
///
/// Note the size test is `<=`, so `min_blk_sz` is the largest size that is
/// SKIPPED, not the smallest that is searched.
#[must_use]
pub fn lpd1_skip_subpel(
    is_intra_bordered: bool,
    use_neighbouring_mode_enabled: bool,
    sq_size: usize,
    subpel_min_blk_sz: usize,
) -> bool {
    (is_intra_bordered && use_neighbouring_mode_enabled) || sq_size <= subpel_min_blk_sz
}

/// The running `ctx->md_me_dist` (`:2740`, `:2796-2798`) — the cheapest
/// post-sub-pel ME cost over every reference this block examined.
///
/// C initialises it to `(uint32_t)~0` and only ever lowers it, and only
/// from candidates that actually ran the sub-pel search. A block whose
/// references all skipped sub-pel therefore leaves the sentinel in place,
/// which downstream code tests for — so this is an [`Option`] rather than
/// a `u32` pre-set to `MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdMeDist(Option<u32>);

impl MdMeDist {
    /// C `:2740`.
    #[must_use]
    pub fn new() -> Self {
        MdMeDist(None)
    }
    /// C `:2796-2798`.
    pub fn observe(&mut self, post_subpel_cost: u32) {
        self.0 = Some(match self.0 {
            Some(best) => best.min(post_subpel_cost),
            None => post_subpel_cost,
        });
    }
    /// The value C's `ctx->md_me_dist` holds, sentinel included.
    #[must_use]
    pub fn raw(self) -> u32 {
        self.0.unwrap_or(u32::MAX)
    }
    /// `None` when no reference ran the sub-pel search.
    #[must_use]
    pub fn get(self) -> Option<u32> {
        self.0
    }
}

#[cfg(test)]
mod lpd1_me_tests {
    use super::*;

    /// `:2767-2769` — list 1 is `+ max_l0`, not `+ list * max_refs`.
    #[test]
    fn the_list_offset_is_max_l0_not_a_stride() {
        // block 3, max_refs 8, ref 2: base 26.
        assert_eq!(lpd1_me_mv_index(3, 8, 2, 0, 4), 26);
        assert_eq!(lpd1_me_mv_index(3, 8, 2, 1, 4), 30);
        // The alternative reading would give 26 + 8 = 34, a different
        // block's row.
        assert_ne!(lpd1_me_mv_index(3, 8, 2, 1, 4), 34);
    }

    /// `:2770` — full-pel to eighth-pel.
    #[test]
    fn the_me_mv_is_scaled_to_eighth_pel() {
        let got = lpd1_me_mv_to_eighth_pel(Mv { x: -3, y: 7 });
        assert_eq!((got.x, got.y), (-24, 56));
    }

    /// `:2772-2774` — two independent reasons, and the size test is `<=`.
    #[test]
    fn subpel_is_skipped_for_either_reason() {
        assert!(lpd1_skip_subpel(true, true, 64, 8));
        assert!(!lpd1_skip_subpel(true, false, 64, 8), "needs both flags");
        assert!(!lpd1_skip_subpel(false, true, 64, 8));
        assert!(lpd1_skip_subpel(false, false, 8, 8), "<= is inclusive");
        assert!(!lpd1_skip_subpel(false, false, 16, 8));
    }

    /// `:2740` / `:2796-2798` — the sentinel survives a block that never
    /// ran the sub-pel search.
    #[test]
    fn md_me_dist_keeps_its_sentinel_until_a_subpel_search_runs() {
        let mut d = MdMeDist::new();
        assert_eq!(d.get(), None);
        assert_eq!(d.raw(), u32::MAX);
        d.observe(500);
        d.observe(700);
        d.observe(400);
        assert_eq!(d.get(), Some(400));
        assert_eq!(d.raw(), 400);
    }
}

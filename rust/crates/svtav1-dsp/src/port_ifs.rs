//! The interpolation-filter search, and the two thin MC wrappers around it.
//!
//! Ported from `Source/Lib/Codec/enc_inter_prediction.c` (SVT-AV1 v4.2.0):
//! `filter_sets` (:2042), `interpolation_filter_search` (:2058) and
//! `svt_aom_simple_luma_unipred` (:2677).
//!
//! `interp_filters` is WRITTEN INTO THE BITSTREAM per inter block, so a wrong
//! choice here desyncs syntax and pixels even when every convolve kernel below
//! is bit-exact.
//!
//! # Evidence
//!
//! TIER 4. `interpolation_filter_search` takes a `PictureControlSet`, a
//! `ModeDecisionContext` and a `ModeDecisionCandidateBuffer`, calls
//! `svt_aom_inter_prediction` and `model_rd_for_sb` on the encoder's scratch
//! buffers, and reads three `NeighborArrayUnit`s. Nothing a shim can
//! synthesise. What IS ported here is the decision structure: the candidate
//! set, the dual-filter gate, the full-pel bypass, the two RD biases and the
//! tie-break — with the per-candidate rate and distortion supplied by the
//! caller, so the arithmetic that decides the syntax element is expressible
//! and testable even though the encoder plumbing is not.
//!
//! `svt_aom_simple_luma_unipred` is exported but is a five-line wrapper whose
//! whole body is one `tf_inter_predictor` call, and THAT is tier-1 gated
//! (`c_parity_port_subpel_params.rs`). It is ported here as the wrapper it is.

use crate::port_convolve::InterpFilterKind;
use crate::port_inter_predictor::{InterpFilters, make_interp_filters};
use crate::port_model_rd::rdcost;

/// `DUAL_FILTER_SET_SIZE` — the 3x3 (x, y) filter pairs the search ranges over.
pub const DUAL_FILTER_SET_SIZE: usize = 9;

/// `filter_sets` (enc_inter_prediction.c:2042) — `[x_filter, y_filter]`.
///
/// The order matters: the tie-break keeps the FIRST pair that strictly beats
/// the running best, so `{0,0}` wins any tie.
pub const FILTER_SETS: [[usize; 2]; DUAL_FILTER_SET_SIZE] = [
    [0, 0],
    [0, 1],
    [0, 2],
    [1, 0],
    [1, 1],
    [1, 2],
    [2, 0],
    [2, 1],
    [2, 2],
];

/// `ifs_smooth_bias` (enc_inter_prediction.c:2086) — indexed by picture QP,
/// four flat runs of 16.
pub const IFS_SMOOTH_BIAS: [u32; 64] = [
    130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 130, 120, 120, 120,
    120, 120, 120, 120, 120, 120, 120, 120, 120, 120, 120, 120, 120, 110, 110, 110, 110, 110, 110,
    110, 110, 110, 110, 110, 110, 110, 110, 110, 110, 100, 100, 100, 100, 100, 100, 100, 100, 100,
    100, 100, 100, 100, 100, 100, 100,
];

fn kind(i: usize) -> InterpFilterKind {
    match i {
        0 => InterpFilterKind::EightTapRegular,
        1 => InterpFilterKind::EightTapSmooth,
        2 => InterpFilterKind::MultiTapSharp,
        _ => unreachable!("filter_sets only holds 0..=2"),
    }
}

/// `is_fp` (enc_inter_prediction.c:2076) — the full-pel bypass.
///
/// TRAP: the test is `mv % 8 == 0` on BOTH components of BOTH references AND
/// `is_not_scaled`. It is `% 8` because MVs are eighth-pel, and it is the
/// C-remainder (which is negative for negative MVs) — but `x % 8 == 0` is
/// sign-agnostic, so a Rust `%` matches. When it holds, the interpolation
/// filter cannot affect the prediction, so the search runs on RATE ALONE and
/// the existing prediction stays valid.
pub fn is_full_pel(mv0: (i32, i32), mv1: Option<(i32, i32)>, is_not_scaled: bool) -> bool {
    let whole = |m: (i32, i32)| m.0 % 8 == 0 && m.1 % 8 == 0;
    whole(mv0) && mv1.is_none_or(whole) && is_not_scaled
}

/// The knobs `interpolation_filter_search` reads out of the encoder.
#[derive(Debug, Clone, Copy)]
pub struct IfsCtrls {
    /// `scs->seq_header.enable_dual_filter`. When 0, only the three pairs with
    /// matching x and y filters are tested.
    pub enable_dual_filter: bool,
    /// `scs->vq_ctrls.sharpness_ctrls.ifs && pcs->ppcs->is_noise_level`.
    pub smooth_bias: bool,
    /// `pcs->ppcs->picture_qp`, the index into [`IFS_SMOOTH_BIAS`].
    pub picture_qp: usize,
    /// `scs->static_config.tx_bias > 0`.
    pub tx_bias: bool,
    /// `full_lambda_divided`.
    pub full_lambda: u32,
}

/// One candidate's cost inputs, as the caller measures them.
#[derive(Debug, Clone, Copy)]
pub struct IfsCandidateCost {
    /// `tmp_rs` — `svt_aom_get_switchable_rate` for this filter pair.
    pub switchable_rate: i32,
    /// `tmp_rate` from `model_rd_for_sb`. Ignored on the full-pel path.
    pub rate: i32,
    /// `tmp_dist` from `model_rd_for_sb`. Ignored on the full-pel path.
    pub dist: i64,
}

/// What the search decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfsResult {
    /// The packed `interp_filters` written back into the candidate.
    pub best_filters: InterpFilters,
    /// `switchable_rate` of the winning pair, added to `fast_luma_rate`.
    pub switchable_rate: i32,
    /// Whether the luma prediction must be re-done
    /// (`!is_fp && org_interp_filters != best_filters`).
    pub invalidates_luma_pred: bool,
}

/// `interpolation_filter_search` (enc_inter_prediction.c:2058), decision half.
///
/// `cost(i)` supplies the per-candidate rate and distortion for
/// `FILTER_SETS[i]`; C measures those with `svt_aom_get_switchable_rate` and
/// `model_rd_for_sb` around a `svt_aom_inter_prediction` call.
///
/// The two RD biases are applied in C's order and with C's integer division:
/// the smooth bias multiplies by `ifs_smooth_bias[qp] / 100` when EITHER axis
/// is SMOOTH, then the tx bias multiplies by `75/100` when either axis is
/// SHARP and — separately, so BOTH can apply — by `80/100` when either is
/// REGULAR. `{0,2}` therefore takes both tx-bias multiplies.
pub fn interpolation_filter_search<F>(
    ctrls: &IfsCtrls,
    org_interp_filters: InterpFilters,
    is_fp: bool,
    mut cost: F,
) -> IfsResult
where
    F: FnMut(usize, InterpFilters) -> IfsCandidateCost,
{
    let mut rd = u64::MAX;
    let mut switchable_rate = 0i32;
    let mut best_filters = 0u32;

    for (i, set) in FILTER_SETS.iter().enumerate() {
        if !ctrls.enable_dual_filter && set[0] != set[1] {
            continue;
        }
        // NOTE the argument order: av1_make_interp_filters takes (y, x), and C
        // passes (filter_sets[i][0], filter_sets[i][1]) — so set[0] lands in
        // the Y half and set[1] in the X half.
        let filters = make_interp_filters(kind(set[0]), kind(set[1]));
        let c = cost(i, filters);
        let mut tmp_rd = if is_fp {
            rdcost(ctrls.full_lambda, c.switchable_rate as i64, 0) as u64
        } else {
            rdcost(
                ctrls.full_lambda,
                (c.switchable_rate + c.rate) as i64,
                c.dist,
            ) as u64
        };

        if ctrls.smooth_bias && (set[0] == 1 || set[1] == 1) {
            tmp_rd = (tmp_rd * IFS_SMOOTH_BIAS[ctrls.picture_qp] as u64) / 100;
        }
        if ctrls.tx_bias {
            if set[0] == 2 || set[1] == 2 {
                tmp_rd = (tmp_rd * 75) / 100;
            }
            // NOT an `else if` in C: {0,2} and {2,0} take both multiplies.
            if set[0] == 0 || set[1] == 0 {
                tmp_rd = (tmp_rd * 80) / 100;
            }
        }

        if tmp_rd < rd {
            rd = tmp_rd;
            switchable_rate = c.switchable_rate;
            best_filters = filters;
        }
    }

    IfsResult {
        best_filters,
        switchable_rate,
        invalidates_luma_pred: !is_fp && org_interp_filters != best_filters,
    }
}

/// `svt_aom_simple_luma_unipred` (enc_inter_prediction.c:2677) — the MC
/// `temporal_filtering.c` uses to build TF references.
///
/// Its whole body is one `tf_inter_predictor` call with `is_compound = 0`, the
/// IDENTITY scale factors, and a CONV_BUF stride of 128. TF alters the SOURCE
/// pixels of the key/alt-ref frame, so video-mode key-frame byte parity
/// depends on this path.
///
/// The port's `temporal_filter.rs` is self-documented as HOMEGROWN, not a
/// port; wiring this in is a caller-side change and is NOT done here.
pub struct SimpleLumaUnipred {
    /// `get_conv_params_no_round(0, tmp_dstY, 128, 0, bit_depth)`.
    pub conv_buf_stride: usize,
    /// `is_compound`, always 0.
    pub is_compound: bool,
}

impl SimpleLumaUnipred {
    /// The fixed parameter set C builds. Kept as a value rather than inlined
    /// so the 128 and the `is_compound = 0` are stated once and cited.
    pub const fn new() -> Self {
        Self {
            conv_buf_stride: 128,
            is_compound: false,
        }
    }

    /// The destination offset C computes:
    /// `(dst_origin_x + dst_origin_y * dst_stride) << is16bit`.
    pub fn dst_offset(
        dst_origin_x: usize,
        dst_origin_y: usize,
        dst_stride: usize,
        is16bit: bool,
    ) -> usize {
        (dst_origin_x + dst_origin_y * dst_stride) << usize::from(is16bit)
    }
}

impl Default for SimpleLumaUnipred {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `enable_dual_filter = 0` reduces the candidate set to the three
    /// diagonal pairs, and the winner must come from that subset.
    #[test]
    fn dual_filter_gate_reduces_the_candidate_set() {
        let mut seen = alloc::vec::Vec::new();
        let ctrls = IfsCtrls {
            enable_dual_filter: false,
            smooth_bias: false,
            picture_qp: 0,
            tx_bias: false,
            full_lambda: 100,
        };
        interpolation_filter_search(&ctrls, 0, true, |i, _| {
            seen.push(i);
            IfsCandidateCost {
                switchable_rate: i as i32,
                rate: 0,
                dist: 0,
            }
        });
        assert_eq!(seen, alloc::vec![0, 4, 8]);
    }

    /// The tie-break is strict `<`, so the FIRST candidate wins a tie — which
    /// with equal costs is `{0,0}`, i.e. `interp_filters == 0`.
    #[test]
    fn ties_go_to_the_first_candidate() {
        let ctrls = IfsCtrls {
            enable_dual_filter: true,
            smooth_bias: false,
            picture_qp: 0,
            tx_bias: false,
            full_lambda: 100,
        };
        let r = interpolation_filter_search(&ctrls, 0, true, |_, _| IfsCandidateCost {
            switchable_rate: 7,
            rate: 0,
            dist: 0,
        });
        assert_eq!(r.best_filters, 0);
        assert_eq!(r.switchable_rate, 7);
        assert!(!r.invalidates_luma_pred);
    }

    /// The tx bias is TWO independent multiplies, not a chain of else-ifs:
    /// `{0,2}` is scaled by 75/100 AND 80/100.
    #[test]
    fn tx_bias_applies_both_multiplies() {
        let ctrls = IfsCtrls {
            enable_dual_filter: true,
            smooth_bias: false,
            picture_qp: 0,
            tx_bias: true,
            full_lambda: 1 << 9, // makes RDCOST(rate) == rate exactly
        };
        // Only {0,0} (index 0) and {0,2} (index 2) are in contention.
        // {0,0}: 100 -> * 80/100 = 80.
        // {0,2}: 130 -> * 75/100 = 97 -> * 80/100 = 77, so it wins.
        // With only ONE of the two multiplies it would be 97 or 104 and
        // {0,0} would win instead.
        let r = interpolation_filter_search(&ctrls, 0, true, |i, _| IfsCandidateCost {
            switchable_rate: match i {
                0 => 100,
                2 => 130,
                _ => 100_000,
            },
            rate: 0,
            dist: 0,
        });
        assert_eq!(
            r.best_filters,
            make_interp_filters(
                InterpFilterKind::EightTapRegular,
                InterpFilterKind::MultiTapSharp
            )
        );
    }

    /// The full-pel test spans both references and the scaling flag.
    #[test]
    fn full_pel_test_covers_both_refs_and_scaling() {
        assert!(is_full_pel((8, -16), None, true));
        assert!(!is_full_pel((8, -16), None, false));
        assert!(!is_full_pel((9, 0), None, true));
        assert!(is_full_pel((8, 8), Some((-24, 0)), true));
        assert!(!is_full_pel((8, 8), Some((-24, 1)), true));
    }

    /// The smooth-bias table is four flat runs of 16.
    #[test]
    fn smooth_bias_table_shape() {
        for qp in 0..64usize {
            let expect = match qp / 16 {
                0 => 130,
                1 => 120,
                2 => 110,
                _ => 100,
            };
            assert_eq!(IFS_SMOOTH_BIAS[qp], expect, "qp {qp}");
        }
    }
}

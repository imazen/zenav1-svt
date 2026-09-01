//! The two tx-bias distortion facades of `Codec/pic_operators.c`.
//!
//! C wraps each distortion kernel in a "facade" that multiplies the result
//! by a set of biases keyed on the candidate's prediction mode, its
//! transform size and the temporal layer:
//!
//! * `svt_spatial_full_distortion_kernel_facade` (pic_operators.c:252) —
//!   one scalar SSE.
//! * `svt_aom_picture_full_distortion32_bits_single_facade`
//!   (pic_operators.c:163) — the `[DIST_CALC_RESIDUAL, DIST_CALC_PREDICTION]`
//!   pair, with the SAME bias applied to both fields.
//!
//! The bias arithmetic was already ported as
//! [`crate::tx_bias::facade_bias`]. What was missing is the MODE
//! CLASSIFICATION that decides which bias applies — C spells it as a nest
//! of `if (is_intra_mode) … else if (is_inter_compound_mode) …` around
//! `mode` / `uv_mode` / `is_interintra_used` / `interinter_comp.type` — and
//! the two compositions themselves. [`classify`] is that nest; the two
//! functions below are the compositions.
//!
//! Correction to an earlier claim: `tx_bias`'s module doc said the bias
//! math was "pinned against the exported C facade with a synthetic
//! BlockModeInfo (`tests/c_parity_ac_bias.rs`)". It was not — that file
//! tests `psy_distortion`, `psy_adjust_rate_light` and
//! `effective_ac_bias`, and never touches either facade.
//! `tests/c_parity_dist_facade.rs`, added with this module, is the
//! differential that claim described.
//!
//! Evidence: tier 1 — both facades are exported and driven directly.

use crate::tx_bias::{BiasModeClass, facade_bias};
use svtav1_dsp::pic_operators::FullDistortion;
use svtav1_types::prediction::{CompoundType, PredictionMode, UvPredictionMode};

/// Everything the facade reads out of C's `BlockModeInfo`.
#[derive(Debug, Clone, Copy)]
pub struct FacadeMode {
    /// C `block_mi->mode`.
    pub mode: PredictionMode,
    /// C `block_mi->uv_mode`.
    pub uv_mode: UvPredictionMode,
    /// C `block_mi->is_interintra_used`.
    pub is_interintra_used: bool,
    /// C `block_mi->interinter_comp.type`.
    pub compound_type: CompoundType,
}

/// C `is_inter_compound_mode` (definitions.h:1622): `NEAREST_NEARESTMV`
/// through `NEW_NEWMV`.
#[must_use]
pub const fn is_inter_compound_mode(mode: PredictionMode) -> bool {
    let m = mode as u8;
    m >= PredictionMode::NearestNearestMv as u8 && m <= PredictionMode::NewNewMv as u8
}

/// The facade's mode nest (pic_operators.c:169-232 / :262-320), returning
/// the bias class and C's `is_intra_mode(mode)`.
///
/// The two arms differ in what they read: the intra arm keys on `mode` for
/// luma and `uv_mode` for chroma; the inter-compound arm ignores both and
/// keys on `is_interintra_used` first, then the compound type. Anything
/// that is neither intra nor inter-compound — a single-reference inter mode
/// — takes no mode bias at all.
#[must_use]
pub fn classify(mi: FacadeMode, is_chroma: bool) -> (BiasModeClass, bool) {
    use PredictionMode as M;
    use UvPredictionMode as U;

    // C `is_intra_mode(mode)`: mode < INTRA_MODE_END (NEARESTMV).
    let is_intra = (mi.mode as u8) < (M::NearestMv as u8);
    if is_intra {
        let class = if is_chroma {
            match mi.uv_mode {
                U::UvDcPred | U::UvSmoothPred | U::UvSmoothVPred | U::UvSmoothHPred => {
                    BiasModeClass::IntraBlurry
                }
                U::UvHPred | U::UvVPred | U::UvPaethPred => BiasModeClass::IntraNeutral,
                _ => BiasModeClass::IntraOther,
            }
        } else {
            match mi.mode {
                M::DcPred | M::SmoothPred | M::SmoothVPred | M::SmoothHPred => {
                    BiasModeClass::IntraBlurry
                }
                M::HPred | M::VPred | M::PaethPred => BiasModeClass::IntraNeutral,
                _ => BiasModeClass::IntraOther,
            }
        };
        return (class, true);
    }

    if is_inter_compound_mode(mi.mode) {
        if mi.is_interintra_used {
            return (BiasModeClass::InterCompoundBlurry, false);
        }
        let class = match mi.compound_type {
            CompoundType::Average | CompoundType::DistWtd => BiasModeClass::InterCompoundBlurry,
            CompoundType::DiffWtd => BiasModeClass::InterCompoundDiffwtd,
            _ => BiasModeClass::Other,
        };
        return (class, false);
    }

    (BiasModeClass::Other, false)
}

/// C `svt_spatial_full_distortion_kernel_facade` (pic_operators.c:252): the
/// spatial SSE with the tx-bias layer applied.
///
/// C selects the kernel on `hbd_md` — `svt_full_distortion_kernel16_bits`
/// when set, `svt_spatial_full_distortion_kernel` when clear. The port takes
/// the already-computed SSE so the caller keeps that choice, which is where
/// the bit-depth decision already lives.
#[must_use]
pub fn spatial_full_distortion_facade(
    spatial_distortion: u64,
    mi: FacadeMode,
    is_chroma: bool,
    area_width: u32,
    area_height: u32,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> u64 {
    let (class, is_intra) = classify(mi, is_chroma);
    facade_bias(
        spatial_distortion as i64,
        class,
        is_intra,
        area_width,
        area_height,
        temporal_layer_index,
        ac_bias,
        tx_bias,
    ) as u64
}

/// C `svt_aom_picture_full_distortion32_bits_single_facade`
/// (pic_operators.c:163): the frequency-domain distortion PAIR with the
/// bias layer applied to each field independently.
///
/// # This is NOT the same function as [`spatial_full_distortion_facade`]
///
/// The two facades are written as twins and are identical line for line —
/// except for ONE brace. In the 32-bit facade the "Transform size related
/// tweaks" block sits INSIDE the outer `if (tx_bias == 1)`
/// (pic_operators.c:170-185, closed at :186); in the spatial facade the
/// same block sits at function scope, outside it (pic_operators.c:381-393).
///
/// Consequence: at `tx_bias == 2` the spatial facade applies the 64x64
/// strong bias and the 32-bit facade applies NOTHING — and the 32-bit
/// facade's own `if (tx_bias == 1 || tx_bias == 2)` is dead, since it can
/// only be reached when `tx_bias == 1`. The port reproduces the asymmetry
/// (`docs/SUSPECTED-C-BUGS.md`: a C bug is still the oracle). It was found
/// by the differential, not by reading: `tests/c_parity_dist_facade.rs`
/// diverged 3:2 on the first `tx_bias == 2`, 64x64, intra cell.
///
/// C applies each multiply to `distortion[DIST_CALC_RESIDUAL]` and
/// `distortion[DIST_CALC_PREDICTION]` in turn, so the two fields cannot
/// share one intermediate — `(a * 5) / 4 + (b * 5) / 4` is not
/// `((a + b) * 5) / 4` under truncating division.
#[must_use]
pub fn picture_full_distortion32_facade(
    dist: FullDistortion,
    mi: FacadeMode,
    is_chroma: bool,
    area_width: u32,
    area_height: u32,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> FullDistortion {
    // The whole body is inside C's `if (tx_bias == 1)`; see above.
    if tx_bias != 1 {
        return dist;
    }
    let (class, is_intra) = classify(mi, is_chroma);
    let apply = |v: u64| {
        facade_bias(
            v as i64,
            class,
            is_intra,
            area_width,
            area_height,
            temporal_layer_index,
            ac_bias,
            1,
        ) as u64
    };
    FullDistortion {
        residual: apply(dist.residual),
        prediction: apply(dist.prediction),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mi(mode: PredictionMode, uv_mode: UvPredictionMode) -> FacadeMode {
        FacadeMode {
            mode,
            uv_mode,
            is_interintra_used: false,
            compound_type: CompoundType::Average,
        }
    }

    #[test]
    fn chroma_reads_uv_mode_and_luma_reads_mode() {
        let m = mi(PredictionMode::DcPred, UvPredictionMode::UvD45Pred);
        assert_eq!(classify(m, false).0, BiasModeClass::IntraBlurry);
        assert_eq!(classify(m, true).0, BiasModeClass::IntraOther);
    }

    #[test]
    fn interintra_wins_over_the_compound_type() {
        let m = FacadeMode {
            mode: PredictionMode::NewNewMv,
            uv_mode: UvPredictionMode::UvDcPred,
            is_interintra_used: true,
            compound_type: CompoundType::DiffWtd,
        };
        assert_eq!(
            classify(m, false),
            (BiasModeClass::InterCompoundBlurry, false)
        );
    }

    #[test]
    fn single_ref_inter_takes_no_mode_bias() {
        let m = mi(PredictionMode::NewMv, UvPredictionMode::UvDcPred);
        assert_eq!(classify(m, false), (BiasModeClass::Other, false));
    }

    #[test]
    fn the_pair_biases_each_field_independently() {
        // DcPred (IntraBlurry), 64x64, ac_bias 0, tx_bias 1: 5/4 then 3/2,
        // truncated SEPARATELY per field.
        //   7 -> (7*5)/4 = 8  -> (8*3)/2  = 12
        //   9 -> (9*5)/4 = 11 -> (11*3)/2 = 16
        let d = FullDistortion {
            residual: 7,
            prediction: 9,
        };
        let m = mi(PredictionMode::DcPred, UvPredictionMode::UvDcPred);
        let out = picture_full_distortion32_facade(d, m, false, 64, 64, 0, 0.0, 1);
        assert_eq!(out.residual, 12);
        assert_eq!(out.prediction, 16);
    }

    #[test]
    fn tx_bias_two_reaches_nothing_in_the_pair_facade_but_does_in_the_spatial_one() {
        // The one-brace asymmetry between C's two facades — see the
        // picture_full_distortion32_facade doc.
        let d = FullDistortion {
            residual: 1000,
            prediction: 1000,
        };
        let m = mi(PredictionMode::DcPred, UvPredictionMode::UvDcPred);
        let pair = picture_full_distortion32_facade(d, m, false, 64, 64, 0, 0.0, 2);
        assert_eq!((pair.residual, pair.prediction), (1000, 1000));
        let spatial = spatial_full_distortion_facade(1000, m, false, 64, 64, 0, 0.0, 2);
        assert_eq!(spatial, 1500);
    }
}

//! Fork mds0 distortion facade — the tx-bias adjustments of
//! `svt_spatial_full_distortion_kernel_facade` (pic_operators.c, fork-only;
//! compiled into both hybrid libs, fires only when `mds0_dist_type != VAR`
//! in SVT_HDR_MODE=1).
//!
//! The facade wraps a plain spatial SSE with multiplicative biases keyed on
//! the candidate's prediction mode. This module ports the BIAS layer as a
//! pure function of (sse, mode-kind, dims, flags) so the caller supplies
//! whatever SSE kernel it already has; parity for the bias math is pinned
//! against the exported C facade with a synthetic BlockModeInfo
//! (`tests/c_parity_ac_bias.rs` companion in svtav1-encoder).
//!
//! Intra-only subset: the port's envelope is all-intra, so the
//! inter-compound branches (interintra / COMPOUND_*) are represented but
//! only the intra paths are reachable today.

/// Which prediction-mode family the candidate belongs to (the facade's
/// bias classes; AV1 PredictionMode values in comments).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiasModeClass {
    /// DC_PRED / SMOOTH_PRED / SMOOTH_V_PRED / SMOOTH_H_PRED
    /// (or the UV_* equivalents when `is_chroma`).
    IntraBlurry,
    /// H_PRED / V_PRED / PAETH_PRED (or UV_*).
    IntraNeutral,
    /// Any other intra directional mode.
    IntraOther,
    /// Inter-compound, interintra or COMPOUND_AVERAGE/DISTWTD.
    InterCompoundBlurry,
    /// Inter-compound COMPOUND_DIFFWTD.
    InterCompoundDiffwtd,
    /// Everything else (plain inter, etc.).
    Other,
}

/// The facade's distortion-bias layer (C pic_operators.c facade body,
/// minus the SSE kernel call). `is_intra` must be true for the three
/// Intra* classes.
#[allow(clippy::too_many_arguments)]
pub fn facade_bias(
    mut spatial_distortion: i64,
    class: BiasModeClass,
    is_intra: bool,
    area_width: u32,
    area_height: u32,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> i64 {
    // Mode-type tweaks: full tx-bias only.
    if tx_bias == 1 {
        match class {
            BiasModeClass::IntraBlurry if ac_bias == 0.0 => {
                spatial_distortion = (spatial_distortion * 5) / 4;
            }
            BiasModeClass::IntraNeutral if ac_bias == 0.0 => {
                spatial_distortion = (spatial_distortion * 9) / 8;
            }
            BiasModeClass::InterCompoundBlurry => {
                spatial_distortion = (spatial_distortion * 5) / 4;
            }
            BiasModeClass::InterCompoundDiffwtd => {
                spatial_distortion = (spatial_distortion * 9) / 8;
            }
            _ => {}
        }
        // Temporal-layer intra bias (any intra class).
        if is_intra && temporal_layer_index >= 2 {
            const WEIGHTS: [i64; 6] = [8, 8, 9, 10, 11, 12];
            let w = WEIGHTS[usize::from(temporal_layer_index).min(5)];
            spatial_distortion = (spatial_distortion * w) / 8;
        }
    }

    // Transform-size tweaks: tx_bias 1 or 2.
    if (tx_bias == 1 || tx_bias == 2) && is_intra {
        if area_width == 64 && area_height == 64 {
            spatial_distortion = (spatial_distortion * 3) / 2;
        } else if tx_bias == 1 && area_width * area_height <= 32 * 32 {
            spatial_distortion = (spatial_distortion * 17) / 16;
        }
    }

    spatial_distortion
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_when_tx_bias_off() {
        for class in [
            BiasModeClass::IntraBlurry,
            BiasModeClass::IntraNeutral,
            BiasModeClass::Other,
        ] {
            assert_eq!(
                facade_bias(10_000, class, true, 64, 64, 3, 0.0, 0),
                10_000
            );
        }
    }

    #[test]
    fn intra_mode_biases_gated_on_zero_ac_bias() {
        // ac_bias > 0 disables the mode-kind bias but NOT the 64x64 bias.
        let d = facade_bias(8000, BiasModeClass::IntraBlurry, true, 16, 16, 0, 1.0, 1);
        // 16x16 <= 32*32 → *17/16 only
        assert_eq!(d, 8000 * 17 / 16);
        let d = facade_bias(8000, BiasModeClass::IntraBlurry, true, 16, 16, 0, 0.0, 1);
        assert_eq!(d, (8000 * 5 / 4) * 17 / 16);
    }

    #[test]
    fn tx_size_biases() {
        // 64x64 strong bias applies at tx_bias 1 and 2.
        for txb in [1u8, 2] {
            let d = facade_bias(9000, BiasModeClass::IntraOther, true, 64, 64, 0, 1.0, txb);
            assert_eq!(d, 9000 * 3 / 2, "tx_bias {txb}");
        }
        // small-block 17/16 is tx_bias 1 only.
        let d = facade_bias(9000, BiasModeClass::IntraOther, true, 32, 32, 0, 1.0, 2);
        assert_eq!(d, 9000);
    }

    #[test]
    fn temporal_layer_weights() {
        for (layer, w) in [(2u8, 9i64), (3, 10), (4, 11), (5, 12)] {
            let d = facade_bias(8000, BiasModeClass::IntraOther, true, 8, 8, layer, 1.0, 1);
            assert_eq!(d, (8000 * w / 8) * 17 / 16, "layer {layer}");
        }
    }
}

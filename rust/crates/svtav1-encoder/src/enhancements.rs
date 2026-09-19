//! Explicit Zen experiments layered on a named C reference and native preset.
//!
//! No experiment is enabled by default or automatically selected by effort.
//! Calibration must precede automatic policy. These controls allow isolated
//! ablation in the Zen continuation immediately beyond native research −1.

/// Independently selectable, uncalibrated search enhancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenEnhancement {
    /// Follow AOM's still-image policy of enabling intra-edge filtering.
    /// Prediction and sequence signaling change together. This can worsen RD;
    /// it is an experiment, not a promise of improved compression.
    AomIntraEdgeFilter,
    /// Evaluate legal restoration-unit sizes using SVT filter search and RD costs.
    AomRestorationUnitSearch,
    /// Apply the measured still-image tune bundle: C's tune-IQ recipe plus
    /// the Zen extras that beat it on the imazen-26 evaluation subset
    /// (see [`apply_still_image_tune`]). Not byte-pinned to C — verified by
    /// decoder + RD measurement, not `cmp`.
    StillImageTune,
}

impl ZenEnhancement {
    /// Stable experiment identity for benchmark/configuration records.
    pub const fn id(self) -> &'static str {
        match self {
            Self::AomIntraEdgeFilter => "aom-intra-edge-filter-v1",
            Self::AomRestorationUnitSearch => "aom-restoration-unit-search-v1",
            Self::StillImageTune => "still-image-tune-v1",
        }
    }
}

/// A deduplicated set of explicit experiments. Empty preserves C decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZenEnhancements {
    intra_edge_filter: bool,
    restoration_unit_search: bool,
    still_image_tune: bool,
}

impl ZenEnhancements {
    /// Add one diagnostic/ablation experiment.
    pub const fn with(mut self, enhancement: ZenEnhancement) -> Self {
        match enhancement {
            ZenEnhancement::AomIntraEdgeFilter => self.intra_edge_filter = true,
            ZenEnhancement::AomRestorationUnitSearch => self.restoration_unit_search = true,
            ZenEnhancement::StillImageTune => self.still_image_tune = true,
        }
        self
    }

    /// Whether a particular experiment is selected.
    pub const fn contains(self, enhancement: ZenEnhancement) -> bool {
        match enhancement {
            ZenEnhancement::AomIntraEdgeFilter => self.intra_edge_filter,
            ZenEnhancement::AomRestorationUnitSearch => self.restoration_unit_search,
            ZenEnhancement::StillImageTune => self.still_image_tune,
        }
    }

    /// Whether every experiment is disabled.
    pub const fn is_empty(self) -> bool {
        !self.intra_edge_filter && !self.restoration_unit_search && !self.still_image_tune
    }

    /// Per-member envelopes. The two research members extend native −1 still
    /// 4:2:0 only — faster-preset reuse needs separate measurements and is not
    /// inferred. [`ZenEnhancement::StillImageTune`] is a still/all-intra bundle
    /// measured at ordinary presets on 4:2:0, so it drops the research-preset
    /// requirement while keeping the intra and chroma ones.
    pub fn validate(
        self,
        preset: i8,
        allintra: bool,
        chroma_420: bool,
    ) -> Result<(), &'static str> {
        if (self.intra_edge_filter || self.restoration_unit_search)
            && (preset != -1 || !allintra || !chroma_420)
        {
            return Err("Zen experiments require native research -1, all-intra 4:2:0");
        }
        if self.still_image_tune && (!allintra || !chroma_420) {
            return Err("still-image-tune-v1 is measured for all-intra 4:2:0 only");
        }
        Ok(())
    }
}

/// The [`ZenEnhancement::StillImageTune`] recipe, applied in
/// `encode_frame_impl` in place of the plain `apply_tune_overrides` call.
///
/// Base layer is C's tune-IQ bundle (QM on at still levels, sharpness 7,
/// variance boost 3/curve 2, `max_tx_size` cap, `screen_content_mode = 3`),
/// installed by `apply_tune_overrides`. The recipe layer then fills any
/// bundle-covered knob the caller left at its mode default with the value
/// the imazen-26 subset measured, while an explicitly-set extra (e.g. via
/// `SVT_FORK_VARIANCE_OCTILE`) survives the tune override — which is also
/// what makes the recipe sweepable through `identity_run` envs.
///
/// Only mainline-live knobs belong here: `variance_boost_*`,
/// `variance_octile`, the QM level bounds, `sharpness`, `max_tx_size`,
/// `screen_content_mode`. `cdef_scaling`, `noise_norm_strength`, `tx_bias`,
/// `complex_hvs`, `sharp_tx` and `alt_ssim_tuning` are `is_fork()`-gated and
/// would be dead writes on this path. `ac_bias` measured neutral on the
/// subset and is not pinned — the IQ overrides never touch it, so any
/// caller/mode value flows through unchanged.
pub fn apply_still_image_tune(hdr: &mut crate::hdr_mode::HdrForkConfig, qp: u8) {
    // Snapshot every knob `apply_tune_overrides` rewrites BEFORE it runs —
    // a caller-set value always wins over the tune's own bundle, which is
    // also what makes the recipe sweepable through `identity_run`'s
    // SVT_FORK_* envs. "Caller-set" is judged against the ACTIVE mode's
    // defaults: the fork config starts at `min_qm = 6` where mainline
    // starts at `8`, and treating a fork default as an override would
    // silently defeat the recipe pins.
    let min_qm_d = if hdr.is_fork() { 6 } else { 8 };
    let vb_strength = hdr.variance_boost_strength;
    let octile = hdr.variance_octile;
    let min_qm = hdr.min_qm_level;
    hdr.tune = crate::tune::TUNE_IQ;
    hdr.apply_tune_overrides(qp);
    // Recipe v1 pins — measured on the imazen-26 subset (benchmarks/
    // still_image_tune_v1_2026-09-19.meta).
    hdr.variance_boost_strength = if vb_strength == 2 {
        STILL_TUNE_VB_STRENGTH
    } else {
        vb_strength
    };
    hdr.variance_octile = if octile == 5 {
        STILL_TUNE_OCTILE
    } else {
        octile
    };
    hdr.min_qm_level = if min_qm == min_qm_d {
        STILL_TUNE_MIN_QM
    } else {
        min_qm
    };
}

/// Recipe v1 keeps tune IQ's own strength 3 — strength 4 measured WORSE on
/// the subset (+3.8% median bytes vs plain IQ at equal ssim2, 27 losing
/// cells).
const STILL_TUNE_VB_STRENGTH: u8 = 3;
/// Recipe v1 pins octile 8: the variance boost anchors on the top-variance
/// octile only, which trims IQ's matched-qp byte inflation by a −13.0%
/// median (p25/p75 −20.7/−6.9) at Δssim2 +0.011 with 108 dominating and
/// ZERO losing cells on the 42-image subset (benchmarks/
/// still_image_tune_v1_2026-09-19.meta). Octile 6 (−4.5%) and 7 (−9.9%)
/// are the same direction but smaller.
const STILL_TUNE_OCTILE: u8 = 8;
/// Recipe v1 keeps tune IQ's own `min_qm_level` 4.
const STILL_TUNE_MIN_QM: u8 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiments_are_explicit_and_do_not_repurpose_native_presets() {
        let off = ZenEnhancements::default();
        assert!(off.is_empty());
        for enhancement in [
            ZenEnhancement::AomIntraEdgeFilter,
            ZenEnhancement::AomRestorationUnitSearch,
        ] {
            let on = off.with(enhancement);
            assert_eq!(on.with(enhancement), on);
            assert!(on.contains(enhancement));
            assert!(on.validate(-1, true, true).is_ok());
            for preset in [-3, -2, 0, 5, 9, 13] {
                assert!(on.validate(preset, true, true).is_err());
            }
            assert!(on.validate(-1, false, true).is_err());
            assert!(on.validate(-1, true, false).is_err());
        }
    }

    #[test]
    fn still_image_tune_is_allintra_420_scoped_not_research_preset_scoped() {
        let on = ZenEnhancements::default().with(ZenEnhancement::StillImageTune);
        assert!(on.contains(ZenEnhancement::StillImageTune));
        assert!(!on.is_empty());
        // Any preset is fine on a still 4:2:0; inter and non-420 are refused.
        for preset in [-1, 0, 2, 6, 9, 13] {
            assert!(on.validate(preset, true, true).is_ok());
            assert!(on.validate(preset, true, false).is_err());
            assert!(on.validate(preset, false, true).is_err());
        }
    }

    #[test]
    fn still_image_tune_recipe_preserves_caller_extras() {
        // The recipe installs tune IQ's bundle then fills unset extras.
        let mut hdr = crate::hdr_mode::HdrForkConfig::mainline();
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.tune, crate::tune::TUNE_IQ);
        assert!(hdr.enable_qm && hdr.enable_variance_boost);
        assert_eq!(hdr.sharpness, 7);
        assert_eq!(hdr.variance_boost_strength, STILL_TUNE_VB_STRENGTH);
        assert_eq!(hdr.variance_octile, STILL_TUNE_OCTILE);
        assert_eq!(hdr.min_qm_level, STILL_TUNE_MIN_QM);
        assert_eq!(hdr.ac_bias, 0.0); // not pinned: mainline default flows through
        assert_eq!(hdr.max_tx_size, 32);
        assert_eq!(hdr.screen_content_mode, Some(3));

        // Caller-set extras survive the tune override; recipe fills only the
        // still-at-default fields. `ac_bias` is not recipe-covered at all —
        // the IQ overrides never touch it.
        let mut hdr = crate::hdr_mode::HdrForkConfig::mainline();
        hdr.ac_bias = 1.0;
        hdr.variance_boost_strength = 4;
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.ac_bias, 1.0);
        assert_eq!(hdr.variance_boost_strength, 4);

        // A FORK-mode pipeline must not confuse its own `min_qm = 6`
        // default for a caller override — the recipe pin still applies
        // there, while its `ac_bias = 1.0` default flows through.
        let mut hdr = crate::hdr_mode::HdrForkConfig::hdr_fork();
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.ac_bias, 1.0);
        assert_eq!(hdr.min_qm_level, STILL_TUNE_MIN_QM);
        assert_eq!(hdr.variance_octile, STILL_TUNE_OCTILE);
    }
}

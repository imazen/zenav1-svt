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
/// Recipe v1 is C's tune-IQ bundle verbatim — QM on at still levels,
/// sharpness 7, variance boost 3/curve 2, `max_tx_size` cap,
/// `screen_content_mode = 3` — installed by `apply_tune_overrides`, plus
/// ONE semantic delta vs a bare `with_tune(Iq)`: every knob the bundle
/// rewrites is snapshotted first, and a value the caller set away from its
/// mode default survives the bundle. That preserves explicit extras (e.g.
/// `SVT_FORK_VARIANCE_OCTILE` through `identity_run`) and keeps the recipe
/// sweepable. With no caller extras the emitted stream is byte-identical
/// to tune IQ.
///
/// Extras were measured on the 42-image imazen-26 subset before pinning
/// (benchmarks/still_image_tune_v1_2026-09-19.meta): `variance_octile`
/// 6–8, `variance_boost_strength` 4 and `ac_bias` 1.0 were all neutral or
/// WORSE than plain IQ on ssim2-BD-rate, so v1 pins nothing beyond what IQ
/// itself sets. `cdef_scaling`, `noise_norm_strength`, `tx_bias`,
/// `complex_hvs`, `sharp_tx` and `alt_ssim_tuning` are `is_fork()`-gated
/// and would be dead writes on this path; `ac_bias` is not bundle-covered
/// at all, so any caller/mode value flows through unchanged.
pub fn apply_still_image_tune(hdr: &mut crate::hdr_mode::HdrForkConfig, qp: u8) {
    // Snapshot every knob `apply_tune_overrides` rewrites BEFORE it runs —
    // a caller-set value always wins over the tune's own bundle, which is
    // also what makes the recipe sweepable through `identity_run`'s
    // SVT_FORK_* envs. "Caller-set" is judged against the ACTIVE mode's
    // defaults: the fork config starts at `min_qm = 6`/`sharpness = 1`/
    // `enable_qm = true` where mainline starts at `8`/`0`/`false`, and
    // treating a fork default as an override would silently defeat the
    // bundle there.
    let fork = hdr.is_fork();
    let snap = (
        hdr.enable_qm,
        hdr.min_qm_level,
        hdr.max_qm_level,
        hdr.min_chroma_qm_level,
        hdr.max_chroma_qm_level,
        hdr.sharpness,
        hdr.enable_variance_boost,
        hdr.variance_boost_strength,
        hdr.variance_boost_curve,
        hdr.max_tx_size,
        hdr.screen_content_mode,
    );
    hdr.tune = crate::tune::TUNE_IQ;
    hdr.apply_tune_overrides(qp);
    let d = if fork {
        (true, 6u8, 10u8, 8u8, 15u8, 1i8, true, 2u8, 0u8, 64u8, None)
    } else {
        (
            false, 8u8, 15u8, 8u8, 15u8, 0i8, false, 2u8, 0u8, 64u8, None,
        )
    };
    if snap.0 != d.0 {
        hdr.enable_qm = snap.0;
    }
    if snap.1 != d.1 {
        hdr.min_qm_level = snap.1;
    }
    if snap.2 != d.2 {
        hdr.max_qm_level = snap.2;
    }
    if snap.3 != d.3 {
        hdr.min_chroma_qm_level = snap.3;
    }
    if snap.4 != d.4 {
        hdr.max_chroma_qm_level = snap.4;
    }
    if snap.5 != d.5 {
        hdr.sharpness = snap.5;
    }
    if snap.6 != d.6 {
        hdr.enable_variance_boost = snap.6;
    }
    if snap.7 != d.7 {
        hdr.variance_boost_strength = snap.7;
    }
    if snap.8 != d.8 {
        hdr.variance_boost_curve = snap.8;
    }
    if snap.9 != d.9 {
        hdr.max_tx_size = snap.9;
    }
    if snap.10 != d.10 {
        hdr.screen_content_mode = snap.10;
    }
}

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
        // The recipe installs tune IQ's bundle verbatim; v1 pins nothing
        // beyond it (measured extras were neutral-or-worse on BD-rate).
        let mut hdr = crate::hdr_mode::HdrForkConfig::mainline();
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.tune, crate::tune::TUNE_IQ);
        assert!(hdr.enable_qm && hdr.enable_variance_boost);
        assert_eq!(hdr.sharpness, 7);
        assert_eq!(hdr.variance_boost_strength, 3);
        assert_eq!(hdr.variance_octile, 5); // IQ does not touch the octile
        assert_eq!(hdr.min_qm_level, 4);
        assert_eq!(hdr.ac_bias, 0.0); // not pinned: mainline default flows through
        assert_eq!(hdr.max_tx_size, 32);
        assert_eq!(hdr.screen_content_mode, Some(3));

        // Caller-set values survive even where the bundle overrides them —
        // this is the recipe's only delta vs a bare `with_tune(Iq)`.
        // `ac_bias` is not bundle-covered at all — the IQ overrides never
        // touch it.
        let mut hdr = crate::hdr_mode::HdrForkConfig::mainline();
        hdr.ac_bias = 1.0;
        hdr.variance_boost_strength = 4;
        hdr.sharpness = 3;
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.ac_bias, 1.0);
        assert_eq!(hdr.variance_boost_strength, 4);
        assert_eq!(hdr.sharpness, 3);

        // A FORK-mode pipeline must not confuse its own `min_qm = 6` /
        // `sharpness = 1` / `enable_qm = true` defaults for caller overrides —
        // the bundle still applies there, while `ac_bias = 1.0` flows through.
        let mut hdr = crate::hdr_mode::HdrForkConfig::hdr_fork();
        apply_still_image_tune(&mut hdr, 32);
        assert_eq!(hdr.ac_bias, 1.0);
        assert_eq!(hdr.min_qm_level, 4);
        assert_eq!(hdr.sharpness, 7);
        assert_eq!(hdr.variance_octile, 5);
    }
}

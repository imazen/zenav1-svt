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
}

impl ZenEnhancement {
    /// Stable experiment identity for benchmark/configuration records.
    pub const fn id(self) -> &'static str {
        match self {
            Self::AomIntraEdgeFilter => "aom-intra-edge-filter-v1",
            Self::AomRestorationUnitSearch => "aom-restoration-unit-search-v1",
        }
    }
}

/// A deduplicated set of explicit experiments. Empty preserves C decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZenEnhancements {
    intra_edge_filter: bool,
    restoration_unit_search: bool,
}

impl ZenEnhancements {
    /// Add one diagnostic/ablation experiment.
    pub const fn with(mut self, enhancement: ZenEnhancement) -> Self {
        match enhancement {
            ZenEnhancement::AomIntraEdgeFilter => self.intra_edge_filter = true,
            ZenEnhancement::AomRestorationUnitSearch => self.restoration_unit_search = true,
        }
        self
    }

    /// Whether a particular experiment is selected.
    pub const fn contains(self, enhancement: ZenEnhancement) -> bool {
        match enhancement {
            ZenEnhancement::AomIntraEdgeFilter => self.intra_edge_filter,
            ZenEnhancement::AomRestorationUnitSearch => self.restoration_unit_search,
        }
    }

    /// Whether every experiment is disabled.
    pub const fn is_empty(self) -> bool {
        !self.intra_edge_filter && !self.restoration_unit_search
    }

    /// This first experimental region extends native −1 still 420 only.
    /// Faster-preset reuse needs separate measurements and is not inferred.
    pub fn validate(
        self,
        preset: i8,
        allintra: bool,
        chroma_420: bool,
    ) -> Result<(), &'static str> {
        if !self.is_empty() && (preset != -1 || !allintra || !chroma_420) {
            return Err("Zen experiments require native research -1, all-intra 4:2:0");
        }
        Ok(())
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
}

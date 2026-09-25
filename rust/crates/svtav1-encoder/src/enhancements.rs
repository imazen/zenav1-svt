//! Explicit Zen experiments layered on a named C reference and native preset.
//!
//! No experiment is enabled by default or automatically selected by effort.
//! Calibration must precede automatic policy. These controls allow isolated
//! ablation in the Zen continuation immediately beyond native research −1.
//!
//! Removed 2026-09-25 on their measurements, so do not re-add them without
//! new evidence: `StillImageTune` (v1 was exactly tune IQ;
//! `benchmarks/still_image_tune_v1_2026-09-19.meta`), `AomAdaptiveSharpness`
//! (a no-op under IQ), `AomAdaptiveCdef` (+0.4 to +0.9 % ssim2 BD against
//! SVT's own strength pick) and `AomDeltaQLf` (RD-neutral;
//! `benchmarks/aom_features_2026-09-20.meta`); and on the same day
//! `AomIntraEdgeFilter` (a small net ssim2 BD loss: median +0.12% / +0.51% at
//! tune 1 / 3 on photos) and `AomRestorationUnitSearch` (median BD 0.000, 2-9%
//! slower), both in `benchmarks/aom_keep_or_drop_2026-09-25.meta`.

/// Independently selectable, uncalibrated search enhancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZenEnhancement {
    /// libaom-style screen-tool availability on stills: when the AA-aware
    /// detector sets `sc_class5`, palette and IntraBC stay ENABLED at every
    /// preset instead of following the allintra ladders that switch them
    /// off (palette dies at M8+, IntraBC at M5+, detection itself is gated
    /// at M7 — `sc_detect::derive_sc`). C's own VIDEO-arm ladders
    /// (`palette_level_default`/`intrabc_level_default`) supply the level
    /// at each preset, clamped so tools never die (palette holds the M10
    /// level above it, IntraBC the M9 level). libaom does not gate these
    /// tools on a speed ladder at all — it enables them whenever screen
    /// content is detected and lets the search decide. Zen extension —
    /// decoder-verified and RD-measured (`benchmarks/aom_keep_or_drop_2026-09-25.meta`:
    /// screen content -23% ssim2 BD-rate at preset 8, -29% at 10/12; inert
    /// below preset 8 and under tune IQ), never byte-claimed against C;
    /// the frame header gains `allow_screen_content_tools`/`allow_intrabc`
    /// bits a C-parity stream at the same preset would not carry.
    AomScreenTools,
    /// Deep-search tier for all-intra stills: the leaf-funnel search-effort
    /// ladders (`intra_arm`, `txs_arm`, `funnel_arm`, `nic_arm`,
    /// `encdec_arm`, `mds0_arm`) and the depth-refinement ladder
    /// (`depth_refine::DrCtrls`) are evaluated at the deepest C tier —
    /// `enc_mode -1` (MR) — at whatever preset the caller picked, so a
    /// p8 encode searches with p-1's unbounded candidate counts, full
    /// tx-size/tx-type search and unrestricted partition depth. Rate,
    /// lambda, quantizer and frame-header derivations keep the caller's
    /// preset; only WHAT IS SEARCHED changes. Byte-inert at preset -1
    /// (the ladders already sit at -1) and on every non-allintra arm.
    /// Zen extension — decoder-verified and RD-measured, never
    /// byte-claimed against C; streams differ by design.
    DeepSearch,
}

impl ZenEnhancement {
    /// Stable experiment identity for benchmark/configuration records.
    pub const fn id(self) -> &'static str {
        match self {
            Self::AomScreenTools => "aom-screen-tools-v1",
            Self::DeepSearch => "deep-search-v1",
        }
    }
}

/// A deduplicated set of explicit experiments. Empty preserves C decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZenEnhancements {
    screen_tools: bool,
    deep_search: bool,
}

impl ZenEnhancements {
    /// Add one diagnostic/ablation experiment.
    pub const fn with(mut self, enhancement: ZenEnhancement) -> Self {
        match enhancement {
            ZenEnhancement::AomScreenTools => self.screen_tools = true,
            ZenEnhancement::DeepSearch => self.deep_search = true,
        }
        self
    }

    /// Whether a particular experiment is selected.
    pub const fn contains(self, enhancement: ZenEnhancement) -> bool {
        match enhancement {
            ZenEnhancement::AomScreenTools => self.screen_tools,
            ZenEnhancement::DeepSearch => self.deep_search,
        }
    }

    /// Whether every experiment is disabled.
    pub const fn is_empty(self) -> bool {
        !self.screen_tools && !self.deep_search
    }

    /// Per-member envelopes, each measured for the scope it names.
    pub fn validate(
        self,
        _preset: i8,
        allintra: bool,
        chroma_420: bool,
        bit_depth: u8,
    ) -> Result<(), &'static str> {
        if self.screen_tools && (!allintra || !chroma_420) {
            return Err("aom-screen-tools-v1 is measured for all-intra 4:2:0 only");
        }
        if self.deep_search && (!allintra || !chroma_420 || bit_depth != 8) {
            return Err("deep-search-v1 is measured for all-intra 4:2:0 8-bit only");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_search_is_allintra_420_8bit_scoped_and_inert_at_native_minus1() {
        let on = ZenEnhancements::default().with(ZenEnhancement::DeepSearch);
        assert!(on.contains(ZenEnhancement::DeepSearch));
        // Any preset — including the native -1 the arm collapses onto — is
        // accepted on a still 4:2:0 8-bit; inter, non-420 (mono, 4:4:4)
        // and 10-bit (the bd10 MD post-pass knobs stay native there —
        // unmeasured) refuse.
        for preset in [-1, 0, 2, 6, 9, 13] {
            assert!(on.validate(preset, true, true, 8).is_ok());
            assert!(on.validate(preset, true, false, 8).is_err());
            assert!(on.validate(preset, false, true, 8).is_err());
            assert!(on.validate(preset, true, true, 10).is_err());
        }
    }
}

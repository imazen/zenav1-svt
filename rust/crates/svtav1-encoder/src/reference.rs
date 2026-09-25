//! Explicit C source identity, separate from the HDR build mode and policy.
//!
//! Legacy encoder constructors retain the hybrid reference for compatibility.
//! A strict mainline policy must select [`SvtReference::Mainline420`] explicitly.
//! Selecting a source controls reference-specific decisions; it does not certify
//! parity for untested settings or replace the policy's full support checks.

use crate::hdr_mode::{HdrForkConfig, SvtHdrMode};

/// Pinned C source whose decisions the encoder follows.
///
/// Each variant pairs with one row of `rust/oracles/oracles.tsv`
/// (`rust/docs/ORACLES.md`); the C harness builds that oracle when
/// `SVT_ORACLE` names it, so both sides of a byte comparison agree on the
/// target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SvtReference {
    /// Pristine SVT-AV1 v4.2.0, without the hybrid's added behavior.
    #[default]
    Mainline420,
    /// The in-tree hybrid source. Its HDR mode remains separately selectable.
    /// Legacy: retires once `GhostRobot` and `Mainline420` are gated
    /// (`rust/docs/PLAN-ORACLES-AND-CLEANUP.md`, phase 3.7).
    Hybrid3115,
    /// svt-av1-hdr 4.2 "Ghost Robot" (`9dabe3ca`): mainline v4.2.0 plus 144
    /// unreleased mainline-master commits plus the fork's own. Fork semantics
    /// always apply (the fork has no mainline mode), so it requires
    /// [`SvtHdrMode::HdrFork`]. Until phase 3 of the plan lands, the port runs
    /// the hybrid's Chromedome fork behavior under this reference: selecting it
    /// names the TARGET, and the gates measure how far the port is from it.
    GhostRobot,
}

impl SvtReference {
    /// Stable source identifier for configuration records and fingerprints.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mainline420 => "svt-mainline-4.2.0-9292ec8e32bce26f781f277ec8739b53426c4300",
            Self::Hybrid3115 => "svt-hybrid-3115c0c1b23e860dfd75c94f6740e0298182dd13",
            Self::GhostRobot => "svt-av1-hdr-ghost-robot-9dabe3ca04390e2e19267bf87a48c6cab44d6111",
        }
    }

    /// The oracle registry name (`rust/oracles/oracles.tsv`) this reference
    /// pairs with. `Hybrid3115` pairs with two rows, one per HDR mode.
    pub const fn oracle_name(self, mode: SvtHdrMode) -> &'static str {
        match (self, mode) {
            (Self::Mainline420, _) => "mainline-4.2.0",
            (Self::Hybrid3115, SvtHdrMode::Mainline) => "hybrid-3115",
            (Self::Hybrid3115, SvtHdrMode::HdrFork) => "hybrid-3115-hdr",
            (Self::GhostRobot, _) => "ghost-robot",
        }
    }

    /// Reject controls that cannot belong to the selected mainline source.
    /// Mainline-owned tuning, QM, variance boost and grain remain available.
    pub fn validate_hdr_config(self, hdr: &HdrForkConfig) -> Result<(), &'static str> {
        hdr.validate_ranges()?;
        // Numbered as every C oracle numbers it (`crate::tune`): 5 is VMAF,
        // whose unsharp pre-processing is not ported.
        if hdr.tune == crate::tune::TUNE_VMAF {
            return Err("tune 5 (VMAF) is not ported");
        }
        if hdr.tune > crate::tune::TUNE_FILM_GRAIN {
            return Err("tune must be 0 through 6");
        }
        // C options the port runs only at their defaults. A different value
        // would be silently ignored, so it is refused instead.
        if hdr.luminance_qp_bias != 0 {
            return Err(
                "luminance_qp_bias is not ported: only 0 (off) is accepted [C: accepts 1..=100]",
            );
        }
        if hdr.hbd_mds != -1 {
            return Err(
                "hbd_mds is not ported beyond -1 (preset-derived 8/10-bit mode decision) \
                 [C: accepts 0, 1, 2]",
            );
        }
        if self == Self::GhostRobot {
            if hdr.enable_qmpsnr == 1 {
                return Err("enable_qmpsnr = 1 (QM-weighted PSNR) is not ported [C: accepts]");
            }
            if hdr.max_hierarchical_levels != 0 {
                return Err(
                    "max_hierarchical_levels > 0 (RTC CBR low-delay mini-GOP resizing) is not \
                     ported [C: accepts]",
                );
            }
        } else if hdr.enable_qmpsnr != -1 || hdr.max_hierarchical_levels != 0 {
            return Err("enable_qmpsnr and max_hierarchical_levels are Ghost Robot options");
        }
        match self {
            Self::Hybrid3115 => return Ok(()),
            Self::GhostRobot => {
                if hdr.mode != SvtHdrMode::HdrFork {
                    return Err(
                        "Ghost Robot has no mainline mode: select SvtHdrMode::HdrFork, or \
                         SvtReference::Mainline420 for pristine mainline",
                    );
                }
                return Ok(());
            }
            Self::Mainline420 => {}
        }
        if hdr.mode != SvtHdrMode::Mainline {
            return Err("pristine mainline reference cannot use hybrid HDR mode");
        }
        if hdr.sharp_tx != 0
            || hdr.kf_tf_strength != 3
            || hdr.alt_lambda_factors
            || hdr.alt_ssim_tuning
            || hdr.tx_bias != 0
            || hdr.complex_hvs != 0
            || hdr.noise_norm_strength != 0
            || hdr.noise_adaptive_filtering != 2
            || hdr.cdef_scaling != 15
            || hdr.noise_strength != 0
            || hdr.noise_strength_chroma != -1
            || hdr.noise_chroma_from_luma != 0
            || hdr.noise_size != -1
        {
            return Err("pristine mainline reference cannot use hybrid-only controls");
        }
        if hdr.tune > 5 {
            return Err("pristine mainline reference supports tune values 0 through 5");
        }
        if hdr.variance_boost_curve > 2 {
            return Err("pristine mainline reference supports variance boost curves 0 through 2");
        }
        let compress = hdr.qp_scale_compress_strength;
        if !compress.is_finite() || !(0.0..=3.0).contains(&compress) || compress.fract() != 0.0 {
            return Err(
                "pristine mainline QP scale compression must be an integer from 0 through 3",
            );
        }
        Ok(())
    }
}

impl core::str::FromStr for SvtReference {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [Self::Mainline420, Self::Hybrid3115, Self::GhostRobot]
            .into_iter()
            .find(|reference| reference.id() == value)
            .ok_or("unrecognized pinned SVT reference identity")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_identity_roundtrip_and_mainline_envelope() {
        for reference in [
            SvtReference::Mainline420,
            SvtReference::Hybrid3115,
            SvtReference::GhostRobot,
        ] {
            assert_eq!(reference.id().parse(), Ok(reference));
        }
        assert!("mainline".parse::<SvtReference>().is_err());
        let mut hdr = HdrForkConfig::mainline();
        hdr.enable_qm = true;
        hdr.enable_variance_boost = true;
        hdr.ac_bias = 1.0;
        hdr.variance_boost_curve = 2;
        hdr.qp_scale_compress_strength = 3.0;
        assert!(SvtReference::Mainline420.validate_hdr_config(&hdr).is_ok());
        for value in [0.5, 4.0, f64::NAN, f64::INFINITY] {
            hdr.qp_scale_compress_strength = value;
            assert!(SvtReference::Mainline420.validate_hdr_config(&hdr).is_err());
        }
        hdr.qp_scale_compress_strength = 0.0;
        hdr.tx_bias = 1;
        assert!(SvtReference::Mainline420.validate_hdr_config(&hdr).is_err());
        assert!(SvtReference::Hybrid3115.validate_hdr_config(&hdr).is_ok());
        assert!(
            SvtReference::Mainline420
                .validate_hdr_config(&HdrForkConfig::hdr_fork_c_mode1())
                .is_err()
        );
    }

    /// The C options the port runs only at their defaults are refused at any
    /// other value, never silently ignored, and the defaults pass for every
    /// reference.
    #[test]
    fn unported_c_options_are_refused_not_ignored() {
        let gr = HdrForkConfig::ghost_robot();
        for reference in [SvtReference::GhostRobot, SvtReference::Hybrid3115] {
            let base = if reference == SvtReference::GhostRobot {
                gr.clone()
            } else {
                HdrForkConfig::hdr_fork_c_mode1()
            };
            assert!(
                reference.validate_hdr_config(&base).is_ok(),
                "{reference:?} defaults"
            );
            let mut h = base.clone();
            h.luminance_qp_bias = 10;
            assert!(reference.validate_hdr_config(&h).is_err());
            for v in [0i8, 1, 2] {
                let mut h = base.clone();
                h.hbd_mds = v;
                assert!(reference.validate_hdr_config(&h).is_err(), "hbd_mds {v}");
            }
        }
        assert!(
            SvtReference::Mainline420
                .validate_hdr_config(&HdrForkConfig::mainline())
                .is_ok()
        );
        // Ghost Robot: QM-PSNR on and mini-GOP resizing are refused; PSNR
        // (0) and auto (-1) are accepted.
        let mut h = gr.clone();
        h.enable_qmpsnr = 1;
        assert!(SvtReference::GhostRobot.validate_hdr_config(&h).is_err());
        h.enable_qmpsnr = 0;
        assert!(SvtReference::GhostRobot.validate_hdr_config(&h).is_ok());
        let mut h = gr.clone();
        h.max_hierarchical_levels = 1;
        assert!(SvtReference::GhostRobot.validate_hdr_config(&h).is_err());
        // The Ghost Robot-only options have no meaning for the other references.
        let mut h = HdrForkConfig::hdr_fork_c_mode1();
        h.enable_qmpsnr = 0;
        assert!(SvtReference::Hybrid3115.validate_hdr_config(&h).is_err());
        let mut h = HdrForkConfig::mainline();
        h.max_hierarchical_levels = 1;
        assert!(SvtReference::Mainline420.validate_hdr_config(&h).is_err());
    }

    #[test]
    fn ghost_robot_requires_fork_mode_and_tunes_follow_c_numbering() {
        use crate::tune::{TUNE_FILM_GRAIN, TUNE_VMAF};
        let fork = HdrForkConfig::hdr_fork_c_mode1();
        assert!(SvtReference::GhostRobot.validate_hdr_config(&fork).is_ok());
        assert!(
            SvtReference::GhostRobot
                .validate_hdr_config(&HdrForkConfig::mainline())
                .is_err()
        );
        let mut grain = fork.clone();
        grain.tune = TUNE_FILM_GRAIN;
        assert!(SvtReference::GhostRobot.validate_hdr_config(&grain).is_ok());
        assert!(SvtReference::Hybrid3115.validate_hdr_config(&grain).is_ok());
        let mut vmaf = fork;
        vmaf.tune = TUNE_VMAF;
        for r in [
            SvtReference::Mainline420,
            SvtReference::Hybrid3115,
            SvtReference::GhostRobot,
        ] {
            assert!(
                r.validate_hdr_config(&vmaf).is_err(),
                "{r:?} must refuse tune 5"
            );
        }
        assert_eq!(
            SvtReference::GhostRobot.oracle_name(SvtHdrMode::HdrFork),
            "ghost-robot"
        );
        assert_eq!(
            SvtReference::Hybrid3115.oracle_name(SvtHdrMode::Mainline),
            "hybrid-3115"
        );
    }
}

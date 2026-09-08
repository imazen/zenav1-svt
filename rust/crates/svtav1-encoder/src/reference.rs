//! Explicit C source identity, separate from the HDR build mode and policy.
//!
//! Legacy encoder constructors retain the hybrid reference for compatibility.
//! A strict mainline policy must select [`SvtReference::Mainline420`] explicitly.
//! Selecting a source controls reference-specific decisions; it does not certify
//! parity for untested settings or replace the policy's full support checks.

use crate::hdr_mode::{HdrForkConfig, SvtHdrMode};

/// Pinned C source whose decisions the encoder follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvtReference {
    /// Pristine SVT-AV1 v4.2.0, without the hybrid's added behavior.
    #[default]
    Mainline420,
    /// The in-tree hybrid source. Its HDR mode remains separately selectable.
    Hybrid3115,
}

impl SvtReference {
    /// Stable source identifier for configuration records and fingerprints.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Mainline420 => "svt-mainline-4.2.0-9292ec8e32bce26f781f277ec8739b53426c4300",
            Self::Hybrid3115 => "svt-hybrid-3115c0c1b23e860dfd75c94f6740e0298182dd13",
        }
    }

    /// Reject controls that cannot belong to the selected mainline source.
    /// Mainline-owned tuning, QM, variance boost and grain remain available.
    pub fn validate_hdr_config(self, hdr: &HdrForkConfig) -> Result<(), &'static str> {
        if self == Self::Hybrid3115 {
            return Ok(());
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
        [Self::Mainline420, Self::Hybrid3115]
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
        for reference in [SvtReference::Mainline420, SvtReference::Hybrid3115] {
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
}

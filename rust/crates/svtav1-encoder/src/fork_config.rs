//! Typed overrides for the fork knobs of [`HdrForkConfig`], by their C names.
//!
//! Every field is `None` by default, which keeps the selected reference's own
//! default (`HdrForkConfig::defaults_for`). A `Some` replaces it verbatim:
//! nothing is clamped here. Out-of-range values are refused when the
//! configuration is validated (`HdrForkConfig::validate_ranges`, the ranges
//! of Ghost Robot's `svt_av1_verify_settings`), and values the port has not
//! implemented are refused by `SvtReference::validate_hdr_config`.
//!
//! `tune` is not here: the facade takes it typed (`AvifEncoder::with_tune`).
//! The C options the port refuses at any non-default value
//! (`luminance_qp_bias`, `hbd_mds`, `enable_qmpsnr`, `max_hierarchical_levels`)
//! are not here either; `HdrForkConfig` carries them at C's defaults.

use crate::hdr_mode::HdrForkConfig;

macro_rules! fork_config {
    ($($(#[doc = $doc:literal])* $name:ident: $ty:ty,)*) => {
        /// Fork knob overrides; see the [module docs](self).
        #[derive(Debug, Clone, Copy, Default, PartialEq)]
        #[non_exhaustive]
        pub struct ForkConfig {
            $($(#[doc = $doc])* pub $name: Option<$ty>,)*
        }

        impl ForkConfig {
            /// The C `EbSvtAv1EncConfiguration` field each member overrides.
            pub const FIELDS: &'static [&'static str] = &[$(stringify!($name)),*];

            /// `self` with every field that `other` sets taken from `other`.
            pub fn merged(mut self, other: ForkConfig) -> ForkConfig {
                $(if other.$name.is_some() {
                    self.$name = other.$name;
                })*
                self
            }

            /// Write every set field into `hdr`.
            pub fn apply(&self, hdr: &mut HdrForkConfig) {
                // `.into()` is the identity, except for `screen_content_mode`,
                // which `HdrForkConfig` holds as `Option<u8>` (None = derive).
                $(if let Some(v) = self.$name {
                    hdr.$name = v.into();
                })*
            }
        }
    };
}

fork_config! {
    /// `--enable-qm`: quantization matrices.
    enable_qm: bool,
    /// `--qm-min` (luma).
    min_qm_level: u8,
    /// `--qm-max` (luma).
    max_qm_level: u8,
    /// `--chroma-qm-min`.
    min_chroma_qm_level: u8,
    /// `--chroma-qm-max`.
    max_chroma_qm_level: u8,
    /// `--enable-variance-boost`.
    enable_variance_boost: bool,
    /// `--variance-boost-strength`, 1 to 4.
    variance_boost_strength: u8,
    /// `--variance-octile`, 1 to 8.
    variance_octile: u8,
    /// `--variance-boost-curve`, 0 to 3.
    variance_boost_curve: u8,
    /// `--sharpness`, -7 to 7.
    sharpness: i8,
    /// `--max-tx-size`, 32 or 64.
    max_tx_size: u8,
    /// `--scm`, 0 to 3.
    screen_content_mode: u8,
    /// `--tf-strength`, 0 to 4.
    tf_strength: u8,
    /// `--kf-tf-strength`, 0 to 4.
    kf_tf_strength: u8,
    /// `--ac-bias`, 0.0 to 8.0.
    ac_bias: f64,
    /// `--qp-scale-compress-strength`, 0.0 to 8.0.
    qp_scale_compress_strength: f64,
    /// `--sharp-tx`, 0 or 1.
    sharp_tx: u8,
    /// `--tx-bias`, 0 to 3.
    tx_bias: u8,
    /// `--complex-hvs`, 0 or 1.
    complex_hvs: u8,
    /// `--noise-norm-strength`, 0 to 4.
    noise_norm_strength: u8,
    /// `--noise-adaptive-filtering`, 0 to 4.
    noise_adaptive_filtering: u8,
    /// `--cdef-scaling`, 1 to 30.
    cdef_scaling: u8,
    /// `--noise`: grain synthesis strength, 0 to 200.
    noise_strength: u8,
    /// `--noise-chroma`, -1 (auto) to 200.
    noise_strength_chroma: i32,
    /// `--noise-size`, -1 (auto) to 13.
    noise_size: i8,
    /// `--noise-chroma-from-luma`, 0 or 1.
    noise_chroma_from_luma: u8,
    /// `--alt-lambda-factors`.
    alt_lambda_factors: bool,
    /// `--alt-ssim-tuning`.
    alt_ssim_tuning: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_only_the_given_fields_and_merge_prefers_the_newer() {
        let base = HdrForkConfig::ghost_robot();
        let mut hdr = base.clone();
        ForkConfig::default().apply(&mut hdr);
        assert_eq!(hdr, base, "an empty override changes nothing");

        let mut a = ForkConfig::default();
        a.ac_bias = Some(1.5);
        a.sharpness = Some(3);
        let mut b = ForkConfig::default();
        b.sharpness = Some(-2);
        let m = a.merged(b);
        assert_eq!((m.ac_bias, m.sharpness), (Some(1.5), Some(-2)));
        m.apply(&mut hdr);
        assert_eq!((hdr.ac_bias, hdr.sharpness), (1.5, -2));
        assert_eq!(hdr.tx_bias, base.tx_bias);
    }
}

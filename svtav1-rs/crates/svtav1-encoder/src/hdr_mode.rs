//! SVT_HDR_MODE — the Rust mirror of the C hybrid's compile-time switch.
//!
//! The in-tree C reference carries the svt-av1-hdr (Chromedome) fork's
//! behavior behind `-DSVT_HDR_MODE=0/1` (see `docs/HDR-ON-4.2.md`). This
//! module is the Rust-side equivalent, as a RUNTIME config so one binary
//! can target either C oracle:
//!
//! - [`SvtHdrMode::Mainline`]  → byte-identity target = stock v4.2.0-final
//!   (`cmake -DSVT_HDR_MODE=OFF`). All fork fields sit at their NEUTRAL
//!   values and every fork code path in this crate is skipped.
//! - [`SvtHdrMode::HdrFork`]   → byte-identity target = the hybrid's MODE1
//!   lib (`cmake -DSVT_HDR_MODE=ON`), i.e. fork semantics on the v4.2 base.
//!
//! Field-for-field this mirrors the fork's additions to
//! `EbSvtAv1EncConfiguration` plus the mainline fields whose DEFAULTS the
//! fork overrides (variance boost, QM, sharpness, tf_strength). Defaults
//! per mode replicate `enc_settings.c` `svt_av1_set_default_params` under
//! each `SVT_HDR_MODE` value.
//!
//! MODE1 PARITY IS AN INCREMENTAL CAMPAIGN (like the mainline identity
//! matrix): each fork behavior lands with its own C-parity witness. The
//! per-feature status table lives in `docs/HDR-ON-4.2.md`; until every row
//! is DONE, HdrFork-mode output is NOT yet byte-identical to the MODE1
//! oracle and the identity harness tracks the growing match set.

/// Which C oracle this encode targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvtHdrMode {
    /// Mainline SVT-AV1 v4.2.0 semantics (the port's primary target).
    #[default]
    Mainline,
    /// svt-av1-hdr fork semantics on the v4.2 base (hybrid MODE1).
    HdrFork,
}

/// Fork feature knobs + fork-overridden mainline defaults.
///
/// Mirrors the C hybrid's config surface. Every field is honored by the
/// code paths that have been ported; unported consumers are listed in the
/// status table (docs/HDR-ON-4.2.md) and tracked as tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrForkConfig {
    pub mode: SvtHdrMode,

    // --- fork-only feature fields (EbSvtAv1EncConfiguration additions) ---
    /// Sharp transform decisions in RDOQ (fork default ON).
    pub sharp_tx: u8,
    /// Keyframe TF strength 0-4 (fork default 1 = 4x weaker; 3 = mainline).
    pub kf_tf_strength: u8,
    /// Alternative RDO lambda factors (from SVT-AV1 3.0.2).
    pub alt_lambda_factors: bool,
    /// Alternative SSIM tuning (SB-normalized rdmult scaling).
    pub alt_ssim_tuning: bool,
    /// TX size/type + interp-filter sharpness bias 0-3.
    pub tx_bias: u8,
    /// Highest-fidelity HVS model (SSD-Psy) in mds0.
    pub complex_hvs: u8,
    /// AC-coefficient boost on low-noise blocks 0-4.
    pub noise_norm_strength: u8,
    /// Content-adaptive CDEF/restoration disable 0-4 (2 = default tune behavior).
    pub noise_adaptive_filtering: u8,
    /// CDEF strength scaling 1-30 (15 = 1x, neutral).
    pub cdef_scaling: u8,
    /// In-encoder photon-noise grain synthesis strength 0-200 (0 = off).
    pub noise_strength: u8,
    /// Chroma grain strength (-1 = ~60% of luma).
    pub noise_strength_chroma: i32,
    /// Chroma grain from luma plane.
    pub noise_chroma_from_luma: u8,
    /// Grain size (-1 = auto by resolution).
    pub noise_size: i8,
    /// AC bias in rate distortion 0.0-8.0 (fork default 1.0).
    pub ac_bias: f64,
    /// QP hierarchical-layer scale compression 0.0-8.0 (fork double form;
    /// mainline mode uses the uint8-indexed weight table semantics).
    pub qp_scale_compress_strength: f64,

    // --- mainline fields whose DEFAULTS the fork overrides ---
    pub enable_variance_boost: bool,
    pub variance_boost_strength: u8,
    pub variance_octile: u8,
    /// 0-2 mainline; 3 = fork-only PQ curve (auto-selected on PQ transfer).
    pub variance_boost_curve: u8,
    /// Loop-filter sharpness (fork default 1, mainline 0).
    pub sharpness: i8,
    pub enable_qm: bool,
    pub min_qm_level: u8,
    pub max_qm_level: u8,
    pub min_chroma_qm_level: u8,
    pub max_chroma_qm_level: u8,
    /// Temporal-filter strength (fork default 1, mainline 3).
    pub tf_strength: u8,
}

impl Default for HdrForkConfig {
    fn default() -> Self {
        Self::mainline()
    }
}

impl HdrForkConfig {
    /// Mainline v4.2.0 defaults — every fork feature neutral/off.
    /// Matches `enc_settings.c` `svt_av1_set_default_params` at
    /// `SVT_HDR_MODE=0` (and therefore stock v4.2.0-final).
    pub fn mainline() -> Self {
        Self {
            mode: SvtHdrMode::Mainline,
            sharp_tx: 0,
            kf_tf_strength: 3,
            alt_lambda_factors: false,
            alt_ssim_tuning: false,
            tx_bias: 0,
            complex_hvs: 0,
            noise_norm_strength: 0,
            noise_adaptive_filtering: 2,
            cdef_scaling: 15,
            noise_strength: 0,
            noise_strength_chroma: -1,
            noise_chroma_from_luma: 0,
            noise_size: -1,
            ac_bias: 0.0,
            qp_scale_compress_strength: 0.0,
            enable_variance_boost: false,
            variance_boost_strength: 2,
            variance_octile: 5,
            variance_boost_curve: 0,
            sharpness: 0,
            enable_qm: false,
            min_qm_level: 8,
            max_qm_level: 15,
            min_chroma_qm_level: 8,
            max_chroma_qm_level: 15,
            tf_strength: 3,
        }
    }

    /// svt-av1-hdr fork defaults — matches the C hybrid at `SVT_HDR_MODE=1`
    /// (enc_settings.c fork-default branches + the fork's README
    /// "Modified Defaults"). NOTE: preset/bit-depth/keyint defaults are
    /// caller decisions in this port and are not carried here.
    pub fn hdr_fork() -> Self {
        Self {
            mode: SvtHdrMode::HdrFork,
            sharp_tx: 1,
            kf_tf_strength: 1,
            alt_lambda_factors: true,
            alt_ssim_tuning: false,
            tx_bias: 0,
            complex_hvs: 0,
            noise_norm_strength: 1,
            noise_adaptive_filtering: 2,
            cdef_scaling: 15,
            noise_strength: 0,
            noise_strength_chroma: -1,
            noise_chroma_from_luma: 0,
            noise_size: -1,
            ac_bias: 1.0,
            qp_scale_compress_strength: 1.0,
            enable_variance_boost: true,
            variance_boost_strength: 2,
            variance_octile: 5,
            variance_boost_curve: 0,
            sharpness: 1,
            enable_qm: true,
            min_qm_level: 6,
            max_qm_level: 10,
            min_chroma_qm_level: 8,
            max_chroma_qm_level: 15,
            tf_strength: 1,
        }
    }

    /// True when any ported fork behavior may fire.
    #[inline]
    pub fn is_fork(&self) -> bool {
        self.mode == SvtHdrMode::HdrFork
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainline_defaults_are_neutral() {
        let c = HdrForkConfig::mainline();
        assert!(!c.is_fork());
        assert_eq!(c.sharp_tx, 0);
        assert_eq!(c.ac_bias, 0.0);
        assert!(!c.enable_variance_boost);
        assert!(!c.enable_qm);
        assert_eq!(c.sharpness, 0);
        assert_eq!(c.kf_tf_strength, 3);
        assert_eq!(c.qp_scale_compress_strength, 0.0);
    }

    #[test]
    fn fork_defaults_match_c_hybrid_mode1() {
        // Values pinned against enc_settings.c SVT_HDR_MODE=1 branches
        // (the C hybrid, commit 8cd68a072) + fork README Modified Defaults.
        let c = HdrForkConfig::hdr_fork();
        assert!(c.is_fork());
        assert_eq!(c.sharp_tx, 1);
        assert_eq!(c.kf_tf_strength, 1);
        assert!(c.alt_lambda_factors);
        assert_eq!(c.ac_bias, 1.0);
        assert_eq!(c.qp_scale_compress_strength, 1.0);
        assert!(c.enable_variance_boost);
        assert_eq!(c.variance_boost_strength, 2);
        assert_eq!(c.variance_octile, 5);
        assert_eq!(c.sharpness, 1);
        assert!(c.enable_qm);
        assert_eq!((c.min_qm_level, c.max_qm_level), (6, 10));
        assert_eq!(c.tf_strength, 1);
        assert_eq!(c.noise_norm_strength, 1);
    }

    #[test]
    fn default_is_mainline() {
        assert_eq!(HdrForkConfig::default(), HdrForkConfig::mainline());
    }
}

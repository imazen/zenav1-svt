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
/// Default tune (PSNR) — both C defaults.
const TUNE_PSNR_DEFAULT: u8 = 1;

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
    /// Fork `--tune` (0=VQ, 1=PSNR, 2=SSIM, 3=IQ, 4=MS_SSIM,
    /// 5=FILM_GRAIN). Default 1 matches both C defaults; the tune-policy
    /// deltas (tune.rs) apply in fork mode only.
    pub tune: u8,
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
            tune: TUNE_PSNR_DEFAULT,
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

    /// svt-av1-hdr UPSTREAM fork defaults — the values the fork's own
    /// `svt_av1_set_default_params` ships (fork README "Modified Defaults").
    ///
    /// **This is NOT the config the in-tree C oracle produces at
    /// `-DSVT_HDR_MODE=ON`** — use [`HdrForkConfig::hdr_fork_c_mode1`] for
    /// that, and for any byte comparison against `capture_c_trace`. The
    /// in-tree C hybrid deliberately neutralizes the fork's feature knobs
    /// UNCONDITIONALLY (`enc_settings.c:1181-1203`: `ac_bias = 0.0`,
    /// `sharp_tx = 0`, `noise_norm_strength = 0`, `alt_lambda_factors = 0`,
    /// `kf_tf_strength = 3`, `qp_scale_compress_strength = 0.0`) — they sit
    /// outside every `#if SVT_HDR_MODE` block, so compiling MODE1 does not
    /// turn them on. Only six defaults actually flip with the mode (bit
    /// depth, preset, QM, variance boost, `tf_strength`, `sharpness`).
    ///
    /// Keep this constructor for "what the shipped fork does"; it is the
    /// right target for feature-behavior work and the wrong one for oracle
    /// byte-identity. NOTE: preset/bit-depth/keyint defaults are caller
    /// decisions in this port and are not carried here.
    pub fn hdr_fork() -> Self {
        Self {
            mode: SvtHdrMode::HdrFork,
            sharp_tx: 1,
            kf_tf_strength: 1,
            alt_lambda_factors: true,
            alt_ssim_tuning: false,
            tune: TUNE_PSNR_DEFAULT,
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

    /// The config the IN-TREE C oracle loads at `-DSVT_HDR_MODE=ON`, i.e.
    /// what `svt_av1_enc_init_handle` hands back from a MODE1 build with no
    /// further overrides. **This is the byte-identity target** for
    /// `capture_c_trace` run under `SVT_HDR_MODE=1`.
    ///
    /// Derived field-by-field from `Source/Lib/Globals/enc_settings.c`
    /// `svt_av1_set_default_params`: the `#if SVT_HDR_MODE` branches flip
    /// exactly `enable_qm` (:1123), `min_qm_level` 6 (:1128), `max_qm_level`
    /// 10 (:1133), `enable_variance_boost` (:1149), `tf_strength` 1 (:1156)
    /// and `sharpness` 1 (:1163) — plus `encoder_bit_depth` 10 (:995) and
    /// `enc_mode` M4 (:1032), which are caller decisions here and so are not
    /// fields of this struct. Everything else keeps the neutralized value
    /// assigned unconditionally at :1181-1203.
    ///
    /// Fork mode is therefore NOT "all fork features on" — it is the fork's
    /// UNCONDITIONAL code-path deltas (unconditional loop filter, `double`
    /// variance pipeline, chroma-qindex derivation, light-RDOQ low-DC chroma,
    /// mds0 dist-type branching, `diff_uv_delta`/`separate_uv_delta_q`) plus
    /// those six defaults. Individual fork FEATURES are opted into on top,
    /// on both sides, via the shared `SVT_FORK_*` env knobs
    /// ([`HdrForkConfig::from_env`]).
    pub fn hdr_fork_c_mode1() -> Self {
        Self {
            mode: SvtHdrMode::HdrFork,
            // --- the six SVT_HDR_MODE-gated default flips ---
            enable_qm: true,
            min_qm_level: 6,
            max_qm_level: 10,
            enable_variance_boost: true,
            tf_strength: 1,
            sharpness: 1,
            // --- everything else: the unconditional neutral values ---
            ..Self::mainline()
        }
    }

    /// Build a config from the environment, using the SAME variable names the
    /// C driver (`rust/tools/capture_c_trace`) reads. One env vector then
    /// configures BOTH encoders, which is what makes a fork-mode byte
    /// comparison meaningful — two parallel naming schemes drift, and a knob
    /// that silently fails to reach one side looks exactly like a knob with no
    /// effect.
    ///
    /// - `SVT_HDR_MODE=1` → start from [`Self::hdr_fork_c_mode1`]; otherwise
    ///   [`Self::mainline`] (so an unset environment is today's behavior).
    /// - `SVT_FORK_<FIELD>` → override that field. Unparseable values are a
    ///   hard error rather than a silent fallback: a typo'd knob that quietly
    ///   encodes the default is the failure mode this whole path exists to
    ///   prevent.
    pub fn from_env() -> Self {
        let fork = std::env::var("SVT_HDR_MODE").map(|v| v == "1").unwrap_or(false);
        let mut c = if fork { Self::hdr_fork_c_mode1() } else { Self::mainline() };

        fn get<T: std::str::FromStr>(name: &str, slot: &mut T) {
            if let Ok(v) = std::env::var(name) {
                *slot = v
                    .parse()
                    .unwrap_or_else(|_| panic!("{name}: cannot parse {v:?} as {}", std::any::type_name::<T>()));
            }
        }
        fn get_bool(name: &str, slot: &mut bool) {
            let mut n: u8 = u8::from(*slot);
            get(name, &mut n);
            *slot = n != 0;
        }

        get("SVT_FORK_AC_BIAS", &mut c.ac_bias);
        get("SVT_FORK_QP_SCALE_COMPRESS_STRENGTH", &mut c.qp_scale_compress_strength);
        get("SVT_FORK_SHARP_TX", &mut c.sharp_tx);
        get("SVT_FORK_TX_BIAS", &mut c.tx_bias);
        get("SVT_FORK_COMPLEX_HVS", &mut c.complex_hvs);
        get("SVT_FORK_NOISE_NORM_STRENGTH", &mut c.noise_norm_strength);
        get("SVT_FORK_NOISE_ADAPTIVE_FILTERING", &mut c.noise_adaptive_filtering);
        get("SVT_FORK_CDEF_SCALING", &mut c.cdef_scaling);
        get("SVT_FORK_NOISE_STRENGTH", &mut c.noise_strength);
        get("SVT_FORK_NOISE_CHROMA_FROM_LUMA", &mut c.noise_chroma_from_luma);
        get("SVT_FORK_NOISE_STRENGTH_CHROMA", &mut c.noise_strength_chroma);
        get("SVT_FORK_NOISE_SIZE", &mut c.noise_size);
        get("SVT_FORK_KF_TF_STRENGTH", &mut c.kf_tf_strength);
        get("SVT_FORK_TF_STRENGTH", &mut c.tf_strength);
        get("SVT_FORK_TUNE", &mut c.tune);
        get("SVT_FORK_VARIANCE_BOOST_STRENGTH", &mut c.variance_boost_strength);
        get("SVT_FORK_VARIANCE_OCTILE", &mut c.variance_octile);
        get("SVT_FORK_VARIANCE_BOOST_CURVE", &mut c.variance_boost_curve);
        get("SVT_FORK_MIN_QM_LEVEL", &mut c.min_qm_level);
        get("SVT_FORK_MAX_QM_LEVEL", &mut c.max_qm_level);
        get("SVT_FORK_MIN_CHROMA_QM_LEVEL", &mut c.min_chroma_qm_level);
        get("SVT_FORK_MAX_CHROMA_QM_LEVEL", &mut c.max_chroma_qm_level);
        get("SVT_FORK_SHARPNESS", &mut c.sharpness);
        get_bool("SVT_FORK_ALT_LAMBDA_FACTORS", &mut c.alt_lambda_factors);
        get_bool("SVT_FORK_ALT_SSIM_TUNING", &mut c.alt_ssim_tuning);
        get_bool("SVT_FORK_ENABLE_QM", &mut c.enable_qm);
        get_bool("SVT_FORK_ENABLE_VARIANCE_BOOST", &mut c.enable_variance_boost);
        c
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

    /// The C oracle at -DSVT_HDR_MODE=ON flips exactly SIX config defaults;
    /// every other fork knob stays neutralized (enc_settings.c:1181-1203 sits
    /// outside all `#if SVT_HDR_MODE` blocks). Pinning this keeps a future
    /// "fork mode means all fork features on" assumption from silently
    /// desynchronising the Rust config from the oracle it is compared against.
    #[test]
    fn c_mode1_flips_exactly_six_defaults_vs_mainline() {
        let m = HdrForkConfig::mainline();
        let f = HdrForkConfig::hdr_fork_c_mode1();
        assert!(f.is_fork());

        // The six that DO flip (enc_settings.c :1123/:1128/:1133/:1149/:1156/:1163).
        assert!(f.enable_qm && !m.enable_qm);
        assert_eq!((f.min_qm_level, f.max_qm_level), (6, 10));
        assert!(f.enable_variance_boost && !m.enable_variance_boost);
        assert_eq!((f.tf_strength, m.tf_strength), (1, 3));
        assert_eq!((f.sharpness, m.sharpness), (1, 0));

        // The fork FEATURE knobs stay at mainline-neutral in MODE1 — this is
        // what separates hdr_fork_c_mode1() from hdr_fork().
        assert_eq!(f.ac_bias, m.ac_bias);
        assert_eq!(f.sharp_tx, m.sharp_tx);
        assert_eq!(f.noise_norm_strength, m.noise_norm_strength);
        assert_eq!(f.alt_lambda_factors, m.alt_lambda_factors);
        assert_eq!(f.kf_tf_strength, m.kf_tf_strength);
        assert_eq!(f.qp_scale_compress_strength, m.qp_scale_compress_strength);

        // ... and those six are exactly where the shipped fork's own defaults
        // (hdr_fork()) diverge from the oracle's — the distinction this test
        // exists to hold.
        let upstream = HdrForkConfig::hdr_fork();
        assert_ne!(upstream, f, "hdr_fork() must not be confused with the MODE1 oracle config");
        assert_eq!(upstream.ac_bias, 1.0);
        assert_eq!(upstream.sharp_tx, 1);
    }
}

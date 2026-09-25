//! SVT_HDR_MODE — the Rust mirror of the C hybrid's compile-time switch.
//!
//! The in-tree C reference carries the svt-av1-hdr (Chromedome) fork's
//! behavior behind `-DSVT_HDR_MODE=0/1` (see `docs/HDR-ON-4.2.md`). This
//! module is the Rust-side equivalent, as a RUNTIME config so one binary
//! can target either C oracle:
//!
//! - [`SvtHdrMode::Mainline`] → the pinned hybrid's MODE0 path
//!   (`cmake -DSVT_HDR_MODE=OFF`). Despite the historical name, this differs
//!   from pristine v4.2.0 in independent-chroma candidate ranking. See
//!   `docs/PARITY-REFERENCE-AUDIT-2026-09-08.md`; this switch alone is not a
//!   strict mainline parity policy.
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
//! standing MODE1 witness is `tools/hdr_bd10_gate.sh`, and README.md states
//! the current claim; `docs/HDR-ON-4.2.md` is the historical record of how
//! the fork was rebased onto v4.2, not a live status table. The oracle
//! carries only the fork's hdr-v4.1.0 "Chromedome" delta, so later fork
//! behavior is absent on both sides (see docs/CODE-REVIEW-2026-09-25.md).
//!
//! [`HdrForkConfig::ghost_robot`] is the third defaults arm: svt-av1-hdr
//! 4.2 "Ghost Robot" (`9dabe3ca`) ships the fork's real feature defaults
//! with no hybrid-style neutralization. [`HdrForkConfig::defaults_for`]
//! maps each (`SvtReference`, `SvtHdrMode`) pair to its oracle's
//! `svt_av1_set_default_params` output.

use crate::reference::SvtReference;

/// Which C oracle this encode targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvtHdrMode {
    /// Historical name for hybrid MODE0; not unrestricted pristine v4.2.0 parity.
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
/// code paths that have been ported; a knob with no ported consumer must be
/// refused at config validation rather than silently ignored.
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
    /// `--tune` (0=VQ, 1=PSNR, 2=SSIM, 3=IQ, 4=MS_SSIM, 5=VMAF — not ported,
    /// refused — 6=FILM_GRAIN, fork only). The numbering is the C oracles'
    /// (`definitions.h`, identical in the hybrid and Ghost Robot). Default 1
    /// matches both C defaults; the tune-policy
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
    /// C `static_config.max_tx_size` (32 or 64; default 64) — MAINLINE
    /// v4.2.0 (`Docs/Parameters.md:536-539`, recommended for still images).
    /// At 32 the partition search may not use 64x64 square blocks:
    /// `max_sq_size = MIN(max_sq_size, 32)` and `min_sq_size = MIN(min_sq_size,
    /// 32)` (`enc_dec_process.c:1494-1500`), plus the same cap in the
    /// depth-refinement (`:1815`). Set to 32 automatically by tune IQ at
    /// qp <= 45.
    pub max_tx_size: u8,
    /// C `static_config.screen_content_mode`. `None` = derive from the preset
    /// exactly as C's allintra rule does; `Some(0)` forces every class off,
    /// `Some(1)` forces every class on; `Some(3)` = force the auto-detector
    /// on, which is what tune IQ does regardless of preset.
    pub screen_content_mode: Option<u8>,
    pub enable_qm: bool,
    pub min_qm_level: u8,
    pub max_qm_level: u8,
    pub min_chroma_qm_level: u8,
    pub max_chroma_qm_level: u8,
    /// Temporal-filter strength (fork default 1, mainline 3).
    pub tf_strength: u8,
    /// C `luminance_qp_bias` (hybrid and Ghost Robot): 0 off, 1..=100 biases
    /// the frame QP by average luminance. Not ported: only 0 is accepted.
    pub luminance_qp_bias: u8,
    /// C `hbd_mds` (hybrid and Ghost Robot): -1 derives the 8/10-bit mode
    /// decision from the preset, which is what the port implements
    /// (`LeafBd10::mds3_hbd`); 0 (full 8-bit), 1 (full 10-bit) and 2 (hybrid)
    /// force it and are not ported.
    pub hbd_mds: i8,
    /// C `enable_qmpsnr` (Ghost Robot only): -1 auto (on at tune IQ), 0 PSNR,
    /// 1 QM-weighted PSNR. QM-PSNR is not ported (plan 3.5), so 1 is refused;
    /// -1 at tune IQ under Ghost Robot is the known remaining divergence.
    /// Must stay -1 for the other references, which have no such option.
    pub enable_qmpsnr: i8,
    /// C `max_hierarchical_levels` (Ghost Robot only): 0 off; >0 allows RTC
    /// CBR low-delay mini-GOP resizing, which is not ported.
    pub max_hierarchical_levels: u8,
}

impl Default for HdrForkConfig {
    fn default() -> Self {
        Self::mainline()
    }
}

impl HdrForkConfig {
    /// Mainline v4.2.0 defaults — every fork feature neutral/off.
    /// Matches `enc_settings.c` `svt_av1_set_default_params` at
    /// `SVT_HDR_MODE=0`. Reference-specific search is selected separately.
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
            max_tx_size: 64,
            screen_content_mode: None,
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
            luminance_qp_bias: 0,
            hbd_mds: -1,
            enable_qmpsnr: -1,
            max_hierarchical_levels: 0,
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
            // Mainline defaults (the fork does not change these two).
            max_tx_size: 64,
            screen_content_mode: None,
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
            luminance_qp_bias: 0,
            hbd_mds: -1,
            enable_qmpsnr: -1,
            max_hierarchical_levels: 0,
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
            luminance_qp_bias: 0,
            hbd_mds: -1,
            enable_qmpsnr: -1,
            max_hierarchical_levels: 0,
            sharpness: 1,
            // --- everything else: the unconditional neutral values ---
            ..Self::mainline()
        }
    }

    /// svt-av1-hdr 4.2 "Ghost Robot" (`9dabe3ca`) defaults — the values its
    /// own `svt_av1_set_default_params` ships (`reference/svt-av1-hdr`
    /// `Source/Lib/Globals/enc_settings.c:1010-1193`). Unlike the hybrid's
    /// MODE1 build ([`Self::hdr_fork_c_mode1`]) nothing is neutralized: the
    /// fork's real feature defaults all apply, on top of the same six flips
    /// MODE1 made (QM on at 6..10, variance boost, `tf_strength` 1,
    /// `sharpness` 1). Notably `alt_lambda_factors` ships OFF here
    /// (`:1185`) even though the older Chromedome fork shipped it on.
    ///
    /// This is the byte-identity target for `SVT_ORACLE=ghost-robot`:
    /// `capture_c_trace` under that oracle runs the C encoder on
    /// `svt_av1_enc_init_handle`'s output plus only the dimensions, preset,
    /// qp and bit depth the harness passes.
    ///
    /// Ghost-Robot-only config fields this struct does not yet carry
    /// (phase-1.2 surface) and their shipped defaults:
    /// `enable_qmpsnr = -1` (`:1190`; auto → on only at `TUNE_IQ`,
    /// resolved in `copy_api_from_app`, `enc_handle.c:4971-4972`),
    /// `luminance_qp_bias = 0` (`:1158`), `hbd_mds = DEFAULT` (`:1174`),
    /// `max_hierarchical_levels = 0` (`:1181`). These four are fields of this
    /// struct, refused at any value but the default until ported
    /// (`SvtReference::validate_hdr_config`). The defaults are neutral on the
    /// ported paths, except at tune IQ, where C turns QM-PSNR on and the port
    /// cannot match until that metric lands (plan item 3.5).
    ///
    /// Caller-decision fields (`encoder_bit_depth` 10 at `:1019`,
    /// `enc_mode` M4 at `:1052`) are inputs to this port's pipeline, not
    /// carried here — same convention as the other two constructors.
    pub fn ghost_robot() -> Self {
        Self {
            mode: SvtHdrMode::HdrFork,
            sharp_tx: 1,                     // enc_settings.c:1186
            kf_tf_strength: 1,               // :1184
            alt_lambda_factors: false,       // :1185
            alt_ssim_tuning: false,          // :1187
            tune: TUNE_PSNR_DEFAULT,         // :1109
            tx_bias: 0,                      // :1188
            complex_hvs: 0,                  // :1189
            noise_norm_strength: 1,          // :1183
            noise_adaptive_filtering: 2,     // :1191
            cdef_scaling: 15,                // :1192
            noise_strength: 0,               // :1091
            noise_strength_chroma: -1,       // :1092
            noise_chroma_from_luma: 0,       // :1093
            noise_size: -1,                  // :1094
            ac_bias: 1.0,                    // :1173
            qp_scale_compress_strength: 1.0, // :1162
            enable_variance_boost: true,     // :1153
            variance_boost_strength: 2,      // :1154
            variance_octile: 5,              // :1155
            variance_boost_curve: 0,         // :1157
            sharpness: 1,                    // :1159
            max_tx_size: 64,                 // :1171
            screen_content_mode: None,       // :1080 (2 = auto)
            enable_qm: true,                 // :1139
            min_qm_level: 6,                 // :1140
            max_qm_level: 10,                // :1141
            min_chroma_qm_level: 8,          // :1142
            max_chroma_qm_level: 15,         // :1143
            tf_strength: 1,                  // :1156
            luminance_qp_bias: 0,             // :1158
            hbd_mds: -1,                      // :1174
            enable_qmpsnr: -1,                // :1190
            max_hierarchical_levels: 0,       // :1181
        }
    }

    /// The defaults the pinned C oracle for (`reference`, `mode`) loads
    /// from its `svt_av1_set_default_params` — the single place that
    /// mapping lives, so `identity_run`, the output pins and the facade
    /// cannot resolve it three different ways.
    ///
    /// `SvtReference::GhostRobot` + `SvtHdrMode::Mainline` returns
    /// [`Self::mainline`]: that combination is refused by
    /// [`SvtReference::validate_hdr_config`] (the fork has no mainline
    /// mode), and returning the mainline defaults is what keeps that
    /// refusal — and its output pin — in place.
    pub fn defaults_for(reference: SvtReference, mode: SvtHdrMode) -> Self {
        match mode {
            SvtHdrMode::HdrFork => match reference {
                SvtReference::GhostRobot => Self::ghost_robot(),
                _ => Self::hdr_fork_c_mode1(),
            },
            SvtHdrMode::Mainline => Self::mainline(),
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
    #[cfg(feature = "std")]
    pub fn from_env() -> Self {
        let fork = crate::dbgenv::raw_var("SVT_HDR_MODE")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self::from_env_with_mode(if fork {
            SvtHdrMode::HdrFork
        } else {
            SvtHdrMode::Mainline
        })
    }

    /// [`Self::from_env`] with the base mode given by the caller instead of
    /// read from `SVT_HDR_MODE`. Mode alone cannot distinguish the two fork
    /// oracles — `GhostRobot` and `hybrid-3115-hdr` are both `HdrFork` — so a
    /// harness that selects its oracle by name (`SVT_ORACLE`,
    /// `rust/oracles/oracles.tsv`) should call
    /// [`Self::from_env_for_reference`], where the registry row fixes the
    /// reference. `SVT_FORK_*` overrides still apply.
    #[cfg(feature = "std")]
    pub fn from_env_with_mode(mode: SvtHdrMode) -> Self {
        let mut c = match mode {
            SvtHdrMode::HdrFork => Self::hdr_fork_c_mode1(),
            SvtHdrMode::Mainline => Self::mainline(),
        };
        c.apply_env_overrides();
        c
    }

    /// [`Self::from_env_with_mode`] keyed on the pinned reference as well as
    /// the mode: `GhostRobot` resolves [`Self::ghost_robot`] defaults rather
    /// than the hybrid MODE1 set the bare-mode entry assumes.
    #[cfg(feature = "std")]
    pub fn from_env_for_reference(reference: SvtReference, mode: SvtHdrMode) -> Self {
        let mut c = Self::defaults_for(reference, mode);
        c.apply_env_overrides();
        c
    }

    /// The `SVT_FORK_*` overrides — applied identically to every base so a
    /// knob can never reach only one arm of the (reference, mode) matrix.
    #[cfg(feature = "std")]
    fn apply_env_overrides(&mut self) {
        let c = self;
        fn get<T: std::str::FromStr>(name: &str, slot: &mut T) {
            if let Ok(v) = crate::dbgenv::raw_var(name) {
                *slot = v.parse().unwrap_or_else(|_| {
                    panic!(
                        "{name}: cannot parse {v:?} as {}",
                        std::any::type_name::<T>()
                    )
                });
            }
        }
        fn get_bool(name: &str, slot: &mut bool) {
            let mut n: u8 = u8::from(*slot);
            get(name, &mut n);
            *slot = n != 0;
        }

        get("SVT_FORK_AC_BIAS", &mut c.ac_bias);
        get(
            "SVT_FORK_QP_SCALE_COMPRESS_STRENGTH",
            &mut c.qp_scale_compress_strength,
        );
        get("SVT_FORK_SHARP_TX", &mut c.sharp_tx);
        get("SVT_FORK_TX_BIAS", &mut c.tx_bias);
        get("SVT_FORK_COMPLEX_HVS", &mut c.complex_hvs);
        get("SVT_FORK_NOISE_NORM_STRENGTH", &mut c.noise_norm_strength);
        get(
            "SVT_FORK_NOISE_ADAPTIVE_FILTERING",
            &mut c.noise_adaptive_filtering,
        );
        get("SVT_FORK_CDEF_SCALING", &mut c.cdef_scaling);
        get("SVT_FORK_NOISE_STRENGTH", &mut c.noise_strength);
        get(
            "SVT_FORK_NOISE_CHROMA_FROM_LUMA",
            &mut c.noise_chroma_from_luma,
        );
        get(
            "SVT_FORK_NOISE_STRENGTH_CHROMA",
            &mut c.noise_strength_chroma,
        );
        get("SVT_FORK_NOISE_SIZE", &mut c.noise_size);
        get("SVT_FORK_KF_TF_STRENGTH", &mut c.kf_tf_strength);
        get("SVT_FORK_TF_STRENGTH", &mut c.tf_strength);
        get("SVT_FORK_TUNE", &mut c.tune);
        get(
            "SVT_FORK_VARIANCE_BOOST_STRENGTH",
            &mut c.variance_boost_strength,
        );
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
        get_bool(
            "SVT_FORK_ENABLE_VARIANCE_BOOST",
            &mut c.enable_variance_boost,
        );
    }

    /// `no_std` has no process environment, so the `SVT_HDR_MODE` / `SVT_FORK_*`
    /// override knobs cannot be read — a `no_std` build always uses the mainline
    /// defaults (identical to an unset environment in the std build).
    #[cfg(not(feature = "std"))]
    pub fn from_env() -> Self {
        Self::mainline()
    }

    /// True when any ported fork behavior may fire.
    #[inline]
    /// C `svt_av1_enc_set_parameter`'s TUNE_IQ / TUNE_MS_SSIM override block
    /// (`enc_handle.c:4889-4915`), applied verbatim.
    ///
    /// `--tune 3` (IQ, documented "still image only") is not a single RD knob:
    /// C rewrites seven settings when it is selected, so a port that honours
    /// `tune` alone is not honouring `--tune 3`. `qp` is the CLI-domain
    /// quantizer (C `static_config.qp`), which is what C keys `max_tx_size` on.
    ///
    /// Idempotent, and a no-op for every other tune — so calling it
    /// unconditionally at encode time leaves tunes 0/1/2/5 byte-unchanged.
    pub fn apply_tune_overrides(&mut self, qp: u8) {
        if self.tune == crate::tune::TUNE_IQ || self.tune == crate::tune::TUNE_MS_SSIM {
            self.enable_qm = true;
            self.min_qm_level = 4;
            self.max_qm_level = 10;
            self.min_chroma_qm_level = 4;
            self.max_chroma_qm_level = 10;
            self.sharpness = 7;
            self.enable_variance_boost = true;
            self.variance_boost_strength = 3;
            self.variance_boost_curve = 2;
        }
        if self.tune == crate::tune::TUNE_IQ {
            // IQ-only, on top of the shared block above.
            self.max_tx_size = if qp <= 45 { 32 } else { 64 };
            self.screen_content_mode = Some(3);
        }
    }

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
        assert_ne!(
            upstream, f,
            "hdr_fork() must not be confused with the MODE1 oracle config"
        );
        assert_eq!(upstream.ac_bias, 1.0);
        assert_eq!(upstream.sharp_tx, 1);
    }

    /// Ghost Robot ships the fork's real feature defaults with no
    /// neutralization (enc_settings.c:1010-1193 @ 9dabe3ca): the same six
    /// flips MODE1 made, plus the fork knobs the hybrid forced off. Pinned
    /// so a careless "= hdr_fork_c_mode1()" or "= hdr_fork()" edit fails
    /// loudly — neither is this oracle's config.
    #[test]
    fn ghost_robot_defaults_are_the_fork_defaults_not_the_hybrids() {
        let g = HdrForkConfig::ghost_robot();
        assert!(g.is_fork());
        // The six MODE1 flips Ghost Robot shares.
        assert!(g.enable_qm);
        assert_eq!((g.min_qm_level, g.max_qm_level), (6, 10));
        assert_eq!((g.min_chroma_qm_level, g.max_chroma_qm_level), (8, 15));
        assert!(g.enable_variance_boost);
        assert_eq!((g.tf_strength, g.sharpness), (1, 1));
        // The five knobs the hybrid neutralized; Ghost Robot ships them on.
        assert_eq!(g.sharp_tx, 1);
        assert_eq!(g.kf_tf_strength, 1);
        assert_eq!(g.noise_norm_strength, 1);
        assert_eq!(g.ac_bias, 1.0);
        assert_eq!(g.qp_scale_compress_strength, 1.0);
        // Alt lambda factors flipped back OFF in Ghost Robot
        // (enc_settings.c:1185) — it matches MODE1 here and is where
        // ghost_robot() differs from the shipped Chromedome defaults
        // (hdr_fork()'s `true`).
        assert!(!g.alt_lambda_factors);
        assert_ne!(g, HdrForkConfig::hdr_fork_c_mode1());
        assert_ne!(g, HdrForkConfig::hdr_fork());

        // defaults_for maps each (reference, mode) pair to its oracle.
        assert_eq!(
            HdrForkConfig::defaults_for(SvtReference::GhostRobot, SvtHdrMode::HdrFork),
            g
        );
        assert_eq!(
            HdrForkConfig::defaults_for(SvtReference::Hybrid3115, SvtHdrMode::HdrFork),
            HdrForkConfig::hdr_fork_c_mode1()
        );
        for r in [
            SvtReference::Mainline420,
            SvtReference::Hybrid3115,
            SvtReference::GhostRobot,
        ] {
            assert_eq!(
                HdrForkConfig::defaults_for(r, SvtHdrMode::Mainline),
                HdrForkConfig::mainline(),
                "{r:?} + Mainline resolves the mainline defaults so the \
                 (GhostRobot, Mainline) refusal pin stays REFUSED"
            );
        }
    }
}

//! Every environment variable `identity_run` reads, parsed once, strictly.
//!
//! A cell is the positional arguments plus this environment. Before this
//! module the reads were scattered through `main`, most as
//! `.and_then(|v| v.parse().ok()).unwrap_or(default)`, so a malformed value
//! silently encoded the default (`SVTAV1_BD=1O` was an 8-bit cell), a
//! presence flag set to `0` turned the feature ON, and a still-only knob on a
//! multi-frame cell was dropped without a word (`SVTAV1_SB=128` on the
//! multi-frame path coded the derived size). Here a value that does not parse
//! is refused, and so is a knob the chosen path would not apply.
//!
//! Conventions: unset and empty mean "default". A flag is `1` (on) or `0`
//! (off). The `SVT_*` names are the ones the C driver reads, so one
//! environment configures both encoders. `SVT_FORK_*` is read by the library
//! (`HdrForkConfig::from_env_for_reference`), not here.

use std::str::FromStr;

use svtav1_encoder::hdr_mode::{HdrForkConfig, SvtHdrMode};
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::RcMode;
use svtav1_encoder::reference::SvtReference;

/// The value of `name`, with empty treated as unset.
fn raw(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `name` parsed as `T`; a value that does not parse is refused.
fn num<T: FromStr>(name: &str) -> Option<T> {
    raw(name).map(|v| {
        v.parse().unwrap_or_else(|_| {
            panic!("{name}={v:?} does not parse as a number of the expected type")
        })
    })
}

/// `name` as a flag: `1` on, `0` or unset off, anything else refused.
fn flag(name: &str) -> bool {
    match raw(name).as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        Some(v) => panic!("{name}={v:?}: a flag is 1 or 0"),
    }
}

/// Film grain, shared with the C capture driver's `SVT_GRAIN_*` names.
pub struct Grain {
    /// `SVT_GRAIN_TABLE` (present = on): the fixed table below; `cfl` scales
    /// chroma from luma, `no_y` drops the luma points.
    pub table: Option<String>,
    /// `SVT_GRAIN_IGNORE_REF` (present = on).
    pub ignore_ref: bool,
    /// `SVT_GRAIN_STRENGTH`.
    pub strength: Option<u8>,
    /// `SVT_GRAIN_APPLY` (0/1).
    pub apply: Option<u8>,
    /// `SVT_GRAIN_ADAPTIVE` (0/1).
    pub adaptive: Option<u8>,
}

/// Diagnostic dump paths. Each writes a file and changes no coded byte.
pub struct Dumps {
    /// `SVTAV1_FINAL_RECON=<path>`: the final (post-filter) recon, cropped to
    /// the true dims, comparable with a decoder's output. Multi-frame runs
    /// write `<path>.f<display order>`.
    pub final_recon: Option<String>,
    /// `SVTAV1_RECON_STAGES=<pfx>` (multi-frame): the pre-deblock recon per
    /// frame, `<pfx>.f<i>.pre.bin`, plus `.pre10.bin` at 10 bits.
    pub recon_stages: Option<String>,
    /// `SVTAV1_RECON_DUMP=<pfx>` (still): uncropped pre/post-deblock recon.
    pub recon_dump: Option<String>,
    /// `SVTAV1_BD10_RECON=<path>` (still): the 10-bit luma recon.
    pub bd10_recon: Option<String>,
    /// `SVTAV1_SR_DUMP=<path>` (still, superres): the downscaled source.
    pub sr_dump: Option<String>,
}

/// One identity cell's configuration, beyond the positional arguments.
pub struct CellSpec {
    /// `SVTAV1_BD`: 8 (default) or 10.
    pub bd: u8,
    /// `SVTAV1_HBD_SRC`: a real 10-bit source instead of `u8 << 2`.
    pub hbd_src: bool,
    /// `SVTAV1_HBD_PQ`: that source's low bits come from a PQ curve.
    pub hbd_pq: bool,
    /// `SVTAV1_MONO`: code luma only.
    pub mono: bool,
    /// `SVT_CHROMA`: 420 (default) or 444 — shared with the C driver, which
    /// encodes the same full-res-chroma .yuv as EB_YUV444/profile 1 on
    /// oracles that accept it (ghost-robot only; mainline and the hybrid
    /// refuse at `svt_av1_verify_settings`).
    pub chroma: svtav1_types::chroma::ChromaFormat,
    /// `SVTAV1_TIMEOUT_MS`: a deadline over the whole run (exit 4 on expiry).
    pub timeout: Option<core::time::Duration>,
    /// `SVTAV1_ASSERT_NONFLAT`: refuse a `crop@` window of one colour.
    pub assert_nonflat: bool,

    /// `SVTAV1_FRAMES`: 1..=256; above 1 takes the multi-frame path.
    pub frames: usize,
    /// `SVTAV1_FRAME_SHIFT`: px/frame of the synthetic warp (default 3).
    pub frame_shift: Option<usize>,
    /// `SVTAV1_FRAME_ZOOM_NUM` / `_DEN`: per-frame zoom of the warp (1/1).
    pub zoom: Option<(i64, i64)>,
    /// `SVTAV1_INTRA_PERIOD` (default 64).
    pub intra_period: u32,
    /// `SVTAV1_HIER_LEVELS` (default: C's `HIERARCHICAL_LEVELS_AUTO`).
    pub hier_levels: u8,
    /// `SVTAV1_RC_MODE`: 0 CQP (default), 1 VBR, 2 CBR.
    pub rc_mode: RcMode,
    /// `SVTAV1_TBR` (kbps), `SVTAV1_VBV` (ms), `SVTAV1_FPS`.
    pub tbr: u32,
    pub vbv: u32,
    pub fps: f64,
    /// `SVT_PRED_STRUCT`: 1 low delay (default), 2 random access.
    pub random_access: bool,
    /// `SVT_ENABLE_TF`: 0 disables temporal filtering.
    pub enable_tf: bool,

    /// `SVT_AQ_MODE` (default 0).
    pub aq_mode: u8,
    /// `SVTAV1_SB`: force 64 or 128; unset derives with C's rule.
    pub sb: Option<usize>,
    /// Still only: `SVTAV1_TILE_ROWS_LOG2` / `SVTAV1_TILE_COLS_LOG2`.
    pub tile_rows_log2: u8,
    pub tile_cols_log2: u8,
    /// Still only: `SVTAV1_SUPERRES=<denom 9..16>`.
    pub superres: Option<u8>,
    /// Still only: `SVTAV1_Y_STRIDE`, a luma stride wider than the frame.
    pub y_stride: Option<usize>,
    /// `SVTAV1_TUNE`. Still only: `SVTAV1_SCM`, `SVTAV1_MAX_TX_SIZE`,
    /// `SVTAV1_CRF_OFFSET`, `SVTAV1_CSP`.
    pub tune: Option<u8>,
    pub scm: Option<u8>,
    pub max_tx_size: Option<u8>,
    pub crf_offset: Option<u8>,
    pub csp: Option<u8>,
    /// `SVTAV1_SCREEN_TOOLS`, `SVTAV1_DEEP_SEARCH`: Zen enhancements.
    pub screen_tools: bool,
    pub deep_search: bool,
    pub grain: Grain,

    /// `SVT_ORACLE`: the registry row that fixes reference and HDR mode.
    pub oracle: Option<String>,
    /// `SVTAV1_REFERENCE`: a pinned source id (legacy switch).
    pub reference: Option<SvtReference>,
    /// `SVT_HDR_MODE`: 1 the fork mode (legacy switch).
    pub hdr_mode: Option<SvtHdrMode>,

    pub dumps: Dumps,
}

/// Enhancements removed on measurement (benchmarks/aom_keep_or_drop_2026-09-25.meta).
const REMOVED: [&str; 2] = [
    "SVTAV1_ZEN_INTRA_EDGE_FILTER",
    "SVTAV1_ZEN_RESTORATION_UNIT_SEARCH",
];

impl CellSpec {
    pub fn from_env() -> Self {
        for gone in REMOVED {
            if raw(gone).is_some_and(|v| v != "0") {
                panic!("{gone}: this Zen enhancement was removed on 2026-09-25 (no RD gain)");
            }
        }
        let bd = num("SVTAV1_BD").unwrap_or(8);
        let hbd_src = bd > 8 && flag("SVTAV1_HBD_SRC");
        let frames = num("SVTAV1_FRAMES").unwrap_or(1);
        assert!(
            (1..=256).contains(&frames),
            "SVTAV1_FRAMES must be 1..256, got {frames}"
        );
        let zoom = match (
            num::<i64>("SVTAV1_FRAME_ZOOM_NUM"),
            num::<i64>("SVTAV1_FRAME_ZOOM_DEN"),
        ) {
            (None, None) => None,
            (n, d) => Some((n.unwrap_or(1), d.unwrap_or(1))),
        };
        if let Some((n, d)) = zoom {
            assert!(
                (1..=64).contains(&n) && (1..=64).contains(&d),
                "SVTAV1_FRAME_ZOOM_NUM/_DEN must each be 1..=64, got {n}/{d}"
            );
        }
        let sb = num("SVTAV1_SB");
        assert!(
            matches!(sb, None | Some(64) | Some(128)),
            "SVTAV1_SB must be 64 or 128, got {sb:?}"
        );
        let hdr_mode = match raw("SVT_HDR_MODE").as_deref() {
            None => None,
            Some("0") => Some(SvtHdrMode::Mainline),
            Some("1") => Some(SvtHdrMode::HdrFork),
            Some(v) => panic!("SVT_HDR_MODE={v:?}: 1 selects the fork mode, 0 mainline"),
        };
        let spec = Self {
            bd,
            hbd_src,
            hbd_pq: hbd_src && flag("SVTAV1_HBD_PQ"),
            mono: flag("SVTAV1_MONO"),
            chroma: match raw("SVT_CHROMA").as_deref() {
                None | Some("420") => svtav1_types::chroma::ChromaFormat::Yuv420,
                Some("444") => svtav1_types::chroma::ChromaFormat::Yuv444,
                other => panic!(
                    "SVT_CHROMA={other:?}: 420 or 444 (400 is SVTAV1_MONO; \
                     422's port arm is not shipped)"
                ),
            },
            timeout: num("SVTAV1_TIMEOUT_MS").map(core::time::Duration::from_millis),
            assert_nonflat: flag("SVTAV1_ASSERT_NONFLAT"),
            frames,
            frame_shift: num("SVTAV1_FRAME_SHIFT"),
            zoom,
            intra_period: num("SVTAV1_INTRA_PERIOD").unwrap_or(64),
            hier_levels: num("SVTAV1_HIER_LEVELS")
                .unwrap_or(svtav1_encoder::port_picstruct::HIERARCHICAL_LEVELS_AUTO),
            rc_mode: match raw("SVTAV1_RC_MODE").as_deref() {
                None | Some("0") => RcMode::Cqp,
                Some("1") => RcMode::Vbr,
                Some("2") => RcMode::Cbr,
                other => panic!("SVTAV1_RC_MODE must be 0/1/2, got {other:?}"),
            },
            tbr: num("SVTAV1_TBR").unwrap_or(0),
            vbv: num("SVTAV1_VBV").unwrap_or(1000),
            fps: num("SVTAV1_FPS").unwrap_or(30.0),
            random_access: match raw("SVT_PRED_STRUCT").as_deref() {
                None | Some("1") => false,
                Some("2") => true,
                Some(v) => panic!("SVT_PRED_STRUCT={v:?}: 1 is low delay, 2 random access"),
            },
            enable_tf: raw("SVT_ENABLE_TF").is_none() || flag("SVT_ENABLE_TF"),
            aq_mode: num("SVT_AQ_MODE").unwrap_or(0),
            sb,
            tile_rows_log2: num("SVTAV1_TILE_ROWS_LOG2").unwrap_or(0),
            tile_cols_log2: num("SVTAV1_TILE_COLS_LOG2").unwrap_or(0),
            superres: num("SVTAV1_SUPERRES"),
            y_stride: num("SVTAV1_Y_STRIDE"),
            tune: num("SVTAV1_TUNE"),
            scm: num("SVTAV1_SCM"),
            max_tx_size: num("SVTAV1_MAX_TX_SIZE"),
            crf_offset: num("SVTAV1_CRF_OFFSET"),
            csp: num("SVTAV1_CSP"),
            screen_tools: flag("SVTAV1_SCREEN_TOOLS"),
            deep_search: flag("SVTAV1_DEEP_SEARCH"),
            grain: Grain {
                // Presence, not value: the C driver tests `getenv(..) != NULL`
                // for these two, so `=0` or empty must mean "set" here too.
                table: std::env::var("SVT_GRAIN_TABLE").ok(),
                ignore_ref: std::env::var_os("SVT_GRAIN_IGNORE_REF").is_some(),
                strength: num("SVT_GRAIN_STRENGTH"),
                apply: num("SVT_GRAIN_APPLY"),
                adaptive: num("SVT_GRAIN_ADAPTIVE"),
            },
            oracle: raw("SVT_ORACLE"),
            reference: raw("SVTAV1_REFERENCE").map(|v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("SVTAV1_REFERENCE={v:?} is not a pinned source id"))
            }),
            hdr_mode,
            dumps: Dumps {
                final_recon: raw("SVTAV1_FINAL_RECON"),
                recon_stages: raw("SVTAV1_RECON_STAGES"),
                recon_dump: raw("SVTAV1_RECON_DUMP"),
                bd10_recon: raw("SVTAV1_BD10_RECON"),
                sr_dump: raw("SVTAV1_SR_DUMP"),
            },
        };
        if spec.mono && spec.chroma == svtav1_types::chroma::ChromaFormat::Yuv444 {
            panic!("SVTAV1_MONO with SVT_CHROMA=444: monochrome carries no chroma planes");
        }
        if spec.frames > 1 {
            spec.refuse_still_only();
        }
        spec
    }

    /// The multi-frame path applies none of these; accepting one there would
    /// report a cell that was never encoded.
    fn refuse_still_only(&self) {
        let set: Vec<&str> = [
            ("SVTAV1_TILE_ROWS_LOG2", self.tile_rows_log2 != 0),
            ("SVTAV1_TILE_COLS_LOG2", self.tile_cols_log2 != 0),
            ("SVTAV1_SUPERRES", self.superres.is_some()),
            ("SVTAV1_Y_STRIDE", self.y_stride.is_some()),
            ("SVTAV1_SCM", self.scm.is_some()),
            ("SVTAV1_MAX_TX_SIZE", self.max_tx_size.is_some()),
            ("SVTAV1_CRF_OFFSET", self.crf_offset.is_some()),
            ("SVTAV1_CSP", self.csp.is_some()),
            ("SVTAV1_RECON_DUMP", self.dumps.recon_dump.is_some()),
            ("SVTAV1_BD10_RECON", self.dumps.bd10_recon.is_some()),
            ("SVTAV1_SR_DUMP", self.dumps.sr_dump.is_some()),
        ]
        .into_iter()
        .filter_map(|(n, on)| on.then_some(n))
        .collect();
        assert!(
            set.is_empty(),
            "SVTAV1_FRAMES={}: {} apply only to a still; the multi-frame path would ignore them",
            self.frames,
            set.join(", ")
        );
        // A real 10-bit source has no multi-frame producer; SVTAV1_BD=10 alone
        // widens the 8-bit frames instead.
        assert!(
            !self.hbd_src,
            "SVTAV1_FRAMES>1 with SVTAV1_HBD_SRC has no multi-frame 10-bit source \
             producer; use SVTAV1_BD=10 alone to widen the 8-bit frames"
        );
    }

    /// Grain, oracle, tune and enhancements, in the order both paths have
    /// always applied them: the oracle resets `pipeline.hdr`, so the tune
    /// must follow it.
    pub fn apply_common(&self, p: &mut EncodePipeline) {
        self.apply_grain(p);
        self.apply_oracle(p);
        if let Some(t) = self.tune {
            p.hdr.tune = t;
        }
        self.apply_enhancements(p);
    }

    /// The still path's extra knobs, applied after [`Self::apply_common`].
    pub fn apply_still(&self, p: &mut EncodePipeline) {
        if let Some(v) = self.scm {
            p.hdr.screen_content_mode = Some(v);
        }
        if let Some(v) = self.max_tx_size {
            p.hdr.max_tx_size = v;
        }
        if let Some(v) = self.crf_offset {
            p.rc_config.extended_crf_qindex_offset = v;
        }
        if let Some(v) = self.csp {
            p.chroma_sample_position = v;
        }
    }

    fn apply_enhancements(&self, p: &mut EncodePipeline) {
        use svtav1_encoder::enhancements::ZenEnhancement;
        // AOM-style screen tools on stills: palette + IntraBC stay enabled at
        // every preset when the detector says screen.
        if self.screen_tools {
            p.enhancements = p.enhancements.with(ZenEnhancement::AomScreenTools);
            eprintln!("SVTAV1_ENHANCEMENT=aom-screen-tools-v1");
        }
        // Deep search: the leaf-funnel ladders, depth refinement and RDOQ
        // evaluate at enc_mode -1; rate, lambda and headers keep the preset.
        if self.deep_search {
            p.enhancements = p.enhancements.with(ZenEnhancement::DeepSearch);
            eprintln!("SVTAV1_ENHANCEMENT=deep-search-v1");
        }
    }

    fn apply_grain(&self, p: &mut EncodePipeline) {
        let g = &self.grain;
        if let Some(kind) = &g.table {
            let mut table = svtav1_encoder::entropy::obu::FilmGrainParams {
                apply_grain: true,
                num_y_points: 2,
                num_cb_points: 2,
                num_cr_points: 2,
                scaling_shift: 8,
                ar_coeff_lag: 1,
                ar_coeff_shift: 7,
                cb_mult: 128,
                cr_mult: 128,
                cb_luma_mult: 192,
                cr_luma_mult: 192,
                cb_offset: 256,
                cr_offset: 256,
                overlap_flag: true,
                ..Default::default()
            };
            table.scaling_points_y[..2].copy_from_slice(&[[0, 20], [255, 35]]);
            table.scaling_points_cb[..2].copy_from_slice(&[[0, 15], [255, 15]]);
            table.scaling_points_cr[..2].copy_from_slice(&[[0, 25], [255, 25]]);
            table.ar_coeffs_y[0] = 3;
            table.ar_coeffs_y[3] = -4;
            table.ar_coeffs_cb[4] = 8;
            table.ar_coeffs_cr[4] = -5;
            match kind.as_str() {
                "cfl" => {
                    table.chroma_scaling_from_luma = true;
                    table.num_cb_points = 0;
                    table.num_cr_points = 0;
                }
                "no_y" => table.num_y_points = 0,
                _ => {}
            }
            p.film_grain.table = Some(table);
            p.film_grain.ignore_ref = g.ignore_ref;
        }
        if let Some(v) = g.strength {
            p.film_grain.denoise_strength = v;
        }
        if let Some(v) = g.apply {
            p.film_grain.denoise_apply = v != 0;
        }
        if let Some(v) = g.adaptive {
            p.film_grain.adaptive = v != 0;
        }
    }

    /// `SVT_ORACLE=<name>`, the switch the C driver reads too
    /// (`rust/tools/oracle`, `rust/oracles/oracles.tsv`): the registry row
    /// fixes the Rust `SvtReference` and HDR mode. The registry is compiled
    /// in, so this binary and the C wrapper cannot read different tables.
    /// Unset, the legacy switches apply: `SVT_HDR_MODE` the mode and
    /// `SVTAV1_REFERENCE` the source. `SVT_ORACLE` with a disagreeing legacy
    /// switch is refused, never silently resolved.
    fn apply_oracle(&self, p: &mut EncodePipeline) {
        const REGISTRY: &str = include_str!("../../../oracles/oracles.tsv");
        let Some(name) = &self.oracle else {
            if let Some(r) = self.reference {
                p.reference = r;
                eprintln!("SVTAV1_REFERENCE={}", r.id());
            }
            let mode = self.hdr_mode.unwrap_or(SvtHdrMode::Mainline);
            // The legacy fork switch pairs with the hybrid-3115-hdr oracle
            // (`tools/oracle`'s default for SVT_HDR_MODE=1), so without an
            // explicit reference it means Hybrid3115, not the default.
            if mode == SvtHdrMode::HdrFork && self.reference.is_none() {
                p.reference = SvtReference::Hybrid3115;
            }
            p.hdr = HdrForkConfig::from_env_for_reference(p.reference, mode);
            return;
        };
        let header: Vec<&str> = REGISTRY
            .lines()
            .find(|l| l.starts_with("#name"))
            .expect("oracles.tsv has a #name header")
            .trim_start_matches('#')
            .split('\t')
            .collect();
        let col = |c: &str| {
            header
                .iter()
                .position(|h| *h == c)
                .expect("registry column")
        };
        let row: Vec<&str> = REGISTRY
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| l.split('\t').collect::<Vec<_>>())
            .find(|r| r[col("name")] == name)
            .unwrap_or_else(|| panic!("SVT_ORACLE={name} is not in rust/oracles/oracles.tsv"));
        let mode = match row[col("rust_hdr_mode")] {
            "HdrFork" => SvtHdrMode::HdrFork,
            "Mainline" => SvtHdrMode::Mainline,
            other => panic!("oracles.tsv: unknown rust_hdr_mode {other}"),
        };
        let reference = match row[col("rust_reference")] {
            "Mainline420" => SvtReference::Mainline420,
            "Hybrid3115" => SvtReference::Hybrid3115,
            "GhostRobot" => SvtReference::GhostRobot,
            other => panic!("oracles.tsv: unknown rust_reference {other}"),
        };
        if let Some(legacy) = self.hdr_mode {
            assert_eq!(
                legacy, mode,
                "SVT_ORACLE={name} conflicts with SVT_HDR_MODE; unset one"
            );
        }
        if let Some(r) = self.reference {
            assert_eq!(
                r, reference,
                "SVT_ORACLE={name} conflicts with SVTAV1_REFERENCE; unset one"
            );
        }
        assert_eq!(
            reference.oracle_name(mode),
            name,
            "oracles.tsv row {name} disagrees with SvtReference::oracle_name"
        );
        p.hdr = HdrForkConfig::from_env_for_reference(reference, mode);
        p.reference = reference;
        eprintln!(
            "SVT_ORACLE={name} reference={} mode={mode:?}",
            reference.id()
        );
    }
}

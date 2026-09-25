//! Output pins: a fingerprint of the PORT's own bitstream over a fixed matrix.
//!
//! This is the safety net for refactoring
//! (`rust/docs/PLAN-ORACLES-AND-CLEANUP.md`). It does not compare against C —
//! the identity gates do that — it proves a change did not move the port's
//! output. Every cell covers a path someone could refactor: stills and video,
//! 8 and 10 bit, monochrome, each `SvtReference` and HDR mode, the tunes, the
//! Zen enhancements, fork knobs, lossless, tiles and threads. A refusal is
//! pinned too (as `REFUSED:<message>`), so a configuration cannot silently
//! start or stop being accepted.
//!
//! On a mismatch the test lists every moved cell. A refactor must not move
//! any. A deliberate behaviour change regenerates the manifest in the same
//! commit, and its message says which cells moved and why:
//!
//! ```sh
//! UPDATE_OUTPUT_PINS=1 cargo nextest run -p zenav1-svt -E 'test(/output_pins/)'
//! ```
//!
//! The switch is set by the caller (see `just pins-update`), never inside the
//! test, and a missing manifest in compare mode fails rather than passing.
//!
//! The fingerprint is FNV-1a 128 plus the byte length: enough to detect
//! change, which is all a pin is for.

use std::collections::BTreeMap;
use std::path::PathBuf;

use svtav1_encoder::enhancements::{ZenEnhancement, ZenEnhancements};
use svtav1_encoder::hdr_mode::{HdrForkConfig, SvtHdrMode};
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::reference::SvtReference;
use svtav1_encoder::speed_config::NativePreset;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Content {
    /// Smooth ramp XOR a small repeating pattern (the identity harness's
    /// `gradient`).
    Gradient,
    /// Seeded white noise: worst case for prediction.
    Noise,
    /// Hard edges: checkerboard plus diagonal strokes (screen-like).
    Edges,
    /// Smooth low-frequency field plus mild noise (photo-like).
    Photo,
}

impl Content {
    fn tag(self) -> &'static str {
        match self {
            Content::Gradient => "grad",
            Content::Noise => "noise",
            Content::Edges => "edges",
            Content::Photo => "photo",
        }
    }
}

/// Deterministic xorshift, so every host generates identical input.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// One 8-bit sample of `content` at (r, c), shifted `dx` pixels right to make
/// motion across frames.
fn sample(
    content: Content,
    r: usize,
    c: usize,
    w: usize,
    h: usize,
    dx: usize,
    rng: &mut Rng,
) -> u8 {
    let c = c.wrapping_sub(dx) % w.max(1);
    match content {
        Content::Gradient => ((r * 255 / h.max(1)) ^ ((c * 3) & 0x3f)) as u8,
        Content::Noise => (rng.next() >> 24) as u8,
        Content::Edges => {
            let checker = if ((r / 8) + (c / 8)) % 2 == 0 {
                40
            } else {
                215
            };
            if (r + c) % 13 == 0 || (r + 2 * c) % 29 == 0 {
                255 - checker
            } else {
                checker
            }
        }
        Content::Photo => {
            // Integer-only smooth field: no floating point, so no host can
            // round differently.
            let a = (r * 37 + c * 23) % 256;
            let b = ((r * r + c * 3) / 7) % 64;
            let n = (rng.next() >> 60) as usize;
            ((a / 2 + b + 32 + n) % 256) as u8
        }
    }
}

struct Frame {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

fn frame(content: Content, w: usize, h: usize, index: usize) -> Frame {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (index as u64 + 1));
    let dx = index * 2;
    let y = (0..h)
        .flat_map(|r| (0..w).map(move |c| (r, c)))
        .map(|(r, c)| sample(content, r, c, w, h, dx, &mut rng))
        .collect::<Vec<_>>();
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let u = (0..cw * ch)
        .map(|i| (100 + (y[(i / cw * 2).min(h - 1) * w + (i % cw * 2).min(w - 1)] / 4)) as u8)
        .collect();
    let v = (0..cw * ch)
        .map(|i| (160u8).wrapping_sub(y[(i / cw * 2).min(h - 1) * w + (i % cw * 2).min(w - 1)] / 5))
        .collect();
    Frame { y, u, v }
}

/// Widen to `bd` bits with deterministic low bits, so 10-bit cells exercise
/// real 10-bit content rather than a shifted 8-bit image.
fn widen(p: &[u8], bd: u8) -> Vec<u16> {
    let sh = u32::from(bd - 8);
    p.iter()
        .enumerate()
        .map(|(i, &s)| (u16::from(s) << sh) | ((i as u16 * 7) & ((1 << sh) - 1)))
        .collect()
}

#[derive(Clone)]
struct Cell {
    content: Content,
    w: usize,
    h: usize,
    bd: u8,
    mono: bool,
    preset: i8,
    qp: u8,
    frames: usize,
    random_access: bool,
    reference: SvtReference,
    mode: SvtHdrMode,
    tune: Option<u8>,
    enhancement: Option<ZenEnhancement>,
    knob: Option<(&'static str, fn(&mut HdrForkConfig))>,
    tile_cols_log2: u8,
    threads: usize,
}

impl Cell {
    fn still(content: Content, w: usize, h: usize, preset: i8, qp: u8) -> Self {
        Cell {
            content,
            w,
            h,
            bd: 8,
            mono: false,
            preset,
            qp,
            frames: 1,
            random_access: false,
            // Legacy constructors default to the hybrid; pin what users get.
            reference: SvtReference::Hybrid3115,
            mode: SvtHdrMode::Mainline,
            tune: None,
            enhancement: None,
            knob: None,
            tile_cols_log2: 0,
            threads: 1,
        }
    }

    fn id(&self) -> String {
        let mut s = format!(
            "{}-{}x{}-b{}{}-p{}-q{}",
            self.content.tag(),
            self.w,
            self.h,
            self.bd,
            if self.mono { "-mono" } else { "" },
            self.preset,
            self.qp
        );
        if self.frames > 1 {
            s += &format!(
                "-f{}{}",
                self.frames,
                if self.random_access { "-ra" } else { "-ld" }
            );
        }
        s += &format!("-{:?}-{:?}", self.reference, self.mode);
        if let Some(t) = self.tune {
            s += &format!("-tune{t}");
        }
        if let Some(e) = self.enhancement {
            s += &format!("-{}", e.id());
        }
        if let Some((name, _)) = self.knob {
            s += &format!("-{name}");
        }
        if self.tile_cols_log2 > 0 {
            s += &format!("-tc{}", self.tile_cols_log2);
        }
        if self.threads > 1 {
            s += &format!("-t{}", self.threads);
        }
        s
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: self.qp,
            ..RcConfig::default()
        };
        let preset = NativePreset::new(self.preset).ok_or("preset out of range")?;
        let (hier, intra_period) = if self.frames > 1 {
            (HIERARCHICAL_LEVELS_AUTO, 64)
        } else {
            (0, 1)
        };
        let mut p = EncodePipeline::new_with_preset(
            self.w as u32,
            self.h as u32,
            preset,
            rc,
            hier,
            intra_period,
        )
        .with_bit_depth(self.bd)
        .with_tile_cols_log2(self.tile_cols_log2)
        .with_thread_count(self.threads);
        if self.random_access {
            p = p.with_pred_structure(PredStructure::RandomAccess);
        }
        if !self.mono {
            p = p.with_chroma_420(true);
        }
        p.reference = self.reference;
        // The (reference, mode) pair resolves the defaults its oracle's
        // `svt_av1_set_default_params` loads: GhostRobot gets the fork's
        // real defaults, Hybrid3115+HdrFork the hybrid MODE1 set. The
        // (GhostRobot, Mainline) cell keeps mainline() so it stays the
        // pinned REFUSED arm.
        p.hdr = HdrForkConfig::defaults_for(self.reference, self.mode);
        if let Some(t) = self.tune {
            p.hdr.tune = t;
        }
        if let Some((_, f)) = self.knob {
            f(&mut p.hdr);
        }
        if let Some(e) = self.enhancement {
            p.enhancements = ZenEnhancements::default().with(e);
        }
        let mut out = Vec::new();
        for i in 0..self.frames {
            let f = frame(self.content, self.w, self.h, i);
            let bytes = match (self.bd > 8, self.mono) {
                (false, true) => p.try_encode_frame(&f.y, self.w),
                (false, false) => p.try_encode_frame_420(&f.y, &f.u, &f.v, self.w),
                (true, true) => p.try_encode_frame_hbd(&widen(&f.y, self.bd), self.w),
                (true, false) => p.try_encode_frame_420_hbd(
                    &widen(&f.y, self.bd),
                    &widen(&f.u, self.bd),
                    &widen(&f.v, self.bd),
                    self.w,
                ),
            }
            .map_err(|e| e.to_string())?;
            out.extend_from_slice(&bytes);
        }
        if self.frames > 1 {
            out.extend_from_slice(&p.try_flush().map_err(|e| e.to_string())?);
        }
        Ok(out)
    }
}

fn fingerprint(bytes: &[u8]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u128::from(b);
        h = h.wrapping_mul(PRIME);
    }
    format!("{}:{h:032x}", bytes.len())
}

fn outcome(cell: &Cell) -> String {
    match cell.encode() {
        Ok(bytes) => fingerprint(&bytes),
        // First line only: refusal strings carry dates and measurements that
        // may be reworded; the pin is about WHETHER it refuses and why.
        Err(e) => format!(
            "REFUSED:{}",
            e.lines().next().unwrap_or("").replace('\t', " ")
        ),
    }
}

fn manifest_path(group: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/output_pins")
        .join(format!("{group}.tsv"))
}

/// Compare `cells` against the committed manifest for `group`, or rewrite it
/// under `UPDATE_OUTPUT_PINS=1`.
fn check_group(group: &str, cells: Vec<Cell>) {
    let mut actual = BTreeMap::new();
    for cell in &cells {
        let id = cell.id();
        assert!(
            actual.insert(id.clone(), outcome(cell)).is_none(),
            "duplicate cell id {id} in group {group}"
        );
    }
    let path = manifest_path(group);
    if std::env::var_os("UPDATE_OUTPUT_PINS").is_some_and(|v| v == "1") {
        let mut text = format!(
            "# output pins, group {group}: cell<TAB>bytes:fnv1a128, or REFUSED:<reason>.\n\
             # Regenerate only with UPDATE_OUTPUT_PINS=1 and say why in the commit.\n"
        );
        for (id, v) in &actual {
            text += &format!("{id}\t{v}\n");
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        eprintln!(
            "output_pins: wrote {} cells to {}",
            actual.len(),
            path.display()
        );
        return;
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "output_pins: manifest {} is missing ({e}); generate it with UPDATE_OUTPUT_PINS=1",
            path.display()
        )
    });
    let expected: BTreeMap<String, String> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let (k, v) = l.split_once('\t').expect("manifest line is cell<TAB>value");
            (k.to_string(), v.to_string())
        })
        .collect();
    let mut problems = Vec::new();
    for (id, v) in &actual {
        match expected.get(id) {
            None => problems.push(format!("  NEW      {id}  {v}")),
            Some(e) if e != v => {
                problems.push(format!("  MOVED    {id}\n    pinned {e}\n    now    {v}"))
            }
            Some(_) => {}
        }
    }
    for id in expected.keys().filter(|k| !actual.contains_key(*k)) {
        problems.push(format!("  REMOVED  {id}"));
    }
    assert!(
        problems.is_empty(),
        "output_pins group {group}: {} of {} cells differ from {}:\n{}\n\
         A refactor must not move any cell. If this is a deliberate behaviour change, \
         regenerate with UPDATE_OUTPUT_PINS=1 and state which cells moved and why.",
        problems.len(),
        actual.len(),
        path.display(),
        problems.join("\n")
    );
}

const CONTENTS: [Content; 4] = [
    Content::Gradient,
    Content::Noise,
    Content::Edges,
    Content::Photo,
];

#[test]
fn output_pins_stills_8bit() {
    let mut cells = Vec::new();
    for content in CONTENTS {
        for (w, h) in [(64, 64), (80, 48)] {
            for preset in [0i8, 3, 6, 9, 13] {
                for qp in [8u8, 32, 58] {
                    cells.push(Cell::still(content, w, h, preset, qp));
                }
            }
        }
    }
    // Partial superblocks on both axes, and a width that is not a multiple of 8.
    for content in [Content::Photo, Content::Edges] {
        cells.push(Cell::still(content, 65, 33, 4, 30));
        cells.push(Cell::still(content, 130, 70, 8, 40));
    }
    // Lossless.
    for content in [Content::Photo, Content::Noise] {
        cells.push(Cell::still(content, 64, 64, 6, 0));
    }
    check_group("stills_8bit", cells);
}

#[test]
fn output_pins_stills_10bit_and_mono() {
    let mut cells = Vec::new();
    for content in [Content::Gradient, Content::Photo, Content::Edges] {
        for preset in [2i8, 6, 10] {
            for qp in [20u8, 50] {
                cells.push(Cell {
                    bd: 10,
                    ..Cell::still(content, 64, 64, preset, qp)
                });
            }
        }
    }
    for content in [Content::Gradient, Content::Photo] {
        for (bd, preset) in [(8u8, 2i8), (8, 8), (10, 6)] {
            cells.push(Cell {
                bd,
                mono: true,
                ..Cell::still(content, 64, 64, preset, 30)
            });
        }
    }
    check_group("stills_10bit_mono", cells);
}

#[test]
fn output_pins_references_and_tunes() {
    let mut cells = Vec::new();
    let targets = [
        (SvtReference::Mainline420, SvtHdrMode::Mainline),
        (SvtReference::Hybrid3115, SvtHdrMode::Mainline),
        (SvtReference::Hybrid3115, SvtHdrMode::HdrFork),
        (SvtReference::GhostRobot, SvtHdrMode::HdrFork),
        // Refused: Ghost Robot has no mainline mode.
        (SvtReference::GhostRobot, SvtHdrMode::Mainline),
    ];
    for (reference, mode) in targets {
        for content in [Content::Gradient, Content::Photo] {
            for (preset, bd) in [(4i8, 8u8), (9, 8), (6, 10)] {
                cells.push(Cell {
                    reference,
                    mode,
                    bd,
                    ..Cell::still(content, 64, 64, preset, 30)
                });
            }
        }
    }
    // Every tune value, in fork mode where all of them are legal; 5 (VMAF)
    // pins its refusal.
    for tune in 0u8..=6 {
        cells.push(Cell {
            reference: SvtReference::Hybrid3115,
            mode: SvtHdrMode::HdrFork,
            tune: Some(tune),
            ..Cell::still(Content::Photo, 64, 64, 6, 30)
        });
    }
    // Monochrome under pristine mainline: refused (C is 4:2:0 only).
    cells.push(Cell {
        mono: true,
        reference: SvtReference::Mainline420,
        ..Cell::still(Content::Photo, 64, 64, 6, 30)
    });
    check_group("references_tunes", cells);
}

#[test]
fn output_pins_fork_knobs_and_enhancements() {
    let knobs: [(&'static str, fn(&mut HdrForkConfig)); 8] = [
        ("vboost", |h| {
            h.enable_variance_boost = true;
            h.variance_boost_strength = 3;
        }),
        ("vboost-pq", |h| {
            h.enable_variance_boost = true;
            h.variance_boost_curve = 3;
        }),
        ("qm", |h| {
            h.enable_qm = true;
            h.min_qm_level = 4;
        }),
        ("sharp7", |h| h.sharpness = 7),
        ("acbias", |h| h.ac_bias = 1.0),
        ("txbias", |h| h.tx_bias = 2),
        ("noisenorm", |h| h.noise_norm_strength = 3),
        ("altssim", |h| h.alt_ssim_tuning = true),
    ];
    let mut cells = Vec::new();
    for (name, f) in knobs {
        for bd in [8u8, 10] {
            cells.push(Cell {
                reference: SvtReference::Hybrid3115,
                mode: SvtHdrMode::HdrFork,
                bd,
                knob: Some((name, f)),
                ..Cell::still(Content::Photo, 64, 64, 6, 30)
            });
        }
    }
    let enhancements = [
        (ZenEnhancement::AomIntraEdgeFilter, -1i8),
        (ZenEnhancement::AomRestorationUnitSearch, -1),
        (ZenEnhancement::StillImageTune, 6),
        (ZenEnhancement::AomAdaptiveCdef, 6),
        (ZenEnhancement::AomAdaptiveSharpness, 6),
        (ZenEnhancement::AomDeltaQLf, 6),
        (ZenEnhancement::AomScreenTools, 8),
        (ZenEnhancement::DeepSearch, 8),
    ];
    for (e, preset) in enhancements {
        let content = if e == ZenEnhancement::AomScreenTools {
            Content::Edges
        } else {
            Content::Photo
        };
        cells.push(Cell {
            enhancement: Some(e),
            ..Cell::still(content, 64, 64, preset, 30)
        });
    }
    // Delta-LF only signals when a per-SB delta-q plan exists.
    cells.push(Cell {
        reference: SvtReference::Hybrid3115,
        mode: SvtHdrMode::HdrFork,
        enhancement: Some(ZenEnhancement::AomDeltaQLf),
        knob: Some(("vboost", |h| h.enable_variance_boost = true)),
        ..Cell::still(Content::Photo, 128, 128, 6, 30)
    });
    check_group("fork_knobs_enhancements", cells);
}

#[test]
fn output_pins_video() {
    let mut cells = Vec::new();
    for content in [Content::Gradient, Content::Photo] {
        for (preset, qp) in [(4i8, 25u8), (10, 45)] {
            cells.push(Cell {
                frames: 4,
                ..Cell::still(content, 64, 64, preset, qp)
            });
        }
        cells.push(Cell {
            frames: 4,
            bd: 10,
            ..Cell::still(content, 64, 64, 8, 35)
        });
        cells.push(Cell {
            frames: 5,
            random_access: true,
            ..Cell::still(content, 64, 64, 8, 35)
        });
    }
    cells.push(Cell {
        frames: 3,
        mono: true,
        ..Cell::still(Content::Photo, 64, 64, 8, 35)
    });
    cells.push(Cell {
        frames: 3,
        reference: SvtReference::Hybrid3115,
        mode: SvtHdrMode::HdrFork,
        ..Cell::still(Content::Photo, 64, 64, 8, 35)
    });
    // alt_ssim_tuning has no inter arm: refused on the inter frame. It used to
    // panic in release (leaf_funnel/mds3.rs), measured 2026-09-25.
    for (preset, qp) in [(5i8, 40u8), (8, 35)] {
        cells.push(Cell {
            frames: 4,
            reference: SvtReference::Hybrid3115,
            mode: SvtHdrMode::HdrFork,
            knob: Some(("altssim", |h| h.alt_ssim_tuning = true)),
            ..Cell::still(Content::Gradient, 128, 128, preset, qp)
        });
    }
    check_group("video", cells);
}

#[test]
fn output_pins_tiles_and_threads() {
    let mut cells = Vec::new();
    for content in [Content::Photo, Content::Edges] {
        for threads in [1usize, 4] {
            cells.push(Cell {
                tile_cols_log2: 1,
                threads,
                ..Cell::still(content, 256, 128, 6, 30)
            });
        }
    }
    // Threads must never change the bytes.
    for pair in cells.chunks(2) {
        assert_eq!(
            outcome(&pair[0]),
            outcome(&pair[1]),
            "{} and {} must be byte-identical",
            pair[0].id(),
            pair[1].id()
        );
    }
    check_group("tiles_threads", cells);
}

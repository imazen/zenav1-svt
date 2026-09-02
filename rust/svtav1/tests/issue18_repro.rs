//! Issue #18 witness: a 10-bit encode must not predict across a TILE edge.
//!
//! AV1 forces a multi-tile grid once the frame exceeds `MAX_TILE_AREA`
//! (4096*2304 px) or `MAX_TILE_WIDTH` (4096 px) — see
//! `TileGrid::resolve`/`svt_av1_get_tile_limits`. A caller that asks for one
//! tile (the AVIF surface never asks for any) still gets >= 2 tiles there.
//! Intra prediction is tile-scoped in AV1: a block on a tile's own top row /
//! left column has NO above / left neighbour, because a conforming decoder
//! reconstructs each tile independently and has no such pixels either.
//!
//! The u8 path honours that (`partition::extract_neighbors_tiled` takes
//! `tile_top`/`tile_left`). TWO bd10 sites did not, one per preset band:
//!
//! 1. **preset <= 8** (the full-RD funnel): `predict_unit_hbd`'s
//!    non-directional arm called `extract_neighbors_hbd`, whose availability
//!    was frame-absolute (`abs_y > 0` / `abs_x > 0`). Its directional arm
//!    already carried `geom.tile` into `dr_predict_hbd`, so the gap was
//!    exactly DC / V / H / smooth* / paeth / filter-intra.
//! 2. **preset >= 9** (the level-only re-encode post-pass,
//!    `bd10_reencode_{luma,chroma}`): both node walks hardcoded
//!    `TileMi::whole_frame`, so fixing (1) did not reach them. MEASURED with
//!    (1) fixed and (2) not: `gradient 256x256 q20` with 2 tile rows still
//!    differed from `aomdec` on 49,606 of 98,304 samples at presets 9, 10 AND
//!    13, while preset 6 was already clean.
//!
//! 3. **the DIRECTIONAL arm, at presets 0-5** (found 2026-09-02 by the HBD
//!    executor's re-verification of the first fix, issue #18 follow-up):
//!    `intra_edge::dr_predict_hbd` computed `have_top`/`have_left` from the
//!    FRAME (`g.mi_row > 0`) and `right_available`/`bottom_available` against
//!    `mi_cols`/`mi_rows`, ignoring all four `g.tile` fields its u8 twin
//!    `dr_predict` honours. So a `DrGeom` carrying the correct tile was
//!    handed in and thrown away.
//!
//! Either way the encoder read real pixels across the tile edge while the
//! decoder used the unavailable-edge fills, and every block from the tile
//! boundary onward drifted.
//!
//! WHY THE FIRST ROUND OF THESE TESTS MISSED (3) — measured, not guessed, and
//! the reason this file now sweeps a preset band instead of a preset. At
//! 256x256 with 2 tile rows, bd10, this exact geometry:
//!
//! ```text
//!            q6      q12     q20     q40
//!   p0/p2/p4  FAIL    FAIL    FAIL    FAIL     (gradient AND diag)
//!   p3/p5     FAIL    FAIL    FAIL    FAIL
//!   p6/p7/p8/p9   ok      ok      ok      ok
//!   uniform (any preset)  ok                    -- flat content picks no
//!                                                  directional mode at all
//! ```
//!
//! The first round tested presets 6 and 9 ONLY. The defect lives at presets
//! 0-5, because that is where the intra candidate set still offers directional
//! modes, and `dr_predict_hbd` is only reached when a directional leaf wins.
//! It was NOT the content (gradient reproduces it), NOT the qp, NOT the tile
//! axis, and NOT rows-vs-columns — every one of those was varied and none of
//! them is the discriminator. `AvifEncoder`'s speed 4 maps to preset 4 and its
//! quality 90 to qp 6, which is dead centre of the failing band; the reported
//! cell was `3000x4000` there.
//!
//! The cheap cells below therefore pin the PRESET BAND, and assert that the
//! tile grid they request is the SAME GRID the reported 12 MP portrait frame
//! resolves to on its own (1 column x 2 rows) — so a sub-second test stands in
//! for a 12 MP encode by construction rather than by hope. MEASURED before the fix on `gradient 4160x64 q20 p6`: 65,054 of
//! 399,360 samples differ, the first at Y r0 c2122, right of the tile-column
//! boundary at x = 33 SB * 64 = 2112; the same cell at bd8 is identical, and
//! `4096x64` (one SB column fewer, so a single tile) is identical at bd10.
//!
//! This is why issue #18 looked like an 8-12 MP SIZE cliff: at 4:2:0/SB64 a
//! frame crosses `MAX_TILE_AREA` at 2304 superblocks (~9.44 MP), so every
//! smaller AVIF encode happened to be single-tile and healthy. Area is a
//! PROXY; the driver is the forced tile grid, which these cells reach at
//! 0.27 MP.
//!
//! The oracle is the AV1 reference decoder, exactly as in `issue13_repro.rs`:
//! `EncodePipeline::last_recon10_final` cropped to the true coded dims must
//! equal what `aomdec` outputs for the same stream, sample for sample. A
//! byte-vs-C gate cannot see this class — it is an encoder/decoder
//! prediction MISMATCH, so both encoders being wrong the same way would stay
//! green.
//!
//! ANTI-VACUITY: each cell asserts the tile grid it is named for actually
//! resolved (`num_tiles() > 1`, and for the forced cells that it happened
//! with `requested == (0, 0)`). The single-tile control asserts the opposite,
//! so a change that silently stopped forcing tiles fails here rather than
//! passing three vacuous comparisons.
//!
//! SKIPPING IS CALLER-CONTROLLED, never silent: the decoder is found via
//! `$AOMDEC` or `aomdec` on `PATH`; absent both the test FAILS with
//! instructions. `ZENAV1_SKIP_DECODER_TESTS=1` skips it deliberately.

use std::path::PathBuf;
use std::process::Command;

use svtav1_encoder::entropy::obu::TileGrid;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

/// The identity harness's `gradient` luma (`identity_run.rs`).
fn gradient_y(w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    y
}

/// The identity harness's `diag` luma (`identity_run.rs`): a 45-degree ramp
/// whose leaves select the DIRECTIONAL intra modes (D45/D135/...) that
/// `gradient` never picks. Its comment there names `dr_predict_hbd` as the
/// thing it exists to exercise — which is the predictor this file's second
/// round is about.
fn diag_y(w: usize, h: usize) -> Vec<u8> {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r as i32 - c as i32).rem_euclid(64)) * 4) as u8;
        }
    }
    y
}

fn luma_for(content: &str, w: usize, h: usize) -> Vec<u8> {
    match content {
        "gradient" => gradient_y(w, h),
        "diag" => diag_y(w, h),
        other => panic!("unknown content {other}"),
    }
}

fn textured_uv(cw: usize, ch: usize, phase: usize) -> Vec<u8> {
    let mut p = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            let ramp = ((r * 5 + c * 3 + phase) % 96) as i32;
            let tex = if (r / 3 + c / 3) % 2 == 0 { 22 } else { -18 };
            p[r * cw + c] = (96 + ramp + tex).clamp(16, 240) as u8;
        }
    }
    p
}

fn widen(s: &[u8]) -> Vec<u16> {
    s.iter().map(|&x| u16::from(x) << 2).collect()
}

fn find_aomdec() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AOMDEC") {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("aomdec"))
        .find(|p| p.is_file())
}

/// Parse a y4m written by `aomdec -o <f>.y4m` for a 10-bit 4:2:0 stream:
/// the first FRAME's samples as little-endian u16, Y|U|V at the TRUE dims
/// (chroma CEILING) — the decoder's own output layout.
fn y4m_first_frame_u16(bytes: &[u8], w: usize, h: usize) -> Vec<u16> {
    let hdr_end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .expect("y4m header line");
    let hdr = std::str::from_utf8(&bytes[..hdr_end]).expect("y4m header utf8");
    assert!(
        hdr.contains("C420p10"),
        "expected a 10-bit 4:2:0 y4m from aomdec, header was: {hdr}"
    );
    let frame_tag = bytes[hdr_end..]
        .windows(5)
        .position(|win| win == b"FRAME")
        .expect("y4m FRAME tag")
        + hdr_end;
    let data_start = bytes[frame_tag..]
        .iter()
        .position(|&b| b == b'\n')
        .expect("FRAME line end")
        + frame_tag
        + 1;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let need = w * h + 2 * cw * ch;
    let data = &bytes[data_start..];
    assert!(
        data.len() >= 2 * need,
        "y4m frame short: {} bytes < {}",
        data.len(),
        2 * need
    );
    data[..2 * need]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect()
}

/// Skip only when the caller said so, loudly. Returns the decoder path.
fn decoder_or_skip(what: &str) -> Option<PathBuf> {
    if std::env::var_os("ZENAV1_SKIP_DECODER_TESTS").is_some() {
        eprintln!(
            "issue18_repro: SKIPPED by ZENAV1_SKIP_DECODER_TESTS — the bd10 {what} \
             recon-vs-aomdec check did NOT run in this invocation"
        );
        return None;
    }
    Some(find_aomdec().expect(
        "aomdec not found. Set AOMDEC=<path to aomdec> (or put it on PATH), or set \
         ZENAV1_SKIP_DECODER_TESTS=1 to skip this check DELIBERATELY. It is the only \
         check that a 10-bit MULTI-TILE encode reconstructs the way a decoder does \
         (issue #18).",
    ))
}

/// Encode one bd10 4:2:0 cell and require its published 10-bit final recon to
/// equal `aomdec`'s output sample for sample.
fn assert_bd10_recon_matches_decoder(
    aomdec: &PathBuf,
    tag: &str,
    w: usize,
    h: usize,
    preset: u8,
    qp: u8,
    rows_log2: u8,
    cols_log2: u8,
) {
    assert_bd10_recon_matches_decoder_content(
        aomdec, tag, "gradient", w, h, preset, qp, rows_log2, cols_log2,
    );
}

/// Content-parameterised form. `diag` is the one that reaches
/// `dr_predict_hbd`; `gradient` reaches it too at the low presets, but `diag`
/// is the content the harness documents for exactly this predictor.
#[allow(clippy::too_many_arguments)]
fn assert_bd10_recon_matches_decoder_content(
    aomdec: &PathBuf,
    tag: &str,
    content: &str,
    w: usize,
    h: usize,
    preset: u8,
    qp: u8,
    rows_log2: u8,
    cols_log2: u8,
) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = widen(&luma_for(content, w, h));
    let u = widen(&textured_uv(cw, ch, 0));
    let v = widen(&textured_uv(cw, ch, 41));

    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_bit_depth(10)
        .with_tile_rows_log2(rows_log2)
        .with_tile_cols_log2(cols_log2)
        .with_sb_size(None)
        .with_chroma_420(true)
        .with_recon_output(true);
    let obu = p
        .try_encode_frame_420_hbd(&y, &u, &v, w)
        .unwrap_or_else(|e| panic!("{tag}: {w}x{h} bd10 4:2:0 preset {preset} must encode: {e:?}"));
    assert!(!obu.is_empty(), "{tag}: empty stream");

    let (ry, ru, rv) = p.last_recon10_final.as_ref().unwrap_or_else(|| {
        panic!(
            "{tag}: with_recon_output(true) on a bd10 in-envelope frame must publish \
             last_recon10_final"
        )
    });
    // Crop the ALIGNED-stride canvas to the true dims (chroma CEILING), the
    // decoder's output layout.
    let aw = p.width as usize;
    let acw = aw / 2;
    let mut enc: Vec<u16> = Vec::with_capacity(w * h + 2 * cw * ch);
    for r in 0..h {
        enc.extend_from_slice(&ry[r * aw..r * aw + w]);
    }
    for pl in [ru, rv] {
        for r in 0..ch {
            enc.extend_from_slice(&pl[r * acw..r * acw + cw]);
        }
    }

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("issue18");
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let obu_path = dir.join(format!("{tag}.obu"));
    let y4m_path = dir.join(format!("{tag}.y4m"));
    std::fs::write(&obu_path, &obu).expect("write obu");
    let status = Command::new(aomdec)
        .arg("-o")
        .arg(&y4m_path)
        .arg(&obu_path)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", aomdec.display()));
    assert!(
        status.success(),
        "{tag}: aomdec rejected the stream: {status}"
    );
    let dec = y4m_first_frame_u16(&std::fs::read(&y4m_path).expect("read y4m"), w, h);

    assert_eq!(enc.len(), dec.len(), "{tag}: sample count");
    let mismatches = enc.iter().zip(&dec).filter(|(a, b)| a != b).count();
    if mismatches != 0 {
        let i = enc.iter().zip(&dec).position(|(a, b)| a != b).unwrap();
        let (plane, pos) = if i < w * h {
            ("Y", format!("r{} c{}", i / w, i % w))
        } else {
            let j = (i - w * h) % (cw * ch);
            (
                if i < w * h + cw * ch { "U" } else { "V" },
                format!("r{} c{}", j / cw, j % cw),
            )
        };
        panic!(
            "{tag}: 10-bit final recon != aomdec: {mismatches} of {} samples differ, first \
             {plane}@{pos} enc={} dec={} (issue #18 — the bd10 intra predictor read \
             neighbours across a tile edge that a conforming decoder cannot see)",
            enc.len(),
            enc[i],
            dec[i]
        );
    }
}

/// `4160x64`: `sb_cols = 65 > max_tile_width_sb = 64`, so AV1 FORCES two tile
/// columns even though the caller requested `(0, 0)` — the same thing that
/// happens to every AVIF encode above `MAX_TILE_AREA`. 0.27 MP, so the cell is
/// cheap; the tile boundary lands at `33 SB * 64 = 2112`.
#[test]
fn issue18_bd10_forced_tile_columns_recon_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("forced-tile-columns") else {
        return;
    };
    // ANTI-VACUITY: the grid must actually be forced multi-tile at (0, 0).
    let grid = TileGrid::resolve(4160, 64, 64, 0, 0);
    assert_eq!(
        (grid.tile_cols, grid.tile_rows),
        (2, 1),
        "4160x64 SB64 must FORCE 2 tile columns at requested (0,0) (sb_cols {} > \
         max_tile_width_sb 64); without that this cell tests nothing",
        grid.sb_cols
    );
    assert_bd10_recon_matches_decoder(
        &aomdec,
        "bd10_4160x64_p6_q20_forcedcols",
        4160,
        64,
        6,
        20,
        0,
        0,
    );
}

/// The tile-ROW axis of the same defect, at a requested `(1, 0)` grid so the
/// cell stays small. MEASURED before the fix: `gradient 256x256 q20 p6` bd10
/// with 2 tile rows differs from the decoder on 24,169 of 98,304 samples,
/// while every bd8 tiling of the same cell is identical.
#[test]
fn issue18_bd10_tile_rows_recon_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("tile-rows") else {
        return;
    };
    let grid = TileGrid::resolve(256, 256, 64, 1, 0);
    assert_eq!(
        (grid.tile_cols, grid.tile_rows),
        (1, 2),
        "256x256 SB64 with TileRowsLog2=1 must resolve to 2 tile rows"
    );
    assert_bd10_recon_matches_decoder(&aomdec, "bd10_256x256_p6_q20_rows2", 256, 256, 6, 20, 1, 0);
}

/// The preset >= 9 arm: the level-only re-encode post-pass produces the coded
/// 10-bit levels there instead of the full-RD funnel, and it walks the merged
/// frame in raster SB order with its own `TileMi`. Fixing the funnel does not
/// reach it — MEASURED at that intermediate state: 49,606 of 98,304 samples
/// differ at presets 9, 10 and 13 while preset 6 reads clean.
#[test]
fn issue18_bd10_tile_rows_recon_matches_aomdec_at_reencode_preset() {
    let Some(aomdec) = decoder_or_skip("tile-rows at the re-encode preset") else {
        return;
    };
    let grid = TileGrid::resolve(256, 256, 64, 1, 0);
    assert_eq!(
        (grid.tile_cols, grid.tile_rows),
        (1, 2),
        "256x256 SB64 with TileRowsLog2=1 must resolve to 2 tile rows"
    );
    assert_bd10_recon_matches_decoder(&aomdec, "bd10_256x256_p9_q20_rows2", 256, 256, 9, 20, 1, 0);
}

/// CONTROL: `4096x64` is one SB column narrower, so the same request resolves
/// to a SINGLE tile and the encode was always correct. It must stay correct —
/// this is what keeps the two cells above from passing for the wrong reason
/// and what a tile-availability fix must not regress.
#[test]
fn issue18_bd10_single_tile_control_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("single-tile control") else {
        return;
    };
    let grid = TileGrid::resolve(4096, 64, 64, 0, 0);
    assert_eq!(
        grid.num_tiles(),
        1,
        "4096x64 SB64 at requested (0,0) must be a SINGLE tile — the control loses its \
         meaning otherwise"
    );
    assert_bd10_recon_matches_decoder(&aomdec, "bd10_4096x64_p6_q20_1tile", 4096, 64, 6, 20, 0, 0);
}

/// The DIRECTIONAL arm at the low preset band — the residual the first round
/// of this file did not catch.
///
/// GRID EQUIVALENCE (asserted, not assumed): `3000x4000` — the frame issue #18
/// was filed on — resolves to 1 tile column x 2 tile rows all by itself, and
/// so does `256x256` with `TileRowsLog2 = 1`. The cheap cell is therefore the
/// same tile shape as the reported one; only the pixel count differs.
///
/// MEASURED at HEAD before the `dr_predict_hbd` fix, `gradient`/`diag` 256x256
/// with 2 tile rows: presets 0, 2, 3, 4 and 5 all differ from `aomdec`
/// (12,480-24,901 of 98,304 samples) at every qp in {6, 12, 20, 40}, while
/// presets 6-9 read clean and `uniform` reads clean everywhere.
#[test]
fn issue18_bd10_directional_tile_rows_recon_matches_aomdec_low_preset_band() {
    let Some(aomdec) = decoder_or_skip("directional / low preset band") else {
        return;
    };
    let cheap = TileGrid::resolve(256, 256, 64, 1, 0);
    let reported = TileGrid::resolve(3000, 4000, 64, 0, 0);
    assert_eq!(
        (cheap.tile_cols, cheap.tile_rows),
        (reported.tile_cols, reported.tile_rows),
        "the cheap cell must resolve to the SAME tile grid as the reported 3000x4000 \
         portrait frame (which forces its own grid at requested (0,0)); it stands in for \
         that encode and stops standing in the moment these differ"
    );
    assert_eq!(
        (reported.tile_cols, reported.tile_rows),
        (1, 2),
        "3000x4000 SB64 at requested (0,0) must FORCE 1 column x 2 rows"
    );
    // Sweep the band rather than a point: the first round pinned presets 6 and
    // 9, which are exactly the two that pass.
    for preset in [0u8, 2, 4, 5] {
        for (content, qp) in [("gradient", 12u8), ("diag", 6)] {
            assert_bd10_recon_matches_decoder_content(
                &aomdec,
                &format!("bd10_256x256_p{preset}_q{qp}_{content}_rows2"),
                content,
                256,
                256,
                preset,
                qp,
                1,
                0,
            );
        }
    }
}

/// The same residual on the COLUMN axis, at a grid AV1 forces rather than one
/// the caller requests (`sb_cols 65 > max_tile_width_sb 64`), and in the low
/// preset band. Keeps the forced-vs-requested half of the coverage honest for
/// the directional arm too.
#[test]
fn issue18_bd10_directional_forced_tile_columns_recon_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("directional / forced tile columns") else {
        return;
    };
    let grid = TileGrid::resolve(4160, 64, 64, 0, 0);
    assert_eq!(
        (grid.tile_cols, grid.tile_rows),
        (2, 1),
        "4160x64 SB64 must FORCE 2 tile columns at requested (0,0)"
    );
    assert_bd10_recon_matches_decoder_content(
        &aomdec,
        "bd10_4160x64_p2_q12_diag_forcedcols",
        "diag",
        4160,
        64,
        2,
        12,
        0,
        0,
    );
}

/// CONTROL for the band sweep: the identical cells at a SINGLE tile must stay
/// clean. Without this, "presets 0-5 differ" could be a plain low-preset bug
/// rather than a tile-boundary one, and the sweep above would keep passing
/// after a fix that simply disabled directional prediction at bd10.
#[test]
fn issue18_bd10_directional_single_tile_control_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("directional single-tile control") else {
        return;
    };
    let grid = TileGrid::resolve(256, 256, 64, 0, 0);
    assert_eq!(grid.num_tiles(), 1, "the control must be a SINGLE tile");
    for preset in [0u8, 2, 4, 5] {
        for (content, qp) in [("gradient", 12u8), ("diag", 6)] {
            assert_bd10_recon_matches_decoder_content(
                &aomdec,
                &format!("bd10_256x256_p{preset}_q{qp}_{content}_1tile"),
                content,
                256,
                256,
                preset,
                qp,
                0,
                0,
            );
        }
    }
}

/// THE REPORTED SHAPE ITSELF, not a stand-in: a PORTRAIT frame whose tile grid
/// is forced by AREA (the way `3000x4000` forces its own), with a partial
/// superblock on BOTH axes, at the preset the reporter used.
///
/// `2920x3270` is the cheapest cell with all four properties: sb `46x52 = 2392
/// > 2304` so AV1 forces 1 column x 2 rows at requested `(0,0)`; `2920 / 64 =
/// 45.6` and `3270 / 64 = 51.1` so both axes straddle; `h > w`. It costs ~6.4 s
/// — real, and deliberately paid, because the first round of this file proved
/// that a cheap stand-in can share a tile grid with the reported cell and still
/// miss its defect.
///
/// MEASURED at HEAD before the `dr_predict_hbd` fix: **4,185,160 of 14,322,600
/// samples differ, first at Y r1664** — exactly the tile-row boundary
/// (`26 SB * 64`). The reported `3000x4000` real-photograph cell at qp 6 /
/// preset 4 read 6,468,452 of 18,000,000 differing, first at Y r2048
/// (`32 SB * 64`), and reads clean after.
#[test]
fn issue18_bd10_forced_by_area_portrait_recon_matches_aomdec() {
    let Some(aomdec) = decoder_or_skip("forced-by-area portrait") else {
        return;
    };
    let grid = TileGrid::resolve(2920, 3270, 64, 0, 0);
    assert_eq!(
        (grid.tile_cols, grid.tile_rows),
        (1, 2),
        "2920x3270 must FORCE 1 column x 2 rows by AREA at requested (0,0) \
         (sb {}x{} = {} > 2304)",
        grid.sb_cols,
        grid.sb_rows,
        grid.sb_cols * grid.sb_rows
    );
    assert!(
        3270 > 2920 && !2920usize.is_multiple_of(64) && !3270usize.is_multiple_of(64),
        "the cell must stay PORTRAIT with a partial superblock on both axes — \
         those are the properties it is here to carry"
    );
    assert_bd10_recon_matches_decoder_content(
        &aomdec,
        "bd10_2920x3270_p4_q12_gradient_forcedarea",
        "gradient",
        2920,
        3270,
        4,
        12,
        0,
        0,
    );
}

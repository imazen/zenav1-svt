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
//! Either way the encoder read real pixels across the tile edge while the
//! decoder used the unavailable-edge fills, and every block from the tile
//! boundary onward drifted. MEASURED before the fix on `gradient 4160x64 q20 p6`: 65,054 of
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
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = widen(&gradient_y(w, h));
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

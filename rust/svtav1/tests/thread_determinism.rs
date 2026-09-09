//! Feature 4 (bounded threading) determinism gate.
//!
//! The tile-parallel encode was rewritten from "spawn every tile at once"
//! into a bounded, order-preserving wave loop. This gate proves the rewrite
//! is BYTE-INERT: a genuinely multi-tile frame must encode to identical bytes
//! regardless of how many OS threads the wave loop is allowed to use.
//!
//! Path: the 4:2:0 still/key path (`SVTAV1_MONO` unset is 4:2:0 in the tile
//! harness), which is what `tools/tile_gate.sh` exercises for tiles.
//! Geometry: 256x64 = 4 SB columns x 1 SB row; `tile_cols_log2 = 2` requests
//! 4 tile columns, so `num_tiles() == 4` and `thread_count` in {1,2,4,0}
//! spans multiple wave sizes (4 waves of 1, 2 waves of 2, 1 wave of 4, auto).

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

const W: usize = 256;
const H: usize = 64;

/// Deterministic 4:2:0 content (mirrors identity_run's "gradient" luma so the
/// tiles actually carry coded coefficients rather than a flat DC block).
fn make_420() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; W * H];
    for r in 0..H {
        for c in 0..W {
            y[r * W + c] = (((r * 255) / H) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    let (cw, ch) = (W / 2, H / 2);
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            u[r * cw + c] = (64 + ((r * 5) & 0x3f)) as u8;
            v[r * cw + c] = (64 + ((c * 7) & 0x3f)) as u8;
        }
    }
    (y, u, v)
}

fn encode(tile_cols_log2: u8, thread_count: usize) -> Vec<u8> {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 40,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(W as u32, H as u32, 6, rc, 0, 1)
        .with_chroma_420(true)
        .with_tile_cols_log2(tile_cols_log2)
        .with_thread_count(thread_count);
    let (y, u, v) = make_420();
    p.encode_frame_420(&y, &u, &v, W)
}

#[test]
fn multitile_encode_is_thread_count_invariant() {
    // Baseline: fully serial spawning (one tile per wave).
    let baseline = encode(2, 1);
    assert!(!baseline.is_empty(), "multi-tile encode produced no output");

    // Every other concurrency bound must yield byte-identical output:
    //   2 -> two waves of two, 4 -> one wave of four, 0 -> auto (clamped to
    //   num_tiles). If any diverges the wave rewrite is NOT byte-inert.
    for &tc in &[2usize, 4, 0] {
        let out = encode(2, tc);
        assert_eq!(
            out, baseline,
            "multi-tile encode diverged at thread_count={tc} (bounded threading must be \
             byte-inert; tiles are reassembled in tile order)"
        );
    }
}

#[test]
fn multitile_gate_is_not_vacuous() {
    // Anti-vacuity: if `tile_cols_log2 = 2` did not actually produce more
    // than one tile, the invariance test above would be trivially true. A
    // 4-tile encode MUST differ from a single-tile encode of the same content
    // (tile boundaries reset entropy contexts and add tile-group syntax), so
    // this inequality confirms the parallel wave path is genuinely exercised.
    let single_tile = encode(0, 1);
    let four_tiles = encode(2, 1);
    assert_ne!(
        single_tile, four_tiles,
        "tiles are not active — the determinism test would be vacuous"
    );
}

// ---------------------------------------------------------------------------
// Widened axes (2026-09-09).
//
// The cell above is 256x64 with tile_cols_log2=2 -> 4 tiles, exercised at
// thread_count {1,2,4,0}. Every one of those is a DIVISOR of 4, so the wave loop
// never runs a SHORT final wave; it also never varies the geometry, never uses
// tile ROWS, and never runs at 10 bits, where an extra per-tile `tile_canvas10`
// assembly exists that the 8-bit path does not have.
// ---------------------------------------------------------------------------

/// Deterministic content at an arbitrary size, same shape as `make_420`.
fn make_420_at(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            y[r * w + c] = (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8);
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            u[r * cw + c] = (64 + ((r * 5) & 0x3f)) as u8;
            v[r * cw + c] = (64 + ((c * 7) & 0x3f)) as u8;
        }
    }
    (y, u, v)
}

fn encode_at(
    w: usize,
    h: usize,
    preset: u8,
    rows_log2: u8,
    cols_log2: u8,
    thread_count: usize,
) -> Vec<u8> {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 40,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(w as u32, h as u32, preset, rc, 0, 1)
        .with_chroma_420(true)
        .with_tile_rows_log2(rows_log2)
        .with_tile_cols_log2(cols_log2)
        .with_thread_count(thread_count);
    let (y, u, v) = make_420_at(w, h);
    p.encode_frame_420(&y, &u, &v, w)
}

/// NON-DIVISOR thread counts, tile ROWS, and a larger grid.
///
/// 3, 5 and 7 against a 16-tile grid all leave a short final wave, which the
/// original {1,2,4} over 4 tiles never produced.
#[test]
fn tile_grid_is_thread_count_invariant_including_short_final_waves() {
    for (w, h, rows_log2, cols_log2) in [
        (256usize, 256usize, 2u8, 2u8),
        (256, 256, 2, 0),
        (256, 256, 0, 2),
    ] {
        let baseline = encode_at(w, h, 6, rows_log2, cols_log2, 1);
        assert!(!baseline.is_empty());

        // ANTI-VACUITY, per geometry: a tiled encode must differ from the
        // single-tile encode of the same content. Tile boundaries reset entropy
        // contexts and add tile-group syntax, so if these match, the grid
        // collapsed to one tile and the invariance below proves nothing.
        let single = encode_at(w, h, 6, 0, 0, 1);
        assert_ne!(
            single, baseline,
            "{w}x{h} r{rows_log2}c{cols_log2}: tiled output equals single-tile \
             output, so the grid collapsed and this cell is vacuous"
        );

        for tc in [2usize, 3, 5, 7, 16, 0] {
            assert_eq!(
                encode_at(w, h, 6, rows_log2, cols_log2, tc),
                baseline,
                "{w}x{h} r{rows_log2}c{cols_log2}: output changed at \
                 thread_count={tc}; tile-parallel encoding must be bit-exact \
                 regardless of how the waves are scheduled"
            );
        }
    }
}

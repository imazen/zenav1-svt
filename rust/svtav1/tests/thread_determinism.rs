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

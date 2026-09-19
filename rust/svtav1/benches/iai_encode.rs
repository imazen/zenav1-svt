//! Instruction-count (callgrind) benchmark for the still-encode perf cell —
//! the same (content, w, h, qp, preset, pipeline config) encode that
//! `examples/perf_encode` times as ENCODE_NS, measured in callgrind Ir instead
//! of wall clock. Setup (file load + pipeline construction) is NOT counted —
//! `#[bench::…(setup())]` args are evaluated outside the measured function,
//! matching perf_encode's "time only encode_frame_420" contract.
//!
//! Run the default build:
//!   cargo bench --bench iai_encode -p zenav1-svt
//!
//! ISA-variant builds (separate target dirs so they coexist):
//!   CARGO_TARGET_DIR=target-v3 RUSTFLAGS="-C target-cpu=x86-64-v3" \
//!     cargo bench --bench iai_encode -p zenav1-svt
//!
//!   # target-cpu=native needs valgrind-safe feature pruning — LLVM emits
//!   # GFNI (vgf2p8affineqb) and friends that valgrind-3.26 cannot execute:
//!   CARGO_TARGET_DIR=target-native RUSTFLAGS="-C target-cpu=native
//!     -C target-feature=-gfni,-vaes,-vpclmulqdq,-avxvnni,-avxvnniint8,-avxvnniint16,-avxifma,-avxneconvert,-sha" \
//!     cargo bench --bench iai_encode -p zenav1-svt
//!
//! Compare two runs:
//!   cargo bench --bench iai_encode -p zenav1-svt -- --save-baseline=base
//!   (rebuild with other flags) … -- --baseline=base
//!
//! NOTE: `-C target-cpu=x86-64-v3` (and native) lets callers satisfy
//! `#[target_feature]`, so `#[arcane]` SIMD kernels INLINE into their scalar
//! callers — leaf self-cost symbols reshape between builds. Compare inclusive
//! costs of stable drivers (tx_unit_inner, optimize_b, cost_coeffs_txb) or
//! total Ir, not leaf self costs.

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use svtav1::encoder::pipeline::EncodePipeline;
use svtav1::encoder::rate_control::{RcConfig, RcMode};

const W: usize = 512;
const H: usize = 512;
const QP: u8 = 32;
const PRESET: u8 = 6;
const CELL_YUV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/perfgap/1014.yuv");

/// The full cell: decoded I420 planes + a fresh pipeline, built outside the
/// measured region (iai does not count `#[bench]` argument evaluation).
fn setup_cell() -> (Vec<u8>, Vec<u8>, Vec<u8>, EncodePipeline) {
    let bytes = std::fs::read(CELL_YUV).expect("read 1014.yuv");
    let ysz = W * H;
    let csz = (W / 2) * (H / 2);
    assert!(bytes.len() >= ysz + 2 * csz, "cell yuv too small");
    let y = bytes[..ysz].to_vec();
    let u = bytes[ysz..ysz + csz].to_vec();
    let v = bytes[ysz + csz..ysz + 2 * csz].to_vec();

    // Same pipeline config as perf_encode's still arm: CQP, hierarchical
    // levels 0, intra_period 1 == allintra/still, SB size by C's rule.
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: QP,
        ..RcConfig::default()
    };
    let p = EncodePipeline::new(W as u32, H as u32, PRESET, rc, 0, 1)
        .with_bit_depth(8)
        .with_tile_rows_log2(0)
        .with_tile_cols_log2(0)
        .with_sb_size(None)
        .with_chroma_420(true);
    (y, u, v, p)
}

#[library_benchmark]
#[bench::cell(setup_cell())]
fn encode_still_1014((y, u, v, mut p): (Vec<u8>, Vec<u8>, Vec<u8>, EncodePipeline)) -> Vec<u8> {
    black_box(p.encode_frame_420(black_box(&y), black_box(&u), black_box(&v), W))
}

library_benchmark_group!(name = still; benchmarks = encode_still_1014);
main!(library_benchmark_groups = still);

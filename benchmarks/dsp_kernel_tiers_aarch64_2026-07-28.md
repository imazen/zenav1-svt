# DSP kernels on aarch64: the NEON tier was never implemented — 2026-07-28

Platform: Apple Silicon (aarch64, NEON), darwin 25.5.0
Bench: `rust/crates/svtav1-dsp/benches/kernel_tiers.rs` (zenbench, interleaved arms)

## Finding 1: every `_neon` arm was a placeholder

All 32 `incant!` sites in `svtav1-dsp` dispatch `[v3, neon, scalar]`, so the crate *advertises*
a NEON tier. Every `_neon` implementation was scalar code wrapped in `#[arcane]`. `sad.rs`
even said so:

```rust
// NEON: use vabdl_u8 + vpaddlq for absolute difference and accumulate
// Starting with scalar-with-autovectorize; will add explicit NEON intrinsics
```

`#[arcane]` adds `#[target_feature(enable = "neon")]`, which is a **no-op on aarch64** because
NEON is baseline. So the NEON tier was bit-for-bit the scalar tier, and the first run of this
bench measured a uniform ~1.00× across all seven kernels — exactly what "both arms are the
same code" looks like.

An audit of every `*_impl_neon` in the crate found the same pattern in ~30 kernels. Three are
now implemented (sad, variance, sse); the rest remain placeholders: forward and inverse
transforms, quant, quant_coding, restoration, intra_pred, inter_pred, hbd cdef, hadamard/satd,
fwd_txfm, inv_txfm.

Note `satd_4x4`/`satd_8x8` and `cdef_find_dir_8bit` measure ~1.00× and are NOT yet done — they
operate on 4- and 8-wide rows, so a 16-lane kernel would be all tail. They need either a
different vectorization strategy (transpose-based Hadamard) or batching across blocks.

## Finding 2: SAD implemented for real — up to 8.7×

`sad_impl_neon` now uses `vabdq_u8` for the absolute difference and widening pairwise adds
(`vpadalq_u8` → u16, `vpadalq_u16` → u32) with a per-row drain so the u16 accumulator cannot
overflow.

| kernel | before (scalar-as-neon) | after | speedup |
|---|---|---|---|
| sad_8x8 | 80.2 ns | 79.5 ns | 1.00× (8 < 16 lanes — all tail) |
| sad_16x16 | 312.9 ns | **44.8 ns** | **6.98×** |
| sad_32x32 | 888 ns | **102 ns** | **8.7×** |
| sad_64x64 | 2558 ns | **402 ns** | **6.4×** |

SAD runs at every candidate position in motion and mode search, so this is on the encoder's
hottest path.

Exact by construction — absolute difference and addition are exact in integer lanes.
`tests/sad_neon_parity.rs` pins it across every AV1 block dimension plus widths that are not
multiples of 16 (exercising the scalar tail), and an all-255-vs-all-0 worst case where the
running sum reaches 4,177,920 — which overflows u16 many times over and would expose a missing
accumulator drain.

## Finding 2b: variance and sse implemented — 4.2×–6.1×

Same treatment: `vabdq_u8` for the difference, `vmull_u8` to square into u16, and `vpadalq`
to drain into u32. The drain cadence is the whole correctness story — a u16 lane tops out at
255² = 65025, so squares cannot accumulate there.

| kernel | scalar | NEON | speedup |
|---|---|---|---|
| variance_16x16 | 207.4 ns | **35.8 ns** | **5.8×** |
| variance_64x64 | 2847 ns | **675 ns** | **4.2×** |
| sse_16x16 | 289.5 ns | **47.1 ns** | **6.1×** |
| sse_64x64 | 2505 ns | **436 ns** | **5.7×** |

`sse` drains its u32 accumulator to u64 per *row*, so block height is unbounded; `variance`
drains per 16-byte chunk, which covers the 128×128 maximum used here (worst case
65025 × 16384 = 1.07e9, inside u32's 4.29e9).

`tests/variance_neon_parity.rs` pins both against scalar across every block dimension and
non-multiple-of-16 widths, plus an all-255-vs-all-0 case summing to 1,065,369,600 — which
overflows a u16 lane by ~16000× and would immediately expose a missing drain.

## Finding 3: the C-parity gates could not LINK on ARM

`svtav1-cref` declared `svt_aom_hadamard_{16x16,32x32}_avx2` in an **ungated** `extern` block.
An aarch64 build of the C library has no AVX2 symbols (it ships NEON kernels instead), so every
`c_parity_*` test failed to link with `Undefined symbols for architecture arm64`.

For a project whose stated bar is *byte-identical OBUs against the C encoder*, this meant the
verification gates were unavailable on the architecture being optimized. Fixed by gating the
extern block, the `hadamard_avx2` wrapper, and the tests that call it to `target_arch =
"x86_64"`.

**233 tests now pass against the real C encoder on aarch64**, including 12 `c_parity_*` suites
that previously could not build.

Note for whoever continues: on ARM the C encoder dispatches its own NEON kernels, so ARM
parity should ultimately be pinned against *those*, not against the `_c` reference. The AVX2
shim exists precisely because it diverges from `_c` at 10-bit (see the comment above
`hadamard_avx2`); the NEON kernels may diverge similarly and that is unverified.

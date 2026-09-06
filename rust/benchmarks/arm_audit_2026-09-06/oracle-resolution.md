# PD0 quantizer oracle resolution

Synced Rust main: 614c1e8b. Local C reference source: 39f909e0 (the existing
saturating-cast oracle branch, not the outer repository submodule pin).
Apple M4 Pro, rustc 1.98.0, LLVM 22; runtime SIMD dispatch, no native CPU flag.

## Decision

Use C scalar for the original wide-input clamp regression. Use both C scalar
and the actual RTCD-dispatched C kernel for producer-derived PD0 inputs,
without altering or clamping those coefficients. Also compare both kernels on
the symmetric [-32767,32767] coefficient grid. Explicitly assert the known
C NEON divergence outside that grid instead of silently dropping those cases.
No production quantizer arithmetic changes are needed for the measured inputs.

## Evidence

The original test still fails on synced main at qindex 0 / Tx4x4 / pattern 2:
Rust and independent scalar reference give EOB 16, C NEON gives EOB 14.
C `Source/Lib/ASM_NEON/mem_neon.h:1276` narrows i32 with `vmovn_s32`, and
`av1_quantize_neon.c:661` takes signed i16 absolute values. Thus ±131072
narrows to zero and ±32768 reaches the unrepresentable positive i16 absolute
value. For a uniform block of any of those four values, the C NEON result is
EOB zero and all-zero qcoeff/dqcoeff; the scalar result is nonzero. The new
AArch64 regression asserts this behavior explicitly.

The producer regression uses the same forward DCT dispatch and 64-dimension
packing as `pd0::tx_quant_core`: 12 shapes, 16 residual patterns within
[-255,255], all 256 qindices, and enabled/disabled Rust SIMD tiers. All
producer outputs match both C oracles. Maximum observed coefficient magnitude:

| Shape | Maximum |
|---|---:|
| 4x4 | 8161 |
| 8x4 | 11545 |
| 8x8 | 16321 |
| 8x16 | 23083 |
| 16x8 | 23091 |
| 16x16 | 32637 |
| 16x32 | 23074 |
| 32x16 | 23074 |
| 32x32 | 32625 |
| 32x64 | 23079 |
| 64x32 | 23072 |
| 64x64 | 32637 |

This is a producer regression grid, not an exhaustive proof that every legal
residual block lies inside the symmetric i16 range. The test deliberately does
not clamp producer output; a future valid-input divergence must fail here and
be investigated against the dispatched encoder behavior. Earlier comments in
`tests/c_parity_quant.rs` asserting a universal input contract are stronger
than this evidence establishes.

Focused validation: all four PD0 parity tests passed, including the unchanged
energy test. Full workspace: 2556/2556 tests passed, zero skipped; test phase
62.349 seconds. Whole-encoder regression spotcheck: 104/104, zero skips. The macOS C
capture driver uses its existing byte-only fallback; this is not operation-trace
coverage. No production arithmetic or public API changed.

Landing was rebased over 136e779c, whose only executable change removes an
unused film-grain estimation call; no quantizer or test changes overlapped.
Clippy completed with existing encoder/test warnings (including the private
Pd0Mode public-interface warning); no warning was emitted for the added test
lines. The full-test and spotcheck records above precede that rebase.

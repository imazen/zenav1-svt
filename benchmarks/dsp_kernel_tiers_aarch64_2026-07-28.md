# DSP kernels on aarch64: the NEON tier was never implemented — 2026-07-28

> **Re-verified at HEAD 2026-08-07 — read
> `rust/benchmarks/neon_tier_audit_2026-08-07.md` alongside this file.**
> Two claims below have moved. (1) `quant_coding` and `inter_pred`, listed here
> as placeholders, now have real intrinsics. (2) The shared-source transform
> port described at the end of this file WAS done — `txfm_simd.rs` has a
> `mod neon` — but it is `[i32; 8]` arrays relying on autovectorisation, not
> intrinsics, and it is capped at `NEON_FWD_MAX_DIM = 16` /
> `NEON_INV_MAX_DIM = 8` because past those dims it measured *slower* than
> scalar. So forward 32/64-point and inverse 16/32/64-point transforms still
> have **no vector tier on aarch64**, and they are top-ten self-time leaves in
> the whole-encoder profile. Also: the seven `*_impl_neon` arms in
> `fwd_txfm.rs`/`inv_txfm.rs` have bodies byte-identical to their `_impl_scalar`
> siblings (harmless — the real dispatch is one level down in `txfm_simd` — but
> the names mislead an audit).

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

An audit of every `*_impl_neon` in the crate found the same pattern in ~30 kernels. Seven are
now implemented (sad, variance, sse, paeth, cdef_filter_block, satd_8x8, quantize); the rest
remain placeholders: forward and inverse
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

## Finding 2c: paeth intra prediction — 6.7× / 8.3×

Intra prediction is THE hot path in an all-intra (AVIF) encoder, and paeth has a structural
property worth exploiting: within a row, `left` and `top_left` are constant and only `above`
varies by column, so one of the three distances collapses to a per-row scalar.

```
base   = top + lft - tl
p_top  = |base - top| = |lft - tl|            <- row-constant
p_left = |base - lft| = |top - tl|
p_tl   = |base - tl|  = |top + lft - 2*tl|
```

i16 lanes are required, not u16: `top + lft - 2*tl` spans [-510, 510].

| block | scalar | NEON | speedup |
|---|---|---|---|
| paeth_16x16 | 566.7 ns | **84.7 ns** | **6.7×** |
| paeth_32x32 | 2243 ns | **271 ns** | **8.3×** |

Verified over the **entire 2²⁴ (top, left, top_left) domain**, not sampled. Paeth is a
three-way argmin whose distances are frequently EQUAL, and the tie order (top, then left, then
top_left) is what makes it deterministic — a vectorized select with the wrong tie order still
produces plausible pixels while silently changing every intra block. Sampling is exactly what
would miss that, so the test sweeps all 16.7M triples plus every block shape through both the
vector body and the scalar tail.

## Finding 2d: CDEF filter — 3.37×, and the C-parity suite earned its keep

`cdef_filter_block` is applied to every reconstructed block. Ported from the existing AVX2 arm
rather than invented: each output pixel is an independent 12-tap integer sum, so 8 columns map
to 8 lanes with no cross-lane reduction. NEON's i32 vectors are 4 lanes, so each 8-wide
quantity is carried as a `[int32x4_t; 2]` pair.

| kernel | scalar | NEON | speedup |
|---|---|---|---|
| cdef_filter_block_8x8 | 1757 ns | **521 ns** | **3.37×** |

**The first version was wrong, and `filter_block_sign_straddle_matches_c` caught it.** The AVX2
arm loads taps with `_mm256_cvtepi16_epi32` — it reads the buffer as `int16_t`, so values at or
above 0x8000 are NEGATIVE. I used `vmovl_u16`, which zero-extends. Every other CDEF parity test
passed; only the one built specifically to place taps either side of 0x8000 failed, with output
differing by small amounts (246 vs 242, 210 vs 209) that would have looked like rounding noise
in a visual check.

That is the concrete payoff from Finding 3 below: had the C-parity gates still been unlinkable
on ARM, this port would have shipped looking correct.

## Finding 2e: satd_8x8 — 4.06×, via a reordering that needed proving

Earlier in this sweep `satd_8x8` was set aside as needing "a different strategy" because it
works on 8-wide rows, so a 16-lane u8 kernel would be all tail. The strategy is to change the
lane type, not the width: `int16x8_t` gives exactly 8 lanes for 8 columns.

| kernel | scalar | NEON | speedup |
|---|---|---|---|
| satd_8x8 | 86.5 ns | **21.3 ns** | **4.06×** |

The NEON path deliberately **reorders the transform**: it runs BOTH Hadamard passes vertically
(one lane per column, no horizontal ops) with a single 8×8 transpose between, where the scalar
runs rows then columns. That is valid because a 2D separable Hadamard commutes and the result
is an absolute sum over all 64 coefficients — but "valid in theory" is exactly the kind of
claim worth pinning, since a wrong transpose yields a plausible SATD that silently changes mode
decisions.

i16 lanes suffice and no widening is needed: the residual is in [-255, 255] and a 2D 8-point
Hadamard amplifies by at most 64, so |coefficient| ≤ 16320, inside i16's 32767.

`tests/satd_neon_parity.rs` covers 5000 random blocks, maximum-amplitude blocks (every residual
±255, driving coefficients to ±16320 — where a wrong lane width overflows), and an
**asymmetric** alternating pattern chosen specifically because a transposed error is invisible
on symmetric input.

## Finding 2f: quantize — 2.2×, exact via a checked bound rather than an assumed one

`quantize_core` divides by `dequant` per coefficient and NEON has no integer divide. It was
deferred twice in this sweep as "needs its own careful pass". The pass:

| block | scalar | NEON | speedup |
|---|---|---|---|
| 64 coefficients | 79.3 ns | **35.8 ns** | **2.2×** |
| 1024 coefficients | 1034 ns | **522 ns** | 2.0× |

The quotient is computed in f32, which is exact only while the numerator is under 2^24 — the
same argument proved for BRAG's unpremultiply. But `TranLow` is `i32` and `shift` is
caller-supplied, so unlike BRAG that bound is **not guaranteed by the types**.

Rather than assume a coefficient range, the kernel **checks** it: one pass takes the maximum
|coeff|, and if `max << shift` could reach 2^24 the whole block falls back to `quantize_core`.
Real AV1 coefficients are far below that so the fast path is what runs — but a pathological
input gets the scalar answer, not a wrong one.

`eob` is recovered by a backward scan instead of being tracked in the loop, which keeps the
vector body branch-free.

`tests/quantize_neon_parity.rs` covers shapes 1–1024, DC/AC divisors including 0, shifts 0–4,
partial `eob_hint`, all-zero input, and inputs deliberately on **both sides** of the 2^24
bound so the fallback itself is exercised.

One note on that last test: its first version used coefficients so large that the SCALAR
reference overflowed `q * dequant` in i32 — it passed in release (wrapping) and failed in
debug (overflow panic). The values now straddle the bound while keeping the final multiply
inside i32, because a test that only exercises its own arithmetic overflow proves nothing.

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


## The transforms: measured, scoped, NOT ported

Transforms are the most expensive kernels measured in this crate:

| kernel | cost (all tiers identical — see below) |
|---|---|
| fwd_txfm2d 16x16 dct_dct | 1171 ns |
| inv_txfm2d 8x8 dct_dct | 835 ns |
| fwd_txfm2d 8x8 dct_dct | 424 ns |
| fwd_txfm2d 4x4 dct_dct | 183 ns |

For comparison, the CDEF filter ported above is 521 ns and SAD 16x16 is 45 ns. An all-intra
encoder runs a forward transform per block plus an inverse for reconstruction, so this is the
largest remaining NEON opportunity in the crate.

### What exists

- `fwd_txfm.rs` / `inv_txfm.rs`: the `*_impl_v3` arms are **also placeholders** — they call the
  same `*_c_exact` body as scalar and neon. Nothing is vectorized at that layer on any target.
- The real SIMD lives one level down: `fwd_txfm2d_c_exact` routes to
  `txfm_simd::try_fwd_dct_square` etc., and *that* has a genuine AVX2 implementation —
  `txfm_simd.rs` contains 47 x86 intrinsic uses and **zero** NEON.
- Structure: `mod v3` is 9 small primitives (`splat`, `hbtf`, `clampv`, `round_shift_v`,
  `wraplow`, `rect_scale`, `transpose8`, `load8`, `store8`), and
  `txfm_simd_drivers.rs` (228 lines, 4 entry points, only 13 direct intrinsic uses) generates
  `fwd_dct_{8,16,32,64}` from a macro.

### An attempted shortcut, and why it does not work

The transform SIMD is written ONCE and `include!`d into `mod v3`
(`txfm_simd_kernels.rs` 1374 lines, `txfm_simd_drivers.rs` 228,
`txfm_simd_rect.rs` 268). Those files barely touch x86 types directly: drivers and rect contain
**zero** `__m256i`, and across all three there are only **five distinct intrinsics**
(`_mm256_setzero_si256` ×26, `_mm_cvtsi32_si128` ×8, `_mm256_sub_epi32` ×3,
`_mm256_add_epi32` ×2).

That suggests an elegant port: add `type Vec8` / `type Tok` aliases plus four wrapper
primitives, making the shared sources fully backend-agnostic, then have a `mod neon` supply the
same 14 primitive names over `[int32x4_t; 2]` and `include!` the identical 1870 lines. One copy
of the butterflies; a backend becomes 14 small functions.

**The alias half works** — implemented and verified byte-identical (242 tests). The include
half first appeared blocked: `#[rite]` is a proc macro that pattern-matches the token
parameter's type *by name* and rejects an alias:

```
error: rite requires a token parameter or a tier name. Supported forms:
       - Tier name: `#[rite(v3)]`, `#[rite(neon)]`
       - Concrete: `token: X64V3Token`
```

**But the error names the way out, and it is verified to work.** The tier-name form takes no
token type at all, and the two backends are already `cfg`'d by architecture — so `cfg_attr`
can pick the tier per target while the token stays aliased:

```rust
pub(super) type Tok = Desktop64;          // or NeonToken in mod neon

#[cfg_attr(target_arch = "x86_64", rite(v3))]
#[cfg_attr(target_arch = "aarch64", rite(neon))]
pub(super) fn splat(_t: Tok, v: i32) -> Vec8 { … }
```

This was probed on a single primitive and **compiles clean**. On x86_64 only `mod v3` exists,
so the shared source gets `rite(v3)`; on aarch64 only `mod neon` does, so it gets
`rite(neon)`. No archmage change is needed.

So the shared-source port is viable, and the remaining work is mechanical:

1. Swap `#[rite]` → the `cfg_attr` pair throughout the three shared files.
2. Add `Vec8` / `Tok` / `ShiftC` aliases and the four wrapper primitives
   (`zero`, `shiftc`, `addv`, `subv`) so the shared files stop naming intrinsics — there are
   only five distinct ones to abstract.
3. Write `mod neon` with the 14 primitives over `[int32x4_t; 2]` and `include!` the same
   sources. The only one with real content is `transpose8` (four 4×4 `vtrnq_s32` transposes
   plus a quadrant swap) and `rect_scale`, which is actually EASIER on NEON — it needs a true
   i64 product, and NEON has `vmull_s32` / `vmull_high_s32` plus a signed 64-bit shift, so it
   avoids the even/odd lane dance the AVX2 arm documents.

`tests/c_parity_txfm.rs` gates the result against real C with SIMD-stressing residual
patterns — the same gate that caught the CDEF sign-extension bug in this sweep.

**Not attempted here**, and this is a risk decision rather than an unknown: the port touches
1870 lines of shared butterfly source, and a DCT rounds at every stage so an error propagates
through the whole transform. It should be done with a clear head, not appended to a long
session that had already produced several mechanical slips.

### If the shared-source route is rejected, a transliterated port requires### What a transliterated port requires

Porting the 9 primitives is mechanical — `hbtf` is `vmulq_s32`/`vaddq_s32` plus a variable
shift, `clampv` is `vminq_s32`/`vmaxq_s32`. Two things are not mechanical:

1. **`transpose8`**: an 8x8 i32 transpose. AVX2 does it with 256-bit shuffles; NEON has 4-lane
   vectors, so it becomes four 4x4 transposes built from `vtrnq`/`vzipq` plus cross-block
   moves. This is the one piece with no direct correspondence.
2. **Lane width**: every 8-wide quantity becomes a `[int32x4_t; 2]` pair, as in the CDEF port
   above. The macro that generates the butterflies has to be adapted, not just the leaves.

### Why it is not done here

A DCT butterfly applies `half_btf` with exact rounding at every stage; unlike CDEF's
independent per-pixel sums, an error at one stage propagates through the rest of the
transform. `tests/c_parity_txfm.rs` WOULD catch it — it has forward/inverse tests against C
with SIMD-stressing residual patterns, and it caught the CDEF sign-extension bug in this same
session — but a port attempted while tired is how that bug happened in the first place, and it
took a purpose-built boundary test to surface it.

This is a well-defined next task with a working verification gate, not an open question.

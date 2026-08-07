# NEON tier audit at HEAD — re-verifying `dsp_kernel_tiers_aarch64_2026-07-28.md`

Method: for every function in `crates/svtav1-dsp/src/` whose name ends in
`_neon`, extract its body by brace-matching and count aarch64 intrinsic tokens
inside it; then resolve thin wrappers to the helper they call. Filenames and
doc claims were not trusted. Commit `1e0e3ef9c`.

The sibling rav1d-safe project just found three `*_arm.rs` modules with zero
aarch64 intrinsics — the scalar reference wearing a NEON name. **That exact
pattern is present here, in two forms.** Neither is a lie in the docs — one is
documented and measured, the other is a naming artefact with no runtime effect
— but both mean "has a NEON tier" is false for the kernels involved.

## A. Real NEON — intrinsic count inside the function body

| module | function | intrinsics | note |
|---|---|---|---|
| `sad.rs` | `sad_impl_neon` | 9 | |
| `variance.rs` | `variance_impl_neon`, `sse_impl_neon` | 14, 13 | |
| `intra_pred.rs` | `predict_paeth_impl_neon` | 19 | paeth only; every other intra mode is scalar |
| `hadamard.rs` | `satd_4x4_impl_neon`, `satd_8x8_impl_neon` | 36, 13 | |
| `quant.rs` | `quantize_impl_neon` | 16 | |
| `quant_coding.rs` | `quantize_fp_raster_impl_neon`, `quantize_b_raster_impl_neon` | 29, 31 | **added since the 2026-07-28 doc**, which listed `quant_coding` as a placeholder |
| `cdef.rs` | `cdef_filter_cols8_neon` (+ load/constrain helpers) | 35 (+18) | 8-wide only |
| `inter_pred.rs` | `convolve_horiz_body_neon`, `convolve_vert_body_neon` | 26, 14 | **added since the doc** |
| `copy.rs` | `block_average_impl_neon`, `block_blend_impl_neon` | 4, 21 | |
| `restoration.rs` | `mac_row_i32_neon` | 7 | see §C |
| `hbd.rs` | `cdef_filter_block_hbd_impl_neon` | 0, but routes 8-wide to `cdef_filter_cols8_neon` | |

So the doc's "seven implemented" is now **eleven**, and two of the modules it
listed as placeholders (`quant_coding`, `inter_pred`) are real.

## B. The pattern the task asked about — found twice

**B1. `fwd_txfm.rs` / `inv_txfm.rs`: seven `*_impl_neon` arms whose bodies are
byte-for-byte identical to their `*_impl_scalar` siblings.** e.g.

```rust
fn fwd_txfm2d_4x4_dct_dct_impl_neon(_token: NeonToken, input: &[TranLow],
                                    output: &mut [TranLow], stride: usize) {
    fwd_txfm2d_c_exact(input, output, stride, 4, 4, 0, 0, false, false);
}
// ...and fwd_txfm2d_4x4_dct_dct_impl_scalar has EXACTLY that body.
```

Affected: `fwd_txfm2d_{4x4,8x8,16x16,32x32}_dct_dct_impl_neon`,
`inv_txfm2d_{4x4,8x8,16x16}_dct_dct_impl_neon`. The `incant!([v3, neon,
scalar])` at each site therefore has two identical arms on aarch64.

Runtime effect: **none**, because `fwd_txfm2d_c_exact` routes one level down
into `txfm_simd::try_fwd_dct_square`, which *does* dispatch a real aarch64 arm.
The names are what lie, not the behaviour. Worth deleting or renaming so the
next audit does not have to re-derive this.

**B2. `txfm_simd.rs`'s `mod neon` contains ZERO aarch64 intrinsics.** Its
vector type is `pub(super) type __m256i = [i32; 8];` — plain safe Rust arrays
`include!`d through the same 3,000 lines of shared butterfly source the AVX2
arm uses, relying on LLVM autovectorisation.

This *is* documented in the module's own doc comment and it is honest about it.
The consequence is measured and pinned in source:

```rust
#[cfg(target_arch = "aarch64")] const NEON_FWD_MAX_DIM: usize = 16;
#[cfg(target_arch = "aarch64")] const NEON_INV_MAX_DIM: usize = 8;
```

with a comment recording that beyond those dims the autovectorised arm **loses
to scalar** (inv 16×16: 1.2 µs → 1.7 µs; inv 32×32: 5.1 µs → 8.6 µs). All ten
`try_*_impl_neon` entry points return `false` above the cap, falling back to
the scalar transform.

**Net: on aarch64 there is no vector transform tier at all for forward
32/64-point or inverse 16/32/64-point.** That is exactly what the encode
profile shows — `idct32`, `idct16`, `idct64`, `fdct32`, `fdct64` are top-ten
self-time leaves at every preset, and the forward/inverse transform stages
together are 28 % of the p6 gap and 39 % of the p2 gap. C ships
`svt_lbd_fwd_txfm2d_{16x16,32x32,32x16,16x8,...}_neon`,
`highbd_fdct64_x4_neon` and dav1d's `svt_dav1d_inv_txfm_add_neon` for these.

## C. `restoration::compute_stats` — the "NEON" arm is a scalar gather

`compute_stats_impl_neon` is 70 lines with **0** intrinsics of its own; its
only vector content is the helper `mac_row_i32_neon`
(`acc[i] += vals[i] as i32 * scalar`, 7 intrinsics). The surrounding structure
— a per-source-pixel column-major gather of a 7×7 = 49-element window through
a strided `dgd` plane, then 1 + 49 short `mac_row` calls (~1,274 MACs/pixel) —
is tier-agnostic scalar Rust.

Measured consequence: at p6 512×512 this single function is **22.7 % of the
port's self time (≈19.2 ms) against 0.74 ms for C's
`svt_av1_compute_stats_neon` — a ~26× per-kernel gap**, the single largest
line item in the whole comparison.

## D. Modules with no aarch64 arm at all

`loop_filter.rs`, `obmc.rs`, `resize.rs`, `scale.rs`, `superres.rs`,
`warp.rs`, `ac_bias.rs` contain zero mentions of NEON — not even a placeholder.
C ships `svt_aom_lpf_{horizontal,vertical}_{4,6,8,14}_neon` plus highbd twins
for the deblock case.

## E. Scale of the surface

`nm -gU Bin/Release/libSvtAv1Enc.a | grep -c neon` → **1,042 distinct NEON
symbols** in the C library (plus SVE, `dotprod` and `i8mm` variants, e.g.
`svt_aom_compute_cdef_dist_8bit_neon_dotprod`, `compute_stats_win7_sve`).
The port has ~11 real NEON kernels. That ratio, not any single defect, is the
shape of the gap.

Specific C kernels covering our measured hot spots that the port has no
counterpart for:

- `svt_av1_compute_stats_neon` / `_sve` (§C)
- `svt_av1_fwd_txfm2d_*_neon`, `highbd_fdct64_x4_neon`, `svt_dav1d_inv_txfm_add_neon` (§B2)
- `svt_av1_get_nz_map_contexts_neon`, `svt_av1_txb_init_levels_neon`,
  `svt_av1_compute_cul_level_neon` (the port's nz-map/level contexts are
  9–45× C's)
- `svt_av1_cdef_filter_block_4xn_8_native_neon` — the port falls back to the
  scalar `cdef_filter_block_core` for every 4-wide chroma block, which is
  6.08 % of self time at p10
- `svt_aom_lpf_*_neon`
- `svt_residual_kernel8bit_neon`, `svt_full_distortion_kernel32_bits_neon` —
  the port inlines both into `leaf_funnel::tx_unit`, whose self time is the
  #1 leaf at p2 (11.05 %), and builds the residual into a **freshly allocated
  `Vec` per transform unit**.

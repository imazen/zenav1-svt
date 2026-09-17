# perf-gap attribution + measured attempts, 512x512 p4 (aarch64)

* date: 2026-09-17
* host: Apple Silicon (darwin 25.5.0), aarch64
* commit: `404ff72f` (post native10-parity close)
* cell: `gradient 512x512 qp40 p4`, one frame — a byte-identical cell
  (port .obu == C .obu), so every ms below is pure speed difference.
* method: `tools/perf_profile/prof.sh` (macOS `sample`, port 15,240 /
  C 15,441 samples) + `tools/perf_profile/classify.py` bucketing;
  attempts measured with `tools/perf_ab.sh` interleaved paired rounds.

## Baseline

port 80.475 ms/encode vs C 44.358 ms/encode = **1.81x**. The gap (36.1ms)
is distributed — no bucket exceeds 26% of it:

| class | port_ms | C_ms | delta | share | port/C |
|---|---:|---:|---:|---:|---:|
| MD_DRIVER | 13.66 | 4.38 | +9.29 | 25.7% | 3.12x |
| INTRA_PRED | 11.92 | 5.43 | +6.49 | 18.0% | 2.20x |
| DISTORTION | 11.90 | 7.84 | +4.05 | 11.2% | 1.52x |
| FWD_TXFM | 7.62 | 4.08 | +3.54 | 9.8% | 1.87x |
| CDEF | 3.87 | 1.08 | +2.79 | 7.7% | 3.58x |
| DEBLOCK | 2.76 | 0.78 | +1.99 | 5.5% | 3.56x |
| LIBC_MEM | 3.20 | 1.67 | +1.53 | 4.2% | 1.91x |
| QUANT_RDOQ | 7.97 | 6.47 | +1.50 | 4.2% | 1.23x |
| ALLOC | 1.44 | 0.08 | +1.36 | 3.8% | 18.6x |
| INV_TXFM | 2.86 | 1.50 | +1.36 | 3.8% | 1.91x |
| SYNTAX_WRITE | 1.15 | 0.03 | +1.12 | 3.1% | 39.9x |
| COEFF_WRITE | 1.85 | 0.87 | +0.99 | 2.7% | 2.14x |

Top port symbols: `tx_unit_inner` (1307 — an aggregate; its hot lines are
the optimize_b/inv_txfm/recon/quantize *callsites*, i.e. orchestration
overhead not one defect), `dr_predictor_edged` (1140),
`optimize_b` (866), `cost_coeffs_txb` (762), `hadamard_8x8` (505),
`fwd_txfm2d_c_exact` (492 — see attempt 3), `predict_unit` (458),
`cdef_filter_cols8_neon` (380), `residual_i32/i16` (731).

Facts that ruled out easy answers:

* `fwd_txfm2d_c_exact`'s 492 samples are pure dispatch overhead:
  an instrumented run counted **2,000,000+ calls/encode, 0 scalar-path
  hits** — every legal (tx_size, tx_type) already resolves SIMD. The
  scalar `fwd_txfm2d_core` (three `vec!` allocs per call) is cold.
* `aom_hadamard_8x8` is already NEON (`aom_hadamard_8x8_impl_neon`); the
  505 samples are call-count from the funnel's screening granularity,
  not a missing kernel. C reaches the same screening via
  `full_distortion`/`satd` on different shapes.
* spatial distortion is already fused (`variance::sse(src, recon)`
  direct, matching `svt_spatial_full_distortion_kernel`); residuals are
  all transform inputs, not waste passes.
* `try_fwd_*` NEON impls BEAT C's per-size monoliths (fdct16_x8 265 vs
  `lbd_fwd_txfm2d_16x16_neon` 506; fdct32_x8 360 vs 560) — the composed
  1-D approach is fine; the loss is in the dispatch shell.

## Landed

1. **`cdef_dist_packed` arm_v2 (dotprod) arm** — the NEON arm was a
   *stub delegating to the scalar core* while x86 had `dist_block_v3`
   and C runs `compute_cdef_dist_8bit_neon_dotprod` (121 vs 27
   samples). New `cdef_dist_block_arm_v2` stages sampled rows
   contiguously (the `dist_block_v3` shape) and folds with
   `vdotq_u32(acc, vabdq_u8(a,b), vabdq_u8(a,b))`. A/B 7r at p4 (CDEF
   search is `preset <= 6` only — the p10 deltas measured are layout
   noise): **1.016x / 1.009x** at 256/512. ident=Y; the
   `all_tiers_match_scalar` + `compute_cdef_dist_8bit_matches_c` FFI
   tests pass. LANDED.
2. **`predict_filter_intra_impl_neon` NEON arm** — second stub of the
   audit (port scalar 83 samples vs C's real NEON+i8mm kernels ~30).
   `filter_intra_emit_neon` mirrors `filter_intra_emit_v3`: the 8
   outputs of a 4x2 sub-block share the 7-tap input, accumulated as
   `vmlal_s16` into i32 (p[j]*tap fits s16, the sum does not), then
   `vrshrq_n_s32::<4>` + `vqmovun_s32`/`vqmovn_u16` — exact vs
   `ROUND_POWER_OF_TWO_SIGNED` because negatives clamp to 0 under both
   roundings. ident=Y; `filter_intra_all_tiers_match_scalar` +
   `filter_intra_predictor_matches_c` (FFI vs C) pass. LANDED.
3. Last stub of the audit: `predict_smooth_impl_neon` (plus
   `predict_smooth_v`/`predict_smooth_h`, which have no dispatch at
   all). The scalar core is branch-free and likely already
   LLVM-vectorized — LOW expected value, untried.

## Measured attempts (null or negative — DO NOT RETRY blindly)

1. **residual_i32/i16 packed NEON strips for w<16** (`n<=512`):
   packed strided rows into stack arrays, contiguous vector subtract.
   A/B 7r, grid 256/512 x p4/p10: speedups 0.987x / 1.002x / 0.998x /
   1.007x. LLVM already auto-vectorizes the scalar subtract tails; the
   pack/copy overhead is a wash. REVERTED.
2. **z2 flat NEON extension to 8-dim blocks** via a padded-segment
   `#[rite]` helper (copy <=16B windows, one vmlaq/vrshrn, copy out) +
   dispatch `bw>=8 && bh>=8`: byte-identical but 0.974x at 512p4 —
   the c0-table setup + per-segment copies cost more than the scalar
   core saves at these sizes. REVERTED. (C's alternative — vqtbl4q
   gather + vbslq select per 16-col group, `dr_prediction_z2_WxH_neon`
   — is a different structure; untried.)
3. **`#[inline]` the whole fwd-txfm dispatch chain** (dispatch ->
   c_exact -> try_* -> incant, plus extracting the fast path so the
   scalar core stays out-of-line): 12-round A/B at 512p4 = 1.001x,
   [p25,p75]=[0.991,1.004] — true null. LLVM was already handling it.
   REVERTED.

## Where the remaining gap actually lives

* **Funnel orchestration** (MD_DRIVER, +9.3ms): `tx_unit_inner` +
  `inject_candidates` (307) + `evaluate_leaf` (182) + nic/mds3 closures
  (~380) + `give_pooled` (164) + Cand/PoolVec drop glue (~45). Per-
  candidate fixed costs — buffer round-trips, slice/pool bookkeeping —
  vs C's shared `cand_bf` arena. Largest single bucket; needs a
  profiling-guided flatten of the eval loop, not a kernel.
* **INTRA_PRED** (+6.5ms): `dr_predictor_edged` is 1.8x C's combined
  directional kernels; z1/z3 NEON arms have the same >=16 gating as z2.
  C's vqtbl4q approach covers all sizes in one group per row.
* **CDEF** (3.58x): `cdef_filter_cols8_neon` 380 vs C's native 211 —
  same work, slower kernel; `cdef_dist_packed` 121 vs C dotprod 27
  (no i8mm/dotprod arm in the port?).
* **DEBLOCK** (3.56x): `filter_plane` driver 240 samples of per-mi-cell
  loop + `lpf14_window`/`filter8_line` kernels ~1.3x C's.
* **SYNTAX_WRITE** (39.9x but 1.1ms): `encode_block_syntax` per-block
  Rust overhead vs C's near-free writes.
* **ALLOC** (18.6x, 1.4ms): Cand/PoolVec churn in the funnel.

"Faster than C" needs most of these; it is a campaign, not a patch.
Every attempt above is documented so the next session doesn't re-pay
the measurement cost.

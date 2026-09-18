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
3. **Loop filters `lpf_{horizontal,vertical}_{6,8,14}` NEON arms** —
   the entire deblock ran scalar on aarch64 (`[v3, scalar]` dispatch);
   DEBLOCK was a +2.0ms/3.56x gap bucket with `filter8_line`/
   `lpf14_window` in the top self-time symbols. Full transliteration
   of the v3 (SSE2 `dlf_intrin_sse2.c` port) arms: same merged-pair
   vector shapes, `vabdq_u8`/`vqsubq_*`/`vzip1q_*`/`vextq`-against-zero/
   `vqmov{n,un}_s16`/`vbicq`/`vminvq_u8` mappings. Two transliteration
   traps fixed during bring-up: `_mm_packs_epi16` is a SIGNED narrow
   (0xFFFF -> 0xFF, not `vqmovun`'s clamp) for the blimit flag, and
   `blimit16` is a `_mm_set1_epi16` (u16-lane) broadcast, not u8.
   `lpf_all_tiers_match_c` + `lpf_kernels_match_c_*` (FFI vs C, full
   level/sharpness space) pass; ident=Y.
   A/B (cumulative with the two arms above): **1.035x / 1.052x** at
   256/512 p4, tight bands. LANDED.
4. **`aom_hadamard_16x16`/`32x32` NEON arms** — 596 samples at 512p4,
   the largest remaining kernel gap (aarch64 fell to a `cfg(not(x86))`
   scalar with no dispatch at all). Composes the existing NEON 8x8 +
   the AVX2-semantics cross-combine: wrapping `vaddq_s16`/`vsubq_s16` +
   `vshrq_n_s16` (the `_mm256_srai_epi16` `>> 1`), and for 32x32 the
   i32 `>> 2` + `vqmovn_s32` (`packs_epi32`) + wrapping-i16 tail.
   `hadamard_{16,32}x32_matches_c` FFI tests pass (8-bit + bd10 wrap
   ranges). ident=Y. LANDED.
5. **`predict_smooth{,_v,_h}` NEON arms** — last intra_pred stub;
   smooth_v/h had no dispatch at all. Transliterates the v3 factored
   form (`(wh*top + ww*d + K) >> 9`, per-row scalars hoisted, column
   vectors widened once per block) to i32x4 `vmlaq` + `vshrq` +
   `vqmovn`/`vqmovun` narrows; w%8==0 && w<=64 vectorized, w=4 stays
   scalar. All-sizes dispatch tests (w,h in 4..64) byte-exact vs scalar;
   ident=Y. A/B (vs post-hadamard base): **1.009x / 1.011x** at
   256/512 p4 — small but consistent, and it closes the stub. LANDED.

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

* **INTRA_PRED z2 small blocks — LANDED**: faithful ports of C's
  `dr_prediction_z2_4xH_neon` / `dr_prediction_z2_8xH_neon` (one
  vmlaq/vrshrn/vbsl row pass, vqtbl3 gather over a CONTIGUOUS 48-byte
  `left[-2..45]` table — C's own 4xH table wraps past index 17 and is
  only sound while base_y <= 14, so the port could not clone it; the
  x86 oracle computes the scalar formula). Rows whose left reach
  exceeds the table (`(y_hi >> frac_bits_y) > 44`) fall back to the
  scalar core. Dispatch-vs-core sweep over w{4,8,16} x h{4..32} x all
  upsample flag combos x all legal (dx,dy) pairs: exact; 827 dsp
  tests pass incl. `dr_prediction_all_tiers_match_c` FFI.
  A/B 7r vs pre-change HEAD (a23fd5da): **1.034x at 256p4, 1.025x at
  512p4**, byte-identical. (ab_z2small_neon_2026-09-17.tsv)

* **INTRA_PRED z1/z3 small blocks — LANDED**: shared
  `dr_small_row_neon` (`u8x8` per row/column, `vld1q`+`vuzp`
  deinterleave for the upsampled pair) serving z1 4xH/8xH directly and
  z3 `bh in {4,8}` via vzip transposed stores. SCALAR-EXACT mask count
  (`ceil`), NOT C's `>> upsample` floor — C's scalar core and BOTH C
  SIMD ports (NEON + AVX2) genuinely disagree on the last boundary
  lane of an upsampled block when `max_base - base` is odd, so the
  port keeps every tier on the scalar formula. Dispatch-vs-core sweeps
  over all sizes/flags/12 legal derivatives: exact; 829 dsp tests
  pass. A/B 7r vs pre-change HEAD: **1.013x at 256p4, 1.008x at
  512p4**, byte-identical. (ab_z1z3small_neon_2026-09-17.tsv)
  NOTE: this also surfaced that the scalar z1/z3 cores can index past
  EDGE_BUF_LEN (160) on upsampled `bw + bh >= 73` blocks — unreachable
  in production (upsample requires `w + h <= 16`), same over-read C
  does into stack slack; the sweeps use 256-byte buffers to cover it.

* **z2 wide-short coverage — LANDED (coverage, null perf here)**:
  relaxed the flat arm's gate from `bh >= 16` to any `bh <= 64` — pass 1
  chunks over columns so 16x8/32x8/64x4 blocks vectorize their bulk;
  pass 2's scalar tail takes the short left region. Matches C, which
  routes every `bw >= 16` to `dr_prediction_z2_WxH_neon`. A/B 7r:
  byte-identical, ratio straddles 1.0 (wide-short z2 blocks are rare
  at p4 on this fixture). Kept as a coverage/parity alignment, not a
  perf claim. (ab_z2wide_neon_2026-09-17.tsv)

* **Gap re-measure (campaign, port vs C oracle)**: 512p4 **1.70x**
  (75.0 vs 44.1 ms), 256p4 **1.67x** — down from 1.83x at the start of
  the NEON sweep. Every DSP symbol still carrying samples in the fresh
  512p4 profile now HAS a NEON arm; the remaining gap is the
  driver-level buckets below. (perf_gap_z13final_2026-09-17.tsv)

## Where the remaining gap actually lives




* **Funnel orchestration** (MD_DRIVER, +9.3ms): `tx_unit_inner` +
  `inject_candidates` (307) + `evaluate_leaf` (182) + nic/mds3 closures
  (~380) + `give_pooled` (164) + Cand/PoolVec drop glue (~45). Per-
  candidate fixed costs — buffer round-trips, slice/pool bookkeeping —
  vs C's shared `cand_bf` arena. Largest single bucket; needs a
  profiling-guided flatten of the eval loop, not a kernel.
* **INTRA_PRED** (+6.5ms): `dr_predictor_edged` is 1.8x C's combined
  directional kernels. z1/z2/z3 small blocks now covered (above);
  what remains is the edged-driver itself (per-block setup + call
  volume), not uncovered kernel shapes.
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

## bd10 cell (PERF_BD=10 harness arm, `diag` content)

- `predict_dc_hbd` NEON arm (`fa44a90a`): u16 edge sums via
  `vaddlvq_u16`/`vaddlv_u16` widening reductions (exact u32 for any
  input — no ≤12-bit tree-sum assumption like C). A/B bd10 p10:
  **1.052x @256², 1.021x @512²**, byte-identical.
- `dr_z1_edged_hbd` + `dr_z3_edged_hbd` flat NEON arms: u32 widening
  `a1*2s + a0*(64-2s)` + `vrshrn::<6>` + `vmin(bd_max)` (exact
  `clip_pixel_highbd` for any input). Gates `bw>=8`/`bh>=8`
  non-upsampled — C's shape coverage. A/B bd10 p2 diag:
  **1.008x @256², 1.005x @512²**, p10 wash (arms cold there),
  byte-identical (`ab_dr_hbd_neon_2026-09-17.tsv`).
- Deferred: `dr_z2_edged_hbd` NEON (needs per-lane u16 gathers — C uses
  vqtbl tables; port's contiguous-table approach needs 96 bytes of u16
  lanes = 6 regs — tractable but larger), `predict_{paeth,smooth*}_hbd`
  (cold in both bd10 profiles: ≤36 samples).

## quantize raster NEON width (8-bit)

- `quantize_{b,fp}_raster_impl_neon` widened 4 -> 8 lanes/iter (two
  independent int32x4 halves; the per-coefficient chain is ~12
  serially-dependent vector ops, so the halves overlap). Same op set
  per lane = byte-identical by construction. A/B vs `9dc19b56`:
  **1.008x @512p4, 1.006x @512p2** (both ratio bands below 1.0),
  wash @256p4 and @512p10 (quantize is cold at p10), byte-identical
  everywhere (`ab_quant8_2026-09-17*.tsv`).

## CDEF tap-group multiply — measured null, reverted

- `cdef_filter_cols{8,4}_neon` regrouped to C's shape: one multiply per
  same-coefficient tap group (`sum += cof*(c0+c1)`, 12->4 muls/row,
  broadcasts hoisted; exact mod 2^16 distributivity). All cdef tests +
  FFI parity byte-exact, but interleaved A/B was a wash (256p4 ratio
  1.001 p25-p75 0.996-1.002; 512p4 0.999 0.998-1.007) — the row cost is
  the 12 tap loads + constrain chain, not the multiplies. Reverted;
  evidence in `ab_cdefgrp_2026-09-17*.tsv`.

- `dr_z2_edged_hbd` NEON (`dr_z2_edged_hbd_simd_neon`): two-pass affine
  decomposition — the index math is affine, not a gather: per row
  `base(c) = base0 + c*step` (contiguous/stride-2 loads, constant shift),
  per column `base2(r) = base2_0 + r*step_y` (same loads vertically,
  scatter-stored tail). No vqtbl machinery needed — simpler than C's
  `highbd_dr_prediction_z2_*_neon` shape. Kernel self-time
  `dr_predictor_edged_hbd` 175 -> 117 samples (-33%) at bd10 256^2 p2
  diag; encode-level A/B is a wash (0.997-1.000, bands straddle 1.0 —
  the bucket is ~0.4% of encode). Retained as coverage alignment: all
  hbd directional predictors (z1/z2/z3) now have NEON arms matching C's
  dispatch coverage. Byte-identical everywhere; all-tiers sweep covers
  sizes x angles x upsample x bd (`ab_z2hbd_2026-09-17.tsv`).

## Real-content check (2026-09-18) — screen + photo vs synthetic gradient

All prior numbers in this doc were on synthetic `gradient`/`diag` fixtures.
First real-content measurement, `perf_encode raw:` (I420 from
`identity_run crop:` — gb82-sc terminal.png and clic2025 photo at 512²,
byte-identical both):

| content | p4 ratio (port/C) | p10 ratio |
|---|---|---|
| screen (terminal.png) | **1.47x** | 1.80x |
| photo (clic2025) | **1.72x** | 1.76x |
| gradient (same harness) | 1.70x | ~1.8x |

The gap HOLDS on real content — screen content is actually *better*
(1.47x vs 1.70x) because the port's palette/IntraBC paths are relatively
cheaper than C's. Artifacts: `perf_gap_real_{screen,photo}*.tsv`.

Real screen content also surfaces a different hot mix the gradient never
exercised (512² p4, self-samples /11394): `inject_candidates` 755,
`palette::calc_indices_dim1` 381, `intrabc_hash::generate_block_hash_value`
353 (CRC-32C), `me_sad::block_sad` 302, `palette::k_means_dim1` 216,
`intrabc::diamond_search_sad` 93. Palette clustering and the CRC hash
are encoder-side scalar loops — the remaining uncovered kernel surface.

## block_sad_x4 NEON arm (measured null on this fixture, kept for coverage)

- `block_sad_x4` gained a `[v3, neon, scalar]` arm mirroring C's
  `sadwxhx4d_neon`: shared source load across 4 refs, `vabdq_u8` +
  `vpadalq_u8` into u16 lanes, C's `2048/w` fold cadence (wide path);
  narrow path folds every 128 rows so arbitrary `h` is safe (C relies on
  `h <= 32`). SAD is an exact integer sum — order-independent.
  All-tiers sweep (incl. odd widths 12x6, 20x3, 5x7, 1x1, 31x9) passes.
- A/B on real screen content @512p4: 11 rounds 0.999x (band 0.997-1.007),
  byte-identical — the x4 path is ~0.8% of encode at p4 (mesh-refinement
  only fires at `step==1`). Kept as coverage alignment: aarch64 no longer
  drops to scalar where C ships `svt_aom_sad*x4d_neon`.
  Evidence: `ab_sadx4_screen_2026-09-18*.tsv`.

## hbd predictor/CfL NEON arms (byte-identical; small p2 gain)

- `predict_paeth_hbd`, `predict_smooth{,_v,_h}_hbd`,
  `predict_filter_intra_hbd`, `cfl_luma_subsampling_420_hbd`,
  `cfl_predict_hbd` all gained `[neon, scalar]` dispatch — the last
  undispatched hbd intra-predictor surface where C ships NEON twins
  (`highbd_intrapred_neon.c`, `cfl_neon.c`).
- Paeth arm uses i32 lanes (u16 inputs can exceed i16 range) and preserves
  C's LEFT-first tie-break — deliberately NOT the u8 arm's top-first order.
  Smooth arms use the same algebraic factorizations as the u8 arms
  (per-row constant collapse, `vmlaq` + exact shift, `vmovn_u32` truncates
  like the scalar `as u16`). filter_intra is an i32x4 matvec across the 8
  outputs (transposed taps hoisted per call; serial p5/p6 reads of
  already-written cells stay scalar). CfL predict reuses the lbd arm's
  `vqrdmulhq_s16` rounding then widens to i32 for the +pred/clamp (pred is
  u16, sum spans [-8192, 73727] — i16 is NOT enough, unlike lbd).
- All-tiers sweep `hbd_predictors_all_tiers_match_core`: every arm vs the
  scalar core across sizes (incl. non-mult-4 tails), strides, bd {8,10,12},
  filter-intra modes 0-4, alphas -16..16. PermutationReport consumed.
  `c_parity_intra_pred_hbd` FFI suite (19 sizes x bd{10,12} vs real C)
  green — covers paeth + smooth arms end-to-end vs C.
- A/B on real screen content, bd10: 512² p10 0.992x (band 0.995-1.015,
  noise — predictors are thin at p10); 256² p2 **1.009x** (band
  0.982-0.992, all rounds faster), byte-identical both. CfL +
  filter_intra hbd have no direct FFI oracle (encoder-internal); the bd10
  byte-identity is their end-to-end coverage.
  Evidence: `ab_hbdpred_screen_bd10{,_p2}.tsv`.

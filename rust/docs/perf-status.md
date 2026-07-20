# Performance status — G4 baseline (port vs C wall clock)

Measured baseline for **G4** (docs/ACCEPTANCE-CRITERIA.md → "Performance"): the
port's per-frame still-image encode wall time against the real C reference,
on the byte-identical envelope. This is the honest starting point of the
ratchet — the port has **not** been performance-tuned yet (G4 is deliberately
the last gate: "a fast encoder that emits different bytes is worthless").

**Verdict: the port is currently 1.5×–11× C on the tested cells; nothing is at
≤1.2× yet.** The gap is almost entirely **per-pixel compute** (slope), not fixed
overhead — at the fast presets the port's fixed per-frame cost is already *below*
C's. See the numbers below.

## The honest caveat

G4 per the criteria is measured *"once parity holds."* Parity holds on the
tested envelope — bd8 4:2:0, still-picture CQP, byte-exact presets — but not yet
across the whole matrix (10-bit, all presets, real content at speed ≥ 1, …). So
this is a baseline on the **byte-identical subset**, not the final gate. Every
cell here is verified byte-identical (port `.obu` == C `.obu`) before its ratio
is trusted; a comparison of two encoders doing *different* work would be
meaningless. All 15 cells below are byte-identical.

## How to run

```
tools/perf_gate.sh [date-suffix]        # default suffix: today's date
```

Env-overridable grid: `PERF_SIZES PERF_PRESETS PERF_CONTENT PERF_QP PERF_ROUNDS
PERF_WARMUP`. It builds the port release (no `target-cpu=native`), builds/links
the C reference harness, runs the interleaved paired sweep, verifies byte
identity per cell, and writes `benchmarks/perf_<suffix>.{tsv,raw.tsv,meta}`.
Intentionally **not** in CI — shared runners are too noisy for a wall-time gate
(rust-gates.yml says so); it runs on fixed hardware and the result is committed.

## Method (the binding rules, and how they're met)

- **Interleaved paired statistics.** Each round runs port and C back-to-back in
  *randomized* order (coin flip per round), so thermal/turbo drift cancels
  within the pair. The headline ratio per cell is the median of the per-round
  paired ratios; the spread is its [p25, p75]. Not back-to-back isolated blocks.
- **No `-C target-cpu=native`.** The port release is built with runtime SIMD
  dispatch (what ships). The C lib is the same Release build; it selects up to
  `avx512icl` at runtime on this host.
- **`total = intercept + slope · pixels`, fit across tiny → large.** So fixed
  per-call cost never hides inside one "ms/MP" number. Both coefficients are
  reported, per preset, for port and C. Nothing is extrapolated — every size is
  measured directly.
- **Setup excluded on both sides, symmetrically.** Only the per-frame encode is
  timed: the port times `encode_frame_420` on a fresh pipeline (`EncodePipeline::
  new` excluded); the C harness times `send_picture` + drain (`svt_av1_enc_init`
  excluded). The two harnesses are `svtav1/examples/perf_encode.rs` and
  `tools/perf_c_encode/perf_c_encode.c`; they consume the identical `.yuv`.

## Results — 2026-07-20, commit `d4c75a762`, host `dev-32gb` (16 cores)

Content `gradient`, qp 40, 20 interleaved paired rounds/cell, warmup 1. All
cells byte-identical.

### Per-cell ratio (port / C, median of paired rounds)

| size | preset | port ms | C ms | ratio | [p25, p75] |
|-----:|:------:|--------:|-----:|------:|:-----------|
| 64   | 6  |   4.635 |  1.177 |  3.95 | [3.86, 4.07] |
| 64   | 10 |   0.963 |  0.616 |  1.58 | [1.51, 1.61] |
| 64   | 13 |   0.969 |  0.636 |  1.55 | [1.33, 1.65] |
| 128  | 6  |  16.956 |  2.264 |  7.46 | [6.87, 8.12] |
| 128  | 10 |   3.081 |  0.873 |  3.55 | [3.45, 3.62] |
| 128  | 13 |   3.087 |  0.872 |  3.55 | [3.49, 3.63] |
| 256  | 6  |  79.400 |  9.026 |  8.76 | [8.61, 8.89] |
| 256  | 10 |  11.325 |  1.769 |  6.40 | [6.11, 6.77] |
| 256  | 13 |  11.421 |  1.768 |  6.38 | [6.16, 6.79] |
| 512  | 6  | 266.275 | 26.859 |  9.92 | [9.51, 10.17] |
| 512  | 10 |  47.408 | 13.814 |  3.52 | [3.27, 3.89] |
| 512  | 13 |  47.051 | 14.145 |  3.35 | [3.06, 3.51] |
| 1024 | 6  | 917.730 | 82.326 | 11.25 | [10.88, 11.59] |
| 1024 | 10 | 177.751 | 19.563 |  9.22 | [8.86, 9.26] |
| 1024 | 13 | 177.261 | 19.531 |  9.05 | [8.66, 9.18] |

Best case ~1.55× (tiny + fast preset, where fixed cost dominates and the port's
is small); worst ~11.25× (1024², preset 6, where per-pixel work dominates).

### Intercept + slope fit (`ms = intercept + slope · pixels`; slope as ms/megapixel)

| preset | port intercept | port slope | C intercept | C slope | slope ratio | intercept ratio | port R² | C R² |
|:------:|---------------:|-----------:|------------:|--------:|:-----------:|:---------------:|:-------:|:----:|
| 6  | 14.767 ms | 867.14 ms/MP | 2.909 ms | 76.68 ms/MP | **11.31×** | 5.08× | 0.998 | 0.995 |
| 10 |  0.841 ms | 169.20 ms/MP | 2.318 ms | 17.93 ms/MP | **9.44×**  | 0.36× | 1.000 | 0.813 |
| 13 |  0.834 ms | 168.69 ms/MP | 2.394 ms | 17.89 ms/MP | **9.43×**  | 0.35× | 1.000 | 0.801 |

Reading the fit:

- **The gap is the slope, not the intercept.** The port does ~9.4× (fast
  presets) to ~11.3× (preset 6) the per-pixel work of C. At presets 10/13 the
  port's *intercept* — fixed per-frame cost — is actually **below** C's (0.36×);
  the port is not losing on startup/dispatch, it is losing on the hot loops.
- **The port scales cleanly with pixels** (R² 0.998–1.000): its cost is a clean
  `a + b·pixels`, which makes the slope a trustworthy per-pixel figure and means
  a per-pixel win propagates to every size. The C reference at presets 10/13 is
  less pixel-linear (R² ≈ 0.80 — the 512² point is high; its encode time is small
  enough, 1–20 ms, that content statistics and the threaded pipeline shape it as
  much as pixel count does), so C's fitted fast-preset slope/intercept carry more
  uncertainty than the port's. Preset 6 fits both sides well (R² > 0.99); the
  per-cell ratios are the firmer view at the fast presets.
- **Preset 6's port intercept (14.8 ms) is a fit artifact of mild
  super-linearity**, not a real 15 ms floor (64² p6 is only 4.6 ms). Read p6 as
  slope-dominated with a large per-pixel constant.

## Top hotspots (where the future work is)

1. **SIMD on the hot per-pixel kernels — the dominant lever.** The ~9–11× slope
   gap is the port's mostly-scalar mode-decision / transform / SAD / quant paths
   against C's `avx512icl` runtime dispatch. This is per-pixel, so it is exactly
   what the slope measures, and a win here moves every size and preset. A callgrind
   self-instruction ranking of a 256² preset-10 frame (restoration off) puts the
   per-pixel cost concretely — **CDEF `cdef_filter_block` 27.8 %** (now SIMD'd, see
   "Landed"), inverse/forward transforms (`inv_txfm2d_c_exact_bd` + `idct*`/`fdct*`/
   `fadst*`) ~25 %, `__memset` (per-frame zeroing) ~6 %, entropy coeff contexts
   (`get_nz_map_contexts`/`nz_map_ctx`/`txb_init_levels`) ~8 %, quant ~3 %. The named
   distortion kernels (`sad`/`sse`/`variance`/`satd`) are only ~2–3 % here — small
   relative to CDEF+transforms — so the remaining fast-preset levers, in order, are
   the **transform butterflies** and the **per-frame allocation/`memset`** (see (3)).
2. **Loop-restoration Wiener stats (`restoration::compute_stats`) — was the single
   biggest function at preset ≤ 6; now SIMD'd.** Callgrind (256² preset-6, debuginfo)
   originally put it at **~46 %** of frame instructions (316.9M direct + inlined
   iterator/bounds machinery) — the inherent O(win²·win²) Wiener M/H accumulation,
   called per Y/U/V plane. Restoration runs only at presets 0–6 (off at ≥ 7), which
   is most of why preset 6's slope is ~5× that of presets 10/13. An **AVX2 port has
   now landed** (see "Landed" below): the M/H outer-product accumulation dropped
   from ~431M → ~165M frame instructions (2.6×), taking the whole 256² p6 encode
   from 938M → 684M (−27 %). The remaining `compute_stats` cost is the (still-scalar,
   cache-unfriendly column-major) window gather and the inherent i32-lane H body;
   a further win needs either a SIMD gather or C's incremental delta-decomposition
   algorithm (`svt_av1_compute_stats_avx2`, ~2800 lines — a much larger port).

   The earlier "per-SB `MdRates`/`CoeffCostTables` rebuild" suspicion was
   investigated and is **not** a material lever: for presets ≥ 7 (update_cdf_level
   0) those tables are already built once per tile, and for presets 0–6
   (update_cdf_level 2) they genuinely evolve per SB from the `ec_ctx_array`
   neighbour chain (`chain_base` in pipeline.rs), so a hoist would change bytes.
   The rebuild is a negligible fraction of frame time either way.
3. **Per-frame allocation discipline** (was the #1 remaining item at p6; the bulk is
   now landed). The port allocates+zeros its working set inside `encode_frame_420`;
   C pre-allocates in `init`. After the `compute_stats` SIMD, `__memset_avx2`
   (per-frame zeroing) was the **largest single item at 256² p6 — ~19 %** (132.9M),
   pre-existing per-txb buffer zeroing, not the LR scratch. The **per-txb level-map
   + tx-scratch zeroing reduction** (see "Landed" below) cut it to ~1.6 % (#9),
   taking 256² p6 frame instructions −15.8 % — byte-inert (reduced zeroing extent +
   32-cap scratch sizing + dead-zero → uninit alloc). What remains is the ~9.2M
   (1.6 %) per-txb `tx_unit` i32 calloc blob (`coeffs`/`qcoeff`/`dqcoeff`/`dq_full`/
   `inv`/`recon`): these are `&mut`-filled or have load-bearing zeros (positions
   past eob, the >32 high-freq tail), so they cannot be turned into uninit `collect`
   like `residual`/`packed` were — eliminating them needs a persistent/thread-local
   `TxScratch` reused across calls, with per-buffer write-coverage verified before
   any zero is skipped (the riskiest byte-identity change; deferred). After that the
   next SIMD-able integer kernel is the entropy coeff context sum
   `get_nz_map_contexts`/`nz_map_ctx` (~6 % combined).

Approach order per the criteria: algorithmic parity (done on this envelope),
then allocation discipline, then SIMD. On these numbers, SIMD on the hot loops
is the biggest single lever.

## Landed byte-inert optimizations

- **Per-txb level-map + tx-scratch zeroing reduction** (crates/svtav1-entropy/src/
  coeff_c.rs, crates/svtav1-encoder/src/{leaf_funnel,quant,pd0}.rs). `__memset_avx2`
  was the #1 remaining preset-6 item (~19 % of 256² p6 frame instructions, item (3)
  above) — per-frame/per-txb buffer zeroing the port pays that C avoids via
  persistent, once-zeroed buffers. Three byte-inert reductions on the per-txb
  coeff-coding hot path: (1) `txb_init_levels` zeros only the padded extent the
  `(width,height)` txb uses (the context readers reach at most 4 rows below the
  bottom-right coeff — `TX_CLASS_VERT` `nz_mag` reads `base+4*stride` — so `used`
  bounds that, capped at len; a 4×4 zeros ~112 B not 4640, matching C's
  md_levels_buf whose pad is zeroed once and only the body re-fills). (2) A new
  `LEVELS_SCRATCH_LEN` const (~1456 B) sizes the per-call level scratch to the
  32×32 coeff-coding cap (`adjusted_tx_size` folds 64-dim tx to a 32-dim map)
  instead of the 64-shaped `TX_PAD_2D`; the two heap `vec![0u8; TX_PAD_2D]` level
  buffers become stack arrays of this length (no per-txb calloc), the two stack
  ones shrink 3.2×. (3) `tx_unit`/`tx_unit_hbd` build `residual` and the >32 fold
  `packed` with `Vec::with_capacity`+push/extend instead of `vec![0; n]` + full
  overwrite (dead zero → uninit alloc). Byte-identical by construction: every read
  and write stays in the zeroed/filled prefix; the dead-zero buffers are fully
  overwritten. Proven two ways: a new `c_parity.rs::
  coeff_c_txb_init_levels_partial_zero_no_stale_reads` pre-fills the scratch with
  0xFF garbage and asserts `get_nz_map_contexts`/`br_ctx` still bit-match real C
  across all 19 tx sizes × {2D,VERT,HORIZ} (0xFF clips to context 3, so any
  over-read diverges), plus all 9 runnable identity gates + `cargo test
  --workspace` (864 tests). Measured (callgrind, deterministic): 256² p6 frame
  instructions **685.9M → 577.3M (−15.8 %)**; `__memset` from the ~19 % #1 item to
  ~1.6 % (#9), and the per-txb calloc blob **44.9M → 9.2M**. Cross-size instr:
  128² −1.4 %, 512² −9.8 % (the win tracks the memset fraction, largest at 256²).
  Wall (40-round interleaved paired, no `target-cpu=native`): 256² p6 −3.1 %, 512²
  p6 −1.3 % — smaller than the instruction delta because `__memset` is
  bandwidth-bound and p6 wall time is dominated by the untouched restoration
  `compute_stats`. `#![forbid(unsafe_code)]` intact. Commits `57f8dc6e8` (perf) +
  `713f7b7f9` (regression test).

- **`compute_stats` / `compute_stats_hbd` accumulation reshape**
  (crates/svtav1-dsp/src/restoration.rs). Re-slice M/H to their exact working
  lengths and walk the upper-triangular `H[k][l] += y[k]·y[l]` (plus
  `M[k] += y[k]·x`) as bounds-check-free `chunks_exact_mut`/`zip` pairs. Identical
  products in the same per-element accumulation order → M/H are bit-for-bit
  unchanged (guarded by the `compute_stats_matches_c` /
  `highbd_compute_stats_matches_c` C-parity tests and all 11 identity gates).
  Measured (benchmarks/perf_cs_{before,after}.*, same host/grid, 20 paired
  rounds): `compute_stats` instructions −22 % (139.2M → 108.1M at 128² preset 6),
  total frame instructions −10.4 %; wall-clock port slope at preset 6
  990.8 → 902.1 ms/MP (−8.9 %), 256² preset 6 −6.5 %, 512² preset 6 −8.3 %.
  Presets 10/13 unchanged (restoration off there).

- **CDEF filter SIMD (AVX2) — `cdef_filter_block` (dst8) + `cdef_filter_block_hbd`
  (dst16)** (crates/svtav1-dsp/src/{cdef,hbd}.rs). Callgrind identified
  `cdef_filter_block` as the single largest per-pixel kernel on the fast-preset hot
  path — **27.8 % of frame instructions at 256² preset 10** (5.3 % at preset 6),
  and it was fully scalar. Each output pixel is an independent 12-tap integer sum
  with no cross-pixel reduction, so the 8 columns of a row map to 8 AVX2 lanes
  (archmage `Desktop64`, `incant!([v3, neon, scalar])`); the scalar core is retained
  as the reference and the cols==4 (4:2:0 chroma) fallback. Byte-exact by
  construction — the per-tap products are summed in i32 and the running sum truncated
  to i16 once at the end, which equals the scalar's per-tap `wrapping_add::<i16>` by
  associativity of two's-complement add mod 2^16; the u16 input is **sign**-extended
  (the C kernel reads it into `int16_t`, cdef.c:205), matching C for every input, not
  just ≤ 0x7f7f pixels. Pinned against the REAL exported `svt_cdef_filter_block_c`
  in tests/c_parity_cdef.rs — every signalable (strength, damping, dir, bsize,
  border) combo + 2000 torture rounds, plus a new all-dispatch-tier lock
  (`filter_block_dispatch_all_tiers_match_c`) and a sign-extension lock
  (`filter_block_sign_straddle_matches_c`, verified to fail on zero-extension).
  Measured (perf_gate.sh, same host, 15 paired rounds, no `target-cpu=native`). The
  cleanest aggregate is the fitted **port per-pixel slope** (across 256²/512²/1024²,
  so per-cell noise averages out): **p10 166.1 → 131.9 ms/MP (−20.6 %), p13 165.0 →
  138.6 (−16.0 %), p6 790.9 → 726.7 (−8.1 %)** — the port/C slope-ratio (the G4 metric)
  drops **p10 12.0× → 8.4×, p13 11.6× → 8.4×, p6 11.3× → 9.9×**. Per-cell wall time
  agrees at the slope-dominated sizes: 512² p10 47.7 → 38.2 ms, 512² p13 49.1 → 38.0 ms,
  1024² p10 178.3 → 144.7 ms, 1024² p13 176.8 → 148.8 ms (256² is noise-dominated at
  ~15 ms, so read the slope, not that row). The dst16 arm
  carries the same win to the bd10/bd12 search (not in the bd8 perf grid; verified by
  the bd10 gates). All 11 byte-identity gates + `cargo test --workspace` green;
  `#![forbid(unsafe_code)]` intact. Data: benchmarks/perf_{before,after}_cdef.tsv.

- **`txb_init_levels` SIMD (AVX2) — coeff-level packing** (`crates/svtav1-entropy/src/
  coeff_simd.rs`, commit `2e71f1f9d`). The per-txb coeff-magnitude → level-buffer pack
  that feeds the nz-map context sum, ~8% of frame instructions. archmage
  `incant!([v3, neon, scalar])`, additive alongside the scalar `coeff_c` path. Integer
  per-element clamp/pack → bit-identical. Proven byte-exact two ways:
  `txb_init_levels_simd_matches_c` (SIMD == exported real-C `av1_txb_init_levels_c`,
  all tx sizes) + all 11 gates unchanged. `#![forbid(unsafe_code)]` intact.

- **Wiener LR `compute_stats` SIMD (AVX2) — the M/H accumulation** (`crates/svtav1-dsp/
  src/restoration.rs`, commits `4107be038` + `429cf91c6`). Callgrind put
  `restoration::compute_stats` at **~46 % of frame instructions at 256² preset 6** —
  the single dominant p6 hotspot, and fully scalar (only a prior bounds-check reshape
  had landed). It is the O(win²·win²) per-source-pixel Wiener outer product
  `M[k] += y[k]·x`, `H[k][l] += y[k]·y[l]` (upper triangle). archmage
  `incant!([v3, neon, scalar])`, additive alongside the scalar reference (also the
  aarch64 neon fallback). The AVX2 path accumulates each restoration region **row**'s
  products in i32 SIMD lanes (`_mm256_mullo_epi32`, 8 columns at a time) then flushes
  the row's partial sums into the i64 output. **Byte-exact by construction:** every
  product is two values in [−255,255] (pixel minus region avg) so it fits i32 exactly
  (`mullo_epi32`'s low 32 bits ARE the product), and a region row is < 512 px wide so
  each i32 cell sums ≤ ~512 products (≤ 3.4e7 ≪ i32::MAX) — no i32 overflow; the final
  i64 is the same set of products, merely regrouped Σrows(Σpixels), identical by
  associativity of integer addition. Proven two ways in `tests/c_parity_wiener.rs`:
  `compute_stats_matches_c` (host tier == real `svt_av1_compute_stats_c`) +
  `compute_stats_all_tiers_match_c` (forces EVERY tier via `for_each_token_permutation`,
  each == real C AND == tier 0, so SIMD == real-C AND == scalar) across both window
  sizes, all content classes, and edge regions (widths <8 / 8 / off-by-one / 1-row /
  tall-multi-flush). Only the bd8 u8 `compute_stats` is touched; the bd10/bd12
  `compute_stats_hbd` path is unchanged. Instruction-count: `compute_stats` ~431M →
  ~165M (2.6×); whole 256² p6 encode 938M → 684M (−27 %). Wall (interleaved paired vs
  the pre-SIMD parent binary, no `target-cpu=native`): **256² p6 −24.5 %, 512² p6
  −26.6 %, 1024² p6 −28.9 %**. All 10 runnable gates byte-identical + workspace green;
  `#![forbid(unsafe_code)]` intact. Data: benchmarks/perf_p6_{4size,computestats_simd}.tsv.

## Campaign summary (2026-07-20)

SIX byte-exact perf wins now landed, profile-ranked: restoration
reshape (−8.9% worst preset), CDEF filter SIMD (the 27.8% hotspot), `txb_init_levels`
SIMD (~8%), the **square DCT transforms SIMD** (fdct/idct 8/16/32/64, commit
`42989abee` — done in an isolated `git worktree` to avoid the shared-checkout
hazard), the **ADST + non-square DCT SIMD** (fifth, below), and the **Wiener LR
`compute_stats` SIMD** (sixth — the dominant p6 hotspot, below). Each is
byte-identical (a `c_parity_*` differential vs real C + the gates) with no `unsafe`.

**Measured G4 progress (port/C slope-ratio, the gate metric):**

| preset | baseline | after CDEF | **after transforms** |
|---|---|---|---|
| p10 | ~12× | ~8.4× | **2.12×** |
| p13 | ~11.6× | ~8.4× | **2.97×** |
| p6  | ~11.3× | ~9.9× | **7.53×** |

The transforms run at ALL presets (CDEF only ≤M6), so they were the dominant fast-
preset cost — **the square DCT SIMD brought p10 to ~2.1× C** (from ~8.4×), close to
the ≤1.2× target, with p13's *intercept*-ratio already ~1.1× (within target on fixed
overhead; the slope is the remaining gap).

A **fifth** win then landed (commit `a29dc02af`, worktree-isolated): **ADST
(`fadst`/`iadst` 8,16) + non-square rectangular DCT** SIMD — byte-exact (the
`c_parity_txfm` differential grew to 14 cases incl. the rectangular `rect_type`
scaling; all 11 gates + workspace green). Measured (clean 15-round self-consistent
before/after): **p10 port-slope −10.5%, p13 −10.0%, p6 −1.3%** — a real ~10% win at
the fast presets (the rectangular sizes are common there), NOT negligible. p6 barely
moves because CDEF+LR search dominate the slowest preset.

**CLEAN post-SIMD baseline (20 rounds × 4 sizes {64,128,256,512} × p{6,10,13}, paired):**

| preset | slope-ratio (port/C) | shape |
|---|---|---|
| p10 | **2.00×** | at 64² the port is **0.79× — FASTER than C** (lower fixed cost); the ~2× is per-pixel slope, dominating at ≥256² |
| p13 | **1.92×** | same |
| p6  | ~~7.65×~~ → **5.18×** | after the `compute_stats` SIMD (below); still carries the CDEF+LR *search*, the still-scalar window gather, and the non-DCT transforms |

After five byte-exact SIMD wins the fast presets are at **~2× C on the slope** (and
faster than C on small frames). To reach ≤1.2× at p10/p13 needs roughly halving the
remaining per-pixel cost — spread across quant + the entropy coeff-coding path +
SAD/SSE, each a smaller slice, so it's an incremental grind, not one big lever.

A **sixth** win then landed (commits `4107be038` + `429cf91c6`): the **Wiener LR
`compute_stats` M/H accumulation SIMD** (see "Landed" above). It was the dominant p6
hotspot (~46 % of 256² p6 instructions) and fully scalar; the AVX2 port cut it 2.6×
(431M → 165M instructions) and took p6 **7.65× → 5.18×** on the same 4-size grid
(measured 2026-07-20, commit `429cf91c6`; interleaved before/after vs the parent
binary: 256²/512²/1024² p6 −24.5 %/−26.6 %/−28.9 %). p6 now needs the CDEF/LR
*search* structure, the still-scalar `compute_stats` window gather, and the non-DCT
transforms. The pre-existing per-frame `__memset` that was the largest single p6
item (~19 %) has since been cut to ~1.6 % by the **per-txb level-map + tx-scratch
zeroing reduction** (seventh landed win, 256² p6 −15.8 % frame instructions; see
"Landed"); `compute_stats` is again the dominant p6 kernel.

An **eighth** win then landed (commits `2027408a2` FLIPADST + IDENTITY, `3e4a9443c`
4-dim), **completing the transform SIMD coverage**: every transform family now has a
byte-exact AVX2 path — the FLIPADST combos (the block edge flip reuses the existing
`fadst`/`iadst` kernels + a `reverse8` lane mirror), IDENTITY (IDTX + the mixed
V_/H_ types, a per-size NewSqrt2 scale), and all five 4-dim sizes (4x4/4x8/8x4/4x16/
16x4, incl. the 4-point sinpi `fadst4`/`iadst4`) across all 16 tx types. The
`c_parity_txfm` differential grew by six cases (fwd/inv ext + fwd/inv 4-dim, bd8 +
bd10, each == real C AND == scalar under every archmage tier); all 11 gates +
workspace byte-identical. Measured component before/after (release, forward, SIMD
dispatch vs the scalar core): FLIPADST **8–12×**, IDTX **4.4–5.0×**, V_/H_ **6.4×**,
4-dim **2.9× (4x4) → 6.5× (16x4)** — the SIMD path also skips the scalar core's
per-call `Vec` allocations. The whole-frame gradient p6 sweep barely moves (smooth
content codes mostly DCT, already SIMD), but the non-DCT transforms are no longer a
scalar residual for the content (real photo/screen) that uses them.

**Not at ≤1.2× yet.** Remaining fast-preset levers, now that the transforms are
SIMD'd: **quant** (`quantize_b`/`quantize_fp`), the **entropy coeff-coding** path
(`get_nz_map_contexts` context sum + the writer), and SAD/SSE — each a smaller slice.
p6 additionally carries the CDEF+LR search. All are byte-exact-portable via the same
archmage pattern in an isolated worktree.

**Process note (learned the hard way 2026-07-20):** do NOT run `perf_gate.sh`'s
before/after (which `git stash`/pops the working tree) in the SAME checkout where a
verification sweep is concurrently reading the tree — it pulls the change out from
under the sweep and corrupts the result (recovered via the snapshot stash, no loss).
Measure perf on the COMMITTED change post-landing, or in an isolated `git worktree`.

## Reproducibility / provenance

- Harness: `tools/perf_gate.sh`, `svtav1/examples/perf_encode.rs`,
  `tools/perf_c_encode/` (`.c` + `build.sh`; binary rebuilt on demand).
- Data: `benchmarks/perf_2026-07-20.tsv` (per-cell summary),
  `benchmarks/perf_2026-07-20.raw.tsv` (every paired sample),
  `benchmarks/perf_2026-07-20.meta` (provenance + fits).
- C oracle: the in-tree `libSvtAv1Enc.a` (mainline-equivalent, HDR mode off) —
  the same reference the identity campaign validates against.

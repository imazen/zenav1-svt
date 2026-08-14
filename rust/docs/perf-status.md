# Performance status — G4 baseline (port vs C wall clock)

> **CURRENT (2026-08-13, aarch64 / Apple M4 Pro — read this first).** Everything
> below the "Results — 2026-07-20" heading is the **x86-64/AVX2 history** on
> `dev-32gb`. The live numbers on the aarch64 box are:
>
> | preset | slope ratio port/C | was (08-13 R1R2) | was (08-13 mid) | was (08-11) | was (08-07) |
> |---|---|---|---|---|---|
> | p2 | **3.91x** | 3.93x | 4.14x | 4.12x | 4.11x |
> | p6 | **3.25x** | 3.27x | 3.39x | 3.52x | 3.50x |
> | p10 | **2.71x** | 2.89x | 3.06x | 3.53x | 4.85x |
> | p13 | **2.71x** | 2.89x | 3.07x | 3.51x | 4.83x |
>
> The p10/p13 step is the MDS0 variance-arm gate (`5bfbcd742`,
> `benchmarks/perf_2026-08-13-mds0var.*`): C's `fast_loop_core` runs the
> Hadamard fast distortion only when more than one candidate was injected
> (`mds0_use_hadamard_blk`, product_coding_loop.c:9473), and at preset >= 9 the
> `dc_only` gate injects exactly one — so C runs NO Hadamard there and the port
> ran it unconditionally, at 4.8-5.1 % of its frame. Attribution is the paired
> A/B (n=17, 9/9 cells 1.048-1.079x, identity control inside the noise floor,
> slow presets NULL): `benchmarks/mds0_variance_ab_2026-08-13.meta`. Read the
> A/B for the size of the change and this table for the position — the absolute
> slopes carry cross-session drift (C's own p10 slope moved 8.26 -> 8.49 ms/MP
> between these two runs, on a box with a concurrent agent on it).
>
> The port is still FASTER than C at 32-64 px on the fast presets — its fixed
> per-frame cost is 0.93x C's at p10. All 24 cells byte-identical.
> `benchmarks/perf_gap_2026-08-13-r1r2.{tsv,raw.tsv,meta}` (the mid-session run
> is `perf_gap_2026-08-13-final.*`, and an earlier one `perf_gap_2026-08-13.*`).
>
> ### WHAT THE REMAINING GAP IS MADE OF (measured 2026-08-13 evening)
>
> `benchmarks/perf_class_attrib_2026-08-13.{tsv,meta}` — paired `/usr/bin/sample`
> profiles of BOTH binaries on the same byte-identical cells, self time
> attributed per symbol, shares scaled by the paired encode ms above. Buckets:
> **SIMD_GAP** = the port's kernel is scalar-only on aarch64 and C ships a
> `SET_NEON`-registered one; **SIMD_QUAL** = both sides vectorised; **ALLOC** =
> allocator + libc mem; **SCALAR_BOTH** = code neither side vectorises.
>
> | cell | ratio | SIMD_GAP | SIMD_QUAL | ALLOC | SCALAR_BOTH |
> |---|---|---|---|---|---|
> | 512² p10 | 2.80x | 11.9 % | 17.3 % | 29.5 % | 41.3 % |
> | 1024² p10 | 2.84x | 12.3 % | 17.1 % | 27.0 % | 43.6 % |
> | 512² p6 | 3.10x | 27.7 % | 19.5 % | 21.7 % | 31.0 % |
> | 512² p2 | 3.79x | 15.6 % | 26.2 % | 19.1 % | 39.1 % |
>
> **At the fast presets, missing SIMD coverage is ~12 % of the gap.** Driving
> every scalar-where-C-is-NEON kernel to C's cost takes 512² p10 from 2.80x to
> 2.47x; also matching C on the kernels BOTH sides vectorise (where the port is
> 1.95-2.20x slower) gives 2.27x; a zero-allocation port on top gives 1.74x.
> **1.03x is not reachable through SIMD, nor through SIMD plus allocation** —
> it additionally needs the port's driver/entropy/RDOQ code (scalar in C too) to
> get 2.1x faster, and nothing measured suggests a mechanism. At p6 the picture
> is different: loop restoration (15.6 %) and CDEF (12.3 %) are 28 % of the gap
> and are almost pure coverage (`compute_stats` 3.83 vs 0.75 ms,
> `wiener_convolve_add_src` 10.3x, `cdef_find_dir` 15x) — that is where SIMD pays.
>
> Three classes are already at or past parity at p10 and are NOT levers any more:
> INV_TXFM **1.08x** (R1's gate did its job), QUANT_RDOQ 1.13x, RANGE_CODER
> 1.25x, and the coefficient WRITER is **0.77x — the port is faster than C**.
> **One item from this sizing has since been LANDED** — the MDS0 Hadamard below
> (`5bfbcd742`, p10/p13 2.89x -> 2.71x). The bucket shares above are as measured
> BEFORE it; the item sat in SIMD_GAP, so the SIMD_GAP share at p10 is now
> smaller than the 11.9 % shown and the conclusion is strengthened, not weakened.
>
> Two traps the `.meta` documents: `<deduplicated_symbol>` is 4.4 % of C's
> samples and is almost all inverse transform (leaving it unattributed overstates
> the transform gap ~2x), and the xzone allocator charges its own
> `mach_absolute_time`/`madvise` to libsystem_kernel (~1.4 points of ALLOC).
>
> **Largest single item found, and it is NOT a SIMD gap: the MDS0 Hadamard.**
> The port spends 4.79 % (512² p10) / 5.12 % (1024² p10) of its whole frame in
> `hadamard_satd` + `dsp::hadamard::*`; C spends **zero** — `grep -ci
> 'hadamard|satd'` over C's entire sampled call graph returns 0 at both cells
> (7,126 and 19,073 samples) while `svt_aom_variance*_neon_dotprod` appears in
> both. C's `fast_loop_core` takes the VARIANCE arm because
> `mds0_use_hadamard_blk = mds0_use_hadamard_sb && fast_candidate_total_count > 1`
> (product_coding_loop.c:9473) is false when the preset >= 9 `dc_only` gate
> injects exactly one candidate. The port has no such gate
> (`leaf_funnel.rs:4923`). Same class as R1/R2 — work whose result cannot reach
> the bitstream — and the fix is C's gate plus C's variance arm, NOT a NEON
> Hadamard (which would still be more work than C does).
>
> **The 08-13-mid -> here step is the code review's R1 and R2** — the first two
> findings of `docs/C-VS-PORT-CODE-REVIEW-2026-08-13.md`, and the first two
> changes of the campaign that remove work whose result was DISCARDED rather
> than DUPLICATED. Note this is the first movement at **p2** all session.
>
> * `56d19efe1` **R1 — the inverse transform ran even where the reconstruction
>   is thrown away.** C gates it on `mds_do_spatial_sse || (!is_inter &&
>   tx_depth)` (product_coding_loop.c:4783-4784) and all-intra pins
>   `spatial_sse_full_loop_level = 3`, so C inverts NOTHING at MDS1/MDS2. A
>   census put the discarded share of inverse-transform pixel work at 40-50 %
>   (p10/p13), 36-50 % (p8), 43-51 % (p7), 28-53 % (p6), 24-44 % (p2). Three
>   call sites opt out via an explicit `need_recon`, each with an
>   exhaustive-scan dead-proof in the commit message. A/B: 12/12 cells
>   1.021-1.053x at qp40, 28 of 28 cells below 1.0 across 6 presets x 3 sizes
>   x 2 qps (control arm 13 below / 15 above; sign test p = 3.7e-9).
>   `benchmarks/recon_gate_r1_ab_2026-08-13.meta`.
> * `8179a7d94` **R2 — the exact coefficient rate was computed and then
>   overwritten** wherever C's closed forms apply (C's tiers are an
>   `if / else if / else`; the estimator is never reached on those arms). Now
>   evaluated in C's order. 1.038-1.060x at p10/p13 **qp20**, and null at
>   qp40/512+ — the wall clock tracks the census share of replaced coefficient
>   work exactly (51-54 % at qp20, 16-38 % at qp40, ZERO at qp55).
>   `benchmarks/ratemode_r2_ab_2026-08-13.meta`.
> * `7dec5f24e` — the census instrument behind both
>   (`benchmarks/txunit_census_2026-08-13.tsv`, feature `__txcensus`).
>
> Two corrections to the review, both from the census: its R2 scope treats the
> **level-2** tier (p7/p8) as a live arm — it fires on 164 of 8,404 calls in one
> of eighteen p7/p8 cells and zero in the other seventeen, so R2's value is
> entirely the level-0 tier at p10/p13; and its R1 site table implies three
> contributing sites at the fast presets — at p10/p13 only MDS1 contributes,
> the other two sit behind `cfg.cfl_enabled`, false from M7 up.
>
> The 08-11 -> 08-13 step is four byte-identical changes, and every one of them
> REMOVES A DUPLICATED COPY of something already computed — none of them makes
> an allocation cheaper (see the corrections below for why that matters):
>
> * `29847e5d3` — **the frame's block-decision set was materialised FOUR
>   times.** A `BlockDecision` owns up to nine heap `Vec`s; it was cloned at
>   each funnel leaf so the partition TREE and a parallel `decisions` list could
>   both hold it, aggregated up the tree by ~25 `extend` calls, deep-cloned
>   again into a `per_tile_decisions` that was **written and never read**, and
>   the whole per-SB tree deep-cloned into its raster slot. Only the tree
>   survives; `num_blocks` comes from `PartitionTree::count_leaves`. A/B
>   1.072-1.111x at p10, 1.014-1.031x at p6
>   (`benchmarks/alloc_decisioncopy_ab_2026-08-13.meta`).
> * `6ad044d00` — **`to_choice` cloned seven of the winning candidate's buffers
>   only because it ran before `commit_leaf`.** Both callers now commit first
>   and `into_choice` MOVES. A/B 1.016-1.027x at p10, null at p6
>   (`benchmarks/into_choice_ab_2026-08-13.tsv`).
>
> * `81a1bb111` — **`funnel_block_decision`'s depth-0 qcoeff "unpack" was a
>   byte-for-byte copy** on every block without a 64-dim side, and
>   **`DecodedPictureBuffer::refresh` deep-cloned the whole picture once per
>   set bit** of `refresh_frame_flags` — eight full Y planes per KEY frame,
>   into slots only ever read as `&ReferenceFrame` (now `Arc`-shared, private
>   field, no API change). A/B 1.012-1.022x at p10, null at p6
>   (`benchmarks/qcoeff_dpb_ab_2026-08-13.meta`).
> * the per-SB recon staging buffer (an allocation, a zero-fill and a second
>   pass over every pixel of every superblock) is gone. **Measured null**
>   (`benchmarks/sb_recon_staging_null_2026-08-13.tsv`); kept only because it
>   is strictly less work.
>
> p2 did not move all session (4.12 -> 4.16 -> 4.14, inside its own spread):
> every change is a per-coded-block cost and p2 spends ~60x longer per block
> inside the RD search itself.
>
> The 08-11 p10/p13 step before that came from ONE change: the in-loop deblock
> and CDEF *application* passes are skipped when nothing reads the
> reconstruction (`EncodePipeline::with_recon_output`, default off — C's API
> produces no recon either). 1.35-1.39x whole-encode at p10/p13, 1.11-1.15x at
> p7, byte-inert at preset >= 7 (0/90 cells) and deliberately still applied at
> preset <= 6, where the CDEF and Wiener searches read the filtered pixels
> (13/36 cells change there). `benchmarks/perf_postfilter_2026-08-11.meta`.
>
> **Where the remaining gap is** — `/usr/bin/sample`, 1 ms, 15 s steady state,
> self time, gradient 512², port at `29847e5d3` (11,478 leaf samples), with the
> C reference's own profile on the identical cell for scale:
>
> | binary | port p10, 08-11 | port p10, now | C p10 |
> |---|---:|---:|---:|
> | the encoder itself | 75.5 % | 81.2 % | 95.0 % |
> | `libsystem_malloc` | 11.9 % | 9.0 % | 0.49 % |
> | `libsystem_platform` (memmove/memset/bzero) | 10.9 % | 8.1 % | 2.70 % |
>
> **The alloc + libc-mem excess over C is still ~13.9 points of the port's self
> time (was 19.6 before this session), i.e. an arithmetic ceiling of
> 1/(1-0.139) = 1.16x** — not the 1.33x that quoting the port's 17.1 % alone
> suggests. Nearest-app-ancestor attribution of the malloc-family samples (taken
> at `29847e5d3`) ranks it: `leaf_funnel::evaluate_leaf` 26.0 %,
> `leaf_funnel::tx_unit_inner` 13.6 %, `partition::extract_neighbors_tiled`
> 7.7 %, `pd0::Pd0Ctx::lvl5_like_block_cost` 6.6 %, `pd0::tx_quant_core` 4.3 %,
> `partition::funnel_block_decision` 4.3 %, then a long tail.
>
> **Two corrections to what this section used to say.** (1) The claim that the
> traffic is so diffuse that "the largest single parent is 2.4 % of mem
> samples" does not survive re-measurement: at 1 ms self-time resolution with
> nearest-app-ancestor attribution the top parent is 26 % and the top six cover
> 62 %. (2) The prescription — "pool the per-block decision's `Vec` members into
> an arena reused across the partition search" — was BUILT and measures
> **NULL** at n=31 against an in-grid control, even though it demonstrably
> removed a whole class of allocations from the profile. On macOS's xzone
> allocator a thread-local `Vec` pool costs about what `malloc`/`free` costs at
> these sizes, and the `calloc` it replaces was doing the same memset the pool's
> `resize` does. Full record + what to try instead (one arena allocated at
> pipeline construction that the buffers are `&mut [T]` slices INTO, with no
> per-buffer bookkeeping — C's actual shape):
> `benchmarks/alloc_bufpool_null_2026-08-13.meta`. **Do not re-run the pool
> experiment.**

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
   any zero is skipped (the riskiest byte-identity change; deferred). The entropy
   coeff context sum `get_nz_map_contexts` is now SIMD'd (see "Landed"); the
   remaining `nz_map_ctx` slice (~3 %) is the RDOQ-trellis
   `lower_levels_ctx_general` path, a separate caller.

Approach order per the criteria: algorithmic parity (done on this envelope),
then allocation discipline, then SIMD. On these numbers, SIMD on the hot loops
is the biggest single lever.

## Landed byte-inert optimizations

- **`get_nz_map_contexts` SIMD (AVX2) — the coeff nz-map context sum**
  (`crates/svtav1-entropy/src/{coeff_simd,coeff_c}.rs`, branch `perf/nzmap-simd`).
  The per-txb scan-order context derivation was the #3 hotspot after the zeroing
  work — callgrind at 256²: `get_nz_map_contexts` + `nz_map_ctx` = **10.2 % of
  frame instructions at p6, 9.7 % at p10**. The port now mirrors C's RTCD split:
  the x86 arm reproduces the production `svt_av1_get_nz_map_contexts_sse2`
  verbatim — an `eob == 1` early-out, then a **raster** fill of the whole padded
  block written directly into `coeff_contexts` (16 positions/iter in C's three
  width shapes: one 16-column row chunk at w ≥ 16, 4 whole rows at w == 4, 2 at
  w == 8 — contiguous loads, no scattered gathers), the 2D DC zero, and the
  scan-last stamp; position-base offsets come from a compile-time `NZ_OFFSET`
  table built from the same `nz_map_ctx_offset_2d/_1d` helpers the scalar path
  uses (no re-transcribed vector constants — the classic nz-map byte-diff
  source). Scalar/NEON arms run the scan-order `_c` loop verbatim, exactly C's
  `SET_ONLY_C`/`SET_NEON` fallbacks. All arms are byte-identical at every
  `scan[0..eob]` position — the only bytes any caller reads (verified for both
  call sites, pd0 `loop_cost_eob_pd0` + the leaf-funnel coeff cost); non-scan
  positions carry tier-dependent raster values exactly as production C's
  `_sse2`/`_avx2` leave them. **Proven** (tests/c_parity.rs): `nz_map_contexts_
  simd_matches_c` — port == exported real-C `_c` AND `_sse2` at every scan
  position across all 19 tx sizes × 3 tx classes × eob buckets (DC-only,
  bucket edges, dense eob == n which makes every raster position a compared
  position), under every archmage tier permutation with a positive ≥ 2-
  permutation assert on AVX2 hosts; the 0xFF stale-read police
  (`coeff_c_txb_init_levels_partial_zero_no_stale_reads`) additionally asserts
  the port's FULL raster buffer == real-C `_sse2` on AVX2 hosts (the raster
  worst-case tap read ends exactly at the `used` zeroing extent). **Measured**:
  callgrind (deterministic) 256² whole-frame instructions **−5.75 % (p6:
  556.7M → 524.7M), −5.7 % (p10: 87.8M → 82.8M)**; kernel-only **5.3× (p6) /
  6.1× (p10)**. Wall (paired before/after A/B, both port binaries interleaved
  per round, randomized order, 40 rounds/cell, per-cell byte-identity
  pre-checked; run under steady ~14 loadavg from concurrent agents — absolute
  ms inflated, the paired ratio is the metric): **every cell of
  {128,256,512,1024}² × p{6,10,13} faster — −5.9 % to −16.3 %, grid median
  −8.4 %** (e.g. 1024² p6 0.928, 256² p13 0.837, 512² p13 0.911). Data:
  benchmarks/perf_nzmap_ab_2026-07-23.{raw.tsv,meta} +
  perf_nzmap_callgrind_2026-07-23.txt + perf_nzmap-before-master.* (clean-box
  sweep; the perf_nzmap-after sweep is load-contaminated and annotated as such
  in its .meta — its per-cell ident=Y identity pre-pass at 64..1024² ×
  p{6,10,13} is the load-independent part that counts). All 14 gates green at
  baseline (identity 54/54, bd10 36/36 + 309/309 + 158/158, partial-SB
  101/101, sb128 22/22, tile 29/29, arb-size 57/57, combos 40/40, panic 60/60,
  palette 50/50, ibc-fh PASS, ibc 20/100 + 80 pins rc=0) + `cargo nextest run
  --workspace` 916/916. `#![forbid(unsafe_code)]` intact; the remaining
  `nz_map_ctx` slice (~3 %) is the RDOQ-trellis `lower_levels_ctx_general`
  path — a separate caller, untouched.

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
  the bd10 gates). All 11 byte-identity gates + `cargo nextest run --workspace` green;
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

**Not at ≤1.2× yet.** Remaining fast-preset levers, now that the transforms and the
`get_nz_map_contexts` context sum are SIMD'd: **quant** (`quantize_b`/`quantize_fp`),
the coeff **writer** + the RDOQ-trellis `lower_levels_ctx_general`/`nz_map_ctx` path,
and SAD/SSE — each a smaller slice. p6 additionally carries the CDEF+LR search. All
are byte-exact-portable via the same archmage pattern in an isolated worktree.

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

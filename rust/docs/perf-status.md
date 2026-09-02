# Performance status — G4 baseline (port vs C wall clock)

> **MEMORY (2026-09-02, aarch64 / Apple M4 Pro) — read this before quoting any
> memory number.** Record: `benchmarks/mem_2026-09-02.{tsv,meta}`, 56 cells,
> median of 7 runs per cell, `/usr/bin/time -l` max RSS, plain `--release`
> port binary (NOT the `symtrace` wrapper the 2026-08-16 record used).
> Supersedes `benchmarks/mem_2026-08-16.meta`.
>
> | path | 64x64 | 256x256 | 1 MP | 4 MP | port fit | C fit |
> |---|---|---|---|---|---|---|
> | still p2  | 0.50x | 0.63x | 0.82x | **0.78x** | 6.69 MiB + 31.28/MP | 10.02 + 38.68 |
> | still p6  | 0.57x | 0.70x | 0.99x | **1.06x** | 4.87 MiB + 30.20/MP |  7.60 + 27.40 |
> | still p10 | 0.64x | 0.77x | 1.07x | **1.18x** | 4.11 MiB + 24.35/MP |  6.11 + 19.93 |
> | still p13 | 0.64x | 0.77x | 1.07x | **1.18x** | 4.11 MiB + 24.35/MP |  6.11 + 19.93 |
> | INTER p13 | 0.81x | 0.86x | 1.27x | **1.53x** | 4.55 MiB + 40.34/MP |  7.24 + 25.51 |
>
> (fit = `alpha + beta*pixels`; both terms are quoted because at 64x64 the
> intercept is ~100 % of the number and at 4 MP it is ~4 %.)
>
> **The STILL path already meets the 25 % memory goal at every size measured**
> — worst cell 1.18x, and the port is LIGHTER than C below ~1 MP at every
> preset and at EVERY size at preset 2. **The INTER path does not, and it is a
> SLOPE problem:** at preset 13 the port's per-pixel term is 40.34 MiB/MP
> against C's 25.51 (1.58x) while its fixed term is 0.63x C's, so the ratio
> grows with size — 1.27x at 1 MP, 1.53x at 4 MP (169.8 vs 111.2 MiB). Same
> preset, still vs inter: the port adds **16.0 MiB/MP** for one reference
> frame where C adds **5.6**. One 8-bit 4:2:0 reference is 1.43 MiB/MP of
> pixels, so C carries ~4x the raw reference and the port ~11x.
>
> Two caveats the `.meta` states in full and that are load-bearing:
> * **No inter cell above 128x128 is byte-identical to C**, so the inter
>   ratios compare two encoders making different decisions. The p6/p8 inter
>   cells that ARE byte-identical (64..256) put the port at 0.64-0.75x. Re-run
>   this once the inter byte frontier reaches 1 MP.
> * **The port's peak RSS is not single-run stable** — 2048x2048 p6 spans
>   126.2-136.6 MiB over seven runs (8 %) where C spans 0.04 %. It encodes
>   tiles on a thread pool; C at `--lp 1` does not. `tools/mem_gate.sh` now
>   takes `MEM_REPS` (default 5) and reports the median with the spread.
>
> **THE INTER PATH PANICS ON 31 OF 36 PARTIAL-SUPERBLOCK CELLS**
> (`tools/inter_completion_scan.sh`,
> `benchmarks/inter_completion_2026-09-02.tsv`: 64 cells, 24 OK — only 4 of
> them byte-identical — 6 REFUSED, **34 CRASH**). Three distinct panics:
> an off-the-end plane index in `port_md/md_search.rs:422` (frame 1, every
> preset scanned), `leaf must be tested` in `pd0.rs:2703` (frame 1, p10/p13,
> sizes with SB remainder 40), and `video pic_pd0_lvl 7` in `pd0.rs:3337`
> (frame **0**, the KEY frame, p9/p10, every size at or above the 360p->480p
> class boundary — 560x560 clean, 568x568 panics; this is why the p10 inter
> memory series stops at 512x512). The inter byte gate and matrix sweep
> {16,64,72,128}, of which only 72 is partial, so the frontier they describe
> is almost entirely 64-aligned and this surface is invisible to them.
>
> **INTER CPU (2026-09-02, aarch64 / Apple M4 Pro) — FIRST port/C wall-clock
> ratio on the inter path, and the answer is NOT "the inter frame".** Records:
> `benchmarks/perf_2026-09-02-arm-{still,videokey,inter}.{tsv,raw.tsv,meta}`,
> paired interleaved, 25 rounds/cell, gradient qp 40, `PERF_FRAMES` /
> `PERF_VIDEO`.
>
> THREE ARMS at the same size, preset, qp and session, so the two variables a
> 2-frame cell changes at once can be separated:
>   * **still** — 1 frame, `avif` still-picture config (what every prior perf
>     number in this file measures)
>   * **videokey** — 1 frame, VIDEO config (`PERF_VIDEO=1` / `SVT_AVIF=0`). A
>     video-mode KEY frame and nothing else. Byte-identical to C at every size
>     and both presets measured.
>   * **inter** — 2 frames, low-delay P, flat GOP: that same key frame plus one
>     inter frame.
>
> | preset 8 | 64x64 | 128x128 | 256x256 | 512x512 | slope ratio |
> |---|---|---|---|---|---|
> | still    | 0.90x | 1.53x | 2.50x | 2.66x | 2.78x |
> | videokey | 1.52x | 2.30x | 2.95x | 3.14x | 3.19x |
> | inter    | 1.92x | 2.74x | 3.40x | 3.83x* | 3.67x |
>
> (* 512x512 inter is the one cell of the three arms that is NOT byte-identical
> at p8; every other cell in the table is. At preset 6 all three arms are
> byte-identical at 64x64 and the videokey arm is byte-identical at every size:
> still 1.67/2.39/3.06/3.06, videokey 2.14/2.62/2.98/3.03, inter 2.26/2.80*/
> 3.10*/3.40*.)
>
> **DIFFERENCING the arms — a subtraction of measured quantities, never a
> projection — splits the port's excess over C on an inter cell three ways.**
> At 256x256 p8, where all three arms are byte-identical:
>
> | component | port | C | ratio | share of the excess |
> |---|---:|---:|---:|---:|
> | still key frame            |  2.98 ms | 1.19 ms | 2.50x | 17 % |
> | what VIDEO CONFIG adds to that key frame |  7.75 ms | 2.45 ms | 3.17x | **49 %** |
> | the INTER frame itself     |  4.65 ms | 0.91 ms | **5.10x** | 35 % |
> | total (2 frames)           | 15.37 ms | 4.55 ms | 3.40x | |
>
> **HALF the port's excess on an inter cell is the VIDEO-MODE KEY FRAME, not
> the inter frame.** That share is 44-52 % at EVERY cell measured (both presets,
> all four sizes) — the most stable number in this table. Encoding the same key
> frame under the video signal derivation instead of the all-intra one costs the
> port 3.2x what it costs C, and it is a bigger absolute item than the inter
> frame at every cell except 64x64 p8. Anyone optimising "the inter path" who
> starts at the motion search is starting at the smaller half.
>
> The inter FRAME on its own is 2.68x (64 p6), 2.94x (64 p8), 4.11x (128 p8),
> **5.10x (256 p8)** — the four cells where all three arms are byte-identical —
> and the ratio GROWS with size, so it is a per-pixel problem in the inter
> frame too, not a fixed cost.
>
> Two independent methods agree on the port's own key/inter split, which is
> why the differencing above is trusted: subtracting the arms gives
> videokey 10.73 ms / inter 4.65 ms at 256x256 p8, and the port harness's own
> `FRAME_NS` (9 runs, median) gives 10.64 / 4.54 — within 1 %.
>
> **THE BOX WAS NOT QUIET AND THE CONTROL SAYS THAT DID NOT MATTER.** A sibling
> agent's `encode_backend_sweep` held ~4 cores for the whole session. The still
> control re-measured 8 cells that `benchmarks/perf_2026-08-13-hadamard.tsv`
> already carries (64/128/256/512 x p6/p10 at qp 40) and reproduced every one
> within 3.4 %, most within 1 %: 64 p6 1.630 -> 1.615, 128 p6 2.386 -> 2.396,
> 256 p6 3.054 -> 3.033, 512 p6 3.058 -> 3.024, 64 p10 0.756 -> 0.770, 128 p10
> 1.365 -> 1.412, 256 p10 2.199 -> 2.273, 512 p10 2.659 -> 2.713. The ABSOLUTE
> times did move (512 p6 port 39.1 -> 44.0 ms, C 12.8 -> 14.6, both ~+13 %) —
> the randomized-order paired design cancelled it out of the ratio, which is
> the first time that claim has been checked against contention rather than
> asserted. Record: `benchmarks/perf_2026-09-02-control-still.{tsv,meta}`.
> Read the ratios, not the absolute ms, out of this session.
>
> **What is NOT measured:** any inter cell above 256x256 apples-to-apples (the
> byte frontier ends there), more than ONE inter frame (the port refuses frame 2
> — `ref_hp_percentage`/`ref_skip_percentage` unported, so every inter number in
> this repo is about the FIRST inter frame after a key frame, the cheapest one a
> GOP has), any partial-SB inter cell (they panic), and 10-bit or real content.
>
> ### WHERE THE INTER GAP IS, BY SYMBOL (2026-09-02)
>
> `benchmarks/perf_inter_attrib_2026-09-02.{tsv,meta}` — paired
> `/usr/bin/sample` profiles of all THREE arms on both binaries at the same
> byte-identical cell (gradient 256x256 p8 q40), self time per symbol, shares
> scaled by the paired encode ms above. The two differences below are
> subtractions of measured quantities.
>
> **The INTER FRAME itself (port 4.65 ms, C 0.91 ms) is 61 % motion-search
> distortion, and every kernel in it is SCALAR while C ships NEON-dotprod:**
>
> | port function | ms | % of the port's inter frame | C counterpart |
> |---|---:|---:|---|
> | `inter_me::sad::nxm_sad_kernel` | 0.955 | **20.5 %** | `svt_sad_loop_kernel*_neon_dotprod` |
> | `dsp::subpel_variance::sub_pixel_variance` | 0.759 | **16.3 %** | `sub_pixel_variance_w*_neon_dotprod` |
> | `inter_me::sad::compute8x4_sad_kernel` | 0.548 | 11.8 % | `svt_ext_all_sad_calculation_8x8_16x16_neon` |
> | `port_md::md_search::PlaneDistortion` impls | 0.339 | 7.3 % | same NEON family |
> | `motion_est::full_pel_search` | 0.142 | 3.0 % | `svt_sad_loop_kernel_neon_dotprod` |
>
> Those five sum to **2.74 ms = 59 % of the port's inter frame. C's whole inter
> frame spends 0.099 ms on the corresponding kernels — a 28x gap.** Verified
> scalar by source read: none of `inter_me/sad.rs`, `dsp/subpel_variance.rs` or
> `port_md/md_search.rs` contains an `incant!`, `#[arcane]`, `#[rite]` or
> `magetypes` anywhere.
>
> **DO NOT budget a 28x SIMD win from that table.** A pure coverage gap
> plausibly explains a single-digit factor, not 28x; the rest is consistent with
> the port ISSUING MORE SEARCH WORK than C — `md_search.rs` carries an MVP scan
> (`best_mvp_by_distortion`), a full-pel PME (`pme_search_for_ref`) and up to two
> sub-pel tree searches per reference (`md_subpel_search`,
> `md_subpel_search_fixed_stage`), and none has had its CALL VOLUME compared
> against C's. Splitting coverage from volume needs an operation census (count
> SAD / variance / sub-pel evaluations per block on both sides), not another
> profile. The still path already taught this: `aom_hadamard_8x8` was 1.88 % of
> p2 and delivered 1.031x because the caller's own scalar loop stayed.
>
> **What the VIDEO CONFIG adds to the KEY frame (port 7.75 ms, C 2.45 ms,
> 3.17x)** lands almost entirely on kernels the still-path queue ALREADY ranks:
> `COEFF_CTX` 1.41 ms (`nz_map_ctx` 0.82 alone), `LOOP_RESTORE` 1.02 (6.0x —
> `compute_stats` 0.68 + `wiener_convolve_add_src` 0.32), `RANGE_CODER` 0.91
> (6.7x), `QUANT_RDOQ` 0.90, `CDEF` 0.89 (7.0x — `cdef_filter_block` 0.41 +
> `cdef_find_dir` 0.21), `COEFF_WRITE` 0.65, `INTRA_PRED` 0.65 (10.7x —
> `dr_predictor_edged` 0.34). Loop restoration and CDEF are ZERO in the still
> arm at p8 and non-zero in the video arm on BOTH sides — the still API produces
> no recon so the post-filters are skipped (`with_recon_output`, the 2026-08-11
> change) while a video frame's recon IS a reference. That is faithful, not
> waste. **It also means the existing SIMD-coverage queue below is worth roughly
> twice what the still-only numbers suggested**, because those same kernels run
> on every frame of a video encode and on none of a still one.


> **CURRENT (2026-08-13, aarch64 / Apple M4 Pro — read this first).** Everything
> below the "Results — 2026-07-20" heading is the **x86-64/AVX2 history** on
> `dev-32gb`. The live numbers on the aarch64 box are:
>
> | preset | slope ratio port/C | was (mds0var) | was (08-13 R1R2) | was (08-13 mid) | was (08-11) | was (08-07) |
> |---|---|---|---|---|---|---|
> | p2 | **3.77x** | 3.91x | 3.93x | 4.14x | 4.12x | 4.11x |
> | p6 | **3.22x** | 3.25x | 3.27x | 3.39x | 3.52x | 3.50x |
> | p10 | **2.74x** | 2.71x | 2.89x | 3.06x | 3.53x | 4.85x |
> | p13 | **2.73x** | 2.71x | 2.89x | 3.07x | 3.51x | 4.83x |
>
> Current record: `benchmarks/perf_2026-08-13-hadamard.*` (24 cells, all
> byte-identical, n=9). The p2/p6 step from the mds0var column is the NEON
> Hadamard; p10/p13 moving 2.71 -> 2.74 is cross-session drift, not a
> regression — the hadamard change is byte-inert AND work-inert at p >= 9 (the
> MDS0 gate already took the port off that arm there), and its A/B measured
> null at p10. Treat +/-0.03x between perf_gate runs as noise; the paired A/B
> records are the attribution evidence.
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
> per-frame cost is 0.86-0.90x C's at p10/p13. All 24 cells byte-identical.
> Current record `benchmarks/perf_2026-08-13-mds0var.{tsv,raw.tsv,meta}`; the
> R1R2 run is `perf_gap_2026-08-13-r1r2.*`, the mid-session one
> `perf_gap_2026-08-13-final.*`, and an earlier one `perf_gap_2026-08-13.*`.
>
> The p2/p6 movement to look for next is `dsp::hadamard` — `aom_hadamard_8x8`
> gained a real NEON arm (`benchmarks/hadamard_neon_ab_2026-08-13.meta`):
> 1.028-1.042x at 512²/1024² p2 and p6, marginal at 256² p2, NULL at 256² p6.
> The rest of that family's 7.5 % (p2) / 4.0 % (p6) frame share is
> `leaf_funnel::hadamard_satd`'s own scalar residual-build loop, untouched.
>
> ### THE SIMD-COVERAGE QUEUE, RANKED BY MEASURED FRAME SHARE
>
> Every entry is a port function that is SCALAR on aarch64 with a
> `SET_NEON`-registered C counterpart, with its self time as a share of the
> port's whole frame at 512² (from `benchmarks/perf_class_attrib_2026-08-13.tsv`
> — profile shares, not A/B results). Do NOT budget a win from these directly:
> `aom_hadamard_8x8` was 1.88 % of p2 and delivered 1.031x at that cell because
> the caller's own scalar loop stayed. Price each one, then build it.
>
> | port function | p6 share | p2 share | C counterpart |
> |---|---|---|---|
> | `restoration::compute_stats` (**already NEON** — quality, not coverage) | **9.83 %** | 0.60 % | `svt_av1_compute_stats_neon` (5.1x) |
> | `cdef::cdef_filter_block` (**already NEON** — quality) | 4.70 % | 2.73 % | 5.6x vs `cdef_filter_block_*_neon` |
> | `restoration::wiener_convolve_add_src` | **2.68 %** | 1.17 % | `svt_av1_wiener_convolve_add_src_neon` (10.3x) |
> | `cdef::cdef_find_dir` | **2.02 %** | — | `svt_aom_cdef_find_dir*_neon` (15x) |
> | `intra_pred::dr_predictor_edged` | — | **4.51 %** | `svt_av1_dr_prediction_z{1,2,3}_neon` |
> | `leaf_funnel::hadamard_satd` (its own residual loop) | 1.47 % | **3.55 %** | `svt_aom_residual_kernel*_neon` inside `hadamard_path` |
> | `intra_pred::predict_dc` | 1.25 % | 0.29 % | `svt_aom_dc_predictor_WxH_neon` |
> | `encoder::cdef::{filter_and_count, cdef_search_still}` | 1.98 % | 0.38 % | `svt_aom_compute_cdef_dist_8bit_neon_dotprod` |
> | `intra_pred::predict_filter_intra` / `predict_smooth` | 1.57 % | 0.24 % | `svt_av1_filter_intra_predictor_neon`, `svt_aom_smooth_predictor_*_neon` |
>
> `intra_pred.rs` carries exactly ONE `incant!` (PAETH); the rest of the file is
> scalar. Note archmage 0.9.28 has no standalone dotprod/i8mm token — those
> capabilities are only reachable bundled in `Arm64V2Token`/`Arm64V3Token`.
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
> | 512² p6 | 3.10x | 16.0 % | 31.2 % | 21.7 % | 31.0 % |
> | 512² p2 | 3.79x | 15.0 % | 26.9 % | 19.1 % | 39.1 % |
>
> **At the fast presets, missing SIMD coverage is ~12 % of the gap.** Driving
> every scalar-where-C-is-NEON kernel to C's cost takes 512² p10 from 2.80x to
> 2.47x; also matching C on the kernels BOTH sides vectorise (where the port is
> 1.95-2.20x slower) gives 2.27x; a zero-allocation port on top gives 1.74x.
> **1.03x is not reachable through SIMD, nor through SIMD plus allocation** —
> it additionally needs the port's driver/entropy/RDOQ code (scalar in C too) to
> get 2.1x faster, and nothing measured suggests a mechanism. At p6 the picture
> is different: loop restoration (15.6 %) and CDEF (12.3 %) are 28 % of that gap
> and are where SIMD pays — but `compute_stats` (3.83 vs 0.75 ms) is a QUALITY
> item, already NEON on both sides, while `wiener_convolve_add_src` (10.3x) and
> `cdef_find_dir` (15x) are coverage items, verified scalar by source read.
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
>   (`benchmarks/txunit_census_2026-08-13.tsv`). **The `__txcensus` feature was
>   DELETED 2026-08-16** along with `__ovf_probe`: both were self-labelled
>   TEMPORARY, both had answered their question, and both had become permanent
>   public surface across four manifests. The committed .tsv is the record; to
>   re-census, re-add the instrument rather than expecting the flag to exist.
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
  --workspace` (864 tests as of `3c34350ce`). Measured (callgrind, deterministic): 256² p6 frame
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
  `#![forbid(unsafe_code)]` intact. Data: benchmarks/perf_{before,after}_cdef.tsv
  — **those two files were never committed** (issue #8 audit, 2026-08-28); the
  numbers in this block are unbacked and must not be cited. The committed perf
  records are the `benchmarks/perf_2026-*.{tsv,raw.tsv,meta}` triples.

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

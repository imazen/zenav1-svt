# Performance status — G4 baseline (port vs C wall clock)

> **THE RESIDUAL IS INSTRUCTION COUNT, NOT STALLS — THE PORT'S IPC IS HIGHER
> THAN C'S ON EVERY CELL MEASURED (2026-09-05).** Record
> `benchmarks/stall_attrib_2026-09-05.{tsv,meta}`. Hardware counters
> (`perf stat`, paired warmup-delta, median of 5, both sides pinned to one
> core), on three byte-identical cells at `2d75a105f`:
>
> | cell | instructions port/C | cycles port/C | IPC port | IPC C |
> |---|---|---|---|---|
> | photo_cid 512² p2 | 1.753x | **1.506x** | 3.506 | 3.012 |
> | photo_cid 512² p6 | 2.310x | **1.899x** | 3.445 | 2.832 |
> | screen_terminal 512² p6 | 2.555x | **1.928x** | 3.584 | 2.704 |
>
> **The port's real-time gap is 14-25 % SMALLER than its instruction gap**,
> because it retires 16-33 % more instructions per cycle than C does. At
> photo_cid p6 the port's 874,697,071 extra instructions cost 211,932,008
> extra cycles — LESS than they would cost at the port's own average IPC. There
> is no stall excess to attribute, and the Ir-based ranking is the honest
> picture of the residual.
>
> **Three lines of investigation close at once.**
> *Cache:* the port misses L1D per instruction at HALF C's rate (4.56 vs
> 10.12 MPKI at photo p6; 2.16 vs 5.53 at p2), its L1I and op-cache miss rates
> are at or below C's, and neither side is DRAM-bound — deterministic
> simulation counts 128,472 last-level misses for the port against C's 132,781
> on the same frame. TLB misses are noise on both sides.
> *Branches:* the port takes 1.08-1.68x C's absolute mispredicts while
> executing 1.75-2.56x C's instructions, so per instruction it mispredicts
> LESS (2.48 vs 3.40 MPKI at photo p6) and by rate 2-3x less.
> *`#[cold]`:* **no candidate is warranted.** The largest single mispredict
> site is `pipeline.rs:9333` — a ~50/50 data-dependent `if coeffs[pos] != 0`
> inside a forward scan of the WHOLE transform block (316,928 simulated
> mispredicts at a 17.16 % rate, 82 % of `encode_block_syntax`'s total). A rare
> path is what `#[cold]` is for; this is not one. The fix is the reverse scan
> with early return already written at `quant.rs:318-324`, whose branch
> mispredicts at 3.10 %. The same forward loop is duplicated at
> `pipeline.rs:9410-9415`.
>
> **What DOES change is the ORDERING.** Ranked by CYCLES instead of Ir
> (`perf record`, port symbols joined to C symbols), photo_cid p6:
>
> | rank | pair | port cyc% | C cyc% | excess | cyc x | IPC port/C |
> |---|---|---|---|---|---|---|
> | 1 | libc memset/memcpy/alloc | 6.65 | 1.60 | **+5.84** | 8.23 | 2.40 / 2.32 |
> | 2 | CDEF filter kernels | 6.35 | 1.21 | +5.74 | 10.39 | 3.43 / 5.03 |
> | 3 | entropy coeff writer | 7.14 | 4.14 | +5.05 | 3.42 | 2.85 / 2.28 |
> | 4 | range coder | 5.42 | 3.22 | +3.80 | 3.34 | 2.96 / 2.46 |
> | 5 | Wiener convolve (LR) | 3.22 | 0.12 | +3.16 | 53.65 | 3.72 / 1.00 |
> | 6 | forward transform | 6.07 | 6.87 | +2.61 | 1.75 | 3.14 / 2.65 |
> | 7 | block syntax packing | 3.25 | 1.78 | +2.36 | 3.62 | **1.76 / 3.33** |
> | 8 | RDOQ optimize_b | 16.72 | 30.14 | +1.51 | 1.10 | 3.66 / 2.97 |
> | — | MD quantize kernels | 1.37 | 4.70 | −1.01 | 0.58 | 3.71 / 2.87 |
> | — | **coeff rate cost_coeffs_txb** | 5.82 | 16.19 | **−2.35** | 0.71 | 5.02 / 3.34 |
>
> **`cost_coeffs_txb` flips from a loss to the port's biggest win** — the Ir
> ranking listed it at #10 with a 1.18x excess; by cycles it is 0.71x C, because
> the port runs 1.07x C's instructions there at IPC 5.02 against C's 3.34. Do
> not "fix" it. `MD quantize kernels`, `inverse transform` and `residual` are
> also cheaper than C in cycles. The `PD0 quantize core` row is still NOT a
> usable ratio (same self-vs-inclusive artifact the compute_stats recheck named).
>
> **`encode_block_syntax` is the ONLY genuine stall item in the port** — the
> only pair whose IPC is materially below C's (1.76 vs 3.33; per-function 1.50
> against the port's 3.44 frame average), 1.60 % of the frame recoverable at
> the port's own average IPC, and 8.07 % of the port's branch misses. Its root
> is the `pipeline.rs:9333` scan above. Content-dependent: at screen_terminal
> p6 the same function runs at IPC 3.43, i.e. at the frame average.
>
> **Safe-Rust cost, localised rather than guessed.** `objdump` of the release
> binary with GOT-indirect call targets resolved through `.rela.dyn` (a first
> pass that read only direct `call <symbol>` found ZERO bounds checks and was
> WRONG — the panic calls go through the GOT) counts **2,042
> `panic_bounds_check` sites, 721 slice-index-fail, 171 `unwrap_failed`, 665
> memcpy, 3,964 allocator** in 574,540 instructions of `.text`. The two biggest
> cycle consumers carry the densest: `quant::optimize_b::{closure#0}` has **72
> bounds checks in 4,721 instructions** (and is 16.35 % of the p6 frame, 27.96 %
> of the p2 frame); `coeff_rate::cost_coeffs_txb_inner::{closure#0}` has 15 in
> 274. `fdct64`/`idct64`/`fdct32`/`idct32` carry 66/69/34/36 — the
> `try_into::<&[T; N]>()`-at-the-boundary pattern applies to all four.
> Aggregate signature: the port executes one branch every 6.1 instructions
> against C's one every 9.1-10.9, and 3.0-4.1x C's absolute branch count.
> Inlined `core` library source carries **317,696,743 Ir — 19.6 % of the p6
> frame** — the largest single lines being `Ord::min` (`core/src/cmp.rs:1077`,
> 3.60 %, compiled as a BRANCH not a `cmov`), `PartialOrd::lt/le` (2.80 %),
> `checked_sub`'s `Option` test (2.68 %) and `Range::spec_next` (2.54 %).
> C does the same work; it is simply attributed to C's own lines. Two port
> kernels are outright scalar where C is not:
> `svtav1-dsp/src/restoration.rs:215-221` (the Wiener 7-tap trial filter, vs
> C's `..._avx512`) and `svtav1-dsp/src/variance.rs:212-219` (`sse`, 21.4 M of
> `__arcane_sse_impl_v3`'s 27.9 M self Ir on one scalar line).
>
> **A thing the Ir campaign structurally could not see: valgrind masks AVX-512,
> so every callgrind number for C is C's AVX2 arm — and on this host C RUNS
> AVX-512** (`svt_spatial_full_distortion_kernel_avx512`,
> `svt_av1_fwd_txfm2d_16x16_avx512`, `fdct32x32_avx512`, `av1_fdct64_new_avx512`,
> `svt_av1_txb_init_levels_avx512`, `compute_stats_win5_avx512`,
> `svt_av1_wiener_convolve_add_src_avx512`). The port executes NO AVX-512 kernel
> of its own. Net effect on totals is small (C's hardware instruction count is
> 5.4 % below its callgrind Ir at p6, the port's 4.8 % below), so the Ir ranking
> is not badly distorted overall — but per kernel it can be, and an AVX-512
> archmage tier is unmeasured headroom no Ir number in this campaign can price.
> The archmage dispatch boundary is NOT a stall site: the `__arcane_*_impl_v3`
> kernels run at IPC 3.13-4.97, at or above the frame average; the out-of-line
> dispatchers cost 0.67 % of the p6 frame in total.
>
> NOT MEASURED: the Zen 4 mispredict penalty in cycles (so no "branch misses
> cost N %" figure appears anywhere above — only counts and rates); the split
> of the port's excess branch instructions between bounds checks and loop
> control; any A/B of a proposed fix (measure-only chunk); aarch64; 10-bit;
> inter; presets other than 2 and 6; qp other than 40.


> **LOOP RESTORATION IS NO LONGER THE p6 LEVER — MEASURED, AND
> `callcount_realimg_2026-09-04` ITEM D IS AMENDED IN PLACE (2026-09-05,
> measure-only).** Record
> `benchmarks/compute_stats_x86_recheck_2026-09-05.{tsv,cells.tsv,meta}`.
> Item D's headline — Wiener `compute_stats` **127x C per call** and
> `search_restoration_still_bd` **33.0 / 47.1 / 55.2 / 49.2 %** of the port's
> preset-6 frame on photo_cid / screen_terminal / lineart_graph / gradient —
> predates the C-shape rewrite (`2d9262178e72`) and had never been re-checked
> on the four contents. Re-run on `origin/main` @ `4c6b5df90`, r7900x, the
> same prebuilt `3115c0c1` oracle, the same `callcount_cells.sh` driver, the
> same four `.yuv` files (sha256 re-verified), all four **byte-identical on
> both checks**: compute_stats is **7,924,744 Ir for the same six calls =
> 1.349x C** (per call 1,320,791 vs 978,955; the executing arm is
> `__arcane_compute_stats_impl_v3`, the AVX2 one, and C's live kernel is
> `compute_stats_win5_avx2.constprop.0` — no `win7` symbol in any cell), which
> reproduces the cshape record's 7,925,038 to 0.004 %. The LR search is
> **3.21 / 5.67 / 7.69 / 6.04 %** of the port's p6 frame against C's unmoved
> `restoration_seg_search` at 1.16 / 2.29 / 3.32 / 2.60 % — **6.38x**, the
> figure the cshape record predicted would be left. p6 port/C is **2.295 /
> 2.591 / 2.747 / 2.745** (item D: 3.395 / 4.723 / 5.794 / 5.120), agreeing
> to three digits with the CFL banner's independently-measured 2.296 / 2.591 /
> 2.745. **Item D's counterfactual was right and is now spent:** the ratio
> landed where it said removing the stage would put it (2.295 vs its 2.27,
> 2.591 vs its 2.50), and removing the stage today buys only 2.247 / 2.502 /
> 2.622 / 2.648. What is LEFT in LR is a different kernel one order of
> magnitude smaller: the trial filter `try_restoration_unit` **37.5 M vs C's
> `try_restoration_unit_seg` 1.34 M = 28x** on all four contents, whose kernel
> `restoration::wiener_convolve_add_src` is **57.9 M self vs C's
> `svt_av1_wiener_convolve_add_src_avx2` 1.1 M = 52.8x** (17,216 port calls
> vs 216) — same shape of finding compute_stats was; plus a **port-only**
> `apply_restoration_frame` pass (27.1 M = 1.67 % on photo_cid, 2.94 % on the
> screenshot, 0 on the gradient) for which C's profile has no symbol at all on
> these cells (named as the next thing to read in C, NOT diagnosed). **The
> self-cost ranking at photo_cid p6 now** (exclusive Ir, rows do not nest;
> port / C / x / % of the port's frame): CDEF filter block 87.6 M / 10.0 M /
> 8.73x / 4.79 %, memset 78.1 M / 6.3 M / 12.5x / 4.44 %, entropy
> `write_coeffs_txb_1d` 87.8 M / 16.4 M / 5.34x / 4.41 %, the LR Wiener
> convolve 57.9 M / 1.1 M / 52.8x / 3.51 %, RDOQ `optimize_b` 260.2 M /
> 203.7 M / 1.28x / 3.49 %, `pd0::tx_quant_core` 40.3 M / 0.44 M / 2.46 %
> (its honest ratio is the INCLUSIVE 12.3x, not the self 91.6x — C's callees
> are named where the port's are inlined), distortion 51.7 M / 19.8 M /
> 2.61x / 1.97 %, nz-map contexts 4.12x / 1.86 %, `fill_levels` 6.48x /
> 1.85 %, and **`compute_stats` last at 1.7 M excess = 0.11 %**. Two traps
> recorded there: the memset row's largest caller is an unnamed frame under
> the allocator (54.4 M of its 78.1 M), so the memset and calloc rows are the
> same bytes twice; and the join's `estimate_transform` vs `tx_unit_inner`
> edge is a COUNT edge whose two sides have different scope, so its 743.5 M
> vs 63.5 M is NOT a like-for-like ratio. p2, p10, the CLIC cells, wall clock
> and aarch64 were not measured.

> **CFL PREDICT IS BRANCH-FREE AND THE ALPHA SEARCH IS 1.99x -> 1.72x C's Ir —
> AND THE `#[arcane]` DISPATCH VARIANT HAD FEWER INSTRUCTIONS AND WAS SLOWER
> EVERYWHERE (2026-09-05).** Record `benchmarks/cfl_branchfree_2026-09-05.{tsv,meta}`.
> `callcount_realimg_2026-09-04` item E: `intra_pred::cfl_predict_lbd` and C's
> `svt_cfl_predict_lbd_avx2` (`ASM_AVX2/cfl_avx2.c:38`) are called the same
> number of times (253,229 vs 253,259 at photo_cid p2) and the port's cost
> 1,207 Ir per call against C's 99 — **12x**. It was a scalar double loop whose
> per-element body BRANCHED on the sign for C's round-half-away-from-zero. It is
> now the same arithmetic branch-free (`s = q6 >> 31`, `|q6| = (q6 ^ s) - s`),
> with per-row slices so the bounds leave the inner loop; the identity holds for
> every `i32` but `i32::MIN` and `q6` is bounded by `16 * 32768` (C
> `cfl_idx_to_alpha`), **pinned EXHAUSTIVELY over all 2,162,720
> (`ac_q3`, `alpha_q3`) pairs**, not sampled. `cfl::md_cfl_rd_pick_alpha`
> inclusive 886.7 M -> 766.7 M against C's `av1_cost_calc_cfl` 444.5 M
> (1.995x -> 1.725x); frame Ir photo_clic -1.03 % (port/C 1.727 -> 1.709),
> photo_cid -0.31 % (1.694 -> 1.688), gradient unchanged (-0.003 %). Wall clock
> (21 interleaved paired rounds, ident=Y): **CLIC photo 512 p2 1.016x**, CID
> photo 1.009x. **THE VARIANT THAT WAS REJECTED, AND WHY IT MATTERS:** wrapping
> the same core in `incant!(..., [v3, neon, scalar])` — this crate's normal
> idiom — gave **fewer instructions on every cell** (photo_cid -0.45 % vs
> -0.31 %, the CFL subtree 713.9 M vs 766.7 M) and was **slower on all four**,
> including a real regression on the gradient (0.993x at 256 AND 512 p2, spans
> entirely above 1.0). `cfl_predict_lbd` is called from inside the per-alpha
> closure; an `#[arcane]` arm is an out-of-line `target_feature` function, so
> the dispatch takes the call out of the inliner's reach and on small blocks the
> call overhead is all that is left. **Ir is what this campaign ranks by, not
> what it decides by** — take the wall clock before keeping a SIMD arm. The
> untried next step is a hand-written intrinsics kernel that the inliner can
> still fold into the alpha closure; the rejected variant is the evidence that
> it must be inlinable to win.
>
> **ALSO MEASURED AND REVERTED, so nobody retries it:** `#[inline]` on
> `palette::palette_color_index_context`, the last item-F helper (2,961,716
> calls / 211.6 M Ir = 1.5 % of a SCREENSHOT's p2). The attribute takes — the
> symbol goes and the screen cell's Ir reads -0.22 % (port/C 2.205 -> 2.200) —
> but the wall clock is a wash with a sign flip by content, 21 paired rounds
> each: `screen_terminal` 512 p2 **1.003x** (span 0.9951-0.9982) against
> `photo_cid` 512 p2 **0.999x** (span 0.9998-1.0032, entirely at or above 1.0).
> That 1.5 % is the function's own WORK (~71 Ir a call), not call overhead:
> making it faster means changing the algorithm, not the linkage.
> **INCIDENTAL, AND THE MOST USEFUL LINE HERE: both open screen-content
> correctness cells of `callcount_realimg_2026-09-04` are now BYTE-IDENTICAL to
> C.** That record's open finding was `screen_terminal` 512x512 p2 qp40 (port
> 4,991 vs C 5,003) and `lineart_graph` 512x480 p2 qp40 (3,087 vs 3,098), both
> with IntraBC live and neither a cell any screen gate runs. Re-measured with
> the same driver and oracle: **5,003 B = 5,003 B** and **3,098 B = 3,098 B**,
> each verified twice. They closed between that record and this one and NOT via
> these four chunks (all byte-inert on 1,100 identity cells) — the credit is the
> screen/IntraBC work that landed on `main` in between. Their port/C ratios read
> 2.200 and 2.385 now; those are NOT a before/after against that record's 2.206
> and 2.444, which were ratios over divergent output and were flagged as
> unquotable there.
>
> **THE FOUR CHUNKS TOGETHER, against the tree they branched from (41e5d8f1),
> 21 interleaved paired rounds, ident=Y on every row:** photo_cid 512 p2
> **1.059x**, the CLIC glitter photo **1.062x**, the gb82-sc terminal crop
> **1.040x**, gradient 256 p2 **1.055x** and 512 p2 **1.042x**, gradient 256 p6
> 1.009x and 512 p6 1.003x. **On the OTHER ISA** (this Mac, gradient, 21 paired
> rounds, box NOT quiet — load 2.79 / 1.80 — so read direction and spans):
> gradient 256 p2 **1.049x**, 512 p2 **1.032x**, 256 p6 1.011x, 512 p6 1.008x,
> and on the SAME CID22 photo the x86 rows use, 512 p2 **1.050x** and p6
> **1.017x** — agreeing with x86's 1.059x / 1.010x on those two cells.
> **A PRESET SWEEP on the photo at 512 names the one cell where the four are
> NEGATIVE**: p6 1.022x, **p10 0.996x (span
> 1.0015-1.0093 — a REAL ~0.4 % slowdown)**, p13 0.996x (span straddles 1.0, a
> null). p10/p13 are the `only_dct` regime where none of the four mechanisms has
> anything to remove, and what they pay is the owned-output wrapper's extra call
> frame; in absolute time it is +0.054 ms on a 9.8 ms frame against -137 ms at
> p2 and -1.6 ms at p6. A SIZE SWEEP at gradient p2 (15 rounds each): 64
> 1.051x, 128 1.051x, 256 1.055x, 512 1.043x, 1024 1.031x — roughly flat in
> frame size, so it is not a per-frame fixed cost.
> Instructions on photo_cid 512 p2 48.43 G -> 46.00 G
> (**-5.02 %**, port/C **1.777 -> 1.688**), p6 1.657 G -> 1.620 G (-2.25 %,
> port/C 2.349 -> 2.296); on the CLIC photo port/C **1.835 -> 1.709**. p10 is
> unmoved (1.803 -> 1.802) and should be — every mechanism these four touched is
> a p2/p6 phenomenon, since the tx-type search collapses to one candidate at p10.
> The port/C position on the other two contents moves the same way: gradient 512
> p2 **2.493 -> 2.408**, p6 2.769 -> 2.745, p10 2.539 -> 2.515; the gb82-sc
> terminal crop p2 **2.294 -> 2.205**, p6 2.643 -> 2.591, p10 2.192 -> 2.188.
> **And the memory axis MOVES THE RIGHT WAY over the four**: x86 2048 inter peak
> RSS, interleaved `mem_bisect.sh` 15 rounds, median 109,332 -> **104,496 KiB**
> (min 108,820 -> 103,680; the distributions do not overlap) — **-4.4 %** — with
> peak heap unchanged throughout (2048 still 61.07 M, inter 101.78 M). It does
> NOT all go one way, and the record says so: the same harness at x86 **1280
> inter** reads 45,792 -> **47,000 KiB (+2.6 %)**, also non-overlapping, and
> 2048 still is a null. Both directions are region-retention effects of a
> changed size mix, not live bytes.

> **THREE OUT-OF-LINE HELPERS C INLINES ARE GONE — 17.7 M CALLS PER 512² PHOTO
> FRAME AT p2, -0.63 % Ir — AND WITH THE TWO BLOCKS BELOW THE PHOTO's p2 PORT/C
> RATIO IS 1.777 -> 1.694 AND THE WHOLE-FRAME WALL CLOCK 1.056x (2026-09-05).**
> Record `benchmarks/txsize_tables_2026-09-05.{tsv,meta}`.
> `callcount_realimg_2026-09-04` item F: `MdRates::txt_rate` 5,974,663 calls /
> 382.3 M Ir, `tx_pipeline::rs_tx_size` 6,984,051 / 167.4 M and
> `coeff_c::tx_size_from_dims` 4,766,423 / 113.0 M — all three now read ZERO.
> **`#[inline]` ALONE FIXES ONE OF THE THREE, and that is the transferable
> finding**: measured as its own step, the attribute inlined `txt_rate`
> (5,974,663 -> 0) and left `rs_tx_size` at 6,984,051 and `tx_size_from_dims` at
> 4,762,229, because LLVM will not inline a 19-arm `match` on `(w, h)` called
> from several sites whatever the attribute says (-0.39 % instead of -0.63 %).
> Both mappings are now a table: `tx_size_from_dims` a 5x5 lookup indexed
> `(log2(w) - 2) * 5 + (log2(h) - 2)` with a sentinel for the six illegal aspect
> ratios, and `rs_tx_size` a const `TX_SIZE_FROM_C[19]` in C's `TX_SIZES_ALL`
> order — which at `tx_unit_inner`'s two dispatch sites is indexed by the `c_tx`
> the function was ALREADY holding, so the second 19-way branch chain per
> transform unit disappears entirely. **Both replaced `match`es are kept in the
> tree as `#[cfg(test)]` oracles** and three new tests assert the tables accept
> exactly the same 19 shapes (by count, so a typo that admits a 20th fails) and
> reject exactly the same six — stronger coverage than any encode grid, which
> never codes most of them. Ir: p2 46.44 G -> 46.15 G (-0.63 %), p6 -0.25 %;
> over the three commits p2 48.43 G -> 46.15 G (**-4.72 %**) and p6 -1.46 %
> (port/C 2.349 -> 2.314). Wall clock (r7900x, 21 interleaved paired rounds,
> ident=Y everywhere) against the previous commit: photo 512 p2 **1.012x**,
> gradient 256 p2 1.007x, 512 p2 1.008x, every p6/p10 cell a null with its span
> straddling 1.0. Against the tree all three chunks branched from: photo 512 p2
> **1.056x**, gradient 256 p2 **1.054x**, 512 p2 **1.042x**, photo 512 p6
> 1.010x, gradient 256 p6 1.012x — and on two more contents at 512 p2, the CLIC
> glitter image **1.054x** and the gb82-sc terminal crop **1.034x**. Peak heap
> identical at every step (this chunk
> allocates nothing). Still out of line and NOT touched:
> `palette::palette_color_index_context`, 2,961,716 calls / 211.6 M Ir = 1.5 %
> of a SCREENSHOT's p2 (C's is `static inline`).

> **THE RESIDUAL IS DERIVED ONCE PER (TX-DEPTH, TXB) AGAIN, AS C DOES IT —
> `residual_i32` 4,256,724 -> 1,307,794 CALLS PER 512² PHOTO FRAME AT p2
> AGAINST C's 1,986,776 (port/C 2.142x -> 0.658x), -2.19 % Ir, AND WITH THE
> BLOCK BELOW THE PHOTO's p2 PORT/C RATIO IS 1.777 -> 1.704 (2026-09-05).**
> Record `benchmarks/residual_hoist_2026-09-05.{tsv,meta}`. C fills
> `cand_bf->residual->y_buffer` ONCE per (tx-depth, TXB) in
> `perform_tx_partitioning` (`svt_aom_residual_kernel`,
> `product_coding_loop.c:5336`) and every tx-type trial transforms THAT buffer
> (`svt_aom_estimate_transform` reads it at `:4730`); `tx_type_search` has no
> call-graph edge into the residual kernel at any preset. The port re-subtracted
> the same `w x h` block once per admitted TRIAL — `callcount_2026-09-04`'s
> HEADLINE #2 and item A of `callcount_realimg_2026-09-04`'s ranked list, where
> the ratio GREW with texture (1.383x gradient, 2.143x this photo, 2.184x a CLIC
> glitter image) because the ratio IS the mean tx-type trial count per TXB.
> `src`, `pred` and the dims are loop-invariant across `txt_search`'s group
> loop, so it now fills one buffer before the loop and hands the same slice to
> every trial through a `pre_residual: Option<&[i32]>` argument; the buffer is a
> third field of the per-thread `TxtScratch` the previous commit introduced, so
> it costs no allocation after warmup. `None` restores the per-call derivation
> for every single-shot site (MDS1, chroma, CfL, the owned-output wrapper) —
> what C's `perform_dct_dct_tx` and `full_loop_uv` do — and the hoist is applied
> only on the multi-candidate (`!only_dct`) path, which also leaves the preset-13
> video/inter arm untouched. **Byte identity is structural** (the same integer
> subtraction of the same two buffers, once instead of k times) and the two
> positive controls confirm no second-order effect: `fwd_txfm2d_dispatch`
> 4,256,724 = 4,256,724 and `optimize_b` 2,854,762 = 2,854,762, so the search
> breadth and the RDOQ work are the same amount of work to the unit. Ir: p2
> 47.48 G -> 46.44 G (-2.19 %), p6 1.647 G -> 1.637 G (-0.64 %); over the two
> commits together p2 48.43 G -> 46.44 G (**-4.12 %**, port/C 1.777 -> 1.704)
> and p6 -1.22 % (2.349 -> 2.320). Wall clock (r7900x, `perf_ab.sh`, 15
> interleaved paired rounds, ident=Y everywhere) against the previous commit:
> photo 512 p2 **1.016x**, gradient 256 p2 **1.029x**, 512 p2 **1.023x**, both
> p6 cells 1.003-1.006x, the two p10 cells null (they are the untouched
> `only_dct` path); against the tree both chunks branched from, photo 512 p2
> **1.042x** and p6 1.009x. Note the two mechanisms have OPPOSITE content
> dependence — the allocator gain is bigger on the photo, the residual gain is
> bigger on the gradient — which is why both were measured on both. Peak HEAP is
> IDENTICAL at every step (x86 2048 still 61.07 M, inter 101.78 M); the one new
> retained buffer is at most 16 KiB per thread and preset 13 never touches it.
> Peak HEAP is IDENTICAL at every step and the aarch64 inter 2048 cell —
> interleaved `mem_bisect.sh`, all three binaries round-robin in one run, 15
> rounds — goes 142,112 -> 142,144 -> **140,080** KiB median (min 140,144 ->
> 140,144 -> 139,968): this chunk is the first of the two to move that cell at
> all, and it moves it DOWN. Gates green on both ISAs (aarch64 nextest
> 2530/2530, spotcheck 104/104, `identity_full_8bit` 1100/1100, `inter_byte_gate`
> PASS 96/0/1, `video_key_matrix` 59/60 unmoved, `fctx_gate` 96/96,
> `inter_decode_gate` 5/5, decode census 96/96, `SCAN_GATE=1` scan 64/0/0,
> `screen_palette_gate` 50/50; x86_64 nextest 2540/2540, spotcheck 104/104,
> identity 1100/1100, inter byte gate PASS).
> NOT touched: the bd10 twin `tx_unit_hbd_screened` still derives its own
> residual per trial.

> **`tx_unit`'s TWO OUTPUT BUFFERS ARE IN C's SHAPE — 24.17 M ALLOCATOR CALLS
> PER 512² PHOTO FRAME AT p2 -> 15.14 M (9,296x C -> 5,824x), -1.97 % Ir, AND
> THE FIRST ALLOCATION REMOVAL IN THIS CAMPAIGN TO CONVERT: 1.026x FASTER ON A
> TEXTURED PHOTO AT 512 p2 (2026-09-05).** Record
> `benchmarks/txout_cshape_2026-09-05.{tsv,meta}`. `callcount_realimg_2026-09-04`
> ranked the allocator #1 (~4.4 % of the port's photo p2 Ir) and named
> `tx_unit_inner`'s 6.6 M `calloc`s as its biggest caller: a `vec![0i32; pw*ph]`
> + `vec![0u8; w*h]` per (candidate x tx-type x transform unit) TRIAL, returned
> by value. C allocates the same two buffers ONCE PER ENCODER THREAD in
> `svt_aom_mode_decision_context_ctor` (`md_process.c:214`, from
> `enc_dec_process.c:108`) as `TX_TYPES`-deep pools (`:585-596`,
> `ctx->{recon,quant_coeff}_ptr[txt_itr]` at `:597-601`), the tx-type search
> selects a slot by index (`product_coding_loop.c:4723-4725`), keeps the winner
> by index (`best_tx_type`, `:4949`), and copies ONCE per transform unit
> (`copy_txt_data`, `:5082-5084`) — which is why C's whole-encode traffic is
> ~2,600 calls at every preset on every content. The port now does the same:
> `tx_unit_screened_into(.., out: &mut TxOutBufs)` writes into caller-owned
> buffers and returns a `TxUnitMeta` of scalars + each buffer's VALID PREFIX
> LENGTH; `txt_search` holds two `TxOutBufs` per thread and `mem::swap`s on a
> new best (C's `best_tx_type`); the winner is materialised once per TXB
> (measured 992,510 mallocs against 2 x C's 496,519 `perform_tx_partitioning`).
> **Neither buffer is re-zeroed** — every quantizer path defines all `pw*ph`
> `qcoeff` positions (`quant_coding.rs:100-107`/`:388-394` write both arms; the
> QM paths `fill(0)` themselves at `qm.rs:134`/`:195`) and every recon path all
> `w*h` — which is the half of the old `calloc` that was worth real time.
> Counts at photo_cid 512 p2 (base -> cand, C): calloc 9,454,380 -> 3,948,389
> (781), malloc 2,630,202 -> 3,622,712 (410), free 12,084,581 -> 7,571,100
> (1,409); memset 18.20 M -> 18.03 M. Ir p2 48.43 G -> 47.48 G (port/C
> **1.777 -> 1.742**), p6 1.657 G -> 1.647 G (2.349 -> 2.335). Wall clock
> (r7900x, `perf_ab.sh`, 15 interleaved paired rounds, ident=Y everywhere):
> photo 512 p2 **1.026x** (span 0.9718-0.9766), photo 512 p6 1.003x, gradient
> 256 p2 1.018x, 512 p2 1.012x — and gradient p6/p10 are null-to-marginally
> negative (256 p10 is a real ~0.7 % slowdown of a 1.39 ms frame; the four fast
> cells take the unchanged owned path and pay only the wrapper's call frame).
> **THE SPLIT IS DRAWN ON `only_dct`, NOT ON C's `tx_type == DCT_DCT`**, because
> both sides of C's select are themselves pooled (`cand_pred_pool` et al.,
> `md_process.c:640-655`) and the port's owned output is a real allocation: a
> one-candidate search has no loser to save an allocation on. **Four variants
> were measured and REJECTED, two of them for memory alone** (record §6):
> pooling every trial is -2.00 % Ir but **+5.8 % aarch64 inter peak RSS at 2048**
> (142,112 -> 150,336 KiB median, non-overlapping, reproduced at threads=1 and
> auto); pre-sizing the buffers in the CALLER is identical arithmetic and still
> **+4.4 %** (142,128 -> 148,352) — **the allocation POINT moves macOS RSS, not
> just the size**, so `grown_out` fills an empty buffer with `vec![0; n]` at the
> original program point; C's literal DCT_DCT select leaves 0.55 pp of Ir behind;
> `resize`-on-empty costs +0.88 M memsets. **Memory as landed is unmoved on both
> quantities and both ISAs**: aarch64 inter 2048 interleaved `mem_bisect.sh` 21
> rounds 142,128 -> 142,144 KiB median (min 140,160 -> 140,112, max 160,464 ->
> 142,384), x86 peak heap 2048 still 61.07 -> 61.07 M and inter 101.78 -> 101.78
> M. What is LEFT of the allocator (~2.0 % of photo p2 Ir): `drop_glue::<Cand>`
> 1.35 M frees + `eval_candidate`'s 0.52 M callocs / 0.64 M mallocs (C's
> `cand_bf_ptr_array` + the three candidate pools, `md_process.c:640-655` — the
> largest coherent block left, ~3.9 M calls / ~370 M Ir), `hadamard_satd`
> 0.95 M + 0.95 M, `extract_neighbors_tiled` 0.96 M mallocs,
> `inject_candidates` 0.47 M callocs — the middle two already have measured
> NEGATIVE results against them (`hadscratch_null`, the neighbours hoist) and
> must not be re-attempted in the shape that failed.

> **THE POSITION, RE-MEASURED ON THE TIP AFTER TEN LANDINGS — ALL THREE ARMS
> MOVED, AND SO DID THE C ORACLE (2026-09-05).** Records
> `benchmarks/perf_2026-09-05-arm10-{still,videokey,inter}.*`,
> `perf_2026-09-05-arm10-photo-{still,videokey,inter}.*`,
> `perf_2026-09-05-arm10-POSITION.meta`, quiet-box evidence in
> `perf_2026-09-05-arm10-quiet.txt`. Tree `41e5d8f1`, mac (M4 Pro, 12 cores),
> 25 interleaved paired rounds, preset 8, gradient qp 40, no
> `-C target-cpu=native`, **every gradient cell of all three arms ident=Y**,
> zero ERR. **This supersedes the arm9 table below.**
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio | intercept (port / C) |
> |---|---|---|---|---|---|---|
> | still | 0.758x | 1.287x | 2.011x | 2.208x | **2.29x** | -0.005 ms / 0.130 ms |
> | videokey | 1.176x | 1.724x | 2.320x | 2.228x | **2.26x** | -0.414 ms / -0.044 ms |
> | inter | 1.441x | 1.899x | 2.425x | 2.432x | **2.46x** | -0.441 ms / 0.054 ms |
>
> Against arm9 (2.46x / 2.60x / 2.82x) that is **-6.9 % / -13.1 % / -12.8 %**
> on the slope ratio, and ten of the twelve cells moved by more than the ~3 %
> session-drift band arm9 measured (the exceptions are videokey 256 at -3.3 %
> and inter 256 at -2.5 %, which are inside it and are not results). **The
> standing caveat is unchanged: a position run cannot resolve a chunk-sized
> change — it resolves the CUMULATIVE change since arm9**, which is what this
> one is for. Nothing here attributes anything to a particular landing; only a
> paired A/B of two trees in one session can do that, and none was run.
>
> **THE INTERCEPT IS A SEPARATE STORY AND IT IS WHY 64x64 READS 0.758x.** On
> the still arm C carries a real fixed per-frame cost (0.130 ms) where the
> port's fit intercept is ~0, so **the port is FASTER than C at 64x64 still**
> and a single "2.29x" hides that. On the two video arms both intercepts come
> out slightly negative — the fit is not resolving a fixed cost across these
> four sizes there, so the 64x64 cell itself (1.18x / 1.44x) is the honest
> small-frame statement, not the intercept.
>
> **C DRIFTED 4-10 % FASTER ON AN UNCHANGED `libSvtAv1Enc.a`** (the prebuilt
> 2026-08-31 oracle, never rebuilt; only the ~200-line driver was recompiled,
> and it does no encoding). C slopes 15.0572 -> 14.0162 (-6.9 %), 62.0431 ->
> 59.4884 (-4.1 %), 74.5633 -> 71.6416 (-3.9 %). So the port's absolute
> -9 to -18 % per cell is **an upper bound on what the landings bought, not a
> measurement of it** — some of it is whatever made the box faster this
> session. Read the ratios; the milliseconds are not comparable across
> sessions on either side.
>
> **REAL CONTENT, and it confirms gradient OVERSTATES the gap.** photo_cid
> (CID22-512 `3571065.png`, native 512x512, sha256(.yuv)
> `88142a48…3ca4a72f` — the same bytes `callcount_realimg_2026-09-04` used),
> 512x512, n=25:
>
> | photo_cid 512, port/C | p2 | p6 |
> |---|---|---|
> | still | **1.634x** | **2.101x** |
> | videokey | (1.571x, ident=**N** — excluded) | **1.559x** |
> | inter | **REFUSED** (no timing) | **1.575x** |
>
> One size, so **no intercept/slope fit exists on this arm** and nothing is
> extrapolated. Every comparable cell is better than the synthetic one, and the
> video arms *invert* on real content: photo p6 videokey (1.559x) and inter
> (1.575x) beat photo p6 still (2.101x), where on gradient the video arms are
> worse than still.
>
> Two photo cells are NOT results. (1) **`photo_cid 512 p2 videokey is
> ident=N`** — the port's video-mode key frame at p2 on this image is not
> byte-identical to C's, so its ratio compares different work. The same image
> at p2 in STILL mode is identical and p6 videokey is identical, so it is
> specific to (video-mode key frame, preset 2, real content); a byte finding
> for whoever owns that axis, not investigated here. (2) **`photo_cid 512 p2
> inter` produces no timing at all**, 25/25 rounds — a clean REFUSAL (exit 3),
> not a crash: *"global motion is not implemented: C `svt_aom_derive_gm_level`
> gives an inter frame at preset <= 4 a non-zero gm_level … use preset >= 5
> for inter frames"*. There is no p2 inter number to have, on any content,
> until global motion lands.
>
> **QUIET-BOX DISCIPLINE, and it fired once.** A sibling lane (`alloc1`) was
> A/B-ing on the same mac. The first `photo-inter` attempt caught its `rustc`,
> `perf_encode_cand` and `perf_encode_base` on the box; that attempt was
> **discarded, not published**, and the arm was re-run on a clean box. Both
> logs — the discarded one and the clean one — are in
> `perf_2026-09-05-arm10-quiet.txt` alongside a snapshot every 15 s inside
> every other arm.

> **WIENER `compute_stats` IS IN C's SHAPE ON BOTH ISAs — 127x C PER CALL ->
> 1.35x, 746.5 M -> 7.9 M Ir per 512x512 frame, byte-identical; the p6 port/C
> ratio on every cell drops from 3.4-5.1x to 2.3-2.8x (2026-09-04).** Record
> `benchmarks/compute_stats_cshape_2026-09-04.{tsv,meta}`. The realimg record's
> one content-independent kernel (D. below) was the port's per-pixel form:
> gather the `win x win` window with `win2` strided scalar loads, run
> `win2 + 1` short i32 multiply-accumulate loops over a `win2 x win2` i32
> scratch, flush per row — 350 products and ~1,900 Ir per pixel at `win = 5`
> against C's 66 products and ~15 Ir. It is now C's six-step kernel
> (`compute_stats_win{5,7}_avx2`, `ASM_AVX2/pickrst_avx2.c:775/:1546`, the
> NEON twin `pickrst_neon.c:147/:698`): full `madd` dots for `M` and the
> first block row and column of `H` (steps 1-2), every other `H` entry
> derived from a neighbour by an exact O(height) column-shift delta (3-4) or
> O(width) row-shift delta (5-6). One `macro_rules! cs_kernel` body over
> seven per-ISA `#[rite]` lane primitives (load / AND / zero / pairwise
> madd / msub / i64 reduce / mask), instantiated per ISA and per window; the
> column deltas run over transposed edge strips instead of C's scalar
> `_mm256_insert_epi16` gathers; steps 1-2 drain i32 -> i64 every
> `floor(i32::MAX / (chunks * 130050))` rows under a `CS_MAX_DIM = 32000`
> structural guard, so every accumulator bound is computed. Not
> `#[magetypes]`: the shape needs a pairwise widening `i16 x i16 -> i32`
> multiply-accumulate (`_mm256_madd_epi16` / `vmlal_s16`) that magetypes has
> at no version — the named missing primitive is
> `i16x16::madd_adjacent(i16x16) -> i32x8` (+ `msub`). Iterations on
> photo_cid p6 (r7900x callgrind, all byte-identical): 746.55 M -> 18.49 M
> (C-shape, per-load `[..16]` checks) -> 16.88 M (chunk slices via
> `core::array::from_fn`, which is NOT inlined and hides the lengths — one
> `cmp; je` per chunk per row survived) -> 8.37 M (views built with plain
> loops; steps 3-4 as two passes so W = 7 stops spilling) -> 7.93 M (SIMD
> `find_average`). Frame totals: gradient p2 2.660x -> 2.493x, p6 5.120x ->
> **2.766x**; photo_cid p2 1.805x -> 1.777x, p6 3.395x -> **2.349x**; the LR
> search's inclusive Ir at p6 790.6 M -> 52.0 M against C's 8.15 M (the rest
> is the tap refinement's convolve, the SGR search and `compute_score`, not
> touched). aarch64 kernel bench (`benches/kernel_tiers.rs`, shared box, read
> the within-run ratio to the unchanged scalar): win5 64x64 10.9x -> 24.8x,
> win7 10.1x -> 26.3x over scalar — the row-pair arm of 2026-09-03 is
> replaced by the same body. Gates: see the record's GATES section
> (aarch64 chain + cross-ISA on r7900x). Wall clock: paired A/B on r7900x in
> the record; the Mac was not quiet.

> **CALL COUNTS RE-COMPARED ON REAL CONTENT (2026-09-04, measure-only): the
> three CPU fixes HOLD on photos, a screenshot and line art — no MD-search
> edge that read ~1.0x on the gradient reads >1.5x on textured content.**
> Record `benchmarks/callcount_realimg_2026-09-04.{tsv,cells.tsv,meta}`;
> driver `tools/perf_profile/callcount_cells.sh` + `callcount_join.py` +
> `tree_callers.py` (new); `perf_encode` gained a `raw:<i420.yuv>` input so
> a corpus image joins the synthetic cells. Six contents (CID22-512 3571065,
> a CLIC 2025 glitter photo Mitchell-resized to 512x512 and to 500x332,
> gb82-sc terminal.png and graph.png crops, the gradient control) x presets
> 2/6/10 at qp 40 on r7900x, 16/18 cells byte-identical. Positive controls
> exact everywhere (SB count, `md_encode_block` = `evaluate_leaf`,
> `full_loop_core` = `eval_candidate` + MDS1 quantize_b commits, PD0
> recursion, MDS0 hadamards). The tx-type promotion: total forward
> transforms and quantizer calls port/C 0.998 / 0.993 (CID), 1.015 / 1.013
> (CLIC) where C admits 74-80 % of its trials on photos vs 55 % on the
> gradient. `run_mds1` = 0 at p10 on all six contents. What real content
> DID change: the per-trial residual 1.38x -> 2.14x (~1.6 % of the port's
> p2 Ir on a photo; C derives it once per TXB at product_coding_loop.c:5337,
> the port once per trial at tx_pipeline.rs:508); the allocator 1,339x ->
> 9,292x C (24.2 M calls; ~3.6 % + 0.9 % memset of the port's p2 Ir; sites
> named — `tx_unit_inner` 6.6 M callocs, `mds3::eval_candidate` 6.2 M
> frees + 0.84 M mallocs, `hadamard_satd` 0.95 M, `extract_neighbors_tiled`
> 0.96 M mallocs); CDEF `cdef_find_dir` run twice per 8x8 (search + apply,
> C caches via `dirinit`), 2x by count, 0.04-0.7 %. Two kernels are 1:1 by
> count and far off per call: `cfl_predict_lbd` 12x C's AVX2 per call
> (2.0x the whole CFL alpha search's Ir on CID, 6.3 % of the port's CLIC
> p2), and Wiener `compute_stats` **127x per call — 746.5 M Ir on EVERY
> 512x512 cell including the gradient, 33-55 % of the port's p6 total on
> every content** (C: 5.9 M); with it removed p6 is 2.3-2.5x, not
> 3.4-5.8x. **[AMENDED 2026-09-05: that compute_stats sentence is STALE —
> it is 1.349x C per call and the LR search is 3.2-7.7 % of p6 now; see the
> banner at the top of this file and the amendment in that record. The rest
> of this banner is un-rechecked and stands.]** The gradient OVERSTATES p2/p10 (2.66x / 2.52x vs 1.80x / 1.80x
> on photos). Corrections: the 2026-09-04 record's "inverse-transform 2.03x,
> not converging" was a misjoin against the ssse3 SUB-dispatch — against
> C's `svt_aom_inv_transform_recon8bit` the port is at or BELOW C at every
> cell; `md_stage_0` is per class, not per leaf. **Open correctness
> finding:** `screen_terminal` and `lineart_graph` at p2 qp 40 are NOT
> byte-identical (4991 vs 5003 B, 3087 vs 3098 B; identical at p6/p10;
> IntraBC live) — not a cell any screen gate runs (they use qp 20/48).
> Wall clock NOT measured.

> **THE aarch64 INTER MEMORY AXIS IS BACK INSIDE THE GOAL — 1.311x -> 1.189x AT
> 2048 AND 1.263x -> 1.042x AT 1280 — AND THE REGRESSION WAS THREE COPIES OF THE
> REFERENCE PICTURE, NOT ALLOCATOR POLICY (2026-09-04).** Records
> `benchmarks/mem_refclone_2026-09-04.{tsv,meta}`; harness
> `tools/mem_bisect.sh` (new: N per-commit binaries round-robin on one cell,
> so the inter arm's 8-15 % upward thread-timing spread lands on every binary
> equally; read the MIN there). Seventeen binaries, one per commit of
> `940b855a..a1a32ca2f`, built at each sha on BOTH hosts: the rise is
> `4e29d8fa7` (+8.83 M x86 peak heap / +9.9 MB x86 RSS / +10.9 MB mac min at
> 2048) and `8fa2d0353` (+1.58 M / +1.7 MB), every other commit a null on
> every quantity — the level scratch included, which reads null on main's
> history where `mem_levelscratch_2026-09-03.meta` read +4.1 MiB on its own
> branch pair (both stated in the record; not reconciled). heaptrack's
> peak-instant contributors on the tip: `DecodedPictureBuffer::refresh`
> 27.69 M (`Arc::new(frame.clone())` of a `&ReferenceFrame` whose owner was
> still alive — the stored picture existed twice), plus `rf.y_plane.clone()`
> 4.19 M and `rf.padded.clone()` 7.15 M per inter frame (the LAST reference
> existed three times). Fix: `refresh` by value, and the frame holds the
> slot's own `Arc` (`get_shared`). Interleaved A/B, `.obu` identical on every
> row: mac 156624 -> 142032 KiB (2048), 66944 -> 55216 (1280); x86 RSS
> 126596 -> 109220 / 54348 -> 45328; x86 peak heap 123.13 -> 101.78 M /
> 49.80 -> 40.90 M. C re-measured flat (mac 52992 / 119440), oracle not
> rebuilt. **Correction to the brief this ran under: x86 RSS was 1.185x on
> the tip, not 1.086x — the regression was on both ISAs; what is
> aarch64-only is the EXCESS over x86**, now decomposed by `vmmap -summary`
> at the peak sample: ~60 MiB of libmalloc regions holding NO live block
> (LARGE-empty 31.1 M, SMALL-empty 28.6 M after the fix; C has none) that
> macOS keeps resident and `/usr/bin/time -l` counts. The live small-zone
> bytes underneath are the per-block coefficient/recon retention C also has
> (`funnel_block_decision` 16.80 M, `tx_unit_inner` 10.49 M at the x86
> peak); shrinking those is a per-block SIZE change with a byte-identity
> surface and is the next lever if the aarch64 margin is ever needed.
> **The still arm moves too** (`refresh` cloned the KEY frame the same way):
> canonical `mem_peak.sh` on the candidate, mac, median of 5 — 1280 still
> 35792 -> 28256 KiB (0.788x -> 0.623x of C), videokey 41984 -> 33664
> (0.883x -> 0.708x); 2048 still 81520 -> 72624 (0.851x -> 0.758x), videokey
> 96192 -> 87856 (0.901x -> 0.788x); `.obu` identical to C on every row.
> Gates: `regression_spotcheck` 102/102, `inter_byte_gate` PASS (96 required, 0 failed, 1 known-open), `video_key_matrix` 58/60 (unmoved), `fctx_gate` 96/96 fields on the reference cell and 96/97 cells over the inter grid (the one failure is the known-open `diag 128 128 20 8`, whose byte-different tile cannot save C's CDFs), `inter_decode_gate` 5/5, decode census PASS, completion scan (`SCAN_GATE=1`) 64 OK / 0 REFUSED / 0 CRASH, six still cells identical at 290/839/63/171/580/693 B, nextest 2526/2526, `identity_full_8bit` 1100/1100 — all on aarch64 (bash 5); re-run on the PUSHED commit `8fed0aa15` after two rebases (`2b1a74edb` mv_err_cost, `e0275930a` dsp inv-txfm): spotcheck 102/102, `inter_byte_gate` PASS, `identity_full_8bit` 1100/1100; cross-ISA on r7900x: spotcheck 102/102, `inter_byte_gate` PASS, nextest 2536/2536, `identity_full_8bit` 1100/1100.

> **THE p10 "2.307x MDS3 CANDIDATES" FINDING IS CLOSED — IT WAS A MISJOINED
> EDGE, AND THE REAL p10 EXCESS WAS THE MDS1 FULL LOOP (2026-09-04, landed).**
> C's `full_loop_core` (product_coding_loop.c:6890-6910) routes each MDS3
> candidate to `perform_dct_dct_tx` (TXS+TXT off: 502 at this cell) OR
> `perform_tx_partitioning` (384); the record below matched the port's
> `eval_candidate` 886 against the 384 alone. C's MDS3 count is
> `full_loop_core` = 886 = the port's, and the whole-frame admission join
> (`SVT_FULLCOST_XY=all` + `tools/perf_profile/mds3_admission_join.py`)
> shows identical admitted sets on 886/886 blocks. What the join ALSO showed:
> C's `perform_mds1` is 0 on all 886 leaves (`enable_skipping_mds1`, nic
> levels 8..=11, `:7879`) while the port ran `run_mds1` on every one — the
> "positive control" below (`run_mds1 -> tx_unit` 886 = C's quantize 886)
> was a coincidence of counts, not a join: C's 886 commits are its MDS3 pass,
> the port's were its MDS1 pass with 886 more at MDS3 (3,768 total). Fixed:
> the funnel honours the flag; `run_mds1` 886 -> 0, `tx_unit_inner` 3,768 ->
> 2,882 = C's `svt_aom_quantize_inv_quantize` 2,882 exactly, p10 total
> 176.6 M -> 159.3 M Ir (-9.8 %, port/C 2.814x -> 2.539x), byte-identical.
> Wall clock NOT measured. Record: `benchmarks/callcount_mds1skip_2026-09-04.
> {tsv,meta}`, `docs/INTER-ENCODE-PLAN.md` §1z³⁹.
> **CALL-COUNT ATTRIBUTION FOUND THE REPEATED WORK — TWO HEADLINE MECHANISMS,
> BOTH SOURCE-LOCATED, ON r7900x/callgrind (2026-09-04).** Records
> `benchmarks/callcount_2026-09-04.{tsv,meta}`. Self-time attribution
> (`perf_still_attrib_2026-09-03`, above) ranks WHICH kernel is slow; it
> cannot tell "same calls, slower body" from "more calls, same body". This
> chunk measures calls: `valgrind --tool=callgrind` on r7900x (x86_64-linux —
> callgrind does not run on the Mac), `tools/perf_profile/callcount.py` (new)
> summing every `calls=N` edge in the raw callgrind output into a
> per-function total (`callgrind_annotate` reports Ir cost, never counts),
> `callgrind_annotate --tree=caller` for caller-specific edge counts + their
> directly-measured inclusive Ir share. gradient 512x512 qp40, presets
> 2/6/10, still config, byte-identical both sides at every cell (verified
> twice per cell).
>
> **Two positive controls, both exact at every preset** — the join
> methodology is sound before any finding is trusted: (1) `svt_aom_
> largest_coding_unit_ctor` (C) / `pipeline::merge_sb_units` (port) = 64/64
> at p2, p6, p10 (pure SB-count geometry); (2) the FINAL (non-search)
> per-candidate quantize commit — `full_loop_core`'s always-taken quantize
> edge (C) / `mds1::run_mds1 -> tx_pipeline::tx_unit` (port) — 67,249/67,249
> (p2), 4,165/4,165 (p6), 886/886 (p10), and the same for the chroma commit
> (35,590/35,590, 3,038/3,038, 1,772/1,772).
>
> **HEADLINE #1 — the port commits the FULL RD pipeline on every tx-type
> trial; C commits only the survivors of a cheap pre-screen.** C's
> `tx_type_search` (`product_coding_loop.c:4582`) tries up to ~8 tx-type
> candidates per (tx-depth, TXB) through forward-transform +
> coefficient-domain SATD ONLY (`:4729`/`:4742`), early-exits losers
> (`:4745-4752`), and quantizes/RDOQs (`:4757`) only the ~55% that survive
> (270,415 of 488,414 trials at p2). The port's `txt_search`
> (`crates/svtav1-encoder/src/leaf_funnel/txt.rs:227`) calls `tx_unit` — the
> WHOLE pipeline: residual, transform, quantize, conditional RDOQ, cost — on
> EVERY gated trial (488,414 at p2, matching C's trial WIDTH almost exactly,
> ratio 1.008x — the search breadth is not the bug), then runs its SATD
> screen POST-HOC (`txt.rs:288-304`) to decide whether to discard a result
> already fully computed; the code's own comment says as much ("SATD early
> exit between transform and quantize in C; we apply it post-hoc"). This ONE
> edge (`eval_candidate -> tx_unit`) is **45.20% of the port's entire p2 Ir
> total** — direct tool output, not estimated. Converges to EXACTLY 1.000x at
> p10, where `only_dct_dct` collapses BOTH sides to one candidate (nothing to
> screen) — strong confirmation this mechanism, not a fixed overhead, drives
> the p2/p6 gap. Everything downstream inherits the inflation: `optimize_b`
> 1.767x (p2)/1.271x (p6)/**1.000x (p10, converges)**, `cost_coeffs_txb`
> 1.572x, quantizer-dispatch total 1.548x, `get_nz_map_contexts` 1.580x —
> ONE root cause, not four.
>
> **HEADLINE #2 — the port's post-hoc SATD screen re-derives the residual on
> every trial; C's never touches it.** C's per-trial SATD
> (`svt_aom_satd`, `:4742`) reads already-transformed coefficients and has
> ZERO call-graph edges into `svt_residual_kernel8bit_avx2` at ANY preset
> (verified via `--tree=caller`) — the residual is computed ONCE upstream
> (`perform_tx_partitioning -> svt_aom_residual_kernel`, once per
> tx-depth/TXB search, shared across every tx-type trial). The port's
> `txb_coeff_satd` (`leaf_funnel/detect.rs:144`) calls `residual_i32`
> unconditionally, re-subtracting the same w*h pixels on EVERY trial:
> 484,442 extra calls at p2 (exactly matching C's trial width, 484,442 —
> genuinely repeated work, not over-search), 10,193 at p6 (again exact), 0 at
> p10 (mechanism absent exactly where TXT search is inactive on both sides).
> Measured cost of just this edge: 237,923,278 Ir = 1.54% of p2 total.
>
> **Allocator call count: 1,675x C at p2, falling to 129x (p6) and 41x
> (p10).** C's `malloc+calloc+free` total is FLAT across presets (~2,500-2,600
> calls, one-time setup) where the port's scales with block/candidate count
> (4.36M/333K/102K). This is the call-count root of the ALLOC bucket's
> self-time ratio (387x/52x/25x) — the port's calls are individually CHEAPER
> than C's rare ones, not proportionally as expensive, which is why the
> call-count ratio exceeds the self-time ratio at every preset.
>
> **One preset-10-specific finding NOT explained by either headline:** MDS3
> candidate count (`perform_tx_partitioning`/`eval_candidate`) is EXACT at p2
> and p6 (9,165=9,165, 1,519=1,519) but diverges 2.307x at p10 (886 vs 384) —
> the opposite preset from where headlines #1/#2 apply (TXT search is
> inactive there). Flagged for the next chunk, not attributed. Also NOT
> explained: the inverse-transform dispatcher's 1.6-2.0x inflation does NOT
> converge to 1.000x at p10 (unlike optimize_b), so it is a separate,
> unattributed source.
>
> **NOT MEASURED in this chunk, and do not quote it as if it were:** any
> ms/wall-clock translation of these Ir shares (this chunk's numbers are
> x86_64-linux/r7900x on commit `c4ff4727e`; the paired wall-clock record
> above is aarch64-darwin on `d56a8ef85` — mixing them would misattribute
> cross-host drift as cross-mechanism signal); the fix's actual ms payoff
> (needs a real code change + `tools/perf_ab.sh`, out of scope for a
> measure-only chunk); anything past still/gradient/qp40/512x512×{p2,p6,p10}.

> **BOTH HEADLINES CLOSED — LANDED 2026-09-04 (the commit carrying
> `benchmarks/callcount_txtscreen_2026-09-04.{tsv,meta}` +
> `perf_ab_txtscreen_2026-09-04.tsv`, rebased onto `4bcd5833`, the IFS wiring
> that landed meanwhile; nextest/spotcheck/inter_byte_gate/six still cells/
> census re-run green on the rebased tree).** The after-counts below are the
> record. The wall-clock numbers are the r7900x paired A/B taken on the
> PRE-rebase tree; timing was NOT re-measured after the rebase, and NOT
> measured on this Mac at all (two other lanes were live on the box — a
> timing taken there would have been noise, so none was taken). `txt_search` now runs C's two
> phases: `tx_pipeline::SatdScreen` is C's `best_satd_tx_search` /
> `satd_early_exit_th` running minimum, evaluated inside
> `tx_unit_screened` / `tx_unit_hbd_screened` at C's position — after the
> forward transform, BEFORE the quantizer — and a rejected trial returns
> there. `detect::txb_coeff_satd{,_hbd}` (the post-hoc screen that re-derived
> the residual AND transform per trial) is deleted. Same grid, same box,
> nine byte-identical runs (before/after/C wrote the same OBUs at every
> preset): the tx-type-search quantize edge is now **EXACTLY C's** —
> 488,414 -> 270,415 vs C 270,415 (p2), 10,537 -> 8,397 vs 8,397 (p6), 608 =
> 608 (p10, no screen); the redundant-residual edge is 484,442 -> **0** and
> 10,193 -> **0**; the forward-transform total 1,080,313 -> 595,871 vs C
> 600,171 (p2), 22,133 = C (p6). p2 Ir total -23.5 % (15.40G -> 11.79G),
> port/C 3.482x -> 2.665x; p6 -2.9 %; p10 +0.04 % (control — unchanged to the
> unit on every count). Wall clock (paired A/B, 9 rounds, ident=Y every
> cell): 512² p2 652.5 -> 491.3 ms (**1.325x**), 256² p2 1.353x, 64² p2
> 1.486x; p6 1.00-1.07x; p10 1.00x. **Do NOT read `tx_unit_inner`'s flat
> 591,253 as "the fix did not land"** — the screen lives inside the unit; the
> C-comparable quantity is the quantizer edge. **Still open, now
> measurable:** the residual is still derived once per tx-type TRIAL (port
> 595,871 vs C 435,245 at p2; C computes it once per TXB in
> `perform_tx_partitioning`) — a pre-computed-residual argument to
> `tx_unit` is the next chunk; C's GATE 3b (`early_cost` pre-rate screen,
> :4908) and GATE 4 (absolute early exit, :4957) have no port transcription
> and are byte-inert; the MDS3 p10 2.307x finding above is untouched.

> **THE GENERIC `#[magetypes]` ROUTE IS OPEN FOR THE DIRECTIONAL-INTRA FAMILY
> — DEMONSTRATED, NOT ASSERTED (2026-09-04). THIS SUPERSEDES THE "WHY IT IS
> HAND-WRITTEN" PARAGRAPHS BELOW.** Those paragraphs are still CORRECT about
> the PIN — `Cargo.lock` holds `magetypes 0.9.28`, the only published version,
> and it has no integer widening. They are now STALE about upstream:
> `imazen/archmage` `origin/main` is **`3cd0a04`** and carries
> `magetypes/src/simd/backends/widen_narrow.rs` plus its generated generic
> surface, i.e. **both** PR #71 (`shl_uniform` / `shr_*_uniform`,
> `saturating_add`/`sub` at 8/16-bit) and PR #74 (`widen_low` / `widen_high`
> on `u8xN`/`i8xN`/`u16xN`/`i16xN`, `narrow_saturating_{i8,u8}` on `i16xN`,
> `narrow_saturating_{i16,u16}` on `i32xN`) — all widths, all six backends.
>
> **MEASURED, in this workspace, against a dev-only git patch (not committed):**
> the zone-2 kernel compiles as ONE `#[magetypes(define(u8x16, u16x8, i16x8),
> v4, v3, neon, wasm128, scalar)]` body and is **byte-exact against the real C
> symbol on every archmage tier** — `dr_prediction_kernels_match_c` and
> `dr_prediction_all_tiers_match_c` both green with `permutations_run >= 2`.
> So the generic route is VIABLE for this family, which is what the earlier
> audit's missing-primitive list was written to unblock.
>
> The whole 16-lane inner step, verbatim, is:
>
> ```rust
> let lo = ((a0v.widen_low().shl_const::<5>()
>     + (a1v.widen_low() - a0v.widen_low()) * sh_v) + r16).shr_logical_const::<5>();
> let hi = ((a0v.widen_high().shl_const::<5>()
>     + (a1v.widen_high() - a0v.widen_high()) * sh_v) + r16).shr_logical_const::<5>();
> lo.bitcast_i16x8().narrow_saturating_u8(hi.bitcast_i16x8()).store(o);
> ```
>
> Two notes for whoever lands it. (1) There is still no ROUNDING narrow; the
> `+ r16` then `shr_logical_const::<5>` above IS `vrshrn_n_u16::<5>` written
> out, and the SATURATING narrow is inert here only because the result is
> provably in `[0, 255]` — the `u16 -> i16` bitcast before it is exact for the
> same reason. Say so at any new call site; it is not a general substitute.
> (2) `narrow_saturating_*` takes an `i16` source by design (the doc comment
> says the x86/wasm instruction sets offer no `u16` shape), so the bitcast is
> mandatory, not stylistic.
>
> **The wiring, dev-only, NOT to be committed** (a path/git dep in a pushed
> manifest fails CI at resolution on the x86 host):
>
> ```toml
> [patch.crates-io]
> archmage  = { git = "https://github.com/imazen/archmage", rev = "3cd0a04" }
> magetypes = { git = "https://github.com/imazen/archmage", rev = "3cd0a04" }
> ```
>
> Patch BOTH: `magetypes` depends on `archmage` by path inside that workspace,
> so patching only one gives two incompatible `archmage` crates and a type
> error on every token.
>
> **NOT ESTABLISHED, and it is the next step:** the generic body's SPEED. The
> A/B that isolates the route — same patched deps, per-ISA arm vs generic body,
> `tools/perf_ab.sh` on the still and videokey arms — was built
> (`~/tmp/pe_mtbase` / `~/tmp/pe_mtgen`) and NOT RUN: the box had a sibling
> workspace's gates on it and the session ended first. The shipped arm's own
> numbers are in `benchmarks/z2neon_ab_2026-09-03.*`. Land the generic body
> only against that A/B — a generic body materially slower than the per-ISA arm
> is a tradeoff to surface, not to absorb — and note that landing it for real
> also needs a `magetypes` RELEASE, which is a separate step.
>
> The same argument covers the rest of the audit's list: `dr_z1_edged_flat_neon`
> and `dr_z3_edged_flat_neon` are the identical kernel with a different
> traversal, `dsp::residual::residual_i16`/`residual_i32` are the same
> widen-arithmetic-narrow shape, and `me_sad` / `variance::sse` need the same
> widening plus `u16x8::widen_low() -> u32x4`, which `3cd0a04` also has.

> **THE MEMSET RULE'S FIRST TEST, AND IT PAYS: THE RDOQ INPUT BUFFER'S
> RE-ZERO WAS DEAD ON ALL FOUR QUANTIZER PATHS (2026-09-03).** Record
> `benchmarks/dqzero_ab_2026-09-03.*`. Two allocation removals had just failed
> in the same chunk, so the next candidate was ranked by what it MEMSETS:
> `tools/perf_profile/ancestor.py` over the `memset`/`bzero`/`memmove`/`memcpy`
> family at 512x512 p2 makes that family **4.0 % of the port's frame** (753 of
> 18,913 self samples, 22.0 ms of 552.9) and ranks its callers
> `tx_unit_inner` **25.5 %** / `eval_candidate` 19.3 % / `cost_coeffs_txb`
> 7.2 % / `SatdScratch::split` 6.1 % / `try_fwd_dct_square` 5.4 % /
> `hadamard_satd` 5.3 %.
>
> `tx_unit`'s `dqcoeff` was `TxScratch::zeroed(..)` — an explicit `fill(0)` of
> up to 4 KiB per transform unit. Dead: `quantize_{fp,b}_raster`'s cores write
> BOTH outputs at every one of the `pw * ph` positions, and the two QM
> quantizers open with their own `fill(0)`, so on that path the caller's was a
> SECOND memset of the same bytes. A/B, every cell `ident=Y`:
>
> | arm | 256 p2 | 512 p2 | 256 p6 | 512 p6 | 256 p10 | 512 p10 |
> |---|---|---|---|---|---|---|
> | still (n=15) | 1.003x | 1.002x | 1.008x | 1.008x | **1.020x** | **1.015x** |
>
> | arm | 128 p6 | 256 p6 | 512 p6 | 128 p8 | 256 p8 | 512 p8 |
> |---|---|---|---|---|---|---|
> | videokey (n=25) | 1.004x | 1.010x | 1.000x | **1.015x** | 1.009x | 1.010x |
>
> **The two biggest gains are the p10 STILL cells** — where LIBC_MEM's share of
> the gap is highest (11.6 % at p10 against 8.4 % at p2) — so the preset
> gradient runs the way the mechanism predicts.
>
> THE CONTROL IS A POISON, UNCONDITIONAL IN RELEASE: filling the buffer with
> `0x5A5A_5A5A` instead of zeroing it leaves `regression_spotcheck` 102/102 and
> `identity_full_8bit` 1100/1100, and the buffer is definitely read (it feeds
> `sse_i32` and the inverse-transform input), so a surviving poison had a path
> to the bitstream and did not take it.
>
> **Five data points now: three memset removals priced, all won
> (`ab7c5ed4`, `700357e2`, this); two allocation removals priced, both lost.**

> **[SUPERSEDED 2026-09-05 by the arm10 position at the top of this file —
> the table below is arm9 and no longer the position.] THE POSITION AFTER THIS
> CHUNK — AND A MEASURED REASON NOT TO READ A 2-3 %
> SLOPE MOVE OUT OF TWO POSITION RUNS (2026-09-03).** Records
> `benchmarks/perf_2026-09-03-arm9-{still,videokey,inter}.*` and
> `perf_2026-09-03-arm9-POSITION.meta`. 25 paired rounds, preset 8, gradient
> qp 40, box quiet, **every cell of all three arms ident=Y** (all four inter
> cells included):
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio |
> |---|---|---|---|---|---|
> | still | 0.791x | 1.402x | 2.214x | 2.376x | **2.46x** |
> | videokey | 1.338x | 1.850x | 2.400x | 2.542x | **2.60x** |
> | inter | 1.592x | 2.040x | 2.486x | 2.767x | **2.82x** |
>
> Against arm8/arm7 (2.40x / 2.58x / 2.80x) that reads like a small
> regression. **It is session drift, and this is the run that measured it.** A
> PAIRED A/B of arm8's tree against this one, in ONE session, 512x512 p8,
> n=25, ident=Y, says the current tree is **1.013x FASTER**, not slower — and
> the same `76fdb802` binary measured 9.493 ms in the arm8 session and
> 9.797 ms in this one, a ~3 % PORT-SIDE offset between sessions. Every
> position record here already warns about C's drift; this one adds that the
> port side drifts by about as much, so **a position run's slope ratio cannot
> resolve a 2-3 % change across sessions — only the paired A/B can.** The same
> pair of A/Bs separates this chunk's z2 arm (~2.4 % at that cell) from the
> three inter commits that landed beside it (~-1.1 % there, a difference of
> TREES, not an attribution to any one of them).

> **TWO ALLOCATION REMOVALS, BOTH MEASURED, BOTH NEGATIVE — AND WHAT
> SEPARATES THEM FROM THE TWO THAT WON (2026-09-03).** Records
> `benchmarks/mds3d0_null_2026-09-03.meta` and
> `benchmarks/hadscratch_null_2026-09-03.meta`. Neither is in the tree.
>
> | attempt | what it removed | result |
> |---|---|---|
> | `mds3::eval_candidate`'s `d0_recon` | a `w * h` alloc + memcpy per candidate whose result is NEVER read | **0.983-0.988x** at 512 p6/p10, span above 1.0, reproduced |
> | `predict::hadamard_satd`'s two buffers | two fixed-bound (`<= 1024` element) allocs per call, the #3 allocator caller at 11.9 % | **0.989-0.992x** across the videokey arm, four cells' spans above 1.0 |
>
> The d0 attempt carries a PARAMETER-ONLY control (same new argument, allocation
> kept) showing the cost is the removal and not the signature; the hadamard
> attempt carries a stack-array variant for small tiles that recovers about half
> the loss and is still negative.
>
> **What separates these from `0c70f3fc` (`coeff_contexts` -> a stack array,
> six of six cells) and `700357e2` (four stack arrays -> one scratch, nine of
> twelve) is not the allocations.** `coeff_contexts` removed one and `700357e2`
> removed NONE; both removed **zeroing**. So the working rule for the next
> chunk: **on this allocator a malloc/free pair is worth about nothing and a
> memset is worth real time.** Rank the remaining share by what it MEMSETS.
> `perf_still_attrib_2026-09-03.tsv`'s LIBC_MEM row (8.4 / 9.9 / 11.6 % of the
> gap at p2 / p6 / p10, 3.9-5.8x C) is that queue; its ALLOC row (10.1 / 16.4 /
> 20.3 %, 387x / 52x / 25x C) has now failed to convert twice in one day.
>
> The measured ceiling, so nobody re-derives it: the whole allocator family is
> **5.5 % of the port's 512 p2 frame** (1,040 of 18,913 self samples, 30.4 ms),
> split `eval_candidate` 28.7 %, `tx_unit_inner` 24.8 %, `hadamard_satd`
> 11.9 %, `drop_glue::<Cand>` 5.8 %, `inject_candidates` 5.2 %.

> **AN ALLOCATION NOTHING READS IS NOT FREE TO REMOVE — MEASURED, REPRODUCED,
> REVERTED (2026-09-03).** Record `benchmarks/mds3d0_null_2026-09-03.meta`.
> `mds3::eval_candidate`'s `d0_recon` is a `vec![0u8; w * h]` + a `w * h`
> memcpy per CANDIDATE, and `cand.y_recon_d0` has exactly one reader —
> `evaluate_leaf`'s `gate_y`, from ONE candidate, only at `bypass_encdec`
> (preset >= 4). Skipping it everywhere else is byte-inert (2512/2512,
> spotcheck 102/102) and is **1.2-1.7 % SLOWER** at 512 p6 and p10, with the
> p25/p75 span entirely above 1.0 and 512 p6 reproduced across two runs. A
> parameter-only control (same new argument, allocation KEPT) is a soft null,
> so the cost is the removal itself, not the signature. Fourth profile share
> this campaign has failed to convert, and the first to go the WRONG way while
> being provably dead work. The measured ceiling for the whole item is small:
> the allocator family is **5.5 % of the port's 512 p2 frame** (1,040 of
> 18,913 self samples = 30.4 ms), `eval_candidate` is 28.7 % of that (1.58 % of
> the frame), `tx_unit_inner` 24.8 % and `hadamard_satd` 11.9 %.

> **THE aarch64 INTER MEMORY AXIS NO LONGER HOLDS — RE-MEASURED, AND IT IS
> WORSE THAN THE ONE COMMIT THAT WAS KNOWN TO COST IT (2026-09-03).** Record
> `benchmarks/mem_inter_axis_2026-09-03b.meta`. `mem_levelscratch_2026-09-03.meta`
> §4 kept `700357e2` over a +4 MiB macOS inter RSS cost and made the next memory
> chunk re-measure the arm. It is re-measured, on `76fdb802`, same harness
> (`tools/mem_peak.sh`, `/usr/bin/time -l` max RSS, median of 7, gradient qp 40
> p13), same C library:
>
> | cell | port THEN | port NOW | C NOW | ratio THEN | ratio NOW |
> |---|---|---|---|---|---|
> | 1280 inter | 59 520 KiB | **67 792** | 53 056 | 1.121x | **1.278x** |
> | 2048 inter | 150 080 KiB | **158 864** | 119 488 | 1.257x | **1.329x** |
>
> **C did not move** (53 072 -> 53 056 and 119 424 -> 119 488, <= 0.05 %), so
> the whole ratio change is the port's, and **both inter cells are now outside
> the 25 % goal** where one was inside it and one was at its boundary. About
> half the 1280 rise is `700357e2`'s known +4.1 MiB; **the rest is NOT
> attributed** — the commits in between include four inter CORRECTNESS fixes
> (`4e29d8fa` the DPB never received an inter frame, `813bc939` NEARMV,
> `aa308152` the hard-coded DPB slot, `8fa2d035` the temporal motion field
> wired), and a DPB that now holds inter frames plus a live motion field are
> real per-frame buffers, so a rise is the expected DIRECTION. Nobody has taken
> a clean per-commit pair across them. **The still and videokey arms did not
> move** (1280: 35 792 / 41 984 KiB, 0.79x / 0.88x of C) — this is the inter
> arm on one ISA, exactly the shape the level-scratch record described.

> **ZONE 2 — THE LAST SCALAR DIRECTIONAL-INTRA KERNEL — RUNS IN NEON LANES
> NOW, AND IT DID NOT NEED C's GATHER (2026-09-03).** Record
> `benchmarks/z2neon_ab_2026-09-03.*`. `perf_still_attrib_2026-09-03.meta` put
> `dr_predictor_edged` at 763 of 19,115 self samples at 512x512 p2 against C's
> `svt_av1_dr_prediction_z2_neon` 323 / `_z3_neon` 94 / `_z1_neon` 83 — z2 is
> C's largest directional kernel there and was the one still unported. A/B,
> box verified quiet before AND after each run, every cell `ident=Y`:
>
> | arm | 256 p2 | 512 p2 | 256 p6 | 512 p6 | 256 p10 | 512 p10 |
> |---|---|---|---|---|---|---|
> | still (n=15) | 1.003x | **1.014x** | 1.009x | 1.005x | 0.997x | 1.002x |
>
> | arm | 128 p6 | 256 p6 | 512 p6 | 128 p8 | 256 p8 | 512 p8 |
> |---|---|---|---|---|---|---|
> | videokey (n=25) | **1.042x** | **1.028x** | **1.025x** | 1.018x | 1.011x | 1.008x |
>
> **All six videokey cells move with their whole p25/p75 span below 1.0**, and
> so does 512 p2 — the still arm's worst cell. The two p10 still cells are the
> near-control (`dr_predictor_edged` is not in the port's top twelve INTRA_PRED
> symbols at p10) and read 1.002x / 0.997x.
>
> **THE STRUCTURE IS THE POINT.** C computes BOTH edges for every 16-column
> group and selects with `vbslq_u8`, reaching the `left` half through
> `vqtbl4q_u8` — a 64-byte table lookup — because within a ROW that half's
> `base_y` and `shift` both vary with the column. This arm needs no gather:
> `base_x` decreases with `r`, so the `above` region is a staircase and its
> complement is the `left` region; pass 1 walks the first ROW-major (z1's
> kernel, constant shift along a row) and pass 2 walks the second COLUMN-major
> (z3's kernel, constant shift down a column, same scatter). Disjoint regions,
> every output byte written once, no select.
>
> **AND THE MAGETYPES ANSWER IS ABOUT THE PIN, NOT UPSTREAM.** `Cargo.lock`
> holds `magetypes 0.9.28`, the only published version, and archmage's
> `v0.9.28` tag PREDATES both PR #71 (`fd66480`, uniform shifts + saturating
> add/sub) and PR #74 (`fd3c609`, `widen_low`/`widen_high`/
> `narrow_saturating`). Verified by grep of the published crate source: all
> four symbols ABSENT. The deciding primitive is still `u8x16 -> u16x8`
> widening, and two gaps would survive a bump anyway — `narrow_saturating` is
> a SATURATING narrow where this needs a ROUNDING one, and `shl_uniform` was
> never what this kernel lacked.

> **THE STILL ARM'S MEMORY TRAFFIC IS DEAD BUFFER WORK IN THE DRIVERS, AND IT
> IS FOUND BY ASKING WHO CALLS `memset` (2026-09-03).** Commits `ab7c5ed4`
> (the `dq_full` elision) and `ee7a755f` (the shared level-map scratch);
> records `benchmarks/dqfull_ab_2026-09-03.*` and
> `benchmarks/levelscratch_ab_2026-09-03.*`. Both are byte-identical, both were
> found the same way, and together they are the largest still-arm movement of
> the day at the slow presets:
>
> | arm | 256 p2 | 256 p6 | 256 p10 | 512 p2 | 512 p6 | 512 p10 |
> |---|---|---|---|---|---|---|
> | `dq_full` (still, n=15) | **1.028x** | 1.013x | 1.006x | **1.023x** | 1.008x | 1.002x |
> | level scratch (still, n=15) | **1.039x** | 1.017x | 1.013x | **1.029x** | 1.013x | 0.998x |
>
> | arm | 128 p6 | 128 p8 | 256 p6 | 256 p8 | 512 p6 | 512 p8 |
> |---|---|---|---|---|---|---|
> | `dq_full` (videokey, n=25) | 1.004x | 1.001x | 1.013x | 1.000x | 1.007x | 1.011x |
> | level scratch (videokey, n=25) | 1.017x | 0.995x | 1.023x | 1.006x | 1.019x | 1.017x |
>
> Stated as a PRODUCT of the two paired measurements and not as a third
> measurement, the pair is **~1.07x on a preset-2 still frame** at both sizes
> and ~1.02-1.03x at p6. Three cells are NULL and are reported (512 p10 still
> for the scratch, 128 p8 videokey for the scratch, and the two p10 still cells
> for `dq_full` cross 1.0).
>
> **1. THE INVERSE TRANSFORM'S INPUT WAS A COPY OF THE BUFFER BESIDE IT.**
> `tx_unit_inner` built `dq_full` — a `w x h` buffer, zeroed then filled row by
> row from the `pw x ph` quantised corner — for every reconstructed transform
> unit, because the inverse dispatch reads its input at the SAME stride it
> writes its output at. But `pw = w.min(32)` and `ph = h.min(32)`, so **for
> every TX up to 32x32 it was a byte-for-byte copy of `dqcoeff` at the same
> stride**: a `memset` of `w*h*4` bytes plus a `memcpy` of `w*h*4`, both dead,
> 16 KiB apiece on a 32x32 TU. Only the 64-dim shapes need the re-lay, and
> there its zero-fill IS load-bearing.
>
> **2. FOUR COPIES OF ONE STACK ARRAY, ZEROED IN FULL AND THEN RE-ZEROED IN
> PART.** `cost_coeffs_txb`, `cost_coeffs_txb_pd0`, `optimize_b_tc` and
> `write_coeffs_txb_1d` each declared `[0u8; LEVELS_SCRATCH_LEN]` (1,456 bytes)
> and two of them a `[0i8; MAX_TXB_COEFF_AREA]` (1,024) beside it —
> per call — and `txb_init_levels` then re-zeroes the only part anything reads,
> the `used` prefix, ~112 bytes for a 4x4. C keeps ONE persistent
> `md_levels_buf` zeroed once at `md_process.c:235`. The four now share a
> per-thread `coeff_c::TxbScratch`.
>
> **THE CONTROL FOR (2) IS WORTH MORE THAN THE WIN.** "Every reader stays inside
> `[0, used)`" was PROSE in a comment. `with_txb_scratch` poisons the buffer
> before every hand-out; made unconditional on a RELEASE build it turns the
> claim into a measurement — **teeth**: with the `eob <= 1` prefix-zeroing
> removed, `regression_spotcheck` is **96/100** with four cells showing real
> SIZE differences; **control**: with it restored, `regression_spotcheck`
> 100/100 and `identity_full_8bit` **1100/1100** with 0xAA in every byte outside
> the zeroed prefix. **`cargo nextest` does NOT witness it** — 2,509/2,509 pass
> with the teeth applied, because the debug suite never reaches an `eob == 1`
> txb whose DC level exceeds `NUM_BASE_LEVELS`. A `debug_assertions`-only
> poison would have been no evidence at all.
>
> **THE METHOD, WHICH IS THE TRANSFERABLE PART.** Neither change came out of the
> class table. `perf_still_attrib_2026-09-03` puts LIBC_MEM at 9.9 % of the gap
> at 512 p6 and 11.6 % at p10, but a class share names a SYMBOL
> (`_platform_memset`), not a cause — and the cause is always its CALLER, the
> same inversion `residual_simd_ab_2026-09-03.meta` recorded for `SIMD_GAP`.
> Nearest-ancestor attribution of the memset / memmove / malloc families
> (`tools/perf_profile/ancestor.py`, one query per family) ranks the callers
> directly. On a fresh 512 p6 profile of `ab7c5ed4`'s parent that ranking was:
>
> | caller | memset | memmove | alloc | total, % of the port's frame |
> |---|---:|---:|---:|---:|
> | `mds3::eval_candidate` | 34 | 60 | 131 | 2.4 % |
> | `tx_pipeline::tx_unit_inner` | 71 | 70 | 74 | 2.3 % |
> | `pipeline::encode_tile_rows::{closure#0}` | 0 | 26 | 71 | 1.0 % |
> | `pd0::lvl1_cost_from_pred` | 47 | 0 | 34 | 0.9 % |
> | `coeff_rate::cost_coeffs_txb` | 66 | 0 | 0 | 0.7 % |
> | `txfm_simd::try_fwd_dct_square` | 49 | 0 | 0 | 0.5 % |
>
> The whole memory-traffic family is **15.6 % of the port's 512 p6 frame and
> ~25 % of its gap to C** (C's is 0.59 ms of 12.79). **Ancestor-attribute those
> three symbol families before planning any of this work**; it is a five-minute
> query and both of this chunk's wins came straight out of it.
>
> **AND A THIRD, SMALLER ONE: THE LARGEST ALLOCATOR CALLER'S PURE TEMPORARIES
> — AND AN EXPLICIT RE-ZERO THAT COST MORE THAN THE `vec!` IT REPLACED.**
> Record `benchmarks/mds3scratch_ab_2026-09-03.meta`. `mds3::eval_candidate` is
> the port's single largest allocator caller on the still arm (33.9 % of the
> malloc/free samples at 512 p2, 19.5 % at p6, 17.8 % at p10), and its
> `txb_pred` / `loc_above` / `loc_left` are the only three of its ~10
> per-candidate allocations that never escape — the rest are MOVED into the
> depth winner and need a flat arena. Recycling those three through a scratch
> borrowed ONCE per leaf (not per call, which is what
> `benchmarks/mdscratch_null_2026-09-03.meta` measured NULL) is worth
> **1.004x at 512 p2 and 1.007x at 512 p6, both confirmed at n=25 with their
> whole span below 1.0, and NULL on the other ten cells** including all six
> videokey ones.
>
> **The transferable part is the version that did NOT work.** Writing
> `clear()` + `resize(n, 0)` — the obvious way to reproduce `vec![0u8; n]` in a
> recycled buffer — **REGRESSED 512 p2 to 0.998x with its whole span above
> 1.0**, reproduced twice. `vec![0; n]` is a `calloc` whose zeros come from
> already-zero pages for free; an explicit `resize` pays a real `memset` of up
> to 4 KiB per TX block. Grow-only (`if len < n { resize }`) plus a slice took
> the cell from -0.2 % to +0.4 %. **When you replace a `vec![0; n]` with a
> recycled buffer, re-zero only what is genuinely read before it is written** —
> `TxScratch::zeroed` is right for the partially-written buffers (`dq_full`,
> `dqcoeff`) and a waste for the fully-written ones.
>
> **THE STILL ARM'S SLOPE RATIO IS 2.48x -> 2.40x ACROSS THE WHOLE CHUNK**
> (`benchmarks/perf_2026-09-03-arm8-still.*`, `perf_gate.sh`, 25 paired rounds,
> 64/128/256/512 at preset 8, gradient qp 40, **box verified quiet** — a `ps`
> listing, not a count). The port's own per-pixel slope is
> **37.3748 -> 36.1673 ms/MP, -3.2 %**, against a C slope that reads 15.0751
> at the start of the chunk and 15.0430 at the end (-0.2 %, i.e. flat), so
> essentially all of it is real. Per cell: 0.79x / 1.37x / **2.19x** / **2.32x**
> at 64/128/256/512, every cell `ident=Y`. The videokey arm stands at
> **2.66x -> 2.58x** (arm7; the third commit is NULL there and it was not
> re-positioned).
>
> **THE POSITION AFTER THE TWO, ALL THREE ARMS** (`perf_gate.sh`, 25 paired
> rounds, sizes 64/128/256/512 at preset 8, gradient qp 40, box verified quiet;
> `benchmarks/perf_2026-09-03-arm7-{still,videokey,inter}.*`). Read as POSITION,
> not attribution — the paired A/Bs above are the attribution, and **the inter
> arm also carries two OTHER agents' commits landed the same day** (`4e29d8fa`
> the DPB fix, `813bc939` the NEARMV injection), which change what an inter
> frame does:
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio |
> |---|---|---|---|---|---|
> | still — arm6 (before) | 0.81x | 1.38x | 2.23x | 2.38x | 2.48x |
> | still — arm7 (after) | 0.80x | 1.37x | **2.20x** | **2.34x** | **2.45x** |
> | videokey — arm6 | 1.32x | 1.84x | 2.37x | 2.61x | 2.66x |
> | videokey — arm7 | 1.28x | 1.81x | **2.35x** | **2.54x** | **2.58x** |
> | inter — arm6 | 1.62x | 2.00x | 2.46x | 2.80x* | 2.64x |
> | inter — arm7 | 1.59x | 2.04x | 2.44x | 2.74x | 2.80x |
>
> (* arm6's 512 inter cell is `ident=N` — two encoders making different
> decisions — and is out of its fit. Every arm7 cell of all three arms is
> `ident=Y`.)
>
> The port's own slope moved 37.37 -> **36.89 ms/MP** on the still arm and
> 166.57 -> **162.03** on the video-mode key frame, against a C slope that
> moved 15.075 -> 15.088 and 62.53 -> 62.73 — so ~1.3 % and ~2.7 % of each is
> real. **The inter arm's slope ratio RISES to 2.80x and that is not this
> chunk**: all four of its cells are `ident=Y` now (the 512 cell never was
> before), i.e. the port is making different, more faithful inter decisions
> after the two commits above. Do not read it as a regression from this chunk.
>
> **MEMORY: ZERO LIVE BYTES, AND ONE macOS-ONLY CELL THAT MOVES.** Full record
> `benchmarks/mem_levelscratch_2026-09-03.meta`. The `dq_full` elision is NULL
> on all twelve cells. The level scratch is **identical to the digit on Linux
> peak HEAP** (heaptrack, 49.18 M and 121.56 M at 1280/2048 inter), **null on
> Linux peak RSS**, and **null on the still and videokey arms of both ISAs** —
> but it raises the **macOS INTER arm's peak RSS by ~4 MiB at 1280
> (59.7 -> 63.8, +6.9 %, three interleaved pairs)**. It is NOT the `Box`: a
> TLS-resident variant with no heap allocation measures the same +4 MiB. Kept
> because the CPU win is 1.3-3.9 % across two arms and the cost is one arm on
> one ISA with no live bytes behind it — but the aarch64 inter arm is the axis
> that was AT its 25 % boundary, so **the next memory chunk must re-measure it**.
>
> **AND ONE NULL FROM THE SAME CHUNK, REVERTED:** routing the two chroma
> detectors (`detect::chroma_detector_fires`, `detect::chroma_var_arm_fires` —
> a hand-rolled triple SAD and a hand-rolled variance-against-128) through the
> tier-dispatched `dsp::sad::sad` and `dsp::variance::variance_diff` they were
> transcriptions of is byte-identical and measures NULL on both arms, eleven of
> twelve spans crossing 1.0 (`benchmarks/chromadetect_ab_2026-09-03.meta`). The
> blocks are chroma-sized and each call crosses an `#[arcane]` boundary — the
> "entry points only, one per hot path" rule priced per call. **Third time a
> profile SHARE has failed to become a win** (`aom_hadamard_8x8` 1.88 %, CDEF
> `int16x8` 14.2 %, this 1.35 %).
>
> **That record also carries a measured cost of timing on a busy box**: the
> first run of that A/B, taken while a sibling workspace ran a 2048x2048
> encode, read 512 p10 still at **1.012x with its whole span below 1.0**; the
> same binaries on a quiet box read **1.002x with the span crossing**. Paired
> interleaving removes DRIFT, not CONTENTION. Check `ps` and defer.
>
> WHAT THE TABLE STILL NAMES, unworked: `mds3::eval_candidate` is the largest
> single allocator caller at both p6 (131 samples) and p10 (273 of 1,262), and
> it is ~10 allocations per candidate — `dep_recon`, `dep_pred`, `txb_pred` per
> txb, `loc_above`/`loc_left`, the `Vec<Vec<i32>>` of per-txb levels, plus the
> `qcoeff`/`recon` `Vec`s `tx_unit_inner` allocates and it frees. Only
> `txb_pred`, `loc_above` and `loc_left` are pure temporaries; the rest are
> MOVED into the depth winner, so the fix is a flat arena, not a scratch
> buffer. `try_fwd_dct_square`'s memset is the `[0i32; N*N]` intermediate in
> `dct_square_driver!` (16 KiB at N=64), fully written by the column pass before
> the row pass reads it — dead, and unremovable in safe Rust without threading
> a scratch through the four driver macro families.

> **THE STILL ARM'S #1 CLASS WAS FOUR HAND-ROLLED LOOPS CALLING NOTHING
> (2026-09-03).** Commit `d1a00ae2`; record
> `benchmarks/residual_simd_ab_2026-09-03.*`. DISTORTION is **17.4 % of the
> still gap at 512x512 p2**, the largest class there, and its two biggest port
> symbols — `predict::hadamard_satd` (854 self samples of 19,115) and
> `detect::txb_coeff_satd::{closure#0}` (690) — **are not the transform**. The
> Hadamard kernels and the forward DCT are separate symbols and already
> vectorised; that self time is the `src as iN - pred as iN` loop each function
> carried INLINE. C computes it with `svt_residual_kernel8bit_neon` (268
> samples).
>
> **And the port already had the kernel.** `dsp::residual::residual_i32` has
> scalar / AVX2 / NEON arms and sits in the same profile at 337 samples; four
> call sites just were not using it (`txb_coeff_satd`, `pd0::lvl1_cost_from_pred`,
> `pd0::lvl5_like_block_cost_rect` — duplicate transcriptions of one loop — and
> `hadamard_satd`, which needed an i16 output that did not exist and is added
> as `residual_i16`). A/B, every cell `ident=Y`:
>
> | arm | 256 p2 | 512 p2 | 256 p6 | 512 p6 | 256 p10 | 512 p10 |
> |---|---|---|---|---|---|---|
> | still (n=15) | **1.037x** | **1.052x** | 1.022x | 1.024x | 1.012x | 1.011x |
>
> | arm | 128 p6 | 256 p6 | 512 p6 | 128 p8 | 256 p8 | 512 p8 |
> |---|---|---|---|---|---|---|
> | videokey (n=25) | 0.990x | 1.006x | **1.017x** | 1.001x | 1.002x | 1.007x |
>
> **All six STILL cells move with their whole p25/p75 span below 1.0, and
> 512 p2 — the still arm's worst cell at 3.17x C — gains 5.2 %**, the largest
> still-arm win of the day. The preset gradient matches the attribution
> (DISTORTION is 17.4 / 7.7 / 6.2 % of the gap at p2 / p6 / p10). The videokey
> 128 p6 cell is a NULL with the widest span of any cell measured this day
> (0.9536/1.0630) and is reported rather than dropped.
>
> **THE LESSON IS NOT "VECTORISE MORE".** Three of the four sites had a
> vectorised kernel sitting in the same crate the whole time. Before writing a
> SIMD arm for a hot loop, grep for one — `perf_class_attrib`'s own
> `SIMD_GAP` bucket cannot see this case, because the port symbol it names is
> the CALLER, not a missing kernel.

> **THE 8-WIDE CDEF FILTER RUNS IN `int16x8` LANES NOW, AND THE MAGETYPES GAP
> THAT KEPT IT HAND-WRITTEN IS RECORDED (2026-09-03).** Commit `c68247aa545b`;
> record `benchmarks/cdef_i16_ab_2026-09-03.*`. The still ranking above puts
> CDEF at **14.2 % of the gap at 512x512 p6** (6.60x) and the video-key record
> at 11.8 %; both sides are NEON, so it was a QUALITY item, and the cause was
> one line of structure — C works in `int16x8` throughout
> (`ASM_NEON/cdef_filter_block_neon.c:26`) where the port carried the same 8
> columns as an `[int32x4_t; 2]` pair. A/B, every cell `ident=Y`:
>
> | arm | 256 p2 | 512 p2 | 256 p6 | 512 p6 | 256 p10 | 512 p10 |
> |---|---|---|---|---|---|---|
> | still (n=15) | 1.000x | **1.006x** | 1.000x | **1.007x** | 0.998x | 0.998x |
>
> | arm | 128 p6 | 256 p6 | 512 p6 | 128 p8 | 256 p8 | 512 p8 |
> |---|---|---|---|---|---|---|
> | videokey (n=25) | **1.030x** | 1.014x | **1.019x** | 1.015x | 1.016x | 1.002x |
>
> The p10 still cells are the CONTROL — the still path runs no CDEF there at
> all — and they read NULL, as they must. **The win is 1.5-3.0 % on the
> videokey arm and 0.6-0.7 % on the still arm at 512 only, well under the
> 14.2 % share**: halving the vector op count of a kernel that is 5.4 % of the
> port's own frame can be worth ~2-3 % at best, and 0.7 % says it was not
> vector-throughput bound. **Price by A/B, not by share** — that is now four
> times this campaign has learned it.
>
> **WHY IT IS HAND-WRITTEN NEON AND NOT `#[magetypes]`.** Verified by source
> read of the local checkout (`~/work/archmage/magetypes`, 0.9.28), not from
> memory. The kernel needs three things the generic API does not have:
> 1. a **variable (runtime-scalar) integer shift** — only `shl_const<N>`,
>    `shr_logical_const<N>` and `shr_arithmetic_const<N>` exist, and CDEF's
>    shift is `max(damping - msb(strength), 0)`, known only at run time.
>    Without it the body would have to be monomorphised on both shift values
>    (81 instantiations at the reachable bounds);
> 2. **saturating integer add/sub** — no `qadd`/`qsub` on any integer backend,
>    and `vqsubq_u16` here IS the `max(thr - shifted, 0)` clamp, not an
>    optimisation of it;
> 3. **integer widening/narrowing conversions** (`i16xN <-> i32xN`,
>    `u8xN <-> u16xN`) — `src/simd/backends/convert_int.rs` carries only
>    same-width bitcasts and `src/simd/generic/cross_width.rs` is f32-only.
>
> The same (3) is what keeps the directional-intra arms and `crate::me_sad`
> hand-written. **Those three primitives are the whole list; with them, CDEF,
> the dr predictors, `residual_i32`, `variance::sse` and the SAD family all
> become one generic body each.**

> **THE STILL ARM HAS ITS OWN RANKING NOW, AND IT IS NOT THE VIDEO-KEY ONE
> (2026-09-03).** `benchmarks/perf_still_attrib_2026-09-03.{tsv,meta,detail.txt}`
> — the first per-class attribution taken on the STILL binary since 2026-08-13,
> at gradient 512x512 qp 40, presets 2 / 6 / 10, scaled by the paired
> byte-identical times in `benchmarks/perf_2026-09-03-still512.tsv`
> (512 p2 **3.174x**, p6 **2.588x**, p10 **2.540x**, n=15).
>
> | rank | 512 p2 | 512 p6 | 512 p10 |
> |---|---|---|---|
> | 1 | DISTORTION 17.4 % | MD_DRIVER 17.0 % | MD_DRIVER 28.9 % |
> | 2 | QUANT_RDOQ 15.3 % | **ALLOC 16.4 %** | **ALLOC 20.3 %** |
> | 3 | MD_DRIVER 12.2 % | **CDEF 14.2 %** | **LIBC_MEM 11.6 %** |
> | 4 | FWD_TXFM 10.8 % | **LIBC_MEM 9.9 %** | FWD_TXFM 9.4 % |
> | 5 | **ALLOC 10.1 %** | DISTORTION 7.7 % | COEFF_CTX 9.2 % |
> | 6 | **LIBC_MEM 8.4 %** | FWD_TXFM 7.4 % | SYNTAX_WRITE 6.6 % |
> | 7 | INV_TXFM 6.5 % | QUANT_RDOQ 6.1 % | DISTORTION 6.2 % |
> | 8 | INTRA_PRED 6.3 % | LOOP_RESTORE 6.0 % | INTRA_PRED 6.0 % |
>
> **ALLOC + LIBC_MEM is the only item in the top five at all three presets** —
> 18.5 / 26.3 / 31.9 % — and it has the worst ratio in the table (ALLOC is
> **387x** C at p2, 52x at p6, 25x at p10). That is what is LEFT after the three
> hoists recorded above.
>
> **SIMD COVERAGE is the SMALLEST `why` bucket on the still arm at every
> preset** (SIMD_GAP 5.3 / 11.2 / 5.1 %, against SIMD_QUAL 33.1 / 25.4 / 20.4 %,
> ALLOC 18.1 / 25.6 / 30.7 % and SCALAR_BOTH 43.4 / 37.8 / 43.8 %). Planning
> still-path work from the inter record's "five scalar kernels" framing picks
> the wrong queue.
>
> TWO CORRECTIONS THE RECORD CARRIES. (1) **INTRA_PRED's hot symbol is
> preset-dependent.** At p2 it is `dr_predictor_edged` (763 samples) against C's
> `svt_av1_dr_prediction_z2_neon` **323** / `_z3_neon` 94 / `_z1_neon` 83 — **z2
> is C's largest directional kernel here, bigger than z1 and z3 combined**, and
> z2 is the one still unported. At p6/p10 `dr_predictor_edged` is not in the top
> twelve at all; `predict_dc`, `predict_filter_intra` and `predict_smooth` are.
> (2) **CDEF is a still item at p6 (14.2 %, 6.60x) and ZERO at p10**, and it is
> a QUALITY gap with a named cause: C's kernel works in `int16x8` lanes
> throughout (`ASM_NEON/cdef_filter_block_neon.c:26`) where the port carries the
> same 8 columns as a `[int32x4_t; 2]` pair — twice the vector ops for the same
> work.

> **A MEASURED NULL AND A MEASURED REGRESSION — THE PORT'S LARGEST
> ALLOCATION-COUNT SITE, HOISTED AND REVERTED (2026-09-03).** Record
> `benchmarks/neighbor_scratch_ab_2026-09-03.{tsv,meta}`; **the change is NOT in
> the tree.** `partition::extract_neighbors_tiled` returns two `Vec<u8>` of at
> most 64 bytes for every predicted transform unit — 637,972 calls on one
> `gradient 2048x2048 p13` two-frame encode, **23.5 % of the whole process's
> allocations**, carrying 128 B of peak heap. Hoisting it into a thread-local
> scratch (the `txb_coeff_satd` shape) removes **488,413 / 499,865 allocations,
> 18-20 % of the process**, with peak heap and `.obu` unchanged to the digit —
> and then: CPU `after/before` 0.9944 / 0.9911 / 0.9903 / 0.9941 on aarch64 (a
> ~0.7 % win) but 1.0018 / 1.0062 / 0.9925 / 0.9978 on x86 (a null); peak RSS
> 0.995 / 1.019 on macOS (a null inside a 15 % spread) and **1.033 on Linux at
> 2048 inter — a 3.7 MiB REGRESSION whose fifteen paired rounds do not overlap
> ([114392, 115004] before against [118060, 118952] after)**. A memory chunk
> does not ship a 3.7 MiB peak-RSS regression for a 0.7 % single-ISA CPU gain,
> so it was reverted. Peak heap is unchanged, so the Linux rise is
> resident-page PLACEMENT, not live bytes.

> **CURRENT — CHURN CANNOT MOVE PEAK HEAP AND *DOES* MOVE PEAK RSS, AT ~100
> BYTES PER ALLOCATION ON macOS (2026-09-03). READ THIS BEFORE EVERY MEMORY
> BLOCK BELOW.** Record: `benchmarks/mem_churn_rss_2026-09-03.{tsv,meta}`.
> `benchmarks/mem_heaptrack_satd_2026-09-03.meta` concluded "removing allocator
> churn cannot lower a peak … the memory gap stays a LIFETIME property" from
> twelve heaptrack cells at +0.01 MiB. **True of peak HEAP, false of peak RSS**,
> and peak RSS is what `tools/mem_gate.sh` and the 25 % goal measure. Differencing
> the videokey and inter arms on gradient 2048x2048 qp 40 with the harness
> subtracted: from p13 to p6 the inter frame's LIVE cost FALLS 47 %
> (23.34 -> 12.44 MB), its macOS resident cost RISES 13 % (48.89 -> 55.39 MB),
> and its allocation count rises 112 % (214,114 -> 454,196). Resident-minus-live
> per allocation is **94.6 B (p6) / 119.3 B (p13) on macOS and -9.7 / +8.3 B on
> Linux**.
>
> **AND THE OBVIOUS CAUSAL READING OF THAT — "remove N allocations, get 100*N
> bytes back" — WAS TESTED DIRECTLY AND IS FALSE.**
> `partition::extract_neighbors_tiled` is the port's largest allocation-COUNT
> site (637,972 calls on that cell, 23.5 % of the process, 128 B of peak heap).
> Hoisting it into a per-thread scratch removes **488,413 / 499,865 allocations
> — 18-20 % of every allocation the process makes** — and moves macOS peak RSS
> by 0.995x at 1536 inter and 1.019x at 2048 inter, both inside a 15 % spread.
> **NULL.** So §2's bytes-per-allocation is a CORRELATION across two presets and
> not a lever; most likely these 64-byte allocations come from libmalloc's
> tiny/nano magazines, which reuse already-resident pages. The hoist is kept as
> a small CPU result (0.9903x-0.9944x over four interleaved cells) and NOT as a
> memory one. What survives for METHOD: a peak-heap null is not a peak-RSS null,
> and a memory claim must name which quantity it is about.

> **CURRENT MEMORY POSITION — THE PEAK IS NOW DECOMPOSED, AND `MeCandidate` WAS
> FIVE BYTES WHERE C'S IS ONE (2026-09-03, fourth chunk of the day). READ THIS
> BEFORE EVERY MEMORY BLOCK BELOW.** Records:
> `benchmarks/mem_massif_2026-09-03.meta` (the decomposition),
> `benchmarks/mem_mecand_2026-09-03.{tsv,meta}` (the A/B), harness now in the
> repo at `rust/tools/mem_peak.sh`.
>
> **1. THE PEAK DECOMPOSES, and heaptrack could not say so.** heaptrack's
> merged per-site totals do NOT co-occur (that file says so itself), so its
> table is a lead list. massif's PEAK SNAPSHOT is one instant, so its entries
> sum to the peak exactly — checked, to the byte. On gradient 2048x2048 p13
> qp40 the inter arm's 138.39 M is: harness 31.45, retained quantized
> coefficients 26.23 (`tx_unit_inner` + `funnel_block_decision`, one structure
> under two sites), the per-tile results 16.02, `DecodedPictureBuffer::refresh`
> 13.45, `MeB64Output` 12.53, the PA pyramid 12.50, four 4.19 M planes, the
> rest small. The time series shows a MONOTONIC RAMP peaking at the END of mode
> decision on every arm — peak heap is linear in how many superblocks have been
> decided but not yet entropy-coded.
>
> **2. TWO RECORDED CLAIMS ARE CORRECTED THERE.** (a) The harness is **31.45 M**
> on the inter arm, not 6.29: `perf_encode` holds THREE copies of the sequence
> (`y/u/v`, the `yuv` concatenation, and the owned `frames`) where
> `perf_c_encode` holds one, so the encoder-side inter-frame cost is port
> **+30.2 M** against C's +0.01, not the +37.65 M the earlier records state.
> (b) **The frame-wide coefficient store is at PARITY with C, not excess** —
> C's `pcs.c:348-368` allocates `quantized_coeff`, one `EB_THIRTYTWO_BIT`
> `sb_size x sb_size` FULL_MASK buffer per b64 for the whole frame, and that is
> the measured 26.18 M `svt_aom_pic_buf_desc_pool_ctor` entry in C's own
> heaptrack column. The port reproduces C's shape at C's size; it just builds
> it per frame where C pools it at init.
>
> **3. THE ONE REAL EXCESS FOUND, AND FIXED.** C's `MeCandidate`
> (me_sb_results.h:29) is five bitfields of ONE `uint8_t`; the port stored five
> `pub u8` fields, so `me_candidate_array` (85 x 23 entries per b64, live for
> the whole frame) cost 10.01 MB at 4 MP against C's 2.00. Packing it to one
> byte is **-8.01 M of peak heap at 2048x2048, -5.6 % to -5.7 % of the inter
> arm at every size, 0.000 on still and videokey** (the control: neither runs
> the picture ME), and the measured delta matches the predicted 8.008 MB to
> 0.1 %. CPU neutral (0.987-1.003 over six interleaved cells).
>
> **4. AND THE HARNESS ASYMMETRY IN (2a) IS FIXED** —
> `benchmarks/mem_harness_2026-09-03.{tsv,meta}`. `perf_encode` now streams each
> frame to the `.yuv` instead of concatenating it, and MOVES the caller's planes
> into frame 0 instead of cloning them, so it holds the same ONE copy of the
> sequence `perf_c_encode` does. Every delta is an exact multiple of the frame
> size (one frame on still, two on videokey, three on inter): **-6.29 / -12.58 /
> -18.88 M of peak heap at 2048x2048**. C's `.obu` is byte-identical before and
> after on all 12 cells, which is the control — C reads the file this code
> writes. **This is a HARNESS result, not an encoder one**: the encoder
> allocates exactly what it did before; what changed is how much of the reported
> peak belongs to the measuring binary.
>
> **5. WHERE THAT LEAVES THE RATIOS** (x86_64-linux, p13 qp40 gradient), after
> both changes:
>
> | | 1280 | 1536 | 1920 | 2048 |
> |---|---|---|---|---|
> | heap still | 0.605 | 0.622 | 0.640 | 0.644 |
> | heap videokey | 0.692 | 0.708 | 0.723 | 0.730 |
> | heap inter | 0.886 | 0.910 | 0.933 | **0.938** |
> | RSS still | 0.905 | 0.929 | 0.958 | 0.969 |
> | RSS videokey | 0.913 | 0.940 | 0.954 | 0.950 |
> | RSS inter | 1.035 | 1.048 | 1.122 | **1.086** |
>
> **Peak heap is below C on all twelve cells and peak RSS is inside 25 % on all
> twelve**, against `main`'s RSS inter 1.279x-1.334x on the same grid. **RSS and
> HEAP move by DIFFERENT amounts** — the `MeCandidate` change saved more RSS
> (5.03 MiB) than heap (3.13 MB) at 1280 and a third as much at 2048 — so never
> convert one into the other. These are x86_64-linux numbers; the aarch64 RSS
> series below is a different allocator and page size and has NOT been
> re-measured.
>
> **6b. AND THE ISA IS WORTH MORE THAN EITHER CHANGE — aarch64 is NOT x86 here**
> (`benchmarks/mem_aarch64_2026-09-03.{tsv,meta}`). The same two commits measured
> on aarch64-darwin at p13 take the inter arm from **1.360x-1.483x to
> 1.121x-1.257x** of C (-15 % to -18 % of peak RSS at every size), and eleven of
> the twelve cells land inside the 25 % goal — but **2048x2048 inter is 1.257x,
> over the line**, where the same commits on x86 read 1.086x. On the same cell
> the port's peak RSS is 153.7 MB on macOS against 117.7 MB on Linux, and its
> Linux peak HEAP is 112.73 MB: **~36 MB of the aarch64 number is not live
> bytes**, and no lifetime change can reach it. NOT ATTRIBUTED — 16 KiB pages,
> libmalloc's retention policy and thread-stack accounting are all candidates.
> **And the gap is measurably NOT live bytes.** Subtract each arm's harness (now
> exactly `n_frames * w*h*3/2` on both sides) and difference the arms: one inter
> frame costs **+23.34 MB of live heap, +21.3 MB of x86 RSS (0.91x live) and
> +48.9 MB of aarch64 RSS (2.09x live)** at 4 MP — 2.13x at 1536 as well. On
> macOS the inter path's resident cost is twice its live cost, so no change to
> what the port keeps ALIVE can close it. Two candidates are already ruled out by
> measurement: libmalloc's retention POLICY (`MallocNanoZone=0` /
> `MallocSpaceEfficient=1` move it <= 4 %) and a flat per-process ISA tax (the
> STILL arm agrees across the two ISAs to within 2-5 MB at every size, so page
> size, thread stacks and static tables cannot carry 36 MB). Leading hypothesis,
> unconfirmed: cross-frame region reuse. Its natural test — does peak RSS climb
> per encoded frame? — CANNOT BE RUN until the frame-2 refusal is lifted (at
> `SVTAV1_FRAMES` 3 and 4 the encoder still writes only `f0` and `f1`, and the
> RSS that does grow is the harness's extra input frames). The other half is an
> A/B of 2026-09-03's three allocation-site hoists (`58fa779e`, `fbd341b3`,
> `0c70f3fc`) on macOS RSS — recorded as a CPU win and a peak-heap NULL, they
> were never measured on the metric they could have moved. Neither has been run.
>
> **6. AND THE RATIO IS A FUNCTION OF PRESET — the port's WORST preset is the
> FASTEST one** (`benchmarks/mem_preset_2026-09-03.{tsv,meta}`). On gradient
> 2048x2048 qp 40, C's peak heap on the inter arm is **240.25 M at p6 and
> 120.12 M at p13** — it DOUBLES — while the port's moves 5 % (146.04 -> 139.62
> on `main`). So a memory ratio quoted without its preset is meaningless, and
> the p13 arm above is the port's hard case, not its typical one. After the two
> changes, all 24 cells of the p6+p13 grid are inside the goal on both metrics
> and only three exceed 1.0 at all (the p13 inter arm, 1.035x-1.122x); at p6 the
> port is **0.27x-0.62x of C on heap and 0.58x-0.90x on RSS**. Note the
> unexplained conflict with `benchmarks/mem_arms_2026-09-02.meta`'s 1.60x at p6
> and 4 MP: that number is aarch64, `capture_c_trace` on the C side, and a
> different tree, and this harness reads 0.84x on `main` at the same preset and
> size. Do not treat either as refuting the other until one harness has been run
> on both hosts.

> **CURRENT — THE ALLOCATION-SITE HOISTS: A MEASURED CPU WIN AND A MEMORY NULL
> (2026-09-03, third chunk of the day). READ THIS BEFORE THE MEMORY BLOCK
> BELOW.** Three byte-identical changes hoist the three biggest per-call
> allocation sites out of the allocator, and the pair of records they produced
> disagree in exactly the way the lifetime argument predicts: every cell moves
> on the CPU and **not one cell moves on the heap**.
>
> Commits: `58fa779e` (the RDOQ trellis's `dqcoeff` into the per-thread
> `TxScratch`), `fbd341b3` (`txb_coeff_satd`'s `residual` + `coeffs` into a
> per-thread scratch), `0c70f3fc` (`cost_coeffs_txb`'s and
> `cost_coeffs_txb_pd0`'s `coeff_contexts` into a fixed stack array).
> Records: `benchmarks/txscratch_dqcoeff_ab_2026-09-03.*`,
> `benchmarks/satdscratch_ab_2026-09-03.*`,
> `benchmarks/coeffctx_ab_2026-09-03.*`,
> `benchmarks/mem_heaptrack_satd_2026-09-03.meta`,
> `benchmarks/perf_2026-09-03-arm4-{still,videokey,inter}.*`.
>
> **1. THE THREE PAIRED A/Bs** (`tools/perf_ab.sh`, interleaved, order
> randomised per round, no `-C target-cpu=native`, gradient qp 40, aarch64 /
> Apple M4 Pro, EVERY cell `ident=Y`). Read each as a speedup over the tree
> immediately before it:
>
> | change | arm | best cell | worst cell | shape |
> |---|---|---|---|---|
> | `dqcoeff` -> TxScratch | still n=15 | 1.022x (256 p6) | 1.003x (512 p10) | four of six spans below 1.0 |
> | | videokey n=25 | 1.009x (128 p8) | 0.998x (256 p6) | NULL-to-marginal |
> | `txb_coeff_satd` scratch | still n=15 | **1.034x (256 p2)** | 0.991x (512 p10, NULL) | concentrated at the SLOW presets |
> | | videokey n=25 | 1.016x (512 p6) | 0.999x (128 p8) | |
> | `coeff_contexts` -> stack | still n=15 | 1.015x (256 p10) | 1.006x (512 p10) | **six of six move** |
> | | videokey n=25 | 1.015x (128 p8) | 1.006x (512 p8) | **six of six move** |
>
> The `coeff_contexts` change is the most uniform of the three — eleven of its
> twelve cells have their whole p25/p75 span below 1.0 — and it is also the
> cheapest: the buffer is `vec![0i8; txb_wide * txb_high]`, `adjusted_tx_size`
> caps that product at `32 * 32` for every reachable `tx_size`, so it needs no
> thread-local at all, only a `[0i8; cc::MAX_TXB_COEFF_AREA]` on the stack.
> **Check for that bound before reaching for a thread-local scratch.**
>
> **2. THE POSITION AFTER ALL THREE** (`perf_gate.sh`, three arms, one session,
> 25 paired rounds, sizes 64/128/256/512 at preset 8, gradient qp 40, all cells
> `ident=Y` except the 512 inter cell). Two independent runs of the same grid
> ten minutes apart agree within 0.01x on every slope ratio:
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio |
> |---|---|---|---|---|---|
> | still — arm3b | 0.87x | 1.50x | 2.42x | 2.61x | 2.70x |
> | still — arm4 | 0.85x | 1.50x | 2.43x | 2.59x | **2.70x** |
> | videokey — arm3b | 1.36x | 1.98x | 2.55x | 2.83x | 2.88x |
> | videokey — arm4 | 1.37x | 1.93x | 2.52x | **2.73x** | **2.77x** |
> | inter — arm3b | 1.68x | 2.16x | 2.66x | 3.00x* | 2.86x |
> | inter — arm4 | 1.65x | 2.15x | 2.60x | **2.90x*** | **2.81x** |
>
> READ THIS AS POSITION, NOT ATTRIBUTION — it carries C's own drift, and this
> session's C is 1.9-3.3 % faster than arm3b's at 512x512 on all three arms.
> The port moved 3.0 % (still), 5.5 % (videokey) and 4.6 % (inter) at that
> cell, so roughly 2-3.6 % of each is real and the rest is drift; the three
> A/Bs above are the attribution. **The video-mode key frame's slope ratio is
> 2.88x -> 2.77x and the inter cell's 2.86x -> 2.81x.**
>
> Re-differenced on the arm4 numbers (a subtraction of measured quantities),
> the port's excess over C on an inter cell splits:
>
> | size | still key frame | what the VIDEO config adds | the inter frame | total excess |
> |---|---|---|---|---|
> | 64  | -0.030 ms (-6.8 %) | +0.216 (**48.6 %**) | +0.258 (58.1 %) | 0.444 ms |
> | 128 | +0.167 (10.3 %) | +0.855 (**53.0 %**) | +0.592 (36.7 %) | 1.614 ms |
> | 256 | +1.575 (22.1 %) | +3.832 (**53.8 %**) | +1.715 (24.1 %) | 7.122 ms |
> | 512 | +6.351 (17.0 %) | +21.518 (**57.6 %**) | +9.505 (25.4 %) | 37.374 ms |
>
> (The 512 row differences an `ident=N` inter cell — two encoders making
> different decisions — and is quoted for shape, not as a precise figure.)
> **The video config is still the largest single item at every size, and its
> share is where the previous record left it (54-59 %).**
>
> **3. THE HEAP DID NOT MOVE AT ALL.** Twelve heaptrack cells on r7900x
> (1280/1536/1920/2048 x still/videokey/inter, gradient qp 40 preset 13, each
> cell's `.obu` checked non-empty first), comparing `061aae79` with
> `fbd341b3`: ten cells read **+0.01 MiB** and two read unchanged, where
> +0.01 MiB is `heaptrack_print`'s smallest printable difference. By
> subtraction at 2048x2048 the video config still adds **+14.88 M** to the port
> and one inter frame still adds **+43.94 M**, i.e. **+37.65 M encoder-side
> against C's +0.01 M** once the harness's own 6.29 MB input frame is removed
> from both sides — the same numbers `mem_heaptrack_2026-09-03.meta` recorded
> before any of this work.
>
> **This is the expected result and it is worth stating plainly: removing
> allocator CHURN cannot lower a PEAK.** All three buffers live for one
> transform unit (or one txb) and are freed before the next is taken, so they
> were never simultaneously live at the peak. The memory gap remains what
> `mem_heaptrack_arena_2026-09-03.meta` diagnosed — a LIFETIME property: the
> port holds the whole frame's decision tree, with a coefficient `Vec` per
> block, until the entropy walk, where C packs a superblock and releases its
> buffers. **Further allocation-shape hoists will keep paying in CPU and keep
> paying nothing in peak heap.** Do not price one as a memory change.
>
> WHAT REMAINS, as fractions of the video config's excess, from
> `benchmarks/perf_videokey_attrib_2026-09-03.tsv` (512x512 p8, unchanged by
> this chunk — the classes it moves are ALLOC and QUANT_RDOQ, both of which
> were already measured small): CDEF **11.8 %** (`cdef_filter_block` is NEON on
> BOTH sides, so a quality item, not a coverage gap), INTRA_PRED **9.3 %**
> (`dr_predictor_edged` at 10.7x, and a genuine coverage gap — C ships
> `svt_av1_dr_prediction_z{1,2,3}_neon`, the port's `dr_z{1,2,3}_edged` are
> scalar), LOOP_RESTORE 9.2 %, MD_DRIVER 6.6 %. ALLOC was 9.7 %; the three
> hoists above are the first bite out of it and the A/Bs say what that bite was
> worth.

> **MEMORY (2026-09-02, aarch64 / Apple M4 Pro) — read this before quoting any
> memory number.** `/usr/bin/time -l` max RSS, median of 7 runs per cell, plain
> `--release` port binary (NOT the `symtrace` wrapper the 2026-08-16 record
> used). Two records, and they answer different questions:
> * `benchmarks/mem_arms_2026-09-02.{tsv,meta}` — **the live numbers below.**
>   Three arms (still / video-mode key frame / inter), 42 cells, `perf_encode`
>   as the port binary, gradient qp 40.
> * `benchmarks/mem_2026-09-02.{tsv,meta}` — the wider preset x size sweep (56
>   cells, presets 2/6/10/13, qp 32) and the **inter completion frontier**. Its
>   STILL ratios are ~0.15x too high; see the correction below.
>
> Both supersede `benchmarks/mem_2026-08-16.meta`.
>
> The THREE ARMS are the same ones the CPU section below uses, so the two can be
> read together:
>
> | arm | 64x64 | 256x256 | 1 MP | 4 MP | port fit | C fit | slope ratio |
> |---|---|---|---|---|---|---|---|
> | still p13    | 0.61x | 0.73x | 1.01x | **1.01x** | 4.21 MiB + 20.61/MP | 6.12 + 19.90 | 1.04x |
> | videokey p13 | 0.67x | 0.75x | 1.09x | **1.11x** | 4.69 MiB + 25.22/MP | 6.67 + 22.27 | 1.13x |
> | INTER p13    | 0.81x | 0.93x | 1.39x | **1.60x** | 5.18 MiB + 42.04/MP | 7.19 + 25.46 | **1.65x** |
> | still p8     |       |       |       | 1.01x | 4.38 MiB + 20.67/MP | 6.32 + 19.88 | 1.04x |
> | videokey p8  |       |       |       | 1.11x | 4.59 MiB + 33.37/MP | 8.13 + 41.11 | **0.81x** |
> | inter p8 (64..512 only — refuses at >= 576) | 0.68x | 0.77x | — | — | 5.44 + 50.99/MP | 8.29 + 50.95 | 1.00x |
>
> (fit = `alpha + beta*pixels`; both terms are quoted because at 64x64 the
> intercept is ~100 % of the number and at 4 MP it is ~4 %.)
>
> **STILL and the VIDEO-MODE KEY FRAME both meet the 25 % memory goal at every
> size measured** — still is at parity from 1 MP up (1.00-1.01x) and lighter
> below it, and at preset 8 the port is LIGHTER than C for the video config
> (0.81x slope). **The whole memory gap is the INTER FRAME**, and it grows with
> size: 1.14x at 512x512, 1.39x at 1 MP, 1.60x at 4 MP.
>
> By subtraction of the arms, what ONE INTER FRAME adds to the peak (preset 13):
> 2.2 / 5.0 / 14.4 / 39.1 / **68.1 MiB** at 256/512/1024/1536/2048 against C's
> 0.8 / 1.5 / 3.9 / 7.8 / **13.2** — 2.8x rising to **5.2x**. At 4 MP that is
> 54.9 MiB of the 64.9 MiB total gap: **85 % of the inter path's memory excess
> is the inter frame's own footprint.** Per megapixel the port adds 17.0 MiB/MP
> for one reference and C adds 3.3; one 8-bit 4:2:0 reference is 1.43 MiB/MP of
> pixels, so C carries ~2.3x the raw reference and the port ~11.9x. The ratio
> GROWING with size says it is per-pixel state, not one oversized fixed
> structure.
>
> **THE INTER FRAME'S MEMORY IS NOW ATTRIBUTED TO ALLOCATION SITES, AND C'S
> ENCODER ADDS NOTHING FOR AN INTER FRAME (2026-09-03, heaptrack on r7900x /
> x86_64-linux — the first heaptrack run on this repo).** Record:
> `benchmarks/mem_heaptrack_2026-09-03.{txt,meta}`. Gradient 2048x2048 (4 MP)
> qp 40 preset 13, one encode per arm. **This is HEAP, not RSS** — a different
> quantity from every other memory number in this file.
>
> | arm | port | C | port/C |
> |---|---:|---:|---:|
> | still | 80.79 M | 115.72 M | **0.70x** |
> | videokey | 95.68 M | 113.82 M | **0.84x** |
> | inter | 139.61 M | 120.12 M | 1.16x |
>
> On the heap the port is LIGHTER than C for both one-frame arms; only the
> inter frame flips it. By subtraction the video config adds +14.89 M to the
> port and **-1.90 M** to C, and one inter frame adds +43.93 M to the port and
> +6.30 M to C.
>
> **C'S +6.30 M IS ENTIRELY ITS HARNESS.** C's peak-consumption site table is
> identical entry-for-entry between its videokey and inter arms —
> `svt_picture_buffer_desc_ctor` 41.63 M, `svt_aom_pic_buf_desc_pool_ctor`
> 26.18 M, `svt_aom_largest_coding_unit_ctor` 14.32 M,
> `picture_control_set_ctor` 9.77 M, all unchanged — with ONE exception:
> `main` goes 6.29 M -> 12.58 M, which is `perf_c_encode` reading one extra
> 2048x2048 I420 frame (6.29 MB exactly). **C allocates its picture-buffer pool
> up front and an inter frame reuses it; its marginal heap cost is ~0.** The
> SAME 6.29 MB sits on the port's side as `perf_encode::translate`, so the
> harness cancels and the comparison is harness-clean — which closes the
> warning the block below had to leave open. Encoder-side, one inter frame:
> **port +37.64 M, C +0.01 M.**
>
> The port's inter-only sites, with call counts (videokey -> inter):
> `RawVecInner::try_allocate_in` 18.87 -> 25.17 M (9 calls),
> `partition::funnel_block_decision` <0.5 -> **16.79 M (4096 calls)**,
> `inter_me::context::MeB64Output::new` **12.53 M (6144)**,
> `pipeline::encode_tile_rows::{closure#0}` **11.53 M (4110)**,
> `inter_me_arm::PaPicture::from_source` 4.77 -> 9.54 M,
> `encode_frame_impl::{closure#9}` 7.15 M, `RawVecInner::finish_grow`
> 0.156 -> 4.53 M (87,253 calls), `PaPlane::decimate` 1.48 -> 2.96 M.
> **A LEAD LIST, NOT A DECOMPOSITION** — heaptrack itself says merged per-site
> peaks are not correct as a sum, and they do not add to 139.61 M.
>
> **FIVE OF THOSE SITES REPRODUCE THE macOS LIST BELOW WITHIN ~10 %** on a
> different OS, ISA and allocator, which is the check that neither list is an
> artefact of its tool. The per-SB call counts (4096 / 6144 / 4110 for a
> 32x32-SB frame) say the shape of the fix: C pre-allocates once at
> `svt_av1_enc_init`; the port allocates per superblock. Do NOT re-run the
> thread-local Vec-pool experiment (`alloc_bufpool_null_2026-08-13.meta`
> measured it NULL); the prescription there — one arena at pipeline
> construction that the buffers are `&mut [T]` slices INTO — is the untried one.
> **CORRECTED 2026-09-03: that arena is now BUILT for the two biggest per-frame
> sites (`7acb8502`) and it measured NULL on twelve heaptrack cells. Pooling
> removes allocation CHURN, not live bytes: at a 2-frame peak both frames'
> structures are simultaneously live, and the recycle cannot even execute
> because it first fires on frame 2 and the encoder refuses frame 2. The gap
> is a LIFETIME property — the port holds the whole frame's decision tree with
> a coefficient `Vec` per block until the entropy walk, where C packs a
> superblock and releases its buffers. Read the block at the top of this file
> before planning any further allocator-shaped work.**
>
> ONE MORE TRAP, MEASURED: the C harness's FIRST inter run scored 12.66 M and
> was a REFUSAL — it needs a 2-frame `.yuv` and printed "short read (need
> SVT_FRAMES * w*h*3/2 bytes)" while writing no `.obu`. A memory number from a
> program that did not encode is smaller than the real one and looks like a win.
> Check the output file exists before quoting any arm.
>
> A LEAD LIST for it (`/usr/bin/heap` under `MallocStackLogging`, max live bytes
> per site over 12 inter / 8 still snapshots at 2048x2048 p13 — **snapshots, not
> a peak: the maxima do not co-occur and do not sum to 68.1 MiB**). Sites the
> inter arm holds and the still arm does not: `DecodedPictureBuffer::refresh`
> 12.89 MiB, `inter_me::context::MeB64Output::new` 12.50 (2048 nodes x 6,400 B),
> `inter_me_arm::PaPicture::from_source` 9.12, `encode_frame_impl::{closure#9}`
> 6.84, `cdef::apply_cdef_frame` 6.05, `inter_me_arm::PaPlane::decimate` 2.88.
> Sites that GROW: `funnel_block_decision` 0.62 -> 12.19, `RawVecInner::
> try_allocate_in` 8.05 -> 20.09, `encode_tile_rows::{closure#0}` 6.05 -> 11.06.
> The ME entries are the same `inter_me::hme` / `motion_estimation_b64` stages
> the CPU record puts at 54 % of the inter frame's distortion time, and
> `apply_cdef_frame` appears for the same reason the videokey arm turns CDEF on.
> **One warning from the same table: `perf_encode::translate` at 6.05 MiB is the
> HARNESS's translated frame.** Both harnesses grow with frame count, so the
> 68.1 / 13.2 MiB arm difference includes harness growth on both sides — the
> same trap as the `identity_run` correction above, one level down. Re-measure
> through a one-frame-at-a-time harness before treating 17.0 MiB/MP as a target.
>
> **CORRECTION to `benchmarks/mem_2026-09-02.meta`, measured the same day.**
> That record's port numbers came through `identity_run`, which holds ~14.5 MiB
> more at 4 MP than the encoder does: same library, same cell, only the harness
> binary changed — 102,832 KiB -> 88,480 KiB at 2048x2048 p13, and the same
> 14.5 MiB at qp 32 and qp 40, so the variable is the binary, not the quantizer.
> **Its STILL ratios are ~0.15x too high (1.17x should read 1.01x).** Its INTER
> ratios stand: the harness's extra buffers sit below the encoder's own peak once
> an inter frame is in flight, and both records put the inter path at 1.55-1.60x
> at 4 MP. Its frontier scan and its corrections to the 08-16 record also stand.
>
> Two caveats that remain load-bearing:
> * **No inter cell above 256x256 is byte-identical to C**, so the 1.39-1.60x
>   cells compare two encoders making different decisions. The inter cells that
>   ARE byte-identical (p8 at 64..256) put the port at 0.68-0.77x. Re-measure
>   once the inter byte frontier reaches 1 MP.
> * **The port's peak RSS is not single-run stable** — 2048x2048 p6 spans
>   126.2-136.6 MiB over seven runs (8 %) where C spans 0.04 %. It encodes
>   tiles on a thread pool; C at `--lp 1` does not. `tools/mem_gate.sh` takes
>   `MEM_REPS` (default 5) and reports the median with the spread.
>
> **THE INTER PATH NO LONGER PANICS ON ANY OF THE 64 CELLS** (2026-09-02,
> `tools/inter_completion_scan.sh`, `benchmarks/inter_completion_2026-09-02b.tsv`:
> **52 OK** — 5 byte-identical — 12 REFUSED, **0 CRASH**; partial-SB cells
> 33 OK / 3 REFUSED / 0 CRASH). The block below is kept because the three
> panics are what the memory series above was measured around, and because two
> of the three were still live when it was taken:
>
> * an off-the-end plane index in `port_md/md_search.rs:422` (frame 1, every
>   preset) — fixed by `628a19cda` BEFORE the first scan was even filed; that
>   scan's binary predated the commit by three minutes and reported it anyway
>   (see `docs/WORKING-ON-THIS.md` §2's trap).
> * `leaf must be tested` in `pd0.rs:2703` (frame 1, p8/p10/p13, sizes with SB
>   remainder 40) — `Pd0Ctx::pick_q` treated "no d1 shape to cost" as "must
>   SPLIT" and walked below `min_sq`, which only an inter frame's
>   `depth_removal_ctrls` raises above 8. Fixed, commit `4ae1ffb6`.
> * `video pic_pd0_lvl 7` in `pd0.rs:3337` (frame **0**, the KEY frame, p9/p10,
>   every size at or above the 360p->480p class boundary — 560x560 clean,
>   568x568 panics; this is why the p10 inter memory series stops at 512x512).
>   `pd0_detector`'s I_SLICE demote out of `PD0_LVL_6` was skipped. Fixed,
>   commit `4974a859`; all four key frames are now byte-identical to C, so the
>   p10 series can be extended past 512x512 whenever someone re-measures.
>
> The inter byte gate and matrix sweep {16,64,72,128}, of which only 72 is
> partial, so the frontier they describe is almost entirely 64-aligned and this
> surface was invisible to them; `inter_completion_scan.sh` is what sees it.

> **THE RDOQ TRELLIS IS MONOMORPHISED ON `tx_class` NOW, AND THE ARENA
> PRESCRIPTION BELOW IS CORRECTED (2026-09-03, second chunk of the day).**
> Commits `de4bfaf7` (tx_class), `7acb8502` (PA/ME pooling — a NULL),
> `061aae79` (tile-buffer reservation). Records:
> `benchmarks/rdoq_txclass_ab_2026-09-03.{videokey,still}.*`,
> `benchmarks/mem_heaptrack_arena_2026-09-03.meta`,
> `benchmarks/tile_reserve_ab_2026-09-03.*`.
>
> **1. `tx_class` MONOMORPHISATION — the untried remainder named in the block
> below is now MEASURED, and it is 1.006x-1.022x, not the ~24 % the class
> attribution implied.** `optimize_b` dispatches ONCE into
> `optimize_b_tc<const TC>` and the whole trellis plus the
> `nz_map_ctx`/`br_ctx` chain under it takes the class as a const generic, so
> the three-way branches fold exactly as C's `UPDATE_COEFF_EOB_CASE` macro
> expansion does. `tools/perf_ab.sh`, every cell `ident=Y`:
>
> | arm | 128 p6 | 128 p8 | 256 p2 | 256 p6 | 256 p8 | 256 p10 | 512 p2 | 512 p6 | 512 p8 | 512 p10 |
> |---|---|---|---|---|---|---|---|---|---|---|
> | videokey (n=25) | 1.014x | 1.008x | — | 1.022x | 1.013x | — | — | 1.015x | 1.021x | — |
> | still (n=15) | — | — | 1.007x | 1.007x | — | 1.011x | 1.006x | 1.014x | — | 1.017x |
>
> Twelve of twelve move; ten of twelve have their whole p25/p75 span below 1.0
> (128 p8 videokey at 0.9960 and 256 p6 still at 1.0019 are the marginal two).
> **It is a SMALLER win than the context-helper inlining that preceded it**
> (4.7 % videokey) — once the calls were gone, the branch that folds away was
> already cheap. Exhaustiveness of the three-arm dispatch rests on
> `tx_type_to_class` being ternary, pinned by
> `coeff_c::tx_class_tests::tx_type_to_class_is_ternary`; a second test pins
> every const-generic helper against its runtime wrapper at every class
> INCLUDING the unreachable fall-through, so the wrappers are byte-identical
> to the pre-monomorphisation code for every input rather than only the
> reachable ones. **This closes the RDOQ item.** What remains of RDOQ's excess
> is NOT MEASURED to a cause; the next probe is a per-symbol profile of
> `optimize_b`'s now-inlined body, not another dispatch change.
>
> **2. THE ARENA IS A NULL, AND THE PRESCRIPTION IT CAME FROM WAS WRONG ABOUT
> WHY.** `mem_heaptrack_2026-09-03.meta` prescribed "one arena allocated at
> pipeline construction that the per-SB buffers are `&mut [T]` slices INTO".
> That pool is now built for the two biggest per-frame sites — `PaPicture` /
> `PaPlane` gained `refill_*`, `FrameMe` gained `run_frame_me_into`,
> `MeB64Output` gained `reset`, and `EncodePipeline` carries
> `pa_scratch` / `me_scratch` — and it moves **nothing**, on twelve heaptrack
> cells (1280/1536/1920/2048 x still/videokey/inter), identical to the digit.
> Two reasons, both structural:
>
> * **The recycle first hands back an allocation on FRAME 2, and
>   `encode_frame_impl` REFUSES frame 2** (the coded-area-statistics refusal).
>   It is unreachable through the public encoder today.
> * **Even once it is reachable it cannot lower a 2-frame peak**, because at
>   that peak BOTH frames' pyramids and result sets are simultaneously live.
>   Pooling removes allocation CHURN — 3,072 mallocs per frame at
>   `MeB64Output::new` alone — not live bytes.
>
> **So the memory gap is a LIFETIME property, not an allocation-count one.**
> The port holds the whole frame's decision tree, with a coefficient `Vec` per
> block, until the entropy walk; C packs a superblock and releases its
> buffers. `funnel_block_decision` at 16.79 M over 2,048 calls and the
> `Vec<PartitionTree>` collect in `encode_tile_rows` are the same structure
> seen from two sites. **Nothing that only changes WHERE the bytes come from
> will close it; the next chunk has to change HOW LONG they live**, which is a
> pipeline restructure (pack per SB) and not an allocator change.
>
> A byte gate cannot witness the recycle either, and the attempt is recorded
> because it looks like coverage: a port-vs-port sweep of 270 cells over
> `SVTAV1_FRAMES` {1,2,3,5,8} reads 270/270 identical while every cell past
> two frames exits 3 at frame 2 and writes only what encoded. The positive
> control is `inter_me_arm::recycle_tests` — four tests, teeth measured by
> reverting the interior copy and the `me_mv_array` clear.
>
> **3. THE TILE-BUFFER RESERVATION IS THE ONE REACHABLE MEMORY WIN, AND ITS
> SIZE IS KEYED ON THE BINARY EXPANSION OF THE TILE AREA.**
> `encode_tile_rows` collected the tile's luma recon and its `PartitionTree`s
> by doubling; both are reserved exactly now, with two `debug_assert`s pinning
> `tile_recon.len()` to both the reservation and the capacity (teeth: `+ 1`
> fails 5 of 19 `pipeline` tests). Peak heap, INTER arm:
>
> | size | still | videokey | inter |
> |---|---|---|---|
> | 1280 | 0.0 % | 0.0 % | 56.38 -> 55.89 M (**-0.9 %**) |
> | 1536 | 0.0 % | 0.0 % | 81.54 -> 79.57 M (**-2.4 %**) |
> | 1920 | 0.0 % | 0.0 % | 123.56 -> 123.01 M (**-0.4 %**) |
> | 2048 | 0.0 % | 0.0 % | 139.61 -> 139.61 M (**0.0 %**) |
>
> **The size sweep IS the result.** 2048x2048 is exactly 4 MiB, so a doubling
> `Vec` lands on its payload with no slack and the change is worth nothing
> there; 1536x1536 is 2.25 MiB, so the doubling reserves 4 MiB and 1.75 MiB is
> slack. A single 2048 cell would have reported NULL and a single 1536 cell
> would have reported 2.4 % as if it generalised. CPU is NULL
> (0.998x-1.000x, n=25, every span crossing 1.0).
>
> GATED ON BOTH ISAs, at `061aae79`. **aarch64**: nextest 2502/2502 (bar 2498;
> +4 are `inter_me_arm::recycle_tests`; the tx_class commit added
> `coeff_c::tx_class_tests`'s two), `identity_full_8bit` 1100/1100,
> `regression_spotcheck` 83/83, `inter_byte_gate` 89 required / 0 failed / 0
> crashed, `video_key_matrix` 58/60, `fctx_gate` 96/96, `inter_decode_gate`
> 5/5, `inter_decode_census` 96/96, `screen_palette_gate` 50/50,
> `inter_completion_scan` (`SCAN_GATE=1`) 52 OK / 12 REFUSED / **0 CRASH**.
> **x86-64 (r7900x)**: 2247 dsp+encoder tests, `identity_full_8bit` 1100/1100,
> `regression_spotcheck` 83/83, `inter_byte_gate` 89/0 — run separately at
> `de4bfaf7` (2243 tests) and at `061aae79`. CI green on all four jobs.
>
> **TWO HARNESS TRAPS HIT WHILE MEASURING THIS, both the "a refusal is not a
> measurement" family.** (a) `perf_ab.sh` prints `measured 768x768 p6 ident=Y`
> for a cell the encoder REFUSES and contributes zero rows — `ident=Y` there
> means two empty files compared equal. **Read the `n` column of the `.tsv`,
> not the `measured` lines of the log.** (b) The 270-cell frame sweep above.
> Both are the same shape as the C-harness refusal
> `mem_heaptrack_2026-09-03.meta` records.

> **VIDEO-KEY CPU (2026-09-03, aarch64 / Apple M4 Pro) — the three arms
> re-measured in ONE session after the ME SIMD chunk, and the first attribution
> of the video-mode key frame. Records:
> `benchmarks/perf_2026-09-03-arm3-{still,videokey,inter}.{tsv,raw.tsv,meta}`
> (paired interleaved, 25 rounds/cell, gradient qp 40, p8) and
> `benchmarks/perf_videokey_attrib_2026-09-03.{tsv,meta}` (paired
> `/usr/bin/sample` profiles of BOTH arms on BOTH binaries at 512x512 p8).**
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio |
> |---|---|---|---|---|---|
> | still    | 0.90x | 1.51x | 2.53x | 2.71x | 2.81x |
> | videokey | 1.49x | 2.34x | 2.95x | 3.14x | 3.21x |
> | inter    | 1.76x | 2.43x | 2.99x | 3.29x* | 3.23x |
>
> (* 512 inter is `ident=N`, as in every prior record, and is out of the fit.
> Every other cell of all three arms is `ident=Y`.)
>
> **CORRECTION: THE VIDEO-MODE KEY FRAME IS NOW 50-64 % OF THE INTER CELL'S
> EXCESS, NOT 44-52 %.** The older figure below was measured BEFORE the ME SIMD
> chunk. That chunk cut the inter frame 1.75x and left the key frame untouched,
> so the key frame's share ROSE and the inter frame's FELL from 35 % to 20-21 %.
> Differencing the three arms above (a subtraction of measured quantities):
>
> | size | still key frame | video config on it | the inter frame | total excess |
> |---|---|---|---|---|
> | 64  | -0.020 (-3.8 %) | +0.262 (**50.4 %**) | +0.278 (53.5 %) | 0.520 ms |
> | 128 | +0.161 (8.3 %)  | +1.192 (**61.2 %**) | +0.594 (30.5 %) | 1.947 ms |
> | 256 | +1.648 (19.3 %) | +5.148 (**60.4 %**) | +1.730 (20.3 %) | 8.526 ms |
> | 512 | +6.585 (15.1 %) | +27.774 (**63.5 %**) | +9.385 (21.5 %) | 43.744 ms |
>
> **POSITION AFTER THE THREE CHANGES THIS FILE RECORDS BELOW** (compute_stats
> row-pair, nz-map offset table, context-helper inlining) — same tool, same
> grid, 25 paired rounds, all cells `ident=Y` except the 512 inter cell.
> Records: `benchmarks/perf_2026-09-03-arm3b-{still,videokey,inter}.*`.
>
> | preset 8, port/C | 64 | 128 | 256 | 512 | slope ratio |
> |---|---|---|---|---|---|
> | still — before | 0.90x | 1.51x | 2.53x | 2.71x | 2.81x |
> | still — after | 0.87x | 1.50x | **2.42x** | **2.61x** | **2.70x** |
> | videokey — before | 1.49x | 2.34x | 2.95x | 3.14x | 3.21x |
> | videokey — after | **1.36x** | **1.98x** | **2.55x** | **2.83x** | **2.88x** |
> | inter — before | 1.76x | 2.43x | 2.99x | 3.29x* | 3.23x |
> | inter — after | **1.68x** | **2.16x** | **2.66x** | **3.00x*** | **2.86x** |
>
> READ THIS AS POSITION, NOT AS ATTRIBUTION — it is a different session from
> the "before" row and carries C's own drift (C's 512 still frame reads 4.097 ms
> here against 3.854 ms an hour earlier, +6 %). The paired A/Bs recorded below
> are the attribution. What the position says is that the video-mode key frame's
> slope ratio moved **3.21x -> 2.88x** and the inter cell's **3.23x -> 2.86x**,
> the first movement on either since the ME SIMD chunk.
>
> Re-differenced on the AFTER numbers, the video config's share of the inter
> cell's excess is 54-59 % (was 60-64 %) and its excess at 512x512 is 23.6 ms
> (was 27.8): still 6.54 / video 23.60 / inter frame 9.74 of a 39.88 ms total.
>
> GATED ON BOTH ISAs, at `64d43c64e` (the four byte-identical changes of
> 2026-09-03: compute_stats row-pair, the nz-map offset table, the
> context-helper inlining, the cdef_find_dir NEON arm). **aarch64**: nextest
> 2496/2496 (bar 2494; +2 are the new
> `coeff_simd::nz_offset_2d_table_matches_the_generator` and
> `c_parity_cdef::find_dir_all_tiers_match_c`), `identity_full_8bit` 1100/1100,
> `regression_spotcheck` 83/83, `video_key_matrix` 58/60, `fctx_gate` 96/96,
> `inter_byte_gate` 89 required / 0 failed / 0 crashed, `inter_decode_gate`
> 5/5, `inter_decode_census` 96/96, `inter_completion_scan` (`SCAN_GATE=1`)
> 52 OK / 12 REFUSED / **0 CRASH**, `screen_palette_gate` 50/50.
> **x86-64 (r7900x)**: 2241 dsp+encoder tests, `c_parity_wiener` 5/5 including
> the all-tiers `compute_stats`, `identity_full_8bit` 1100/1100,
> `inter_byte_gate` 89/0, `regression_spotcheck` 83/83. CI green on all four
> jobs for every push. `screen_ibc_gate.sh` did not run on either host — it
> self-reports a HARNESS portability failure (literal path deps in its oracle),
> pre-existing and unrelated.
>
> The cross-ISA run is not a formality even though three of the four changes
> are aarch64-only or compiler directives: the cdef change also refactored the
> SCALAR path (splitting the shared cost tail out), and both new tests assert
> `permutations_run >= 2` with no excluded tokens, so the tier sweeps provably
> RAN on x86 rather than collapsing to the native arm.
>
> WHAT THE VIDEO CONFIG ADDS TO THE KEY FRAME, BY CLASS at 512x512 p8 (port
> delta / C delta of the same subtraction; the port's deltas sum to 39.4 ms
> against the 39.887 ms the paired gate measures, so the profile accounts for
> ~99 % of it):
>
> | class | port | C | ratio | excess | % of the 27.774 ms excess |
> |---|---:|---:|---:|---:|---:|
> | COEFF_CTX    | 7.037 | 2.265 | 3.11x | 4.772 | **17.2 %** |
> | CDEF         | 3.636 | 0.367 | **9.92x** | 3.269 | 11.8 % |
> | ALLOC        | 2.690 | 0.000 | **inf** | 2.690 | 9.7 % |
> | INTRA_PRED   | 3.271 | 0.687 | 4.76x | 2.584 | 9.3 % |
> | LOOP_RESTORE | 3.073 | 0.513 | 5.99x | 2.560 | 9.2 % |
> | QUANT_RDOQ   | 5.660 | 3.405 | 1.66x | 2.255 | 8.1 % |
> | MD_DRIVER    | 2.593 | 0.762 | 3.40x | 1.831 | 6.6 % |
> | LIBC_MEM     | 2.042 | 0.489 | 4.18x | 1.553 | 5.6 % |
> | RANGE_CODER  | 1.657 | 0.299 | 5.55x | 1.359 | 4.9 % |
> | DISTORTION   | 1.423 | 0.179 | 7.97x | 1.245 | 4.5 % |
> | FWD_TXFM     | 1.580 | 0.463 | 3.41x | 1.117 | 4.0 % |
> | INV_TXFM     | 1.511 | 0.487 | 3.10x | 1.024 | 3.7 % |
> | COEFF_WRITE  | 1.517 | 0.603 | 2.52x | 0.914 | 3.3 % |
> | SYNTAX_WRITE | 0.782 | 0.000 | inf   | 0.782 | 2.8 % |
> | DEBLOCK      | 0.361 | 0.098 | 3.67x | 0.263 | 0.9 % |
>
> Port symbols, ten largest additions: `nz_map_ctx` 4.236,
> `quant::optimize_b` 3.871, `restoration::compute_stats` 2.563,
> `cdef::cdef_filter_block` 1.556, `coeff_rate::cost_coeffs_txb` 1.338,
> `intra_pred::dr_predictor_edged` 1.321, `write_coeffs_txb_1d` 1.317,
> `_xzm_free` 1.283, `tx_pipeline::tx_unit_inner` 1.017,
> `OdEcEnc::normalize` 1.004. C's five: `svt_aom_quantize_inv_quantize` 3.000,
> `svt_av1_cost_coeffs_txb` 1.589, `svt_av1_compute_stats_neon` 0.468,
> `full_loop_core` 0.359, `av1_write_coeffs_txb_1d` 0.321.
>
> **THE CLASS TABLE UNDERSTATES RDOQ AND OVERSTATES COEFF_CTX. MEASURED:
> 94.8 % of `nz_map_ctx`'s self time is inside `quant::optimize_b`**
> (`tools/perf_profile/ancestor.py` over the videokey arm: 1,431 of 1,509 self
> samples of the `nz_map_ctx|nz_mag|br_ctx` family, 4.738 ms of the 50.326 ms
> frame; `cost_coeffs_txb` is 3.8 %, `write_coeffs_txb_1d` 0.9 %). C's trellis
> derives the same contexts INSIDE `svt_aom_quantize_inv_quantize`, so the
> classifier splits the port's RDOQ in two and leaves C's whole. Re-joined, the
> port's RDOQ is ~10.2 ms against C's 3.4 — **~3.0x and ~24 % of the video
> config's excess, the largest single item** — and COEFF_CTX proper drops to
> near parity. **Rank by excess, and join `nz_map_ctx` to RDOQ before ranking.**
>
> **LANDED FROM THIS ATTRIBUTION (2026-09-03): the 2D nz-map context offset is
> now a TABLE READ, as C's is.** `get_nz_map_ctx_from_stats` in C reads one byte
> out of `eb_av1_nz_map_ctx_offset[tx_size][coeff_idx]` (coefficients.h:178);
> the port re-DERIVED it per call — `adjusted_tx_size`, two log2 table loads, a
> row/col split and four branches — while a compile-time table built from that
> same `const fn`, and already pinned to the exported C data, sat unused next to
> it in `coeff_simd::NZ_OFFSET`. Byte-identical by construction (same
> generator), and pinned cell-by-cell by
> `coeff_simd::nz_offset_tests::nz_offset_2d_table_matches_the_generator`.
> A/B `tools/perf_ab.sh`, every cell `ident=Y`
> (`benchmarks/nzmap_table_ab_2026-09-03.{videokey,still}.*`):
>
> | arm | 128 p6 | 128 p8 | 256 p2 | 256 p6 | 256 p8 | 256 p10 | 512 p2 | 512 p6 | 512 p8 | 512 p10 |
> |---|---|---|---|---|---|---|---|---|---|---|
> | videokey (n=25) | 1.034x | 1.033x | — | 1.029x | 1.036x | — | — | 1.027x | 1.027x | — |
> | still (n=15) | — | — | 1.024x | 0.998x | — | 1.017x | **1.032x** | 1.017x | — | 1.000x |
>
> All six VIDEOKEY cells move 2.7-3.6 % with every p25/p75 span entirely below
> 1.0. On the still arm p2 moves most (1.024x / 1.032x) — the trellis runs
> hardest at the slow presets — and two cells (256 p6, 512 p10) are NULL with
> spans across 1.0.
>
> **AND THE SECOND HALF, SAME DAY: the six context helpers are
> `#[inline(always)]` now, which is a BIGGER win than the table.** Plain
> `#[inline]` was being declined — `nz_map_ctx` showed 4.61 ms of SELF time as
> its own symbol in a 50.33 ms frame, and `update_coeff_eob` pays that call
> four times per coefficient (`lower_levels_ctx_general` twice,
> `coeff_cost_general`/`br_ctx` twice). A/B against the table commit, every
> cell `ident=Y` (`benchmarks/nzmap_inline_ab_2026-09-03.*`):
>
> | arm | 128 p6 | 128 p8 | 256 p2 | 256 p6 | 256 p8 | 256 p10 | 512 p2 | 512 p6 | 512 p8 | 512 p10 |
> |---|---|---|---|---|---|---|---|---|---|---|
> | videokey (n=25) | 1.047x | 1.048x | — | 1.045x | 1.037x | — | — | 1.039x | 1.047x | — |
> | still (n=15) | — | — | 1.032x | 1.028x | — | 1.019x | 1.041x | 1.023x | — | 1.026x |
>
> **Twelve of twelve cells move**, every p25/p75 span entirely below 1.0,
> including the two still cells that were NULL for the table change. Stated as
> a PRODUCT of the two paired measurements and not as a third measurement, the
> two commits together are ~1.08x on the video-mode key frame and ~1.06-1.07x
> on a preset-2 still frame.
>
> Two further facts from the same profiles, both recorded because the next
> chunk needs them. (1) The STILL arm is the same shape: 97.5 % of the family's
> self time is under `optimize_b` there too (0.377 ms of 10.44), so this is a
> property of the port's structure, not of the video config. (2) On C's side
> `get_nz_map_ctx` **does not appear as a symbol at all** — `grep -c` over the
> whole videokey call graph returns 0 — because C's trellis is a `switch
> (tx_class)` over three macro-expanded loops (`UPDATE_COEFF_EOB_CASE`,
> full_loop.c) with `tx_class` a literal, so the whole context derivation
> inlines and constant-folds into `svt_aom_quantize_inv_quantize`. The port
> passes `tx_class` as a runtime `usize` and `nz_map_ctx` stays out of line.
> **Monomorphising the trellis on `tx_class` WAS the untried remainder here.
> It is now DONE and MEASURED at 1.006x-1.022x — see the block at the top of
> this file (`de4bfaf7`,
> `benchmarks/rdoq_txclass_ab_2026-09-03.*`), which is a good deal less than
> this attribution implied.**
>
> THREE THINGS THIS SAYS THAT THE 256x256 RECORD DID NOT.
> * **ALLOC is the third-largest item and C's is ZERO** — 2.690 ms added on the
>   port (`_xzm_free` 1.28, `calloc` entry 0.26, `madvise` 0.24, `_xzm_xzone_malloc`
>   0.22, tail) against **0.000** on C, every C symbol below the 0.02 ms cut. With
>   LIBC_MEM (`__bzero` 0.71, `_platform_memmove` 0.66, `_platform_memset` 0.62)
>   the memory-traffic pair is 4.24 ms, 15.3 % of the excess. NOT MEASURED: which
>   structure. This is a DIFFERENT population from the one
>   `benchmarks/alloc_bufpool_null_2026-08-13.meta` measured NULL — that pooled
>   the per-block decision `Vec`s; these are buffers the video config turns on.
> * **CDEF's 9.92x is the worst ratio of any class** and it outranks loop
>   restoration at this size. `cdef_filter_block` (1.556) is NEON on both sides
>   — a quality item; `cdef_find_dir` (0.709) is a coverage gap, still scalar by
>   source read, against `svt_aom_cdef_find_dir_8bit_neon`
>   (`cdef_block_neon.c:348`).
> * **QUANT_RDOQ as the classifier reports it is NOT a lever** (1.66x, the
>   closest to parity here) — but see the re-join above: it is the largest lever
>   once `nz_map_ctx` is put back where it belongs.
>
> SINGLE CELL: 512x512 p8. The class ORDER differs from the 2026-09-02 256x256
> record (CDEF and ALLOC move up), so neither ordering is size-independent.


> **ME SIMD COVERAGE LANDED (2026-09-02, aarch64 / Apple M4 Pro) — the five
> scalar motion-search kernels are now vectorised, and the INTER cell moved
> 1.15x.** Records: `benchmarks/{me_sad,ext_sad8,subpel_stream,me_dist,
> subpel_simd}_ab_2026-09-02.{tsv,raw.tsv,meta}` (five paired A/Bs, one per
> step) and `benchmarks/perf_2026-09-02-arm-inter-simd.{tsv,raw.tsv,meta}`
> (the port/C position afterwards).
>
> WHAT LANDED. `svtav1-dsp::me_sad` (new) exports two block primitives as
> tier-suffixed `#[arcane]` helpers — `block_sad_*` and `block_sum_sse_*`
> (`(SUM(a-b), SUM((a-b)^2))`) — with `scalar` / `neon` / `arm_v2` / `v3`
> arms. **`arm_v2` is the one that matches C**: `Arm64V2Token` bundles
> `dotprod`, so `vabdq_u8` + `vdotq_u32` is the shape of C's
> `svt_sad_loop_kernel*_neon_dotprod`. Every caller in the by-symbol table
> below now summons ONE token per call and runs its search loop inside the
> tiered body, so no target-feature boundary is crossed per search position:
> `inter_me::sad::sad_loop_kernel`, the three `ext_*_sad_calculation_8x8_16x16`
> kernels, `port_md::pme::pme_sad_loop_kernel`,
> `port_md::md_search::PlaneDistortion::{sad, variance, ssd}` and
> `motion_est::full_pel_search`. `dsp::subpel_variance::sub_pixel_variance`
> additionally STOPPED ALLOCATING (it held two heap buffers per call) and its
> two bilinear passes plus the variance accumulate are now SIMD, with the
> vertical pass fused into the accumulation so C's `H x W` `temp2` buffer is
> gone rather than merely smaller.
>
> CUMULATIVE, paired A/B `main@884f94e8f` vs the landed state
> (`tools/perf_ab.sh`, 25 interleaved randomised-order rounds/cell, no
> `-C target-cpu=native`, gradient qp 40, INTER arm). **Every cell
> byte-identical (`ident=Y`)** — this is a port-vs-port A/B, so it reads
> speedup, not a port/C ratio:
>
> | inter cell | p6 | p8 |
> |---|---|---|
> | 64x64   | 1.073x | 1.080x |
> | 128x128 | 1.083x | **1.154x** |
> | 256x256 | 1.067x | **1.150x** |
> | 512x512 | 1.107x | **1.158x** |
>
> **THE STILL AND VIDEOKEY ARMS ARE NULL, AS THEY SHOULD BE** — these are
> inter-path kernels. Still (n=15, sizes 64/256/512 x p6/p10): 0.993-1.023x,
> every span crossing 1.0. Videokey (n=25, 64/128/256/512 p8): 0.996-1.003x,
> every span crossing 1.0. An n=15 videokey pass had read 0.992x at 256x256
> with its whole span above 1.0; the n=25 re-measure put the span back across
> 1.0, so that was noise and is recorded here rather than dropped.
>
> POSITION AFTERWARDS (`perf_gate.sh`, port vs C, same 25-round paired design,
> `benchmarks/perf_2026-09-02-arm-inter-simd.*`). The preset-8 inter row of the
> table below, re-measured:
>
> | inter, port/C | 64x64 | 128x128 | 256x256 | 512x512 | slope ratio |
> |---|---|---|---|---|---|
> | p8 was (`perf_2026-09-02-arm-inter`) | 1.92x | 2.74x | 3.40x | 3.83x* | 3.67x |
> | p8 now | **1.82x** | **2.47x** | **2.99x** | 3.29x* | **3.22x** |
> | p6 was (`perf_2026-09-02-arm-inter`) | 2.26x | 2.80x* | 3.10x* | 3.40x* | — |
> | p6 now | 2.42x | 2.69x | 2.95x | 3.22x* | 3.01x |
>
> **The p6 64x64 cell went the WRONG WAY across sessions (2.26x -> 2.42x) while
> the same-session A/B measured it 1.073x FASTER.** Both can be true: the A/B
> is port-vs-port and C is not in it, and a perf_gate ratio moves when C's own
> absolute time moves. Do not read a cross-session perf_gate delta as an
> attribution — that is what the five A/B records are for. What the perf_gate
> is for is POSITION, and the p8 slope-ratio move (3.67x -> 3.22x) is the
> position claim.
>
> (* 512x512 is `ident=N` on both presets and is excluded from the fit, exactly
> as before. 128 and 256 at p6 are `ident=Y` in this run where the earlier
> record marked them `*`; that is the three inter chunks of 2026-09-02 that
> moved the byte frontier 55 -> 67 -> 89, not this change. **The p6 fit's
> intercept comes out NEGATIVE (-0.978 ms), so its intercept-ratio of 18.06x
> in the `.meta` is meaningless — read the p6 slope only.**)
>
> READ THE A/B FOR THE SIZE OF THE CHANGE AND THE PERF_GATE FOR THE POSITION.
> The two disagree in the expected direction: the A/B is port-vs-port in one
> session and cancels drift, while the perf_gate re-measures C in a different
> session and C's own absolute times move (the 2026-09-02 control block below
> measured ~13 % session-to-session movement in the absolute ms with the
> ratios reproducing within 3.4 %).
>
> WHAT LANDED ON THE INTER FRAME ITSELF, by DIFFERENCING the two arms above —
> a subtraction of measured quantities, never a projection. Both arms were
> measured with the same tool at n=25 on the same box within the same hour,
> but in SEPARATE `perf_ab` invocations, which is a weaker pairing than the
> 2026-09-02 three-arm record's (all three in one session) and is why the
> numbers are quoted to two figures:
>
> | p8 cell | inter FRAME, main | inter FRAME, landed | speedup |
> |---|---:|---:|---:|
> | 64x64   |  0.64 ms |  0.54 ms | 1.19x |
> | 128x128 |  1.48 ms |  0.98 ms | 1.52x |
> | 256x256 |  4.89 ms |  2.80 ms | **1.75x** |
> | 512x512 | 24.24 ms | 13.96 ms | **1.74x** |
>
> (inter FRAME = the 2-frame INTER cell minus the 1-frame VIDEOKEY cell, on
> each binary. 256x256's 4.89 ms reproduces the 2026-09-02 three-arm record's
> 4.65 ms within 5 %, which is the check that the differencing is sound.)
>
> THE C SIDE OF THAT DIFFERENCING WAS THEN MEASURED TOO —
> `benchmarks/perf_2026-09-02-arm-videokey-simd.{tsv,raw.tsv,meta}`, the same
> 25-round paired design, all four cells `ident=Y`. The videokey arm is
> UNCHANGED by this work, which is the control the A/B already implied:
>
> | videokey, port/C p8 | 64x64 | 128x128 | 256x256 | 512x512 | slope ratio |
> |---|---|---|---|---|---|
> | was (`perf_2026-09-02-arm-videokey`) | 1.52x | 2.30x | 2.95x | 3.14x | 3.19x |
> | now | 1.51x | 2.28x | 2.89x | 3.15x | 3.21x |
>
> Subtracting that from the inter arm gives the INTER FRAME's own port/C:
>
> | p8 cell | port ms | C ms | ratio | was |
> |---|---:|---:|---:|---:|
> | 64x64   |  0.461 | 0.160 | 2.88x | 2.94x |
> | 128x128 |  0.834 | 0.254 | 3.28x | 4.11x |
> | 256x256 |  2.386 | 0.687 | **3.47x** | **5.10x** |
> | 512x512 | 12.350 | 3.070 | (4.02x) | — |
>
> **TWO WARNINGS ON THAT TABLE, both of which the reader needs.** (1) The
> 512x512 row differences an `ident=N` inter cell and is parenthesised for that
> reason — it compares two encoders making different decisions. (2) It
> differences two ~4 ms numbers to get a ~0.7 ms one, which amplifies noise: the
> port-side inter frame at 256x256 comes out 2.39 ms here and 2.80 ms from the
> A/B differencing above, 17 % apart, and C's own 256x256 inter frame reads
> 0.687 ms here against 0.91 ms in the 2026-09-02 record. **Quote the A/B for
> what changed (1.75x on the inter frame at 256x256) and treat the 5.10x ->
> 3.47x as the direction and rough size of the position move, not a precise
> figure.**
>
> GATED ON BOTH ISAs, at `59f40fe99`. aarch64: nextest 2493/2493 (bar 2487,
> and it includes the two `tier_invariance` whole-encoder byte tests),
> `identity_full_8bit` 1100/1100, `inter_byte_gate` 89 required / 0 failed,
> `regression_spotcheck` 83/83, `video_key_matrix` 58/60, `fctx_gate` 96/96,
> `inter_decode_gate` 5/5, `inter_decode_census` 96/96,
> `inter_completion_scan` 52 OK / 12 REFUSED / **0 CRASH**,
> `screen_palette_gate` 50/50, `screen_ibc_fh_gate` PASS. x86-64 (r7900x):
> 2238 dsp+encoder tests, `inter_byte_gate` 89/0, `regression_spotcheck` 83/83,
> `identity_full_8bit` 1100/1100. **The cross-ISA run is not a formality here**
> — every SIMD arm added is hand-written per ISA, and `docs/SUSPECTED-C-BUGS.md`
> #6/#11/#20/#21/#26 are all "a SIMD kernel disagreed with its `_c` twin". The
> dsp tier-parity tests assert `permutations_run >= 2` with no excluded tokens,
> so the `_v3` arms provably RAN on x86 rather than being skipped.
> `screen_ibc_gate.sh` did not run on either host: it self-reports a HARNESS
> portability failure (literal path deps in its oracle), pre-existing and
> unrelated.
>
> WHAT THIS DOES NOT DO. It does not touch the VIDEO-MODE KEY FRAME, which the
> differencing below puts at 44-52 % of the port's excess on an inter cell
> (**50-64 % as re-measured 2026-09-03 — see the VIDEO-KEY CPU block above**) —
> a bigger item than the inter frame at almost every cell. The kernels it
> lands on are the inter frame's 61 % motion-search distortion.

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
> (**the inter row is SUPERSEDED** by the ME SIMD COVERAGE LANDED block
> above: re-measured 2026-09-02 after the ME kernels were vectorised it reads
> 1.82 / 2.47 / 2.99 / 3.29 with a 3.22x slope. The still and videokey rows
> stand — both arms measured NULL against that change. The DIFFERENCING
> below was taken on the pre-SIMD binary and its component ms are therefore
> pre-SIMD; its SHARES are what to reuse, not its absolute times.)
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
> all four sizes) — the most stable number in this table. **SUPERSEDED: after the
> ME SIMD chunk it is 50-64 %; the VIDEO-KEY CPU block above re-measures all
> three arms in one session.** Encoding the same key
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
> **ALL FIVE ARE NOW VECTORISED** (2026-09-02, see the ME SIMD COVERAGE LANDED
> block above). The table is kept as the ATTRIBUTION that motivated the work,
> and its numbers are pre-SIMD. What the five delivered together is 1.15x on
> the inter cell, NOT the 28x the last line of this block quotes — that gap is
> what the kernels would cost if the callers around them were free, and they
> are not. The delivered fractions, cell by cell, are in
> `benchmarks/*_ab_2026-09-02.meta`. `sub_pixel_variance` under-delivered
> against its 16.3 % share (1.014-1.030x for the whole SIMD step): the MD
> sub-pel tree evaluates many small blocks, down to 4x4, where neither the
> 16-wide nor the 8-wide arm fires. A per-block-size histogram of those calls
> is NOT MEASURED and is the next thing to measure there.
>
> Those five sum to **2.74 ms = 59 % of the port's inter frame. C's whole inter
> frame spends 0.099 ms on the corresponding kernels — a 28x gap.** Verified
> scalar by source read: none of `inter_me/sad.rs`, `dsp/subpel_variance.rs` or
> `port_md/md_search.rs` contains an `incant!`, `#[arcane]`, `#[rite]` or
> `magetypes` anywhere.
>
> **WHICH SEARCH STAGE that time is in** — `tools/perf_profile/ancestor.py`,
> nearest-named-ancestor attribution of the distortion kernels' self samples,
> with the videokey arm as the control (it must return ~0 and returns 0.09 ms
> against the inter arm's 2.93). Distortion in the INTER FRAME: port 2.835 ms,
> C 0.122 ms — **23.2x**.
>
> | stage | port ms | port % | C % |
> |---|---:|---:|---:|
> | **picture-level open-loop ME / HME** (`inter_me::b64::motion_estimation_b64`, `hme::prehme_core`, `hme::hme_level_{0,1}`) | **1.529** | **53.9 %** | 52.5 % |
> | `port_md::md_search::md_subpel_search` | 0.781 | 27.6 % | 29.5 % |
> | `port_md::md_search::md_full_pel_search` | 0.246 | 8.7 % | 9.8 % |
> | `port_md::md_search::pme_search_for_ref` | 0.052 | 1.8 % | (in `md_encode_block`) |
> | `leaf_funnel::inject::inject_candidates` | 0.062 | 2.2 % | — |
> | unattributed (inlined) | 0.152 | 5.4 % | 8.2 % |
>
> **The composition matches C's within ~2 points at every stage.** The port is
> not spending its search time somewhere C does not — it is spending ~23x more
> in the same places, in the same proportions. So the 28x is NOT the port
> issuing a categorically different set of searches at this cell; a per-block
> operation census would still be needed to rule out uniformly wider search
> areas, but the "extra stage" hypothesis is measured and does not hold here.
>
> **This corrects the framing that prompted the measurement.** The per-block MD
> searches recent chunks added are together 1.08 ms — 38 % of the inter frame's
> distortion, 23 % of the inter frame, 7 % of the whole 2-frame encode. The PME
> is **1.8 %** of the distortion, the smallest of the three; the MVP variance
> scan (`best_mvp_by_distortion`) does not appear as a distinct ancestor at all,
> i.e. it is at or below the ~0.02 ms floor. **The dominant half is the
> picture-level open-loop ME/HME, which is not per-block work and was not on the
> list of suspects.**
>
> **DO NOT budget a 23-28x SIMD win from these tables.** The still path already
> taught this: `aom_hadamard_8x8` was 1.88 % of p2 and delivered 1.031x because
> the caller's own scalar loop stayed. And this grid is a pure horizontal
> translation whose ME distortion is ZERO on C's side (§5 of
> `docs/WORKING-ON-THIS.md`) — a search that terminates on an exact match is not
> necessarily priced like one on real content, so the STAGE SHARES may be
> atypical in either direction. What does not depend on the content is that the
> 23x sits in kernels that are scalar on one side and NEON-dotprod on the other.
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
> **This queue is the STILL path's. The INTER path's five-kernel queue — the
> one the by-symbol block above ranks — was worked 2026-09-02 and is now
> EMPTY: all five are vectorised via `svtav1-dsp::me_sad`. Nothing below moved.**
>
> | port function | p6 share | p2 share | C counterpart |
> |---|---|---|---|
> | `restoration::compute_stats` (**already NEON** — quality, not coverage; **WORKED 2026-09-03, see below; REWORKED 2026-09-04 to C's six-step shape on both ISAs, 1.35x C per call on x86 — see the top of this file**) | **9.83 %** | 0.60 % | `svt_av1_compute_stats_neon` (5.1x) |
>
> **WHERE `compute_stats`'s 5.1x LIVED, AND THE FIX THAT LANDED (2026-09-03).**
> This paragraph used to record a structural read of the port's NEON arm and a
> prescription. **The read was right and the CONCLUSION WAS WRONG**; both are
> kept here because the wrong conclusion sat in this file (and in three agent
> briefs) for a day.
>
> WHAT WAS OBSERVED, and still holds: for each output row the old arm issued
> `win2 + win2*(win2+1)/2` = **1,274 separate `dot_i16_neon` calls** at
> `wiener_win = 7`, each re-entering with four `vdupq_n_s32` zeroed
> accumulators and leaving through a cross-lane `vaddvq_s32`.
>
> WHAT WAS CONCLUDED, and is FALSE: *"The MAC COUNT is inherent — `H` has 1,225
> entries and each needs `width` products — so the lever is not fewer
> multiplies, it is hoisting the horizontal reduce"*, plus a blocker that any
> such hoist *"needs a periodic drain into i64 and a test that pins the drain
> interval"*. Neither survives. Writing `k = (kk, ll)` and `t = (tt, mm)` for
> the window's column and row offsets,
>
> ```text
>   H[k][t] = sum_i dot( d[i+ll][kk .. kk+width], d[i+mm][tt .. tt+width] )
> ```
>
> the dot depends only on the PAIR OF `d` ROWS and the pair of column offsets,
> never on `i`, `ll` and `mm` separately — so most of those 1,225 dots are the
> same dot. Two collapses follow: **row-pair sharing** (parameterise by the top
> row and the row delta: 1,225 dots per region row become 322 per `d` row) and
> **column sliding** (for a fixed row pair and column delta the up-to-7 column
> positions are a sliding window,
> `P(c1+1) = P(c1) - A[c1]*B[c1+dc] + A[c1+width]*B[c1+dc+width]`, an exact
> O(1) update: 322 become 85). **1,274 dots per region row become 85 per `d`
> row plus M's 49 per region row — about 9x fewer multiply-accumulates**, and
> 134 dot CALLS instead of 1,274, so the per-call reduce overhead the old note
> blamed falls by the same factor as a side effect. This is the asymptotic
> shape C's `compute_stats_win7_neon` already had (its step 1 spends ~98
> products per pixel and steps 3-4 derive the rest from O(width+height) edge
> deltas) — the 5.1x was **C doing less arithmetic**, not C reducing better.
> And no i64 drain interval is introduced: the flush boundary is still ONE ROW
> of products accumulated in `i32`, the grouping the scalar core and the AVX2
> arm already document, so the `NEON_STATS_MAX_ROW` envelope is untouched.
>
> LANDED (commit on 2026-09-03). A/B `tools/perf_ab.sh`, interleaved
> randomised-order paired rounds, gradient qp40, aarch64 / M4 Pro, **every cell
> `ident=Y`**; records `benchmarks/compute_stats_rowpair_ab_2026-09-03.{still,videokey}.{tsv,raw.tsv}`:
>
> | arm | 128 p6 | 128 p8 | 256 p2 | 256 p6 | 256 p8 | 512 p2 | 512 p6 | 512 p8 |
> |---|---|---|---|---|---|---|---|---|
> | videokey (n=25) | 1.030x | 1.053x | — | 1.009x | 1.048x | — | 1.016x | 1.036x |
> | still (n=15) | — | — | 1.005x | **1.074x** | 0.990x | 1.005x | **1.074x** | 1.003x |
>
> **The two p8 STILL cells are the control and they are NULL** (both p25/p75
> spans cross 1.0): at preset >= 7 the still API produces no reconstruction, the
> post-filters are skipped and loop restoration never runs. Every p6 and
> videokey cell's span sits entirely below 1.0. Back-solving 1.074x against the
> 9.83 % p6 frame share puts the FUNCTION at ~3.35x faster, i.e. roughly 1.5x C
> rather than 5.1x — NOT the 9x the MAC count alone would suggest, because the
> sub-average build, `find_average` and the H scatter are unchanged. ~~The x86
> `_v3` arm still uses the old per-pixel gather; the same two collapses apply
> to it and are UNMEASURED there.~~ **SUPERSEDED 2026-09-04:** the x86 arm was
> MEASURED at 127x C per call on real content (the realimg record), and both
> arms — this row-pair NEON arm included — were replaced by C's six-step
> kernel (`cs_kernel!`), 1.35x C per call on x86 and 2.3x faster than the
> row-pair arm on the aarch64 kernel bench; record
> `benchmarks/compute_stats_cshape_2026-09-04.meta`.
> | `cdef::cdef_filter_block` (**already NEON** — quality) | 4.70 % | 2.73 % | 5.6x vs `cdef_filter_block_*_neon` |
> | `restoration::wiener_convolve_add_src` | **2.68 %** | 1.17 % | `svt_av1_wiener_convolve_add_src_neon` (10.3x) |
> | `cdef::cdef_find_dir` (**WORKED 2026-09-03 — the 15x was NOT available**) | **2.02 %** | — | `svt_aom_cdef_find_dir*_neon` (15x) |
>
> **`cdef_find_dir` — WORKED 2026-09-03, AND THE 15x IN THIS TABLE WAS NOT A
> BUDGET.** That 15x is the RATIO between two functions (the port's 0.918 ms
> against C's `cdef_dir_from_lines_neon` 0.069 at 512x512 p8), and a direct
> NEON vectorisation of the same algorithm gets **1.82x on the kernel**
> (`benches/kernel_tiers.rs`: 159 ns vs 289 ns) and 1.005-1.015x on the three
> p8 videokey cells with everything else NULL
> (`benchmarks/cdef_find_dir_neon_ab_2026-09-03.*`, every cell `ident=Y`). It
> is kept because it is strictly less work, byte-identical, and closes a tier
> coverage hole — not because it moved the frame.
>
> Where the rest of C's 15x is: C batches TWO 8x8 blocks per call
> (`svt_aom_cdef_find_dir_dual_8bit_neon`), reads 8-bit source with no u16
> widen, and vectorises the COST FOLD, which the port still runs scalar and
> which is now about a third of the kernel. **A ratio between two functions is
> an upper bound on a rewrite, not an estimate of one** — the same lesson
> `aom_hadamard_8x8` taught (1.88 % of p2, delivered 1.031x). Re-read every
> other "Nx" in this table with that in mind.
>
> One measurement from the attempt that generalises: a first version kept the
> eight direction accumulators in MEMORY (`[[i16; 16]; 8]`, `acc[off..off+8]
> += v`) and measured **1.56x on the kernel and NULL on the frame** — each
> accumulator is re-loaded immediately after being stored and the
> store-to-load latency serialises all eight rows. Register accumulators plus
> `vextq_s16` took it to 1.78x and a `vpaddq_s16` row-sum tree to 1.82x.
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

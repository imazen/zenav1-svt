# Performance status — G4 baseline (port vs C wall clock)

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
> | `restoration::compute_stats` (**already NEON** — quality, not coverage; **WORKED 2026-09-03, see below**) | **9.83 %** | 0.60 % | `svt_av1_compute_stats_neon` (5.1x) |
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
> sub-average build, `find_average` and the H scatter are unchanged. The x86
> `_v3` arm still uses the old per-pixel gather; the same two collapses apply
> to it and are UNMEASURED there.
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

# Working on this port — read this before your first change

The goal of this file is that you **fall into doing the right thing**. Every
rule below was paid for: each one exists because someone (usually an AI session,
often a very careful one) drew a confident wrong conclusion and cost hours.

---

## 1. The one-minute loop

```bash
cd rust
cargo nextest run --workspace -j 4      # ~3s, 1000+ tests. NOT `cargo test`.
tools/regression_spotcheck.sh           # ~90s, one cell per bug we ever fixed
```

That is your inner loop. Both must be green before you look at anything else.

The **spot-check** is the important one: every cell in it is the minimal
reproducer of a bug that once shipped, so a red cell *names its own regression*
instead of leaving you to bisect a 2,000-cell sweep.

## 2. The full sweeps — when you actually need them

```bash
tools/identity_full_8bit.sh             # ~25 min, 1036 cells, all presets
IF_TIER=real tools/identity_full_8bit.sh    # ~45 min, real corpora
python3 tools/coverage_matrix.py        # instant: what is COVERED, not what passes
```

Run these before landing anything that touches mode decision, partition,
quantization or the bitstream writer. Not on every edit.

**Read `coverage_matrix.py` output before you read any pass rate.** It prints
`--` for an axis with no cells. A missing cell count is a coverage claim nobody
tested, and it is strictly more dangerous than a failing one — see §5.

## 2b. Perf and memory — where they actually are

```bash
tools/perf_gate.sh          # port-vs-C wall clock, paired statistics
tools/mem_gate.sh 6         # peak RSS, port vs C, tiny -> large
tools/fp_cross_isa.sh       # transcendentals, this host vs emulated x86-64
```

**Wall clock, STILL (2026-08-13, aarch64):** port/C slope 3.77x at p2, 3.22x p6,
2.74x p10, 2.73x p13 — and the port is FASTER than C below ~64 px on the fast
presets (0.86-0.90x fixed cost). `docs/perf-status.md` leads with the live table
and a SIMD-coverage queue ranked by measured frame share; read that before
optimising anything, because the top entries are already NEON and the queue is
about quality, not coverage.

**MEMORY TRAFFIC IS THE STILL ARM'S CHEAPEST REMAINING WORK, AND YOU FIND IT
WITH `ancestor.py`, NOT WITH THE CLASS TABLE (2026-09-03, CURRENT).** Two
byte-identical commits (`ab7c5ed4`, `ee7a755f`, `96083a8e`) took the still
arm's slope ratio **2.48x -> 2.40x** (the port's own slope 37.37 -> 36.17
ms/MP against a flat C slope) and the videokey arm's 2.66x -> 2.58x; records
`benchmarks/{dqfull,levelscratch,mds3scratch}_ab_2026-09-03.*` and
`benchmarks/perf_2026-09-03-arm8-still.*`, full account at the top of
`docs/perf-status.md`. Both were dead buffer work — an inverse-transform input
that was a byte-for-byte copy of the buffer beside it, and four copies of one
stack array zeroed in full and then re-zeroed in part. **A class share names a
SYMBOL (`_platform_memset`), and the cause is always its CALLER**; run
`tools/perf_profile/ancestor.py <profile> '_platform_memset|__bzero'
'svtav1_|perf_encode' <arm_ms> <samples>` (and the memmove / malloc twins)
before planning any of it. The whole family is 15.6 % of the port's 512 p6
frame against C's 4.6 %.

**Wall clock, THREE ARMS (2026-09-03, CURRENT — read this before the 09-02
paragraph below).** Latest position, after the three allocation-site hoists
(`benchmarks/perf_2026-09-03-arm4-{still,videokey,inter}.*`, 25 paired rounds,
p8, gradient qp 40, two independent runs of the grid agreeing within 0.01x on
every slope ratio): **still 2.70x, videokey 2.77x, inter 2.81x** slope ratio;
per cell at 64/128/256/512 still 0.85/1.50/2.43/2.59, videokey
1.37/1.93/2.52/2.73, inter 1.65/2.15/2.60/2.90*. The video-mode key frame is
49-58 % of the inter cell's excess. **And the same three commits moved the
peak HEAP by nothing at all** — twelve heaptrack cells at +0.01 MiB or less
(`benchmarks/mem_heaptrack_satd_2026-09-03.meta`), because removing allocator
CHURN cannot lower a peak the churned buffers were never live at. The block
below is the previous position and its attribution.

**Wall clock, VIDEO-KEY (2026-09-03).** Three byte-identical changes landed the same day and moved
the video-mode key frame's slope ratio **3.21x -> 2.88x** and the inter cell's
**3.23x -> 2.86x** (`benchmarks/perf_2026-09-03-arm3b-*`, position; the paired
A/Bs `compute_stats_rowpair_*`, `nzmap_table_*`, `nzmap_inline_*` are the
attribution): `restoration::compute_stats` re-derived by row-pair correlation
(~9x fewer MACs), the 2D nz-map context offset read from the table C reads it
from instead of re-derived, and the RDOQ trellis's six context helpers promoted
to `#[inline(always)]` as C's are. The first two corrected recorded claims in
`docs/perf-status.md` — "the MAC count is inherent" and the drain-interval
blocker — so re-read that file's compute_stats block before quoting it. All three arms re-measured in ONE session after the ME SIMD
chunk: `benchmarks/perf_2026-09-03-arm3-{still,videokey,inter}.*`. At preset 8
the arms read still 0.90/1.51/2.53/2.71x, videokey 1.49/2.34/2.95/3.14x, inter
1.76/2.43/2.99/3.29x at 64/128/256/512. **The video-mode KEY frame is now
50-64 % of the port's excess on an inter cell, not the 44-52 % the paragraph
below quotes** — the ME work cut the inter frame and left the key frame alone,
so its share rose while the inter frame's fell to 20-21 %. What that key frame
is made of is attributed per class and per symbol at 512x512 p8 in
`benchmarks/perf_videokey_attrib_2026-09-03.{tsv,meta}`; the two things that
record says and no earlier one did are that **94.8 % of `nz_map_ctx`'s time is
inside the RDOQ trellis** (so the classifier's COEFF_CTX/QUANT_RDOQ split hides
the largest single item — re-joined, RDOQ is ~3.0x and ~24 % of the excess) and
that **the video config adds 2.69 ms of allocator work to the port and 0.000 ms
to C**. **BOTH OF THOSE LEADS HAVE NOW BEEN WORKED, and both paid less than the
attribution implied**: monomorphising the trellis on `tx_class` (the remainder
that record named) is **1.006x-1.022x**, not ~24 % (`de4bfaf7`,
`benchmarks/rdoq_txclass_ab_2026-09-03.*`), and the allocator prescription is a
**NULL** — see the memory paragraph above.

**Wall clock, INTER (2026-09-02, first measurement):** `PERF_FRAMES=2` /
`PERF_VIDEO=1` on `perf_gate.sh`; records
`benchmarks/perf_2026-09-02-arm-{still,videokey,inter}.*` and
`benchmarks/perf_inter_attrib_2026-09-02.{tsv,meta}`. At preset 8 the 2-frame
cell is 1.92x / 2.74x / 3.40x at 64/128/256 (slope ratio 3.67x). **Half the
port's excess is the VIDEO-MODE KEY FRAME, not the inter frame** — 44-52 % at
every cell measured. The inter frame alone is 5.10x C at 256x256 p8 and is 61 %
motion-search distortion in kernels that are SCALAR on the port and NEON-dotprod
in C (2.84 ms vs C's 0.122 — 23x). Nearest-ancestor attribution
(`tools/perf_profile/ancestor.py`) puts **54 % of that in the picture-level
open-loop ME/HME**, 28 % in `md_subpel_search`, 9 % in `md_full_pel_search` and
**1.8 % in the PME** — and C's proportions match within ~2 points at every stage,
so the port is spending ~23x more in the SAME places, not running extra searches.
Two warnings before sizing anything from it: this grid's ME distortion is ZERO on
C's side (§5), so the stage shares may be atypical; and the still path already
showed a 1.88 %-of-frame kernel deliver 1.031x because its caller stayed scalar.

**Per-gate wall clock (CI, x86-64 `ubuntu-latest`, measured 2026-08-27):**
`benchmarks/gate_wallclock_ci_2026-08-27.md` — every CI step's duration from
run 33101031800, so "how long does gate X take" has a measured answer. The
whole differential job is ~21 min; the three biggest single steps are the
SIMD tier-invariance suite (207 s), the workspace test suite (167 s) and the
C oracle build (141 s, cached since 2026-08-28). Local arm64 numbers differ;
measure with `time` and record the host.

**Memory — CHURN CANNOT MOVE PEAK HEAP AND DOES MOVE PEAK RSS, AT ~100 BYTES
PER ALLOCATION ON macOS (2026-09-03, CURRENT — read this first).**
`benchmarks/mem_churn_rss_2026-09-03.{tsv,meta}`.
`benchmarks/mem_heaptrack_satd_2026-09-03.meta` concluded "removing allocator
churn cannot lower a peak … the memory gap stays a lifetime property" from
twelve heaptrack cells reading +0.01 MiB. **That is true of PEAK HEAP and false
of PEAK RSS**, which is the quantity `mem_gate.sh` and the goal use. Measured on
gradient 2048x2048 qp 40, harness subtracted, differencing the videokey and
inter arms: between p13 and p6 the inter frame's LIVE cost FALLS 47 %
(23.34 -> 12.44 MB) while its macOS resident cost RISES 13 % (48.89 -> 55.39 MB)
and its allocation count rises 112 % (214,114 -> 454,196). Resident-minus-live
per allocation is **94.6 B at p6 and 119.3 B at p13 on macOS, and -9.7 to
+8.3 B on Linux**. So removing N allocations from a frame is worth ~100*N bytes
of peak RSS on macOS and nothing on Linux. **BUT THE CAUSAL READING OF THAT IS
FALSIFIED**: hoisting `partition::extract_neighbors_tiled` (the port's largest
allocation-COUNT site, 23.5 % of the process, 128 B of peak heap) into a
per-thread scratch removes 18-20 % of ALL the process's allocations and moves
macOS peak RSS by 0.995x / 1.019x — a NULL inside a 15 % spread — while making
**Linux peak RSS 3.3 % WORSE** at 2048 inter, over fifteen paired rounds whose
distributions do not overlap. That change was REVERTED and is not in the tree
(`benchmarks/neighbor_scratch_ab_2026-09-03.*`). So the bytes-per-allocation figure is a CORRELATION across two presets,
not a budget; do not plan work against it. What survives, and it is the part
that matters: **a peak-heap null is not a peak-RSS null, the two move
independently, and every memory claim must name which quantity it is about.**

**Memory — THE PEAK IS DECOMPOSED NOW, AND THE HARNESS WAS 31 MB OF IT
(2026-09-03, CURRENT — read this before every memory paragraph below).**
`benchmarks/mem_massif_2026-09-03.meta` + `benchmarks/mem_mecand_2026-09-03.*`;
harness `tools/mem_peak.sh` (`MP_MODE=heap` for heaptrack, default for max RSS).
**Use massif, not heaptrack, to ask what is live at the peak** — a massif peak
snapshot is one instant so its entries sum to the peak exactly (checked), where
heaptrack's merged per-site totals do not co-occur and cannot be budgeted. The
time series says the peak is a MONOTONIC RAMP that ends where mode decision
ends, on every arm. Three corrections it forces: (1) `perf_encode` holds THREE
copies of the input sequence where `perf_c_encode` holds one, so the port's
harness is **31.45 M** on the 2-frame arm at 4 MP and the encoder-side inter
frame costs **+30.2 M**, not the +37.65 M the older records state; (2) the
port's frame-wide retained coefficients (~26 M at 4 MP) are **at parity with C**
— `pcs.c:348-368` allocates the same `EB_THIRTYTWO_BIT` `sb_size x sb_size`
FULL_MASK buffer per b64 for the whole frame, measured as C's own 26.18 M
`svt_aom_pic_buf_desc_pool_ctor` entry — so "the port holds the frame's
coefficients and C does not" is FALSE; (3) the one real excess found was
`MeCandidate`, five `pub u8` fields where C's is five bitfields of ONE
`uint8_t`, costing 10.01 MB of live `me_candidate_array` at 4 MP against C's
2.00. Packed: **-8.01 M peak heap on the inter arm at 2048x2048** (-5.6 to
-5.7 % at every size), 0.000 on still and videokey, CPU neutral. Peak HEAP
port/C is now 0.70x / 0.84x / **1.096x** at 4 MP — all three inside 25 %. Peak
RSS port/C is 1.045x / 1.086x / **1.308x**, and **RSS and HEAP do not move
together**: this change saved 5.03 MiB of RSS against 3.13 MB of heap at 1280
and 2.72 MiB against 8.01 MB at 2048. Quote each from its own column.

**And the harness asymmetry in (1) is fixed** (`benchmarks/mem_harness_2026-09-03.*`):
`perf_encode` streams each frame to the `.yuv` and moves the caller's planes
into frame 0, so it holds the same ONE copy of the sequence `perf_c_encode`
does — **-6.29 / -12.58 / -18.88 M of peak heap at 4 MP on still / videokey /
inter**, every delta an exact multiple of the frame size, with C's `.obu`
byte-identical before and after (the control: C reads the file this writes).
**A HARNESS result, not an encoder one.** After both changes, x86_64-linux
p13 qp40 gradient: peak HEAP port/C is **below 1.0 on all twelve cells**
(0.605-0.644 still, 0.692-0.730 videokey, 0.886-0.938 inter) and peak RSS is
**inside 25 % on all twelve** (0.905-0.969 still, 0.913-0.954 videokey,
1.035-1.122 inter) against `main`'s 1.279x-1.334x on the inter arm. aarch64
NOT re-measured — different allocator, different page size.

**And the ISA is worth more than either change** (`benchmarks/mem_aarch64_2026-09-03.*`):
the same two commits on aarch64-darwin at p13 take the inter arm from
1.360x-1.483x to **1.121x-1.257x** of C, eleven of twelve cells inside the 25 %
goal and **2048x2048 inter at 1.257x, over the line**, where x86 reads 1.086x.
On that cell the port's peak RSS is 153.7 MB on macOS against 117.7 MB on Linux
and 112.73 MB of peak HEAP on Linux — **~36 MB of the aarch64 number is not
live bytes and no lifetime change can reach it** (unattributed: 16 KiB pages,
libmalloc retention, thread stacks are all candidates; there is no heaptrack on
macOS). **And the gap is measurably NOT live bytes**: subtract each arm's harness (now
exactly `n_frames * w*h*3/2` on both sides) and one inter frame costs +23.34 MB
of live heap, +21.3 MB of x86 RSS (0.91x live) and **+48.9 MB of aarch64 RSS
(2.09x live)** at 4 MP; 2.13x at 1536 too. On macOS the inter path's resident
cost is twice its live cost, so **no change to what the port keeps alive can
close it**. Two things it is NOT, measured: libmalloc's retention POLICY (the
env knobs move it <= 4 %) and a flat per-process ISA tax (the STILL arm agrees
across the ISAs to within 2-5 MB at every size). Leading hypothesis, still
unconfirmed: cross-frame region reuse. The natural test — does RSS climb per
encoded frame? — CANNOT RUN until the frame-2 refusal is lifted (at
`SVTAV1_FRAMES` 3 and 4 the encoder still writes only f0 and f1, and the RSS
that grows is the harness's extra input frames). The falsifying A/B for the
churn half is the three 2026-09-03 hoists measured on macOS RSS; not run.

**And a memory ratio without its PRESET is meaningless**
(`benchmarks/mem_preset_2026-09-03.*`): on gradient 2048x2048 qp 40 the inter
arm's peak heap is **C 240.25 M at p6 and 120.12 M at p13** — C DOUBLES — while
the port moves 5 % (146.04 -> 139.62). The port's worst preset is therefore the
FASTEST one. Across the p6+p13 grid after the two changes, all 24 cells are
inside the 25 % goal on both heap and RSS and only three exceed 1.0 at all (the
p13 inter arm); at p6 the port is 0.27x-0.62x of C on heap. **Unresolved
conflict**: `benchmarks/mem_arms_2026-09-02.meta` records 1.60x at p6 / 4 MP on
aarch64 through `capture_c_trace`, and `tools/mem_peak.sh` reads 0.84x on
`main` at the same preset and size on x86 through `perf_c_encode`. ISA, C
driver and tree are all confounded; run one harness on both hosts before
quoting either as the position.

**Memory — THE ARENA IS A NULL AND THE GAP IS A LIFETIME PROBLEM (2026-09-03,
CURRENT — read this before the heaptrack paragraph below).** The prescription
that paragraph ends with ("one arena allocated at pipeline construction") is
now BUILT for the two biggest per-frame sites — `PaPicture`/`PaPlane`
`refill_*`, `FrameMe::run_frame_me_into`, `MeB64Output::reset`, and
`EncodePipeline`'s `pa_scratch`/`me_scratch` (`7acb8502`) — and it moves
**nothing**: twelve heaptrack cells (1280/1536/1920/2048 x still/videokey/
inter) identical to the digit
(`benchmarks/mem_heaptrack_arena_2026-09-03.meta`). Two structural reasons:
the recycle first hands back an allocation on **frame 2** and
`encode_frame_impl` REFUSES frame 2, so it cannot execute through the public
encoder at all; and even once it can, at a 2-frame peak BOTH frames'
structures are simultaneously live, so pooling removes allocation CHURN and
not live bytes. **The memory gap is a LIFETIME property** — the port holds the
whole frame's decision tree, with a coefficient `Vec` per block, until the
entropy walk, where C packs a superblock and releases its buffers
(`funnel_block_decision` 16.79 M and the `Vec<PartitionTree>` collect in
`encode_tile_rows` are that one structure seen from two sites). Nothing that
only changes WHERE the bytes come from will close it. The one reachable win
found alongside was reserving the tile recon/tree buffers exactly
(`061aae79`): **-0.4 % to -2.4 % of the inter arm's peak and 0.0 % at
2048x2048**, because 2048x2048 is exactly 4 MiB and a doubling `Vec` lands on
its payload there with no slack — a single-size measurement would have
reported either NULL or 2.4 % and both would have been wrong. CPU null.

**A byte gate cannot witness a recycle that starts at frame 2, and the attempt
looks exactly like coverage.** A port-vs-port sweep of 270 cells over
`SVTAV1_FRAMES` {1,2,3,5,8} read 270/270 identical while every cell past two
frames exited 3 at frame 2 and wrote only what encoded. Same family:
`tools/perf_ab.sh` prints `measured 768x768 p6 ident=Y` for a cell the encoder
REFUSES and contributes zero rows — `ident=Y` there means two empty files
compared equal. **Read the `n` column of a perf_ab `.tsv`, not the `measured`
lines of its log**, and check the `.obu` exists before quoting any arm.

**Memory — WHERE THE INTER FRAME'S BYTES GO (2026-09-03, heaptrack on
r7900x / x86_64-linux, the first heaptrack run on this repo):**
`benchmarks/mem_heaptrack_2026-09-03.{txt,meta}`. **C's encoder adds NOTHING
for an inter frame** — its peak-consumption site table is identical entry for
entry between its videokey and inter arms, and its whole +6.30 M is
`perf_c_encode`'s own input buffer growing by one 2048x2048 I420 frame
(6.29 MB exactly). The port's harness costs the SAME 6.29 MB
(`perf_encode::translate`), so it cancels: encoder-side, one inter frame is
**port +37.64 M against C's +0.01 M** on the heap at 4 MP. On the heap the port
is LIGHTER than C for both one-frame arms (still 0.70x, videokey 0.84x); only
the inter frame flips it (1.16x). The lead list — `funnel_block_decision`
16.79 M over 4096 calls, `MeB64Output::new` 12.53 M over 6144,
`encode_tile_rows::{closure#0}` 11.53 M over 4110, `PaPicture::from_source`
9.54 M, `RawVecInner::finish_grow` 4.53 M over 87,253 — reproduces five of the
macOS `/usr/bin/heap` entries within ~10 % on a different OS, ISA and
allocator. It is a LEAD LIST, not a decomposition (the per-site peaks do not
co-occur). **Trap that run hit: the C harness needs a 2-frame `.yuv` for the
inter arm and its first run REFUSED with "short read", scoring 12.66 M — a
memory number from a program that did not encode. Check the `.obu` exists.**

**Memory (2026-09-02, supersedes the 2026-08-16 first-ever measurement):**
`benchmarks/mem_arms_2026-09-02.{tsv,meta}` is the live record (three arms,
`perf_encode` as the port binary); `benchmarks/mem_2026-09-02.{tsv,meta}` is the
wider preset/size sweep and carries the inter completion frontier, but its STILL
ratios are ~0.15x too high because it measured through `identity_run`, which
holds ~14.5 MiB more at 4 MP than the encoder does (measured; the correction is
written into both files). **STILL is at PARITY — 1.01x at 4 MP, 1.04x slope —
and so is the video-mode KEY frame (1.11x).** **INTER is not: 1.39x at 1 MP and
1.60x at 4 MP**, and 85 % of that excess is the inter frame's own footprint —
one inter frame adds 68.1 MiB at 4 MP where C adds 13.2 (5.2x). Which structure
is NOT measured; that needs an allocation trace, not RSS.
`MEM_FRAMES=2` runs the inter arm, `MEM_VIDEO=1` the video-key arm, `MEM_REPS`
sets the repeat count.
**Do not quote a MiB/MP figure at a size you did not measure** — the slope moves
with the range, which is why the gate prints the adjacent-pair slopes next to
the least-squares fit.

**Inter completion (2026-09-03, CURRENT):** `tools/inter_completion_scan.sh` +
`benchmarks/inter_completion_2026-09-03.{tsv,meta}` — of 64 video-mode cells,
**64 OK** (8 byte-identical), **0 REFUSED**, **0 CRASH**. Before measuring
anything on an inter cell, check that the port completes it — a refusal or a
crash makes any ratio a comparison against a smaller workload.

The twelve refusals the 09-02 scan had (568/576/1024/2048 square at p6/p8/p10)
were ONE refusal, and it named the wrong precondition: `use_ref_frame_mvs` at
`mfmv_level >= 2`, refused as needing "the TPL r0 and the references' own
is_mfmv_used". C only reads those when `scs->tpl` is on — `mfmv_controls` sets
`r0_th = tpl ? 0.1x : 0` and guards the whole block behind `if (r0_th)` — and
`get_tpl` (`Globals/enc_handle.c:3657`) returns 0 for `aq_mode == 0`, which this
port refuses to be anything else. **The rule was already ported, tier-1
C-parity-tested, in `port_enc_mode_config::tail::mfmv_controls`, with a doc
comment saying exactly that; `inter_hdr_arm` had re-derived it and refused, and
`inter_mvp_env` carried a THIRD copy spelled `mfmv_level == 1`.** §4's "grep
before you write the second" again. `mfmv_level` is 2 above R360p at preset
<= M8, and R360p's threshold is 314,880 luma samples — which is why 552x552
(304,704) was fine and 568x568 (322,624) was not.

Two of the three newly-byte-identical cells are from that lift (576x576 at p6
and p8, the smallest 64-aligned size past R360p). **The third, 256x256 p6, is
NOT** — 256x256 is R240p, where `mfmv_level` is 1 and the change is
bit-identical by construction; it moved somewhere else on main between 09-02
and 09-03. Attribute from the resolution class, not from the diff of two scans.

**Lifting a refusal made those cells MEASURABLE; it did not make them
identical.** 568x568 is a partial superblock and its frame 1 is 55 B against
C's 53 at every preset — the pre-existing partial-SB frontier (0 of 33
partial-SB cells byte-identical, before and after), not an mfmv effect.

**And the 33 partial-SB cells it unblocked immediately paid for themselves.**
The first thing measured on one of them (`gradient 168x168 q32 p8` frame 1,
against C's `SVT_PD0CFG_OUT`) was that every `me_*_distortion` was normalised
by the PICTURE's area instead of the superblock's, so all three
`disallow_below_*` decisions and hence `min_sq` were wrong on every partial
superblock — invisible while those cells panicked, and invisible to
`me_8x8_cost_variance`, which matched C throughout because it is computed
before the normalisation. Fixed; the other half of that join (C's
`fast_lambda_md` is per-SUPERBLOCK, the port's is per-frame) is in
`benchmarks/pd0_depth_removal_join_2026-09-02.md` and was **SOLVED 2026-09-02**
on a second cell: the per-SB factor is `update_lambda`'s
`stats_based_sb_lambda_modulation` block, keyed on `me_q_index`, which
`svt_av1_generate_b64_me_qindex_map` derives from `me_8x8_cost_variance` —
a quantity the port already computes EXACTLY. On `diag 72x72 q40 p6` frame 1
the derivation predicts factors 100 / 100 / 100 / 150 and C's dump reads
5182 / 5182 / 5182 / 7773 against the port's flat 6633, both points exact from
the port's own value as the base. That file carries the arithmetic, the C
sites and the cell to verify an implementation against; it also refutes the
earlier geometry reading (it is not "cropped width 40", it is "non-zero ME
cost variance"). **The map, the consumer and the corrected factor arm are all
PORTED and tested as of §1z²³; what is open is the WIRING**, and it is two
chunks rather than one — PD0's is a call-site edit, MD's needs a per-SB lambda
threaded through `inter_md_arm` / the funnel, and landing only the first would
price the partition search and the mode search against different lambdas. **A crash is not only a
crash: it is a region of the configuration space nothing can measure.**

**And with the crashes gone that grid describes the BYTE frontier honestly for
the first time, which is much narrower than the campaign's headline ratio.**
Of the 52 cells that complete, **5** are byte-identical on both frames: p6 2/12,
p8 3/12, **p10 0/12, p13 0/16**; 64-aligned 5/19, **partial-SB 0/33**; and every
one of the five is 64, 128 or 256 square. The campaign's 96-cell grid sweeps
{16, 64, 72, 128} — one partial size out of four, none above 128 — which is why
its ratio is so much higher. Neither number is wrong; they describe different
grids. **Quote the grid with the ratio.**

**TRAP, and it cost this file its own numbers: the FIRST completion scan
(`benchmarks/inter_completion_2026-09-02.tsv`, 24 OK / 6 REFUSED / 34 CRASH)
measured a binary built THREE MINUTES BEFORE a fix that was already landing.**
Its `port=` header names `/Users/lilith/tmp/perf1-bin/identity_run.nosym`, built
13:15 local; `628a19cda` ("the MD motion search read PAST the source plane")
was committed at 13:18. Twenty of its thirty-four crashes were that already-fixed
panic, so the scan described a tree that was never on `main` — and this
paragraph, `docs/perf-status.md`'s memory block and an agent brief all quoted it
as the frontier. The tell was in the panic text: "the len is 10816 but the index
is 10816", and 10816 = 104x104, an UNPADDED plane, which is precisely the field
that commit changed. **A scan's `port=` line is a claim about a BINARY, not
about a branch; check its mtime against `git log` before quoting it.**
`benchmarks/inter_completion_2026-09-02a.tsv` is the honest BEFORE — the same
grid re-measured on `main` at `fed59574` — at 38 OK / 8 REFUSED / 18 CRASH.
`benchmarks/inter_completion_2026-09-02b.meta` carries all three side by side.

## 3. Adding a fix? Add its cell.

When you fix a bug, add a line to `tools/regression_spotcheck.sh`. The rule,
which the file enforces on itself:

> A cell earns its place ONLY if it **failed before** the fix and **passes
> after**. If you cannot state the observed failure — the byte counts, the panic
> message, the decoder error — the cell does not go in.

A cell that never failed cannot detect the regression of a fix it never
witnessed. Two cells were rejected from that file on exactly this ground the day
it was written; one of them (`end_tx_depth`) is a real, faithful fix that is
byte-inert on everything measured, so it deliberately has **no** cell and says
so.

If the fix is a size/quality win rather than byte-identity, use the `ratio`
helper instead of `byte` — pretending a size fix delivered byte-parity is how a
registry rots into something people delete.

## 4. Evidence tiers — say which one you have

Ranked, strongest first. State the tier in your commit message.

1. **A differential against the real exported C symbol** (`crates/svtav1-cref` +
   a `c_parity_*.rs` test). This drives the actual C code.
2. **A byte-identity cell** against `capture_c_trace` (the real encoder).
3. **A decode gate** — `aomdec`/`dav1d` accept the stream, and the encoder's own
   recon equals the decoder's output.
4. **Hand-derived vectors traced against the C source.** The weakest tier. Use
   only when the C function is `static` with no exported symbol, and say so.

A transcribed oracle agreeing with transcribed code proves only that both were
transcribed the same way.

**And TWO transcriptions of the same C function will diverge — grep before you
write the second.** MEASURED 2026-09-02: `port_rc_process::compute_rd_mult` had
C's MD-lambda chain right, including the part that is easy to get wrong — the
rdmult BASE reads `ppcs->update_type` while the frame-type FACTOR reads
`update_lambda`'s OWN `gf_update_type`, and on a flat low-delay P GOP those two
DISAGREE (`LF_UPDATE` vs `ARF_UPDATE`). `pd0::inter_full_lambda_8bit`
re-transcribed the same chain, collapsed the two selectors, and was the copy
the inter path used: the inter frame's MD lambda was 244 792 where C's is
241 378, with a correct implementation sitting in the same crate and nothing
pointing one at the other. When you find a second transcription, do not just
fix it — PIN IT to the first with a sweep test, which is what
`pd0::inter_lambda_tests::it_agrees_with_port_rc_process_compute_rd_mult_over_a_sweep`
now does over 160 (qindex, update-type, lambda-weight) points.


### TRAP: the oracle's identity is a BRANCH now, not just the pin (2026-09-02)

`reference/svt-av1` is a separate repo (`imazen/zenav1-svt-c`) and was sitting on
a **detached HEAD at `3115c0c` that no branch or tag pointed at** — one `git gc`
from losing the reference the entire byte-parity campaign is measured against.
It is now preserved as `oracle-base-hdr-fork` and pushed.

The working tree is currently on `fix/suspected-c-bug-17` (`39f909e`), which
saturates two UB double-to-int casts (SUSPECTED-C-BUGS 17). **Measured inert:
`identity_full_8bit` 1100/1100 against a library rebuilt from it.** The outer
repo's submodule pin still reads `3115c0c`, so tree and pin disagree — if you
need to reproduce a number exactly, check out the branch above, do not assume
the pin.

Two things that will mislead you here:
- `crates/svtav1-cref/build.rs` has `rerun-if-changed` for its own shims ONLY,
  not for the C library sources. Editing C and running cargo does NOT rebuild
  the oracle. Force it with `ninja -C cbuild-static`.
- Object mtimes are useless for deciding whether a C edit is compiled in — a
  `.o` can carry the same minute as the source that postdates it. Grepping the
  object for a constant is ALSO useless: `(int)AOMMIN(v, (double)INT_MAX)` emits
  no INT_MAX constant on aarch64, because `fcvtzs` already saturates. Force the
  rebuild and diff behaviour instead of inspecting artefacts.

## 5. The harness traps

Each of these produced a confident wrong answer in a single day. They are not
hypothetical.

**A silent harness and a genuine absence are indistinguishable.** An inline
shell loop that never ran gave `grep -c` = 0, which was read as "this C rule has
no counterpart to fix". The rule was live at preset 7. **Before you trust a
ZERO, prove the probe fires somewhere.** Print a positive control. Prefer a
script *file* over an inline shell loop for anything whose result you will act
on.

**`SVTAV1_PACKTREE` appends.** `rm -f` it before every run. A first pass at a
per-preset IntraBC table reported preset 7 coding 3502 blocks when it codes
zero, because the counts were cumulative. It was caught only because p7 exactly
equalled p4.

**Never edit a shell script while bash is executing it.** Bash reads
incrementally; a mid-run edit corrupts the running script and killed a 300-cell
sweep mid-flight.

**Never rebuild Rust while a sweep is using the binary.**

**A `until ! pgrep -f <script>` waiter MATCHES ITSELF and never exits.** The
waiter's own command line contains the pattern, so `pgrep -f` finds it,
`! pgrep` is false forever, and the loop spins after the job it was watching has
long finished — a hang that looks exactly like "the sweep is still running".
MEASURED 2026-09-02: `identity_full_8bit.sh` printed `1100 / 1100` and the
waiters kept reporting RUNNING for another twenty minutes. Match the SCRIPT PATH
(`pgrep -f "tools/identity_full_8bit.sh"`), or watch the log's own completion
line, or use a pid — never a bare name the waiter also carries.

**A grid whose ME distortion is ZERO cannot witness a motion-keyed defect —
and the REASON it is zero was itself wrong for a day.** The inter campaign's
96-cell grid is synthetic content translated by exactly `SVTAV1_FRAME_SHIFT`
pixels, and **every superblock of every cell measured reports
`me_{64,32,16,8}x*_distortion = 0` and `me_8x8_cost_variance = 0` on C's side**
(2026-09-02, `SVT_PD0CFG_OUT`). That was read as "C's open-loop search finds an
exact match", and it is NOT what happens. MEASURED 2026-09-02 through
`SVT_HME_OUT` (`gradient 128x128 q40 p8`): C's LIST-0 search ends at
`p_sb_best_sad = 18816` with MV `(40,0)` — it does not find the match at all.
The zero comes from **list 1**, whose HME is skipped by
`set_final_search_centre_sb`'s `temporal_layer_index > 0 || list_index == 0`
guard so its full-pel search starts at (0,0) and walks straight onto the true
`(-3,0)` with SAD 0; `construct_me_candidate_array_mrp_off` then takes
`MIN(list0, list1)`. Full account: `docs/INTER-ENCODE-PLAN.md` §1z¹³, which
supersedes §1z¹². **The lesson is the one this section keeps repeating: a
statistic being zero does not tell you which code path zeroed it.** Everything
downstream that keys on ME distortion is still evaluated at its trivial
corner:
`set_depth_removal_level_controls`' three cost thresholds and both `dev_*`
comparisons, `compute_subres_th`'s `cost_64x64`, `compute_intra_pd0_th`. A fix
in any of them can be perfectly C-correct and move ZERO bytes on this grid, and
a DEFECT in any of them is equally invisible. When a chunk lands in that
region, say so and reach for real content or a non-integer displacement rather
than reading the flat verdict count as coverage.

**AN INTERPOSER READS THE CONTEXT AT ITS OWN CALL SITE, NOT AT THE BLOCK IT
NAMES.** `SVT_INJCFG_OUT`'s `PMEST` line prints `ctx->mvp_array`,
`fp_me_mv`, `fp_me_dist` and `best_pme_mv` from inside
`__wrap_svt_aom_update_mi_map` — and that wrapper is called from
`md_update_all_neighbour_arrays` (`product_coding_loop.c:669`), i.e. AFTER a
partition decision, once the whole depth has been searched. `ctx->blk_ptr` is
the named block's (so `CINTER` and the injector-config half are sound), but
every per-REFERENCE search field belongs to whatever block MD happened to
search last. MEASURED 2026-09-02: on `gradient 128x128 q40 p8` the `PMEST`
line for `mi=(0,0)` reported a list-1 `fp_me_dist` of 27 816 while three
sibling superblocks' real values were 26 961 / 24 670 / 26 643 — the numbers
happened to be close enough to look like a clean join and wrong enough to
chase. **Hook a function that runs INSIDE the thing you are measuring.**
`SVT_SUBPEL_OUT` wraps `svt_av1_find_best_sub_pixel_tree_pruned`, which is
exported and fires once per `(block, list_idx, ref_idx, search_stage)`, and
it is the join point for anything the MD motion searches touch.

**A gate that cannot reach a feature cannot guard it.** The panic-freedom gate
encoded `gradient` only — which never arms the screen-content detector — so
palette and IntraBC were switched off in all 64 of its cells, and it sailed past
two real out-of-bounds panics. Synthetic content also **never** codes an IntraBC
block at any preset (measured), so IntraBC can only be tested on the real screen
corpus. Ask what your test actually reaches, not what it nominally covers.

**A CRASH AND A WRONG BYTE ARE NOT THE SAME DEFECT, AND A HARNESS THAT
CANNOT TELL THEM APART WILL REPORT THE WRONG ONE.** MEASURED 2026-09-02:
`identity_diff_inter.sh` propagated a Rust panic's raw exit status (101),
and all three of its consumers classified a cell by asking only "was the
status 3 (a refusal)?" and "does `rs.obu.f0` exist?". **Frame 0 is written
BEFORE a frame-1 panic**, so a crash passed both checks and fell through to
the byte comparison, where a missing `rs.obu.f1` scores as "frame 1
differs". The port panicked on EIGHTEEN of the inter campaign's 96 cells
(`md_search.rs`'s source gather, off the end of an unpadded 72x72 plane)
and every one of them was counted as an ordinary F1DIFF — so
`docs/INTER-ENCODE-PLAN.md` §1z¹⁵'s "55 F1DIFF" was 37 divergences and 18
crashes, and four chunks ranked mechanisms against that frontier.
`inter_byte_gate.sh` was WORSE than the sweep, not better: `uniform 72 72
40 6` sat in its `OPEN_CELLS` printing `open ... known` for the whole
defect, because "this cell is allowed to differ" and "this cell panicked"
produced the same string. A known-open cell may DIFFER; it may never
crash. There is now an exit code 4 for a crash, a CRASH verdict in the
matrix, and a gate that fails on one from either list — and the fix was a
single field (`InterMdFrame.src` was the aligned plane where the
SB-extent-padded `sb_input` was already in scope, and already read by PD0
and by every straddling leaf's residual gather).

**A `cmp` "first differing byte" inside a length-prefixed container points
at the LENGTH, not at the defect.** The 72x72 cell above reports "first
differing byte 4" on a 22-vs-23-byte frame: byte 4 is the OBU size field
reflecting an extra byte that a block near the END of the tile produced.
Same misdirection as `vdiff_cell.sh`'s `FIRST DIVERGING OP: 0`. Localize
with the per-block dumps (`SVT_CINTER_OUT` vs `SVTAV1_PACKTREE`'s `PDV`
line), never with the byte offset.

**A gate hidden behind a FAILING gate accrues its own debt silently, and
"nobody looked" is how five chunks land on a red `main`.** MEASURED 2026-09-02:
`refusal inventory is current` had been the ONLY failing step of the only
failing job since §1s landed — `docs/REFUSED-CONFIGS.md` was one CONTRACT entry
stale — and every other gate in that run, including the x86-64 workspace suite,
was green. Five consecutive chunks pushed onto that red without reading it. The
FOUR steps after it were SKIPPED as collateral — `PORT-NOTE index is current`,
`regression spot-check`, `8-bit identity — EVERY preset 0..13` and
`screen-content palette identity` — so **CI had not run this repo's two biggest
byte sweeps since §1s**, and the first of the four had gone stale too (44
markers indexed against 45 in source). Both ledgers are generated files with
`--check` CI gates, which is the right design; what failed is the
habit `CLAUDE.md` already states — **check CI after pushing** — and the reading
that a skipped step is a passing one. Run both `--check`s locally before a push
if you have touched a refusal string or a `PORT-NOTE` marker; they take under a
second.

**Corpus gates look for their images in several roots, and say so when they
miss.** Fourteen gates once hard-coded `/root/work/codec-corpus/...` — the path
on one CI image — so on any other host every image `SKIP-MISSING`d.
`screen_palette_gate.sh` then reported `0 / 0 byte-identical` and failed
anti-vacuity; with the corpora found it is `50 / 50` with 38 palette-coding
cells. They now resolve through `tools/lib_corpus.sh` (`$ZENAV1_CORPUS_ROOT`,
then `~/work/zen`, then `/root/work`, then `~/work`). Same lesson as `ionice`
and `-Wl,--wrap`: **probe, never assume one host.**

**`tools/decode_diff` cannot build off the CI image.** Its `Cargo.toml` has a
literal path dependency on `/root/aom-rs/crates/aom-decode`, and Cargo path deps
take no env override, so `real_image_matrix.sh` and `screen_ibc_gate.sh` cannot
build their pixel-classification oracle elsewhere. They now fail with a message
that says exactly that. Treat it as a harness failure, never as a parity result.

**A parameter can be SHIFTED OUT of relevance before the code under test sees
it.** `svt_inter_predictor_light_pd1` calls `revert_scale_extra_bits`, which
shifts the sub-pel phases right by `SCALE_EXTRA_BITS` (6). The landed
`inter_predictor_light_pd1_8bit_matches_c` cell sweeps phases `(0,0)`, `(3,0)`,
`(0,9)`, `(15,15)` as raw Q4 values — **all four become (0,0)** after the
revert, so that sweep drives only the COPY corner of `svt_aom_convolve[][][]`
and reports four-corner coverage. Nothing in a pass/fail comparison can see it.
The fix is a positive control that asserts the phase SURVIVES the transform
(`c_parity_port_light_pd1_hbd.rs::the_four_dispatch_corners_are_actually_
reached`), and the same shape applies to any harness that feeds a value through
a normalising step before the code under test.

**A macro's name is not its arithmetic — check the definition before choosing
test values.** `ROUND_UV(x)` is `((x) >> 3) << 3` (definitions.h:348): a
multiple of **8**, not "an even chroma pair". A differential for the OBMC
chroma predictions used origins 3, 5 and 7, which ALL round to 0, so every
shift applied to them was inert and a `>> ss_x`-instead-of-`>> ss_y` mutation
passed the whole suite. Pick inputs the transform cannot collapse, and assert
that it does not collapse them.

**A CONSTANT AT A CALL SITE IS A CLAIM, AND ITS COMMENT IS NOT EVIDENCE. This
has now been the root cause SIX times.** The shape is always the same: a
derivation is ported and tier-1 gated, a caller hands its consumer a
`Default::default()` / `0` / `Some(1)` instead, and a comment beside the
constant explains why that is C-faithful. `dlf_level = 0`, PD0's `inter`
argument, `md_config.rs:948`, `was_intra: Some(1)`, `refresh_frame_flags: 0`,
and on 2026-09-03 `near_count_ctrls: Default::default()` — whose comment read
"C caps the NEAR DRL loop to ZERO unless this control is enabled ... so
`NEARMV` is absent exactly the way C makes it absent". The first clause is a
correct reading of C's `enabled == 0` arm; the conclusion is wrong because
`enabled` is **1 in all seven arms** of `set_cand_reduction_ctrls`, so C
injects up to three `NEARMV` candidates on every frame and the port injected
none (`docs/INTER-ENCODE-PLAN.md` §1z²⁶). **When you find a constant with a
justifying comment, go read the C table it cites and check the arm the
envelope actually takes** — the comment was written by someone who read one
arm. Grepping for `Default::default()` in a `*_arm.rs` injector/config
construction is a five-minute audit that has paid six times.

**A parameter that is genuinely inert should be SAID to be inert, not swept.**
The OBMC single-prediction functions pass `is_compound = 0`, and the
single-prediction kernels never read `conv_params->dst` — so their CONV_BUF
stride cannot be observed through them at all (measured: changing it leaves
every cell green). Sweeping it anyway would have looked like coverage. The port
reproduces the value for faithfulness and its module doc says why no test can
see it.

**A harness PRECONDITION is a coverage hole.** `identity_run`'s `crop:` mode
rejects odd dimensions ("I420 needs even dims"), so no gate cell could ever
encode an odd-height frame of REAL content. That precondition hid a public-API
panic (`unsupported partition shape (Horz4, 3)`) on a shape only real content
picks, through every sweep in this repo. It was found by a test that builds its
own planes and therefore skips the check. When a harness refuses an input,
write down what that makes untestable — the refusal is not the same as the
input being impossible.

**A `c_parity_*` oracle can be correct by LINKER LUCK. Two of them were.**
Both halves of this were found on 2026-08-31 by running the suite on
x86_64-linux for the first time; both had been green on aarch64-darwin all day.

- **Some C entries derive an argument from a POINTER'S ADDRESS.**
  `svt_aom_convolve8_{horiz,vert}_c` take no phase index: they recover the
  16-phase table and the phase from the filter pointer itself
  (`convolve.c:54-61`, `get_filter_base` = `ptr & ~0xFF`, commented "this
  assumes that the filter table is 256-byte aligned"). Real call sites satisfy
  it with `DECLARE_ALIGNED(256, …)`; a shim that forwards a Rust
  `&[i16; 8]` straight through does not, and C then applies the taps at
  `addr - (addr % 16)`. The Rust `SUB_PEL_FILTERS_8` static landed at
  `%16 == 0` in the aarch64 test binary (oracle right, by accident) and
  `%16 == 8` in the x86 one (oracle silently wrong, whole-block value
  mismatch) — and the residue moves between binaries on ONE host too: three
  builds on the Mac gave `%16` of 8, 8 and 0. **Stage caller-supplied filter
  taps into `_Alignas(256) int16_t table[16][8]`, replicated into every row**
  (`ref_shims.c:1124` had this right; `inter_me_shims.c` regressed it).
  Pinned by `convolve8_oracle_is_alignment_invariant`, which feeds the same
  taps from every 2-byte residue in a 256-byte window.
- **An RTCD function pointer is NULL on x86 and does not exist on arm.**
  `svt_memcpy` is a pointer in `.bss` (`common_dsp_rtcd.h:1083`) until
  `svt_aom_setup_common_rtcd_internal` runs; the header even provides a
  null-safe `SVT_MEMCPY` for call sites that might run early, and
  `C_DEFAULT/variance.c:92` does not use it. Under NEON devirtualization
  `svt_memcpy` is `#define`d to the concrete `svt_memcpy_neon`
  (`common_dsp_rtcd_neon_devirt.h:266`), so on aarch64 there is no pointer to
  be NULL. A shim entry that skips RTCD setup therefore works on the Mac and
  lands at `rip = 0x0` on x86. **Every shim entry point calls its
  `*_ensure_rtcd()` first**, even when the function it wraps is a `_c` spelling
  — the `_c` body can still reach an RTCD pointer.

- **AVX-512 kernels use ALIGNED stores, and only x86 has AVX-512.**
  `svt_av1_fwd_txfm2d_*_avx512` write columns with `vmovdqa32` (64-byte
  aligned); the real encoder satisfies that because every residual/coefficient
  buffer is `EB_MALLOC_ALIGNED`. A Rust `Vec<i32>` is 4-byte aligned, so the
  store faults — `SIGSEGV` inside `av1_fdct64_new_avx512`, not a NULL
  dereference. `ref_shims.c:1315` had already documented and solved the AVX2
  (`_mm256_load_si256`, 32-byte) form of this for `ref_quantize_b`; the
  transform shims re-hit it one ISA wider. **Stage caller buffers through
  `_Alignas(64)` scratch**, and copy the OUTPUT buffer in as well as out when
  the test prefills it and asserts C leaves untouched positions alone.

- **Every shim is compiled into ONE archive, so a `ref_*` name is
  workspace-global — and a duplicate definition is not a link error
  everywhere.** Two byte-identical `ref_get_wedge_params_bits` (one in
  `inter_pred_shims.c`, one added later in `md_subpel_shims.c`) linked fine
  under Apple's `ld64`, which takes an archive's first definition, and were a
  hard `rust-lld: error: duplicate symbol` on x86_64-linux that took the WHOLE
  workspace down at link time. `grep -rn ref_<name> crates/svtav1-cref/shims/`
  before you add a wrapper; if it exists, declare it in your Rust module and
  call the one that is already there.

The general rule: **a differential passes on the host you ran it on. Nothing
more.** Before a `c_parity_*` file is quoted as tier-1 evidence, run it on the
other ISA — `ssh r7900x` is the x86 box — because the ways an oracle can be
accidentally right (static layout, per-ISA dispatch tables, devirtualized
symbols, an ISA that simply lacks the instruction that would have trapped)
are all invisible from inside one host.

**Measured on 2026-08-31**, the first day the suite was run on both: three
separate lanes landed shims that were green on aarch64-darwin and broken on
x86_64-linux the same day — 2 tests (obmc), 7 (entropy_inter), 9 (transforms),
18 in total, plus a duplicate-symbol link break, one instance of each trap above. Every one of them re-broke a
pattern `ref_shims.c` had already solved and commented. **Before you write a
new shim, grep `ref_shims.c` for the entry closest to yours** — the caller
contract you need is very likely already written down there.

**A differential's GENERATOR has a contract, and the `_c` kernel is usually
domain-wider than the SIMD one — by different amounts per ISA.** The masked
d16 blend's C-vs-C control reported "C's dispatched blend disagrees with its own
`_c` kernel on 20 of 20 cells" **on x86 and nowhere else**, which reads as an
RTCD defect and is not one: the generator drew CONV_BUF values `% 40000`, and
C's x86 kernels multiply through `_mm_madd_epi16` (SIGNED int16), so they leave
`_c` at exactly 32768 while aarch64's unsigned NEON kernel never does. The
encoder cannot produce such a value — `svt_av1_jnt_convolve_2d_c`'s own assert
bounds an 8-bit compound entry to `< 16384`, and driving that convolve measures
`[2919, 12159]`. **Bound a generator by what the PRODUCER can produce, and prove
the bound by driving the producer**, not by a comment about the range. Full
measurement in `docs/SUSPECTED-C-BUGS.md` #19 (and #20, the aarch64 highbd
kernel that takes the 8-bit arm for every bit depth except 10).

Corollary for cross-ISA work: **a green on the wider ISA is not evidence about
the narrower one.** The aarch64 pass here was structural — NEON is unsigned end
to end — and told you nothing about x86, exactly as #11's aarch64 obmc alias
tells you nothing about the x86 table.

**An exported `_c` symbol is NOT pure C, and one RTCD table is not both of
them.** `svt_av1_resize_plane_c` is exported and looks like the scalar
reference, but on x86-64 its leaves go through the RTCD pointers to the AVX2
kernels — which write a fixed-width block regardless of the requested length
and disagree with their own `_c` twins below it. On aarch64 the same source
line resolves to `_c`, because `aom_dsp_rtcd.c`'s AARCH64 arm is `SET_ONLY_C`
for every resize symbol. So an unpinned differential compares the port against
a DIFFERENT function on each host: three tests were green on aarch64 and two
SIGSEGV'd on x86-64 at the same commit. Separately, `resize_multistep`'s
identity fast path calls `svt_memcpy`, an RTCD pointer owned by
**common**_dsp_rtcd.c — a shim that inits only the aom_dsp table leaves it NULL
and every identity cell jumps to address 0. When a shim drives C, hand it the
contract the ENCODER hands it: **both** tables, and a deliberate decision about
which dispatch tier the oracle is supposed to be. Two rules follow. (1) When a
host passes, ask whether it passed structurally or by luck — here it was
structural twice over (`SET_ONLY_C`, plus NEON devirtualization making
`svt_memcpy` a direct call with no pointer to be NULL), which is checkable with
`nm -u` on the object file, not by reading the source. (2) `rip = 0x0` in a
backtrace is an uninitialised function pointer, not an overread — get the
backtrace before you theorise about buffer sizes. Full measurement:
`docs/SUSPECTED-C-BUGS.md` #26, pinned by
`crates/svtav1-dsp/tests/c_parity_resize_avx2_divergence.rs`.

**On macOS there is no arithmetic-coder op trace IN-PROCESS — run the C side in
a Linux container instead.** `capture_c_trace` needs `-Wl,--wrap`, which
Apple's `ld64` lacks, so `build.sh` falls back to a byte-only driver and
`identity_diff.sh` degrades to a byte + header-field comparison. Byte verdicts
are unaffected; symbol-level localization needs a GNU-ld host, and
`tools/ctrace-linux/` is one:

```bash
# one cell, real content, full op-trace diff (crop:/file:/raw: all work)
tools/ctrace-linux/diff_cell.sh 96 88 33 4 crop:/path/to/screenshot.png
# the VIDEO-mode sibling (low-delay-P GOP on both sides, frame 0 diffed;
# the port's inter refusal is expected, not a failure)
tools/ctrace-linux/vdiff_cell.sh 64 64 40 11 diag
# raw driver, drop-in for tools/capture_c_trace/capture_c_trace's argv
SVT_TRACE_OUT=~/tmp/zenav1-ctrace/c.trace \
  tools/ctrace-linux/run.sh 96 88 33 4 in.yuv out.obu 8
```

It bind-mounts the repo READ-ONLY and builds the C lib + wrap driver into a
docker volume, so it can never write into the tree (in particular it can never
leave a Linux ELF where the macOS `capture_c_trace` wrapper would exec it). The
`wrap_recon.c` dump vars are forwarded and their paths mapped:
`SVT_CTREE_OUT` (join against `SVTAV1_PACKTREE` with `tools/tree_diff.py`),
`SVT_QLEVELS_OUT` (+ `_XY`/`_COMP`), `SVT_PICKPART_OUT`, `SVT_CCOEF_OUT`,
`SVT_CCOST_OUT`, `SVT_PART_OUT`, `SVT_SEED_OUT`, `SVT_PD0COST_OUT` (+ `_SBY`)
and `SVT_PD0CFG_OUT`. The last two are the PD0 pair: `SVT_PD0CFG_OUT` prints
what `svt_aom_sig_deriv_enc_dec_pd0` RESOLVED for each superblock (level,
subres step, early-exit thresholds, rate-estimation level,
`pd0_use_src_samples`) and `SVT_PD0COST_OUT` prints C's per-block PD0 RD, which
joins field-for-field against the port's `SVTAV1_PD0DBG` `PD0BLK` line. Four
guesses at C's video PD0 were recorded in `INTER-ENCODE-PLAN.md` §1h before
anything observed that function; §1i is what one dump replaced them with.
Scratch must live under
`$CTRACE_WORK` (default `~/tmp/zenav1-ctrace`) — paths outside it are refused
rather than silently written where the host cannot see them.

**`run.sh` is a drop-in for `capture_c_trace`'s ARGV — and argv does not carry
the CONFIGURATION.** Until 2026-09-01 it forwarded the dump-path vars and
nothing else, so `SVT_FRAMES` / `SVT_AVIF` / `SVT_INTRA_PERIOD` /
`SVT_HIER_LEVELS` / `SVT_PRED_STRUCT` / `SVT_CPU_FLAGS` / `SVT_TILE_*` /
`SVT_TUNE` / `SVT_MAX_TX_SIZE` / `SVT_CRF_OFFSET` / `SVT_CSP` /
`SVT_SUPERRES_KF_DENOM` were dropped at the container boundary. A caller
asking for the inter campaign's VIDEO-mode 2-frame GOP got a container that
encoded ONE STILL frame: no error, a valid `.obu`, a valid op trace — of a
different encode than the one requested. So the only op-trace oracle a macOS
host has could not localize anything in the campaign that needed it, and would
have answered confidently if asked. They are forwarded now (`CONFIG_ENV` in
`run.sh`); keep that list in sync with `capture_c_trace.c`'s `getenv` calls.

**The C submodule is a SYMLINK in every `jj workspace add` sibling**, which is
the layout `CLAUDE.md` tells you to work in. A symlink resolves outside the
`/repo:ro` mount, so the container followed it into nothing and
`incontainer.sh` reported the submodule as uninitialised — a setup failure that
reads like a missing `git submodule update --init`. `run.sh` now mounts the
resolved directory over it when `pwd -P` lands outside the repo.

**`identity_diff.py`'s OP INDEX is not reliable on a video cell; its BYTE
verdict is.** Its alignment assumes one frame per trace and the still driver's
prologue, so on a two-frame run it names an op that did not diverge. Use
`tools/ctrace-linux/optrace_first_diff.py` (which `vdiff_cell.sh` runs for you)
for the localization: it splits both traces on `W RESET` and compares C frame 0
against the port's REAL PACK writer, because a run creates more writers than it
packs frames — MEASURED on `gradient 72x88 q40 p4`, where C has 2 segments and
the port has FIVE (the per-SB CDF-chain simulation and the tile re-walks each
have their own). Concatenating the port's segments reports a divergence at op 3
of a byte-IDENTICAL cell. It also normalizes C's `BOOL` / `BOOLEQ` spellings
against the port's 2-symbol `CDF` writes; without that, a raw diff of the two
traces disagrees on every literal bit. Its positive control is any
byte-identical video cell ("op streams identical").

When it names an op, grep the printed `icdf` value in
`crates/svtav1-encoder/src/entropy/default_cdfs.rs` — that names the CDF table,
and the table names the syntax element. That is how a `tx_size` symbol written
under TX_MODE_LARGEST was found (`docs/INTER-ENCODE-PLAN.md` §1j) on a cell
whose tree, every leaf field, every luma level and all three recon planes
already equalled C's.

**Verify the container oracle before trusting a trace from it.** Encode a cell
that ALREADY agrees on the host and confirm the container's C bytes are
identical; only then read the trace. Done for issue #15 on Linux arm64 vs
macOS arm64: identical on the diverging cell (`terminal` 96x88 p4 q33, 523 B)
and on an aligned control (`terminal` 64x64 p4 q33, 297 B, where port == C ==
container-C). Re-done 2026-09-01 for the VIDEO-mode path the config
passthrough opened up: `SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0
SVT_PRED_STRUCT=1 ./run.sh 64 64 40 6 rs.yuv c.obu 8` gives 961 + 22 B, and
its `c.obu.pts0` is BYTE-IDENTICAL to the host driver's — so `SVT_CTREE_OUT`
from the container is a trace of the same encode the byte gate compares. Build
the image for the SAME architecture as the host oracle (`run.sh` does) — C's
kernels are runtime-dispatched, so an x86 container is a different oracle, not
the same one.

**A byte gate run CONCURRENTLY with a cargo build reports a fake encode
failure.** `tools/identity_run` re-checks freshness on every invocation, so a
parallel `cargo build` (or a second gate script) holds the build lock and the
cell comes back `[port failed to encode]` — indistinguishable in the summary
from a real panic. MEASURED 2026-09-01: `regression_spotcheck.sh` reported
`64 / 65` with `cropped-tx-72x88` failing, and `65 / 65` when re-run alone at
the same commit. Run the byte gates ONE AT A TIME, and treat "port failed to
encode" as "check for a concurrent cargo" before treating it as a regression.

**Editing the tree DURING a byte sweep does the same thing from the other
side.** `identity_run`'s rebuild fails on a half-finished edit, and
`identity_full_8bit.sh` records the cells that were in flight as `RS_ERR`.
MEASURED 2026-09-01: `1091 / 1100` with the last NINE cells failing
contiguously and all nine passing individually. A CONTIGUOUS TAIL of failures
is the tell — a real regression does not respect sweep order.

**An EMPTY `OPEN_CELLS` array aborts a `set -u` gate on macOS's `/bin/bash`.**
Every frontier gate here has the shape `PASS_CELLS` + `OPEN_CELLS`, and moving
the last open cell to `PASS_CELLS` is how progress is recorded — but on bash
< 4.4 (`/bin/bash` is 3.2.57 on every macOS) `"${arr[@]}"` on an EMPTY array
under `set -u` is an "unbound variable" error. So the moment
`inter_decode_gate.sh`'s open list emptied, the gate printed five green
required cells and then **aborted**, nonzero, at the `for` — a state
indistinguishable in a summary line from a real gate failure, and one that a
handoff brief recorded as "PASS" because it had been run under a newer `env
bash`. MEASURED 2026-09-02. Write `${ARR[@]+"${ARR[@]}"}` in every such loop;
both inter gates do now. Same shape as the corpus gates' `0 / 0 identical`:
**a gate that cannot finish is not a gate that passed.**

**Every byte gate in this repo compares the port to C. NONE of them asks
whether the port's own bytes are a bitstream.** Running a real decoder over
the port's stream is a different question with a different answer: on
2026-09-01 the first `dav1d` run over the experimental 2-frame inter stream
found SIX defects in one afternoon, every one of them invisible to every byte
count here, because each wrote a symbol a decoder does not read (or read one
it does not write) and showed up only as a desync. One of them was hidden
behind a `debug_assert!` — and `identity_run` builds RELEASE, where the assert
is compiled out. `tools/inter_decode_gate.sh` is that question as a gate
(evidence tier 3); `tools/decode_conformance.sh` is its still-picture sibling.
**Decode the control first**: C's stream must decode, or the finding is about
the decoder, not the port.

**A DOC COMMENT ASSERTING AN INVARIANT IS NOT THE CODE THAT MAINTAINS IT — and
when the two disagree, the comment is usually the thing that let the gap in.**
MEASURED 2026-09-02: `pd0::video_pd0_mode` panicked on `pic_pd0_lvl` 7 under a
comment reading "the video ladder never assigns a level whose `pd0_level` is
`PD0_LVL_6` at the presets this port encodes. C asserts the same invariant at
`:2514`." Both halves were wrong in the same direction. The ladder DOES assign
it — `set_pic_pd0_lvl_default`'s M9..M10 row gives `lpd0_lvl` 7 to any base
picture above the 360p class with NORMAL coefficients — and what makes C's
assert hold is `pd0_detector`'s I_SLICE demote, i.e. the very code the port had
skipped. The port had a faithful transcription of that demote sitting in
`port_pd0_detector`, unused on this path. **When you write "C guarantees X",
name the LINE that guarantees it and check the port runs it.** A citation to an
assert is a citation to a POSTcondition; the precondition is somewhere else.

**A SCAN'S `port=` LINE IS A CLAIM ABOUT A BINARY, NOT ABOUT A BRANCH.**
MEASURED 2026-09-02: `benchmarks/inter_completion_2026-09-02.tsv` reported 34
crashing cells, and twenty of them were a panic that had been fixed on `main`
three minutes before the scan's binary was built. That number reached two docs
and an agent brief as "the frontier". Re-measuring the same grid on `main` gave
18. **Before quoting a recorded sweep, check its binary's mtime against
`git log` for the files it exercises** — and prefer re-running it to quoting
it, which for that scan costs fifteen seconds.

**A "TRIED IT, DOES NOT WORK" NOTE IS A CLAIM ABOUT ONE IMPLEMENTATION.**
`docs/INTER-ENCODE-PLAN.md` §1q recorded that interleaving a `get_packet`
between `send_picture` calls "makes it WORSE: the 2-frame cell then segfaults
too", and closed with "do not fix it again without a measurement". The
measurement (2026-09-02) says the 2-frame cell is fine with the drain in place,
and the three-frame ceiling it was blocking was never the encoder's — it was
the driver holding every output buffer until after the last send. Taking the
note at face value would have left the campaign permanently two frames deep.
Honour the "without a measurement" half; do not read the note as the answer.

## 5b. Drills you don't have to write

Localizing a divergence starts with narrowing WHAT changed, not reading code.
These are committed so nobody rebuilds them in a scratch dir:

```bash
tools/drill_two_images.sh     # per-preset/per-qp verdicts for the two open images
tools/sc_tool_bisect.sh       # palette? IntraBC? neither? (SVTAV1_SC_TOOLS)
tools/regression_spotcheck.sh # every fixed bug, ~90s
tools/inter_byte_matrix.sh    # the inter campaign's 96-cell frontier:
                              # BOTH / F1DIFF (inter defect) / F0DIFF (video-KEY
                              # defect, so every frame-1 reading below it is void)
python3 tools/coverage_matrix.py
```

The VIDEO-mode key frame's tree diff, which is the inter campaign's inner loop
and now runs on macOS (the container gained the config passthrough on
2026-09-01, §5 above):

```bash
W=~/tmp/zenav1-ctrace/refcell; mkdir -p $W
SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
  SVTAV1_PACKTREE=$W/rs.tree tools/identity_run gradient 64 64 40 6 $W/rs
SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
  SVT_CTREE_OUT=$W/c.tree tools/ctrace-linux/run.sh 64 64 40 6 $W/rs.yuv $W/c.obu 8
head -14 $W/c.tree > $W/c.f0.tree    # BOTH dumps append across frames (§5)
python3 tools/tree_diff.py $W/c.f0.tree $W/rs.tree
```

`SVTAV1_PD0_NOSPLIT=1` is the inter campaign's sibling knob: it forces the
video arm's PD0 to test only the 64x64 square **on an INTER frame**, leaving
frame 0 — and therefore frame 1's reference — untouched, so "is the residual
divergence in MD or only in the partition?" is one run instead of an argument.
It answered that on `diag 64x64 q40 p8` (frame 1 byte-identical to C with it,
35 vs 22 B without: `docs/INTER-ENCODE-PLAN.md` §1z⁸). **C never runs this way**
— it is a CONTROL, not a configuration, and a byte count it produces is never a
parity result. As of §1z⁹ that cell is byte-identical WITHOUT the knob (PD0 now
does inter compensation), so it has answered the question it was built for and
no longer stands in for a fix.

`SVTAV1_SC_TOOLS={nopalette,noibc,none}` forces a screen-content tool off at
runtime so you can bisect without editing and rebuilding. It deliberately does
NOT touch `allow_screen_content_tools` (the frame-header bit), so the streams
stay comparable — only the RD candidate set changes.

**Read the sizes it prints, not just the verdicts.** On `graph.png` at q32,
turning palette off moves the port FURTHER from C (3792 → 4186 against C's
3781). That is how "the port over-picks palette" was refuted for those cells:
the port's palette is winning real RD.

`SVT_CPU_FLAGS=<mask>` does the same job on the C side — it pins C's RTCD
dispatch level, which is how you test whether a divergence is C's own SIMD
choice (see `docs/SUSPECTED-C-BUGS.md` #9). `SVT_CPU_FLAGS=0` is pure-C kernels
and works on x86-64; it SEGFAULTS on aarch64, where Neon is mandatory.

### Which SYMBOLS a tile coded, without an arithmetic-coder trace

An encoder saves its END-OF-FRAME CDFs onto the reference it refreshes
(`packetization_process.c:741-744`), and a CDF only moves if a symbol was coded
against it. So **diffing one frame's saved context against the previous frame's
names exactly which syntax elements that frame's tile coded** — a symbol-level
comparison with no `-Wl,--wrap` op trace, which means it works on macOS (§5).

```bash
tools/fctx_gate.sh                      # the reference cell, both sides, diffed
tools/fctx_gate.sh 72 88 40 4 2 diag    # any cell
```

It needs docker (the C side needs `-Wl,--wrap`) and **fails loudly** when there
is none, rather than skipping. What it runs, if you need the pieces:

```bash
W=~/tmp/zenav1-ctrace/fctx; mkdir -p $W
SVTAV1_FCTX_OUT=$W/rs.fctx SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAMES=2 \
  SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 tools/identity_run gradient 64 64 40 6 $W/rs
SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
  SVT_FCTX_OUT=$W/c.fctx tools/ctrace-linux/run.sh 64 64 40 6 $W/rs.yuv $W/c.obu 8
python3 tools/fctx_diff.py $W/c.fctx $W/rs.fctx --frame=0   # do the SAVES agree?
```

`fctx_diff.py --frame=N` compares the two sides' saved contexts field for field;
comparing frame N against frame N+1 **on one side** instead names that frame's
coded symbol set. That second reading is what showed C's 3-byte inter tile codes
`partition/skip/intra_inter/comp_inter/single_ref/newmv/switchable_interp` plus
the COLUMN half of the MV context and nothing else, while the port's 94-byte one
codes intra modes and every coefficient CDF
(`docs/INTER-ENCODE-PLAN.md` §1s).

The C side is `__wrap_svt_av1_reset_cdf_symbol_counters`, which dumps the
FRAME_CONTEXT **after** the real reset — byte-for-byte what lands in
`EbReferenceObject::frame_context`. Its sibling `SVT_CINTER_OUT` prints the
committed per-block inter decision (`mode`, `ref_frame`, `mv`, `predmv`,
`interp_filters`, `motion_mode`, `drl_*`, `inter_mode_ctx`, `skip`) from inside
`svt_aom_update_mi_map` — the exact field set
`port_entropy_inter::write_inter_mode_info` reads, so a tile byte gate can be
built from C's MEASURED decision instead of one fitted to the bytes.

## 5c. Cross-ISA questions need an emulator, not an argument

CI runs ONE architecture. Every cross-ISA question was therefore answered by
inference until 2026-08-05, and the inference had a hole big enough to matter:

> `tier_invariance.rs` walks the SIMD tiers present on the host it runs on. A
> difference that is uniform across tiers on EACH host and differs BETWEEN hosts
> — a per-ISA libm, a compile-time-selected kernel variant — is invisible to it.
> Tier-invariance within a host does not imply invariance across hosts.

Set up the local emulator once:

```bash
brew install qemu lima-additional-guestagents
colima start --profile x86 --arch x86_64 --cpu 4 --memory 6 --vm-type qemu
```

Then:

```bash
tools/fp_cross_isa.sh            # are the transcendentals bit-identical?
tools/cross_isa_port_check.sh    # does the PORT emit the same bytes on both?
```

The second needs no C oracle: to ask "is the PORT the variable side?" you only
need the port's own bytes on two ISAs. Run it whenever a pinned cell looks
host-dependent, BEFORE concluding anything about C.

**Three traps, each of which yields a confident wrong answer:**

- **LLVM constant-folds transcendentals.** With `-O` and loop-constant inputs it
  evaluates them at compile time with its own host-independent evaluator and
  never calls either libm — so a naive dump compares LLVM against itself and
  prints "identical" no matter what the libms do. `black_box` every input, and
  check the folded and unfolded runs agree before trusting a cross-host result.
- **musl is not glibc.** CI is Ubuntu. A musl container compares a libm CI never
  uses, and can report a difference that does not exist there (or miss one that
  does). The tools use `rust:1-slim` deliberately.
- **The emulated build SHARES `target/`.** No `--target` is passed, so it leaves
  an x86-64 ELF at `target/release/examples/identity_run`. Left alone the next
  gate silently runs a foreign binary. `cross_isa_port_check.sh` rebuilds
  natively at the end; if you build by hand, do the same. For the C library,
  mount the repo **read-only** and copy out — the host's
  `Bin/Release/libSvtAv1Enc.a` is aarch64 and load-bearing.

## 5d. Scripting a file split — three traps, all measured

`leaf_funnel.rs` (11,247 lines) became `leaf_funnel/` on 2026-08-16, byte-neutral
at 1100/1100. If you split another mega-file, the compiler catches everything —
but only after you avoid these, each of which cost a rebuild cycle:

- **A line regex cannot tell a struct field from a function parameter.** Bumping
  `^    name: type,` to `pub(super)` also hit multi-line fn parameter lists: 744
  errors. Parameters live inside `(`, fields inside `{`; only brace/paren depth
  tracking distinguishes them.
- **Locating sections by title text matches PROSE.** "The funnel" appears at
  line 424 as well as its banner at 2852, so taking the first hit mis-sliced
  every section and one module came out 9 lines long. Require the preceding line
  to be a `// ----` rule.
- **A glob re-export CAPS visibility.** `pub(crate) use m::*;` silently demoted a
  genuinely-`pub` item and broke two integration tests; a blanket `pub use` then
  warned on every module exporting nothing public. Use crate-scoped globs for
  internals plus explicit `pub use` for the few real public items.

File-private becomes `pub(super)` — the same scope, now that the "file" is a
module tree. And the acceptance test is byte-identity, not a reading of the diff.

**Pre-split line numbers.** Docs written before 2026-08-16 cite
`leaf_funnel.rs:LINE`. Those numbers are stale for anything that moved into
`tx_pipeline` / `rate_tables` / `predict` / `coeff_rate`. **Re-locate by symbol
— every name is unchanged.** Do not chase the numbers.

**A control that produces NO change is only evidence once you have separately
shown the code is REACHED.** Measured 2026-08-31 while checking that a new line
in `avg_cdf_with` was byte-neutral: the verdict was 32/32 cells identical, and
the positive control — perturbing `skip_cdf[0][0]` by -2000 in the same
function — ALSO changed no byte. That reads exactly like "the function is never
called", which would have made the 32/32 vacuous, and it was one step from
being recorded that way.

It was reached. An `eprintln!` probe fired **twice per frame** at presets 0/4/6
and **zero** times at preset 8 — and 2 is what the geometry predicts (64x64 SBs
make 192x160 a 3x3 grid; the call site needs `left_avail && topright_avail`,
i.e. `col == 1, row in {1,2}`). Zero at p8 matches `funnel_chain = use_funnel
&& preset in 0..=6 && multi_sb` (pipeline.rs). A stronger control (halving
`partition_cdf` at the same site) then moved 12/12 cells.

So the weak control's silence meant *"this perturbation flipped no RD
decision"*, not *"this code did not run"* — two readings a byte diff cannot
tell apart. **Count the calls; do not infer reachability from a byte diff.**
This bites hardest on preset- and geometry-gated paths: a grid that misses
`multi_sb`, or sits at preset >= 7, exercises none of the funnel chain.
Record: `benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md`.

**`cargo build -p <crate>` hides test-target breakage; build `--all-targets`.**
On 2026-08-31 a field was added to `SeqTools` and a `#[cfg(test)]` literal in
`entropy/obu.rs` was not updated. The lib built clean, so the author saw
nothing; CI's `Workspace tests` step failed to COMPILE, and because that is
step 12, steps 13-25 were **skipped** — decode conformance, bd10 identity, SIMD
tier invariance, the spot-check and the 8-bit all-preset sweep never ran, for
that commit or for the nine others from three lanes that inherited the same
parent. A compile error in one lane silently erases every gate's evidence for
everyone, so a skipped gate reads as "no result", never as "pass".

**A shim may only reference a symbol `nm -g` shows in the archive — and
`objcopy --globalize-symbol` exits 0 when it matches NOTHING.** Three lanes hit
this on 2026-08-31 and it took `main` red on Linux twice, invisibly to every
aarch64 developer.

Several `static` C functions are promoted to linkable symbols by
`crates/svtav1-cref/build.rs` (`llvm-objcopy --globalize-symbol` on a private
copy of the object) so they can be reached at evidence tier 1. That mechanism
is sound. What was not: success was read from objcopy's exit status.

**GCC renames statics.** Its interprocedural passes emit `.isra.N`
(scalar replacement), `.constprop.N`, `.part.N` and `.cold` clones, and may
eliminate a function outright. Measured, same source, two hosts:

| C symbol | clang / macOS | gcc / Linux |
|---|---|---|
| `clamp_qindex` | `_clamp_qindex` | `clamp_qindex.isra.0` |
| `aom_ssim2` | `_aom_ssim2` | `aom_ssim2.part.0` |
| `get_regulated_q_overshoot` | present | absent entirely |

So `--globalize-symbol=clamp_qindex` matched nothing, exited 0, the cfg and the
shim's `#ifdef` define both switched on, and the link failed with
`undefined symbol`. On macOS the plain names survive, so it linked and the
breakage was invisible on the host every lane develops on.

Rules, if you add a promotion site:
- Verify the RESULT, never the exit code — `globalized_symbols_present()` runs
  `nm -g` on the promoted object and requires each name to be global, matching
  the WHOLE name so `clamp_qindex.isra.0` does not satisfy `clamp_qindex`.
- Guard the shim wrappers on the matching `SVTAV1_CREF_*` define, so a failed
  promotion means the C side does not reference the symbol at all. Those
  functions then fall back to tier 4, and `SVT_CREF_REQUIRE_*_STATICS=1` turns
  that skip into a loud failure for a caller who requires it.
- Before writing any shim at all, `nm -g` the archive **on both hosts**. A
  symbol `nm` reports as `t`/`b` (local) is not linkable, and one that is `T`
  on your host may be renamed on the other.

## 6. Refuse, never emit a plausible-but-wrong stream

Out-of-envelope configs return a typed `Err` from `encode_frame_impl`. They do
**not** encode. A wrong-pixels output indistinguishable from a correct one at
the integration seam is a shipping bug, not a known limitation.

Corollary the harness must respect: **a refusal is not a crash.** `identity_run`
exits **3** on a typed refusal, and gates count that separately.
`arbitrary_size_robustness.sh` once reported 48 correct refusals as PANIC — it
could not tell the port's best behaviour from its worst.

## 6b. A refusal is not a solution — check the ledger

```bash
tools/refusal_inventory.sh          # regenerate docs/REFUSED-CONFIGS.md
python3 tools/coverage_matrix.py    # what is COVERED
```

Refusing an out-of-envelope config beats emitting a wrong bitstream (§6). That
rule is right and it stays. Know its side effect:

- `arbitrary_size_robustness.sh` counts its **48 refusals as PASSES**, because
  refusing IS the correct behaviour. Nothing in that line separates "genuinely
  out of scope" from "nobody did the work".
- `coverage_matrix.py` prints `--` for an untested axis — but a REFUSED config
  produces no cell at all, so it cannot even show as `--`. The one tool built to
  surface gaps is blind to this one.
- Nothing ages a refusal. No owner, no expiry.

**Measured cost, 2026-08-04:** 10-bit at non-64-aligned dimensions — the actual
AVIF product case — sat behind `bit_depth_config_error` while every gate was
green. It was quoted in a status report the same day and moved past, because the
scoreboard said fine.

`docs/REFUSED-CONFIGS.md` splits refusals into **CONTRACT** (caller misuse,
permanent) and **CAPABILITY** (unimplemented — debt), and is CI-gated so the list
cannot accrete quietly. **Read the CAPABILITY table as a backlog.**

## 7. Dead-looking C stays translated

If a faithful translation appears to have no effect: **keep it, document the
reachability, do not revert.** The analysis calling it dead is often wrong (this
happened and was reversed within the hour), and upstream can re-enable a path
with one commit. Write down what you measured — which presets reach it, which do
not.

Suspected *C* bugs go in `docs/SUSPECTED-C-BUGS.md`, not into a fix. A C bug is
still the oracle; byte-identity means reproducing it.

## 7b. INTER frames: the refusal is still the shipped behaviour

`EncodePipeline` refuses every non-key frame (§6). `SVTAV1_INTER_EXPERIMENTAL`
lifts that guard **for the differential harness only**. It must never leave the
inter harness (`tools/identity_diff_inter.sh`, `tools/inter_fh_gate.sh`,
`tools/inter_byte_gate.sh`, `tools/inter_byte_matrix.sh`), and it is to be
DELETED once the tile is byte-identical broadly — never promoted to a feature
flag.

**Where that stands, re-measured 2026-09-02** (this paragraph used to say "the
frame HEADER is field-exact but for two CDEF strengths, while the TILE is the
pre-campaign homegrown path", which has been wrong since §1z; it then said
36/59/1, which §1z¹⁵ superseded, then 55/40/1, which §1z²¹ superseded): on
the campaign's 96-cell grid — `{uniform,gradient,diag,screen}` x
`{16,64,72,128}` x `{q20,q40,q55}` x `{p6,p8}`, all `frames=2` low-delay P —
**91 cells are byte-identical on BOTH frames**, 4 have a byte-identical
frame 0 and a differing frame 1, and 1 still differs on frame 0 —
**91 / 4 / 1 as of §1z²⁴**, and **all 96 streams DECODE**.
`tools/inter_byte_matrix.sh` is that sweep and `tools/inter_byte_gate.sh`
asserts the 91 plus two 576x576 cells (93 required). `tools/inter_decode_census.sh` asks the OTHER question — does
the stream decode — of all 96, because "byte-identical" and "decodable" are
not the same question and 22 cells once answered them differently
(§1z¹⁸/§1z¹⁹).
**§1z²⁰ split the then-40 in half and BOTH halves are closed.** The header
half (20 cells, every one differing first at `loop_filter_level[0]`) was the
DLF video arm, §1z²¹. The tile half was NOT a cost model: §1z²² found that
`pipeline.rs` ran the ALLINTRA PD0 entry point on every inter frame at preset
<= 6, so PD0 predicted DC intra, priced with the KEY-frame lambda and ignored
the per-superblock `min_sq` its own probe had already computed — 80 evaluated
nodes against C's 5 on `gradient 64x64 q20 p6`. **The residual is now FOUR** — three
72x72 partial-superblock cells plus `diag 128x128 q20 p8`. §1z²⁴ wired C's
PER-SUPERBLOCK MD lambda (`svt_aom_mode_decision_configure_sb` ->
`svt_aom_get_me_qindex`, which this port had as one value per FRAME: 6633
against C's 5182 / 5182 / 5182 / 7773 on `diag 72x72 q40 p6` frame 1) and
that CLOSED §1z²²'s edge-shape under-split — the partition tree on that cell
is now C's exactly, five inter blocks with C's shapes, and the residual byte
is a MODE: the port codes NEWMV where C codes NEARMV at `mi=(8,16)`, same MV
`(24,0)`. The next chunk there is the candidate/MVP lane, not the partition
cost model. Read §1z²⁰ for what no longer needs investigating, then §1z²¹,
§1z²² and §1z²⁴ for the corrections they make to it.
**And read §1z²¹'s three corrections to §1z²⁰ first** — the plan predicted the
DLF split would fall on p6-wrong / p8-right, from an `is_not_last_layer` that
is actually TRUE on a flat GOP (`pd_process.c:5560` ANDs in
`hierarchical_levels != 0`), so BOTH presets were wrong and by two different
pickers. A ladder read off the source without running it got the direction
right and the arms backwards.
The refusal stays
because 91 of 96 is not "broadly": a stream the public API emits has to be right
on content the grid does not cover, not on the cells that happen to be closed.
Full measurement: `docs/INTER-ENCODE-PLAN.md` §1q for the header, §1z''..§1z¹⁶
for the tile.

**Until 2026-09-02 the then-55 F1DIFF included EIGHTEEN CRASHES** — see the
crash-vs-divergence trap in §5. The sweep now has a CRASH column and fails on
one; every count above is genuine divergences.

**A STALE CHECKOUT IS THE BRANCH VERSION OF THE STALE-BINARY TRAP, AND THIS
FILE'S OWN WARNING DID NOT COVER IT** (2026-09-02). The §1z²¹ chunk took its
baseline, its result and four gate numbers on a working copy whose parent was
**15 commits behind `origin/main`** — including the `me_*_distortion`
normalisation fix its own brief had flagged as possibly having moved the cells
under measurement. TWO numbers disagreed with the brief (spot-check 76 against
81, nextest 2478 against 2474) and both were written off as a stale brief; the
real cause was that `tools/regression_spotcheck.sh` had gained 30 lines in the
commits the checkout was missing. **A number that disagrees with the handoff is
evidence about the TREE first and about the handoff second.** Run
`jj log -r '@- | main@origin'` before the first measurement — it takes a
second, and re-running a 96-cell sweep plus a 1100-cell identity sweep does
not.

**A PROBE THAT AGREES IS NOT EVIDENCE THAT THE VALUE IS USED** (§1z²²,
2026-09-02, worth TWENTY-TWO cells). `pipeline.rs`'s `PD0DR` line — added for
exactly this join — printed `minsq=32` for an inter superblock, and C's
`SVT_PD0CFG_OUT` printed `dr=1/0/1/1` for the same one: field for field
agreement, on a value the entry point the refinement path actually calls never
received. The port's PD0 evaluated EIGHTY nodes there where C evaluates five,
with a DC intra prediction and the KEY-frame lambda, on an inter frame. Two
chunks read that `PD0DR` line as confirmation. **Join the probe to the
DECISION, not to C's probe**: the cheap version here was counting PD0 nodes
per frame in the port's own dump (80 vs 5) which needs no oracle at all.

**THE PORT'S DPB NEVER RECEIVED AN INTER FRAME, AND NO GATE COULD SEE IT**
(§1z²⁵, 2026-09-03). `PictureControlSet::new_inter_frame` hard-codes
`refresh_frame_flags: 0`, and that constant — not `pic.rps.refresh_frame_mask`,
which the frame HEADER already writes — is what reached
`self.dpb.refresh(..)`. So the stream announced C's real mask while this
encoder's own DPB stayed all-key-frame. MEASURED at poc 2 of
`gradient 64x64 q32 p8 frames=3`: `rps.ref_dpb_index[0]` is slot 1 and all
EIGHT slots still held the key frame, so LAST resolved to poc 0 where C's
resolves to poc 1.

Two things to carry. **It is invisible at two frames** — nothing reads the DPB
after frame 1 — and two frames is the entire envelope every gate in this repo
covers, so "93 byte-identical cells" said nothing about it. **And any frame-2
reading taken before 2026-09-03 is void**, including numbers a future chunk
might lift out of the 09-02 notes. This is the FIFTH "a caller passes a
constant where the derivation is already ported" finding of the campaign; the
sibling that is STILL live is `part_arm::video_pd0_params`' `was_intra:
Some(1)`, which is true only of a key-frame reference.

**THE DLF VIDEO ARM WAS ONE `else` ARM, AND IT WAS WORTH TWELVE CELLS**
(§1z²¹, 2026-09-02). `pipeline.rs` derived `dlf_level` through the ported
ladder for both arms, mapped it through `set_dlf_controls` — and then handed
every non-key frame `LfLevels::default()`, because `deblock.rs` carried both
level pickers SPECIALIZED to a key frame and the inter arms of
`svt_av1_pick_filter_level` had nowhere to live. The port switched deblocking
OFF on every inter frame while C signalled 8/9/12/16/20/24.
This is the FOURTH "the leaf is already ported, a caller passes a constant"
finding of the campaign, and the second where the constant carried a comment
explaining why it was faithful — `md_config.rs:948`'s `let dlf_level = 0u8`
is faithful for the DIFFERENTIAL's shim, which pins `enable_dlf_flag = 0`, and
was never true of the encoder. **When a constant's justification names a
specific surface, check whether the code you are reading is that surface.**

**FIXED the same day (§1z¹⁹) — `av1_find_samples` is ported, `num_proj_ref`
is real, and all 96 streams decode; the census pin is empty.** The entry
below stays because the TRAP is not the bug: a byte gate still cannot tell
"wrong bytes" from "bytes no decoder will accept", and the next feature that
gets switched off in the SEARCH while staying in the BITSTREAM will look
exactly like this one did.

**A BYTE GATE CANNOT TELL "WRONG BYTES" FROM "BYTES NO DECODER WILL
ACCEPT", AND 22 OF 96 CELLS WERE THE SECOND** (§1z¹⁸, 2026-09-02, measured
on both ISAs). `aomdec` REJECTS the port's frame 1 on 22 of the campaign's
96 cells — "Failed to decode tile data" — and `inter_decode_gate.sh` was
green throughout, because none of its five named cells is one of the 22.
The mechanism is ONE operation: at preset 6 the frame header carries
`allow_warped_motion = 1` (at preset 8 it is 0, which is the whole split),
so C's `motion_mode_allowed` promotes a block with an overlappable
neighbour to `WARPED_CAUSAL` and writes the motion mode from the
THREE-symbol `MOTION_MODE_CDF`; the port's `num_proj_ref` is always 0
because `av1_find_samples` is unported, so it writes the TWO-symbol
`OBMC_CDF`. Both write symbol 0. They write it from different alphabets,
and the arithmetic coder desyncs. `tools/inter_decode_census.sh` is the
gate; the 22 are pinned by name.

Two things to carry from it. **Turning a control OFF does not remove its
SYNTAX** — `wm_ctrls` being off keeps warped motion out of the candidate
SET and does nothing about the symbol every inter block writes; the
alphabet depends on the SAMPLE COUNT, not on whether the tool would ever
be chosen. And **a decision-level dump can show every block agreeing while
the stream is still broken**: all four coded blocks matched C exactly on
the cell that localized this, and 20 of its 22 bytes matched. Only the
op-trace differ could see it — after normalising C's `BOOL` and the port's
2-symbol `CDF` to one spelling, without which every frame "diverges" at
operation 1 and the tool tells you nothing.

**MD PRICED A CONTEXT IT DID NOT CODE, and that was worth nine cells**
(§1z¹⁷, 2026-09-02). `entropy::context::get_intra_inter_context`'s
four-entry table was INVERTED — it returned 0 for "both neighbours intra"
and 3 for "both inter", where C returns 3 and 0 — and its call sites
collapsed "no neighbour" into "intra". The correct transcription,
`port_entropy_inter::intra_inter_context`, was already in the repo, already
tier-1 gated, already used by the WRITER, and its own doc comment said "the
two must agree — if they ever do not, one of them is wrong and this
module's parity test is the one with a C oracle behind it." It was right.
The encoder priced context 3 and coded context 0 on every inter candidate
of every block with two neighbours: **1207 rate units**, measured against
C's own `svt_aom_inter_fast_cost` through the `SVT_IFCOST_OUT` interposer.
Fixing it took the 96-cell grid from 40 BOTH to 49. This is the THIRD
duplicate transcription this campaign has found (`svt_mv_err_cost`, the MD
lambda, this) — §4's rule is not a style preference. **It has since found two
more** (§1z²²'s `pd0_frame_lambda_and_min_sq`, and §1z²⁴'s: the frame inter
lambda was derived once for `c_quant` and again for `SearchFrameCfg`, so
making one per-superblock would silently have left the other stale).

**The NSQ motion search is wired as of 2026-09-02** (§1z¹⁶) and the byte
grid did not move: `md_nsq_motion_search` plus C's `sq_sb_me_mv` seed now
make the port's full-pel MD motion vector join C's on every row either side
can observe (`tools/inter_me_join_gate.sh`, 34 rows, 16 NSQ, 0 disagree) —
and the 55 F1DIFF cells stayed 55. **A closed mechanism that moves no bytes
is a real result, and it narrows the next one**: the divergence is downstream
of the motion search.

**What C's coded decision actually uses on that grid** — measured, not
inferred, by `tools/inter_cinter_census.sh` (§1z¹⁶): of 340 coded inter
blocks, **zero** are compound, zero use a motion mode, zero are inter-intra,
zero carry a nonzero DRL index and zero are GLOBALMV; 106 are NSQ shapes and
3 are NEARMV. Before you spend a chunk unsuppressing one of
`inter_md_arm`'s eight OFF controls, run that census — it ranks them from C's
own dump, and it retired compound prediction (the feature two briefs called
the largest remaining gap) in one run.

Two things §1q proves that a reader will otherwise re-derive:

* **The C oracle used to die above TWO frames in low-delay mode — FIXED
  2026-09-02, and the ceiling was the DRIVER, not the library.** The library's
  single-thread object pool exhausted (`sys_resource_manager.c:791`) because
  `capture_c_trace` sent every frame before draining any packet, so the finite
  output-stream buffer pool ran dry on the third send. It now drains one
  packet after each send **when `n_frames > 2`**, which is safe precisely
  because ST mode runs the whole pipeline inside `svt_av1_enc_send_picture`
  (`enc_handle.c:5805`) — the packet is already in the fifo, and
  `svt_get_full_object`'s ST arm is a direct pop that would deref NULL on an
  empty one. MEASURED: `SVT_FRAMES=3` on `gradient 64x64 q32 p8` gives
  1480 / 22 / 21 B and decodes 3/3 in aomdec AND dav1d.
  **`docs/INTER-ENCODE-PLAN.md` §1q said this fix "makes it WORSE — the
  2-frame cell then segfaults too". That does not reproduce.** Measured on the
  same cell the plan named: with the drain UNGATED (every send, at every
  `n_frames`), `gradient 128x128 q32 p8` at `SVT_FRAMES=2` still exits 0 and
  still writes 5015 / 24 B, identical to the send-all-then-drain build. So the
  earlier attempt differed from this one somewhere the note does not record —
  most likely in the FINAL drain, which must not ask for a packet after the
  last one has been taken. The `n_frames > 2` gate is kept anyway, not because
  the ungated form was observed to fail but because it makes every 1- and
  2-frame run byte-identical BY CONSTRUCTION rather than by measurement.
  Verified regardless: byte gate 55 required / 0 failed, spot-check 76/76.
  The PORT still refuses frame 2, but the reason CHANGED on 2026-09-03
  (§1z²⁵): the coded-area statistics the old refusal named are carried on the
  DPB entry now and joined to C's own `SVT_REFSTATS_OUT` field for field
  (`slice/intra/skip/hp = 0/0/100/0` on both sides). What is left is the
  per-superblock `pd0_detector` input — `ref_obj_l0->sb_intra[sb_index]`, which
  `part_arm::VideoPic` has no `InterOnInterRef` arm to consume. **Do not read
  the lift as close because two of four mechanisms landed**: with them in and
  nothing else, frame 2 codes 466 B against C's 21, every block intra. Two
  `refuses_inter3` cells in `regression_spotcheck.sh` pin that from the other
  side.
* **`fh_fields.py` used to GUESS `skipModeAllowed`** and got it wrong on the
  first inter cell, shifting every field after `skip_mode_present` by one bit
  without any sign in the printout. It now implements the real rule and threads
  the decoder's `RefOrderHint[]` across the stream. Any inter-frame reading
  taken off that tool before 2026-09-01 is suspect.

## 8. What is actually true right now

`STATUS.md` leads with the measured envelope; `docs/*-port-map.md` holds
per-feature plans. Both contain claims written by earlier sessions that
measurement has since overturned — **at least three were wrong on the day they
were written**, and the corrections are recorded in place rather than quietly
patched.

So: **re-measure before you build on a doc claim.** If a doc and the source
disagree, the source wins and the doc gets fixed in the same change.

## 9. Where the bodies are

| you want | look at |
|---|---|
| what is byte-identical, and where it is not | `STATUS.md` |
| coverage per preset per axis | `python3 tools/coverage_matrix.py` |
| every bug we have fixed, with its reproducer | `tools/regression_spotcheck.sh` |
| C code that looks broken | `docs/SUSPECTED-C-BUGS.md` |
| which C file a Rust module ports | `../PORTING.md` |
| the leaf funnel (SPLIT 2026-08-16) | `leaf_funnel/{mod,tx_pipeline,rate_tables,predict,coeff_rate}.rs` |
| perf + memory | `docs/perf-status.md`, `benchmarks/mem_2026-08-16.meta` |
| the working agreement + envelope guards | `CLAUDE.md` |
| per-feature plans and open chunks | `docs/*-port-map.md` |
| why a `product_coding_loop.c` row reads MISSING | `docs/pcl-md-port-map.md` |
| committed measurements | `benchmarks/*.tsv` + the `.meta` beside each |

## 10. The habits that matter most

- **Measure the premise before building on it.** One unverified assumption
  cascades into hours of wrong work.
- **Report what you ran, not what you believe.** "I did not run it" is a fine
  sentence; "verified" for something you inferred is not.
- **An honest localization beats a speculative fix.** "The first divergence is
  at block (x,y), the port picks A at cost C1 and C picks B at C2, here is the
  differing term" is a complete result even with nothing fixed.
- **When you are wrong, correct it in place** — the doc, the comment, the commit
  message. This file's whole value is that its predecessors did that.

# Changelog

All notable changes to `zenav1-svt` (the pure-Rust SVT-AV1 still-image encoder
port). The project's unit of progress is **byte-identity with the C reference**,
so entries state what became byte-identical and under which gate, not just what
code was added.

Crates are not published to crates.io yet — depend by git.

## [Unreleased]

### QUEUED BREAKING CHANGES

<!-- Batch API breaks here; ship them in one version bump, never piecemeal. -->
- **Crate consolidation 6 → 4 publishable packages (issue #3, 2026-08-28).**
  `zenav1-svt-tables` is folded into `zenav1-svt-types` as
  `svtav1_types::tables::{block, interp, partition, scan, transform}` and
  `zenav1-svt-entropy` into `zenav1-svt-encoder` as
  `svtav1_encoder::entropy::{cdf, coeff, coeff_c, context, default_cdfs,
  default_coef_cdfs, lr, mv_coding, obu, range_coder, scan_tables, tile,
  writer}`; both former packages are deleted. Path rename only for the two
  crates' consumers (`svtav1_tables::X` → `svtav1_types::tables::X`,
  `svtav1_entropy::X` → `svtav1_encoder::entropy::X`); the facade keeps
  `svtav1::tables` / `svtav1::entropy` as re-exports so facade users are
  unaffected. The entropy crate's `unchecked_entropy` / `symtrace` features
  moved onto `zenav1-svt-encoder` (the facade's `symtrace` forwards there).
  Bitstream bytes unchanged: `byteid_fingerprint` 144/144 cells identical
  before/after, identity_matrix 54/54, bd10 36/36, partial_sb 146/146,
  regression_spotcheck 33/33, decode_conformance 1260 + 1575 / 0 failed.
  Nothing is published yet, so this is a pre-release rename, not a semver
  event.
- **`AvifEncoder` knob surface (issue #9 item 7, 2026-08-28).** REMOVED
  `with_trellis`, `with_seg_boost`, the `seg_boost()` getter and
  `with_still_image_tuning` — all four were recorded-and-ignored with no
  counterpart in this pipeline or in C. `with_vaq(bool, f64)` is REPLACED by
  `with_variance_boost(bool, u8)` (C's 1-4 strength scale). `encode_yuv420`
  keeps its signature but its OUTPUT CONTRACT changes from three
  length-prefixed monochrome streams to one real AV1 4:2:0 bitstream — the old
  format was not decodable, so nothing can have depended on it.
  `AvifEncoder::{enable_qm, enable_variance_boost}` now default to `false`
  (C's mainline defaults); the emitted bytes for a caller that sets neither
  are unchanged.
- **`crate::pd0::pd0_pick_sb_partition{,_lvl0,_m6,_m6_eval}` take a
  `lambda_weight: u32` after `qindex` (issue #9 item 4, 2026-08-28).** C's
  frame `lambda_weight` (`pcs->lambda_weight`, enc_mode_config.c:10093-10115)
  is a frame-level fact — the tune-IQ curve, the PSNR ladder, and the
  extended-CRF bump — that these entry points cannot derive from their `qp`
  argument once a fractional CRF moves `picture_qp` off `static_config.qp`.
  Callers pass `pd0::frame_lambda_weight(picture_qp, tune_iq, bump)`;
  `frame_lambda_weight(qp, false, 0)` reproduces the previous internal ladder
  exactly, so the change is byte-neutral at CRF offset 0.
- None queued otherwise. `EncodePipeline`'s new surface (`try_encode_frame_420_hbd`,
  `try_encode_frame_hbd`, `with_superres`) is additive; the `SeqTools` and
  `ScSignal` structs gained fields (`enable_superres`, `superres`), which is a
  break only for out-of-crate struct literals — there are none.

### Added

- **Interpolation-filter search wired at MDS3 (2026-09-04).** C runs
  `interpolation_filter_search` once per MDS3 inter candidate
  (`ifs_ctrls.level == IFS_MDS3` for every video-arm preset the port accepts);
  the port hardcoded `EIGHTTAP_REGULAR` and paid no switchable rate. New
  `leaf_funnel::ifs::ifs_at_mds3` (full-pel: rate-only pick; sub-pel: per-pair
  luma prediction + `model_rd_for_sb`, chroma rebuilt on a changed pair) adds
  `fast_luma_rate += switchable_rate` as C does. New `SVT_IFS_OUT` interposer
  on the exported caller and `tools/ifs_join_gate.sh`: 96/96 cells, 330 MDS3
  candidates joined, 0 mismatches. On the grid every MDS3 MV is full-pel and C
  keeps REGULAR on all 367, so the filter bytes were already right; the rate
  is now paid. An inter frame under tune vq / film-grain / alt-ssim+ssim is
  REFUSED (the smooth bias needs `is_noise_level`, not derived).
  `docs/INTER-ENCODE-PLAN.md` §1z³⁶.
- **NSQ recon-dist gate: the parent-mode threshold now reads C's unified
  `block_mi.mode` (2026-09-04).** `depth_refine::skip_by_recon_dist` modulated
  `max_part0_to_part1_dev` by the intra y_mode, which every inter winner
  carries as 0, so an inter parent always took C's `DC_PRED` arm (`* 2`)
  instead of `* 75 / 100` (NEWMV), `<< 2` (GLOBALMV) or none (NEARESTMV). The
  table is C's full 25-mode switch (`product_coding_loop.c:9867-9895`), pinned
  25 x 101 to `port_md::nsq_skip::modulate_by_parent_mode`. Measured on the
  96-cell inter grid: the arm is entered ONCE (`gradient 16x16 q20 p6`,
  threshold 73 -> 54 where it was 146) and no cell moved — 94 BOTH / 1 F1DIFF
  / 1 F0DIFF before and after; still envelope 1100/1100. New
  `tools/nsq_inter_reach_census.sh` counts which NSQ gate an inter frame's
  shapes reach. `docs/INTER-ENCODE-PLAN.md` §1z³⁵.
- **`compute_qdelta_by_rate` measured NULL on this envelope (2026-09-04).**
  C reaches it only under TPL, which `get_tpl` disables for `aq_mode 0`,
  allintra and low-delay alike, and the delta it returns only feeds the recode
  loop CQP/CRF forces off. New `SVT_QDELTA_OUT` interposer + `SVT_AQ_MODE`
  positive control. `docs/INTER-ENCODE-PLAN.md` §1z³⁴.

- **The NIC CLASS prunes C runs only on inter frames — the inter byte grid goes
  92 -> 94 of 96.** Each of C's three NIC prunes has an inter-CLASS half that
  sets a whole candidate class's stage count to ZERO and an intra-class half
  that trims within a class; `leaf_funnel::nic` carried the trims and only
  `post_mds2`'s class half, applied with the I-slice `MAX(25, base * mult)`
  re-floor unconditionally. C forces `mds1_class_th` / `mds2_class_th` to the
  disabled sentinel on an I_SLICE (`product_coding_loop.c:7826` / `:7897`) and
  applies that re-floor only there (`:7977`), so all three were right for a
  still frame and wrong for an inter one. MEASURED on `diag 72x72 q55 p6`
  frame 1 at block (64,32) 16x32 with an extended `SVT_FULLCOST_OUT`: C injects
  29 intra / 6 MVP-inter / 3 NEWMV-inter candidates and admits 0/3/3 to MDS1
  and 0/1/0 to MDS3, while the port reached MDS3 with three and coded the NEWMV
  C had already dropped. Both `diag 72x72 q55` cells are now byte-identical on
  both frames. `inter_byte_gate` 94 required / 0 failed, `video_key_matrix`
  58/60 unchanged (its two open cells are key frames, which this cannot reach),
  `identity_full_8bit` 1100/1100, `regression_spotcheck` 102/102, nextest 2519.
  Record `rust/benchmarks/nic_class_prune_2026-09-03.md`.
- **The RDOQ input buffer's re-zero was dead — 1.5-2.0 % at the still arm's
  p10 cells, and the first confirmation of the memset-not-malloc rule.**
  `tx_unit` zeroed its `dqcoeff` scratch (up to 4 KiB) once per
  (candidate x tx type x tx unit), but all four quantizer paths write every one
  of the `pw * ph` positions before anything reads them — and the two QM
  quantizers open with their OWN `fill(0)`, so on that path the caller's was a
  second memset of the same bytes. Ten of twelve A/B cells gain, six with their
  whole span below 1.0, and the two biggest are the p10 STILL cells where
  LIBC_MEM's share of the gap is highest. Control is a RELEASE-unconditional
  `0x5A5A5A5A` poison in place of the zero-fill: spot-check 102/102,
  identity_full_8bit 1100/1100. Record
  `rust/benchmarks/dqzero_ab_2026-09-03.meta` (a32d82e8).
- **A second NULL, reverted, and the rule the two of them establish.**
  Recycling `predict::hadamard_satd`'s two fixed-bound buffers — the #3
  allocator caller on the still arm at 11.9 % — is **~1 % SLOWER across the
  videokey arm** in both a thread-local variant and a stack-array-for-small-
  tiles variant. With the `d0_recon` null above, that is two measured,
  controlled, negative allocation removals in one chunk. What separates them
  from the two that WON (`0c70f3fc` six of six cells, `700357e2` nine of
  twelve) is not the allocations — `coeff_contexts` removed one and `700357e2`
  removed NONE; both removed **zeroing**. Working rule recorded in
  `rust/docs/perf-status.md`: on this allocator a malloc/free pair is worth
  about nothing and a memset is worth real time, so rank the remaining ALLOC
  share by what it MEMSETS. Record
  `rust/benchmarks/hadscratch_null_2026-09-03.meta` (ce34ec82).
- **Zone 2 directional intra runs in NEON lanes — 1.4 % at the still arm's
  worst cell and 2.5-4.2 % on the video key frame at p6.** `dr_predictor_edged`
  was 763 of 19,115 self samples at gradient 512x512 preset 2 against C's
  `svt_av1_dr_prediction_z2_neon` 323 — z2 is C's largest directional kernel
  there and was the last one unported. C reaches zone 2's `left` half with
  `vqtbl4q_u8`, a 64-byte gather, because within a row that half's `base_y` and
  `shift` both vary with the column; this arm needs no gather at all, splitting
  the block into the `above` staircase (walked row-major, z1's kernel) and its
  complement (walked column-major, z3's kernel) so each region is contiguous
  along the axis it is walked on. Byte-identical: identity_full_8bit 1100/1100,
  regression_spotcheck 102/102, inter_byte_gate 94/0, fctx_gate 96/96,
  video_key_matrix 58/60 unchanged, and the C-parity tier sweep gained ten
  RECTANGULAR sizes because the staircase depends on `bw` and `bh`
  independently. Records `rust/benchmarks/z2neon_ab_2026-09-03.*` (d552c62c).
- **The aarch64 inter peak-RSS axis is re-measured and no longer holds** —
  1280 inter 1.121x -> **1.278x**, 2048 inter 1.257x -> **1.329x** of C, with
  C itself unmoved to 0.05 %. About half the 1280 rise is `700357e2`'s known
  +4.1 MiB; the rest is unattributed and sits across four inter CORRECTNESS
  fixes that give the port reference state it did not previously hold. The
  still and videokey arms did not move (0.79x / 0.88x of C).
  Record `rust/benchmarks/mem_inter_axis_2026-09-03b.meta` (24bd95cc).
- **A NULL, reverted, with its teeth and its ceiling: removing an allocation
  nothing reads made the still arm SLOWER.** `mds3::eval_candidate`'s
  `d0_recon` is a `w * h` alloc + memcpy per candidate whose only reader is
  `gate_y`, from one candidate, only at `bypass_encdec`; eliding it elsewhere
  is byte-inert and 1.2-1.7 % slower at 512 p6/p10, reproduced, with a
  parameter-only control showing the cost is the removal and not the signature.
  Also records the measured ceiling for the item (the allocator family is 5.5 %
  of the port's 512 p2 frame) and a pre-existing latent panic it surfaced —
  `gate_y` is an EMPTY slice on a coded-lossless 8x8 leaf.
  Record `rust/benchmarks/mds3d0_null_2026-09-03.meta` (869947ea).
- **`MeCandidate` is ONE byte, as C's is — -8.01 MB of peak heap on the inter
  arm at 4 MP.** C declares all five fields as bitfields of a single `uint8_t`
  (`me_sb_results.h:29`: `direction : 2`, `ref_idx_l* : 2`, `ref*_list : 1`),
  so `sizeof(MeCandidate) == 1`; the port stored five `pub u8` fields. That
  width multiplies by the frame — `me_candidate_array` is 85 x 23 entries per
  b64 and lives from the picture-level motion search to the pack — so at
  2048x2048 it cost 10.01 MB of live heap against C's 2.00. Packed into one
  byte with `set()` masking on write (unchanged) and five `const` accessors
  masking on read: **-8.01 M peak heap at 2048x2048, -5.6 % to -5.7 % of the
  inter arm at 1280/1536/1920/2048, and 0.000 on the still and videokey arms**
  (the control — neither runs the picture ME). Predicted 8.008 MB from C's
  struct and the port's own sizing; measured 8.01. Peak RSS moves too, by a
  DIFFERENT amount (-5.03 MiB at 1280, -2.72 at 2048), because RSS counts
  resident pages and not live bytes. CPU neutral (0.987-1.003, six interleaved
  cells). Records `rust/benchmarks/mem_mecand_2026-09-03.{tsv,meta}`.
- **The peak is decomposed for the first time, with massif instead of
  heaptrack** — `rust/benchmarks/mem_massif_2026-09-03.meta`. A massif peak
  snapshot is one instant, so its entries sum to the peak exactly (checked to
  the byte), where heaptrack's merged per-site totals do not co-occur and could
  only ever be a lead list. It shows peak heap is a MONOTONIC RAMP that ends
  where mode decision ends, on every arm, and it corrects two recorded claims:
  the port's harness is **31.45 M** on the 2-frame arm at 4 MP (three copies of
  the sequence against `perf_c_encode`'s one), so the encoder-side inter frame
  is **+30.2 M** and not +37.65; and the port's frame-wide retained
  coefficients are **at parity with C**, whose `pcs.c:348-368` allocates the
  same `EB_THIRTYTWO_BIT` `sb_size x sb_size` FULL_MASK buffer per b64 for the
  whole frame (C's own measured 26.18 M `svt_aom_pic_buf_desc_pool_ctor`).
- **`examples/perf_encode` holds ONE copy of the input sequence, as
  `tools/perf_c_encode` always has — -18.88 MB off the reported peak at 4 MP on
  the 2-frame arm.** It held three: the generated `y/u/v`, a whole-sequence
  `yuv` concatenation built only so `fs::write` could take one slice, and an
  owned `frames` whose frame 0 was a `to_vec()` of the first. Now each frame is
  streamed to the `.yuv` through a `BufWriter` as it is produced and the
  caller's planes are MOVED into frame 0. **A HARNESS change, not an encoder
  one** — the encoder allocates exactly what it did before; what moves is how
  much of the measured peak belongs to the measuring binary, which is what made
  every port-vs-C memory ratio in this repo carry a 6.3-18.9 MB handicap C's
  driver never had. Every delta is an exact multiple of the frame size (one
  frame on still, two on videokey, three on inter). C's `.obu` is byte-identical
  before and after on all 12 cells — the control, since C reads the file this
  code writes. After it and the `MeCandidate` packing, on x86_64-linux at p13
  qp40 gradient: **peak heap port/C is below 1.0 on all twelve cells** (inter
  0.886-0.938) and **peak RSS is inside the 25 % goal on all twelve** (inter
  1.035-1.122, against `main`'s 1.279-1.334 on the same grid). Records
  `rust/benchmarks/mem_harness_2026-09-03.{tsv,meta}`.
- **Allocator churn cannot move peak HEAP and *does* move peak RSS — ~100 bytes
  of resident-not-live memory per allocation on macOS, ~0 on Linux.**
  `rust/benchmarks/mem_churn_rss_2026-09-03.{tsv,meta}` corrects
  `mem_heaptrack_satd_2026-09-03.meta`'s "removing allocator churn cannot lower a
  peak … the memory gap stays a LIFETIME property", which is true of peak heap
  and false of peak RSS — the quantity `tools/mem_gate.sh` and the 25 % goal
  actually use. Differencing the videokey and inter arms on gradient 2048x2048
  qp 40 with the harness subtracted: from p13 to p6 the inter frame's LIVE cost
  FALLS 47 % (23.34 -> 12.44 MB) while its macOS resident cost RISES 13 %
  (48.89 -> 55.39 MB) and its allocation count rises 112 % (214,114 -> 454,196).
  Resident-minus-live per allocation is 94.6 B (p6) and 119.3 B (p13) on macOS
  against -9.7 and +8.3 B on Linux. **The obvious causal reading of that —
  "remove N allocations, get 100*N bytes back" — is FALSIFIED in the same file**:
  hoisting `partition::extract_neighbors_tiled` (the port's largest
  allocation-COUNT site, 23.5 % of the process) removes 18-20 % of every
  allocation the process makes, moves macOS peak RSS by 0.995x / 1.019x (a null
  inside a 15 % spread) and makes LINUX peak RSS 3.3 % WORSE. What survives is the METHOD point: a peak-heap null
  is not a peak-RSS null, and a memory claim must name which quantity it is
  about.
- **A measured NULL and a measured REGRESSION: the port's largest
  allocation-COUNT site, hoisted and REVERTED.**
  `rust/benchmarks/neighbor_scratch_ab_2026-09-03.{tsv,meta}`; **the change is
  not in the tree.** `partition::extract_neighbors_tiled` returns two `Vec<u8>`
  of at most 64 bytes per predicted transform unit — 637,972 calls on one
  `gradient 2048x2048 p13` two-frame encode, 23.5 % of the whole process's
  allocations, 128 B of peak heap. A thread-local scratch removes 488,413 /
  499,865 of them (18-20 % of the process) with peak heap and `.obu` unchanged
  to the digit, and then buys: aarch64 CPU 0.9944 / 0.9911 / 0.9903 / 0.9941 (a
  ~0.7 % win), x86 CPU 1.0018 / 1.0062 / 0.9925 / 0.9978 (a null), macOS peak
  RSS 0.995 / 1.019 (a null inside a 15 % spread), and **Linux peak RSS 1.033 at
  2048 inter — a 3.7 MiB regression whose fifteen paired rounds do not overlap**.
  Reverted: a memory chunk does not ship a 3.7 MiB peak-RSS regression for a
  0.7 % single-ISA CPU gain. Peak heap is unchanged, so the Linux rise is
  resident-page placement rather than live bytes.
- **The same two changes on aarch64-darwin, and the ISA is worth more than
  either of them** — `rust/benchmarks/mem_aarch64_2026-09-03.{tsv,meta}`. Peak
  RSS at p13 goes from 1.360x-1.483x of C to **1.121x-1.257x** on the inter arm
  (-15 % to -18 % at every size), eleven of twelve cells inside the 25 % goal —
  and **2048x2048 inter is 1.257x, over the line**, where x86 reads 1.086x on
  the same commits and cells. On that cell the port's peak RSS is 153.7 MB on
  macOS against 117.7 MB on Linux and 112.73 MB of peak HEAP on Linux, so ~36 MB
  of the aarch64 figure is not live bytes and no lifetime change can reach it.
  Unattributed (16 KiB pages, libmalloc retention and thread-stack accounting
  are all candidates; heaptrack does not exist on macOS). Raises an untested
  hypothesis: allocator CHURN may move macOS RSS even though it provably cannot
  move peak heap, which would mean the three allocation-site hoists of the same
  day were never measured on the metric they could have moved.
- **The port/C memory ratio is a function of PRESET, and the port's worst preset
  is the FASTEST one** — `rust/benchmarks/mem_preset_2026-09-03.{tsv,meta}`.
  Every memory record before today measured one preset. On gradient 2048x2048
  qp 40 the inter arm's peak heap is **C 240.25 M at p6 against 120.12 M at
  p13** — C doubles — while the port's moves 5 % (146.04 -> 139.62 on `main`).
  A memory ratio quoted without its preset is therefore meaningless. After the
  two changes above, all 24 cells of the p6+p13 grid are inside the 25 % goal on
  both peak heap and peak RSS, and only three exceed 1.0 at all (the p13 inter
  arm, 1.035x-1.122x); at p6 the port is 0.27x-0.62x of C on heap and
  0.58x-0.90x on RSS. Records an unresolved conflict rather than papering over
  it: `mem_arms_2026-09-02.meta` has the inter arm at 1.60x at p6 / 4 MP
  (aarch64, `capture_c_trace`), and this harness reads 0.84x on `main` at the
  same preset and size (x86_64, `perf_c_encode`) — ISA, C driver and tree are
  all confounded.
- **`rust/tools/mem_peak.sh`** — the peak-memory harness the `mem_heaptrack_*`
  records were produced with, now in the repo instead of a scratch directory.
  Measures peak HEAP (heaptrack) or peak RSS (`/usr/bin/time`, median of N) for
  port and C over the three arms and a size sweep, with the refusal trap
  (exit status AND a non-empty `.obu`) on every cell.


- **The refusal ledger gained the axis that decides what is workable: does C
  support it?** `docs/REFUSED-CONFIGS.md` split refusals into CAPABILITY (debt)
  and CONTRACT (caller misuse), which is a claim about the WORDS someone wrote.
  It did not answer the question that ranks a CAPABILITY refusal — can C v4.2.0
  encode this configuration at all? Each refusal now declares its own answer
  with a trailing `[C: accepts | rejects | no mono mode]` marker,
  `tools/refusal_inventory.sh` lifts it into a `C?` column and counts how many
  refusals a byte gate could ever close, and `tools/c_envelope_probe.sh` (NEW)
  checks the markers by running each configuration through the real C library —
  with a positive AND a negative control that abort the probe if they come out
  wrong, so a broken driver cannot report "C rejects everything". Ranked triage
  with both kinds of evidence per row:
  `rust/benchmarks/refused_config_triage_2026-09-03.md`; raw probe:
  `rust/benchmarks/c_envelope_2026-09-03.tsv`.
  **The largest finding is structural: C v4.2.0 has no monochrome mode at all**
  (`verify_settings` rejects any `encoder_color_format` other than EB_YUV420,
  `Globals/enc_settings.c:473`), so no mono refusal can EVER be closed by
  byte-parity — the substitute is the recon oracle this repo already uses.

- **A NEON arm for `cdef::cdef_find_dir` — 1.82x on the kernel, NULL-to-marginal
  on the frame, and a correction to the queue entry it closes.** All eight of
  C's direction formulas are the same shape ("place a row-derived vector at an
  offset"), so the scalar's 8 x 64 accumulations become one 8-lane vector add
  per placed direction per row, with the accumulators in registers and
  `vextq_s16` for the offset. Both tiers now share ONE copy of the
  cost/argmax/variance tail (`cdef_dir_from_partials`), so only the
  accumulation is tier-specific. The i16 exactness bound is CHECKED at run time
  (`vmaxvq_u16` over the shifted rows) with a scalar fallback, not argued. New
  `find_dir_all_tiers_match_c` forces every dispatch tier against real C over
  240 rounds and asserts the sweep was real; `find_dir_matches_c` only ever ran
  the host tier. **The "15x" `docs/perf-status.md` listed is the RATIO between
  the port's function and C's, not a budget** — the measured win is 1.82x on
  the kernel (`benches/kernel_tiers.rs` 159 ns vs 289 ns) and 1.005-1.015x on
  the three preset-8 videokey cells with every other cell NULL. Kept because it
  is strictly less work, byte-identical, and closes a tier coverage hole.
  Records `benchmarks/cdef_find_dir_neon_ab_2026-09-03.*`.

- **First heaptrack run on this repo: the inter frame's memory is attributed to
  allocation sites, and C's encoder is measured to add NOTHING for one.** C's
  peak-consumption site table is identical entry for entry between its
  video-key and its inter arm; its entire +6.30 M is `perf_c_encode`'s own
  input buffer growing by one 2048x2048 I420 frame (6.29 MB exactly). The
  port's harness costs the same 6.29 MB (`perf_encode::translate`), so the
  harness cancels and the comparison is harness-clean — closing the caveat
  `benchmarks/mem_arms_2026-09-02.meta` had to leave open. Encoder-side, one
  inter frame at 4 MP is **port +37.64 M against C's +0.01 M** on the heap.
  On the heap the port is LIGHTER than C for both one-frame arms (still 0.70x,
  videokey 0.84x); only the inter frame flips it (1.16x). Lead list with call
  counts (`funnel_block_decision` 16.79 M / 4096 calls, `MeB64Output::new`
  12.53 M / 6144, `encode_tile_rows::{closure#0}` 11.53 M / 4110, …) reproduces
  five macOS `/usr/bin/heap` entries within ~10 % on a different OS, ISA and
  allocator. Record `benchmarks/mem_heaptrack_2026-09-03.{txt,meta}`. NOT
  MEASURED: any MiB/MP figure (one size), any RSS statement (this is heap),
  aarch64 sizes, or repeat variance.

- **The RDOQ trellis's six context helpers are `#[inline(always)]` now, as C's
  are — 12 of 12 A/B cells move, 1.9-4.8 %.** C's `get_nz_map_ctx` does not
  appear as a symbol anywhere in its profile (grep over the whole 512x512 p8
  videokey call graph: 0 hits) because it is `static INLINE` beside the
  trellis; the port's carried plain `#[inline]`, LLVM declined, and
  `nz_map_ctx` showed 4.61 ms of SELF time in a 50.33 ms frame with 94.8 % of
  it inside `quant::optimize_b`. `update_coeff_eob` pays that call four times
  per coefficient. Promoting `nz_mag`, `nz_map_ctx_from_stats`, `nz_map_ctx`,
  `lower_levels_ctx_general`, `br_ctx` and `br_ctx_eob` gives videokey
  1.037-1.048x (n=25) and still 1.019-1.041x (n=15), every p25/p75 span below
  1.0, every cell `ident=Y` — a LARGER win than the table lookup above, and it
  moves the two still cells that were NULL there. Records
  `benchmarks/nzmap_inline_ab_2026-09-03.*`. NOT MEASURED: monomorphising the
  trellis on `tx_class`, which is what makes C's version fold away completely.

- **The 2D nz-map context offset is a TABLE READ now, as C's is — 2.7-3.6 % on
  every video-mode key-frame cell, byte-identical.** C's
  `get_nz_map_ctx_from_stats` reads one byte out of
  `eb_av1_nz_map_ctx_offset[tx_size][coeff_idx]` (coefficients.h:178); the port
  re-derived it on every call (`adjusted_tx_size`, two log2 table loads, a
  row/col split, four branches) while a compile-time table built from that same
  `const fn` — and already pinned to the exported C data by
  `tests/c_parity_entropy.rs` — sat unused beside it in
  `coeff_simd::NZ_OFFSET`. 94.8 % of those calls are inside the RDOQ trellis
  (`benchmarks/perf_videokey_attrib_2026-09-03.meta`). Byte-identical by
  construction (same generator, evaluated at compile time) and pinned
  cell-by-cell by a new
  `coeff_simd::nz_offset_tests::nz_offset_2d_table_matches_the_generator`.
  A/B, every cell `ident=Y`: videokey 1.027-1.036x across all six cells with
  every p25/p75 span below 1.0 (n=25); still 1.024x/1.032x at preset 2, 1.017x
  at 512 p6 and 256 p10, NULL at 256 p6 and 512 p10 (n=15). Records
  `benchmarks/nzmap_table_ab_2026-09-03.*`. NOT MEASURED: monomorphising the
  trellis on `tx_class`, which is what makes C's version inline away entirely
  (`get_nz_map_ctx` does not appear as a symbol in C's profile at all).
- **`compute_stats_all_tiers_match_c` now ASSERTS its tier sweep was real.** It
  dropped the `#[must_use]` `PermutationReport`, so a build where archmage
  excludes every token (an ambient `-C target-cpu=native`, or no
  `testable_dispatch`) would collapse the sweep to the single native arm and
  still report green — the test would silently stop being an all-tiers test.
  It now uses the same `for_each_tier` helper `tests/c_parity_txfm.rs` has,
  which fails on any exclusion warning and on `permutations_run < 2`.

- **`restoration::compute_stats` re-derived by ROW-PAIR CORRELATION — about 9x
  fewer multiply-accumulates, byte-identical, 1.074x on the whole preset-6
  still frame.** The old NEON arm issued one dot product per `(region row, k,
  t)` triple — 1,225 per row for `H` plus 49 for `M` at `wiener_win = 7`. Most
  of those are the SAME dot: `H[k][t]` depends only on the pair of `d` ROWS and
  the pair of column offsets, so row-pair sharing collapses 1,225 to 322 per
  `d` row and an exact O(1) sliding-window update over the column offset
  collapses that to 85. `docs/perf-status.md` had recorded the opposite
  ("the MAC count is inherent") and prescribed a hoisted reduce with an i64
  drain interval; the correction and the evidence are in that file. The i32
  flush boundary is still ONE ROW, so no drain interval exists to pin.
  Byte-identical: `c_parity_wiener::compute_stats_all_tiers_match_c` (220
  iterations, widths 1..90, both window sizes, every dispatch tier, against the
  real exported C symbol), plus the full gate set. A/B, every cell `ident=Y`:
  videokey arm 1.030-1.053x (n=25), still arm 1.074x at 256 and 512 preset 6
  (n=15), NULL at preset 8 where loop restoration does not run — the control.
  Records `benchmarks/compute_stats_rowpair_ab_2026-09-03.*`. NOT MEASURED: the
  x86 `_v3` arm, which still uses the old per-pixel gather.
- **First attribution of the VIDEO-MODE KEY FRAME, and a correction to the
  standing 44-52 % figure.** All three arms (still / video-mode key frame /
  inter) re-measured in one session after the ME SIMD chunk: the video-mode key
  frame is now **50-64 %** of the port's excess on an inter cell, and the inter
  frame 20-21 %. Per-class and per-symbol attribution at 512x512 preset 8 in
  `benchmarks/perf_videokey_attrib_2026-09-03.{tsv,meta}`; two findings no
  earlier record carries — 94.8 % of `nz_map_ctx`'s time is inside the RDOQ
  trellis (so re-joining it puts RDOQ at ~3.0x and ~24 % of the excess, the
  largest single item), and the video config adds 2.69 ms of allocator work to
  the port against 0.000 ms to C. Records
  `benchmarks/perf_2026-09-03-arm3-{still,videokey,inter}.*`.

- **`dsp::sad::sad` was a SECOND TRANSCRIPTION of the kernel `me_sad`
  transcribes, and is now a thin alias for it.** It carried its own scalar /
  AVX2 / NEON arms; two transcriptions of one C function with nothing pointing
  either at the other is exactly the hazard `docs/WORKING-ON-THIS.md` §4
  records. Pointing both at `me_sad::block_sad` removes it, and incidentally
  gives this entry point (intrabc's block search and `port_global_motion`) the
  `arm_v2` dotprod arm and the 8-wide arm it never had — its NEON path fell
  entirely to scalar below 16 px wide. Byte-identical: `identity_full_8bit`
  1100/1100, `inter_byte_gate` 89/0, `regression_spotcheck` 83/83,
  `screen_palette_gate` 50/50, `screen_ibc_fh_gate` PASS, nextest 2493/2493
  including the tier-1 `c_parity_{sad,intrabc_search}` and
  `sad_neon_parity`. NOT MEASURED: whether the better arm makes intrabc or
  global motion faster — no A/B was run on that path.
- **The five scalar motion-search kernels are vectorised — the INTER cell is
  1.15x faster, byte for byte** (`rust/docs/perf-status.md`, "ME SIMD COVERAGE
  LANDED"). New `svtav1-dsp::me_sad` exports two block primitives as
  tier-suffixed `#[arcane]` helpers — `block_sad_*` and `block_sum_sse_*`
  (`(SUM(a-b), SUM((a-b)^2))`) — with `scalar` / `neon` / `arm_v2` / `v3` arms.
  `arm_v2` is the arm that matches C: `Arm64V2Token` bundles `dotprod`, so
  `vabdq_u8` + `vdotq_u32` is the shape of `svt_sad_loop_kernel*_neon_dotprod`.
  Callers summon ONE token per call and run their search loop inside the tiered
  body, so no target-feature boundary is crossed per search position:
  `inter_me::sad::{sad_loop_kernel, nxm_sad_kernel, compute8x4_sad_kernel,
  compute8x8_sad_kernel}` and the three `ext_*_sad_calculation_8x8_16x16`,
  `port_md::pme::pme_sad_loop_kernel`,
  `port_md::md_search::PlaneDistortion::{sad, variance, ssd}`,
  `motion_est::full_pel_search`, and
  `dsp::subpel_variance::{sub_pixel_variance, variance_diff_sse}`.
  `sub_pixel_variance` additionally stopped allocating — it held two heap
  buffers per call — and its vertical bilinear pass is fused into the variance
  accumulation, so C's `H x W` `temp2` buffer is gone rather than smaller.
  MEASURED, paired A/B `main@884f94e8f` vs landed, 25 interleaved
  randomised-order rounds/cell on aarch64 / M4 Pro with no
  `-C target-cpu=native`, gradient qp40, INTER arm, **every cell
  byte-identical**: p8 1.080x / 1.154x / 1.150x / 1.158x and p6 1.073x /
  1.083x / 1.067x / 1.107x at 64/128/256/512. Port/C on the same arm moved
  from 1.92 / 2.74 / 3.40 / 3.83 to 1.82 / 2.47 / 2.99 / 3.29 (slope ratio
  3.67x -> 3.22x). The STILL and VIDEOKEY arms measured NULL, as they should —
  these are inter-path kernels. Records:
  `benchmarks/{me_sad,ext_sad8,subpel_stream,me_dist,subpel_simd}_ab_2026-09-02.*`
  and `benchmarks/perf_2026-09-02-arm-inter-simd.*`. Exactness is pinned by
  `me_sad::tests::{me_sad_all_tiers_agree, me_sum_sse_all_tiers_agree}` and
  `subpel_variance::tests::streaming_matches_materialised`, all of which run
  under `for_each_token_permutation` and CONSUME the `PermutationReport`, plus
  the unchanged tier-1 `c_parity_{inter_me,md_pme,md_subpel,motion_est,
  dist_facade,subpel_variance}` suites. Verified on BOTH ISAs: x86-64 (r7900x)
  runs 2238 dsp+encoder tests green with `inter_byte_gate` 89 and
  `regression_spotcheck` clean at the same commit. NOTE: `#[magetypes]` cannot
  express these kernels — magetypes 0.9.28 has no integer-widening conversion,
  no `abs_diff`, and `U8x16Backend::reduce_add` returns `u8`, which wraps.

- **The per-superblock MD lambda: the map, the corrected arm and C's own two
  numbers as a test — PORTED, NOT WIRED** (`rust/docs/INTER-ENCODE-PLAN.md`
  §1z²³, c25a464). Byte-inert as landed: every caller still passes qdiff 0, so
  the only changed code is unreachable. C's `fast_lambda_md` /
  `full_lambda_md` are PER-SUPERBLOCK through `update_lambda`'s
  `stats_based_sb_lambda_modulation` factor, keyed on `me_q_index`, which
  `svt_av1_generate_b64_me_qindex_map` (rc_aq.c:656) derives from
  `me_8x8_cost_variance` — a quantity this port already computes exactly. That
  map is now ported, and on `diag 72x72 q40 p6` frame 1 it reproduces C's own
  dumped `fastlam` of **5182 / 5182 / 5182 / 7773** against the port's flat
  6633, both points exact. It also found a FOURTH duplicate transcription of
  `update_lambda`: `pd0::inter_full_lambda_8bit` carried the
  `delta_q_present` arm (±8, low factor 90) where an inter frame takes the
  `me_q_index` arm (±4, low factor 100) — inert at qdiff 0, wrong on the first
  real value, corrected, and the `inter_lambda_tests` sweep now drives that
  axis against the tier-1-gated `compute_rd_mult` (mutation-verified). The
  WIRING is two chunks and is named in the entry, with the reason not to land
  only the PD0 half. `nextest --workspace` 2487/2487, spot-check 83/83,
  `inter_byte_gate` 89/0.

- **`av1_find_samples` ported — every one of the 96 inter streams now DECODES,
  and the grid goes 49 BOTH → 55** (`rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁹).
  New `inter_mvp::find_warp_samples` (C `adaptive_mv_pred.c:1610-1750`) plus
  its caller gate `svt_aom_init_wm_samples`, run per single reference in
  `inter_md_arm`, so the injector's `wm_sample_num` is real instead of
  `[0u8; 8]` and `num_proj_ref` reaches the writer. That count decides the
  motion-mode ALPHABET, so the conformance defect from §1z¹⁸ is closed: the
  decode census went from 22 rejections to 0, and it FAILED first with "22
  pinned cell(s) now decoding" rather than letting the fix land quietly. Six
  cells gained, none lost — all preset 6, the only place
  `allow_warped_motion` is 1. `inter_byte_gate.sh` asserts 55 and refusal
  #12's envelope reads 55 of 96. Five tier-4 tests pin the scan; the first
  draft of one of them expected the wrong count and was re-derived from C
  after it failed. **Warped motion itself is still unported** — the port
  writes the symbol C writes from the alphabet C writes it from, and still
  never SELECTS `WARPED_CAUSAL`. Hot-path work added: one neighbour scan per
  (block, single reference), no pixels touched, C runs the same scan under
  the same gate; not measured and no number quoted.

- **CONFORMANCE: the port emits an UNDECODABLE inter stream on 22 of 96 grid
  cells, every one preset 6** (`rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁸).
  `aomdec` rejects frame 1 with "Failed to decode tile data"; measured on both
  aarch64-darwin and x86_64-linux, same 22 cells. Traced to ONE operation: at
  p6 the frame header carries `allow_warped_motion = 1` (0 at p8 — the whole
  split), so C's `motion_mode_allowed` promotes a block with an overlappable
  neighbour to `WARPED_CAUSAL` and writes the motion mode from the
  THREE-symbol `MOTION_MODE_CDF[10]` (`[12408, 4706]`); the port's
  `num_proj_ref` is always 0 because `av1_find_samples` is unported, so it
  writes the TWO-symbol `OBMC_CDF[10]` (`[9945]`). Both write symbol 0, from
  different alphabets, and the arithmetic coder desyncs. **No user is
  exposed** — the public API refuses inter frames (refusal #12) and all 96
  cells are reached only through `SVTAV1_INTER_EXPERIMENTAL`. NOT fixed here;
  gated, and the next chunk is named (port `av1_find_samples`).
- **`tools/inter_decode_census.sh` — does the port's stream DECODE, on all 96
  cells?** `inter_decode_gate.sh` asks that of five. The 22 rejections are
  pinned BY NAME: a new one is a conformance regression, and a pinned cell
  that starts decoding means the defect moved and the list must shrink in the
  same commit. Five arms proved by mutation (unpinned rejection, pinned cell
  now decoding, no decoder → exit 2, zero decoded → anti-vacuity, a panicking
  cell → fail). The mutation also found a bug in the gate itself — the pin was
  compared against the whole list rather than the cells actually swept, so a
  narrowed grid failed for a reason that was purely its own scope. Runs in CI.

- **C's NSQ motion search and its square-MV seed are WIRED** — the ME join
  gate's two open rows close (`rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁶). C's
  `read_refine_me_mvs` seeds an NSQ block from
  `(sq_sb_me_mv[list][ref] + 4) & ~0x07` when the square parent was tested,
  then runs `md_nsq_motion_search` on it; both leaves were ported and the
  caller passed `false` / `None`. New `inter_search_arm::SqMeState` carries
  C's `ctx->sq_sb_me_mv` and answers `pc_tree->tested_blk[PART_N][0]` by
  storing the square's `(org_x, org_y, size)` rather than a boolean — a
  boolean would have answered "was ANY square tested". `inter_me_arm` gains
  `pu_geometry` (C's `pu_search_index_map` + `partition_width`/`height`,
  generated and pinned against C's literals), `number_of_pus`, `mv_at_pu` and
  `me_data_present_at_pu`. Positive controls: with the search off the rows
  read the old `(-8,-32)`, so the search is reached and load-bearing; with the
  seed off and the search on all 34 joined rows still agree, so the seed is
  **measured inert on 0 of 16 NSQ rows** and is kept for faithfulness, not for
  effect. C's third seed arm (`BLOCK_4X4` off the parent node) is NOT ported
  and is unreachable at the presets measured. Grid unchanged at 40 BOTH / 55
  F1DIFF / 1 F0DIFF with zero verdict flips; one frame-1 byte count moved
  (`diag 72x72 q55 p6`, 29 → 27 against C's 29, attributed by control to the
  search). Hot-path work was added — up to 6 MVC evaluations plus a 3-pass
  full-pel ladder per NSQ block per reference — and is **unmeasured**; C runs
  the same search at these presets, so the algorithm matches rather than
  exceeds C's, but no delta was measured and none is quoted.
- **MD priced an `intra_inter` context it did not code — the 96-cell inter grid
  goes 40 BOTH → 49** (`rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁷).
  `entropy::context::get_intra_inter_context`'s four-entry table was INVERTED
  (0 for "both neighbours intra", 3 for "both inter"; C says 3 and 0, and both
  mixed cases are 1 — 2 is the one-available-intra-neighbour value that
  signature cannot express), and its call sites collapsed "not available" into
  "intra". The correct transcription,
  `port_entropy_inter::intra_inter_context`, was already tier-1 gated and
  already used by the WRITER, so the encoder priced one context and coded
  another. **1207 rate units on every inter candidate of every block with two
  neighbours**, measured against C's own `svt_aom_inter_fast_cost`. Nine cells
  closed, none regressed; `inter_byte_gate.sh` now asserts 49 and refusal #12's
  envelope reads 49 of 96. Both MD call sites now use the oracle
  transcription, the bool form is corrected and PINNED to it over the quadrant
  they share.
- **The port paid the interpolation-filter rate at MDS0 where C pays none.**
  C's gate is `ctx->ifs_ctrls.level == IFS_MDS0` and
  `pcs->interpolation_search_level` is 2 (`IFS_MDS1`) at MR and 4 (`IFS_MDS3`)
  above it — never 1 — so no preset this port reaches prices the filter at
  MDS0. `SearchFrameCfg` carries the real level. 20-109 rate units per
  candidate; byte-neutral on the grid, landed because the port's
  `inter_fast_cost` now equals C's EXACTLY (1787 / 5216 / 10364 against
  C's 1787 / 5216 / 10364).
- **`SVT_IFCOST_OUT` — an interposer on the exported
  `svt_aom_inter_fast_cost`**, the exact counterpart of the port's
  `inter_fast_cost`, pinned by `SVT_IFCOST_XY`. It is what named both defects
  above: `SVT_FULLCOST_OUT` could only say that C reached MDS3 with one
  candidate and the port with two, which is a total, and a total is rate and
  lambda and distortion collapsed into one integer. It also showed C injecting
  and PRICING compound candidates (`NEAREST_NEARESTMV` / `NEW_NEWMV` off
  `LAST_BWD`) — so §1z¹⁶'s census result stands as "zero coded compound
  blocks" but not as "compound has zero influence": injected candidates occupy
  NIC slots.
- **`tools/inter_me_join_gate.sh` — the assertion that can SEE the NSQ
  motion-search gap.** `rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁵ recorded that
  `md_nsq_motion_search` is ported and never called and that "no assertion in
  the repo can see the difference"; this joins C's `SVT_SUBPEL_OUT` stage-0
  `start=` (the full-pel chain's output, NSQ search included) against the
  port's `PMEDBG fpme=`, per (origin, shape, list, ref). First run: 6 cells,
  34 joined rows, 16 NSQ, **2 disagreements** — both NSQ shapes on the frame
  edge of a 72x72 picture, C `(0,0)` against the port's `(-8,-32)`. It also
  named a SECOND unported thing: C seeds an NSQ block from
  `(sq_sb_me_mv + 4) & ~0x07` when the square parent was tested
  (`product_coding_loop.c:2857-2862`), where the port always takes
  `raw_me_mv * 8`. The two rows are pinned BY KEY, not by a count; the gate
  fails on an unpinned disagreement, on a pinned row that starts agreeing, on
  a run joining zero NSQ rows, and on a linker without `-Wl,--wrap`. All four
  arms proved by mutation. Runs in CI.

- **The MD motion search read PAST the source plane — 18 of the 96 inter grid
  cells PANICKED, and the sweep reported every one as an ordinary byte
  divergence** (`rust/docs/INTER-ENCODE-PLAN.md` §1z¹⁶). `InterMdFrame.src`
  took the ALIGNED source (`encode_input` at stride `w`) where C's MD searches
  read the block's whole extent and a straddling block runs into C's
  replicated border; the SB-extent-padded `sb_input` / `in_stride` the
  pipeline already builds — and that PD0's b64 variance and every straddling
  leaf's residual gather already read — was in scope the whole time. Measured:
  `port_md/md_search.rs` "the len is 5184 but the index is 5184" (5184 = 72·72)
  on every 72x72 cell of uniform, diag and screen. Byte-neutral on 64-aligned
  frames by construction; grid unchanged at 40 BOTH / 55 F1DIFF / 1 F0DIFF
  with ZERO verdict flips, and 0 CRASH.
- **A crash is now SAYABLE.** `identity_diff_inter.sh` exits **4** for a port
  panic (distinct from 3 = refused, 1 = bytes differ); `inter_byte_matrix.sh`
  has a CRASH verdict and fails on one; `inter_byte_gate.sh` fails on a crash
  from either its required or its known-open list. All three previously asked
  only "was the status 3?" and "does `rs.obu.f0` exist?" — and frame 0 IS
  written before a frame-1 panic, so `uniform 72 72 40 6` printed
  `open ... known` through the entire defect. Three 72x72 cells (one per
  panicking content class) are now crash-regression cells in `OPEN_CELLS`.
- **`tools/inter_cinter_census.sh` — what C's coded inter decision actually
  USES, per cell, joined against that cell's byte verdict.** Counts compound /
  NSQ / motion-mode / inter-intra / DRL / GLOBALMV / NEARMV straight out of
  `SVT_CINTER_OUT`, so `inter_md_arm`'s eight suppressed controls can be
  RANKED from C's own dump instead of guessed between. **It retired compound
  prediction as the next mechanism in one run: across 96 cells and 340 coded
  inter blocks C codes ZERO compound blocks**, on the 40 cells that match and
  the 55 that do not, even though `reference_select = 1` makes it reachable on
  every one. 106 blocks are NSQ shapes (94 of the 259 on F1DIFF cells).
  Both failure arms proved: exit 2 without a `-Wl,--wrap` linker, exit 1 on
  zero parsed blocks.
- **`NSQDBG ICAND` — the port-side field join against C's `SVT_CINTER_OUT`
  line** (`imc` / `drl` / `mv0` / `pmv0` / `ovl` / `rf` / ref-frame bits /
  fast luma rate), behind `SVTAV1_CANDDBG` + `SVTAV1_NSQDBG`. The funnel's
  `NSQDBG CAND` reports only the finished rate, and a total is one number
  where C's dump has six fields. On `uniform 72x72 q20 p8` frame 1 it showed
  the 8x8 corner block's inter inputs joining C's EXACTLY — so the port
  choosing intra there is candidate SELECTION (C reaches MDS3 with one
  candidate, the port with two), not the motion search and not the rate.

- **The INTER MULTI-REFERENCE path, PME included — the 96-cell grid goes
  36 BOTH / 59 F1DIFF / 1 F0DIFF to 40 / 55 / 1** (`rust/docs/
  INTER-ENCODE-PLAN.md` §1z¹⁵). New `svtav1_encoder::inter_search_arm` runs
  C's `build_single_ref_mvp_array` -> `read_refine_me_mvs` -> `pme_search`
  chain (product_coding_loop.c:9425-9447) once per single-reference entry of
  `ref_frame_type_arr`; `inter_md_arm` builds an MVP stack per reference,
  propagates the ME candidate's own list direction, predicts from the
  candidate's own reference picture and turns `inject_new_pme` on. The
  reference set and PME are ONE mechanism: on the cells C codes `rf=1` the
  LAST_FRAME NEWMV exists only because PME ran. Compound stays suppressed
  (`inter_pred_arm` has no two-reference path). `inter_byte_gate.sh` is 40
  required; `identity_full_8bit` 1100/1100, `regression_spotcheck` 67/67 and
  every other envelope gate unmoved.
- **`SVT_SUBPEL_OUT` — an interposer on the exported
  `svt_av1_find_best_sub_pixel_tree_pruned`**, which fires once per
  `(block, list_idx, ref_idx, search_stage)` and is the per-block join point
  for the MD motion searches. It exists because `SVT_INJCFG_OUT`'s `PMEST`
  line reads that state at neighbour-array-update time, where it belongs to
  whatever block MD searched last (recorded in `rust/docs/WORKING-ON-THIS.md`
  §5). It immediately named a lambda defect the byte output could not: the
  inter search was running at exactly 2x C's `full_lambda_md`.
- **`SVT_HME_OUT` and `SVT_INJCFG_OUT` — two interposers on the exported
  per-b64 ME entry and on `svt_aom_update_mi_map`** (a473fa38, fe51c8a7). The
  first dumps C's whole HME pyramid, the ENTIRE `svt_aom_sig_deriv_me` signal
  set, both pre-HME regions, `me_distortion[]` and `p_sb_best_{sad,mv}` for
  BOTH lists; the second dumps `ref_frame_type_arr`, every inter injector's
  enable flag and `valid_pme_mv` / `best_pme_mv`. Between them they refuted
  three standing premises of the inter campaign in one measurement — see
  `rust/docs/INTER-ENCODE-PLAN.md` §1z¹³ and §1z¹⁴.
- **`port_md::md_search::md_subpel_search`** (50e67db4) and
  **`md_nsq_motion_search`** — two of the three MD-search drivers between the
  port and C's reference set, tier 4, byte-inert until their callers land.
  Wiring the first found a SECOND transcription of `svt_mv_err_cost`
  (`md_subpel::mv_err_cost` vs `port_md::pme::mv_err_cost`), now pinned over
  576 cells per `rust/docs/WORKING-ON-THIS.md` §4.

- **`SVTAV1_PD0_NOSPLIT` — a CONTROL that scopes the remaining inter frontier.**
  Forces the video arm's PD0 to test only the 64x64 square on an INTER frame,
  leaving frame 0's recon (and so frame 1's reference) untouched. With it,
  `diag 64x64 q40 p8` frame 1 is BYTE-IDENTICAL to C at 22 B where the port
  otherwise emits 35 — so the inter mode decision, MVP stack, DRL choice, MV
  coding, entropy path and pack are all correct on that cell and the whole gap
  is the PD0 partition: C codes ONE 64x64 NEARESTMV skip block, the port codes
  sixteen 8x8s. C never runs this way; a byte count it produces is never a
  parity result. Full record and the next chunk's shape in
  `rust/docs/INTER-ENCODE-PLAN.md` §1z⁸.
- **`tools/inter_byte_matrix.sh` — the inter campaign's 96-cell frontier
  sweep.** `inter_byte_gate.sh` asserts the closed cells; this walks the whole
  grid and classifies each as BOTH / F1DIFF (an inter-decision defect) /
  F0DIFF (a video-KEY defect, which makes every frame-1 reading below it
  meaningless). Committed because `INTER-ENCODE-PLAN.md` §1z, §1z' and §1z''
  each re-derived the same loop in a scratch directory.
- **`SVTAV1_CANDDBG`'s `NSQDBG PINTER` line.** The candidate dump printed an
  `NSQDBG PFAST` line per INTRA candidate and nothing for the inter one, so
  "the injector ran and lost" and "the injector never ran" were
  indistinguishable. That ambiguity hid the defect below for a whole chunk.
- **The port's OWN inter ME and MVP produce C's decision** (aeca0196). Two
  permanent gates in `pipeline.rs::inter_decision_probe` answer the question
  §1s's inventory presumes: `inter_me::motion_estimation_b64` (configured by
  the ported `svt_aom_sig_deriv_me`) recovers this cell's full-pel `(-3, 0)`
  with SAD 0, and `inter_mvp::setup_ref_mv_list` +
  `port_md::drl::choose_best_av1_mv_pred` reproduce C's `pmv0 = 0,0`,
  `imc = 8` and "no DRL symbol" — with a negative control that
  `use_ref_frame_mvs = 0` gives a different mode context. So no ported inter
  algorithm is wrong on this cell; the remaining divergence is WIRING. The
  homegrown ME's quarter-pel miss was a call-site gap, not a search gap.
- **INTER MODE INFO — the real pack walk writes C's tile** (a56ef2df, ed1b10cf). The
  pre-campaign inter arm in `pipeline.rs`'s block writer wrote an MV and
  nothing else, through a `NmvContext` it rebuilt per block: no
  `write_ref_frames`, no inter mode symbol, no DRL, no interpolation filter,
  `allow_hp` hard-coded against a header that writes 0, and a RAW MV where an
  MVP difference belongs. It is replaced by
  `port_entropy_inter::block::write_inter_mode_info`, and an `is_inter` block
  that arrives without its payload now REFUSES rather than falling back —
  a quiet fallback turns an undecodable stream back into a byte divergence.
  Gated by `inter_decision_probe::the_real_pack_walk_writes_cs_inter_tile`,
  which runs C's measured frame-1 decision through `encode_block_syntax` (the
  function the entropy walk actually calls) and gets C's `94 9a b0`.
  `predmv` / `inter_mode_ctx` / `drl_ctx` are DERIVED in the pack from the
  committed mode-info grid rather than cached from MD as C does, so an MD path
  whose own grid lags cannot write a context no decoder can reproduce; new
  `EntropyCtx::mvp_grid` is that grid, stamped by every coded block.
- **INTER PREDICTION — C's 8-tap convolve replaces a homegrown bilinear**
  (81be1cab). `port_pd_pred::av1_inter_prediction_light_pd1` and the
  `port_convolve` family under it were ported and tier-1 gated and nothing in
  the encoder called them. New `crate::inter_pred_arm` is the adapter — block
  origin + size + padded reference + eighth-pel MV into `BlkGeom` /
  `RefPlane` / `MbEdges` / `ScaleFactors` — and it is deliberately its own
  module, because every mode-decision path needs that conversion while only
  the call site is code the campaign's item 1 will bypass. Gated by a
  positive control shaped to distinguish an 8-tap filter from a 2-tap one: a
  half-pel prediction must leave the interval bounded by its two neighbouring
  samples, which a bilinear average cannot do. The experimental inter cell's
  frame 1 goes 75 B -> 85 B — NOT a regression: the smaller number came from a
  prediction no decoder has, and the increase is evidence about the homegrown
  ME's quarter-pel MV under an 8-tap filter, not about the convolve.
- **SIX inter-syntax defects a byte gate cannot see, found by DECODING**
  (SHA4). Every gate here compares the port's bytes to C's; none asks whether
  the port's own bytes are a bitstream. `dav1d` over the experimental 2-frame
  stream said no (`aomdec`: "Failed to decode tile data"), with C's stream
  decoding as the control. Fixed: `write_is_inter` used a CONSTANT context
  where C computes a 4-valued one; an INTER block wrote the intra `uv_mode`
  symbol (behind a `debug_assert!` that RELEASE builds compile out); the
  var-tx arm, BOTH luma coefficient call sites and the chroma tx type all
  picked `use_intrabc` where C's predicate is `is_inter_block` =
  `use_intrabc || ref_frame[0] > INTRA_FRAME`; and the mi grid stamped
  `DC_PRED` as an inter neighbour's mode, moving `mode_context`. Four of the
  six are that one predicate, which was only ever right because IntraBC was
  the sole inter-classified block the pack could emit. New
  `tools/inter_decode_gate.sh` (evidence tier 3) requires three cells to
  decode completely and lists two known-open ones with their measured reason;
  its anti-vacuity was checked by reverting fixes, and it witnesses the
  `uv_mode` leak but NOT the constant-context fix, which is recorded rather
  than glossed. New `SVTAV1_INTERDBG=1` prints the per-block inter decision as
  the WRITER sees it, and `SVTAV1_PACKTREE`'s `PDV` line gained the inter
  fields — `PACKTREE` prints `intra_mode`, which an inter block leaves at 0,
  so an inter leaf had been indistinguishable from a DC intra one.
- **REFERENCE PADDING — the DPB carries C's replicated margin** (ed1b10cf).
  `pad_ref_and_set_flags` (enc_dec_process.c:1072) pads a recon with
  `border = BLOCK_SIZE_64 + 4` before it becomes a reference, because a legal
  MV puts the predicted block partly outside the frame. The port stored bare
  planes and filled those samples with the constant 128. New
  `picture::PaddedPlane` / `PaddedRef` on `ReferenceFrame::padded`, built from
  the tier-1-gated `port_preanalysis::generate_padding`. It is a MODE-DECISION
  requirement and not only a conformance one: on the campaign's own cell the
  correct MV matches EXACTLY only against a replicated margin, so C's
  `skip = 1` is unreachable against a fill.
- **`av1_code_tx_size` picked its arm on `use_intrabc` instead of
  `is_inter_block`** (a56ef2df). C's predicate is
  `use_intrabc || ref_frame[0] > INTRA_FRAME` (block_structures.h:119); while
  IntraBC was the only inter-classified block the pack could emit the two
  agreed. A genuinely inter block took the INTRA arm and coded a `tx_size`
  depth symbol C does not write — MEASURED as one extra 3-symbol write with
  every symbol before it identical in `nsyms`, `s`, `icdf` and range.
  `record_inter_dims` had the same predicate.
- **`port_enc_mode_config::me::apply_me_signals`** (aeca0196), the bridge from the
  ported `svt_aom_sig_deriv_me` to the ported `inter_me` search: seven C
  structs were transcribed TWICE, once per lane, with no conversion. Five
  fields differ in width from `me_context.h` and `inter_me` is the faithful
  side in all five; measured inert on every value the derivation can produce.

- **CDF CONTINUATION — the per-reference-slot frame-context store** (31bdc16e).
  The byte-exact inter frame header says `primary_ref_frame = 0` with
  `error_resilient_mode = 0`, so the tile's CDFs must start from the REFERENCED
  frame's end-of-frame state; the port had no such store anywhere. New
  `crate::port_frame_cdf` holds C's `FRAME_CONTEXT` as one object (the port
  splits it across `entropy::context::FrameContext`, `entropy::coeff_c::CoeffFc`
  and `port_entropy_inter::InterCdfs`) plus the port of
  `svt_av1_reset_cdf_symbol_counters`; `picture::ReferenceFrame` carries it; the
  entropy walk seeds every tile from `ref_dpb_index[primary_ref_frame]`'s saved
  state (C `ec_process.c:101-112`). Note what C does NOT do on that arm:
  `svt_av1_default_coef_probs` is skipped, so the coefficient CDFs come from the
  reference and not from this frame's own `base_q_idx`. A frame that names a
  `primary_ref_frame` whose slot carries no saved CDFs REFUSES rather than
  falling back to the defaults. MEASURED: the port's saved end-of-frame-0
  context is byte-identical to C's for all 96 shared fields, through a new
  `__wrap_svt_av1_reset_cdf_symbol_counters` oracle (`SVT_FCTX_OUT`) and
  `tools/fctx_diff.py`; the four fields C carries and the port does not
  (`delta_lf`, `delta_lf_multi`, `palette_uv_size`, `palette_uv_color_index`)
  are asserted to be exactly that set. New TIER-1 gate
  `tests/c_parity_frame_cdf.rs` drives `svt_aom_init_mode_probs`,
  `svt_av1_default_coef_probs` and `svt_av1_reset_cdf_symbol_counters`.
- **The inter TILE is BYTE-IDENTICAL to C's, from C's measured decision**
  (e092afd2). C's frame-1 tile on `gradient 64x64 q40 p6 frames=2` is three
  bytes (`94 9a b0`) and the port now produces exactly those. What that proves
  byte-exact: CDF continuation, the partition symbol, skip, is_inter,
  `write_ref_frames`, the inter mode symbol, DRL gating, MV coding, the
  interintra/motion-mode/compound gates and the interpolation filter. What it
  does NOT test is MODE DECISION — the block decision fed in is C's own,
  measured through a new `SVT_CINTER_OUT` dump rather than fitted to the bytes
  (one 64x64 `PARTITION_NONE` block, `NEWMV` off `LAST_FRAME`, MV `(0,-24)`
  eighth-pel, `EIGHTTAP_REGULAR`, `skip = 1`). **So for that cell the entire
  remaining divergence is mode decision.** Two permanent negative controls: the
  same decision from DEFAULT CDFs does not reproduce C's bytes, and frame 0 is
  asserted to be 961 B before anything is read out of it.
- **The INTER frame emits, and its header is field-exact but for two CDEF
  strengths.** Frame 1 of a 2-frame low-delay-P encode was refused at the
  `pipeline.rs` entry guards; it now encodes. `entropy/obu.rs`'s
  `key_frame_header_bits_lr` becomes `frame_header_bits_lr` with an
  `Option<&InterSignal>` (`None` reproduces the key layout bit for bit through
  a `write_key_frame_header_full_lr_sb` shim), the bitstream assembly is one
  path for both frame types, and the new `crate::inter_hdr_arm` feeds the
  header from `port_picstruct::picture_decision_per_picture` (references,
  refresh mask, `primary_ref_frame`) and
  `svt_aom_sig_deriv_mode_decision_config_default` (the tool ladders). The CDEF
  pick now runs on every coded frame with `cdef_frame_is_boosted` /
  `cdef_is_not_highest_layer` read from the picture decision's `update_type`
  instead of a literal `is_key`, and `ReferenceFrame` carries chroma. On
  `gradient 64x64 q40 p6`, 13 of the 15 frame-header bytes match C and the
  field walk names the two open ones (`cdef_y_pri_strength[0]`,
  `cdef_uv_pri_strength[0]`, both `search_best_ref_fs` — see
  `docs/INTER-ENCODE-PLAN.md` §1q). **The public API still refuses inter
  frames**; `SVTAV1_INTER_EXPERIMENTAL` lifts the guard for the differential
  harness only. New gate `tools/inter_fh_gate.sh`. No regression:
  `identity_full_8bit` 1100/1100, `regression_spotcheck` 64/64, video-key
  matrix 58/60, six pinned still cells at 290/839/63/171/580/693 B, workspace
  2422/2422 (aarch64); cross-ISA on x86-64: the same gate with the same
  result, spot-check 65/65, workspace 2432/2432.
- **The inter frame header is BYTE-IDENTICAL to C's** — all 15 bytes of frame
  1 on `gradient 64x64 q40 p6` (`docs/INTER-ENCODE-PLAN.md` §1r). The residual
  was never a CDEF search difference: C does not search on that frame.
  `set_cdef_search_controls` level 5 sets
  `search_best_ref_fs = is_not_highest_layer ? 0 : 1`, which is 0 for every key
  frame, so `update_cdef_filters_on_ref_info` (`md_config_process.c:681`) is
  unreachable on the still envelope; on the first inter frame it takes the
  `use_reference_cdef_fs` arm and hands the frame the REFERENCE picture's own
  strengths with no search. `ReferenceFrame` now carries
  `cdef_{y,uv}_strengths` (C's `EbReferenceObject::ref_cdef_strengths`) and
  `port_enc_mode_config::cdef_search` gains the function. Named gap: C reaches
  it only after `me_based_cdef_skip` declines, which needs ME distortion this
  pipeline does not produce (inert on any I_SLICE). Frame 1's whole remaining
  divergence is the TILE: C 3 bytes, port 94.

### Changed

- **The tx-type search runs C's two phases — transform + SATD screen first,
  quantize/RDOQ/cost only for the survivors — and is 1.33x faster at 512² p2,
  byte for byte.** `txt_search` committed the WHOLE `tx_unit` pipeline on every
  gated tx-type trial and applied C's SATD early exit post-hoc from a
  `txb_coeff_satd` that re-derived the residual AND the forward transform
  (`benchmarks/callcount_2026-09-04`: 488,414 committed trials vs C's 270,415
  at gradient 512x512 qp40 p2, one edge = 45.2 % of the port's p2
  instructions). `tx_pipeline::SatdScreen` is now C's `best_satd_tx_search`
  running minimum (`product_coding_loop.c:4741-4755`), evaluated inside
  `tx_unit_screened` / `tx_unit_hbd_screened` between the transform and the
  quantizer; a rejected trial returns there; `detect::txb_coeff_satd{,_hbd}`
  is deleted. MEASURED (r7900x callgrind, nine byte-identical runs): the
  tx-type-search quantize edge is EXACTLY C's — 270,415 = 270,415 (p2),
  8,397 = 8,397 (p6), 608 = 608 (p10); the redundant-residual edge 484,442 ->
  0; p2 Ir total -23.5 %. Wall clock (paired A/B, 9 rounds): 512² p2 652.5 ->
  491.3 ms (1.325x), 64² p2 1.486x, p6 1.00-1.07x, p10 1.00x (control).
  Gates on the tree rebased onto the IFS wiring
  (aarch64): nextest 2524/2524, `regression_spotcheck` 102/102, six still
  cells byte-identical, `inter_byte_gate` 96 required / 0 failed / 1 known-open,
  `inter_decode_gate` 5/5, `inter_decode_census` 96/96; pre-rebase
  `identity_full_8bit` 1100/1100 (aarch64 AND x86-64), completion scan 64/64
  OK, x86-64 nextest 2529/2529. Records `rust/benchmarks/callcount_txtscreen_2026-09-04.{tsv,meta}`,
  `perf_ab_txtscreen_2026-09-04.tsv`; `rust/docs/perf-status.md` updated in
  place. Still open: the residual is derived once per tx-type TRIAL where C
  derives it once per TXB (595,871 vs 435,245 calls at p2).

### Fixed

- **NIC stage caps use C's PICTURE TYPE on inter frames (2026-09-04).**
  `leaf_funnel::rate_tables::nic_counts` hardcoded the I_SLICE row of
  `MD_STAGE_NICS` (definitions.h:811), so every inter frame ran I-slice stage
  caps — at `p6 q40` an MDS1 cap of 5 where C (`set_md_stage_counts`,
  product_coding_loop.c:1398, picture type 1 on a flat GOP) runs 3, and 3 vs 2
  at `p8 q20`. It is now a front on the tier-1 `port_md::nics::set_nics`, with
  the picture type from the new `port_picstruct::is_highest_layer`
  (pd_process.c:5560 — FALSE on every picture of a flat GOP); the same helper
  replaces the `temporal_layer_index != hierarchical_levels` paraphrase in
  `inter_hdr_arm` (wrong at (0,0)) and the DLF block's inline copy. MEASURED
  byte-inert: the 96-cell inter grid is identical row for row (94 BOTH / 1
  F1DIFF / 1 F0DIFF), the eight `frames=3` cells unchanged, stills 1100/1100 —
  the extra MDS1 survivors the I-slice row admitted never won. Record:
  `docs/INTER-ENCODE-PLAN.md` §1z³⁷.
- **The three residual F1DIFF cells are a COST comparison, not a search — and a
  module header said otherwise.** `inter_md_arm`'s header claimed
  `md_nsq_motion_search` is "PORTED but NOT CALLED here ... so an NSQ block here
  takes the square path", quoting 94 of 259 coded inter blocks as its reach.
  `inter_search_arm` builds that search's MVC list and passes it into
  `refine_me_mv_for_ref`; the search runs. MEASURED on `diag 72x72 q55 p6`, the
  cell that reading would have explained: both sides code the SAME six blocks at
  the same positions and differ at one, and C's own `SVT_SUBPEL_OUT` there
  reports `start=(32,8) best=(32,8)` — **the port's ME MV exactly** — with
  `nsqme=1` confirmed from C's `SVT_INJCFG_OUT`. C codes NEARMV `(24,0)` because
  its COST wins, not because its search found something else; the port injects
  that candidate at C's own MDS0 rate (2845) and picks NEWMV (6774) on
  distortion. The residual is the same class as `video_key_matrix`'s two unmoved
  cells, and the instrument for it is `SVT_FULLCOST_OUT`, not the ME. Header
  corrected with the stale census kept and dated. **Drilled to the end the same
  day**: the port's MDS1 costs match C's to the UNIT on five of six candidates
  at that block (distortion, rate and lambda) and to 0.30 % on the sixth, and
  NEARMV wins at MDS1 on BOTH sides — what differs is that C admits TWO
  candidates to MDS3 and the port admits THREE, i.e. C's post-MDS1 NIC prune
  drops the NEWMV the port keeps, whose distortion collapses from 95 239 to
  38 192 once the real transform and RDOQ run. The target is
  `nic::stage_mds1_to_mds3`, and it is the same target as `video_key_matrix`'s
  two unmoved cells. Full record
  `rust/benchmarks/f1diff_q55_localization_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z³².

- **`skip_mode` is signalled and coded — FIVE of eight three-frame cells are now
  byte-identical end to end.** `pd_process.c:4958` assigns
  `frm_hdr->skip_mode_params.skip_mode_flag = skip_mode_allowed`, and
  `entropy_coding.c:5119` codes a `skip_mode` symbol on every block of an inter
  FRAME whose `bsize` allows compound. The port hard-coded the flag `false`,
  coded no symbol, and priced no skip-mode rate — right by accident on every
  frame this repo's gates reach, because `skip_mode_allowed` needs two
  references at DIFFERENT order hints and the campaign's first inter frame has
  every DPB slot holding the key frame. Three wires, no new transcription:
  `skip_mode_context`, `encode_skip_mode`, `is_comp_ref_allowed`,
  `setup_skip_mode_allowed` and the `InterFacBits::skip_mode` rate table were
  all already in tree. MEASURED at `frames=3` with the frame-2 refusal lifted
  behind a throwaway env: `gradient 64x64 q32 p8`, `diag 64x64 q40 p8`,
  `uniform 64x64 q40 p6`, `screen 64x64 q40 p6` and `diag 128x128 q40 p6` are
  now byte-identical on **every** frame, where this chunk sequence started with
  frame 2 at 466 B against C's 21. **The refusal STAYS** — `gradient 64x64
  q40 p6`, `diag 72x72 q40 p8` and `gradient 128x128 q40 p8` are still wrong, so
  lifting it would be the partial lift the two `refuses_inter3` cells exist to
  prevent; converting it into the PASS/OPEN gate model the frame-1 path uses is
  written down as a decision, not taken. Byte-inert on the two-frame envelope
  (grid 92 BOTH / 3 F1DIFF / 1 F0DIFF cell for cell, identity 1100/1100). Full
  record `rust/benchmarks/frame2_skip_mode_wired_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z³¹.

- **`fctx_gate.sh` compares EVERY frame's end-of-frame CDF state, not just
  frame 0** — and the first thing it found is the symbol frame 2 is missing.
  The gate stopped at frame 0 on the reasoning that "frame 1's saved context
  can only match once the inter tile does"; frame 1's tile is byte-identical on
  the campaign's cells, so that state had simply never been under test even
  though it is what a THIRD frame restores from. Extended, it reports 96/96
  identical at frames 0 and 1 of `diag 64x64 q40 p8 frames=3` and exactly ONE
  differing field at frame 2: `skip_mode`, C 138 against the port's 147 — the
  DEFAULT, i.e. C adapted that CDF and the port never coded the symbol. That
  localized the frame-2 tile divergence in one command to
  `frm_hdr->skip_mode_params.skip_mode_flag`, which `pd_process.c:4958` assigns
  from `skip_mode_allowed` while `inter_hdr_arm` hard-codes `false` (the ninth
  "a caller passes a constant where the derivation is already ported" of this
  campaign; `setup_skip_mode_allowed` is ported at tier 1 and
  `encode_skip_mode` / `skip_mode_context` are ported and called by nothing).
  It is inert before frame 2 because `skip_mode_allowed` needs two references
  at different order hints. Mutation-tested: changing one value of frame 1's
  `skip_mode` row makes the gate report `95 identical, 1 differ` and exit 1.
  Full record `rust/benchmarks/frame2_skip_mode_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z³⁰.

- **C's frame-2 header codes the low two bits of MINUS THREE — the CDEF-off
  gate the port never tested.** The frame-2 divergence on
  `diag 64x64 q40 p8 frames=3` looked like `cdef_damping_minus_3` (C 1,
  port 2), which reads as a CDEF search output. It is not:
  `CDEF_DAMPING_FROM_QP(160) = 5` (`enc_cdef.c:895`) means the field must be 2
  on both sides, and 1 is the low two bits of `0 - 3` — i.e. `cdef_damping` was
  still its `resource_coordination_process.c:423` initialiser because **C's
  frame 2 never ran CDEF at all**. C's `md_config_process.c:980-985` tests three
  CDEF-off gates and only ELSE-IF none fired rewrites the candidate set from the
  reference; the port ran the rewrite unconditionally. The live gate here is
  `cdef_ctrls->skip_th && skip_perc >= CLIP3(25, 100, skip_th + (base_q_idx -
  128) / 4)`: at preset 8 `skip_th` is 80 on a non-base frame, the threshold is
  88, and `ref_skip_percentage` is 0 at frame 1 (an I_SLICE reference) but
  **100** at frame 2, whose reference is a 22-byte all-skip frame. Now ported as
  `cdef_search::cdef_skip_gate` with four tier-4 tests for the two details that
  are easy to lose (the guard is on the RAW `skip_th`; C's `/ 4` truncates
  toward zero). MEASURED: **no frame-header field differs on that cell any
  more** — the first divergence moves from byte 15 to byte 18, into the tile
  payload — and six of the eight `frames=3` cells move likewise. Byte-inert on
  the two-frame envelope (grid 92 BOTH / 3 F1DIFF / 1 F0DIFF cell for cell,
  identity 1100/1100) because `skip_th` is 0 at every preset up to M7 and on
  every base frame. `me_based_cdef_skip`, the first of the three gates, stays
  unmodelled and is inert below preset 9 by C's own `zero_filter_strength_lvl`
  table. Full record `rust/benchmarks/frame2_cdef_skip_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z²⁹.

- **The temporal motion field was PORTED and never WIRED.** C's
  `av1_copy_frame_mvs` (`coding_loop.c:1038`), `motion_field_projection` and
  `av1_setup_motion_field` (`md_config_process.c:427/523`) were all in tree at
  tier 4 with traced vectors, called by nothing — and `port_coding_loop`'s own
  module doc had said since it landed that without them "every frame from the
  SECOND inter frame onward gets wrong TMVP candidates". What was missing was
  the state between them: `ReferenceFrame` now carries C's per-8x8 `MV_REF`
  grid and `ref_order_hint[7]`, the walk's `update_b` port folds the grid under
  C's own gate (`mfmv_enabled && !I_SLICE && is_ref`), and
  `inter_mvp_env.tpl_mvs` is `setup_motion_field`'s output over the DPB rather
  than an all-`INVALID_MV` constant — with `ref_frame_side`, its other product,
  carried to the walk from the same call so the two cannot disagree. MEASURED
  at poc 2 of `diag 64x64 q40 p8 frames=3`: the port's `NEARESTMV` becomes C's
  `(0,-24)` off a stack of 1 where it was `(0,0)` off an empty one, and **six
  of eight `frames=3` cells now match C's frame-2 byte count** (21/21, 21/21,
  21/21, 21/21, 21/21, 23/23, 26/27, 23/35). None is byte-identical, so the
  frame-2 refusal STAYS, re-keyed on the recon. Byte-inert on the two-frame
  envelope structurally, not by luck — C's own projection returns 0 for a
  key-frame start frame — and the grid is 92 BOTH / 3 F1DIFF / 1 F0DIFF cell
  for cell before and after. Guarded by two new `mfmvField` spot-check cells
  reading a `PORTREFSTATS ... mfmv=<named>/<len>` census, because the wire's
  only consumer is a frame the port still refuses and so it has no byte
  observable at all. Full record
  `rust/benchmarks/frame2_mfmv_wiring_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z²⁸.

- **The frame-2 refusal named the wrong mechanism, and its "466 B" was a
  hard-coded DPB slot.** Three pipeline sites read the reference picture —
  `ref_frame_data` (the open-loop ME's plane), `ref_padded_luma` (what motion
  compensation indexes) and PD0's `sb_min_sq_size` read — and all three took
  `self.dpb.get(0)`, a hard-coded slot, where C resolves LAST through
  `pcs->ppcs->ref_pic_ptr_array[REF_LIST_0][0]` i.e. `rps.ref_dpb_index[LAST]`.
  They agree on every frame this repo's gates cover (poc 1's LAST *is* slot 0)
  and diverge at poc 2, because frame 1 refreshes slot 1 — which the previous
  entry's DPB fix made real. MEASURED on `gradient 64x64 q32 p8 frames=3`: the
  port's frame-2 MD searched `mv=(2,-36)` against poc 0 where the true poc-1
  displacement is `(0,-24)` and coded 100 % intra at **466 B against C's 21**;
  after, **22 B**, with frames 0 and 1 byte-identical and the two-frame byte
  grid unmoved cell for cell (92 BOTH / 3 F1DIFF / 1 F0DIFF). Seven other
  `frames=3` cells land at 21/21, 21/21, 21/21, 26/27, 24/23, 24/35, 22/21.
  The frame-2 refusal is **re-keyed, not lifted**: `fh_fields.py` shows
  `use_ref_frame_mvs = 1` on BOTH sides at poc 2 while the port's `tpl_mvs` are
  all `INVALID_MV` — and that is a missing WIRE, not a missing port:
  `inter_mvp::{motion_field_projection, setup_motion_field}` and
  `port_coding_loop::copy_frame_mvs` are all ported and tested at tier 4, while
  `ReferenceFrame` carries no per-8x8 `MV_REF` grid and `tpl_mvs` is built as a
  constant. C codes that frame as
  `NEARESTMV mv=(0,-24)` off a stack with zero spatial matches where the port
  reports `refmvcnt=0` and `(0,0)`. Faithful at two frames, where C's own
  projection returns 0 for a KEY-frame reference. Full record
  `rust/benchmarks/frame2_last_slot_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z²⁷.

- **C injects a `NEARMV` candidate on every inter frame and the port injected
  NONE — the inter byte grid goes 91 → 92 of 96.** `inter_md_arm` handed the
  candidate injector a defaulted `near_count_ctrls`, justified by a module
  comment reading "C caps the NEAR DRL loop to ZERO unless this control is
  enabled ... so `NEARMV` is absent exactly the way C makes it absent". That
  is a correct reading of C's `enabled == 0` arm (`mode_decision.c:1377-1381`)
  and a wrong conclusion: `enabled` is **1 in all seven arms** of
  `set_cand_reduction_ctrls` (`enc_mode_config.c:4113` onward) and the video
  arm's `pcs->cand_reduction_level` is 0, 1 or 2 (`:9039-9050`) — each with
  `near_count = 3`. MEASURED on `diag 72x72 q40 p6` frame 1 by joining C's
  `SVT_IFCOST_OUT` to the port's `SVTAV1_CANDDBG` at `mi=(8,16)`: C's MDS0
  list carries `mode=14 NEARMV` at `fast_luma_rate = 2845` and codes it, the
  port had no such candidate and coded `NEWMV` at 4187 with the SAME MV
  `(24,0)`; the port's rate model was already exact on both candidates the two
  lists share (2520 and 4957 to the unit). The control is now derived through
  the already-ported, tier-1-gated
  `port_enc_mode_config::encdec::set_cand_reduction_ctrls`, so this was a
  missing wire and not a missing port — the **sixth** "a caller passes a
  constant where the derivation is already ported" finding of the inter
  campaign. `inter_byte_gate` 93 → **94 required, 0 failed** (mutation-verified:
  forcing the control off fails exactly `diag 72 72 40 6`); the three residual
  F1DIFF cells did not move by a byte. Full record
  `rust/benchmarks/inter_near_candidate_2026-09-03.md`,
  `rust/docs/INTER-ENCODE-PLAN.md` §1z²⁶.

- **The port's DPB never received an inter frame, and the frame-2 refusal was
  naming a gap it had closed.** `PictureControlSet::new_inter_frame`
  hard-coded `refresh_frame_flags: 0`, and that constant — not
  `pic.rps.refresh_frame_mask`, which the frame HEADER already writes — is
  what reached `self.dpb.refresh(..)`, so the stream announced C's real mask
  while the encoder's own DPB stayed all-key-frame. MEASURED at poc 2 of
  `gradient 64x64 q32 p8 frames=3`: `rps.ref_dpb_index[0]` is slot 1 and all
  eight slots still held the key frame, so LAST resolved to poc 0 where C's
  resolves to poc 1. Invisible at two frames (nothing reads the DPB after
  frame 1), which is the whole envelope every gate covers — so **any frame-2
  reading taken before this fix is void**. Alongside it, the DPB entry now
  carries C's three coded-area percentages (`intra`/`skip`/`hp`), the
  per-superblock `sb_intra` / `sb_skip` flags and the picture's `slice_type`,
  accumulated in the walk exactly where C's `update_b` does
  (coding_loop.c:1605-1643) and VERIFIED against C's own reference objects
  through `SVT_REFSTATS_OUT`: C reads its frame-2 list-0 reference as
  `slice/intra/skip/hp = 0/0/100/0` and the port writes exactly that onto
  frame 1's entry. `MdConfigInputs` derives all three `ref_*_percentage`
  values from them instead of from placeholder zeros, and its
  `ref_list{0,1}_count_try` fields are now filled from the CAPPED counts C
  reads rather than the uncapped `ref_list{0,1}_count`. Byte-inert on the
  two-frame envelope by construction. The frame-2 refusal is NOT lifted: with
  these in and nothing else, frame 2 codes 466 B against C's 21, so it is
  re-keyed on the gap that is actually left — `pd0_detector` reads
  `ref_obj_l0->sb_intra[sb_index]` per superblock and
  `part_arm::VideoPic` has no `InterOnInterRef` arm.
  (`docs/INTER-ENCODE-PLAN.md` 1z25,
  `benchmarks/frame2_mechanisms_2026-09-03.md`)


- **C's MD lambda is PER-SUPERBLOCK and this port had one per frame — the
  inter byte grid goes 89 -> 91 of 96, `inter_byte_gate` 91 -> 93.**
  `svt_aom_mode_decision_configure_sb` (md_process.c:796) is called per
  superblock with `svt_aom_get_me_qindex(pcs, sb_ptr, ..)`, so
  `full_lambda_md[0]` / `fast_lambda_md[0]` / `full_sb_lambda_md[0]` vary by
  superblock even with no per-SB delta-q signalled: `update_lambda`'s
  `stats_based_sb_lambda_modulation` block keys on `me_q_index - base_q_idx`,
  and `me_q_index` is derived from `me_8x8_cost_variance` alone (rc_aq.c:656).
  MEASURED against C's own `SVT_PD0CFG_OUT` on `diag 72x72 q40 p6` frame 1:
  C's `fastlam` is 5182 / 5182 / 5182 / 7773 across the four superblocks of
  one frame where the port reported a flat 6633. Wiring it promoted
  `gradient 72x72 q20 p6` and `diag 72x72 q20 p8` to byte-identical and
  CLOSED the residual 72x72 under-split — the partition tree on
  `diag 72x72 q40 p6` is now C's exactly (five inter blocks, `mi=(8,16)`
  included, both edge shapes `BLOCK_16X32` `PARTITION_VERT`), with the
  remaining byte a MODE divergence (port NEWMV, C NEARMV, same MV). The
  producer and consumer had been ported and tested since 2026-09-02 and only
  the pipeline threading was missing. Byte-inert on stills and key frames by
  construction (the per-SB array is `None` unless the frame has a DPB
  reference). It also deleted a duplicate transcription: the frame inter
  lambda was derived once for `c_quant` and again for
  `inter_search_arm::SearchFrameCfg`, so the two lambda fields moved to the
  per-block `BlockSearchIn` and `SearchFrameCfg` can no longer carry one.
  Mutation-verified: with the per-SB array forced to `None` the gate reports
  the two promoted cells failing; restored, 0 failed. The per-SB path is
  gated on C's own `stats_based_sb_lambda_modulation`
  (`enc_mode <= (rtc ? ENC_M10 : ENC_M11)`, enc_handle.c:4375), which is OFF
  at presets 12-13 — where C never builds `b64_me_qindex` at all — and that
  boundary is guarded by a unit test because no byte gate in this repo
  reaches it.
  (`docs/INTER-ENCODE-PLAN.md` §1z24,
  `benchmarks/inter_byte_matrix_2026-09-03-sblambda.{tsv,meta}`)


- **`use_ref_frame_mvs` at `mfmv_level >= 2` is a CLOSED FORM here, and
  refusing it cost twelve inter cells.** `inter_completion_scan.sh` goes
  **52 OK / 12 REFUSED -> 64 OK / 0 REFUSED**, and `576x576` at presets 6 and 8
  is byte-identical to C on BOTH frames (frame 0 41,537 B, frame 1 35 B) where
  it previously produced no stream at all. The refusal said the bit "needs the
  TPL r0 and the references' own is_mfmv_used" — true of C in general, false of
  any configuration this port can encode: C's `mfmv_controls` sets
  `r0_th = scs->tpl ? 0.1x : 0` and guards that whole block behind
  `if (r0_th)`, and `get_tpl` (`Globals/enc_handle.c:3657`) returns 0 for
  `aq_mode == 0`, which this port refuses to be anything else. `mfmv_controls`
  was ALREADY ported and tier-1 C-parity-tested in
  `port_enc_mode_config::tail`, with a doc comment stating that argument in
  those words; `inter_hdr_arm` re-derived the rule and refused, and
  `inter_mvp_env` carried a THIRD copy spelled `mfmv_level == 1`. All three now
  call the one ported function (`docs/WORKING-ON-THIS.md` §4). Refused only
  when TPL is genuinely on. New `inter_byte_gate.sh` cells at 576x576 — the
  first cells any byte gate in this repo has had above 360p, which is exactly
  where `mfmv_level` stops being 1. CI floors raised to 64 OK / 0 REFUSED.
- **Global motion was never refused — only a comment said it was.**
  `inter_syntax_state` claimed `inter_hdr_arm::inter_signal` "refuses a
  non-identity model, so every reference is IDENTITY here by the same rule the
  header is written under — not by assumption", and
  `InterHdrError::GlobalMotionNotImplemented` existed for it. That variant was
  never constructed anywhere in the crate. C's `svt_aom_derive_gm_level`
  (`enc_mode_config.c:194`) gives a NON-I-slice at `enc_mode <= ENC_M4` a
  non-zero `gm_level`, so C searches a model and `global_motion_params()` codes
  its type and parameters while this port writes seven `is_global = 0` bits —
  and the whole inter campaign measures preset >= 6, where C's own level is 0,
  so nothing could reach it. Now a real refusal at the `encode_frame_impl`
  choke point.
- **The sequence header underflowed `frame_width_bits_minus_1` at width or
  height 1, on BOTH the 4:2:0 and monochrome paths.**
  `32 - (1 - 1).leading_zeros()` is 0, so `w_bits - 1` wrapped: a release build
  wrote 15 into that 4-bit field ("16 bits follow") and then wrote ZERO bits of
  `max_frame_width_minus_1`, shifting every later field. MEASURED: the port's
  1x1 stream was 21 B and dav1d said "Error parsing sequence header / Overrun
  in OBU bit buffer"; C's 1x1 stream is 21 B and decodes. After: 24 B on both
  sides, **byte-identical**, and likewise at 1x8, 1x64, 64x1. `verify_settings`
  accepts width and height down to 1, so these are inside C's envelope and
  always were; no gate encoded a 1-pixel dimension. Found while lifting the
  mono arbitrary-dims refusal, which made these sizes reachable on a second
  path. Five new `regression_spotcheck.sh` cells (four cases + a 2x2 control);
  reverting the fix fails exactly those four **at identical byte counts**,
  which is why they are byte cells and not size cells.
- **QP 0 (coded-lossless) on SCREEN CONTENT works at preset >= 6, and the
  blanket refusal was hiding a crash at the other end.** The refusal read "not
  byte-verified against C so far" — a statement about effort. Measured:
  presets 6..13 are **48/48 byte-identical to C** over `{screen, screenrep}` x
  `{64x64, 128x128, 96x80, 200x136}` and lossless under aomdec in every cell;
  preset 5 diverges on `screenrep` only (128x128 port 17,241 B vs C 17,242,
  both lossless — an RD residual like the pinned p0..p3 set); presets 0..4
  **PANIC** in `intrabc_hash::get_block_hash_value`, QP-0-specific (qp 1, 2, 5,
  20, 40 all encode at preset 4). `AvifEncoder`'s DEFAULT speed 6 maps to
  preset 7, so lossless AVIF of a screenshot was refused at the default setting
  for want of running it once. The refusal now stops at preset 6 and names both
  causes. Pinned from BOTH sides: three `byte` cells for the lift and two
  `refuses` cells (a new `regression_spotcheck.sh` helper that tells exit 3
  apart from a panic) for the presets that must stay refused — without the
  second pair, deleting the refusal outright would pass every test in the repo
  and re-enable the panic. New CI invocation of `lossless_gate.sh` over the
  screen contents at presets 6..13.

- **Monochrome encodes arbitrary (non-8-aligned) dimensions — the AVIF alpha
  case.** An alpha plane is a monochrome AV1 image at the picture's own size,
  and the mono path refused anything not already 8-aligned ("arbitrary-dims
  padding is wired on the 4:2:0 path only"). It is not 4:2:0-specific:
  `encode_frame_impl` already takes the padded plane at the ALIGNED stride and
  signals the TRUE size, and the mono arm differs only in plane count. The
  padding is now shared (`encode_frame_mono_core`). MEASURED: 100x100, 98x78,
  99x77, 171x33 and 250x150 all encode, decode under BOTH aomdec and dav1d, and
  the decoder emits exactly `w*h` luma bytes — which is what proves the stream
  announces the true size. Recon-equality cells added to
  `regression_spotcheck.sh` (93/93, was 83/83). No byte oracle exists and never
  will: C cannot encode monochrome at all.


- **PD0's INTER arm never reached the REFINEMENT path — the grid goes
  67 BOTH → 89** (`rust/docs/INTER-ENCODE-PLAN.md` §1z²², 89ec75a). `pipeline.rs`
  builds `pd0_inter` (the reference planes, the frame's update types and the
  superblock's `min_sq`) and the non-refined arm already used it; the
  `refined` arm — every preset ≤ 6 on both arms — called the ALLINTRA entry
  point instead, so an inter frame's PD0 predicted DC intra, priced with the
  KEY-frame lambda and descended to 8x8. On `gradient 64x64 q20 p6` frame 1
  that is 80 evaluated nodes against C's 5 and a 64x64 distortion of 2 045 904
  against C's 50 800; after the fix all five nodes match C field for field.
  Twenty-two cells promoted, none regressed. `inter_byte_gate` 67 → **89**,
  mutation-verified (reverting the argument fails exactly the 22). Refusal
  #12's envelope reads 89 of 96. The residual six are five 72x72 partial-SB
  cells plus `diag 128x128 q20 p8`, and the direction on them has FLIPPED to
  an UNDER-split at the frame's 8-px right edge (b97e71a) — an edge-shape
  DEPTH, which the per-SB lambda's direction predicts.
  identity_full_8bit 1100/1100, spot-check 83/83, nextest 2483/2483,
  video_key_matrix 58/60, fctx 96/96, decode census 96/96, completion scan
  52 OK / 12 REFUSED / 0 CRASH.

- **The DLF VIDEO arm — the port switched deblocking OFF on every inter frame;
  the grid goes 55 BOTH → 67** (`rust/docs/INTER-ENCODE-PLAN.md` §1z²¹,
  a7dd951 + 445d3b1). `pipeline.rs` derived `dlf_level` through the ported
  ladder for both arms and then discarded it for anything but a key frame,
  because `deblock.rs` carried both level pickers specialized to a KEY frame.
  C signals `loop_filter_level[0]` of 8/9/12/16/20/24 where the port signalled
  0, on exactly the 20 cells §1z²⁰ measured as differing FIRST in the frame
  header — a half now closed to zero. New `dlf_arm` carries C's whole
  `svt_av1_pick_filter_level` (both pickers, `me_based_dlf_skip`, the
  reference-average arms and the `prev_dlf_dist < 5` shut-off) and
  `deblock.rs` delegates to it, so there is ONE transcription; `ReferenceFrame`
  gained `lf_levels` and `dlf_dist_dev` with C's -1 "never computed" sentinel.
  Corrects three claims in §1z²⁰, including its ladder prediction —
  `is_not_last_layer` is TRUE on a flat GOP, so both presets were wrong and by
  two different pickers. `inter_byte_gate` 55 → **67**, mutation-verified;
  two new `fhInterFrame` spot-check cells, one per ladder arm.
  identity_full_8bit 1100/1100, nextest 2483/2483.

- **`tools/ctrace-linux/run.sh` never forwarded three interposer env vars**
  (a7dd951). `SVT_IFCOST_OUT`, `SVT_PICKPART0_OUT` and `SVT_REFSTATS_OUT`
  were read by `wrap_recon.c` and listed in neither of the script's forwarding
  loops, so on a macOS host they wrote inside the container and the host read
  the silence as "C never called this" — the same failure the script's own
  comment records for `SVT_RECON_OUT`. All three are forwarded, and a DRIFT
  GUARD now derives the required set from `wrap_recon.c` itself and refuses to
  run on a name in neither list (mutation-verified both ways). It found two of
  the three on its first run against a current `main`.

- **The inter path PANICKED on 18 of 64 video-mode completion cells; it now
  panics on none** (4974a859, 4ae1ffb6). Two distinct defects, both found by
  `tools/inter_completion_scan.sh`, both with the same shape — a comment
  asserting an invariant the code did not implement.
  (1) A **KEY frame** at 480p and up ran `set_pic_pd0_lvl_default`'s
  `lpd0_lvl` 7 = `PD0_LVL_6`, which C's `pd0_detector` demotes on an I_SLICE
  because VERY_LIGHT_PD0 does inter compensation only;
  `part_arm::video_pd0_params` skipped the detector on an I_SLICE and
  `pd0::video_pd0_mode` panicked on the level its own doc comment said could
  not occur. Four cells (568/576/1024/2048 square at p10, and p9 by the same
  ladder row), frame 0 never written — an ordinary still-image configuration.
  All four key frames are now byte-IDENTICAL to C.
  (2) An **INTER frame** whose superblock remainder is 40 px: `Pd0Ctx::pick_q`
  treated C's `tot_shapes == 0` ("no d1 shape to cost here") as "this node must
  SPLIT", where C sets `mds->split_flag` from `sq_size > min_sq_size` alone and
  leaves such a node INVALID. The port descended below `min_sq` — a value only
  an inter frame's `depth_removal_ctrls` raises above 8 — into a node with no
  cost. Fourteen cells at p8/p10/p13. Byte-neutral on every key frame,
  measured against the pre-fix binary.
  Completion frontier 38 OK / 8 REFUSED / 18 CRASH → **52 OK / 12 REFUSED /
  0 CRASH**; partial-superblock cells 19/2/15 → **33/3/0**. Records:
  `rust/benchmarks/inter_completion_2026-09-02{a,b}.tsv` + the `b.meta`, which
  also documents why the first scan of that day (24 OK / 34 CRASH) described a
  binary that predated a landed fix by three minutes and was never main.
  New gates: `regression_spotcheck.sh` grows an `encodesInter` helper (the
  existing `noPanic` drives the PUBLIC API, which refuses inter frames and so
  could never reach this code) and five cells, each proved to fail before and
  pass after; `part_arm::video_pd0_level_tests` pins the PD0 level with a
  positive control on the raw ladder value; and `inter_completion_scan.sh`
  gains a `SCAN_GATE=1` mode wired into CI that fails on any crash, on more
  than `SCAN_MAX_REFUSED` refusals (so a panic cannot be retired by widening a
  refusal) and on a grid that did not run — proved able to fail all three ways
  and to pass.
  EVIDENCE TIER 2 for the first defect, not just byte-identity: C's own
  `SVT_PD0CFG_OUT` dump for `gradient 568x568 q32 p10` (taken on the Linux
  host, where `-Wl,--wrap` works) reports `lvl=5` on ALL 81 superblocks of the
  key frame — `PD0_LVL_6` demoted to `PD0_LVL_5`, the level the port now
  computes, read out of C's live `ModeDecisionContext`.
  CROSS-ISA: aarch64 and x86-64 agree on every gate — completion 52/12/0 on
  both, spot-check 76/76 on both, `inter_byte_gate` 55 required / 0 failed on
  both, plus `identity_full_8bit` 1100/1100 and `video_key_matrix` 58/60.

- **Every `me_*_distortion` was normalised by the PICTURE's area instead of the
  superblock's, so all three `disallow_below_*` decisions were wrong on every
  partial superblock** (this release). C divides by
  `pix_num = b64_geom->width * b64_geom->height`
  (`compute_distortion`, motion_estimation.c:2779) and `b64_geom`'s dims are
  the CROPPED per-superblock extent, `MIN(picture_dim - org, 64)`
  (pcs.c:1507); `inter_me_arm::run_frame_me` built one `MePicParams` per FRAME
  and put `p.width` / `p.height` in that field for every b64. On
  `gradient 168x168` the port divided by 28224 where C divides by 4096, 2560
  or 1600. MEASURED against C's own `SVT_PD0CFG_OUT` on frame 1: C
  36736/35776/32640/23584 against the port's 3332/3244/2960/2139 at the
  (128,0) superblock, and 52326/51640/47933/35553 against 2966/2927/2717/2015
  at the 40x40 corner — ratios of 11.02 and 17.64, which are
  `(4096/pix_num_C)/(4096/28224)` exactly. AFTER, all nine superblocks' `med=`
  AND `dr=` equal C's, and `min_sq` at the three partial ones goes 16 -> 8,
  which is C's.
  `me_8x8_cost_variance` matched C throughout and could not have caught this:
  it is computed from the RAW distortion array before normalisation. That is
  why the defect survived — the checked statistic was the one it cannot move.
  BYTE-INERT on everything measured (inter byte gate 55 required / 0 failed,
  the completion grid's 5 identical cells unchanged, `identity_full_8bit`
  1100/1100, `video_key_matrix` 58/60, and the four 40-remainder cells emit
  identical frame-1 bytes before and after), so per the spot-check's own rule
  it gets no cell there and is gated by
  `inter_me_arm::tests::a_partial_superblocks_distortions_are_normalised_by_its_own_cropped_extent`,
  which pins C's numbers and was proved to fail on the old code.
  Full per-superblock join, and the still-open per-superblock `fast_lambda`
  divergence beside it: `rust/benchmarks/pd0_depth_removal_join_2026-09-02.md`.

- **The C oracle could not encode more than two frames — and the ceiling was
  `capture_c_trace`, not the library** (ab253150). It sent every frame before
  draining any packet, so the finite output-stream buffer pool ran dry on the
  third send; in a `CONFIG_SINGLE_THREAD_KERNEL` build that is fatal
  (`ST mode: empty object pool exhausted after pumping dispatcher`) and wrote
  ZERO packets. The driver now drains one packet after each send when
  `n_frames > 2`, which is safe because ST mode runs the whole pipeline inside
  `svt_av1_enc_send_picture`. MEASURED: `SVT_FRAMES=3` on `gradient 64x64 q32
  p8` codes 1480 / 22 / 21 B and decodes 3/3 frames in both aomdec and dav1d.
  Gated on `n_frames > 2`, so every 1- and 2-frame run — every gate in this
  repo — takes byte-identical code. `docs/INTER-ENCODE-PLAN.md` §1q's note that
  this fix "makes it WORSE" does not reproduce and is corrected in place.
  The PORT still refuses frame 2, for an unrelated and now precisely scoped
  reason: the reference picture's `hp_coded_area` / `skip_coded_area` /
  `intra_coded_area`, which C accumulates per coded block in `update_b`
  (coding_loop.c:1605-1638). Not the DPB, not reference management, not the
  GOP requirement — `generate_rps_info` already produces frame 2's RPS.

- **The open-loop ME searched ONE list where C searches two, with four wrong
  signal fields, and mode decision read an `me_mv_array` slot C never writes**
  (a473fa38). `inter_me_arm::run_frame_me` hard-coded all four HME flags to 1
  (C's level 2 is `sc_class5 && enc_mode <= M2`) and passed the qp-based
  search-area scaling as OFF (C sets it for every preset above `ENC_MR`), so
  the port searched C's UNSCALED ME/HME areas at every preset and qp; and
  `num_of_list_to_search = 1` left out the LIST-1 search, whose ZERO HME
  centre — `set_final_search_centre_sb` skips HME for list 1 at temporal layer
  0 — is where C's `me_64x64_distortion = 0` actually comes from. C's own
  list-0 search does NOT find the match (`p_sb_best_sad` 18816 / 13312).
  Consumers now read the `me_mv_array` slot named by the ME CANDIDATE's own
  direction, as `inject_new_candidates` does. The port's per-b64 ME output is
  now an exact join with C's, and `SVTAV1_PD0DBG`'s `PD0DR` line joins
  `SVT_PD0CFG_OUT` field for field. 96-cell grid unchanged at BOTH 36 /
  F1DIFF 59 / F0DIFF 1; two cells stopped passing for the WRONG reason (HME
  level 2 wrongly enabled had been refining list 0 onto the MV C reaches
  through list 1). Full record: `rust/docs/INTER-ENCODE-PLAN.md` §1z¹³.

- **bd10 DIRECTIONAL intra prediction still crossed tile boundaries after the
  first fix — issue #18 round 2, the half that real photographs actually hit.**
  `intra_edge::dr_predict_hbd` took a `DrGeom` carrying the correct tile and
  derived every availability predicate from the FRAME anyway (`have_top` /
  `have_left` from `g.mi_row > 0`, `right_available` / `bottom_available`
  against `mi_cols` / `mi_rows`), while its u8 twin `dr_predict` scoped all
  four to `g.tile`. Round 1's note that *"the DIRECTIONAL arm was already
  correct — it passed `tile: geom.tile`"* was the error: **passing a tile is
  not using one.** The failing band is presets **0-5**, exactly where the intra
  candidate set still offers directional modes — and round 1's tests pinned
  presets 6 and 9, the two that pass, so four green tests sat over a live bug.
  MEASURED at 256x256 / 2 tile rows / bd10: p0,p2,p3,p4,p5 differ from `aomdec`
  on `gradient` AND `diag` at every qp in {6,12,20,40} (12,480-24,901 of
  98,304); p6..p9 clean; `uniform` clean everywhere — so it is the PRESET axis,
  not content, qp, tile axis or orientation. On the reported cell itself, the
  real 3000x4000 photograph at `AvifEncoder` quality 90 / speed 4 (= qp 6,
  preset 4): **6,468,452 of 18,000,000 samples differ, first at Y r2048** = the
  32-SB tile-row boundary, **0 after**. Forced-by-AREA portrait control
  `gradient 2920x3270` (9.55 MP, 46x52 = 2392 SB, partial SB both axes):
  4,185,160 of 14,322,600, first Y r1664 = 26 SB x 64, **0 after**. The whole
  60-cell {gradient,diag,uniform} x preset {0,2,4,6,9} x qp {6,12,20,40} sweep
  is clean after at 2 tile rows and at 2x2 tiles; **bd8 was clean before and
  after** (its directional path was always tile-scoped). Byte-INERT elsewhere:
  **30 of 32** A/B cells emit identical OBUs — every single-tile cell at both
  depths across presets 0/2/3/4/5/6/9/10/13 including partial-SB and `screen`,
  and every bd8 multi-tile cell. Gates extended: `issue18_repro.rs` grew a
  preset-BAND sweep, a directional forced-tile-column cell, a single-tile band
  control, and a forced-by-area PORTRAIT cell at the reported shape
  (`2920x3270`, ~6.4 s, partial SB on both axes); `regression_spotcheck.sh`
  grew 5 `bd10ReconEq` cells. The stale "a single tile spanning the frame"
  premise in `intra_edge`'s module doc is retracted in place.

- **bd10 intra prediction crossed TILE boundaries, so every forced-multi-tile
  10-bit encode produced wrong pixels (issue #18).** AV1 forces a multi-tile
  grid once a frame exceeds `MAX_TILE_AREA` (4096*2304 = 9,437,184 px of
  SB-aligned area) or `MAX_TILE_WIDTH` (4096 px) — `TileGrid::resolve`, C
  `svt_av1_get_tile_limits` — so an AVIF caller that never requests a tile
  still gets two above ~9.44 MP. Intra prediction is tile-scoped in AV1; the
  u8 path honoured that (`extract_neighbors_tiled`), two bd10 sites did not:
  `predict_unit_hbd`'s non-directional arm called `extract_neighbors_hbd` with
  frame-absolute availability (preset <= 8, the full-RD funnel), and
  `bd10_reencode_{luma,chroma}_node` hardcoded `TileMi::whole_frame`
  (preset >= 9, the level re-encode post-pass). The encoder read real pixels
  across the tile edge while a conforming decoder used the unavailable-edge
  fills, so everything from the boundary onward drifted. Reported as an
  "8-12 MP size cliff" (mean SSIMULACRA2 **-57.05** at 3000x4000 q90 where the
  8-bit control read **86.57**); the size threshold is a proxy for the forced
  tile grid, and the same defect reproduces at **0.27 MP** on a 4160x64 frame.
  MEASURED, encoder final 10-bit recon vs `aomdec`: `gradient 4160x64 q20 p6`
  65,054 of 399,360 samples differ before / 0 after; `gradient 256x256 q20`
  with 2 tile rows 24,169 (p6) and 49,606 (p9/p10/p13) before / 0 after;
  `gradient 2944x3264` (9.61 MP, 2346 SB > the 2304 SB limit) 3,448,059 of
  14,413,824 before / 0 after, while `2944x3200` (9.42 MP, 2300 SB, single
  tile) was and is clean. Byte-INERT outside the broken configuration: 26 of 28
  A/B cells emit identical OBUs, including every single-tile cell at both
  depths and every bd8 multi-tile cell; only bd10 x multi-tile moved. New
  `TileGrid::tile_mi_for_sb` is the single owner of "which tile is this SB in".
  Gates: `svtav1/tests/issue18_repro.rs` (4 cells + a single-tile control) and
  four `bd10ReconEq` cells in `tools/regression_spotcheck.sh`. Scope correction
  recorded in `rust/docs/coverage-combos-map.md` — the 2026-07-22 note that
  threading this was "byte-inert" was true on the C-byte oracle and blind to
  this class.

- **`inter_decode_gate.sh` could not report PASS on macOS.** Its `OPEN_CELLS`
  array emptied when the last open cell was promoted, and `"${arr[@]}"` on an
  EMPTY array under `set -u` is an "unbound variable" error on bash < 4.4 —
  `/bin/bash` on every macOS is 3.2.57. The gate printed five green required
  cells and then aborted nonzero, which reads as a gate failure. Both inter
  gates now use `${ARR[@]+"${ARR[@]}"}`. Recorded in
  `rust/docs/WORKING-ON-THIS.md` §5.

- **An unsigned underflow in C's NSQ shape gate — the last `diag` video-KEY
  cluster.** `product_coding_loop.c:9732` computes
  `MAX(1, nsq_split_cost_th - rate_th_offset_lte16)` in `uint32_t`
  (md_process.h:565/576). `set_nsq_search_ctrls`'s tail rescales the threshold
  by `MAX(10, qp) / 63` below CLI qp 46 and does NOT rescale the offset, so at
  low quantizers the subtraction WRAPS to ~4.29e9 and the gate that reads as
  "skip this shape when its split rate is significant" skips nothing. The port
  had `saturating_sub(..).max(1)` = 1, the opposite extreme. MEASURED on
  `diag 64x64 q20 p6` mi=(8,12): C evaluates and CHOOSES `PART_H` (449905 summed
  against the square's 514776) where the port printed
  `NSQDBG SKIP ... shape=1 gate=1`; reproducing the underflow makes all three
  `diag {64,72,128} q20 p6` key frames byte-identical and leaves q40/q55
  unchanged. Video key frames 4 F0DIFF -> 1. Not reachable on the still
  envelope (`nsq_qp_based_th_scaling` is 0 through M3 on the allintra arm, the
  only band that reaches the tail) — `identity_full_8bit` 1100/1100. Recorded
  as `rust/docs/SUSPECTED-C-BUGS.md` #28, plan §1z⁷.

- **PD0_LVL_5 was unreachable on the pred-depth-only path, and C's
  `pd0_detector` runs on every inter frame.** Two defects that had to be fixed
  together: (A) `pipeline.rs`'s pred-depth-only branch took its PD0 model from
  `part_arm::refined_pd0_model`, which carries levels 3 and 4 and falls back to
  `Pd0Mode::Lvl1` otherwise — so a CLI-qp-20 M8 video key frame, whose
  `set_pic_pd0_lvl_default` row is `3 + ldp0_lvl_offset[qp_band]` = 5, ran
  PD0_LVL_1's block cost against C's PD0_LVL_5, and the port's p8 output was
  byte-identical to its own p6 output where C's differed. (B) `pd0_detector`
  (enc_dec_process.c:2406) gates every test on `slice_type != I_SLICE`, so on a
  KEY frame the picture level IS the SB level, but on an INTER frame whose L0
  reference is a key frame the `use_ref_info` arms walk 5 -> 4 -> 3 without
  reading any ME threshold — every inter frame in this envelope runs PD0_LVL_3.
  New `port_pd0_detector::pd0_ctrls_for_level` (C `set_pd0_ctrls`) plus
  `part_arm::VideoPic` give the already-ported detector its first caller.
  Video key frames 6 F0DIFF -> 4 (`gradient {64,72} q20 p8` byte-identical at
  2044 and 2747 B); nothing regressed. `identity_full_8bit` 1100/1100,
  `regression_spotcheck` 65/65, `video_key_matrix` 58/60, `fctx_gate` 96/96,
  `inter_byte_gate` 31 required PASS. Full record in
  `rust/docs/INTER-ENCODE-PLAN.md` §1z⁶.

- **`fixed_partition` is a TWO-term predicate and the port had one term — 27
  of 96 cells becomes 31.** C: `fixed_partition = pred_depth_only &&
  md_disallow_nsq_search` (enc_dec_process.c:3054), where
  `md_disallow_nsq_search = !nsq_geom_ctrls.enabled || !nsq_search_ctrls.enabled`
  (:7846). `pipeline.rs`'s `refined = dr.adaptive && use_funnel` had only the
  first conjunct, so a picture that is pred-depth-only but still SEARCHES NSQ
  shapes coded squares where C codes an H/V/4-way shape at the same depth. The
  allintra arm never separated the two terms (`get_nsq_search_level_allintra`
  is 0 from M4 up), which is why the still envelope never saw it; the video
  arm's search level saturates to 0 only at CLI qp <= 43, so the whole
  `{gradient,diag} x {64,72,128} q55 p8` F0DIFF cluster was one cause. Video
  key frames 12 F0DIFF -> 6, BOTH 27 -> 31, nothing regressed.
  `identity_full_8bit` 1100/1100, `regression_spotcheck` 65/65,
  `video_key_matrix` 58/60, `fctx_gate` 96/96 all unchanged. The SAME defect is
  still live in the `preset >= 9` PD0 branch, measured and named in
  `rust/docs/INTER-ENCODE-PLAN.md` §1z''''' rather than fixed.

- **The INTER arm was dropped by one of three leaf paths — 19 of 96 cells
  becomes 27.** `pipeline.rs` builds `leaf_funnel::FunnelCtx` at three sites;
  the PD0 FIXED-TREE one hardcoded `inter: None`. That path is taken whenever
  `DrCtrls::for_arm` reports a non-adaptive depth-refinement level, which on
  the VIDEO arm is `pic_block_based_depth_refinement_level == 10`, i.e. **M8
  and above** — so every preset-8 inter frame in the campaign's grid decided
  its blocks from an intra-only candidate set and emitted an intra block with
  a full residual where C emits a 22-byte skip frame. MEASURED with the new
  `NSQDBG PINTER` line: 2 inter candidates offered at p6, ZERO at p8, on
  `gradient 64x64 q40 frames=2`. Byte-inert on the still envelope by
  construction (`inter_md` is `None` on every key frame) and measured so:
  `identity_full_8bit` 1100/1100, `regression_spotcheck` 65/65,
  `video_key_matrix` 58/60, `fctx_gate` 96/96 all unchanged. Full record in
  `rust/docs/INTER-ENCODE-PLAN.md` §1z'''.

- **The frame-CDF shim needed RTCD setup — SIGSEGV on x86-64 only** (4c2e61bb).
  `svt_aom_init_mode_probs` / `svt_av1_default_coef_probs` copy through
  `svt_memcpy`, an RTCD pointer that is NULL until
  `svt_aom_setup_common_rtcd_internal` runs and that NEON devirtualization turns
  into a direct call on aarch64. Two of the four new tier-1 tests crashed on
  x86-64 and passed on aarch64; the two that use only the PAINTED shim modes
  (which call neither initializer) passed on both, which is the fingerprint of a
  NULL RTCD pointer rather than a buffer bug. `entropy_inter_shims.c:107-118`
  had already solved and commented this exact trap.

- **`tools/fh_fields.py` was GUESSING `skipModeAllowed`** and got it wrong on
  the campaign's own first inter cell: C writes no `skip_mode_present` bit
  there, the tool read one, and every field after it was off by one bit with no
  sign in the printout (it reported `allow_warped_motion = 0` where the stream
  says 1). It now implements the real `skip_mode_params()` and threads the
  decoder's `RefOrderHint[]` across the frames of the stream to do it.

### Added

- **`tools/ctrace-linux/vdiff_cell.sh` + `optrace_first_diff.py`** — op-trace
  localization for a VIDEO-mode cell. `diff_cell.sh` is still-only (it cannot
  express the low-delay-P GOP, and it treats the port's expected frame-1 inter
  refusal as a failure), and `identity_diff.py`'s op INDEX is wrong on a
  two-frame run even though its byte verdict is right. The normalizer splits
  both traces on `W RESET` and compares C frame 0 against the port's REAL PACK
  writer — a run creates more writers than it packs frames (measured: C 2
  segments, port 5 on `gradient 72x88 q40 p4`) — and canonicalizes C's
  `BOOL`/`BOOLEQ` against the port's 2-symbol `CDF` writes. Positive control:
  any byte-identical video cell reports "op streams identical". It is what found
  the `TX_SIZE_CDF` write in the Fixed entry below, and it localizes the one
  still-open video-key cell (`gradient 72x88 q40 p9`) to a PARTITION symbol at
  op 3269/10219 — C `PARTITION_NONE`, port `PARTITION_SPLIT`, which
  `tree_diff.py` then resolves to a RIGHT-EDGE shape divergence (C codes
  `BLOCK_8X16` on the 8-wide partial column where the port codes `BLOCK_8X8`
  plus a split; 5 bsize flips, 7 port-only blocks). (edd83bf7)

- `SVT_CCOEF_OUT` now dumps EVERY coded txb when `SVT_CCOEF_XY` is unset (it
  was pinned-block-only, which cannot answer "which block diverges"), and
  `SVT_QLEVELS_OUT` gained a pre-quant `co=[]` field so a levels dump can
  separate "the residual differs" from "the quantizer decision differs". The
  CCOEF dump is gated on `pcs->cdf_ctrl.update_coef`, which the video arm
  clears above M8 — so at preset >= 9 an EMPTY file means the probe did not
  run, not that C coded no coefficients; the comment beside it says so.
  (edd83bf7)

- **THREE byte-identical VIDEO-MODE KEY frames, and the held `wip/video-md-arms`
  bundle landed with them.** `gradient 72x88 q40` at presets 4 and 5 and
  `screenrep 72x88 q40 p7` are now byte-for-byte C's frame 0; before this they
  were 1.996 %, 0.067 % and 0.377 % off, and those three spot-check cells are
  PROMOTED from `ratioVideoKey` to `byteVideoKey`. A fourth, `gradient 72x88
  q40 p9`, reaches C's exact byte COUNT (1589 B) without being byte-identical —
  measured by attempting the promotion and watching it fail, and left as a
  ratio cell with that recorded. What landed is the two
  held MD arms (`mds0_use_hadamard_sb`, `nic_arm`) plus the video PD0 at CLI
  preset >= 9, and four defects the bundle was waiting on — three in PD0, one in
  the CDEF search. Full trail in `docs/INTER-ENCODE-PLAN.md` §1i.

- **The PD0 coefficient rate ignored `mds_subres_step` twice.**
  `svt_aom_txb_estimate_coeff_bits_pd0` doubles the coefficient bits under
  subres (`rd_cost.c:1224`) AND drops its fast-estimate divisor from 2 to 1
  (`:329`), so the whole coefficient scan is priced instead of half. The port
  did neither, under-pricing a sub-sampled PD0 block by up to 3x — measured on
  the reference cell's 64x64 root as C 2355794 bits against the port's 777355,
  with the distortion already agreeing to the unit. This is what made
  `docs/INTER-ENCODE-PLAN.md` §1h's four LVL_3 experiments all look wrong.

- **`ctx->pd0_use_src_samples` is now ported (`pd0::Pd0ReconCanvas`).** C's
  video PD0 predicts each block from the RECON it generates per block, not from
  the source (`enc_mode_config.c:7309`); the port always used the source. With
  it, C's per-block PD0 costs and the port's agree 75/75 on
  `gradient 64x64 q40 p6` and 138/138 on `gradient 72x88 q40 p5` — dist, coeff
  bits, RD cost and the pruned block set. Wired on the LVL_1 family (CLI
  presets 0..=8) only; the preset >= 9 fixed-tree path still predicts from
  source, which is recorded as an open gap, not as parity.

- **FIX: the PD0 depth early-exit ran on OUT-OF-BOUNDS quadrants**, on both
  arms. `test_split_partition_pd0` (`product_coding_loop.c:10456`) `continue`s
  such a quadrant BEFORE the early-exit test; the port tested it anyway, and an
  out-of-bounds child contributes 0 to the running split cost, so the extra
  test at quadrant 3 could turn C's "split wins" into the port's "parent wins".

- **FIX: `cdef_recon_ctrls.zero_fs_cost_bias` was unported on both arms.**
  `finish_cdef_search` scales the zero-filter-strength candidate's mse down by
  `factor/64` before the joint RD search (`enc_cdef.c:986`); the level is
  `enc_mode <= M7 ? 0 : 1` on the allintra arm and `<= M8 ? 0 : <= M10 ? 1 : 2`
  on the video arm. Found by verifying the landing rather than by reading: the
  held bundle broke `video-key-txs-arm-tx-mode-p11`, whose coded tree is EXACT
  (0 field flips, 0 geometry difference) and whose only divergence was
  `cdef_uv_pri_strength[0]` C=0 port=15. Not wired into the bd10 search, whose
  C ladder is a different one.

- **`SVT_PD0CFG_OUT`** (`tools/capture_c_trace/wrap_recon.c`) — a `--wrap`
  interposer on `svt_aom_sig_deriv_enc_dec_pd0` that dumps C's RESOLVED PD0
  configuration (level, subres step, early-exit thresholds, rate-estimation
  level, `pd0_use_src_samples`), and `SVTAV1_PD0DBG`'s new `PD0BLK` line, the
  port-side twin of `SVT_PD0COST_OUT`. The four wrong guesses §1h records were
  only necessary because nothing observed that function.

- **FIX: `mds0_arm` was never called.** The commit that added it shipped the
  module and the prune but lost its `pipeline.rs` call site, so every
  "no cell moved" number in that commit message was vacuous. `cargo build
  --all-targets` did not catch it because the build was grepped for
  `^warning: unused` and a never-called function warns as
  `warning: function ... is never used`. Wired and re-measured, now with a
  POSITIVE CONTROL: the prune abandons 146 candidates across the 16 leaves of
  `diag 64x64 q40 p11` in VIDEO mode and 0 in still mode, and every cell is
  byte-identical — six video key-frame cells, `identity_full_8bit` 1100/1100,
  `regression_spotcheck` 49/49, `cargo nextest run --workspace` 2409/2409.
  Recorded in `docs/INTER-ENCODE-PLAN.md` rather than amended away.

- **CORRECTION: the `pic_pd0_lvl` values in the previous entry were probed at
  the wrong `seq_qp_mod`.** C sets `scs->seq_qp_mod = 2` unconditionally
  (`Globals/enc_handle.c:3994`) and `set_pic_pd0_lvl_default`'s qp-offset term
  is gated on `seq_qp_mod > 1`, so at CLI qp 40 the video arm's level at
  M9..M13 / 240p is **5**, not the 4 a `Case::default()` probe (`seq_qp_mod =
  0`) reports. Any ladder with a `seq_qp_mod` term must be probed at 2.

- **`pcs->mds0_level` wired to `scs->allintra` (`svtav1_encoder::mds0_arm`) —
  faithful, measured byte-inert, and kept anyway.** The video arm assigns
  level 2 above M10 (enc_mode_config.c:9250) where the allintra arm is a
  literal 0 at every preset (`:10042`); level 2 is `pruning_method_th =
  (uint8_t)~0` + `dist_to_cost_th = 0`, which selects `fast_loop_core`'s GLOBAL
  MDS0 prune (product_coding_loop.c:1325) — abandon any candidate whose
  distortion ALONE costs more than the best complete fast cost so far. Tier 1
  on the ladder (`md_config::mds0_level_default`, driven by the exported
  `svt_aom_sig_deriv_mode_decision_config_default` in
  `c_parity_sig_deriv_md_config.rs`); the `set_mds0_controls` table is
  transcribed and pinned against the allintra flattening. The prune FIRES
  heavily (6-9 of ~12 candidates per 16x16 leaf on `diag 64x64 q40 p11`) and
  moves NO cell — six still identity cells and six video key-frame cells
  byte-for-byte unchanged, `regression_spotcheck.sh` 49/49,
  `cargo nextest run --workspace` 2409/2409 — because the port's PD0 partition
  tree at those presets is the ALLINTRA one and dominates the outcome. Kept
  per `rust/CLAUDE.md`'s "dead-looking C stays translated"; no spot-check cell,
  per §3's "a cell earns its place only if it failed before".

- **CORRECTION recorded (`docs/INTER-ENCODE-PLAN.md` §1f): the two held arms on
  `wip/video-md-arms` do NOT break the three blocking cells' geometry.**
  Measured with C's own coded tree (`SVT_CTREE_OUT` via `tools/ctrace-linux/`)
  against `SVTAV1_PACKTREE`: at `gradient 72x88 p9` and `diag 64x64 p11` the
  C-only / port-only block sets are IDENTICAL with and without the arms (12/7
  and 12/6, same mi lists) while the arms cut field flips 47 -> 26 and 18 -> 6.
  The byte regression is the removal of a COMPENSATING mode error against a
  partition tree that is already wrong on `main`. The remaining OPEN arm
  divergence at all three failing presets is `pic_pd0_lvl` (M4 1/3, M9 and M11
  7/4) — `PD0_LVL_3` and `PD0_LVL_4` are unimplemented in `pd0.rs`, and C's
  video PD0 additionally predicts from RECON (`ctx->pd0_use_src_samples =
  allintra || hbd_md`, enc_mode_config.c:7309) where the port always predicts
  from source.

- **The inter campaign's reference cell now matches C's MODE DECISION exactly
  — and the arm that does it is HELD, not landed (`wip/video-md-arms`,
  `59458226`, which supersedes `f898794f9`).** `ctx->mds0_use_hadamard_sb` selects MDS0's luma distortion in
  `fast_loop_core` (product_coding_loop.c:1259) — `hadamard_path` (a SATD,
  `:1283`) vs the two-buffer VARIANCE `fn_ptr->vf` = `svt_aom_variance{W}x{H}`
  (`:1296-1306`). It is a literal, not a ladder:
  `svt_aom_sig_deriv_enc_dec_allintra` writes `true`
  (enc_mode_config.c:8148), `_default` (`:7916`) and `_rtc` (`:8032`) write
  `false`, and `fast_loop_core_light_pd1` (`:1040`) is variance
  unconditionally. The port ran the allintra value on video frames. New
  `svtav1_encoder::encdec_arm` wires it; `FunnelCfg::mds0_use_hadamard_sb`
  defaults to the allintra `true`, so the still path is byte-neutral by
  construction (six reference identity cells IDENTICAL at 290/839/63/171/580/
  693 B). Variance is DC-invariant and SATD is not, so the two metrics rank a
  flat-prediction candidate set completely differently — with `nic_arm`,
  `tools/tree_diff.py` on `gradient 64x64 q40 p6` video frame 0 goes from 12
  field flips and four wrong 32x32 leaf modes to **0 field flips** (947 -> 965
  B against C's 961; the residual is residual coding). HELD because
  `gradient 72x88 p4` (2.00% vs limit 1.0), `p9` (1.20% vs 1.0) and
  `diag 64x64 p11` (16.96% vs 2.0) go outside their `ratioVideoKey` limits,
  while `gradient 72x88 p5` closes to 0.000% and `screenrep 72x88 p7` becomes
  BYTE-IDENTICAL. Full record + where to start on the blocker:
  `rust/docs/INTER-ENCODE-PLAN.md` §1e. (`diag p11` is pure GEOMETRY with 0
  mode flips; and C runs REGULAR PD1 on every key frame — `pic_lpd1_lvl` is
  `is_base ? 0 : …` through M11, `is_islice ? 0 : …` above — so light PD1 is
  not the explanation.)
- **Tier-1 gate on `leaf_funnel::rate_tables::nic_counts`, the funnel's own
  MDS stage-count derivation.** Its only test was six transcribed values;
  `tests/c_parity_md_nics.rs` gates a DIFFERENT transcription
  (`port_md::nics::set_nics`) that has no caller on the live path, over a
  numerator grid that omits `(4,4,4)` and a qp list that omits 40 — i.e. the
  exact configuration the held `nic_arm` work is blocked on sat in a hole in
  both. Now swept against the real exported `svt_aom_set_nics` over every
  `MD_STAGE_NICS_SCAL_NUM` row x every CLI qp 0..=63. The result is a NEGATIVE
  and that is the point: `nic_counts` is correct everywhere, which retires
  "the port's stage-count floor" as the suspect for that blocker.

- **Rate ladders: the video arm of `rdoq_level` + `rate_est_level` +
  `update_cdf_level` wired (lane `wv-rdoq`).** New
  `svtav1_encoder::rate_arm` resolves the three frame-level rate ladders per
  `scs->allintra` from the `ScArm` the frame already carries, replacing the
  inline allintra flattening `pipeline.rs` applied to every frame
  (`preset.min(9)` for the preset clamp, `quant::rdoq_level_allintra` for
  `enc_mode_config.c:9904`, `FunnelCfg::for_preset`'s baked
  `(coeff_rate_est_lvl, real_coeff_ctx)` for `:9917` -> `set_rate_est_ctrls`,
  and `matches!(preset, 0..=6)` as the per-SB CDF-chain gate for `:8534`). The
  arms diverge from M6 up: the video arm is a flat rdoq 1 to M10 (2 above,
  under its own `> M11 -> M11` preset clamp) and a flat `rate_est_level` 1,
  and it keeps CDF adaptation ON at M7/M8 where the still arm switches it off
  entirely. All three are wired together because `set_cdf_controls` couples
  them (`update_coef = rate_est_level || rdoq_level`), so the chunk's brief —
  which named only two — was extended by one. Still path byte-neutral by
  construction: `rate_arm::allintra_flattening_matches_the_ladder` pins each
  allintra arm against the inline expression it replaced, entry-for-entry over
  presets 0..=13 x all four `coeff_lvl`s. Measured: the six still identity
  cells IDENTICAL (290 / 839 / 63 / 171 / 580 / 693 B), `identity_full_8bit`
  1100 / 1100, workspace 2390 / 2390. Video-mode key frame, gradient 72x88 q40
  frame 0: p9 1630 -> 1587 B against C's 1589 (2.580% -> 0.126% off), p10 1630
  -> 1587 (C 1599); p7 1511 -> 1499 (C 1539) and p11..13 1630 -> 1592
  (C 1634) move FURTHER, because only 3 of the ~30 picture-level ladders are on
  the video arm so far. Presets 0..=5 do not move at all — the arms agree
  there. See `rust/docs/rate-arm-port-map.md`. (evidence tier 1 both arms)
- **Tier-1 differential for the ALLINTRA rate ladders.** New
  `ref_sig_deriv_md_config_allintra` shim in `sigderiv_shims.c` drives the
  exported `svt_aom_sig_deriv_mode_decision_config_allintra` and reads back
  `pcs->rdoq_level`, `pcs->rate_est_level` and `pcs->cdf_ctrl`. This upgrades
  `quant::rdoq_level_allintra` from a hand-transcription with unit tests
  (evidence tier 4) to tier 1, and pins `FunnelCfg::for_preset`'s baked
  rate-estimation pair to the real ladder. Mutation-verified (flipping the
  ladder's `<= M5` to `<= M4` fails the new
  `allintra_rdoq_ladder_matches_c` at M5). No `build.rs` change — the shim
  lives in the sigderiv lane's existing TU.
- **`regression_spotcheck.sh`: `video-key-nsq-arm-p7-72x88` REPLACED, not
  re-limited.** Wiring the rate arms made that cell vacuous — the port emits
  1499 B both with the partition arms wired and with them forced back to
  Allintra, so no limit could make it witness its own fix. Per the
  anti-vacuity rule it is replaced by `video-key-nsq-arm-p7-screenrep-72x88`
  (`screenrep 72x88 q40 p7`), which separates 2414 B (1.089% off C's 2388)
  from 2386 B (0.084%), at a TIGHTER limit of 0.5%. The gradient p4 / p5 cells
  still separate and are untouched. New cell
  `video-key-rate-arm-p9-72x88` guards this chunk: 1630 B (2.580% off) before,
  1587 B (0.126%) after, limit 1.0%. Spot-check 45 / 45.
- **Partition search: the video arm of `max_block_size` + NSQ geometry / search
  wired (lane `c2blk`).** New `svtav1_encoder::part_arm` resolves the three
  partition-search ladders per `scs->allintra` from the `ScArm` the frame
  already carries, replacing the inline allintra flattening `pipeline.rs`
  applied to every frame (`preset >= 8 && full_sb` for
  `get_max_block_size_allintra`, `preset <= 6` for
  `svt_aom_get_nsq_geom_level_allintra`, and `NsqCfg::for_preset_qp`'s base
  table for `svt_aom_get_nsq_search_level_allintra`). The arms disagree
  sharply: NSQ search is off from M4 up on the allintra arm and runs to M13 on
  the video arm, NSQ geometry is off above M6 on the allintra arm and never off
  on the video arm, and the video arm never applies the `max_block_size`
  variance cap at all. `NsqCfg` gains `for_arm` / `for_levels`, takes
  `(allow_HV4, min_nsq_block_size)` from `svt_aom_set_nsq_geom_ctrls`
  (`:8180`) instead of a hardcoded `(true, 0)`, and applies the
  `set_nsq_search_ctrls` qp-scaling tail (`:7110-7121`), which is inert on the
  still path and live on the video one. **TIER 1**: nothing is re-transcribed —
  all six ladders are the ones already gated against the exported C symbols by
  `c_parity_sig_deriv_{leaf,common}.rs`, and two new tier-1 tests
  (`nsq_levels_treat_invalid_coeff_lvl_as_normal` plus its positive control)
  pin the `INVALID_LVL == NORMAL_LVL` premise the video-I-slice `coeff_lvl`
  rests on. **Still path byte-neutral by construction and by measurement**:
  `part_arm::tests::allintra_flattening_matches_the_ladder` pins the new
  ladders against the old inline predicates entry-for-entry over presets
  0..=13 x qp 0..=63, `identity_full_8bit.sh` is 1100/1100 and all six
  reference identity cells hold their pinned byte counts. **MEASURED on the
  video-mode key frame**, `gradient 72x88 q40` frame 0: p4 1492 -> 1398 B
  against C's 1403, p5 1499 -> 1484 against 1485, p7 1502 -> 1511 against 1539.
  `regression_spotcheck.sh` 44/44 (41/44 with the change reverted) via a new
  `ratioVideoKey` helper + three cells. Full map, including the five things
  deliberately NOT wired, in `docs/nsq-port-map.md`. (this commit)

- **Screen-content tool derivation: the video arm of the intra-BC ladder
  wired (lane `ibcvid`).** `sc_detect.rs` grows `ScArm` +
  `derive_sc(arm, ...)`; `derive_allintra_sc` delegates to it and is unchanged
  in behaviour. C branches every picture-level tool level on `scs->allintra`
  (`= intra_period_length == 0 || avif || pred_structure == ALL_INTRA`,
  `enc_handle.c:4406`) and the two intra-BC ladders disagree at every preset —
  allintra (`enc_mode_config.c:2346-2369`) is OFF from M5 up, video
  (`:2033-2052`) gives level 5 at M6..M8 — so a video-mode key frame signalled
  the wrong `frm_hdr->allow_intrabc`, which also suppresses the LF/CDEF/LR
  parameter blocks and therefore changed the header's SHAPE, not just one bit.
  **TIER 1**: the video ladder is the one already inside the exported
  `svt_aom_sig_deriv_multi_processes_default`, extracted as
  `port_enc_mode_config::multi_processes::intrabc_level_default` so the wiring
  and `c_parity_sig_deriv_multi_processes.rs` drive the same code. The arm's
  scm gate is wired with it (`enc_handle.c:4638-4670`: allintra auto-detects
  at `<= M7`, video at `<= M8`). MEASURED on
  `identity_diff_inter.sh 64 64 40 <p> 2 screen`, frame 0: the first diverging
  frame-header field was `allow_intrabc` at p6 (C=1 port=0, 92 B vs 143 B) and
  `allow_screen_content_tools` at p8 (114 B vs 697 B); after, EVERY
  frame-header field is identical on both (port 138 B / 691 B) and the
  divergence has moved into the tile payload. New spot-check cells
  `video-key-ibc-arm-p6` / `-p8` (a new `fhVideoKey` helper, frame-header
  fields only, labelled as the weaker assertion) fail before and pass after:
  39/41 → 41/41. Still envelope unmoved and re-measured, not assumed:
  gradient 64x64 q40 p6 290 B, q20 p3 839 B, q55 p0 63 B, 128x128 q55 p8
  171 B, 64x64 q30 p13 580 B, screenrep 64x64 q35 p4 693 B — all identical;
  `identity_full_8bit.sh` 1100/1100; `cargo nextest` 2379/2379. NOT wired: the
  video **palette** ladder (`:2054-2072`), ported at tier 1 and still
  un-called; it cannot move a frame-header bit (unit-proved) but does price
  the RD candidate set at the still palette level.

- **CDEF search signal derivation: `set_cdef_search_controls` + both level
  ladders, and the video arm wired (lane `cdefvideo`, chunk C1a).** New
  `svtav1-encoder/src/port_enc_mode_config/cdef_search.rs`,
  `svtav1-cref/src/cdef_search.rs` + `shims/cdef_shims.c`, and
  `tests/c_parity_cdef_search_ctrls.rs`. **TIER 1**: `set_cdef_search_controls`
  (`enc_mode_config.c:891`) is file-`static` and both `cdef_search_level`
  ladders are inline in their callers, but the exported
  `svt_aom_sig_deriv_multi_processes_{default,allintra}` run all three and
  leave the answer in `pcs->cdef_level` + `pcs->cdef_search_ctrls`, which the
  shim reads back — the level, the nine scalar control fields and all 64
  entries of all four candidate arrays are compared on both arms, over an
  anti-vacuity sweep that reaches every level 0..=10.
  Coverage: 11 of 11 control levels; 2 of the 3 ladder arms (MISSING: `_rtc`,
  `:2255`, the only source of levels 8/9 and outside this envelope).
  `pipeline.rs` now derives the CDEF policy from the ladder that matches
  `scs->allintra` and dispatches on `use_qp_strength`, instead of the
  `is_single_frame && allintra_preset_uses_cdef_search(preset)` predicate that
  dropped a VIDEO-mode key frame onto the qp fast path. On
  `identity_diff_inter.sh 64 64 40 6 2 gradient` frame 0 the signalled
  `cdef_y_pri`/`cdef_y_sec` go from `1`/`0` to C's `0`/`2`; the first diverging
  frame-header field moves to `cdef_uv_pri_strength[0]` (C 7, port 0).
  Video-mode key frames on flat content become byte-identical end to end
  (`uniform 64x64 q40` frames=2 frame 0, presets 0/3/6/8 = 28/28/28/30 B).
  No still regression: identity_full_8bit 1100/1100, regression_spotcheck
  39/39, workspace tests 2373/2373, and the six pinned still cells at their
  expected sizes. `regression_spotcheck.sh` gains its FIRST video-mode cells
  (`byteVideoKey`, four presets) — they DIFFER at identical byte counts before
  the fix, which is precisely the shape a size-based gate cannot see.

- **Rate control: the `rc_process.c` group is ported, mostly tier 1 (lane
  `wp-ratecontrol`).** New `svtav1-encoder/src/port_rc_process.rs` +
  `port_rc_lambda_tables.rs`, `port_rc_vbr_cbr.rs` + `port_rc_vbr_tables.rs`,
  `port_rc_rtc_cbr.rs`, `port_pass2_strategy.rs`, and
  `svtav1-cref/src/rate_control.rs` + `shims/rc_shims.c`.
  **TIER 1** against the real exported symbols: `svt_av1_rc_bits_per_mb`,
  `svt_av1_compute_qdelta_by_rate` (the inter unblocker — `rc_crf_cqp.c:170-178`
  calls it on every non-intra frame and its delta moves `base_q_idx`),
  `svt_aom_compute_rd_mult_based_on_qindex` over all seven update types (the
  port previously hardcoded only the KF arm at five sites in `pd0.rs`),
  `svt_aom_compute_rd_mult`, `svt_aom_compute_fast_lambda`,
  `svt_aom_lambda_assign`, `svt_aom_set_rc_param`, `svt_av1_rc_init`,
  `svt_av1_new_framerate`, `svt_av1_get_cqp_kf_boost_from_r0`,
  `svt_av1_get_gfu_boost_from_r0_lap`, `svt_av1_calculate_boost_bits`, the
  seven `rc_process.c` const tables (exported data symbols), the three
  `av1_lambda_mode_decision*_bit_sad` tables and the eighteen `rc_tables.h`
  minq tables (5,376 entries, all read out of the real C arrays through
  shims). **TIER 4** (`static` in C, no exported symbol, hand-derived vectors):
  the three ref-frame percentage helpers, `rc_init_frame_stats`, `get_ref_obj`,
  `update_rc_counts`, `clamp_qp`/`clamp_qindex`, `generate_sb_qindex`'s control
  flow, and the `rc_vbr_cbr.c` / `rc_rtc_cbr.c` / `pass2_strategy.c` scalar
  cores. Two rows the inventory reported as ported were doc-comment substring
  hits with no implementation — `svt_av1_rc_init` and `generate_sb_qindex` —
  and both now exist. (cb6fa82, 1dc29e4, 1920b89, 7f9cfac, 62167c8, a9db88f,
  7434410, 0ed920f)


- **`transforms.c`'s reduced-coefficient-shape family is ported, tier 1 —
  76 of 76 `_N2` / `_N4` / `ONLY_DC` functions plus the entry points above
  them.** New `svtav1-dsp/src/fwd_txfm_pf.rs`: the 26 pruned 1-D kernels
  (`fdct{4,8,16,32,64}`, `fadst{4,8,16}`, `fidentity{4,8,16,32,64}` in both
  shapes), `fwd_txfm_type_to_func_N2/_N4`, one 2-D core covering
  `av1_tranform_two_d_core_{N2,N4}_c` **and** `av1_tranform_two_d_core_c`
  (`div == 1` reduces to it exactly), all 57 exported 2-D entries, the 54
  `highbd_fwd_txfm_WxH{,_n2,_n4}` wrappers as one table,
  `svt_av1_highbd_fwd_txfm{,_n2,_n4}`, `svt_av1_wht_fwd_txfm` (TPL's only
  transform entry), the ten `svt_handle_transform*{,_N2_N4}_c`,
  `svt_aom_estimate_transform` + its four static shape dispatchers,
  `svt_aom_transform_config`, `svt_av1_gen_fwd_stage_range`,
  `set_fwd_txfm_non_scale_range` and `svt_av1_get_inv_txfm_cfg`.
  Evidence tier 1 throughout (42 tests in `c_parity_txfm_pf{,_2d,_entry}.rs`
  and `c_parity_estimate_transform.rs`, new shims in
  `svtav1-cref/shims/txfm_pf_shims.c`); workspace 1418/1418. Byte-inert on
  the existing envelope — nothing calls the new module yet; it is the
  transform surface TPL needs for `ppcs->r0`, which the video-mode CRF
  qindex derivation (campaign chunk C1a) is gated on.
  (352cfa0f, 1f085b65, 348ab209, 103ab793)
- Two upstream defects recorded in `rust/docs/SUSPECTED-C-BUGS.md` #12 and
  #13, both found while gating the above: `highbd_fwd_txfm_4x16_n2/_n4` call
  the UNPRUNED 4x16 transform (alone among 18 siblings), and
  `svt_av1_fwd_txfm2d_*_neon` NULL-derefs at bd > 8 for any ADST-containing
  tx_type on a 32-dimension block.

- **`svt_aom_generate_av1_mvp_table`'s ref loop is now gated too — chunk C2,
  evidence TIER 1.** The per-ref sweep could not reach it: C keeps ONE
  `Mv mv_ref0[64]` across the whole `ref_frames` loop (adaptive_mv_pred.c:1336)
  and the `symteric_refs` shortcut in `add_tpl_ref_mv` depends on that sharing
  — the `LAST_FRAME` pass stores a projected MV in slot *i* and the
  `BWDREF_FRAME` / `LAST_BWD_FRAME` passes read it back. The C shim now takes
  the scratch IN and OUT, so `c_parity_generate_av1_mvp_table_threads_mv_ref0`
  drives the oracle three times threading it exactly as C's loop does. Teeth
  verified: dropping the threading in the port fails the cell
  (`stack[0]` 0 against C's 0x1e6_4bb2 at `rf=BWDREF`), and the cell
  additionally re-runs each ref with a FRESH scratch and requires the answers
  to differ in more than 10 cells — so it cannot pass vacuously against a port
  that restarts the scratch per ref.
- **`svt_aom_mode_context_analyzer` and the OBMC overlappable-neighbour counts
  — chunk C2, evidence TIER 1.** `mode_context_analyzer`
  (inter_prediction.c:2565) collapses `setup_ref_mv_list`'s packed mode context
  into the single compound context through `svt_aom_compound_mode_ctx_map`;
  `count_overlappable_neighbors` (adaptive_mv_pred.c:1893) plus its two static
  helpers `count_overlappable_nb_{above,left}` (:1830, :1864) produce
  `blk_ptr->overlappable_neighbors`, the OBMC gate. Both are gated against the
  exported C symbols in `tests/c_parity_inter_mvp.rs` (now 8 tests): the
  analyzer over every context `setup_ref_mv_list` can emit crossed with single
  and both kinds of compound pair, and the neighbour count over randomized
  grids with a high 4xN population — which is what drives the `mi_step == 1`
  arm that rewinds the LOOP VARIABLE before reading the cell to its right.

- **OBMC motion search — the other half of `av1me.c` (`inter_me/obmc_search.rs`)
  — chunk C4, evidence TIER 1.** `av1me.c`'s IntraBC half was already in
  `intrabc.rs`; this completes the file: `get_obmc_mvpred_var`,
  `obmc_refining_search_sad`, `svt_av1_obmc_full_pixel_search`,
  `set_subpel_mv_search_range`, `setup_obmc_center_error`,
  `upsampled_obmc_pref_error`, `upsampled_setup_obmc_center_error`,
  `sp`/`pre`/`search_step_table` and
  `svt_av1_find_best_obmc_sub_pixel_tree_up`, plus the four C_DEFAULT kernels it
  drives that nothing in this port needed yet (`obmc_sad`, `obmc_variance`,
  `obmc_sub_pixel_variance` with both bilinear passes, `svt_aom_upsampled_pred`
  and `svt_aom_convolve8_{horiz,vert}`). Nothing calls it yet. Gated by
  `tests/c_parity_obmc_search.rs` (9 tests: the kernel families over 10 block
  sizes x 64 sub-pel offsets, `convolve8` both directions, `upsampled_pred` over
  all offsets x {2,4,8}-tap, and BOTH search drivers against the real C with an
  `IntraBcContext` + `ModeDecisionContext` assembled in the shim).

- **Recorded upstream defect: every NEON `obmc_sub_pixel_variance` above 4x8 is
  the 4x8 kernel** (`docs/SUSPECTED-C-BUGS.md` #11). `aom_dsp_rtcd.c:731-750`
  aliases all 20 sizes from 4x16 to 128x128 to
  `svt_aom_obmc_sub_pixel_variance4x8_neon`; measured on macOS aarch64 for
  `BLOCK_8X16` across all 64 offsets, the RTCD result is bit-identical to the
  `_c` 4x8 kernel. `obmc_sad`/`obmc_variance` in the same block and the x86 SSE4
  table are correct. The port follows the C SOURCE; the test suite compares the
  live `USE_8_TAPS` path against the C binary everywhere and the `osvf` path only
  where a control test proves this host's dispatch is faithful — that control
  fails the day upstream fixes the table (fb5f8fa).

- **Open-loop motion estimation — a wholesale port of `motion_estimation.c`
  (`inter_me/`) — inter-encode campaign chunk C4, evidence TIER 1 where a C
  symbol exists.** All 40 functions of SVT-AV1's 2,964-line
  `Source/Lib/Codec/motion_estimation.c`, in a new module tree: the seven SAD
  accumulators plus the two `compute_sad_c.c` loop kernels (`sad.rs`),
  `MeContext` and the padded-plane view (`context.rs`), pre-HME + HME levels
  0/1/2 + the search-area derivation + `check_00_center` +
  `set_final_search_centre_sb` + the two reference-pruning ladders (`hme.rs`),
  the one- and eight-point search-point blocks + `integer_search_b64` +
  `me_prune_ref` (`integer.rs`), the three ME candidate-array constructors +
  global-motion detection + `compute_distortion` (`candidates.rs`), and
  `init_me_hme_data` / `me_static_b64_bypass` / `svt_aom_motion_estimation_b64`
  (`b64.rs`). Deliberately NOT ported: `get_me_reference`'s `SVT_WARN` log line
  (its `*dist` output IS ported) and the `tf_*` half of `MeContext` that belongs
  to `temporal_filtering.c` — the five `tf_` fields `motion_estimation.c` reads
  are carried. **Nothing calls it yet**: `motion_est.rs`'s homegrown searcher is
  still what `partition.rs` and `pipeline.rs` use, and moving those call sites
  is a separate chunk. Gated by `tests/c_parity_inter_me.rs` (11 tests, tier 1
  against the real `libSvtAv1Enc.a` via the new `shims/inter_me_shims.c`:
  `svt_aom_compute8x4_sad_kernel_c`, `svt_nxm_sad_kernel_helper_c`,
  `svt_sad_loop_kernel_c`, the four `svt_ext_*sad_calculation*_c` accumulators,
  `svt_aom_get_scaled_picture_distance` exhaustively over all 65,536 inputs,
  `hme_level_2` and `check_00_center`) and `tests/inter_me_traced.rs` (18 tests:
  `hme_level_0`/`hme_level_1`/`prehme_core` against the REAL C `hme_level_2` in
  the domain where the C bodies coincide, an eight-point-vs-eight-singles
  structural invariant, a pure-translation recovery test through
  `motion_estimation_b64`, and hand-traced vectors for the remaining `static`
  bookkeeping — labelled tier 4 in the file). MEASURED finding recorded at the
  function: pre-HME does not round its search width up to a multiple of 8 and
  does not apply the `& ~7` round-down after the right-edge crop, so it searches
  a different column count than the HME levels near a right edge (1194e4b,
  8224df7, 9f41610).

- **Inter-frame MVP (motion-vector-predictor) stack (`inter_mvp.rs`) —
  inter-encode campaign chunk C2, evidence TIER 1.** The general
  (`ref_frame > INTRA_FRAME`) branch of `adaptive_mv_pred.c` that
  `intrabc_mvp.rs` could not reach: `add_ref_mv_candidate`'s compound arm and
  its `is_global_mv_block` substitution, the temporal (MFMV) candidates
  (`add_tpl_ref_mv` + `get_mv_projection` + `lower_mv_precision`, both the
  single and compound projections and the `symteric_refs` LAST/BWD shortcut
  with its `mv_ref0[64]` scratch threaded across the ref loop as C does),
  `scan_row_col_light`'s compound arm and the `ref_frame_sign_bias` flips in
  both arms, `setup_ref_mv_list`'s MFMV block including the `sb64_sq_no4xn_geom`
  walk and the 3-position extension, `svt_aom_gm_get_motion_vector_enc`,
  `svt_aom_generate_av1_mvp_table`'s inter `gm_mv` derivation,
  `svt_aom_get_av1_mv_pred_drl`, `svt_aom_compute_inter_mode_ctx_light`, the
  compound-aware `svt_av1_get_ref_mv_from_stack` /
  `svt_av1_find_best_ref_mvs_from_stack`, and `av1_set_ref_frame` /
  `av1_ref_frame_type` / `get_list_idx` / `get_ref_frame_idx`.
  `tests/c_parity_inter_mvp.rs` drives the REAL exported C symbols through new
  shims (`crates/svtav1-cref/shims/inter_mvp_shims.c`, its own translation unit
  so concurrent lanes never share a shim file) over randomized inter mode-info
  grids: 4,000+ cases across 14 ref-frame types (7 single + 7 compound), 11
  block sizes, two tiles, both SB sizes, MFMV on/off, high-precision MVs on/off
  and four global-motion model classes, comparing the full 8-slot stack
  (`this_mv`, `comp_mv`, weight), the count, the mode context, nearest/near and
  the `mv_ref0` scratch. The MFMV anti-vacuity check re-runs the C oracle with
  the block disabled and requires the temporal candidates to CHANGE the output,
  not merely to execute.
- **Motion-field projection (`get_block_position`, `motion_field_projection`,
  `setup_motion_field` in `inter_mvp.rs`) — chunk C2, evidence TIER 4.** All
  three are `static` in `md_config_process.c` and export no symbol (verified
  with `nm -gU Bin/Release/libSvtAv1Enc.a`), so
  `tests/inter_mvp_motion_field.rs` pins them with hand-derived vectors traced
  against the C source, each with its arithmetic written out beside it. Covers
  the `is_lst_overlay` suppression, the KEY/INTRA_ONLY and resolution-mismatch
  refusals, `ref_frame_side` ahead/coincident/behind, and the
  `use_ref_frame_mvs == 0` early return that leaves `tpl_mvs` untouched.

- **Inter MV entropy coding + MV rate (`inter_mv_code.rs`) — inter-encode
  campaign chunk C3, evidence TIER 1.** The layers between the already-gated
  MV symbol writer (`entropy/mv_coding.rs`) and the already-gated cost-table
  build chain (`intrabc.rs`): the `force_integer_mv` precision override
  `svt_av1_encode_mv` performs internally (entropy_coding.c:1498-1500), the
  per-inter-mode dispatch deciding WHICH of a block's MVs are coded
  (:5216-5244) and priced (rd_cost.c:1088-1128), the full
  `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-488 — the
  `approx_inter_rate` zero-fill early return, the hp/non-hp stack selection,
  the `allow_intrabc` dv arm), the CDF adaptation `av1_update_mv_stats` /
  `update_mv_component_stats` (:650-705), `reset_nmv_counter`
  (cabac_context_model.c:1956), `avg_nmv` (enc_dec_process.c:2567), the
  `update_mv` cadence (`set_cdf_controls`, enc_mode_config.c:8468-8498) and
  `copy_mv_rate` + the per-SB rebuild-vs-copy choice its two call sites make
  (enc_dec_process.c:36-56, :2802-2806, :2908-2912), and `svt_aom_mv_err_cost`
  / `_light` over the NMV tables (av1me.c:141/:126 — the arm the inter sub-pel
  search reads through `x->mv_cost_stack`, which `c_parity_intrabc.rs` covers
  only at `MvSubpelPrecision::None` over the DV tables). `FrameContext` gains
  the `nmvc` field C's `FRAME_CONTEXT` has beside `ndvc` (seeded from the same
  `default_nmv_context`, cabac_context_model.c:794-795) and `avg_cdf_with` now
  averages BOTH through the ported `avg_nmv`, as C's `avg_cdf_symbols` does
  (enc_dec_process.c:2638-2639), replacing an inline re-enumeration of `ndvc`'s
  fields. Byte-neutral: nothing reads `nmvc` yet (the inter refusal still
  stands), and averaging two equal contexts is the identity — pinned by
  `avg_nmv_matches_the_previous_inline_ndvc_enumeration` (replays the old
  inline code verbatim), `avg_cdf_with_actually_averages_nmvc` (anti-vacuity:
  fails if the new call is dropped) and `nmvc_defaults_and_is_inert_under_avg`, and
  MEASURED at the bitstream level in
  `benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md` — 32 / 32 cells
  identical with the new averaging present vs removed, with `avg_cdf_with`
  proven REACHED (an `eprintln` probe fires 2x/frame at presets 0/4/6 on a
  3x3-SB frame, 0x at preset 8) and proven able to MOVE bytes (halving
  `partition_cdf` in the same place changes 12 / 12 cells). That record also
  documents the trap the run hit: a first, weaker control changed no byte and
  read exactly like "never called".
  The module docs also record the nine-step emission order around the MV write
  (entropy_coding.c:5196-5300), which of those steps have no port, and the two
  traps in it — the DRL predicate being a different mode set from the MV
  predicate, and the MV write reading the already-`lower_mv_precision`-rounded
  `predmv[ref]` rather than a raw ref-MV-stack entry.
  Gate: `crates/svtav1-encoder/tests/c_parity_mv_code.rs`, 17 tests driving
  the REAL exported symbols `svt_av1_encode_mv`, `svt_av1_get_mv_joint`,
  `svt_aom_estimate_mv_rate`, `svt_av1_mv_bit_cost{,_light}`,
  `svt_aom_have_newmv_in_inter_mode`, `svt_av1_reset_cdf_symbol_counters` and
  `svt_aom_get_update_cdf_level_{default,rtc,allintra}` through three new
  `svtav1-cref` shims. Unlike the pre-existing `c_parity_mv.rs` (a C-side
  transcription, default context, `ref_mv == 0`, bytes only) this drives the
  real writer from RANDOMIZED `NmvContext`s with NONZERO reference MVs across
  every (`allow_high_precision_mv`, `force_integer_mv`, `allow_update_cdf`)
  combination and compares the ADAPTED CDF STATE as well as the bytes.
  Teeth proved by six mutations (precision override, hp-bit stats update,
  rate-table precision, the mode→ref plan, one `reset_nmv_counter` field, one
  `avg_nmv` field) — each caught, naming the diverging context field.
  Records a C asymmetry deliberately reproduced: under `force_integer_mv` the
  WRITER codes at `MV_SUBPEL_NONE` while the RATE tables are still built at
  `MV_SUBPEL_LOW_PRECISION`, because `svt_aom_estimate_mv_rate` passes
  `allow_high_precision_mv` straight in and never consults `force_integer_mv`
  (pinned by `c_parity_rate_tables_ignore_force_integer_mv`). NOT WIRED: the
  public entry point still refuses inter frames at `pipeline.rs`'s `if !is_key`
  guard, and `FrameContext` still carries no `nmvc` field — both belong to the
  chunks that own those files.

- **`AvifEncoder::encode_yuv420` emits a REAL AV1 bitstream — issue #9 item 6.**
  It returned three concatenated MONOCHROME streams behind u32 length prefixes,
  as `Ok(...)`, which no decoder accepts. It now routes through
  `EncodePipeline::with_chroma_420(true)` + `try_encode_frame_420` — the same
  4:2:0 path every C-oracle gate covers — and is asserted BYTE-IDENTICAL to
  driving that pipeline directly with the same config
  (`encode_yuv420_is_the_mainline_420_path_byte_for_byte`). It also no longer
  pre-pads: the pipeline signals the TRUE frame size and pads internally, so a
  98x66 image is a 98x66 stream. **Gate: `tools/decode_conformance.sh <dir>
  avif` — a new corpus driven entirely through `AvifEncoder`'s public entry
  points, 240/240 streams decode under BOTH aomdec and dav1d** (120 4:2:0 +
  120 monochrome, sizes {32,48,64,66,98,128} x qualities {10,35,60,85} x
  speeds {1,5,6,8,10}).
- **`AvifEncoder::with_lossless(true)` is now honoured on 4:2:0** — it sets
  QP 0, the coded-lossless path issue #5 landed byte-identically to C. On the
  monochrome path it stays a typed `UnsupportedConfig` (the mono leaf coder has
  no lossless arm). Same for `quality > 99.2`, which maps to QP 0. This is the
  first CAPABILITY refusal this port has ever RETIRED: the inventory goes 15 ->
  14 capability refusals.

- **Fractional CRF — issue #9 item 4.** `RcConfig` gains
  `extended_crf_qindex_offset: u8` (quarter-qindex steps) and the
  `RcConfig::crf(f32)` constructor that splits a fractional `--crf` exactly as
  C's `str_to_crf` does (enc_settings.c:1655-1670): `--crf 35.25` is
  `qp = 35, offset = 1`. The offset is consumed where C consumes it —
  `scs_qindex = clamp_qindex(quantizer_to_qindex[qp] +
  extended_crf_qindex_offset)` (rc_crf_cqp.c:471) — and the extended
  63.25..70 range's frame `lambda_weight` bump (`+= offset * 28`,
  enc_mode_config.c:10109-10114) is applied too. **The port now keeps C's TWO
  qp values apart:** `static_config.qp` (the CLI value, unchanged by the
  offset) still keys every level derivation, while `ppcs->picture_qp =
  (base_q_idx + 2) >> 2` (rc_process.c:861) keys only the frame
  `lambda_weight` ladder. Collapsing both onto the qindex-derived value
  diverged from C at preset 2 / qp 20 / offsets 2-3 — measured, then fixed.
  Offset 0 makes the two equal, so every pre-existing cell is unchanged.
  **Gate: `tools/issue9_knobs_gate.sh`, fractional-CRF cells 19/19
  byte-identical to the C oracle** (presets 2/6/10 x qp 20/40 x offsets 1-3,
  plus the qp-63 extended-range cell), with an anti-vacuity check that fails
  if a knob never moves the C oracle's own bytes.
- **`max_tx_size` (32|64) — issue #9 item 3, now byte-gated.** Already
  threaded through the PD0 scan and the depth refinement
  (enc_dec_process.c:1494-1500 / :1815); `tools/issue9_knobs_gate.sh` adds the
  C-oracle cells that prove it: **9/9 byte-identical** at
  `max_tx_size = 32` over presets 2/6/10 x qp 20/40/55.
- **`chroma_sample_position` — issue #9 item 5.**
  `EncodePipeline::with_chroma_sample_position(0|1|2)` writes the two 4:2:0
  `color_config` bits C writes from `static_config.chroma_sample_position`
  (entropy_coding.c:2743); 3 is reserved and refused at encode time, matching
  `verify_settings` (enc_settings.c:762). Default 0 (CSP_UNKNOWN) keeps every
  pre-existing stream bit-identical. **Gate: 2/2 byte-identical** cells
  (vertical + colocated).
- **`EncodePipeline::knob_config_error`** refuses the three configurations C
  rejects in `svt_av1_verify_settings` rather than encoding them:
  `max_tx_size` outside {32, 64}, an `extended_crf_qindex_offset` above 3
  (or above 28 at qp 63), and `chroma_sample_position > 2`.

- **Coded-lossless (QP 0) ENCODES — issue #5 chunk 2, the tile half.** The
  refusal is gone on the 8-bit 4:2:0 still path (mainline mode, no
  screen-content tools, no superres); every arm outside that envelope keeps
  a typed `UnsupportedConfig` (`EncodePipeline::lossless_config_error`,
  ledgered in `docs/REFUSED-CONFIGS.md`). What landed, each cited to C:
  the forced 8x8 / TX_4X4 partition tree (`pd0::lossless_tree` —
  `max_sq_size` 8 under `mimic_only_tx_4x4`, enc_dec_process.c:1492), the
  4x4 Walsh-Hadamard forward + inverse in `tx_unit` with C's transposed
  coefficient store (transforms.c:3950, inv_transforms.c:3141 — the u16
  scratch + `highbd_iwht4x4_16_add` always, never the eob<=1 shortcut),
  depth 1 forced at EVERY MD stage incl. a per-txb-predicted MDS1 loop
  (product_coding_loop.c:6734 inside `full_loop_core`), RDOQ and the
  tx-type search off (full_loop.c:1756, :7065/:7173), the DCT-chroma-only
  candidate filter on the regular / filter-intra / palette injection lists
  and both uv searches (mode_decision.c:3245/3298/3393,
  product_coding_loop.c:7376/7584 — which collapses the intra set to
  {DC, PAETH} at qp 0), zero tx_size bits priced (rd_cost.c:1755) and no
  tx_size symbol coded (entropy_coding.c:4657), RDOQ level 0, and
  deblock / CDEF / LR neither searched nor applied
  (md_config_process.c:1022-1035). `FunnelCfg::apply_coded_lossless` is C's
  `txs_level = 1` override. **Gate: `tools/lossless_gate.sh`** (in CI on the
  72-cell 64x64 + 96x80 subset): byte-identity to C AND `aomdec --rawvideo`
  output == the source planes, per cell. Local arm64 run 2026-08-28
  (`benchmarks/lossless_gate_2026-08-28.md`): **112 / 144 byte-identical +
  32 pinned, 144 / 144 lossless** — presets 4..13 are 96/96 across
  {gradient, diag, uniform} x {64x64, 128x128, 96x80, 200x136}; presets
  0..3 on textured content are pinned self-promotingly (lossless in both
  encoders, e.g. gradient 64x64 p3 port 2966 B vs C 2973 B; root by
  elimination — the port's p3 == its p4 == C's p4, and the only M3-boundary
  knob live at qp 0 is `svt_aom_get_disallow_4x4_allintra`: all-intra
  allows 4x4 partitions at <= M3, so C's lossless partition search decides
  8x8-vs-four-4x4 per block where the port forces 8x8 leaves; real CID22
  crops at 64x64/512x512 are 8/8 byte-identical at p7/p12 and lossless at
  p1). Neutral at qp >= 1 by
  construction and by measurement: identity_matrix 54/54, bd10_matrix 36/36,
  regression_spotcheck 33/33, workspace 1051/1051. In-crate witnesses:
  `tests/lossless_fh_c_capture.rs::qp0_coded_lossless_stream_matches_c_capture`
  (full-stream equality to the committed C capture; MUTATION-VERIFIED —
  DCT instead of WHT: 3759 B vs 2699 B; tx_size symbol coded: 2702 B) and
  `pipeline::tests::qp0_420_encodes_losslessly_and_out_of_envelope_arms_refuse`
  (recon == source at qp 0, lossy at qp 1, 10-bit and fork refused).
  `AvifEncoder`'s three-monochrome-stream surface still refuses lossless
  (the mono leaf coder has no WHT arm); its messages now say where QP 0 is
  available. Also fixed while gating: rustc 1.98 clippy on `restoration.rs`
  (`as_chunks` in the three NEON/AVX2 stats kernels, byte-neutral),
  `picture.rs`, and the `zensim_census` example.
- **Issue #8 doc-debt residuals closed, and `rust/Cargo.lock` is now
  committed.** The lock decision the audit asked for: the product is a
  byte-identical bitstream and `archmage` is a semver dependency, so an
  unpinned fresh-box resolve could change codegen under the gates; the lock
  pins what CI measured (`rust/README.md` "Building"; `cargo update` is its own
  commit). Both READMEs' gate tables are re-tallied from CI run 33101031800
  (`1ed7db46`) and split into CI-run vs corpus-gated-local blocks with each
  local number dated and its committed artifact named (or "no committed
  artifact" said outright — the 177/180 `real_image_matrix` figure was one;
  the committed real-corpus record is the 450-cell
  `identity_full_8bit_real_2026-08-03.tsv`, 403 IDENTICAL, p6/p10/p13
  90/90). `rust/README.md`'s "197/309 non-flat" was an arm64 measurement
  (309/309 on x86 CI); "Rust 1.85+" is 1.89 (the real `rust-version`); test
  counts are 1056 as of `1ed7db46`. Unbacked MEASURED numbers are now
  labelled as such at the citation (CLAUDE.md kernel throughput,
  perf-status.md's never-committed `perf_{before,after}_cdef.tsv`,
  HDR-ON-4.2.md 48/48, ACCEPTANCE-CRITERIA 0/36, bd10-port-map's 540-cell
  `/tmp` sweep, ibc-port-map's 25,356 blocks). Docs that described landed
  work as open carry a dated STATUS banner verified against source:
  finishing-survey D7 + ibc-port-map §B (IntraBC is wired, `allow_intrabc`
  derived, dsp placeholder deleted), C-TEST-PORTING-AUDIT 1h (superres
  ported + CI-gated; `scale.rs` still the pinned stub), STATUS.md
  "Architecture direction", practical-usage-plan, sc-detection-port-map,
  arbitrary-dims-port-map, IDENTITY-STATUS, `specs/README.md` (pinned
  pre-v4.2.0; the C tree wins). Per-gate wall-clock budgets exist for the
  first time: `rust/benchmarks/gate_wallclock_ci_2026-08-27.md` (every CI
  step's duration from the same run; the job is ~21 min, the three largest
  steps 207/167/141 s), linked from WORKING-ON-THIS.md §2b.
- **CI caches the cargo-built C oracle, keyed on the submodule SHA — issue
  #4 invariant C's last open piece.** `.github/workflows/rust-gates.yml`
  restores `Bin/Release` + `Bin/ReleaseHdr` (lib, `SvtAv1EncApp`, the
  `.zenav1-cref-stamp`) from `actions/cache` under a key of `<submodule HEAD>
  + hash(build.rs)`; on a hit `cargo build -p zenav1-svt-cref` is a stamp
  no-op. Only the output dirs are cached, on purpose: a restored ninja tree
  would see the fresh checkout as newer than every object and rebuild inside
  the shell tools' silent `cmake --build` freshness check. Measured cost being
  removed: 141 s per run (run 33101031800). `actions/checkout` v5 -> v7 and
  `actions/cache` v4 -> v6 (the v4 entry was on the Node 20 deprecation
  path).
- **`coverage_combos_gate.sh` runs in CI (issue #8 item 7) with a
  caller-selected axis set.** New `CC_AXES` env (default `sb128 bd10 real`);
  CI passes `sb128 bd10` because axis 3 needs the CID22 / gb82-sc corpora, so
  the skip lives in the workflow, not in a file-exists check; a run that
  selects no axis exits 2 rather than reporting 0/0. Also `wc -c` replaces
  `stat -c%s` (BSD stat has no `-c`; the byte columns were empty on macOS).
  Local arm64 measurement of axes 1+2 before wiring:
  `benchmarks/coverage_combos_2026-08-28_arm64_axes12.{tsv,meta}` — 26/28,
  SB128 x tiles 16/16 byte-exact, and TWO bd10 `diag 256x256` eff-M9 cells
  whose single-tile CONTROL diverges on this host; the x86 CI run is the
  arbiter (bd10 non-flat is ISA-dependent on the C side here, STATUS.md
  "Measurement caveat for arm64 hosts").
- **Coded-lossless frame header, byte-identical to C — issue #5, chunk 1 of
  the lossless envelope.** `key_frame_header_bits_lr` now derives
  `CodedLossless` / `AllLossless` (spec 5.9.2: `base_q_idx == 0` with zero
  chroma deltas, segmentation off; AllLossless additionally unscaled) and, like
  C `write_uncompressed_header_obu` (entropy_coding.c:3594-3612), writes no
  `loop_filter_params()`, `cdef_params()`, `lr_params()` or `tx_mode_select`
  bits when it holds (`delta_q_present` was already gated on `base_q_idx > 0`).
  Witness `crates/svtav1-encoder/tests/lossless_fh_c_capture.rs` against a
  committed C capture (`tests/data/c_gradient64_p7_qp{0,1}.obu`, 64x64
  gradient, preset 7): the qp-0 header is a byte prefix of C's frame OBU and
  strictly shorter than the qp-1 header from the same parameters; the qp-1
  control reproduces C's temporal delimiter, sequence header and frame-header
  prefix so the parameter set is known-good. The qp-0 capture was checked to
  decode LOSSLESSLY (aomdec output == source) before being adopted as an
  oracle — `SUSPECTED-C-BUGS.md` #1's variance-boost caveat does not bite on
  mainline defaults. Mutation-verified (forcing `coded_lossless = false`
  fails the qp-0 test). **The public envelope is unchanged: QP 0 is still
  refused** — the tile half (TX_4X4-only coding with no tx_size / tx_type
  symbols, WHT residuals, lossless MD gates: `mds_do_txt = 0`, RDOQ off,
  `svt_av1_is_lossless_segment` sites in product_coding_loop.c:7065/7173/
  7376/7584, full_loop.c:1756/1925/1936, rd_cost.c) is the next chunk, and
  the refusal comes off only when that is byte-verified against this oracle.
- **The C reference oracle is cargo-driven, both variants, SHA-stamped —
  issue #4 invariants B and C.** `crates/svtav1-cref/build.rs` used to link a
  prebuilt `Bin/Release/libSvtAv1Enc.a` and panic with a cmake line to type by
  hand; the `SVT_HDR_MODE=ON` oracle had no cargo path at all. Now a fresh
  clone's first `cargo test` configures and builds `Bin/Release`
  (mainline, `BUILD_APPS=ON` — the config CI and every shell tool already
  assumed) and `Bin/ReleaseHdr` (fork), each stamped with the submodule's git
  SHA + a config key in `.zenav1-cref-stamp`, so an unchanged tree never
  rebuilds and a submodule move triggers an incremental `cmake --build`. A
  pre-stamp hand build is trusted and stamped rather than rebuilt. Missing
  cmake / cc / nasm (x86 only) panics with the install one-liner.
  `SVT_CREF_LIB_DIR` (link a prebuilt archive, build nothing) and
  `SVT_CREF_SKIP_HDR=1` are the knobs. CI's hand-typed cmake step is replaced
  by `cargo build -p zenav1-svt-cref`. Measured locally (Apple-silicon
  laptop, ninja + clang, `-j4`): mainline from nothing 245 objects / 15.6 s
  wall, fork 238 objects / 16.0 s, the second `cargo build` a 0.75 s no-op;
  the differential suites and `regression_spotcheck.sh` pass against the
  cargo-built oracle.
- **CI runs the pure-Rust tier on `windows-11-arm`, `macos-15-intel` and
  `i686-unknown-linux-gnu` (via `cross`)** — issue #4 phase 4. The tier is
  `cargo build --workspace --exclude zenav1-svt-cref` + `cargo test -p
  zenav1-svt`: the facade's dev graph has no cref dependency, so the e2e /
  golden-parity / SIMD-tier-invariance / issue-repro suites run with no C
  toolchain. Corpus and decoder skips are set at workflow scope
  (`ZENAV1_SKIP_CORPUS_TESTS`, `ZENAV1_SKIP_DECODER_TESTS`). Three
  `real_encode.rs` tests wrote to a literal `/tmp`; they use the OS temp dir.
- **10-bit encoding at NON-64-ALIGNED dimensions — the product case for 10-bit
  AVIF** (`bd10_partial_sb_gate.sh`, **157/157 byte-identical to the C
  reference**; every one of those cells was a refusal before). Both bd10 level
  producers now handle partial superblocks: the full-RD funnel (preset ≤ 8),
  which needed only the gate lifted because it rides the same partition search
  and leaf funnel as the already-partial-SB-correct 8-bit path; and the
  level-only re-encode post-pass (preset ≥ 9), which needed SB-extent-sized
  recon buffers, straddle-clipped recon writes, SB-extent-padded 10-bit
  sources, and the pack's skip-off-frame-quadrant child walk in place of a
  fixed `(partition_type, children.len())` offset table that a pruned
  partial-SB child list makes both `panic!`-prone and positionally wrong.
  `bit_depth_config_error` no longer refuses ANY 10-bit configuration on
  dimension grounds; `docs/REFUSED-CONFIGS.md` drops 12 → 10 CAPABILITY
  refusals, and `arbitrary_size_robustness.sh` goes from 80/80 with **48
  refused** to **128/128 with 0 refused** — those 48 are exactly these cells,
  and every one now decodes under the AV1 reference decoder.
  Data: `benchmarks/bd10_partial_sb_2026-08-04.tsv`; full record in
  `docs/bd10-port-map.md`. Residual (NOT closed, pinned self-promotingly in the
  gate): a set of non-flat cells, measured to be the known bd10 non-flat gap
  (21.5% of non-flat cells at 64-aligned dims vs 26.3% at partial-SB dims;
  `uniform` is 100% everywhere) rather than a partial-SB gap.

- **MAINLINE's chroma-q derivation is ported (tune IQ), refs #9 item 2.**
  `rc_crf_cqp.c` has TWO chroma-qindex blocks separated by `#if SVT_HDR_MODE`
  and this port had only the fork one, gated behind `is_fork()`, so mainline
  always emitted zero chroma deltas. The mainline arm (`:592-602`) is a
  DIFFERENT derivation, not a subset: the ramp is off `new_qindex` rather than
  the post-offset value, the clip ceiling is 16 rather than 12, and U gets no
  `+12` (both planes carry the same delta). Found by
  `tools/identity_diff.sh` on `gradient 128x128 q40 p6 SVT_TUNE=3`, which put
  the first divergence at `FH delta_q_u_dc.coded C=1 Rust=0` with the tile
  payload already the same size on both sides. All-zero at any tune but IQ, so
  every non-tune-IQ cell is byte-identical by construction. Tune IQ is still
  NOT byte-identical to C — the knobs gate is 31/36 with a 1-6 byte tile-payload
  residual — but the frame header now matches.
- **PQ-shaped 10-bit source + a photographic native-10-bit gate (issue #7 /
  task #6 chunk 2b).** `identity_run` gains `SVTAV1_HBD_PQ`: the 8-bit luma is
  linearized as sRGB, mapped to a 1000-nit display, run through the SMPTE
  ST 2084 (PQ) OETF and quantized to 10-bit limited range; chroma is rescaled
  8-bit limited -> 10-bit limited. The low bits are then a consequence of a
  real transfer curve rather than the synthetic `(3r + 5c + v) % 4` pattern the
  chunk-2 gate uses, and the code-value histogram is PQ-shaped. Two gates
  consume it: a corpus-free PQ tier inside `tools/bd10_hbd_src_gate.sh`
  (**18/18 byte-identical**, and it runs in CI where a photographic gate
  cannot — no runner has the corpora), and the new
  `tools/bd10_hbd_pq_gate.sh` on real CID22-512 photographs (**presets 8 and 9
  40/40 byte-identical**; preset 6 carries 12 `uname -m`-scoped aarch64 pins,
  see below).
- **Measured: C's per-host bitstream divergence is far wider at bd10 than
  `docs/SUSPECTED-C-BUGS.md` #9 recorded.** Same commit, same port binary:
  `bd10_nonflat_gate.sh` is 309/309 in CI (x86-64) and **197/309** locally
  (aarch64/macOS); `bd10_photo_gate.sh` (not in CI) is **53/191** locally. The
  port is not the variable side — `tier_invariance.rs` holds its bytes across
  every dispatch tier, and failing photographic cells were re-encoded by a
  build of the pre-session tree (`bfae1b69`) with byte-identical output. Flat
  and low-complexity synthetic content agrees on both hosts; non-flat and
  photographic content diverges. Entry #9 now carries the table and the
  quantified case for an aarch64 CI runner.

- **`RcConfig::aq_mode != 0` is now REFUSED (issue #9 item 8).** C's
  `--aq-mode` default is 2 and it is INERT for a single still — aq-mode-2's
  deltaq is TPL-gated (`rc_aq.c:899`) and one frame has no lookahead — while
  this port's non-zero `aq_mode` ran a HOMEGROWN frame-level VAQ/TPL qindex
  shift that is a port of nothing. So `aq_mode = 2`, the value a caller copies
  straight out of C's documentation, meant "C: no change" and "port: shift the
  whole frame". Refused rather than documented, because documentation does not
  stop a caller from copying C's default. `0` (the default) is the value that
  matches C. C's segmentation-side `aq_mode` is a different parameter and stays
  C-parity-tested.
- **`SpeedConfig` lost 12 dead fields (issue #9 item 9).** `enable_cdef`,
  `enable_restoration`, `enable_cfl`, `enable_palette`, `enable_identity_tx`,
  `enable_obmc`, `enable_warped_motion`, `enable_compound`,
  `subpel_precision`, `hme_levels`, `me_search_width`, `me_search_height` had
  ZERO consumers anywhere in the workspace while reading as an authoritative
  preset table — two tests asserted `enable_palette` / `enable_obmc`, which
  tested nothing but the table's own initializer. Note the issue's own list was
  partly wrong and is corrected here: **`max_intra_candidates` is LIVE**
  (`PartitionSearchConfig::from_speed_config` → the NIC cap at
  `partition.rs:2206`), as are `enable_adst`, `enable_directional_modes`,
  `enable_filter_intra`, `rdo_tx_decision`, `max_partition_depth`,
  `lambda_scale` and `preset`; `enable_temporal_filter` is read on the dormant
  inter path and stays.

### Changed

- **`AvifEncoder` has no inert knobs left — issue #9 item 7.** Two are now
  wired to the real pipeline settings, each with a liveness test that fails if
  the knob stops moving the emitted bytes:
  - `with_qm(bool)` -> `EncodePipeline::hdr.enable_qm`.
  - `with_variance_boost(bool, u8)` -> `hdr.{enable_variance_boost,
    variance_boost_strength}`. **Replaces `with_vaq(bool, f64)`**; the strength
    is now C's documented 1-4 scale, not an invented 0.0-1.0 float.
  The remaining four were REMOVED rather than faked, because neither this
  pipeline nor C has a counterpart: `with_trellis` (SVT-AV1 has no trellis
  knob; RDOQ level comes from preset + coeff level, C-exactly),
  `with_seg_boost` + the `seg_boost()` getter (no segmentation on the still
  path), and `with_still_image_tuning` (the encoder is unconditionally
  still-image: one KEY frame, temporal tools forced off for all-intra as C
  does).
- **`AvifEncoder::{enable_qm, enable_variance_boost}` now default to `false`**
  — C's mainline defaults, and the bytes this encoder has always emitted. They
  previously defaulted to `true` while being ignored, so leaving them `true`
  once live would have silently changed every caller's output.
- `AvifEncoder::encode_y8` is documented MONOCHROME-only (`mono_chrome = 1`):
  correct for a gray image or an AVIF alpha plane, not a way to encode the luma
  of a colour image. It still pre-pads to a multiple of 64 because
  `EncodePipeline`'s TRUE -> ALIGNED padding is wired on the 4:2:0 path only —
  so for a non-64-multiple gray image the coded frame is larger than
  `EncodedAvif::{width, height}`. Arbitrary-dims MONOCHROME is a pipeline gap.

### Fixed

- **`pd0_use_src_samples` on the fixed-tree PD0 path — a REJECTED experiment,
  re-run over a fixed premise, closes every open video-key scoreboard cell.**
  C's video PD0 predicts each block from the RECON it generates
  (`ctx->pd0_use_src_samples = allintra || hbd_md`, enc_mode_config.c:7309;
  product_coding_loop.c:8430); the port's LVL_5 predicted from the SOURCE. This
  had been wired once and REJECTED — no movement on p4/p5/p7 and p9 worse
  (0.189% -> 0.378%) — but that measurement was taken over the light-PD0
  boundary-shape defect fixed in the entry below, which was still splitting
  every edge node underneath it. Re-run over the fixed premise, a 45-cell
  video-mode matrix (`72x88 q40`, 5 content classes x 9 presets, one build each
  side) goes **28 -> 34 byte-identical with nothing worse**: `gradient` and
  `screenrep` at presets 9, 10 and 11 all close (`gradient p11` 1.285% ->
  byte-identical), p12/p13 fall to ONE byte across five content classes, and the
  other 37 cells are unchanged to the byte. TWO spot-check cells PROMOTED to
  `byteVideoKey` — `video-key-rate-arm-p9-72x88`, the LAST `ratioVideoKey`
  video-key cell, and `video-key-lpd0-edge-shape-p9-screenrep` — and two added.
  One weaker cell survives and is NOT this chunk's: `video-key-ibc-arm-p8`
  (`screen 64x64 q40 p8`) is `fhVideoKey`, header-only, and its payload is
  **398% off** (C 114 B vs the port's 568) with its 72x88 sibling at 409% —
  same content class, same preset, untouched here. Still open at 72x88 q40 and
  NOT touched by this chunk (the LVL_1 refinement path, verified unchanged on
  both sides of the A/B): presets 0/3/8 — `diag p3` at 22.3% and `screen p8`
  are the loudest cells in the campaign — and the one-byte p12/p13 row. No still regression: `identity_full_8bit` 1100/1100,
  `regression_spotcheck` 53/53, `cargo nextest --workspace` 2415/2415. See
  `rust/docs/INTER-ENCODE-PLAN.md` §1l.

- **The LIGHT-PD0 boundary SHAPE — a partial-SB edge node was priced as the
  square that does not fit.** `pd0.rs` gave a one-false boundary node its
  fitting `PART_H`/`PART_V` rectangle only for the LVL_1 family; LVL_5 (light
  PD0, the fixed-tree path at preset >= 9) got the square cost, which prices
  twice the pixels that fit and therefore loses to SPLIT — so the port coded
  `BLOCK_8X8` + a split where C codes `BLOCK_8X16`. Invisible on the allintra
  arm, where `nsq_geom_level` is 0 above M6 and such a node force-splits before
  it is costed; the video arm never turns NSQ geometry off. C's own
  `svt_aom_full_cost_pd0` dump prices only rectangles in the x=64 superblock of
  a 72-wide frame (`32x64`, `16x32`, `8x16`), and the two C functions that
  decide it carry no `pd0_level` term. Coded trees against C, measured both
  ways: `gradient 72x88 q40 p9` 19 field flips / 7 port-only blocks -> **9 / 3**,
  `p11` 9 / 7 -> **1 / 3**. Bytes: `screenrep 72x88 q40 p9` 0.749% -> 0.125%
  (new spot-check cell at a 0.5 limit), `p11` 0.827% -> 0.165%, `gradient p9`
  0.189% -> 0.126%, `p10` 0.125% -> 0.063%; `gradient p11..p13` move the other
  way (1.04% -> 1.29%) with a strictly closer tree, the same
  worse-tree-nearer-size cancellation §1f describes. `SVTAV1_PD0DBG` now also
  fires on the light-PD0 path, which it never did — the first join it enables
  shows C and the port testing the SAME 135 PD0 blocks with 101 of the costs
  differing. No still regression: `identity_full_8bit` 1100/1100,
  `regression_spotcheck` 51/51, `cargo nextest --workspace` 2415/2415. See
  `rust/docs/INTER-ENCODE-PLAN.md` §1k.

- **The video-mode KEY frame's last two named residuals — both CLOSED, and one
  was a conformance bug.** `gradient 64x64 q40 p6` (the campaign's reference
  cell) and `diag 64x64 q40 p11` are now byte-identical to C, as is
  `gradient 64x64 q40 p11`. Two defects, both an ALLINTRA constant running on
  every frame:
  (1) `svt_av1_optimize_b`'s RDOQ rate weight is
  `plane_rd_mult[allintra || rtc][is_inter][plane_type]` (full_loop.c:994/1085)
  and the port hardcoded the allintra row (17 luma / 13 chroma). A video frame
  takes index 0, where CHROMA is **20** — luma is 17 on both arms, so the
  divergence was chroma-only. Ported as `quant::PLANE_RD_MULT` +
  `plane_rd_mult()`, selected by a new `allintra_rd_mult` flag on
  `CodingQuantCfg` / `FunnelFrame` and threaded through `tx_unit_hbd` and the
  bd10 re-encode. Reference cell 965 B -> 961 B, byte-identical.
  (2) `encode_block_syntax` gated the per-block `tx_size_cdf` symbol on
  `is_key` instead of on the frame header's own `tx_mode`, so at video preset
  >= 10 — where the video arm signals TX_MODE_LARGEST — the port announced
  LARGEST and coded a `tx_depth` symbol per block anyway: an **undecodable**
  stream, not just a parity gap. `EntropyCtx` now carries `tx_mode_select`
  from one helper shared with the header writer, the pack walk and the per-SB
  CDF-chain simulation. `diag 64x64 q40 p11` 403 B -> 401 B and
  `gradient 64x64 q40 p11` 1025 B -> 1024 B, both byte-identical.
  Localized with the C `--wrap` interposers in `tools/ctrace-linux/`
  (`SVT_CCOEF_OUT`, widened here so an unset `SVT_CCOEF_XY` dumps every coded
  txb instead of one pinned block; `SVT_QLEVELS_OUT`, which gained a pre-quant
  `co=[]` field; `SVT_RECON_BIN`) plus the op-trace differ, which put the first
  divergence at `TX_SIZE_CDF[0][0]` in the first coded block. Spot-check:
  `video-key-edge-filter-diag-p11` and `video-key-txs-arm-tx-mode-p11`
  PROMOTED to `byteVideoKey`, new `video-key-rdoq-plane-rd-mult-p6-64x64`.
  No still regression: `identity_full_8bit` 1100/1100, `regression_spotcheck`
  50/50 (49 + the new cell), `cargo nextest --workspace` 2415/2415, and the six
  reference identity cells at their pinned sizes (290 / 839 / 63 / 171 / 580 /
  693 B). See `rust/docs/INTER-ENCODE-PLAN.md` §1j. (93958230)


- **The PD0 -> PD1 subresolution leak does not exist — refuted at tier 1, and
  it was the inter campaign's named next chunk.** `md_stage_1` reads
  `ctx->subres_ctrls.step` with no `PD_PASS_1` guard
  (product_coding_loop.c:7027) where `md_stage_2`/`md_stage_3` zero it, which
  reads as "the video arm's `pic_pd0_lvl = 3` makes PD1's survivor-choosing
  MDS1 run at half vertical resolution". But `set_subres_controls` has FOUR
  call sites, not one: each regular-PD1 derivation calls
  `set_subres_controls(ctx, 0)` unconditionally (`_default` :7919, `_rtc`
  :8035, `_allintra` :8151) and `enc_dec_process.c:3038-3050` runs one of them
  on the SAME context between PD0 and PD1's md loop. New shim
  `ref_subres_pd0_then_pd1` drives PD0 then one PD1 arm on one context in C's
  order; `tests/c_parity_subres_carry.rs` pins step 1 -> 0 on all three
  regular arms at every `pd0_level` 0..=6, with the two light-PD1 arms (which
  really do leave the step alone, and never call `md_stage_1`) as the positive
  control.
- **`tools/identity_full_8bit.sh` aborted at cell 0 under bash 3.2** once
  `KNOWN_DIFF` emptied out: `"${KNOWN_DIFF[@]}"` on an empty array with
  `set -u` is an unbound-variable error there, and `set -e` took the sweep
  down before a single verdict printed. macOS finds bash 3.2 first on a login
  PATH, so the failure was per-shell. Guarded; 1100/1100 after.
- **`tools/ctrace-linux/run.sh` dropped every CONFIGURATION selector at the
  container boundary** — `SVT_FRAMES`, `SVT_AVIF`, `SVT_INTRA_PERIOD`,
  `SVT_HIER_LEVELS`, `SVT_PRED_STRUCT`, `SVT_CPU_FLAGS`, `SVT_TILE_*`,
  `SVT_TUNE`, `SVT_MAX_TX_SIZE`, `SVT_CRF_OFFSET`, `SVT_CSP`,
  `SVT_SUPERRES_KF_DENOM`. A caller asking for the inter campaign's VIDEO-mode
  2-frame GOP got a container that encoded ONE STILL frame, with no error and
  a valid op trace of a different encode. It also could not run at all from a
  `jj workspace` sibling, where the C submodule is a symlink resolving outside
  the `/repo:ro` mount. Both fixed; verified by encoding the reference cell in
  the container and getting 961 + 22 B with frame 0 BYTE-IDENTICAL to the host
  driver's.

- **Three more x86_64-only NULL-RTCD SIGSEGVs, and the ISA-dependent C UB
  behind the fourth.** `init_wedge` (`inter_pred_shims.c`) ran
  `svt_av1_init_wedge_masks` without RTCD setup — it builds its tables with
  bare `svt_memcpy` — and `ref_warp_error` / `ref_refine_integerized_param`
  (`ref_shims.c`) initialized only the COMMON table, while `warp_error`
  (`enc_warped_motion.c:21`) accumulates with `svt_nxm_sad_kernel` from the
  ENCODER dsp table; all four calls landed at `rip = 0x0` on x86 and could not
  fire on aarch64. The fifth x86 failure,
  `c_parity_rc_process::new_framerate_matches_c`, is NOT a port defect and is
  left for its owning lane: `pass2_strategy.c:887` casts a `double` past
  `INT_MAX` to `int`, which is UB, and the hardware disagrees — `cvttsd2si`
  gives `INT_MIN`, `fcvtzs` saturates to `INT_MAX`. The test probes
  `target_bit_rate = 4_000_000_000`, which `verify_settings`
  (`enc_settings.c:110`) rejects outright, so no port behaviour can satisfy
  the cell on both hosts. Recorded as SUSPECTED-C-BUGS #17 with the boundary a
  test may legitimately reach.

- **Duplicate `ref_get_wedge_params_bits` broke the whole workspace link on
  x86_64-linux.** Byte-identical definitions in `inter_pred_shims.c:487` and
  `md_subpel_shims.c:333`; Apple's `ld64` takes the archive's first definition
  and links, `rust-lld` errors with `duplicate symbol` and nothing in the
  workspace builds. Removed the newer copy; `svtav1_cref::md_subpel` calls the
  existing one. All shims land in one archive, so `ref_*` names are
  workspace-global.

- **16 more x86_64-only shim SIGSEGVs, from two lanes that landed the same
  day.** Found by re-running the suite on x86 after the obmc fix below; all 16
  were green on aarch64-darwin. (a) `c_parity_entropy_inter` (7 tests):
  `ec_build_xd` and `EC_FC_TABLE` call `svt_aom_init_mode_probs`, whose
  `COPY_CDF` is bare `svt_memcpy` (`cabac_context_model.c:735`, while the same
  file uses the null-safe `SVT_MEMCPY` at :1923) — the NULL RTCD pointer again;
  both sites now route through a one-shot `ec_init_mode_probs`. (b)
  `c_parity_estimate_transform` + `c_parity_txfm_pf_entry` (9 tests):
  `svt_av1_fwd_txfm2d_*_avx512` store with `vmovdqa32`, the 64-byte ALIGNED
  store, into Rust `Vec` buffers that are 2/4-byte aligned — measured fault at
  `vmovdqa32 %zmm0,-0x40(%rax)`, target 48 bytes past a 64-byte boundary.
  `ref_wht_fwd_txfm`, `ref_highbd_fwd_txfm` and `ref_estimate_transform` now
  stage through 64-byte-aligned scratch (copying the coefficient buffer IN as
  well as out, since these tests prefill it and assert C leaves unwritten
  positions alone). Both are re-breaks of contracts `ref_shims.c` had already
  documented — the RTCD one-shot at :790 and the AVX2 32-byte staging at :1315.
  Verified 1542/1542 on x86_64-linux and 1535/1535 on aarch64-darwin.

- **Two `c_parity_obmc_search` oracles were unsound; both were green on
  aarch64 by accident and broke on x86_64.** Found by the first cross-ISA run
  of the suite (2026-08-31): `convolve8_matches_c` failed with a whole-block
  value mismatch and `upsampled_pred_matches_c` SIGSEGV'd, on x86_64-linux
  only. Neither was a port defect — the port's `convolve8_horiz` is
  ISA-invariant scalar integer code and produced the right answer on both
  hosts. (a) `svt_aom_convolve8_{horiz,vert}_c` derive the filter phase from
  the filter POINTER'S ADDRESS (`convolve.c:54`, `get_filter_base` =
  `ptr & ~0xFF`, documented as assuming a 256-byte-aligned table), so
  forwarding a Rust `&[i16; 8]` made the oracle apply the taps at
  `addr - (addr % 16)` — correct only when the Rust static happened to land
  16-byte aligned, which it did on aarch64 and did not on x86.
  `ref_me_convolve8_{horiz,vert}` now stage the taps into an
  `_Alignas(256) int16_t[16][8]`, matching `ref_shims.c`'s existing
  `ref_convolve8_horiz`. (b) `ref_upsampled_pred` did not initialize RTCD, and
  `svt_aom_upsampled_pred_c` reaches bare `svt_memcpy` — a `.bss` function
  pointer that is NULL before setup on x86 and a devirtualized concrete symbol
  on aarch64 — so the call landed at `rip = 0x0`; it now calls
  `obmc_ensure_init()` first. Pinned by two new controls:
  `convolve8_oracle_is_alignment_invariant` (feeds the same taps from every
  2-byte residue in a 256-byte window; fails pre-fix on aarch64 too, so it is
  ISA-independent) and `upsampled_pred_cold_rtcd_zero_subpel` (the minimal
  reproducer, first C call in its own process). Verified 1275/1275 on
  x86_64-linux and 1268/1268 on aarch64-darwin.

- **`has_top_right`'s `PARTITION_VERT_A` check now reads the MUTATED `bs` in
  `intrabc_mvp.rs` too.** The same defect fixed in `inter_mvp.rs` was present
  in the IntraBC copy of the function, where the randomized `c_parity` sweep
  had never happened to place a VERT_A cell on a geometry that advances `bs`.
  Pinned by `c_parity_has_top_right_vert_a_uses_mutated_bs` in
  `tests/c_parity_intrabc_mvp.rs`, which fails before the fix
  (`ref_mv_stack[0].weight` 672 against C's 668) and passes after.
  **Byte impact MEASURED, and it is none:** a 120-cell port-only sweep
  (gb82-sc x 10 images x presets 1-4 x qp {20,32,48}) was run before and after,
  and all 120 `(bytes, sha256)` pairs are identical —
  `benchmarks/intrabc_has_top_right_vert_a_2026-08-31.{tsv,meta}`. So this is a
  correctness fix with no shipped-byte change on that corpus; per
  `docs/WORKING-ON-THIS.md` §3 it deliberately gets NO
  `regression_spotcheck.sh` cell (a cell that never failed cannot guard it),
  and per §7 it STAYS — the same function serves the inter MVP stack, where the
  geometry is far less constrained. `regression_spotcheck.sh` is 35/35 after.
- **Shim data race: per-call state in `static` (test harness).** cargo runs a
  test binary's tests on several threads, so a `static` scratch buffer shared
  by two concurrently-running `c_parity` tests is a data race that surfaces as
  an occasional WRONG NUMBER, not a crash — which reads exactly like a port
  bug. Measured: with `static CandidateMv stack2d[...]` in
  `ref_setup_ref_mv_list_intra`, `c_parity_intrabc_mvp` failed at partition=0
  with count 1 vs 2 under `--test-threads=3` and passed under
  `--test-threads=1`. `shims/ref_shims.c` was then audited end to end: five
  per-call `static`s found, all five now `calloc`/`free` per call — that
  `stack2d`, `ref_lf_limits`'s `LoopFilterInfoN`, the three
  `RestorationLineBuffers` scratch banks in the loop-restoration apply shims,
  and `ref_noise_normalization`'s synthetic `SequenceControlSet` /
  `PictureControlSet` (whose `noise_norm_strength` is written per call and read
  by the callee). What stays `static` is documented in the file header with the
  reason each is not per-call state: `g_fc` (a deliberate two-call protocol
  with a caller-held mutex) and the three idempotent RTCD init flags. The rule
  itself now leads that header so the next shim author does not re-introduce
  it.

- **`has_top_right`'s `PARTITION_VERT_A` check must read the MUTATED `bs`
  (chunk C2).** C's `has_top_right` (adaptive_mv_pred.c:266-325) shifts `bs`
  left inside its 4x4-group loop (`:303-313`) and the `PARTITION_VERT_A` test
  at `:314-322` then reads that MUTATED value. Reading the ORIGINAL `bs` there
  diverges: measured against the exported C symbol at `mi = (36, 10)`, an 8x8
  block in a 64x64-mi superblock whose current cell has
  `partition == PARTITION_VERT_A`, `bs` enters as 2 and the loop advances it to
  4, after which `mask_row == 4` makes C drop the top-right candidate — the
  port kept it, for `ref_mv_stack[0].weight = 672` against C's 668. Only
  `partition == 6` diverged; the nine other partition types agreed, which is
  what localizes it. Pinned by
  `c_parity_has_top_right_vert_a_uses_mutated_bs` (failed before, passes
  after). **`crates/svtav1-encoder/src/intrabc_mvp.rs` carries the same
  original-`bs` reading and is therefore latently wrong on the same geometry**;
  it is another chunk's file and was NOT edited here.
- **`add_ref_mv_candidate`'s `assert(weight % 2 == 0)` does not hold (chunk
  C2).** C asserts it (adaptive_mv_pred.c:63) but ships with `NDEBUG`, so it is
  never checked. With `row_adj == 1` — an 8x4 block at an odd `mi_row` —
  `max_row_offset` is -5 and `scan_row_mbmi`'s `inc` reaches 5 for a candidate
  8 or 16 mi tall, giving `weight == 5`. Reproduced on the randomized grids in
  `tests/c_parity_inter_mvp.rs`. The assert is deliberately NOT transcribed;
  an odd weight is a legal input and changes nothing downstream.

- **Mainline chroma delta-q desynced every decoder — `entropy::obu::ChromaQSignal`
  (2026-08-28).** Porting mainline's chroma-q derivation (below) made tune IQ
  produce non-zero chroma deltas, and they were emitted through the only form
  the frame-header writer had: the FORK's `diff_uv_delta = 1` + four
  independent deltas. That form REQUIRES the sequence header to have signalled
  `separate_uv_delta_q = 1`; the fork's does, MAINLINE's signals 0, and spec
  5.9.12 reads `diff_uv_delta` only when that bit is 1 — so the extra bit and
  the two extra V deltas shifted every following bit of the frame header. Not a
  byte-count difference: a desync. `tools/variance_boost_recon.sh` went 0
  passed / 60 failed, every cell DECODE FAILED (CI run 33220828356), and a
  plain tune-IQ 128x128 q40 p6 encode was rejected by aomdec AND dav1d.
  The fix is a type rather than a branch — `ChromaQSignal::Shared { dc, ac }`
  (SH bit 0, one pair reused for V, no `diff_uv_delta`) vs
  `ChromaQSignal::Separate([i8; 4])` (SH bit 1, the fork's four) — so a frame
  header that disagrees with its sequence header no longer type-checks. The
  same SH bit also gates `qm_v`, which was keyed on `chroma_q.is_some()` and
  would have emitted a stray 4-bit field the moment QM and tune IQ were on
  together. After: variance_boost_recon **60/60**, decode_conformance 4:2:0
  1575/0. Two cells added to `tools/regression_spotcheck.sh` (now **35/35**),
  earned the hard way — the writer was temporarily reverted to the buggy form
  and both cells confirmed to fail under aomdec, then restored.


- **Monochrome straddling edge block wrapped its recon into the next row
  (every SB row after the first decoded wrong on frames with a thin right
  edge).** The second half of the mono partial-SB fix below: once a one-false
  edge leaf is coded as the single legal rect, a thin right edge makes that
  rect STRADDLE the aligned width (a VERT 32x64 at x=192 on an aligned-200
  frame keeps 8 in-frame columns). `encode_single_block` stored the full
  block width at the aligned stride, so the off-aligned columns wrapped into
  the next row's columns 0..24 and overwrote an already-committed
  neighbour's recon — the encoder then predicted the next SB ROW from wrapped
  pixels the decoder never had. Measured (rav1d-safe, gradient qp 10, preset
  6): 200x136 27.9 dB with the first SB row at 55 dB and the second at 23 dB
  from column 0 outward; 136x200 25.0, 200x72 35.3, 72x136 31.0, 264x136
  28.1, 200x200 24.4 dB; 192x136 / 200x64 / 64x136 clean (no thin right edge,
  or nothing below it). aomdec DECODES the broken streams, so decodability
  was hiding it. The store now carries the same straddle clip
  `leaf_funnel::commit_leaf` already had (nothing reads past the aligned
  extent — `extract_neighbors_tiled` clamps like the decoder's spec-7.11.2
  replicate). After: 200x136 56.96 dB, every cell above 56-58 dB, 22/22
  zenavif svt-rs tests. Regression: `mono-straddle-wrap-p6-200x136` in
  `tools/regression_spotcheck.sh` — a recon oracle (encoder FINAL recon vs
  `aomdec --rawvideo`, luma at true dims) on the `(x+y)` ramp fed as `raw:`
  content, because on the synthetic `gradient` the PD0 resolves that node to
  SPLIT and nothing straddles (bytes identical with and without the clip).
  Witnessed before the clip: 14,720 of 27,200 luma bytes differ (encoder
  recon 56.97 dB vs source, aomdec output 27.89 dB); after: byte-equal. A
  96x80 control cell (32-wide edge, no straddle) is byte-equal either way.
  The decoded round-trip over seven geometries is gated on the zenavif side.
- **Monochrome partial superblocks at preset 6 emitted an undecodable stream
  (a `PARTITION_NONE` square coded at a frame edge).** The M6 PD0 keeps NSQ
  geometry on, so a one-false edge node is TESTED with the rect edge-shape
  cost instead of force-split; `encode_fixed_tree`'s funnel arm (4:2:0) codes
  such a leaf as the single legal `PARTITION_HORZ` / `PARTITION_VERT` rect,
  but the mono arm (no funnel) fell through to a full-size `PARTITION_NONE`
  square — illegal per spec 5.11.4, refused by the pack's debug_assert in a
  debug build and written as-is in release (96x80 / 128x80 / 200x136 gradient
  at qp 10: "Corrupt frame detected" under aomdec; zenavif measured 18 dB
  garbage at 96x80 q85). Presets >= 7 were never affected (NSQ geometry off
  -> forced SPLIT in PD0) and 4:2:0 is byte-neutral by construction (its arm
  returns first; on 64-aligned frames both edge flags are true). The mono arm
  now applies the same rule. Found by zenavif's seam canary
  `svt_rs_direct_mono_partial_sb_preset6_still_broken` the day its CI first
  ran `cargo test` (dev profile) against this tree. Regression:
  `pipeline::tests::mono_partial_sb_preset6_edge_leaf_codes_the_edge_shape`
  (7 geometries; panicked with the pack's assert before, passes after) + three
  `mono-partial-sb-p6-*` decode cells in `tools/regression_spotcheck.sh`.
  Decode round-trip (rav1d-safe + aom-rs, 56 dB at 96x80) is gated on the
  zenavif side.
- **MDS1 candidate costs 103 rate units cheaper than C on DC / IntraBC
  candidates — issue #16 root-caused and closed.** The probe the issue named
  (`SVT_FASTCOST_XY` + `SVT_FULLCOST_XY` in the `--wrap` container vs the
  port's `SVTAV1_CANDDBG` dump) split the delta in one run: all 57 signalling
  rates and every `ydist` matched; only the tx-type rate on the ADAPTED CDF
  rows (intra `DC_PRED`, inter) differed. C's MD-side coefficient cost
  (`svt_av1_cost_coeffs_txb`) keys `is_inter` on `is_inter_mode(mode)` without
  `use_intrabc`, so its encode pass adapts the intra DC ext-tx row for an
  IntraBC txb while its writer adapts the inter row; the port's per-SB chain
  simulation re-coded with writer semantics and rebuilt rate tables from a
  DC row C never sees (`docs/SUSPECTED-C-BUGS.md` #10 — the UPDATE half of
  the quirk whose READ half `cost_dir` already reproduced). Fix:
  `CoeffFc::md_side_ibc_txt_update` on the chain contexts routes IntraBC
  tx-type adaptation through `md_update_tx_type_ibc_quirk` (intra set, DC
  row, no update at DCT-only sizes). After: 57/57 MDS1 costs at
  `terminal 188x256 p2 q55` mi=(50,42) equal C's (was 54/57), stream unchanged.
  Byte-neutral on every gate run: `regression_spotcheck` 28/28,
  `alignment_gate` 74/74 (+ the IBC / palette screen gates, see the commit).
  Unit witness `md_side_ibc_tx_type_update_adapts_the_intra_dc_row_like_c`
  (mutation-verified). Record: `benchmarks/issue16_mds1_txt_cdf_2026-08-27.md`.
- **The 10-bit reconstruction never received the loop restoration it
  signalled — issue #13.** `recon10` fed the Wiener SEARCH (taps picked on
  10-bit data, signalled in the frame header) but only the u8 chain was handed
  to `apply_restoration_frame`, so no 10-bit plane in the port ever carried the
  filter a conforming decoder applies — and nothing could observe it, because
  no post-filter 10-bit recon was published. Now: the DSP stripe-boundary
  machinery (`StripeBoundariesT<T>`, `save_tile_row_boundary_lines`,
  setup/restore) is generic over the pixel type with the u8 names unchanged,
  `loop_restoration_filter_unit_hbd` is the highbd apply arm WITH boundaries
  (C `svt_av1_loop_restoration_filter_unit` at `highbd = 1`, pinned by the new
  `highbd_filter_unit_with_boundaries_matches_c` differential — 200 random
  cells, both `need_boundaries` arms, `data` restored exactly), the encoder's
  `save_lr_boundaries_bd` / `apply_restoration_frame_bd` are the generic
  bodies (u8 delegates, byte-neutral by construction), and the pipeline
  applies LR to the 10-bit canvas with boundary lines from the 10-bit
  post-deblock / post-CDEF planes. Published as the additive
  `EncodePipeline::last_recon10_final` (deblock -> CDEF -> LR on the 10-bit
  canvas; the 10-bit twin of `last_recon`, `with_recon_output` gated).
  Witness `svtav1/tests/issue13_repro.rs`: 383x512 bd10 p6 q40 (luma Wiener
  fires) — `last_recon10_final` == `aomdec` sample for sample; with the apply
  disabled 175,734 samples differ. `SVTAV1_FINAL_RECON` dumps the 10-bit final
  recon (u16 LE) at bd10, and `alignment_gate.sh`'s RECON leg now runs at
  BOTH bit depths (it was bd8-only because nothing 10-bit was comparable).
- **The MDS3 independent-chroma search ran on blocks where C skips it —
  issue #15 closed at 648/648** (`leaf_funnel.rs`). C gates
  `search_best_mds3_uv_mode` on `perform_ind_uv_search_last_mds`
  (product_coding_loop.c:1472-1504); the port implemented only its first arm
  and had nothing for the `inter_vs_intra_cost_th` arm (:1498-1501), which
  zeroes the intra count when `best_inter_cost * 100 < best_intra_cost * 100`.
  `is_inter` there is `is_inter_mode(mode) || use_intrabc`, so on SCREEN
  CONTENT a winning IntraBC candidate makes C skip the search entirely, keep
  `ind_uv_avail = 0`, and code each MDS3 candidate's injected uv-follows-luma
  chroma — where the port's uv table substituted `UV_DC_PRED`. Measured on
  `terminal` 188x256: p2 q55 C MDS1 best intra 97,762,561 vs best IntraBC
  84,376,537 (C codes uv=D113/-1), p4 q12 163,691 vs 148,994 (C codes
  `UV_CFL_PRED`); `ind_uv_avail = 0` read directly off C via the new
  `svt_aom_get_intra_uv_fast_rate` interposer. This was the last of #15's 67
  cells: `unaligned_identity_scan.sh` **646 → 648 / 648, 2 fixed, 0 broken**.
  Byte-neutral wherever no IntraBC candidate exists — the arm is genuinely
  inert there (`byteid_fingerprint` 168/168, **0 rows moved**). Regression cell
  `ind-uv-ibc-cost-gate-188x256` (spot-check 27 → 28). Data:
  `benchmarks/unaligned_real_identity_2026-08-14-induv.{tsv,meta}`.
- **`sse_i32` subtracted coefficients in i32 where C subtracts in `int64_t`,
  and panicked in debug where C's `uint64_t` wraps** (`svtav1-dsp`
  `residual.rs`; C `svt_full_distortion_kernel32_bits_c`, `pic_operators.c:86`).
  Three widths were Rust's rather than C's — the subtraction (`(x - y) as i64`),
  the square, and the accumulator — and the accumulator is what left
  `residual_recon_distortion_all_tiers_match_core` RED on `main`. All three now
  match C in every build. The NEON arm cannot widen first (no i64xi64 multiply
  exists to square an `int64x2_t`), so it keeps `vsubq_s32` and DETECTS a wrap
  by comparing against `vqsubq_s32`, falling back to the exact scalar core;
  fast path exact, slow path exact. New gate
  `sse_i32_matches_c_widths_at_i32_extremes` checks every tier against an i128
  oracle and asserts its own case set discriminates the two widths. **Byte-inert
  on every grid** (byteid 168/168 with 0 cells moved, unaligned scan 648 cells
  with 0 changed, partial_sb 146/146, decode grid 120/120, recon parity
  432/432). Measured: the wrap is unreachable on a real encode — 0 in 59,088,480
  elements, max |difference| 788 against an i32 ceiling of 2,147,483,647
  (`benchmarks/sse_i32_width_2026-08-11.meta`), so this does NOT explain issue
  #15, which stays open.

- **Loop restoration walked a different unit grid than the one the search
  sized — an out-of-bounds panic on the public encode API** (issue #11,
  `restoration.rs:985`, `index out of bounds: the len is 2 but the index is 2`).
  C derives the restoration-unit count (`svt_av1_alloc_restoration_struct`) and
  every unit walk (`svt_av1_loop_restoration_filter_frame`,
  `svt_av1_loop_restoration_save_boundary_lines`) from ONE
  `whole_frame_rect(&cm->frm_size, ..)`, and `cm->frm_size` is the pre-8-alignment
  coded size (`pcs.c:1337`, `picture_width - non_m8_pad_w`), CEILING-subsampled
  for chroma. The port's SEARCH used the true extent (task #95 goal 1) but
  `apply_restoration_frame` / `save_lr_boundaries` were still handed the ALIGNED
  `w`/`h`, so wherever the 8-alignment crossed a `count_units_in_tile(256, ..)`
  boundary the walk visited more units than the grid holds: true 383 counts one
  horizontal unit, aligned 384 walks two. Both now take the true extent plus the
  aligned canvas STRIDE, and chroma rounds up like C rather than down. Reported
  on 5 real renditions (115 of 34,200 HDR-grid cells); reproduced synthetically
  at `383x512` / `766x128` / `258x128` / `385x257` at bd8 AND bd10. The
  bitstream was never affected — the panic came after the tile was written — and
  the previously-panicking cells are now byte-identical to the C encoder
  (`regression_spotcheck.sh` cells `lr-align-cross-*`). A 2,280-cell A/B of the
  pre- and post-fix encoders over 19 dimensions × 5 presets × 4 qps × 2 depths ×
  3 contents shows every previously-working cell byte-unchanged.
- **The bd10 per-tile recon canvases were MERGED at the wrong stride.**
  `commit_leaf` writes them at the ALIGNED stride (the SB-extent product exists
  only so a right-straddle write wraps into slack rather than out of bounds),
  but the frame merge read them at the SB-EXTENT stride. Byte-inert while every
  gated bd10 cell had `ext_w == w`; it scrambled the 10-bit recon that the bd10
  deblock / CDEF / Wiener searches read the moment a frame had a partial SB.
- **The native-u16 source had no SB-extent twin.** `HbdSource` is padded
  TRUE→ALIGNED only while `blk_y_src10` gathers by absolute coordinates, so a
  straddling block would read past the plane or wrap into the next row. Added
  the `sb_input` / `sb_chroma_owned` equivalents and threaded `in_stride` into
  `FunnelSrc10`; the `debug_assert_eq!(in_stride, w, "bd10 hbd source assumes a
  64-aligned frame")` that stood in for this is gone.
- **Two out-of-bounds panics on the public encode API**
  (`crates/svtav1-encoder/src/intrabc_hash.rs`). C computes
  `x_end = pic_width - block_size + 1` as a SIGNED int
  (`hash_motion.c:195-196`, `:222-223`), so a picture smaller than the hash
  block just yields an empty loop; the port used `usize`, underflowed to ~2^64
  and indexed off the end. A 32x32 screen frame at preset 0 panicked twice
  (`len is 1024 but the index is 1024`, and `index 2048`). Found by the new
  8-bit gate's dims tier — no earlier gate encoded anything below 60x60 with
  the screen-content tools armed.

### Changed

- **Doc debt from the 2026-07-25 publication audit, second pass (issue #8).**
  The HDR-fork verification bar no longer contradicts itself between
  `README.md` and `rust/README.md`: fork mode IS byte-gated vs a
  `SVT_HDR_MODE=ON` C build at 10-bit (`hdr_bd10_gate.sh` 64/64, standing);
  the 8-bit 48/48 is a 2026-07-19 measurement (`docs/HDR-ON-4.2.md`) with no
  standing gate script, and `hdr_fork_e2e` is named for what it is (liveness +
  decode witnesses, 36/36). `identity_matrix` is described as its 54-cell
  default grid, with the 132/132 figure dated to the 2026-07-16 wider sweep it
  came from (`rust/README.md`, `C-TEST-PORTING-AUDIT.md`). `screen_ibc_gate`
  20/100 -> 22/100 (the script's `BYTE_EXACT` list has 22 entries; 78 open).
  `bd10_photo_gate` is 191 cells (counted from the script's groups A-H:
  30+64+18+18+12+15+1+32+1); the 154 and 187 figures in `STATUS.md` are dated
  records and now say so. Every test-count tally the audit listed (669/669,
  873/873, 902/902, 915/915 x2, 864) carries `(as of <commit>)`, found with
  `git log -S`. `finishing-survey.md`, `bd10-port-map.md` and `ibc-port-map.md`
  open with a "line numbers as of <creation commit>; re-locate by symbol"
  header. The fresh-box README lists `cargo-nextest`, `just`, `aomdec`/`dav1d`
  and `tools/decode_diff` as the prerequisites cargo does not install.
  Still open from #8: whether to commit `rust/Cargo.lock` (a decision, not a
  doc fix), per-gate wall-clock budgets (unmeasured), the "landed work
  described as open" sections of the port maps, and the CI runner matrix
  (tracked under #4).
- **Encode speed: the port-vs-C per-pixel slope gap closes to 2.89x at presets
  10 and 13, 3.27x at preset 6, and — for the first time this campaign — 3.93x
  at preset 2** (from 3.06x / 3.07x / 3.39x / 4.14x). All 24 campaign cells
  byte-identical to C (`rust/benchmarks/perf_gap_2026-08-13-r1r2.meta`). Two
  byte-identical changes, and unlike everything before them these remove work
  whose result was **discarded**, not duplicated — the two top findings of
  `rust/docs/C-VS-PORT-CODE-REVIEW-2026-08-13.md`:
  - **R1: the inverse transform + reconstruction ran even where the
    reconstruction is thrown away.** C gates both on `mds_do_spatial_sse ||
    (!is_inter && tx_depth)` (product_coding_loop.c:4783-4784) and the all-intra
    derivation pins `spatial_sse_full_loop_level = 3`, so C inverts nothing at
    MDS1/MDS2; the port inverted unconditionally. A census measured the
    discarded share of inverse-transform pixel work at 40-50% (p10/p13), 36-50%
    (p8), 43-51% (p7), 28-53% (p6) and 24-44% (p2). Three call sites (MDS1
    luma, the CfL alpha search, the non-CfL chroma re-cost) now pass an explicit
    `need_recon = false`, each with an exhaustive-scan proof that the
    reconstruction is unread in its whole binding scope. 56d19efe1 — A/B 12/12
    cells 1.021-1.053x at qp40, and 28 of 28 cells below 1.0 across 6 presets x
    3 sizes x 2 qps against a control arm that split 13/15 (sign test
    p = 3.7e-9).
  - **R2: the exact coefficient rate was computed and then overwritten**
    wherever C's closed forms apply. C's rate tiers are an `if / else if /
    else` and the estimator is never reached on those arms
    (product_coding_loop.c:4914-4934, :5540-5564); the port called
    `cost_coeffs_txb` first and discarded it. Now evaluated in C's order.
    8179a7d94 — 1.038-1.060x at p10/p13 **qp20**, null at qp40/512+; the wall
    clock tracks the census share of replaced coefficient work (51-54% at qp20,
    16-38% at qp40, zero at qp55), which is what identifies the win as the
    mechanism rather than code placement.
  - the census instrument behind both, `leaf_funnel::txcensus` (cargo feature
    `__txcensus`, off by default, zero cost when off). 7dec5f24e.
- Preceding this, four byte-identical changes that took p10/p13 from 3.53x to
  3.06x, every one of them removing a duplicated COPY of something already
  computed rather than making an allocation cheaper:
  - the frame's block-decision set was materialised **four** times per frame —
    a leaf-level clone so the partition tree and a parallel `decisions` list
    could both own it, an aggregation of that list up the tree, a deep clone
    into a `per_tile_decisions` that was **written and never read**, and a deep
    clone of each superblock tree into its raster slot. Only the tree survives;
    `PartitionResult::decisions` is now populated by the legacy
    `partition_search` path alone and `num_blocks` comes from the new
    `PartitionTree::count_leaves` (29847e5d3, A/B 1.07-1.11x at p10).
  - `LeafEval::to_choice` deep-cloned seven of the winning candidate's buffers
    only because it ran *before* `commit_leaf`; both callers now commit first
    and `into_choice` moves (6ad044d00, A/B 1.02-1.03x at p10).
  - `funnel_block_decision`'s depth-0 qcoeff "unpack" was a byte-for-byte copy
    on every block without a 64-dim transform side, and
    `DecodedPictureBuffer::refresh` deep-cloned the whole picture once per set
    bit of `refresh_frame_flags` — eight full Y planes per KEY frame, into
    slots only ever read as `&ReferenceFrame` (now `Arc`-shared; the field is
    private and `store`/`get`/`refresh` keep their signatures, so no API
    change). 81a1bb111, A/B 1.01-1.02x at p10.
  - the per-SB reconstruction staging buffer (an allocation, a zero-fill and a
    second pass over every pixel of every superblock) is gone; **measured
    null**, kept only because it is strictly less work.
- **Measured negative, recorded so it is not retried**: a thread-local `Vec`
  pool for the mode-decision buffers removed a whole class of allocations from
  the profile (`drop_glue::<Cand>` 7.1% of malloc samples -> 0) and measured
  **null** at n=31 against an in-grid identity control. On macOS's xzone
  allocator the pool's machinery costs about what `malloc`/`free` costs at
  these sizes. `rust/benchmarks/alloc_bufpool_null_2026-08-13.meta` names the
  shape that is still unpriced (one construction-time arena the buffers are
  slices into, which is what the C reference does).
- **CI gates four more 8-bit surfaces**: partial-SB / odd dimensions (104
  cells), tiles across rows AND columns (29), SB128 (22), and panic-freedom on
  gradient AND screen (80). All four already failed loudly — they were simply
  never in the workflow.
- **`identity_run` reports a REFUSAL distinctly from a crash** (exit 3). It
  called the infallible `encode_frame*` wrappers, whose `.expect()` turned every
  deliberate out-of-envelope refusal into a panic; `arbitrary_size_robustness.sh`
  therefore reported 48 correct bd10 refusals as PANIC, unable to tell the
  port's best behaviour from its worst. That gate now reads 80/80 + 48 refused
  where it read 80/128, on identical encoder behaviour.
- **`tools/arbitrary_size_robustness.sh` now sweeps `screen` content as well as
  `gradient`, and adds sub-64 cells.** It previously ran gradient only, which
  never arms the screen-content detector — so palette and IntraBC were off in
  every cell and the gate could not reach the code paths they use. It ran
  straight past the `intrabc_hash` panics above. A panic-freedom gate that
  cannot arm half the encoder's tools is not a panic-freedom gate.

### Added

- **A comprehensive 8-bit byte-parity gate, and CI coverage for it**
  (`tools/identity_full_8bit.sh`). Until now there was **no 8-bit
  byte-vs-C identity gate in CI at any preset**: `identity_matrix.sh` is a
  scoreboard whose own header says "Exit 0 always", and it was not in the
  workflow either — so every 8-bit byte-identity claim, on the port's primary
  product surface, rested on hand-run measurements that nothing re-checked.
  The new gate exits nonzero, sweeps **every preset 0..13** (C clamps all-intra
  above M9 to M9 but the port does not, so 10..13 are distinct configurations
  here), carries low-q density where structural problems hide, covers
  partial-SB / odd / tiny / large geometry and four content classes including
  screen, pins divergences **self-promotingly** (a pinned cell that starts
  matching fails until promoted), and fails on harness errors so a cell that
  could not run can never look like a pass. `identity_matrix.sh` keeps its
  scoreboard role and gains `IM_STRICT=1` for gate use.


- **Native 10-bit input** (#6). `EncodePipeline::try_encode_frame_420_hbd` /
  `try_encode_frame_hbd` take real `u16` planes. The low 2 bits reach the mode
  decision, the coded levels, and the deblock / CDEF / Wiener searches — the
  port no longer widens an 8-bit source internally (35743ebd5, f319ec298).
  Gate: `tools/bd10_hbd_src_gate.sh`, 100/100 cells byte-identical to C.
- **Super-resolution**, opt-in via `EncodePipeline::with_superres(denom)` with
  `denom` in 9..=16, off by default exactly as in C (5c69edcb2, f4a1b7516,
  2f4d24cba, f319ec298, 174b0f184). Gate: `tools/superres_gate.sh`, 128/128
  cells checked three ways — byte-parity vs C, decodability at the upscaled
  size under the reference decoder, and anti-vacuity vs the non-superres stream.
  - `svtav1-dsp::superres` — the normative 64-phase upscale (was a 16-phase
    stub); `svtav1-dsp::resize` — the source downscale (new).
  - Sequence-header `enable_superres` + frame-header `superres_params()`.
  - C's stale full-resolution variance array, read through coded-grid indices,
    is reproduced deliberately (chunk B.4) — matching C requires it.
- `tools/bd10_hbd_src_gate.sh` and `tools/superres_gate.sh`, both wired into CI.
- `CONTEXT-HANDOFF.md` — build-from-scratch, gate, and open-work guide.

### Changed

- The test runner is `cargo nextest run` (CI and `just test`); each test gets
  its own process, which prevents archmage's process-wide dispatch-tier state
  from leaking between tests (d807fa0fe).
- Out-of-envelope configurations are REFUSED with
  `EncodeError::UnsupportedConfig` rather than silently encoding truncated or
  mis-scaled content (`hbd_source_consumed`, `superres_config_error`).

### Fixed

- **Partial-superblock RD mis-pricing: the cropped-TX distortion bound is now
  wired** (#95 chunk 2 (b)+(c)). On a frame whose aligned dims are not a
  multiple of 64, a coded TX block can straddle the frame edge; C prices only
  the part inside the ALIGNED frame (`cropped_tx_width`/`cropped_tx_height`,
  `Source/Lib/Codec/product_coding_loop.c:4664-4665` and `:5752-5754`;
  `cropped_tx_width_uv`/`_height_uv`, `full_loop.c:2228-2232`), while the port
  scored the whole block — so every boundary block was mis-priced. The
  already-written `frame_geom::cropped_tx_dims` (plus a new `cropped_tx_dims_uv`
  for C's chroma-domain expression) now feeds `leaf_funnel::tx_unit`,
  `tx_unit_hbd` and `txt_search`. The crop touches ONLY the spatial distortion
  kernels; the residual, transform, quantizer, RDOQ, recon and coefficient rate
  still run over the full TX block, exactly as in C.
  Measured crop-off → crop-on over 48 partial-SB cells: 8 changed bytes,
  **3 went divergent → byte-identical to C** (`gradient 80x88 / 104x88 / 72x88
  at q55 preset 6`, the straddle-win trio), **0 regressed**. Those three are now
  gated: `tools/partial_sb_gate.sh` 101 → **104/104**. Byte-neutral everywhere
  else (`identity_matrix` 54/54, `bd10_matrix` 36/36) — on a 64-aligned frame
  the crop is the identity. New differential test
  `leaf_funnel::tests::cropped_tx_distortion_matches_c_spatial_facade` pins the
  cropped distortion to the real exported
  `svt_spatial_full_distortion_kernel_facade` via `svtav1-cref`.
- `coeff_c_txb_init_levels_partial_zero_no_stale_reads` failed at default test
  parallelism: archmage token disabling is process-wide, so a sibling
  permutation test could move it onto the scalar arm. It now holds
  `lock_token_testing`, and 31 further dsp tests pin their tier the same way
  (d807fa0fe). No bitstream impact — every consumer reads only scan positions
  below `eob`.
- `perf_report` example declared `required-features = ["std"]`; a bare
  `cargo test -p zenav1-svt-dsp` previously failed to build it (f319ec298).

### Removed

- `svtav1_dsp::superres::{superres_upscale, superres_upscale_row}` — the
  non-normative 16-phase stub, replaced by the real kernel. No in-tree callers.

### Changed

- **`AvifEncoder::encode_y8` no longer pre-pads to a multiple of 64, and now
  REFUSES the case it used to paper over.** It padded the gray plane up to 64
  and built the pipeline AT THE PADDED SIZE while still returning
  `EncodedAvif::{width, height}` = the caller's TRUE size, so for every
  non-64-multiple gray image the AV1 frame and the announced frame disagreed —
  a 100x100 alpha plane came back as a 128x128 stream labelled 100x100. It now
  hands the pipeline the true dimensions. The residual is that below preset 6
  (speeds 1-4) the mono pipeline still refuses a PARTIAL superblock, which is
  now a typed `UnsupportedConfig` instead of a padded encode:
  `examples/decode_conformance.rs`'s avif corpus already had the `Err` arm and
  a comment saying refusing is the correct behaviour, and the pre-pad was what
  kept that arm dead. 16 of its 240 mono cells now refuse (the four
  non-64-multiple sizes at speed 1); 224 encode and all 224 decode.


## Earlier history

This file starts at 2026-07-24. Prior progress (the 8-bit byte-identity
campaign, chroma/4:2:0, deblocking, CDEF, Wiener restoration, palette, tiles,
arbitrary dimensions, the 10-bit MD path) is recorded per-feature in
`rust/docs/*.md` and in `rust/CLAUDE.md`'s status sections, with the commit
hashes cited inline there.

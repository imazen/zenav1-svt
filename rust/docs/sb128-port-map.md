# SB128 port map (task #91, extracted 2026-07-17 from v4.2.0 C)

Full whole-chain trace for super_block_size==128 on the allintra KEY bd8
4:2:0 path (real M0/M1). CORRECTIONS to earlier notes: the selection gate
is Globals/enc_handle.c:4071-4111 (not Codec/enc_handle.c:3992); SB128 ->
seq sb_size=BLOCK_128X128, sb_mi_size=32, sb_size_log2=5 (MI-based; PIXEL
log2 = +MI_SIZE_LOG2, entropy_coding.c:2457 — keep BOTH representations).

## The one architectural fact

SVT keeps TWO grids at all times (resource_coordination_process.c:1052):
a b64 grid ALWAYS 64x64 (ME/TF/variance/per-64 stats) and the sb grid
following super_block_size. Every SB128-specific fn bridges them:
- get_sb128_variance (enc_mode_config.c:119-142): AVERAGE of up to 4 b64
  cells, edge-clamped divisor.
- get_sb128_me_data (:62-114): AVERAGE for dists BUT **MAX for
  me_8x8_cost_var** (:83/:91/:100) — easy to mis-port as average.
- svt_aom_get_me_qindex (md_rate_estimation.c:1084): same 4-cell average.

## Geometry

- 128 partitions: H4/V4 GEOMETRICALLY illegal at 128 (no 128x32);
  HA/HB/VA/VB ARE legal. Table swap, not an if: ns_blk_offset_128_md
  (common_utils.c:269-270) + md_scan_all_blks part_it+2 offset
  (utility.c:242-267); consumer product_coding_loop.c:10893.
- Alphabet: svt_aom_partition_cdf_length (entropy_coding.c:922) = 8 at
  128 (EXT-2); partition_gather_*_alike skip H4/V4 probs iff 128
  (cabac_context_model.h:378-406); md_rate_estimation.c:80-129 builds
  SEPARATE ..._128x128_fac_bits tables for edge-SB has_rows/cols costs.
- GEOM set: M0/M1 allintra -> disallow_4x4 FALSE + allow_HVA_HVB=0 ->
  GEOM_9, max_block_cnt=4421 (enc_handle.c:4216-4223). At the 128 square
  the tested set is {N,H,V,S} — confluence of spec (no H4/V4) + M0/M1
  picture-wide allow_HVA_HVB=0. Sub-128 depths DO test H4/V4.
- mi grid: 32x32 mi cells/SB; mip alloc pcs.c:1030/:1070 (no coarsening
  at M0/M1 since disallow_4x4=false).
- PARTIAL 128 SBs exist on 64-aligned frames (e.g. 320px): sb_geom width
  64 < 128 -> max_block_size kept at sb_size (enc_mode_config.c:7046,
  SVT heuristic, distinct from spec has_rows/cols).

## Headers

- use_128x128_superblock bit: derived sb_size==BLOCK_128X128 at
  entropy_coding.c:2800 (the struct field :190 is DEAD — never written).
- Tile limits: max_tile_width_sb = 4096 >> PIXEL sb_size_log2
  (entropy_coding.c:2450-2467) — HALVED at SB128.
- LR header: the ">64" unit-size bit written only when sb_size==64
  (:2225-2236).

## MD walk — search algorithm PROVABLY IDENTICAL SB64 vs SB128 at M0/M1

- Root bsize = seq sb_size (md_process.c:497 setup_pc_tree); walk code is
  size-agnostic, only offset tables differ.
- pic_pd0_lvl: M0/M1 -> 0 EITHER WAY (enc_mode_config.c:8744-8745); the
  SB128 cap :8753-8755 is a no-op for M0/M1.
- pic_block_based_depth_refinement_level = 6 for M0/M1 (:10080-10089),
  PD0_DEPTH_ADAPTIVE s1/e1=15 limit_max_min=1 (:6903-6918) — NO sb_size
  branch. NOT a full-depth search (level 0 would be NO_RESTRICTION).
- nsq_search_level M0->3 M1->10 (:8363-8376), no sb dependency.

## Intra at 128

- Max TX 64: eb_max_txsize_lookup 128*->TX_64X64; tx_blocks_per_depth
  {4,4,4}; tx_org 128 = 4x (0/64,0/64) at EVERY depth; get_end_tx_depth
  = 0 for 128-variants (product_coding_loop.c:4100) — NO TXS search.
- CfL <=32 both dims (rd_cost.c:483 + entropy_coding.c:5035/:5171);
  filter-intra <=32 (mode_decision.c:102); palette <=64 (:4223) — all
  128-variants excluded. Angular has only the >=8x8 LOWER bound.
- svt_aom_intra_has_top_right SPECIAL CASE (intra_prediction.c:716-724):
  inside a 128 block, the txb whose top-right is the block CENTER does
  have top-right samples — per-64 seam availability must replicate.

## Loop filters (HIGHEST RISK = CDEF)

- DLF: y/x_range = 32 vs 16 mi (deblocking_filter.c:353/:479); clean.
- LR: luma unit size HARDCODED 256 (RESTORATION_UNITSIZE_MAX, pcs.c:29-41
  voids its params) — NOT sb-dependent, NO size search. Port: hardcode.
- CDEF three-phase contract (fb = 64x64 ALWAYS):
  1. SEARCH (cdef_process.c:352-392): a 128-variant block is ONE unit;
     stats computed ONLY at its top-left 64-quadrant; other quadrant fb
     slots (mse_seg/dir/var, b64-indexed) left STALE and skipped via the
     (fbc&1)/(fbr&1) bsize test.
  2. PROPAGATE (enc_cdef.c:874-893): the chosen strength is EXPLICITLY
     fanned out to every covered 64-quadrant grid slot keyed by bsize
     (128X128 -> +3 slots, 128X64/64X128 -> +1). Late-assigned field —
     mi-grid aliasing does NOT cover it (aliasing happens at MD time;
     cdef_strength is post-MD).
  3. APPLY (enc_cdef.c:392-455): every fb filtered INDEPENDENTLY; dlist
     recomputed per-fb with hardcoded BLOCK_64X64 (use_dlist_cache
     unconditionally false at SB128, :328-330); dirinit forced fresh for
     non-top-left quadrants (:446-455).
  4. write_cdef (entropy_coding.c:3986-4017): cdef_idx once per coded
     block; 64-based mask (:4001) + quadrant index (:4011) + per-SB
     cdef_transmitted[4].
  Miss any one -> encoder-internal recon != decoder recon (the aom-rs
  KB-1 #2 class; SVT's own :4071 comment records an SB128 r2r history).

## Pipeline state

- ec_ctx_array: sb_total_count entries; left(3)/top-right(1) averaging
  keyed on the SB grid (enc_dec_process.c:2866-2897) -> at SB128 the CDF
  seed refreshes ONCE PER 128 REGION = 4x coarser rate-CDF granularity
  feeding ALL RDO in the region. REAL behavioral delta.

## Port order (SB64 byte-identical after every chunk)

1 geometry tables (dead at SB64) -> 2 header plumbing -> 3 dual-grid
shims (variance avg + me_8x8_cost_var MAX) -> 4 MD root/alphabet
parameterization -> 5 intra/TX gates + has_top_right seam case ->
6 DLF ranges -> 7 LR (no-op if 256 already) -> 8 CDEF as its OWN
reviewed chunk with the 3-phase checklist + a synthetic propagate/apply
unit test -> 9 ec_ctx seeding parameterization -> 10 M0/M1 signal
confirmation tests -> 11 flip the Globals/enc_handle.c:4071 gate.
Audit the port's existing chroma sq_size<128 gates during chunk 5.
Baseline cells: benchmarks/real_image_identity_p01_2026-07-16.tsv (12).

---

# Reachability + port state (measured 2026-07-19, task #91 chunk 2)

## How an SB128 encode is REACHED (this changes the gate design)

There is **no `super_block_size` field in `EbSvtAv1EncConfiguration`** and no
`--sb-size` option in `SvtAv1EncApp`. C derives the value in
`Globals/enc_handle.c:4071-4111`, so the oracle cannot be *asked* for SB128 —
it can only be *steered into it*. Two clauses decide everything on the
allintra path:

1. **AREA.** `input_resolution == INPUT_SIZE_240p_RANGE` forces 64
   **unconditionally** (it is in the first, highest-priority clause). That
   bucket is `aligned_luma_area < INPUT_SIZE_240p_TH = 0x28500 = 165,120`
   (`Codec/definitions.h:1834`, classified by
   `svt_aom_derive_input_resolution`). It is classified on the **8-ALIGNED**
   dims: `enc_handle.c:3920` folds `max_input_pad_right/bottom` into
   `max_input_luma_width/height` before `:3992` derives the bucket.
2. **PRESET.** Inside the allintra branch only `enc_mode <= ENC_M1` picks
   128; presets 2..13 are SB64 at every frame size.

Also forcing 64: `resize_mode`, `rtc`, `sframe`, `fast_decode && qp<=56 &&
res>360p`, and **`enable_variance_boost`** — which the HDR fork defaults ON
(`enc_settings.c:1150`), so an sb128 x fork cell needs
`SVT_FORK_ENABLE_VARIANCE_BOOST=0`.

**MEASURED** with the real encoder, reading `use_128x128_superblock` back out
of the emitted sequence header (`tools/sb128_seqhdr.py`):

| request | aligned px | preset | `use_128x128_superblock` |
|---|---|---|---|
| 512x384 | 196,608 | 0 | **1** |
| 512x384 | 196,608 | 1 | **1** |
| 512x384 | 196,608 | 2 | 0 |
| 512x384 | 196,608 | 3 | 0 |
| 256x256 | 65,536 | 0 | 0 |
| 512x320 | 163,840 | 0 | 0 |
| 512x336 | 172,032 | 0 | **1** |
| 404x404 | 166,464 (pads to 408x408) | 0 | **1** |

**Consequence for gating: a 128x128 / 192x192 / 256x256 cell can NEVER
exercise SB128** — C forces SB64 there. Every cell in `tools/sb128_gate.sh`
is therefore >= 165,120 aligned luma samples at preset 0 or 1, and the gate
asserts the oracle's SH bit per cell so a mis-sized cell fails loudly instead
of silently re-proving the SB64 gate.

A second consequence: **SB128 is the DEFAULT for allintra M0/M1 at any real
image size.** Every existing gate cell is below the threshold, which is the
only reason the port has been correct so far — not because SB64 was the right
answer, but because no cell was ever big enough to ask the question.

## What C actually codes at the 128 root

Measured on `512x384 preset 0` by counting partition symbols in the C
`SVT_TRACE_OUT` op stream (`W CDF nsyms=8` is the 8-symbol 128-alphabet;
`icdf0=4869` is `PARTITION_CDF[16]`, i.e. bsl=4 ctx 16):

- **uniform, q32/q55/q63: exactly 12 ops, ALL `s=3` (PARTITION_SPLIT)** —
  one per SB in the 4x3 SB128 grid. C never keeps a 128x128 NONE here, even
  on dead-flat content at q63.
- gradient q32: 156 `nsyms=8` ops — **do NOT read this as 156 partitions.**
  On textured content the 8-symbol alphabet is shared with `eob_pt_128` (the
  eob-class symbol for 128-coefficient transform blocks: eob_pt_16..1024 are
  5,6,7,8,9,10,11 symbols, so the 128 class collides with the 128x128
  partition alphabet, exactly as `eob_pt_512` collides with the 10-symbol
  sub-128 partition alphabet). Separating them needs a real parse, not an
  op-census; only the FIRST op of the frame is unambiguous (it is SB(0,0)'s
  root, at the pristine `PARTITION_CDF[16]` = icdf0 4869), and it is `s=3`
  SPLIT on both uniform and gradient. **Whether C ever keeps a non-SPLIT 128
  partition on textured content is therefore UNVERIFIED** — measure it with a
  bitstream parse before assuming a forced-split implementation is
  sufficient beyond flat content.

For **uniform 512x384** the two op streams differ by *only* the 12 root
SPLIT symbols. Per-64-block both encoders emit the identical 5-op group
(`nsyms=10 s=0` PARTITION_NONE, skip, two `nsyms=13` mode symbols, one
`nsyms=3`), 48 blocks each (8x6 64-blocks). That makes uniform the cheapest
possible first byte-match target for the SB128 encode path.

## First divergence, as of this landing

`tools/identity_diff.sh 512 384 32 0 gradient` reports:

```
STAGE: SH | use_128x128_superblock C=1 Rust=0
ALSO: tile-op | op 0 C=CDF8:s3 Rust=CDF10:s3
VERDICT: NOT IDENTICAL (C=7828B Rust=7806B)
```

i.e. the sequence-header bit, and then immediately the very first tile
symbol: C codes the 128 root partition against the **8-symbol** alphabet
(`nsyms=8`, CDF row 16) while the port codes a 64 root against the
**10-symbol** alphabet (`nsyms=10`, CDF row 12).

## What IS wired (this landing)

- `sb128_geom::derive_super_block_size` — C's rule transcribed branch for
  branch, with `SbSizeInputs` for the force-64 knobs. Unit-tested against
  **every measured row of the table above** (not a transcription guess).
- `EncodePipeline::sb_size` / `sb_size_override` / `sb128_fallback` — the
  pipeline derives the SB size at construction. `SVTAV1_SB=64|128` pins it.
- `SeqTools::use_128x128_superblock` -> the SH bit (C derives it the same
  way, at write time, from `sb_size == BLOCK_128X128`,
  `entropy_coding.c:2800`).
- `resolve_tile_rows_log2_sb` / `tile_row_log2_limits(.., sb_size)` /
  `write_tile_info(.., sb_size)` / `write_key_frame_header_full_lr_sb` —
  the tile limits are SB-derived (`max_tile_width_sb` halves,
  `max_tile_area_sb` quarters at SB128). The 64px entry points are kept as
  compat shims so every existing caller is byte-identical by construction.
- **`EntropyCtx::bsl` 128 fix (a real bug).** `bsl` used to fold 128 into
  the 64 level via a `_ => 3` catch-all, which capped `partition_ctx` at
  ctx 15 and made ctx 16..19 — the only rows carrying the 8-symbol 128
  alphabet — unreachable dead code. A 128-wide node would have coded its
  partition symbol against the 64x64 CDF row with a 10-symbol alphabet:
  wrong probabilities *and* wrong alphabet length. Byte-neutral at SB64
  (no node is ever 128 wide there).

## What is NOT wired — remaining scope, in dependency order

`EncodePipeline::sb128_encode_supported()` returns **false**, so a cell C
would code at 128 falls back to a valid 64px-SB stream and sets
`sb128_fallback`. Deliberate: never a panic, never an undecodable stream,
and the fallback is reported on stdout and by the gate.

1. **MD root at 128.** `pd0.rs` is structurally 64-rooted: `ctx.pick(64,0,0)`
   at `:1741/:1807/:1862/:1926`, `SbVariance([u16; 85])` (the 1+4+16+64 tree
   for a 64 root) at `:69`, and `blk_var_map`'s `lvl = 6 - block_size.ilog2()`
   at `:143` **underflows** at 128. The cheapest correct first step is NOT a
   128-rooted variance tree: it is to build the 128 node as
   `Pd0Tree::Split([q0..q3])` over four existing 64-rooted decisions in
   Z-order, which is exactly what C chooses on all measured uniform cells.
   A true 128 NONE/H/V/AB search is a later chunk.

   **Keep each quadrant's variance map b64-rooted.** `encode_fixed_tree`
   takes `sb_vars: &SbVariance` plus `sb_org` and indexes the map by the
   node's offset RELATIVE to `sb_org` (`is_dc_only_safe(sb_vars, size,
   abs_x - sb_org.0, abs_y - sb_org.1)`). Handing it the 128 SB's origin
   with a 64-rooted 85-entry map would push three of the four quadrants past
   the end of the map. The recursion must enter each quadrant with THAT
   quadrant's own `compute_b64_variance` result and its own `sb_org` — which
   is also what C does: the b64 grid stays 64 at SB128, which is the entire
   reason the `get_sb128_*` bridge functions exist.
2. **`depth_refine.rs` caps** — `sq == 64` / `sq*2 == 64` at `:252-258`,
   `sq < 64` at `:330`.
3. **Partition WRITE at 128** — the ctx side is fixed (`bsl`), but the
   pack walk must emit the root symbol and recurse Z-order into the four
   64 quadrants.
4. **b64 <-> sb stat bridges** — `sb128_geom::sb128_variance` (AVERAGE) and
   `sb128_bridge_avg`/`sb128_bridge_max` (**MAX for `me_8x8_cost_var`**) are
   written but have no consumers. `pipeline.rs` indexes the variance-boost
   plan on the *sb* grid while producing it on the *b64* grid — those
   desync at SB128.
5. **CDEF, the highest-risk chunk** — the port's cdef_idx path is
   structurally one filter block per SB (`pipeline.rs` cdef_pending is a
   single slot). SB128 needs the full three-phase contract plus the
   4-quadrant `CdefTransmit` state machine and `cdef_strength_fanout_offsets`
   (both already written in `sb128_geom`, both unconsumed).
6. **DLF ranges** (32 vs 16 mi), **`intra_edge`'s hardcoded
   `MAX_MIB_SIZE_LOG2 = 5`** and the `svt_aom_intra_has_top_right` 128-centre
   special case, and **`ec_ctx` seeding** (which should follow automatically
   once `sb_cols` is 128-derived, but is unverified).

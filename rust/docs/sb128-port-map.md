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

---

# LANDED — the SB128 encode path (2026-07-19, task #91 chunk 3)

**12 of 14 `tools/sb128_gate.sh` cells byte-match real `SvtAv1EncApp`.** The
gate is 18/18 (4 SB64 controls + 12 byte-exact + 2 self-promoting pins).
Every SB64 gate re-verified byte-UNCHANGED after each commit:
identity_matrix 54/54, partial_sb 101/101, bd10_matrix 36/36,
bd10_nonflat 170/170, bd10_photo 112/112, hdr_bd10 46/46,
`cargo test --workspace` 58 suites green.

## The architecture that made it small

`sb128_geom::sb_coding_units` turns the map's "two grids" fact into the
walk: an SB is visited as its **b64 CODING UNITS** — one unit at SB64 (the
SB itself), up to four Z-order quadrants at SB128 with off-frame ones
dropped. Everything below that is the byte-proven per-64 machinery,
unchanged; `pipeline::merge_sb_units` folds the units back into one
`PartitionTree`, which at SB64 is the IDENTITY (one unit moved out). That
is what makes SB64 byte-identical *by construction* rather than by testing.

Per-SB state deliberately stays OUTSIDE the unit loop — notably the
`ec_ctx` chain, because C's `ec_ctx_array[sb]` really is SB-indexed. At
SB128 the rate-CDF seed therefore refreshes once per 128 REGION (4x
coarser), which §"Pipeline state" flagged as a real behavioural delta. The
structure gets it for free; MEASURED: port-SB64 and port-SB128 agree for
1714 coded decisions on `gradient 512x384 q32 p0` and then diverge, which
is that seeding difference showing up exactly where it should.

The entropy walk needed NO change: `encode_partition_tree` already derives
ctx/nsymbs from the node width (`EntropyCtx::bsl`, fixed in chunk 2) and
passes `is_128` to `write_partition_edge`, which selects the H4/V4-free
gathers at a frame edge. Both partial-SB128 shapes (a 128 COLUMN at 448
wide, a 128 ROW at 448 tall) byte-match on the first try.

## CORRECTION to §"What C actually codes at the 128 root"

The caveat "proven for uniform, **UNVERIFIED for textured content**" is now
**resolved: the 128 root is ALWAYS PARTITION_SPLIT on a KEY frame, and that
is STRUCTURAL, not a heuristic.** Verified first-hand in
`Codec/enc_dec_process.c:1483-1499` (`set_blocks_to_be_tested`):

```c
int max_sq_size = ctx->max_block_size;
if (pcs->mimic_only_tx_4x4)             max_sq_size = MIN(.., 8);
else if (static_config.max_tx_size==32) max_sq_size = MIN(.., 32);
else if (pcs->slice_type == I_SLICE)    max_sq_size = MIN(.., 64);
```

On an I_SLICE the largest square ever entered into the MD scan is 64x64
whatever the superblock size, so `BLOCK_128X128` is never an MD candidate
and the root has no codable outcome but SPLIT. (`ctx->max_block_size` is
itself `super_block_size` unconditionally at M0..M7 —
`enc_mode_config.c:7055-7080` sets `base_var_th_cap = (uint16_t)~0`, making
the `variance <= var_th_cap` test on a `uint16_t` a tautology; the I_SLICE
clamp is what decides.) SCOPE: I_SLICE only, which is the port's target. An
INTER frame is not clamped and would need a real 128-level NONE/HORZ/VERT
search — `debug_assert`ed in `merge_sb_units`.

**This collapses §5, "CDEF, the highest-risk chunk."** Search-skips-stale-
quadrants, strength fan-out, and forced-fresh dirinit all exist for
128-VARIANT coding BLOCKS, and on a KEY frame there are none. Every 64x64
filter block owns its own blocks and its own searched strength, exactly as
at SB64. `cdef_fb_is_stale_quadrant` and `cdef_strength_fanout_offsets`
stay unconsumed and correctly so. Only phase 4 (the WRITE) differs — see
below.

## The three defects found, in the order they bit

1. **lr_params unit-size bit.** `encode_restoration_mode`
   (`entropy_coding.c:2225-2236`) writes `unit_size > 64` ONLY when
   `sb_size == 64`; a restoration unit may never be smaller than the SB, so
   at SB128 the bit is implied. Writing it anyway shifted every following
   header bit and corrupted `tx_mode_select` / `reduced_tx_set`. Closed 2
   cells that were ALREADY tile-identical (4857 and 18818 ops, all matching)
   with one differing header byte, 0x00 vs 0x80.
2. **`seq_header.sb_mi_size` hardcoded to 16.** All six
   `intra_edge::has_top_right` / `has_bottom_left` call sites used the SB64
   value. Those index `mi & (sb_mi_size - 1)`, so a block at mi_col 16 reads
   as the SB's LEFT column under 16 but its RIGHT half under 32 — different
   top-right / bottom-left availability, different directional prediction.
   Threaded as data (`FunnelFrame` -> `UnitGeom` -> `DrGeom`, plus
   `PartitionSearchConfig` and a `build_directional_edges` parameter);
   defaults are 16 so SB64 is unchanged. **This is the only genuinely
   SB128-specific SUB-64 defect found.** Closed 2 cells.
3. **`cdef_idx` is per 64x64 FILTER BLOCK, not per SB** — and this one
   produced a **CORRUPT stream**, not merely a mismatched one. `write_cdef`
   (`entropy_coding.c:3986-4017`) latches `cdef_transmitted[4]` and emits at
   the first non-skip block of each quadrant, `index = !!(mi_col & 16) + 2 *
   !!(mi_row & 16)`. The port emitted one literal per SB, so the decoder
   read literals the encoder never wrote: aomdec rejected the SB128 stream
   with "Failed to decode tile data" at 6 of 10 qps on `gradient 512x384 p0`
   (the qps where `cdef_bits > 0`), while the same frame forced to SB64
   decoded at every qp. The strength lookup was wrong the same way —
   `fb_idx` is B64-indexed but was read with SB coordinates. Closed 1 cell.
   The gate now carries assert **(E) DECODABILITY** over every cell
   INCLUDING pinned ones, because a byte-comparison gate is structurally
   blind to this bug class: a pinned cell is expected to differ from C, so
   "differs" hid "is corrupt".

## The 2 formerly-pinned cells were NOT SB128 — CLOSED 2026-07-22

`gradient 512x384 q32 p0` and `gradient 448x384 q32 p0`. First divergence was
a PARTITION decision at a 32x32 node: C coded `s=9` (PARTITION_VERT_4) where
the port coded `s=0` (NONE), CDF row icdf0=14306. Measured three ways that it
was a pre-existing sub-64 leaf-cost near-tie, not SB128:

1. The port makes the same NONE decision under `SVTAV1_SB=64` and SB128 —
   the SB128 walk did not influence it.
2. `gradient 424x384 q32 p0` (162,816 px, BELOW the area threshold, so C
   codes it at **SB64**) reproduces it exactly: same node, same icdf row,
   C `s=9` vs port `s=0`. `gradient` is `((r*255)/h) ^ ((c*3)&0x3f)`, so at
   equal `h` the top-left block is bit-identical to the 512x384 cell's.
3. The port's own NSQ dump (`SVTAV1_NSQDBG=1 SVTAV1_DBG_MI=0,0`) shows V4 is
   EVALUATED, not gated: `shape=4 valid=0 part_cost=70131638` vs NONE's
   `69986899` — it LOSES by 0.207%.

### Root cause + fix (per-candidate leaf-RD dump)

C's `SVT_PICKPART_OUT` interposer (`CSQ`/`CNSQ` per tested shape) vs the port's
`SVTAV1_NSQDBG`/`SVTAV1_UVDBG` on the 424 SB64 repro pinned it to ONE term: the
VERT_4 divergence is ENTIRELY in the first 8x32 sub-block's CHROMA mode. C codes
UV_PAETH (uv_mode 12); the port coded UV_DC (0). Luma is bit-identical (same
mode 12, same dist 60592). C's V4 leaf-sum 69441889 + partrate 448023 =
69889912 < NONE 69986899 (C keeps V4); the port's UV_DC inflated that sub-block
by exactly 241726, pushing V4 to 70131638 > NONE (port kept NONE). The whole
0.207% was one wrong chroma mode.

WHY the port picked UV_DC: `search_best_independent_uv_mode`
(product_coding_loop.c:7518) scores its fast-loop candidates by
`ctx->mds0_ctrls.mds0_dist_type`, which is **NEVER assigned anywhere in
`Source/Lib`** (definitions.h:892 `enum { SAD=0, VAR=1, SSD=2 }`), so it stays
zero-initialized = **SAD** for every preset/bit-depth. The port's **bd8** arm
scored residual **VARIANCE** instead (the bd10 arm already used SAD). Variance
is DC-invariant, so on the flat 4x16 chroma of this 8x32 block many candidates
tied at 0 and pushed UV_PAETH — injected last — just past the `nfl=32` survivor
cut (rank 32); C's SAD keeps it inside the top 32.

**Fix:** `crates/svtav1-encoder/src/leaf_funnel.rs` — `residual_sad` (u8 SAD,
mirrors `residual_sad_hbd` / C `svt_nxm_sad_kernel`) replaces `residual_variance`
in the bd8 ind_uv fast loop. bd8-only; bd10 arm untouched. Promoted in
`tools/sb128_gate.sh` (SB128_BYTE_EXACT, now 18/18). Byte-inert on every green
cell: identity_matrix 54/54, sb128 18/18, partial_sb 101/101, tile 25/25,
arbitrary_size 57/57, bd10 {matrix 36, nonflat 307, photo 154, recon 13}, and
`cargo test --workspace`.

NOTE the 424 SB64 repro is NOT fully closed by this: 512/448 are multiples of
64 but 424 = 6*64 + **40**, so it has a partial-width edge SB (mi_col 96) with
its OWN separate luma partition near-tie (C VERT vs port NONE at the 16x16
mi(8,96)) that the real pins do not exercise. Orthogonal to this fix and to
SB128; left as-is.

## Remaining SB128 scope (honest list)

- **`av1_intra_luma_prediction`'s `multipler`** (`product_coding_loop.c:4027`):
  `(txb_origin_y % sb_size + tx_height * intra_size.left) > sb_size ? 1 :
  intra_size.left` bounds how many LEFT reference samples C memcpys. The
  port does not model it at all — and is byte-exact across 54+101+170+112+46
  SB64 cells, so the port's availability-driven sample count is evidently
  equivalent in the tested envelope. At SB128 the threshold doubles, so the
  clamp bites LESS often (C copies more), which is the safe direction.
  UNMODELLED and empirically inert; revisit if a directional SB128 cell
  diverges.
- **`tx_reset_neighbor_arrays`** (`product_coding_loop.c:4169+`):
  `MIN(bheight * 2, sb_size - org_y)` bounds the TX-depth-1/2 neighbour
  copy. Only live when `tx_depth > 0`. Not audited.
- **The b64<->sb stat bridges** (`sb128_variance`, `sb128_bridge_avg/max`,
  `svt_aom_get_me_qindex`) remain unconsumed. A survey of their C call sites
  concluded every one is dead or inert on the ALLINTRA KEY M0/M1 path; the
  one arm re-read first-hand is `get_max_block_size_allintra`, whose
  averaged variance is discarded at M0..M7 by the `(uint16_t)~0` cap AND
  masked again by the I_SLICE clamp above. The rest of that survey is NOT
  individually re-verified here — treat as a lead, not a fact.
- **The variance-boost plan** is still produced on the b64 grid and indexed
  on the sb grid (§4). Inert in mainline (`enable_variance_boost` FORCES
  SB64, `derive_super_block_size`), so an sb128 x fork cell cannot even
  exist today.
- **bd10 at SB128** is untested. `sb_mi_size` is threaded through the bd10
  re-encode chain so it carries no latent wrong constant, but no gate cell
  exercises bd10 x SB128.
- **INTER frames at SB128** need a real 128-level RD search (see the
  CORRECTION above). `debug_assert`ed, and inter is unported throughout.

---

# LANDED — whole-128-SB PD0 max/min for depth refinement (2026-07-22, task #91)

**codec_wiki 512 center-crop preset 0: q48 and q63 now byte-match real aomenc**
(were 1727 vs 1684 B and 450 vs 445 B). Root, verified with instrumented
sibling-C (`perform_pred_depth_refinement` dump, byte-inert):

C's `get_max_min_pd0_depths` (enc_dec_process.c:1943) walks the ENTIRE SB
pc_tree and derives `max_pd0_size`/`min_pd0_size` over ALL four 64x64
coding-unit quadrants, then feeds them to `set_start_end_depth`'s
`limit_max_min_to_pd0` gate (:1830-1846) for EVERY block in the SB. The port
computed them PER 64x64 coding unit (`build_refined_scan_at` did
`root.max_min_picked` on that one quadrant's PD0 eval).

On `codec_wiki` SB(0,0) the four quadrants' PD0 max/min were TL 32/4, TR 16/8,
BL 32/4, BR 32/8. C's whole-SB fold is **max_pd0=32 min_pd0=4** (confirmed by
the instrument); at a 16x16 node in the TR quadrant `set_start_end_depth` then
sets `s_depth=-1` (:1840, `sq*2==max_pd0` = 32) and TESTS the 32x32 parent. The
port's per-quadrant TR `max_pd0=16` instead hit `sq==max_pd0` (:1833) -> `s=0`,
so the 32x32 depth was NEVER tested and its nodes were force-split. C keeps a
32x32 HORZ at mi(0,24) (leaf-sum 177800141 < NONE 181492610 < the port's split);
the port, never evaluating it, split to 16x16. This is a DEPTH-PREDICTION scope
bug, not a leaf-cost near-tie: the port did not mis-score the 32x32, it never
searched it.

Only bites at SB128 (units.len() > 1); at SB64 the whole-SB fold equals the
single unit's own max/min, so the fix passes `None` there and is byte-identical
by construction. **Fix:** `pipeline.rs` folds the whole-128-SB PD0 max/min
across every coding-unit quadrant (the pure `pd0_pick_sb_partition_m6_eval`,
cached once per SB, full-64-units only for bounds-safety) and passes it into
`build_refined_scan_at` (new `sb_max_min: Option<(usize,usize)>` param).
Gate: `tools/sb128_gate.sh` gains `codec_wiki 512x512 q48/q63 p0` (SC_CORPUS,
local-only like coverage_combos). No regressions: sb128 20/20, identity 54/54,
coverage_combos 40/40, nextest 873/873, bd10 {matrix 36, nonflat 309, photo
154}, partial_sb 101, tile 29, arbitrary_size 57.

## Two RESIDUAL near-ties on codec_wiki (SEPARATE roots, NOT the depth scope)

`codec_wiki 512x512` after the fix: p0 {q32 DIFFERS, q48 OK, q63 OK}, p1 {q32
OK, q48 DIFFERS by 1 byte, q63 OK}. Both residuals are preset-specific RD
near-ties in a DIFFERENT layer than this depth-scope fix:

- **p0 q32** — a per-txb TX-TYPE near-tie at the 16x16 NONE mi(4,24). The port
  picks `ADST_ADST` (tx_type 3, eob 53) on the 3rd 8x8 txb where C picks
  `DCT_ADST` (2, eob 47); tx blocks 0/1/3 match C to the eob. That inflates the
  port's NONE rate (+20563) and dist (-9536) so its NONE cost 35270402 loses to
  its own VERT 33892339, while C's NONE 32696655 wins. SB-agnostic: diverges at
  p0 (SB128) but MATCHES at p1 (SB128) and p2/p3 (SB64), so it is the preset-0
  tx-type search settings tipping the tie on this content, not SB128.
- **p1 q48** — a 1-byte near-tie (port 1747 vs C 1748); a single symbol. Not
  drilled to the term.

Both are the tail of leaf-cost/tx-type RD ties, orthogonal to the SB128
partition/depth machinery. Next step for either: sibling-C per-txb tx-type RD
dump (the KB-2/KB-7 method extended to `tx_type_search`).

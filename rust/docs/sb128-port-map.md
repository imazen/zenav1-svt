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

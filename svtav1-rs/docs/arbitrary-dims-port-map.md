# Arbitrary dimensions port map (task #95, extracted 2026-07-17 from v4.2.0 C)

allintra KEY bd8 4:2:0 single-tile scope. THE model: TWO boundary systems
coexist — conflating them is the #1 porting risk.

| system | value | formula | consumers |
|---|---|---|---|
| ALIGNED (mi grid) | aligned_width/height | round TRUE up to mult of 8 (spec 7.2.6) | SB/mi grid, partition has_rows/cols, tx cropped-RDO bound, CDEF fb grid+dlist, tile info |
| TRUE/CODED (crop) | frm_size.frame_width (can be ODD) | aligned - pad_right | seq/frame header size fields, DLF plane bounds, LR unit sizing, recon output crop |
| SB-grid extent | ceil(aligned/sb)*sb | outer loop trip-count only | per-SB sb_geom clamped to ALIGNED |

## Input geometry
- MIN_BLOCK_SIZE=8 alignment (definitions.h:2034); the 8-align arithmetic
  EXISTS TWICE in C (enc_handle.c:3918-3930 + resource_coordination_
  process.c:717-729) — port ONCE.
- seq max_frame_width captured from the STILL-UNALIGNED value BEFORE
  alignment (enc_handle.c:4792-4799) — true dims flow to headers.
- pad_input_picture (pic_operators.c:561-604): replicate last col, then
  last row (incl. new right pad) — only up to 8-aligned, NOT sb.
- ppcs->frame_width == ALIGNED everywhere (pcs.h:1034 comment is STALE);
  only av1_cm->frm_size carries TRUE.
- CHROMA ROUNDING SPLIT (verify with odd-width differential!): ceiling
  (w+1)>>1 in pic_buffer_desc/restoration/app; DLF uses plain FLOOR >>1
  (deblocking_filter.c:167-168) — at odd true width DLF treats the last
  valid chroma column as out of bounds (under-filters). REPLICATE, don't
  fix.

## Headers
- frame_size_override_flag (entropy_coding.c:3373): FALSE for single-res
  allintra stills => per-frame size bits NEVER written; true size carried
  ONLY by seq header max_frame_width bits (:2760-2783, log2f+adjust,
  4-bit bits_minus_1 + literal minus_1).
- render_size: only when resize engaged — bit 0 for us.
- reduced_still_picture_header ONLY when config.avif (enc_handle.c:4964).

## Partial SBs / partition edges
- sb_geom width/height = MIN(ALIGNED - org, sb) => partial sizes always
  multiples of 8 (pcs.c:1535-1555). No is_complete_sb; B64Geom has
  is_complete_b64.
- encode_partition_av1 (entropy_coding.c:932-981) = spec 5.11.4:
  has_rows/cols vs ALIGNED dims; both false -> forced SPLIT, NO symbol;
  one false -> BINARY split-vs-H (or V) via partition_gather_*_alike.
  !has_cols => PART_V; !has_rows => PART_H (confirmed 4 sites).
- Search mirrors x3 (product_coding_loop.c:10538/:10618/:10896 + enc_dec_
  process.c:1394-1438 set_blocks_to_test: inj_hv_incomp EXCLUDES PART_N).
  Leaf blocks always fully inside ALIGNED — nothing codes half a block.

## MD at edges
- subres off at incomplete b64 (enc_mode_config.c:7327).
- end_tx_depth=0 for blocks touching the ALIGNED boundary
  (product_coding_loop.c:6710-6717).
- depth-removal disabled when sb_geom requires 8x8 coverage
  (enc_mode_config.c:3253-3264 dimensions_require_8x8); allintra
  disallow_8x8 unconditionally false (:8212).

## Filters at edges
- DLF: TRUE dims (spec 7.14.2), floor chroma (THE flagged discrepancy);
  outer SB loop bound = ALIGNED-based sb count.
- CDEF: ALIGNED grid throughout; per-fb nhb/nvb clamps vs mi_cols/rows;
  8x8 skip list bounded by ALIGNED; frame edge flags clamp halo reads.
- LR: TRUE dims; unit size always 256; count = round-to-nearest with
  FLOOR 1 => plane <=384px collapses to ONE unit (restoration.c:71-73).
- <64-in-either-dim inputs: restoration + AQ FORCE-DISABLED with warning
  (enc_settings.c:214-233, implementation limit) — replicate.
- Recon output crops to TRUE dims, ceiling chroma (app_context.c:114).

## TX at edges
- TX blocks NEVER skipped/cropped in the coding path — only the RDO
  DISTORTION metric crops (cropped_tx_width/height vs ALIGNED,
  product_coding_loop.c:4664 + full_loop.c:2228): affects which mode
  wins, not what's coded. txb ctx (get_txb_ctx) uses NOMINAL tx_size.

## Chunk order (64-aligned output byte-identical per chunk)
1 geometry plumbing (true-vs-aligned threading + sb_geom clamps; harness
stops rounding input dims) -> 2 headers (seq size bits off true dims) ->
3 partition edge rules (has_rows/cols + binary alphabets + no-symbol
case in the ONE port walk) -> 4 cropped-RDO distortion -> 5 filters
(DLF true+floor-chroma, CDEF aligned, LR collapse) -> 6 config side
effects (<64 disables). MILESTONE: 96x80 FIRST (8-aligned, no odd-chroma
ambiguity; exercises partial SB + CDEF clamp + LR collapse), THEN 65x65
(pressure-tests the DLF floor-vs-ceiling discrepancy — the highest-value
differential in the map). SvtAv1EncApp accepts arbitrary -w/-h directly.

## Gate design notes (measured 2026-07-18)

- The C side ALREADY handles 96x80: `capture_c_trace 96 80 32 6` on a
  synthesized I420 input exits 0 and emits a stream, so the milestone gate
  needs no C-side work — only the port's search restructure.
- **Use GRADIENT, not uniform, for the 96x80 cell.** Uniform-128 at q32/p6
  codes to just 25 bytes: everything is skip/NONE and the forced-split levels
  emit no symbols at all, so a uniform cell can pass while the edge coding is
  still wrong. (That same "forced splits code nothing" property is exactly what
  made the pre-guard mono partial-SB bug silent at 16x16 — see CLAUDE.md #95.)
  Gradient forces real partition decisions at the three edge SBs.
- 96x80 covers all three non-interior branches in one frame: SB(0,1) right
  edge (binary SPLIT-vs-VERT), SB(1,0) bottom edge (binary SPLIT-vs-HORZ),
  SB(1,1) both-false (forced SPLIT, no symbol). Unit-locked in
  `frame_geom::tests::edge_flags_match_c_rule_on_the_96x80_milestone`.

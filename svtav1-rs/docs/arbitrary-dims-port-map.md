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
  has_rows/cols vs ALIGNED dims (`hbs = (mi_size_wide[bsize]<<2)>>1`;
  `has_rows = org_y+hbs < aligned_h`, `has_cols = org_x+hbs < aligned_w`);
  both false -> forced SPLIT, NO symbol (`assert(p==SPLIT); return`);
  both true -> full `aom_write_symbol(p, partition_cdf[ctx], len)`;
  one false -> BINARY `aom_write_symbol(p==PARTITION_SPLIT, cdf, 2)`.
  OPTION mapping (spec): !has_cols(+has_rows) => SPLIT-vs-VERT (PART_V);
  !has_rows(+has_cols) => SPLIT-vs-HORZ (PART_H). **GATHER-FN TRAP (VERIFIED
  2026-07-18, cabac_context_model.h:378/393): the gather is CROSS-named vs the
  option** — the `!has_rows` case (option HORZ) uses `partition_gather_VERT_alike`
  (entropy_coding.c:972); the `!has_cols` case (option VERT) uses
  `partition_gather_HORZ_alike` (:976). Each gather sums out {its-dir, SPLIT,
  the two A-variants of its dir + one of the other, its _4 unless 128} from the
  full partition_cdf into a 2-symbol cdf. Do NOT pair "horz option -> horz
  gather"; it is crossed. ctx = `(left*2+above) + bsl*PARTITION_PLOFFSET`,
  `bsl = mi_size_wide_log2[bsize] - mi_size_wide_log2[BLOCK_8X8]`, above/left
  from the partition-context neighbour array `>> bsl & 1`. Sub-8x8 (bsize <
  BLOCK_8X8) writes NO partition symbol. The port's `frame_geom::edge_has_rows_cols`
  (VERT/HORZ doc) is spec-correct; only the pack CONSUMER + the search remain.
- Search restriction — `set_blocks_to_test` (enc_dec_process.c:1394-1438,
  VERIFIED 2026-07-18): `hbs = mi_size_wide[bsize]>>1` (MI units), `has_rows =
  mi_row+hbs < av1_cm->mi_rows` (ALIGNED grid), `has_cols = mi_col+hbs < mi_cols`.
  * `!has_cols && !has_rows` -> `tot_shapes=0` -> FORCED SPLIT (no shape tested).
  * exactly one false AND (NSQ disabled OR `sq_size <= max(min_nsq(=4, or 8 in
    PD1 non-REGULAR), nsq_geom_ctrls.min_nsq_block_size)`) -> `tot_shapes=0` ->
    FORCED SPLIT (an incomplete block too small for NSQ must split).
  * otherwise (`inj_hv_incomp = !has_cols||!has_rows`): the loop over
    PART_N..max_part SKIPS every shape except the one edge shape —
    `if (has_cols && part!=PART_H) continue; if (has_rows && part!=PART_V) continue;`
    => bottom edge (`!has_rows`, has_cols) tests ONLY {PART_H}; right edge
    (`!has_cols`, has_rows) tests ONLY {PART_V}. **PART_N is excluded** (it fails
    both guards), so an incomplete block never evaluates NONE — only its single
    H/V shape, with SPLIT implicit via the recursion. Matches the coding
    (H<->!has_rows, V<->!has_cols) above.
  So chunk-2 search = at a partial node, restrict the shape set to {single H|V}
  (or {} => forced split) per the rule; the pack then codes it with the binary
  gather (cross-named!) or the no-symbol forced split. Leaf blocks always land
  fully inside ALIGNED — nothing codes half a block.
- Three search mirror sites: product_coding_loop.c:10538/:10618/:10896.

### CHUNK 2 — LANDED (2026-07-18, preset 6 / bd8 4:2:0)

96x80 (the milestone) byte-matches real aomenc, plus 96x64, 96x96, 64x80,
80x96, 200x120, 48x48, 88x56, 72x72 — `tools/partial_sb_gate.sh` = 11/11.
The full-SB identity matrix stays 54/54 (+ bd10 36/36, bd10-nonflat 8/8):
every change is byte-neutral where the frame has no incomplete SB.

Two CORRECTIONS to the model above, learned by cmp-verified iteration:

1. **The boundary shape is NOT forced-split; it is an edge-shape-vs-SPLIT
   decision made in PD0, and at the fixed-tree presets it is DETERMINISTIC**
   (md_disallow_nsq_search => `set_blocks_to_test` injects exactly one shape).
   C's LPD0 (product_coding_loop.c:127) prices "a single block per shape ...
   PART_H/PART_V for boundary blocks", i.e. the NON-SQUARE in-frame block
   (`size x size/2` HORZ, `size/2 x size` VERT) — NOT the square PART_N with a
   cropped distortion. The square block over-costs (twice the pixels/coeffs)
   and wrongly loses to SPLIT: the port's `pd0::lvl1_block_cost_rect` prices
   the edge shape, so a PD0-leaf boundary node keeps its edge shape exactly
   where C does (e.g. 96x80 bottom-edge 32-node -> HORZ 32x16; the right-edge
   64-root's VERT 32x64 loses to SPLIT, matching C).
2. **VERT boundary blocks are TALLER than wide** (`sq/2 x sq`), so the PD0
   transform needs Tx32x64 / Tx16x32 / Tx8x16 (+ the 32x64 height-fold), which
   the wide-only subres table lacked. Without them a right-edge node panics.

Coding: a one-false PD0 leaf codes its single in-frame block through
`leaf_funnel::decide_leaf_rect` (evaluate_leaf/commit_leaf are already
dimension-general) and a `PartitionTree::Split{Horz|Vert, [1 child]}`;
`encode_partition_av1` writes the binary SPLIT-vs-{H,V} symbol (the gather is
cross-named per the trap above) and the single block (the pack's Horz/Vert
arms code child 1 only when it exists). Off-frame quadrants are `Pd0Tree::Off`
(coded as nothing) and pruned by position in both the fixed-tree encode and
the entropy walk (C `svt_aom_write_modes_sb` early-return / SPLIT `continue`).

3. **The map's "leaves land fully inside ALIGNED" is FALSE in general — C
   CODES straddling boundary blocks** (verified 2026-07-18 via C `PICKPART`).
   For 96x80 the edge shapes fit exactly, but at most partial dims a boundary
   block reaches PAST the aligned extent: e.g. an 88x56 frame's 32x32 NONE at
   (64,0) reaches x=96 (aligned_w=80), and an 80x88 frame's chroma TX reaches
   past the aligned chroma plane. A node can even have `has_rows && has_cols`
   BOTH true (not a spec edge) yet still straddle (a 64x64 SB root of a 48x56
   frame). C codes those blocks reading the SB-extent pad and CROPPING the RDO
   distortion (cropped_tx_width/height). The port must therefore NOT force-
   split straddling nodes (that regresses byte-exact cells where the straddle
   is harmless, e.g. 88x56) — it sizes the recon working buffers + chroma
   source to the SB-extent PRODUCT (aligned stride; a right-straddle write
   wraps into slack rows, a bottom-straddle lands in extra rows — never OOB).
   Panic-free across `{40..192}^2 x qp{20,32,40,55}` preset 6 (400/400); every
   straddle cell decodes under aomdec.

### ODD TRUE DIMS — LANDED (2026-07-18, task #95 goal 1)

Odd -w/-h are now byte-comparable and 11 odd partial-SB cells are gated (26/26
in `partial_sb_gate.sh`; identity_matrix stays 54/54, bd10 36/36 + 8/8). Two
changes, both byte-neutral for even/8-aligned dims (ceiling == floor there):

1. **Harness ceiling chroma** (identity_run.rs + capture_c_trace.c): both sides
   feed CEILING chroma ((w+1)/2) for the flat-chroma synthetic content, matching
   AV1 4:2:0 + the port's `encode_frame_420` intake. C internally reads FLOOR
   chroma (luma_width>>1) from the ceiling-strided buffer, inert on flat chroma.
2. **LR search TRUE dims** (pipeline.rs + restoration.rs): `search_restoration_
   still` + `write_lr_for_sb`/`corners_in_sb` ran on ALIGNED dims, but C
   `whole_frame_rect` (restoration.c:51-62) uses TRUE luma / CEILING chroma and
   reads the recon at the aligned buffer stride while `extend_frame` replicates
   the TRUE edge (never the aligned padding). The port now extracts tight
   true/ceil buffers from the aligned-strided recon+source for the search. This
   fixed the odd-HEIGHT FH `lr_type[0]` divergence (C=WIENER, Rust=NONE) that the
   7 aligned-padding rows caused.

GATED odd cells: 65x64/65x63/71x64/73x64/81x64 (odd width, right-edge partial),
73x73 (odd both, aligned 80x80), 63x96/63x48 (odd width + ≥32-tall bottom
partial), 63x63 (odd both, full SB = odd header + true crop, no partial SB).

### PD0 BOUNDARY-NODE COST — FIXED (2026-07-18)

The partial-SB PD0 partition near-tie was TWO real bugs in the boundary
edge-shape node cost (pinned via a new `SVT_PD0COST` interposer on C's
`svt_aom_full_cost_pd0`, unit-comparing C's PD0 costs against the port's
NSQDBG PD0 dump — the port's 8x8 children + dist already matched C; only the
16x8 edge-shape ybits and the split rate were off):

1. **Rectangular tx-type rate** (`pd0::TxTypeRatesDc::rate_for`):
   `av1_transform_type_rate_estimation` charges the intra tx-type bit via the
   tx's SQUARE-MAPPED CDF row; the port returned it only for the 3 SQUARE PD0
   sizes and 0 for rectangular transforms, so every 16x8/8x16 edge shape was
   748 bits too cheap (TX_16X8 -> its square TX_8X8's rate). Fix: map rect tx
   through `TXSIZE_SQR_MAP` (0 only when DCT-only, sqr_up >= TX_32X32).
2. **Binary boundary split rate** (`pd0::pick` + `context::partition_alike_
   split_cost`): `svt_aom_partition_rate_cost` prices SPLIT at a one-false node
   with the BINARY `partition_{vert,horz}_alike_fac_bits[ctx][SPLIT]` (bottom ->
   vert_alike, right -> horz_alike, CROSS-named), NOT the full-alphabet
   `partition_fac_bits[ctx][SPLIT]` the port's `split_bits` used.

With both fixes the port's PD0 costs match C EXACTLY. Unblocked EVERY
single-edge partial cell at EVERY qp (64x65 / 64x72 / 72x64 + all odd-width
right-edge) AND the straddle-win cells at low-mid qp (80x88 / 104x88 / 72x88
q<=32, 80x104 / 104x80 all qp) — the straddle cells were NOT an uncropped-
distortion problem after all; they shared this boundary-cost root. Gate:
`partial_sb_gate.sh` 37/37; identity_matrix 54/54; bd10 36/36 + 8/8; both fixes
partial-SB-only (rect tx / one-false nodes never occur 64-aligned).

### PRESETS >= 9 (M9 LPD0 / PD0_LVL_5/6) — FIXED (2026-07-18, commit 8813a12e7)

The "boundary nodes fall back to the square cost" hypothesis was WRONG. The
higher-LVL boundary path had TWO LPD0-only roots, both byte-neutral for full
SBs and for the LVL_1 presets 0-6 (drilled 96x80 p10 with `SVTAV1_PD09` port
dump + the C `SVT_PD0COST`/`SVT_FULLCOST` interposers):

1. **subres forced OFF on an INCOMPLETE b64** (`pd0::pd0_pick_sb_partition`).
   C `enc_mode_config.c:7326` sets `subres_level = 0` when `!is_complete_b64`.
   The port applied subres (step 1, `is_subres_safe` set only by the 64x64
   odd/even check) on partial SBs, so a LVL_5 16x16 block computed a subres
   (half-height) transform with dist=215398 vs C's full-res 143032 -> over-split
   16x16 -> 8x8. Fix: seed `is_subres_safe = 0` (determined, not safe) on any
   SB whose 64x64 extent reaches past ALIGNED, so step stays 0.
2. **one-false boundary nodes are FORCED SPLIT at LPD0** (`pd0::pick`). NSQ geom
   is DISABLED for allintra `enc_mode > M6` (`svt_aom_get_nsq_geom_level_allintra`
   returns level 0 => `enabled = 0` for preset >= 7; enc_mode_config.c:8240). So
   `set_blocks_to_test` yields `tot_shapes = 0` for a one-false node — C never
   injects the edge shape; a thin 8-wide/8-tall edge descends straight to the
   fitting 8x8s (C `SVT_PD0COST` at a 72x64 thin right edge shows ONLY 8x8). The
   port priced the boundary node as a SQUARE and kept a coarser VERT/HORZ shape
   (under-split). Fix: force-split one-false nodes for Lvl5/Lvl6 (both-false
   already did); the SPLIT rate feeding a straddling parent is the binary alike
   (2x at LVL_5, 0 at LVL_6 allintra), via the new
   `context::partition_alike_split_symbol_cost`. LVL_1 (preset <= 6) keeps NSQ
   enabled -> the injected edge shape, unchanged. NOTE this is a preset>=7 (M7)
   boundary, NOT M9 — presets 7/8 (LVL_1 path) also have NSQ disabled, but their
   partial-SB cells are not yet gated (a latent item: the `one_false && Lvl1`
   edge-shape branch is only correct for preset <= 6).

Result: the FULL p9/p10/p13 gradient partial-SB sweep byte-matches — set 1
(single-edge/multi-SB, 9 dims) 108/108, set 2 (odd/bottom/straddle, 14 dims)
168/168 — AND the both-partial 65x65/65x72/65x80 (which still diverge at p6 =
the PD1 near-tie below) match at p9+. `partial_sb_gate.sh` 37 -> 55 (18 new
p9/p10/p13 cells). identity_matrix 54/54, bd10 36/36 + 8/8, no-panic sweep
1936/1936 clean.

REMAINING (diverge, NOT in the gate — all DECODABLE):
- **BOTH-partial PD1 intra MODE near-tie — preset 6 ONLY** (aligned 72x72:
  65x65 / 65x72 / 65x80). SHARPENED op-trace (65x65 q32 p6, mi=(16,4) = the
  16x8 boundary leaf at pixel (16,64); C `SVT_FULLCOST` + `SVT_PICKPART`
  interposers + port `SVTAV1_CANDDBG`/`PREDDBG` dumps):
  * Tree MATCHES C; the ONLY divergence is the luma MODE — C V_PRED (mode 1,
    eob 28, DCT_DCT), port DC (mode 0). NOT a partition/PD0 bug.
  * At MDS1 C's V beats DC (cost 6617701 < 9348297) so V survives to MDS3 and
    wins (ycb 16258, eob 28); the port's V LOSES to DC (10676372 > 9562657) and
    is pruned before MDS3. Both use DCT_DCT (C `full_loop_core` non-txt path,
    product_coding_loop.c:4441), so NOT a tx-type diff.
  * The port's V_PRED costs coeff_rate=45833 vs C's ycb=22722 for a *similar*
    dist (11396 vs 12380). C's eob 28 = a COMPACT residual -> its above
    reference must carry the horizontal sawtooth. But the port's encoder recon
    at row 63, x16-23 is **column-FLAT 243** (rows 56/60/62/63 = 214/232/240/243
    across ALL of x0-23), and the port's OWN decoded output there is ~197, not
    243. So the port predicts V_PRED from a recon reference that does not match
    its own bitstream — a cross-SB ENCODER-RECON MISMATCH at the SB(0,0) bottom
    boundary feeding the wrong V_PRED reference. NOT a coeff-rate/txt_rate bug
    (the earlier "PD1 txt_rate handles rect tx" note stands).
  * NEXT DRILL: the SB(0,0) block covering (x16-23, row 63) — why the port's
    encoder recon there is column-flat where the bitstream/decoder is not
    (the tree-join for this SB showed the recon "matching" but the reference
    is a DIFFERENT SB's bottom row; suspect a cross-SB recon plumbing / write
    ordering issue for the aligned-72 frame, or a lost horizontal residual in
    the block above). This is preset-6-specific; presets 9+ match (the lighter
    PD1 does not make V a contender at that leaf).

## MD at edges
- **b64 VARIANCE on a partial SB reads PAST the aligned extent — do NOT
  "fix" this with a clamp (measured 2026-07-18).** `compute_b64_variance`
  (pic_analysis_process.c:318) is called per b64 with `input_padded_pic` and
  NO clamping (:611, inside `compute_picture_spatial_statistics`) — it always
  walks a full 8x8 grid of 8x8s. The documented input pad,
  `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions` ->
  `pad_input_picture` (pic_operators.c:561), only extends TRUE -> ALIGNED
  (`scs->pad_right/pad_bottom`, a multiple of MIN_BLK_SIZE = 8), NOT to the
  64-wide SB grid. So on a partial SB C reads beyond ALIGNED into whatever the
  picture buffer's border padding holds. The port's `pd0::compute_b64_variance`
  carries the same unclamped 64x64 walk and a note that "every current caller
  pads frames to 64-aligned dimensions".
  CONSEQUENCE: clamping the port's reads to the frame would compute DIFFERENT
  variance than C and therefore different PD0 partition decisions — it would
  break byte-exactness at partial SBs rather than enable it. The port must
  reproduce C's padded CONTENT out to the SB extent instead.
  RESOLVED (2026-07-18, full source trace): the region `[aligned, SB-extent)`
  is **edge/replication of the TRUE edge** — deterministic, in-bounds, NOT
  uninitialised. C reaches it in two steps into the SHARED y8b luma buffer
  (`input_padded_pic->y_buffer` == `enhanced_pic->y_buffer` == `buff_y8b`,
  resource_coordination_process.c:1016/1193/1320; the pa_ref desc allocates no
  pixels, reference_object.c:248-250) BEFORE the variance runs:
  (1a) `pad_input_picture` (pic_operators.c:561) replicates the true edge
  col→row into `[true, aligned]` (pad_right/bottom, mult of MIN_BLK=8), then
  (1b) `svt_aom_generate_padding` (pic_operators.c:434, called at
  pic_analysis_process.c:1555 — BEFORE the :2000 `svt_aom_gathering_picture_
  statistics`) edge-replicates the 8-aligned buffer over the full
  `border = BLOCK_SIZE_64+4 = 68` px (enc_handle.c:4256), which covers the b64
  grid (the (64,64) b64 reads only 32px past aligned in x, 48 in y — both < 68).
  Net content of `[true, ext)` = the TRUE edge pixel (corner = `(true_w-1,
  true_h-1)`). LUMA ONLY matters (`compute_b64_variance` reads `y_buffer`
  alone, pic_analysis_process.c:333). **Do NOT clamp `compute_b64_variance`.**
  DONE: `frame_geom::pad_input_plane(plane, dims, sb)` now edge-replicates the
  true edge out to `dims.sb_ext_w/h(sb)` (horizontal-then-vertical, corner-
  correct) — C-faithful, unit-tested on the 96x80 milestone + a full-SB no-op
  case, byte-neutral (the fn is not yet wired into the pipeline; the current
  full-SB matrix is unaffected). REMAINING chunk-2 WIRING: allocate the input
  plane at the SB extent (`sb_ext_w`-strided), call `pad_input_plane` after
  ingestion, and pass the ext stride to `compute_b64_variance` — then the
  partial-SB PD0 variance matches C and the 96x80 search restructure can land.
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

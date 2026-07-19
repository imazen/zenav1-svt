# Luma palette pipeline — port-ready map (task #71, extracted 2026-07-16 from v4.2.0 C)

Whole-chain trace for the allintra KEY bd8 4:2:0 path. Chroma palette is DEAD
in SVT (palette_size[1] hard-0 at injection mode_decision.c:3372; UV mode bit
written literal 0 entropy_coding.c:4365; k_means_dim2 has zero callers).
bd8 => hbd_md=0 always (enc_handle.c:4394 + enc_mode_config.c:2476-2483):
search reads enhanced_pic 8-bit, palette_bit_depth=8, svt_av1_count_colors
(8-bit variant).

## SEARCH (palette.c:388-530 search_palette_luma)

- Gate: svt_av1_allow_palette(md_palette_level, bsize) (mode_decision.c:3348)
  called from generate_md_stage_0_cand :3589. md_palette_level = raw level int.
- Injection (inject_palette_candidates :3356-3406): per produced size, cand =
  DC_PRED + intrabc 0 + DCT_DCT + fi NONE + uv DC; class = CAND_CLASS_3
  (:3646-3660; NIC class th 50, enc_mode_config.c:6775).
- Dims via svt_aom_get_block_dimensions (rows/cols WITHIN-BOUNDS clipped;
  block_width/height nominal). Color gate: count_colors in (1,64] else zero
  candidates. max_n=min(colors,8), min_n=2.
- A) Dominant-color (dominant_color_step != 0xFF): top_colors by repeated
  argmax of count_buf (ties -> LOWEST value), n from max_n down by step;
  seed = prefix; palette_rd_y(opt_colors=false, cache=NULL).
- B) K-means (kmean_color_step != 0xFF): n_cache = svt_get_palette_cache_y
  ONCE outside the loop. colors==2 -> centroids={lb,ub} no iteration. Else
  uniform seed centroids[i]=lb+(2i+1)(ub-lb)/n/2 then av1_k_means dim1.
  palette_rd_y(opt_colors=true, cache).
- K-means exact (k_means_template.h:25-114): calc_indices strict `<`
  (tie -> FIRST/lowest index); calc_centroids DIVIDE_AND_ROUND(sum,count)
  (round-half-up), empty cluster -> reseed data[lcg_rand16(&state)%n], LCG
  seeded from data[0]; main loop: snapshot, recompute, REGRESSION rule
  (this_dist > pre_dist -> restore+break), CONVERGENCE (centroids unchanged
  -> break). max_itr per level: 50(l1) / 2(l2-6) / 1(l7-9).
- Color cache (palette.c:164-210): above/left mbmi palette_colors sorted
  2-way merge w/ dedup (ties advance both, above wins); ABOVE DROPPED at SB
  row tops (row % 64 == 0 -> above_mi NULL). index_color_cache (:111-141):
  per cache entry first-exact-match; returns not-found colors packed.
- palette_rd_y final (:296-325): optimize_palette_colors (snap centroid to
  nearest cache color if diff <= (6+(qp_index>>6))<<(bd-8)); qsort ascending
  + dedup (k<2 -> reject); clip+store; AUTHORITATIVE map recompute via
  av1_calc_indices against the FINAL sorted list (tie -> lower color value);
  extend_palette_color_map replicates last col/row for edge-clipped blocks
  (extension never rate-charged).

## RD (single computation point!)

- svt_aom_intra_fast_cost (rd_cost.c:579-605), gate =
  svt_aom_allow_palette(allow_screen_content_tools,bsize) && mode==DC:
  palette_ymode_fac_bits[bctx][mctx][use_palette]; if used:
  + palette_ysize_fac_bits[bctx][n-2]
  + write_uniform_cost(n, color_map[0])            (the (0,0) pixel)
  + svt_av1_palette_color_cost_y (cache flags: n_cache bits; first literal
    bd bits; 2-bit bits-per-delta indicator; monotone-shrinking delta bits
    = delta_encode_cost palette.c:80-109, MUST share code with the writer)
  + svt_av1_cost_color_map (wavefront over anti-diagonals EXCLUDING (0,0),
    rate += palette_ycolor_fac_bits[n-2][color_ctx][color_new_idx]).
- full cost (rd_cost.c:1417) just reuses fast_luma_rate + real coeff_rate.
  NO palette re-derivation at MDS3.
- color_new_idx is the RANK-REMAPPED index (av1_fast_palette_color_index_
  context, palette.c:612-743): neighbors {left,top,topleft} scores {2,2,1},
  merge equal colors, sort (score desc, value asc), remap current index
  against that ranking; ctx = 9 - hash via lookup {-1,-1,0,-1,-1,4,3,2,1}.
- Prediction = per-tx-block color substitution (enc_intra_prediction.c:
  631-651), then the ORDINARY tx/quant/recon pipeline (no identity-tx
  shortcut); tx_type subject to the generic intra tx search.

## PACK (write_modes_b I_SLICE order, entropy_coding.c:4968-5104)

skip -> cdef -> dq -> [intrabc] -> y mode -> chroma mode -> PALETTE MODE
INFO (:5026, y flag + size cdf + colors when DC) -> filter-intra ->
PALETTE MAP TOKENS (:5052-5077, tokenize + pack_map_tokens: write_uniform
for (0,0) then aom_write_symbol per token with palette_y_color_index_cdf
[n-2][ctx], nsymbs = n) -> tx_size -> coeffs. Colors writer
(delta_encode_palette_colors :4244-4276) bit-identical to the cost fn.

## STATE

- mi stamping already landed (update_mi_map). get_palette_mode_ctx =
  above+left palette_size>0 counts.
- CDF rows: palette_y_size_cdf[7][7+1], palette_y_color_index_cdf[7][5][8+1]
  (effective nsymbs = n!), + the landed y/uv mode CDFs. Defaults
  cabac_context_model.c:477-547. avg_cdf rows enc_dec_process.c:2595-2609
  (AVG_CDF_STRIDE with nsymbs=j+2 for the color-index rows).
- TWO independent CDF tracks: MD-side ec_ctx_array chain (rates only;
  update_palette_cdf md_rate_estimation.c:733-759 advances it per finalized
  block) vs the REAL pack CDF (reset to defaults at packetization, adapts
  inside aom_write_symbol). Port equivalents: fun/sim ectx chain + frame_ctx.

## GATES

- set_palette_level (enc_mode_config.c:1841-1915) table:
  lvl: enabled/dom_step/kmeans_step/centroid_ref/max_itr
  2: 1/2/1/0/2   3: 1/off/1/0/2   4: 1/off/2/0/2   5: 1/off/3/0/2
  7: 1/off/5/0/1     (allintra-reachable set = {0,2,3,4,5,7})
- allintra: sc_class5 gate; M0-M2->2 M3->3 M4-M5->4 M6->5 M7->7 M8+->0.
- Light-PD1 NEVER engaged on allintra (pic_lpd1_lvl=0 unconditional,
  enc_mode_config.c:10081) -> palette injection always reachable via the
  regular md_encode_block path; PD0 never injects palette.
- IBC interaction (mode_decision.c:3587-3620): at intrabc palette_hint=1
  (levels 2..7 = M0-M4 sc allintra), IBC injection for a block is SKIPPED
  when the palette search produced zero candidates. Search-space coupling —
  must be replicated.

## FFI-parity surface (exported T symbols)

search_palette_luma, svt_av1_allow_palette, svt_aom_get_palette_{bsize,mode}
_ctx, svt_get_palette_cache_y, svt_av1_index_color_cache,
svt_av1_palette_color_cost_y, svt_av1_cost_color_map,
svt_av1_tokenize_color_map, svt_av1_count_colors, svt_av1_k_means_dim1_c/
_avx2, svt_av1_calc_indices_dim1_c/_avx2.

## Port order

1. Pure math: index_color_cache, delta_encode (ONE shared routine for
   cost+writer), count_colors, k_means_dim1 + calc_indices, remove_
   duplicates, optimize_palette_colors, extend_palette_color_map. FFI parity
   per fn.
2. Map context/tokenization: fast_palette_color_index_context (+edge) +
   wavefront cost/tokenize. FFI parity vs cost_color_map.
3. search_palette_luma + palette_rd_y (neighbor stub + PaletteCtrls).
4. RD integration: fac-bits from CDFs, fast-cost palette block, class-3.
5. PACK: mode-info n>0 arm + colors + map tokens in the block order.
6. State wiring: mi palette stamping (exists), per-SB chain rows, rates.
7. Gates glue: level table + palette_hint coupling (with IBC vertical).

## RISK (validate at runtime)

AVX2 k-means (`assert((n & 15)==0)` in _avx2, palette_avx2.c:159) is called
with edge-clipped rows*cols NOT guaranteed %16 — SIMD strides past n read
stale scratch and can perturb this_dist -> accepted centroids on right/
bottom-edge blocks. Real-world C output = AVX2 path. If an edge cell
diverges with a clean-C-algorithm port, replicate the AVX2 stride behavior
(or prove the perturbation never flips acceptance). 512-aligned CID22 has
no partial blocks — the risk activates on non-multiple-of-8 dims only.

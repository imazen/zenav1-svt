# `Codec/entropy_coding.c` — per-function port map

**Written 2026-08-31 by the `wx-entropy` lane.** It answers one question for
each of the 191 function definitions in `entropy_coding.c`: *is there a Rust
counterpart, and where?*

Read this INSTEAD of `tools/c_surface_inventory.py`'s row count for this file.
That tool matches by NAME and says so; on this file the name match is a poor
proxy, because the port deliberately renames (`encode_skip_coeff_av1` ->
`write_skip`), inlines (`write_cdef` into `encode_block_syntax`) and replaces
by design (every ctor/dtor). On 2026-08-31 the tool reported **59 / 191
matched, 132 missing**. Auditing all 132 by hand gives:

| verdict | count | meaning |
|---|--:|---|
| ported under another name, or inlined into a caller | **118** | a Rust counterpart exists and was read |
| not translatable — SVT allocation / pooling / buffer plumbing | **13** | replaced by Rust ownership, counted OUT of the queue |
| still unported | **0** | — |

So the file's real state is **178 of 191 ported, 13 replaced by design, 0
gaps**. Do not take those numbers on trust:
`tools/entropy_coding_coverage.py` recomputes them from the live inventory
plus the mapping, and exits nonzero if any row is unclassified.

**"Ported" here means a counterpart exists and was compared — it does NOT mean
byte-gated.** §4 says which groups have which evidence tier, and that is the
number to quote when someone asks whether this file is finished.

## 1. The 13 that are NOT translated, and why (named first)

| C | :line | reason |
|---|---|---|
| `svt_aom_entropy_coding_context_ctor` | — | SVT object-system ctor; the port has no `EntropyCodingContext` object. |
| `svt_aom_entropy_coder_ctor` | 1285 | ctor for a POOLED `EntropyCoder`; `AomWriter` owns its buffer. |
| `entropy_coder_dctor` | 1275 | dtor for the same. |
| `svt_aom_entropy_tile_info_ctor` | 1242 | ctor for `EntropyTileInfo`. |
| `entropy_tile_info_dctor` | 1237 | dtor for the same. |
| `svt_aom_bitstream_ctor` | 1255 | ctor for `Bitstream`. |
| `bitstream_dctor` | 1250 | dtor for the same. |
| `svt_aom_bitstream_reset` | 1261 | `Vec::clear` on an owned buffer. |
| `svt_aom_bitstream_get_bytes_count` | 1265 | `Vec::len`. |
| `svt_aom_bitstream_copy` | 1270 | `extend_from_slice`. |
| `svt_aom_reset_entropy_coder` | 1226 | re-seeds a pooled coder object the port does not pool. |
| `svt_aom_encode_slice_finish` | 1218 | `AomWriter::done` is the arithmetic-coder half; the SVT `Bitstream` flush around it has no object to flush. |
| `tx_size_to_depth` | 4579 | an adapter from `TxSize` BACK to a depth. The port stores `tx_depth` directly (C's `mbmi->block_mi.tx_depth`), so the inverse this computes is already the port's representation. |
| `get_vartx_max_txsize` | 4422 | folded into `vartx.rs`'s walk, which starts from the block dims rather than looking the max TX up. |

**Two corrections this audit made to its own first draft**, kept here because
they are the exact failure mode `WORKING-ON-THIS.md` §5 warns about — a
plausible verdict reached without opening the file:

* `block_signals_txsize` (:4418) was first written down as "not translatable,
  a one-line gate applied at each call site". It is genuinely PORTED, in
  `leaf_funnel/tx_geom.rs::block_signals_txsize`. Found only because the
  inventory started matching it by name.
* `svt_aom_entropy_coding_context_ctor` was first written down as a row of
  this file. It is defined in `Codec/ec_process.c:35` and is not in
  `entropy_coding.c` at all.

## 2. Where the 118 renamed / inlined ones live

### 2a. Renamed

| C | Rust |
|---|---|
| `av1_get_skip_context` :983 | `entropy/context.rs::get_skip_context` |
| `encode_skip_coeff_av1` :995 | `entropy/context.rs::write_skip` |
| `encode_partition_av1` :932 | `entropy/context.rs::write_partition` / `write_partition_edge` |
| `encode_intra_luma_mode_kf_av1` :1026 | `entropy/context.rs::write_intra_mode_kf` |
| `encode_intra_luma_mode_nonkey_av1` :1046 | `port_entropy_inter/modes.rs::encode_intra_luma_mode_nonkey` |
| `encode_intra_chroma_mode_av1` :1077 | `entropy/context.rs::write_uv_mode` |
| `encode_skip_mode_av1` :1109 | `port_entropy_inter/modes.rs::encode_skip_mode` |
| `av1_get_skip_mode_context` :1097 | `port_entropy_inter/modes.rs::skip_mode_context` |
| `av1_write_delta_q_index` :3967 | `entropy/mv_coding.rs::write_delta_q_index` |
| `write_selected_tx_size` :4630 | `entropy/context.rs::write_tx_depth` |
| `get_tx_size_context` :4594 | `pipeline.rs::EntropyCtx::tx_size_ctx` |
| `set_txfm_ctx` / `set_txfm_ctxs` :4559/:4566 | `pipeline.rs::EntropyCtx::record_txfm_dims` |
| `get_sqr_tx_size` :4470 | `vartx.rs::sqr_tx_size_of_dim` |
| `txfm_partition_update` :4453 | `vartx.rs::VarTxWalk::update` |
| `av1_code_tx_size` :4649 / `code_tx_size` :4746 | `vartx.rs::write_tx_size_vartx` (inter arm) + `pipeline.rs` (intra arm + the context stamp) |
| `pack_map_tokens` :4343 | `entropy/context.rs::write_palette_map_tokens` |
| `delta_encode_palette_colors` :4256 | `entropy/context.rs::write_delta_encoded_colors` |
| `svt_aom_get_palette_bsize_ctx` :4228 | `entropy/context.rs::palette_bsize_ctx` |
| `svt_aom_get_palette_mode_ctx` :4240 | `port_entropy_inter/primitives.rs::palette_mode_ctx` |
| `svt_aom_allow_palette` :4223 | `entropy/context.rs::allow_palette` |
| `svt_aom_allow_intrabc` :4401 / `svt_av1_encode_dv` :4381 | `intrabc.rs` |
| `av1_get_mv_joint_diff` :1482 | `entropy/mv_coding.rs` (the `MvJointType` derivation) |
| `av1_write_tx_type` :317 | `entropy/coeff_c.rs::write_tx_type_intra` + `write_tx_type_inter` |
| `av1_write_coeffs_txb_1d` :355 | `entropy/coeff_c.rs` |
| `av1_encode_coeff_1d` / `_tx_coef_y` / `_tx_coef_uv` | `pipeline.rs` + `entropy/coeff_c.rs` |
| `ec_update_neighbors` :4161 | `pipeline.rs::EntropyCtx`'s update methods |
| `svt_aom_write_modes_sb` :5426 | `pipeline.rs::encode_partition_tree` |
| `write_modes_b` :4935 | `pipeline.rs::encode_block_syntax` (intra branch) + `port_entropy_inter/block.rs::write_inter_mode_info` (inter branch) |
| `loop_restoration_write_sb_coeffs` :4102 | `entropy/lr.rs` + `restoration.rs` |
| `mem_put_varsize` :32 | `entropy/obu.rs::put_varsize` |
| the six `svt_aom_wb_*` bit-buffer entry points | `entropy/obu.rs::BitWriter` |
| `aom_wb_write_primitive_{quniform,subexpfin,refsubexpfin}` | `port_entropy_inter/gm.rs::wb_write_primitive_*` |
| `svt_aom_encode_sps_av1` / `_td_av1` | `entropy/obu.rs::write_sequence_header` / `write_temporal_delimiter` |
| `svt_aom_write_frame_header_av1` :3848 | `entropy/obu.rs::write_key_frame_header*` / `write_inter_frame_header` |
| `svt_aom_write_metadata_av1` :3809 | `port_entropy_inter/metadata.rs::write_metadata_obus` |
| `svt_aom_get_kf_y_mode_ctx` :1004 | `port_entropy_inter/primitives.rs::kf_y_mode_ctx` |
| the tile-geometry four (`svt_av1_get_tile_limits`, `_calculate_tile_cols`, `_rows`, `svt_aom_set_tile_info`) | `entropy/obu.rs::TileGrid` |
| `svt_av1_reset_loop_restoration` :4019 | `restoration.rs` |
| `svt_av1_update_segmentation_map` :4847 | `entropy/context.rs` (`SegmentationMap::update`) |

### 2b. The inter context / CDF-selector family

Every `svt_a{om,v1}_get_pred_{cdf,context}_*`, `get_pred_context_*`,
`svt_aom_get_{reference_mode,comp_reference_type}_{cdf,context_new}`,
`svt_aom_collect_neighbors_ref_counts_new`,
`svt_aom_get_comp_{index,group_idx}_context_enc`,
`svt_aom_get_pred_context_switchable_interp` and `av1_is_interp_needed` lives
in `port_entropy_inter/{refframe,modes,interp}.rs`.

### 2c. Inlined into a caller

`write_cdef` :3986, `encode_cdef` :2338, `encode_loopfilter` :2278,
`encode_quantization` :2368, `encode_restoration_mode` :2183,
`encode_segmentation` :2247 (`entropy/obu.rs::write_segmentation_params`),
`write_delta_q` :2359, `write_profile` :2671, `write_bitdepth` :2676,
`write_color_config` :2686, `write_render_size` :2616, `write_superres_scale`
:2631 (`SuperresParams::write`), `write_frame_size` :2652,
`write_bitstream_level` :3698 and `set_bitstream_level_tier` :111
(`compute_seq_level_idx` + `does_level_match`), `write_tile_info_max_tile`
:2403 (`entropy/obu.rs::write_tile_info`), `write_tile_group_header` :3770
(`entropy/tile.rs::write_tile_group`), `write_frame_header_obu` :3789,
`write_sequence_header_obu` :3704, `write_uncompressed_header_obu` :3299,
`write_uleb_obu_size` :3661.

`svt_aom_write_uniform_cost` :4308 and `svt_aom_uleb_size_in_bytes` :1310 were
in this bucket until 2026-08-31. They are now named functions in
`port_entropy_inter/primitives.rs` for one reason worth generalising: **a
named function can be gated at tier 1 and an inlined expression cannot.** When
an inlined C helper is also an EXPORTED symbol, promoting it out of its caller
buys a differential.

## 3. What this lane added on 2026-08-31

| new | C | evidence |
|---|---|---|
| `port_entropy_inter/compound.rs` | `write_modes_b` steps 7 + 9 (:5245-5343) and the three compound predicates | tier 1 / tier 1-header for the predicates, tier 4 for the two writers |
| `port_entropy_inter/block.rs` | `write_modes_b`'s inter mode-info WALK (:5196-5343) | tier 4 (the walk is `static`); its inputs are tier 1 |
| `port_entropy_inter/neighbors.rs` | `set_mi_row_col` (:4681), `max_block_wide` / `_high` (:4431/:4442) | **tier 1** for `set_mi_row_col` — it is exported |
| `port_entropy_inter/primitives.rs` | `svt_aom_{uleb_size_in_bytes, write_uniform_cost, get_palette_mode_ctx, get_kf_y_mode_ctx}` | tier 1 |
| `port_entropy_inter/metadata.rs` | `write_obu_metadata` (:3683), `svt_aom_write_metadata_av1` (:3809), `add_trailing_bits` (:3673) | tier 4; every primitive under them is tier 1 |
| 11 new tier-1 gates | the `AomWriteBitBuffer` primitives, `uleb_encode`, `count_primitive_quniform`/`_subexpfin`, the skip / palette / kf-y-mode contexts, `partition_cdf_length` | all previously UNGATED |

## 4. Evidence tiers (`docs/WORKING-ON-THIS.md` §4)

| group | tier | gate |
|---|---|---|
| the inter context / CDF-selector family | 1 | `tests/c_parity_entropy_inter.rs` |
| `set_mi_row_col` | 1 | `tests/c_parity_entropy_block.rs` |
| the `AomWriteBitBuffer` primitives, `uleb_encode`, `uleb_size_in_bytes` | 1 | `tests/c_parity_entropy_block.rs` |
| `count_primitive_quniform` / `_subexpfin` | 1 | `tests/c_parity_entropy_block.rs` |
| skip / palette / kf-y-mode contexts, `partition_cdf_length`, `write_uniform_cost` | 1 | `tests/c_parity_entropy_block.rs` |
| the compound / interintra predicates | 1 (exported) and 1-header (`static INLINE`) | `tests/c_parity_entropy_compound.rs` |
| the intra branch, end to end | 2 | the byte-identity matrix |
| `write_modes_b`'s inter WALK, the interintra + compound-type WRITERS, `max_block_wide/high`, the metadata OBUs | 4 | module unit tests, hand-derived vectors traced against the C source |

**Why so much of this file can only be tier 4:** `write_modes_b`,
`write_modes_sb`'s leaf writers and every `write_*` helper in the block group
are `static` in `entropy_coding.c`, and `shims/ref_shims.c` never compiles
that file — so no shim can reach them. Their INPUTS are tier 1; what stays
tier 4 is the branch structure on top. A byte gate arrives when the inter
frame path is wired at `pipeline.rs`'s `if !is_key` guard.

## 5. Reproducing the count

```
rust/tools/entropy_coding_coverage.py
```

It re-runs `c_surface_inventory.py`, classifies every row it calls MISSING
against the mapping above (held as data in the script), and prints the table
in §0. It **exits nonzero when any row is unclassified** — which is the whole
point: a new C function, or a port rename this audit has not caught up with,
shows up as an unclassified row instead of quietly disappearing into a stale
total. It also lists entries that are classified but no longer reported
missing, so redundant rows can be pruned.

Run it in the same change as any edit to this file's port surface.

## 6. Two C shapes recorded, not fixed

* `write_obu_header` (:3644) writes `obuExtension & 0xFF` as one 8-bit
  literal; `entropy/obu.rs::write_obu_header` writes `3 + 2 + 3` zero bits for
  that field. Byte-identical for the only value the port ever passes
  (extension absent) and unreachable otherwise — but they are not the same
  function, and a future caller that sets the extension would diverge.
* `port_md_rate_estimation.rs`'s `WEDGE_PARAMS_BITS` doc says "exactly the ten
  block sizes"; the table it documents has NINE non-zero entries (8x8, 8x16,
  16x8, 16x16, 16x32, 32x16, 32x32, 8x32, 32x8), which the tier-1 sweep in
  `tests/c_parity_entropy_compound.rs` now confirms against C. The table is
  right; only the count in the prose is wrong. Not fixed here because that
  file belongs to another lane.

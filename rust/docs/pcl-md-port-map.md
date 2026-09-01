# `product_coding_loop.c` — mode-decision port map (lane `wx-pcl`)

Written 2026-08-31. **Read this before trusting a `MISSING` row for
`Codec/product_coding_loop.c` in `tools/c_surface_inventory.py`.**

That tool matches by NAME (`fn <name>` in non-test, non-cref source) and
says so. This port deliberately does not transliterate C's names when a
clearer one exists, so a `MISSING` row here can mean any of four things.
This file says which, per function.

## 1. Why the inventory under-counts this file

After the `wx-pcl` lane, `product_coding_loop.c` reads **90 MISSING of
127**. The real picture:

| bucket | count | meaning |
|---|--:|---|
| name-matched | 37 | the tool sees them |
| **ported under a different name** | **11** | §2 |
| **ported in PART, deliberately** | **6** | §3 |
| pre-existing counterpart in the intra path | ~40 | §4 |
| not translatable — plumbing this port replaces | ~19 | §5 |

The buckets in §4 and §5 overlap the 90 and are approximate because they
were not audited one by one; §2 and §3 are exact and were written by the
lane that produced them.

## 2. Ported under a different name — EXACT

| C (`product_coding_loop.c`) | Rust |
|---|---|
| `av1_txt_rate_est` `:4553` | `port_md::tx_gates::txt_rate_source` |
| `lpd1_should_perform_tx` `:6329` | `port_md::lpd1::should_perform_tx` |
| `lpd1_blk_skip_luma_rd` `:6417` | `port_md::lpd1::blk_skip_luma_rd` |
| `lpd1_chroma_energy_skip` `:6453` | `port_md::lpd1::chroma_energy_skip` |
| `update_skip_nsq_based_on_split_rate` `:9710` | `port_md::nsq_skip::skip_by_split_rate` |
| `update_skip_nsq_based_on_sq_recon_dist` `:9847` | `port_md::nsq_skip::skip_by_sq_recon_dist` |
| `update_skip_nsq_shapes` `:9982` | `port_md::nsq_skip::skip_by_shapes` |
| `update_skip_nsq_based_on_sq_txs` `:10063` | `port_md::nsq_skip::skip_by_sq_txs` |
| `get_skip_processing_nsq_block` `:10352` | `port_md::nsq_skip::skip_processing_nsq_block` |
| `md_stage_3_light_pd1` `:7119` | `port_md::lpd1_loop::md_stage_3_light_pd1_settings` |
| `compute_lpd0_cost_inter` `:8267` | `port_md::coding_loop::lpd0_inter_candidate_walk` |

`derive_ssim_threshold_factor_for_tx_type_search` `:4578` likewise maps to
`port_md::coding_loop::ssim_threshold_factor_for_tx_type_search` (that one
predates this lane).

## 3. Ported in PART, deliberately — EXACT

Each of these is a C function whose DECISIONS are translated and whose
buffer / DSP operations are not, because the port owns those elsewhere.
**Do not read them as done.**

| C | translated | NOT translated |
|---|---|---|
| `lpd1_try_mds0_bypass` `:8939` | the five preconditions (`lpd1_loop`… `lpd1::globalmv_bypass_allowed`) | the synthetic GLOBALMV candidate injection, its prediction and its pricing |
| `full_loop_core_light_pd1` `:6541` | `lpd1_loop::{plan_chroma, luma_tx_skipped, luma_skip_rd_applies, second_chroma_detector_runs, no_chroma_epilogue}` | the residual/transform/quantise chain, the chroma full loop, the full cost, the predictor — and therefore their call ORDER |
| `perform_dct_dct_tx_light_pd1` `:5434` | `lpd1_loop::{luma_eob_zero_takes_the_early_exit, luma_tx_skipped}` | the transform, the quantiser, the distortion and the coefficient rate |
| `read_refine_me_mvs_light_pd1` `:2737` | `lpd1_loop::{lpd1_me_mv_index, lpd1_me_mv_to_eighth_pel, lpd1_skip_subpel, MdMeDist}` | the sub-pel search, the MV predictor, the reference scaling (all ported elsewhere) |
| `update_redundant` `:10267` | `nsq_skip::redundant_shape_source` | the block-data / recon / coefficient copies |
| `md_encode_block` `:9343` | its MDS0..MDS3 STAGE LOOP `:9459-9640` (`port_md::md_stages::run_md_stages`) | everything else in that function |

## 4. Already had a counterpart before this lane

Roughly forty rows are the all-intra funnel's versions, living in
`leaf_funnel/` (`cfl.rs`, `mds1.rs`, `mds3.rs`, `txt.rs`, `tx_geom.rs`,
`nic.rs`, `predict.rs`), `pd0.rs`, `depth_refine.rs`, `md_subpel.rs` and
`port_md/{md_search,motion_mode,pme,coding_loop}.rs`. Examples confirmed by
reading while this lane worked: `av1_cost_calc_cfl`,
`compute_cfl_ac_components`, `check_best_indepedant_cfl` and
`cfl_prediction` (`leaf_funnel/cfl.rs`); `svt_aom_get_blk_var_map`,
`compute_lpd0_cost_allintra`, `full_loop_core_pd0`, `perform_tx_pd0`
(`pd0.rs`); `calc_scr_to_recon_dist_per_quadrant`, `test_depth`,
`test_split_partition` (`depth_refine.rs`); `get_end_tx_depth`,
`non_normative_txs` (`leaf_funnel/tx_geom.rs`); `get_tx_type_group`,
`search_dct_dct_only` (`leaf_funnel/txt.rs`);
`post_mds{0,1,2}_nic_pruning` (`leaf_funnel/nic.rs`).

**Those are the INTRA specialisations.** Where this lane re-ported one it
is because the general form is a different function off an I-slice, and
each such module's header says exactly which arms only exist there — see
`port_md::nic_prune`, `port_md::nsq_skip` and `port_md::tx_gates`. The
intra versions are untouched and remain the ones the still-image encoder
uses.

`get_sb_tpl_inter_stats` `:2946` is an exception in this bucket: it is
REFERENCED by `port_md::coding_loop` (which records that
`use_best_references == 2` needs it) and is **not implemented anywhere**.

## 5. Not translatable — this port replaces it rather than translating it

One line each, per the campaign rule that a non-translatable function is
counted OUT of the queue with a reason rather than reported as missing.

| C | why not |
|---|---|
| `svt_aom_init_sb_data`, `init_block_data`, `product_coding_loop_init_fast_loop` | per-SB / per-block context INITIALISATION of C's `ModeDecisionContext`; the port has no such object |
| `svt_aom_move_blk_data`, `move_blk_data_redund`, `copy_txt_data`, `init_tx_cand_bf`, `update_tx_cand_bf` | candidate-buffer bookkeeping in C's fixed buffer pool |
| `md_update_all_neighbour_arrays`, `md_update_all_neighbour_arrays_multiple`, `mode_decision_update_neighbor_arrays`, `mode_decision_update_neighbor_arrays_pd0`, `update_neighbour_arrays`, `update_neighbour_arrays_pd0`, `svt_aom_copy_neighbour_arrays`, `tx_initialize_neighbor_arrays`, `tx_reset_neighbor_arrays`, `tx_update_neighbor_arrays`, `tx_search_update_recon_sample_neighbor_array`, `update_part_neighs` | C's neighbour-array machinery; the port carries neighbour state in its own entropy/recon contexts |
| `copy_recon_md`, `copy_recon_light_pd1`, `av1_perform_inverse_transform_recon_luma`, `convert_md_recon_16bit_to_8bit`, `pad_hbd_pictures` | recon buffer copies / bit-depth conversion / padding |
| `md_rtime_alloc_palette_info` | runtime allocation |

## 6. Evidence tiers in this lane

Two functions in this C file are EXPORTED (`nm -g
Bin/Release/libSvtAv1Enc.a`, no `svt_aom_` prefix, no header prototype) and
are therefore **tier 1**:

* `sort_full_cost_based_candidates` `:1438` —
  `crates/svtav1-encoder/tests/c_parity_pcl_nic.rs`
* `chroma_complexity_check_pred` `:6013` —
  `crates/svtav1-encoder/tests/c_parity_pcl_lpd1.rs`

Both drive the real symbol through `crates/svtav1-cref/shims/pcl_shims.c`.
Everything else this lane ported is `static` in C and is **tier 4**.

Two things the tier-1 work paid for, recorded so nobody re-pays them:

1. **`svt_aom_mefn_ptr` needs `init_fn_ptr`, which is NOT part of the RTCD
   setup.** The variance arm of `chroma_complexity_check_pred` dispatches
   through `svt_aom_mefn_ptr[bsize].vf`; that table is a COMMON symbol
   zeroed at load and populated only by `init_fn_ptr` (av1me.c:26), which is
   exported but separate. The shim SIGSEGV'd until it called it — and it
   must run AFTER `svt_aom_setup_rtcd_internal`, because it copies the
   `svt_aom_variance<W>x<H>` dispatch pointers that setup writes. This is
   `WORKING-ON-THIS.md` §5 trap 2, one level lower than the trap's own
   example.
2. **A random-content differential grid can be completely vacuous and
   green.** The first `c_parity_pcl_lpd1.rs` drew random planes at four
   spreads over eight geometries and four priors — 1,024 cases — and every
   one returned `COMPONENT_LUMA`: with independent random input and
   prediction no chroma SAD reaches twice the luma SAD, and a CONSTANT
   plane has variance ZERO by construction (`variance_c` subtracts
   `sum^2/n`). Mutating the port's `y_dist << 1` to `<< 2` and its variance
   threshold 150 to 75 left all 1,024 passing. The grids are now CONSTRUCTED
   to straddle each threshold and every test asserts an outcome census
   before its green counts. Six mutations now fail.

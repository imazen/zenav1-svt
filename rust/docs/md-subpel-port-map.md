# MD sub-pixel search + the full_loop / md_rate_estimation / coding_loop leftovers

Lane `wp-search`, 2026-08-31. Module group: `Codec/mcomp.c`, `Codec/full_loop.c`,
`Codec/coding_loop.c`, `Codec/md_rate_estimation.c`.

This is a STATUS record of what landed, at which evidence tier, and what is
still missing. Every claim below was run, not inferred.

## What landed

| C file | function | line | Rust | tier |
|---|---|---|---|---|
| mcomp.c | `svt_mv_err_cost` | 42 | `md_subpel::mv_err_cost` | 1 |
| mcomp.c | `svt_mv_err_cost_` | 74 | `md_subpel::MvCostParams::err_cost` | 1 |
| mcomp.c | `svt_get_subpel_part` | 99 | `md_subpel::get_subpel_part` | 1 |
| mcomp.c | `svt_get_buf_from_mv` | 106 | `md_subpel::get_buf_from_mv` | 1 |
| mcomp.c | `svt_upsampled_pref_error` | 112 | `SubpelSearchVarParams::upsampled_pref_error` | 1 |
| mcomp.c | `svt_estimated_pref_error` | 156 | `SubpelSearchVarParams::estimated_pref_error` | 1 |
| mcomp.c | `svt_check_better_fast` | 176 | `md_subpel::check_better_fast` | 1 |
| mcomp.c | `svt_check_better` | 219 | `md_subpel::check_better` | 1 |
| mcomp.c | `get_best_diag_step` | 248 | `md_subpel::get_best_diag_step` | 1 |
| mcomp.c | `svt_first_level_check` | 256 | `md_subpel::first_level_check` | 1 |
| mcomp.c | `svt_second_level_check_v2` | 289 | `md_subpel::second_level_check_v2` | 1 |
| mcomp.c | `svt_upsampled_setup_center_error` | 351 | `md_subpel::upsampled_setup_center_error` | 1 |
| mcomp.c | `first_level_check_fast` | 364 | `md_subpel::first_level_check_fast` | 1 |
| mcomp.c | `second_level_check_fast` | 422 | `md_subpel::second_level_check_fast` | 1 |
| mcomp.c | `two_level_checks_fast` | 559 | `md_subpel::two_level_checks_fast` | 1 |
| mcomp.c | `svt_av1_find_best_sub_pixel_tree_pruned` | 599 | `md_subpel::find_best_sub_pixel_tree_pruned` | 1 |
| mcomp.c | `svt_av1_find_best_sub_pixel_tree` | 683 | `md_subpel::find_best_sub_pixel_tree` | 1 |
| mcomp.c | `svt_aom_fp_mv_err_cost` | 775 | `md_subpel::fp_mv_err_cost` | 1 |
| C_DEFAULT/variance.c | `aom_var_filter_block2d_bil_{first,second}_pass_c` + `SUBPIX_VAR` / `VAR` | 29/55/192/184 | `svtav1_dsp::subpel_variance` | 1 |
| md_rate_estimation.c | `get_interinter_wedge_bits` | 23 | `port_md_rate_estimation::get_interinter_wedge_bits` | 1 |
| md_rate_estimation.c | `svt_aom_get_me_qindex` | 1084 | `port_md_rate_estimation::get_me_qindex` | 1 |
| inter_prediction.c | `svt_aom_get_wedge_params_bits` | 2053 | `port_md_rate_estimation::get_wedge_params_bits` | 1 |
| full_loop.c | `update_coeff_eob_fast` | 1006 | `port_full_loop::update_coeff_eob_fast` | 4 |
| full_loop.c | `svt_fast_optimize_b` | 1028 | `port_full_loop::fast_optimize_b` | 4 |
| coding_loop.c | `av1_copy_frame_mvs` | 1038 | `port_coding_loop::copy_frame_mvs` | 4 |

Tiers are `docs/WORKING-ON-THIS.md` §4. The four tier-4 rows are C `static`
functions whose only callers are also `static`; each module doc records the
exported ancestor that WOULD be needed and why building that shell would be
less trustworthy than the code under test.

## Why fourteen `static` mcomp.c functions are still tier 1

`nm -g Bin/Release/libSvtAv1Enc.a` prints `T` for exactly three of mcomp.c's
seventeen functions — the two entry points and `svt_aom_fp_mv_err_cost`
(positive control) — and NOTHING for `svt_check_better_fast`,
`two_level_checks_fast`, `get_best_diag_step`, … (negative control). The other
fourteen are reachable only THROUGH the entry points, so
`crates/svtav1-cref/shims/md_subpel_shims.c` builds
`SUBPEL_MOTION_SEARCH_PARAMS` + `MacroBlockD` + `ModeDecisionContext` from
plain scalars and calls the real entry points. One differential then covers the
whole tree — every helper, tie-break and early exit — which is strictly
stronger than fourteen hand-derived vectors, since a hand-derived vector is a
second transcription of the same logic.

`tests/c_parity_md_subpel.rs` compares `besterr`, the out `bestmv`, the out
`distortion`, the out `sse1` and `ctx->fp_me_dist` across 10 block shapes,
`forced_stop` 0-3, `allow_hp`, `iters_per_step` 1-3, `skip_diag_refinement`
0-5, all six `MV_COST_TYPE`s, `error_per_bit` / `early_exit_th`, `bias_fp`,
tight vs open `mv_limits`, `abs_th_mult`, `pred_variance_th`, `round_dev_th`
(including negative), the `PD_PASS_1` `mvp_th`/`hp_mv_th` arm, flat vs random
content, and a 600-cell randomised sweep.

Two guards keep that from being vacuous:
* `the_search_actually_searches` — the winner must move off `start_mv` and land
  on a FRACTIONAL position on at least a quarter of cells, and the pruned and
  unpruned trees must disagree somewhere (i.e. both entry points are driven).
* `host_simd_tier_agrees_with_the_c_kernels` — every cell is re-run with
  `svt_aom_mefn_ptr[bsize]` (this host's RTCD tier) in place of the `_c`
  kernels, so a green run cannot mean "the port matches `_c` while the shipping
  encoder does something else". On aarch64 (2026-08-31) they agree, which also
  covers the RTCD `svt_aom_upsampled_pred` the unpruned tree calls and the shim
  cannot override.

## Three C facts measured here, each of which looked like a port bug first

1. **`if (mvcost)` in `svt_mv_err_cost` (mcomp.c:48) is NOT a null-table
   guard.** The parameter is `const int* const mvcost[2]`, adjusted to
   `const int* const*`, so the test is on the ADDRESS OF THE ARRAY — a live
   struct member when reached through `svt_mv_err_cost_`, never null. The guard
   is always taken, and a null `mvcost[0]` SEGFAULTS inside `svt_mv_cost`
   instead of returning 0. Verified by calling the real
   `svt_aom_fp_mv_err_cost` with null element pointers. The `return 0` is
   transcribed and documented as unreachable; the differential drives null
   tables only on the four arms that never dereference them.

2. **`svt_aom_get_me_qindex`'s SB128 average can never divide by 3.**
   `valid_b64_cnt` starts at 1, gains 1 for a right neighbour and 1 for a below
   neighbour, and only if it thereby reaches exactly 3 does it take the
   diagonal and become 4. Reachable divisors are 1, 2 and 4. An implementation
   that averaged three cells on a right or bottom edge would set a different MD
   lambda on every edge SB. The differential drives 1x1, 1xN, Nx1 and NxN b64
   grids plus non-multiple-of-64 dimensions and asserts 3 is never reached.

3. **The unpruned tree's `mvp_th` arithmetic overflows on purpose.**
   `const int mvp_err = best_mvperr + 1; const int me_err = besterr + 1;` — both
   `+ 1`s happen in UNSIGNED 32-bit and only then convert to `int`; the
   `(me_err - mvp_err) * 100` is then signed and wraps. A naive `i32` port
   panics on a large `best_fp_mvp_dist`, which is how this was found.

## Still missing from the group

Named, not summarised:

* **full_loop.c** — `svt_av1_optimize_b` and the LPD1 driver around it;
  `svt_aom_quantize_inv_quantize` / `_light`, `svt_aom_inv_transform_recon_wrapper`,
  `svt_aom_full_loop_chroma_light_pd1`, `svt_aom_full_loop_uv`,
  `svt_aom_do_md_recon`, `shave_coeff`. (The quantize kernels themselves —
  `svt_aom_quantize_b_c`, the `quantize_fp` family, `svt_av1_compute_cul_level_c`,
  `svt_av1_perform_noise_normalization` — are already ported elsewhere in the
  tree: `quant.rs`, `pd0.rs`, `noise_norm.rs`.)
* **coding_loop.c** — `update_b`, `encode_b`, `svt_aom_encode_sb`,
  `update_coeff_cdf`, `svt_aom_update_mi_map_enc_dec`,
  `svt_aom_convert_recon_16bit_to_8bit`, `svt_aom_store16bit_input_src`.
* **md_rate_estimation.c** — `svt_aom_estimate_syntax_rate` as a whole (only its
  `wedge_idx_fac_bits` loop is covered here; the rest overlaps
  `leaf_funnel/rate_tables.rs` and the `port_entropy_inter` lane),
  `svt_aom_update_stats`, `svt_aom_update_part_stats`. `svt_aom_estimate_mv_rate`
  and `svt_aom_get_syntax_rate_from_cdf` are already ported
  (`inter_mv_code.rs`, `quant::syntax_rate_from_cdf`).
* **mcomp.c** — nothing.

## Open question raised, NOT fixed here

`svt_aom_quantize_inv_quantize_light` (full_loop.c:1263) quantizes with the
**V-plane** tables — `enc_ctx->quants_8bit.v_zbin / v_round / v_quant /
v_quant_shift[q_index]`, which `md_config_process.c:140-147` derives from
`v_dc_delta_q` / `v_ac_delta_q` — while dequantizing with the **Y-plane**
`deq_8bit.y_dequant_qtx[q_index]` (`:131`, derived from `y_dc_delta_q` and an
AC delta of 0). That cross-plane mix is in the C source as written.

`pd0.rs`'s `build_quant_entry` builds ONE entry from the luma
`DC_QLOOKUP_8` / `AC_QLOOKUP_8` and uses it for both halves, so the mix is not
modelled. That is invisible whenever the chroma delta_q is zero, and the port's
still-picture envelope is byte-identical today — so this is a QUESTION, not a
known defect: it is unverified whether any reachable configuration gives
`v_*_delta_q != 0` in PD0. Whoever chases PD0 partition divergence on a
chroma-delta_q frame should start here. Not changed in this lane because
`pd0.rs` belongs to another lane and the existing behaviour is a measured,
passing expectation (`docs/WORKING-ON-THIS.md`: report, do not edit someone
else's expectation).

## Not wired

`get_me_qindex` reads `pcs->b64_me_qindex`, produced by
`svt_av1_generate_b64_me_qindex_map` (rc_aq.c:656 <- rc_process.c:748), which
belongs to the rate-control group and is not ported. So the function currently
has no producer inside the port. It is translated now because it is the second
qindex argument to `svt_aom_mode_decision_configure_sb`
(enc_dec_process.c:2926) and `svt_aom_compute_rd_mult` (rc_aq.c:767) — a wrong
lambda changes the RD winner on every block — and because
`docs/WORKING-ON-THIS.md` §7 says a faithful translation stays translated with
its reachability written down.

Likewise `md_subpel` is not yet called from the encoder: `motion_est.rs`'s
`half_pel_refine` / `quarter_pel_refine` are still the homegrown bilinear
refine the inter path uses. Swapping the caller over is the next chunk and is
deliberately separate from landing a C-gated implementation.

## No regression_spotcheck cell

`docs/WORKING-ON-THIS.md` §3: a cell earns its place only if it FAILED before
the change and passes after. Nothing here fixes an observed failure — these are
new translations of code the port did not have — so no cell was added.

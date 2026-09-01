# Picture-decision / GOP structure port map (`port_picstruct.rs`)

What the lane `wp-picstruct` translated out of `Codec/pd_process.c`,
`Codec/initial_rc_process.c` and `Codec/pic_manager_process.c`, at what
evidence tier, and — first — **what is missing**.

Read `docs/WORKING-ON-THIS.md` §4 for the tier definitions. Tier 1 means a
differential against the real exported C symbol; tier 4 means hand-derived
vectors traced against the C source, and it is the weakest tier.

## MISSING — read this before the coverage table

**Per-function coverage for the whole lane lives in
`docs/pd-pcs-resize-lr-coverage.md`** (added 2026-08-31): one row per C
function across `pd_process.c`, `pcs.c`, `resize.c` and
`restoration_pick.c`, splitting ported / out-of-scope / missing, with the
evidence tier reached for each group. `pd_process.c` has **0 missing** there:
the 15 remaining rows are thread kernels and object pools this port replaces
by design, named individually.

**UPDATED 2026-08-31 (lane `wx-picstruct`).** The row that used to head this
table — `av1_generate_rps_info`'s random-access hierarchical branches,
`hierarchical_levels` 1..5, `pd_process.c:2270-3482` — is **no longer
missing**. They are translated in
[`crate::port_picstruct_ra`](../crates/svtav1-encoder/src/port_picstruct_ra.rs)
and gated at **tier 2** by `tests/c_parity_picstruct_ra_rps.rs`, which reads
`refresh_frame_flags`, `ref_frame_idx[]`, `show_frame` and
`frame_to_show_map_idx` out of ten real C-encoder bitstreams (HL1..HL5 x
presets 8 and 4). Every `pic_idx` of every table is exercised; 865 of the
1,092 compared reference columns carry the table's own value and 227 carry
`prune_refs`'s (a folded column cannot witness the entry it overwrote). Still
uncovered by those captures, and stated in the test: an INCOMPLETE trailing
mini-GOP (the only shape that drives the LOW_DELAY-inside-RA toggle
adjustment), overlay frames, and `referencing_scheme == 2`.

`RpsBranchUnsupported` is now the payload of `RpsError::UnsupportedBranch` and
means what C's own `exit(0)` arm means — `hierarchical_levels` outside 0..=5.
`RpsError::MiniGopIndex` is new: where C logs `Error in MG indexing` and falls
through with the PREVIOUS picture's slots, the port refuses.

| what | where | why |
|---|---|---|
| every S-frame path — `set_sframe_type`, `set_sframe_rps`, `decide_sframe_mg`, `prune_sframe_refs`, the `IS_SFRAME_FLEXIBLE_INSERT` override in `get_pic_idx_in_mg` | `pd_process.c` various | Outside the port's envelope. All are no-ops when no S-frame is pending, which is every configuration this port encodes. |
| app-driven reference management — `apply_ref_mgmt_events`, `ref_mgmt_reset_state`, `apply_ref_use`, the STORE/CLEAR/USE masking of `refresh_frame_mask` | `pd_process.c:1400-1478` | Outside the envelope; no-op when no event is queued and no slot is STOREd. |
| the noise-estimation half of `derive_tf_window_params` | `pd_process.c:3752-3846` | `svt_estimate_noise_fp16` / `svt_aom_noise_log1p_fp16` belong to the pre-analysis lane and are C-gated there. The counts derived FROM the noise level are ported here. |
| the reorder-queue and pre-assignment-buffer SEARCHES that fill the TF window | `pd_process.c:3860-4090` | Buffer plumbing this port replaces. The window COUNTS, the compaction and the averages are ported. |
| the `ext_group` build half of `store_extended_group` | `initial_rc_process.c:406-424` | A walk over `ctx->lad_queue`'s circular buffer. The SELECTION half — which decides membership, and therefore `r0` and every SB's qindex — is ported and tier-1 gated. |
| `ref_global_motion[]` copy inside the primary-ref block | `pic_manager_process.c:837-846` | Needs the reference object's warp params (global-motion module). |
| live-count / release bookkeeping (`svt_object_inc_live_count`, `svt_release_object`) in `send_picture_out` and `low_delay_{store,release}_tf_pictures` | `pd_process.c` | Buffer plumbing. The release ORDER is recorded in the doc comment because it is load-bearing in C. |
| `copy_tf_params`' `IS_SFRAME_FLEXIBLE_INSERT` guard, `init_pic_settings`' `copy_tf_params` and `svt_aom_sig_deriv_multi_processes_*` calls | `pd_process.c:4480-4484`, `:4949-4955` | Other modules / outside the envelope; each is named in the function's doc comment. |
| the screen-content DETECTION calls inside `perform_sc_detection` | `pd_process.c:4772-4792` | `svt_aom_is_screen_content*` / `svt_aom_is_input_luma_dominant` live in the screen-content module. The INHERITANCE half — which is what keeps SC-tuned thresholds stable across a GOP — is ported. |

Nothing else from the lane's 59-item queue is missing.

## Evidence tiers

**Tier 1 — differential against the real exported C symbol (17 functions).**

| function | C |
|---|---|
| `svt_aom_is_pic_used_as_ref` | `pd_process.c:1770` |
| `svt_av1_setup_skip_mode_allowed` | `pd_process.c:102` |
| `update_count_try` | `pd_process.c:4507` |
| `svt_aom_is_incomp_mg_frame` | `pd_process.c:4986` |
| `is_pic_cutting_short_ra_mg` | `pd_process.c:928` |
| `svt_aom_is_delayed_intra` | `pd_process.c:3620` |
| `search_this_pic` | `pd_process.c:3606` |
| `dg_detector_hme_level0` | `pd_process.c:532` |
| `get_similar_ref_brightness` | `pd_process.c:4251` |
| `svt_aom_get_gm_needed_resolutions` | `pd_process.c:990` |
| `svt_aom_get_mini_gop_stats` | `utility.c:168` |
| `svt_aom_tf_max_ref_per_struct` | `enc_handle.c:2506` |
| `svt_aom_get_tpl_group_level` | `initial_rc_process.c:190` |
| `svt_aom_set_tpl_group` | `initial_rc_process.c:204` |
| `validate_pic_for_tpl` | `initial_rc_process.c:171` |
| `store_extended_group` | `initial_rc_process.c:406` |
| `search_ref_in_ref_queue` | `pic_manager_process.c:178` |

Plus two `static` functions reached through symbol promotion (see below):
`set_ref_list_counts` (`pd_process.c:1804`) and `set_all_ref_frame_type`
(`pd_process.c:1044`).

**Tier 4 — hand-derived vectors, because the C function is `static` with no
usable symbol (the rest).** `port_picstruct_traced.rs`.

## Promoting a `static` C function to a tier-1 oracle

`crates/svtav1-cref/build.rs` copies
`cbuild-static/Source/Lib/Codec/CMakeFiles/CODEC.dir/pd_process.c.o`, runs
`llvm-objcopy --globalize-symbol` on the copy, wraps it in an archive and links
it ahead of `libSvtAv1Enc.a`. No duplicate symbols result, because the promoted
object supplies everything the archive member would have and the member is
therefore never pulled.

**A promoted symbol's ABI is NOT guaranteed to be its source signature.** LLVM
may change an `internal` function's calling convention.
`scene_transition_detector` is the counterexample: its
`PictureParentControlSet** window` parameter was promoted to the current PPCS,
so calling it as declared segfaults on `NULL+0x68`. It is excluded and stays at
tier 4. **Disassemble the prologue before adding a name to `SYMS`.**

The promotion is best-effort: `rust-gates.yml` caches `Bin/Release` and
deliberately not `cbuild-static`, so on a cache hit the object is absent and
build.rs says so with a `cargo:warning`. The same two functions keep their
unconditional tier-4 coverage, so a host without the object loses evidence
STRENGTH, never coverage. `SVT_CREF_REQUIRE_PICSTRUCT_STATICS=1` makes it
strict where the caller knows the object is there.

## Configuration facts, measured — these decide what is live

Every one of these was read out of the C source, not inferred from a name.

- **Temporal filtering is OFF in low delay.** `enc_handle.c:3339-3343`
  short-circuits `tf_level` to 0 for all `LOW_DELAY` before any preset logic.
  So the whole TF window group is dead for the campaign's first cell and ON BY
  DEFAULT in random access (tf_level 5 at M3-M7), where it rewrites the SOURCE
  PIXELS of base-layer frames.
- **TPL is OFF in low delay.** `get_tpl` (`enc_handle.c:3657-3668`) returns 0
  for `LOW_DELAY`, for allintra and for `aq_mode == 0`.
- **Dynamic GOP is ON by default in random access.** `enc_handle.c:4294-4300`
  sets `enable_dg = 0` for VBR, CBR, >= 4K, non-`RANDOM_ACCESS` and multi-pass,
  else 1 — so the detector runs for single-pass CQP/CRF RA below 4K.
- **Scene-change detection is not dead.** `static_config.scene_change_detection`
  is force-zeroed (`enc_settings.c:839-843`), which makes the first arm of
  `perform_scene_change_detection` dead — but
  `vq_ctrls.sharpness_ctrls.scene_transition` is 1 in BOTH arms of
  `derive_vq_params` (`enc_handle.c:3282, 3291`) and zeroed only for
  `LOW_DELAY` (`3324-3326`), so the SECOND arm runs in random access.
  `scs->calc_hist` follows the same shape (`enc_handle.c:1353`), so
  `calc_ahd_pd` is live there too.

## Call order — not what a reading of the file suggests

`svt_aom_picture_decision_kernel_iter` (`pd_process.c:5672-5692`), transcribed
into [`picture_decision_per_picture`]:

```text
frm_hdr.frame_type = ...     (5674-5679)
set_gf_group_param(pcs)      (5680)   <-- BEFORE the RPS, not after
av1_generate_rps_info(...)   (5681)
update_dpb(pcs, ctx)         (5688)
init_pic_settings(...)       (5691)
```

`set_gf_group_param` running FIRST is what makes `frame_is_boosted` — and
therefore the base-vs-non-base MRP caps inside `set_ref_list_counts`, which
`av1_generate_rps_info` calls — read THIS picture's `update_type` instead of a
stale one.

## Traps this port had to transcribe rather than tidy

Each is pinned by a test; the test names it.

1. `set_ref_list_counts`' list-1 inner loop starts at `LAST2` when `i == BWD`
   and at `LAST` otherwise, and its `j + 1 > ref_list0_count` guard skips
   list-0 entries the frame will not signal. With all seven POCs equal the
   result is `(1, 1)`, not `(1, 0)`.
2. `frame_is_boosted` is `frame_is_kf_gf_arf` — intra-only or `update_type` in
   {ARF, GF} — **not** `temporal_layer == 0`.
3. `set_frame_update_type`'s flat arm is `frame_offset % MAX(4, 1 << 0)`, i.e.
   `% 4`.
4. `ctx->sframe_hier_lvls`' constructor value (`pd_process.c:249`) is DEAD; the
   kernel overwrites it at `picture_number == 0` (`:5407-5409`). Taking the
   ctor value literally collapses every mini-GOP to level 0.
5. `initialize_mini_gop_activity_array`'s IDR guard is
   `count >= N && !(count == N && idr_flag)`, so a 4-picture buffer headed by
   an IDR splits into TWO 2-picture mini-GOPs.
6. `INVALID_LUMA` is **256** (`definitions.h:90`), not -1.
7. `svt_aom_set_tpl_group`'s `pcs->slice_type ? A : B` takes the TRUE arm for
   an **I** slice (`B_SLICE = 0`, `I_SLICE = 1`).
8. `reduced_tpl_group == 0` means "base layer only"; "no reduction" is -1.
9. `store_extended_group`'s two intra arms are ASYMMETRIC: a non-delayed intra
   at `i != 0` is ADDED and closes the GOP, a delayed one BREAKS without being
   added.
10. `send_picture_out`'s `safe_limit_nref` limiter lowers the try counts AFTER
    `set_all_ref_frame_type` ran, leaving `ref_frame_type_arr` stale. C flags
    this with its own TODO; reproduced, not fixed.
11. `scene_transition_detector`'s `region_width`/`region_height` are declared
    outside the region loops and updated with `+=` inside, so the remainder
    ACCUMULATES and the threshold differs between regions of identical size.
12. `average_intensity_per_region` is `uint64_t[4][4]`, narrowed by an explicit
    `(int16_t)` cast before subtraction and then truncated to `uint8_t`.
13. `svt_aom_tf_max_ref_per_struct`'s `direction` is `(void)`-cast unused, and
    only the I_SLICE row grows with the hierarchy.
14. `mctf_frame`'s STORE and RELEASE gates are not symmetric — RELEASE also
    requires `temporal_layer_index == 0`.
15. `tf_motion_direction`'s 1.5x margin is `other * 6 / 4` with INTEGER
    division, so at `vert == 1` the threshold is 1.
16. `calc_mini_gop_activity` is FULLY SYMMETRIC in its two sub layers, which is
    why C's call site can invert `sub_layer_mv_in_out_count1`/`2` relative to
    the indices they are named after without any effect.

## A C defect found on the way

`derive_tf_window_params`' past-window compaction (`pd_process.c:3915-3920`,
`:4091-4098`) is unreachable dead code — `actual_past_pics` is initialised to
`num_past_pics` and never modified. Written up as
`docs/SUSPECTED-C-BUGS.md` #18, including the second-order finding that fixing
the counter alone would not fix the block.

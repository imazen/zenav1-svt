# `pd_process.c` / `pcs.c` / `resize.c` / `restoration_pick.c` — per-function coverage

What is translated, what is **deliberately not** (pipeline plumbing this port
replaces rather than translates), and what is genuinely **missing**. One row
per C function, because a file-level percentage hides exactly the thing a
reader needs.

Built 2026-08-31 by lane `wx-picstruct` by reading BOTH sides. It is NOT
`tools/c_surface_inventory.py`'s output: that tool matches by name and
therefore both over- and under-counts here. It reports
`svt_aom_ref_mgmt_storeable_slots_mask` as missing (the port calls it
`storeable_slots_mask`), `av1_generate_rps_info` as missing (ported as
`generate_rps_info`), and `svt_av1_compute_stats_highbd_c` as missing (ported
as `compute_stats_hbd`). Treat its numbers as a work queue, never as coverage.

| file | functions | ported | out of scope | MISSING |
|---|---|---|---|---|
| `Codec/pd_process.c` | 44 | 29 | 15 | **0** |
| `Codec/pcs.c` | 32 | 6 | 26 | **0** |
| `Codec/resize.c` | 39 | 22 | 3 | **14** |
| `Codec/restoration_pick.c` | 23 | 19 | 0 | **4** |
| **total** | **138** | **76** | **44** | **18** |

## MISSING — read this first

### `Codec/resize.c` — frame resize's per-picture driver and reference cache (14)

The kernel layer AND the frame-level plane loop are complete as of 2026-08-31
(`svtav1_dsp::resize::resize_plane` / `port_resize_hbd::highbd_resize_plane` /
`resize_frame`, all tier 1). What is left is everything above them:

| function | what it is |
|---|---|
| `svt_aom_init_resize_picture` | the per-picture driver: `calc_superres_params` + `validate_size_scales` + the scaler |
| `scale_pcs_params` | rewrites the frame-size fields after a rescale |
| `svt_aom_reset_resized_picture` | restores the unscaled picture pointers |
| `scale_input_references`, `scale_source_references`, `svt_aom_scale_rec_references` | build the scaled-reference cache |
| `svt_aom_use_scaled_rec_refs_if_needed`, `svt_aom_use_scaled_source_refs_if_needed` | select from that cache |
| `pack_highbd_pic_2d`, `svt_aom_unpack_highbd_pic_2d` | 10-bit pack/unpack for the cache |
| `fill_col_to_arr`, `fill_arr_to_col`, `highbd_fill_col_to_arr`, `highbd_fill_arr_to_col` | **PORTED** — listed here only because the inventory tool cannot see them |

Reachability: all of it is behind `--resize-mode`, which is off by default.

### `Codec/restoration_pick.c` — the SGR wiring (4)

The SGR/SWITCHABLE **decision bodies are ported** (`port_sgr_search::
search_sgrproj_unit`, `sgrproj_finish_decision`, `switchable_decision`, all
built on tier-1 kernels). They are **not wired** into `restoration.rs`'s frame
walk, which still offers `RESTORE_WIENER` vs `RESTORE_NONE` only. The four
rows below are the parts of that wiring:

| function | what it is |
|---|---|
| `copy_unit_info` | copies the winning unit info into the frame-level array; only reachable once SWITCHABLE is wired |
| `reset_rsc` | zeroes the `RestSearchCtxt` accumulators between types |
| `init_rsc_seg` | builds `RestSearchCtxt` from the PCS; the port passes plain slices instead |
| `rest_tiles_in_plane` | tile-count helper; the port is single-tile-per-plane for LR |

**Next chunk, decomposed.** In `crates/svtav1-encoder/src/restoration.rs`:
extend `search_restoration_still_bd`'s per-unit loop to also run
`port_sgr_search::search_sgrproj_unit`, extend its frame-level argmin from
`{NONE, WIENER}` to `{NONE, WIENER, SGRPROJ, SWITCHABLE}` using
`sgrproj_finish_decision` / `switchable_decision`, and extend
`apply_restoration_frame_bd` and `write_lr_for_sb` to the SGR filter and
syntax. Reachability first: `port_lr_level::sg_filter_level_default` is 3 at
presets 0..=3 in VIDEO mode and 0 everywhere else, and
`svt_aom_get_sg_filter_level_allintra` is 0 for every representable preset
(`rust/CLAUDE.md` guard 5), so the change is **byte-neutral on the all-intra
path** — the existing identity gates are the regression test.

## Out of scope, per function

Not a gap: these are thread kernels, object pools, constructors/destructors and
buffer plumbing that this port replaces by design.

**`pd_process.c` (15).** `svt_aom_picture_decision_kernel` and `_iter` (thread
entry and per-picture driver — the driver's ORDER is ported in
`port_picstruct::picture_decision_per_picture`), `process_pics`,
`svt_aom_picture_decision_context_ctor` / `_dctor`,
`release_prev_picture_from_reorder_queue`, `assign_and_release_pa_refs`,
`search_ref_in_ref_queue_pa`, `check_window_availability`,
`low_delay_store_tf_pictures`, `process_first_pass`,
`initialize_overlay_frame`, `perform_simple_picture_analysis_for_overlay`,
`update_rc_param_queue` (mutex-guarded circular pool of shared
`RateControlParam` objects), `print_pre_ass_buffer` (`SVT_LOG` behind
`#if LAD_MG_PRINT`).

**`pcs.c` (26).** Every `*_ctor` / `*_dctor` / `*_creator`, the three
`*_update_param` re-allocators for resolution changes,
`create_neighbor_array_units`, and `alloc_sb_geoms` / `free_sb_geoms` (the
`EB_MALLOC_ARRAY` / `EB_FREE_ARRAY` pair around an array a Rust caller owns).

**`resize.c` (3).** `allocate_downscaled_reference_pics`,
`allocate_downscaled_source_reference_pics`,
`svt_aom_downscaled_source_buffer_desc_ctor` — buffer-descriptor allocation.

## Where the ported ones live

| C file | Rust |
|---|---|
| `pd_process.c` RPS + GOP | `port_picstruct.rs`, `port_picstruct_ra.rs` (the five random-access hierarchical branches), `port_pd_gop.rs` |
| `pd_process.c` long-term references | `port_ref_mgmt.rs` |
| `pd_process.c` S-frames | `port_sframe.rs` |
| `pcs.c` geometry + sizing | `port_pcs_geom.rs` |
| `resize.c` decision | `port_superres_decision.rs` |
| `resize.c` kernels | `svtav1-dsp::resize`, `svtav1-dsp::port_resize_hbd`, `svtav1-dsp::superres` |
| `restoration_pick.c` Wiener | `restoration.rs`, `svtav1-dsp::restoration` |
| `restoration_pick.c` SGR | `port_sgr_search.rs`, `svtav1-dsp::port_sgr` |

## Evidence tiers reached

Per `docs/WORKING-ON-THIS.md` §4.

**Tier 1** (differential against the real exported C symbol):
`svt_aom_ref_mgmt_storeable_slots_mask` (and, through it, the file-static
`exclusive_write_slots_mask_ld_cbr`), `svt_aom_is_pic_skipped`,
`b64_geom_init`, `sb_geom_init`, `svt_aom_get_max_allocated_me_refs`,
`svt_aom_get_out_buffer_size`, `svt_aom_get_frame_update_type`,
`svt_aom_get_denom_idx`, `svt_av1_resize_plane_c`,
`svt_av1_highbd_resize_plane_c`. Shims:
`crates/svtav1-cref/shims/refmgmt_shims.c`.

**Tier 2** (byte-identity against the real encoder's output): the whole
random-access hierarchical RPS — `tests/c_parity_picstruct_ra_rps.rs` reads
`refresh_frame_flags`, `ref_frame_idx[]`, `show_frame` and
`frame_to_show_map_idx` out of ten committed C bitstreams (HL1..HL5 x presets
8 and 4). Regenerate with `tools/gen_ra_rps_captures.sh`; inspect any stream
with `tools/ra_rps_oracle.py`.

**Tier 4** (hand-derived vectors traced against the C source) for everything
`static` with no exported symbol: the RPS branch tables themselves, the
S-frame family, the long-term-reference dispatcher, `store_gf_group`,
`mrp_detector_hme_level0` and the five `static` superres-decision helpers.
`av1_generate_rps_info` and all ten S-frame functions were confirmed absent
from `nm -g Bin/Release/libSvtAv1Enc.a`.

## Coverage the tier-2 gate cannot reach, measured

`prune_refs` folds unused reference-list slots onto LAST / BWD, so a folded
column carries prune's value rather than the branch table's and no bitstream
oracle can witness it. The gate counts the split and asserts it: **865 of 1,092
compared reference columns carry the table's own value, 227 carry prune's.**
That is also why the captures use two presets — at preset 8 alone the list caps
are 3 and 2, so GOLD and ALT are folded on every frame (verified by mutation).

Also uncovered by those captures, and stated in the test: an incomplete
trailing mini-GOP (the only shape that drives the LOW_DELAY-inside-RA toggle
adjustment), overlay frames, and `referencing_scheme == 2`. The first is a
harness limit — the C driver's ST-mode object pool exhausts above 7 / 9 / 17 /
25 / 41 frames at HL1..HL5.

## C defects found

`docs/SUSPECTED-C-BUGS.md` #25 — `prune_sframe_refs` reads
`ref_order_hint[-1]` on every single-reference candidate, because
`av1_set_ref_frame` writes `rf[1] = NONE_FRAME = -1` and the guard is
`rf[1] < BWDREF_FRAME`. The pruning decision is therefore not deterministic in
C. The same expression also indexes `ref_order_hint[rf]` where every other
reader uses `rf - 1`.

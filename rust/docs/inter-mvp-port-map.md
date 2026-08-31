# Inter-frame MVP stack — port map and coverage (chunk C2)

**What this file is.** The honest inventory for `crates/svtav1-encoder/src/inter_mvp.rs`:
which C functions are translated, at which evidence tier each is gated, what is
NOT translated, and the two C findings the port surfaced. Written 2026-08-31,
the day the chunk landed. Read `docs/WORKING-ON-THIS.md` first — its §4
evidence tiers are what the columns below mean.

> **Line numbers are as of the `reference/svt-av1` submodule at v4.2.0.**
> Re-locate every citation by SYMBOL NAME, not by line, per the standing rule.

## 1. The one-line status

The inter MVP predictor stack is ported and differentially locked. It is NOT
wired into `pipeline.rs` — nothing calls it yet. `pipeline.rs` still refuses
inter frames at the public entry point, and closing that refusal is C1/C4/C5
work, not this chunk's.

## 2. Coverage — `adaptive_mv_pred.c` (2,040 lines, 37 function definitions)

**27 of 37 translated here, 1 partial, 8 not attempted, 1 lives elsewhere.**

### Translated and gated (27)

| C symbol | tier | gate |
|---|---|---|
| `get_block_mv` `:40` | 1 | via `setup_ref_mv_list` |
| `is_inside` `:44` | 1 | via `setup_ref_mv_list` |
| `clamp_mv_ref` `:49` | 1 | via `setup_ref_mv_list` (extreme-MV cells in the sweep) |
| `add_ref_mv_candidate` `:57` — BOTH arms | 1 | via `setup_ref_mv_list`, compound refs swept |
| `scan_row_mbmi` `:130` | 1 | via `setup_ref_mv_list` |
| `scan_col_mbmi` `:186` | 1 | via `setup_ref_mv_list` |
| `scan_blk_mbmi` `:241` | 1 | via `setup_ref_mv_list` |
| `has_top_right` `:266` | 1 | via `setup_ref_mv_list` + a directed VERT_A cell |
| `find_valid_row_offset` `:327` | 1 | via `setup_ref_mv_list` |
| `find_valid_col_offset` `:331` | 1 | via `setup_ref_mv_list` |
| `get_relative_dist` `:335` | 1 + 4 | via the MFMV block; directed vectors for the wrap |
| `add_tpl_ref_mv` `:352` | 1 | via `setup_ref_mv_list` with `use_ref_frame_mvs = 1` |
| `sort_mvp_table` `:450` | 1 | reused from `intrabc_mvp.rs`, gated there |
| `scan_row_col_light` `:469` — BOTH arms | 1 | via `setup_ref_mv_list` |
| `setup_ref_mv_list` `:651` | 1 | `c_parity_setup_ref_mv_list_inter` |
| `block_center_x` `:973` | 1 | via `gm_get_motion_vector_enc` |
| `block_center_y` `:978` | 1 | via `gm_get_motion_vector_enc` |
| `svt_aom_gm_get_motion_vector_enc` `:983` | 1 | `c_parity_gm_get_motion_vector_enc` |
| `count_ref_match` `:1128` | 1 | via `compute_inter_mode_ctx_light` |
| `svt_aom_compute_inter_mode_ctx_light` `:1138` | 1 | `c_parity_compute_inter_mode_ctx_light` |
| `svt_aom_generate_av1_mvp_table` `:1329` — inter path | 1 | gm-candidate derivation gated; the loop shape is transcription |
| `svt_aom_get_av1_mv_pred_drl` `:1407` | 1 | `c_parity_get_av1_mv_pred_drl` |
| `count_overlappable_nb_above` `:1830` | 1 | via `count_overlappable_neighbors` |
| `count_overlappable_nb_left` `:1864` | 1 | via `count_overlappable_neighbors` |
| `svt_av1_count_overlappable_neighbors` `:1893` | 1 | `c_parity_count_overlappable_neighbors` |
| `svt_av1_get_ref_mv_from_stack` `:2002` | 1 | via `find_best_ref_mvs_from_stack` |
| `svt_av1_find_best_ref_mvs_from_stack` `:2030` | 1 | `c_parity_setup_ref_mv_list_inter` |

### Partial (1)

- **`svt_aom_init_xd` `:1038`.** Only the MVP-relevant slice is derived, and it
  is derived in `intrabc_mvp::derive_block_ctx` (n8 dims, availability,
  `is_sec_rect`, the `mb_to_*` edges, the tile bounds). NOT ported: the chroma
  neighbour derivation (`chroma_up_available` / `chroma_above_mbmi` /
  `chroma_left_mbmi`), the `mi_grid_base` / `mip` pointer plumbing, and the
  `xd->mi[0]->partition = from_shape_to_part[ctx->shape]` writeback. Those
  serve chroma prediction and the mi-map, not the MV stack.

### Not attempted here (8)

`svt_aom_update_mi_map_enc_dec` `:1459`, `svt_copy_mi_map_grid_c` `:1492`,
`get_mbmi` `:1522`, `svt_aom_update_mi_map` `:1541` — the mi-grid writeback.
`record_samples` `:1594`, `av1_find_samples` `:1610`,
`svt_aom_init_wm_samples` `:1752`, `svt_aom_warped_motion_parameters` `:1776` —
warped-motion sample collection. Both groups serve encode paths several chunks
downstream; deliberately deferred, not overlooked.

### Elsewhere (1)

`svt_aom_is_dv_valid` `:1908` — IntraBC, already ported in `intrabc.rs`.

## 3. Coverage — the rest

| C source | what | tier | note |
|---|---|---|---|
| `inter_prediction.h:203-266` | `integer_mv_precision`, `lower_mv_precision`, `get_mv_projection`, `check_sb_border` | 1 + 4 | tier 1 indirectly through the MFMV block; directed vectors pin `den == 0`, the ±`MAX_FRAME_DISTANCE` clamps, `MV_UPP`/`MV_LOW` saturation and C's truncating `%` |
| `inter_prediction.h:411-545` | `is_global_mv_block`, `av1_set_ref_frame`, `av1_ref_frame_type`, `get_list_idx`, `get_ref_frame_idx`, `compound_ref{0,1}_mode` | 1 | driven through the shims; `ref_frame_type_roundtrip` additionally checks all 29 types round-trip |
| `inter_prediction.c:2565` | `svt_aom_mode_context_analyzer` | 1 | `c_parity_mode_context_analyzer` |
| `md_config_process.c:396-580` | `get_block_position`, `motion_field_projection`, `av1_setup_motion_field` | **4** | all three are `static` and export NO symbol (`nm -gU Bin/Release/libSvtAv1Enc.a`) — hand-derived vectors traced against the C source, in `tests/inter_mvp_motion_field.rs`, with the arithmetic written out beside each expectation |

## 4. Where the evidence lives

- `crates/svtav1-cref/shims/inter_mvp_shims.c` — its OWN translation unit, not
  appended to `ref_shims.c`, so the C2 and C3 lanes never share an editable
  file in one working copy. Drives `setup_ref_mv_list`,
  `svt_aom_gm_get_motion_vector_enc`, `svt_aom_compute_inter_mode_ctx_light`,
  `svt_aom_get_av1_mv_pred_drl`, `svt_aom_mode_context_analyzer`,
  `svt_av1_count_overlappable_neighbors` and
  `svt_av1_find_best_ref_mvs_from_stack`.
- `tests/c_parity_inter_mvp.rs` — 8 tests. The main sweep is 4,000+ cases over
  randomized inter mode-info grids: 14 ref-frame types (7 single + 7 compound,
  spanning both the bidir and the unidir block of `ref_frame_map`), 11 block
  sizes, two tiles, both SB sizes, MFMV on/off, high-precision MVs on/off, four
  global-motion model classes. It compares the FULL 8-slot stack (`this_mv`,
  `comp_mv`, weight — the beyond-count gm-fill included), the count, the mode
  context, nearest/near, and C's `mv_ref0[64]` scratch.
- `tests/inter_mvp_motion_field.rs` — 8 tier-4 tests.

**The MFMV anti-vacuity check is the one worth copying.** "The temporal-MVP
code ran" is not evidence. The test re-runs the SAME C oracle with
`use_ref_frame_mvs = 0` and requires the temporal candidates to have CHANGED
the stack, the count or the mode context in more than 300 cases. A sweep that
only proves a branch executed can pass while the branch is inert.

## 5. Two C findings, both measured

1. **`has_top_right` mutates `bs` and the `PARTITION_VERT_A` check reads the
   MUTATED value** (`:303-313` then `:314-322`). Reading the original argument
   diverges. Measured at `mi = (36, 10)`, an 8x8 block in a 64x64-mi SB whose
   current cell has `partition == PARTITION_VERT_A`: `bs` enters as 2,
   `mask_col == 10` drives the loop to advance it to 4, and `mask_row == 4`
   then makes C drop the top-right candidate — `ref_mv_stack[0].weight` 668 in
   C against 672. Only `partition == 6` diverges; the nine other partition
   types agree. `intrabc_mvp.rs` carried the same defect and is fixed;
   byte-inert on the IntraBC corpus
   (`benchmarks/intrabc_has_top_right_vert_a_2026-08-31.{tsv,meta}` — 120 cells,
   0 changed), so it gets no `regression_spotcheck.sh` cell, per §3.
2. **`add_ref_mv_candidate`'s `assert(weight % 2 == 0)` (`:63`) does not
   hold.** C ships with `NDEBUG` so it is never checked. With `row_adj == 1` —
   an 8x4 block at an odd `mi_row` — `max_row_offset` is -5 and
   `scan_row_mbmi`'s `inc` reaches 5, giving `weight == 5`. Reproduced on the
   randomized grids. The assert is deliberately NOT transcribed.

## 6. What the next chunk needs from this one

`setup_ref_mv_list` needs an `InterMvpEnv` (global motion, sign bias, order
hints, the `tpl_mvs` field, `use_ref_frame_mvs`, `sb64_sq_no4xn_geom`,
`symmetric_refs`) and an `MvpGrid` of `MvpMiEntry` in ABSOLUTE mi coordinates.
Nothing populates either from the real pipeline yet — the mi-grid writeback
(§2, "not attempted") is what would, and `av1_setup_motion_field` is what would
fill `tpl_mvs`. Wiring is the consumer chunk's call, not this one's.

Two shapes to preserve when wiring:

- **`Mv mv_ref0[64]` is ONE local shared across `generate_av1_mvp_table`'s
  whole ref loop** (`:1336`). The `symteric_refs` shortcut depends on that
  sharing: the `LAST_FRAME` pass stores a projected MV in slot *i* and the
  `BWDREF_FRAME` / `LAST_BWD_FRAME` passes read it back. Driving
  `setup_ref_mv_list` per ref with a fresh scratch is a DIFFERENT computation —
  use `generate_av1_mvp_table`, or `setup_ref_mv_list_seeded`.
- **`svt_aom_get_av1_mv_pred_drl` leaves `nearestmv`/`nearmv` UNINITIALIZED on
  the branches it does not write.** The port takes them as an explicit
  `initial` so that is reproducible rather than accidental; a caller must pass
  what its own arrays held.

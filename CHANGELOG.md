# Changelog

All notable changes to `zenav1-svt` (the pure-Rust SVT-AV1 still-image encoder
port). The project's unit of progress is **byte-identity with the C reference**,
so entries state what became byte-identical and under which gate, not just what
code was added.

Crates are not published to crates.io yet — depend by git.

## [Unreleased]

### QUEUED BREAKING CHANGES

<!-- Batch API breaks here; ship them in one version bump, never piecemeal. -->
- **Crate consolidation 6 → 4 publishable packages (issue #3, 2026-08-28).**
  `zenav1-svt-tables` is folded into `zenav1-svt-types` as
  `svtav1_types::tables::{block, interp, partition, scan, transform}` and
  `zenav1-svt-entropy` into `zenav1-svt-encoder` as
  `svtav1_encoder::entropy::{cdf, coeff, coeff_c, context, default_cdfs,
  default_coef_cdfs, lr, mv_coding, obu, range_coder, scan_tables, tile,
  writer}`; both former packages are deleted. Path rename only for the two
  crates' consumers (`svtav1_tables::X` → `svtav1_types::tables::X`,
  `svtav1_entropy::X` → `svtav1_encoder::entropy::X`); the facade keeps
  `svtav1::tables` / `svtav1::entropy` as re-exports so facade users are
  unaffected. The entropy crate's `unchecked_entropy` / `symtrace` features
  moved onto `zenav1-svt-encoder` (the facade's `symtrace` forwards there).
  Bitstream bytes unchanged: `byteid_fingerprint` 144/144 cells identical
  before/after, identity_matrix 54/54, bd10 36/36, partial_sb 146/146,
  regression_spotcheck 33/33, decode_conformance 1260 + 1575 / 0 failed.
  Nothing is published yet, so this is a pre-release rename, not a semver
  event.
- **`AvifEncoder` knob surface (issue #9 item 7, 2026-08-28).** REMOVED
  `with_trellis`, `with_seg_boost`, the `seg_boost()` getter and
  `with_still_image_tuning` — all four were recorded-and-ignored with no
  counterpart in this pipeline or in C. `with_vaq(bool, f64)` is REPLACED by
  `with_variance_boost(bool, u8)` (C's 1-4 strength scale). `encode_yuv420`
  keeps its signature but its OUTPUT CONTRACT changes from three
  length-prefixed monochrome streams to one real AV1 4:2:0 bitstream — the old
  format was not decodable, so nothing can have depended on it.
  `AvifEncoder::{enable_qm, enable_variance_boost}` now default to `false`
  (C's mainline defaults); the emitted bytes for a caller that sets neither
  are unchanged.
- **`crate::pd0::pd0_pick_sb_partition{,_lvl0,_m6,_m6_eval}` take a
  `lambda_weight: u32` after `qindex` (issue #9 item 4, 2026-08-28).** C's
  frame `lambda_weight` (`pcs->lambda_weight`, enc_mode_config.c:10093-10115)
  is a frame-level fact — the tune-IQ curve, the PSNR ladder, and the
  extended-CRF bump — that these entry points cannot derive from their `qp`
  argument once a fractional CRF moves `picture_qp` off `static_config.qp`.
  Callers pass `pd0::frame_lambda_weight(picture_qp, tune_iq, bump)`;
  `frame_lambda_weight(qp, false, 0)` reproduces the previous internal ladder
  exactly, so the change is byte-neutral at CRF offset 0.
- None queued otherwise. `EncodePipeline`'s new surface (`try_encode_frame_420_hbd`,
  `try_encode_frame_hbd`, `with_superres`) is additive; the `SeqTools` and
  `ScSignal` structs gained fields (`enable_superres`, `superres`), which is a
  break only for out-of-crate struct literals — there are none.

### Added

- **Rate control: the `rc_process.c` group is ported, mostly tier 1 (lane
  `wp-ratecontrol`).** New `svtav1-encoder/src/port_rc_process.rs` +
  `port_rc_lambda_tables.rs`, `port_rc_vbr_cbr.rs` + `port_rc_vbr_tables.rs`,
  `port_rc_rtc_cbr.rs`, `port_pass2_strategy.rs`, and
  `svtav1-cref/src/rate_control.rs` + `shims/rc_shims.c`.
  **TIER 1** against the real exported symbols: `svt_av1_rc_bits_per_mb`,
  `svt_av1_compute_qdelta_by_rate` (the inter unblocker — `rc_crf_cqp.c:170-178`
  calls it on every non-intra frame and its delta moves `base_q_idx`),
  `svt_aom_compute_rd_mult_based_on_qindex` over all seven update types (the
  port previously hardcoded only the KF arm at five sites in `pd0.rs`),
  `svt_aom_compute_rd_mult`, `svt_aom_compute_fast_lambda`,
  `svt_aom_lambda_assign`, `svt_aom_set_rc_param`, `svt_av1_rc_init`,
  `svt_av1_new_framerate`, `svt_av1_get_cqp_kf_boost_from_r0`,
  `svt_av1_get_gfu_boost_from_r0_lap`, `svt_av1_calculate_boost_bits`, the
  seven `rc_process.c` const tables (exported data symbols), the three
  `av1_lambda_mode_decision*_bit_sad` tables and the eighteen `rc_tables.h`
  minq tables (5,376 entries, all read out of the real C arrays through
  shims). **TIER 4** (`static` in C, no exported symbol, hand-derived vectors):
  the three ref-frame percentage helpers, `rc_init_frame_stats`, `get_ref_obj`,
  `update_rc_counts`, `clamp_qp`/`clamp_qindex`, `generate_sb_qindex`'s control
  flow, and the `rc_vbr_cbr.c` / `rc_rtc_cbr.c` / `pass2_strategy.c` scalar
  cores. Two rows the inventory reported as ported were doc-comment substring
  hits with no implementation — `svt_av1_rc_init` and `generate_sb_qindex` —
  and both now exist. (cb6fa82, 1dc29e4, 1920b89, 7f9cfac, 62167c8, a9db88f,
  7434410, 0ed920f)


- **`transforms.c`'s reduced-coefficient-shape family is ported, tier 1 —
  76 of 76 `_N2` / `_N4` / `ONLY_DC` functions plus the entry points above
  them.** New `svtav1-dsp/src/fwd_txfm_pf.rs`: the 26 pruned 1-D kernels
  (`fdct{4,8,16,32,64}`, `fadst{4,8,16}`, `fidentity{4,8,16,32,64}` in both
  shapes), `fwd_txfm_type_to_func_N2/_N4`, one 2-D core covering
  `av1_tranform_two_d_core_{N2,N4}_c` **and** `av1_tranform_two_d_core_c`
  (`div == 1` reduces to it exactly), all 57 exported 2-D entries, the 54
  `highbd_fwd_txfm_WxH{,_n2,_n4}` wrappers as one table,
  `svt_av1_highbd_fwd_txfm{,_n2,_n4}`, `svt_av1_wht_fwd_txfm` (TPL's only
  transform entry), the ten `svt_handle_transform*{,_N2_N4}_c`,
  `svt_aom_estimate_transform` + its four static shape dispatchers,
  `svt_aom_transform_config`, `svt_av1_gen_fwd_stage_range`,
  `set_fwd_txfm_non_scale_range` and `svt_av1_get_inv_txfm_cfg`.
  Evidence tier 1 throughout (42 tests in `c_parity_txfm_pf{,_2d,_entry}.rs`
  and `c_parity_estimate_transform.rs`, new shims in
  `svtav1-cref/shims/txfm_pf_shims.c`); workspace 1418/1418. Byte-inert on
  the existing envelope — nothing calls the new module yet; it is the
  transform surface TPL needs for `ppcs->r0`, which the video-mode CRF
  qindex derivation (campaign chunk C1a) is gated on.
  (352cfa0f, 1f085b65, 348ab209, 103ab793)
- Two upstream defects recorded in `rust/docs/SUSPECTED-C-BUGS.md` #12 and
  #13, both found while gating the above: `highbd_fwd_txfm_4x16_n2/_n4` call
  the UNPRUNED 4x16 transform (alone among 18 siblings), and
  `svt_av1_fwd_txfm2d_*_neon` NULL-derefs at bd > 8 for any ADST-containing
  tx_type on a 32-dimension block.

- **`svt_aom_generate_av1_mvp_table`'s ref loop is now gated too — chunk C2,
  evidence TIER 1.** The per-ref sweep could not reach it: C keeps ONE
  `Mv mv_ref0[64]` across the whole `ref_frames` loop (adaptive_mv_pred.c:1336)
  and the `symteric_refs` shortcut in `add_tpl_ref_mv` depends on that sharing
  — the `LAST_FRAME` pass stores a projected MV in slot *i* and the
  `BWDREF_FRAME` / `LAST_BWD_FRAME` passes read it back. The C shim now takes
  the scratch IN and OUT, so `c_parity_generate_av1_mvp_table_threads_mv_ref0`
  drives the oracle three times threading it exactly as C's loop does. Teeth
  verified: dropping the threading in the port fails the cell
  (`stack[0]` 0 against C's 0x1e6_4bb2 at `rf=BWDREF`), and the cell
  additionally re-runs each ref with a FRESH scratch and requires the answers
  to differ in more than 10 cells — so it cannot pass vacuously against a port
  that restarts the scratch per ref.
- **`svt_aom_mode_context_analyzer` and the OBMC overlappable-neighbour counts
  — chunk C2, evidence TIER 1.** `mode_context_analyzer`
  (inter_prediction.c:2565) collapses `setup_ref_mv_list`'s packed mode context
  into the single compound context through `svt_aom_compound_mode_ctx_map`;
  `count_overlappable_neighbors` (adaptive_mv_pred.c:1893) plus its two static
  helpers `count_overlappable_nb_{above,left}` (:1830, :1864) produce
  `blk_ptr->overlappable_neighbors`, the OBMC gate. Both are gated against the
  exported C symbols in `tests/c_parity_inter_mvp.rs` (now 8 tests): the
  analyzer over every context `setup_ref_mv_list` can emit crossed with single
  and both kinds of compound pair, and the neighbour count over randomized
  grids with a high 4xN population — which is what drives the `mi_step == 1`
  arm that rewinds the LOOP VARIABLE before reading the cell to its right.

- **OBMC motion search — the other half of `av1me.c` (`inter_me/obmc_search.rs`)
  — chunk C4, evidence TIER 1.** `av1me.c`'s IntraBC half was already in
  `intrabc.rs`; this completes the file: `get_obmc_mvpred_var`,
  `obmc_refining_search_sad`, `svt_av1_obmc_full_pixel_search`,
  `set_subpel_mv_search_range`, `setup_obmc_center_error`,
  `upsampled_obmc_pref_error`, `upsampled_setup_obmc_center_error`,
  `sp`/`pre`/`search_step_table` and
  `svt_av1_find_best_obmc_sub_pixel_tree_up`, plus the four C_DEFAULT kernels it
  drives that nothing in this port needed yet (`obmc_sad`, `obmc_variance`,
  `obmc_sub_pixel_variance` with both bilinear passes, `svt_aom_upsampled_pred`
  and `svt_aom_convolve8_{horiz,vert}`). Nothing calls it yet. Gated by
  `tests/c_parity_obmc_search.rs` (9 tests: the kernel families over 10 block
  sizes x 64 sub-pel offsets, `convolve8` both directions, `upsampled_pred` over
  all offsets x {2,4,8}-tap, and BOTH search drivers against the real C with an
  `IntraBcContext` + `ModeDecisionContext` assembled in the shim).

- **Recorded upstream defect: every NEON `obmc_sub_pixel_variance` above 4x8 is
  the 4x8 kernel** (`docs/SUSPECTED-C-BUGS.md` #11). `aom_dsp_rtcd.c:731-750`
  aliases all 20 sizes from 4x16 to 128x128 to
  `svt_aom_obmc_sub_pixel_variance4x8_neon`; measured on macOS aarch64 for
  `BLOCK_8X16` across all 64 offsets, the RTCD result is bit-identical to the
  `_c` 4x8 kernel. `obmc_sad`/`obmc_variance` in the same block and the x86 SSE4
  table are correct. The port follows the C SOURCE; the test suite compares the
  live `USE_8_TAPS` path against the C binary everywhere and the `osvf` path only
  where a control test proves this host's dispatch is faithful — that control
  fails the day upstream fixes the table (fb5f8fa).

- **Open-loop motion estimation — a wholesale port of `motion_estimation.c`
  (`inter_me/`) — inter-encode campaign chunk C4, evidence TIER 1 where a C
  symbol exists.** All 40 functions of SVT-AV1's 2,964-line
  `Source/Lib/Codec/motion_estimation.c`, in a new module tree: the seven SAD
  accumulators plus the two `compute_sad_c.c` loop kernels (`sad.rs`),
  `MeContext` and the padded-plane view (`context.rs`), pre-HME + HME levels
  0/1/2 + the search-area derivation + `check_00_center` +
  `set_final_search_centre_sb` + the two reference-pruning ladders (`hme.rs`),
  the one- and eight-point search-point blocks + `integer_search_b64` +
  `me_prune_ref` (`integer.rs`), the three ME candidate-array constructors +
  global-motion detection + `compute_distortion` (`candidates.rs`), and
  `init_me_hme_data` / `me_static_b64_bypass` / `svt_aom_motion_estimation_b64`
  (`b64.rs`). Deliberately NOT ported: `get_me_reference`'s `SVT_WARN` log line
  (its `*dist` output IS ported) and the `tf_*` half of `MeContext` that belongs
  to `temporal_filtering.c` — the five `tf_` fields `motion_estimation.c` reads
  are carried. **Nothing calls it yet**: `motion_est.rs`'s homegrown searcher is
  still what `partition.rs` and `pipeline.rs` use, and moving those call sites
  is a separate chunk. Gated by `tests/c_parity_inter_me.rs` (11 tests, tier 1
  against the real `libSvtAv1Enc.a` via the new `shims/inter_me_shims.c`:
  `svt_aom_compute8x4_sad_kernel_c`, `svt_nxm_sad_kernel_helper_c`,
  `svt_sad_loop_kernel_c`, the four `svt_ext_*sad_calculation*_c` accumulators,
  `svt_aom_get_scaled_picture_distance` exhaustively over all 65,536 inputs,
  `hme_level_2` and `check_00_center`) and `tests/inter_me_traced.rs` (18 tests:
  `hme_level_0`/`hme_level_1`/`prehme_core` against the REAL C `hme_level_2` in
  the domain where the C bodies coincide, an eight-point-vs-eight-singles
  structural invariant, a pure-translation recovery test through
  `motion_estimation_b64`, and hand-traced vectors for the remaining `static`
  bookkeeping — labelled tier 4 in the file). MEASURED finding recorded at the
  function: pre-HME does not round its search width up to a multiple of 8 and
  does not apply the `& ~7` round-down after the right-edge crop, so it searches
  a different column count than the HME levels near a right edge (1194e4b,
  8224df7, 9f41610).

- **Inter-frame MVP (motion-vector-predictor) stack (`inter_mvp.rs`) —
  inter-encode campaign chunk C2, evidence TIER 1.** The general
  (`ref_frame > INTRA_FRAME`) branch of `adaptive_mv_pred.c` that
  `intrabc_mvp.rs` could not reach: `add_ref_mv_candidate`'s compound arm and
  its `is_global_mv_block` substitution, the temporal (MFMV) candidates
  (`add_tpl_ref_mv` + `get_mv_projection` + `lower_mv_precision`, both the
  single and compound projections and the `symteric_refs` LAST/BWD shortcut
  with its `mv_ref0[64]` scratch threaded across the ref loop as C does),
  `scan_row_col_light`'s compound arm and the `ref_frame_sign_bias` flips in
  both arms, `setup_ref_mv_list`'s MFMV block including the `sb64_sq_no4xn_geom`
  walk and the 3-position extension, `svt_aom_gm_get_motion_vector_enc`,
  `svt_aom_generate_av1_mvp_table`'s inter `gm_mv` derivation,
  `svt_aom_get_av1_mv_pred_drl`, `svt_aom_compute_inter_mode_ctx_light`, the
  compound-aware `svt_av1_get_ref_mv_from_stack` /
  `svt_av1_find_best_ref_mvs_from_stack`, and `av1_set_ref_frame` /
  `av1_ref_frame_type` / `get_list_idx` / `get_ref_frame_idx`.
  `tests/c_parity_inter_mvp.rs` drives the REAL exported C symbols through new
  shims (`crates/svtav1-cref/shims/inter_mvp_shims.c`, its own translation unit
  so concurrent lanes never share a shim file) over randomized inter mode-info
  grids: 4,000+ cases across 14 ref-frame types (7 single + 7 compound), 11
  block sizes, two tiles, both SB sizes, MFMV on/off, high-precision MVs on/off
  and four global-motion model classes, comparing the full 8-slot stack
  (`this_mv`, `comp_mv`, weight), the count, the mode context, nearest/near and
  the `mv_ref0` scratch. The MFMV anti-vacuity check re-runs the C oracle with
  the block disabled and requires the temporal candidates to CHANGE the output,
  not merely to execute.
- **Motion-field projection (`get_block_position`, `motion_field_projection`,
  `setup_motion_field` in `inter_mvp.rs`) — chunk C2, evidence TIER 4.** All
  three are `static` in `md_config_process.c` and export no symbol (verified
  with `nm -gU Bin/Release/libSvtAv1Enc.a`), so
  `tests/inter_mvp_motion_field.rs` pins them with hand-derived vectors traced
  against the C source, each with its arithmetic written out beside it. Covers
  the `is_lst_overlay` suppression, the KEY/INTRA_ONLY and resolution-mismatch
  refusals, `ref_frame_side` ahead/coincident/behind, and the
  `use_ref_frame_mvs == 0` early return that leaves `tpl_mvs` untouched.

- **Inter MV entropy coding + MV rate (`inter_mv_code.rs`) — inter-encode
  campaign chunk C3, evidence TIER 1.** The layers between the already-gated
  MV symbol writer (`entropy/mv_coding.rs`) and the already-gated cost-table
  build chain (`intrabc.rs`): the `force_integer_mv` precision override
  `svt_av1_encode_mv` performs internally (entropy_coding.c:1498-1500), the
  per-inter-mode dispatch deciding WHICH of a block's MVs are coded
  (:5216-5244) and priced (rd_cost.c:1088-1128), the full
  `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-488 — the
  `approx_inter_rate` zero-fill early return, the hp/non-hp stack selection,
  the `allow_intrabc` dv arm), the CDF adaptation `av1_update_mv_stats` /
  `update_mv_component_stats` (:650-705), `reset_nmv_counter`
  (cabac_context_model.c:1956), `avg_nmv` (enc_dec_process.c:2567), the
  `update_mv` cadence (`set_cdf_controls`, enc_mode_config.c:8468-8498) and
  `copy_mv_rate` + the per-SB rebuild-vs-copy choice its two call sites make
  (enc_dec_process.c:36-56, :2802-2806, :2908-2912), and `svt_aom_mv_err_cost`
  / `_light` over the NMV tables (av1me.c:141/:126 — the arm the inter sub-pel
  search reads through `x->mv_cost_stack`, which `c_parity_intrabc.rs` covers
  only at `MvSubpelPrecision::None` over the DV tables). `FrameContext` gains
  the `nmvc` field C's `FRAME_CONTEXT` has beside `ndvc` (seeded from the same
  `default_nmv_context`, cabac_context_model.c:794-795) and `avg_cdf_with` now
  averages BOTH through the ported `avg_nmv`, as C's `avg_cdf_symbols` does
  (enc_dec_process.c:2638-2639), replacing an inline re-enumeration of `ndvc`'s
  fields. Byte-neutral: nothing reads `nmvc` yet (the inter refusal still
  stands), and averaging two equal contexts is the identity — pinned by
  `avg_nmv_matches_the_previous_inline_ndvc_enumeration` (replays the old
  inline code verbatim), `avg_cdf_with_actually_averages_nmvc` (anti-vacuity:
  fails if the new call is dropped) and `nmvc_defaults_and_is_inert_under_avg`, and
  MEASURED at the bitstream level in
  `benchmarks/nmvc_avg_byte_neutrality_2026-08-31.md` — 32 / 32 cells
  identical with the new averaging present vs removed, with `avg_cdf_with`
  proven REACHED (an `eprintln` probe fires 2x/frame at presets 0/4/6 on a
  3x3-SB frame, 0x at preset 8) and proven able to MOVE bytes (halving
  `partition_cdf` in the same place changes 12 / 12 cells). That record also
  documents the trap the run hit: a first, weaker control changed no byte and
  read exactly like "never called".
  The module docs also record the nine-step emission order around the MV write
  (entropy_coding.c:5196-5300), which of those steps have no port, and the two
  traps in it — the DRL predicate being a different mode set from the MV
  predicate, and the MV write reading the already-`lower_mv_precision`-rounded
  `predmv[ref]` rather than a raw ref-MV-stack entry.
  Gate: `crates/svtav1-encoder/tests/c_parity_mv_code.rs`, 17 tests driving
  the REAL exported symbols `svt_av1_encode_mv`, `svt_av1_get_mv_joint`,
  `svt_aom_estimate_mv_rate`, `svt_av1_mv_bit_cost{,_light}`,
  `svt_aom_have_newmv_in_inter_mode`, `svt_av1_reset_cdf_symbol_counters` and
  `svt_aom_get_update_cdf_level_{default,rtc,allintra}` through three new
  `svtav1-cref` shims. Unlike the pre-existing `c_parity_mv.rs` (a C-side
  transcription, default context, `ref_mv == 0`, bytes only) this drives the
  real writer from RANDOMIZED `NmvContext`s with NONZERO reference MVs across
  every (`allow_high_precision_mv`, `force_integer_mv`, `allow_update_cdf`)
  combination and compares the ADAPTED CDF STATE as well as the bytes.
  Teeth proved by six mutations (precision override, hp-bit stats update,
  rate-table precision, the mode→ref plan, one `reset_nmv_counter` field, one
  `avg_nmv` field) — each caught, naming the diverging context field.
  Records a C asymmetry deliberately reproduced: under `force_integer_mv` the
  WRITER codes at `MV_SUBPEL_NONE` while the RATE tables are still built at
  `MV_SUBPEL_LOW_PRECISION`, because `svt_aom_estimate_mv_rate` passes
  `allow_high_precision_mv` straight in and never consults `force_integer_mv`
  (pinned by `c_parity_rate_tables_ignore_force_integer_mv`). NOT WIRED: the
  public entry point still refuses inter frames at `pipeline.rs`'s `if !is_key`
  guard, and `FrameContext` still carries no `nmvc` field — both belong to the
  chunks that own those files.

- **`AvifEncoder::encode_yuv420` emits a REAL AV1 bitstream — issue #9 item 6.**
  It returned three concatenated MONOCHROME streams behind u32 length prefixes,
  as `Ok(...)`, which no decoder accepts. It now routes through
  `EncodePipeline::with_chroma_420(true)` + `try_encode_frame_420` — the same
  4:2:0 path every C-oracle gate covers — and is asserted BYTE-IDENTICAL to
  driving that pipeline directly with the same config
  (`encode_yuv420_is_the_mainline_420_path_byte_for_byte`). It also no longer
  pre-pads: the pipeline signals the TRUE frame size and pads internally, so a
  98x66 image is a 98x66 stream. **Gate: `tools/decode_conformance.sh <dir>
  avif` — a new corpus driven entirely through `AvifEncoder`'s public entry
  points, 240/240 streams decode under BOTH aomdec and dav1d** (120 4:2:0 +
  120 monochrome, sizes {32,48,64,66,98,128} x qualities {10,35,60,85} x
  speeds {1,5,6,8,10}).
- **`AvifEncoder::with_lossless(true)` is now honoured on 4:2:0** — it sets
  QP 0, the coded-lossless path issue #5 landed byte-identically to C. On the
  monochrome path it stays a typed `UnsupportedConfig` (the mono leaf coder has
  no lossless arm). Same for `quality > 99.2`, which maps to QP 0. This is the
  first CAPABILITY refusal this port has ever RETIRED: the inventory goes 15 ->
  14 capability refusals.

- **Fractional CRF — issue #9 item 4.** `RcConfig` gains
  `extended_crf_qindex_offset: u8` (quarter-qindex steps) and the
  `RcConfig::crf(f32)` constructor that splits a fractional `--crf` exactly as
  C's `str_to_crf` does (enc_settings.c:1655-1670): `--crf 35.25` is
  `qp = 35, offset = 1`. The offset is consumed where C consumes it —
  `scs_qindex = clamp_qindex(quantizer_to_qindex[qp] +
  extended_crf_qindex_offset)` (rc_crf_cqp.c:471) — and the extended
  63.25..70 range's frame `lambda_weight` bump (`+= offset * 28`,
  enc_mode_config.c:10109-10114) is applied too. **The port now keeps C's TWO
  qp values apart:** `static_config.qp` (the CLI value, unchanged by the
  offset) still keys every level derivation, while `ppcs->picture_qp =
  (base_q_idx + 2) >> 2` (rc_process.c:861) keys only the frame
  `lambda_weight` ladder. Collapsing both onto the qindex-derived value
  diverged from C at preset 2 / qp 20 / offsets 2-3 — measured, then fixed.
  Offset 0 makes the two equal, so every pre-existing cell is unchanged.
  **Gate: `tools/issue9_knobs_gate.sh`, fractional-CRF cells 19/19
  byte-identical to the C oracle** (presets 2/6/10 x qp 20/40 x offsets 1-3,
  plus the qp-63 extended-range cell), with an anti-vacuity check that fails
  if a knob never moves the C oracle's own bytes.
- **`max_tx_size` (32|64) — issue #9 item 3, now byte-gated.** Already
  threaded through the PD0 scan and the depth refinement
  (enc_dec_process.c:1494-1500 / :1815); `tools/issue9_knobs_gate.sh` adds the
  C-oracle cells that prove it: **9/9 byte-identical** at
  `max_tx_size = 32` over presets 2/6/10 x qp 20/40/55.
- **`chroma_sample_position` — issue #9 item 5.**
  `EncodePipeline::with_chroma_sample_position(0|1|2)` writes the two 4:2:0
  `color_config` bits C writes from `static_config.chroma_sample_position`
  (entropy_coding.c:2743); 3 is reserved and refused at encode time, matching
  `verify_settings` (enc_settings.c:762). Default 0 (CSP_UNKNOWN) keeps every
  pre-existing stream bit-identical. **Gate: 2/2 byte-identical** cells
  (vertical + colocated).
- **`EncodePipeline::knob_config_error`** refuses the three configurations C
  rejects in `svt_av1_verify_settings` rather than encoding them:
  `max_tx_size` outside {32, 64}, an `extended_crf_qindex_offset` above 3
  (or above 28 at qp 63), and `chroma_sample_position > 2`.

- **Coded-lossless (QP 0) ENCODES — issue #5 chunk 2, the tile half.** The
  refusal is gone on the 8-bit 4:2:0 still path (mainline mode, no
  screen-content tools, no superres); every arm outside that envelope keeps
  a typed `UnsupportedConfig` (`EncodePipeline::lossless_config_error`,
  ledgered in `docs/REFUSED-CONFIGS.md`). What landed, each cited to C:
  the forced 8x8 / TX_4X4 partition tree (`pd0::lossless_tree` —
  `max_sq_size` 8 under `mimic_only_tx_4x4`, enc_dec_process.c:1492), the
  4x4 Walsh-Hadamard forward + inverse in `tx_unit` with C's transposed
  coefficient store (transforms.c:3950, inv_transforms.c:3141 — the u16
  scratch + `highbd_iwht4x4_16_add` always, never the eob<=1 shortcut),
  depth 1 forced at EVERY MD stage incl. a per-txb-predicted MDS1 loop
  (product_coding_loop.c:6734 inside `full_loop_core`), RDOQ and the
  tx-type search off (full_loop.c:1756, :7065/:7173), the DCT-chroma-only
  candidate filter on the regular / filter-intra / palette injection lists
  and both uv searches (mode_decision.c:3245/3298/3393,
  product_coding_loop.c:7376/7584 — which collapses the intra set to
  {DC, PAETH} at qp 0), zero tx_size bits priced (rd_cost.c:1755) and no
  tx_size symbol coded (entropy_coding.c:4657), RDOQ level 0, and
  deblock / CDEF / LR neither searched nor applied
  (md_config_process.c:1022-1035). `FunnelCfg::apply_coded_lossless` is C's
  `txs_level = 1` override. **Gate: `tools/lossless_gate.sh`** (in CI on the
  72-cell 64x64 + 96x80 subset): byte-identity to C AND `aomdec --rawvideo`
  output == the source planes, per cell. Local arm64 run 2026-08-28
  (`benchmarks/lossless_gate_2026-08-28.md`): **112 / 144 byte-identical +
  32 pinned, 144 / 144 lossless** — presets 4..13 are 96/96 across
  {gradient, diag, uniform} x {64x64, 128x128, 96x80, 200x136}; presets
  0..3 on textured content are pinned self-promotingly (lossless in both
  encoders, e.g. gradient 64x64 p3 port 2966 B vs C 2973 B; root by
  elimination — the port's p3 == its p4 == C's p4, and the only M3-boundary
  knob live at qp 0 is `svt_aom_get_disallow_4x4_allintra`: all-intra
  allows 4x4 partitions at <= M3, so C's lossless partition search decides
  8x8-vs-four-4x4 per block where the port forces 8x8 leaves; real CID22
  crops at 64x64/512x512 are 8/8 byte-identical at p7/p12 and lossless at
  p1). Neutral at qp >= 1 by
  construction and by measurement: identity_matrix 54/54, bd10_matrix 36/36,
  regression_spotcheck 33/33, workspace 1051/1051. In-crate witnesses:
  `tests/lossless_fh_c_capture.rs::qp0_coded_lossless_stream_matches_c_capture`
  (full-stream equality to the committed C capture; MUTATION-VERIFIED —
  DCT instead of WHT: 3759 B vs 2699 B; tx_size symbol coded: 2702 B) and
  `pipeline::tests::qp0_420_encodes_losslessly_and_out_of_envelope_arms_refuse`
  (recon == source at qp 0, lossy at qp 1, 10-bit and fork refused).
  `AvifEncoder`'s three-monochrome-stream surface still refuses lossless
  (the mono leaf coder has no WHT arm); its messages now say where QP 0 is
  available. Also fixed while gating: rustc 1.98 clippy on `restoration.rs`
  (`as_chunks` in the three NEON/AVX2 stats kernels, byte-neutral),
  `picture.rs`, and the `zensim_census` example.
- **Issue #8 doc-debt residuals closed, and `rust/Cargo.lock` is now
  committed.** The lock decision the audit asked for: the product is a
  byte-identical bitstream and `archmage` is a semver dependency, so an
  unpinned fresh-box resolve could change codegen under the gates; the lock
  pins what CI measured (`rust/README.md` "Building"; `cargo update` is its own
  commit). Both READMEs' gate tables are re-tallied from CI run 33101031800
  (`1ed7db46`) and split into CI-run vs corpus-gated-local blocks with each
  local number dated and its committed artifact named (or "no committed
  artifact" said outright — the 177/180 `real_image_matrix` figure was one;
  the committed real-corpus record is the 450-cell
  `identity_full_8bit_real_2026-08-03.tsv`, 403 IDENTICAL, p6/p10/p13
  90/90). `rust/README.md`'s "197/309 non-flat" was an arm64 measurement
  (309/309 on x86 CI); "Rust 1.85+" is 1.89 (the real `rust-version`); test
  counts are 1056 as of `1ed7db46`. Unbacked MEASURED numbers are now
  labelled as such at the citation (CLAUDE.md kernel throughput,
  perf-status.md's never-committed `perf_{before,after}_cdef.tsv`,
  HDR-ON-4.2.md 48/48, ACCEPTANCE-CRITERIA 0/36, bd10-port-map's 540-cell
  `/tmp` sweep, ibc-port-map's 25,356 blocks). Docs that described landed
  work as open carry a dated STATUS banner verified against source:
  finishing-survey D7 + ibc-port-map §B (IntraBC is wired, `allow_intrabc`
  derived, dsp placeholder deleted), C-TEST-PORTING-AUDIT 1h (superres
  ported + CI-gated; `scale.rs` still the pinned stub), STATUS.md
  "Architecture direction", practical-usage-plan, sc-detection-port-map,
  arbitrary-dims-port-map, IDENTITY-STATUS, `specs/README.md` (pinned
  pre-v4.2.0; the C tree wins). Per-gate wall-clock budgets exist for the
  first time: `rust/benchmarks/gate_wallclock_ci_2026-08-27.md` (every CI
  step's duration from the same run; the job is ~21 min, the three largest
  steps 207/167/141 s), linked from WORKING-ON-THIS.md §2b.
- **CI caches the cargo-built C oracle, keyed on the submodule SHA — issue
  #4 invariant C's last open piece.** `.github/workflows/rust-gates.yml`
  restores `Bin/Release` + `Bin/ReleaseHdr` (lib, `SvtAv1EncApp`, the
  `.zenav1-cref-stamp`) from `actions/cache` under a key of `<submodule HEAD>
  + hash(build.rs)`; on a hit `cargo build -p zenav1-svt-cref` is a stamp
  no-op. Only the output dirs are cached, on purpose: a restored ninja tree
  would see the fresh checkout as newer than every object and rebuild inside
  the shell tools' silent `cmake --build` freshness check. Measured cost being
  removed: 141 s per run (run 33101031800). `actions/checkout` v5 -> v7 and
  `actions/cache` v4 -> v6 (the v4 entry was on the Node 20 deprecation
  path).
- **`coverage_combos_gate.sh` runs in CI (issue #8 item 7) with a
  caller-selected axis set.** New `CC_AXES` env (default `sb128 bd10 real`);
  CI passes `sb128 bd10` because axis 3 needs the CID22 / gb82-sc corpora, so
  the skip lives in the workflow, not in a file-exists check; a run that
  selects no axis exits 2 rather than reporting 0/0. Also `wc -c` replaces
  `stat -c%s` (BSD stat has no `-c`; the byte columns were empty on macOS).
  Local arm64 measurement of axes 1+2 before wiring:
  `benchmarks/coverage_combos_2026-08-28_arm64_axes12.{tsv,meta}` — 26/28,
  SB128 x tiles 16/16 byte-exact, and TWO bd10 `diag 256x256` eff-M9 cells
  whose single-tile CONTROL diverges on this host; the x86 CI run is the
  arbiter (bd10 non-flat is ISA-dependent on the C side here, STATUS.md
  "Measurement caveat for arm64 hosts").
- **Coded-lossless frame header, byte-identical to C — issue #5, chunk 1 of
  the lossless envelope.** `key_frame_header_bits_lr` now derives
  `CodedLossless` / `AllLossless` (spec 5.9.2: `base_q_idx == 0` with zero
  chroma deltas, segmentation off; AllLossless additionally unscaled) and, like
  C `write_uncompressed_header_obu` (entropy_coding.c:3594-3612), writes no
  `loop_filter_params()`, `cdef_params()`, `lr_params()` or `tx_mode_select`
  bits when it holds (`delta_q_present` was already gated on `base_q_idx > 0`).
  Witness `crates/svtav1-encoder/tests/lossless_fh_c_capture.rs` against a
  committed C capture (`tests/data/c_gradient64_p7_qp{0,1}.obu`, 64x64
  gradient, preset 7): the qp-0 header is a byte prefix of C's frame OBU and
  strictly shorter than the qp-1 header from the same parameters; the qp-1
  control reproduces C's temporal delimiter, sequence header and frame-header
  prefix so the parameter set is known-good. The qp-0 capture was checked to
  decode LOSSLESSLY (aomdec output == source) before being adopted as an
  oracle — `SUSPECTED-C-BUGS.md` #1's variance-boost caveat does not bite on
  mainline defaults. Mutation-verified (forcing `coded_lossless = false`
  fails the qp-0 test). **The public envelope is unchanged: QP 0 is still
  refused** — the tile half (TX_4X4-only coding with no tx_size / tx_type
  symbols, WHT residuals, lossless MD gates: `mds_do_txt = 0`, RDOQ off,
  `svt_av1_is_lossless_segment` sites in product_coding_loop.c:7065/7173/
  7376/7584, full_loop.c:1756/1925/1936, rd_cost.c) is the next chunk, and
  the refusal comes off only when that is byte-verified against this oracle.
- **The C reference oracle is cargo-driven, both variants, SHA-stamped —
  issue #4 invariants B and C.** `crates/svtav1-cref/build.rs` used to link a
  prebuilt `Bin/Release/libSvtAv1Enc.a` and panic with a cmake line to type by
  hand; the `SVT_HDR_MODE=ON` oracle had no cargo path at all. Now a fresh
  clone's first `cargo test` configures and builds `Bin/Release`
  (mainline, `BUILD_APPS=ON` — the config CI and every shell tool already
  assumed) and `Bin/ReleaseHdr` (fork), each stamped with the submodule's git
  SHA + a config key in `.zenav1-cref-stamp`, so an unchanged tree never
  rebuilds and a submodule move triggers an incremental `cmake --build`. A
  pre-stamp hand build is trusted and stamped rather than rebuilt. Missing
  cmake / cc / nasm (x86 only) panics with the install one-liner.
  `SVT_CREF_LIB_DIR` (link a prebuilt archive, build nothing) and
  `SVT_CREF_SKIP_HDR=1` are the knobs. CI's hand-typed cmake step is replaced
  by `cargo build -p zenav1-svt-cref`. Measured locally (Apple-silicon
  laptop, ninja + clang, `-j4`): mainline from nothing 245 objects / 15.6 s
  wall, fork 238 objects / 16.0 s, the second `cargo build` a 0.75 s no-op;
  the differential suites and `regression_spotcheck.sh` pass against the
  cargo-built oracle.
- **CI runs the pure-Rust tier on `windows-11-arm`, `macos-15-intel` and
  `i686-unknown-linux-gnu` (via `cross`)** — issue #4 phase 4. The tier is
  `cargo build --workspace --exclude zenav1-svt-cref` + `cargo test -p
  zenav1-svt`: the facade's dev graph has no cref dependency, so the e2e /
  golden-parity / SIMD-tier-invariance / issue-repro suites run with no C
  toolchain. Corpus and decoder skips are set at workflow scope
  (`ZENAV1_SKIP_CORPUS_TESTS`, `ZENAV1_SKIP_DECODER_TESTS`). Three
  `real_encode.rs` tests wrote to a literal `/tmp`; they use the OS temp dir.
- **10-bit encoding at NON-64-ALIGNED dimensions — the product case for 10-bit
  AVIF** (`bd10_partial_sb_gate.sh`, **157/157 byte-identical to the C
  reference**; every one of those cells was a refusal before). Both bd10 level
  producers now handle partial superblocks: the full-RD funnel (preset ≤ 8),
  which needed only the gate lifted because it rides the same partition search
  and leaf funnel as the already-partial-SB-correct 8-bit path; and the
  level-only re-encode post-pass (preset ≥ 9), which needed SB-extent-sized
  recon buffers, straddle-clipped recon writes, SB-extent-padded 10-bit
  sources, and the pack's skip-off-frame-quadrant child walk in place of a
  fixed `(partition_type, children.len())` offset table that a pruned
  partial-SB child list makes both `panic!`-prone and positionally wrong.
  `bit_depth_config_error` no longer refuses ANY 10-bit configuration on
  dimension grounds; `docs/REFUSED-CONFIGS.md` drops 12 → 10 CAPABILITY
  refusals, and `arbitrary_size_robustness.sh` goes from 80/80 with **48
  refused** to **128/128 with 0 refused** — those 48 are exactly these cells,
  and every one now decodes under the AV1 reference decoder.
  Data: `benchmarks/bd10_partial_sb_2026-08-04.tsv`; full record in
  `docs/bd10-port-map.md`. Residual (NOT closed, pinned self-promotingly in the
  gate): a set of non-flat cells, measured to be the known bd10 non-flat gap
  (21.5% of non-flat cells at 64-aligned dims vs 26.3% at partial-SB dims;
  `uniform` is 100% everywhere) rather than a partial-SB gap.

- **MAINLINE's chroma-q derivation is ported (tune IQ), refs #9 item 2.**
  `rc_crf_cqp.c` has TWO chroma-qindex blocks separated by `#if SVT_HDR_MODE`
  and this port had only the fork one, gated behind `is_fork()`, so mainline
  always emitted zero chroma deltas. The mainline arm (`:592-602`) is a
  DIFFERENT derivation, not a subset: the ramp is off `new_qindex` rather than
  the post-offset value, the clip ceiling is 16 rather than 12, and U gets no
  `+12` (both planes carry the same delta). Found by
  `tools/identity_diff.sh` on `gradient 128x128 q40 p6 SVT_TUNE=3`, which put
  the first divergence at `FH delta_q_u_dc.coded C=1 Rust=0` with the tile
  payload already the same size on both sides. All-zero at any tune but IQ, so
  every non-tune-IQ cell is byte-identical by construction. Tune IQ is still
  NOT byte-identical to C — the knobs gate is 31/36 with a 1-6 byte tile-payload
  residual — but the frame header now matches.
- **PQ-shaped 10-bit source + a photographic native-10-bit gate (issue #7 /
  task #6 chunk 2b).** `identity_run` gains `SVTAV1_HBD_PQ`: the 8-bit luma is
  linearized as sRGB, mapped to a 1000-nit display, run through the SMPTE
  ST 2084 (PQ) OETF and quantized to 10-bit limited range; chroma is rescaled
  8-bit limited -> 10-bit limited. The low bits are then a consequence of a
  real transfer curve rather than the synthetic `(3r + 5c + v) % 4` pattern the
  chunk-2 gate uses, and the code-value histogram is PQ-shaped. Two gates
  consume it: a corpus-free PQ tier inside `tools/bd10_hbd_src_gate.sh`
  (**18/18 byte-identical**, and it runs in CI where a photographic gate
  cannot — no runner has the corpora), and the new
  `tools/bd10_hbd_pq_gate.sh` on real CID22-512 photographs (**presets 8 and 9
  40/40 byte-identical**; preset 6 carries 12 `uname -m`-scoped aarch64 pins,
  see below).
- **Measured: C's per-host bitstream divergence is far wider at bd10 than
  `docs/SUSPECTED-C-BUGS.md` #9 recorded.** Same commit, same port binary:
  `bd10_nonflat_gate.sh` is 309/309 in CI (x86-64) and **197/309** locally
  (aarch64/macOS); `bd10_photo_gate.sh` (not in CI) is **53/191** locally. The
  port is not the variable side — `tier_invariance.rs` holds its bytes across
  every dispatch tier, and failing photographic cells were re-encoded by a
  build of the pre-session tree (`bfae1b69`) with byte-identical output. Flat
  and low-complexity synthetic content agrees on both hosts; non-flat and
  photographic content diverges. Entry #9 now carries the table and the
  quantified case for an aarch64 CI runner.

- **`RcConfig::aq_mode != 0` is now REFUSED (issue #9 item 8).** C's
  `--aq-mode` default is 2 and it is INERT for a single still — aq-mode-2's
  deltaq is TPL-gated (`rc_aq.c:899`) and one frame has no lookahead — while
  this port's non-zero `aq_mode` ran a HOMEGROWN frame-level VAQ/TPL qindex
  shift that is a port of nothing. So `aq_mode = 2`, the value a caller copies
  straight out of C's documentation, meant "C: no change" and "port: shift the
  whole frame". Refused rather than documented, because documentation does not
  stop a caller from copying C's default. `0` (the default) is the value that
  matches C. C's segmentation-side `aq_mode` is a different parameter and stays
  C-parity-tested.
- **`SpeedConfig` lost 12 dead fields (issue #9 item 9).** `enable_cdef`,
  `enable_restoration`, `enable_cfl`, `enable_palette`, `enable_identity_tx`,
  `enable_obmc`, `enable_warped_motion`, `enable_compound`,
  `subpel_precision`, `hme_levels`, `me_search_width`, `me_search_height` had
  ZERO consumers anywhere in the workspace while reading as an authoritative
  preset table — two tests asserted `enable_palette` / `enable_obmc`, which
  tested nothing but the table's own initializer. Note the issue's own list was
  partly wrong and is corrected here: **`max_intra_candidates` is LIVE**
  (`PartitionSearchConfig::from_speed_config` → the NIC cap at
  `partition.rs:2206`), as are `enable_adst`, `enable_directional_modes`,
  `enable_filter_intra`, `rdo_tx_decision`, `max_partition_depth`,
  `lambda_scale` and `preset`; `enable_temporal_filter` is read on the dormant
  inter path and stays.

### Changed

- **`AvifEncoder` has no inert knobs left — issue #9 item 7.** Two are now
  wired to the real pipeline settings, each with a liveness test that fails if
  the knob stops moving the emitted bytes:
  - `with_qm(bool)` -> `EncodePipeline::hdr.enable_qm`.
  - `with_variance_boost(bool, u8)` -> `hdr.{enable_variance_boost,
    variance_boost_strength}`. **Replaces `with_vaq(bool, f64)`**; the strength
    is now C's documented 1-4 scale, not an invented 0.0-1.0 float.
  The remaining four were REMOVED rather than faked, because neither this
  pipeline nor C has a counterpart: `with_trellis` (SVT-AV1 has no trellis
  knob; RDOQ level comes from preset + coeff level, C-exactly),
  `with_seg_boost` + the `seg_boost()` getter (no segmentation on the still
  path), and `with_still_image_tuning` (the encoder is unconditionally
  still-image: one KEY frame, temporal tools forced off for all-intra as C
  does).
- **`AvifEncoder::{enable_qm, enable_variance_boost}` now default to `false`**
  — C's mainline defaults, and the bytes this encoder has always emitted. They
  previously defaulted to `true` while being ignored, so leaving them `true`
  once live would have silently changed every caller's output.
- `AvifEncoder::encode_y8` is documented MONOCHROME-only (`mono_chrome = 1`):
  correct for a gray image or an AVIF alpha plane, not a way to encode the luma
  of a colour image. It still pre-pads to a multiple of 64 because
  `EncodePipeline`'s TRUE -> ALIGNED padding is wired on the 4:2:0 path only —
  so for a non-64-multiple gray image the coded frame is larger than
  `EncodedAvif::{width, height}`. Arbitrary-dims MONOCHROME is a pipeline gap.

### Fixed

- **16 more x86_64-only shim SIGSEGVs, from two lanes that landed the same
  day.** Found by re-running the suite on x86 after the obmc fix below; all 16
  were green on aarch64-darwin. (a) `c_parity_entropy_inter` (7 tests):
  `ec_build_xd` and `EC_FC_TABLE` call `svt_aom_init_mode_probs`, whose
  `COPY_CDF` is bare `svt_memcpy` (`cabac_context_model.c:735`, while the same
  file uses the null-safe `SVT_MEMCPY` at :1923) — the NULL RTCD pointer again;
  both sites now route through a one-shot `ec_init_mode_probs`. (b)
  `c_parity_estimate_transform` + `c_parity_txfm_pf_entry` (9 tests):
  `svt_av1_fwd_txfm2d_*_avx512` store with `vmovdqa32`, the 64-byte ALIGNED
  store, into Rust `Vec` buffers that are 2/4-byte aligned — measured fault at
  `vmovdqa32 %zmm0,-0x40(%rax)`, target 48 bytes past a 64-byte boundary.
  `ref_wht_fwd_txfm`, `ref_highbd_fwd_txfm` and `ref_estimate_transform` now
  stage through 64-byte-aligned scratch (copying the coefficient buffer IN as
  well as out, since these tests prefill it and assert C leaves unwritten
  positions alone). Both are re-breaks of contracts `ref_shims.c` had already
  documented — the RTCD one-shot at :790 and the AVX2 32-byte staging at :1315.
  Verified 1542/1542 on x86_64-linux and 1535/1535 on aarch64-darwin.

- **Two `c_parity_obmc_search` oracles were unsound; both were green on
  aarch64 by accident and broke on x86_64.** Found by the first cross-ISA run
  of the suite (2026-08-31): `convolve8_matches_c` failed with a whole-block
  value mismatch and `upsampled_pred_matches_c` SIGSEGV'd, on x86_64-linux
  only. Neither was a port defect — the port's `convolve8_horiz` is
  ISA-invariant scalar integer code and produced the right answer on both
  hosts. (a) `svt_aom_convolve8_{horiz,vert}_c` derive the filter phase from
  the filter POINTER'S ADDRESS (`convolve.c:54`, `get_filter_base` =
  `ptr & ~0xFF`, documented as assuming a 256-byte-aligned table), so
  forwarding a Rust `&[i16; 8]` made the oracle apply the taps at
  `addr - (addr % 16)` — correct only when the Rust static happened to land
  16-byte aligned, which it did on aarch64 and did not on x86.
  `ref_me_convolve8_{horiz,vert}` now stage the taps into an
  `_Alignas(256) int16_t[16][8]`, matching `ref_shims.c`'s existing
  `ref_convolve8_horiz`. (b) `ref_upsampled_pred` did not initialize RTCD, and
  `svt_aom_upsampled_pred_c` reaches bare `svt_memcpy` — a `.bss` function
  pointer that is NULL before setup on x86 and a devirtualized concrete symbol
  on aarch64 — so the call landed at `rip = 0x0`; it now calls
  `obmc_ensure_init()` first. Pinned by two new controls:
  `convolve8_oracle_is_alignment_invariant` (feeds the same taps from every
  2-byte residue in a 256-byte window; fails pre-fix on aarch64 too, so it is
  ISA-independent) and `upsampled_pred_cold_rtcd_zero_subpel` (the minimal
  reproducer, first C call in its own process). Verified 1275/1275 on
  x86_64-linux and 1268/1268 on aarch64-darwin.

- **`has_top_right`'s `PARTITION_VERT_A` check now reads the MUTATED `bs` in
  `intrabc_mvp.rs` too.** The same defect fixed in `inter_mvp.rs` was present
  in the IntraBC copy of the function, where the randomized `c_parity` sweep
  had never happened to place a VERT_A cell on a geometry that advances `bs`.
  Pinned by `c_parity_has_top_right_vert_a_uses_mutated_bs` in
  `tests/c_parity_intrabc_mvp.rs`, which fails before the fix
  (`ref_mv_stack[0].weight` 672 against C's 668) and passes after.
  **Byte impact MEASURED, and it is none:** a 120-cell port-only sweep
  (gb82-sc x 10 images x presets 1-4 x qp {20,32,48}) was run before and after,
  and all 120 `(bytes, sha256)` pairs are identical —
  `benchmarks/intrabc_has_top_right_vert_a_2026-08-31.{tsv,meta}`. So this is a
  correctness fix with no shipped-byte change on that corpus; per
  `docs/WORKING-ON-THIS.md` §3 it deliberately gets NO
  `regression_spotcheck.sh` cell (a cell that never failed cannot guard it),
  and per §7 it STAYS — the same function serves the inter MVP stack, where the
  geometry is far less constrained. `regression_spotcheck.sh` is 35/35 after.
- **Shim data race: per-call state in `static` (test harness).** cargo runs a
  test binary's tests on several threads, so a `static` scratch buffer shared
  by two concurrently-running `c_parity` tests is a data race that surfaces as
  an occasional WRONG NUMBER, not a crash — which reads exactly like a port
  bug. Measured: with `static CandidateMv stack2d[...]` in
  `ref_setup_ref_mv_list_intra`, `c_parity_intrabc_mvp` failed at partition=0
  with count 1 vs 2 under `--test-threads=3` and passed under
  `--test-threads=1`. `shims/ref_shims.c` was then audited end to end: five
  per-call `static`s found, all five now `calloc`/`free` per call — that
  `stack2d`, `ref_lf_limits`'s `LoopFilterInfoN`, the three
  `RestorationLineBuffers` scratch banks in the loop-restoration apply shims,
  and `ref_noise_normalization`'s synthetic `SequenceControlSet` /
  `PictureControlSet` (whose `noise_norm_strength` is written per call and read
  by the callee). What stays `static` is documented in the file header with the
  reason each is not per-call state: `g_fc` (a deliberate two-call protocol
  with a caller-held mutex) and the three idempotent RTCD init flags. The rule
  itself now leads that header so the next shim author does not re-introduce
  it.

- **`has_top_right`'s `PARTITION_VERT_A` check must read the MUTATED `bs`
  (chunk C2).** C's `has_top_right` (adaptive_mv_pred.c:266-325) shifts `bs`
  left inside its 4x4-group loop (`:303-313`) and the `PARTITION_VERT_A` test
  at `:314-322` then reads that MUTATED value. Reading the ORIGINAL `bs` there
  diverges: measured against the exported C symbol at `mi = (36, 10)`, an 8x8
  block in a 64x64-mi superblock whose current cell has
  `partition == PARTITION_VERT_A`, `bs` enters as 2 and the loop advances it to
  4, after which `mask_row == 4` makes C drop the top-right candidate — the
  port kept it, for `ref_mv_stack[0].weight = 672` against C's 668. Only
  `partition == 6` diverged; the nine other partition types agreed, which is
  what localizes it. Pinned by
  `c_parity_has_top_right_vert_a_uses_mutated_bs` (failed before, passes
  after). **`crates/svtav1-encoder/src/intrabc_mvp.rs` carries the same
  original-`bs` reading and is therefore latently wrong on the same geometry**;
  it is another chunk's file and was NOT edited here.
- **`add_ref_mv_candidate`'s `assert(weight % 2 == 0)` does not hold (chunk
  C2).** C asserts it (adaptive_mv_pred.c:63) but ships with `NDEBUG`, so it is
  never checked. With `row_adj == 1` — an 8x4 block at an odd `mi_row` —
  `max_row_offset` is -5 and `scan_row_mbmi`'s `inc` reaches 5 for a candidate
  8 or 16 mi tall, giving `weight == 5`. Reproduced on the randomized grids in
  `tests/c_parity_inter_mvp.rs`. The assert is deliberately NOT transcribed;
  an odd weight is a legal input and changes nothing downstream.

- **Mainline chroma delta-q desynced every decoder — `entropy::obu::ChromaQSignal`
  (2026-08-28).** Porting mainline's chroma-q derivation (below) made tune IQ
  produce non-zero chroma deltas, and they were emitted through the only form
  the frame-header writer had: the FORK's `diff_uv_delta = 1` + four
  independent deltas. That form REQUIRES the sequence header to have signalled
  `separate_uv_delta_q = 1`; the fork's does, MAINLINE's signals 0, and spec
  5.9.12 reads `diff_uv_delta` only when that bit is 1 — so the extra bit and
  the two extra V deltas shifted every following bit of the frame header. Not a
  byte-count difference: a desync. `tools/variance_boost_recon.sh` went 0
  passed / 60 failed, every cell DECODE FAILED (CI run 33220828356), and a
  plain tune-IQ 128x128 q40 p6 encode was rejected by aomdec AND dav1d.
  The fix is a type rather than a branch — `ChromaQSignal::Shared { dc, ac }`
  (SH bit 0, one pair reused for V, no `diff_uv_delta`) vs
  `ChromaQSignal::Separate([i8; 4])` (SH bit 1, the fork's four) — so a frame
  header that disagrees with its sequence header no longer type-checks. The
  same SH bit also gates `qm_v`, which was keyed on `chroma_q.is_some()` and
  would have emitted a stray 4-bit field the moment QM and tune IQ were on
  together. After: variance_boost_recon **60/60**, decode_conformance 4:2:0
  1575/0. Two cells added to `tools/regression_spotcheck.sh` (now **35/35**),
  earned the hard way — the writer was temporarily reverted to the buggy form
  and both cells confirmed to fail under aomdec, then restored.


- **Monochrome straddling edge block wrapped its recon into the next row
  (every SB row after the first decoded wrong on frames with a thin right
  edge).** The second half of the mono partial-SB fix below: once a one-false
  edge leaf is coded as the single legal rect, a thin right edge makes that
  rect STRADDLE the aligned width (a VERT 32x64 at x=192 on an aligned-200
  frame keeps 8 in-frame columns). `encode_single_block` stored the full
  block width at the aligned stride, so the off-aligned columns wrapped into
  the next row's columns 0..24 and overwrote an already-committed
  neighbour's recon — the encoder then predicted the next SB ROW from wrapped
  pixels the decoder never had. Measured (rav1d-safe, gradient qp 10, preset
  6): 200x136 27.9 dB with the first SB row at 55 dB and the second at 23 dB
  from column 0 outward; 136x200 25.0, 200x72 35.3, 72x136 31.0, 264x136
  28.1, 200x200 24.4 dB; 192x136 / 200x64 / 64x136 clean (no thin right edge,
  or nothing below it). aomdec DECODES the broken streams, so decodability
  was hiding it. The store now carries the same straddle clip
  `leaf_funnel::commit_leaf` already had (nothing reads past the aligned
  extent — `extract_neighbors_tiled` clamps like the decoder's spec-7.11.2
  replicate). After: 200x136 56.96 dB, every cell above 56-58 dB, 22/22
  zenavif svt-rs tests. Regression: `mono-straddle-wrap-p6-200x136` in
  `tools/regression_spotcheck.sh` — a recon oracle (encoder FINAL recon vs
  `aomdec --rawvideo`, luma at true dims) on the `(x+y)` ramp fed as `raw:`
  content, because on the synthetic `gradient` the PD0 resolves that node to
  SPLIT and nothing straddles (bytes identical with and without the clip).
  Witnessed before the clip: 14,720 of 27,200 luma bytes differ (encoder
  recon 56.97 dB vs source, aomdec output 27.89 dB); after: byte-equal. A
  96x80 control cell (32-wide edge, no straddle) is byte-equal either way.
  The decoded round-trip over seven geometries is gated on the zenavif side.
- **Monochrome partial superblocks at preset 6 emitted an undecodable stream
  (a `PARTITION_NONE` square coded at a frame edge).** The M6 PD0 keeps NSQ
  geometry on, so a one-false edge node is TESTED with the rect edge-shape
  cost instead of force-split; `encode_fixed_tree`'s funnel arm (4:2:0) codes
  such a leaf as the single legal `PARTITION_HORZ` / `PARTITION_VERT` rect,
  but the mono arm (no funnel) fell through to a full-size `PARTITION_NONE`
  square — illegal per spec 5.11.4, refused by the pack's debug_assert in a
  debug build and written as-is in release (96x80 / 128x80 / 200x136 gradient
  at qp 10: "Corrupt frame detected" under aomdec; zenavif measured 18 dB
  garbage at 96x80 q85). Presets >= 7 were never affected (NSQ geometry off
  -> forced SPLIT in PD0) and 4:2:0 is byte-neutral by construction (its arm
  returns first; on 64-aligned frames both edge flags are true). The mono arm
  now applies the same rule. Found by zenavif's seam canary
  `svt_rs_direct_mono_partial_sb_preset6_still_broken` the day its CI first
  ran `cargo test` (dev profile) against this tree. Regression:
  `pipeline::tests::mono_partial_sb_preset6_edge_leaf_codes_the_edge_shape`
  (7 geometries; panicked with the pack's assert before, passes after) + three
  `mono-partial-sb-p6-*` decode cells in `tools/regression_spotcheck.sh`.
  Decode round-trip (rav1d-safe + aom-rs, 56 dB at 96x80) is gated on the
  zenavif side.
- **MDS1 candidate costs 103 rate units cheaper than C on DC / IntraBC
  candidates — issue #16 root-caused and closed.** The probe the issue named
  (`SVT_FASTCOST_XY` + `SVT_FULLCOST_XY` in the `--wrap` container vs the
  port's `SVTAV1_CANDDBG` dump) split the delta in one run: all 57 signalling
  rates and every `ydist` matched; only the tx-type rate on the ADAPTED CDF
  rows (intra `DC_PRED`, inter) differed. C's MD-side coefficient cost
  (`svt_av1_cost_coeffs_txb`) keys `is_inter` on `is_inter_mode(mode)` without
  `use_intrabc`, so its encode pass adapts the intra DC ext-tx row for an
  IntraBC txb while its writer adapts the inter row; the port's per-SB chain
  simulation re-coded with writer semantics and rebuilt rate tables from a
  DC row C never sees (`docs/SUSPECTED-C-BUGS.md` #10 — the UPDATE half of
  the quirk whose READ half `cost_dir` already reproduced). Fix:
  `CoeffFc::md_side_ibc_txt_update` on the chain contexts routes IntraBC
  tx-type adaptation through `md_update_tx_type_ibc_quirk` (intra set, DC
  row, no update at DCT-only sizes). After: 57/57 MDS1 costs at
  `terminal 188x256 p2 q55` mi=(50,42) equal C's (was 54/57), stream unchanged.
  Byte-neutral on every gate run: `regression_spotcheck` 28/28,
  `alignment_gate` 74/74 (+ the IBC / palette screen gates, see the commit).
  Unit witness `md_side_ibc_tx_type_update_adapts_the_intra_dc_row_like_c`
  (mutation-verified). Record: `benchmarks/issue16_mds1_txt_cdf_2026-08-27.md`.
- **The 10-bit reconstruction never received the loop restoration it
  signalled — issue #13.** `recon10` fed the Wiener SEARCH (taps picked on
  10-bit data, signalled in the frame header) but only the u8 chain was handed
  to `apply_restoration_frame`, so no 10-bit plane in the port ever carried the
  filter a conforming decoder applies — and nothing could observe it, because
  no post-filter 10-bit recon was published. Now: the DSP stripe-boundary
  machinery (`StripeBoundariesT<T>`, `save_tile_row_boundary_lines`,
  setup/restore) is generic over the pixel type with the u8 names unchanged,
  `loop_restoration_filter_unit_hbd` is the highbd apply arm WITH boundaries
  (C `svt_av1_loop_restoration_filter_unit` at `highbd = 1`, pinned by the new
  `highbd_filter_unit_with_boundaries_matches_c` differential — 200 random
  cells, both `need_boundaries` arms, `data` restored exactly), the encoder's
  `save_lr_boundaries_bd` / `apply_restoration_frame_bd` are the generic
  bodies (u8 delegates, byte-neutral by construction), and the pipeline
  applies LR to the 10-bit canvas with boundary lines from the 10-bit
  post-deblock / post-CDEF planes. Published as the additive
  `EncodePipeline::last_recon10_final` (deblock -> CDEF -> LR on the 10-bit
  canvas; the 10-bit twin of `last_recon`, `with_recon_output` gated).
  Witness `svtav1/tests/issue13_repro.rs`: 383x512 bd10 p6 q40 (luma Wiener
  fires) — `last_recon10_final` == `aomdec` sample for sample; with the apply
  disabled 175,734 samples differ. `SVTAV1_FINAL_RECON` dumps the 10-bit final
  recon (u16 LE) at bd10, and `alignment_gate.sh`'s RECON leg now runs at
  BOTH bit depths (it was bd8-only because nothing 10-bit was comparable).
- **The MDS3 independent-chroma search ran on blocks where C skips it —
  issue #15 closed at 648/648** (`leaf_funnel.rs`). C gates
  `search_best_mds3_uv_mode` on `perform_ind_uv_search_last_mds`
  (product_coding_loop.c:1472-1504); the port implemented only its first arm
  and had nothing for the `inter_vs_intra_cost_th` arm (:1498-1501), which
  zeroes the intra count when `best_inter_cost * 100 < best_intra_cost * 100`.
  `is_inter` there is `is_inter_mode(mode) || use_intrabc`, so on SCREEN
  CONTENT a winning IntraBC candidate makes C skip the search entirely, keep
  `ind_uv_avail = 0`, and code each MDS3 candidate's injected uv-follows-luma
  chroma — where the port's uv table substituted `UV_DC_PRED`. Measured on
  `terminal` 188x256: p2 q55 C MDS1 best intra 97,762,561 vs best IntraBC
  84,376,537 (C codes uv=D113/-1), p4 q12 163,691 vs 148,994 (C codes
  `UV_CFL_PRED`); `ind_uv_avail = 0` read directly off C via the new
  `svt_aom_get_intra_uv_fast_rate` interposer. This was the last of #15's 67
  cells: `unaligned_identity_scan.sh` **646 → 648 / 648, 2 fixed, 0 broken**.
  Byte-neutral wherever no IntraBC candidate exists — the arm is genuinely
  inert there (`byteid_fingerprint` 168/168, **0 rows moved**). Regression cell
  `ind-uv-ibc-cost-gate-188x256` (spot-check 27 → 28). Data:
  `benchmarks/unaligned_real_identity_2026-08-14-induv.{tsv,meta}`.
- **`sse_i32` subtracted coefficients in i32 where C subtracts in `int64_t`,
  and panicked in debug where C's `uint64_t` wraps** (`svtav1-dsp`
  `residual.rs`; C `svt_full_distortion_kernel32_bits_c`, `pic_operators.c:86`).
  Three widths were Rust's rather than C's — the subtraction (`(x - y) as i64`),
  the square, and the accumulator — and the accumulator is what left
  `residual_recon_distortion_all_tiers_match_core` RED on `main`. All three now
  match C in every build. The NEON arm cannot widen first (no i64xi64 multiply
  exists to square an `int64x2_t`), so it keeps `vsubq_s32` and DETECTS a wrap
  by comparing against `vqsubq_s32`, falling back to the exact scalar core;
  fast path exact, slow path exact. New gate
  `sse_i32_matches_c_widths_at_i32_extremes` checks every tier against an i128
  oracle and asserts its own case set discriminates the two widths. **Byte-inert
  on every grid** (byteid 168/168 with 0 cells moved, unaligned scan 648 cells
  with 0 changed, partial_sb 146/146, decode grid 120/120, recon parity
  432/432). Measured: the wrap is unreachable on a real encode — 0 in 59,088,480
  elements, max |difference| 788 against an i32 ceiling of 2,147,483,647
  (`benchmarks/sse_i32_width_2026-08-11.meta`), so this does NOT explain issue
  #15, which stays open.

- **Loop restoration walked a different unit grid than the one the search
  sized — an out-of-bounds panic on the public encode API** (issue #11,
  `restoration.rs:985`, `index out of bounds: the len is 2 but the index is 2`).
  C derives the restoration-unit count (`svt_av1_alloc_restoration_struct`) and
  every unit walk (`svt_av1_loop_restoration_filter_frame`,
  `svt_av1_loop_restoration_save_boundary_lines`) from ONE
  `whole_frame_rect(&cm->frm_size, ..)`, and `cm->frm_size` is the pre-8-alignment
  coded size (`pcs.c:1337`, `picture_width - non_m8_pad_w`), CEILING-subsampled
  for chroma. The port's SEARCH used the true extent (task #95 goal 1) but
  `apply_restoration_frame` / `save_lr_boundaries` were still handed the ALIGNED
  `w`/`h`, so wherever the 8-alignment crossed a `count_units_in_tile(256, ..)`
  boundary the walk visited more units than the grid holds: true 383 counts one
  horizontal unit, aligned 384 walks two. Both now take the true extent plus the
  aligned canvas STRIDE, and chroma rounds up like C rather than down. Reported
  on 5 real renditions (115 of 34,200 HDR-grid cells); reproduced synthetically
  at `383x512` / `766x128` / `258x128` / `385x257` at bd8 AND bd10. The
  bitstream was never affected — the panic came after the tile was written — and
  the previously-panicking cells are now byte-identical to the C encoder
  (`regression_spotcheck.sh` cells `lr-align-cross-*`). A 2,280-cell A/B of the
  pre- and post-fix encoders over 19 dimensions × 5 presets × 4 qps × 2 depths ×
  3 contents shows every previously-working cell byte-unchanged.
- **The bd10 per-tile recon canvases were MERGED at the wrong stride.**
  `commit_leaf` writes them at the ALIGNED stride (the SB-extent product exists
  only so a right-straddle write wraps into slack rather than out of bounds),
  but the frame merge read them at the SB-EXTENT stride. Byte-inert while every
  gated bd10 cell had `ext_w == w`; it scrambled the 10-bit recon that the bd10
  deblock / CDEF / Wiener searches read the moment a frame had a partial SB.
- **The native-u16 source had no SB-extent twin.** `HbdSource` is padded
  TRUE→ALIGNED only while `blk_y_src10` gathers by absolute coordinates, so a
  straddling block would read past the plane or wrap into the next row. Added
  the `sb_input` / `sb_chroma_owned` equivalents and threaded `in_stride` into
  `FunnelSrc10`; the `debug_assert_eq!(in_stride, w, "bd10 hbd source assumes a
  64-aligned frame")` that stood in for this is gone.
- **Two out-of-bounds panics on the public encode API**
  (`crates/svtav1-encoder/src/intrabc_hash.rs`). C computes
  `x_end = pic_width - block_size + 1` as a SIGNED int
  (`hash_motion.c:195-196`, `:222-223`), so a picture smaller than the hash
  block just yields an empty loop; the port used `usize`, underflowed to ~2^64
  and indexed off the end. A 32x32 screen frame at preset 0 panicked twice
  (`len is 1024 but the index is 1024`, and `index 2048`). Found by the new
  8-bit gate's dims tier — no earlier gate encoded anything below 60x60 with
  the screen-content tools armed.

### Changed

- **Doc debt from the 2026-07-25 publication audit, second pass (issue #8).**
  The HDR-fork verification bar no longer contradicts itself between
  `README.md` and `rust/README.md`: fork mode IS byte-gated vs a
  `SVT_HDR_MODE=ON` C build at 10-bit (`hdr_bd10_gate.sh` 64/64, standing);
  the 8-bit 48/48 is a 2026-07-19 measurement (`docs/HDR-ON-4.2.md`) with no
  standing gate script, and `hdr_fork_e2e` is named for what it is (liveness +
  decode witnesses, 36/36). `identity_matrix` is described as its 54-cell
  default grid, with the 132/132 figure dated to the 2026-07-16 wider sweep it
  came from (`rust/README.md`, `C-TEST-PORTING-AUDIT.md`). `screen_ibc_gate`
  20/100 -> 22/100 (the script's `BYTE_EXACT` list has 22 entries; 78 open).
  `bd10_photo_gate` is 191 cells (counted from the script's groups A-H:
  30+64+18+18+12+15+1+32+1); the 154 and 187 figures in `STATUS.md` are dated
  records and now say so. Every test-count tally the audit listed (669/669,
  873/873, 902/902, 915/915 x2, 864) carries `(as of <commit>)`, found with
  `git log -S`. `finishing-survey.md`, `bd10-port-map.md` and `ibc-port-map.md`
  open with a "line numbers as of <creation commit>; re-locate by symbol"
  header. The fresh-box README lists `cargo-nextest`, `just`, `aomdec`/`dav1d`
  and `tools/decode_diff` as the prerequisites cargo does not install.
  Still open from #8: whether to commit `rust/Cargo.lock` (a decision, not a
  doc fix), per-gate wall-clock budgets (unmeasured), the "landed work
  described as open" sections of the port maps, and the CI runner matrix
  (tracked under #4).
- **Encode speed: the port-vs-C per-pixel slope gap closes to 2.89x at presets
  10 and 13, 3.27x at preset 6, and — for the first time this campaign — 3.93x
  at preset 2** (from 3.06x / 3.07x / 3.39x / 4.14x). All 24 campaign cells
  byte-identical to C (`rust/benchmarks/perf_gap_2026-08-13-r1r2.meta`). Two
  byte-identical changes, and unlike everything before them these remove work
  whose result was **discarded**, not duplicated — the two top findings of
  `rust/docs/C-VS-PORT-CODE-REVIEW-2026-08-13.md`:
  - **R1: the inverse transform + reconstruction ran even where the
    reconstruction is thrown away.** C gates both on `mds_do_spatial_sse ||
    (!is_inter && tx_depth)` (product_coding_loop.c:4783-4784) and the all-intra
    derivation pins `spatial_sse_full_loop_level = 3`, so C inverts nothing at
    MDS1/MDS2; the port inverted unconditionally. A census measured the
    discarded share of inverse-transform pixel work at 40-50% (p10/p13), 36-50%
    (p8), 43-51% (p7), 28-53% (p6) and 24-44% (p2). Three call sites (MDS1
    luma, the CfL alpha search, the non-CfL chroma re-cost) now pass an explicit
    `need_recon = false`, each with an exhaustive-scan proof that the
    reconstruction is unread in its whole binding scope. 56d19efe1 — A/B 12/12
    cells 1.021-1.053x at qp40, and 28 of 28 cells below 1.0 across 6 presets x
    3 sizes x 2 qps against a control arm that split 13/15 (sign test
    p = 3.7e-9).
  - **R2: the exact coefficient rate was computed and then overwritten**
    wherever C's closed forms apply. C's rate tiers are an `if / else if /
    else` and the estimator is never reached on those arms
    (product_coding_loop.c:4914-4934, :5540-5564); the port called
    `cost_coeffs_txb` first and discarded it. Now evaluated in C's order.
    8179a7d94 — 1.038-1.060x at p10/p13 **qp20**, null at qp40/512+; the wall
    clock tracks the census share of replaced coefficient work (51-54% at qp20,
    16-38% at qp40, zero at qp55), which is what identifies the win as the
    mechanism rather than code placement.
  - the census instrument behind both, `leaf_funnel::txcensus` (cargo feature
    `__txcensus`, off by default, zero cost when off). 7dec5f24e.
- Preceding this, four byte-identical changes that took p10/p13 from 3.53x to
  3.06x, every one of them removing a duplicated COPY of something already
  computed rather than making an allocation cheaper:
  - the frame's block-decision set was materialised **four** times per frame —
    a leaf-level clone so the partition tree and a parallel `decisions` list
    could both own it, an aggregation of that list up the tree, a deep clone
    into a `per_tile_decisions` that was **written and never read**, and a deep
    clone of each superblock tree into its raster slot. Only the tree survives;
    `PartitionResult::decisions` is now populated by the legacy
    `partition_search` path alone and `num_blocks` comes from the new
    `PartitionTree::count_leaves` (29847e5d3, A/B 1.07-1.11x at p10).
  - `LeafEval::to_choice` deep-cloned seven of the winning candidate's buffers
    only because it ran *before* `commit_leaf`; both callers now commit first
    and `into_choice` moves (6ad044d00, A/B 1.02-1.03x at p10).
  - `funnel_block_decision`'s depth-0 qcoeff "unpack" was a byte-for-byte copy
    on every block without a 64-dim transform side, and
    `DecodedPictureBuffer::refresh` deep-cloned the whole picture once per set
    bit of `refresh_frame_flags` — eight full Y planes per KEY frame, into
    slots only ever read as `&ReferenceFrame` (now `Arc`-shared; the field is
    private and `store`/`get`/`refresh` keep their signatures, so no API
    change). 81a1bb111, A/B 1.01-1.02x at p10.
  - the per-SB reconstruction staging buffer (an allocation, a zero-fill and a
    second pass over every pixel of every superblock) is gone; **measured
    null**, kept only because it is strictly less work.
- **Measured negative, recorded so it is not retried**: a thread-local `Vec`
  pool for the mode-decision buffers removed a whole class of allocations from
  the profile (`drop_glue::<Cand>` 7.1% of malloc samples -> 0) and measured
  **null** at n=31 against an in-grid identity control. On macOS's xzone
  allocator the pool's machinery costs about what `malloc`/`free` costs at
  these sizes. `rust/benchmarks/alloc_bufpool_null_2026-08-13.meta` names the
  shape that is still unpriced (one construction-time arena the buffers are
  slices into, which is what the C reference does).
- **CI gates four more 8-bit surfaces**: partial-SB / odd dimensions (104
  cells), tiles across rows AND columns (29), SB128 (22), and panic-freedom on
  gradient AND screen (80). All four already failed loudly — they were simply
  never in the workflow.
- **`identity_run` reports a REFUSAL distinctly from a crash** (exit 3). It
  called the infallible `encode_frame*` wrappers, whose `.expect()` turned every
  deliberate out-of-envelope refusal into a panic; `arbitrary_size_robustness.sh`
  therefore reported 48 correct bd10 refusals as PANIC, unable to tell the
  port's best behaviour from its worst. That gate now reads 80/80 + 48 refused
  where it read 80/128, on identical encoder behaviour.
- **`tools/arbitrary_size_robustness.sh` now sweeps `screen` content as well as
  `gradient`, and adds sub-64 cells.** It previously ran gradient only, which
  never arms the screen-content detector — so palette and IntraBC were off in
  every cell and the gate could not reach the code paths they use. It ran
  straight past the `intrabc_hash` panics above. A panic-freedom gate that
  cannot arm half the encoder's tools is not a panic-freedom gate.

### Added

- **A comprehensive 8-bit byte-parity gate, and CI coverage for it**
  (`tools/identity_full_8bit.sh`). Until now there was **no 8-bit
  byte-vs-C identity gate in CI at any preset**: `identity_matrix.sh` is a
  scoreboard whose own header says "Exit 0 always", and it was not in the
  workflow either — so every 8-bit byte-identity claim, on the port's primary
  product surface, rested on hand-run measurements that nothing re-checked.
  The new gate exits nonzero, sweeps **every preset 0..13** (C clamps all-intra
  above M9 to M9 but the port does not, so 10..13 are distinct configurations
  here), carries low-q density where structural problems hide, covers
  partial-SB / odd / tiny / large geometry and four content classes including
  screen, pins divergences **self-promotingly** (a pinned cell that starts
  matching fails until promoted), and fails on harness errors so a cell that
  could not run can never look like a pass. `identity_matrix.sh` keeps its
  scoreboard role and gains `IM_STRICT=1` for gate use.


- **Native 10-bit input** (#6). `EncodePipeline::try_encode_frame_420_hbd` /
  `try_encode_frame_hbd` take real `u16` planes. The low 2 bits reach the mode
  decision, the coded levels, and the deblock / CDEF / Wiener searches — the
  port no longer widens an 8-bit source internally (35743ebd5, f319ec298).
  Gate: `tools/bd10_hbd_src_gate.sh`, 100/100 cells byte-identical to C.
- **Super-resolution**, opt-in via `EncodePipeline::with_superres(denom)` with
  `denom` in 9..=16, off by default exactly as in C (5c69edcb2, f4a1b7516,
  2f4d24cba, f319ec298, 174b0f184). Gate: `tools/superres_gate.sh`, 128/128
  cells checked three ways — byte-parity vs C, decodability at the upscaled
  size under the reference decoder, and anti-vacuity vs the non-superres stream.
  - `svtav1-dsp::superres` — the normative 64-phase upscale (was a 16-phase
    stub); `svtav1-dsp::resize` — the source downscale (new).
  - Sequence-header `enable_superres` + frame-header `superres_params()`.
  - C's stale full-resolution variance array, read through coded-grid indices,
    is reproduced deliberately (chunk B.4) — matching C requires it.
- `tools/bd10_hbd_src_gate.sh` and `tools/superres_gate.sh`, both wired into CI.
- `CONTEXT-HANDOFF.md` — build-from-scratch, gate, and open-work guide.

### Changed

- The test runner is `cargo nextest run` (CI and `just test`); each test gets
  its own process, which prevents archmage's process-wide dispatch-tier state
  from leaking between tests (d807fa0fe).
- Out-of-envelope configurations are REFUSED with
  `EncodeError::UnsupportedConfig` rather than silently encoding truncated or
  mis-scaled content (`hbd_source_consumed`, `superres_config_error`).

### Fixed

- **Partial-superblock RD mis-pricing: the cropped-TX distortion bound is now
  wired** (#95 chunk 2 (b)+(c)). On a frame whose aligned dims are not a
  multiple of 64, a coded TX block can straddle the frame edge; C prices only
  the part inside the ALIGNED frame (`cropped_tx_width`/`cropped_tx_height`,
  `Source/Lib/Codec/product_coding_loop.c:4664-4665` and `:5752-5754`;
  `cropped_tx_width_uv`/`_height_uv`, `full_loop.c:2228-2232`), while the port
  scored the whole block — so every boundary block was mis-priced. The
  already-written `frame_geom::cropped_tx_dims` (plus a new `cropped_tx_dims_uv`
  for C's chroma-domain expression) now feeds `leaf_funnel::tx_unit`,
  `tx_unit_hbd` and `txt_search`. The crop touches ONLY the spatial distortion
  kernels; the residual, transform, quantizer, RDOQ, recon and coefficient rate
  still run over the full TX block, exactly as in C.
  Measured crop-off → crop-on over 48 partial-SB cells: 8 changed bytes,
  **3 went divergent → byte-identical to C** (`gradient 80x88 / 104x88 / 72x88
  at q55 preset 6`, the straddle-win trio), **0 regressed**. Those three are now
  gated: `tools/partial_sb_gate.sh` 101 → **104/104**. Byte-neutral everywhere
  else (`identity_matrix` 54/54, `bd10_matrix` 36/36) — on a 64-aligned frame
  the crop is the identity. New differential test
  `leaf_funnel::tests::cropped_tx_distortion_matches_c_spatial_facade` pins the
  cropped distortion to the real exported
  `svt_spatial_full_distortion_kernel_facade` via `svtav1-cref`.
- `coeff_c_txb_init_levels_partial_zero_no_stale_reads` failed at default test
  parallelism: archmage token disabling is process-wide, so a sibling
  permutation test could move it onto the scalar arm. It now holds
  `lock_token_testing`, and 31 further dsp tests pin their tier the same way
  (d807fa0fe). No bitstream impact — every consumer reads only scan positions
  below `eob`.
- `perf_report` example declared `required-features = ["std"]`; a bare
  `cargo test -p zenav1-svt-dsp` previously failed to build it (f319ec298).

### Removed

- `svtav1_dsp::superres::{superres_upscale, superres_upscale_row}` — the
  non-normative 16-phase stub, replaced by the real kernel. No in-tree callers.

## Earlier history

This file starts at 2026-07-24. Prior progress (the 8-bit byte-identity
campaign, chroma/4:2:0, deblocking, CDEF, Wiener restoration, palette, tiles,
arbitrary dimensions, the 10-bit MD path) is recorded per-feature in
`rust/docs/*.md` and in `rust/CLAUDE.md`'s status sections, with the commit
hashes cited inline there.

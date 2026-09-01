# Inter-frame ENCODE — campaign plan, registered 2026-08-31 before any chunk lands

Owner directive (2026-08-31, verbatim): *"Zenav1-svt inter encode is better to
port than aom, right? Let's get that completed by midnight denver time, pay
attention to and fix things that make iteration slow; try wholesale porting of
thousands of lines and optimizing commandline runtimes."*

This file is the shared map for the campaign. It is a PLAN, not a status —
each chunk records its own result in its own doc/commit, and this file's
per-chunk lines are updated only with measured outcomes.

## 0. Where inter actually stands today (measured 2026-08-31, do not re-derive)

The port is still-picture only and **refuses inter frames at the public entry
point** (`pipeline.rs`, the `if !is_key` guard). That refusal is correct and
was measured: on 2026-08-03 a 5-frame 64x64 gradient encode produced a stream
`aomdec` called "Corrupt frame detected" and `dav1d` called "Overrun in OBU bit
buffer". The refusal names four header defects plus fresh-CDF MV coding, no
chroma in the DPB, and a homegrown (non-SVT) ME.

What is ALREADY ported and C-gated, and is therefore not the gap:

| piece | where | evidence |
|---|---|---|
| convolve8 horiz / vert (the **ME upsample** kernels) | `svtav1-dsp/src/inter_pred.rs` (768 lines) | `c_parity_inter_pred.rs` |
| warped motion — model derivation + the normative 193-phase kernel (`find_projection`, `get_shear_params`, `select_samples`, `warp_affine` 8/10-bit, `warp_plane`, `av1_warp_plane`) | `svtav1-dsp/src/port_warp/` | `c_parity_warp_model.rs`, tier 1 |
| OBMC blending | `svtav1-dsp/src/obmc.rs` | `c_parity_obmc.rs` |
| MVP stack machinery (intra-frame branch) | `svtav1-encoder/src/intrabc_mvp.rs` (940 lines) — `scan_row_mbmi`, `scan_col_mbmi`, `add_ref_mv_candidate`, `sort_mvp_table`, `setup_ref_mv_list_intra` | IntraBC byte gates |
| SAD / variance / hadamard / satd kernels | `svtav1-dsp` | `c_parity_sad.rs`, `c_parity_variance.rs`, `c_parity_hadamard.rs` |

**CORRECTION, 2026-08-31 (measured, wp-interpred lane).** The first row of that
table used to read "convolve horiz / vert / 2d / copy (the MC core)". That was
wrong, and `c_parity_inter_pred.rs:1-16` said so itself the whole time: the
kernels ported there are `svt_aom_convolve8_{horiz,vert}_c` — single-pass,
`clip_pixel(ROUND_POWER_OF_TWO(sum, 7))`, the kernels
`svt_aom_upsampled_pred_c` uses for ME sub-pel refinement — and its
`convolve_2d` composes two of those through a **u8** intermediate. The AV1
inter *reconstruction* kernels (`svt_av1_convolve_{2d,x,y,2d_copy}_sr_c`, the
whole `jnt_*` compound family, and the highbd family) use a **16-bit**
intermediate with the `ROUND0_BITS = 3` / `round_1 = 11` offset scheme; the two
rounding contracts do not agree, so one cannot stand in for the other. The
8-bit single and compound halves are now ported and C-gated at
`svtav1-dsp/src/port_convolve.rs` + `tests/c_parity_port_convolve.rs`; the
highbd half is not yet. So the campaign is **not** purely encoder-side for this
group.

Likewise `svtav1-dsp/src/scale.rs` is a homegrown Q14 divide, not a port of
`svt_av1_setup_scale_factors_for_frame`; `tests/c_parity_scale.rs` pins that
with an `assert_ne!`.

**STATE AS OF 2026-08-31, end of the wp-interpred lane.** The whole MC KERNEL
surface is now ported and C-gated: the four 8-bit `*_sr_c` kernels, the four
8-bit `jnt_*` compound kernels, all eight of their 10/12-bit twins, and both
scaled kernels — plus the four dispatchers (`svt_inter_predictor_pd0`,
`svt_inter_predictor`, `svt_inter_predictor_light_pd1` 8-bit arm,
`svt_highbd_inter_predictor`), the reference scale factors, the MV ->
`SubpelParams` derivation, the wedge mask tables, inter-intra blending, the
masked-compound blend and search, the OBMC `wsrc`/`mask` producer and blend,
and the fast RD models. See `crates/svtav1-dsp/src/port_*.rs` and the matching
`tests/c_parity_port_*.rs`; each commit states its evidence tier per function.

So the campaign's remaining inter gap really is encoder-side — but it was NOT
before this lane, and the table above said otherwise.

**STATE AS OF 2026-08-31, end of the wx-interpred lane** (the second pass over
the same two files). The line above was right that the KERNELS were done and
wrong to imply the two files were. Seven groups were still absent, and the
distinction that matters is between *ported* and *executable*: several
functions had their DECISIONS ported as predicates
(`port_make_pred`, `port_full_pd1_pred`, `port_obmc_build`, `port_ifs`) with
nothing that ran them. Now landed, each C-gated:

| what | where | evidence |
|---|---|---|
| `svt_av1_build_compound_diffwtd_mask_d16` + `diffwtd_mask_d16` — the CONV_BUF-domain mask, a DIFFERENT function from the pixel-domain pair already in `port_masked_compound` (rounding vs truncating shift) | `port_diffwtd_d16.rs` | tier 1, scalar + RTCD entries |
| `svt_aom_pack_block` -> `svt_aom_pack2d_src` -> `svt_enc_msb_pack2_d` — SVT's 8+2 -> 10-bit pack | `port_pack.rs` | tier 1, both dispatch arms |
| `svt_inter_predictor_light_pd1`'s 10-bit arm | `port_inter_predictor.rs` | tier 1, 240 cells at bd 10 |
| `svt_aom_enc_make_inter_predictor` EXECUTABLE, all four leaves (regular, masked-compound, warp, masked-warp) + `av1_make_masked_{scaled,warp}_inter_predictor` | `port_enc_make_pred.rs` | tier 1 |
| `tf_inter_predictor` + `svt_aom_simple_luma_unipred` EXECUTABLE | `port_tf_pred.rs` | tier 1 |
| `get_single_prediction_for_obmc_{luma,chroma}` + `_hbd` EXECUTABLE | `port_obmc_single_pred.rs` | tier 1 for the call |
| `build_prediction_by_{above,left}_pred` EXECUTABLE | `port_obmc_nb_pred.rs` | tier 1 for the leaves |

`port_warp` gained `HbdWarpRef` on the way (its high-bit-depth kernel took an
already-unpacked plane; C reads `ref8b` + `ref2b` per sample), byte-neutral for
every existing caller.

**What is still NOT executable in these two files, named:**
`svt_aom_inter_prediction` (:3204), `inter_intra_prediction` (:2217, blocked on
wiring `svt_av1_predict_intra_block`), `inter_chroma_4xn_pred` (:3023),
`av1_inter_prediction_obmc` (:2925) + `svt_aom_precompute_obmc_data` (:1816),
and the four MD entry points `svt_aom_inter_pu_prediction_av1{,_pd0,_light_pd1,_obmc}`.
Each has its decisions ported and its leaves gated; what is missing is the
`ModeDecisionContext` plumbing around them.

**NOT translatable, with the reason, so they stop showing as gaps:**
`svt_aom_asm_set_convolve_asm_table` / `_hbd_` copy RTCD function pointers into
a 2x2x2 table that `port_inter_predictor`'s `dispatch_convolve_{8,hbd}` replaces
with a `match` on the same three booleans; `svt_aom_get_recon_pic` and
`svt_aom_get_ref_pic_buffer` select a buffer out of a `PictureControlSet` /
reference-list object graph the port does not have by design.

**CORRECTION, 2026-08-31 (wp-filters lane).** The warped-motion row above
originally read "warped motion | `svtav1-dsp` | `c_parity_warp.rs`" and was
WRONG — it listed the gap as already closed. `svtav1-dsp/src/warp.rs` is a
157-line homegrown approximation: 16-phase `SUB_PEL_FILTERS_8`, no shear, no
8x8 tiling, `(sum + 64) >> 7` twice instead of ROUND0/ROUND1. Its own gate,
`c_parity_warp.rs:112`, is an `assert_ne!` GAP-PIN whose message says so, and
`rust/CLAUDE.md`'s 2026-07-14 audit had it right ("warp.rs / scale.rs /
superres.rs — STUBS"). Anyone sequencing inter work off the original table
would have skipped the largest normative gap in that module group. The row now
points at the real port (`src/port_warp/`), which is tier-1 gated against
`svt_av1_warp_affine_c`, `svt_find_projection`, `svt_get_shear_params`,
`svt_aom_select_samples`, `svt_warp_plane`, `svt_av1_warp_plane` and
`svt_av1_highbd_warp_affine_c`. `warp.rs` itself is untouched and still
stubbed; its `assert_ne!` pin stays until a caller migration retires it.

So the campaign is **encoder-side**, not kernel-side. The gap is: multi-frame
plumbing, the real SVT ME, the inter branch of the MVP stack, inter candidate
injection in MD, and inter syntax/MV entropy writing.

## 1. Why svt and not aom (the directive's question, answered with evidence)

For the ENCODER, svt is the better base:
- The differential oracle here reaches **real exported C symbols**
  (`svtav1-cref`, 185 bindings) — evidence tier 1 in `WORKING-ON-THIS.md` §4 —
  and it already exposes the inter-relevant ones: `svt_av1_encode_mv`,
  `svt_av1_get_mv_class`, `svt_aom_mv_err_cost`, `svt_av1_mv_bit_cost`,
  `svt_aom_estimate_mv_rate`, `svt_av1_find_best_ref_mvs_from_stack`,
  `svt_aom_mefn_ptr`.
- The MC/warp/OBMC DSP is already ported AND C-parity gated (table above).
- The byte-identity harness, op-trace differ and 1,057-test suite exist and are
  green, so a new inter cell inherits a working comparison rig.

The honest counterweight, recorded so nobody discovers it late: `zenav1-aom`
has a 2,499-line inter-ENCODE skeleton (`inter_me`/`inter_rd`/`inter_pack`/
`inter_costs`/`inter_frame`/`interp_rd`) and a byte-exact inter DECODER that
can validate an encoder in-repo. svt has neither of those, and SVT-AV1's inter
C surface is larger (~19.5k lines across `motion_estimation.c` 2964,
`mode_decision.c` 4419, `enc_inter_prediction.c` 3898, `inter_prediction.c`
2581, `adaptive_mv_pred.c` 2040, `av1me.c` 1159, `pcs.c` 1575, `md_process.c`
812) before TPL, temporal filtering and picture decision. "Complete inter
encode" is not a one-day scope on either base; what IS achievable is a chain of
byte-gated chunks, smallest-demoable-first, which is how every other envelope
in this repo was built.

## 1b. C0's first measurement changes the shape of C1 (2026-08-31)

Running the new harness immediately produced a finding nobody had named, and it
is bigger than the four header fields the refusal comment lists.

Cell: `gradient 64x64 q40 p6`, the SAME `.yuv` on both sides.

| C configuration | frame 0 bytes |
|---|--:|
| still (`avif = true`, the whole existing 280/280 envelope) | **290** |
| video (`avif = false`), still ONE frame | **930** |
| video, 2-frame low-delay-P GOP | **961** |

So **3.2x of the difference is still-vs-video configuration, not multi-frame**.
The port's key frame is a *still-picture* key frame; C's video-mode key frame is
a different encode of the same pixels, because the still path bypasses the
video rate-control qindex derivation (`rc_crf_cqp.c` — `active_worst_quality` /
`active_best_quality`, `svt_av1_frame_type_qdelta`, and the temporal-layer
adjustment) and the sequence-level GOP machinery. The remaining ~31 bytes
between the 1-frame and 2-frame video runs is the GOP itself (temporal
filtering is on at `kf_tf_strength = 3` in this config).

**Consequence for the campaign:** byte-parity on an INTER frame is gated behind
byte-parity on a VIDEO-MODE KEY frame, which needs the video qindex derivation
ported first. C1 is therefore two steps, and the first is measurable on a
ONE-frame cell (`SVT_AVIF=0`) with no inter machinery at all — the smallest
demoable chunk in the whole campaign, and the one that must land first.

The C driver gained `SVT_AVIF=0` precisely so this stays attributable: it
separates "still vs video configuration" from "one frame vs many", two
variables a naive multi-frame run changes at once.

## 2. Chunks

Ownership is per-file and strict — two chunks must never edit the same file.

| # | chunk | owns | gate |
|---|---|---|---|
| C0 | Multi-frame C oracle + inter identity harness | `tools/capture_c_trace/capture_c_trace.c`, `tools/identity_run`, new `tools/identity_diff_inter.sh` | a C reference 2-frame stream exists and the harness diffs frame 1 |
| C1a | **Video-mode KEY frame parity** — the video qindex derivation (`rc_crf_cqp.c`) + the non-reduced sequence header. Measurable on a 1-frame cell via `SVT_AVIF=0`; no inter machinery involved | `entropy/obu.rs`, `pipeline.rs` header path, rate control | `identity_diff_inter.sh` with `frames=1`, byte-identical |
| C1b | Inter frame header + CDF continuation (`primary_ref_frame`) | same | frame 1 header bits byte-identical to C |
| C2 | MVP stack, INTER branch | new `svtav1-encoder/src/inter_mvp.rs` + its tests | c_parity vs `svt_av1_find_best_ref_mvs_from_stack` + traced vectors |
| C3 | MV entropy coding + MV rate | new `svtav1-encoder/src/inter_mv_code.rs` + its tests | c_parity vs `svt_av1_encode_mv`, `svt_av1_get_mv_class`, `svt_av1_mv_bit_cost`, `svt_aom_estimate_mv_rate` |
| C4 | Wholesale ME port (`motion_estimation.c` + `av1me.c`) replacing the homegrown `motion_est.rs` | new `svtav1-encoder/src/inter_me.rs` + its tests | c_parity where symbols export; traced vectors where static, labelled tier 4 |
| C5 | Iteration speed | `docs/WORKING-ON-THIS.md`, test-harness config | measured before/after wall clock |

C1 depends on C0. C2/C3/C4/C5 are independent of both and of each other.

## 3. Standing rules for every chunk

- `WORKING-ON-THIS.md` governs. State your evidence tier (§4) in the commit
  message. A transcribed oracle agreeing with transcribed code proves nothing.
- Never relax a test, threshold or assertion; never add `#[ignore]`; never let
  a test silently skip when a fixture is missing.
- A cell earns a place in `regression_spotcheck.sh` only if it failed before
  the fix and passes after (§3).
- Report coverage as a fraction of the C surface, listing what is MISSING
  first. "Ported `motion_estimation.c`" means every function or a named subset,
  never "the important parts".

## 4. A cross-lane PREREQUISITE the wiring chunk must land first (measured 2026-08-31, wp-entropy)

Found by reading `entropy/context.rs::FrameContext::new_default`, not inferred:
**seven of the inter CDF tables on `FrameContext` are UNIFORM PLACEHOLDERS, not
the C defaults**, and twelve more have no field at all.

| state | tables |
|---|---|
| placeholder (field exists, value is uniform) | `skip_mode_cdf`, `newmv_cdf`, `globalmv_cdf`, `refmv_cdf`, `drl_cdf`, `inter_compound_mode_cdf`, `interp_filter_cdf` |
| absent (no field) | `comp_ref_type_cdf`, `uni_comp_ref_cdf`, `comp_bwdref_cdf`, `obmc_cdf`, `motion_mode_cdf`, `comp_group_idx_cdf`, `compound_index_cdf`, `compound_type_cdf`, `interintra_cdf`, `interintra_mode_cdf`, `wedge_interintra_cdf`, `wedge_idx_cdf` |
| already correct | `single_ref_cdf`, `comp_ref_cdf`, `comp_inter_cdf`, `intra_inter_cdf`, `kf_y_mode_cdf`, `y_mode_cdf`, `tx_size_cdf`, `angle_delta_cdf` (the first three are asserted against C in `tests/c_parity_entropy_inter.rs`) |

This is **byte-inert on the existing still envelope** — the public entry point
still refuses inter frames and no intra site touches those tables — but the
first inter block coded against a uniform table desyncs the tile immediately,
so it gates EVERY inter block writer.

All nineteen correct tables already exist, extracted from the real
`svt_aom_init_mode_probs` and re-asserted against it at tier 1, in
`svtav1_encoder::port_entropy_inter::cdfs`, with
`port_entropy_inter::InterCdfs` as the per-frame mutable carrier. `FrameContext`
is owned by the C1 lane, so the constants are **ready to lift, not lifted**:
move them onto `FrameContext`, point `InterCdfs`'s users at it, and move
`default_inter_cdf_tables_match_c` with them. Nothing else about the tables
needs re-deriving.

Two smaller notes from the same pass:

- `entropy/obu.rs::write_inter_frame_header` writes **seven hardcoded zero
  bits** for global motion. That is correct ONLY for all-IDENTITY with
  `primary_ref_frame == PRIMARY_REF_NONE`;
  `port_entropy_inter::gm::write_global_motion` is the real writer (its
  `write_global_motion_traced` case pins the seven-zero-bit case as one
  outcome, not the definition).
- Guard #5 in `rust/CLAUDE.md` covered only the ALL-INTRA arm; SGR loop
  restoration is LIVE in video mode at presets 0..3. Corrected in place as
  guard 5c.

## 4. C1a measured outcomes (append-only log — one entry per landed chunk)

### C1a-dlf — video-mode key frame, deblock arm (2026-08-31)

Reference cell `gradient 64x64 q40 p6 frames=2`
(`tools/identity_diff_inter.sh`), frame 0 (`c.obu.pts0` vs `rs.obu.f0`):

| | before | after |
|---|--:|--:|
| C frame 0 | 961 B | 961 B |
| port frame 0 | 973 B | 971 B |
| `loop_filter_level[0]` C / port | 0 / **3** | 0 / **0** |
| first diverging FH field | `loop_filter_level[0]` | `cdef_y_pri_strength[0]` |

Localized with the new `tools/fh_fields.py` (the frame-header sibling of
`tools/sh_fields.py`; there was none before this chunk).

**Root cause, and the shape the remaining C1a chunks share.** C dispatches the
whole per-picture signal derivation on `scs->allintra`
(`md_config_process.c:924-930`, `picture_decision`'s
`svt_aom_sig_deriv_multi_processes_{allintra,rtc,default}`). The port had
flattened the ALLINTRA resolution of several of those ladders into per-preset
predicates gated on `is_single_frame`, so a VIDEO-mode key frame fell through
to whichever arm the still path used. That is correct for still and wrong for
video at every ladder that forks.

- **Deblock (this chunk, fixed).** `get_dlf_level_allintra` (`:1540`) gives
  level 5 at M6 -> `sb_based_dlf = 1` -> the by-q closed form (filt_guess 3 at
  qindex 67). `get_dlf_level_default` (`:1466`) gives level 3 on a base picture
  -> `sb_based_dlf = 0` -> the full-image SSE search, which picks 0 on this
  content. The port now runs both ladders through the ported
  `svt_aom_set_dlf_controls` table and lets `enabled` / `sb_based_dlf` /
  `early_exit_convergence` choose the picker.
- **CDEF (this chunk, fixed).** `svt_aom_sig_deriv_multi_processes_allintra`
  (`:2337`) gives `cdef_search_level = 7` at M6; `_default` (`:1973`) gives
  `is_base ? 5 : 6` = 5 on a key frame. The port's `is_single_frame &&
  allintra_preset_uses_cdef_search(preset)` gate dropped a video key frame onto
  the qp fast path, exactly the deblock bug one filter later. The port now runs
  both ladders and maps the level through the ported `set_cdef_search_controls`
  (`:891`), letting `enabled` / `use_qp_strength` choose the arm.

  One correction to the line above, which said the `_default` level ladder
  needed porting: it did NOT. It was already in
  `port_enc_mode_config::multi_processes` at tier 1 — it was merely unreachable
  from `pipeline.rs`. What was missing was `set_cdef_search_controls` (the port
  carried only the ALLINTRA ladder's resolved candidate sets, flattened per
  preset) and the allintra ladder as a function. Check what is ported before
  scoping a chunk from this file.

  MEASURED on the reference cell (`64 64 40 6 2 gradient`, frame 0): before,
  `cdef_y_pri 1 / y_sec 0 / uv_pri 1` against C's `0 / 2 / 7`; after, the luma
  pair matches and the first divergence is **`cdef_uv_pri_strength[0]`,
  C = 7, port = 0**. The port picks C's level-5 luma candidate
  `pf_gi[0] + 2 = 2`; C's chroma pick is `pf_gi[7] = 28` from the same set.
  NOTE for whoever takes that field: on this cell the TILE payload already
  differs (C 961 B, port 971 B, first differing tile byte at 0x1b), so the CDEF
  search is scoring a different recon and the chroma gap is not yet
  attributable to the chroma search itself — narrow the tile divergence first,
  or find a cell where the tile agrees. Flat content is such a cell: video-mode
  `uniform 64x64 q40` frame 0 is byte-identical at presets 0/3/6/8
  (28/28/28/30 B).

### What the CDEF chunk measured about the REST of the chain (2026-08-31)

Two facts, both measured, that shape every chunk after this one. Neither was
known when this file was written.

**1. On the reference cell the frame header is nearly done and the tile is
untouched — but that is ONE cell, see the correction below.** On
`64 64 40 6 2 gradient` frame 0 `tools/fh_fields.py` now reports exactly
ONE differing frame-header field — `cdef_uv_pri_strength[0]` (C 7, port 0) —
and every field after it equal. The uncompressed header ends at **bit 71** of
the frame OBU payload (payload starts at file byte 0x12), so the header is file
bytes 0x12..0x1a and the tile starts at **0x1b** — which is the first byte of
tile data and it DIFFERS (C 0xc5, port 0xb5). The frame OBU payload is 943 B
for C and 953 B for the port with a fixed-length, field-identical header, so
all 10 bytes of the size difference are tile payload.

So the video-mode tile diverges **at its first symbol**. The remaining CDEF
chroma gap is downstream of that (the search scores a recon those symbols
produced), which is why it should NOT be chased as a chroma-search bug until
a cell exists where the tile agrees. Flat content is one: video-mode
`uniform 64x64 q40` frame 0 is byte-identical at presets 0/3/6/8, and those
four are now cells in `regression_spotcheck.sh` (`byteVideoKey`).

**1b. CORRECTION to 1, measured before this file was committed: the reference
cell is not representative.** A five-content probe of the video-mode key frame
(`identity_diff_inter.sh W H 40 P 2 <content>`, frame 0, first diverging FH
field per `fh_fields.py`):

| cell | C / port bytes | first diverging FH field |
|---|---|---|
| `uniform 64x64 p0/p3/p6/p8` | 28/28, 28/28, 28/28, 30/30 | **none — byte-identical** |
| `gradient 64x64 p6` | 961 / 971 | `cdef_uv_pri_strength[0]` 7 / 0 |
| `diag 64x64 p3` | 185 / 120 | `loop_filter_level[0]` 12 / 0 |
| `diag 64x64 p6` | 238 / 336 | `loop_filter_level[0]` 0 / 8 |
| `screen 64x64 p6` | 92 / 143 | **`allow_intrabc` 1 / 0** |
| `screenrep 64x64 p6` | 1144 / 1141 | `cdef_y_pri_strength[0]` 0 / 7 |

Read that table by CLASS, not by field:

* The `loop_filter_level` and `cdef_*_strength` rows are **searches reading a
  recon that already differs** — both pickers are now on C's arm (deblock
  chunk, CDEF chunk) and both sides are choosing from the same candidate set;
  they land differently because the tile that produced the recon differs.
  Chasing these before the tile agrees is chasing a symptom.
* **`screen 64x64 p6` `allow_intrabc` C = 1, port = 0 is NOT recon-driven.**
  It is a pure frame-level tool derivation, and it is the same unwired-arm bug
  again: C's video ladder gives `intrabc_level = 5` for a screen-content
  I-slice at `enc_mode <= ENC_M8` (`enc_mode_config.c:2034-2052`), while the
  allintra ladder gives 0 above ENC_M4 (`:2347-2369`) and `pipeline.rs` calls
  `sc_detect::derive_allintra_sc` unconditionally. The video ladder is ALREADY
  ported, at tier 1, in `port_enc_mode_config::multi_processes`
  (`intrabc_level` + `palette_level` + `allow_screen_content_tools`) — with no
  pipeline caller, exactly like the table in 2 below. That makes it the
  cheapest genuinely-attributable next chunk: a derivation divergence a gate
  can pin without first fixing the tile.

**2. Only TWO of `port_enc_mode_config`'s ladders are wired into
`pipeline.rs`.** Measured by grep on 2026-08-31: every
`port_enc_mode_config::` reference in `pipeline.rs` / `pd0.rs` /
`partition.rs` is DLF (`get_dlf_level_{allintra,default}` +
`set_dlf_controls`) or CDEF (`cdef_search_level_{allintra,default}` +
`set_cdef_search_controls`). The other ~7,400 lines of that module —
`common`, `encdec`, `leaf`'s remaining ladders, `light_pd1`, `md_config`,
`me`, `multi_processes`, `pd0`, `tail` — are ported at tier 1 and have **no
pipeline caller at all**.

The pipeline instead calls the ALLINTRA resolution directly and
unconditionally at, among others:

| site | call | video twin |
|---|---|---|
| `pipeline.rs:1939` | `quant::rdoq_level_allintra` | not ported |
| `pipeline.rs:3692` | `restoration::wn_filter_ctrls_allintra` | `port_lr_level::wn_filter_level_default` — ported, unused |
| `pd0.rs:2010`, `partition.rs:1736` | `pd0::pd0_detector_allintra_demotes` | `port_pd0_detector::pd0_detector` — ported, unused |
| `pd0.rs:2009/2125/2313` | `pd0::max_block_size_allintra` | `port_enc_mode_config::{common,encdec}` — ported, unused |

**This is the same bug the deblock and CDEF chunks each fixed once, and the
tile-payload divergence is almost certainly a pile of it.** The work is
mostly WIRING, not porting: for each site, derive the level from the arm that
matches `scs->allintra` and route it through the already-ported controls
table. Do not scope a chunk from this file without first grepping for the
`_default` twin — the CDEF chunk's brief said the `_default` CDEF ladder was
missing, and it had been ported (at tier 1) all along.

**Evidence (CDEF).** `crates/svtav1-encoder/tests/c_parity_cdef_search_ctrls.rs`
is **tier 1** by the same route: `set_cdef_search_controls` is file-`static` and
both ladders are inline in their callers, but the exported
`svt_aom_sig_deriv_multi_processes_{default,allintra}` run all three and leave
the answer in `pcs->cdef_level` + `pcs->cdef_search_ctrls`, which
`shims/cdef_shims.c` reads back. The differential compares the level, the nine
scalar control fields and **all 64 entries of all four candidate arrays**, over
both arms, and its anti-vacuity test asserts the sweep reaches every level
0..=10 and both `use_qp_strength` states. Verified live by mutation: flipping
level 5's `subsampling_factor` from 1 to 2 reddens both arms.

**Evidence (deblock).** `crates/svtav1-encoder/tests/c_parity_dlf_ctrls.rs` is **tier 1**:
`get_dlf_level_{default,allintra}`, `dlf_level_modulation` and
`svt_aom_set_dlf_controls` are all file-`static`, but the exported
`svt_aom_sig_deriv_mode_decision_config_{default,allintra}` reach all four and
leave the answer in `ppcs->dlf_ctrls`, which `shims/dlf_shims.c` reads back.
The eight control fields are distinct for each of the eight levels, so the
level is pinned even though C never stores it. This retires the tier-4 claim in
`tests/sig_deriv_dlf_traced.rs`.

**No still regression.** 6/6 identity cells byte-identical at their expected
sizes (gradient 64x64 q40 p6 = 290 B, q20 p3 = 839 B, q55 p0 = 63 B,
128x128 q55 p8 = 171 B, 64x64 q30 p13 = 580 B, screenrep 64x64 q35 p4 = 693 B);
`tools/regression_spotcheck.sh` 35/35; `cargo nextest run --workspace`
2085/2085.

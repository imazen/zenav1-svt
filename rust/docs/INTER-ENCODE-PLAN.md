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

## 1c. The video-vs-still MD divergence, MEASURED — the campaign's work queue

Written 2026-09-01. Before this, each chunk picked its next ladder by reading
`enc_mode_config.c` and guessing which fork mattered. That is now unnecessary:
`ref_sig_deriv_md_config_allintra` reads back the SAME 52-slot `MD_O_*` layout
as `ref_sig_deriv_md_config_default`, so the two arms are diffed FIELD FOR
FIELD from one input population through the real exported C symbols.
`tests/c_parity_sig_deriv_md_config.rs::video_key_frame_arm_divergence_at_m6_is_exactly_this_set`
asserts the COMPLETE set at the reference cell (gradient 64x64 q40 p6, R240p,
I-slice, base), so every slot NOT listed is proven equal, and a chunk that
wires one and forgets to move its row fails the test.

At M6, on a video-mode KEY frame:

| slot | allintra | video | state |
|---|---|---|---|
| `rdoq_level` | 3 | 1 | WIRED (`rate_arm`) |
| `pic_filter_intra_level` | 2 | 0 | WIRED (`intra_arm`) |
| `intra_level` | 6 | 2 | WIRED (`intra_arm`) |
| `nsq_geom_level` / `nsq_search_level` | 0 | 15 | WIRED (`part_arm` + `NsqCfg::for_arm`) |
| `pic_block_based_depth_refinement_level` | 10 | 6 | WIRED (`DrCtrls::for_arm`) |
| `txt_level` | 8 | 7 | WIRED (`funnel_arm`) |
| `cfl_level` | 4 | 2 | WIRED (`funnel_arm`) |
| `nic_level` | 6 | 8 | HELD on `wip/video-md-arms` — complete and tier-1 verified, but it is the one arm that pushes `video-key-nsq-arm-p5-72x88` back outside its 0.3% limit (0.067% without it, 0.539% with) |
| `pic_pd0_lvl` | 1 | **3** | OPEN, the LAST live divergence at M6 on a key frame. `PD0_LVL_3` is unimplemented in `pd0.rs`. Its **subres** half cannot reach PD1 — refuted at tier 1 2026-09-01, see §1d item 2; what remains is the PD0 costs the depth gates read and `pd0_use_src_samples` |
| `pic_depth_removal_level` | 0 | 5 | INERT on a key frame — `set_depth_removal_level_controls` (enc_mode_config.c:2968) zeroes `enabled` for an `I_SLICE` before it reads the level, and the port already models that (`port_enc_mode_config::common`). The LEVELS differ; the CONTROLS cannot. |
| `allow_high_precision_mv`, `is_motion_mode_switchable`, `pic_obmc_level`, `interpolation_search_level`, `interpolation_filter`, `md_nsq_mv_search_level`, `md_pme_level`, `me_subpel_level`, `pme_subpel_level` | | | inter-only, cannot move a key frame's bytes |

At other presets the same probe adds `txs_level` (allintra 0 vs video 4 at M9 —
now WIRED, `txs_arm`) and moves several rows; run it rather than extrapolating
from the M6 column.

**Not in that table, because it is not an MD level at all:**
`scs->seq_header.enable_intra_edge_filter`. The video arm signals it as 1 at
EVERY preset (`enc_mode_config.c:2820`) where the allintra arm signals it only
at preset 5 (`:2815`), and `FunnelCfg::for_preset` baked the allintra rule and
ran it on every frame. That made the sequence header and the encoder's own
prediction disagree on a video key frame — the header told the decoder to
edge-filter and upsample directional predictions and the funnel predicted
unfiltered. Fixed 2026-09-01; both now read
`intra_arm::intra_edge_filter`. `frm_hdr->tx_mode` had the same shape (the
writer emitted C's allintra unconditional TX_MODE_SELECT, where the video arm
signals TX_MODE_LARGEST from preset 10 up) and is fixed with it.

**Lesson for the rest of the campaign: the divergence table is necessary but
not sufficient.** It enumerates what `sig_deriv_mode_decision_config` assigns.
Anything the arms fork on OUTSIDE that function — the sequence-header
derivation, `sig_deriv_enc_dec_*`, `set_qp_based_th_scaling_ctrls_*` — is
invisible to it, and the edge-filter bug lived in the first of those.

**The second one landed 2026-09-01 and it is bigger: `ctx->mds0_use_hadamard_sb`,
which decides WHICH DISTORTION MDS0 SCORES WITH.** `svt_aom_sig_deriv_enc_dec_allintra`
writes `true` (`enc_mode_config.c:8148`); `_default` (`:7916`) and `_rtc`
(`:8032`) write `false` — literals, at every preset, on every frame type — and
`fast_loop_core` then picks `hadamard_path` (a SATD, `product_coding_loop.c:1283`)
or the two-buffer VARIANCE `fn_ptr->vf` = `svt_aom_variance{W}x{H}` (`:1296-1306`).
`fast_loop_core_light_pd1` (`:1040`) uses `vf` unconditionally, so the video arm
is variance everywhere in C. Full record in §1e; the wiring is on
`wip/video-md-arms`.

## 1d. First byte-identical VIDEO-MODE key frames (2026-09-01)

`screen` 64x64 preset 6, frames=2 frame 0, is byte-identical to C at q20, q40
AND q55 (C 92 B; the port emitted 119 / 118 / 116 B before the
depth-refinement arm chunk). The same scan finds `screen` 64x64 q40 identical
at presets 0, 2, 4 and 5, and `uniform` identical through preset 9. Pinned by
three `byteVideoKey` cells in `tools/regression_spotcheck.sh`.

`uniform` had been identical since the CDEF arm chunk, but it codes 28 B and
reaches almost nothing; `screen` is the first content that exercises the
search.

### The reference cell: the PARTITION TREE already matches (measured 2026-09-01)

Run on the Linux box, where `capture_c_trace` gets its `-Wl,--wrap` op-trace
build (macOS `ld64` has no `--wrap`, `WORKING-ON-THIS.md` §5):

```bash
SVT_CTREE_OUT=~/tmp/c.tree SVTAV1_PACKTREE=~/tmp/rs.tree \
  tools/identity_diff_inter.sh 64 64 40 6 2 gradient ~/tmp/cell
head -14 ~/tmp/c.tree > ~/tmp/c.f0.tree     # SEE THE TRAP BELOW
python3 tools/tree_diff.py ~/tmp/c.f0.tree ~/tmp/rs.tree
```

Result on `gradient 64x64 q40 p6`: **4 blocks joined, 0 port-only geometry** —
both encoders code the SB as four 32x32 squares. Every divergence is the intra
MODE: C codes D135(+3) / SMOOTH_V / D135(+3) / H, the port DC on all four. So
the remaining gap at the reference cell is a LEAF decision, not a partition
decision, and `pic_pd0_lvl` (the last OPEN row in §1c) is not obviously its
cause.

**Harness trap, and it produced a wrong answer first:** `SVT_CTREE_OUT`
APPENDS across frames, exactly like `SVTAV1_PACKTREE` (§5). The last four lines
of a 2-frame dump are the INTER frame — `mode` 13/16, `skip=1` — and joining on
them invents mode and skip flips that do not exist on frame 0. Cut the file at
frame 0 before diffing.

**The reference cell is still open.** On `gradient` and `diag` the port
UNDER-shoots C's byte count at 64x64 q40 p6 (947 B vs C 961; 163 vs 238), and
that is worth reading carefully rather than as "nearly there": a smaller stream
at the same qp is what over-searching looks like, and the port over-searches
because the ladders wired so far are the search-WIDENING ones. Do not read a
smaller number as "closer".

The tree-diff above says where to look next, and it is NOT where §1c's last
OPEN row points. The partition tree already agrees, so `pic_pd0_lvl` — a
PARTITION-search level — is not obviously what decides these four blocks; the
port's own MDS3 dump (`SVTAV1_NSQDBG=1 SVTAV1_CANDDBG=1`) shows it injecting
the full intra_level-2 candidate set, evaluating D135 / SMOOTH_V / V at MDS3,
and ranking DC best by about 1% where C picks the directional mode. Two
candidates for that, in order of cheapness to test:

1. the MDS0/MDS3 costs themselves — the port's `PFAST` fast costs rank
   `mode=4 delta=+3` LAST of the D135 trio while C picks exactly that
   candidate, which is a prediction- or SATD-domain difference, not a
   candidate-set one;
2. `pic_pd0_lvl` after all, through `pd0_use_src_samples`: `svt_aom_sig_deriv_enc_dec_pd0`
   sets it `allintra || pcs_hbd_md`, so a VIDEO frame's PD0 predicts from
   RECON where the port always predicts from SOURCE. That changes PD0 costs
   (and so the depth-refinement gates that read them) at EVERY video preset,
   independently of the level.

### Where the reference cell's four blocks are actually decided (2026-09-01)

Measured with the C-side interposers (`SVT_FASTCOST_OUT` / `SVT_FULLCOST_OUT`
with `..._XY="0,0"`, Linux `--wrap` build) against the port's
`SVTAV1_NSQDBG=1 SVTAV1_CANDDBG=1` dump, on `gradient 64x64 q40 p6` video mode,
block (0,0) 32x32. This replaces guessing at the last OPEN row with a reading
of both encoders' candidate lists.

**MDS0 is NOT the divergence.** Both rank DC poorly. C's fast costs put
`mode=10` first (2,763,652,131) and `mode=0` nineteenth (2,851,972,217); the
port's `PFAST` likewise puts `mode=10` first and DC fourth. The two agree that
DC is a bad MDS0 candidate.

**The SURVIVOR SET is.** C's MDS1 (`CFULL st=1`) at that block is exactly
`{10, 4/ang0, 4/ang+3, 6/ang-3, 6/ang0}` — five candidates, no DC — and its
MDS3 (`st=3`) is `{4/ang0, 4/ang+3}`, two candidates, which is why C codes
`mode=4 ady=3`. The port's MDS3 list is `{DC, mode1, mode4/ang0, mode10}` and
DC wins it at 48,310,575 against mode10's 48,780,160. **The port's problem is
that DC reaches MDS3 at all.**

Two mechanisms feed C's tighter set, and NEITHER is in the port today:

1. **`nic_level` 8 vs 6** — stage counts `{2,1,1}` of 16 instead of
   `{6,6,6}`, and `mds1_cand_base_th_intra` 300 instead of 1200. That is
   `nic_arm`, complete on `wip/video-md-arms` (below).

2. ~~**A subresolution LEAK from PD0 into PD1's MDS1, which is the real
   content of the last OPEN row.**~~ **REFUTED 2026-09-01, at tier 1. There is
   no leak, and this whole item was wrong.** It is kept in place rather than
   deleted because the reading that produced it is one grep away and reads as
   obviously true.

   What it said: `set_subres_controls` has exactly ONE call site
   (`svt_aom_sig_deriv_enc_dec_pd0`, `enc_mode_config.c:7357`), so
   `ctx->subres_ctrls` is derived from `pic_pd0_lvl` and persists into PD1 on
   the same context; `md_stage_1` reads it with NO `PD_PASS_1` guard
   (`product_coding_loop.c:7027`) where `md_stage_2` (`:7052`) and `md_stage_3`
   (`:7156`) both zero it; so the video arm's `pic_pd0_lvl = 3` would make the
   MDS1 full loop that chooses the MDS3 survivors run on half the rows.

   Why it is wrong: **`set_subres_controls` has FOUR call sites, not one.** The
   three REGULAR-PD1 derivations each call `set_subres_controls(ctx, 0)`
   unconditionally — `svt_aom_sig_deriv_enc_dec_default` `:7919`,
   `_rtc` `:8035`, `_allintra` `:8151`, none behind a branch and none after a
   `return` — and `enc_dec_process.c:3038-3050` runs one of them on the SAME
   `ModeDecisionContext` between PD0 and PD1's md loop. By the time `:7027`
   runs, the step is 0 on every regular-PD1 arm. The unguarded read is
   redundant with the guarded ones, not a divergence.

   Pinned by `crates/svtav1-encoder/tests/c_parity_subres_carry.rs` (tier 1,
   `crates/svtav1-cref/shims/sigderiv_shims.c::ref_subres_pd0_then_pd1` drives
   PD0 and then one PD1 arm on ONE context, in C's order): on the reference
   cell's video-arm population PD0 leaves `step = 1, dev_th = 5`, and every
   regular arm leaves `0, 0` — at every `pd0_level` 0..=6.

   The **light-PD1** arms are the exception, and they are the positive control
   that the probe can see a surviving step at all: `_light_pd1_default`
   (`:7574`) and `_light_pd1_rtc` (`:7811`) set only
   `subres_ctrls.odd_to_even_deviation_th = 0` and leave `step` alone, so PD0's
   1 does survive there. It is unread — light PD1's loop is
   `md_stage_0_light_pd1` + `md_stage_3_light_pd1`, and the latter forces
   `mds_subres_step = 0` (`:7133`); `md_stage_1` is never called on that path.

   The C dump quoted as confirmation ("every 32x32 `CFAST` row carries
   `subres=1 lam=18500`") does not survive this either: `CFAST` is the MDS0
   fast-cost interposer, and `mds_subres_step` is read by `full_loop_core`, not
   by the fast cost — so a `subres=` value on a `CFAST` row is whatever the
   previous stage left in the field, and the two lambdas in that dump are PD0's
   and PD1's. **A dumped field is evidence only where the code under test reads
   it** (`WORKING-ON-THIS.md` §5, the shifted-out-of-relevance trap).

**Consequence for scoping `pic_pd0_lvl`.** The subres half is NOT the part that
reaches the PD1 leaf decision — it cannot reach it at all. `pic_pd0_lvl` moves
PD1 only through what PD0 leaves behind: the partition tree, the PD0 costs the
depth-refinement gates read, and `pd0_use_src_samples` (PD0 predicting from
RECON on a video frame where the port always predicts from SOURCE). At the
reference cell the tree already matches C (4 blocks joined, 0 port-only
geometry), so the remaining `pic_pd0_lvl` surface there is the PD0-cost /
`pd0_use_src_samples` half. Item 1 above — `nic_arm` — is what changes the MDS1
survivor set, and the measurement below says it is the whole of the reference
cell's remaining candidate-set gap.

### 1e. The reference cell's MODE DECISION is CLOSED (2026-09-01) — `mds0_use_hadamard_sb`

`gradient 64x64 q40 p6` video, frame 0: with `encdec_arm` + `nic_arm` the
port's coded partition tree and **every leaf mode, uv mode and angle delta
equal C's** — `tools/tree_diff.py` reports **0 field flips** where main reports
12 and four wrong leaf modes. C codes D135(+3) / SMOOTH_V / D135(+3) / H on the
four 32x32 leaves and the port now codes exactly that. Bytes 947 -> 965 against
C's 961; the residual 4 B are in the RESIDUAL CODING, not in mode decision.
Both arms are required — encdec alone leaves 8 flips (948 B), nic alone 12
(952 B).

**What it was.** §1d's item 1 named `nic_level`, and it was half the answer.
The other half is not in §1c's table at all: `ctx->mds0_use_hadamard_sb`
selects MDS0's luma distortion, C's video arm sets it FALSE (variance) where
its allintra arm sets it TRUE (Hadamard SATD), and the port ran the allintra
value on video frames. **Variance is DC-invariant and SATD is not**, so every
candidate whose prediction is FLAT scores identically under one and
differently under the other. Measured at block (0,0) 32x32 of the reference
cell — C's `SVT_FASTCOST_OUT` interposer against the port's `SVTAV1_CANDDBG`:

| | flat group (DC, V/0, V/-3, H/0, H/+3, D45/\*, D203/\*, D67/\*) | D135/0 | D135/+3 | D157/-3 |
|---|---|---|---|---|
| C (`hadblk=0`, variance) | all exactly `1392540` | `1356698` | `1356851` | `1359225` |
| port (SATD) | SPREAD: DC `53600`, V/0 `53472`, D45/0 `53472`, D67/0 `53472` | `53504` | `54064` | `53986` |

So C's MDS1 survivor set was `{SMOOTH_V, D135/0, D135/+3, D157/-3, D157/0}`
and the port's `{SMOOTH_V, V/0, D67/0, D45/0, D67/-3}` — one candidate in
common, and its MDS1 full cost agreed to the BYTE (`48577658` on both sides),
which is what says the divergence was the MDS0 METRIC and not the machinery
around it.

**Why it is on `wip/video-md-arms` and not on main**, per-cell (frame 0, %
off C's byte count; the four `ratioVideoKey` scoreboard cells plus the
reference cell):

| cell | base | +encdec | +nic | +both | limit |
|---|--:|--:|--:|--:|--:|
| `gradient 72x88 q40 p4` | 0.57 | 0.86 | 0.93 | **2.00** | 1.0 |
| `gradient 72x88 q40 p5` | 0.07 | 0.40 | 0.54 | **0.00** | 0.3 |
| `screenrep 72x88 q40 p7` | 0.38 | 0.25 | 0.42 | **0.00 (byte-identical)** | 0.5 |
| `gradient 72x88 q40 p9` | 0.06 | 2.39 | 0.31 | **1.20** | 1.0 |
| `diag 64x64 q40 p11` | 0.75 | 16.96 | 0.75 | **16.96** | 2.0 |
| `gradient 64x64 q40 p6` (reference) | 1.46 | 1.35 | 0.94 | **0.42** | — |

The pair CLOSES the cell that held `nic_arm` back (`video-key-nsq-arm-p5-72x88`
is now exactly C's byte count) and makes `video-key-nsq-arm-p7-screenrep-72x88`
byte-identical; p4, p9 and the p11 edge-filter witness go outside their limits.
Moving a limit is a threshold change, so the pair waits.

**Where to start on the blocker.** `diag 64x64 q40 p11` is the cleanest: it is
pure GEOMETRY — 0 mode flips, 12 C-only / 6 port-only blocks, the port coding
16x16 everywhere where C codes an 8x8/32x32 mix — and `nic_arm` does not move
it at all (0.75% with and without). So the suspect is the DEPTH decision that
consumes the funnel's leaf costs, not the leaf decision itself.

### 1f. CORRECTION to §1e's blocker reading — the arms did not break the geometry (2026-09-01)

The paragraph above is right about WHAT the p11 cell looks like with the arms
on and wrong about what that implies. **The 12-C-only / 6-port-only geometry
gap is IDENTICAL without the arms.** Measured with C's own coded tree
(`SVT_CTREE_OUT`, the `svt_aom_update_mi_map` `--wrap` dump, run in
`tools/ctrace-linux/` because Apple `ld64` has no `--wrap`) joined against
`SVTAV1_PACKTREE` by `tools/tree_diff.py`, on ONE build with only
`mds0_use_hadamard_sb` forced back to the allintra `true`:

| cell | geometry C-only / port-only, arms OFF | arms ON | field flips OFF -> ON |
|---|---|---|---|
| `gradient 72x88 p4` | 56 / 12 | 59 / 11 | 159 -> 164 |
| `gradient 72x88 p9` | 12 / 7 | **12 / 7, same mi list** | 47 -> **26** |
| `diag 64x64 p11` | 12 / 6 | **12 / 6, same mi list** | 18 -> **6, all `bsize`** |

At p9 and p11 the arms leave the coded GEOMETRY bit-identical and cut the
mode / uv flips by about two thirds; at p11 every comparable field except
`bsize` then equals C, which is the same result §1e reports for the reference
cell. The port codes 16 uniform 16x16 leaves there either way — 398 B without
the arms, 469 B with — while C codes `4x 8x8 | 16x16 | 16x16 | 4x 8x8 | 32x32 |
32x32 | (8x8/16x16 mix)` for 401 B.

So the byte-count regression is **the removal of a COMPENSATING mode error**,
not a new geometry error, and `main`'s 0.75 % on that cell is the wrong tree
coded with the wrong modes landing near C's size by cancellation. That is
exactly the trap §1d warns about from the other side ("a smaller stream at the
same qp is what over-searching looks like"): here a stream INSIDE its limit was
the wrong tree scored with the wrong metric. **The three cells cannot be closed
by changing anything the two held arms touch.**

**What they can be closed by, named at tier 1.** Running
`c_parity_sig_deriv_md_config.rs`'s two exported entry points per preset on the
reference cell's key-frame population (`is_islice`, `is_base`, R240p, qp 40)
and diffing all 52 `MD_O_*` slots, the arm-divergent set at each failing preset
contains exactly ONE row that is not inter-only, not INERT on an I-slice, and
not already wired:

| preset | `PD0_LVL` allintra / video | other unwired divergence |
|---|---|---|
| M4 | 1 / **3** | none |
| M9 | 7 / **4** | none |
| M11 | 7 / **4** | `MDS0` 0 / **2** — now WIRED, see below |

(`DEPTH_REFINE` is 6/6 at M4 and 10/10 at M9 and M11 — the arms AGREE on it, so
it is not the depth-refinement level. `DEPTH_REMOVAL` diverges but
`set_depth_removal_level_controls` zeroes `enabled` on an I_SLICE before it
reads the level, per §1c.)

**`pic_pd0_lvl` changes three things, and the port models only the first.**

1. **The PD0 LEVEL.** `pipeline.rs` hardcodes the ALLINTRA resolution at every
   preset: `pd0::max_block_size_allintra` + `pd0_detector_allintra_demotes` +
   `Pd0Mode::{Lvl6, Lvl5}` at preset >= 9, `Lvl1` below. C's video arm asks for
   `PD0_LVL_3` at M3..M7 and `PD0_LVL_4` at M8..M13 (`set_pic_pd0_lvl_default`,
   `enc_mode_config.c:8592`; `set_pd0_ctrls`, `:5413`). **Neither level exists
   in `pd0.rs`** — it carries LVL_0, LVL_1, LVL_5 and LVL_6 only.
2. **`svt_aom_sig_deriv_enc_dec_pd0`'s level-dependent knobs, none of which are
   in §1c's table** (that table reads `sig_deriv_mode_decision_config`, and this
   is the OTHER derivation — the same blind spot that hid
   `mds0_use_hadamard_sb`):
   * `depth_early_exit_lvl` is **2** (`split_cost_th` 50, `early_exit_th`
     **900**) for `pd0_level > PD0_LVL_1`, where the allintra M2..M8 LVL_1 takes
     1 (`early_exit_th` 0 — which `Pd0Ctx::pick` spells as `th = 1000`).
     `:7230-7236`.
   * PD0 `subres_level` is **1 on an I-slice** at LVL_3 and LVL_4 (`:7337-7341`,
     gated on `disallow_4x4` and a complete b64), where `pd0_level <=
     PD0_LVL_2` forces 0.
   * `rate_est_level` is 2 at LVL_0..LVL_3 and **4** at LVL_4 (`:7355-7365`),
     i.e. `coeff_rate_est_lvl` 2 — the fast coeff approximation — where LVL_5 /
     LVL_6 use 0 (`lpd0_qp_offset` 8 + the `5000 + 100*eob` closed form).
3. **`ctx->pd0_use_src_samples = allintra || pcs->hbd_md` (`:7309-7313`) —
   FALSE on every video frame.** C's video PD0 predicts each block from the
   RECON it generates per block (`product_coding_loop.c:8430`,
   `mode_decision_update_neighbor_arrays_pd0` at `:123`); the allintra arm
   instead copies the SOURCE row / column into the recon-neighbour arrays
   (`:8370`) and generates no recon at all. **The port's PD0 always predicts
   from source.** This is §1d item 2's surviving half, it is a per-BLOCK
   behaviour change at EVERY video preset (not only where the level moved), and
   it is the largest single piece of the chunk.

Method notes for whoever takes it: `SVT_CTREE_OUT` APPENDS across frames just
like `SVTAV1_PACKTREE` (§1d), and on a 2-frame run the inter frame's blocks are
the tail — cut the file at the LAST `mi=(0,0)` line, not at a fixed line count,
because the frame-0 block count varies with the preset. `tools/ctrace-linux/`
runs fine under colima's native arm64 profile on this host; the `zenav1-svt-
ctrace-cbuild` volume makes a second cell a ~20 s run.

### 1g. `pic_pd0_lvl` WIRED at preset >= 9 — `diag p11`'s tree is now EXACTLY C's (2026-09-01)

Held on `wip/video-md-arms` with the two MD arms, because the three only make
sense together (below). What it does:
`crate::pd0::pd0_pick_sb_partition_video` + `crate::part_arm::video_pd0_params`
run the VIDEO arm's PD0 on the fixed-tree path — the level from
`set_pic_pd0_lvl_default` instead of `pd0_detector_allintra`, `max_block_size`
uncapped (`get_max_block_size_default` returns `super_block_size` outright),
and NSQ geometry ON (`nsq_geom_level` 3 against the allintra arm's 0).

**Result on `diag 64x64 q40 p11`: `tools/tree_diff.py` reports 22 blocks
joined, 0 C-only / 0 port-only geometry** — every `bsize` equal, against
`main`'s 12 / 6 and its 16 uniform 16x16 leaves. That tree was dumped with the
two MD arms OFF, which is the point: the geometry is PD0's alone, and the arms
then supply the MDS0 metric that fixes the 8 mode / 8 uv flips left on those 22
blocks. Arms off the cell is 325 B against C's 401 (18.953 %, the right tree
with the wrong modes); arms on it is 403 B (**0.499 %**).

**A CORRECTION to §1f's own table, and it is the same class of error §1f was
written to fix.** §1f says "M9 and M11 allintra 7 / video 4". That was measured
with `c_parity_sig_deriv_md_config.rs`'s `Case::default()`, which sets
`seq_qp_mod = 0`. **C sets `scs->seq_qp_mod = 2` unconditionally**
(`Globals/enc_handle.c:3994`), and `set_pic_pd0_lvl_default`'s qp offset is
`(seq_qp_mod <= 1) ? 0 : ldp0_lvl_offset[qp_band]`. At the cells' CLI qp 40
(band 2, offset 1) the video arm's level at M9..M13 / 240p is therefore **5**,
not 4 — PD0_LVL_5, which the port already models — and 6 at qp <= 27, 4 at
qp >= 44. The M3..M7 row is unaffected (a flat 3 at 240p, no offset term).
`part_arm::SEQ_QP_MOD` already carried the right value; the probe did not.
**Any ladder with a `seq_qp_mod` term must be probed at 2.**

**Per-cell, frame 0, % off C's byte count:**

| cell | limit | main | + the two arms | + arms + video PD0 |
|---|--:|--:|--:|--:|
| `gradient 72x88 q40 p4` | 1.0 | 0.570 | 2.000 | **1.996** |
| `gradient 72x88 q40 p5` | 0.3 | 0.067 | 0.000 | **0.000** |
| `screenrep 72x88 q40 p7` | 0.5 | 0.377 | 0.000 | **0.000** |
| `gradient 72x88 q40 p9` | 1.0 | 0.063 | 1.196 | **0.189** |
| `diag 64x64 q40 p11` | 2.0 | 0.748 | 16.958 | **0.499** |
| `gradient 64x64 q40 p6` (ref) | — | 1.457 | 0.416 | 0.416 |

The video PD0 **alone**, with the arms off, is WORSE than main on both cells it
moves — p9 1.825 %, p11 18.953 % — which is the same cancellation §1f
describes, seen from the other side. So the three pieces are one landing, and
that landing still leaves `video-key-nsq-arm-p4-72x88` at 1.996 % against its
1.0 limit. Moving a limit is a threshold change; the bundle waits.

**No still regression on the held bundle, measured**: `identity_full_8bit`
**1100 / 1100** byte-identical on the `wip/video-md-arms` head itself, not only
on main. It is byte-neutral by construction — every piece is gated on
`ScArm::Video` and `pd0_pick_sb_partition_video` has no allintra caller — but
the sweep is what says so.

### 1h. The M3..M8 half of `pic_pd0_lvl` — four variants MEASURED, none good

**SUPERSEDED 2026-09-01 by §1i, and the reason is worth more than the table.**
Every row below was measured over a PD0 whose coefficient rate ignored
`mds_subres_step` twice — C doubles the coeff bits under subres AND prices the
whole scan instead of half (rd_cost.c:1224 and :329), and the port did neither,
so a sub-sampled block came out up to 3x too cheap. The level reading here is
RIGHT (C's dump confirms PD0_LVL_3, subres 1, `early_exit_th` 900); what made
LVL_3 look catastrophic was the port's own rate, plus two more defects §1i
names. Read the table as "what a broken subres rate does", not as evidence
about C. Kept rather than deleted because the conclusion it invited — "something
in the LVL_3 subres block cost is wrong" — was correct, and because the next
session should see what a measurement over an unverified premise looks like.

`set_pic_pd0_lvl_default` gives the video arm **PD0_LVL_3** at M3..M7 (240p,
flat, no qp term), where the allintra arm takes PD0_LVL_1. Per
`svt_aom_sig_deriv_enc_dec_pd0` the two differ in exactly two things — subres
step 1 on an I-slice (`:7345`, LVL_1 is forced 0 by `pd0_level <= PD0_LVL_2`)
and `depth_early_exit_th` 900 instead of 1000 (`:7232`, since
`pic_pred_depth_only` is FALSE on the refinement path) — because
`rate_est_level` is 2 for every `pd0_level <= PD0_LVL_3`, i.e. LVL_1's own.

Wiring that through `pd0_pick_sb_partition_m6_eval` (presets 0..=8), measured
on the same cells:

| refinement-path model | p4 (1.0) | p5 (0.3) | p7 (0.5) | p6 ref |
|---|--:|--:|--:|--:|
| allintra LVL_1, th 1000 (today) | 1.996 | **0.000** | **0.000** | 0.416 |
| LVL_3 + subres, th 900 | **0.641** | 1.953 | 0.042 | 5.619 |
| LVL_3 + subres, th 1000 | **0.356** | 1.953 | **0.000** | 5.619 |
| LVL_1, th 900 | 2.067 | 0.875 | 0.042 | 0.416 |
| LVL_3 + subres + C's `is_complete_b64` gate, th 900 | 0.641 | 1.953 | 0.042 | 5.619 |

Read it as three findings, not one:

* **The subres step is what moves p4** — every variant carrying it improves p4
  by more than a factor of three — **and it is also what breaks p5 and the
  reference cell.** The threshold is a second-order effect (p4 0.641 vs 0.356).
* **The `is_complete_b64` gate is not the explanation.** C forces
  `subres_level = 0` on an incomplete b64 (`:7337`) and the port now seeds its
  `is_subres_safe` sentinel accordingly; the row is BYTE-IDENTICAL to the
  ungated one, so on these cells the odd/even-deviation check was already
  refusing subres on those SBs.
* **The reference cell is the decisive witness.** `gradient 64x64 q40 p6` is a
  single COMPLETE superblock, so every gate that could legitimately suppress
  subres is open, and turning it on moves the cell from 0.416 % to 5.619 % —
  undoing §1e's result that its coded tree and every leaf mode already equal
  C's. Something in the LVL_3 subres block cost is wrong, or a C gate outside
  `sig_deriv_enc_dec_pd0` closes it.

**The next probe, and it should not be another guess:** dump C's own per-block
PD0 costs and the resolved `subres_ctrls.step` for `gradient 64x64 q40 p6`
video through the `--wrap` interposers in `tools/ctrace-linux/`
(`SVT_PD0COST_OUT` / `SVT_PICKPART_OUT`) and compare against the port's, block
for block. Two candidates it will separate: (a) the port's `is_subres_safe`
scope — C determines it ONCE per SB (`enc_dec_process.c:2943` seeds it before
PD0 runs) while `pd0_pick_sb_partition_m6_eval` re-seeds it on every call, and
the refinement walk calls that function several times per SB; (b)
`ctx->pd0_use_src_samples`, still `true` in the port where the video arm's C is
`false` — §1f item 3, unported on both PD0 paths.

### 1i. The M3..M7 half CLOSED (2026-09-01) — three PD0 defects, and §1h's four variants were all measured over two of them

`video-key-nsq-arm-p4-72x88` is at **0.000 %** — byte-identical — and so are
`p5` and the `screenrep` p7 cell. The held bundle landed with it. What §1h was
missing was not a C gate; it was three defects in the port's own PD0, found by
DUMPING C's resolution instead of guessing at it a fifth time.

**The dump.** `svt_aom_sig_deriv_enc_dec_pd0` is the function that decides
everything PD0 runs with and nothing observed it, so §1h had to infer. A
`--wrap` interposer on it (`SVT_PD0CFG_OUT`, `tools/capture_c_trace/
wrap_recon.c`) now prints the resolved values. On `gradient 64x64 q40 p6`
video, frame 0, SB0 — and identically on every SB of the p4 and p5 cells:

```
lvl=3 subres=1 dev_th=5 split_th=50 exit_th=900 rate_lvl=1 qpoff=0
fastcoef=2 srcsamp=0 pred_only=0 d4=1 d8=0 maxbs=64 cb64=1 bias=1000
intra=1/12/1 nsq=1
```

PD0_LVL_3 with subres step 1 and `early_exit_th` 900 is therefore CONFIRMED,
not inferred — §1h's reading of the level was right. `subres=1` only on a
COMPLETE b64: the three edge superblocks of the 72x88 cells all report
`subres=0`, which is `!b64_geom->is_complete_b64` (`:7337`). The container
oracle was verified on the cell first (961 B, identical to the host's) per
`docs/WORKING-ON-THIS.md` §5.

**Defect 1 — the PD0 coefficient rate ignored `mds_subres_step`, twice.**
`svt_aom_txb_estimate_coeff_bits_pd0` (rd_cost.c:1224) ends with
`*y_txb_coeff_bits <<= ctx->mds_subres_step`, and the middle loop it calls
takes `c_start = MIN(eob - 2, eob / MAX(1, fast_coeff_est_level -
mds_subres_step))` (rd_cost.c:329) — so at step 1 the divisor drops from 2 to 1
and the WHOLE scan is priced, then the total is doubled. The port did neither.
MEASURED on the reference cell's 64x64 root: C `ybits=2355794`, port
`ybits=777355` — 3.03x low — while `dist` already agreed to the unit
(`2009984` on both), which is what says the residual, transform and quantizer
were right and only the rate was wrong. **Every row of §1h's table was measured
over this**, which is why "LVL_3 + subres" looked catastrophic.

**Defect 2 — `pd0_use_src_samples` (§1f item 3), now ported.** With the rate
fixed, C and the port agree to the unit on every block that has NO neighbour
and diverge on every block that has one. `crate::pd0::Pd0ReconCanvas` is the
port's model of `ctx->recon_neigh_y`: PD0 generates each block's recon
(`av1_perform_inverse_transform_recon` — inverse the sub-sampled transform into
the even rows, copy each onto the odd row below) and writes it back at exactly
C's decision points (`mode_decision_update_neighbor_arrays_pd0`: a leaf with
`mds->index < 3` writes itself, an abandoned split writes the parent, a won
parent writes itself, a won split writes quadrant 3). A pixel canvas rather
than C's two 1-D arrays, because PD0's decided blocks tile the superblock, so
the array value is always the canvas pixel at `(x, y-1)` / `(x-1, y)` — and
that lets `extract_neighbors_tiled` supply C's `n_top_px` clamp and edge
replication unchanged.

**Defect 3 — the depth early-exit ran on OUT-OF-BOUNDS quadrants.**
Pre-existing, on BOTH arms, and it is what kept the edge superblocks diverging
after defects 1 and 2 were fixed. `test_split_partition_pd0`
(product_coding_loop.c:10456) `continue`s a quadrant whose origin is outside
the mi grid BEFORE the early-exit test; the port tested it anyway. Because an
out-of-bounds child contributes 0 to `split_cost`, the extra test at `i == 3`
fires on a total C has already finished accumulating. MEASURED on
`gradient 72x88 q40 p5` SB1's 16x16 node at (64,16): parent `4972162` vs split
`4700296`, so C splits — the port's `i == 3` test (`4972162 * 900 <=
4700296 * 1000`) fired and kept the parent. It was invisible until PD0 started
predicting from its own recon, because the wrong winner is also what goes into
the neighbour arrays: the block below then predicted off an 8x16's bottom row
where C uses an 8x8's.

**Verification, and it is per-block, not per-byte.** `SVT_PD0COST_OUT` (C's
`svt_aom_full_cost_pd0`) joined against the port's new `SVTAV1_PD0DBG`
`PD0BLK` line — same fields, same order:

| cell | PD0 blocks, frame 0 | port vs C |
|---|--:|---|
| `gradient 64x64 q40 p6` | 75 | **75 / 75 identical** (dist, coeff bits, RD cost, lambda) |
| `gradient 72x88 q40 p5` | 138 | **138 / 138 identical** |

The block COUNT matching is part of the result: it is the depth early exit
pruning the same nodes.

**Per-cell, frame 0, % off C's byte count:**

| cell | limit | main before | held bundle | landed |
|---|--:|--:|--:|--:|
| `gradient 72x88 q40 p4` | 1.0 | 0.570 | 1.996 | **0.000** |
| `gradient 72x88 q40 p5` | 0.3 | 0.067 | 0.000 | **0.000** |
| `screenrep 72x88 q40 p7` | 0.5 | 0.377 | 0.000 | **0.000** |
| `gradient 72x88 q40 p9` | 1.0 | 0.063 | 0.189 | 0.000 (SIZE only) |
| `diag 64x64 q40 p11` | 2.0 | 0.748 | 0.499 | 0.499 |
| `gradient 64x64 q40 p6` (ref) | — | 1.457 | 0.416 | 0.416 |

THREE byte-identical VIDEO-MODE KEY frames on non-degenerate content at three
different presets, where before there were none outside the 64-aligned
`screen` cells.

**p9 is 0.000 % and NOT byte-identical, and the distinction is the point.** The
CDEF fix below moved it from 1586 B to 1589 B — C's exact byte COUNT — so its
`ratioVideoKey` cell now reads zero while a `byteVideoKey` run on the same cell
FAILS (`C=1589B port=1589B`). That was measured by trying the promotion, not
assumed: a ratio cell cannot tell "same size" from "same bytes", so a zero
percentage is not a parity claim. It stays `ratioVideoKey` with the attempt
recorded beside it.

**A cost this buys, recorded rather than discovered later.**
`Pd0ReconCanvas::new` allocates and fills `stride * 66` bytes per PD0 entry
call, and the refinement path calls that entry more than once per superblock.
At 64x64 and 72x88 that is nothing; at 4K it is ~250 KB of memcpy per call, on
the VIDEO path only (the allintra arm carries no canvas and is untouched). It
is correctness-first and deliberately so — the obvious narrowing is to seed
only the row above and the column left, which is all `extract_neighbors_tiled`
can read, but that is an optimisation to make against a measurement, not while
closing the cells.

**What is still open, said plainly.** `pd0_use_src_samples` is wired on the
LVL_1 FAMILY only — the refinement path at CLI presets 0..=8. The fixed-tree
path at preset >= 9 (`pd0_pick_sb_partition_video`, LVL_5 / LVL_6) still
predicts from source on both arms, and C's video arm does not; that is why
`diag p11` sits at 0.499 % rather than 0. The reference cell's residual 0.416 %
is likewise NOT in PD0 — its 75 PD0 blocks are exact — it is downstream of the
partition decision.

**A FOURTH defect, found while verifying the landing, and it is not in PD0.**
The held bundle passed the five ratio cells but broke a spotcheck cell nobody
had re-run against it: `video-key-txs-arm-tx-mode-p11` (`gradient 64x64 q40
p11`, `fhVideoKey`). MEASURED on the bundle head `59458226` itself, before any
of this chunk's changes: `cdef_uv_pri_strength[0]` C=0 port=15, C 1024 B vs
port 1026 B — where `main` passes the cell. So "the bundle is one cell from
landing" was half the story; it was two.

The cell's coded tree is EXACT (`tools/tree_diff.py`: 22 -> here 7 blocks
joined, **0 field flips, 0 C-only / 0 port-only geometry**), which is what
says the divergence is downstream of mode decision. It is
`cdef_recon_ctrls.zero_fs_cost_bias` (`set_cdef_recon_controls`,
enc_mode_config.c:1200) — `finish_cdef_search` scales the ZERO-strength
candidate's mse down by `factor/64` before the joint RD search, biasing toward
switching CDEF off, and the port ran no side of that ladder. It is the same
arm shape as everything else in this campaign:

| arm | `cdef_recon_level` | bias |
|---|---|--:|
| allintra (`:2432`) | `enc_mode <= M7 ? 0 : 1` | 0 / 61 |
| video (`:2102`) | `<= M8 ? 0 : <= M10 ? 1 : 2` | 0 / 61 / 61 |

At video M11 the bias is 61, and the port's own mse rows make the arithmetic
checkable by hand: luma `[32656, …]` -> `(63 * 32656) >> 6 = 32145` (the
`> 25000` rung), which does not move the luma pick (slot 3 wins either way, and
C signals the same 15/2); chroma `744` -> `(61 * 744) >> 6 = 709`, which drops
BELOW slot 1's `734` and flips the chroma pick from 15/0 to **0/0 — C's**.
The other two fields `set_cdef_recon_controls` carries are inert here:
`zero_filter_strength_lvl` and `prev_cdef_dist_th` are read only by
`me_based_cdef_skip`, which returns false immediately on an I_SLICE
(md_config_process.c:781). The bd10 search deliberately does NOT get this — C
selects a different, wider ladder on `encoder_bit_depth > 8` and porting the
8-bit one there would be a guess.

### `mds0_level` — WIRED 2026-09-01 (`crate::mds0_arm`), byte-inert and said so

The second row of the M11 table above is closed. `pcs->mds0_level` is
`is_islice ? 0 : 2` through M10 and **2 unconditionally above M10**
(`enc_mode_config.c:9232-9251`) on the video arm, against a literal 0 at every
preset on the allintra arm (`:10042`), so a video KEY frame diverges at
M11..M13. Level 2 is `pruning_method_th = (uint8_t)~0` + `dist_to_cost_th = 0`,
which selects `fast_loop_core`'s GLOBAL prune (`product_coding_loop.c:1325`):
any candidate whose distortion ALONE already costs more than the best complete
fast cost so far is abandoned with `MAX_MODE_COST`. `crate::mds0_arm` wires it
off the same `ScArm` every other `*_arm` module uses, through the tier-1-gated
`md_config::mds0_level_default`.

**A VACUITY BUG in the first landing of this, fixed the same day and recorded
rather than quietly amended.** The commit that introduced `mds0_arm`
(`f3020ddb`) shipped the module, the `FunnelCfg` field and the prune — but its
`pipeline.rs` call site was lost when the two held arms were reverted out of the
working copy around it, so **nothing called `mds0_arm::apply` and every "no cell
moved" measurement in that commit message was vacuous**. The build did not say
so: `cargo build --all-targets` was grepped for `^error` and `^warning: unused`,
and a never-called function warns as `warning: function ... is never used`,
which that pattern does not match. Same shape as `WORKING-ON-THIS.md` §5's
silent-harness rule, one level up: *a green build is not evidence that your code
runs — grep the whole warning stream, or make the call site the thing you
verify.* Wired and re-measured in the follow-up commit; the numbers below are
the WIRED ones.

**MEASURED byte-inert, with a POSITIVE CONTROL, and that is reported rather
than hidden.** On `diag 64x64 q40 p11` the prune abandons **146** candidates
across the frame's 16 leaves in VIDEO mode and **0** in still mode — the arm
split itself, on one build, which is what says the probe can see the feature at
all (`WORKING-ON-THIS.md` §5). And it changes NO cell: all six `ratioVideoKey` /
`fhVideoKey`-adjacent video cells and all six still identity cells are
byte-for-byte what they were, with and without the held arms. It earns no
`regression_spotcheck.sh` cell for exactly the reason §3 of
`docs/WORKING-ON-THIS.md` gives, and it is kept for the reason
`rust/CLAUDE.md`'s "DEAD-LOOKING C STAYS TRANSLATED" section gives: it is a
faithful translation of a live C rule whose effect is masked by the PD0 gap
above, and it will stop being inert the moment that gap closes.

**A correction to my own first reading of that cell, recorded because it is the
kind of premise that would send the next session sideways:** light PD1 is NOT
what C runs there. `pic_lpd1_lvl` (`enc_mode_config.c:9408-9432`) is
`is_base ? 0 : …` at every preset through M11 and `is_islice ? 0 : …` above it,
so a KEY frame takes **`pic_lpd1_lvl = 0` = REGULAR PD1 at every preset** —
`svt_aom_sig_deriv_enc_dec_default`, exactly the arm `encdec_arm` models. (That
also makes the `fast_loop_core_light_pd1` variance note above true but
irrelevant to these cells; it matters for inter frames.) So "the metric is
right and light PD1 explains the rest" is wrong: the metric is right and
something in the port's depth path is not.

**Two things this measurement RETIRES**, both recorded as leads in the previous
revision of this file:

* "the port's stage-count floor" as the `nic_arm` suspect.
  `leaf_funnel::rate_tables::nic_counts` is now gated at tier 1 against the
  real exported `svt_aom_set_nics` over every `MD_STAGE_NICS_SCAL_NUM` row and
  every CLI qp 0..=63, and it is CORRECT everywhere — including
  `(4,4,4)` at qp 40, which is the failing cell's own configuration and which
  neither of the two pre-existing gates covered.
* `enable_skipping_mds1` as that suspect. It is 1 only at `nic_level` 8..=11
  (`enc_mode_config.c` `set_nic_controls` cases 8-11); the failing cell is
  preset 5, where the video arm takes `nic_level` **7**, so the flag is 0 and
  cannot explain it.

### `wip/video-md-arms` — LANDED 2026-09-01 (kept below as the record)

The bookmark's content is on `main` as `43e38fdd`, together with the PD0 and
CDEF fixes §1i describes. It is a REWRITE, not a fast-forward: `59458226` was
rebased off the pre-landing `main`, so that hash is reachable only in the
operation log and this file's earlier references to it name a commit that is
no longer an ancestor of anything. The bookmark itself is deleted. The section below is the record of why it was held and
what was measured while it was; read §1i for what closed it. Two claims in it
are now wrong and are corrected there rather than edited away: "what holds the
pair off main now is `gradient 72x88 p4`, `p9` and the `diag p11` edge-filter
witness" missed a FOURTH cell (`video-key-txs-arm-tx-mode-p11`, which the
bundle broke and nobody had re-run against it), and the three named cells are
now byte-identical rather than merely inside their limits.


**Head is `59458226` as of 2026-09-01** (it supersedes `f898794f9`, which is
still reachable by hash; see §1g for what the new head adds and why the pair is
still held) — `nic_arm` VERBATIM from the previous
head `9d9b92526` (diff empty) plus `encdec_arm`, rebased onto main. Read §1e
first: with both arms the reference cell's mode decision matches C exactly, and
the cell named below is CLOSED (0.539% -> 0.000%); what holds the pair off main
now is `gradient 72x88 p4`, `p9` and the `diag p11` edge-filter witness.

The rest of this section is the `nic_arm`-only record it was written as.

`nic_level` (`nic_arm`) is done, tier 1 on both arms, and still-path
byte-neutral (identity_full_8bit 1100/1100). It is on the bookmark
`wip/video-md-arms`, not on `main`, because it is the one arm that pushes a
`ratioVideoKey` scoreboard cell back outside its limit:

| cell | C | with `nic_arm` | without | limit |
|---|---|---|---|---|
| `video-key-nsq-arm-p5-72x88` | 1485 B | 1493 B (0.539%) | 1486 B (0.067%) | 0.3% |

Re-deriving a limit is a threshold change and needs the owner's sign-off, so
the arm waits rather than the gate moving. ~~**The lead:** at `nic_level` 8 the port prunes HARDER than C ... the suspect
is the port's stage-count floor or the unmodelled `enable_skipping_mds1`.~~
**BOTH REFUTED 2026-09-01 — see §1e.** The stage counts are now tier-1 gated
against the real exported `svt_aom_set_nics` over every `MD_STAGE_NICS_SCAL_NUM`
row and every CLI qp 0..=63 and are CORRECT — including the failing cell's own
`(4, 4, 4)` at qp 40, which sat in a hole in BOTH pre-existing gates
(`c_parity_md_nics.rs` covers a different transcription with no live caller,
over a numerator grid that omits 4 and a qp list that omits 40). And
`enable_skipping_mds1` is 1 only at `nic_level` 8..=11, while the failing cell
is preset 5, where the video arm takes `nic_level` **7** — so the flag is 0
there and cannot explain it. The real co-requisite was
`mds0_use_hadamard_sb`; with it the cell below is at 0.000%.

The bisect that chose what landed: on the four ratio cells, over the on/off
combinations of (edge filter, `txs_arm`, `funnel_arm`, `nic_arm`), the maximal
all-green configuration is **edge + txs + funnel, nic off**. Edge alone and
edge+txs both leave `video-key-rate-arm-p9-72x88` at 1.196% against its 1.0
limit; adding `funnel_arm` brings it to 0.063%.

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

## 2b. Landed after C1a — the video-arm tool ladders (2026-08-31)

C1a closed the video qindex; the frame header then diverged at
`allow_intrabc`, because `sc_detect.rs` had only the ALLINTRA arm of
`enc_mode_config.c`'s picture-level tool derivations. `sc_detect::derive_sc`
now takes an `ScArm` and the pipeline resolves it from the same
`gop.intra_period <= 1` predicate C1a uses.

**Wired:** the intra-BC ladder (`sig_deriv_multi_processes_default`,
`enc_mode_config.c:2033-2052`, reached through the extracted
`port_enc_mode_config::multi_processes::intrabc_level_default` so the wiring
and the tier-1 parity test drive the same code) and the arm's scm gate
(`enc_handle.c:4638-4670`, allintra `<= M7` vs video `<= M8`).

**MEASURED** on `identity_diff_inter.sh 64 64 40 6 2 screen`, frame 0: the
first diverging FH field was `allow_intrabc` (C=1, port=0); after wiring,
**every frame-header field on that cell is identical** and the frame is
92 B (C) vs 138 B (port) — the divergence is now in the TILE payload, which is
the next chunk's target. Same on `... p8 ...`, where the first diverging field
was `allow_screen_content_tools`. Non-screen cells (gradient/diag/screenrep
p6) are byte-for-byte unchanged, and the still envelope is unmoved
(`identity_full_8bit.sh` 1100/1100, `regression_spotcheck.sh` 39/39).

**NOT wired:** the video **palette** ladder (`:2054-2072`) — ported at tier 1,
still un-called; `derive_sc` uses the allintra palette table on both arms. It
cannot move a frame-header bit (proved by unit test, see
`docs/ibc-port-map.md`), only the RD candidate set. That is the cheapest
remaining attributable step on the tile payload.

## 2c. Landed after 2b — the video-arm PARTITION ladders (2026-08-31)

With the header's tool bits closed, the remaining divergence on every cell sits
in the TILE payload, i.e. in the recon. Three more ladders were still being run
from the ALLINTRA arm on every frame, flattened into inline predicates in
`pipeline.rs`: `get_max_block_size_allintra` (`preset >= 8 && full_sb`),
`svt_aom_get_nsq_geom_level_allintra` (`preset <= 6`) and
`svt_aom_get_nsq_search_level_allintra` (`NsqCfg::for_preset_qp`'s base table).
These decide the partition SEARCH, so taking the wrong arm moves the coded tree.

**Wired** — `src/part_arm.rs` selects the arm from the `ScArm` chunk 2b already
threads through `encode_tile_rows`, and calls the tier-1-gated ladders in
`port_enc_mode_config::{common,leaf}` rather than a second transcription. Also
wired, because the video arm makes them live: `svt_aom_set_nsq_geom_ctrls`'s
`(allow_HV4, min_nsq_block_size)` pair (was hardcoded `(true, 0)`), and the
`set_nsq_search_ctrls` tail's qp-based scaling (`nsq_qp_based_th_scaling` is 0
through M3 on the allintra arm — the only band that reaches the tail there —
and 1 at every reachable preset on the video arm).

**MEASURED**, `identity_diff_inter.sh` frame 0, `gradient 72x88 q40` (the
partial-SB cell, the only shape where the geometry ladder is observable):
p4 1492 -> **1398** B against C's 1403; p5 1499 -> **1484** against 1485;
p7 1502 -> **1511** against 1539. On the 64-aligned `gradient 64x64 q40` cell
the geometry ladder cannot fire at all (no boundary node) and the sizes move
only where the SEARCH ladder does: p2 953 -> 959, p3 948 -> 966, p4 948 -> 967
against C's 974/975/951. Full table + the honest read of it in
`docs/nsq-port-map.md` §3.

**No still regression, measured:** `identity_full_8bit.sh` **1100/1100**, all
six reference identity cells byte-identical at their pinned sizes,
`regression_spotcheck.sh` **44/44** (41/44 with the three source files reverted
— the three new `ratioVideoKey` cells are exactly the ones that fail),
`cargo nextest run --workspace` 2387/2387.

**NOT wired**, all named in `docs/nsq-port-map.md` §4: NSQ geom level 1's
`allow_HVA_HVB` (reachable only at video preset 0; the funnel has no HVA/HVB
candidate), `pcs->mimic_only_tx_4x4`'s forced level 0 on a coded-lossless
frame, `nsq_search_ctrls.sub_depth_block_lvl`, the `PD_PASS_0` control-row
override, and the whole rtc arm.

**Next on the tile payload.** The first diverging frame-header field on the
reference cell (`gradient 64x64 q40 p6`) is unchanged by this chunk —
`cdef_uv_pri_strength[0]` C=7 port=0, C 961 B vs port 971 B — because at p6 on
a 64-aligned frame all three of these ladders agree between the arms. That
field is a CDEF SEARCH output, so it is downstream of the recon, and the recon
is downstream of the remaining unwired video ladders (the palette level from
2b, and whatever else `sig_deriv_mode_decision_config_default` sets that the
port still takes from the allintra arm).

## 2d. Landed after 2c — the video-arm RATE ladders (2026-09-01)

Chunk `wv-rdoq`. Full record: `docs/rate-arm-port-map.md`.

`pipeline.rs` ran the ALLINTRA arm of three ladders on every frame, flattened
inline: the preset clamp (`preset.min(9)`), `rdoq_level`
(`quant::rdoq_level_allintra`), the `set_rate_est_ctrls` row `FunnelCfg::
for_preset` bakes, and the per-SB CDF-chain gate (`matches!(preset, 0..=6)`).
New `svtav1_encoder::rate_arm` dispatches all of them on the `ScArm` chunk 2b
already threads. The three are wired TOGETHER because `set_cdf_controls`
couples them (`update_coef = rate_est_level || rdoq_level`, `:8479`) — the
chunk brief named `rdoq_level` and `update_cdf_level` only, and wiring
`update_cdf_level` without `rate_est_level` would run the per-SB chain at M7/M8
under a controls row C never pairs it with.

Where the arms bite: M6 up. Video is a flat rdoq 1 to M10 (2 above, under its
own `> M11 -> M11` clamp — allintra's is `> M9 -> M9`), a flat `rate_est_level`
1, and keeps CDF adaptation ON at M7/M8 where allintra turns it off entirely.
M4..M6 carry different update_cdf LEVELS (2 vs 1) but identical controls,
because `set_cdf_controls` forces `update_mv = 0` on an I_SLICE.

**Evidence tier 1 on BOTH arms.** The video arm was already gated
(`c_parity_sig_deriv_md_config.rs` drives the exported `_default` and reads
`pcs->rdoq_level` / `rate_est_level` / `cdf_ctrl` back). The allintra arm is
NEW: `svt_aom_sig_deriv_mode_decision_config_allintra` is exported too
(`nm -g` GLOBAL on both hosts), so a new `ref_sig_deriv_md_config_allintra`
shim drives it and reads the same six fields — upgrading
`quant::rdoq_level_allintra` from tier 4 to tier 1, mutation-verified.

**No still regression, measured:** `identity_full_8bit.sh` **1100/1100**, the
six reference identity cells IDENTICAL at their pinned sizes (290 / 839 / 63 /
171 / 580 / 693 B), `regression_spotcheck.sh` **45/45**,
`cargo nextest run --workspace` 2390/2390.

**One spot-check cell went vacuous and was replaced, not re-limited.**
`video-key-nsq-arm-p7-72x88` (chunk 2c's) now emits 1499 B whether the
partition arms are wired or forced to Allintra, so it can no longer witness its
fix at any limit. Replaced by `screenrep 72x88 q40 p7` — 2414 B vs 2386 B
against C's 2388 — at a tighter 0.5% limit.

**Not uniformly closer, and that is the honest reading.** gradient 72x88 q40
frame 0: p9 1630 -> 1587 B (C 1589, 2.580% -> 0.126% off) and p10 1630 -> 1587
(C 1599); p7 1511 -> 1499 (C 1539) and p11..13 1630 -> 1592 (C 1634) move
further. Presets 0..=5 do not move at all. Only 3 of the ~30 picture-level
ladders `sig_deriv_mode_decision_config_*` assigns are on the video arm now, so
a video frame is a hybrid and its size wanders; read the first-diverging
frame-header field, not the byte count.

**First diverging frame-header field, reference cell `gradient 64x64 q40 p6`:
unchanged — `cdef_uv_pri_strength[0]` C=7 port=0**, now C 961 B vs port 947 B
(was 971). It is a CDEF SEARCH output, downstream of the recon. What DID move:
`gradient 64x64 q40 p8` advanced from `lr_type[0]` to `cdef_uv_pri_strength[0]`,
and `screenrep 128x128 q35 p7` advanced past `cdef_y_pri_strength[0]` (which
now matches C) to `cdef_uv_pri_strength[0]`, with its differing-field count
going 3 -> 1.

**Next.** The remaining ~16 unwired picture-level ladders, in the order they
touch the recon: `txt_level` (allintra 10 at M7/M8 vs video 7 for a base
I-slice), `nic_level`, `txs_level`, `intra_level` /
`dist_based_ang_intra_level`, `chroma_level` / `cfl_level`,
`spatial_sse_full_loop_level`, `pic_bypass_encdec`, `pic_disallow_4x4`,
`pd0_cost_bias_weight`, ~~`mds0_level`~~ (WIRED 2026-09-01, `mds0_arm` — see
§1f), `tx_shortcut_level`, `pic_depth_removal_level`,
`pic_block_based_depth_refinement_level`, `lambda_weight`, `pic_pd0_lvl` (the
one §1f names as the blocker for all three held-arm cells). Every one has a tier-1-ported `_default` twin
in `port_enc_mode_config::md_config` already — this is wiring, not porting, and
`rate_arm` / `part_arm` are the pattern.

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

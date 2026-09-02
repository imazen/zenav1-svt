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
predicts from source on both arms, and C's video arm does not. The reference
cell's residual 0.416 % is NOT in PD0 — its 75 PD0 blocks are exact — it is
downstream of the partition decision.

**CORRECTED 2026-09-01 by §1j, and the correction is the useful part.** The
sentence above attributed `diag p11`'s 0.499 % to that unwired
`pd0_use_src_samples`. It was wrong, and the naive repair (wiring it at preset
>= 9) had already been measured and rejected — no movement on p4/p5/p7, p9 worse
(0.189 -> 0.378) — which should have retired the reading rather than leaving it
in place. `diag p11`'s tree, every leaf field, every LUMA level and the
pre-deblock recon of ALL THREE PLANES were byte-identical to C; the two bytes
were a `tx_size` symbol the port wrote after its own frame header had signalled
TX_MODE_LARGEST. `pd0_use_src_samples` at preset >= 9 remains genuinely unwired
and is now a lead with NO cell attached to it.

**AND ITS REJECTION IS STALE — see §1k.** "Wiring it at preset >= 9 was measured
and rejected (p9 0.189 -> 0.378)" was measured over the LIGHT-PD0 boundary-shape
defect §1k fixes, the same way §1h's four variants were measured over the PD0
coefficient-rate defect. Re-run it on the current head before believing either
verdict.

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

### 1j. BOTH named residuals CLOSED (2026-09-01) — the RDOQ plane rate weight, and a tx_size symbol the header forbade

`gradient 64x64 q40 p6` — the reference cell, open since the campaign began —
and `diag 64x64 q40 p11` are **byte-identical video-mode KEY frames**. So are
`gradient 64x64 q40 p11`, which was the weaker `fhVideoKey` cell. That is SIX
byte-identical cells on non-degenerate content across five presets (4, 5, 6, 7,
11), plus the three `screen` cells and `uniform`.

Neither residual was where §1i pointed, and both were found the same way: build
the observation, then read it.

**Defect 1 — the RDOQ rate weight ran the ALLINTRA row on every frame.**
`svt_av1_optimize_b` computes `rdmult = ((lambda *
plane_rd_mult[allintra || rtc][is_inter][plane_type]) * rweight / 100 + 2) >>
rshift` (full_loop.c:1085). The table (`:994`, the MAINLINE `#else` —
`TUNE_CHROMA_SSIM` is 0 outside `SVT_HDR_MODE`) is

| first index | intra {luma, chroma} | inter {luma, chroma} |
|---|---|---|
| 0 — neither allintra nor rtc (**video**) | {17, **20**} | {16, **20**} |
| 1 — allintra or rtc | {17, 13} | {16, 10} |

and `crate::quant::rdoq_rdmult_full` hardcoded `if plane_type == 0 { 17 } else
{ 13 }` — index 1, at every preset, on every frame. **Luma is 17 on both arms**,
which is why this presented as a chroma-only divergence: every luma-side probe
this campaign has run would agree, and did. A larger multiplier weights RATE
more, so C's video-arm RDOQ zeroes chroma coefficients the port keeps.

*How it was localized.* On the reference cell the coded tree, every leaf mode,
uv mode and angle delta already equalled C's (§1e), and `tools/fh_fields.py`
reported **zero** differing frame-header fields, so the 4 B were in the tile
payload. The C `svt_aom_txb_estimate_coeff_bits` `--wrap` interposer
(`SVT_CCOEF_OUT`) was widened so an UNSET `SVT_CCOEF_XY` dumps EVERY coded txb
instead of one pinned block — the pinned mode cannot answer "which block
diverges" without already knowing — and joined against the port's
`SVTAV1_PACKTREE_COEFF` `PCOEF` dump:

| block | C | port |
|---|---|---|
| (0,0) 32x32 | `cbeob=0 creob=0` | `unz=[0:-1,1:-1,16:1]` |
| (32,0) | `unz=[0:-1]` | `unz=[]` |
| (0,32) | `unz=[0:-1]` | `unz=[0:-1,1:-1]` |
| (32,32) | `unz=[0:-1]` | `unz=[16:1]` |

Every LUMA level, `yeob` and `txt` matched to the entry on all four blocks; the
`txtuv` C reports is exactly `intra_mode_to_tx_type` of the port's own uv modes.
Prediction was ruled out before quantization was suspected: C's pre-deblock
recon chroma (`SVT_RECON_BIN`, and block (0,0) has `cbeob = 0` so its recon IS
its prediction) matches the port's wherever the port's residual is zero.

The table is now `crate::quant::PLANE_RD_MULT` + `plane_rd_mult()`, selected by
`allintra_rd_mult` = C's `scs->allintra || scs->static_config.rtc`, carried on
`CodingQuantCfg` and `FunnelFrame` and threaded to `tx_unit_hbd` so the bd10
re-encode cannot disagree with the 8-bit funnel. `is_inter` is a real axis of
C's table and is ported, but every call site passes `false`: the pipeline
refuses inter frames, so those rows are unreachable and say so.

**One thing this deliberately does NOT do, recorded rather than half-done.**
The fork has its own twin of the table (`full_loop.c:985`, the
`#if TUNE_CHROMA_SSIM` arm, which `SVT_HDR_MODE` defines to 1) whose two arm
rows are BOTH the allintra numbers — so in fork mode the arm index is inert and
chroma stays at 13/10. `quant::PLANE_RD_MULT` is the MAINLINE table only, and
the RDOQ path in the funnel (`FunnelFrame`) carries no `hdr_fork` flag today
(only its derived `sharp_tx_active` / `noise_norm_strength` / `qm_levels`),
where `CodingQuantCfg` does. So a FORK-mode VIDEO frame would take mainline's
chroma 20. That combination has no gate and no cell in this repo; threading the
flag through 14 `tx_unit_hbd` call sites to fix an unmeasured path is not worth
doing blind, so it is written down here instead of guessed at. Whoever picks it
up: `CodingQuantCfg::hdr_fork` is already the right input for the encode-pass
site; only the funnel needs the new field.

**Defect 2 — the walk wrote a `tx_size` symbol the frame header had forbidden,
and that is a CONFORMANCE bug.** §2d's chunk fixed the HEADER half of
`frm_hdr->tx_mode` (the video arm signals `TX_MODE_LARGEST` from preset 10 up,
where the allintra arm signals `TX_MODE_SELECT` unconditionally). The WALK half
was not fixed: `encode_block_syntax` gated the per-block `tx_size_cdf` symbol on
`is_key` — the allintra rule again — so at video preset >= 10 the port announced
TX_MODE_LARGEST and then coded one `tx_depth` symbol per block anyway. A decoder
reading that stream is desynchronised from the first block.

*How it was localized.* Everything decodable already agreed: `tree_diff.py` on
`diag 64x64 q40 p11` reported **22 blocks joined, 0 field flips, 0 C-only / 0
port-only geometry**; C's luma `SVT_QLEVELS_OUT` dump matched the port's `PCOEF`
on `eob`, `txt` and every level for all 22; and the pre-deblock recon planes were
BYTE-IDENTICAL on all three planes. That leaves only the entropy layer, so the
op-trace differ was the right tool — and on macOS it only exists in
`tools/ctrace-linux/` (`ld64` has no `--wrap`, §5). Its `vdiff_cell.sh` +
`optrace_first_diff.py` (both landed with this chunk) put the first divergence
at **op 7, inside the FIRST coded block**: the port emits
`CDF nsyms=2 s=0 icdf=[12800]`, which is `TX_SIZE_CDF[0][0]`, and C emits
nothing.

`EntropyCtx` now carries `tx_mode_select` — the same bit the header writes, from
one helper (`EncodePipeline::frame_tx_mode_select`) — beside `seq_filter_intra`
and `allow_sct`, the two frame-level syntax gates it already held. The funnel
walk and the per-SB CDF-chain simulation read it too, or the chain evolves
different CDFs from the pack.

**Per-cell, frame 0, % off C's byte count.** The "before" column is a real A/B
on ONE build with both of this chunk's fixes forced back to `main`'s behaviour,
not a value copied from an earlier section:

| cell | before | after |
|---|--:|--:|
| `gradient 64x64 q40 p6` (the reference cell) | 0.416 | **0.000, BYTE-IDENTICAL** |
| `diag 64x64 q40 p11` | 0.499 | **0.000, BYTE-IDENTICAL** |
| `gradient 64x64 q40 p11` | 0.195 (1026 B vs C 1024) | **0.000, BYTE-IDENTICAL** |
| `gradient 72x88 q40 p4` | 0.000 | 0.000, byte-identical |
| `gradient 72x88 q40 p5` | 0.000 | 0.000, byte-identical |
| `screenrep 72x88 q40 p7` | 0.000 | 0.000, byte-identical |
| `screen 64x64 q20/q40/q55 p6` | 0.000 | 0.000, byte-identical |
| `gradient 72x88 q40 p9` | 0.000 (SIZE only) | 0.189 (1586 B vs C 1589) |

The p9 row is the honest one. §1i recorded that its `0.000 %` was a SIZE
coincidence and not byte-identity (`C=1589B port=1589B`, different bytes); the
chroma rate weight moves it to 1586 B, so the coincidence is gone and the ratio
now reads 0.189 % against its 1.0 limit. It is the last open video-key cell in
the scoreboard, and it is now LOCALIZED rather than merely open:

```
tools/ctrace-linux/vdiff_cell.sh 72 88 40 9 gradient
```

puts its first diverging op at index **3269 of 10219**, and it is a PARTITION
symbol — `CDF n=10` on identical `icdf` and identical `rng`, C coding **s=0**
(PARTITION_NONE) where the port codes **s=3** (SPLIT). Everything before it
agrees to the op, so the first ~3200 ops of the frame are already exact.
`SVTAV1_TRACEMARK=1` puts that op just after the leaf at `mi=(4,12)`.

`tree_diff.py` on the same cell then says what the shape of the remaining work
is, and it is a CHUNK rather than a one-liner: **44 blocks joined, 0 C-only / 7
port-only geometry, 5 `bsize` flips, 7 mode and 7 uv flips.** Every `bsize` flip
is C coding `BLOCK_8X16` where the port codes `BLOCK_8X8` plus a split, and all
five sit at **`mi_col` 16 or 0 of the partial column** — x = 64 on a 72-wide
frame, i.e. the RIGHT-EDGE superblock, whose width is 8:

```
FLIP mi=(0,16) bsize C=4 port=3        FLIP mi=(8,16)  bsize C=4 port=3
FLIP mi=(8,0)  bsize C=6 port=3        FLIP mi=(12,16) bsize C=4 port=3
                                        FLIP mi=(16,16) bsize C=4 port=3
port-only mi: (2,16), (8,2), (10,0)
```

So p9 is an EDGE partition divergence on the fixed-tree PD0 path
(`pd0_pick_sb_partition_video`, PD0_LVL_5 at this qp band per §1g), not a rate,
metric or entropy defect.

**§1k took that chunk and the hypothesis below held**: p9 is now 0.126 % (1587 B
against C's 1589) with its coded tree at 9 field flips instead of 19. Read §1k
for what remains.

Two steps of that chunk are already done, so the next session starts from a
NAMED hypothesis rather than a search.

*Ruled out.* `pd0.rs`'s `forced_split = both_false || (one_false &&
!nsq_enabled)` is the C rule that makes a thin right edge descend to all-8x8
when NSQ geometry is off, and the video arm never turns NSQ geometry off — the
call site already passes `part_arm::nsq_geom_enabled(sc_arm, preset)`. So the
node is NOT force-split; the edge path is reached.

*The hypothesis.* Inside that path the edge SHAPE is priced only for the LVL_1
family: `if one_false && self.is_lvl1_family()` picks the fitting
`lvl1_block_cost_rect(half, sq_size)`, and the `else` costs the full SQUARE.
Its own comment says why that matters — "The square block would over-cost
(twice the pixels/coeffs) and wrongly lose to SPLIT" — and then says
"LVL_5/6 boundary nodes keep the square cost". That restriction was safe on the
ALLINTRA arm, where `nsq_geom_level` is 0 above M6 so an LVL_5/6 boundary node
force-splits before it can be costed at all. On the VIDEO arm NSQ geometry is
never off, so at p9 a one-false node reaches the `else` and is priced as a
square that cannot fit — which is exactly "the port splits to 8x8 where C codes
8x16". The same `is_lvl1_family()` guard sits on the boundary SPLIT rate a few
lines below.

C says the guard is wrong, and says it in two places that carry NO pd0-level
condition:

* `set_blocks_to_test` (enc_dec_process.c:1394) zeroes `tot_shapes` on an
  incomplete node only when NSQ geom is DISABLED or the square is at/below
  `MAX(min_nsq, min_nsq_block_size)`; otherwise its loop injects exactly the
  fitting shape — `has_cols` keeps PART_H, `has_rows` keeps PART_V
  (`:1420-1423`). `min_nsq` is 4 for `PD_PASS_0`.
* `svt_aom_pick_partition_pd0` (product_coding_loop.c:10534-10544) re-checks
  the same thing and then costs `get_blk_geom_mds(mds_idx +
  ns_blk_offset_md[shape])` — the RECTANGLE, not the square.

Neither reads `pd0_ctrls.pd0_level`. (`mode_decision_update_neighbor_arrays_pd0`
at `:126` says the same in prose — "either PART_N or PART_H/PART_V for boundary
blocks where PART_N is invalid" — but do not lean on its "LPD0" wording: that
function runs at every level, so the word is not a level claim.)

**Still not measured.** This is a reading of C plus a reading of the port's own
comment, not an A/B, and neither C's LVL_5 boundary shape nor its cost has been
dumped on the cell. Do that before changing the guard — the last three sessions
each refuted a premise that read exactly this convincing.

**Two tools landed with this chunk**, because both were built here and neither
belongs in scratch: `tools/ctrace-linux/vdiff_cell.sh` (the VIDEO-mode sibling
of `diff_cell.sh` — the still one cannot express the GOP, and the port's frame-1
refusal is expected rather than a failure) and
`tools/ctrace-linux/optrace_first_diff.py`. The second exists because
`identity_diff.py`'s op INDEX is wrong on a video cell while its byte verdict is
right; see `docs/WORKING-ON-THIS.md` §5 for the two normalizations it does and
the positive control it passes.

**Two cells PROMOTED, one added** (`tools/regression_spotcheck.sh`):
`video-key-edge-filter-diag-p11` ratioVideoKey -> byteVideoKey,
`video-key-txs-arm-tx-mode-p11` fhVideoKey -> byteVideoKey, and a new
`video-key-rdoq-plane-rd-mult-p6-64x64` (gradient 64x64 q40 p6: 965 B -> 961 B,
byte-identical). Each was measured BOTH ways on one build by forcing its fix off.

**A method note worth more than either fix, and the next sweep it names.** Both
defects are the same shape as `enable_intra_edge_filter` (§1c) and
`mds0_use_hadamard_sb` (§1e): an ALLINTRA constant baked into a place that runs
on every frame, OUTSIDE `sig_deriv_mode_decision_config` and therefore invisible
to the 52-slot arm diff §1c is built on. That table is necessary and not
sufficient, and this is the third time it has said so.

The bounded list of places left to check is the DIRECT reads —
`grep -rn 'scs->allintra\|static_config\.rtc' Source/Lib/Codec/` — 30-odd hits
outside `enc_mode_config.c`, of which `full_loop.c:1045-1046` was one. Two were
opened while writing this, and only one matters:

* **`enc_cdef.c:913` is LIVE and writes the frame header directly.** Inside
  `cdef_search_ctrls->use_qp_strength`, C picks the screen-content class as
  `allintra ? sc_class5 : sc_class1` and feeds it to `svt_pick_cdef_from_qp`,
  which sets `cdef_y_strength[0]` / `cdef_uv_strength[0]` and returns before any
  search. Not reached on the cells above (their CDEF search runs), so it is a
  LEAD, not a measurement.
* `coding_loop.c:1603` is `tot_intra_coded_area` / `tot_hp_coded_area`
  bookkeeping — byte-inert for a key frame's own bytes.

The rest (`enc_dec_process.c:2951/3040-3047/3097`, `md_config_process.c:899/924`,
`md_process.c:220`, `rest_process.c:76`, `pic_analysis_process.c:415`,
`rc_crf_cqp.c:396`, `pcs.c:386`) are UNAUDITED. Read them before guessing at
another ladder.

### 1k. The LIGHT-PD0 boundary SHAPE (2026-09-01) — and the p9 rejection that was measured over it

`pd0.rs` priced a one-false BOUNDARY node — a square that hangs off the right or
bottom edge — as its fitting `PART_H` / `PART_V` rectangle **only for the LVL_1
family**. LVL_5, the light PD0 the fixed-tree path runs at preset >= 9, got the
SQUARE cost: twice the pixels that actually fit, so it lost to SPLIT and the
port coded `BLOCK_8X8` + a split where C codes `BLOCK_8X16`.

It could not matter on the ALLINTRA arm, which is why it survived: there
`nsq_geom_level` is 0 above M6, so an LVL_5/6 boundary node force-splits before
it is costed at all. The video arm never turns NSQ geometry off.

**C prices the rectangle — measured, not inferred.** `SVT_PD0COST_OUT` (the
`svt_aom_full_cost_pd0` `--wrap`) on `gradient 72x88 q40 p9` video, the x = 64
superblock of a 72-wide frame:

```
PD0COST org=(64,0)  32x64 dist=1414884 ybits=103700 cost=184867517
PD0COST org=(64,0)  16x32 dist=52988   ybits=51000  cost=8640630
PD0COST org=(64,0)  8x16  dist=17922   ybits=16500  cost=2905600
PD0COST org=(64,0)  8x8   dist=10736   ybits=11000  cost=1787062
PD0COST org=(64,8)  8x8   dist=10407   ybits=9300   cost=1683524
```

Every block C tests there is a RECTANGLE; there is no square in the column. And
the 8x16's 2,905,600 beats the two 8x8s' 1,787,062 + 1,683,524 plus the split
rate, which is exactly the leaf C codes. The two C functions that decide this
carry no `pd0_level` term — `set_blocks_to_test` (enc_dec_process.c:1394,
`:1420-1423`) injects the fitting shape whenever NSQ geom is on, and
`svt_aom_pick_partition_pd0` (product_coding_loop.c:10534-10560) costs
`get_blk_geom_mds(mds_idx + ns_blk_offset_md[shape])`.

**What landed.** `lvl5_like_block_cost_rect` (the light-PD0 twin of the LVL_1
family's `lvl1_block_cost_rect` — the DC predictor, residual gather,
`tx_quant_core` and closed-form coeff rate were all dimension-general already),
selected by a new `Pd0Ctx::prices_edge_shape()`, plus the BINARY
split-vs-{H,V} rate at such a node (`svt_aom_partition_rate_cost`,
rd_cost.c:1846-1863) which LVL_5 was paying at the full-alphabet rate.

**Measured, both ways on one build.** Coded trees against C's `SVT_CTREE_OUT`
via `tools/tree_diff.py`, 44 blocks joined on both cells:

| cell | field flips OFF -> ON | port-only geometry OFF -> ON |
|---|---|---|
| `gradient 72x88 q40 p9` | 19 -> **9** | 7 -> **3** |
| `gradient 72x88 q40 p11` | 9 -> **1** (one `bsize`) | 7 -> **3** |

Every one of the four right-edge `bsize` flips at `mi_col` 16 is gone, and at
p11 every comparable field except one `bsize` now equals C's.

| cell | before | after |
|---|--:|--:|
| `screenrep 72x88 q40 p9` | 0.749 | **0.125** |
| `screenrep 72x88 q40 p11` | 0.827 | **0.165** |
| `gradient 72x88 q40 p9` | 0.189 | **0.126** |
| `gradient 72x88 q40 p10` | 0.125 | **0.063** |
| `gradient 72x88 q40 p11 / p12 / p13` | 1.040 / 1.102 / 1.102 | 1.285 / 1.346 / 1.346 |
| `diag 72x88 q40 p9` and `p11` | byte-identical | byte-identical |

The last two rows are the honest ones. p11..p13 move FURTHER from C in bytes
while their tree moves from 9 flips / 7 port-only blocks to 1 / 3 — the §1f
pattern exactly, a worse tree that landed nearer in size. The witness cell is
therefore `screenrep 72x88 q40 p9` at a 0.5 limit (between 0.749 and 0.125), not
p11. `diag 72x88 q40 p9` / `p11` are byte-identical BOTH ways and so witness
nothing; they are recorded because a cell that cannot fail is worse than none.

**A dump that did not exist.** `SVTAV1_PD0DBG` emitted `PD0BLK` lines only from
the LVL_1 family, so the video arm's PD0 at preset >= 9 — the path the whole
fixed-tree branch runs — had no port-side counterpart to C's `SVT_PD0COST_OUT`.
It does now, same fields in the same order. The first join it makes possible is
already interesting on `gradient 72x88 q40 p9`:

* **135 blocks on each side, the SAME SET** — 0 C-only, 0 port-only. PD0 tests
  exactly the blocks C tests, including every edge rectangle.
* **101 of the 135 COSTS differ**, and in two distinguishable shapes: small
  `dist` deltas on 8x8/16x16 blocks (e.g. C 3014 vs port 2858 at `(0,8)`), and
  large ones at 32x32/64x64 where C's `ybits` is 5000 — the closed form's
  eob-0 floor — against the port's 55,800-107,100.

**The next chunk, and the correction that unblocks it.** §1i recorded that
`pd0_use_src_samples` is unwired on the fixed-tree path at preset >= 9 (C's
video PD0 predicts each block from the RECON it generates; the port predicts
from SOURCE), and that wiring it there had been MEASURED AND REJECTED — no
movement on p4/p5/p7 and p9 worse, 0.189 -> 0.378. **That rejection was
measured over the square-cost defect this section fixes**, exactly as §1h's four
variants were measured over the PD0 coefficient-rate defect. It has to be
re-run, and the small per-block `dist` deltas above are what it predicts.

**§1l re-ran it. It closes six cells** — the rejection was an artifact of the
premise, and the prediction above was right about which deltas it collapses.

Not touched, with reasons rather than silence: **LVL_6** has no block cost to
make rectangular (`compute_lpd0_cost_allintra` / `_inter` run no transform), and
**LVL_0** is the bd10-forced path whose partial-SB cells are byte-identical
today with nothing here having dumped C's bd10 boundary cost — widening it blind
trades a green gate for a guess.

### 1l. `pd0_use_src_samples` at preset >= 9 — a REJECTED experiment, re-run over a fixed premise, closes every open video-key scoreboard cell

`gradient 72x88 q40` and `screenrep 72x88 q40` at presets **9, 10 and 11** — six
cells that were all still off — are byte-identical video-mode KEY frames. Every
`ratioVideoKey` video-key cell in the scoreboard is now a `byteVideoKey`,
including `video-key-rate-arm-p9-72x88`, the last one.

Measured as a 45-cell matrix (`72x88 q40`, five content classes x nine presets)
on ONE build each side: **28 -> 34 byte-identical, six closed, nothing worse,
and every cell this chunk cannot reach unchanged to the byte.**

**ONE weaker video-key cell survives and it is not this chunk's**:
`fhVideoKey "video-key-ibc-arm-p8" screen 64 64 40 8` asserts the frame HEADER
only, and its tile payload is not close — MEASURED here, `screen 64x64 q40 p8`
video is **114 B in C against the port's 568**, 398 % off. Its 72x88 sibling is
409 %. Those two are by a wide margin the worst video-key cells left, they are
the SAME content class (`screen`, the only one that arms the screen-content
detector) at the SAME preset, and neither is touched by anything in §1j-§1l.
`screenrep 64x64 q40 p8` is byte-identical, so it is not "screen content at p8"
in general — it is this cell's tools.

**This experiment had already been run and REJECTED.** §1i recorded it: wiring
`ctx->pd0_use_src_samples = false` (the video arm's value — PD0 predicts each
block from the RECON it generates, `product_coding_loop.c:8430`, not from the
source) on the fixed-tree path at preset >= 9 moved nothing on p4/p5/p7 and made
p9 WORSE, 0.189 % -> 0.378 %. That verdict was correct about what it measured
and wrong about what it meant: it was measured over the LIGHT-PD0
boundary-shape defect §1k fixes, so the recon prediction was being fed into a
partition search that was still splitting every edge node. §1k said the
rejection was stale and had to be re-run. Re-run, it closes six cells at once.

That is the same lesson as §1h's four PD0 variants and §1i's three defects, for
the third time: **a negative result is only as good as the premise underneath
it.** When a chunk fixes something upstream, re-run the experiments that were
rejected over it rather than treating the old verdict as settled.

**What landed.** `lvl5_like_block_cost_rect` gained the two halves the LVL_1
family has had since §1i:
* neighbours come from `Pd0ReconCanvas` when there is one, through the same
  `extract_neighbors_tiled` with the same window shift — and from the source
  through the untiled extractor when there is not, which keeps the ALLINTRA arm
  byte-identical by construction;
* the block's recon is generated (inverse transform into the even rows at the
  subres stride, each even row copied onto the odd row below, a straight copy of
  the prediction when `eob == 0`) and handed to `pending_recon`, which
  `pick_q`'s existing writes push into the canvas at C's decision points.

`pd0_pick_sb_partition_video` now takes the canvas, from the SAME
`pd0_video_recon.then_some((&tile_frame_recon[..], w))` the refinement path at
preset <= 8 was already being given — three lines at the call site, because the
plumbing existed and only this entry point was passing `None`.

**Every cell that MOVED, frame 0, % off C's byte count.** The "before" column is
this repo's state one commit earlier — §1k's boundary-shape fix already in — and
both columns come from one build each, not from earlier sections:

| cell | before (§1k) | after |
|---|--:|--:|
| `gradient 72x88 q40 p9` | 0.126 | **0.000, BYTE-IDENTICAL** |
| `gradient 72x88 q40 p10` | 0.063 | **0.000, BYTE-IDENTICAL** |
| `gradient 72x88 q40 p11` | 1.285 | **0.000, BYTE-IDENTICAL** |
| `screenrep 72x88 q40 p9` | 0.125 | **0.000, BYTE-IDENTICAL** |
| `screenrep 72x88 q40 p10` | 0.125 | **0.000, BYTE-IDENTICAL** |
| `screenrep 72x88 q40 p11` | 0.165 | **0.000, BYTE-IDENTICAL** |
| `gradient 72x88 q40 p12` / `p13` | 1.346 | 0.061 (ONE byte) |
| `screenrep 72x88 q40 p12` / `p13` | 0.124 | 0.041 (ONE byte) |

**The other 37 cells of the matrix are byte-for-byte unchanged**, including
every preset 0..8 cell and every already-identical one. Nothing regressed.

`gradient 72x88 p11` is the row to read twice: §1k moved it FURTHER from C in
bytes (1.040 -> 1.285) while its tree went from 9 field flips to 1, and that
reading — "a worse tree that landed nearer in size" — is what this chunk
confirms. The remaining flip was the wrong prediction source.

**Where the envelope ends now, from the same matrix.** Everything still open at
`72x88 q40` in video mode:

| preset | cells still off |
|---|---|
| 0 | `gradient` 0.447 %, `diag` 0.483 %, `screenrep` 0.043 % |
| 3 | `gradient` 1.628 %, `diag` **22.257 %** |
| 8 | `gradient` 1.673 %, `screen` **408.939 %** (179 B vs the port's 911) |
| 12 / 13 | `gradient` / `diag` / `screen` / `screenrep` / `uniform`, ONE byte each |

Presets 0..8 are the LVL_1-family REFINEMENT path, which this chunk does not
touch, and they are the next frontier. Two of those cells are far louder than
anything the campaign has been chasing — `diag p3` at 22 % and `screen p8` at
409 %, both **pre-existing and unchanged by this chunk** (verified on both sides
of the A/B, not assumed). `screen` is the content that arms the screen-content
detector, so p8 is a palette/IntraBC-shaped lead, not a partition one.

Presets 12/13 are a different and much smaller shape: FIVE content classes, all
exactly one byte short. A single shared cause is likely and none of it is
partition — the trees are not being compared there yet.

**A cost this inherits, recorded rather than discovered later.** §1i's note on
`Pd0ReconCanvas::new` — `stride * 66` bytes allocated and filled per PD0 entry
call — now applies at preset >= 9 as well, on the VIDEO path only (the allintra
arm still passes `None` and carries no canvas). The fixed-tree path calls that
entry ONCE per superblock where the refinement path calls it several times, so
it is the cheaper of the two, but the same narrowing applies: seed only the row
above and the column left, which is all `extract_neighbors_tiled` can read. That
is an optimisation to make against a measurement, not while closing cells.

### 1m. The VIDEO arm's PALETTE ladder (2026-09-01) — `screen p8`, the campaign's worst cell, closes to the byte

`screen 72x88 q40 p8` was **408.939 %** off (C 179 B against the port's 911) and
its 64x64 sibling about the same (C 114 B; §1l recorded the port at 568 and the
spot-check cell's own comment at 697 — neither was re-measured here, only the
AFTER). Both are byte-identical now, and
so is `screen 72x88 q40 p7`, which §1l's table did not list at all — **a
correction to that table: it recorded presets 0, 3, 8 and 12/13 as the open
`72x88` rows and missed `screen p7` at 13.095 %.** The two failing cells were one
cause, and p7 is the gentler witness of it.

**What it was — the fourth instance of this campaign's one shape, found by
reading the port's own PORT-NOTE rather than by tracing.** `sc_detect::derive_sc`
ran C's ALLINTRA palette ladder (`enc_mode_config.c:2374-2390`) on BOTH arms. The
note above the binding said so, deliberately, and argued the cost was bounded:
"What it provably does not cost: `allow_screen_content_tools`, the only
frame-header bit palette_level feeds." That argument is CORRECT and it is beside
the point — `palette_level` is what MODE DECISION searches with, so it moves the
TILE. The `fhVideoKey` cell on that very cell passed throughout, because the
header bit was right; the payload was 5x.

The two ladders and where they part:

| preset | allintra `:2374-2390` | video `:2056-2075` |
|---|--:|--:|
| M0 | 2 | **1** |
| M1 | 2 | 2 |
| M2 | 2 | **4** |
| M3 | 3 | **5** |
| M4 / M5 | 4 | **5** |
| M6 | 5 | **6** |
| M7 | 7 | **6** |
| M8 / M9 | **0** | **6** |
| M10 | 0 | **8** |
| M11+ | 0 | 0 |

M8 is the cliff: the port coded a screen-content video key frame with palette
switched OFF entirely. M7 is the same fault a rung lower — level 7 is level 6
with `k_means_max_itr` 1 instead of 2 — which is why it showed as 13 % rather
than 409 %.

**A second defect underneath it, and it would have silently swallowed the fix.**
`PaletteCtrls::for_level` carried only the ALLINTRA-reachable rows
{0, 2, 3, 4, 5, 7} with `_ => PaletteCtrls::default()`, i.e. `enabled: false`.
A correctly derived level 6 would have fallen through that arm and disabled
palette anyway, and nothing would have said so. All nine `set_palette_level`
rows are transcribed now, including the RTC-only 9.

**Level 1 needed a function that did not exist.** Video M0 asks for level 1, the
only row with `centroid_refinement = 1`, and `cache_based_centroid_refinement`
(palette.c:330-386) was unported — reasonably, since no allintra level reaches
it. It is ported literally, including that `baseline_sse` is computed once and
never refreshed as earlier centroids move. **Positive control** rather than an
inference from byte-identity: a probe counting entries reports **74 calls** on
`screen 72x88 q40 p0` and **0** at p3 / p7 / p8, which is exactly what the
ladder says (levels 5/6/7 carry `centroid_refinement = 0`).

State it honestly: `screen p0` was byte-identical BEFORE this chunk under the
wrong level (allintra 2, no refinement) and is byte-identical AFTER under the
right one (video 1, 74 refinement calls). So the refinement is REACHED and
byte-neutral on that cell; it is **not** independently validated against C, and
it cannot be at tier 1 — the C function is `static`. Tier 4, and the cell that
would witness it is still wanted.

**Per-cell, `screen`, video frame 0, before -> after:**

| cell | before | after |
|---|--:|--:|
| `screen 72x88 q40 p7` | 13.095 % (C 168 B, port 190) | **0.000, BYTE-IDENTICAL** |
| `screen 72x88 q40 p8` | 408.939 % (C 179 B, port 911) | **0.000, BYTE-IDENTICAL** |
| `screen 64x64 q40 p8` | ~5x (C 114 B; §1l recorded port 568, the cell's own comment 697 — neither re-measured here) | **0.000, BYTE-IDENTICAL** |
| `screen 72x88 q40 p0..p6, p9..p11` | byte-identical | byte-identical, same bytes |

`video-key-ibc-arm-p8` is PROMOTED from `fhVideoKey` to `byteVideoKey`; it had
carried the note "promote it when the payload closes" since 2026-08-31.

The other 48 cells of the 60-cell matrix (`72x88 q40`, five content classes x
twelve presets) are byte-for-byte unchanged.

### 1n. C's per-arm PRESET CLAMP, reaching the deblock ladder (2026-09-01) — the p12/p13 one-byte row, five content classes, one cause

Ten cells — `gradient` / `diag` / `screen` / `screenrep` / `uniform` at 72x88
q40, presets **12 and 13** — were each exactly ONE byte short of C. §1l guessed
"a single shared cause is likely and none of it is partition". Both halves were
right.

**It is not a ladder row; it is which `enc_mode` the ladders are READ AT.**
`svt_av1_enc_set_parameter` rewrites `scs->static_config.enc_mode` ONCE
(`enc_handle.c:4415-4436`) — allintra `> ENC_M9 -> ENC_M9`, RTC
`> ENC_M13 -> ENC_M13`, video non-RTC `> ENC_M11 -> ENC_M11` — so every
downstream derivation sees the CLAMPED value and CLI presets 12 and 13 are
M11 in video mode. `rate_arm::eff_enc_mode` models exactly this and its doc
says "applied once ... so EVERY downstream ladder sees the clamped
`enc_mode`" — but `pipeline.rs`'s `dlf_enc_mode` was bound from the RAW
`speed_config.preset`.

At 12/13 that drops `get_dlf_level_default` into its `else` arm — `dlf_level =
0`, deblock disabled — where C, seeing M11, takes `<= ENC_M11` and returns 6 on
a base picture: `sb_based_dlf = 1`, the by-q closed form, `filt_guess = 3` at
qindex 67.

**MEASURED** with `tools/fh_fields.py` on `uniform 72x88 q40 p12` (a 30-byte C
frame, so almost pure header): the FIRST diverging field is
`loop_filter_level[0]`, C **3** against the port's **0**, and the walk then
desynchronises. A zero level also elides `loop_filter_level[2..3]` and
`loop_filter_delta_enabled`, which is where the missing byte goes. Frame OBU
payloads, C then port:

```
10 00 88 60 06 18 20 82 00 00 98 b0 40      (13 B)
10 00 88 60 00 00 20 00 00 98 b0 40         (12 B)
```

**Per-cell, 72x88 q40 video frame 0, before -> after. All ten closed:**

| content | C | port before | after |
|---|--:|--:|---|
| `uniform` p12 / p13 | 30 B | 29 B | **BYTE-IDENTICAL** |
| `gradient` p12 / p13 | 1634 B | 1633 B | **BYTE-IDENTICAL** |
| `diag` p12 / p13 | 643 B | 642 B | **BYTE-IDENTICAL** |
| `screen` p12 / p13 | 1144 B | 1143 B | **BYTE-IDENTICAL** |
| `screenrep` p12 / p13 | 2418 B | 2417 B | **BYTE-IDENTICAL** |

**Byte-neutral on the still path by construction AND measured.** The allintra
arm's clamp is `min(preset, 9)` and `get_dlf_level_allintra` returns 5 for
every preset from M6 up, so 9, 10, 11, 12 and 13 all resolve to the same level;
`identity_full_8bit` is 1100/1100 either way.

**The generalisation, said rather than left implicit.** The clamp is a
PROPERTY OF `pcs->enc_mode`, not of the dlf ladder — every other ladder the
port reads at a raw preset above the arm's clamp is wrong in the same way, and
this one was merely the first to become visible. `rate_arm::eff_enc_mode` is
already the right helper; what is missing is an audit of its call sites against
every `speed_config.preset` read in a ladder. Nothing else moved on the 60-cell
matrix at 12/13, so no OTHER unclamped read is observable on synthetic content
at those presets today — which is a bound on the audit, not a substitute for it.

### 1o. `disallow_4x4` — a ONE-PRESET arm fork, and it is all of `diag p3`'s 22 %

`diag 72x88 q40 p3` was **22.257 %** off (C 319 B against the port's 248 — the
port UNDER-coded) and `gradient 72x88 q40 p3` 1.628 %. `diag p3` is
byte-identical now and `gradient p3` is 0.212 %. p3 was the ONLY preset either
content was off at, which is the shape of the answer.

**Localized by joining the trees, not by reading ladders.** C's coded tree
(`SVT_CTREE_OUT` through `tools/ctrace-linux/`, cut at the last `mi=(0,0)`
line so the inter frame's blocks are dropped) against the port's
`SVTAV1_PACKTREE`, via `tools/tree_diff.py`:

```
TREE: 53 blocks joined, 22 field flips, 7 skip flips,
      geometry: 2 C-only / 10 port-only
  flip counts: ady=6 bsize=4 mode=4 txd=7 uv=1
  FLIP mi=(0, 0)  bsize: C=3 port=0      <- C codes BLOCK_8X8, the port BLOCK_4X4
  FLIP mi=(0, 16) bsize: C=3 port=0
  FLIP mi=(16, 0) bsize: C=3 port=0
```

Every `bsize` flip is C's 8x8 against the port's 4x4, and every port-only block
is a 4x4 quadrant of one of them. That names `disallow_4x4` outright.

**The ladders, and the fork is exactly one row wide:**

| | `<= M2` | `M3` | `>= M4` |
|---|---|---|---|
| `svt_aom_get_disallow_4x4_allintra` (`:8181`) | false | **false** | true |
| `svt_aom_get_disallow_4x4_default` (`:8169`) | false | **true** | true |

M0..M2 allow 4x4 on both arms and M4+ forbid it on both, so preset 3 is the
only cell in the whole preset domain where the two arms disagree. The port
carried the allintra rule flattened as the literal `preset >= 4`, in THREE
places — `DrCtrls::for_level`, and `pic_disallow_4x4` plus
`IbcFrameState::disallow_4x4` in `pipeline.rs`.

**What landed.** `leaf::get_disallow_4x4_{default,rtc,allintra}` (all three C
rows), `part_arm::disallow_4x4(arm, preset)` selecting between them through
`rate_arm::eff_enc_mode`, and the three literals replaced by calls to it. Both
C symbols are EXPORTED, so `c_parity_sig_deriv_leaf.rs::disallow_4x4_matches_c`
pins the pair at **tier 1** over the whole `ENC_MODES` sweep, with an
anti-vacuity assert on the M3 row itself (the sweep would pass trivially if the
arms ever agreed everywhere).

**Measured, `72x88 q40` video frame 0, before -> after:**

| cell | before | after |
|---|--:|--:|
| `diag p3` | 22.257 % (C 319 B, port 248) | **0.000, BYTE-IDENTICAL** |
| `gradient p3` | 1.628 % (C 1413 B, port 1390) | 0.212 % (port 1410) |

and the trees, on the same builds:

| cell | field flips | port-only geometry |
|---|---|---|
| `diag p3` | 22 -> **0** | 10 -> **0** |
| `gradient p3` | (not dumped before) | **0 flips, 0 port-only** after |

**`gradient p3`'s residual 3 bytes are NOT in mode decision — they are LOOP
RESTORATION.** Its coded tree, every leaf mode, uv mode, angle delta, tx depth
and skip flag equal C's (116 blocks joined, **0 field flips, 0 port-only
geometry**), and the op trace localizes the rest exactly:

```
C tile ops 9461, port 9439.  C ops 0..21 are a BOOL + 21 BOOLEQ prefix the
port does not emit AT ALL; C op 22 == port op 0 and every op after lines up.
  C  0: W BOOL   val=1 f=15913 rng=32768      port 0: W CDF nsyms=10 s=3 ...
  C 22: W CDF nsyms=10 s=3 icdf=[12631,...]   port 0: the same symbol
```

A one-bool-plus-literal-bits prefix at the head of a superblock's symbols is
`read_lr` — the AV1 spec codes each superblock's loop-restoration unit
parameters BEFORE its `decode_partition`, and a Wiener/SGR unit's coefficients
are `decode_signed_subexp_with_ref` literals. `tools/fh_fields.py` confirms it
in the header, where the same decision is signalled:

| field | C | port |
|---|--:|--:|
| `lr_type[0]` (`gradient 72x88 q40 p3`) | **3** | **0** (RESTORE_NONE) |
| `lr_type[0]` (`gradient 72x88 q40 p4`, byte-identical control) | 0 | 0 |

**CORRECTION (2026-09-01): `lr_type` 3 is RESTORE_SGRPROJ, not
RESTORE_SWITCHABLE.** `lr_type` is the CODED 2-bit field and the spec maps it
through `Remap_Lr_Type = {NONE, SWITCHABLE, WIENER, SGRPROJ}` (5.9.20), which
is a DIFFERENT order from SVT's internal `RestorationType` enum
(`NONE, WIENER, SGRPROJ, SWITCHABLE`) — the two disagree on exactly the values
1 and 3. That matters for scope, and independently: `rest_finish_search` sets
`num_rtypes = (plane_ntiles > 1) ? RESTORE_TYPES : RESTORE_SWITCHABLE_TYPES`
(`restoration_pick.c:1600`), so a plane with a SINGLE restoration unit — which
72x88 is, at `unit_size` 256 — never even CONSIDERS switchable. C could not
have picked it here.

So the next chunk on this cell is the VIDEO arm's RESTORATION ladder, and the
fork is one row wide again. At 240p on a base key frame
(`is_not_last_layer = true`):

| preset | `wn_filter_level` allintra / video | `sg_filter_level` allintra / video |
|---|---|---|
| M0..M3 | 3 / **4** | **0 / 3** |
| M4..M6 | 4 / 5 | 0 / 0 |
| M7..M8 | 0 / 5 | 0 / 0 |

(`svt_aom_get_wn_filter_level_{allintra,default}` `:1386` / `:1357`,
`svt_aom_get_sg_filter_level_{allintra,default}` `:1431` / `:1402`.)

**`sg_filter_level` is the one that matters at p3**: the allintra ladder is 0
everywhere except `<= ENC_MR`, so self-guided restoration is UNREACHABLE on the
still path, while the video arm asks for level 3 through M3. With Wiener alone
the port's search picks RESTORE_NONE; with SGR in the set C picks
RESTORE_SWITCHABLE. That is also why p4 is byte-identical either way — there
both arms have `sg = 0`.

The search is already ported and NOT wired: `crates/svtav1-encoder/src/
port_sgr_search.rs` carries `apply_sgr`, `search_selfguided_restoration`,
`count_sgrproj_bits` and the decision bodies of `search_sgrproj_finish` /
`search_switchable`, and says in its own header that "the wiring is a separate
change". `MultiProcessesSignals` models `wn_filter_level`, `sg_filter_level`
and `enable_restoration` at tier 1, and `speed_config::seq_tools_for_preset`
ALREADY takes the VIDEO ladders for the sequence-header bit
(`enable_restoration = wn_nonzero || sg_nonzero`, i.e. preset <= 8). So the
header side is done; the frame side is not.

**Scope it honestly — this is NOT the wiring-only shape §1m..§1p had.**
`restoration.rs`'s `search_restoration_still_bd` is a two-way
{NONE, WIENER} frame RD and `RestUnit` carries a `WienerInfo` and nothing else;
`entropy/lr.rs` has no SGRPROJ writer at all (`write_lr_for_sb` even
`debug_assert`s `frame_rtype == RESTORE_WIENER`, with the comment "only WIENER
frame types are searched/signaled (sg_filter_lvl = 0)"); and
`apply_restoration_frame` applies Wiener only. Four new pieces, not one:
a per-unit SGR sse, the SGRPROJ and SWITCHABLE frame walks feeding a four-way
`rest_finish_search` argmin, the unit-param writer, and the apply.

**And do not wire `wn_filter_level` on its own as a warm-up.** The video ladder
is 5 at M4..M8 where the allintra one is 0 above M6, so it would turn the
Wiener search ON at presets 7 and 8 — where `gradient` and `screenrep` are
byte-identical TODAY with the search off, i.e. C's own level-5 search picks
RESTORE_NONE there. Flipping that on cannot close p3 (whose gap is `sg`, not
`wn`) and can only put two green cells at risk. Land the SGR arm and the wn
arm together, or measure the wn arm on the full 60-cell matrix before
believing it.

### 1o'. The SGR arm LANDED (2026-09-01) — `gradient p3` closes, and the wn half was harmless

Both arms landed together, as the paragraph above says to. **57 of 60**
byte-identical, up from 56; `gradient p3` 0.212 % -> **byte-identical**;
nothing else moved a byte in either direction.

**Scope, corrected downward by two facts before any code was written.** §1o
called this "four new pieces" against a SWITCHABLE reading of `lr_type` 3. It
is still four pieces, but SWITCHABLE is not the one this cell needed — see the
`Remap_Lr_Type` correction above — and the pieces were smaller than they read,
because `port_sgr_search.rs` already carried every decision body:

| piece | where it landed |
|---|---|
| per-unit SGR sse | `restoration.rs`: `search_sgrproj_seg`'s sweep through a new `LrPixel::sgr_search_unit`, then the EXISTING `try_restoration_unit` generalised from Wiener-only to C's `RestorationUnitInfo` |
| the SGRPROJ + SWITCHABLE frame walks | `restoration.rs`: the finish phase is now C's `rest_finish_search` — one walk per candidate type in order, `force_restore_type_d`, the `plane_ntiles == 1` switchable exclusion, the per-unit `best_rtype[]` the switchable walk reads, the chroma skips |
| the unit-param writer | `entropy/lr.rs::write_sgrproj_filter` + all three `frame_rtype` arms in `write_lr_for_sb`, with `LrWalkRefs` carrying the SGR reference |
| the apply | `svtav1-dsp::restoration`: `filter_unit_impl` dispatches C's `filter_idx = 2 * highbd + (rtype == SGRPROJ)` into the already-ported `sgrproj_filter_stripe{,_highbd}` |

**The wn half did NOT put p7/p8 at risk, measured on the full matrix.** The
port now runs a level-5 (luma-only) Wiener search at video p4..p8 where it ran
none above M6 before, and every one of those cells is byte-identical on BOTH
sides of the chunk — i.e. C's own level-5 search picks RESTORE_NONE on this
content and the port's reproduction of it picks RESTORE_NONE too. The warning
was right to demand the measurement and wrong about the outcome.

**Reachability positive control** (`SVTAV1_LR_DBG` unit counts, `gradient
72x88 q40` — counting CALLS, not reading a zero, per `WORKING-ON-THIS.md` §5):

| cell | SGR units | Wiener units | finish walks |
|---|--:|--:|--:|
| video p3 | **3** (Y+U+V) | 3 | 9 (3 planes x {NONE, WIENER, SGRPROJ}) |
| video p4 / p7 / p8 | 0 | **1** (luma only) | 2 |
| still p3 | 0 | 3 | 6 |
| still p7 | 0 | 0 | 0 |

Every row is what the ladder table above predicts, including the two the
all-intra arm cannot produce (video p7/p8's single luma Wiener unit) and the
one this chunk exists for (video p3's three SGR units).

**Two transcription defects found in `port_lr_level.rs` while wiring it**, both
latent because the module was unwired and both wrong the moment it is not:

- `INPUT_SIZE_8K_RANGE` was **5**, which is `INPUT_SIZE_4K_RANGE`, and
  `INPUT_SIZE_360P_RANGE` was **0**, which is 240p (`definitions.h:1824-1831`;
  `port_enc_mode_config::ResolutionRange` and
  `port_picstruct::INPUT_SIZE_360P_RANGE` already had them right). The first
  would have killed BOTH filters at 4K where C keeps them; the second would
  have killed SGR at 360p under `fast_decode` where C keeps it. The module's
  own test asserted the wrong constant for its "above 360p" case and now
  passes 480p, which is what it meant.
- `wn_filter_level_allintra` applied an 8K force-off. C's all-intra variant
  (`enc_mode_config.c:1386`) takes `EncMode` ALONE and has no resolution
  clause — only `_default` and `_rtc` do.

**Evidence.** The SGR writer is gated by
`c_parity_lr_syntax::write_sgrproj_filter_matches_c` against a new
`ref_write_sgrproj_filter_bytes` shim that composes C's own exported
`aom_write_literal` + `svt_aom_write_primitive_refsubexpfin` over C's real
`svt_aom_eb_sgr_params` table: coded BYTES over all 16 `ep` x 3 references x 25
`xqd` pairs, with an anti-vacuity assert that all three radius shapes are
exercised, plus `set_default_sgrproj` pinned against a second shim. Tier 1
arithmetic, tier 4 radius dispatch, stated as such in the test rather than
blurred. The frame walk itself is tier 4 (`rest_finish_search` needs a built
`RestSearchCtxt`); the byte cell is the gate.

**Gates.** `tools/video_key_matrix.sh` 57/60 (new tool, committed — §1m..§1p
each rebuilt this loop by hand); six still cells at their pinned sizes
(290 / 839 / 63 / 171 / 580 / 693 B); `regression_spotcheck.sh` 62/62 with the
new `video-key-lr-sgr-arm-p3-gradient` cell; `cargo nextest run --workspace`
2417/2417.

**A CORRECTION to the sentence this replaces**, which said the residual was
"downstream, the same shape as §1j's two residuals" and that
`vdiff_cell.sh`'s alignment "does not resolve it". Both halves were wrong. The
tool resolves it perfectly once you read its segments rather than its
first-diff line: its positive control passes on `gradient p4`
("op streams identical", 9400 ops a side), and on p3 the 22-op offset is the
whole answer. Its `STAGE:`/`FIRST DIVERGING OP: 0` line is what misled — that
comes from `identity_diff.py`, whose op index is documented as unreliable on a
video cell. Read `optrace_first_diff.py`'s segment lengths, and when they
differ by a constant, look for a PREFIX one side omits rather than a symbol
both sides got wrong.

### 1o''. The p0 cluster, LOCALIZED (2026-09-01) — and §1o's PD0_LVL_0 lead is REFUTED

The three cells §1o hands forward (`gradient` / `diag` / `screenrep` at
preset 0) are still open; this section is the localization, not a fix, and it
moves the lead somewhere else entirely.

**§1o said the lead was PD0_LVL_0's block cost. It is not, on two independent
counts, both read out of the C source rather than argued.**

1. **The cost row at video PD0_LVL_0 is exactly the LVL_1 family's.**
   `svt_aom_sig_deriv_enc_dec_pd0` (`enc_mode_config.c:7357-7370`) computes
   `rate_est_level` INSIDE `if (pcs->rate_est_level)`, and it is
   `pd0_level <= PD0_LVL_3 -> 2`. On the video arm `pcs->rate_est_level` is 1
   at every preset, so PD0_LVL_0 gets `MAX(2, 1) = 2`, and
   `set_rate_est_ctrls` case 2 is `coeff_rate_est_lvl 1`, `lpd0_qp_offset 0`,
   `pd0_fast_coeff_est_level 2` — the row `Pd0Mode::Lvl1` already implements.
   The closed form `Pd0Mode::Lvl0` models (`coeff_rate_est_lvl 0`,
   `qp_offset 8`) is reachable ONLY through the bd10 forcing, where
   `pcs->rate_est_level` is 0 and the `if` never runs. C's own
   `SVT_PD0CFG_OUT` on both cells says so directly:
   `lvl=0 subres=0 dev_th=0 split_th=50 exit_th=0 rate_lvl=1 qpoff=0
   fastcoef=2 srcsamp=0 pred_only=0 d4=0 d8=0 maxbs=64 bias=1000 intra=1/0/0
   nsq=1` — every field of which the port's LVL_1 arm already matches.
2. **`intra=1/0/0` changes nothing in PD0.** §1o read the DC-only intra row
   (`set_intra_ctrls`'s `MAX_INTRA_LEVEL - 1`) as narrowing PD0's candidate
   set against p8's `1/12/1`. PD0's injector is
   `generate_md_stage_0_cand_pd0` -> `inject_intra_candidates_pd0`
   (`mode_decision.c:3164`), which injects EXACTLY ONE candidate — `DC_PRED`,
   `angle_delta 0`, `tx_depth 0`, `DCT_DCT` — and reads nothing from
   `intra_ctrls` at all. The only field that gates it is `enable_intra`, which
   is 1 in BOTH rows (`set_intra_ctrls` cases 1 and `MAX_INTRA_LEVEL - 1`).
   `intra_mode_end` / `angular_pred_level` are consumed by PD1's
   `inject_intra_candidates` and the md-stage pruning, neither of which PD0
   runs.

**What the trees actually say.** Container C oracle verified byte-equal to the
host oracle on both cells first (`diag` 207 B, `gradient` 1341 B), then
`SVT_CTREE_OUT` joined against `SVTAV1_PACKTREE` with `tools/tree_diff.py`:

| cell | field flips | port-only geometry | where |
|---|--:|--:|---|
| `diag 72x88 q40 p0` | **10** | 1 | ALL of them at `mi_row` 16/18/20 — the bottom partial-SB row |
| `gradient 72x88 q40 p0` | **180** | 4 | 127 above `mi_row` 16, from the FIRST superblock |

`diag`'s ten flips are all in superblock 2 and 3 of a 2x2 grid; `gradient`'s
reach superblock 0 as well. That reads as two different bugs and it is not —
see the correction below, which was measured on a 64-ALIGNED cell after this
table and supersedes the partial-SB framing.

**CORRECTION, same session, measured after the paragraph this replaces.** That
paragraph read the `bsize` flips as the lead and pointed at
`depth_refine::build_refined_scan_at`. A wider cell says the partition is
almost right and the LEAF decision is not, so read the following instead.

**The discriminator: a 64-ALIGNED cell isolates the same three presets with no
partial-SB confound.** `gradient 128x128 q40`, four complete superblocks, one
build per side:

| preset | still (allintra) | video |
|---|---|---|
| 0 | IDENTICAL (543 B) | **diff** (C 2665, port 2714) |
| 1 | IDENTICAL (763 B) | **diff** (C 3012, port 2961) |
| 2 | IDENTICAL (695 B) | **diff** (C 3257, port 3264) |
| 3 | IDENTICAL (679 B) | IDENTICAL (3223 B) |
| 4 / 6 / 8 | IDENTICAL | IDENTICAL |

So it is the VIDEO arm at M0..M2 exactly — the presets where
`set_pic_pd0_lvl_default` gives PD0_LVL_0 — and it is not a partial-superblock
effect at all. The 72x88 reading above, where every differing PD0 block had
`x >= 64 or y >= 64`, is the SAME SET as "not superblock 0" on that geometry;
the partial-SB framing was an artefact of a 2x2 SB grid whose only complete SB
is the first one.

**And it is not PD0.** `SVT_PD0COST_OUT` against the port's `SVTAV1_PD0DBG`
on `gradient 128x128 q40 p0`, frame 0 (C's dump carries both frames; cut on the
lambda change):

* superblock 0 — **336 of 336 PD0 blocks agree exactly** on dist, ybits and
  cost;
* superblocks 1, 2 and 3 — every block differs, and the first one is SB 1's
  64x64 ROOT with `dist` EQUAL (3462072) and `ybits` C 937463 / port 937361.

A rate-only divergence on the first block of the second superblock, with the
first superblock's PD0 exact, is the per-SB rate table chained from superblock
0's coded CDFs — i.e. a CONSEQUENCE of superblock 0 coding differently, not a
cause. On the 72x88 cell the same shape holds (339 interior blocks exact, all
194 others differing, first divergence at the first block of the second SB).

**Where it actually starts.** The coded tree of `gradient 128x128 q40 p0`
(255 blocks joined) flips **287** fields, and they are LEAF fields:

| field | flips |
|---|--:|
| `mode` | 81 |
| `uv` | 76 |
| `fi` (filter-intra) | 58 |
| `ady` (angle delta) | 53 |
| `txd` | 5 |
| `bsize` | **2** |

Two `bsize` flips in 255 blocks: the PARTITION is right and the MODE DECISION
is not. 66 of those flips are inside superblock 0 — whose PD0 is exact — and
the earliest is `mi=(4,12)` (y=16, x=48): `mode` C=DC / port=SMOOTH, `uv` C=6 /
port=9, `fi` C=5 (off) / port=3, `txd` C=2 / port=1. The `fi` flips go BOTH
ways (27 where C is off and the port picks one, 24 the other way), so it is not
a filter-intra over-pick — it is the whole M0..M2 leaf funnel configuration on
the video arm.

**So the next chunk is the VIDEO arm's LEAF/PD1 ladder at M0..M2, and the cell
to drive it on is `gradient 128x128 q40 p0`, not 72x88** — four complete
superblocks, the same three presets, and no partial-SB geometry in the way.
Start at the first in-SB0 flip, `mi=(4,12)`, with the port's `SVTAV1_CANDDBG`
against C's `SVT_FASTCOST_OUT` / `SVT_FULLCOST_OUT`, which is the drill §1e and
issue #16 both used to split a leaf decision into its rate and distortion
terms.

**`SVT_CTREE_OUT` contains BOTH FRAMES and the cut matters enormously.** §1o
says to cut "at the last `mi=(0,0)` line"; that is not a usable rule, because
`mi=(0,0)` recurs dozens of times per frame through MD re-stamps. The reliable
cut is the FIRST record with `mode >= 13`: an I-slice cannot code an inter
mode, so that line is frame 1's first block. Uncut, `diag p0` reports 24 field
flips and 24 C-only blocks; cut, it reports **10 and 24** — and the 14
phantom flips are frame 1's `mode=16`/`skip=1` blocks landing on frame 0's
keys. Read any tree number without the cut and it is inflated.

### 1o'''. The video arm's SPATIAL SSE at MDS1 (2026-09-01) — `diag p0` closes

The chunk §1o'' hands forward, taken the same session. **58 of 60** on
the 72x88 scoreboard, up from 57; `diag p0` byte-identical.

**The fork, and it is one row wide again.**
`svt_aom_sig_deriv_mode_decision_config_allintra` pins
`spatial_sse_full_loop_level = 3` — SSSE_MDS3 — at EVERY preset
(`enc_mode_config.c:10010`), which is why MDS1 has always been a
frequency-domain stage in this port. The VIDEO ladder (`:9161-9165`) is

```c
if (enc_mode <= ENC_M2) pcs->spatial_sse_full_loop_level = 1;   /* SSSE_MDS1 */
else                    pcs->spatial_sse_full_loop_level = 3;   /* SSSE_MDS3 */
```

and `md_stage_1` reads it as `ctx->mds_do_spatial_sse =
spatial_sse_ctrls.level <= SSSE_MDS1` (`product_coding_loop.c:7025`), with
`SSSE_MDS1` the FIRST enum value (`definitions.h:886`) — so that test is
`level == SSSE_MDS1`, not "anything at or below". At video M0..M2, therefore,
MDS1's distortion is the SPATIAL SSE of the reconstruction against the source,
and the inverse transform that `full_loop_core`'s gate (`:4784`) skips at MDS1
everywhere else runs.

The ladder was ALREADY ported and unwired (`port_enc_mode_config::md_config`'s
`spatial_sse_full_loop_level`, `encdec::set_spatial_sse_full_loop_level`) — the
same shape as the SGR chunk. `FunnelCfg` gains `spatial_sse_mds1`,
`intra_arm::apply` stamps it per arm, and `mds1.rs` passes it as `tx_unit`'s
`spatial_dist`. No `need_recon` change: `tx_unit` derives it
(`do_recon = need_recon || spatial_dist`).

**Measured, `gradient 128x128 q40` (the 64-aligned cell), one build per side:**

| preset | still | video before | video after |
|---|---|--:|--:|
| 0 | IDENTICAL | 2714 (C 2665) | 2651 |
| 1 | IDENTICAL | 2961 (C 3012) | 2990 |
| 2 | IDENTICAL | 3257 (C 3257, differing bytes) | **IDENTICAL** |
| 3 / 4 / 6 / 8 | IDENTICAL | IDENTICAL | IDENTICAL |

and at video p0, block-level, C's `SVT_FULLCOST_OUT` against the port's
`SVTAV1_CANDDBG` at `mi=(0,0)` over every tested block size:

| | ydist |
|---|---|
| before | differed on all 46 MDS1 candidates of every size |
| after | **EQUAL on all 46 of every size**, 64x64 through 4x8 |

and that cell's coded tree: **287 field flips -> 201**, `bsize` 2 -> **0**,
`txd` 5 -> **0**, port-only geometry 4 -> **0**. The partition and the tx depth
are exact now.

**On the 72x88 scoreboard: 57 -> 58.** `diag p0` C 207 / port 206 ->
byte-identical. `screenrep p0` unchanged at 0.043 %. `gradient p0` moved
0.447 % -> **0.522 %** — FURTHER in bytes (1335 -> 1348 against C's 1341, i.e.
it crossed over) while its tree got much closer. That is §1f's
cancellation pattern read from the other side, and it is recorded as a
percentage regression rather than hidden: the byte count is not the thing
improving here.

**What is left at video p0/p1, measured.** At `mi=(5,0)` of the 128x128 cell —
the first remaining flip inside superblock 0 — MDS1 is now EXACT: same 46
candidates, same order, `ydist` identical, rate off only by the inert dump
offset below. C picks PAETH with no filter-intra there and the port picks V/DC
with filter-intra 1, so the divergence has moved to **MDS3** (TXS / TXT / RDOQ
/ chroma); 24 of the 201 flips are inside superblock 0 and the rest are
downstream of them. Ruled out by reading both `sig_deriv_mode_decision_config`
arms at M0..M2: `rdoq_level` (1 on both), `txt_level` (2 on both for
`is_base`), `intra_level` (1 on both — mode_end PAETH, angular 1),
`rate_est_level` (1 on both).

**One ladder IS forked and IS unwired, and it explains p1 but not p0:**
`svt_aom_get_chroma_level_{default,allintra}` (`enc_mode_config.c:8547` /
`:8573`) disagree at **M1 alone** on an I-slice — allintra 2, video 4 — and
agree at M0 (1 on both) and M2..M5 (4 on both). Both rows are already ported
and tier-1 EXPORTED in `port_enc_mode_config::leaf::get_chroma_level_*`;
`funnel_arm` wires `txt_level` and `cfl_level` but NOT `chroma_level`, so the
funnel still takes the allintra row. That is the next thing to wire, and it
cannot be p0's cause. Also still unread for p0: `nsq_search_level` and
`tx_shortcut_level`.

**A dump offset to not chase.** The port's `NSQDBG PMDS1 coeff_rate` runs a
constant ABOVE C's `ycb`, per tx-size class: 8x8 392, 16x16 750, 32x32 709,
64x64 1306. It is present at video **p6**, which is BYTE-IDENTICAL, so it is
inert — a dump-field difference, not a cost divergence.

### 1p. PD0_LVL_4 on the REFINEMENT path (2026-09-01) — `gradient p8`, and a latent `th` defect it exposed

`gradient 72x88 q40 p8` was 1.673 % off (C 1554 B, the port 1528 — the port
UNDER-split) and was the largest cell left after §1o. It is byte-identical now.

**Localized by DUMPING C's PD0 configuration, not by reading the ladder.** The
coded-tree join said the shape of the answer — `tools/tree_diff.py` on C's
`SVT_CTREE_OUT` against the port's `SVTAV1_PACKTREE`: **24 C-only blocks, 0
port-only**, with `bsize` C=BLOCK_8X8 / port=BLOCK_32X32 at `mi=(0,0)` and
C=BLOCK_16X16 / port=BLOCK_32X32 at three more — i.e. the port stops splitting
where C keeps going, which is a PD0 cost story. `SVT_PD0CFG_OUT` (the
`svt_aom_sig_deriv_enc_dec_pd0` `--wrap`) then said exactly what C runs, on
frame 0 SB0 of that cell:

```
lvl=4 subres=1 dev_th=5 split_th=50 exit_th=0 rate_lvl=2 qpoff=0
fastcoef=2 srcsamp=0 pred_only=1 d4=1 d8=0 maxbs=64 cb64=1 nsq=1
```

Three facts in one line, none of which the port had:

1. **`lvl=4`.** `set_pic_pd0_lvl_default`'s `enc_mode <= ENC_M8` arm is
   `MIN(MAX_PD0_LVL, 3 + ldp0_lvl_offset[qp_band])` (`:8631`) — 4 at 240p and
   CLI qp 40. `refined_pd0_model` matched only `3 => Lvl3` and returned the
   ALLINTRA `Lvl1` model for everything else. **A doc correction falls out of
   this**: `pd0_pick_sb_partition_video`'s comment said the level was
   `4 + ldp0_lvl_offset` "for M8 up", i.e. 5 at M8. The implementation was
   right — it is tier-1 gated — the comment was wrong, and is fixed in place.
2. **`rate_lvl=2`** — PD0 derives its OWN `rate_est_level` from `pd0_level`
   (`:7358-7366`: `<= PD0_LVL_3 -> 2`, `<= PD0_LVL_4 -> 4`, else 0), and
   `set_rate_est_ctrls` maps 4 to `coeff_rate_est_lvl = 2`. The three
   `pd0_pick_sb_partition_m6_eval` call sites passed the FRAME's
   `FunnelCfg::coeff_rate_est_lvl` instead. On the allintra arm the two agree
   at every preset that path serves, which is why it was right there.
3. **`pred_only=1`** — `ctx->pic_pred_depth_only` is
   `depth_refinement_ctrls.mode == PD0_DEPTH_PRED_PART_ONLY`, which only
   depth-refinement level 10 sets, and the video arm's non-`sc_class5` ladder
   takes level 10 from M8 up (`:9393`). That makes
   `set_depth_early_exit_ctrls` pick level 1 — `early_exit_th` 0, which
   `Pd0Ctx::pick` spells as `th = 1000` — even at a `pd0_level` above LVL_1.
   `refined_pd0_model` had hardcoded 900 for its one non-allintra row and
   documented `pic_pred_depth_only` as "FALSE on this path by construction";
   at M8 it is not. `DrCtrls` now carries the flag and the caller passes it.

   **BOTH branches of that flag are confirmed against C's own dump**, which
   matters because the ladder forks on `sc_class5` at exactly this preset —
   the `sc_class5` row is level 6 (`:9361`), the other level 10 (`:9393`):

   | cell at `72x88 q40 p8` | C `pred_only` | C `exit_th` | port `th` |
   |---|--:|--:|--:|
   | `gradient` (not screen content) | 1 | 0 | 1000 |
   | `screenrep` (not screen content) | 1 | 0 | 1000 |
   | `screen` (`sc_class5`) | **0** | **900** | 900 |

   A single hardcoded threshold would have been wrong on one of the two
   groups whichever value it took.

**A SECOND defect, latent until the first was fixed, and it caused a
REGRESSION that the matrix caught.** Wiring PD0_LVL_4 closed `gradient p8` and
simultaneously broke `screenrep 72x88 q40 p8`, which had been byte-identical:
2401 B against C's 2390. The PD0 block-cost join named it in one read —
C's `SVT_PD0COST_OUT` against the port's `SVTAV1_PD0DBG`, 130 blocks each side,
**83 of 130 costs differing** and every 8x8 the same way:

| block | C `ybits` | port `ybits` |
|---|--:|--:|
| `(0,0) 8x8` | 31528 | **6500** |
| `(0,8) 8x8` | 33646 | **6500** |
| `(8,8) 8x8` | 31598 | **6500** |

6500 is `6000 + eob*500` at `eob = 1` — the port was taking the
`coeff_rate_est_lvl >= 2 && eob < th` shortcut where C priced the real
coefficient rate. The threshold is `th = (bwidth * bheight) >> 5`, and
**`bheight` is the TRANSFORM's height, not the block's**: at
`mds_subres_step == 1` C rewrites `tx_size` TX_NxN -> TX_NxN/2
(`product_coding_loop.c:4332-4344`) before `txbheight` is read, so an 8x8 under
subres has `th = (8*4)>>5 = 1` and `eob = 1` is NOT below it. The port computed
`(8*8)>>5 = 2`.

It could not have mattered earlier: `th` is read only at
`coeff_rate_est_lvl >= 2`, and the only rows that set that before this chunk
were the ALLINTRA M7/M8 ones — which are PD0_LVL_1, subres step 0, where the
transform height IS the block height. A latent defect in a live function,
switched on by the level that first reaches it.

With `tx_h` instead of `bh`: **130 / 130 PD0 block costs identical** on
`screenrep p8` — dist, ybits, RD cost and lambda — and the cell is
byte-identical again.

**Per-cell, 72x88 q40 video frame 0:**

| cell | before | LVL_4 only | LVL_4 + the `th` fix |
|---|--:|--:|--:|
| `gradient p8` | 1.673 % (C 1554 B, port 1528) | **0.000, BYTE-IDENTICAL** | **BYTE-IDENTICAL** |
| `screenrep p8` | byte-identical | 0.460 % (2401 B vs 2390) | **BYTE-IDENTICAL** |
| every other cell | — | unchanged | unchanged |

The intermediate column is why `screenrep p8` earns a spot-check cell it would
not otherwise deserve: it is byte-identical on both sides of the chunk as a
whole and fails loudly on the half of it.

**Every video-key spot-check cell is now a `byteVideoKey`.** After §1m
promoted `video-key-ibc-arm-p8`, `tools/regression_spotcheck.sh` invokes
`byteVideoKey` 26 times and `ratioVideoKey` / `fhVideoKey` **zero** times —
both helpers survive only as definitions. Every video-mode key-frame assertion
in the registry is the strong one; there is no longer a cell whose payload is
being watched by proxy. Keep it that way: a new cell goes in as
`byteVideoKey` or with a written reason why its payload cannot close yet.

### The state of the 72x88 q40 video-key matrix after 1m..1p

**42 -> 56 byte-identical of 60**, fourteen cells closed, one improved
(`gradient p3` 1.628 % -> 0.212 %), nothing worse.
`benchmarks/video_key_matrix_72x88_2026-09-01.tsv` carries the full
before/after. Everything still open:

| preset | cells still off |
|---|---|
| 0 | `gradient` 0.447 %, `diag` 0.483 %, `screenrep` 0.043 % |
| 3 | `gradient` 0.212 % (coded tree EXACT — `lr_type[0]` C=3 port=0) |

Nothing above 0.5 % survives at `72x88 q40` in video mode.

**UPDATE 2026-09-01 (§1o'): 57 / 60.** The `gradient p3` row above is closed —
it was the video arm's loop-restoration ladder, and `lr_type` 3 is
RESTORE_SGRPROJ, not SWITCHABLE. Only the three p0 cells remain, and §1o' does
not touch them. Reproduce the scoreboard with `tools/video_key_matrix.sh`.

**UPDATE 2026-09-01 (§1o'''): 58 / 60.** `diag p0` is closed too — the video
arm's SPATIAL SSE at MDS1. Open: `gradient p0` (0.522 %, a percentage that went
UP while its tree got closer) and `screenrep p0` (0.043 %).

**But read the p0 cluster as a WARNING, not as "nearly closed" — measured,
2026-09-01.** `gradient 72x88 q40 p0` is 0.447 % off in BYTES and its coded
tree is not close: `tools/tree_diff.py` on C's `SVT_CTREE_OUT` against the
port's `SVTAV1_PACKTREE` reports **214 field flips and 4 port-only blocks**
over 128 joined keys, including `bsize` C=BLOCK_32X32 / port=BLOCK_4X16 at
`mi=(0,0)` and four `C_skip=1` blocks where the port codes 22-40 luma
coefficients. Port-only geometry is the tool's own alarm condition (C-only keys
are expected — C re-stamps sub-keys), so those trees genuinely differ. This is
§1f's cancellation pattern at its most extreme in the campaign so far: a
completely different tree landing within half a percent of C's size. **A cell at
0.447 % is not "six bytes from parity" here; it is a different encode that
happens to cost the same.**

What C runs there, from `SVT_PD0CFG_OUT` on that cell (frame 0, SB0):

```
lvl=0 subres=0 dev_th=0 split_th=50 exit_th=0 rate_lvl=1 qpoff=0
fastcoef=2 srcsamp=0 pred_only=0 d4=0 d8=0 maxbs=64 cb64=1 intra=1/0/0 nsq=1
```

`lvl=0` is PD0_LVL_0, which `refined_pd0_model` explicitly does not carry — it
returns the allintra PD0_LVL_1 model and says so. Note `intra=1/0/0`
(`intra_mode_end = DC_PRED`, `angular_pred_level = 0`) against p8's `1/12/1`:
`set_intra_ctrls` gives PD0_LVL_0 `MAX_INTRA_LEVEL - 1`, whose row is
DC-only, where every other level on an I-slice takes `intra_level = 1` and
tests through PAETH with angular prediction (`enc_mode_config.c:7241-7247`).
`d4=0` also puts 4x4 blocks back in PD0's search at M0..M2 on both arms.

That is the next chunk, and it is a bigger one than §1p: PD0_LVL_0's block cost
is not a variant of LVL_1 the way LVL_3 and LVL_4 are.

**A harness note for whoever takes it.** `SVTAV1_PACKTREE` emits each coded leaf
TWICE per run at presets 3..13 and THREE times at preset 0 — the port packs the
tile more than once — so a per-key count off that file is not a leaf count.
`tree_diff.py` takes the last record per key and is unaffected; a hand-rolled
`grep -c` is not.

### 1q. THE INTER FRAME EMITS — and its header is field-exact but for CDEF (2026-09-01)

*(Superseded within the day by §1r, which closes the CDEF residual and makes
the header byte-identical. Kept because the field-by-field record and the two
harness traps below are what the next chunk needs.)*

Frame 1 of a 2-frame encode no longer refuses. `gradient 64x64 q40 p6`,
`tools/identity_diff_inter.sh 64 64 40 6 2 gradient`:

| | frame 0 | frame 1 |
|---|--:|--:|
| C | 961 B | 22 B |
| port | **961 B, IDENTICAL** | 113 B, DIFFERS |

The 22-byte target decomposes as a 2-byte temporal delimiter, a 2-byte OBU
header, a **15-byte frame header** and a 3-byte tile. The port's frame header
is also **15 bytes**, and **12 of those 15 bytes are identical**:

```
C    30 02 00 80 00 db 3b 40 00 00 04 04 e0 1c 00
port 30 02 00 80 00 db 3b 40 00 00 00 00 00 1c 00   <- before the CDEF wiring
port 30 02 00 80 00 db 3b 40 00 00 04 3e 00 1c 00   <- after
```

Same length means the same FIELD LAYOUT — every conditional field's presence
decision agrees, which is the part that shifts everything after it when it is
wrong. `tools/fh_fields.py --index 1` names the three differing fields:

| field | C | port before | port after |
|---|--:|--:|--:|
| `cdef_damping_minus_3` | 2 | 0 | **2** |
| `cdef_y_sec_strength[0]` | 2 | 0 | **2** |
| `cdef_y_pri_strength[0]` | 0 | 0 | 15 |
| `cdef_uv_pri_strength[0]` | 7 | 0 | 0 |

**That is an ENCODE-side gap, not a header defect** — the fields are in the
right places with the wrong values. `pipeline.rs` ran the CDEF pick under
`if is_key`, so an inter frame fell through to `CdefFrameParams::default()`:
damping 3 and every strength 0. The pick now runs on every coded frame, and
`cdef_frame_is_boosted` / `cdef_is_not_highest_layer` — literal `is_key` while
only key frames were encodable — come from the picture decision's
`update_type` (C's `frame_is_kf_gf_arf` and `update_type != LF_UPDATE`). That
closed `cdef_damping_minus_3` and `cdef_y_sec_strength[0]`.

**CLOSED, same day — the inter frame header is BYTE-IDENTICAL to C's.**
See §1r below. What follows is the localization that closed it, kept because
the mechanism is the reason no key frame could ever have exercised it.

Frame 1 of this GOP is an `LF_UPDATE` (`set_frame_update_type`:
`hierarchical_levels == 0`, so `frame_offset % 4` — offset 1 is odd, hence LF),
so `frame_is_boosted` and `is_not_highest_layer` are BOTH false. At preset 6
`cdef_search_level_default` gives level **5**, and level 5's row is
`search_best_ref_fs = is_not_highest_layer ? 0 : 1`
(`enc_mode_config.c:1073`) — so on this frame it is **1**, a value no key frame
can ever select (a key frame's `is_not_highest_layer` is true).

`search_best_ref_fs = 1` is not a threshold; it REPLACES the candidate set.
`update_cdef_filters_on_ref_info` (`md_config_process.c:681-745`) sets
`first_pass_fs_num = 1`, clears the second pass, and then ADDS the list-0 (and
list-1) REFERENCE pictures' own chosen strengths
(`EbReferenceObject::ref_cdef_strengths[0][0]`) as extra first-pass
candidates. The port searches its default `{0, 7, 15}` set instead and picks
`y_pri = 15`; C, searching the reference's strength, picks `y_pri = 0`,
`uv_pri = 7`.

So the chunk is: **store the chosen CDEF strengths on the DPB entry and port
`update_cdef_filters_on_ref_info`** (both its `use_reference_cdef_fs` and its
`search_best_ref_fs` arms). `ReferenceFrame` already gained chroma in this
chunk; `ref_cdef_strengths` / `ref_cdef_strengths_num` belong beside it. Note
the sibling trap while you are there: the `use_qp_strength` fast path reads
`allintra ? ppcs->sc_class5 : ppcs->sc_class1` (`enc_cdef.c:913-918`), and
`pipeline.rs` passes `sc_class5` unconditionally — correct on the allintra arm
it was written for, unverified on the video arm.

Everything else in the header is C's, from C's own derivations rather than
from constants:

* `refresh_frame_flags = 2` and `ref_frame_idx = [0,0,0,0,3,3,3]` come from
  `port_picstruct::picture_decision_per_picture` — the ported
  `av1_generate_rps_info` low-delay CQP branch plus `prune_refs`. Nothing was
  tuned to make them match.
* `primary_ref_frame = 0` from `bind_refs_and_primary_ref_frame`. **CDF
  continuation is therefore LIVE**: `error_resilient_mode = 0`, so C's inter
  frame inherits slot 0's end-of-frame CDFs. The port does not implement that
  yet (there is no per-ref-slot CDF store anywhere in the tree) — which is the
  single largest remaining tile-side prerequisite, ahead of the MVs.
* `allow_high_precision_mv = 0`, `is_filter_switchable = 1`,
  `is_motion_mode_switchable = 1`, `use_ref_frame_mvs = 1`,
  `allow_warped_motion = 1` from
  `sig_deriv_mode_decision_config_default` (EXPORTED, tier 1) via
  `crate::inter_hdr_arm`.

Gate: `tools/inter_fh_gate.sh`. It asserts frame 0 byte-identity outright and,
for frame 1, that the set of differing HEADER FIELDS is a SUBSET of a listed
open set — so closing a field keeps it green while a new divergence, or a
changed field PRESENCE, turns it red. **That open set is now EMPTY** (§1r), so
the gate is a plain field-identity assertion on the inter frame header.

#### Two traps this chunk paid for

**`SWITCHABLE` is 4, not 3.** `definitions.h:844-846` — `SWITCHABLE_FILTERS =
BILINEAR = 3` and `SWITCHABLE = SWITCHABLE_FILTERS + 1 = 4`. Comparing
`interpolation_filter` against 3 made the header write
`is_filter_switchable = 0` plus a 2-bit filter index C never wrote, and every
field from bit 50 on was off by two bits. Use
`port_enc_mode_config::md_config::SWITCHABLE`, never a literal.

**`fh_fields.py` was GUESSING `skipModeAllowed`, and it lied on this cell.**
It approximated the rule as "1 whenever `reference_select` is set" (its own
comment said the real rule needs the reference order hints). On the 2-frame
cell every DPB slot still holds the key frame, so there is no second distinct
forward reference and C writes **no** `skip_mode_present` bit — but the tool
read one, and then reported `allow_warped_motion = 0` when the stream says 1.
Every field after `skip_mode_present` was off by one bit and the printout gave
no sign of it, because the shifted values were all zeros. **A reading taken off
that tool before 2026-09-01, on any inter frame, is suspect.** It now
implements the real `skip_mode_params()` and threads the decoder's
`RefOrderHint[]` across the frames of the stream to do it, which is why it
takes `--index N` by walking frames 0..N rather than jumping to N.

### 1r. The inter frame header is BYTE-IDENTICAL (2026-09-01)

```
C    30 02 00 80 00 db 3b 40 00 00 04 04 e0 1c 00
port 30 02 00 80 00 db 3b 40 00 00 04 04 e0 1c 00
```

All 15 bytes. `tools/inter_fh_gate.sh` reports "field-exact except 0
known-open field(s)". The whole of frame 1's remaining divergence is now the
TILE: C's is **3 bytes**, the port's is 94.

What closed it was NOT a CDEF search fix. C never searches on that frame.

`set_cdef_search_controls` level 5 sets
`search_best_ref_fs = is_not_highest_layer ? 0 : 1` (`enc_mode_config.c:1073`),
and `is_not_highest_layer` is `update_type != LF_UPDATE` — TRUE for every key
frame. So on a key frame that flag is always 0 and
`update_cdef_filters_on_ref_info` (`md_config_process.c:681-772`) is
unreachable; on the first inter frame of a flat low-delay GOP it is 1, and that
function then:

1. seeds the candidate list with the list-0 reference's own chosen strength,
2. finds that the list-1 reference chose the SAME one — every DPB slot still
   holds the key frame, so list 0 and list 1 ARE the same picture — and
3. takes the `use_reference_cdef_fs` arm: **`first_pass_fs_num = 0`**, no
   search at all, `pred_y_f = ref_l0.y0`, `pred_uv_f = (ref_l0.uv0 +
   ref_l1.uv0) / 2`.

The prediction that confirmed it before a line was written: if that is what C
does, frame 1's strengths must EQUAL frame 0's. `fh_fields.py --index 0` on the
same stream gives `y_pri 0 / y_sec 2 / uv_pri 7 / uv_sec 0` — frame 1's values
exactly. (Damping legitimately differs, 4 vs 5: it is
`3 + (base_q_idx >> 6)` per frame, not inherited.)

So the port needed two things it did not have, and both are now in:
`ReferenceFrame::cdef_{y,uv}_strengths` (C's
`EbReferenceObject::ref_cdef_strengths`, written from the FRAME HEADER at
`rest_process.c:207-210`), and the port of
`update_cdef_filters_on_ref_info` in
`port_enc_mode_config::cdef_search`.

**One gap in that path is NAMED, not silently absent.** C reaches
`update_cdef_filters_on_ref_info` only after `me_based_cdef_skip`
(`md_config_process.c:781`) declined to switch CDEF off, and that skip needs ME
distortion this pipeline does not produce. It returns false immediately on an
I_SLICE, so the omission cannot affect a key frame; on an inter frame whose ME
distortion would have tripped it, the port will filter where C would not.

#### What the TILE needs next, in dependency order

C's frame-1 tile is **3 bytes** for a whole 64x64 frame. The port's is 94. The
prerequisites, ordered so nothing downstream is measured over a broken premise:

1. **CDF continuation — the blocker, and it is not optional.** *(CLOSED
   2026-09-01 — see §1s.)* The header this
   chunk made byte-exact says `primary_ref_frame = 0` and
   `error_resilient_mode = 0`, which means the tile's CDFs start from the
   REFERENCED frame's END-OF-FRAME state, not from the defaults. The port has
   **no per-ref-slot CDF store anywhere in the tree** (`save_cdfs` /
   `restore_cdfs` / `load_cdfs`: zero occurrences), and `pipeline.rs` says in
   so many words that it always assumes `PRIMARY_REF_NONE`. Until that exists,
   the FIRST symbol of the tile is coded against the wrong probabilities and no
   amount of correct mode decision can produce matching bytes. It also needs
   `disable_frame_end_update_cdf` to stay 0 (it is) AND the frame-end CDF
   save to actually run.
2. **The inter branch of the tile walk.** `entropy/tile.rs` writes no inter
   syntax at all; `port_entropy_inter/` (4,321 lines, 12 files) is ported and
   tested but is not reachable from `encode_frame_impl`. Wiring it is a
   plumbing chunk, not a porting one.
3. **The MVP stack's inter branch and MV coding** (`inter_mvp.rs` 2,530 lines,
   `inter_mv_code.rs` 833 — both ported, both islands), then the real ME
   (`inter_me/`, ~4.7k lines, also an island). The campaign's map called these
   C2/C3/C4; the measurement above says they are BEHIND the CDF store, not
   ahead of it.

The counting matters: roughly 25 kLOC of tier-1/tier-4-gated inter port is
already in tree and unreachable, against ~1 kLOC of homegrown inter code that
is what the pipeline calls today. The next chunks are mostly WIRING.

#### No regression, measured after the chunk

`identity_full_8bit.sh` **1100 / 1100**, `regression_spotcheck.sh` **65 / 65**
(the new `inter-frame-header-gradient-p6` cell included — before the chunk it
could not run at all, because the port refused frame 1), the 72x88 q40
video-key matrix **58 / 60** (unchanged — `gradient p0` and `screenrep p0`
still open), the six pinned still cells byte-identical at
290 / 839 / 63 / 171 / 580 / 693 B, and `cargo nextest run --workspace`
**2422 / 2422** on aarch64 (2418 + the four new
`update_cdef_filters_on_ref_info` unit tests). Cross-ISA on x86-64 (`r7900x`, from `main@origin` after
the chunk landed): the inter frame-header gate passes with the SAME result
(frame 0 identical, frame 1 header field-exact with an empty open set),
`regression_spotcheck.sh` **65 / 65** and `cargo nextest run --workspace`
**2432 / 2432**. Both the reference-derived CDEF path and the header assembly
are therefore ISA-independent, which was not a given: the CDEF pick is a SIMD
search on both arms.

Two things are unchanged BY CONSTRUCTION rather than only by measurement, which
is the stronger claim:

* the key-frame header layout — every pre-inter caller reaches the writer
  through a shim that passes `inter = None`;
* the reference-derived CDEF path — every level derives
  `use_reference_cdef_fs` / `search_best_ref_fs` from `!is_base` or
  `!is_not_highest_layer`, and a key frame has both true. That is asserted for
  all eleven levels by
  `cdef_search::tests::a_key_frame_never_asks_for_a_reference_derived_set`.

#### The C oracle is capped at TWO frames in this GOP — measured, not assumed

`capture_c_trace` **segfaults (rc 139) for every low-delay run
(`SVT_PRED_STRUCT=1`) of three or more frames**, at 64x64 preset 8 q35 and
preset 6 q40 alike. The library prints
`ST mode: empty object pool exhausted after pumping dispatcher`
(`sys_resource_manager.c:791`) and then dereferences the wrapper it did not
pop. Random-access (`SVT_PRED_STRUCT=2`) survives at least 3 frames, which is
why the RA capture script could take 7/9/17/25/41.

**The obvious fix does not work.** Interleaving a non-blocking
`svt_av1_enc_get_packet(.., pic_send_done = 0)` after every `send_picture` —
which is exactly what `SvtAv1EncApp` does
(`app_process_cmd.c:1104-1111`) — makes it WORSE: the 2-frame cell then
segfaults too, after writing `pts0`. That was tried, measured and reverted; the
driver in tree is the send-all-then-drain one that gets 2 frames. Do not
"fix" it again without a measurement.

Consequence for the campaign: every inter cell is a 2-frame cell until that is
solved, and the first inter frame's references are therefore all the key frame,
which is what collapses both reference lists to one entry.

### 1s. CDF CONTINUATION LANDED, and the inter TILE is BYTE-IDENTICAL from C's decision (2026-09-01)

Two chunks. The first closed §1r's named blocker; the second turned the
remaining question from "is the entropy path right?" into "is the mode
decision right?", which is a much smaller and much better-posed question.

#### The store (`crate::port_frame_cdf`)

C has three sites and this chunk landed two of them:

| C | what it does | port |
|---|---|---|
| `packetization_process.c:741-744` | `svt_av1_reset_cdf_symbol_counters(ec->fc)` then `((EbReferenceObject*)…)->frame_context = *ec->fc` | `FrameCdfs::reset_symbol_counters` + `ReferenceFrame::frame_cdfs` |
| `ec_process.c:101-112` | every tile of a frame with `primary_ref_frame != PRIMARY_REF_NONE` copies `ref->frame_context` into `ec->fc` | the entropy walk's per-tile seed |
| `md_config_process.c:299-310` | the same copy into `pcs->md_frame_context` | **NOT wired** — see below |

Four things about the C semantics that a natural port gets wrong:

* **The counter reset is not cosmetic.** `update_cdf` reads `cdf[nsymbs]` to
  choose the adaptation RATE, so a save that kept a frame's final counts makes
  the next frame adapt at the slow late-frame rate from its first symbol. And
  `nsymbs` is NOT always `len - 1`: `partition` (4 / 10 / 8 in a stride of 11),
  `uv_mode[0]` (13 in 15), `tx_size[0]` (2 in 4) and the ext-tx sets
  (7 / 5 / 16 / 12 / 2 in 17) all use a stride wider than their alphabet.
  Getting one wrong zeroes a PROBABILITY.
* **`svt_av1_default_coef_probs` is SKIPPED on the restore arm.** The
  coefficient CDFs come from the reference, NOT from this frame's own
  `base_q_idx` — even though the two frames' qindexes differ.
* **The DPB slot is `ref_frame_idx[primary_ref_frame]`.** C indexes its own
  `ref_pic_ptr_array[list][idx]` through `get_list_idx`/`get_ref_frame_idx`;
  the two agree here, and the spec mapping is the one conformance depends on.
* **C's save loop OVERWRITES per tile**, so the LAST tile's context is what
  lands on the reference object. Single-tile frames (every inter cell so far)
  make that tile 0, which is also the `context_update_tile_id` a decoder uses.
  They can only differ on a multi-tile frame.

**The store's POSITIVE CONTROL is a refusal.** A frame whose header names a
`primary_ref_frame` whose DPB slot carries no saved CDFs now refuses, rather
than falling back to the defaults — falling back emits a stream a conforming
decoder turns into garbage, and would read as a quiet byte regression rather
than a break.

#### A new oracle: C's SAVED frame context, dumped

`tools/capture_c_trace/wrap_recon.c` gained
`__wrap_svt_av1_reset_cdf_symbol_counters`, which dumps the FRAME_CONTEXT
**after** the real reset — i.e. byte-for-byte what C copies onto the reference
object. `SVTAV1_FCTX_OUT` makes the port emit the same field names in the same
flat order and `tools/fctx_diff.py` compares them.

**It needs no working inter tile walk**, which is the point: the saved state was
proven right before anything consumed it.

Result on `gradient 64x64 q40 p6 frames=2`: the port's saved end-of-frame-0
context is **byte-identical to C's for all 96 shared fields**. Four fields C
carries and the port does not — `delta_lf`, `delta_lf_multi`,
`palette_uv_size`, `palette_uv_color_index` — are identical between C's frame-0
and frame-1 saves on this cell, so the omission is inert here;
`c_parity_frame_cdf.rs` asserts that absent set is EXACTLY those four.

#### The tile is 94 9a b0, and the port writes exactly that

C's frame-1 tile is three bytes. Feeding C's OWN measured block decision through
the port's restored CDFs and `port_entropy_inter::write_inter_mode_info`
reproduces them exactly (`pipeline.rs`,
`inter_tile_byte_gate::the_inter_tile_matches_c_from_cs_measured_decision`).

The decision was MEASURED, not guessed from the bytes — a decision fitted to
three bytes would be curve fitting. A new `SVT_CINTER_OUT` dump prints, from
inside `svt_aom_update_mi_map`, the committed fields the writer reads:

```
CINTER poc=1 mi=(0,0) bsize=12 part=0 mode=16 rf=1,-1 mv0=0,-24 pmv0=0,0
       interp=0x0 mm=0 npr=0 ovl=0 imc=8 drl=0 drlctx=-1,-1 iiu=0 skip=1
```

**One 64x64 `PARTITION_NONE` block, `NEWMV` off `LAST_FRAME`, MV `(0,-24)`
eighth-pel** — the harness's 3-pixel horizontal translation — predicted from
`(0,0)`, `EIGHTTAP_REGULAR`, `skip = 1`, for the whole frame.

So for this cell, **the entire remaining divergence is MODE DECISION**. Every
entropy step behind it is byte-exact.

Two negative controls, both permanent: the same decision coded from DEFAULT
CDFs does NOT reproduce C's bytes (so the gate cannot pass with the restore
deleted), and frame 0 is asserted to be 961 B before anything is read out of it
(so a wrong configuration cannot hand the writer plausible CDFs from the wrong
encode). Checked by hand while writing it: MV `(0,-22)` gives `94 9a 94`.

#### A cheap symbol-level tile differ, with no op trace

Diffing a frame's SAVED context against the previous frame's names exactly
which CDFs its tile ADAPTED — i.e. which syntax elements it coded. That is a
symbol-level comparison of two tiles that needs no arithmetic-coder trace, and
therefore works on macOS (§5: Apple `ld64` has no `-Wl,--wrap`).

| | fields the frame-1 tile adapts |
|---|---|
| C | `partition` `skip` `intra_inter` **`comp_inter` `single_ref` `newmv` `switchable_interp` `nmvc.joints` `nmvc.comp1.{classes,bits,fp,sign}`** |
| port | `partition` `skip` `intra_inter` **`y_mode` `uv_mode` `angle_delta` `intra_ext_tx` `tx_size` `txb_skip` `dc_sign` `eob_extra` `eob_flag{64,128,256}` `coeff_base` `coeff_base_eob` `coeff_br`** |

Note what is absent on C's side and what it rules out: no `refmv`/`zeromv` (the
mode symbol chain stopped at `NEWMV`), no `drl` (one ref-MV candidate), no
`motion_mode` (`num_proj_ref = 0`, `overlappable_neighbors = 0` at the frame's
first block), no `tx_size` (`skip && is_inter`), and only `nmvc.comp1` — the
COLUMN component — because the MV's row is zero.

**A CORRECTION to the obvious reading of that table, and the reason the
absence of `nmvc` on the port's row is the interesting part.** "The port codes
everything intra" is WRONG. Instrumenting the pack walk on the same cell
counts **24 committed INTER leaves** on frame 1 (8x8 to 32x8, against C's one
64x64). They do not appear in the CDF delta because the homegrown inter arm
writes its MV through `entropy::mv_coding::write_mv`, which builds a **fresh
`NmvContext::default()` on every call** — so no MV symbol adapts the frame
context, and no MV after the first is coded against probabilities a decoder
holds. A CDF delta shows what a tile ADAPTED, which is not quite what it
CODED; when a writer bypasses the frame context, the two differ.

#### The homegrown inter arm is not a bitstream — four named defects

Reachable only under `SVTAV1_INTER_EXPERIMENTAL`. Recorded at the call site in
`pipeline.rs` as well, so the chunk that replaces it has the list:

1. **A fresh `NmvContext` per block** (above). A decoder desync, not a size
   difference.
2. **No `write_ref_frames`, no inter mode symbol, no DRL, no interp filter.**
   A decoder that reads `is_inter = 1` reads all of those *before* the MV, so
   it consumes the MV's bits as a reference index.
3. **`allow_hp = true` hard-coded**, while the frame header writes
   `allow_high_precision_mv = 0` (§1r).
4. **The MV is raw, not a difference from the MVP stack's predictor** —
   `inter_mvp.rs` is ported and unwired.

#### The homegrown ME misses an exact integer match by a quarter pel

Same instrumentation, same cell: every committed inter leaf carries
`mv.x = -22` eighth-pel, on content translated by exactly 3 pixels, where C
finds the integer `-24`. A sub-pel refinement that prefers a fractional
position over an exact integer match is evidence about the search, not about
rounding — and it is measured on the easiest possible input, a pure global
translation with no occlusion. Chunk C4 replaces this search wholesale; the
number is recorded so that replacement has a before.

#### What is next, in dependency order

1. **The inter branch of MODE DECISION.** It is now the only thing between the
   port and a byte-identical inter frame on this cell — the entropy path
   behind it is byte-exact. The islands
   (`inter_mvp.rs` 2,530 lines, `inter_mv_code.rs` 833, `inter_me/` ~4.7k,
   `port_md/`) are ported; what is missing is a pipeline that injects an inter
   candidate, prices it, and commits `is_inter` + `ref_frame` + `mv` +
   `interp_filters` + `skip` onto a `BlockDecision`.
2. **Wiring `write_inter_mode_info` into the tile walk itself.** The byte gate
   drives it directly; `pipeline.rs`'s block writer still has only the
   pre-campaign homegrown `write_mv` arm.
3. **`md_config_process.c`'s `md_frame_context` restore.** It changes MD RATE
   ESTIMATES, not syntax, and its call sites are inside `encode_tile_rows`,
   which runs before the reference binding is computed. It is meaningless until
   (1) exists — there are no inter candidates whose cost it could change — but
   it must land with (1), not after it.


#### An INVENTORY of what the inter-MD chunk actually needs (measured 2026-09-01)

Everything downstream of mode decision is ported and gated, so this list is the
chunk. Each line was checked against the tree, not inferred from the map above.

| # | what is missing | where it goes | what already exists |
|---|---|---|---|
| 1 | **BOTH C-exact MD paths are switched OFF on any frame with a reference.** `use_pd0 = ref_ctx.is_none() && …` (`pipeline.rs:9156`) and `use_funnel = … && ref_frame_data.is_none() && …` (`:8469`). An inter frame therefore bypasses the PD0 partition search AND the leaf funnel and runs the pre-campaign `partition::partition_search_with_config` / `encode_single_block` recursion — the code every video-KEY chunk of this campaign was built to replace. Flipping `use_funnel` alone changes nothing (MEASURED: same 94-byte tile, same head bytes), because `use_pd0` gates the branch that reaches it | `pipeline.rs:8469` + `:9156` | everything the KEY path uses |
| 1b | **an inter candidate inside the funnel** — `leaf_funnel/` names no reference plane, no ME and no inter mode anywhere; every MV it handles is an IntraBC DV | `leaf_funnel/inject.rs` + `types.rs` | the IntraBC injection path is the shape to copy |
| 2 | **an inter entry in the MD mi grid** | `leaf_funnel/commit.rs:42-65` | the grid is ALREADY stamped per commit as `intrabc_mvp::MvpMiEntry`, with `ref_frame: [0, -1]` hard-coded to `{INTRA_FRAME, NONE}`; and `inter_mvp::setup_ref_mv_list` reads exactly this type. It is allocated only when `ibc_state.is_some()` (`pipeline.rs:8804`), i.e. never on an inter frame |
| 3 | **the MVP stack call** to get `pred_mv` / `inter_mode_ctx` / `drl_ctx` | MD, per candidate | `inter_mvp::setup_ref_mv_list` is TIER-1 gated including `mode_context` (`c_parity_inter_mvp.rs:269`, randomized grids) — it needs wiring, not gating |
| 4 | **a PADDED reference view.** `port_pd_pred::RefPlane` takes `buf` + `origin` because C's reference pictures carry a replicated margin the MC indexes negatively into; `picture::ReferenceFrame` stores bare planes and `partition::RefFrameCtx` hands them over raw | the DPB / the per-frame reference setup | the padding PRIMITIVE is already ported and tier-1 gated — `port_preanalysis::generate_padding` vs `svt_aom_generate_padding` (`c_parity_preanalysis.rs:115`). What is missing is applying it to a DPB plane and handing out the `origin`/`stride` view |
| 5 | **the real reconstruction MC on the encoder's inter path.** `partition::generate_inter_pred` is a hand-rolled BILINEAR that also fills out-of-frame samples with **128** instead of replicating the edge | `partition.rs` | `port_convolve.rs` (the `_sr` + `jnt_` families, tier 1) and its drivers `port_pd_pred::av1_inter_prediction_{pd0,light_pd1}` / `port_enc_make_pred::enc_make_inter_predictor` |
| 6 | **chroma inter prediction.** `generate_inter_pred` is luma-only; an inter block's chroma still goes through `encode_chroma_block_dc`, i.e. INTRA DC. A stream that signals inter and reconstructs chroma from an intra DC predictor cannot match any decoder | `partition.rs` / the chroma pass | the drivers above take all three planes |
| 7 | **the inter payload on `BlockDecision`** (`ref_frame`, mode, `pred_mv`, `inter_mode_ctx`, `drl`, `interp_filters`, `motion_mode`, `num_proj_ref`, `overlappable_neighbors`) and the `write_inter_mode_info` call in the pack | `partition.rs` + `pipeline.rs`'s block writer | `port_entropy_inter::block::write_inter_mode_info`, now proven byte-exact end to end |
| 8 | `md_config_process.c`'s `md_frame_context` restore | `encode_tile_rows` | `crate::port_frame_cdf`, landed |

Items 4, 5 and 6 are **decoder-conformance** requirements, not RD ones: they
decide whether the encoder's recon equals what a decoder produces from the
bytes it wrote. Items 1, 2, 3 and 7 are what make the bytes exist at all.

**STATUS, updated in place as each landed. Every item in this table is now
closed** — 2 and 3 in §1u, 4 in §1v, 5's luma half in §1w, and 1, 1b, 5's
remaining half, 6 and 8 in **§1y**, which also records the ONE thing that
replaced them: C's `blk_skip_decision` fires in the port and disagrees with C
on the reference cell. A THIRD gate the table did not list is recorded there
too — `c_quant` was itself `if is_key`, so item 1's two `ref_*.is_none()`
terms could not have armed the funnel on an inter frame whatever they said.

**Item 1 is DE-RISKED but must land with 1b, measured 2026-09-01.** Taking
both gates off behind a temporary env var (since reverted) runs the C-exact
PD0 + leaf-funnel MD on the inter frame **without panicking**: frame 0 stays
at 961 B (the key path cannot see the change — the gates only differ when a
reference exists) and frame 1's tile goes 94 -> 119 B. It is LARGER because
the C-exact path has no inter candidate, so it codes the frame all-intra where
the legacy recursion at least inter-predicts 24 leaves. So item 1 is safe to
do and useless on its own: it has to land in the same change as 1b, and the
byte number to watch is not 119 but C's 3.

**Item 1 is the one that reorders the rest.** Until the two `ref_*.is_none()`
gates come off, work on the inter path lands in `partition.rs`'s legacy
recursion — code that item 1 then bypasses. That is why this chunk did NOT
swap `generate_inter_pred` for the real reconstruction convolve even though
the convolve is ported and tier-1 gated (item 5): the swap belongs in whatever
MD path survives item 1, and doing it in the legacy one first would be churn
with no gate able to see it. The order is 1 → (2, 3) → 7 → (4, 5, 6) → 8.

#### No regression, measured after both chunks

`identity_full_8bit.sh` **1100 / 1100**, `regression_spotcheck.sh` **65 / 65**,
`tools/inter_fh_gate.sh` PASS (frame 0 identical, frame 1 header field-exact
with an empty open set), the six pinned still cells unchanged, and
`cargo nextest run --workspace` **2429 / 2429** on aarch64 (2422 + 4
`c_parity_frame_cdf` + 2 `port_frame_cdf` units + the tile byte gate) and
**2439 / 2439** on x86-64 (`r7900x`, from `main@origin`), where
`regression_spotcheck.sh` is also **65 / 65** and `inter_fh_gate.sh` PASSes
with the same result (frame 0 identical, frame 1 header field-exact, empty
open set).

**The x86-64 run was not a formality — it caught a real defect.** The new
`frame_cdf_shims.c` re-hit the trap `entropy_inter_shims.c:107-118` had already
solved and commented: `svt_aom_init_mode_probs` / `svt_av1_default_coef_probs`
copy through `svt_memcpy`, an RTCD pointer that is NULL until
`svt_aom_setup_common_rtcd_internal` runs, and that NEON devirtualization makes
a direct call on aarch64. The two tests that call an initializer SIGSEGV'd on
x86 while the two using only the PAINTED modes passed — which is the
fingerprint of a NULL RTCD pointer rather than a buffer bug. Fixed with an
`fctx_ensure_rtcd()` at the entry point. **Grep the nearest existing shim
before writing a new one** (`docs/WORKING-ON-THIS.md` §5) — this is the fourth
lane to pay for that lesson.

### 1t. The port's OWN ME and MVP already produce C's decision — the gap is WIRING (2026-09-01)

§1s ends with an inventory and an order. Before wiring anything, this chunk
asked the question the inventory presumes an answer to: **when the ported
islands ARE reached with this cell's inputs, do they produce C's numbers?**
The answer is yes, for every field of C's decision except the ones that need a
prediction loop, and it was measured rather than argued.

Two permanent gates, `pipeline.rs::inter_decision_probe`:

| C's `SVT_CINTER_OUT` field | port's own machinery | agrees? |
|---|---|---|
| `mv0 = 0,-24` (eighth-pel) | `inter_me::motion_estimation_b64`, configured by `sig_deriv_me` | **yes** — full-pel `(-3, 0)`, SAD **0** |
| `pmv0 = 0,0` | `port_md::drl::choose_best_av1_mv_pred` over `inter_mvp::setup_ref_mv_list` | **yes** |
| `imc = 8` | `inter_mvp::setup_ref_mv_list`'s `mode_context` | **yes** |
| `drl = 0`, `drlctx = -1,-1` | `predicates::get_max_drl_index` == 1, so NO DRL symbol | **yes** |

So on this cell **no ported algorithm is wrong**. Every remaining byte is
§1s's items 1/1b/2/7 — connecting code that already exists.

**The homegrown ME's `-22` was not a search-quality gap either.** §1s recorded
`crate::motion_est` landing a quarter pel short of the exact integer match.
The wholesale port finds `-3` with SAD 0 on the same content, first try, at
both `frame_is_boosted` values (swept, because that is the one derivation input
this cell does not pin from a dump). C4 does not need to be *invented*; it
needs to be *called*.

**A conformance item turns out to be load-bearing for the DECISION.** §1s
classes item 4 (a padded reference view) as decoder-conformance rather than RD.
It is both. The harness builds frame 1 by translating right 3 px **with left-
edge replication**, so at MV `-3` the block's first three columns read OUTSIDE
the reference — and they match EXACTLY only against a replicated margin. With
a zero-filled or 128-filled margin the SAD is non-zero, the residual is
non-zero, and C's `skip = 1` is not reachable at all. The probe asserts the
margin replicates column 0 for exactly this reason. Item 4 is a prerequisite of
items 1b/7, not a follow-up to them.

#### Two lanes ported the same C structs, and the widths disagree

Wiring `sig_deriv_me` (`port_enc_mode_config::me`) to the search
(`inter_me::context`) needed a bridge, `me::apply_me_signals`, because seven C
structs — `SearchArea`, `SearchAreaMinMax`, `MeHmeRefPruneCtrls`, `MeSrCtrls`,
`Me8x8VarCtrls`, `MvBasedSearchAdj`, `PreHmeCtrls` — exist TWICE in the tree,
once per lane, with no conversion between them. Nothing called either lane, so
nothing forced them together.

Checked field by field against `me_context.h`: five fields differ in WIDTH from
C, and `inter_me` is the faithful side in all five.

| field | C | `port_enc_mode_config` | `inter_me` |
|---|---|---|---|
| `MeHmeRefPruneCtrls::enable_me_hme_ref_pruning` | `bool` | `u8` | `bool` |
| `MeHmeRefPruneCtrls::{zz,phme}_sad_pct` | `uint16_t` | `u32` | `u16` |
| `MeSrCtrls::stationary_hme_sad_abs_th` | `uint16_t` | `u32` | `u16` |
| `MeSrCtrls::reduce_me_sr_based_on_hme_sad_abs_th` | `uint16_t` | `u32` | `u16` |
| `MvBasedSearchAdj::sa_multiplier` | `uint16_t` | **`u8`** | `u16` |

MEASURED inert on every value the derivation can produce: `enc_mode_config.c`
assigns the `*_pct` pair only 0 or 5, the `*_abs_th` pair at most 24000 and
`sa_multiplier` only 2; the only post-switch arithmetic on the `*_abs_th` pair
is `/4` or `/16` (:513-522), and the QP `q_weight` rescale touches `me_sa`
alone (:338-342). The bridge's casts are therefore C's own assignment
truncation, and a `debug_assert` fires if a future preset row breaks that.

`PreHmeCtrls` differs in SHAPE too: C's `prehme_sa_cfg[2]` is an array in
`inter_me` and two named `_vert`/`_horz` fields in the config lane.

#### What this changes about §1s's order

Nothing about the order, one thing about its risk profile. §1s's `1 -> (2,3)
-> 7 -> (4,5,6) -> 8` stands, with item 4 promoted out of the "conformance
only" bucket (above). What it removes is the possibility that a ported island
is simply wrong: on this cell they are not, so any divergence found while
wiring is a wiring defect and should be hunted as one.

### 1u. The REAL pack walk writes C's inter tile — item 7 landed (2026-09-01)

§1s item 7's two halves are asymmetric and only one of them is churn. The MD
half (filling an inter payload) lands in whichever mode-decision path survives
item 1. The PACK half does not: `encode_block_syntax` is the block writer BOTH
MD paths feed, so replacing its arm is work item 1 cannot invalidate. This
chunk did the pack half, and it removes the conformance debt §1s named.

**Gate:** `pipeline.rs::inter_decision_probe::the_real_pack_walk_writes_cs_inter_tile`
— the reference cell's decision through `encode_block_syntax`, the function
the frame's entropy walk actually calls, producing **`94 9a b0`**, C's frame-1
tile. The older `inter_tile_byte_gate` drives `write_inter_mode_info` directly
with every field spelled out; this one hands the pack only what MODE DECISION
decides and derives the rest, so it additionally gates the frame-syntax
plumbing, the neighbour derivation and the mi grid.

#### The four defects are gone, and the arm that had them is deleted

`pipeline.rs`'s inter arm no longer calls `entropy::mv_coding::write_mv`. It
calls `port_entropy_inter::block::write_inter_mode_info`, which writes
`write_ref_frames`, the inter mode symbol, the DRL group and the interpolation
filter in C's order, differences the MV against `pred_mv`, and takes the
FRAME's single adapting `nmvc` at the header's own `allow_high_precision_mv`.
Defects 1-4 cannot recur: there is no second writer.

The arm REFUSES rather than falling back. `BlockDecision::is_inter` without
`BlockDecision::inter` panics with a message saying why — a quiet fallback
would turn an undecodable stream back into a byte divergence, which is the
failure mode that is hardest to see.

#### A split C does not make, for a reason C does not need

C caches `predmv`, `inter_mode_ctx` and `drl_ctx` on `BlkStruct` at MD time,
with the comment *"Store drl_ctx in blk to avoid storing final_ref_mv_stack
for EC"* (mode_decision.c:3708) — a caching decision. This port DERIVES them
in the pack (`EntropyCtx::inter_mvp_fields`) from the committed mode-info grid
and carries only MD's actual choices in `partition::InterDecision` (mode,
`ref_frame`, `mv`, `drl_index`, interp filters, motion mode, skip mode).

That is not a cosmetic rearrangement. The values are a pure function of the
reference-MV stack, which is a pure function of the mode-info map — the map a
DECODER rebuilds. Deriving them where the map is committed makes it
structurally impossible for an MD path whose own grid lags (the pre-campaign
recursion stamps no mi map at all) to write a context no decoder can
reproduce. The gate proves the move is byte-neutral on the reference cell: the
derived triple equals C's measured `pmv0 = 0,0`, `imc = 8`, `drlctx = -1,-1`.

#### A REAL defect the wiring exposed: `is_inter_block` is not `use_intrabc`

C's `av1_code_tx_size` picks its var-tx arm on `is_inter_block(mbmi)` =
`use_intrabc || ref_frame[0] > INTRA_FRAME` (block_structures.h:119). The pack
tested `use_intrabc` alone. While IntraBC was the only inter-CLASSIFIED block
this pack could emit the two predicates were the same; a genuinely inter block
made them differ, fell into the INTRA arm, and coded a `tx_size` depth symbol
C does not write.

MEASURED: the tile came out `94 9a 9e` against C's `94 9a b0` — one extra
3-symbol write at the end (`tx_size_cdf[2]`), with every symbol before it
identical in `nsyms`, `s`, `icdf` AND range. The same predicate was wrong at
`record_inter_dims`, the neighbour override `get_tx_size_context` reads. Both
are `use_intrabc || decision.is_inter` now.

This is the shape to expect from the rest of the campaign: the pack has ten
years of "the only inter-ish block here is IntraBC" baked into it, and each
one surfaces as exactly one extra or missing symbol.

#### Items 2 and 3, in the pack

* **Item 2 (the mi grid)** is `EntropyCtx::mvp_grid` — one `MvpMiEntry` per
  4x4 cell, allocated only on a frame that HAS references (`arm_inter_mvp`),
  stamped by every coded block from `record_inter_mi` alongside the
  above/left rows, with the real `ref_frame` / `mv` / `partition` instead of
  §1s's hard-coded `{INTRA_FRAME, NONE}`. Intra blocks inside a P frame are
  stamped too — they are neighbours the inter contexts read.
* **Item 3 (the MVP call)** is `inter_mvp::setup_ref_mv_list` +
  `get_av1_mv_pred_drl` + `port_md_winner::drl_contexts_for`, the last of
  which this chunk exposed: `winner_signals` wraps the same two loops in a
  `pd_pass == PD_PASS_1` gate that belongs to the mode-decision site, not to
  the arithmetic. Nothing was re-transcribed.

#### Measured byte movement, and what it is NOT

`SVTAV1_INTER_EXPERIMENTAL=1` on `gradient 64x64 q40 p6 frames=2`: frame 1
goes **113 B -> 90 B** against C's 22 B (frame 0 stays IDENTICAL at 961 B).

**That number is not progress toward parity and must not be read as such.**
The old 113 B was not a bitstream, so the two are not comparable as encodings;
the new 90 B is a real inter tile whose CONTENT is still wrong because the
decision feeding it is the pre-campaign one — 20-odd small leaves from the
homegrown ME at `mv.x = -22`, predicted through a bilinear MC, where C codes
one 64x64 block. Parity on this cell needs items 1/1b/4/5/6, not a smaller
number here.

#### A harness trap, paid for once

`regression_spotcheck.sh` reported `64 / 65` with `cropped-tx-72x88 [port
failed to encode]` while a `cargo build` and `identity_full_8bit.sh` were
running in parallel. The cell is fine: `tools/identity_run` re-checks
freshness on EVERY invocation, so a concurrent cargo job holds the build lock
and the run reports a port failure that looks exactly like a real encode
panic. Serially it is `65 / 65`. **Run the byte gates one at a time**, and
read "port failed to encode" as "check for a concurrent cargo" before reading
it as a regression.

#### What the MD half still owes

`partition::InterDecision` is filled by the legacy recursion with `NEWMV` off
`LAST_FRAME`, `drl_index` 0, `EIGHTTAP_REGULAR`, `SimpleTranslation` and no
projected or overlappable neighbours — which is exactly what that search
decides, one candidate against one reference with no filter search and no
motion-mode search. Every field is honest about an unported search rather
than a placeholder. A candidate that sets `interintra`, `compound` or a
warped `motion_mode` must EXTEND `InterDecision`; the pack passes `None` for
those groups and cannot invent them.

### 1v. The DPB reference carries C's margin — item 4 (2026-09-01)

§1s files a padded reference view under "decoder-conformance, not RD". §1t
measured that it is BOTH, and this chunk lands it.

C pads a recon before it becomes a reference: `pad_ref_and_set_flags`
(enc_dec_process.c:1072-1112) calls `svt_aom_generate_padding` on all three
planes with `scs->border = BLOCK_SIZE_64 + 4` = **68**
(Globals/enc_handle.c:4256), chroma at `(border + ss_x) >> ss_x`. AV1 clamps a
motion vector so the predicted block plus its filter taps stays inside the
frame PLUS that margin, and the MC then indexes NEGATIVE offsets from pixel
(0,0) — a reference stored without it cannot answer a legal MV at all.

The port stored bare planes and `partition::generate_inter_pred` filled every
out-of-frame sample with the constant **128**. That is a value no decoder
produces, and on this campaign's own cell it also makes C's decision
unreachable: the harness translates frame 1 right by 3 px WITH left-edge
replication, so at the correct MV `-3` the block's first three columns read
outside the reference and match EXACTLY only against a replicated margin.
Against a 128 fill the residual is large and `skip = 1` is impossible at any
quantizer.

`picture::PaddedPlane` / `PaddedRef` are built once per stored reference from
the already tier-1-gated `port_preanalysis::generate_padding`
(`c_parity_preanalysis.rs:115`), and `ReferenceFrame::padded` carries them
beside the bare planes — which stay, because every non-MC reader (TPL's SB qp
offsets, the open-loop ME's own pyramid) indexes them at frame stride.

**Gate:** `inter_decision_probe::the_dpb_reference_carries_cs_replicated_margin`
— the margin exists on the DPB slot at C's widths, replicates the edge in all
four directions AND at the corners (which `generate_padding` gets right by
replicating the already-padded first and last ROWS; a port that padded
vertically first leaves them zero), and the LIVE prediction path reads it. The
last is a positive control with teeth: it compares against the 128 the old
path produced, so a padded plane built but never consulted fails.

**MEASURED:** `SVTAV1_INTER_EXPERIMENTAL=1` frame 1 goes **90 B -> 75 B**
against C's 22 B. Same caveat as §1u — the content is still wrong, the
decision feeding it is still the pre-campaign one, and a smaller number is not
parity.

**What item 5 still owes.** The interpolation is still the homegrown BILINEAR;
`port_convolve`'s 8-tap family and its drivers
(`port_pd_pred::av1_inter_prediction_light_pd1`, which does luma AND both
chroma planes in one call, so it is item 6 as well) are ported and tier-1
gated but unwired. That swap deliberately did NOT happen here: per §1s it
belongs in whatever MD path survives item 1, and the padded reference it needs
is exactly what this chunk provides. `generate_inter_pred` now REFUSES
(returns the untouched buffer) when no padded reference is supplied rather
than falling back to the fill it replaced.

### A second harness trap: EDITING SOURCE DURING A GATE RUN

`identity_full_8bit.sh` reported `1091 / 1100` with the LAST NINE cells
(`screen 512 512 48 p5..p13`) as `RS_ERR`. They pass individually. The cause
is the same mechanism as §1u's concurrent-cargo note, from the other side:
`tools/identity_run` rebuilds on EVERY invocation, so a source edit made while
the sweep is running turns into a build failure mid-run, and the harness
records it as "the port failed to encode" — which reads exactly like a crash
in those cells. Do not edit the tree while a byte sweep is running; the
position of the failures (a contiguous tail) is the tell.

### 1w. C's 8-tap convolve replaces the homegrown bilinear — item 5's luma half (2026-09-01)

`svtav1_dsp::port_pd_pred::av1_inter_prediction_light_pd1` and the
`port_convolve` family under it have been ported and tier-1 gated for some
time, and **nothing in the encoder called them**. `crate::inter_pred_arm` is
the adapter that finally does: it turns a block origin, a block size, a padded
reference and an eighth-pel MV into the `BlkGeom` / `RefPlane` / `MbEdges` /
`ScaleFactors` set the driver takes.

**The adapter is not churn; only its call site is.** §1s warns that work
landed in the pre-campaign recursion is thrown away the moment item 1's gates
come off. That applies to the CALL, not to the conversion — every MD path
needs exactly this `MbEdges` derivation and exactly these `RefPlane`s. So the
adapter is its own module and `partition::generate_inter_pred` is one call
into it.

**MEASURED, and the number goes the "wrong" way: frame 1 is 75 B -> 85 B**
against C's 22 B. That is not a regression, and reading it as one is the trap
this paragraph exists to prevent. The 75 B stream's luma recon came from a
BILINEAR no decoder has, so it was never a stream a decoder could reproduce;
the 85 B one's luma prediction is C's. It got bigger because the homegrown ME
lands on the quarter-pel `mv.x = -22` (§1s) where C finds the integer `-24`,
and a fractional position that a 2-tap average smoothed into a decent match is
a poor one under an 8-tap filter with negative taps. **That is evidence about
the SEARCH, not about the convolve** — and it is the second independent
measurement pointing at C4's call site (§1t was the first: the ported
`inter_me` finds `-3` with SAD 0 on the same content).

**The positive control is shaped for exactly this.** Asserting "the bytes
changed" would pass for any bug. The gate asserts that a HALF-PEL prediction
leaves the interval bounded by its two neighbouring samples: a 2-tap average
can never do that, an 8-tap filter with negative taps routinely does. So the
test can tell "an 8-tap convolve ran" from "something else changed".

**Item 6 is still open, and it is now the largest remaining conformance gap.**
An inter block's chroma still goes through `encode_chroma_block_dc`, i.e. an
INTRA DC predictor, while the bitstream says the block is inter — so a decoder
predicts chroma with the MC and the port's recon does not match it. The driver
needs one extra `component_mask` and the two `PaddedPlane`s from
`picture::PaddedRef::uv` (both already built and gated); what it does NOT have
is a chroma pass willing to take an inter prediction, because `ChromaPass`
routes every block through the intra path. `inter_pred_arm` deliberately does
NOT expose a chroma entry point yet: surface with no caller has no positive
control.

### 1x. The port emits a DECODABLE inter frame — six defects a byte gate cannot see (2026-09-01)

Every gate in this repo compares the port's bytes to C's. None of them asks
whether the port's OWN bytes are a bitstream. Running `dav1d` on the
experimental 2-frame stream asked that for the first time, and the answer was
no: frame 0 decoded, frame 1 came back `Invalid argument` (`aomdec`: "Failed
to decode tile data"). **The C stream decodes both frames**, which is the
control that makes this a port defect rather than a harness one.

Six defects, found one at a time by fixing and re-decoding. Five are the SAME
mistake:

| # | defect | why no byte gate saw it |
|---|---|---|
| 1 | `write_is_inter` passed a CONSTANT context 0 where C computes `svt_av1_get_intra_inter_context` — a FOUR-valued context off the neighbours' `is_inter_block` | inert while no inter frame could reach the pack; the port's own bytes were self-consistent, just not decodable |
| 2 | an INTER block wrote the intra **`uv_mode`** symbol. In C the whole chroma mode-info slice lives inside `write_modes_b`'s intra branch (:5199-5215) | it was guarded by `debug_assert!(!decision.is_inter, "420 path is key/intra only")` — and `identity_run` builds RELEASE, where the assert is compiled out, so the extra symbol was written SILENTLY |
| 3 | `av1_code_tx_size` picked its var-tx arm on `use_intrabc` instead of `is_inter_block` = `use_intrabc \|\| ref_frame[0] > INTRA_FRAME` (block_structures.h:119), coding a `tx_size` depth symbol C omits | see §1u — this one WAS byte-visible, as `94 9a 9e` vs `94 9a b0` |
| 4 | the luma coefficient writer picked the tx-type CDF ROWS the same wrong way, at **two** call sites (`tx_depth == 0` and `> 0`) | the two arms are 60 lines apart and only one was found by the tile gate |
| 5 | the chroma tx type followed `uv_mode` instead of the luma type on an inter block | chroma codes no tx-type symbol of its own — the type only selects the SCAN ORDER, so it is invisible until the levels come out in the wrong places |
| 6 | the mi grid stamped `intra_mode` (0 = `DC_PRED`) as an inter neighbour's `mode` | `setup_ref_mv_list` counts `have_newmv_in_inter_mode` over it into `newmv_count`, which moves `mode_context` — the CDF ROW, not the symbol |

**Four of the six are one predicate.** `use_intrabc` and `is_inter_block` were
the same thing for as long as IntraBC was the only inter-classified block this
pack could emit. Expect more: grep `use_intrabc` at every site that is really
asking "is this block inter".

**`SVTAV1_INTERDBG=1`** was added for the hunt and kept: it prints the
per-block inter decision AS THE WRITER SEES IT — including the three fields
derived in the pack (`pred_mv`, `inter_mode_ctx`, `drl_ctx`) and the neighbour
pair their contexts read. `SVTAV1_PACKTREE` could not do this: it prints
`intra_mode`, which an inter block leaves at 0, so an inter leaf was
indistinguishable from a DC intra one. `SVTAV1_PACKTREE`'s `PDV` line now
carries `inter=/mvr=/mvc=/rf=/mode=` for the same reason.

#### A new gate: `tools/inter_decode_gate.sh` (evidence tier 3)

Three cells must decode COMPLETELY; two known-open cells are listed with the
measured reason, so the gate states a frontier instead of hiding one. Moving a
cell from `OPEN_CELLS` to `PASS_CELLS` is how it records progress; a `PASS`
cell regressing fails it.

**Its anti-vacuity was checked by reverting fixes, and the result is honest
rather than flattering.** Restoring defect 2 (the `uv_mode` leak) fails 2 of
the 3 required cells. Restoring defect 1 (the constant `intra_inter` context)
does NOT fail any of them — every passing cell has a single block, whose
context is 0 either way. So this gate witnesses the leak; the neighbour-context
fixes are not yet covered by any cell, and will be exactly when the open cells
below are closed.

#### The remaining defect, with its minimal reproduction

`gradient 16 16 44 6` frames=2 `SVTAV1_FRAME_SHIFT=1` is **TWO 8x16 all-skip
NEWMV blocks off LAST_FRAME**, and it does not decode. Everything narrowed:

* **ONE block decodes.** The same cell at q50 is a single 8x16 block and
  decodes 2/2. So it is neighbour-dependent.
* **It is not the residual.** Both blocks are `skip`, with no luma or chroma
  coefficients at all.
* **It is not the MV coding.** Forcing the coded MV to equal `pred_mv` (a zero
  difference on every block) still fails.
* **It is not interintra or motion-mode.** Forcing
  `enable_interintra_compound = false` and `is_motion_mode_switchable = false`
  each still fail.
* **It is inter-specific.** Forcing the same frame ALL-INTRA (the inter
  candidate never wins) decodes 2/2.
* The frame header is field-exact with C's (`fh_fields.py`), and the sequence
  header's tool bits match C's (`sh_fields.py`: `enable_interintra_compound 1`,
  `enable_masked_compound 0`, `enable_jnt_comp 0`, `enable_dual_filter 0`,
  `enable_ref_frame_mvs 1`).

So the defect is a NEIGHBOUR-DEPENDENT context inside the inter mode-info
group. `SVTAV1_INTERDBG` on the failing cell prints, for the second block:
`imc=58 pmv=(0,0) drl_ctx=[-1,-1] nbL=(mode 16, rf [1,-1], interp 0)` — each of
which matches a hand-derivation from the spec, so the next chunk's search space
is the three remaining neighbour-driven contexts:
`collect_neighbors_ref_counts` -> `write_ref_frames`'s comp/single-ref
contexts, `av1_get_pred_context_switchable_interp`, and `mode_context` itself.

#### A frame-header finding, from widening the cell

**Still open as of §1y** — nothing in the inter-MD chunk touched the CDEF
derivation, and re-measuring it was not attempted.

`inter_fh_gate.sh` covers ONE cell. On `uniform 64x64 q40 p6 frames=2` the
frame-1 header differs from C's in `cdef_damping_minus_3` (C 1, port 0) — a
CDEF derivation gap on an inter frame that the gradient cell happens not to
expose. Recorded rather than gated: adding a knowingly-diverging cell to a
green gate would make it red without adding information the gate can act on.

### 1y. The port's OWN C-exact MD path produces C's BLOCK — items 1, 1b, 6 and 8 (2026-09-01)

§1s item 1 said both C-exact mode-decision paths were switched off on any
frame with a reference, and that item 1 was useless without 1b. Both landed
together, with 6 and 8, and the reference cell's frame-1 leaf is now decided
by the port's own PD0 + leaf funnel:

```
PTREE mi=(0,0) bsize=12 part=0 ...      # 64x64, PARTITION_NONE
PDV   mi=(0,0) inter=1 mvr=0 mvc=-24 rf=1 mode=16
```

against C's measured decision (`SVT_CINTER_OUT`, §1s):

```
CINTER poc=1 mi=(0,0) bsize=12 part=0 mode=16 rf=1,-1 mv0=0,-24 ... skip=1
```

Same block size, same partition, same mode, same reference, same MV. The one
field that still differs is `skip`, and that is the whole of the remaining
byte gap — see "the residual" below.

#### A THIRD gate nobody had named, and it is why item 1 alone measured nothing

`use_funnel` is `chroma_420 && chroma_src && ref_frame_data.is_none() &&
c_quant.is_some()`, and **`c_quant` was itself `if is_key`**. So taking the
two `ref_*.is_none()` terms off could not have armed the funnel on an inter
frame no matter what: the third term was already false. §1s's measurement of
item 1 ("frame 1's tile goes 94 -> 119 B") was real, but it measured the PD0
half only — the funnel never ran on that frame either before or after.

The inter arm of `c_quant` is C's `derive_inter_coeff_level`
(md_config_process.c:650) plus C's non-key MD lambda:

* the coeff level keys on `ppcs->norm_me_dist`, the MEAN of the open-loop ME's
  per-b64 8x8 distortion (`initial_rc_process.c:718-726`) — so it cannot be
  derived before the motion search, which is why the search moved above it;
* the thresholds are the INTER set `{5833/96, 5833/48, 16666/48}` with C's
  INTEGER division = `{60, 121, 347}` (definitions.h:279-281), NOT the intra
  `{25, 50, 150}`; `quant::inter_coeff_level_tests` pins the truncation
  because getting it wrong moves a band boundary by one, which is a different
  RDOQ level for a whole frame;
* the lambda is `crate::pd0::inter_full_lambda_8bit` — C
  `compute_rd_mult_based_on_qindex` + `update_lambda` (rc_process.c:365-449).
  It differs from the KF builder in exactly two frame-type switches: the base
  multiplier (`def_kf_rd_multiplier` 3.3, `def_arf_` 3.25, `def_inter_` 3.2)
  and the `rd_frame_type_factor` row. A flat low-delay P GOP puts every frame
  at temporal layer 0, which `update_lambda` (:406-410) maps to `ARF_UPDATE`.

Note what C does NOT do: a VIDEO-mode KEY frame takes NEITHER coeff-level arm
(`md_config_process.c:898-903` needs `allintra` for the intra one and
`slice_type != I_SLICE` for the inter one) and keeps `INVALID_LVL`. The video
RDOQ ladder ignores `coeff_lvl` up to M10, which is how C gets away with it.

#### Item 1b — the candidate, and the class it belongs to

`crate::inter_md_arm::build_inter_candidate` is the caller every ported island
was waiting for: `inter_me_arm` for the MV, `inter_mvp::setup_ref_mv_list` +
`port_md::drl::choose_best_av1_mv_pred` for the predictor and the DRL choice,
`inter_pred_arm::predict_inter_yuv` for the prediction, and
`port_rd_cost::inter_cost::inter_fast_cost` — C's real
`svt_aom_inter_fast_cost` — for the MDS0 rate. Nothing was re-transcribed.

**Stated as a fraction, MISSING first** (`docs/WORKING-ON-THIS.md`): no
compound candidate, no NEAREST/NEAR/GLOBAL, no second reference, no
interpolation-filter search, no motion-mode search (no OBMC, no warp), no
inter-intra, no predictive-ME refinement, no sub-pel refinement, no
`skip_mode`. C's `inject_inter_candidates` (mode_decision.c:2264) builds all
of those. What IS here is the ONE candidate C commits on this cell.

**It gets its own NIC lane.** C classes an inter NEWMV candidate
`CAND_CLASS_1` (definitions.h:787-794) and prunes every class against its OWN
best fast cost. Putting the inter candidate in lane 0 would have reproduced
the palette defect §"#71 palette calibration" records from the other side: an
inter candidate on a well-predicted block has a fast cost far below any intra
mode's, so it would prune out every regular candidate before MDS1. `nic.rs`
is now `LANES = 4` in C's class order (C0, C1, C3, C4) rather than three
hard-coded segments; `CAND_CLASS_2` (inter NEAREST/NEAR) has no candidate to
put in it and is deliberately absent rather than repurposed.

#### Item 6 — chroma is MOTION COMPENSATED, and it is ONE call

§1w left an inter block's chroma going through `encode_chroma_block_dc`, an
INTRA DC predictor, while the bitstream said the block was inter. C's driver
does luma and both chroma planes in a single
`av1_inter_prediction_light_pd1` under a `component_mask`, and the chroma arm
REUSES the luma block's `compute_subpel_params` result at a halved origin
(`port_pd_pred.rs:302-306`) — so predicting chroma separately would be
different arithmetic, not a refactor. `inter_pred_arm::predict_inter_yuv` is
that one call; the prediction is produced at injection and carried on the
candidate, and MDS3's chroma loop runs it through the same `tx_unit` the
IntraBC arm uses, with the same INTER chroma tx-type rule.

The bd10 chroma loop has no inter arm and REFUSES rather than falling back to
the intra predictor — scoring chroma from a prediction the stream does not
describe would decide the block on a fiction.

#### Item 8 — `md_frame_context`, which is a RATE change and was measurable

C's `init_frame_rate_tables` (md_config_process.c:292-310) seeds
`md_frame_context` from the primary reference's SAVED end-of-frame CDFs
whenever the header names one, and only otherwise from
`svt_av1_default_coef_probs(base_q_idx)` + `svt_aom_init_mode_probs`. Both the
intra rate tables (`build_md_rates`) and the new inter ones
(`InterFacBits::from_cdfs` + `RefFrameFacBits::from_cdfs`) now come from it.

Three of `RefFrameFacBits`' six rows and most of `InterFacBits` are built from
`port_entropy_inter::InterCdfs` rather than `FrameContext`, and that is not a
convenience: `FrameContext`'s `newmv` / `zeromv` / `refmv` / `drl` /
`skip_mode` / `interp_filter` fields are UNIFORM PLACEHOLDERS (documented at
`port_entropy_inter/cdfs.rs:14`), and pricing against a placeholder gives
every inter mode the same rate.

MEASURED on the reference cell: it moves the luma coefficient rate estimate
from 66787 to 66968 and `skip_fac_bits` from `{26, 2510}` to `{20, 2699}` —
small, because frame 0's CDFs have barely moved off the defaults on a
single-superblock frame. It is landed because C does it, not because it paid
here; on a frame with more superblocks the gap is not small.

#### C's `blk_skip_decision` is ported, and it is the residual

C gives an INTER block an explicit RD comparison between coding its residual
and signalling `skip` (rd_cost.c:1371-1406), gated on
`is_inter_mode(cand->block_mi.mode)` — the MODE, so IntraBC is excluded — and
on `ctx->blk_skip_decision`, which is `uv_ctrls.uv_mode <= CHROMA_MODE_1`
(enc_mode_config.c:7858), i.e. on exactly when MD evaluated chroma. An intra
block gets no such comparison, which is why the funnel had no counterpart.

It is implemented in `mds3.rs` and it FIRES — and on this cell it picks
CODING where C picks SKIP. The numbers, measured:

| | rate | distortion (`sse << 4`) | cost |
|---|--:|--:|--:|
| code the residual | 66 968 + 125 + 125 + 102 + 20 | 1 091 744 | 171 939 117 |
| skip | 2 699 | 1 474 512 | 190 027 953 |

`rdcost` is C's `((rate * lambda + 256) >> 9) + (dist << 7)` with
`lambda = 244 792`. Coding reduces the luma SSE from 90 965 to 67 042 for
~131 bits, and at this lambda that trade wins by 18.1 M. For skip to win the
lambda would have to exceed ~388 224, i.e. 1.59x — so this is NOT a near-tie
that a rounding difference explains.

**What that rules out.** The lambda is C's own chain and its inputs
(`lambda_scale_factors` defaults to 128 at `enc_settings.c:1021`, so the final
scale is a no-op; `lambda_weight` is 150 at `picture_qp >= 16` on BOTH the
allintra arm at `:10103` and the video arm at `:9456`). The rate estimate is
corroborated by the coded stream: the port's frame-1 tile is 18 bytes = 144
bits, of which the mode info is ~20, and the estimate says 131.

**And the dump answers the rest. The port's INTER PREDICTION is 3.4x worse
than C's, and that is the whole of it.** One `SVT_FULLCOST_OUT` run in the
Linux container (§5: Apple `ld64` has no `-Wl,--wrap`, so the macOS
`capture_c_trace` is the byte-only driver and NONE of the `SVT_*_OUT`
interposers exist there) on the reference cell prints, for the winning
candidate:

```
CFULL org=(0,0) 64x64 st=3 mode=16 fi=0 ang=0 uv=0 ibc=0 ycb=1519 ydist=426160 cost=60964418
```

`ycb = 1519` is about THREE bits — C's residual quantizes to nothing, so C
does not reach the skip comparison at all; it simply has no coefficients.
`ydist = 426160` is `y_distortion[DIST_SSD][0]`, which the skip arm has
already overwritten with `[1]`, so it is C's PREDICTION distortion:
`426160 / 16 = 26 635`.

The port's is **90 965**. And 26 635 is not an opaque C number — it is
reproducible from the DECODER, which is the ground truth here:

| quantity | value |
|---|--:|
| `sse(frame-0 source, dav1d's recon of frame 0)` | 26 658 |
| `sse(frame-1 source, dav1d's recon0 shifted -3 with left replication)` | **26 635** |
| C's `ydist / 16` | **26 635** |
| `sse(frame-1 source, dav1d's recon of frame 1)` | 26 635 |
| the port's MD prediction distortion | 90 965 |

So the correct MV against the correct reference gives EXACTLY C's number, and
the port's mode decision is scoring a prediction 3.4x worse. Four gross
hypotheses were ruled out by computing them against the decoder's recon0:
zero MV (1 726 660), the wrong sign (2 889 368), a 128 fill instead of the
replicated margin (1 074 664), a full-pel-vs-eighth-pel unit bug
(1 135 799) — none is 90 965, and all are far worse, so the MV, its
direction, its units and the margin are all right.

**What that leaves is the REFERENCE BUFFER itself: the port's stored recon of
the video-mode KEY frame is not what a decoder reconstructs from the same
bytes.** That is an encoder/decoder recon MISMATCH — the class
`rust/CLAUDE.md`'s issue-#15 note calls out as worse than a byte divergence —
and no gate covers it, because `recon_parity` and `decode_conformance` run the
STILL path and the byte gates compare bytes, not pixels. The video arm's
deblock / CDEF / SGR-LR chain is new as of §1n-§1o' and has never been
compared against a decoder's output on the frame it produces.

**The next chunk is therefore a RECON gate, not an RD investigation**: dump
the port's per-frame final recon on a video-mode cell (`identity_run` returns
before its recon dumps on any multi-frame run today), compare it against
`dav1d`'s decode of the port's own bytes, and localise which filter stage
diverges. Do that before touching the skip decision — the skip decision is
arithmetically C's and is being fed a wrong number.

**CORRECTED by §1z, same day: the last sentence is right and the reading that
led to it is WRONG.** The recon gate was built and it proved the recon
INNOCENT — the port's stored recon0 is byte-identical to dav1d's, and the
port's prediction is byte-identical to the ideal shifted copy. The wrong
number was the SOURCE: the port was temporally filtering it. §1z has the
chain.

#### Measured byte movement, with the same caveat as §1u and §1v

`gradient 64x64 q40 p6 frames=2`, `SVTAV1_INTER_EXPERIMENTAL=1`:

| state | frame 1 |
|---|--:|
| the pre-campaign recursion (this chunk's `before`) | 74 B |
| the C-exact MD path with NO inter candidate (item 1 alone) | 139 B |
| the C-exact MD path WITH the inter candidate | **37 B** |
| C | 22 B |

Frame 0 is IDENTICAL at 961 B in every row — the two gates item 1 removed can
only differ on a frame that HAS a reference, so the key path cannot see them.
Of the port's 37 bytes, 19 are the frame header, which is byte-identical to
C's; the tile is 18 B against C's 3 B, and every one of those 15 bytes is the
skip decision above.

**A smaller number is still not parity.** What makes this chunk a milestone is
not 37 < 74; it is that the block the port DECIDES is now C's block, decided
by the C-exact path, with the tile writer behind it already proven byte-exact
(§1u).

### 1z. The FIRST byte-identical INTER frame (2026-09-01) — the MD source was TEMPORALLY FILTERED

§1y closed with "the open question is the distortion pair, and it needs one
`SVT_FULLCOST_OUT` dump". The dump was taken, and the answer was not where §1y
predicted it would be.

`tools/identity_diff_inter.sh 64 64 40 6 2 gradient`:

```
frame 0: IDENTICAL (961 B)
frame 1: IDENTICAL (22 B)
```

and the tile the port's own pack writes is `94 9a b0` — C's.

#### The root is ONE predicate, and no byte count could have found it

`encode_frame_impl` ran `crate::temporal_filter` over the SOURCE of every
non-key frame at preset <= 12 (`speed_config.enable_temporal_filter`, a
homegrown ladder). MODE DECISION therefore scored a byte-EXACT motion
compensation against a picture no decoder will ever see.

C `derive_tf_params` (`Globals/enc_handle.c:3333`) says NO for every cell this
port can encode, twice over:

* `pred_structure == LOW_DELAY` sets `tf_level = 0` and RETURNS
  (`:3339-3343`); C's own comment on that line is *"TF disabled for all LD"*.
* Outside LD, `do_tf` still needs `hierarchical_levels >= 1` (`:3336`), and
  this campaign's GOP is flat.

`derive_pic_params` constructs `PredStructure::LowDelay` for every picture, so
the answer is a constant — but the gate is spelled as C's predicate so a
future non-LD structure has to change the line rather than inherit a `false`.

#### The measurement chain, because the diagnosis was wrong twice first

| step | number |
|---|--:|
| C `SVT_FULLCOST_OUT`, frame-1 winner: `ycb` | 1 519 (about THREE bits — C's residual quantizes to nothing) |
| C `ydist` (= `y_distortion[DIST_SSD][1]` after the skip arm's overwrite) | 426 160, i.e. **26 635** per `/16` |
| `sse(frame-1 source, dav1d's recon0 shifted -3, left-replicated)` | **26 635** |
| the port's MD prediction distortion | 90 965 |
| the port's PREDICTION vs that ideal shifted copy | **0 — byte-identical** |
| the port's stored recon0 vs dav1d's recon0 | **0 — byte-identical** |

The last two rows are what turned the diagnosis around. §1y read the 90 965 as
an encoder/decoder RECON mismatch and wrote that the next chunk should be a
recon gate; the recon gate was built, and it proved the recon INNOCENT. With
the prediction and the reference both exact and the distortion still 3.4x too
big, the only remaining term is the SOURCE.

Four gross MC hypotheses were ruled out numerically before any of that, each
computed against the decoder's own recon0: zero MV (1 726 660), the wrong sign
(2 889 368), a 128 fill instead of the replicated margin (1 074 664), a
full-pel-vs-eighth-pel unit bug (1 135 799). None is 90 965 and all are far
worse, which is how the MV, its direction, its units and the margin were
cleared in one step.

**Method note worth keeping.** The interposer prints `ydist` AFTER
`svt_aom_full_cost` has run, and C's skip arm overwrites
`y_distortion[DIST_SSD][0]` with `[1]` — so on a block C skips, the field
labelled "the coded distortion" is the PREDICTION distortion. That is what
made it comparable to a quantity reproducible outside both encoders.

#### `identity_run` gained the dump that made this answerable

The multi-frame path returned before every recon dump, so "is the port's
reference what a decoder reconstructs?" had no answer at all.
`SVTAV1_FINAL_RECON=<path>` now writes `<path>.f<i>` per frame on that path
too. It is byte-INERT: `recon_output` reaches the encode only through
`postfilter_consumed`, which a multi-frame run already forces true because a
later frame may predict from the recon.

#### The frontier, stated

96 cells (`{uniform, gradient, diag, screen}` x `{16, 64, 72, 128}` x
`{q20, q40, q55}` x `{p6, p8}`, all `frames=2` low-delay P):

| result | cells |
|---|--:|
| BOTH frames byte-identical | **6** — `gradient` 64 and 16 at q40/q55 p6, `screen` 16 at q20/q40 p6 |
| frame 0 identical, frame 1 differs | 78 |
| frame 0 already differs (the video-KEY frontier, not this chunk's) | 12 |

A new gate locks it in: **`tools/inter_byte_gate.sh`**, six PASS cells and
three named open ones, same OPEN -> PASS progress shape as
`inter_decode_gate.sh`. Its teeth were measured rather than asserted:
reverting the temporal-filter gate fails **2 of the 6** — `gradient 64x64 q40
p6` and `gradient 16x16 q40 p6`. The other four stay green because at q55, and
on `screen` 16x16, the filtered source still quantizes to the same decision.
That is the honest number, and it is written into the script so nobody has to
re-derive it.

Two shapes in the residual are worth naming because they are different
problems:

* **`uniform` at 16/64, and `screen`/`gradient` 16 at some qp: C's LENGTH
  exactly, differing at byte 15.** Byte 15 is inside the frame HEADER. That is
  §1x's recorded `cdef_damping_minus_3` finding (C 1, port 0) finally showing
  up as a byte rather than as a field-walk note. **CLOSED the same day — see
  §1z' below.**
* **preset 8 diverges far more widely than preset 6** (873 B against C's 22 on
  `gradient 64 q20 p8`). The video arm's p8 mode-decision ladder is a separate
  frontier from p6's and has never been exercised on an inter frame.

### 1z′. §1x's `cdef_damping_minus_3` field CLOSED — it is an unsigned underflow in C (2026-09-01)

The frame-1 header field §1x recorded as open (`C 1, port 0` on `uniform
64x64 q40 p6`) is now field-exact. It was NOT a derivation the port was
missing; it is C reading a field its own pick never assigned.

`frm_hdr->cdef_params.cdef_damping` is initialised to 0
(`resource_coordination_process.c:423`) and assigned ONLY inside
`finish_cdef_search`. When `pcs->ppcs->cdef_level == 0` that function is never
called — `cdef_process.c:683-696` takes the `else` arm, which zeroes
`cdef_bits` and both strengths and leaves `cdef_damping` alone. The header
writer runs anyway (it is gated only on `!coded_lossless && !allow_intrabc &&
enable_cdef`) and emits `cdef_damping - 3` as a 2-BIT literal
(`entropy_coding.c:2349`): `(uint8_t)0 - 3` promotes to `-3`, whose low two
bits are **1**.

**The disambiguating measurement, because two explanations fit one data
point.** `3 + (base_q_idx >> 6) - 3` also equals 1 when `base_q_idx` is in
64..127, and §1x's single cell was q40. Re-running `uniform 64x64 p6
frames=2` at three quantizers whose `base_q_idx >> 6` are 1, 2 and 3
(`base_q_idx` 80 / 160 / 220) settles it: C writes **1 at all three**. The qp
derivation is refuted; the underflow is not.

It is reachable on an inter frame through `update_cdef_filters_on_ref_info`
(`md_config_process.c:713-758`), which sets `cdef_level = 0` when the
reference's own strengths were all zero — which is every `uniform` cell. In
the still envelope C's allintra `cdef_level` is nonzero at every representable
preset, which is why the still gates never saw it.

Ported as `cdef::CdefFrameParams::never_picked()` (damping 0, i.e. C's
never-assigned value) plus a writer that emits `damping.wrapping_sub(3) & 3`
— reproducing the underflow explicitly instead of underflowing in debug and
silently differing in release. Filed as `docs/SUSPECTED-C-BUGS.md` #27.

**AFTER: `uniform 64x64 q20/q40/q55 p6` frame-1 headers are field-exact and
the remaining difference is the TILE — 2 bytes each way, C `93 a0` vs port
`91 34`.** And C's own decision there names the next gap outright:

```
CINTER poc=1 mi=(0,0) bsize=12 part=0 mode=13 rf=1,-1 mv0=0,0 ... skip=1
```

`mode = 13` is `NEARESTMV`, and §1y's own MISSING list says this port injects
no NEAREST/NEAR/GLOBAL candidate at all — only `NEWMV`. On flat content
NEARESTMV codes no MV, so it is strictly cheaper than the NEWMV the port
injects at the same `(0,0)`; C's `mv_is_already_injected` even suppresses the
NEWMV duplicate. The port picked an INTRA DC block instead.

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

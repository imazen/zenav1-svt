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

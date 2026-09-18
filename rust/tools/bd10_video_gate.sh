#!/usr/bin/env bash
# 10-BIT VIDEO: two-frame differential against C on real public-domain clips.
#
# WHY THIS GATE EXISTS. Until 2026-09-10 a 10-bit INTER frame was not merely
# non-identical, it was UNREACHABLE, in three stacked ways:
#
#   1. `try_encode_frame_420_hbd` refused every non-key frame outright, and the
#      refusal had no 10-bit counterpart in the 8-bit differential harness, so
#      the surface could not be measured at all.
#   2. Past that, `tx_unit_hbd` indexed a zero-length slice: an inter
#      candidate's `Cand::pred10` had NO producer. The bd10 full-RD funnel
#      residuals every candidate against a 10-bit prediction, an intra
#      candidate got one from `predict_unit_hbd`, and an inter candidate got
#      nothing, because the DPB stored only 8-bit reference planes.
#   3. Past THAT, the bd10 chroma full loop was a literal `panic!` saying it
#      "has no INTER arm".
#
# All three are closed: the DPB carries a 10-bit twin of each reference
# (`PaddedRef::hbd`), `av1_inter_prediction_light_pd1_hbd` produces the 10-bit
# luma AND chroma prediction in one call, and `chroma::eval_uv_inter_hbd`
# scores it. Every cell below now ENCODES and DECODES.
#
# WHAT IT DOES NOT CLAIM. It is NOT byte-identical to C. The table pins the
# measurement exactly as `real_video_inter_gate.sh` does, so neither a
# regression nor a silent improvement passes unnoticed, and the honest state is
# in the repo rather than in a commit message:
#
#     39 of 48 frames byte-identical, 24 of 24 streams decodable (2026-09-19)
#
# WHAT "DECODES" MEANS HERE, AND WHAT IT DOES NOT. The decode leg runs
# `dav1d -o /dev/null` and asserts only that the stream PARSES. It does NOT
# compare the port's reconstruction with the decoder's, and the difference is
# not academic: an inter frame predicts from the previous frame's recon, so an
# encoder can emit a parseable stream while holding a canvas the decoder never
# rebuilds, and the error then compounds down the GOP invisibly to this gate.
# Read the pass line as "24 streams parsed", never as "24 streams reconstruct".
#
# THAT GAP IS MEASURED AND IT IS REAL. 2026-09-11, `SVTAV1_FINAL_RECON` against
# `aomdec` over three of these clips x qp {20,40} x presets {6,8,10} x 4
# frames: only 8 of 18 cells reconstruct identically, the rest drifting from
# frame 1, 2 or 3. That is why `encode_frame_impl` REFUSES a 10-bit inter frame
# for callers and why `AvifEncoder` codes a 10-bit animation all-intra; this
# gate reaches the surface only through `SVTAV1_INTER_EXPERIMENTAL`. Closing it
# means adding a recon leg here with a pinned per-cell table, exactly as
# `video_selfcheck_gate.sh` does for 8 bits.
#
# (The first attempt at that measurement read 0 of 18, from frame 0 onward,
# because `identity_run`'s MULTI-frame `SVTAV1_FINAL_RECON` wrote the 8-BIT
# canvas whatever the depth. It now refuses that substitution. A tool that can
# report a confidently wrong number is a defect like any other.)
#
# WHERE THE DIVERGENCE IS NOT. bd10 STILLS on this same content and geometry
# are 16/16 byte-identical to C (johnny + vidyo3 x {128,256} x presets
# {6,8,9,10}, measured the same day), and the 8-bit VIDEO key frame of this
# very cell is identical (1176 B on both sides at johnny 128x128 q40 p6). So
# frame 0 diverging here is neither a still defect nor a video-plumbing defect:
# it is bd10 mode decision under the VIDEO configuration. It does not track
# `bd10_full_rd` either -- presets 9 and 10, where that gate is off, diverge
# MORE than 6 and 8, not less.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
DAV1D="${DAV1D:-dav1d}"

if [ ! -f "$ASSETS/vidyo3_128x128_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "bd10 video gate: could not obtain assets" >&2; exit 1; }
fi

# clip size preset  expected_f0/expected_f1
# MEASURED 2026-09-10 at cli_qp 40, frames=2, bit depth 10, on x86-64.
# kristenandsara 128x128 p6 frame 1 was 0/1 and dropped to 0/0 on
# 2026-09-13 when the inter MDS0 lane switched to C's video variance arm
# (`mds0_use_hadamard_sb = 0` has no bit-depth branch). The prior match was
# coincidence: frame 0 on this cell diverges in the coded payload (first
# tile bytes), which is how frame 1 could match at all. The C-faithful arm
# now exposes the underlying bd10 MD divergence.
# 2026-09-18: kristenandsara p6 frame 1 is 0/1 again — this time NOT
# coincidence. The P-slice now ships the u8-domain coefficients C emits at
# pcs->hbd_md == 0 (enc_mode_config.c:2163 `is_islice ? 2 : 0`, traced on
# this clip: frame 0 saved=2, frame 1 saved=0) instead of running the bd10
# level re-encode.
#
# 2026-09-18 (later): the KEYFRAME converged — 23 of 24 cells are 1/x, i.e.
# the hbd_md=2 I-slice is byte-identical to C. Two stacked fixes did it:
#
#   1. `set_pd0_ctrls` forces `pd0_level = PD0_LVL_0` when `hbd_md != 0`
#      (enc_mode_config.c:5415) — the port now applies the force on the
#      video arm (`video_pd0_params`'s hbd_md parameter), and maps it to
#      the VIDEO-arm LVL_0 cost model: `sig_deriv_enc_dec_pd0` resolves
#      `rate_est_level = 2` there (`pd0_level <= PD0_LVL_3`, :7357), i.e.
#      `lpd0_qp_offset = 0` + `coeff_rate_est_lvl = 1` — `Pd0Mode::Lvl1`'s
#      real coefficient pricing, NOT the allintra LVL_0's closed form
#      (qindex+8, `5000 + 100*eob`). Per-node PD0 dist/ybits/cost now match
#      C's `SVT_PD0COST_OUT` exactly.
#   2. The bd10 MDS0 fast loop scored every candidate with
#      `hadamard_satd_hbd`, but the VIDEO arm sets `mds0_use_hadamard_sb =
#      false` (:7916) — C's `fast_loop_core` takes the `vf_hbd_10` variance
#      arm (product_coding_loop.c:1296-1307), a DC-invariant metric. The
#      port now mirrors the same SSD/hadamard/variance three-way at bd10
#      that the u8 lane already had.
#
# 2026-09-19: `kristenandsara 256x256 p6` converged too — 24 of 24 I-slices
# byte-identical. The missing piece was `ctx->bypass_encdec`: the funnel
# carried the allintra bake (`preset >= 4`) on the video arm, but C's video
# ladder is bit-depth-aware (`get_bypass_encdec_default`, enc_mode_config.c
# :8418) — bd10 keeps bypass OFF through M7. At bypass=0 C's winner rebuild
# lands in `cand_bf->recon` at the winner's tx_depth, which is what the
# skip-sub-depth quad-dist gate measures; the port's gate had already modeled
# both arms (`win_recon10` vs `gate_y10`) and was reading the wrong one. On
# node mi=(16,56) C measured quadrant SSE std ~213 (< 250) and kept the
# 16x16; the port measured ~1540 on the depth-0 eval recon and split.
# `encdec_arm::apply` now stamps the arm+bit-depth ladder.
#
# 2026-09-19 (later): the P-slice encode pass itself was missing. C forces
# `pic_bypass_encdec` OFF whenever `encoder_bit_depth > 8`
# (md_config_process.c:1046), so EVERY bd10 frame — P included — re-quantizes
# in the encode pass at 10 bits under `full_lambda_md[EB_10_BIT_MD]` (the
# av1_lambda_assign_md chain, lambda_weight included, NOT `pic_full_lambda`)
# with rate tables derived from the live `md_frame_context` (primary-ref CDFs
# on inter frames). The port's post-pass now runs on every non-full-RD bd10
# frame with `inter_full_lambda_bd10` + `primary_ref_cdfs`-seeded rates, and
# honors C's `md_skip_blk` semantics: skip_mode and committed-all-zero inter
# leaves stay zero (coding_loop.c:387), luma records the set for chroma.
# The last TU-level divergence was `txb_skip_ctx`: the post-pass passed
# `plane_bsize == tx_bsize` unconditionally, but a tx-split leaf's TU does
# not span the block — `contexts()` now takes the leaf dims so the
# `skip_contexts` table applies (entropy_coding.c:284). Three more cells
# promoted: johnny/vidyo1/vidyo4 256x256 p6.
#
# 2026-09-19 (later): the encode-pass RDOQ lambda is per-SUPERBLOCK. C
# re-runs `av1_lambda_assign_md` for every SB
# (`svt_aom_mode_decision_configure_sb`, md_process.c:796), and
# `update_lambda`'s stats_based_sb_lambda_modulation arm scales
# `full_lambda_md` by a `me_q_index - base_q_idx` factor (rc_process.c:
# 437-446) derived from `me_8x8_cost_variance` — live even with TPL off
# (low-delay) and `delta_q_present == 0`. The port's `SbInterLambda` map
# already carried the per-SB 8-bit MD lambdas; it now carries
# `full_10bit` too (`inter_full_lambda_bd10` at the SB's me_qdiff), and
# both bd10 re-encode walks resolve their RDOQ lambda per SB instead of
# per frame. Measured on vidyo3 128x128 p6 frame 1: C's optimize_b
# rdmult was 13188800/19783232 per SB vs the port's uniform 16881729 —
# enough to flip two marginal eob-boundary coefficients. Four cells
# promoted: johnny 256x256 p8, vidyo3 128x128 p6+p8, vidyo3 256x256 p6.
#
# Remaining: the P-slice where frame 0's recon did NOT already match — the
# inter-mode surface itself, not the pd0/funnel fixes above.
CELLS=(
    "fourpeople     128x128 6 1/1"
    "fourpeople     128x128 8 1/1"
    "fourpeople     256x256 6 1/1"
    "fourpeople     256x256 8 1/1"
    "johnny         128x128 6 1/1"
    "johnny         128x128 8 1/0"
    "johnny         256x256 6 1/1"
    "johnny         256x256 8 1/1"
    "kristenandsara 128x128 6 1/1"
    "kristenandsara 128x128 8 1/1"
    "kristenandsara 256x256 6 1/1"
    "kristenandsara 256x256 8 1/1"
    "vidyo1         128x128 6 1/1"
    "vidyo1         128x128 8 1/1"
    "vidyo1         256x256 6 1/1"
    "vidyo1         256x256 8 1/0"
    "vidyo3         128x128 6 1/1"
    "vidyo3         128x128 8 1/1"
    "vidyo3         256x256 6 1/1"
    "vidyo3         256x256 8 1/0"
    "vidyo4         128x128 6 1/1"
    "vidyo4         128x128 8 1/0"
    "vidyo4         256x256 6 1/1"
    "vidyo4         256x256 8 1/0"
)

QP="${BVG_QP:-40}"
work="${TMPDIR:-$HOME/tmp}/bd10-video.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; promoted=0; err=0; ran=0; undec=0
echo "== bd10 video gate (public-domain derf clips, qp $QP, bit depth 10) =="
printf '%-16s %-8s %-3s %-7s %-7s %-6s %s\n' clip size p expect got dec note
for spec in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $spec
    clip=$1; size=$2; preset=$3; expect=$4
    w=${size%x*}; h=${size#*x}
    asset="$ASSETS/${clip}_${size}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; err=$((err + 1)); continue
    fi
    out="$work/${clip}_${size}_p${preset}"
    mkdir -p "$out"
    env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        SVTAV1_INTER_EXPERIMENTAL=1 IDI_BD=10 \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$QP" "$preset" 2 \
        "rawseq:$asset" "$out" >"$out/diff.txt" 2>&1
    st=$?
    if [ $st -ne 0 ] && [ $st -ne 1 ]; then
        printf '  %-16s %-8s %-3s %-7s %-7s %-6s %s\n' "$clip" "$size" "$preset" "$expect" ERR - "exit $st"
        err=$((err + 1)); continue
    fi
    a=0; b=0
    cmp -s "$out/c.obu.pts0" "$out/rs.obu.f0" && a=1
    cmp -s "$out/c.obu.pts1" "$out/rs.obu.f1" && b=1
    got="$a/$b"
    ran=$((ran + 1))
    # DECODABILITY IS THE HARD ASSERTION HERE, not byte-identity: a 10-bit
    # inter stream this port emits must be a stream, whatever it compares to.
    dec=ok
    if ! "$DAV1D" -i "$out/rs.obu" -o /dev/null >/dev/null 2>&1; then
        dec=UNDECODABLE; undec=$((undec + 1))
    fi
    note=""
    if [ "$got" = "$expect" ]; then
        note="ok"
    elif [ $((a + b)) -gt $(( ${expect%/*} + ${expect#*/} )) ]; then
        note="PROMOTED — update the table"; promoted=$((promoted + 1))
    else
        note="REGRESSED"; fail=$((fail + 1))
    fi
    printf '  %-16s %-8s %-3s %-7s %-7s %-6s %s\n' "$clip" "$size" "$preset" "$expect" "$got" "$dec" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   regressed: $fail   promoted: $promoted   undecodable: $undec   errors: $err"
if [ "$ran" -ne "${#CELLS[@]}" ]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
[ $((fail + promoted + err + undec)) -eq 0 ] || exit 1
echo "bd10 video gate: OK"

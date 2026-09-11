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
#     0 of 48 frames byte-identical, 24 of 24 streams decodable (2026-09-10)
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
CELLS=(
    "fourpeople     128x128 6 0/0"
    "fourpeople     128x128 8 0/0"
    "fourpeople     256x256 6 0/0"
    "fourpeople     256x256 8 0/0"
    "johnny         128x128 6 0/0"
    "johnny         128x128 8 0/0"
    "johnny         256x256 6 0/0"
    "johnny         256x256 8 0/0"
    "kristenandsara 128x128 6 0/1"
    "kristenandsara 128x128 8 0/0"
    "kristenandsara 256x256 6 0/0"
    "kristenandsara 256x256 8 0/0"
    "vidyo1         128x128 6 0/0"
    "vidyo1         128x128 8 0/0"
    "vidyo1         256x256 6 0/0"
    "vidyo1         256x256 8 0/0"
    "vidyo3         128x128 6 0/0"
    "vidyo3         128x128 8 0/0"
    "vidyo3         256x256 6 0/0"
    "vidyo3         256x256 8 0/0"
    "vidyo4         128x128 6 0/0"
    "vidyo4         128x128 8 0/0"
    "vidyo4         256x256 6 0/0"
    "vidyo4         256x256 8 0/0"
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

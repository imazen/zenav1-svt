#!/usr/bin/env bash
# Two-frame differential against C on REAL public-domain video.
#
# WHY THIS GATE EXISTS. Every other inter cell in this repo encodes synthetic
# content: `inter_byte_gate.sh`'s 96 cells are {uniform,gradient,diag,screen},
# and `video_key_matrix.sh` defaults to `gradient diag screen screenrep
# uniform`. The multi-frame path builds later frames by translating frame 0 by
# a global integer offset, which open-loop ME finds exactly -- measured
# `avg_me_sad=0` and `is_gm_on=0` across {gradient,diag,screen} x
# {64,128,256,512} (INTER-ENCODE-PLAN.md). So the entire inter surface was
# being asserted against a motion field C's search never has to work for.
#
# MEASURED 2026-09-10, and this is the whole argument for the gate. At the
# identical cell shape (128x128 q40 p6 frames=2), synthetic `gradient` is
# byte-identical on BOTH frames, with frame 1 coding to 24 bytes -- a skip.
# Real video at that same shape diverges: `johnny` codes frame 1 as 33 bytes in
# C against the port's 36, `vidyo3` 113 against 101. The synthetic cell was
# green because it was asserting a skip frame.
#
# WHAT IT PINS. The exact measured (frame0, frame1) identity pair for each of
# 24 cells, so neither a regression nor a silent improvement can pass
# unnoticed. A cell that gets WORSE fails. A cell that gets BETTER is reported
# as PROMOTED and also fails, because the table below is a measurement and an
# out-of-date measurement is the thing this repo keeps getting burned by.
#
# THE ASSETS ARE NOT SYNTHETIC AND NOT IN GIT. Twelve I420 sequences cut from
# six clips that Xiph's Derf collection marks public domain, published at the
# R2 prefix `video/pd-derf-720p/` and fetched anonymously (no secrets). The
# still corpora this repo uses -- gb82-sc, CID22, clic2025 -- stay on their
# git sparse clone; see `fetch_r2_assets.sh` for why the two are kept apart.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"

# No silent skip: if the assets are absent, FETCH them. A gate that reports
# 0/0 and exits 0 reads as a pass, which is precisely the failure mode
# `lib_corpus.sh` documents three separate instances of.
if [ ! -f "$ASSETS/vidyo3_128x128_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "real video inter gate: could not obtain assets" >&2; exit 1; }
fi

# clip size preset  expected_f0/expected_f1
# MEASURED 2026-09-10 at cli_qp 40, frames=2, on x86-64.
#
# The frame-0 column is the interesting one: it is a KEY frame, so the six
# zeros are a STILL divergence on real content reached through the video
# configuration, and every one of them is preset 6 -- preset 8 key frames are
# 12/12. That is not the same surface `real_image_matrix.sh` covers (180/180 on
# CID22-512 at presets {2,6,10}), which runs the all-intra path at 512x512.
CELLS=(
    "fourpeople     128x128 6 1/1"
    "fourpeople     128x128 8 1/1"
    "fourpeople     256x256 6 0/0"
    "fourpeople     256x256 8 1/1"
    "johnny         128x128 6 1/0"
    "johnny         128x128 8 1/0"
    "johnny         256x256 6 0/0"
    "johnny         256x256 8 1/0"
    "kristenandsara 128x128 6 0/0"
    "kristenandsara 128x128 8 1/1"
    "kristenandsara 256x256 6 0/0"
    "kristenandsara 256x256 8 1/1"
    "vidyo1         128x128 6 1/1"
    "vidyo1         128x128 8 1/0"
    "vidyo1         256x256 6 1/0"
    "vidyo1         256x256 8 1/0"
    "vidyo3         128x128 6 1/0"
    "vidyo3         128x128 8 1/0"
    "vidyo3         256x256 6 0/0"
    "vidyo3         256x256 8 1/0"
    "vidyo4         128x128 6 1/1"
    "vidyo4         128x128 8 1/0"
    "vidyo4         256x256 6 0/0"
    "vidyo4         256x256 8 1/0"
)

QP="${RVIG_QP:-40}"
work="${TMPDIR:-$HOME/tmp}/real-video-inter.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; promoted=0; err=0; ran=0
echo "== real video inter gate (public-domain derf clips, qp $QP) =="
printf '%-16s %-8s %-3s %-7s %-7s %s\n' clip size p expect got note
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
    # SVTAV1_FRAME_SHIFT/_ZOOM_* are the SYNTHETIC motion model; identity_run
    # refuses them alongside `rawseq:` rather than silently layering a warp on
    # top of real motion, so they are cleared here.
    env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$QP" "$preset" 2 \
        "rawseq:$asset" "$out" >"$out/diff.txt" 2>&1
    st=$?
    if [ $st -ne 0 ] && [ $st -ne 1 ]; then
        printf '  %-16s %-8s %-3s %-7s %-7s %s\n' "$clip" "$size" "$preset" "$expect" ERR "exit $st"
        err=$((err + 1)); continue
    fi
    a=0; b=0
    cmp -s "$out/c.obu.pts0" "$out/rs.obu.f0" && a=1
    cmp -s "$out/c.obu.pts1" "$out/rs.obu.f1" && b=1
    got="$a/$b"
    ran=$((ran + 1))
    note=""
    if [ "$got" = "$expect" ]; then
        note="ok"
    elif [ $((a + b)) -gt $(( ${expect%/*} + ${expect#*/} )) ]; then
        note="PROMOTED — update the table"; promoted=$((promoted + 1))
    else
        note="REGRESSED"; fail=$((fail + 1))
    fi
    printf '  %-16s %-8s %-3s %-7s %-7s %s\n' "$clip" "$size" "$preset" "$expect" "$got" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   regressed: $fail   promoted: $promoted   errors: $err"
if [ "$ran" -ne "${#CELLS[@]}" ]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
[ $((fail + promoted + err)) -eq 0 ] || exit 1
echo "real video inter gate: OK"

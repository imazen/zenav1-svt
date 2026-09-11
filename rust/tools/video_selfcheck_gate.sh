#!/usr/bin/env bash
# ENCODER RECON vs AN INDEPENDENT DECODER over a MULTI-FRAME inter encode.
#
# WHAT THIS ASSERTS, AND WHY NOTHING ELSE DOES IT. Byte-identity to the C
# encoder cannot see an encoder/decoder MISMATCH: both encoders can agree on a
# bitstream that neither describes. An inter frame PREDICTS from the previous
# frame's reconstruction, so a reference that is not what a decoder rebuilds
# makes every later mode decision score a prediction the stream does not carry
# -- and the error compounds silently down the GOP. This gate decodes the
# port's own stream with `aomdec` and requires the port's final (deblock ->
# CDEF -> LR) reconstruction to be byte-identical to the decoder's, for EVERY
# frame of an 8-frame encode.
#
# THE SURFACE IT COVERS is the one that had no coverage at all until the
# temporal motion-vector field was fixed. Modes NEARESTMV and NEARMV do not
# signal an MV -- they DERIVE it from the reference-MV stack, part of which is
# projected from motion saved in earlier frames. Frames 0 and 1 cannot exercise
# it (C aborts the projection when the start frame is a KEY frame,
# md_config_process.c:441), so the field only goes live from frame 2 on, which
# is exactly where every earlier multi-frame defect appeared. Eight frames is
# the shortest encode that gives the field six live frames.
#
# MEASURED 2026-09-11: 18 of 18 cells, ALL 8 frames byte-identical, with the
# temporal field ON. That supersedes benchmarks/video_mfmv_isolation_2026-09-10.meta,
# which recorded four clips drifting and two failing to decode; the defect it
# isolated was `sb64_sq_no4xn_geom` being set from `sb_size == 64` alone, so the
# SIMPLIFIED MFMV block walk ran on rectangular blocks too and used `n4_w` for
# both extents -- half the rows of a 16x32 never contributed a temporal
# candidate. `SVTAV1_MFMV_OFF` remains available to re-isolate the field.
#
# ASSETS are the same public-domain Derf clips `real_video_inter_gate.sh` uses
# and are fetched the same way; there is no silent skip.
#
# Usage: tools/video_selfcheck_gate.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RUN="$HERE/identity_run"
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
SIZE="${VSG_SIZE:-256x256}"
FRAMES="${VSG_FRAMES:-8}"
PRESET="${VSG_PRESET:-6}"
W=${SIZE%x*}; H=${SIZE#*x}

AOMDEC="${AOMDEC:-aomdec}"
if ! command -v "$AOMDEC" >/dev/null 2>&1; then
    echo "video selfcheck gate: aomdec not on PATH -- this gate needs an" >&2
    echo "  INDEPENDENT decoder and will not fall back to the port's own." >&2
    exit 1
fi
if [ ! -f "$ASSETS/vidyo3_${SIZE}_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "video selfcheck gate: could not obtain assets" >&2; exit 1; }
fi

CLIPS=(fourpeople kristenandsara johnny vidyo1 vidyo3 vidyo4)
QPS=(20 40 55)
EXPECT=$(( ${#CLIPS[@]} * ${#QPS[@]} ))

work="${TMPDIR:-$HOME/tmp}/video-selfcheck.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0; inter_seen=0
echo "== video selfcheck gate (port recon vs aomdec, ${SIZE} p${PRESET}, ${FRAMES} frames) =="
printf '  %-16s %-4s %-8s %s\n' clip qp frames note
for clip in "${CLIPS[@]}"; do
    asset="$ASSETS/${clip}_${SIZE}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; fail=$((fail + 1)); continue
    fi
    for qp in "${QPS[@]}"; do
        out="$work/${clip}_q${qp}"; mkdir -p "$out"
        # SVTAV1_FRAME_SHIFT/_ZOOM_* are the SYNTHETIC motion model; cleared so
        # a stale environment cannot layer a warp on top of real motion.
        if ! env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
            SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_INTER_CHAIN_EXPERIMENTAL=1 \
            SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
            SVTAV1_FINAL_RECON="$out/rec" \
            "$RUN" "rawseq:$asset" "$W" "$H" "$qp" "$PRESET" "$out/p" \
            >"$out/stdout.txt" 2>"$out/trace.txt"; then
            printf '  %-16s %-4s %-8s %s\n' "$clip" "$qp" - "ENCODE REFUSED/FAILED"
            fail=$((fail + 1)); continue
        fi
        ran=$((ran + 1))
        # ANTI-VACUITY, per cell, and BEFORE the decode: a run that coded no
        # frame past the key frame would pass the comparison trivially, so
        # require a frame 1 on disk. Counted here rather than after the decode
        # so that a DECODE failure is reported as the failure it is instead of
        # also tripping the vacuity check.
        if [ -s "$out/p.obu.f1" ]; then
            inter_seen=$((inter_seen + 1))
        fi
        if ! "$AOMDEC" --rawvideo -o "$out/dec.yuv" "$out/p.obu" >/dev/null 2>&1; then
            printf '  %-16s %-4s %-8s %s\n' "$clip" "$qp" - "UNDECODABLE"
            fail=$((fail + 1)); continue
        fi
        got=$(RECON_DIR="$out" W="$W" H="$H" N="$FRAMES" python3 - <<'PY'
import os
w, h, n = int(os.environ['W']), int(os.environ['H']), int(os.environ['N'])
fl = w * h + 2 * (w // 2) * (h // 2)
d = open(os.path.join(os.environ['RECON_DIR'], 'dec.yuv'), 'rb').read()
ok, first = 0, None
for i in range(n):
    p = os.path.join(os.environ['RECON_DIR'], f'rec.f{i}')
    if not os.path.exists(p):
        break
    if open(p, 'rb').read()[:fl] == d[i * fl:(i + 1) * fl]:
        ok += 1
    elif first is None:
        first = i
print(f"{ok}/{n}" + (f" drift-from-f{first}" if first is not None else ""))
PY
)
        note=ok
        if [ "$got" != "$FRAMES/$FRAMES" ]; then note="RECON MISMATCH"; fail=$((fail + 1)); fi
        printf '  %-16s %-4s %-8s %s\n' "$clip" "$qp" "$got" "$note"
    done
done

echo
echo "cells run: $ran of $EXPECT   inter-coding cells: $inter_seen   failed: $fail"
if [ "$ran" -ne "$EXPECT" ]; then
    echo "ANTI-VACUITY FAIL: $ran of $EXPECT cells actually ran" >&2; exit 1
fi
if [ "$inter_seen" -ne "$EXPECT" ]; then
    echo "ANTI-VACUITY FAIL: $inter_seen of $EXPECT cells coded a frame past the key frame" >&2; exit 1
fi
[ "$fail" -eq 0 ] || exit 1
echo "video selfcheck gate: OK"

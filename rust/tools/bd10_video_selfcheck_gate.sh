#!/usr/bin/env bash
# bd10 VIDEO SELFCHECK — the 10-bit twin of video_selfcheck_gate.sh.
#
# WHAT IT ASSERTS. For every cell below, the port encodes an 8-frame
# low-delay-P stream at bit depth 10, `aomdec` decodes it, and the ENCODER's
# final reconstruction (SVTAV1_FINAL_RECON, u16 LE) must equal the decoder's
# output frame for frame — the property a wrong inter stream actually
# violates. This is the leg `bd10_video_gate.sh` documents as its gap:
# byte-identity to C cannot see an encoder/decoder recon mismatch, because
# an inter frame predicts from a canvas the bitstream never describes.
#
# WHY IT EXISTS. The `encode_frame_impl` refusal of a 10-bit inter frame
# stood on a 2026-09-11 measurement — 8 of 18 cells reconstructing as
# aomdec does. The `hbd_md = 2` MDS3 bump mirror (product_coding_loop.c:9649;
# see bd10_video_gate.sh's header) closed the residual: re-measured
# 2026-09-18 on this exact grid — 270 cells at 256x256 (all presets -1..13)
# plus 126 at 128x128 (presets -1..5, the band the 8-bit gate also covers
# there) — 396/396 cells, all 8 frames byte-identical to aomdec. This gate
# pins that so the refusal cannot silently need reinstating.
#
# KNOWN OUT-OF-ENVELOPE DEFECT (shared with bd8, not gated here): at
# 200x120 / 232x200 preset 13 the encoder panics in `eob_cost` on an eob-0
# call (quant.rs `eob_pt - 1` underflow) — IDENTICALLY at bd8, so it is a
# pre-existing u8-path bug, not a bd10 regression. Tracked separately.
#
# ASSETS: the same public-domain Derf clips `video_selfcheck_gate.sh` uses,
# fetched the same way; there is no silent skip.
#
# Usage: tools/bd10_video_selfcheck_gate.sh
# Env:   VSG_SIZE / VSG_FRAMES / VSG_PRESETS override the grid (same names as
#        the 8-bit gate); AOMDEC points at the decoder.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RUN="$HERE/identity_run"
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
SIZE="${VSG_SIZE:-256x256}"
FRAMES="${VSG_FRAMES:-8}"
PRESETS="${VSG_PRESETS:--1 0 1 2 3 4 5 6 7 8 9 10 11 12 13}"
W=${SIZE%x*}; H=${SIZE#*x}

AOMDEC="${AOMDEC:-aomdec}"
if ! command -v "$AOMDEC" >/dev/null 2>&1; then
    echo "bd10 video selfcheck gate: aomdec not on PATH -- this gate needs an" >&2
    echo "  INDEPENDENT decoder and will not fall back to the port's own." >&2
    exit 1
fi
if [ ! -f "$ASSETS/vidyo3_${SIZE}_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "bd10 video selfcheck gate: could not obtain assets" >&2; exit 1; }
fi

CLIPS=(fourpeople kristenandsara johnny vidyo1 vidyo3 vidyo4)
QPS=(20 40 55)
# shellcheck disable=SC2206
PRESET_LIST=($PRESETS)
EXPECT=$(( ${#CLIPS[@]} * ${#QPS[@]} * ${#PRESET_LIST[@]} ))

work="${TMPDIR:-$HOME/tmp}/bd10-video-selfcheck.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0; inter_seen=0
echo "== bd10 video selfcheck gate (port recon vs aomdec, ${SIZE}, presets ${PRESETS}, ${FRAMES} frames) =="
printf '  %-16s %-4s %-3s %-8s %s\n' clip qp p frames note
for clip in "${CLIPS[@]}"; do
    asset="$ASSETS/${clip}_${SIZE}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; fail=$((fail + 1)); continue
    fi
    for qp in "${QPS[@]}"; do
      for PRESET in "${PRESET_LIST[@]}"; do
        out="$work/${clip}_q${qp}_p${PRESET}"; mkdir -p "$out"
        # SVTAV1_FRAME_SHIFT/_ZOOM_* are the SYNTHETIC motion model; cleared so
        # a stale environment cannot layer a warp on top of real motion.
        if ! env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
            SVTAV1_BD=10 \
            SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
            SVTAV1_FINAL_RECON="$out/rec" \
            "$RUN" "rawseq:$asset" "$W" "$H" "$qp" "$PRESET" "$out/p" \
            >"$out/stdout.txt" 2>"$out/trace.txt"; then
            printf '  %-16s %-4s %-3s %-8s %s\n' "$clip" "$qp" "$PRESET" - "ENCODE REFUSED/FAILED"
            fail=$((fail + 1)); continue
        fi
        ran=$((ran + 1))
        # ANTI-VACUITY, per cell, and BEFORE the decode: a run that coded no
        # frame past the key frame would pass the comparison trivially, so
        # require a frame 1 on disk.
        if [ -s "$out/p.obu.f1" ]; then
            inter_seen=$((inter_seen + 1))
        fi
        if ! "$AOMDEC" --rawvideo -o "$out/dec.yuv" "$out/p.obu" >/dev/null 2>&1; then
            printf '  %-16s %-4s %-3s %-8s %s\n' "$clip" "$qp" "$PRESET" - "UNDECODABLE"
            fail=$((fail + 1)); continue
        fi
        # 10-bit: two bytes per sample, u16 LE, both sides.
        got=$(RECON_DIR="$out" W="$W" H="$H" N="$FRAMES" python3 - <<'PY'
import os
w, h, n = int(os.environ['W']), int(os.environ['H']), int(os.environ['N'])
fl = 2 * (w * h + 2 * (w // 2) * (h // 2))
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
        printf '  %-16s %-4s %-3s %-8s %s\n' "$clip" "$qp" "$PRESET" "$got" "$note"
      done
    done
done

echo
echo "cells run: $ran of $EXPECT   inter-coding cells: $inter_seen   failed: $fail"
if [ "$ran" -ne "$EXPECT" ]; then
    echo "ANTI-VACUITY FAIL: $ran of $EXPECT cells actually ran" >&2; exit 1
fi
if [ "$inter_seen" -ne "$EXPECT" ]; then
    echo "ANTI-VACUITY FAIL: $inter_seen of $EXPECT cells coded a frame past the key frame" >&2
    exit 1
fi
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "bd10 video selfcheck gate: OK"

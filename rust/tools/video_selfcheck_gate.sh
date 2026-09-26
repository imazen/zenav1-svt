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
# MEASURED 2026-09-11: 18 of 18 cells at presets 6..13, ALL 8 frames
# byte-identical, with the temporal field ON. That superseded
# benchmarks/video_mfmv_isolation_2026-09-10.meta, which recorded four clips
# drifting and two failing to decode; the defect it isolated was
# `sb64_sq_no4xn_geom` being set from `sb_size == 64` alone, so the SIMPLIFIED
# MFMV block walk ran on rectangular blocks too and used `n4_w` for both
# extents -- half the rows of a 16x32 never contributed a temporal candidate.
# `SVTAV1_MFMV_OFF` remains available to re-isolate the field.
#
# RE-SWEPT 2026-09-15 down the WHOLE ladder: presets -1..13 all pass at
# 256x256, and presets -1..5 pass at 128x128 as well. The residual low-preset
# drift the 2026-09-11 run recorded (and attributed half-wrongly to the
# spatial MV stack) was the OBMC neighbour-prediction cache serving one
# frame's predictions to the next; `obmc_pred_arm::begin_leaf` now resets it
# per leaf eval, the way `md_encode_block` resets C's ready flags. The preset
# floor in `pipeline.rs` came off in the same change.
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
PRESETS="${VSG_PRESETS:--1 0 1 2 3 4 5 6 7 8 9 10 11 12 13}"
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
# shellcheck disable=SC2206
PRESET_LIST=($PRESETS)
EXPECT=$(( ${#CLIPS[@]} * ${#QPS[@]} * ${#PRESET_LIST[@]} ))

work="${TMPDIR:-$HOME/tmp}/video-selfcheck.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

# The cells are a list for tools/cellrun.py (plan T3), run in parallel
# (VSG_JOBS, default 4): each cell is the port alone (no C side) with two
# checks, `recon` (aomdec's output == the port's final recon on EVERY frame)
# and `inter` (the port coded a frame past the key frame; anti-vacuity, per
# cell). VSG_BD=10 runs the 10-bit twin (bd10_video_selfcheck_gate.sh).
BD="${VSG_BD:-8}"
LABEL="video selfcheck gate"
[ "$BD" = 10 ] && LABEL="bd10 video selfcheck gate"
echo "== $LABEL (port recon vs aomdec, ${SIZE}, presets ${PRESETS}, ${FRAMES} frames) =="
LIST="$work/video_selfcheck.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv\tcheck\n' >"$LIST"
for clip in "${CLIPS[@]}"; do
    asset="$ASSETS/${clip}_${SIZE}_8f.i420"
    [ -f "$asset" ] || { echo "  MISSING ASSET $asset" >&2; exit 1; }
    for qp in "${QPS[@]}"; do
      for PRESET in "${PRESET_LIST[@]}"; do
        printf '%s_q%s_p%s\trawseq:%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_FRAMES=%s;SVTAV1_INTRA_PERIOD=64;SVTAV1_HIER_LEVELS=0\trecon,inter\n' \
          "$clip" "$qp" "$PRESET" "$asset" "$W" "$H" "$qp" "$PRESET" "$BD" "$FRAMES" >>"$LIST"
      done
    done
done
# SVTAV1_FRAME_SHIFT/_ZOOM_* are the SYNTHETIC motion model; cleared so a
# stale environment cannot layer a warp on top of real motion.
env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN AOMDEC="$AOMDEC" \
    python3 "$HERE/cellrun.py" "$LIST" --out "$work/result.tsv" --jobs "${VSG_JOBS:-4}"
rc=$?
python3 - "$work/result.tsv" "$EXPECT" "$LABEL" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
expect, label = int(sys.argv[2]), sys.argv[3]
ran = [r for r in rows if r["verdict"] != "ERROR"]
inter = [r for r in rows if "inter=ok" in r["checks"]]
bad = [r for r in rows if r["ok"] != "yes"]
for r in bad:
    print(f"  {r['name']:<24} {r['verdict']} {r['detail']} {r['checks']}")
print(f"\ncells run: {len(ran)} of {expect}   inter-coding cells: {len(inter)}   failed: {len(bad)}")
if len(ran) != expect:
    sys.exit(f"ANTI-VACUITY FAIL: {len(ran)} of {expect} cells actually ran")
if len(inter) != expect:
    sys.exit(f"ANTI-VACUITY FAIL: {len(inter)} of {expect} cells coded a frame past the key frame")
if bad:
    sys.exit(1)
print(f"{label}: OK")
PY
st=$?
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]

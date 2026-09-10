#!/usr/bin/env bash
# WARPED MOTION: the port selects it, and its reconstruction is a decoder's.
#
# WHY THIS GATE EXISTS. Until 2026-09-10 `inter_md_arm` injected no candidate
# whose `motion_mode != SimpleTranslation`, and `CONTEXT-HANDOFF.md` justified
# that with "if C selected these tools in the tested envelope the bytes would
# already differ, and they do not". `benchmarks/c_motion_mode_census_2026-09-10.meta`
# measured the premise false: C codes **88 of 1158** inter blocks WARPED_CAUSAL
# over the 24-cell video gate, all at preset 6.
#
# WHAT IT ASSERTS, and the order matters:
#
#   1. The port SELECTS warped motion where C does — a positive control. A
#      wiring that injects the candidate but never wins with it would pass
#      every byte gate in this repo unchanged and be worth nothing.
#   2. Its RECONSTRUCTION is byte-identical to an independent decoder on every
#      frame. This is the load-bearing assertion: the warp parameters are NOT
#      in the bitstream — the decoder re-derives them from the same neighbour
#      scan — so a wrong derivation, a wrong sample set, a wrong band gate or a
#      wrong chroma fallback all show up here and NOWHERE in a size comparison.
#   3. Cells where C selects NO warped block still code zero of them, so the
#      injection cannot have widened the candidate set where C's own gate is
#      closed.
#
# It does NOT assert byte-identity with C. The MDS1 warp MV refinement
# (`opt_non_translation_motion_mode_warp`, wm_level 3 -> refine_level 1) is not
# wired, so the port's warp candidates carry the unrefined MV. See
# `benchmarks/warped_motion_2026-09-10.meta`.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
DAV1D="${DAV1D:-dav1d}"

if [ ! -f "$ASSETS/vidyo3_128x128_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "warped motion gate: could not obtain assets" >&2; exit 1; }
fi

# clip size preset  min_warped_blocks  (0 = C selects none, so the port must not either)
# MEASURED 2026-09-10 at cli_qp 40, 8 frames, preset 6/8 as given.
CELLS=(
    "vidyo3         256x256 6 20"
    "vidyo1         256x256 6 5"
    "vidyo3         128x128 6 1"
    "johnny         256x256 6 1"
    "fourpeople     128x128 6 0"
    "kristenandsara 128x128 8 0"
    "vidyo3         256x256 8 0"
    "vidyo1         128x128 8 0"
)

QP="${WMG_QP:-40}"
FRAMES="${WMG_FRAMES:-8}"
work="${TMPDIR:-$HOME/tmp}/warped-motion.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0
echo "== warped motion gate (public-domain derf clips, qp $QP, $FRAMES frames) =="
printf '%-16s %-8s %-3s %-8s %-8s %-10s %s\n' clip size p warped min recon note
for spec in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $spec
    clip=$1; size=$2; preset=$3; minw=$4
    w=${size%x*}; h=${size#*x}
    asset="$ASSETS/${clip}_${size}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; fail=$((fail + 1)); continue
    fi
    out="$work/${clip}_${size}_p${preset}"
    mkdir -p "$out"
    # SVTAV1_INTER_CHAIN_EXPERIMENTAL is what lets frame 2+ reference an INTER
    # picture; without it this would be a two-frame cell and warp would barely
    # appear. Both flags are diagnostics — see `crate::dbgenv`.
    if ! env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_INTER_CHAIN_EXPERIMENTAL=1 \
        SVTAV1_FRAMES="$FRAMES" SVTAV1_INTERDBG=1 SVTAV1_FINAL_RECON="$out/rec" \
        "$HERE/identity_run" "rawseq:$asset" "$w" "$h" "$QP" "$preset" "$out/p" \
        >"$out/stdout.txt" 2>"$out/idbg.txt"; then
        printf '  %-16s %-8s %-3s %-8s %-8s %-10s %s\n' "$clip" "$size" "$preset" ERR "$minw" - "encode failed"
        fail=$((fail + 1)); continue
    fi
    ran=$((ran + 1))
    warped=$(grep -c 'mm=WarpedCausal' "$out/idbg.txt" || true)
    if ! "$DAV1D" -i "$out/p.obu" -o "$out/dec.yuv" >/dev/null 2>&1; then
        printf '  %-16s %-8s %-3s %-8s %-8s %-10s %s\n' "$clip" "$size" "$preset" "$warped" "$minw" UNDECODABLE FAIL
        fail=$((fail + 1)); continue
    fi
    recon=$(RECON_DIR="$out" W="$w" H="$h" N="$FRAMES" python3 - <<'PY'
import os
w, h, n = int(os.environ['W']), int(os.environ['H']), int(os.environ['N'])
fl = w * h + 2 * (w // 2) * (h // 2)
d = open(os.path.join(os.environ['RECON_DIR'], 'dec.yuv'), 'rb').read()
ok = 0
for i in range(n):
    p = os.path.join(os.environ['RECON_DIR'], f'rec.f{i}')
    if not os.path.exists(p):
        break
    if open(p, 'rb').read()[:fl] == d[i * fl:(i + 1) * fl]:
        ok += 1
print(f'{ok}/{n}')
PY
)
    note=ok
    if [ "$recon" != "$FRAMES/$FRAMES" ]; then
        note="RECON MISMATCH"; fail=$((fail + 1))
    elif [ "$minw" -eq 0 ] && [ "$warped" -ne 0 ]; then
        note="warp where C selects NONE"; fail=$((fail + 1))
    elif [ "$warped" -lt "$minw" ]; then
        note="selects FEWER warped blocks than pinned"; fail=$((fail + 1))
    fi
    printf '  %-16s %-8s %-3s %-8s %-8s %-10s %s\n' "$clip" "$size" "$preset" "$warped" "$minw" "$recon" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   failed: $fail"
if [ "$ran" -ne "${#CELLS[@]}" ]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
[ "$fail" -eq 0 ] || exit 1
echo "warped motion gate: OK"

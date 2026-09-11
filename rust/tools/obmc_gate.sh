#!/usr/bin/env bash
# OBMC: the port selects it where C does, and its reconstruction is a decoder's.
#
# WHY THIS GATE EXISTS. `benchmarks/obmc_census_2026-09-10.meta` measured C
# coding OBMC on 22.5 % of every coded inter block at preset 0 (987 of 4382
# over twelve real-video cells), 22.2 % at MR and 27.0 % at preset 1, and
# EXACTLY ZERO from preset 2 up. The port injected no OBMC candidate at all,
# on the strength of a census that had only ever run at presets 6 and 8.
#
# WHAT IT ASSERTS, and the order matters:
#
#   1. The port SELECTS OBMC where C does — a positive control. A wiring that
#      builds the candidate but never wins with it would pass every byte gate
#      in this repo unchanged and be worth nothing.
#   2. Its RECONSTRUCTION is byte-identical to an independent decoder on every
#      frame. This is the load-bearing assertion. OBMC's blend depends on the
#      NEIGHBOURS' motion, which is not re-transmitted — the decoder re-derives
#      it from the same mi grid — so a wrong neighbour, a wrong pairing, a
#      wrong edge or a wrong blend depth shows up HERE and nowhere in a size
#      comparison. The 4-wide pairing rule (C takes a 4-wide neighbour's
#      GEOMETRY from the start of the pair and its MODE INFO from the pair's
#      second half) cost 120 luma samples on vidyo3 256x256 p0 and was
#      invisible to every other gate.
#   3. Cells where C selects NO OBMC still code none, so the injection cannot
#      have widened the candidate set where C's own ladder closes it. That is
#      what the preset-2 cells are for: `svt_aom_get_obmc_level` gives level 5
#      there and C picks OBMC on zero blocks.
#
# It does NOT assert byte-identity with C. The MDS-stage MV refinement IS wired
# (`opt_non_translation_motion_mode_obmc` -> `obmc_motion_refinement` ->
# `single_motion_search`), which is what these counts include; the
# INJECTION-time refinement (`obmc_ctrls.refine_level == 0`, obmc_level 1,
# preset MR only) is not. See `benchmarks/obmc_wiring_2026-09-11.meta`.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
DAV1D="${DAV1D:-dav1d}"
if [ ! -f "$ASSETS/vidyo3_256x256_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "obmc gate: could not obtain assets" >&2; exit 1; }
fi

# clip size preset min_obmc_blocks
# MEASURED 2026-09-11 at cli_qp 40, 2 frames. A 0 means C's ladder closes OBMC
# at that preset (level 5), so the port must code none either.
#
# THESE COUNTS MOVED when the MV refinement landed (186/76/12/54 -> 168/92/14/56).
# That is the refinement doing its job — it changes which MV each OBMC
# candidate carries, so it changes which blocks win RD — and it is NOT a
# weakening of the gate: the load-bearing assertion is the RECON column, which
# went from "byte-identical to dav1d with the unrefined MV" to the same thing
# with C's real search in the loop. A count that moves with no named cause is a
# regression; this one has one.
CELLS=(
    "vidyo3 256x256 0 168"
    "vidyo1 256x256 0 92"
    "johnny 256x256 0 14"
    "vidyo3 128x128 0 56"
    "vidyo3 256x256 2 0"
    "vidyo1 256x256 2 0"
)

QP="${OBMC_QP:-40}"
FRAMES="${OBMC_FRAMES:-2}"
work="${TMPDIR:-$HOME/tmp}/obmc-gate.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0; selecting=0
echo "== OBMC gate (public-domain derf clips, qp $QP, $FRAMES frames) =="
printf '%-10s %-8s %-3s %-8s %-8s %-8s %s\n' clip size p obmc min recon note
for spec in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $spec
    clip=$1; size=$2; preset=$3; mino=$4
    w=${size%x*}; h=${size#*x}
    asset="$ASSETS/${clip}_${size}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; fail=$((fail + 1)); continue
    fi
    out="$work/${clip}_${size}_p${preset}"
    mkdir -p "$out"
    # Below the shipped preset-6 inter floor; see `dbgenv::inter_experimental`.
    if ! env SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_INTERDBG=1 \
        SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        SVTAV1_FINAL_RECON="$out/rec" \
        "$HERE/identity_run" "rawseq:$asset" "$w" "$h" "$QP" "$preset" "$out/p" \
        >"$out/stdout.txt" 2>"$out/idbg.txt"; then
        printf '  %-10s %-8s %-3s %-8s %-8s %-8s %s\n' "$clip" "$size" "$preset" ERR "$mino" - "encode failed"
        fail=$((fail + 1)); continue
    fi
    ran=$((ran + 1))
    obmc=$(grep -c 'mm=ObmcCausal' "$out/idbg.txt" || true)
    [ "$obmc" -gt 0 ] && selecting=$((selecting + 1))
    if ! "$DAV1D" -i "$out/p.obu" -o "$out/dec.yuv" >/dev/null 2>&1; then
        printf '  %-10s %-8s %-3s %-8s %-8s %-8s %s\n' "$clip" "$size" "$preset" "$obmc" "$mino" UNDECODABLE FAIL
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
    elif [ "$mino" -eq 0 ] && [ "$obmc" -ne 0 ]; then
        note="OBMC where C's ladder codes none"; fail=$((fail + 1))
    elif [ "$obmc" -lt "$mino" ]; then
        note="selects FEWER OBMC blocks than pinned"; fail=$((fail + 1))
    fi
    printf '  %-10s %-8s %-3s %-8s %-8s %-8s %s\n' "$clip" "$size" "$preset" "$obmc" "$mino" "$recon" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   cells selecting OBMC: $selecting   failed: $fail"
if [ "$ran" -ne "${#CELLS[@]}" ]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
if [ "$selecting" -eq 0 ]; then
    echo "ANTI-VACUITY FAIL: no cell selected a single OBMC block, so nothing" >&2
    echo "  here exercised OBMC at all." >&2; exit 1
fi
[ "$fail" -eq 0 ] || exit 1
echo "obmc gate: OK"

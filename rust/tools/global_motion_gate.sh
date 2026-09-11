#!/usr/bin/env bash
# GLOBAL MOTION: the port codes C's fitted model, and the result is a decoder's.
#
# WHY THIS GATE EXISTS. Until 2026-09-10 an inter frame whose
# `svt_aom_global_motion_estimation` fitted a NON-IDENTITY model was REFUSED
# (`pipeline::gm_search_config_error`): the header wrote seven `is_global = 0`
# bits and mode decision priced GLOBALMV against IDENTITY, so emitting one
# would have been a plausible-but-wrong stream. `tools/gm_join_gate.sh` proves
# the port's frame-level DERIVATION matches C's, including which frames fit a
# model; this gate proves the port then CODES it.
#
# WHAT IT ASSERTS, and the order matters:
#
#   1. The search reaches a non-identity model on the cells pinned for it
#      (`all_identity=0`). Without this the gate is vacuous — a pure
#      translation is what open-loop ME cancels, so the ordinary cells never
#      leave IDENTITY and a broken GM path would pass unnoticed. That is what
#      the ZOOM cells are for: a zoom about the frame centre is a ROTZOOM no
#      integer MV cancels.
#   2. The frame ENCODES rather than refusing, and dav1d decodes it.
#   3. Every frame's reconstruction is byte-identical to dav1d's. This is the
#      load-bearing assertion. A GLOBALMV block codes NO motion vector — the
#      decoder rebuilds it from the coded model — and `setup_ref_mv_list`
#      fills the tail of EVERY block's MV stack with that same projection, so
#      a model that does not survive the header, or a `gm_mv` the pack and the
#      decoder disagree about, moves pixels frame-wide and shows up NOWHERE in
#      a size comparison.
#   4. The port actually SELECTS GLOBALMV where the pinned count says it does,
#      and codes none where the model stays IDENTITY.
#
# It does NOT assert byte-identity with C: `tools/gm_join_gate.sh` owns the
# derivation join, and the MD-side ladders this envelope runs are not yet a
# byte match on a fitted-model frame.
set -uo pipefail
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION)" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
. "$HERE/lib_corpus.sh"

DAV1D="${DAV1D:-dav1d}"
PHOTO_DIR=$(corpus_dir codec-corpus/CID22/CID22-512/training) || true
PHOTO="$PHOTO_DIR/3571065.png"
[[ -r "$PHOTO" ]] || { echo "FATAL: corpus image missing: $PHOTO" >&2; exit 2; }

# content w h qp preset shift zoom_num zoom_den want_nonidentity min_globalmv
# MEASURED 2026-09-10; see benchmarks/global_motion_2026-09-10.meta.
CELLS=(
    "crop:$PHOTO 256 256 40 2 0 33 32 1 24"
    "crop:$PHOTO 512 512 40 2 0  9  8 1 4098"
    "crop:$PHOTO 256 256 40 2 3  1  1 0 0"
    "crop:$PHOTO 128 128 40 2 3  1  1 0 0"
)

FRAMES="${GMG_FRAMES:-2}"
work="${TMPDIR:-$HOME/tmp}/global-motion.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0; nonident=0
echo "== global motion gate (CID22 photo, zoom = a ROTZOOM the ME cannot cancel) =="
printf '%-10s %-4s %-6s %-8s %-8s %-8s %s\n' size p zoom nonident globalmv recon note
for spec in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $spec
    content=$1; w=$2; h=$3; qp=$4; preset=$5; shift_px=$6; zn=$7; zd=$8; want_ni=$9; ming=${10}
    out="$work/${w}x${h}_p${preset}_z${zn}-${zd}"
    mkdir -p "$out"
    if ! env SVTAV1_GMDBG=1 SVTAV1_INTERDBG=1 \
        SVTAV1_FRAME_SHIFT="$shift_px" SVTAV1_FRAME_ZOOM_NUM="$zn" SVTAV1_FRAME_ZOOM_DEN="$zd" \
        SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        SVTAV1_FINAL_RECON="$out/rec" \
        "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$preset" "$out/p" \
        >"$out/stdout.txt" 2>"$out/trace.txt"; then
        printf '  %-10s %-4s %-6s %-8s %-8s %-8s %s\n' "${w}x${h}" "$preset" "$zn/$zd" - "$ming" - "encode failed/REFUSED"
        fail=$((fail + 1)); continue
    fi
    ran=$((ran + 1))
    # 1. did the SEARCH leave identity?
    got_ni=0
    if grep -q '^GMPORT .*all_identity=0' "$out/trace.txt"; then got_ni=1; fi
    [[ "$got_ni" == 1 ]] && nonident=$((nonident + 1))
    globalmv=$(grep -c 'mode=GlobalMv' "$out/trace.txt" || true)
    note=ok
    if [[ "$got_ni" != "$want_ni" ]]; then
        note="all_identity disagrees with the pin"; fail=$((fail + 1))
    fi
    # 2 + 3. decode and compare every frame
    if ! "$DAV1D" -i "$out/p.obu" -o "$out/dec.yuv" >/dev/null 2>&1; then
        printf '  %-10s %-4s %-6s %-8s %-8s %-8s %s\n' "${w}x${h}" "$preset" "$zn/$zd" "$got_ni" "$globalmv" UNDECODABLE FAIL
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
    if [[ "$recon" != "$FRAMES/$FRAMES" ]]; then
        note="RECON MISMATCH"; fail=$((fail + 1))
    elif [[ "$ming" -eq 0 && "$globalmv" -ne 0 ]]; then
        note="GLOBALMV where the model is IDENTITY"; fail=$((fail + 1))
    elif [[ "$globalmv" -lt "$ming" ]]; then
        note="fewer GLOBALMV blocks than pinned"; fail=$((fail + 1))
    fi
    printf '  %-10s %-4s %-6s %-8s %-8s %-8s %s\n' "${w}x${h}" "$preset" "$zn/$zd" "$got_ni" "$globalmv" "$recon" "$note"
done

echo
echo "cells run: $ran of ${#CELLS[@]}   non-identity cells: $nonident   failed: $fail"
if [[ "$ran" -ne "${#CELLS[@]}" ]]; then
    echo "ANTI-VACUITY FAIL: $ran of ${#CELLS[@]} cells actually ran" >&2; exit 1
fi
if [[ "$nonident" -eq 0 ]]; then
    echo "ANTI-VACUITY FAIL: no cell reached a non-identity model, so nothing here" >&2
    echo "  exercised global motion at all. Add a zoom cell." >&2; exit 1
fi
[[ "$fail" -eq 0 ]] || exit 1
echo "global motion gate: OK"

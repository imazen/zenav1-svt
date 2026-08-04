#!/usr/bin/env bash
# IND-UV / CfL arbitration vs a LUMA-PALETTE candidate — decision gate.
#
# WHAT IT LOCKS. `check_best_indepedant_cfl` (product_coding_loop.c:3893)
# arbitrates CfL against the independent-chroma table with
#
#     ctx->best_uv_cost[mode] + ind_palette_cost_diff   <   cfl_uv_cost
#
# where `best_uv_cost[]` was built by the independent chroma search
# (product_coding_loop.c:7484) from `svt_aom_get_intra_uv_fast_rate(..., 0)`
# over ITS OWN buffers — candidates whose `palette_info` is NULL, so the UV_DC
# row is priced with `palette_uv_mode_fac_bits[0][*]` (rd_cost.c:514-521) — and
# `ind_palette_cost_diff` (:3912-3925) is what converts that [0] row into the
# candidate's [1] row. Feeding a luma-palette candidate's OWN fast_chroma_rate
# (already the [1] row) into that compare AND adding the diff charges the
# correction twice and flips the arbitration to CfL on blocks where C keeps
# UV_DC_PRED.
#
# WHY IT IS A REAL-CORPUS GATE. Measured 2026-08-04: a 360-cell synthetic
# screen/screenrep sweep (2 contents x {64,128,192,256} x 9 qp x presets 0-4)
# produced ZERO cells whose bytes move when the correction is doubled — the
# path needs a luma-palette candidate to reach the CfL arbitration with UV_DC
# as the independent best, which no synthetic content in that grid does. The
# smallest reproducer found is a real photo: CID22 1028637, the ONE image of 12
# photographic sources (6 CID22 + 6 gb82) for which C turns screen-content
# tools on.
#
# WHAT IT ASSERTS, on `crop:1028637.png 512x512 qp 32 preset 0`:
#   1. ANTI-VACUITY — the port still codes a luma-palette leaf at mi(16,28)
#      (8x8, pal>0). If palette stops winning there the block below proves
#      nothing, so a missing/palette-free leaf FAILS rather than passes.
#   2. That leaf's chroma mode is UV_DC_PRED (uv=0), which is what C codes.
#      C ground truth, measured 2026-08-04 with an instrumented copy of
#      entropy_coding.c (a per-block dump at the top of `write_modes_b`,
#      byte-identical output verified):
#          W CBLK mi=(16,28) 8x8 mode=0 uv=0 ibc=0 skip=1 pal=2 txd=0
#      Pre-fix the port coded uv=13 (UV_CFL_PRED) plus two CfL alpha symbols.
#   3. A REGRESSION BOUND on the first C-vs-port differing byte: >= 2000.
#      Pre-fix it was tile-payload +29 (whole-stream offset 39); post-fix
#      +2749 (offset 2759). This cell is NOT yet byte-identical — a separate
#      IntraBC gap at mi(36,16) 16x16 (C codes IntraBC at tx_depth 2, the port
#      codes SMOOTH_H) is the next divergence — so the bound is what can be
#      locked today. Raise it when that gap closes; never lower it.
#
# CORPUS: CID22-512 required. Fails loudly when absent — a gate that silently
# passes without encoding is worse than one that fails.
#
# Usage: tools/ind_uv_palette_gate.sh
# Env:   IND_UV_PAL_CORPUS (dir holding 1028637.png)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"

CORPUS="${IND_UV_PAL_CORPUS:-$HOME/work/zen/codec-corpus/CID22/CID22-512/training}"
IMG="$CORPUS/1028637.png"
if [ ! -f "$IMG" ]; then
    echo "ind-uv palette gate: $IMG not found" >&2
    echo "  set IND_UV_PAL_CORPUS=<dir containing 1028637.png> to point at it." >&2
    exit 1
fi

W="${TMPDIR:-/tmp}/induvpal.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

# SVTAV1_PACKTREE APPENDS — the file must not pre-exist or the leaf grep below
# would read a previous run's rows.
rm -f "$W/ptree.txt"
if ! SVTAV1_PACKTREE="$W/ptree.txt" $LOWPRI "$HERE/identity_run" \
    "crop:$IMG" 512 512 32 0 "$W/rs" >"$W/rs.log" 2>&1; then
    echo "ind-uv palette gate: port encode FAILED" >&2
    cat "$W/rs.log" >&2
    exit 2
fi
if ! SVT_TRACE_OUT=/dev/null $LOWPRI "$HERE/capture_c_trace/capture_c_trace" \
    512 512 32 0 "$W/rs.yuv" "$W/c.obu" 8 >"$W/c.log" 2>&1; then
    echo "ind-uv palette gate: C encode FAILED" >&2
    cat "$W/c.log" >&2
    exit 2
fi

fail=0

# ---- 1. anti-vacuity: the luma-palette leaf at mi(16,28) must exist --------
leaf=$(grep -m1 '^PTREE mi=(16,28) ' "$W/ptree.txt")
if [ -z "$leaf" ]; then
    echo "FAIL(anti-vacuity): no coded leaf at mi=(16,28) — the arbitration this" >&2
    echo "  gate locks is not exercised; re-derive the witness block." >&2
    fail=1
else
    echo "leaf: $leaf"
    pal=$(printf '%s\n' "$leaf" | sed -n 's/.* pal=\([0-9]*\).*/\1/p')
    if [ "${pal:-0}" -lt 1 ]; then
        echo "FAIL(anti-vacuity): mi=(16,28) codes no luma palette (pal=$pal) — a" >&2
        echo "  palette-free block cannot exercise ind_palette_cost_diff." >&2
        fail=1
    fi
    # ---- 2. the C-matching chroma decision -------------------------------
    uv=$(printf '%s\n' "$leaf" | sed -n 's/.* uv=\([0-9]*\).*/\1/p')
    if [ "${uv:-x}" != "0" ]; then
        echo "FAIL: mi=(16,28) codes uv=$uv; C codes uv=0 (UV_DC_PRED)." >&2
        echo "  uv=13 is UV_CFL_PRED — the doubled ind_palette_cost_diff signature." >&2
        fail=1
    fi
fi

# ---- 3. first-differing-TILE-PAYLOAD-byte regression bound ------------------
# Raw `cmp` is useless here: the frame OBU's LEB128 size field differs whenever
# the payload length does, so it always reports byte 13. identity_diff.py walks
# the OBUs and reports the offset INTO THE TILE PAYLOAD, which is the coded
# decision the bound is about.
if cmp -s "$W/rs.obu" "$W/c.obu"; then
    echo "NOTE: cell is now BYTE-IDENTICAL to C — the IntraBC gap must have closed."
    echo "  Promote this gate to a plain byte-parity assertion."
else
    python3 "$HERE/identity_diff.py" --c-obu "$W/c.obu" --rust-obu "$W/rs.obu" \
        --verbose >"$W/report.txt" 2>&1
    off=$(sed -n 's/.*first tile byte diff at +\([0-9]*\).*/\1/p' "$W/report.txt" | head -1)
    echo "first differing TILE-PAYLOAD byte: +${off:-<none>} (C=$(wc -c <"$W/c.obu") port=$(wc -c <"$W/rs.obu"))"
    if [ -z "$off" ]; then
        echo "FAIL: identity_diff.py reported no tile-payload offset — harness broken." >&2
        sed -n '1,40p' "$W/report.txt" >&2
        fail=1
    elif [ "$off" -lt 2000 ]; then
        echo "FAIL: first divergence regressed to tile-payload byte +$off (bound: >= 2000)." >&2
        echo "  Pre-fix this cell diverged at tile-payload +29, inside the mi(16,28)" >&2
        echo "  palette block's chroma syntax." >&2
        fail=1
    fi
fi

if [ "$fail" -eq 0 ]; then
    echo "ind-uv palette gate: PASS"
else
    echo "ind-uv palette gate: FAIL"
fi
exit "$fail"

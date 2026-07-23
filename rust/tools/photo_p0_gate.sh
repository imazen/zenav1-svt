#!/usr/bin/env bash
# PHOTO preset-0 bd8 byte-identity gate (the "FH loop_filter_level" class).
#
# Locks the 2026-07-23 closure of the dominant real-content residual: photo
# preset-0 bd8 went 61/135 -> 135/135 byte-identical on the wider-corpus photo
# grid (27 CID22+clic images x qp {5,20,32,48,63}, 512x512 — see
# benchmarks/photo_p0_bd8_sortfix_2026-07-23.meta) from TWO roots in the M0/M1
# independent-uv chroma search:
#
#   1. 79cc43d3c — the bd8 ind-uv fast-candidate sort must replicate C's
#      UNSTABLE swap-on-`<` selection sort (sort_fast_cost_based_candidates,
#      product_coding_loop.c:1415, ind-uv call :7680). The stable sort_by_key
#      admitted a different nfl=32 full-loop set whenever flat-chroma SAD tie
#      groups straddled the cut — i.e. on nearly every real photo.
#   2. 78bb5d361 — the ind-uv CfL arbitration keeps CfL on EXACT RD ties
#      (check_best_indepedant_cfl reverts only when best < cfl, :3927); the
#      port's bd8 arm had the compare reversed.
#
# Cells: a fixed subset of the 135-cell probe, all verified `cmp`-identical at
# the commit that added them, INCLUDING both root witnesses:
#   - 1200348 q32 (root-1 witness: first coded flip was the mi(32,48) 32x32
#     chroma angle delta C=-3/port=0, cascading into 1604 drifted chroma DC
#     inputs across SB(1,1)..SB(3,3))
#   - 5739122 q5 (root-2 witness: mi(31,80) 8x4 DC+filter-intra, both sides'
#     RD terms byte-identical and colliding at exactly 130518==130518 — C
#     codes CfL, the reversed tie-break coded H)
#
# CORPUS: CID22-512 required (fails loudly when absent, like
# bd10_photo_gate.sh — a gate that silently passes without encoding is worse
# than one that fails). The two clic cells are dropped with a LOUD warning
# when the clic corpus is absent (secondary corpus, sb128_gate SC_CORPUS
# pattern — the skip decision is made HERE at the caller).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
    for _c in aomdec /root/aomdec-debug/aomdec; do
        if command -v "$_c" >/dev/null 2>&1; then aomdec=$_c; break; fi
    done
fi
command -v "$aomdec" >/dev/null 2>&1 || {
    echo "photo p0 gate: aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}

CID22="${PHOTO_P0_CORPUS:-/root/work/codec-corpus/CID22/CID22-512/training}"
CLIC="${PHOTO_P0_CLIC:-/root/work/codec-corpus/clic2025/training}"
if [ ! -d "$CID22" ]; then
    echo "photo p0 gate: CID22 corpus not found at $CID22" >&2
    echo "  set PHOTO_P0_CORPUS=<dir of 512x512 PNGs> to point at it." >&2
    exit 1
fi

# "<content-arg> <qp>" cells, all preset 0, bd8, 512x512. file: for native-512
# CID22; crop: (center-crop) for the larger clic sources — the same content
# args the discovery sweep used, so these cells reproduce probe rows exactly.
CELLS=(
    "file:$CID22/1200348.png 32"
    "file:$CID22/5739122.png 5"
    "file:$CID22/1459534.png 63"
    "file:$CID22/1001682.png 48"
    "file:$CID22/45258.png 20"
    "file:$CID22/1618269.png 5"
)
if [ -d "$CLIC" ]; then
    CELLS+=(
        "crop:$CLIC/9466a908d088d32ee73f04116c40e48c.png 48"
        "crop:$CLIC/2c1f84548ef99faec2b4f9bf12227c83.png 20"
    )
else
    echo "WARNING: clic corpus not found at $CLIC — 2 clic cells SKIPPED (set PHOTO_P0_CLIC)" >&2
fi

OUT="${TMPDIR:-/tmp}/photop0.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
pass=0
fail=0
failed=()

for cell in "${CELLS[@]}"; do
    content=${cell% *}
    qp=${cell##* }
    stem=$(basename "${content#*:}" .png)
    tag="${stem}_q${qp}_p0"
    if [ ! -f "${content#*:}" ]; then
        fail=$((fail + 1))
        failed+=("${tag}[no-png]")
        continue
    fi
    if ! "$HERE/identity_run" "$content" 512 512 "$qp" 0 "$OUT/rs" >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[rs-err]")
        continue
    fi
    if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        512 512 "$qp" 0 "$OUT/rs.yuv" "$OUT/c.obu" >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[c-err]")
        continue
    fi
    # Decodability BEFORE byte-identity (same contract as bd10_photo_gate).
    if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[undecodable]")
        continue
    fi
    if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        pass=$((pass + 1))
        echo "  OK       $tag (byte-exact, $(stat -c%s "$OUT/c.obu")B)"
    else
        fail=$((fail + 1))
        failed+=("$tag")
        echo "  FAIL     $tag (C=$(stat -c%s "$OUT/c.obu")B port=$(stat -c%s "$OUT/rs.obu")B)"
    fi
done

total=$((pass + fail))
echo
echo "photo p0 gate: $pass / $total"
if [ "$fail" -ne 0 ]; then
    echo "FAILED cells: ${failed[*]}" >&2
    exit 1
fi

#!/usr/bin/env bash
# bd10 PHOTOGRAPHIC identity gate (task #94, low-preset/real-content axis).
#
# Real 10-bit photographic content, byte-identical to the real C encoder. Every
# cell below was verified with `cmp` at the commit that added it.
#
# WHY THIS GATE EXISTS: every other bd10 gate feeds SYNTHETIC content
# (uniform/gradient/diag). Synthetic patterns exercise a narrow slice of the
# mode/tx/partition space, so a port can be synthetic-green and still diverge on
# the content users actually encode — which is exactly what happened here:
# docs/bd10-port-map.md recorded an 18/18 photographic FAILURE ("on real content
# the partition tree ALWAYS diverges at bd10"). That measurement predated the
# PD0_LVL_0 partition fix, the TXS-coupling gate, the bd10 chroma re-encode and
# the AVX2-hadamard fix; with all four landed, photographic bd10 at eff-M9 is
# byte-identical. This gate pins that so it cannot silently regress.
#
# SCOPE — the eff-M9 band (presets 9..13) ONLY. Photographic bd10 at presets 0/3/6
# still DIVERGES (measured: 9/9 at p6, all PARTITION flips — PD1 depth-refine +
# NSQ at bd10). Those are NOT listed here; adding them would be a false claim.
# See docs/bd10-port-map.md "low-preset failure map".
#
# CORPUS: CID22-512 (250 real 512x512 photographic PNGs, natively 64-aligned).
# Override with BD10_PHOTO_CORPUS=<dir>. If the corpus is absent this gate FAILS
# LOUDLY — it never skips silently, because a gate that quietly passes without
# encoding anything is worse than one that fails.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

CORPUS="${BD10_PHOTO_CORPUS:-/root/work/codec-corpus/CID22/CID22-512/training}"
if [ ! -d "$CORPUS" ]; then
    echo "bd10 photo gate: corpus not found at $CORPUS" >&2
    echo "  set BD10_PHOTO_CORPUS=<dir of 512x512 PNGs> to point at it." >&2
    exit 1
fi

# Images verified byte-identical at every qp x preset in the grids below.
# Named explicitly (NOT selected by index into `ls`) so the gate is stable
# against corpus additions/removals.
IMAGES_A=(1001682 1484678 2119713 2738653 4666751)
QPS_A=(12 32 55)
IMAGES_B=(1080721 1454613116 167491 2208891 2666598 3571065 708587 pexels-photo-3214683)
QPS_B=(5 20 40 63)
PRESETS=(10 13)
# The rest of the eff-M9 band. Presets 9/11/12 share the closed envelope but
# were never gated (the synthetic gate was pinned on 10 and 13 only); measured
# 18/18 byte-identical on the group-C images below.
IMAGES_C=(1001682 2119713 4666751)
QPS_C=(12 40)
PRESETS_C=(9 11 12)

OUT="${TMPDIR:-/tmp}/bd10photo.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
pass=0
fail=0
missing=0
failed=()

run_cell() {
    local stem=$1 qp=$2 p=$3
    local png="$CORPUS/$stem.png"
    local tag="${stem}_q${qp}_p${p}"
    if [ ! -f "$png" ]; then
        missing=$((missing + 1))
        failed+=("${tag}[no-png]")
        return
    fi
    if ! SVTAV1_BD=10 "$HERE/identity_run" "file:$png" 512 512 "$qp" "$p" "$OUT/rs" \
        >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[rs-err]")
        return
    fi
    if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        512 512 "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[c-err]")
        return
    fi
    if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
        failed+=("$tag")
    fi
}

for stem in "${IMAGES_A[@]}"; do
    for qp in "${QPS_A[@]}"; do
        for p in "${PRESETS[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_B[@]}"; do
    for qp in "${QPS_B[@]}"; do
        for p in "${PRESETS[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_C[@]}"; do
    for qp in "${QPS_C[@]}"; do
        for p in "${PRESETS_C[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done

total=$((pass + fail + missing))
echo "bd10 photographic identity: $pass / $total byte-identical"
[ "$((fail + missing))" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$((fail + missing))" -eq 0 ]

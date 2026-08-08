#!/usr/bin/env bash
# Decode-verify the port's OBUs over a grid, with TWO independent decoders.
#
# The correctness half of the perf program: a faster encoder that emits corrupt
# bytes is a shipping bug, and this project has repeatedly caught desyncs that
# one decoder masked and another rejected. Every cell must decode OK under BOTH
# aomdec and dav1d; the run also re-checks the port's bytes against the C
# reference's per cell, so a pass means "same bytes as C AND both decoders
# accept them".
#
# Usage: tools/decode_gate_grid.sh <out.tsv>
# Env:   AOMDEC / DAV1D binaries; DG_SIZES / DG_PRESETS / DG_QPS / DG_CONTENT.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

OUT="${1:?usage: decode_gate_grid.sh <out.tsv>}"
AOMDEC="${AOMDEC:-aomdec}"
DAV1D="${DAV1D:-dav1d}"
command -v "$AOMDEC" >/dev/null 2>&1 || { echo "aomdec not found" >&2; exit 1; }
command -v "$DAV1D" >/dev/null 2>&1 || { echo "dav1d not found" >&2; exit 1; }

read -r -a SIZES <<<"${DG_SIZES:-32 64 128 256 512}"
read -r -a PRESETS <<<"${DG_PRESETS:-2 6 10 13}"
read -r -a QPS <<<"${DG_QPS:-20 40 55}"
read -r -a CONTENTS <<<"${DG_CONTENT:-gradient uniform}"

PE="$RS_ROOT/target/release/examples/perf_encode"
CE="$HERE/perf_c_encode/perf_c_encode"
WORK="$RS_ROOT/target/decgate"; mkdir -p "$WORK"
[[ -x "$PE" ]] || { echo "missing $PE" >&2; exit 1; }

{
    echo "# two-decoder gate over the port's OBUs"
    echo "# commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown) date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# aomdec=$($AOMDEC --version 2>&1 | head -1) dav1d=$($DAV1D --version 2>&1 | head -1)"
    printf 'content\tsize\tpreset\tqp\tbytes\taomdec\tdav1d\tsame_as_c\n'
} >"$OUT"

fail=0 n=0
for content in "${CONTENTS[@]}"; do
    for sz in "${SIZES[@]}"; do
        for preset in "${PRESETS[@]}"; do
            for qp in "${QPS[@]}"; do
                line=$("$PE" "$content" "$sz" "$sz" "$qp" "$preset" "$WORK/g" 0 2>/dev/null)
                bytes=$(printf '%s' "$line" | sed -n 's/.*BYTES=\([0-9]*\).*/\1/p')
                a=FAIL; d=FAIL; c=NA
                "$AOMDEC" "$WORK/g.obu" -o /dev/null >/dev/null 2>&1 && a=OK
                "$DAV1D" -i "$WORK/g.obu" -o /dev/null >/dev/null 2>&1 && d=OK
                if [[ -x "$CE" ]]; then
                    "$CE" "$sz" "$sz" "$qp" "$preset" "$WORK/g.yuv" "$WORK/g.c.obu" 0 >/dev/null 2>&1 &&
                        { cmp -s "$WORK/g.obu" "$WORK/g.c.obu" && c=Y || c=N; }
                fi
                printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                    "$content" "$sz" "$preset" "$qp" "${bytes:-0}" "$a" "$d" "$c" >>"$OUT"
                n=$((n + 1))
                [[ "$a" == OK && "$d" == OK ]] || { fail=$((fail + 1)); echo "  DECODE FAIL $content ${sz}x${sz} p$preset qp$qp aomdec=$a dav1d=$d"; }
                [[ "$c" != N ]] || { fail=$((fail + 1)); echo "  BYTES DIFFER FROM C $content ${sz}x${sz} p$preset qp$qp"; }
            done
        done
        printf '  %s %sx%s done\n' "$content" "$sz" "$sz"
    done
done

okc=$(awk -F'\t' 'NR>1 && $6=="OK" && $7=="OK"' "$OUT" | grep -vc '^#')
same=$(awk -F'\t' 'NR>1 && $8=="Y"' "$OUT" | grep -vc '^#')
echo "cells=$n  aomdec+dav1d OK=$okc  byte-identical to C=$same  failures=$fail"
echo "wrote $OUT"
exit $((fail > 0))

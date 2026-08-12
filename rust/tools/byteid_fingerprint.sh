#!/usr/bin/env bash
# Byte-identity fingerprint of the PORT's encoder output over a grid.
#
# The gate for every "bit-identical" optimization in the perf program: run it
# BEFORE a change, run it AFTER, and `diff` the two files. Any line that moves
# means the change altered the bitstream and is NOT a bit-identical win — it is
# a product change and must be argued on RD, not smuggled in as perf.
#
# It also records the C reference's md5 per cell (column `c_md5`) so the run
# doubles as a port-vs-C identity check: `port_md5 == c_md5` on a cell means the
# port and C emit the same OBU there.
#
# Usage: tools/byteid_fingerprint.sh <out.tsv>
# Env:   BID_SIZES / BID_PRESETS / BID_QPS / BID_CONTENT override the grid.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

OUT="${1:?usage: byteid_fingerprint.sh <out.tsv>}"
# 1024 and 2048 added 2026-08-11 (issue #15 action 1). Both are byte-identical
# to C on synthetic AND real content — the divergence #15 saw is an ALIGNMENT
# effect, not a size one (see tools/unaligned_identity_scan.sh and
# benchmarks/unaligned_real_identity_2026-08-11.meta). Keeping them here is what
# makes the perf gate's port-vs-port fingerprint cover production sizes; the
# whole grid at preset 2 / 2048 is ~6 s per cell, the rest are sub-second.
read -r -a SIZES <<<"${BID_SIZES:-32 64 128 256 512 1024 2048}"
read -r -a PRESETS <<<"${BID_PRESETS:-2 6 10 13}"
read -r -a QPS <<<"${BID_QPS:-20 40 55}"
read -r -a CONTENTS <<<"${BID_CONTENT:-gradient uniform}"

PE="$RS_ROOT/target/release/examples/perf_encode"
CE="$HERE/perf_c_encode/perf_c_encode"
WORK="$RS_ROOT/target/byteid"
mkdir -p "$WORK"
[[ -x "$PE" ]] || { echo "missing $PE (cargo build --release --examples)" >&2; exit 1; }
HAVE_C=0
[[ -x "$CE" ]] && HAVE_C=1

{
    echo "# byte-identity fingerprint of the port's OBU output"
    echo "# commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown) date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# grid: content=[${CONTENTS[*]}] sizes=[${SIZES[*]}] presets=[${PRESETS[*]}] qps=[${QPS[*]}]"
    printf 'content\tsize\tpreset\tqp\tbytes\tport_md5\tc_md5\n'
} >"$OUT"

for content in "${CONTENTS[@]}"; do
    for sz in "${SIZES[@]}"; do
        for preset in "${PRESETS[@]}"; do
            for qp in "${QPS[@]}"; do
                line=$("$PE" "$content" "$sz" "$sz" "$qp" "$preset" "$WORK/bid" 0 2>/dev/null)
                bytes=$(printf '%s' "$line" | sed -n 's/.*BYTES=\([0-9]*\).*/\1/p')
                pmd5=$(md5 -q "$WORK/bid.obu" 2>/dev/null || echo MISSING)
                cmd5="-"
                if ((HAVE_C)); then
                    # gradient/uniform are generated identically on both sides;
                    # the C driver reads the .yuv the port just wrote.
                    "$CE" "$sz" "$sz" "$qp" "$preset" "$WORK/bid.yuv" "$WORK/bid.c.obu" 0 \
                        >/dev/null 2>&1 && cmd5=$(md5 -q "$WORK/bid.c.obu" 2>/dev/null || echo MISSING)
                fi
                printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                    "$content" "$sz" "$preset" "$qp" "${bytes:-0}" "$pmd5" "$cmd5" >>"$OUT"
            done
        done
        printf '  %s %sx%s done\n' "$content" "$sz" "$sz"
    done
done

nc=$(grep -vc '^#' "$OUT")
same=$(awk -F'\t' 'NR>1 && $6==$7' "$OUT" | grep -vc '^#')
echo "cells=$((nc - 1))  port==C on $same"
echo "wrote $OUT"

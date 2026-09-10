#!/usr/bin/env bash
# Allocation-count / peak-heap measurement for the canonical alloc cell.
#
# The cell is a 512x320 screen-content still at qp20 (gb82-sc/windows.png). It is
# the one used for every allocation number in benchmarks/alloc_*_2026-09-10.meta:
# it is small enough to heaptrack in seconds and it exercises the full MD funnel
# (tx_unit_inner, mds3, inject, txt_search, chroma) that dominates the port's
# allocator traffic.
#
#   tools/heaptrack_alloc_cell.sh <arm-name>        # measure the port
#   tools/heaptrack_alloc_cell.sh --c c             # measure the C reference
#
# Writes ~/tmp/heap/<arm>.{heap.zst,txt,obu} and prints the two lines that
# matter (calls to allocation functions, peak heap consumption) plus the sha256
# of the emitted bitstream, so an arm that changed the OUTPUT can never be
# reported as an allocation win.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
OUT=${HEAPDIR:-$HOME/tmp/heap}
mkdir -p "$OUT"
CORPUS=${CORPUS:-$HOME/work/zen/codec-corpus/gb82-sc/windows.png}
mode=port
if [ "${1:-}" = "--c" ]; then mode=c; shift; fi
arm=${1:?usage: heaptrack_alloc_cell.sh [--c] <arm-name>}
cd "$OUT"
rm -f "$arm.zst" "$arm.txt"
if [ "$mode" = c ]; then
  heaptrack -o "$arm" "$ROOT/tools/capture_c_trace/capture_c_trace.bin" \
    512 320 20 -1 ./p.yuv "./$arm.obu" >/dev/null 2>&1
else
  heaptrack -o "$arm" "$ROOT/target/release/examples/identity_run" \
    "crop:$CORPUS" 512 320 20 -1 "./$arm" >/dev/null 2>&1
fi
heaptrack_print "$arm.zst" > "$arm.txt" 2>/dev/null
sum=$(tail -20 "$arm.txt")
printf '%-10s %s\n' "$arm" "$(printf '%s\n' "$sum" | grep -m1 'calls to allocation functions')"
printf '%-10s %s\n' "" "$(printf '%s\n' "$sum" | grep -m1 'peak heap memory consumption')"
printf '%-10s obu sha256 %s\n' "" "$(sha256sum "$arm.obu" | cut -c1-16)"

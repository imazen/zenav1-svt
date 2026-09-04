#!/usr/bin/env bash
# PEAK RSS OF SEVERAL perf_encode BINARIES ON ONE CELL, INTERLEAVED — the
# per-commit attribution harness for the memory axis.
#
#   tools/mem_bisect.sh <bin1> <bin2> ... <binN>
#   MB_SIZE=2048 MB_ARM=inter MB_ROUNDS=7 tools/mem_bisect.sh bins/*
#
# WHY NOT tools/mem_peak.sh: that script measures ONE binary (the tree's) per
# invocation, so a per-commit series taken with it is N serial blocks, and the
# aarch64 inter cell's run-to-run spread (8-15 %, docs/perf-status.md) is
# larger than most single-commit deltas. This runs the N binaries ROUND-ROBIN
# — bin1, bin2, ..., binN, bin1, ... — so drift in the box's state lands on
# every binary equally, and reports each binary's median/min/max plus every
# raw value (the raw column is the evidence; the median is the summary).
#
# The cell's environment is exactly mem_peak.sh's (same arm table), and the
# same REFUSAL TRAP applies: a run with a non-zero exit or an empty .obu is
# printed as REFUSED and excluded, never averaged in.
#
# rss  = /usr/bin/time maximum resident set size (BSD -l or GNU -v).
# heap = MB_MODE=heap: heaptrack's "peak heap memory consumption" (LIVE
#        malloc'd bytes, Linux only), ONE encode per binary — it is
#        deterministic for a deterministic encode, so rounds are not taken.
#        The .zst trace is kept under MB_OUT (ht_<i>.*) for heaptrack_print's
#        per-site attribution. heaptrack wraps the BINARY, never an env/nice
#        prefix (tools/mem_peak.sh records why: wrapping `env` measures `env`).
#
# Env: MB_MODE (rss|heap) MB_SIZE (2048) MB_ARM (inter) MB_ROUNDS (7) MB_QP (40)
#      MB_PRESET (13) MB_CONTENT (gradient) MB_SHIFT (3) MB_OUT ($HOME/tmp/mem_bisect)
#      MB_THREADS — if set, exported as SVTAV1_THREADS for every run
set -uo pipefail
(( BASH_VERSINFO[0] >= 4 )) || { echo "FATAL: needs bash >= 4 (this is ${BASH_VERSION})" >&2; exit 2; }
MODE=${MB_MODE:-rss}
SIZE=${MB_SIZE:-2048}; ARM=${MB_ARM:-inter}; ROUNDS=${MB_ROUNDS:-7}
[[ "$MODE" == rss || "$MODE" == heap ]] || { echo "FATAL: MB_MODE must be rss or heap" >&2; exit 2; }
if [[ "$MODE" == heap ]]; then
  command -v heaptrack >/dev/null && command -v heaptrack_print >/dev/null \
    || { echo "FATAL: MB_MODE=heap needs heaptrack + heaptrack_print on PATH (Linux)" >&2; exit 2; }
fi
QP=${MB_QP:-40}; PRESET=${MB_PRESET:-13}; CONTENT=${MB_CONTENT:-gradient}; SHIFT=${MB_SHIFT:-3}
OUT=${MB_OUT:-$HOME/tmp/mem_bisect}; mkdir -p "$OUT"
(( $# >= 1 )) || { echo "usage: $0 <perf_encode binary>..." >&2; exit 2; }
# Resolve every binary to an ABSOLUTE path: the run goes through `env`/`nice`,
# whose exec does a PATH lookup on a bare name — `perf_encode.abc` in the cwd
# is rc=127 (measured: sixteen binaries, 112 REFUSED rows, first run).
bins=()
for b in "$@"; do
  [[ -x "$b" ]] || { echo "FATAL: $b is not executable" >&2; exit 2; }
  bins+=("$(cd "$(dirname "$b")" && pwd)/$(basename "$b")")
done

case "$ARM" in
  still)    ev=() ;;
  videokey) ev=(SVTAV1_VIDEO=1 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0) ;;
  inter)    ev=(SVTAV1_VIDEO=1 SVTAV1_FRAMES=2 "SVTAV1_FRAME_SHIFT=$SHIFT" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0) ;;
  *) echo "FATAL: unknown arm $ARM" >&2; exit 2 ;;
esac
ev+=(SVTAV1_INTER_EXPERIMENTAL=1)
[[ -n "${MB_THREADS:-}" ]] && ev+=("SVTAV1_THREADS=$MB_THREADS")

BSD_TIME=false; /usr/bin/time -l true >/dev/null 2>&1 && BSD_TIME=true
rss_once() { # -> PEAK_KIB CMD_RC
    local t="$OUT/.t"
    if $BSD_TIME; then
        /usr/bin/time -l "$@" >/dev/null 2>"$t"; CMD_RC=$?
        PEAK_KIB=$(( $(grep -i "maximum resident" "$t" | awk '{print $1}') / 1024 ))
    else
        /usr/bin/time -v "$@" >/dev/null 2>"$t"; CMD_RC=$?
        PEAK_KIB=$(grep -i "Maximum resident" "$t" | awk '{print $NF}')
    fi
}

echo "mem_bisect: mode=$MODE size=$SIZE arm=$ARM rounds=$ROUNDS qp=$QP preset=$PRESET content=$CONTENT host=$(hostname) threads=${MB_THREADS:-auto}"
n=${#bins[@]}
if [[ "$MODE" == heap ]]; then
  printf 'bin\tpeak_heap_MB\trc\tobu_bytes\ttrace\n'
  for ((i = 0; i < n; i++)); do
    pfx="$OUT/c_${SIZE}_${ARM}_$i"; rm -f "$pfx".obu* "$OUT/ht_$i".*
    ( for kv in "${ev[@]}"; do export "${kv?}"; done
      nice -n 19 heaptrack -o "$OUT/ht_$i" "${bins[i]}" "$CONTENT" "$SIZE" "$SIZE" "$QP" "$PRESET" "$pfx" 1 >/dev/null 2>"$OUT/.ht_$i.err" )
    rc=$?
    obu=0; [[ -f "$pfx.obu" ]] && obu=$(wc -c <"$pfx.obu" | tr -d ' ')
    trace=$(ls "$OUT/ht_$i".* 2>/dev/null | head -1)
    peak=$(heaptrack_print "$trace" 2>/dev/null | grep -m1 "peak heap memory consumption" | sed 's/.*: *//')
    if [[ "$rc" != 0 || "$obu" -le 0 || -z "$peak" ]]; then peak="REFUSED"; fi
    printf '%s\t%s\t%s\t%s\t%s\n' "${bins[i]}" "$peak" "$rc" "$obu" "$trace"
  done
  echo "MEM_BISECT DONE"; exit 0
fi
declare -a vals refused obus
for ((i = 0; i < n; i++)); do vals[i]=""; refused[i]=0; obus[i]=""; done
for ((r = 0; r < ROUNDS; r++)); do
  for ((i = 0; i < n; i++)); do
    pfx="$OUT/c_${SIZE}_${ARM}_$i"; rm -f "$pfx".obu*
    rss_once env "${ev[@]}" nice -n 19 "${bins[i]}" "$CONTENT" "$SIZE" "$SIZE" "$QP" "$PRESET" "$pfx" 1
    obu=0; [[ -f "$pfx.obu" ]] && obu=$(wc -c <"$pfx.obu" | tr -d ' ')
    if [[ "$CMD_RC" != 0 || "$obu" -le 0 ]]; then
      refused[i]=$(( refused[i] + 1 )); echo "round $r bin $i ${bins[i]}: REFUSED rc=$CMD_RC obu=$obu" >&2
    else
      vals[i]+="$PEAK_KIB "; obus[i]="$obu"
    fi
  done
done
printf 'bin\tmedian_KiB\tmin_KiB\tmax_KiB\tn\trefused\tobu_bytes\traw_KiB\n'
for ((i = 0; i < n; i++)); do
  if [[ -z "${vals[i]}" ]]; then printf '%s\tREFUSED\t-\t-\t0\t%s\t-\t-\n' "${bins[i]}" "${refused[i]}"; continue; fi
  read -r med mn mx cnt <<<"$(printf '%s\n' ${vals[i]} | sort -n | awk '
    { v[NR] = $1 } END { printf "%d %d %d %d", (NR % 2) ? v[(NR+1)/2] : int((v[NR/2] + v[NR/2+1]) / 2), v[1], v[NR], NR }')"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "${bins[i]}" "$med" "$mn" "$mx" "$cnt" "${refused[i]}" "${obus[i]}" "${vals[i]% }"
done
echo "MEM_BISECT DONE"

#!/usr/bin/env bash
# PEAK MEMORY, port vs C, over the three arms and a size sweep — the harness
# `benchmarks/mem_heaptrack_*.meta` were produced with, kept in the repo instead
# of in a scratch directory that gets wiped.
#
#   tools/mem_peak.sh                       # rss, 4 sizes x 3 arms, both sides
#   MP_MODE=heap tools/mem_peak.sh          # peak HEAP via heaptrack (Linux)
#   MP_SIZES="2048" MP_ARMS=inter tools/mem_peak.sh
#
# TWO QUANTITIES, and they are NOT interchangeable.
#   rss  = `/usr/bin/time` maximum resident set size. Counts thread stacks,
#          static tables, and every page the allocator ever touched — including
#          memory that was freed and never returned to the OS. Allocator CHURN
#          therefore CAN move it.
#   heap = heaptrack's "peak heap memory consumption": the high-water mark of
#          LIVE malloc'd bytes. Churn cannot move it; only lifetimes can.
# A change can move one and not the other, so state which one a number is.
#
# REFUSAL TRAP (docs/WORKING-ON-THIS.md §5, benchmarks/mem_heaptrack_2026-09-03.meta):
# a memory number from a program that did not encode is not a memory number.
# The C harness once scored 12.66 M on an inter cell it REFUSED for want of a
# 2-frame `.yuv`. Every cell below checks the exit status AND a non-empty `.obu`
# before its peak is reported, and prints NO-OBU-REFUSED instead of a number.
#
# The port writes `<prefix>.yuv`; C reads it, so the port arm of a cell must run
# first — the loop does that.
#
# Env:
#   MP_MODE     rss | heap                       (default rss)
#   MP_SIZES    square dims                      (default "1280 1536 1920 2048")
#   MP_ARMS     still videokey inter             (default all three)
#   MP_QP / MP_PRESET / MP_CONTENT               (default 40 / 13 / gradient)
#   MP_SHIFT    px/frame translation, inter arm  (default 3)
#   MP_REPS     repeats per cell, median reported (rss only; default 5)
#   MP_SIDES    port c                           (default both)
#   MP_OUT      scratch dir                      (default $HOME/tmp/mem_peak)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
MODE=${MP_MODE:-rss}
SIZES=${MP_SIZES:-"1280 1536 1920 2048"}
ARMS=${MP_ARMS:-"still videokey inter"}
QP=${MP_QP:-40}
PRESET=${MP_PRESET:-13}
CONTENT=${MP_CONTENT:-gradient}
SHIFT=${MP_SHIFT:-3}
REPS=${MP_REPS:-5}
SIDES=${MP_SIDES:-"port c"}
OUT=${MP_OUT:-$HOME/tmp/mem_peak}
PE="$RS_ROOT/target/release/examples/perf_encode"
CE="$HERE/perf_c_encode/perf_c_encode"
mkdir -p "$OUT"

[[ -x "$PE" ]] || { echo "mem_peak: build the port first: cargo build --release --example perf_encode" >&2; exit 1; }
case " $SIDES " in *" c "*) [[ -x "$CE" ]] || { echo "mem_peak: $CE missing (tools/perf_c_encode/build.sh)" >&2; exit 1; };; esac
if [[ "$MODE" == heap ]]; then
    command -v heaptrack >/dev/null || { echo "mem_peak: heaptrack not on PATH (Linux only)" >&2; exit 1; }
fi

BSD_TIME=false
/usr/bin/time -l true >/dev/null 2>&1 && BSD_TIME=true

# Peak RSS in KiB of one command -> PEAK_KIB, CMD_RC.
rss_once() {
    local t="$OUT/.t"
    if $BSD_TIME; then
        /usr/bin/time -l "$@" >/dev/null 2>"$t"; CMD_RC=$?
        PEAK_KIB=$(( $(grep -i "maximum resident" "$t" | awk '{print $1}') / 1024 ))
    else
        /usr/bin/time -v "$@" >/dev/null 2>"$t"; CMD_RC=$?
        PEAK_KIB=$(grep -i "Maximum resident" "$t" | awk '{print $NF}')
    fi
}
# Median peak RSS over $REPS -> MED_KIB / MIN_KIB / MAX_KIB / CMD_RC (last run).
rss_median() {
    local vals=() i
    for ((i = 0; i < REPS; i++)); do rss_once "$@"; vals+=("$PEAK_KIB"); done
    read -r MED_KIB MIN_KIB MAX_KIB <<<"$(printf '%s\n' "${vals[@]}" | sort -n | awk '
        { v[NR] = $1 } END { printf "%d %d %d", (NR % 2) ? v[(NR+1)/2] : int((v[NR/2] + v[NR/2+1]) / 2), v[1], v[NR] }')"
}
# Peak live heap in MB of one command -> HEAP_MB, CMD_RC.
#
# heaptrack must wrap the BINARY ITSELF, not an `env`/`nice` prefix: wrapping
# the prefix profiles the prefix. MEASURED 2026-09-03 — `heaptrack ... env V=1
# nice -n 19 perf_encode ...` reported 220 KB for every cell, which is `env`'s
# own heap and not the encoder's. So the caller passes the env through the
# ENVIRONMENT (the arm sets it with `export` below) and the command here starts
# at the executable.
heap_once() { # <tag> <cmd...>
    local tag=$1; shift
    rm -f "$OUT/ht_$tag".*
    nice -n 19 heaptrack -o "$OUT/ht_$tag" "$@" >/dev/null 2>&1; CMD_RC=$?
    HEAP_MB=$(heaptrack_print "$OUT"/ht_"$tag".* 2>/dev/null \
        | grep -m1 "peak heap memory consumption" | sed 's/.*: *//')
}

echo "mem_peak: mode=$MODE content=$CONTENT qp=$QP preset=$PRESET reps=$REPS host=$(hostname)"
echo "mem_peak: port=$PE"
echo "mem_peak: C=$CE"
printf 'size\tarm\tside\tpeak\trc\tobu_bytes\n'
for s in $SIZES; do
  for arm in $ARMS; do
    case "$arm" in
      # The GOP env is the one tools/perf_gate.sh matches on BOTH sides, and it
      # is applied to the videokey arm too: C's default GOP is a random-access
      # pyramid whose reference pool is far larger than the low-delay-P one the
      # port encodes, so leaving it off measured C's videokey arm at 139.9 MiB
      # RSS against its own 2-FRAME arm's 106.1 (measured 2026-09-03) — a
      # 1-frame encode "costing" more than a 2-frame one is the tell.
      still)    pev=()
                cev=() ;;
      videokey) pev=(SVTAV1_VIDEO=1 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0
                     SVTAV1_INTER_EXPERIMENTAL=1)
                cev=(SVT_AVIF=0 SVT_FRAMES=1 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1) ;;
      inter)    pev=(SVTAV1_VIDEO=1 SVTAV1_FRAMES=2 "SVTAV1_FRAME_SHIFT=$SHIFT"
                     SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 SVTAV1_INTER_EXPERIMENTAL=1)
                cev=(SVT_AVIF=0 SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1) ;;
      *) echo "mem_peak: unknown arm $arm" >&2; exit 1 ;;
    esac
    pfx="$OUT/c_${s}_${arm}"
    for side in $SIDES; do
      if [[ $side == port ]]; then
        rm -f "$pfx".obu*
        cell_env=("${pev[@]}" SVTAV1_INTER_EXPERIMENTAL=1)
        bin=("$PE" "$CONTENT" "$s" "$s" "$QP" "$PRESET" "$pfx" 1)
        obuf="$pfx.obu"
      else
        rm -f "$pfx".c.obu*
        cell_env=("${cev[@]}" SVT_TRACE_OUT=/dev/null)
        bin=("$CE" "$s" "$s" "$QP" "$PRESET" "$pfx.yuv" "$pfx.c.obu" 1)
        obuf="$pfx.c.obu"
      fi
      if [[ "$MODE" == heap ]]; then
        ( for kv in "${cell_env[@]}"; do export "${kv?}"; done
          heap_once "${s}_${arm}_${side}" "${bin[@]}"
          printf '%s\t%s\n' "$HEAP_MB" "$CMD_RC" ) >"$OUT/.heap"
        read -r peak CMD_RC <"$OUT/.heap"
      else
        rss_median env "${cell_env[@]}" nice -n 19 "${bin[@]}"
        peak="$MED_KIB KiB [$MIN_KIB,$MAX_KIB]"
      fi
      obu=$(wc -c <"$obuf" 2>/dev/null | tr -d ' ' || echo 0); obu=${obu:-0}
      if [[ "$CMD_RC" != 0 || "$obu" -le 0 ]]; then peak="NO-OBU-REFUSED"; fi
      printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$s" "$arm" "$side" "$peak" "$CMD_RC" "$obu"
    done
  done
done
echo "MEM_PEAK DONE"

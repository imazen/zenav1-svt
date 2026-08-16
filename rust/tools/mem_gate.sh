#!/usr/bin/env bash
# Peak RSS of the port vs the C encoder, across image sizes.
#
#   tools/mem_gate.sh [preset]        # default 6
#
# WHY. `docs/ACCEPTANCE-CRITERIA.md` requires memory numbers to come from
# heaptrack or `time -v` and never from struct arithmetic — and then the project
# never measured any. "How much memory does it use?" had no answer at all.
#
# WHY MULTIPLE SIZES. A single figure is meaningless: peak RSS is
# `alpha + beta * pixels`, and at 64x64 the intercept dominates while at 1024x1024
# the slope does. Reporting one number without the intercept mis-sizes every
# decision at the other end of the range. This sweeps tiny -> large and prints
# both terms.
#
# `/usr/bin/time -l` (BSD/macOS) reports maximum resident set size; `-v` on GNU.
# Both are the sanctioned evidence. RSS is a ceiling on what the process touched,
# not an allocator trace — for allocation-level questions use heaptrack on Linux.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."
PRESET=${1:-6}
SIZES=${MEM_SIZES:-"64 256 512 1024"}
QP=${MEM_QP:-32}
W=${TMPDIR:-/tmp}/memgate.$$; mkdir -p "$W"; trap 'rm -rf "$W"' EXIT

# Portable "peak RSS in KiB" for one command.
peak_kib() {
    local out
    if /usr/bin/time -l true >/dev/null 2>&1; then           # BSD / macOS
        out=$(/usr/bin/time -l "$@" 2>&1 >/dev/null | grep -i "maximum resident" | awk '{print $1}')
        echo $(( out / 1024 ))                                # bytes -> KiB
    else                                                      # GNU
        out=$(/usr/bin/time -v "$@" 2>&1 >/dev/null | grep -i "Maximum resident" | awk '{print $NF}')
        echo "$out"                                           # already KiB
    fi
}

printf "%-7s %-6s %12s %12s %8s\n" "size" "px(MP)" "port KiB" "C KiB" "port/C"
rows=""
for s in $SIZES; do
    mp=$(awk -v s="$s" 'BEGIN{printf "%.3f", s*s/1048576}')
    p=$(peak_kib env SVTAV1_BD=8 "$HERE/identity_run" gradient "$s" "$s" "$QP" "$PRESET" "$W/rs")
    c=$(peak_kib env SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
            "$s" "$s" "$QP" "$PRESET" "$W/rs.yuv" "$W/c.obu" 8)
    ratio=$(awk -v a="$p" -v b="$c" 'BEGIN{ if (b>0) printf "%.2f", a/b; else print "-" }')
    printf "%-7s %-6s %12s %12s %8s\n" "${s}x${s}" "$mp" "$p" "$c" "$ratio"
    rows="$rows$s $p $c
"
done

# alpha + beta*pixels, least squares on both series. The INTERCEPT is the part a
# single-size measurement hides.
echo
printf '%s' "$rows" | awk '
  NF==3 { n++; x=$1*$1; sx+=x; sxx+=x*x; sp+=$2; sxp+=x*$2; sc+=$3; sxc+=x*$3 }
  END {
    if (n < 2) { print "  (need >= 2 sizes to fit)"; exit }
    d = n*sxx - sx*sx
    bp = (n*sxp - sx*sp)/d; ap = (sp - bp*sx)/n
    bc = (n*sxc - sx*sc)/d; ac = (sc - bc*sx)/n
    printf "  port:  %.1f MiB fixed  +  %.2f MiB/MP\n", ap/1024, bp*1048576/1024
    printf "  C   :  %.1f MiB fixed  +  %.2f MiB/MP\n", ac/1024, bc*1048576/1024
  }'

#!/usr/bin/env bash
# Sample the port or C encoder on one cell. Usage:
#   prof.sh port <size> <preset> <qp> <iters> <secs> <out>
#   prof.sh c    <size> <preset> <qp> <iters> <secs> <out>
set -uo pipefail
RS="${RS_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
W="${PROF_WORK:-$HOME/tmp/zsvtprof}"; mkdir -p "$W"
which=$1; sz=$2; preset=$3; qp=$4; iters=$5; secs=$6; out=$7

# ensure the yuv exists (port harness writes it)
"$RS/target-perf/release/examples/perf_encode" gradient "$sz" "$sz" "$qp" "$preset" "$W/in_${sz}_${qp}" 0 >/dev/null 2>&1

if [ "$which" = port ]; then
  "$RS/target-perf/release/examples/perf_encode" gradient "$sz" "$sz" "$qp" "$preset" "$W/o_p" "$iters" >/dev/null 2>&1 &
else
  "$RS/tools/perf_c_encode/perf_c_encode" "$sz" "$sz" "$qp" "$preset" "$W/in_${sz}_${qp}.yuv" "$W/o_c.obu" "$iters" >/dev/null 2>&1 &
fi
pid=$!
sleep 0.6
/usr/bin/sample "$pid" "$secs" 1 -file "$out" -mayDie >/dev/null 2>&1
kill -9 "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
echo "wrote $out"

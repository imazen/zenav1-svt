#!/usr/bin/env bash
# Sample the port or C encoder on one cell. Usage:
#   prof.sh port <size> <preset> <qp> <iters> <secs> <out>
#   prof.sh c    <size> <preset> <qp> <iters> <secs> <out>
#
# INTER cells: set the same env `perf_gate.sh` exports at PERF_FRAMES > 1
# (SVTAV1_FRAMES/SVTAV1_FRAME_SHIFT/SVTAV1_INTRA_PERIOD/SVTAV1_HIER_LEVELS/
# SVTAV1_INTER_EXPERIMENTAL on the port, SVT_FRAMES/SVT_INTRA_PERIOD/
# SVT_HIER_LEVELS/SVT_PRED_STRUCT on C) — both harnesses read it straight out
# of the inherited environment, so nothing here needs an inter-specific flag.
# NOTE the sample then covers the KEY frame as well; use enough frames that the
# inter ones dominate, and say which mix a share was measured over.
set -uo pipefail
RS="${RS_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
W="${PROF_WORK:-$HOME/tmp/zsvtprof}"; mkdir -p "$W"
which=$1; sz=$2; preset=$3; qp=$4; iters=$5; secs=$6; out=$7

# The perf harness binary. `target-perf` is the dedicated dir perf_gate builds
# into on the main checkout; a scratch workspace usually has only `target`.
# Resolving it here rather than hard-coding one path is what lets this script
# run in a jj workspace at all.
PE="$RS/target-perf/release/examples/perf_encode"
[ -x "$PE" ] || PE="$RS/target/release/examples/perf_encode"
[ -x "$PE" ] || { echo "prof.sh: no perf_encode binary (looked in target-perf/ and target/)" >&2; exit 1; }

# ensure the yuv exists (port harness writes it)
"$PE" gradient "$sz" "$sz" "$qp" "$preset" "$W/in_${sz}_${qp}" 0 >/dev/null 2>&1

if [ "$which" = port ]; then
  "$PE" gradient "$sz" "$sz" "$qp" "$preset" "$W/o_p" "$iters" >/dev/null 2>&1 &
else
  "$RS/tools/perf_c_encode/perf_c_encode" "$sz" "$sz" "$qp" "$preset" "$W/in_${sz}_${qp}.yuv" "$W/o_c.obu" "$iters" >/dev/null 2>&1 &
fi
pid=$!
sleep 0.6
/usr/bin/sample "$pid" "$secs" 1 -file "$out" -mayDie >/dev/null 2>&1
kill -9 "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
echo "wrote $out"

#!/usr/bin/env bash
# Full CPU POSITION run, port vs C — the six arms the position records use
# (`benchmarks/perf_<date>-<host>-POSITION.meta`), driven from ONE script so
# the recipe is not re-typed by hand per host:
#
#   gradient qp 40, sizes 64/128/256/512, preset 8:   still / videokey / inter
#   real photo (CID22 3571065, 512x512) p2 + p6:        still / videokey / inter
#
# plus the SAME-BINARY controls the trap list demands (`perf_ab.sh` port vs a
# byte-identical copy of itself) so the harness floor is measured IN THIS
# SESSION ON THIS BOX and never borrowed from another lane or host.
#
# Around every arm it snapshots `uptime` + the top CPU consumers into
# `benchmarks/perf_<suffix>-quiet.txt` (before, every 15 s inside, after), so a
# perturbed arm can be seen and discarded rather than averaged in. It also
# re-claims `<repo>/.workongoing` between arms with the id in `$POS_AGENT`
# (perf_gate.sh writes `rust/.workongoing` with its own hard-coded id).
#
# Builds are the callers job (do them under run-heavy); the timed encodes are
# NOT niced — same rule as perf_ab.sh / perf_gap_campaign.sh.
#
# Usage: tools/perf_position.sh <suffix> <photo.yuv>
#   e.g. tools/perf_position.sh 2026-09-05-lilith1 ~/tmp/pos-lilith/yuv/photo_cid.yuv
# Env:   PERF_ROUNDS (default 25), POS_AGENT (marker id), POS_GRAD_PRESETS
#        (gradient presets, default 8),
#        POS_ARMS (space list from: ctl still videokey inter photo-still
#        photo-videokey photo-inter; default all)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
REPO=$(cd "$RS_ROOT/.." && pwd)
cd "$RS_ROOT"

SUFFIX="${1:?usage: perf_position.sh <suffix> <photo.yuv>}"
PHOTO="${2:?usage: perf_position.sh <suffix> <photo.yuv>}"
[[ -s "$PHOTO" ]] || { echo "photo yuv missing: $PHOTO" >&2; exit 1; }
ROUNDS="${PERF_ROUNDS:-25}"
AGENT="${POS_AGENT:-claude-perf-position}"
ARMS="${POS_ARMS:-ctl still videokey inter photo-still photo-videokey photo-inter}"
QUIET="$RS_ROOT/benchmarks/perf_${SUFFIX}-quiet.txt"
PE="$RS_ROOT/target/release/examples/perf_encode"
[[ -x "$PE" ]] || { echo "build perf_encode first" >&2; exit 1; }

snap() { # <label>
    {
        echo "--- $1  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        uptime
        ps -eo pid,pcpu,pmem,comm --sort=-pcpu | head -6
    } >>"$QUIET"
}
claim() { printf "%s %s %s\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$AGENT" "perf_position $1" >"$REPO/.workongoing"; }
arm() { # <name> <cmd...>
    local name=$1; shift
    claim "$name"
    snap "BEFORE $name"
    ( while :; do sleep 15; snap "DURING $name"; done ) & local sp=$!
    "$@" 2>&1 | tee "$RS_ROOT/benchmarks/perf_${SUFFIX}-${name}.log"
    kill "$sp" 2>/dev/null; wait "$sp" 2>/dev/null
    snap "AFTER $name"
}

echo "# perf_position quiet log — suffix=$SUFFIX host=$(hostname) $(uname -r) $(nproc) cores" >"$QUIET"
grep -E "MemTotal|MemAvailable" /proc/meminfo >>"$QUIET"
grep -m1 "model name" /proc/cpuinfo >>"$QUIET"

G=(PERF_SIZES="64 128 256 512" PERF_PRESETS="${POS_GRAD_PRESETS:-8}" PERF_CONTENT=gradient PERF_QP=40 PERF_ROUNDS="$ROUNDS" PERF_WARMUP=1)
P=(PERF_SIZES=512 PERF_PRESETS="2 6" PERF_CONTENT="raw:$PHOTO" PERF_QP=40 PERF_ROUNDS="$ROUNDS" PERF_WARMUP=1)

for a in $ARMS; do case "$a" in
  ctl)
    cp -f "$PE" "$RS_ROOT/target/perf_encode.ctl"
    arm ctl-gradient env AB_SIZES="64 512" AB_PRESETS=8 AB_CONTENT=gradient AB_QP=40 AB_ROUNDS=21 \
        tools/perf_ab.sh "$PE" "$RS_ROOT/target/perf_encode.ctl" "$RS_ROOT/benchmarks/perf_${SUFFIX}-ctl-gradient.tsv"
    arm ctl-photo env AB_SIZES=512 AB_PRESETS="2 6" AB_CONTENT="raw:$PHOTO" AB_QP=40 AB_ROUNDS=21 \
        tools/perf_ab.sh "$PE" "$RS_ROOT/target/perf_encode.ctl" "$RS_ROOT/benchmarks/perf_${SUFFIX}-ctl-photo.tsv" ;;
  still)          arm still          env "${G[@]}"                                            tools/perf_gate.sh "${SUFFIX}-still" ;;
  videokey)       arm videokey       env "${G[@]}" PERF_VIDEO=1                               tools/perf_gate.sh "${SUFFIX}-videokey" ;;
  inter)          arm inter          env "${G[@]}" PERF_VIDEO=1 PERF_FRAMES=2 PERF_SHIFT=3    tools/perf_gate.sh "${SUFFIX}-inter" ;;
  photo-still)    arm photo-still    env "${P[@]}"                                            tools/perf_gate.sh "${SUFFIX}-photo-still" ;;
  photo-videokey) arm photo-videokey env "${P[@]}" PERF_VIDEO=1                               tools/perf_gate.sh "${SUFFIX}-photo-videokey" ;;
  photo-inter)    arm photo-inter    env "${P[@]}" PERF_VIDEO=1 PERF_FRAMES=2 PERF_SHIFT=3    tools/perf_gate.sh "${SUFFIX}-photo-inter" ;;
  *) echo "unknown arm $a" >&2; exit 2 ;;
esac; done
rm -f "$RS_ROOT/.workongoing"
claim "DONE"
echo "perf_position: DONE $(date -u +%Y-%m-%dT%H:%M:%SZ)"

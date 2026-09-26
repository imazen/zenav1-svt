#!/usr/bin/env bash
# Region-coverage floor for the workspace test suite.
#
#   tools/coverage_gate.sh            # measure, compare against the floor, print
#   COV_WRITE=1 tools/coverage_gate.sh  # re-record the floor after a deliberate rise
#
# WHY THIS EXISTS
#
# Coverage had never been measured in this repo. The first measurement
# (2026-09-09, benchmarks/coverage_2026-09-09.meta) was 91.83% of 173,483
# regions. Nothing defended it, so a change could delete a test — or add a large
# untested module — and every gate would stay green.
#
# WHAT THIS NUMBER DOES **NOT** INCLUDE: the CI gate scripts. They verify
# the inter/video tools against C and a decoder, but they are not nextest,
# so for a long time those paths read as uncovered here. Since 2026-09-26
# svtav1/tests/video_paths.rs drives the public video pipeline (LD at low and
# high presets, RA with and without TPL, CBR) on moving content: it took the
# total from 88.06% to 91.50% locally and found a TPL panic on its first
# run. Paths still only the gates reach should get a smoke test there, not a
# lower floor.
#
# So this gate defends the nextest surface against silent test loss. It is a
# floor, not a target: raising it is good, and COV_WRITE=1 records the new value.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
FLOOR_FILE="$RS_ROOT/benchmarks/coverage_floor.txt"
# Slack absorbs ordinary churn (a new match arm, a new error branch) without
# nagging, while still catching a deleted test or a large untested module.
SLACK="${COV_SLACK:-0.50}"

command -v cargo-llvm-cov >/dev/null 2>&1 || {
    echo "coverage_gate: cargo-llvm-cov not installed (cargo install cargo-llvm-cov --locked)" >&2
    exit 2
}

cd "$RS_ROOT"
OUT="${TMPDIR:-$HOME/tmp}/coverage_gate.$$.log"
cargo llvm-cov nextest --workspace --locked --no-fail-fast --summary-only \
    --ignore-filename-regex '(tests?/|examples/|benches/)' >"$OUT" 2>&1 || {
    echo "coverage_gate: the instrumented test run FAILED — that is a test failure, not a coverage result" >&2
    tail -30 "$OUT" >&2
    exit 1
}

read -r REGIONS MISSED PCT < <(
    awk '/^TOTAL/ { gsub(/%/,"",$4); print $2, $3, $4; exit }' "$OUT"
)
[ -n "${PCT:-}" ] || { echo "coverage_gate: could not parse a TOTAL line from $OUT" >&2; exit 2; }

if [ "${COV_WRITE:-0}" = "1" ]; then
    printf '%s\n' "$PCT" >"$FLOOR_FILE"
    echo "coverage_gate: recorded floor $PCT% ($REGIONS regions, $MISSED missed)"
    exit 0
fi

[ -f "$FLOOR_FILE" ] || { echo "coverage_gate: no floor recorded; run COV_WRITE=1 $0" >&2; exit 2; }
FLOOR=$(tr -d '[:space:]' <"$FLOOR_FILE")

if awk -v p="$PCT" -v f="$FLOOR" -v s="$SLACK" 'BEGIN { exit !(p + s < f) }'; then
    echo "coverage_gate: FAIL — region coverage $PCT% is below the recorded floor $FLOOR% (slack $SLACK)" >&2
    echo "  $REGIONS regions, $MISSED missed. A drop this size usually means a test stopped running." >&2
    echo "  If the drop is deliberate and justified, re-record with COV_WRITE=1 $0 and say why in the commit." >&2
    exit 1
fi

echo "coverage_gate: OK — $PCT% region coverage ($REGIONS regions, $MISSED missed); floor $FLOOR% slack $SLACK"

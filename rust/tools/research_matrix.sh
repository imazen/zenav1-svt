#!/usr/bin/env bash
# Byte-identity matrix for C's RESEARCH preset -1.
#
#   tools/research_matrix.sh
#   RM_CONTENTS="uniform gradient" RM_DIMS="64x64" RM_QPS="20" RM_BDS="8" tools/research_matrix.sh
#
# WHY THIS EXISTS
#
# ENCODER-POLICY-GOAL.md section 3 requires proving -1 parity "across useful
# quality levels, content classes, native 8/10-bit input, boundaries and tile
# settings", and completion gate 2 requires research behaviour to have
# ENABLED-feature tests rather than only neutral/off ones. Until 2026-09-09 there
# was no such gate anywhere: `grep -n 'research\|preset -1'` over
# .github/workflows/rust-gates.yml returned ZERO hits, identity_full_8bit.sh and
# bd10_matrix.sh never emit -1, and the only -1 coverage was 7 cells in
# regression_spotcheck.sh. docs/research-preset-port-map.md said outright that
# "-1 is not yet a verified parity mode".
#
# FIRST MEASUREMENT (2026-09-09, i265, benchmarks/research_matrix_2026-09-09.meta):
# 88 / 88 byte-identical, over both bit depths, four content classes, and
# geometries including partial-SB and odd dimensions. So -1 IS a verified parity
# mode in this envelope, and this script is what keeps it one.
#
# SCOPE: still/KEY frames, 4:2:0, single tile. Tile settings are NOT swept here --
# tile_gate.sh owns that axis and does not run -1; extending it is follow-up work.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)

read -r -a CONTENTS <<<"${RM_CONTENTS:-uniform gradient diag screen}"
read -r -a DIMS     <<<"${RM_DIMS:-64x64 65x67 120x104 128x128}"
read -r -a QPS      <<<"${RM_QPS:-20 55}"
read -r -a BDS      <<<"${RM_BDS:-8 10}"
CELL_TIMEOUT="${RM_CELL_TIMEOUT:-300}"

pass=0
fail=0
failed=()

for content in "${CONTENTS[@]}"; do
  for dim in "${DIMS[@]}"; do
    w=${dim%x*}
    h=${dim#*x}
    for qp in "${QPS[@]}"; do
      for bd in "${BDS[@]}"; do
        cell="${content}_${dim}_q${qp}_bd${bd}"
        out="${TMPDIR:-/tmp}/research_matrix.$$/$cell"
        if SVTAV1_BD="$bd" timeout "$CELL_TIMEOUT" \
             bash "$HERE/identity_diff.sh" "$w" "$h" "$qp" -1 "$content" "$out" \
             >/dev/null 2>&1; then
          pass=$((pass + 1))
        else
          fail=$((fail + 1))
          failed+=("$cell")
        fi
      done
    done
  done
done

total=$((pass + fail))
echo "research preset -1 identity: $pass / $total byte-identical"
if ((fail > 0)); then
  printf '  FAILED: %s\n' "${failed[*]}"
  echo "FAIL research_matrix" >&2
  exit 1
fi
echo "PASS research_matrix"

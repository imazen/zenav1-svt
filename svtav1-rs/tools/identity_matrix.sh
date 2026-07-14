#!/usr/bin/env bash
# Identity-matrix gate: sweep (content x size x cli_qp x preset), run the
# bitstream-identity differ on each cell, and tally byte-identical vs not
# with the first-divergence STAGE classified (SH / FH / tile-ops / tile-size)
# so the decision-parity campaign can see exactly where identity breaks and
# which fix unlocks the most cells.
#
# Writes a scoreboard to benchmarks/identity_matrix_<date>.tsv (pass a date
# suffix as $1; default 'latest') and prints a summary. Exit 0 always (this
# is a measurement/tracking gate, not pass/fail — identity is a long ratchet).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
SUFFIX="${1:-latest}"
OUT="$RS_ROOT/benchmarks/identity_matrix_${SUFFIX}.tsv"
mkdir -p "$RS_ROOT/benchmarks"

# Overridable via env for broader sweeps, e.g.
#   IM_PRESETS="0 1 2 3 4 5 6 7 8 9 10" IM_CONTENTS="uniform gradient photo"
read -r -a CONTENTS <<<"${IM_CONTENTS:-uniform gradient}"
read -r -a SIZES <<<"${IM_SIZES:-64 128}"
read -r -a QPS <<<"${IM_QPS:-20 40 55}"
read -r -a PRESETS <<<"${IM_PRESETS:-13 10 6}"
# Per-cell wall-clock guard: an unported preset path may hang (as the M6
# funnel did on chroma sub-8x8) — cap each cell so the sweep still finishes
# and classifies the hang instead of stalling.
CELL_TIMEOUT="${IM_CELL_TIMEOUT:-90}"

printf 'content\tsize\tcli_qp\tpreset\tverdict\tstage\tdetail\n' >"$OUT"
identical=0
total=0

for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for preset in "${PRESETS[@]}"; do
        total=$((total + 1))
        d="$RS_ROOT/target/identity/m_${content}_${sz}_q${qp}_p${preset}"
        rep="$d/report.txt"
        timeout "$CELL_TIMEOUT" "$HERE/identity_diff.sh" "$sz" "$sz" "$qp" "$preset" "$content" "$d" \
             >/dev/null 2>&1
        rc=$?
        if [[ $rc -eq 0 ]]; then
          printf '%s\t%s\t%s\t%s\tIDENTICAL\t-\t-\n' \
            "$content" "$sz" "$qp" "$preset" >>"$OUT"
          identical=$((identical + 1))
          continue
        fi
        if [[ $rc -eq 124 ]]; then
          printf '%s\t%s\t%s\t%s\tDIFFERS\tHANG\t(cell timed out at %ss)\n' \
            "$content" "$sz" "$qp" "$preset" "$CELL_TIMEOUT" >>"$OUT"
          continue
        fi
        # Classify the first divergence stage from the differ report.
        stage="unknown"; detail="-"
        if [[ -f "$rep" ]]; then
          if grep -q "SEQUENCE_HEADER: .* -> DIFFERS" "$rep"; then
            stage="SH"
            detail=$(grep -m1 -oE "seq_level_idx.*|color_.*|enable_.*|separate_uv.*|film_grain.*" "$rep" | head -1)
          elif grep -q "FRAME.* -> DIFFERS" "$rep" && grep -q "FRAME field walk" "$rep" \
               && grep -qE "DIFF #.*: C @" "$rep"; then
            stage="FH"
            detail=$(grep -m1 -oE "base_q_idx.*|loop_filter.*|cdef_.*|lr_type.*|tx_mode.*|delta_q.*" "$rep" | head -1)
          elif grep -q "first divergence at op" "$rep"; then
            stage="tile-op"
            detail=$(grep -m1 "first divergence at op" "$rep" | grep -oE "op [0-9]+")
          elif grep -q "op counts" "$rep"; then
            stage="tile-count"
            detail=$(grep -m1 "op counts" "$rep")
          fi
        fi
        printf '%s\t%s\t%s\t%s\tDIFFERS\t%s\t%s\n' \
          "$content" "$sz" "$qp" "$preset" "$stage" "$detail" >>"$OUT"
      done
    done
  done
done

echo "identity matrix: $identical / $total byte-identical"
echo "first-divergence stage histogram (non-identical cells):"
awk -F'\t' 'NR>1 && $5=="DIFFERS" {print $6}' "$OUT" | sort | uniq -c | sort -rn
echo "scoreboard: $OUT"

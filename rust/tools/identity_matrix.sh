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
# Default sizes: 64 and 128 (full-SB), plus 60 — an arbitrary (non-64,
# even) dimension that aligns to a single 64x64 SB, guarding the task #95
# chunk-1 arbitrary-dimensions path (input pad + true-size seq header +
# small-frame restoration disable). Partial-SB sizes (56, 200, ...) are
# chunk 2 and not yet in the default gate.
read -r -a CONTENTS <<<"${IM_CONTENTS:-uniform gradient}"
read -r -a SIZES <<<"${IM_SIZES:-64 128 60}"
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
        # Classify from the differ's concise `STAGE: <stage> | <detail>` line.
        stage="unknown"; detail="-"
        if [[ -f "$rep" ]]; then
          line=$(grep -m1 "^STAGE: " "$rep" || true)
          if [[ -n "$line" ]]; then
            rest="${line#STAGE: }"
            stage="${rest%% | *}"
            detail="${rest#* | }"
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

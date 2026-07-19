#!/usr/bin/env bash
# Tile-configuration MAP — the measurement sweep behind tools/tile_gate.sh.
#
# For every (size, tile_rows_log2, tile_cols_log2, qp, preset) cell it runs
# BOTH encoders on the same .yuv and records, as TSV:
#
#   size  r  c  qp  preset  c_bytes  rs_bytes  verdict  decode  c_vs_c0  stage
#
#   verdict : MATCH | DIFF | c-err | rs-err
#   decode  : DEC | UNDEC (aomdec on the PORT's stream — a byte gate alone is
#             structurally blind to corruption among expected-DIFF cells)
#   c_vs_c0 : ANTI-VACUITY. The C oracle's bytes for this tile request vs the
#             C oracle's bytes at rows=cols=0 on the SAME input. DIFFER means
#             the request genuinely changed the reference encode; SAME means
#             it did not (clamped away by the geometry) and the cell proves
#             nothing about tiling however the request was spelled.
#   stage   : on DIFF, identity_diff.py's first-divergence stage (SH / FH /
#             tile-op), so the map says WHERE it diverges, not just that it does.
#
# Usage: tile_map.sh [out.tsv]
# Env:   TILE_MAP_QUICK=1  — the small grid (smoke); default is the full sweep.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/tilemap.$$"
mkdir -p "$OUT"
TSV="${1:-$RS_ROOT/benchmarks/tile_map_latest.tsv}"

aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  for cand in /root/aomdec-build/aomdec /root/aomdec-debug/aomdec; do
    [ -x "$cand" ] && { aomdec="$cand"; break; }
  done
fi
command -v "$aomdec" >/dev/null 2>&1 || [ -x "$aomdec" ] || aomdec=""

# Sizes chosen so the tile grid is actually exercised (a tile is >= 1 SB):
#   256x256 -> 4x4 SBs  : every log2 divides evenly (clean power-of-two grid)
#   512x384 -> 8x6 SBs  : 6 rows do NOT divide by 4 -> C codes 3 tiles at
#                         rows_log2=2, the case that used to emit an
#                         out-of-range context_update_tile_id
#   640x448 -> 10x7 SBs : neither axis divides -> ragged on BOTH axes
SIZES=("256 256" "512 384" "640 448")
QPS=(20 45)
PRESETS=(6 10 13)
CONTENT=gradient
if [ -n "${TILE_MAP_QUICK:-}" ]; then
  SIZES=("512 384"); QPS=(45); PRESETS=(6)
fi

printf 'size\tr\tc\tqp\tpreset\tc_bytes\trs_bytes\tverdict\tdecode\tc_vs_c0\tstage\n' >"$TSV"

n=0
for sz in "${SIZES[@]}"; do
  read -r w h <<<"$sz"
  for r in 0 1 2; do
    for c in 0 1 2; do
      for qp in "${QPS[@]}"; do
        for p in "${PRESETS[@]}"; do
          n=$((n + 1))
          tag="${w}x${h}_r${r}c${c}_q${qp}_p${p}"
          rsb=-1; cb=-1; verdict=DIFF; dec=n/a; cvc0=-; stage=-

          if ! SVTAV1_TILE_ROWS_LOG2=$r SVTAV1_TILE_COLS_LOG2=$c \
               "$HERE/identity_run" "$CONTENT" "$w" "$h" "$qp" "$p" "$OUT/rs" \
               >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
            verdict=rs-err
          elif ! SVT_TILE_ROWS=$r SVT_TILE_COLUMNS=$c \
                 SVT_TRACE_OUT="$OUT/c.trace" \
                 "$HERE/capture_c_trace/capture_c_trace" \
                 "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" \
                 >"$OUT/c.log" 2>"$OUT/c.stderr"; then
            verdict=c-err
          else
            rsb=$(stat -c%s "$OUT/rs.obu")
            cb=$(stat -c%s "$OUT/c.obu")
            # Anti-vacuity reference: the SAME input at rows=cols=0.
            if SVT_TILE_ROWS=0 SVT_TILE_COLUMNS=0 SVT_TRACE_OUT=/dev/null \
               "$HERE/capture_c_trace/capture_c_trace" \
               "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c0.obu" \
               >/dev/null 2>&1; then
              if cmp -s "$OUT/c.obu" "$OUT/c0.obu"; then cvc0=SAME; else cvc0=DIFFER; fi
            fi
            if [ -n "$aomdec" ]; then
              if "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
                dec=DEC
              else
                dec=UNDEC
              fi
            fi
            if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
              verdict=MATCH
            else
              verdict=DIFF
              stage=$(python3 "$HERE/identity_diff.py" \
                        --c-obu "$OUT/c.obu" --rust-obu "$OUT/rs.obu" \
                        --c-trace "$OUT/c.trace" --rust-trace "$OUT/rs.trace" \
                        2>/dev/null | grep -m1 '^STAGE:' | sed 's/^STAGE: *//' \
                        | cut -c1-40 | tr '\t' ' ')
              [ -z "$stage" ] && stage=?
            fi
          fi
          printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "${w}x${h}" "$r" "$c" "$qp" "$p" "$cb" "$rsb" \
            "$verdict" "$dec" "$cvc0" "$stage" >>"$TSV"
          printf '%-28s C=%-7s RS=%-7s %-7s %-5s %-6s %s\n' \
            "$tag" "$cb" "$rsb" "$verdict" "$dec" "$cvc0" "$stage"
        done
      done
    done
  done
done

rm -rf "$OUT"
echo
echo "cells: $n   tsv: $TSV"
awk -F'\t' 'NR>1{v[$8]++; d[$9]++} END {
  printf "verdict:"; for (k in v) printf " %s=%d", k, v[k]; printf "\n";
  printf "decode :"; for (k in d) printf " %s=%d", k, d[k]; printf "\n" }' "$TSV"
awk -F'\t' 'NR>1 && $10=="SAME" {n++} END {printf "vacuous (C ignored the request): %d\n", n+0}' "$TSV"

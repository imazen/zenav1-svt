#!/usr/bin/env bash
# VIDEO-MODE KEY-FRAME byte-identity matrix — the inter campaign's scoreboard.
#
# The still sibling is `identity_matrix.sh`. This one drives the SAME cell
# shape the campaign's `benchmarks/video_key_matrix_*.tsv` records: both sides
# encode a 2-frame low-delay-P stream of the same `.yuv` and only FRAME 0 is
# compared, because the port refuses frame 1 (exit 3) while the inter chunks
# are still landing. A refusal is not a failure here — see
# `docs/WORKING-ON-THIS.md` §6.
#
# It exists because §1m..§1p each re-derived this loop by hand, and a hand
# loop is exactly what the "silent harness" trap in §5 warns about: it prints
# ONE LINE PER CELL, always, so a cell that never ran is visible.
#
# Usage: tools/video_key_matrix.sh [outdir]
# Env:
#   VKM_W / VKM_H / VKM_QP   cell geometry (default 72 / 88 / 40)
#   VKM_CONTENT              space-separated (default the five synthetic classes)
#   VKM_PRESETS              space-separated (default 0 3 4 5 6 7 8 9 10 11 12 13)
#   VKM_FRAMES               default 2
#
# Output: a TSV on stdout (content, preset, c_bytes, port_bytes, pct, verdict)
# plus a summary line. Exit 0 iff every cell is IDENTICAL.
#
# READ THE VERDICT COLUMN, NOT THE PERCENTAGE. A zero percentage is a SIZE
# claim, not a byte claim — this campaign has had a cell read 0.000 % while
# differing (docs/INTER-ENCODE-PLAN.md §1i).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"

W="${VKM_W:-72}"; H="${VKM_H:-88}"; QP="${VKM_QP:-40}"
FRAMES="${VKM_FRAMES:-2}"
CONTENTS="${VKM_CONTENT:-gradient diag screen screenrep uniform}"
PRESETS="${VKM_PRESETS:-0 3 4 5 6 7 8 9 10 11 12 13}"
OUT="${1:-$RS_ROOT/target/video-key-matrix}"
mkdir -p "$OUT"

RUN="$HERE/identity_run"
CT="$HERE/capture_c_trace/capture_c_trace"

printf 'content\tpreset\tc_bytes\tport_bytes\tpct\tverdict\n'
ident=0; diff=0; broke=0
for content in $CONTENTS; do
  for p in $PRESETS; do
    d="$OUT/${content}_p${p}"
    mkdir -p "$d"
    rs=0
    SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
      $LOWPRI "$RUN" "$content" "$W" "$H" "$QP" "$p" "$d/rs" \
      >"$d/rs.out" 2>"$d/rs.trace" || rs=$?
    # 3 = the frame-1 inter refusal, which is the expected state.
    if [[ $rs -ne 0 && $rs -ne 3 ]]; then
      printf '%s\t%s\t-\t-\t-\tPORTFAIL(%d)\n' "$content" "$p" "$rs"
      broke=$((broke+1)); continue
    fi
    if [[ ! -s "$d/rs.obu.f0" ]]; then
      printf '%s\t%s\t-\t-\t-\tNOFRAME0\n' "$content" "$p"
      broke=$((broke+1)); continue
    fi
    SVT_FRAMES="$FRAMES" SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
      SVT_TRACE_OUT=/dev/null \
      $LOWPRI "$CT" "$W" "$H" "$QP" "$p" "$d/rs.yuv" "$d/c.obu" 8 \
      >"$d/c.out" 2>"$d/c.err" || {
        printf '%s\t%s\t-\t-\t-\tCFAIL\n' "$content" "$p"
        broke=$((broke+1)); continue
      }
    cb=$(wc -c <"$d/c.obu.pts0" | tr -d ' ')
    pb=$(wc -c <"$d/rs.obu.f0" | tr -d ' ')
    pct=$(awk -v c="$cb" -v r="$pb" 'BEGIN{ if(c==0){print "-"}else{printf "%.3f", (c>r?c-r:r-c)*100.0/c} }')
    if cmp -s "$d/c.obu.pts0" "$d/rs.obu.f0"; then
      printf '%s\t%s\t%s\t%s\t%s\tIDENTICAL\n' "$content" "$p" "$cb" "$pb" "$pct"
      ident=$((ident+1))
    else
      printf '%s\t%s\t%s\t%s\t%s\tdiff\n' "$content" "$p" "$cb" "$pb" "$pct"
      diff=$((diff+1))
    fi
  done
done
total=$((ident+diff+broke))
printf '# %d / %d byte-identical (%d diff, %d broken)\n' "$ident" "$total" "$diff" "$broke"
[[ $diff -eq 0 && $broke -eq 0 ]]

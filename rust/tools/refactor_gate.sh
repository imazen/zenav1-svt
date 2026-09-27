#!/usr/bin/env bash
# Refactor-preflight byte-parity gate.
#
# WHAT IT ASSERTS. One pinned census cell per preset (-1..13) — the rows the
# census already carries a verdict for — plus a byte-identical stills slice
# and a bd10 slice. The cell list is curated so EVERY pipeline arm a
# signature-level refactor can perturb runs at least once:
#
#   * the PD1 depth-refine walk (`decide_sb_refined`, DepthWalk::pick) —
#     inter cells at presets where NSQ/adaptive arms fire;
#   * `encode_fixed_tree` — still cells at the funnel presets;
#   * `bd10_reencode_{luma,chroma}` walks — the bd10 cells;
#   * `encode_coding_unit` / `encode_tile_rows` — every cell, every preset.
#
# A verdict must EQUAL the census pin (tools/pins/video_census.tsv) for the
# inter cells — a refactor that improves OR regresses a cell fails the same
# way the census does, because the pins are the oracle. Stills and bd10
# cells must be byte-identical to C (their pinned envelope already is).
#
# This is the ~90-second subset of `video_census_gate.sh` (540 cells,
# ~5 min at 4 jobs): run it before and after a signature/structure refactor;
# run the census itself when a semantic change is expected to move a pin.
#
# Usage: tools/refactor_gate.sh
# Env: RG_JOBS (4), SVT_ORACLE (oracle name, as identity_run/capture_c_trace)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"

RUN="$HERE/identity_run"
CT="$HERE/capture_c_trace/capture_c_trace"
PINS="$HERE/pins/video_census.tsv"
W="${TMPDIR:-$HOME/tmp}/refactor-gate.$$"
mkdir -p "$W/inter"
trap 'rm -rf "$W"' EXIT

# ---------------------------------------------------------------- inter ---
# One cell per preset, spread across clips/sizes/qps so the sweep also walks
# different content classes. Chosen from the pinned census so every expected
# verdict already exists — a refactor sees the same oracle the census does.
INTER_CELLS="
fourpeople       128  20  -1
johnny           128  40  0
kristenandsara   256  55  1
vidyo1           128  20  2
vidyo3           256  40  3
fourpeople       256  20  4
johnny           256  55  5
kristenandsara   128  40  6
vidyo1           256  20  7
vidyo3           128  55  8
vidyo4           128  20  9
fourpeople       128  40  10
johnny           256  20  11
kristenandsara   128  55  12
vidyo1           256  40  13
"

# The census feeds identity_run a `rawseq:<i420>` path under the derf asset
# dir — bare clip names are not a content kind.
DERF="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
if [ ! -f "$DERF/vidyo3_128x128_8f.i420" ]; then
  "$HERE/fetch_r2_assets.sh" "video/pd-derf-720p/" "$DERF" >&2 || {
    echo "refactor gate: could not fetch the derf clips" >&2
    exit 2
  }
fi

# One cell in its own workdir; writes "<label>\t<got>\t<want>" to stdout.
# The parent joins the results — a subshell cannot pass/fail on its own.
inter_cell() {
  local content=$1 size=$2 qp=$3 p=$4 wd=$5
  local label="inter-${content}-${size}-q${qp}-p${p}"
  local want
  want=$(awk -v c="$content" -v s="$size" -v q="$qp" -v p="$p" \
    '$1==c && $2==s && $3==q && $4==p {print $6; exit}' "$PINS")
  if [ -z "$want" ]; then
    printf '%s\t%s\t%s\n' "$label" "NOPIN" "-"; return
  fi
  if ! SVTAV1_FRAMES=8 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
       $LOWPRI "$RUN" "rawseq:$DERF/${content}_${size}x${size}_8f.i420" "$size" "$size" "$qp" "$p" "$wd/rs" >/dev/null 2>&1; then
    printf '%s\t%s\t%s\n' "$label" "PORT-FAILED" "$want"; return
  fi
  if ! SVT_FRAMES=8 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
       SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$size" "$size" "$qp" "$p" \
       "$wd/rs.yuv" "$wd/c.obu" 8 >/dev/null 2>&1; then
    printf '%s\t%s\t%s\n' "$label" "C-FAILED" "$want"; return
  fi
  local got=IDENTICAL i
  for i in 0 1 2 3 4 5 6 7; do
    local cf="$wd/c.obu.pts$i" rf="$wd/rs.obu.f$i"
    if [ ! -e "$cf" ] || [ ! -e "$rf" ]; then
      got=ERROR; break
    fi
    if ! cmp -s "$cf" "$rf"; then
      got="TU$i"; break
    fi
  done
  printf '%s\t%s\t%s\n' "$label" "$got" "$want"
}
echo "== refactor gate: pinned inter cells (one per preset) ==" >&2
printf '%s\n' "$INTER_CELLS" | sed 's/^ *//;s/ *$//' | grep -v '^$' > "$W/cells.txt"
nl=0
running=0
while read -r c s q p; do
  mkdir -p "$W/inter/$nl"
  inter_cell "$c" "$s" "$q" "$p" "$W/inter/$nl" >> "$W/inter-results.tsv" </dev/null &
  nl=$((nl+1))
  running=$((running+1))
  if [ "$running" -ge "${RG_JOBS:-4}" ]; then
    wait -n
    running=$((running-1))
  fi
done < "$W/cells.txt"
wait

# ------------------------------------------------------------ still/10bit -
pass=0; fail=0; skip=0
failed=(); skipped=()
while IFS=$'\t' read -r label got want; do
  if [ "$got" = "$want" ]; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$label [pin=$want got=$got]")
  fi
done < "$W/inter-results.tsv"

# byte <label> <content> <w> <h> <qp> <preset> [bd] — the stream must be
# byte-identical to the real C encoder (the spotcheck shape).
byte() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=${7:-8}
  if ! SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [port failed to encode]"); return
  fi
  if ! SVT_TRACE_OUT=/dev/null $LOWPRI "$CT" "$w" "$h" "$qp" "$p" "$W/rs.yuv" "$W/c.obu" "$bd" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$label [C oracle failed]"); return
  fi
  if cmp -s "$W/c.obu" "$W/rs.obu"; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    failed+=("$label [C=$(wc -c <"$W/c.obu"|tr -d ' ')B port=$(wc -c <"$W/rs.obu"|tr -d ' ')B]")
  fi
}

byte "still-gradient-p8"   gradient 128 128 20 8
byte "still-diag-p6"       diag     128 128 32 6
byte "still-screen-p4"     screen    96  88 40 4
byte "still-gradient-p13"  gradient  64  64 55 13
byte "bd10-gradient-p8"    gradient 128 128 20 8 10
byte "bd10-screen-p2"      screen   128 128 32 2 10

# The non-funnel arm (`partition_search_frame_edges`) — a qp0 coded-lossless
# still runs the lossless PD0 path, and the mono leg exercises the arm the
# 4:2:0 funnel never reaches.
byte "lossless-q0-p8"      gradient  64  64  0 8
decodes() {
  local label=$1 content=$2 w=$3 h=$4 qp=$5 p=$6 bd=${7:-8}
  local dec=${AOMDEC:-$(command -v aomdec || true)}
  if [ -z "$dec" ]; then
    skip=$((skip+1)); skipped+=("$label (no aomdec on PATH; set AOMDEC=)")
    return
  fi
  SVTAV1_BD="$bd" $LOWPRI "$RUN" "$content" "$w" "$h" "$qp" "$p" "$W/rs" >/dev/null 2>&1 || {
    fail=$((fail+1)); failed+=("$label rs-err"); return
  }
  if "$dec" --summary -o /dev/null "$W/rs.obu" >/dev/null 2>&1; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$label DECODE-FAIL ($(wc -c < "$W/rs.obu" | tr -d ' ')B)")
  fi
}
SVTAV1_MONO=1 decodes "mono-p6-96x80" gradient 96 80 10 6

echo "refactor gate: $pass pass, $fail fail, $skip skip"
for f in "${failed[@]:-}"; do [ -n "$f" ] && echo "  FAIL $f"; done
for f in "${skipped[@]:-}"; do [ -n "$f" ] && echo "  SKIP $f"; done
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# Frontier sweep: bytes + SSIMULACRA2 + wall-clock per cell across
# presets x qps x arms on an imazen-26 class subset. This is the
# "BD-rate vs speed" instrument — a preset IS an operating point on the
# speed-quality curve, and the ms column catches arms whose tools cost
# real encode time.
#
# Input TSV rows: <class>\t<relpath-under-corpus-root>\n
#   (the 42-image k-means subset format used by
#   benchmarks/aom_features_sweep_2026-09-20.py)
#
# Arms (FRONTIER_ARMS env, default "d e s"):
#   d = default, e = SVTAV1_SCREEN_TOOLS=1 (aom-screen-tools-v1),
#   s = SVTAV1_DEEP_SEARCH=1 (deep-search-v1), i = SVTAV1_STILL_TUNE=1
#   (still-image-tune-v1).
#
# Output: <workdir>/frontier.tsv —
#   class  img  preset  qp  arm  bytes  ssim2  ms
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
SUBSET=${1:-/home/lilith/tmp/tune_subset.tsv}
W=${2:-/tmp/frontier_sweep}
ROOT=${ZEN_CORPUS_ROOT:-/home/lilith/work/zen}
AOMDEC=${AOMDEC:-$(command -v aomdec || true)}
SSIM2=${SSIM2:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}
CONV="$HERE/i444_png.py"
IRUN="$RS/target/release/examples/identity_run"
DIM=${FRONTIER_DIM:-512}
QPS=${FRONTIER_QPS:-"20 32 44 56"}
PRESETS=${FRONTIER_PRESETS:-"4 6 8 10"}
ARMS=${FRONTIER_ARMS:-"d e s"}

[ -x "$AOMDEC" ] && [ -x "$SSIM2" ] || { echo "need aomdec + fast-ssim2-cli" >&2; exit 2; }
[ -r "$SUBSET" ] || { echo "subset tsv not readable: $SUBSET" >&2; exit 2; }
mkdir -p "$W"
if [ ! -x "$IRUN" ]; then
  (cd "$RS" && cargo build -q --release -p zenav1-svt --example identity_run) || {
    echo "identity_run build failed" >&2; exit 2; }
fi

TSV="$W/frontier.tsv"
[ -f "$TSV" ] || echo -e "class\timg\tpreset\tqp\tarm\tbytes\tssim2\tms" > "$TSV"

cell() { # class img relpath preset qp arm
  local cls=$1 img=$2 rel=$3 p=$4 qp=$5 arm=$6
  local b="$W/${img}_${p}_${qp}_${arm}"
  local envv=()
  case "$arm" in
    e) envv=(SVTAV1_SCREEN_TOOLS=1) ;;
    s) envv=(SVTAV1_DEEP_SEARCH=1) ;;
    i) envv=(SVTAV1_STILL_TUNE=1) ;;
  esac
  local t0 t1 ms s
  t0=$(date +%s%N)
  if ! env "${envv[@]}" "$IRUN" "crop:$ROOT/$rel" "$DIM" "$DIM" "$qp" "$p" "$b" >/dev/null 2>"$b.err"; then
    echo -e "$cls\t$img\t$p\t$qp\t$arm\tFAIL\t-\t-" >> "$TSV"; return
  fi
  t1=$(date +%s%N); ms=$(( (t1-t0)/1000000 ))
  # identity_run writes the I420 source as <b>.yuv — promote it to the
  # scoring reference PNG once per cell.
  if [ ! -f "$b.src.png" ] \
     && ! { python3 "$CONV" up444 "$b.yuv" "$DIM" "$DIM" "$b.src.i444" \
            && python3 "$CONV" png "$b.src.i444" "$DIM" "$DIM" "$b.src.png"; }; then
    echo -e "$cls\t$img\t$p\t$qp\t$arm\tSRCERR\t-\t-" >> "$TSV"; return
  fi
  if "$AOMDEC" --rawvideo -o "$b.dec.i420" "$b.obu" >/dev/null 2>&1 \
     && python3 "$CONV" up444 "$b.dec.i420" "$DIM" "$DIM" "$b.dec.yuv" \
     && python3 "$CONV" png "$b.dec.yuv" "$DIM" "$DIM" "$b.dec.png"; then
    s=$("$SSIM2" image "$b.src.png" "$b.dec.png" 2>/dev/null | awk '{print $2}')
    echo -e "$cls\t$img\t$p\t$qp\t$arm\t$(stat -c%s "$b.obu")\t$s\t$ms" >> "$TSV"
  else
    echo -e "$cls\t$img\t$p\t$qp\t$arm\tDECERR\t-\t$ms" >> "$TSV"
  fi
}

n=0
while IFS=$'\t' read -r cls rel; do
  [ -n "$cls" ] || continue
  img=$(basename "$rel" .png | tr -cd '[:alnum:]_-' | cut -c1-40)
  for p in $PRESETS; do
    for qp in $QPS; do
      for arm in $ARMS; do
        # skip cells already in the TSV (restartable)
        if [ -f "$TSV" ] && grep -qF "$cls	$img	$p	$qp	$arm	" "$TSV"; then continue; fi
        cell "$cls" "$img" "$rel" "$p" "$qp" "$arm"; n=$((n+1))
      done
    done
  done
  echo "done: $img" >&2
done < "$SUBSET"
echo "frontier_sweep: $n cells -> $TSV" >&2

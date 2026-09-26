#!/usr/bin/env bash
# RD + SSIMULACRA2 validation for the NON-byte-exact surfaces: 4:4:4 and mono.
#
# WHY: 4:4:4 and mono have no C oracle (SVT-AV1 refuses non-420; mono is a Rust
# extension), so byte-parity gates cannot see them. "aomdec parses the stream"
# proves syntax, not quality. This sweep answers "is the encoder acting right":
# does quality rise monotonically with rate, is chroma fidelity real at 444,
# and how does the rate-quality frontier sit against libaom (aomenc), the only
# other AV1 encoder that accepts the same formats.
#
# For each (image, qp) cell and each leg {ours, aomenc}:
#   bytes — coded stream size
#   ssim2 — SSIMULACRA2 of decoded-vs-source RGB (fast-ssim2-cli)
#   psnr_y/u/v — per-plane PSNR of decoded-vs-source YUV (i444_png.py)
#
# A third mode {420 ours, 420 aomenc} encodes the same cells at 4:2:0,
# upsamples decoded chroma 2x to I444, and scores against the SAME 444
# source — the format-value check: does full-res chroma earn its bits.
#
# Output: <workdir>/results.tsv — mode img w h qp leg bytes ssim2 py pu pv
#
# Corpus: gb82 photos (photographic) + gb82-sc screen content (chroma-critical
# edges — where 444's full-res chroma earns its bits), one odd-height native
# frame (graph 796x481).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
W=${1:-/tmp/rd_ext_sweep}
mkdir -p "$W"

CORPUS=${ZENAV1_CORPUS_ROOT:-/home/lilith/work/codec-corpus}
AOMDEC=${AOMDEC:-$(command -v aomdec || true)}
AOMENC=${AOMENC:-$(command -v aomenc || true)}
SSIM2=${SSIM2:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}
P444=$("$HERE/example" --path probe_444_file) || exit 2
PMONO=$("$HERE/example" --path probe_mono_file) || exit 2
CONV="$HERE/i444_png.py"
QPS="10 20 30 40 50 60"
PRESET=${RD_PRESET:-8}
AOM_CPU=${AOM_CPU:-2}

[ -x "$AOMDEC" ] && [ -x "$AOMENC" ] && [ -x "$SSIM2" ] || {
  echo "need aomdec + aomenc + fast-ssim2-cli (SSIM2=)" >&2; exit 2; }

TSV="$W/results.tsv"
echo -e "mode\timg\tw\th\tqp\tleg\tbytes\tssim2\tpsnr_y\tpsnr_u\tpsnr_v" > "$TSV"

enc_444() { # img w h qp -> appends ours + aomenc rows
  local img=$1 w=$2 h=$3 qp=$4 tag
  tag=$(basename "$img" .png | tr -cd '[:alnum:]_-')
  local b="$W/444_${tag}_${w}x${h}_q${qp}"
  # ours
  if "$P444" "$img" "$w" "$h" "$qp" "$PRESET" "$b" >/dev/null 2>"$b.ours.err" \
     && "$AOMDEC" --rawvideo -o "$b.ours.dec.yuv" "$b.obu" >/dev/null 2>&1 \
     && python3 "$CONV" png "$b.ours.dec.yuv" "$w" "$h" "$b.ours.dec.png" >/dev/null 2>&1; then
    local s p
    s=$("$SSIM2" image "$b"_src.png "$b.ours.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" psnr "$b.yuv" "$b.ours.dec.yuv" "$w" "$h")
    echo -e "444\t$tag\t$w\t$h\t$qp\tours\t$(stat -c%s "$b.obu")\t$s\t$p" >> "$TSV"
  else
    echo -e "444\t$tag\t$w\t$h\t$qp\tours\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
  # aomenc oracle — same .yuv source
  if "$AOMENC" --i444 -w "$w" -h "$h" --profile=1 --codec=av1 \
      --end-usage=q --cq-level="$qp" --limit=1 --passes=1 \
      --cpu-used="$AOM_CPU" -u 2 -o "$b.aom.ivf" "$b.yuv" >/dev/null 2>&1 \
     && "$AOMDEC" --rawvideo -o "$b.aom.dec.yuv" "$b.aom.ivf" >/dev/null 2>&1 \
     && python3 "$CONV" png "$b.aom.dec.yuv" "$w" "$h" "$b.aom.dec.png" >/dev/null 2>&1; then
    local s p
    s=$("$SSIM2" image "$b"_src.png "$b.aom.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" psnr "$b.yuv" "$b.aom.dec.yuv" "$w" "$h")
    echo -e "444\t$tag\t$w\t$h\t$qp\taomenc\t$(stat -c%s "$b.aom.ivf")\t$s\t$p" >> "$TSV"
  else
    echo -e "444\t$tag\t$w\t$h\t$qp\taomenc\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
}

enc_mono() { # img w h qp
  local img=$1 w=$2 h=$3 qp=$4 tag
  tag=$(basename "$img" .png | tr -cd '[:alnum:]_-')
  local b="$W/mono_${tag}_${w}x${h}_q${qp}"
  if "$PMONO" "$img" "$w" "$h" "$qp" "$PRESET" "$b" >/dev/null 2>"$b.ours.err" \
     && "$AOMDEC" --rawvideo -o "$b.ours.dec.y" "$b.obu" >/dev/null 2>&1 \
     && python3 "$CONV" gray "$b.ours.dec.y" "$w" "$h" "$b.ours.dec.png" >/dev/null 2>&1; then
    local s p
    s=$("$SSIM2" image "$b"_src.png "$b.ours.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" ypsnr "$b.y" "$b.ours.dec.y" "$w" "$h")
    echo -e "mono\t$tag\t$w\t$h\t$qp\tours\t$(stat -c%s "$b.obu")\t$s\t$p\t-\t-" >> "$TSV"
  else
    echo -e "mono\t$tag\t$w\t$h\t$qp\tours\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
  if "$AOMENC" -w "$w" -h "$h" --codec=av1 --monochrome \
      --end-usage=q --cq-level="$qp" --limit=1 --passes=1 \
      --cpu-used="$AOM_CPU" -u 2 -o "$b.aom.ivf" "$b.i420" >/dev/null 2>&1 \
     && "$AOMDEC" --rawvideo -o "$b.aom.dec.y" "$b.aom.ivf" >/dev/null 2>&1 \
     && python3 "$CONV" gray "$b.aom.dec.y" "$w" "$h" "$b.aom.dec.png" >/dev/null 2>&1; then
    local s p
    s=$("$SSIM2" image "$b"_src.png "$b.aom.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" ypsnr "$b.y" "$b.aom.dec.y" "$w" "$h")
    echo -e "mono\t$tag\t$w\t$h\t$qp\taomenc\t$(stat -c%s "$b.aom.ivf")\t$s\t$p\t-\t-" >> "$TSV"
  else
    echo -e "mono\t$tag\t$w\t$h\t$qp\taomenc\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
}

CELLS_444="
$CORPUS/gb82/baby-lossless.png    576 576
$CORPUS/gb82/bulb-lossless.png    576 576
$CORPUS/gb82/city-lossless.png    576 576
$CORPUS/gb82/dog-lossless.png     576 576
$CORPUS/gb82/flowers-lossless.png 576 576
$CORPUS/gb82/girl-lossless.png    576 576
$CORPUS/gb82-sc/codec_wiki.png    576 576
$CORPUS/gb82-sc/imessage.png      576 576
$CORPUS/gb82-sc/terminal.png      576 576
$CORPUS/gb82-sc/gui.png           576 576
$CORPUS/gb82-sc/graph.png         796 481
"

echo "$CELLS_444" | while read -r img w h; do
  [ -n "$img" ] || continue
  for qp in $QPS; do enc_444 "$img" "$w" "$h" "$qp"; done
  echo "444 done: $(basename "$img")" >&2
done

echo "$CELLS_444" | while read -r img w h; do
  [ -n "$img" ] || continue
  for qp in $QPS; do enc_mono "$img" "$w" "$h" "$qp"; done
  echo "mono done: $(basename "$img")" >&2
done

# 4:2:0 comparison legs — same cells encoded at 4:2:0, decoded I420 chroma
# nearest-upsampled to I444, scored against the 444 run's source. The 444
# section must run first in the same workdir (it writes src.png/.yuv).
IRUN="$HERE/identity_run"
enc_420() { # img w h qp -> appends ours + aomenc rows
  local img=$1 w=$2 h=$3 qp=$4 tag src
  tag=$(basename "$img" .png | tr -cd '[:alnum:]_-')
  src="$W/444_${tag}_${w}x${h}_q10"   # src.png/.yuv are qp-independent
  local b="$W/420_${tag}_${w}x${h}_q${qp}"
  # ours — identity_run writes <b>.yuv (I420 src) + <b>.obu
  if "$IRUN" "crop:$img" "$w" "$h" "$qp" "$PRESET" "$b" >/dev/null 2>"$b.ours.err" \
     && "$AOMDEC" --rawvideo -o "$b.ours.dec.i420" "$b.obu" >/dev/null 2>&1 \
     && python3 "$CONV" up444 "$b.ours.dec.i420" "$w" "$h" "$b.ours.dec.yuv" \
     && python3 "$CONV" png "$b.ours.dec.yuv" "$w" "$h" "$b.ours.dec.png"; then
    local s p
    s=$("$SSIM2" image "$src"_src.png "$b.ours.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" psnr "$src.yuv" "$b.ours.dec.yuv" "$w" "$h")
    echo -e "420\t$tag\t$w\t$h\t$qp\tours\t$(stat -c%s "$b.obu")\t$s\t$p" >> "$TSV"
  else
    echo -e "420\t$tag\t$w\t$h\t$qp\tours\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
  # aomenc 420
  if "$AOMENC" --i420 -w "$w" -h "$h" --profile=0 --codec=av1 \
      --end-usage=q --cq-level="$qp" --limit=1 --passes=1 \
      --cpu-used="$AOM_CPU" -u 2 -o "$b.aom.ivf" "$b.yuv" >/dev/null 2>&1 \
     && "$AOMDEC" --rawvideo -o "$b.aom.dec.i420" "$b.aom.ivf" >/dev/null 2>&1 \
     && python3 "$CONV" up444 "$b.aom.dec.i420" "$w" "$h" "$b.aom.dec.yuv" \
     && python3 "$CONV" png "$b.aom.dec.yuv" "$w" "$h" "$b.aom.dec.png"; then
    local s p
    s=$("$SSIM2" image "$src"_src.png "$b.aom.dec.png" 2>/dev/null | awk '{print $2}')
    p=$(python3 "$CONV" psnr "$src.yuv" "$b.aom.dec.yuv" "$w" "$h")
    echo -e "420\t$tag\t$w\t$h\t$qp\taomenc\t$(stat -c%s "$b.aom.ivf")\t$s\t$p" >> "$TSV"
  else
    echo -e "420\t$tag\t$w\t$h\t$qp\taomenc\tFAIL\t-\t-\t-\t-" >> "$TSV"
  fi
}

echo "$CELLS_444" | while read -r img w h; do
  [ -n "$img" ] || continue
  for qp in $QPS; do enc_420 "$img" "$w" "$h" "$qp"; done
  echo "420 done: $(basename "$img")" >&2
done

echo "wrote $TSV" >&2

#!/usr/bin/env bash
# Positive-control gate for ZenEnhancement::AomScreenTools
# ("aom-screen-tools-v1").
#
# WHY: a tool that can be configured on but never fires is a silent no-op,
# and a tool that fires where it must not is a parity leak. C's allintra
# ladders switch palette off at M8+ and IntraBC at M5+, and stop running the
# screen-content detector entirely above M7 — so at preset 8 a detected
# screen frame MUST change when the arm is on (FH bits + coded blocks),
# while a photo MUST stay byte-identical (the detector's classes never set,
# so the arm substitutes nothing). Both directions are asserted here:
#
#   1. FIRE:   screen image, p8, arm on vs off -> streams differ AND the
#              arm-on stream decodes under aomdec.
#   2. INERT:  photo image, p8, arm on vs off -> byte-identical streams.
#   3. INERT:  screen image where sc_class5 does not fire (codec_wiki's
#              center crop fails the 3-of-4-quadrant rule), arm on vs off
#              -> byte-identical.
#
# Cell choices are the ones measured in docs/DEVIN-CHANGE-LOG.md (the
# aom-screen-tools-v1 entry): imessage/terminal/gui fire sc_class5 at
# 576x576; codec_wiki does not; baby-lossless is the photo control.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
CORPUS=${ZENAV1_CORPUS_ROOT:-/home/lilith/work/codec-corpus}
AOMDEC=${AOMDEC:-$(command -v aomdec || true)}
[ -x "$AOMDEC" ] || { echo "need aomdec" >&2; exit 2; }

(cd "$RS" && cargo build -q --release -p zenav1-svt --example identity_run)
IRUN="$RS/target/release/examples/identity_run"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

pass=0; fail=0
check() { # name condition
  if [ "$2" = "0" ]; then pass=$((pass+1)); echo "PASS $1";
  else fail=$((fail+1)); echo "FAIL $1"; fi
}

enc() { # img w h qp preset out [env]
  local img=$1 w=$2 h=$3 qp=$4 p=$5 out=$6
  shift 6
  env "$@" "$IRUN" "crop:$img" "$w" "$h" "$qp" "$p" "$out" >/dev/null 2>&1
}

# --- 1. FIRE: detected screen content at p8 must change with the arm on ---
for img in imessage terminal gui; do
  enc "$CORPUS/gb82-sc/$img.png" 576 576 30 8 "$W/d" || { echo "FAIL encode-d-$img"; fail=$((fail+1)); continue; }
  enc "$CORPUS/gb82-sc/$img.png" 576 576 30 8 "$W/e" SVTAV1_SCREEN_TOOLS=1 || { echo "FAIL encode-e-$img"; fail=$((fail+1)); continue; }
  if cmp -s "$W/d.obu" "$W/e.obu"; then r=1; else r=0; fi
  check "fires-$img" "$r"
  if "$AOMDEC" --rawvideo -o "$W/e.yuv" "$W/e.obu" >/dev/null 2>&1; then r=0; else r=1; fi
  check "decodes-$img" "$r"
done

# --- 2. INERT on photos ---
enc "$CORPUS/gb82/baby-lossless.png" 576 576 30 8 "$W/d"
enc "$CORPUS/gb82/baby-lossless.png" 576 576 30 8 "$W/e" SVTAV1_SCREEN_TOOLS=1
if cmp -s "$W/d.obu" "$W/e.obu"; then r=0; else r=1; fi
check "inert-photo-baby" "$r"

# --- 3. INERT where sc_class5 does not fire ---
enc "$CORPUS/gb82-sc/codec_wiki.png" 576 576 30 8 "$W/d"
enc "$CORPUS/gb82-sc/codec_wiki.png" 576 576 30 8 "$W/e" SVTAV1_SCREEN_TOOLS=1
if cmp -s "$W/d.obu" "$W/e.obu"; then r=0; else r=1; fi
check "inert-sc5-miss-codec_wiki" "$r"

echo "screen_tools_gate: $pass pass / $fail fail"
[ "$fail" = 0 ]

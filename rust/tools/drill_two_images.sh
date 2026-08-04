#!/usr/bin/env bash
# Per-preset, per-qp verdict for the two real-corpus images that still diverge
# (docs/finishing-survey.md C2b). Prints a line per cell ALWAYS, and runs a
# POSITIVE CONTROL first -- a known-identical cell -- so a screen full of
# "IDENTICAL" cannot be a silently broken harness.
#
#   tools/drill_two_images.sh
#   IMG_A=/path/photo.png IMG_B=/path/screen.png tools/drill_two_images.sh
# Per-preset verdict for the two images the 8-bit real-tier sweep still shows
# diverging. Prints a line per cell ALWAYS.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."
. "$HERE/lib_corpus.sh"
IMG_A=${IMG_A:-$(corpus_dir codec-corpus/CID22/CID22-512/training)/1028637.png}
IMG_B=${IMG_B:-$(corpus_dir codec-corpus/gb82-sc)/graph.png}
run() { # label img qp preset
  local v
  v=$(./tools/identity_diff.sh 512 512 "$3" "$4" "crop:$2" 2>&1 | grep -oE "VERDICT: .*" | head -1)
  printf "  %-8s q%-3s p%-2s : %s\n" "$1" "$3" "$4" "${v:-<NO VERDICT>}"
}
echo "== positive control (known-good cell) =="
run ctl "$IMG_A" 20 6
echo "== 1028637.png (CID22 photo) =="
for p in 0 1 2 3 4 5; do for q in 5 20 32 48 63; do run photo "$IMG_A" $q $p; done; done
echo "== graph.png (gb82-sc screen) =="
for p in 0 1 2 3 4 5; do for q in 5 20 32 48 63; do run screen "$IMG_B" $q $p; done; done

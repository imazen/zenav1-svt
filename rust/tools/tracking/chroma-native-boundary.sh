#!/usr/bin/env bash
set -euo pipefail
cd /home/lilith/work/zen/zenav1-svt/rust
out=/home/lilith/tmp/svt-tracking/chroma-native-boundary
mkdir -p "$out"
for bd in 8 10; do
  for p in -1 0 1 4 5; do
    for q in 10 30; do
      SVTAV1_BD="$bd" SVTAV1_HBD_SRC=1 tools/identity_diff.sh 376 512 "$q" "$p" raw:/home/lilith/tmp/svt-tracking/preset-fill-worst-replay/measured.yuv "$out/bd${bd}-p${p}-q${q}" > "$out/bd${bd}-p${p}-q${q}.log" 2>&1
      echo "PASS bd=$bd p=$p q=$q"
    done
  done
done

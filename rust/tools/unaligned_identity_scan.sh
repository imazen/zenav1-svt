#!/usr/bin/env bash
# Port-vs-C byte identity on UNALIGNED (partial-superblock) frame dimensions
# with REAL image content — the axis every existing identity gate misses.
#
# WHY THIS EXISTS (issue #15, measured 2026-08-11).
# The two identity gates that run real photographs both round the encode
# dimensions UP to a multiple of 64 (`real_image_matrix.sh`'s
# `png_dims_aligned`), and the one gate that runs partial superblocks
# (`partial_sb_gate.sh`) runs only synthetic `gradient` / `uniform` content at
# <= 200x120. Nothing crossed the two. Crossing them finds divergences:
# 96x88 preset 4 qp 33 on a real screenshot is 403 B (port) vs 392 B (C),
# 24 differing bytes, while the SAME dims on `gradient` byte-match. The
# partial-SB RD near-tie the port map lists as "follow-up 2" is reachable on
# real content at ordinary presets, which the synthetic cells never showed.
#
# It also disposes of the size hypothesis in #15: ALIGNED square 1024 and 2048
# are byte-identical to C on both synthetic and real content (see
# `benchmarks/parity_unaligned_real_2026-08-11.tsv`). The axis is alignment,
# not size.
#
# NOT PASS/FAIL. Like `real_image_matrix.sh`, this is a tracking scoreboard —
# it always exits 0 and prints the identical fraction. Divergences here are
# findings to drill, not test failures; the pass/fail gates are
# `partial_sb_gate.sh` (synthetic partial SB) and `identity_matrix.sh`.
#
# The C oracle is `tools/perf_c_encode` — the SAME still-picture/AVIF CQP
# config as `capture_c_trace` (`--rc 0 --aq-mode 0 --lp 1 --avif 1`, 8-bit
# 4:2:0, single tile), driven on the very `.yuv` `identity_run` just wrote, so
# both encoders consume one byte stream.
#
# HARNESS TRAP THIS SCRIPT AVOIDS (measured 2026-08-11): comparing against
# `SvtAv1EncApp` at ITS default frame rate instead of the port's 30 fps moves
# `seq_level_idx` (5 -> 8 at 512 and 1024 luma-sample rates), which is a
# 2-byte sequence-header difference at offset 4 that looks like a port defect
# and is not. The C level derivation is a function of the sample RATE, so any
# cross-encoder identity check must match fps, not just dimensions.
#
# Usage: tools/unaligned_identity_scan.sh [out.tsv]
# Env:   UIS_DIMS     "WxH ..."   (default: a partial-SB spread + one aligned control)
#        UIS_PRESETS  "..."       (default: 2 4 6 8 10 13)
#        UIS_QPS      "..."       (default: 12 33 55)
#        UIS_IMAGES   "png ..."   (default: 3 photo + 3 screen from codec-corpus)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_corpus.sh
. "$HERE/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)

OUT="${1:-$RS_ROOT/benchmarks/unaligned_real_identity_latest.tsv}"
IR="$HERE/identity_run"
CE="$HERE/perf_c_encode/perf_c_encode"
WORK="$RS_ROOT/target/unaligned_identity"
mkdir -p "$WORK" "$(dirname "$OUT")"

[[ -x "$CE" ]] || { echo "missing $CE — run tools/perf_c_encode/build.sh" >&2; exit 2; }

CORPUS=$(corpus_dir codec-corpus)
read -r -a DIMS <<<"${UIS_DIMS:-64x64 96x88 128x136 256x166 188x256 256x160}"
read -r -a PRESETS <<<"${UIS_PRESETS:-2 4 6 8 10 13}"
read -r -a QPS <<<"${UIS_QPS:-12 33 55}"
read -r -a IMAGES <<<"${UIS_IMAGES:-$CORPUS/gb82/city-lossless.png $CORPUS/gb82/flowers-lossless.png $CORPUS/clic2025/training/097cb426910ba8ce2525dd8bb7fb1777.png $CORPUS/gb82-sc/gui.png $CORPUS/gb82-sc/graph.png $CORPUS/gb82-sc/terminal.png}"

{
    echo "# port-vs-C byte identity at UNALIGNED (partial-SB) dims, real content"
    echo "# commit=$(git -C "$RS_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown) host=$(hostname -s) date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# oracle=tools/perf_c_encode (matched still/CQP config, 30 fps both sides)"
    echo "# dims=[${DIMS[*]}] presets=[${PRESETS[*]}] qps=[${QPS[*]}] images=${#IMAGES[@]}"
    printf 'image\tw\th\taligned_w\taligned_h\tpreset\tqp\tport_bytes\tc_bytes\tndiff\tfirst_diff\tverdict\n'
} >"$OUT"

n=0
same=0
for img in "${IMAGES[@]}"; do
    [[ -f "$img" ]] || { echo "SKIP-MISSING $img" >&2; continue; }
    stem=$(basename "$img" .png)
    for wh in "${DIMS[@]}"; do
        w=${wh%x*}
        h=${wh#*x}
        for preset in "${PRESETS[@]}"; do
            for qp in "${QPS[@]}"; do
                if ! "$IR" "crop:$img" "$w" "$h" "$qp" "$preset" "$WORK/p" >/dev/null 2>&1; then
                    printf '%s\t%s\t%s\t-\t-\t%s\t%s\t-\t-\t-\t-\tPORTFAIL\n' \
                        "$stem" "$w" "$h" "$preset" "$qp" >>"$OUT"
                    n=$((n + 1))
                    continue
                fi
                "$CE" "$w" "$h" "$qp" "$preset" "$WORK/p.yuv" "$WORK/c.obu" 0 >/dev/null 2>&1
                row=$(python3 - "$WORK/p.obu" "$WORK/c.obu" "$stem" "$w" "$h" "$preset" "$qp" <<'PY'
import sys
p, c, stem, w, h, preset, qp = sys.argv[1:]
P = open(p, "rb").read()
try:
    C = open(c, "rb").read()
except OSError:
    C = b""
n = sum(1 for i in range(min(len(P), len(C))) if P[i] != C[i]) + abs(len(P) - len(C))
f = next((i for i in range(min(len(P), len(C))) if P[i] != C[i]),
         -1 if len(P) == len(C) else min(len(P), len(C)))
up8 = lambda x: (int(x) + 7) // 8 * 8
verdict = "IDENTICAL" if (n == 0 and C) else ("CFAIL" if not C else "DIFFERS")
print("\t".join(map(str, [stem, w, h, up8(w), up8(h), preset, qp,
                          len(P), len(C), n, f, verdict])))
PY
)
                printf '%s\n' "$row" >>"$OUT"
                n=$((n + 1))
                [[ "$row" == *IDENTICAL* ]] && same=$((same + 1))
            done
        done
        printf '  %s %sx%s done (%d/%d identical so far)\n' "$stem" "$w" "$h" "$same" "$n" >&2
    done
done

echo "cells=$n identical=$same" | tee -a "$OUT"
awk -F'\t' 'NR>1 && $12=="DIFFERS" {d[$4"x"$5"_p"$6]++} END {for (k in d) printf "  DIFFERS %-16s %d\n", k, d[k]}' "$OUT" | sort >&2
echo "wrote $OUT"

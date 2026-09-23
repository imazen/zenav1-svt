#!/usr/bin/env bash
# INTER-INTRA / MASKED-COMPOUND reach census — which of the inter MD path's
# inter-intra and masked-compound candidates actually get CODED, per cell,
# counted from the port's SVTAV1_INTERDBG dump (pipeline.rs's IDBG line,
# which prints the writer's `InterModeInfo` per coded inter block).
#
# ANTI-VACUITY IS THE POINT. The feature chain — `inter_intra_search`,
# `calc_pred_masked_compound`, `search_compound_diff_wedge`, the
# `predict_inter_yuv_compound_md` driver, the IiPreds precompute — can all
# be wired and still produce zero coded blocks on content that never wins
# the mode decision. This census requires POSITIVE counts: a cell that
# codes no inter-intra AND no compound blocks at a preset where C reaches
# them (the compound/II-live band is presets -1..2 — `inter_compound_mode`
# 3/4 and `inter_intra_level` 2) is a wiring defect, not an RD choice.
#
# Columns: BLOCKS = coded inter blocks; II = blocks with
# `is_interintra_used` (rf[1] == INTRA_FRAME); IIWDG = II blocks coded with
# `use_wedge_interintra`; COMP = bipred blocks (rf[1] > INTRA_FRAME);
# WEDGE = COMPOUND_WEDGE (interinter_comp_type 2); DIFF = COMPOUND_DIFFWTD
# (type 3). DIST/AVG compounds land in COMP with ctype 0.
#
# Usage: tools/inter_intra_masked_census.sh [out.tsv]
# Env:   IMC_CLIPS / IMC_SIZES / IMC_QPS / IMC_PRESETS / IMC_FRAMES —
#        default real-video cells at the II/masked-live presets.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
OUT=${1:-/dev/stdout}
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
CLIPS=${IMC_CLIPS:-"johnny vidyo3 fourpeople"}
SIZES=${IMC_SIZES:-"128x128"}
QPS=${IMC_QPS:-"20"}
PRESETS=${IMC_PRESETS:-"0"}
FRAMES=${IMC_FRAMES:-6}
D=$(mktemp -d "${TMPDIR:-$HOME/tmp}/imc.XXXXXX")
printf 'clip\tsize\tqp\tpreset\tframes\tverdict\tBLOCKS\tII\tIIWDG\tCOMP\tWEDGE\tDIFF\n' > "$OUT"
fail=0
for c in $CLIPS; do for s in $SIZES; do for q in $QPS; do for p in $PRESETS; do
    a="$ASSETS/${c}_${s}_8f.i420"
    if [ ! -f "$a" ]; then
        echo "MISSING ASSET $a" >&2; fail=$((fail + 1)); continue
    fi
    w=${s%x*}; h=${s#*x}
    d="$D/${c}_${s}_${q}_${p}"; mkdir -p "$d"
    env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        SVTAV1_FRAMES="$FRAMES" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        SVTAV1_INTERDBG=1 \
        nice -n 19 "$HERE/identity_run" "rawseq:$a" "$w" "$h" "$q" "$p" "$d/p" \
        > "$d/out.txt" 2> "$d/trace.txt"
    rc=$?
    case $rc in 0) v=OK;; *) v=FAIL$rc;; esac
    t="$d/trace.txt"
    blocks=$(grep -c 'IDBG' "$t" 2>/dev/null)
    ii=$(grep 'IDBG' "$t" | grep -c 'ii=Some' || true)
    iiwdg=$(grep 'IDBG' "$t" | grep -c 'use_wedge: true' || true)
    comp=$(grep 'IDBG' "$t" | grep -cE 'rf=\[[0-9]+, [1-9]' || true)
    wedge=$(grep 'IDBG' "$t" | grep -c 'ctype=2' || true)
    diff=$(grep 'IDBG' "$t" | grep -c 'ctype=3' || true)
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$c" "$s" "$q" "$p" "$FRAMES" "$v" \
        "${blocks:-0}" "${ii:-0}" "${iiwdg:-0}" "${comp:-0}" "${wedge:-0}" "${diff:-0}" >> "$OUT"
done; done; done; done
tot=$(awk 'NR>1 {b+=$7; i+=$8; c+=$10} END {print b+0, i+0, c+0}' "$OUT")
rm -rf "$D"
set -- $tot
if [ "$fail" -gt 0 ]; then
    echo "inter_intra_masked_census: FAIL — $fail cell(s) missing/refused" >&2; exit 1
fi
if [ "${1:-0}" -eq 0 ]; then
    echo "inter_intra_masked_census: FAIL — ZERO coded inter blocks; the harness" >&2
    echo "  produced nothing (a vacuous pass is a harness failure)." >&2; exit 1
fi
if [ "${2:-0}" -eq 0 ] && [ "${3:-0}" -eq 0 ]; then
    echo "inter_intra_masked_census: FAIL — ZERO II and ZERO compound coded" >&2
    echo "  blocks across all cells at the live presets; the feature chain is" >&2
    echo "  wired but unreachable." >&2; exit 1
fi
echo "inter_intra_masked_census: OK — $1 coded inter blocks, $2 II, $3 compound." >&2

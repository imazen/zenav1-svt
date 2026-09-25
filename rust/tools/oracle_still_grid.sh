#!/usr/bin/env bash
# Oracle still grid — the byte-identity baseline for a named C oracle.
#
# Runs tools/cellrun.py over a fixed still matrix (content x size x
# bit depth x qp x preset) with ONE SVT_ORACLE exported, so the C side and
# the Rust side are driven by the same registry row (rust/oracles/oracles.tsv,
# rust/docs/ORACLES.md). Writes a per-cell TSV: identity verdict plus the
# first-divergence STAGE the differ classified, the same fields
# tools/identity_matrix.sh uses.
#
# This is a SCOREBOARD, not a gate: it exits 0 whatever the tally, because
# its purpose is recording the distance to an oracle that is being ported
# to (phase 3 of docs/PLAN-ORACLES-AND-CLEANUP.md). The numbers it produces
# end up in benchmarks/*.meta.
#
# Usage: tools/oracle_still_grid.sh <oracle> <out.tsv>
# Env overrides: OSG_CONTENTS / OSG_SIZES / OSG_BDS / OSG_QPS / OSG_PRESETS
#                OSG_CELL_TIMEOUT (default 120s per step) / OSG_JOBS (default 1)
#                GRID_PHOTO (PNG used for the photo arm; default CID22-512
#                1001682 via lib_corpus.sh)
set -uo pipefail
ORACLE="${1:?usage: oracle_still_grid.sh <oracle> <out.tsv>}"
OUT="${2:?usage: oracle_still_grid.sh <oracle> <out.tsv>}"
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
# shellcheck source=lib_corpus.sh
. "$HERE/lib_corpus.sh"

PHOTO="${GRID_PHOTO:-$(corpus_dir codec-corpus/CID22/CID22-512/training)/1001682.png}"
[[ -f "$PHOTO" ]] || { echo "oracle_still_grid: photo corpus image not found at $PHOTO" >&2; exit 1; }

export SVT_ORACLE="$ORACLE"
mkdir -p "$(dirname "$OUT")"

read -r -a SIZES <<<"${OSG_SIZES:-64 128 256}"
read -r -a BDS <<<"${OSG_BDS:-8 10}"
read -r -a QPS <<<"${OSG_QPS:-20 32 45 55}"
read -r -a PRESETS <<<"${OSG_PRESETS:-2 4 6 8 10 13}"
CELL_TIMEOUT="${OSG_CELL_TIMEOUT:-120}"

# The grid is a cell list for tools/cellrun.py (plan T3): the cross product
# is written as data, and one runner does port + C + classify for every cell.
CELLS="${OUT%.tsv}.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\n' >"$CELLS"
for content in "gradient" "crop:$PHOTO"; do
    cname=$([[ "$content" == "gradient" ]] && echo gradient || echo photo)
    for sz in "${SIZES[@]}"; do
        for bd in "${BDS[@]}"; do
            for qp in "${QPS[@]}"; do
                for p in "${PRESETS[@]}"; do
                    printf 'osg_%s_%s_%s_q%s_p%s_b%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                        "$ORACLE" "$cname" "$sz" "$qp" "$p" "$bd" \
                        "$content" "$sz" "$sz" "$qp" "$p" "$bd" >>"$CELLS"
                done
            done
        done
    done
done
RESULT="${OUT%.tsv}.result.tsv"
python3 "$HERE/cellrun.py" "$CELLS" --out "$RESULT" --timeout "$CELL_TIMEOUT" \
    --jobs "${OSG_JOBS:-1}" || true

# Back to this scoreboard's historical columns.
printf 'oracle\tcontent\tsize\tpreset\tqp\tbd\tverdict\tstage\tdetail\n' >"$OUT"
tail -n +2 "$RESULT" | while IFS=$'\t' read -r name verdict stage detail _expect _ok; do
    rest="${name#osg_${ORACLE}_}"
    IFS=_ read -r cname sz q p b <<<"$rest"
    # A cell that could not run (refusal, timeout) counts as not identical,
    # with the failing step in the stage column.
    [[ "$verdict" == ERROR ]] && verdict=DIFFERS
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$ORACLE" "$cname" "$sz" "${p#p}" "${q#q}" "${b#b}" "$verdict" "$stage" "$detail" >>"$OUT"
done
echo "oracle_still_grid: wrote $OUT" >&2

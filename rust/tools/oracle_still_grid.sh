#!/usr/bin/env bash
# Oracle still grid — the byte-identity baseline for a named C oracle.
#
# Runs tools/identity_diff.sh over a fixed still matrix (content x size x
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
#                OSG_CELL_TIMEOUT (default 120s per cell)
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

printf 'oracle\tcontent\tsize\tpreset\tqp\tbd\tverdict\tstage\tdetail\n' >"$OUT"

for content in "gradient" "crop:$PHOTO"; do
    cname=$([[ "$content" == "gradient" ]] && echo gradient || echo photo)
    for sz in "${SIZES[@]}"; do
        for bd in "${BDS[@]}"; do
            for qp in "${QPS[@]}"; do
                for p in "${PRESETS[@]}"; do
                    d="$RS_ROOT/target/identity/osg_${ORACLE}_${cname}_${sz}_q${qp}_p${p}_b${bd}"
                    rep="$d/report.txt"
                    SVTAV1_BD=$bd timeout "$CELL_TIMEOUT" "$HERE/identity_diff.sh" \
                        "$sz" "$sz" "$qp" "$p" "$content" "$d" >/dev/null 2>&1
                    rc=$?
                    stage="-"; detail="-"; verdict="DIFFERS"
                    if [[ $rc -eq 0 ]]; then
                        verdict="IDENTICAL"
                    elif [[ $rc -eq 124 ]]; then
                        stage="TIMEOUT"
                    elif [[ -f "$rep" ]]; then
                        line=$(grep -m1 "^STAGE: " "$rep" || true)
                        if [[ -n "$line" ]]; then
                            rest="${line#STAGE: }"; stage="${rest%% | *}"; detail="${rest#* | }"
                        fi
                    fi
                    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
                        "$ORACLE" "$cname" "$sz" "$p" "$qp" "$bd" "$verdict" "$stage" "$detail" \
                        >>"$OUT"
                done
            done
            echo "oracle_still_grid: $ORACLE $cname ${sz} bd$bd done" >&2
        done
    done
done
echo "oracle_still_grid: wrote $OUT" >&2

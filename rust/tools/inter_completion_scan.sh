#!/usr/bin/env bash
# INTER completion frontier — which (size, preset) inter cells the port can
# ENCODE AT ALL, and with what verdict.
#
# WHY THIS IS SEPARATE FROM inter_byte_gate.sh / inter_byte_matrix.sh. Those
# answer "do the bytes match C". This one answers the question that comes
# BEFORE it and that nothing else asks: does the port produce a stream at all?
# It exists because a memory or wall-clock comparison against C is meaningless
# on a cell where the port refused or crashed — its peak RSS is then the peak of
# a SMALLER workload and its elapsed time is the time to fail. The 2026-09-02
# memory baseline (benchmarks/mem_2026-09-02.meta) needed this inventory to know
# which of its cells were comparable, and found three distinct panics doing it.
#
# THE THREE VERDICTS ARE DELIBERATELY DISTINCT, and conflating them is a
# documented failure of this repo's harnesses (see identity_diff_inter.sh's
# header, which grew exit code 4 for exactly this):
#   OK       both frames encoded. `ident` then says whether they match C.
#   REFUSED  identity_run exit 3 — the port declined a config it cannot encode
#            faithfully. This is the SHIPPED behaviour (docs/WORKING-ON-THIS.md
#            §7b), not a bug.
#   CRASH    identity_run exit 101 — a panic. Always a defect.
# The `frames` column says how many frames reached disk, so a frame-1 failure is
# visibly different from a frame-0 one, and `note` carries the panic text.
#
# Usage: tools/inter_completion_scan.sh [out.tsv]
# Env:   SCAN_SIZES, SCAN_PRESETS, SCAN_QP, SCAN_CONTENT, SCAN_SHIFT,
#        SCAN_RS_BIN (default tools/identity_run — the symtrace wrapper; point
#        it at a plain --release build to scan the shipped configuration).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."

OUT=${1:-}
SIZES=${SCAN_SIZES:-"64 88 104 120 128 152 168 232 256 296 512 552 568 576 1024 2048"}
PRESETS=${SCAN_PRESETS:-"6 8 10 13"}
QP=${SCAN_QP:-32}
CONTENT=${SCAN_CONTENT:-gradient}
SHIFT=${SCAN_SHIFT:-3}
RS_BIN=${SCAN_RS_BIN:-$HERE/identity_run}

W="${TMPDIR:-$HOME/tmp}/inter-completion.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

emit() { printf '%s\n' "$*"; [[ -n "$OUT" ]] && printf '%s\n' "$*" >>"$OUT"; }

if [[ -n "$OUT" ]]; then
    : >"$OUT"
    emit "# inter completion frontier — does the port ENCODE this cell at all?"
    emit "# host=$(hostname)  date=$(date -u +%Y-%m-%dT%H:%M:%SZ)  port=$RS_BIN"
    emit "# content=$CONTENT qp=$QP shift=$SHIFT frames=2 (low-delay P, flat GOP, key frame 0 only)"
    emit "# status: OK | REFUSED (exit 3, the shipped behaviour) | CRASH (panic, exit 101)"
    emit "# frames_written: how many of the 2 frames reached disk before the verdict"
    emit "# ident: Y/N vs the C reference, only meaningful when status=OK"
else
    echo "size preset status frames ident note"
fi
[[ -n "$OUT" ]] && emit "size	preset	status	frames_written	ident	note"

for preset in $PRESETS; do
    for s in $SIZES; do
        rm -rf "${W:?}/c"; mkdir -p "$W/c"
        SVTAV1_BD=8 SVTAV1_FRAMES=2 SVTAV1_INTER_EXPERIMENTAL=1 \
        SVTAV1_FRAME_SHIFT="$SHIFT" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
            "$RS_BIN" "$CONTENT" "$s" "$s" "$QP" "$preset" "$W/c/rs" \
            >/dev/null 2>"$W/c/err"
        rc=$?
        nf=0
        [[ -s "$W/c/rs.obu.f0" ]] && nf=$((nf + 1))
        [[ -s "$W/c/rs.obu.f1" ]] && nf=$((nf + 1))
        case $rc in
            0)   status=OK ;;
            3)   status=REFUSED ;;
            101) status=CRASH ;;
            *)   status="rc$rc" ;;
        esac
        note=""
        if [[ $status == CRASH ]]; then
            note=$(grep -A1 "panicked at" "$W/c/err" | tail -1 | cut -c1-90)
        elif [[ $status == REFUSED ]]; then
            note=$(grep -o "REFUSED.*" "$W/c/err" | head -1 | cut -c1-90)
        fi
        ident="-"
        if [[ $status == OK ]]; then
            SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
            SVT_TRACE_OUT=/dev/null \
                "$HERE/capture_c_trace/capture_c_trace" "$s" "$s" "$QP" "$preset" \
                "$W/c/rs.yuv" "$W/c/c.obu" 8 >/dev/null 2>&1
            ident=Y
            for i in 0 1; do
                cmp -s "$W/c/rs.obu.f$i" "$W/c/c.obu.pts$i" || ident=N
            done
        fi
        if [[ -n "$OUT" ]]; then
            emit "$s	$preset	$status	$nf	$ident	$note"
        else
            printf '%-6s p%-3s %-8s %s  %-3s %s\n' "$s" "$preset" "$status" "$nf" "$ident" "$note"
        fi
    done
done

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

n_ok=0
n_refused=0
n_crash=0
n_other=0
n_cells=0
crashed_cells=()
refused_cells=()

for preset in $PRESETS; do
    for s in $SIZES; do
        n_cells=$((n_cells + 1))
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
            0)   status=OK;      n_ok=$((n_ok + 1)) ;;
            3)   status=REFUSED; n_refused=$((n_refused + 1)); refused_cells+=("${s}x${s} p${preset}") ;;
            101) status=CRASH;   n_crash=$((n_crash + 1));     crashed_cells+=("${s}x${s} p${preset}") ;;
            *)   status="rc$rc"; n_other=$((n_other + 1));     crashed_cells+=("${s}x${s} p${preset} rc=$rc") ;;
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

# ---------------------------------------------------------------------------
# GATE MODE (SCAN_GATE=1). Off by default so the scan stays a plain inventory.
#
# THREE ASSERTIONS, and the middle one is the point:
#
#   1. NO CRASHES. A panic is always a defect. This is the assertion CI never
#      had: `arbitrary_size_robustness.sh` is the repo's panic-freedom gate and
#      it drives the PUBLIC API, which refuses inter frames outright — so it
#      cannot reach the inter mode-decision path at all, and 18 inter panics
#      lived behind it (docs/WORKING-ON-THIS.md section 5, "a gate that cannot
#      reach a feature cannot guard it").
#
#   2. NOT MORE THAN `SCAN_MAX_REFUSED` REFUSALS. Converting a panic into a
#      refusal makes a crash gate green while the gap is untouched, and
#      docs/REFUSED-CONFIGS.md's own preamble warns that a refusal makes a gap
#      look handled. Raising this ceiling has to be a deliberate edit with a
#      reason beside it, exactly like raising the byte gate's floor.
#
#   3. AT LEAST `SCAN_MIN_OK` cells encode, and the grid actually ran
#      (anti-vacuity: a scan that executed zero cells must never pass).
#
# The floors are LIMITS, not targets. Lower one only with a measured reason.
if [[ ${SCAN_GATE:-0} == 1 ]]; then
    MIN_OK=${SCAN_MIN_OK:-52}
    MAX_REFUSED=${SCAN_MAX_REFUSED:-12}
    MIN_CELLS=${SCAN_MIN_CELLS:-64}
    fail=0
    echo
    echo "inter completion gate: $n_cells cells — $n_ok OK, $n_refused REFUSED, $n_crash CRASH, $n_other other"
    if ((n_cells < MIN_CELLS)); then
        echo "  FAIL: only $n_cells cells ran, expected at least $MIN_CELLS (a scan that reaches nothing must not pass)"
        fail=1
    fi
    if ((n_crash + n_other > 0)); then
        echo "  FAIL: $((n_crash + n_other)) cell(s) did not complete:"
        printf '    %s\n' "${crashed_cells[@]}"
        fail=1
    fi
    if ((n_ok < MIN_OK)); then
        echo "  FAIL: $n_ok cells encoded, floor is $MIN_OK"
        fail=1
    fi
    if ((n_refused > MAX_REFUSED)); then
        echo "  FAIL: $n_refused refusals, ceiling is $MAX_REFUSED — a panic must not be"
        echo "        retired by widening a refusal. If the new refusal is genuine, raise"
        echo "        SCAN_MAX_REFUSED in the CI step and say what is unimplemented."
        printf '    %s\n' "${refused_cells[@]}"
        fail=1
    fi
    if ((fail == 0)); then
        echo "inter completion gate: PASS"
    fi
    exit $fail
fi

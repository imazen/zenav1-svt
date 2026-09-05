#!/usr/bin/env bash
# Callgrind BOTH encoders on a list of still cells and emit per-function call
# counts for each — the driver behind benchmarks/callcount_*_2026-09-04 and
# benchmarks/callcount_realimg_2026-09-04. Linux only (valgrind).
#
# Usage:
#   callcount_cells.sh <outdir> <cell>...
#     cell = <name>:<content>:<width>:<height>
#     content = gradient | uniform | raw:<abs path to I420 .yuv of exactly WxH>
# Env:
#   PRESETS   presets to run per cell            (default "2 6 10")
#   QP        cli qp 0..63                       (default 40)
#   PE        port harness binary                (default <rs>/target/release/examples/perf_encode)
#   CE        C harness binary                   (default <rs>/tools/perf_c_encode/perf_c_encode)
#   VALGRIND  valgrind binary                    (default valgrind)
#   FRAMES    frames per encode (N > 1 = an INTER cell)        (default 1)
#   VIDEO     1 = video-mode config even at FRAMES=1 (a video-mode KEY
#             frame — the N=1 control an inter differencing needs)    (default 0)
#   SHIFT     px/frame horizontal translation for FRAMES > 1  (default 3)
#
# INTER / VIDEO cells (FRAMES > 1 or VIDEO=1) export the SAME env set
# tools/perf_gate.sh exports (SVTAV1_FRAMES/_FRAME_SHIFT/_INTRA_PERIOD/
# _HIER_LEVELS/_INTER_EXPERIMENTAL + SVTAV1_VIDEO on the port; SVT_FRAMES/
# _INTRA_PERIOD/_HIER_LEVELS/_PRED_STRUCT + SVT_AVIF=0 on C), so a cell here
# is the same encode the wall-clock harness times. Identity is then checked
# PER FRAME (port <pfx>.obu.f<i> vs C <pfx>.c.obu.pts<i>), never on the
# concatenation — a frame-0 length change shifts every later byte and a
# whole-stream cmp names the wrong frame. The inter frame's OWN cost is the
# per-symbol DIFFERENCE of two cells, FRAMES=2 minus (FRAMES=1, VIDEO=1):
# see inter_delta.py. The env applies to every cell of one invocation, so run
# the script once per (FRAMES, VIDEO) pair and give the cells distinct names.
#
# Per (cell, preset) it does, IN THIS ORDER, and stops the cell on any failure:
#   1. identity pre-pass, NOT instrumented: port writes <yuv>+<obu>, C reads
#      the SAME <yuv> and writes its own .obu; `cmp` -> ident=Y/N. A cell that
#      is not byte-identical is recorded (ident=N) and its counts are STILL
#      taken, but the summary flags them: a count on a divergent encode
#      compares different work and must not be read as a ratio.
#   2. callgrind the port (warmup=0: exactly one init+encode), then `cmp` the
#      OBU the instrumented run itself wrote against the pre-pass's.
#   3. callgrind C the same way, same check.
#   4. callcount.py --demangle on both -> cc_{port,c}_<name>_p<preset>.tsv
#      callgrind_annotate --threshold=100                 -> self_{port,c}_<name>_p<preset>.txt
#      callgrind_annotate --inclusive=yes --threshold=100 -> incl_{port,c}_<name>_p<preset>.txt
#      callgrind_annotate --tree=caller                   -> tree_{port,c}_<name>_p<preset>.txt
#      (--threshold=100 keeps EVERY function; the default drops everything
#      below the 99 % cumulative mark, which is exactly the small kernels a
#      per-symbol difference needs.)
# Summary row per (cell, preset) -> <outdir>/cells.tsv; progress -> progress.log;
# the exit code lands in <outdir>/done.rc when everything has run (wait on that
# file, never on a pgrep of this script's name — WORKING-ON-THIS.md §5).
#
# Both binaries must be the generic-baseline builds (port: `env -u RUSTFLAGS`,
# no target-cpu=native; C: NATIVE=OFF, runtime RTCD) — this script does not
# build anything, on purpose: a rebuild under a live sweep is the §5 trap.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/../.." && pwd)
OUT=$1; shift
mkdir -p "$OUT"
read -r -a PRESETS <<<"${PRESETS:-2 6 10}"
QP="${QP:-40}"
PE="${PE:-$RS/target/release/examples/perf_encode}"
CE="${CE:-$RS/tools/perf_c_encode/perf_c_encode}"
VG="${VALGRIND:-valgrind}"
FRAMES="${FRAMES:-1}"
VIDEO="${VIDEO:-0}"
SHIFT="${SHIFT:-3}"
# The matched GOP + the port's experimental-inter unlock — byte-for-byte the
# block tools/perf_gate.sh exports, exported ONLY for video/inter cells so a
# still invocation's environment is unchanged.
if [[ "$FRAMES" -gt 1 || "$VIDEO" == "1" ]]; then
    export SVTAV1_FRAMES="$FRAMES" SVTAV1_FRAME_SHIFT="$SHIFT" \
           SVTAV1_INTRA_PERIOD="${SVTAV1_INTRA_PERIOD:-64}" \
           SVTAV1_HIER_LEVELS="${SVTAV1_HIER_LEVELS:-0}" \
           SVTAV1_INTER_EXPERIMENTAL=1
    [[ "$VIDEO" == "1" ]] && export SVTAV1_VIDEO=1 SVT_AVIF=0
    export SVT_FRAMES="$FRAMES" \
           SVT_INTRA_PERIOD="${SVT_INTRA_PERIOD:--1}" \
           SVT_HIER_LEVELS="${SVT_HIER_LEVELS:-0}" \
           SVT_PRED_STRUCT="${SVT_PRED_STRUCT:-1}"
fi
[[ -x "$PE" ]] || { echo "no port harness at $PE" >&2; exit 1; }
[[ -x "$CE" ]] || { echo "no C harness at $CE" >&2; exit 1; }
command -v "$VG" >/dev/null || { echo "no valgrind" >&2; exit 1; }
command -v callgrind_annotate >/dev/null || { echo "no callgrind_annotate" >&2; exit 1; }
command -v rustfilt >/dev/null || { echo "no rustfilt (cargo install rustfilt)" >&2; exit 1; }

LOG="$OUT/progress.log"
SUM="$OUT/cells.tsv"
rm -f "$OUT/done.rc"
say() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" | tee -a "$LOG"; }
if [[ ! -s "$SUM" ]]; then
    {
        echo "# callcount_cells.sh summary — one row per (cell, preset)"
        echo "# port=$PE"
        echo "# c=$CE"
        echo "# valgrind=$($VG --version) qp=$QP presets=[${PRESETS[*]}] frames=$FRAMES video=$VIDEO shift=$SHIFT date=$(date -u +%Y-%m-%dT%H:%M:%SZ) host=$(hostname)"
        printf 'cell\tcontent\twidth\theight\tpreset\tident\tobu_bytes\tobu_sha256\tport_ir\tc_ir\tport_over_c\tport_cg_ident\tc_cg_ident\tframes\tvideo\tident_frames\n'
    } >"$SUM"
fi
ir_total() { # <callgrind out file>
    # callgrind writes `summary: <Ir>` (and `totals:`) at the end of the file.
    awk '/^(summary|totals):/ { print $2; exit }' "$1"
}
sha() { sha256sum "$1" | cut -c1-16; }
# Port-vs-C identity for one cell. Still cells: the whole stream. Video/inter
# cells: PER FRAME (port <pfx>.obu.f<i> vs C <pfx>.c.obu.pts<i>), the rule
# perf_gate.sh and identity_diff_inter.sh state. Sets `ident` (Y/N) and
# `identf` (f0=Y,f1=N,... or `-` for a still cell); a missing per-frame file
# is N, never "not compared".
ident_check() { # <pfx>
    local pfx=$1 fi
    identf="-"; ident=Y
    if [[ "$FRAMES" -eq 1 && "$VIDEO" != "1" ]]; then
        cmp -s "$pfx.obu" "$pfx.c.obu" || ident=N
        return
    fi
    identf=""
    for ((fi = 0; fi < FRAMES; fi++)); do
        local v=Y
        if [[ ! -s "$pfx.obu.f$fi" || ! -s "$pfx.c.obu.pts$fi" ]]; then v=N
        elif ! cmp -s "$pfx.obu.f$fi" "$pfx.c.obu.pts$fi"; then v=N; fi
        [[ "$v" == N ]] && ident=N
        identf+="${identf:+,}f$fi=$v"
    done
}

rc=0
for cell in "$@"; do
    IFS=: read -r name content w h <<<"$cell"
    # `raw:<path>` carries a colon of its own: re-split from the right.
    if [[ "$content" == raw ]]; then
        # name:raw:/abs/path.yuv:w:h  -> content=raw:/abs/path.yuv
        rest=${cell#"$name":raw:}
        h=${rest##*:}; rest=${rest%:*}
        w=${rest##*:}; rest=${rest%:*}
        content="raw:$rest"
    fi
    for p in "${PRESETS[@]}"; do
        pfx="$OUT/${name}_p${p}"
        say "== $name ($content ${w}x${h}) p$p: identity pre-pass"
        rm -f "$pfx".obu "$pfx".obu.f* "$pfx".c.obu "$pfx".c.obu.pts* \
              "$pfx".cgp.obu "$pfx".cgp.obu.f* "$pfx".cgc.obu "$pfx".cgc.obu.pts*
        "$PE" "$content" "$w" "$h" "$QP" "$p" "$pfx" 0 >"$pfx.port.txt" 2>&1 \
            || { say "   port pre-pass FAILED (rc=$?), see $pfx.port.txt"; rc=1; continue; }
        "$CE" "$w" "$h" "$QP" "$p" "$pfx.yuv" "$pfx.c.obu" 0 >"$pfx.c.txt" 2>&1 \
            || { say "   C pre-pass FAILED (rc=$?), see $pfx.c.txt"; rc=1; continue; }
        ident_check "$pfx"
        bytes=$(stat -c %s "$pfx.obu"); osha=$(sha "$pfx.obu")
        say "   ident=$ident [$identf] port=$(stat -c %s "$pfx.obu")B C=$(stat -c %s "$pfx.c.obu")B"

        say "   callgrind port"
        nice -n 19 "$VG" --tool=callgrind --callgrind-out-file="$OUT/cg_port_${name}_p${p}.out" \
            "$PE" "$content" "$w" "$h" "$QP" "$p" "$pfx.cgp" 0 >"$pfx.cgp.log" 2>&1 \
            || { say "   port callgrind FAILED, see $pfx.cgp.log"; rc=1; continue; }
        pcg=Y; cmp -s "$pfx.cgp.obu" "$pfx.obu" || pcg=N
        say "   callgrind C"
        nice -n 19 "$VG" --tool=callgrind --callgrind-out-file="$OUT/cg_c_${name}_p${p}.out" \
            "$CE" "$w" "$h" "$QP" "$p" "$pfx.yuv" "$pfx.cgc.obu" 0 >"$pfx.cgc.log" 2>&1 \
            || { say "   C callgrind FAILED, see $pfx.cgc.log"; rc=1; continue; }
        ccg=Y; cmp -s "$pfx.cgc.obu" "$pfx.c.obu" || ccg=N

        for side in port c; do
            cg="$OUT/cg_${side}_${name}_p${p}.out"
            python3 "$HERE/callcount.py" "$cg" --demangle >"$OUT/cc_${side}_${name}_p${p}.tsv"
            callgrind_annotate --auto=no --threshold=100 "$cg" >"$OUT/self_${side}_${name}_p${p}.txt" 2>/dev/null
            callgrind_annotate --auto=no --inclusive=yes --threshold=100 "$cg" >"$OUT/incl_${side}_${name}_p${p}.txt" 2>/dev/null
            callgrind_annotate --tree=caller "$cg" >"$OUT/tree_${side}_${name}_p${p}.txt" 2>/dev/null
        done
        pir=$(ir_total "$OUT/cg_port_${name}_p${p}.out"); cir=$(ir_total "$OUT/cg_c_${name}_p${p}.out")
        ratio=$(awk -v a="$pir" -v b="$cir" 'BEGIN { if (b > 0) printf "%.3f", a / b; else print "na" }')
        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
            "$name" "$content" "$w" "$h" "$p" "$ident" "$bytes" "$osha" "$pir" "$cir" "$ratio" "$pcg" "$ccg" \
            "$FRAMES" "$VIDEO" "$identf" >>"$SUM"
        say "   done: Ir port=$pir C=$cir ratio=$ratio (cg-run ident port=$pcg C=$ccg)"
    done
done
echo "$rc" >"$OUT/done.rc"
say "ALL DONE rc=$rc"
exit "$rc"

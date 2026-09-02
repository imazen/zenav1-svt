#!/usr/bin/env bash
# Peak RSS of the port vs the C encoder, across image sizes — STILL and INTER.
#
#   tools/mem_gate.sh [preset]        # default 6, still (1 frame)
#   MEM_FRAMES=2 tools/mem_gate.sh 6  # low-delay P: key + one INTER frame
#
# WHY. `docs/ACCEPTANCE-CRITERIA.md` requires memory numbers to come from
# heaptrack or `time -v` and never from struct arithmetic — and then the project
# never measured any. "How much memory does it use?" had no answer at all.
#
# WHY MULTIPLE SIZES. A single figure is meaningless: peak RSS is
# `alpha + beta * pixels`, and at 64x64 the intercept dominates while at 1024x1024
# the slope does. Reporting one number without the intercept mis-sizes every
# decision at the other end of the range. This sweeps tiny -> large and prints
# both terms — plus the ADJACENT-PAIR slopes, because a single least-squares
# slope over a wide range hides that the slope itself moves with the range
# (measured: 33.6 MiB/MP over 64..1024 against 29.2 over 1024..2048). Quoting
# either fit at a size outside its own range is the extrapolation this repo bans.
#
# WHY BYTE IDENTITY IS CHECKED HERE TOO. A memory ratio between two encoders
# doing DIFFERENT work is as meaningless as a wall-clock ratio between them —
# `perf_gate.sh` refuses to fit non-identical cells for exactly that reason. This
# gate cannot refuse (the inter frontier is small and the sizes worth measuring
# are outside it), so it MEASURES identity per cell and prints it in the `ident`
# column: Y = both encoders emitted the same bytes, N = they did not and the
# ratio compares different work, - = not checked. Read an `N` row as two
# independent absolute numbers, never as a ratio.
#
# `/usr/bin/time -l` (BSD/macOS) reports maximum resident set size; `-v` on GNU.
# Both are the sanctioned evidence. RSS is a ceiling on what the process touched,
# not an allocator trace — for allocation-level questions use heaptrack on Linux.
#
# Env:
#   MEM_SIZES    square dims to sweep            (default "64 256 512 1024")
#   MEM_QP       CLI qp 0..63                    (default 32)
#   MEM_CONTENT  synthetic content               (default gradient)
#   MEM_FRAMES   frames per encode               (default 1 = still)
#   MEM_SHIFT    px/frame translation when >1 fr (default 3)
#   MEM_RS_BIN   port binary to measure          (default tools/identity_run,
#                the always-fresh wrapper — which builds with `--features
#                symtrace`. Point this at a plain `--release` build of the
#                `identity_run` example to measure the SHIPPED configuration.)
#   MEM_VIDEO    1 = video-mode config even at MEM_FRAMES=1 — a video-mode KEY
#                frame and nothing else. The same control `perf_gate.sh`'s
#                PERF_VIDEO is, and for the same reason: a 2-frame cell changes
#                the still-vs-video signal derivation AND adds an inter frame,
#                and only three arms can tell which of them costs the memory.
#                REQUIRES MEM_RS_BIN pointing at the `perf_encode` example —
#                `identity_run` has no single-frame video arm.
#   MEM_REPS     repeats per cell, median reported   (default 5)
#   MEM_TSV      write a machine-readable TSV here as well as the table
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE/.."
PRESET=${1:-6}
SIZES=${MEM_SIZES:-"64 256 512 1024"}
QP=${MEM_QP:-32}
CONTENT=${MEM_CONTENT:-gradient}
FRAMES=${MEM_FRAMES:-1}
SHIFT=${MEM_SHIFT:-3}
VIDEO=${MEM_VIDEO:-0}
RS_BIN=${MEM_RS_BIN:-$HERE/identity_run}
REPS=${MEM_REPS:-5}
TSV=${MEM_TSV:-}
W=${TMPDIR:-$HOME/tmp}/memgate.$$; mkdir -p "$W"; trap 'rm -rf "$W"' EXIT

# Portable "peak RSS in KiB" for one command. Sets PEAK_KIB and CMD_RC.
#
# CMD_RC IS LOAD-BEARING, not bookkeeping. `identity_run` exits 3 when the port
# REFUSED a frame and 101 when it PANICKED, and in both cases it still leaves
# the frames it managed to encode on disk. A gate that looks only at the files
# therefore reads a refusal and a crash as an ordinary byte divergence — the
# exact conflation `identity_diff_inter.sh` grew its own exit code 4 to stop
# (see its header). MEASURED 2026-09-02: at >= 576x576 the port refuses frame 1
# at presets 6/8 and panics on frame 0 at presets 9/10, while C encodes both
# frames — so those cells' port RSS is the peak of a DIFFERENT, SMALLER
# workload, and comparing it to C would understate the port by ~2x.
peak_kib() {
    local rcf="$W/.rc"
    if $BSD_TIME; then                                        # BSD / macOS
        { /usr/bin/time -l "$@" 2>"$W/.t" >/dev/null; echo $? >"$rcf"; }
        PEAK_KIB=$(( $(grep -i "maximum resident" "$W/.t" | awk '{print $1}') / 1024 ))
    else                                                      # GNU
        { /usr/bin/time -v "$@" 2>"$W/.t" >/dev/null; echo $? >"$rcf"; }
        PEAK_KIB=$(grep -i "Maximum resident" "$W/.t" | awk '{print $NF}')   # already KiB
    fi
    CMD_RC=$(cat "$rcf")
}
BSD_TIME=false
/usr/bin/time -l true >/dev/null 2>&1 && BSD_TIME=true

# Median peak RSS over $REPS runs of one command, plus the observed spread.
# Sets MED_KIB / MIN_KIB / MAX_KIB / CMD_RC (the LAST run's).
#
# WHY A MEDIAN AND A SPREAD, not one number. MEASURED 2026-09-02, gradient
# 2048x2048 preset 6: the port's peak RSS over six runs of the same binary on
# the same input spanned 126.3 - 134.6 MiB (6.6 %), while C's spanned
# 119.5 - 119.9 MiB (0.3 %). The port encodes tiles on a `std::thread::scope`
# pool, so how much of each tile's working set is live at once depends on
# scheduling; C at `--lp 1` does not have that freedom. A single-run port
# figure therefore carries several percent of noise that a single-run C figure
# does not, and a ratio built from two single runs inherits all of it.
peak_kib_median() {
    local vals=() i
    for ((i = 0; i < REPS; i++)); do
        peak_kib "$@"
        vals+=("$PEAK_KIB")
    done
    read -r MED_KIB MIN_KIB MAX_KIB <<<"$(printf '%s\n' "${vals[@]}" | sort -n | awk '
        { v[NR] = $1 }
        END { printf "%d %d %d", (NR % 2) ? v[(NR+1)/2] : int((v[NR/2] + v[NR/2+1]) / 2), v[1], v[NR] }')"
}

# The GOP shape is the one identity_diff_inter.sh uses, and it is matched on
# both sides: low-delay P, flat (no pyramid), only frame 0 a key frame.
port_env=(SVTAV1_BD=8 "SVTAV1_FRAMES=$FRAMES")
c_env=(SVT_TRACE_OUT=/dev/null "SVT_FRAMES=$FRAMES")
if [[ $FRAMES -gt 1 || $VIDEO == 1 ]]; then
    port_env+=(SVTAV1_INTER_EXPERIMENTAL=1 "SVTAV1_FRAME_SHIFT=$SHIFT"
               SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0)
    c_env+=(SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1)
    [[ $VIDEO == 1 ]] && { port_env+=(SVTAV1_VIDEO=1); c_env+=(SVT_AVIF=0); }
fi

echo "mem_gate: content=$CONTENT preset=$PRESET qp=$QP frames=$FRAMES shift=$SHIFT video=$VIDEO"
echo "mem_gate: port=$RS_BIN"
printf "%-9s %-7s %12s %-15s %12s %-15s %8s %6s %9s\n" \
    "size" "px(MP)" "port KiB" "[min,max]" "C KiB" "[min,max]" "port/C" "ident" "port"
rows=""
[[ -n "$TSV" ]] && {
    printf '# mem_gate: content=%s preset=%s qp=%s frames=%s shift=%s host=%s date=%s\n' \
        "$CONTENT" "$PRESET" "$QP" "$FRAMES" "$SHIFT" "$(hostname)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$TSV"
    printf '# video=%s\n' "$VIDEO" >>"$TSV"
    printf '# reps=%s (median reported; min/max are the observed spread)  port=%s\n' "$REPS" "$RS_BIN" >>"$TSV"
    printf 'size\tmegapixels\tframes\tpreset\tqp\treps\tport_kib\tport_min\tport_max\tc_kib\tc_min\tc_max\tratio\tident\tport_status\n' >>"$TSV"
}
for s in $SIZES; do
    mp=$(awk -v s="$s" 'BEGIN{printf "%.4f", s*s/1048576}')
    rm -f "$W"/rs.obu* "$W"/c.obu*
    peak_kib_median env "${port_env[@]}" "$RS_BIN" "$CONTENT" "$s" "$s" "$QP" "$PRESET" "$W/rs"
    p=$MED_KIB; pmin=$MIN_KIB; pmax=$MAX_KIB; prc=$CMD_RC
    peak_kib_median env "${c_env[@]}" "$HERE/capture_c_trace/capture_c_trace" \
            "$s" "$s" "$QP" "$PRESET" "$W/rs.yuv" "$W/c.obu" 8
    c=$MED_KIB; cmin=$MIN_KIB; cmax=$MAX_KIB; crc=$CMD_RC
    # The port's own verdict FIRST — see the note on peak_kib. `identity_run`
    # exits 3 on a REFUSAL and 101 on a PANIC, and both leave partial output.
    case "$prc" in
        0)   pstat=OK ;;
        3)   pstat=REFUSED ;;
        101) pstat=CRASH ;;
        *)   pstat="rc$prc" ;;
    esac
    [[ "$crc" != 0 ]] && pstat="$pstat/Crc$crc"
    # Byte identity, per frame. Only meaningful when BOTH sides completed.
    ident="-"
    if [[ "$pstat" == OK ]]; then
        # One frame compares the whole stream on both sides — including the
        # MEM_VIDEO arm, because `capture_c_trace` writes its per-PTS files only
        # when SVT_FRAMES > 1, so `c.obu.pts0` does not exist here.
        if [[ $FRAMES -eq 1 ]]; then
            [[ -s "$W/rs.obu" && -s "$W/c.obu" ]] && { ident=N; cmp -s "$W/rs.obu" "$W/c.obu" && ident=Y; }
        else
            allsame=1; seen=0
            for ((i = 0; i < FRAMES; i++)); do
                [[ -s "$W/rs.obu.f$i" && -s "$W/c.obu.pts$i" ]] || { allsame=0; continue; }
                seen=1
                cmp -s "$W/rs.obu.f$i" "$W/c.obu.pts$i" || allsame=0
            done
            [[ $seen -eq 1 ]] && { ident=N; [[ $allsame -eq 1 ]] && ident=Y; }
        fi
    fi
    if [[ "$pstat" == OK ]]; then
        ratio=$(awk -v a="$p" -v b="$c" 'BEGIN{ if (b>0) printf "%.2f", a/b; else print "-" }')
    else
        # A refusal or a crash means the port did LESS work than C. Printing a
        # ratio here would read as "the port is lighter", which is the exact
        # wrong conclusion; the two absolute numbers stay, the ratio does not.
        ratio="-"
    fi
    printf "%-9s %-7s %12s %-15s %12s %-15s %8s %6s %9s\n" \
        "${s}x${s}" "$mp" "$p" "[$pmin,$pmax]" "$c" "[$cmin,$cmax]" "$ratio" "$ident" "$pstat"
    # Only cells where the port completed enter the fit.
    [[ "$pstat" == OK ]] && rows="$rows$s $p $c
"
    [[ -n "$TSV" ]] && printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$s" "$mp" "$FRAMES" "$PRESET" "$QP" "$REPS" "$p" "$pmin" "$pmax" \
        "$c" "$cmin" "$cmax" "$ratio" "$ident" "$pstat" >>"$TSV"
done

# alpha + beta*pixels, least squares on both series. The INTERCEPT is the part a
# single-size measurement hides — and the adjacent-pair slopes below are the part
# a single least-squares fit hides.
echo
printf '%s' "$rows" | awk '
  NF==3 { n++; sz[n]=$1; P[n]=$2; C[n]=$3; x=$1*$1; X[n]=x
          sx+=x; sxx+=x*x; sp+=$2; sxp+=x*$2; sc+=$3; sxc+=x*$3 }
  END {
    if (n < 2) { print "  (need >= 2 sizes to fit)"; exit }
    d = n*sxx - sx*sx
    bp = (n*sxp - sx*sp)/d; ap = (sp - bp*sx)/n
    bc = (n*sxc - sx*sc)/d; ac = (sc - bc*sx)/n
    printf "  least squares over %dx%d..%dx%d (cells where the port COMPLETED):\n", sz[1], sz[1], sz[n], sz[n]
    printf "    port:  %.2f MiB fixed  +  %.2f MiB/MP\n", ap/1024, bp*1048576/1024
    printf "    C   :  %.2f MiB fixed  +  %.2f MiB/MP\n", ac/1024, bc*1048576/1024
    print  "  adjacent-pair slopes (the fit above is only valid inside its own range):"
    for (i = 2; i <= n; i++) {
      dpx = (X[i]-X[i-1])/1048576
      printf "    %dx%d -> %dx%d :  port %.2f MiB/MP   C %.2f MiB/MP\n",
             sz[i-1], sz[i-1], sz[i], sz[i], (P[i]-P[i-1])/1024/dpx, (C[i]-C[i-1])/1024/dpx
    }
  }'

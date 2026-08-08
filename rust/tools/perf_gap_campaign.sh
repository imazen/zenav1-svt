#!/usr/bin/env bash
# Encode-speed gap campaign: port vs the C SVT-AV1 reference, aarch64.
#
# Same contract as tools/perf_gate.sh (interleaved paired rounds, randomized
# per-round order, byte-identity pre-check per cell, intercept+slope fit) with
# three differences that matter on this box:
#   * NEITHER SIDE IS NICED. `nice` on macOS maps to background QoS, which
#     parks the work on the E-cores and costs multiples of wall clock. Builds
#     are niced; measurements never are.
#   * a TINY 32x32 cell is included, because fixed per-call overhead is
#     invisible at 512+ and the fit's intercept is meaningless without it.
#   * presets 2/6/10/13 so the fit spans the RD-heavy and the fast tiers.
#
# Both harnesses time ONLY the frame encode (setup excluded on both sides).
# Usage: tools/perf_gap_campaign.sh <suffix>
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

SUFFIX="${1:-$(date +%Y-%m-%d)}"
OUT="$RS_ROOT/benchmarks/perf_gap_${SUFFIX}.tsv"
RAW="$RS_ROOT/benchmarks/perf_gap_${SUFFIX}.raw.tsv"
META="$RS_ROOT/benchmarks/perf_gap_${SUFFIX}.meta"

read -r -a SIZES <<<"${PERF_SIZES:-32 64 128 256 512 1024}"
read -r -a PRESETS <<<"${PERF_PRESETS:-2 6 10 13}"
CONTENT="${PERF_CONTENT:-gradient}"
QP="${PERF_QP:-40}"
ROUNDS="${PERF_ROUNDS:-9}"
WARMUP="${PERF_WARMUP:-1}"

PE="$RS_ROOT/target/release/examples/perf_encode"
CE="$HERE/perf_c_encode/perf_c_encode"
WORK="$RS_ROOT/target/perf"; mkdir -p "$WORK"
[[ -x "$PE" && -x "$CE" ]] || { echo "harness binaries missing" >&2; exit 1; }

COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
HOST=$(hostname -s)
NCORES=$(sysctl -n hw.ncpu)
GRID="content=$CONTENT sizes=[${SIZES[*]}] presets=[${PRESETS[*]}] qp=$QP rounds=$ROUNDS warmup=$WARMUP"

{
    echo "# perf_gap raw samples — one row per interleaved paired round (NEITHER SIDE NICED)"
    echo "# commit=$COMMIT host=$HOST cores=$NCORES date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# $GRID"
    printf 'content\tsize\tpreset\tqp\tround\tport_ns\tc_ns\tident\n'
} >"$RAW"

run_port() { "$PE" "$CONTENT" "$1" "$1" "$QP" "$2" "$WORK/gap" "$WARMUP" 2>/dev/null | sed -n 's/.*ENCODE_NS=\([0-9]*\).*/\1/p'; }
run_c()    { "$CE" "$1" "$1" "$QP" "$2" "$WORK/gap.yuv" "$WORK/gap.c.obu" "$WARMUP" 2>/dev/null | sed -n 's/.*ENCODE_NS=\([0-9]*\).*/\1/p'; }

for sz in "${SIZES[@]}"; do
    for preset in "${PRESETS[@]}"; do
        run_port "$sz" "$preset" >/dev/null
        run_c "$sz" "$preset" >/dev/null
        if cmp -s "$WORK/gap.obu" "$WORK/gap.c.obu"; then ident=Y; else ident=N
            echo "  WARN ${sz}x${sz} p${preset}: NOT byte-identical — excluded from fit"; fi
        for ((r = 1; r <= ROUNDS; r++)); do
            if (( RANDOM % 2 )); then pns=$(run_port "$sz" "$preset"); cns=$(run_c "$sz" "$preset")
            else cns=$(run_c "$sz" "$preset"); pns=$(run_port "$sz" "$preset"); fi
            [[ -n "$pns" && -n "$cns" ]] || continue
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$CONTENT" "$sz" "$preset" "$QP" "$r" "$pns" "$cns" "$ident" >>"$RAW"
        done
        printf '  measured %sx%s p%-2s ident=%s\n' "$sz" "$sz" "$preset" "$ident"
    done
done

gawk -v commit="$COMMIT" -v host="$HOST" -v cores="$NCORES" -v grid="$GRID" \
     -v out="$OUT" -v meta="$META" -v content="$CONTENT" '
function median(a, n,   b, m) { m = asort(a, b); if (m == 0) return 0;
    return (m % 2) ? b[(m + 1) / 2] : (b[m / 2] + b[m / 2 + 1]) / 2 }
function pct(a, n, p,   b, m, i) { m = asort(a, b); if (m == 0) return 0;
    i = int(p / 100 * m + 0.5); if (i < 1) i = 1; if (i > m) i = m; return b[i] }
BEGIN { FS = OFS = "\t" }
/^#/ || /^content\t/ { next }
{ key = $2 SUBSEP $3; n = ++cnt[key]
  pns[key,n]=$6; cns[key,n]=$7; ratio[key,n]=$6/$7; ident[key]=$8
  sizes[$2]=1; presets[$3]=1 }
END {
    print "# perf_gap summary — port vs C per-frame encode wall time, aarch64, NEITHER SIDE NICED" > out
    print "# commit=" commit "  host=" host "  cores=" cores "  date=" strftime("%Y-%m-%dT%H:%M:%SZ", systime(), 1) > out
    print "# " grid > out
    print "# ratio = port/C (median of per-round paired ratios). ident=Y means byte-identical output." > out
    print "size\tpreset\tident\tn\tport_ms\tc_ms\tratio\tratio_p25\tratio_p75" > out
    ns=0; for (s in sizes) sz_list[++ns]=s+0
    np=0; for (p in presets) pr_list[++np]=p+0
    asort(sz_list); asort(pr_list)
    for (i=1;i<=ns;i++) for (j=1;j<=np;j++) {
        s=sz_list[i]; p=pr_list[j]; key=s SUBSEP p; if (!(key in cnt)) continue
        n=cnt[key]; delete pa; delete ca; delete ra
        for (k=1;k<=n;k++){pa[k]=pns[key,k];ca[k]=cns[key,k];ra[k]=ratio[key,k]}
        printf "%d\t%d\t%s\t%d\t%.4f\t%.4f\t%.3f\t%.3f\t%.3f\n", s,p,ident[key],n,
            median(pa,n)/1e6, median(ca,n)/1e6, median(ra,n), pct(ra,n,25), pct(ra,n,75) > out
        if (ident[key]=="Y"){PM[p,s]=median(pa,n)/1e6; CM[p,s]=median(ca,n)/1e6; HAVE[p,s]=1}
    }
    close(out)
    fc=0
    for (j=1;j<=np;j++){ p=pr_list[j]; n=0; sx=sy_p=sxx=sxy_p=sy_c=sxy_c=0
        for (i=1;i<=ns;i++){ s=sz_list[i]; if(!((p,s) in HAVE)) continue
            x=s*s; n++; sx+=x; sxx+=x*x; sy_p+=PM[p,s]; sxy_p+=x*PM[p,s]; sy_c+=CM[p,s]; sxy_c+=x*CM[p,s] }
        if (n<2) continue
        den=n*sxx-sx*sx
        bp=(n*sxy_p-sx*sy_p)/den; ap=(sy_p-bp*sx)/n
        bc=(n*sxy_c-sx*sy_c)/den; ac=(sy_c-bc*sx)/n
        FIT[++fc]=sprintf("p%-2d  port: a=%.4f ms  b=%.2f ms/MP   |   C: a=%.4f ms  b=%.2f ms/MP   |   slope-ratio=%.2fx  intercept-ratio=%.2fx",
            p, ap, bp*1e6, ac, bc*1e6, (bc!=0?bp/bc:0), (ac!=0?ap/ac:0)) }
    print ""; print "==== PER-CELL (port/C) ===="
    printf "%-6s %-7s %-6s %-11s %-11s %-8s %s\n","size","preset","ident","port_ms","C_ms","ratio","[p25,p75]"
    for (i=1;i<=ns;i++) for (j=1;j<=np;j++){
        s=sz_list[i]; p=pr_list[j]; key=s SUBSEP p; if(!(key in cnt)) continue
        n=cnt[key]; delete pa; delete ca; delete ra
        for(k=1;k<=n;k++){pa[k]=pns[key,k];ca[k]=cns[key,k];ra[k]=ratio[key,k]}
        printf "%-6d %-7d %-6s %-11.4f %-11.4f %-8.2f [%.2f, %.2f]\n", s,p,ident[key],
            median(pa,n)/1e6, median(ca,n)/1e6, median(ra,n), pct(ra,n,25), pct(ra,n,75) }
    print ""; print "==== FIT  ms = a + b*pixels  (byte-identical cells) ===="
    for (f=1;f<=fc;f++) print FIT[f]
    print "# perf_gap fit" > meta
    print "date: " strftime("%Y-%m-%dT%H:%M:%SZ", systime(), 1) > meta
    print "commit: " commit > meta
    print "host: " host " (" cores " logical cores)" > meta
    print "grid: " grid > meta
    print "harness: interleaved paired, randomized per-round order; no target-cpu=native; NEITHER SIDE NICED; setup excluded both sides" > meta
    print "fits (ms = a + b*pixels):" > meta
    for (f=1;f<=fc;f++) print "  " FIT[f] > meta
    close(meta)
}' "$RAW"
echo; echo "summary : $OUT"; echo "raw     : $RAW"; echo "meta    : $META"

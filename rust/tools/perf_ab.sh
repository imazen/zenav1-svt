#!/usr/bin/env bash
# Interleaved A/B of two PORT `perf_encode` binaries (baseline vs candidate).
#
# The per-optimization measurement tool for the perf program. `perf_gap_campaign.sh`
# answers "how far are we from C"; this answers "did THIS change help, and by how
# much", which is the question you must be able to answer per commit or a
# regression is unattributable.
#
# Contract, same shape as perf_gap_campaign.sh:
#   * interleaved paired rounds, order randomised per round (kills drift/thermal
#     bias — a back-to-back A-then-B layout bakes it in),
#   * one untimed warmup encode inside each sample (the harness's own flag),
#   * NEITHER SIDE NICED (nice = background QoS on macOS),
#   * median of the per-round paired ratios, plus p25/p75 so noise is visible.
#
# Usage: tools/perf_ab.sh <baseline_bin> <candidate_bin> <out.tsv>
# Env:   AB_SIZES / AB_PRESETS / AB_QP / AB_CONTENT / AB_ROUNDS override the grid.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

BASE="${1:?usage: perf_ab.sh <baseline_bin> <candidate_bin> <out.tsv>}"
CAND="${2:?usage: perf_ab.sh <baseline_bin> <candidate_bin> <out.tsv>}"
OUT="${3:?usage: perf_ab.sh <baseline_bin> <candidate_bin> <out.tsv>}"
[[ -x "$BASE" && -x "$CAND" ]] || { echo "both binaries must be executable" >&2; exit 1; }

read -r -a SIZES <<<"${AB_SIZES:-64 256 512}"
read -r -a PRESETS <<<"${AB_PRESETS:-2 6 10}"
CONTENT="${AB_CONTENT:-gradient}"
QP="${AB_QP:-40}"
ROUNDS="${AB_ROUNDS:-7}"

WORK="$RS_ROOT/target/perfab"; mkdir -p "$WORK"
RAW="${OUT%.tsv}.raw.tsv"

{
    echo "# perf_ab raw — one row per interleaved paired round (NEITHER SIDE NICED)"
    echo "# base=$BASE"
    echo "# cand=$CAND"
    echo "# commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown) host=$(hostname -s) date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# content=$CONTENT sizes=[${SIZES[*]}] presets=[${PRESETS[*]}] qp=$QP rounds=$ROUNDS"
    printf 'size\tpreset\tround\tbase_ns\tcand_ns\tident\n'
} >"$RAW"

run() { "$1" "$CONTENT" "$2" "$2" "$QP" "$3" "$4" 1 2>/dev/null | sed -n 's/.*ENCODE_NS=\([0-9]*\).*/\1/p'; }

for sz in "${SIZES[@]}"; do
    for preset in "${PRESETS[@]}"; do
        run "$BASE" "$sz" "$preset" "$WORK/b" >/dev/null
        run "$CAND" "$sz" "$preset" "$WORK/c" >/dev/null
        if cmp -s "$WORK/b.obu" "$WORK/c.obu"; then ident=Y; else ident=N
            echo "  *** ${sz}x${sz} p${preset}: OUTPUT DIFFERS — not a bit-identical change"; fi
        for ((r = 1; r <= ROUNDS; r++)); do
            if ((RANDOM % 2)); then bn=$(run "$BASE" "$sz" "$preset" "$WORK/b"); cn=$(run "$CAND" "$sz" "$preset" "$WORK/c")
            else cn=$(run "$CAND" "$sz" "$preset" "$WORK/c"); bn=$(run "$BASE" "$sz" "$preset" "$WORK/b"); fi
            [[ -n "$bn" && -n "$cn" ]] || continue
            printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$sz" "$preset" "$r" "$bn" "$cn" "$ident" >>"$RAW"
        done
        printf '  measured %sx%s p%-2s ident=%s\n' "$sz" "$sz" "$preset" "$ident"
    done
done

gawk -v out="$OUT" '
function median(a,   b, m) { m = asort(a, b); if (m == 0) return 0;
    return (m % 2) ? b[(m + 1) / 2] : (b[m / 2] + b[m / 2 + 1]) / 2 }
function pct(a, p,   b, m, i) { m = asort(a, b); if (m == 0) return 0;
    i = int(p / 100 * m + 0.5); if (i < 1) i = 1; if (i > m) i = m; return b[i] }
BEGIN { FS = OFS = "\t" }
/^#/ || /^size\t/ { next }
{ k = $1 SUBSEP $2; n = ++c[k]; B[k,n]=$4; C[k,n]=$5; R[k,n]=$5/$4; ID[k]=$6
  sz[$1]=1; pr[$2]=1 }
END {
    print "# perf_ab — candidate/base per-frame encode wall time (median of paired rounds)"
    print "# speedup = base/cand (>1 means the candidate is FASTER). ident=Y: byte-identical output."
    print "size\tpreset\tident\tn\tbase_ms\tcand_ms\tratio\tspeedup\tratio_p25\tratio_p75"
    ns=0; for (s in sz) SL[++ns]=s+0; np=0; for (p in pr) PL[++np]=p+0
    asort(SL); asort(PL)
    for (i=1;i<=ns;i++) for (j=1;j<=np;j++) {
        s=SL[i]; p=PL[j]; k=s SUBSEP p; if (!(k in c)) continue
        delete ba; delete ca; delete ra
        for (t=1;t<=c[k];t++) { ba[t]=B[k,t]; ca[t]=C[k,t]; ra[t]=R[k,t] }
        m=median(ra)
        printf "%d\t%d\t%s\t%d\t%.4f\t%.4f\t%.4f\t%.3f\t%.4f\t%.4f\n", s,p,ID[k],c[k],
            median(ba)/1e6, median(ca)/1e6, m, (m!=0?1/m:0), pct(ra,25), pct(ra,75)
    }
}' "$RAW" >"$OUT"

cat "$OUT"
echo "raw: $RAW"

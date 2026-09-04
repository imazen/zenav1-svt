#!/usr/bin/env bash
# IFS JOIN GATE — the port's MDS3 interpolation-filter search
# (`leaf_funnel::ifs::ifs_at_mds3`) against C's, per candidate.
#
# C's `interpolation_filter_search` is `static` and takes the whole MD
# context, so it has no tier-1 shim. What CAN be observed is its caller:
# `svt_aom_inter_pu_prediction_av1` is exported and reached through a
# function table, so `-Wl,--wrap` binds it, and the `SVT_IFS_OUT` interposer
# (tools/capture_c_trace/wrap_recon.c) logs, per MDS3 candidate, the filter
# pair BEFORE and AFTER the call and the `fast_luma_rate` it added. The port's
# `SVTAV1_IFSDBG` dump prints the same tuple from `ifs_at_mds3`. This gate
# runs both on the SAME .yuv and joins them on the candidate identity
# (origin, size, mode, refs, MV):
#
#   * a joined candidate whose full-pel verdict, filter pair after the search
#     or added rate differ  -> MISMATCH (the gate FAILS);
#   * a candidate on one side only -> a CANDIDATE-SET difference (which MDS3
#     admitted; §1z³³'s MDS1-cap gap), reported, not a failure of the search;
#   * a cell where C searched at least once but nothing joined -> VACUOUS
#     (the gate FAILS — see docs/WORKING-ON-THIS.md §5 on gates that pass
#     without testing anything).
#
# The interposed driver's OBU must equal the host driver's (the probe is
# inert); that is checked per cell too.
#
# Usage: tools/ifs_join_gate.sh [outdir]
# Env:   IJG_CONTENT / IJG_SIZES / IJG_QPS / IJG_PRESETS (default: the 96-cell
#        grid's axes), IJG_FRAMES (default 2), IJG_SHIFT (SVTAV1_FRAME_SHIFT,
#        default 3 = the grid's). Needs docker for the C side
#        (tools/ctrace-linux/run.sh).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:-$HERE/../target/ifs-join}
CONTENT=${IJG_CONTENT:-"uniform gradient diag screen"}
SIZES=${IJG_SIZES:-"16 64 72 128"}
QPS=${IJG_QPS:-"20 40 55"}
PRESETS=${IJG_PRESETS:-"6 8"}
FRAMES=${IJG_FRAMES:-2}
export CTRACE_WORK=${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}
W=$CTRACE_WORK/ifs-join; mkdir -p "$W" "$OUT"
TSV=$OUT/ifs_join.tsv
printf 'cell\tc_bytes_match\tc_n\tp_n\tjoined\tmismatch\tc_only\tp_only\tverdict\n' > "$TSV"
fail=0; cells=0
for c in $CONTENT; do for s in $SIZES; do for q in $QPS; do for p in $PRESETS; do
    n="${c}_${s}x${s}_q${q}_p${p}"; d="$OUT/$n"; cells=$((cells+1))
    SVTAV1_IFSDBG=1 SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="${IJG_SHIFT:-3}" "$HERE/identity_diff_inter.sh" "$s" "$s" "$q" "$p" "$FRAMES" "$c" "$d" > "$d.report" 2>&1
    cp "$d/rs.yuv" "$W/$n.yuv"
    SVT_FRAMES=$FRAMES SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 SVT_IFS_OUT="$W/$n.ifs" \
        "$HERE/ctrace-linux/run.sh" "$s" "$s" "$q" "$p" "$W/$n.yuv" "$W/$n.obu" 8 > "$W/$n.log" 2>&1
    match=$(cmp -s "$W/$n.obu" "$d/c.obu" && echo yes || echo NO)
    [ -f "$W/$n.ifs" ] || : > "$W/$n.ifs"
    line=$(python3 - "$d/rs.trace" "$W/$n.ifs" <<'PY'
import sys, re, collections
port, cref = sys.argv[1], sys.argv[2]
def key_val(fields):
    f = dict(kv.split('=', 1) for kv in fields if '=' in kv)
    size = [t for t in fields if 'x' in t and '=' not in t][0]
    key = (f['org'], size, f['mode'], f['rf'], f['mv0'])
    b, a = f['interp'].split('->')
    d0, d1 = f['flr'].split('->')
    return key, (f['fp'], a, int(d1) - int(d0))
P = collections.Counter(); C = collections.Counter()
for l in open(port, errors='replace'):
    if l.startswith('IFSDBG sl=1 '):
        k, v = key_val(l.split()[1:]); P[(k, v)] += 1
for l in open(cref, errors='replace'):
    t = l.split()
    if len(t) > 5 and t[0] == 'IFS' and t[1] == 'poc=1' and t[3] == 'st=3' and t[4] == 'doifs=1':
        k, v = key_val(t[1:]); C[(k, v)] += 1
Pk = collections.Counter(); Ck = collections.Counter()
for (k, v), n in P.items(): Pk[k] += n
for (k, v), n in C.items(): Ck[k] += n
joined = sum(min(P[kv], C[kv]) for kv in set(P) | set(C))
keyjoin = sum(min(Pk[k], Ck[k]) for k in set(Pk) | set(Ck))
mism = keyjoin - joined
c_only = sum(C.values()) - keyjoin
p_only = sum(P.values()) - keyjoin
print(f"{sum(C.values())}\t{sum(P.values())}\t{keyjoin}\t{mism}\t{c_only}\t{p_only}")
PY
)
    c_n=$(echo "$line" | cut -f1); joined=$(echo "$line" | cut -f3); mism=$(echo "$line" | cut -f4)
    v=PASS
    if [ "$match" != yes ]; then v=PROBE_NOT_INERT; fi
    if [ "$mism" != 0 ]; then v=MISMATCH; fi
    if [ "$c_n" != 0 ] && [ "$joined" = 0 ]; then v=VACUOUS; fi
    [ "$v" = PASS ] || fail=$((fail+1))
    printf '%s\t%s\t%s\t%s\n' "$n" "$match" "$line" "$v" >> "$TSV"
done; done; done; done
echo "ifs join gate: $cells cells, $fail failed  ($TSV)"
[ "$fail" = 0 ] && echo "ifs join gate: PASS" || { echo "ifs join gate: FAIL"; exit 1; }

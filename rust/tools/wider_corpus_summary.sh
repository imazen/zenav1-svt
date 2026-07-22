#!/usr/bin/env bash
# Summarize a wider_corpus_sweep scoreboard into a DIVERGENCE MAP.
#   wider_corpus_summary.sh <scoreboard.tsv>
# Columns: corpus image width height bd preset qp verdict decodable c_bytes rust_bytes first_diff
set -uo pipefail
TSV="${1:?usage: wider_corpus_summary.sh <scoreboard.tsv>}"
[ -f "$TSV" ] || { echo "no such scoreboard: $TSV" >&2; exit 2; }

echo "======================================================================"
echo "WIDER-CORPUS DIVERGENCE MAP  ($TSV)"
echo "======================================================================"

awk -F'\t' '
NR==1 { next }
{
  total++
  v=$8
  cnt[v]++
  # class: screen vs photo
  cls = ($1=="screen") ? "screen" : "photo"
  ctot[$1]++;   cid[$1] += (v=="IDENTICAL")
  clstot[cls]++; clsid[cls] += (v=="IDENTICAL")
  ptot[$6]++;   pid[$6] += (v=="IDENTICAL")
  qtot[$7]++;   qid[$7] += (v=="IDENTICAL")
  btot[$5]++;   bid[$5] += (v=="IDENTICAL")
  if (v=="IDENTICAL") ident++
  if ($9=="no") undec++
  if (v!="IDENTICAL" && v!="DIFFERS") errs++
}
END {
  printf "\nOVERALL: %d/%d byte-identical (%.1f%%)\n", ident, total, 100*ident/total
  printf "  verdict histogram:\n"
  for (k in cnt) printf "    %-12s %d\n", k, cnt[k]
  if (undec>0) printf "  *** UNDECODABLE port streams: %d (BAD — investigate) ***\n", undec
  if (errs>0)  printf "  *** harness errors (rs-err/c-err): %d ***\n", errs

  printf "\nBY CONTENT CLASS (screen vs photo):\n"
  for (k in clstot) printf "  %-8s %d/%d (%.1f%%)\n", k, clsid[k], clstot[k], 100*clsid[k]/clstot[k]

  printf "\nBY CORPUS:\n"
  for (k in ctot) printf "  %-8s %d/%d (%.1f%%)\n", k, cid[k], ctot[k], 100*cid[k]/ctot[k]

  printf "\nBY PRESET:\n"
  n=asorti(ptot, pk, "@ind_num_asc")
  for (i=1;i<=n;i++){k=pk[i]; printf "  p%-3s %d/%d (%.1f%%)\n", k, pid[k], ptot[k], 100*pid[k]/ptot[k]}

  printf "\nBY QP:\n"
  n=asorti(qtot, qk, "@ind_num_asc")
  for (i=1;i<=n;i++){k=qk[i]; printf "  q%-3s %d/%d (%.1f%%)\n", k, qid[k], qtot[k], 100*qid[k]/qtot[k]}

  printf "\nBY BIT-DEPTH:\n"
  for (k in btot) printf "  bd%-3s %d/%d (%.1f%%)\n", k, bid[k], btot[k], 100*bid[k]/btot[k]
}
' "$TSV"

echo ""
echo "DIVERGENCE MAP — identity rate per (class x preset x bd) [id/total]:"
awk -F'\t' '
NR==1 { next }
{
  cls = ($1=="screen") ? "screen" : "photo"
  key = cls SUBSEP $6 SUBSEP $5
  tot[key]++; if ($8=="IDENTICAL") id[key]++
  seenp[$6]=1; seenb[$5]=1
}
END {
  np=asorti(seenp, pk, "@ind_num_asc"); nb=asorti(seenb, bk, "@ind_num_asc")
  printf "  %-8s", "class"
  for (i=1;i<=np;i++) for (j=1;j<=nb;j++) printf " p%s/bd%s", pk[i], bk[j]
  printf "\n"
  split("photo screen", classes, " ")
  for (ci=1; ci<=2; ci++){
    cl=classes[ci]; printf "  %-8s", cl
    for (i=1;i<=np;i++) for (j=1;j<=nb;j++){
      k=cl SUBSEP pk[i] SUBSEP bk[j]
      if (tot[k]>0) printf " %d/%d", id[k], tot[k]; else printf "   -"
    }
    printf "\n"
  }
}
' "$TSV"

echo ""
echo "DIVERGING CELLS (verdict=DIFFERS), grouped by corpus/preset/bd:"
awk -F'\t' 'NR>1 && $8=="DIFFERS" {printf "  %-8s p%-3s bd%-3s q%-3s %-28s first_diff=%s  (C=%s rs=%s)\n", $1,$6,$5,$7,$2,$12,$10,$11}' "$TSV" \
  | sort | head -200

ndiff=$(awk -F'\t' 'NR>1 && $8=="DIFFERS"' "$TSV" | wc -l)
echo ""
echo "total DIFFERS cells: $ndiff"

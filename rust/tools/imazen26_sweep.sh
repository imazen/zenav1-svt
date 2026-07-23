#!/usr/bin/env bash
# imazen26 K300 production-corpus byte-identity DISCOVERY sweep (coverage task).
#
# Purpose: byte-parity map of the port vs the C reference over the K300
# diversity-sampled representative subset (273 PNGs) of the imazen-26
# production corpus (2,160 images). K300 spans content classes NO existing
# gate corpus has — bilevel patent scans, government document pages (text +
# charts), synthetic plots (hard-edge), AI clipart/illustration/product
# renders, manuscript scans — alongside photos, art, and screenshots.
# Divergences are the valuable signal (new near-tie / bug targets), NOT test
# failures — this tool always exits 0 (it is a measurement, not a gate).
#
# Corpus: IM26_DIR (default /root/work/imazen26-cache/K300), with the
# content_class of each image joined from the manifest IM26_MANIFEST
# (default /root/work/imazen26-cache/K300.tsv; columns
# url/crop_label/content_class/cluster_id/cluster_size — join on basename).
# Every image is CENTER-CROPPED to 512x512 via identity_run's `crop:`
# (images smaller than 512 in an axis are clamped + edge-replicated).
#
# Each cell: port encode (identity_run, SVTAV1_BD) -> C encode
# (capture_c_trace, matched bd) -> aomdec DECODABILITY of the port stream
# (a port stream aomdec rejects = self-desync = zero-tolerance find) ->
# `cmp` byte-identity. Scoreboard rows are appended atomically from parallel
# workers (<4KB row => O_APPEND atomic on Linux). The C driver is --lp 1 and
# the port is single-threaded, so IM26_PAR cells run concurrently under
# nice/ionice.
#
# Job ordering: presets cheap-first (13, 10, 6, 0) and images round-robin
# across content classes, so an interrupted run still yields a class-complete
# map for every finished preset tier and a class-balanced partial tier.
#
# Env (all overridable): IM26_PRESETS "0 6 10 13"  IM26_QPS "5 20 32 48 63"
#   IM26_BDS "8 10"  IM26_PAR 8  IM26_CELL_TIMEOUT 300
#   IM26_OUT benchmarks/imazen26_sweep_<date>.tsv
#   IM26_N <per-class image cap, default 0 = all>
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CTRACE_BIN="$HERE/capture_c_trace/capture_c_trace.bin"
AOMDEC="${AOMDEC:-}"
if [ -z "$AOMDEC" ]; then
  for c in aomdec /root/aomdec-debug/aomdec; do command -v "$c" >/dev/null 2>&1 && { AOMDEC=$c; break; }; done
fi

IM26_DIR="${IM26_DIR:-/root/work/imazen26-cache/K300}"
IM26_MANIFEST="${IM26_MANIFEST:-/root/work/imazen26-cache/K300.tsv}"

# ---- single-cell mode (re-invoked by the orchestrator) ----------------------
# argv after --cell: class stem path W H bd preset qp outdir out_tsv
if [[ "${1:-}" == "--cell" ]]; then
  shift
  class=$1 stem=$2 path=$3 W=$4 H=$5 bd=$6 preset=$7 qp=$8 d=$9 OUT=${10}
  mkdir -p "$d"
  t0=$(date +%s.%N)
  verdict="rs-err"; decodable="-"; cb="-"; rb="-"; fdiff="-"
  if SVTAV1_BD="$bd" timeout "${IM26_CELL_TIMEOUT:-300}" "$RUN_BIN" \
        "crop:$path" "$W" "$H" "$qp" "$preset" "$d/rs" >/dev/null 2>"$d/rs.err"; then
    rb=$(stat -c%s "$d/rs.obu" 2>/dev/null || echo -)
    if timeout "${IM26_CELL_TIMEOUT:-300}" "$CTRACE_BIN" \
          "$W" "$H" "$qp" "$preset" "$d/rs.yuv" "$d/c.obu" "$bd" >/dev/null 2>&1; then
      cb=$(stat -c%s "$d/c.obu" 2>/dev/null || echo -)
      if [ -n "$AOMDEC" ] && "$AOMDEC" --rawvideo -o /dev/null "$d/rs.obu" >/dev/null 2>&1; then
        decodable="yes"; else decodable="no"; fi
      if cmp -s "$d/rs.obu" "$d/c.obu"; then
        verdict="IDENTICAL"
      else
        verdict="DIFFERS"
        fdiff=$(cmp "$d/rs.obu" "$d/c.obu" 2>/dev/null | awk '{print $NF}')
      fi
    else
      verdict="c-err"
    fi
  else
    rc=$?
    # Distinguish a timeout (HANG) and a panic (the zero-tolerance find)
    # from a plain nonzero exit.
    if [ "$rc" = 124 ]; then verdict="rs-hang"
    elif grep -q "panicked at" "$d/rs.err" 2>/dev/null; then verdict="rs-panic"
    fi
  fi
  [ -s "$d/rs.err" ] || rm -f "$d/rs.err"
  dt=$(printf '%.1f' "$(echo "$(date +%s.%N) - $t0" | bc)")
  rm -f "$d/rs.yuv"   # 393KB(bd8)/786KB(bd10) per cell — drop to bound disk
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$class" "$stem" "$W" "$H" "$bd" "$preset" "$qp" "$verdict" "$decodable" "$cb" "$rb" "$fdiff" >>"$OUT"
  echo "[$class $stem bd$bd p$preset q$qp] $verdict (${dt}s)" >&2
  exit 0
fi

# ---- orchestrator -----------------------------------------------------------
DATE=$(date +%Y-%m-%d)
OUT="${IM26_OUT:-$RS_ROOT/benchmarks/imazen26_sweep_${DATE}.tsv}"
read -r -a PRESETS <<<"${IM26_PRESETS:-13 10 6 0}"
read -r -a QPS <<<"${IM26_QPS:-5 20 32 48 63}"
read -r -a BDS <<<"${IM26_BDS:-8 10}"
PAR="${IM26_PAR:-8}"
NCAP="${IM26_N:-0}"
export IM26_CELL_TIMEOUT="${IM26_CELL_TIMEOUT:-300}"

[ -x "$AOMDEC" ] || { echo "imazen26_sweep: aomdec not found (set AOMDEC=)" >&2; exit 2; }
[ -d "$IM26_DIR" ] || { echo "imazen26_sweep: corpus dir $IM26_DIR missing" >&2; exit 2; }
[ -f "$IM26_MANIFEST" ] || { echo "imazen26_sweep: manifest $IM26_MANIFEST missing" >&2; exit 2; }

echo "priming builds (freshness check)..." >&2
(cd "$RS_ROOT" && CARGO_BUILD_JOBS=8 nice -n 19 ionice -c3 \
   cargo build --release -p zenav1-svt --features symtrace --example identity_run) >&2 \
   || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CTRACE_BIN" ] || { echo "raw binaries missing after prime" >&2; exit 2; }

# basename -> content_class join from the manifest.
declare -A CLASS
while IFS=$'\t' read -r url _crop cls _cid _csz; do
  [ "$url" = "url" ] && continue
  CLASS["$(basename "$url")"]="$cls"
done <"$IM26_MANIFEST"

# One "class|stem|path" entry per PNG, round-robin across classes so a
# truncated run keeps every class represented. Optional per-class cap IM26_N.
declare -A BYCLASS_LIST
declare -A BYCLASS_COUNT
while read -r p; do
  b=$(basename "$p")
  cls="${CLASS[$b]:-unmapped}"
  n="${BYCLASS_COUNT[$cls]:-0}"
  if [ "$NCAP" -gt 0 ] && [ "$n" -ge "$NCAP" ]; then continue; fi
  BYCLASS_COUNT[$cls]=$((n + 1))
  BYCLASS_LIST[$cls]+="${p}"$'\n'
done < <(find "$IM26_DIR" -name '*.png' | sort)

JOBS=()
maxn=0
for cls in "${!BYCLASS_COUNT[@]}"; do
  [ "${BYCLASS_COUNT[$cls]}" -gt "$maxn" ] && maxn="${BYCLASS_COUNT[$cls]}"
done
mapfile -t CLS_SORTED < <(printf '%s\n' "${!BYCLASS_COUNT[@]}" | sort)
for ((k = 0; k < maxn; k++)); do
  for cls in "${CLS_SORTED[@]}"; do
    p=$(sed -n "$((k + 1))p" <<<"${BYCLASS_LIST[$cls]}")
    [ -n "$p" ] || continue
    stem=$(basename "$p" .png)
    # Keep stems short: drop the trailing _WxH.sdr suffix noise for dirs.
    JOBS+=("$cls|$stem|$p")
  done
done

ncells=$(( ${#JOBS[@]} * ${#PRESETS[@]} * ${#QPS[@]} * ${#BDS[@]} ))
echo "imazen26 sweep: ${#JOBS[@]} images (${#CLS_SORTED[@]} classes) x ${#PRESETS[@]}p x ${#QPS[@]}q x ${#BDS[@]}bd = $ncells cells" >&2
echo "presets: ${PRESETS[*]}  qps: ${QPS[*]}  bds: ${BDS[*]}  par: $PAR" >&2
echo "scoreboard (incremental): $OUT" >&2

mkdir -p "$(dirname "$OUT")"
printf 'class\timage\twidth\theight\tbd\tpreset\tqp\tverdict\tdecodable\tc_bytes\trust_bytes\tfirst_diff\n' >"$OUT"

# Jobfile: preset-major cheap-first (13/10/6/0), then class-round-robin
# images, then bd, then qp — so early wall-time yields complete cheap tiers.
selfpath="$HERE/$(basename "$0")"
JOBFILE=$(mktemp)
for preset in "${PRESETS[@]}"; do
  for job in "${JOBS[@]}"; do
    IFS='|' read -r cls stem path <<<"$job"
    for bd in "${BDS[@]}"; do for qp in "${QPS[@]}"; do
      d="$RS_ROOT/target/imazen26/${stem}_bd${bd}_p${preset}_q${qp}"
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$cls" "$stem" "$path" "$bd" "$preset" "$qp" "$d"
    done; done
  done
done >"$JOBFILE"

start=$(date +%s)
while IFS=$'\t' read -r cls stem path bd preset qp d; do
  nice -n 19 ionice -c3 "$selfpath" --cell \
    "$cls" "$stem" "$path" 512 512 "$bd" "$preset" "$qp" "$d" "$OUT" &
  while (( $(jobs -rp | wc -l) >= PAR )); do wait -n; done
done <"$JOBFILE"
wait
rm -f "$JOBFILE"

elapsed=$(( $(date +%s) - start ))
echo "" >&2
echo "sweep complete in ${elapsed}s" >&2

# ---- summary: match rate by class / preset / qp / bd ------------------------
awk -F'\t' '
NR>1 {
  tot[$1]++; if ($8=="IDENTICAL") id[$1]++
  ptot[$6]++; if ($8=="IDENTICAL") pid[$6]++
  vt[$8]++
  key=$1"|p"$6; ctot[key]++; if ($8=="IDENTICAL") cid[key]++
}
END {
  print "== verdicts =="
  for (v in vt) printf "  %-10s %d\n", v, vt[v]
  print "== by preset =="
  for (p in ptot) printf "  p%-3s %d/%d\n", p, pid[p]+0, ptot[p]
  print "== by class =="
  n=asorti(tot, s); for (i=1;i<=n;i++) { c=s[i]; printf "  %-42s %d/%d\n", c, id[c]+0, tot[c] }
  print "== class x preset (non-clean only) =="
  m=asorti(ctot, t); for (i=1;i<=m;i++) { k=t[i]; if (cid[k]+0 != ctot[k]) printf "  %-48s %d/%d\n", k, cid[k]+0, ctot[k] }
}' "$OUT" >&2

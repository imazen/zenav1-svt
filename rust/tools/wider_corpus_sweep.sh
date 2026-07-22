#!/usr/bin/env bash
# Wider-corpus byte-identity DISCOVERY sweep (coverage task).
#
# Purpose: surface real-content divergences the existing gates miss. The gates
# feed synthetic content + a CID22 subset (and default real-image presets
# 2/6/10). This sweep broadens to THREE real corpora — crucially including
# SCREEN content, a class no current gate has — across a preset x qp x
# bit-depth grid, and produces a DIVERGENCE MAP: which (corpus/content-class,
# preset, qp, bd) cells diverge from the C reference vs the byte-identical
# majority. Divergences are the valuable signal (new near-tie / bug targets),
# NOT test failures — this tool always exits 0 (it is a measurement, not a gate).
#
# Corpora (all on-box, verified present):
#   cid22  = CID22-512/training   (512x512 photo, encoded via file:, no crop)
#   clic   = clic2025             (~2.7MP photo, CENTER-CROPPED to 512 via crop:)
#   screen = gb82-sc              (screen/UI/text, CENTER-CROPPED to 512 via crop:)
# Large clic/screen images are center-cropped to 512x512 so preset-0 (the
# primary, SB128-triggering config — 512x512 aligned area 262144 >= 165120 AND
# preset<=1 => SB128) stays tractable while still exercising each corpus's real
# content statistics.
#
# Each cell: port encode (identity_run, SVTAV1_BD) -> C encode (capture_c_trace,
# matched bd) -> aomdec DECODABILITY of the port stream -> `cmp` byte-identity.
# Scoreboard rows are appended atomically from parallel workers (<4KB row =>
# O_APPEND atomic on Linux). The C driver is --lp 1 and the port is
# single-threaded, so WCS_PAR cells run concurrently under nice/ionice.
#
# Env (all overridable): WCS_PRESETS "0 6 10 13"  WCS_QPS "5 20 32 48 63"
#   WCS_BDS "8 10"  WCS_CID22_N 15  WCS_CLIC_N 12  (screen: all, always)
#   WCS_PAR 8  WCS_CELL_TIMEOUT 300  WCS_OUT /tmp/wider_corpus_<date>.tsv
#   WCS_CORPORA "cid22 clic screen"
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CTRACE_BIN="$HERE/capture_c_trace/capture_c_trace.bin"
AOMDEC="${AOMDEC:-}"
if [ -z "$AOMDEC" ]; then
  for c in aomdec /root/aomdec-debug/aomdec; do command -v "$c" >/dev/null 2>&1 && { AOMDEC=$c; break; }; done
fi

# Corpus dirs (env-overridable, e.g. WCS_CID22_DIR=/mnt/v/collections/imazen26 to
# swap in the production corpus when /mnt/v is mounted).
CID22_DIR="${WCS_CID22_DIR:-/root/work/codec-corpus/CID22/CID22-512/training}"
CLIC_DIR="${WCS_CLIC_DIR:-/root/work/codec-corpus/clic2025}"
SCREEN_DIR="${WCS_SCREEN_DIR:-/root/work/codec-corpus/gb82-sc}"

# ---- single-cell mode (re-invoked by xargs) ---------------------------------
# argv after script name: --cell corpus content stem W H bd preset qp outdir out_tsv
if [[ "${1:-}" == "--cell" ]]; then
  shift
  corpus=$1 content=$2 stem=$3 W=$4 H=$5 bd=$6 preset=$7 qp=$8 d=$9 OUT=${10}
  mkdir -p "$d"
  t0=$(date +%s.%N)
  verdict="rs-err"; decodable="-"; cb="-"; rb="-"; fdiff="-"
  if SVTAV1_BD="$bd" timeout "${WCS_CELL_TIMEOUT:-300}" "$RUN_BIN" \
        "$content" "$W" "$H" "$qp" "$preset" "$d/rs" >/dev/null 2>/dev/null; then
    rb=$(stat -c%s "$d/rs.obu" 2>/dev/null || echo -)
    if timeout "${WCS_CELL_TIMEOUT:-300}" "$CTRACE_BIN" \
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
  fi
  dt=$(printf '%.1f' "$(echo "$(date +%s.%N) - $t0" | bc)")
  rm -f "$d/rs.yuv"   # 393KB/cell — drop to avoid filling disk over ~1500 cells
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$corpus" "$stem" "$W" "$H" "$bd" "$preset" "$qp" "$verdict" "$decodable" "$cb" "$rb" "$fdiff" >>"$OUT"
  echo "[$corpus $stem bd$bd p$preset q$qp] $verdict (${dt}s)" >&2
  exit 0
fi

# ---- orchestrator -----------------------------------------------------------
DATE=$(date +%Y-%m-%d)
OUT="${WCS_OUT:-/tmp/wider_corpus_${DATE}.tsv}"
read -r -a PRESETS <<<"${WCS_PRESETS:-0 6 10 13}"
read -r -a QPS <<<"${WCS_QPS:-5 20 32 48 63}"
read -r -a BDS <<<"${WCS_BDS:-8 10}"
CID22_N="${WCS_CID22_N:-15}"
CLIC_N="${WCS_CLIC_N:-12}"
PAR="${WCS_PAR:-8}"
read -r -a CORPORA <<<"${WCS_CORPORA:-cid22 clic screen}"
export WCS_CELL_TIMEOUT="${WCS_CELL_TIMEOUT:-300}"

[ -x "$AOMDEC" ] || { echo "wider_corpus_sweep: aomdec not found (set AOMDEC=)" >&2; exit 2; }

echo "priming builds (freshness check)..." >&2
(cd "$RS_ROOT" && CARGO_BUILD_JOBS=8 nice -n 19 ionice -c3 \
   cargo build --release -p zenav1-svt --features symtrace --example identity_run) >&2 \
   || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CTRACE_BIN" ] || { echo "raw binaries missing after prime" >&2; exit 2; }

# Deterministic evenly-spaced sample of N paths from a dir tree (recursive).
sample_paths() {
  local dir=$1 n=$2
  mapfile -t all < <(find "$dir" -name '*.png' | sort)
  local tot=${#all[@]}
  [ "$tot" -eq 0 ] && return
  [ "$n" -gt "$tot" ] && n=$tot
  local stride=$((tot / n)); [ "$stride" -lt 1 ] && stride=1
  local k
  for ((k=0; k<n; k++)); do echo "${all[$((k*stride))]}"; done
}

# Build "corpus|content_spec|stem" job entries (one per image).
JOBS=()
for c in "${CORPORA[@]}"; do
  case $c in
    cid22)  while read -r p; do JOBS+=("cid22|file:$p|$(basename "$p" .png)"); done < <(sample_paths "$CID22_DIR" "$CID22_N") ;;
    clic)   while read -r p; do JOBS+=("clic|crop:$p|$(basename "$p" .png)"); done < <(sample_paths "$CLIC_DIR" "$CLIC_N") ;;
    screen) while read -r p; do JOBS+=("screen|crop:$p|$(basename "$p" .png)"); done < <(find "$SCREEN_DIR" -name '*.png' | sort) ;;
  esac
done

ncells=$(( ${#JOBS[@]} * ${#PRESETS[@]} * ${#QPS[@]} * ${#BDS[@]} ))
echo "wider-corpus sweep: ${#JOBS[@]} images x ${#PRESETS[@]}p x ${#QPS[@]}q x ${#BDS[@]}bd = $ncells cells" >&2
echo "corpora: ${CORPORA[*]}  presets: ${PRESETS[*]}  qps: ${QPS[*]}  bds: ${BDS[*]}  par: $PAR" >&2
echo "scoreboard (incremental): $OUT" >&2

printf 'corpus\timage\twidth\theight\tbd\tpreset\tqp\tverdict\tdecodable\tc_bytes\trust_bytes\tfirst_diff\n' >"$OUT"

# Write a tab-separated jobfile (one cell/line) and dispatch with a bounded
# wait -n semaphore — robust against any path quoting (avoids xargs argv games).
selfpath="$HERE/$(basename "$0")"
JOBFILE=$(mktemp)
for job in "${JOBS[@]}"; do
  IFS='|' read -r corpus content stem <<<"$job"
  for bd in "${BDS[@]}"; do for preset in "${PRESETS[@]}"; do for qp in "${QPS[@]}"; do
    d="$RS_ROOT/target/wider_corpus/${corpus}_${stem}_bd${bd}_p${preset}_q${qp}"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$corpus" "$content" "$stem" "$bd" "$preset" "$qp" "$d"
  done; done; done
done >"$JOBFILE"

start=$(date +%s)
while IFS=$'\t' read -r corpus content stem bd preset qp d; do
  nice -n 19 ionice -c3 "$selfpath" --cell \
    "$corpus" "$content" "$stem" 512 512 "$bd" "$preset" "$qp" "$d" "$OUT" &
  while (( $(jobs -rp | wc -l) >= PAR )); do wait -n; done
done <"$JOBFILE"
wait
rm -f "$JOBFILE"

elapsed=$(( $(date +%s) - start ))
echo "" >&2
echo "sweep complete in ${elapsed}s" >&2

# ---- summary + divergence map -----------------------------------------------
"$HERE/wider_corpus_summary.sh" "$OUT" || true

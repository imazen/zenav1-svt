#!/usr/bin/env bash
# WHICH motion mode does C actually CODE, per cell, per preset?
#
# `inter_cinter_census.sh` answers this on the SYNTHETIC grid and lumps every
# non-translation mode into one `motmode` column. This one runs on REAL VIDEO
# and splits OBMC (mm=1) from WARPED_CAUSAL (mm=2), because they are different
# features with different owners and the lumped column cannot tell you which
# one to port.
#
# WHY IT EXISTS. `benchmarks/c_motion_mode_census_2026-09-10.meta` measured
# presets 6 and 8 and reported "OBMC is never selected at these presets". True,
# and it was then read as "OBMC is never selected" — the same shape of
# over-read that meta itself was written to retract. Presets <= 4 became
# reachable when global motion landed (2026-09-10), and there OBMC is 22 % of
# every coded inter block. A census that never ran at the preset that matters
# is indistinguishable from a feature that is never chosen.
#
# ANTI-VACUITY IS THE WHOLE POINT, and this script exists in this form because
# the ad-hoc version of it produced all-zeros TWICE and both times looked like
# a clean measurement:
#
#   1. `tools/capture_c_trace` is a DIRECTORY; the driver is one level down.
#      `nice tools/capture_c_trace ...` fails with EACCES, the dump is never
#      written, and every count reads 0.
#   2. Without `SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0` C encodes a DIFFERENT
#      prediction structure than the port's flat low-delay P, so the counts are
#      of a configuration nobody ships.
#
# So this script (a) fails loudly if a dump is empty, and (b) VALIDATES itself
# against the pinned preset-6 warped counts before reporting anything. A
# harness that cannot reproduce a known measurement does not get to publish a
# new one.
#
# Usage: tools/motion_mode_census.sh [outdir]
# Env:   MMC_PRESETS (default "-1 0 1 2 4 6"), MMC_QP (40), MMC_FRAMES (2)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

DRV="$HERE/capture_c_trace/capture_c_trace"
[[ -x "$DRV" ]] || { echo "FATAL: no C trace driver at $DRV" >&2; exit 2; }

ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
if [[ ! -f "$ASSETS/vidyo3_256x256_8f.i420" ]]; then
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "FATAL: could not obtain the derf assets" >&2; exit 2; }
fi

PRESETS="${MMC_PRESETS:--1 0 1 2 4 6}"
QP="${MMC_QP:-40}"
FRAMES="${MMC_FRAMES:-2}"
OUT="${1:-$RS_ROOT/target/motion-mode-census}"
mkdir -p "$OUT"

# One cell. Echoes "<blocks> <simple> <obmc> <warped>", or dies.
cell() {
    local clip=$1 size=$2 preset=$3
    local a="$ASSETS/${clip}_${size}_8f.i420"
    [[ -f "$a" ]] || { echo "MISSING $a" >&2; return 1; }
    local w=${size%x*} h=${size#*x}
    local d="$OUT/${clip}_${size}_p${preset}"
    mkdir -p "$d"
    rm -f "$d/cinter.txt"
    # The GOP the port ships: flat low-delay P, no hierarchy, no intra period.
    # Omitting these measures a structure this port does not encode.
    SVT_FRAMES="$FRAMES" SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 \
        SVT_CINTER_OUT="$d/cinter.txt" \
        nice -n19 ionice -c3 "$DRV" "$w" "$h" "$QP" "$preset" "$a" "$d/c.obu" \
        >"$d/log" 2>&1
    if [[ ! -s "$d/cinter.txt" ]]; then
        echo "FATAL: empty SVT_CINTER_OUT for $clip $size p$preset — the C driver" >&2
        echo "  never dumped. This is a HARNESS failure, not 'C codes nothing'." >&2
        sed -n 1,5p "$d/log" >&2
        return 1
    fi
    local n s o wp
    n=$(grep -c " mm=" "$d/cinter.txt"); s=$(grep -c " mm=0 " "$d/cinter.txt")
    o=$(grep -c " mm=1 " "$d/cinter.txt"); wp=$(grep -c " mm=2 " "$d/cinter.txt")
    echo "${n:-0} ${s:-0} ${o:-0} ${wp:-0}"
}

# ---- harness validation, BEFORE any new number is reported ----------------
# The pinned preset-6 warped counts from
# `benchmarks/c_motion_mode_census_2026-09-10.meta`. That census fed C the
# port's own `rs.yuv`; this one feeds the original `.i420`, so the counts are
# close but not equal — the check is a BAND, and its job is to catch a harness
# that reports zero, not to re-pin the numbers.
echo "== harness validation against the pinned 2026-09-10 preset-6 census =="
bad=0
for spec in "johnny 5" "vidyo1 18" "vidyo3 56"; do
    set -- $spec
    read -r n s o wp <<<"$(cell "$1" 256x256 6)" || { bad=1; continue; }
    lo=$(( $2 * 8 / 10 )); hi=$(( $2 * 13 / 10 + 2 ))
    if [[ "$wp" -lt "$lo" || "$wp" -gt "$hi" ]]; then
        printf '  FAIL %-8s p6 warped=%s, expected %s..%s (pinned %s)\n' "$1" "$wp" "$lo" "$hi" "$2"
        bad=1
    else
        printf '  ok   %-8s p6 warped=%s (pinned %s)\n' "$1" "$wp" "$2"
    fi
done
if [[ "$bad" -ne 0 ]]; then
    echo "HARNESS FAILED VALIDATION — refusing to report a census it cannot trust." >&2
    exit 1
fi

echo
printf 'clip\tsize\tpreset\tblocks\tsimple\tobmc\twarped\n'
declare -A TB TS TO TW
for clip in fourpeople johnny kristenandsara vidyo1 vidyo3 vidyo4; do
  for size in 128x128 256x256; do
    for p in $PRESETS; do
      read -r n s o wp <<<"$(cell "$clip" "$size" "$p")" || exit 1
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$clip" "$size" "$p" "$n" "$s" "$o" "$wp"
      TB[$p]=$(( ${TB[$p]:-0} + n )); TS[$p]=$(( ${TS[$p]:-0} + s ))
      TO[$p]=$(( ${TO[$p]:-0} + o )); TW[$p]=$(( ${TW[$p]:-0} + wp ))
    done
  done
done

echo
echo "== totals per preset =="
for p in $PRESETS; do
    b=${TB[$p]:-0}
    [[ "$b" -gt 0 ]] || { echo "ANTI-VACUITY FAIL: preset $p parsed ZERO blocks" >&2; exit 1; }
    printf 'preset %-3s blocks=%-6s simple=%-6s OBMC=%-5s (%5.1f%%)  warped=%-5s (%5.1f%%)\n' \
        "$p" "$b" "${TS[$p]}" "${TO[$p]}" \
        "$(awk "BEGIN{printf \"%.1f\", 100*${TO[$p]}/$b}")" \
        "${TW[$p]}" \
        "$(awk "BEGIN{printf \"%.1f\", 100*${TW[$p]}/$b}")"
done
echo "motion mode census: OK"

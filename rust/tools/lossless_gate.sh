#!/usr/bin/env bash
# Coded-lossless (QP 0 / base_q_idx 0) gate — issue #5 chunk 2.
#
# Two oracles per cell, both hard:
#   (1) BYTE-IDENTITY to the C encoder at qp 0 (SvtAv1Enc via
#       capture_c_trace, still/AVIF CQP) — the same contract every other
#       byte gate in this directory asserts;
#   (2) LOSSLESSNESS: the port's stream, decoded by the reference decoder
#       (`aomdec --rawvideo`), must equal the SOURCE planes byte-for-byte.
#       This is what the frame header promises a decoder (CodedLossless,
#       spec 5.9.2), and it is the property the pre-chunk-2 encoder violated
#       (valid syntax, wrong pixels: ssim2 -200..-1100). Byte-identity alone
#       cannot see that class if the oracle itself were wrong — the C qp-0
#       capture was checked to decode losslessly before adoption
#       (tests/lossless_fh_c_capture.rs), and this gate re-checks every cell.
#
# Anti-vacuity: the port's qp-0 stream must DIFFER from its qp-1 stream on the
# same content (otherwise the cell would pass with the lossless arms removed
# — it is the lossless PATH under test, not qp 1's), except for `uniform`,
# which codes zero residual at both qps and is kept only as the all-skip
# control (its lossless check still bites).
#
# Cells: 5 synthetic contents x 4 geometries (64-aligned and PARTIAL-SB,
# 8-aligned) x the preset ladder 0..13. Presets 10..13 are M9 on the C side
# (all-intra clamp) but distinct port configurations. Env overrides:
#   LL_CONTENTS, LL_DIMS ("WxH ..."), LL_PRESETS, LL_JOBS (4), AOMDEC.
#
# Every cell must match C and decode exactly to the source. The former
# 32 low-preset pins were closed by the lossless PD0/PD1 and MD CDF wiring
# corrections (2026-09-07); no expected byte differences remain.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
. "$HERE/lib_nice.sh" 2>/dev/null || true

read -r -a CONTENTS <<<"${LL_CONTENTS:-gradient diag uniform screen screenrep}"
read -r -a DIMS <<<"${LL_DIMS:-64x64 128x128 96x80 200x136}"
read -r -a PRESETS <<<"${LL_PRESETS:-0 1 2 3 4 5 6 7 8 9 10 13}"

aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1 && [ ! -x "$aomdec" ]; then
  echo "error: aomdec not found (set AOMDEC=...) — the lossless-decode oracle is REQUIRED here" >&2
  exit 2
fi

# The cells are a list for tools/cellrun.py (plan T3): each qp-0 cell checks C
# byte identity and lossless decode, and names its qp-1 sibling (port only)
# as the stream it must differ from.
OUT="${TMPDIR:-$HOME/tmp}/lossless.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
CELLS="$OUT/lossless.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tcheck\tdiffers_from\n' >"$CELLS"
for content in "${CONTENTS[@]}"; do
  for dim in "${DIMS[@]}"; do
    w=${dim%x*}
    h=${dim#*x}
    for p in "${PRESETS[@]}"; do
      cell="${content}_${w}x${h}_q0_p${p}"
      if [ "$content" = "uniform" ]; then
        printf '%s\t%s\t%s\t%s\t0\t%s\tc,lossless\t\n' "$cell" "$content" "$w" "$h" "$p" >>"$CELLS"
      else
        printf '%s\t%s\t%s\t%s\t0\t%s\tc,lossless\t%s\n' "$cell" "$content" "$w" "$h" "$p" "${cell}_q1" >>"$CELLS"
        printf '%s_q1\t%s\t%s\t%s\t1\t%s\tnone\t\n' "$cell" "$content" "$w" "$h" "$p" >>"$CELLS"
      fi
    done
  done
done

AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$CELLS" --out "$OUT/result.tsv" \
  --bytes-only --jobs "${LL_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" <<'PY'
import csv, sys
rows = [r for r in csv.DictReader(open(sys.argv[1]), delimiter="\t") if not r["name"].endswith("_q1")]
passed = [r for r in rows if r["verdict"] == "IDENTICAL" and "FAIL" not in r["checks"]]
print(f"coded-lossless identity + lossless-decode: {len(passed)} / {len(rows)} byte-identical, all lossless")
for r in rows:
    if r in passed:
        continue
    why = [c for c in r["checks"].split() if c.endswith("=FAIL")]
    if r["verdict"] != "IDENTICAL":
        why.insert(0, f"{r['verdict']}: {r['detail']}")
    print(f"  FAILED: {r['name']}[{'; '.join(why)}]")
    if any(c.startswith("differs_from") for c in why):
        print("  GATE PREMISE FAILED — qp-0 stream == qp-1 stream (the lossless path was not exercised)")
PY
exit "$rc"

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
#   LL_CONTENTS, LL_DIMS ("WxH ..."), LL_PRESETS, AOMDEC.
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

OUT="${TMPDIR:-/tmp}/lossless.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
vacuous=()

for content in "${CONTENTS[@]}"; do
  for dim in "${DIMS[@]}"; do
    w=${dim%x*}
    h=${dim#*x}
    for p in "${PRESETS[@]}"; do
      cell="${content}_${w}x${h}_q0_p${p}"
      # (a) port at qp 0 -> rs.obu + rs.yuv (the SOURCE planes, I420)
      if ! "$HERE/identity_run" "$content" "$w" "$h" 0 "$p" "$OUT/rs" >"$OUT/rs.log" 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
      fi
      # (b) C at qp 0 on the identical planes
      if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
           "$w" "$h" 0 "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
      fi
      # (c) anti-vacuity: the port's qp-1 stream on the same content
      if [ "$content" != "uniform" ]; then
        if ! "$HERE/identity_run" "$content" "$w" "$h" 1 "$p" "$OUT/rs1" >"$OUT/rs1.log" 2>&1; then
          fail=$((fail + 1)); failed+=("$cell[rs1-err]"); continue
        fi
        if cmp -s "$OUT/rs.obu" "$OUT/rs1.obu"; then
          vacuous+=("$cell")
        fi
      fi
      # (2) losslessness under the reference decoder — required for every cell.
      rm -f "$OUT/dec.yuv"
      if ! "$aomdec" --rawvideo -o "$OUT/dec.yuv" "$OUT/rs.obu" >"$OUT/dec.log" 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[aomdec-rejects]"); continue
      fi
      if ! cmp -s "$OUT/dec.yuv" "$OUT/rs.yuv"; then
        fail=$((fail + 1)); failed+=("$cell[NOT-LOSSLESS: decoded != source]"); continue
      fi
      # (1) byte-identity, with no exceptions.
      if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        pass=$((pass + 1))
      else
        fail=$((fail + 1)); failed+=("$cell[bytes: port $(wc -c <"$OUT/rs.obu" | tr -d ' ') B vs C $(wc -c <"$OUT/c.obu" | tr -d ' ') B]")
      fi
    done
  done
done

total=$((pass + fail))
echo "coded-lossless identity + lossless-decode: $pass / $total byte-identical, all lossless"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[@]}"
fi
if [ "${#vacuous[@]}" -gt 0 ]; then
  echo "  GATE PREMISE FAILED — qp-0 stream == qp-1 stream on textured content (the lossless"
  echo "  path was not exercised): ${vacuous[*]}"
fi
rm -rf "$OUT"
[ "$fail" -eq 0 ] && [ "${#vacuous[@]}" -eq 0 ]

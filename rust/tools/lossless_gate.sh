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
# Cells: 3 synthetic contents x 4 geometries (64-aligned and PARTIAL-SB,
# 8-aligned) x the preset ladder 0..13. Presets 10..13 are M9 on the C side
# (all-intra clamp) but distinct port configurations. Env overrides:
#   LL_CONTENTS, LL_DIMS ("WxH ..."), LL_PRESETS, AOMDEC.
#
# PINNED cells (self-promoting, same contract as screen_ibc_gate.sh): textured
# content at presets 0..3 is LOSSLESS in both encoders (oracle 2 holds and is
# still REQUIRED there) but not byte-identical — an RD-decision residual, not a
# pixel defect (MEASURED 2026-08-28: gradient 64x64 p3 port 2966 B vs C 2973 B,
# both decode to the source; see docs/REFUSED-CONFIGS.md's neighbour, the
# CHANGELOG entry, and rust/CLAUDE.md for the root: 4x4 partitions are
# ALLOWED at M0..M3 in all-intra (`svt_aom_get_disallow_4x4_allintra`,
# enc_mode_config.c:8181 — exactly the failing set), so C's lossless
# partition search decides 8x8-vs-four-4x4 per block while the port forces
# 8x8 leaves). A pinned cell that starts byte-matching FAILS the gate so the
# improvement gets promoted, never silently absorbed.
#
# Exit 0 iff every non-pinned cell passes (1) and (2), every pinned cell
# passes (2) and still differs, and no anti-vacuity premise fails.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
. "$HERE/lib_nice.sh" 2>/dev/null || true

read -r -a CONTENTS <<<"${LL_CONTENTS:-gradient diag uniform}"
read -r -a DIMS <<<"${LL_DIMS:-64x64 128x128 96x80 200x136}"
read -r -a PRESETS <<<"${LL_PRESETS:-0 1 2 3 4 5 6 7 8 9 10 13}"

aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1 && [ ! -x "$aomdec" ]; then
  echo "error: aomdec not found (set AOMDEC=...) — the lossless-decode oracle is REQUIRED here" >&2
  exit 2
fi

# Pinned-diverging cells: "<content>_<w>x<h>_q0_p<preset>". Presets 0..3 on
# textured content (uniform codes zero residual at every preset and is
# byte-exact there — it must NOT be listed).
pinned_cell() {
  case "$1" in
    gradient_*_p[0-3]|diag_*_p[0-3]) return 0 ;;
    *) return 1 ;;
  esac
}

OUT="${TMPDIR:-/tmp}/lossless.$$"
mkdir -p "$OUT"
pass=0
pinned=0
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
      # (2) losslessness under the reference decoder — REQUIRED for every
      #     cell, pinned or not: a byte-diverging stream must still be right.
      rm -f "$OUT/dec.yuv"
      if ! "$aomdec" --rawvideo -o "$OUT/dec.yuv" "$OUT/rs.obu" >"$OUT/dec.log" 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[aomdec-rejects]"); continue
      fi
      if ! cmp -s "$OUT/dec.yuv" "$OUT/rs.yuv"; then
        fail=$((fail + 1)); failed+=("$cell[NOT-LOSSLESS: decoded != source]"); continue
      fi
      # (1) byte-identity, or the self-promoting pin
      if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        if pinned_cell "$cell"; then
          fail=$((fail + 1)); failed+=("$cell[PROMOTE: pinned cell now byte-matches C — remove it from pinned_cell]")
          continue
        fi
        pass=$((pass + 1))
      else
        if pinned_cell "$cell"; then
          pinned=$((pinned + 1))
          echo "  pinned   $cell (lossless in both; port $(wc -c <"$OUT/rs.obu" | tr -d ' ') B vs C $(wc -c <"$OUT/c.obu" | tr -d ' ') B)"
        else
          fail=$((fail + 1)); failed+=("$cell[bytes: port $(wc -c <"$OUT/rs.obu" | tr -d ' ') B vs C $(wc -c <"$OUT/c.obu" | tr -d ' ') B]")
        fi
      fi
    done
  done
done

total=$((pass + pinned + fail))
echo "coded-lossless identity + lossless-decode: $pass / $total byte-identical (+$pinned pinned-diverging, all lossless)"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[@]}"
fi
if [ "${#vacuous[@]}" -gt 0 ]; then
  echo "  GATE PREMISE FAILED — qp-0 stream == qp-1 stream on textured content (the lossless"
  echo "  path was not exercised): ${vacuous[*]}"
fi
rm -rf "$OUT"
[ "$fail" -eq 0 ] && [ "${#vacuous[@]}" -eq 0 ]

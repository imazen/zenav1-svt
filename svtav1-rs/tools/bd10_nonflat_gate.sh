#!/usr/bin/env bash
# bd10 NON-FLAT identity gate (task #94): the first bd10 cells with a coded
# residual that byte-match real aomenc at bit depth 10 via the u16 MD re-encode
# path (predict_unit_hbd + tx_unit_hbd + bd10_reencode_luma + the highbd
# quantize_fp, no INT16 clamp).
#
# Ported bd10 u16 re-encode envelope (task #94): DC-family AND directional AND
# filter-intra luma leaves, tx_depth 0, rdoq fp OR level 0, 64-dim transforms.
#   - quant::quantize_b_hbd          (rdoq level 0, no INT16 clamp)
#   - intra_edge::dr_predict_hbd     (directional intra, edge_filter off)
#   - hbd::predict_filter_intra_hbd  (filter-intra)
#   - 64-dim qcoeff re-expansion     (TX_64X64 at high qindex)
# Cells STILL outside the envelope fall back to the non-panicking u8 output
# (bd10_tree_supported gate): tx_depth>0, directional WITH the SH edge filter on
# (M5), and the u16 chroma path. Separately, cells whose u8 partition/mode tree
# is NOT bit-depth-scale-invariant (low-qindex / 128px: the u8 tree diverges from
# C's bd10 tree — a partition-symbol divergence, not a level bug) and cells with
# a bd10 CDEF-search / Wiener-LR post-filter divergence (M0..M6 at mid qindex)
# are NOT re-encode-fixable and are documented (docs/bd10-port-map.md). Adding a
# cell here means it BYTE-MATCHES via `cmp`; do NOT add a cell that only falls
# back or that matches only by coincidence.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/bd10nf.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
# Each cell: "content w h qp preset" — known byte-exact in the bd10 u16 envelope.
CELLS=(
  "gradient 64 64 40 13"
  "gradient 64 64 40 10"
  "gradient 64 64 55 3"
  "gradient 64 64 55 6"
  "gradient 64 64 55 10"
  "gradient 64 64 55 13"
  "gradient 128 128 55 10"
  "gradient 128 128 55 13"
)
for cell in "${CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! SVTAV1_BD=10 "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1)); failed+=("$tag")
  fi
done
rm -rf "$OUT"
echo "bd10 non-flat identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

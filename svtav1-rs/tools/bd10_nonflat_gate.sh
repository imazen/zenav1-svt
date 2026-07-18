#!/usr/bin/env bash
# bd10 NON-FLAT identity gate (task #94): the first bd10 cells with a coded
# residual that byte-match real aomenc at bit depth 10 via the u16 MD re-encode
# path (predict_unit_hbd + tx_unit_hbd + bd10_reencode_luma + the highbd
# quantize_fp, no INT16 clamp).
#
# Scope is deliberately NARROW — the ported bd10 u16 envelope is currently the
# DC-family / tx_depth-0 / rdoq-fp subset. These specific gradient cells fall in
# it (all-DC luma leaves). Cells outside the envelope (directional/filter-intra
# intra, tx_depth>0, rdoq level 0, non-uniform chroma) fall back to the
# non-panicking u8 output (bd10_tree_supported gate) and are NOT byte-exact yet
# — they are documented #94 follow-ups (dr_predict_hbd, predict_filter_intra_hbd,
# quantize_b_hbd, the u16 chroma path). Adding a cell here means it BYTE-MATCHES;
# do not add a cell that only falls back.
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

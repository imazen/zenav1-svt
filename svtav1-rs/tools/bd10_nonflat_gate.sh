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
# The re-encode RAN (last_recon10_y populated) for every cell here and each
# BYTE-MATCHES real aomenc at bd10 (verified by `cmp` below).
#
# Coverage rationale (task #94, extended 2026-07-19): the original 8 cells
# jumped from q40 to q55, leaving the re-encode's working qindex range
# (q42..q50 -> base_qindex 168..200) ungated. Those cells are LOAD-BEARING on
# the re-encode: Q10/Q8 is 3.997..3.999 there (NOT the exact 4.000 it reaches
# only at q55/qindex 220), so the coded levels genuinely differ from the u8
# fallback — the u16 quant (quantize_fp_hbd / quantize_b_hbd, bd10 tables) is
# what makes them match. All 64x64 (single-SB, tree bit-depth-scale-invariant
# at qindex>=168). Presets 3/6 exercise the search-based LF/CDEF path, 10/13
# the LPF_PICK_FROM_Q closed form. 128px q58 broadens the multi-SB path.
# Everything at lower qindex / larger-than-64 non-flat falls back to u8 (a
# bit-depth-dependent partition/mode RD decision C makes differently at bd10 —
# see docs/bd10-port-map.md "true bd10 MD"); those cells are NOT gated here.
CELLS=(
  "gradient 64 64 40 13"
  "gradient 64 64 40 10"
  "gradient 64 64 42 10"
  "gradient 64 64 42 13"
  "gradient 64 64 44 10"
  "gradient 64 64 44 13"
  "gradient 64 64 44 3"
  "gradient 64 64 46 10"
  "gradient 64 64 46 13"
  "gradient 64 64 48 6"
  "gradient 64 64 50 10"
  "gradient 64 64 50 13"
  "gradient 64 64 50 6"
  "gradient 64 64 55 3"
  "gradient 64 64 55 6"
  "gradient 64 64 55 10"
  "gradient 64 64 55 13"
  "gradient 128 128 55 10"
  "gradient 128 128 55 13"
  "gradient 128 128 58 10"
  "gradient 128 128 58 13"
  # PD0_LVL_0 partition fix (task #94, this landing): at bd10 C forces
  # PD0_LVL_0 (full-RD partition search) regardless of preset — where bd8
  # uses the preset's LVL_6/LVL_5 variance heuristic (set_pd0_ctrls,
  # enc_mode_config.c:5415). PD0_LVL_0 runs entirely at 8-bit, so the fix is a
  # pure 8-bit partition search (pd0::pd0_pick_sb_partition_lvl0) gated on
  # bd10; the LVL_6 heuristic OVER-SPLITS these low-qindex cells where the
  # full-RD keeps the parent (e.g. q20 p10: C bd10 keeps 4x BLOCK_32X32, the
  # LVL_6 tree SPLIT to 16x BLOCK_16X16). Each cell below was a partition-flip
  # DIFF before the fix and BYTE-MATCHES after (verified `cmp`). Only cells
  # whose ONLY divergence was the partition are here; cells that ALSO have a
  # bd10-sensitive mode/tx flip (the true-bd10-MD axis, e.g. q20 p10's
  # tx_depth) are NOT — they need the u16 leaf funnel (docs/bd10-port-map.md).
  "gradient 64 64 12 10"
  "gradient 64 64 12 13"
  "gradient 64 64 32 10"
  "gradient 64 64 32 13"
  "gradient 128 128 12 10"
  "gradient 128 128 12 13"
  "gradient 128 128 20 10"
  "gradient 128 128 20 13"
  "gradient 128 128 32 10"
  "gradient 128 128 32 13"
  # bd10 TXS-coupling gate fix (task #94, this landing): at bd10 C forces
  # pd0_ctrls.pd0_level = PD0_LVL_0 (set_pd0_ctrls, enc_mode_config.c:5416), so
  # the eff-M9 per-SB TXS coupling (svt_aom_sig_deriv_enc_dec_allintra,
  # enc_mode_config.c:8114-8118: `pcs->txs_level == 0 && pd0_level ==
  # PD0_LVL_6`) NEVER fires — TXS stays OFF (tx_depth 0 everywhere), where the
  # port's u8 funnel bumped it to level 5 for undemoted PD0_LVL_6 SBs and coded
  # tx_depth 1 on some leaves. Those tx_depth-1 leaves were out of the bd10
  # re-encode envelope (bd10_reencode_node asserts tx_depth==0) -> the whole
  # frame fell back to the u8 output and DIFFERED. Forcing sb_is_lvl6=false at
  # bd10 (partition.rs) restores tx_depth 0, the re-encode runs, and the cell
  # byte-matches. These are the gradient eff-M9 (p10/p13) cells whose ONLY
  # remaining flip was the tx_depth (now closed); each was a tx_depth DIFF
  # before the fix and BYTE-MATCHES after (verified `cmp`). Load-bearing on the
  # re-encode: Q10 != 4*Q8 at these qindexes, so a u8 fallback would NOT match.
  # (diag q40 exercises the directional dr_predict_hbd re-encode arm.)
  "gradient 64 64 5 10"
  "gradient 64 64 5 13"
  "gradient 64 64 20 10"
  "gradient 64 64 20 13"
  "gradient 128 128 5 10"
  "gradient 128 128 5 13"
  "gradient 128 128 40 10"
  "gradient 128 128 40 13"
  "diag 64 64 40 10"
  "diag 64 64 40 13"
  # bd10 CHROMA re-encode (task #94, this landing): the luma re-encode
  # (bd10_reencode_luma) recomputes only luma levels; chroma stayed at the u8
  # MD decision. On `diag` the subsampled chroma carries a coded residual (the
  # diagonal edge), so the u8 chroma levels diverged from C's bd10 chroma quant
  # (decode-both proved LUMA byte-identical, chroma off by 1 LSB: port coded
  # flat 512 where C coded 511). bd10_reencode_chroma now recomputes U+V at
  # bd10 (predict_unit_hbd + tx_unit_hbd plane 1 + uv_tx_type + the bd10 chroma
  # quant table) and overwrites chroma_dec. Each cell below was a chroma DIFF
  # before the fix and BYTE-MATCHES after (verified `cmp`). Load-bearing on the
  # chroma re-encode (non-flat chroma; a u8 fallback would NOT match). q40 p10/
  # p13 above ALSO ride the chroma re-encode now (previously matched only
  # because their chroma residual happened to agree). gradient/uniform chroma is
  # flat -> re-encodes to zero -> the other gate cells stay byte-unchanged.
  "diag 64 64 5 10"
  "diag 64 64 5 13"
  "diag 64 64 12 10"
  "diag 64 64 12 13"
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

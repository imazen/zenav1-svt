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
# Reference decoder for the DECODABILITY assert below. Required, never skipped:
# a byte-identity gate is structurally blind to a stream that is byte-equal to
# nothing (a cell that regresses out of the list) and to corruption on the cells
# it does not list, so every port OBU this gate produces must also be provably
# decodable. Override with AOMDEC=/path/to/aomdec.
aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
    for _c in aomdec /root/aomdec-debug/aomdec; do
        if command -v "$_c" >/dev/null 2>&1; then aomdec=$_c; break; fi
    done
fi
# Hard fail, never a graceful skip: a gate that silently stops checking
# decodability is a gate that lies.
command -v "$aomdec" >/dev/null 2>&1 || {
    echo "bd10 non-flat gate: aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}
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
  # ---------------------------------------------------------------------
  # bd10 FULL-RD mode funnel (task #94, MODE axis — this landing). Below
  # eff-M9 `nic_counts` is (6,6,6), so the coded mode is the MDS1/MDS3
  # full-RD winner, NOT the MDS0 fast survivor. Running those stages at 8
  # bits picked C's *bd8* winner; they now run at true 10 bits (10-bit
  # prediction from the bd10 canvas, 10-bit residual/quant/RDOQ, the bd10
  # quant tables and full_lambda_md[EB_10_BIT_MD]), luma AND chroma, so the
  # joint block RD is entirely 10-bit.
  #
  # p7/p8 were NEVER gated before (the gate jumped p6 -> p9) and were 12/40
  # byte-exact; they are now 40/40 — 28 cells newly closed, MEASURED by
  # re-running this exact sweep with the change stashed. All 40 are gated
  # here so the whole p7..p13 band is covered rather than sampled.
  "gradient 64 64 12 7"
  "gradient 64 64 20 7"
  "gradient 64 64 32 7"
  "gradient 64 64 40 7"
  "gradient 64 64 55 7"
  "gradient 128 128 12 7"
  "gradient 128 128 20 7"
  "gradient 128 128 32 7"
  "gradient 128 128 40 7"
  "gradient 128 128 55 7"
  "diag 64 64 12 7"
  "diag 64 64 20 7"
  "diag 64 64 32 7"
  "diag 64 64 40 7"
  "diag 64 64 55 7"
  "diag 128 128 12 7"
  "diag 128 128 20 7"
  "diag 128 128 32 7"
  "diag 128 128 40 7"
  "diag 128 128 55 7"
  "gradient 64 64 12 8"
  "gradient 64 64 20 8"
  "gradient 64 64 32 8"
  "gradient 64 64 40 8"
  "gradient 64 64 55 8"
  "gradient 128 128 12 8"
  "gradient 128 128 20 8"
  "gradient 128 128 32 8"
  "gradient 128 128 40 8"
  "gradient 128 128 55 8"
  "diag 64 64 12 8"
  "diag 64 64 20 8"
  "diag 64 64 32 8"
  "diag 64 64 40 8"
  "diag 64 64 55 8"
  "diag 128 128 12 8"
  "diag 128 128 20 8"
  "diag 128 128 32 8"
  "diag 128 128 40 8"
  "diag 128 128 55 8"
  # p6, newly closed by the same change (the p6 MODE class went from 19
  # DIFF cells to 7; the rest are now COEFF/FH, documented in
  # docs/bd10-port-map.md). Each was a mode/uv flip before and byte-matches
  # after (verified by `cmp` below).
  "gradient 64 64 12 6"
  "diag 64 64 12 6"
  "diag 128 128 12 6"
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
  # bd10 u16 LUMA MODE FUNNEL (task #94, this landing): a TRUE 10-bit recon
  # canvas threaded through the eff-M9 leaf funnel so each block's MDS0 mode
  # decision is made on the 10-bit recon (predict_unit_hbd + hadamard_satd_hbd
  # + the bd10 fast lambda = kf_full_lambda_bd10/16), not the MSB-truncated u8
  # recon. On `diag` q20 the true bd10 recon (the ~+20/px hbd-predictor
  # divergence from recon8<<2) tips the near-tie so C picks SMOOTH(9)/V(1) on
  # the diagonal-edge 8x8s where the u8 SATD keeps DC(0) — the DC->SMOOTH flip
  # the port previously could not reproduce. commit_leaf writes the winner's
  # bd10 recon into the canvas for the next block's neighbours (the sequential
  # coupling). bd10-gated (bd8/other-preset/partial-SB pass None -> byte-
  # IDENTICAL). Each was a LUMA MODE flip DIFF before and BYTE-MATCHES after
  # (verified `cmp`). Higher-qindex diag (q32/q55) + diag 128 stay DIFF on a
  # SEPARATE, pre-existing bd10 recon-precision cascade (the shared bd10 recon
  # path's DC-prediction averages drift from C on dense coded content, DIFF
  # with the funnel OFF too) — NOT this funnel; see docs/bd10-port-map.md.
  "diag 64 64 20 10"
  "diag 64 64 20 13"
  # bd10 AVX2-HADAMARD fix (task #94, this landing): the MDS0 fast-loop SATD
  # kernels were ported from `svt_aom_hadamard_{16x16,32x32}_c`, but the encoder
  # binds those RTCD pointers to the AVX2 implementations
  # (SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2), common_dsp_rtcd.c:1047-1048),
  # and the two are NOT equivalent above the 8-bit residual range they were
  # written for: `_avx2` carries the 16x16 cross-combine in WRAPPING int16 lanes
  # and buffers the 32x32's four 16x16 sub-transforms in an `int16_t temp_coeff`
  # (is_final=0) before sign-extending, while `_c` keeps both in int32. At 8-bit
  # the stage maxima ([-32640,32640] / [-16320,16320]) cannot wrap and the two
  # agree bit-for-bit; at 10-bit the 16x16 stage reaches ~+/-130560 and the AVX2
  # kernel WRAPS. The bd10 mode funnel feeds exactly such residuals, so the port
  # computed a DIFFERENT fast-loop SATD than the encoder and picked a different
  # MDS0 winner (measured on `diag 64 64 32 10`, the 32x32 leaf at (32,0): C's
  # DC candidate satd 49152 vs the port's 143360 -> C coded DC_PRED, the port
  # V_PRED). Porting the AVX2 int16 semantics (src/hadamard.rs) closed the whole
  # eff-M9 class. Each cell below was a DIFF before and BYTE-MATCHES after
  # (verified `cmp`); the full {diag,gradient} x {64,128} x q{5..63} x p{10,13}
  # sweep is now 64/64 with ZERO diffs.
  "diag 64 64 32 10"
  "diag 64 64 32 13"
  "diag 64 64 48 10"
  "diag 64 64 48 13"
  "diag 64 64 55 10"
  "diag 64 64 55 13"
  "diag 64 64 63 10"
  "diag 64 64 63 13"
  "diag 128 128 5 10"
  "diag 128 128 5 13"
  "diag 128 128 12 10"
  "diag 128 128 12 13"
  "diag 128 128 20 10"
  "diag 128 128 20 13"
  "diag 128 128 32 10"
  "diag 128 128 32 13"
  "diag 128 128 40 10"
  "diag 128 128 40 13"
  "diag 128 128 48 10"
  "diag 128 128 48 13"
  "diag 128 128 55 10"
  "diag 128 128 55 13"
  "diag 128 128 63 10"
  "diag 128 128 63 13"
  "gradient 64 64 48 10"
  "gradient 64 64 48 13"
  "gradient 64 64 63 10"
  "gradient 64 64 63 13"
  "gradient 128 128 48 10"
  "gradient 128 128 48 13"
  "gradient 128 128 63 10"
  "gradient 128 128 63 13"
  # eff-M9 BAND WIDENED (2026-07-19): the gate previously covered only
  # presets 10 and 13, leaving the rest of the eff-M9 band (9, 11, 12)
  # ungated even though the same closed envelope applies to it. Measured with
  # tools/bd10_classify.sh: {gradient,diag} x {64,128}^2 x q{5,20,40,63} x
  # p{9,11,12} is 48/48 byte-identical. No product code changed — these cells
  # already matched; the gate simply now covers the presets between the two it
  # was pinned on, so a regression anywhere in the band is caught.
  "gradient 64 64 5 9"
  "gradient 64 64 5 11"
  "gradient 64 64 5 12"
  "gradient 64 64 20 9"
  "gradient 64 64 20 11"
  "gradient 64 64 20 12"
  "gradient 64 64 40 9"
  "gradient 64 64 40 11"
  "gradient 64 64 40 12"
  "gradient 64 64 63 9"
  "gradient 64 64 63 11"
  "gradient 64 64 63 12"
  "gradient 128 128 5 9"
  "gradient 128 128 5 11"
  "gradient 128 128 5 12"
  "gradient 128 128 20 9"
  "gradient 128 128 20 11"
  "gradient 128 128 20 12"
  "gradient 128 128 40 9"
  "gradient 128 128 40 11"
  "gradient 128 128 40 12"
  "gradient 128 128 63 9"
  "gradient 128 128 63 11"
  "gradient 128 128 63 12"
  "diag 64 64 5 9"
  "diag 64 64 5 11"
  "diag 64 64 5 12"
  "diag 64 64 20 9"
  "diag 64 64 20 11"
  "diag 64 64 20 12"
  "diag 64 64 40 9"
  "diag 64 64 40 11"
  "diag 64 64 40 12"
  "diag 64 64 63 9"
  "diag 64 64 63 11"
  "diag 64 64 63 12"
  "diag 128 128 5 9"
  "diag 128 128 5 11"
  "diag 128 128 5 12"
  "diag 128 128 20 9"
  "diag 128 128 20 11"
  "diag 128 128 20 12"
  "diag 128 128 40 9"
  "diag 128 128 40 11"
  "diag 128 128 40 12"
  "diag 128 128 63 9"
  "diag 128 128 63 11"
  "diag 128 128 63 12"
  # ---------------------------------------------------------------------
  # bd10 PART axis (task #94, this landing): the PD1 depth-refine + NSQ walk
  # (`decide_sb_refined`, presets 0..=5) now runs at TRUE 10 bits — C's PD1
  # is `hbd_md = 2`, so `test_depth` / `test_split_partition`
  # (product_coding_loop.c:10857 / :10770) sum 10-bit MDS3 leaf costs and
  # take `full_sb_lambda_md[EB_10_BIT_MD]` for the partition rate. Running
  # that walk on 8-bit leaf costs picked C's *bd8* geometry.
  #
  # This band (p0..p5) had NEVER been gated — the gate jumped p6 -> p13. The
  # six q12/q20 cells below are NEWLY closed by this landing, MEASURED by
  # re-running each with the change reverted (all six were DIFF before,
  # MATCH after); the four q55 cells were already byte-exact but ungated, so
  # they are added as regression cover for the band rather than as wins.
  # Everything else at p0..p5 is still open (docs/bd10-port-map.md): the
  # residual is dominated by the bd10 full-RD's missing CfL arm, which real
  # photographic content exercises heavily.
  #
  # NEWLY CLOSED by the 10-bit PD1 walk (before: DIFF, after: MATCH):
  "gradient 64 64 12 2"
  "gradient 64 64 12 3"
  "gradient 64 64 12 4"
  "gradient 64 64 12 5"
  "diag 64 64 20 4"
  "diag 128 128 20 4"
  # Already byte-exact pre-landing, previously ungated (band regression cover):
  "gradient 64 64 55 1"
  "gradient 64 64 55 2"
  "gradient 64 64 55 4"
  "gradient 64 64 55 5"
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
  # Decodability BEFORE byte-identity: a stream that aomdec rejects is a
  # corruption bug regardless of what `cmp` says, and `cmp` alone cannot see it.
  if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[undecodable]"); continue
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

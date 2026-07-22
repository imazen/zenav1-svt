#!/usr/bin/env bash
# HDR-FORK x bd10 identity gate — the fork-mode counterpart of
# bd10_nonflat_gate.sh.
#
# Both encoders run under the SAME env vector: SVT_HDR_MODE=1 selects the
# SVT_HDR_MODE=ON C oracle (lib Bin/ReleaseHdr, driver capture_c_trace.hdr.bin,
# built by tools/capture_c_trace/build.sh) on the C side and
# HdrForkConfig::hdr_fork_c_mode1() on the Rust side (identity_run calls
# HdrForkConfig::from_env). Any SVT_FORK_* knob set here reaches BOTH encoders.
#
# WHAT FORK MODE IS. Compiling the C library with -DSVT_HDR_MODE=ON does NOT
# turn on every fork feature. enc_settings.c neutralizes the fork's feature
# knobs UNCONDITIONALLY at :1181-1203 (ac_bias 0.0, sharp_tx 0,
# noise_norm_strength 0, alt_lambda_factors 0, kf_tf_strength 3,
# qp_scale_compress_strength 0.0) — they sit outside every `#if SVT_HDR_MODE`
# block. What MODE1 actually changes is:
#   * the fork's UNCONDITIONAL code-path deltas — unconditional loop filter
#     (deblocking_filter.c:695), the double variance pipeline
#     (definitions.h:235, enc_dec_process.c:2351-2378), fork chroma-qindex
#     derivation (rc_crf_cqp.c:553-590), light-RDOQ low-DC chroma
#     (full_loop.c:1852), mds0 dist-type branching (product_coding_loop.c:975),
#     diff_uv_delta / separate_uv_delta_q forced true (entropy_coding.c:2375,
#     :2749);
#   * SIX flipped defaults — QM on with luma levels 6..10 (:1123/:1128/:1133),
#     variance boost on (:1149), tf_strength 1 (:1156), sharpness 1 (:1163).
# (Bit depth and preset also flip, but this harness sets both explicitly.)
#
# A cell listed here BYTE-MATCHES the fork oracle via `cmp`. Do NOT add a cell
# that merely falls back or matches by coincidence.
#
# QM IN PD0 (this landing) closed class A + one class-B cell. C's PD0 light
# encode (`svt_aom_quantize_inv_quantize_light`, full_loop.c:1263) applies the
# frame luma quantization matrix whenever `using_qmatrix` is set (fork default)
# — its QM arm calls `svt_av1_quantize_b_qm`. The port's PD0 leaf quantize
# (`pd0.rs` `tx_quant_core` -> `quantize_b`) was QM-BLIND, so a QM-tipped
# partition NEAR-TIE (the top-left 32x32 of a smooth SB) coded PARTITION_SPLIT
# where C's QM-aware PD0 keeps PARTITION_NONE. C forces PD0_LVL_0 at bd10
# (hbd_md set), so the fix is threaded through `pd0_pick_sb_partition_lvl0`
# (bd10-only) and gated on the frame luma qm_level < 15 (fork-only; mainline
# qm_level 15 keeps the non-QM `quantize_b`, byte-inert). This closed all four
# QM-path cells (each matches with SVT_FORK_ENABLE_QM=0, diverges with QM on):
# gradient 64 q12, gradient 128 q40, diag 64 q48 (the docs' 3-combo class A)
# AND diag 128 q48 (same QM-path root, just omitted from that list); all now
# gated above. The remaining 10 (below) are a separate, deeper axis.
#
# QUANT-SHARPNESS (this landing) closed the 4 gradient q5 cells (class B,
# Group 1). C's `svt_av1_build_quantizer` (md_config_process.c:106-120) applies
# a fork sharpness adjustment to the dead-zone quantizer factors:
# `qzbin_factor -= offset; qrounding_factor += offset` (offset =
# max(sharpness<<1, |q - base|)) for every table entry whose `q < base_q_idx`,
# where `base_q_idx = 31` is the FIXED init value the table is built against
# (`resource_coordination_process.c:365`; the build runs ONCE at picture-0 init,
# `initial_rc_process.c:804`, "1 time per sequence assuming qindex offset 0").
# A q5 frame codes blocks at qindex 20 (< 31) -> the sharpened table[20]
# (qzbin 84->73, qround 48->59), keeping more/larger coeffs (+23B). The port's
# `build_quant_table[_bd]` never applied it, so it coded the sharpness-OFF
# bytes at every sharpness. Fix: `apply_quant_sharpness_factors` (quant.rs),
# routed through the bd10 re-encode (pipeline `bd10_reencode_{luma,chroma}`).
# Byte-inert at mainline (sharpness 0 -> no-op) and at qindex >= 31 (q12+
# blocks), which is why only q5 flipped. Sibling-C RD dump (throwaway,
# reverted) confirmed the exact zbin/round + eob mechanism.
#
# KNOWN-OPEN fork x bd10 residual — 6 of the 64-cell sweep
# ({gradient,diag} x {64,128}^2 x q{5,12,20,32,40,48,55,63} x p{10,13}) are NOT
# gated here. They are EXCLUDED, not weakened; see docs/HDR-ON-4.2.md:
#   * class B (Group 2) — the diag q5 cells + diag 128 q12: a SEPARATE,
#     non-sharpness root. Measured: they diverge even with `SVT_FORK_SHARPNESS=0`
#     (the quant-sharpness fix above is symmetric on them — it shifts BOTH port
#     and C by the same amount, leaving a residual port-over-codes-by-1 delta
#     that is present at sharpness 0 too). diag carries a coded CHROMA residual
#     the gradient cells do not, so the root is in the bd10 chroma re-encode /
#     a coeff near-tie, not the quantizer. Needs the sibling-C RD-dump treatment
#     to close.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/hdrbd10.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()

# Each cell: "content w h qp preset" — fork mode, bd10, byte-exact vs the
# SVT_HDR_MODE=ON oracle.
#
# QM AT BD10 (this landing) is what makes these match. The bd10 re-encode
# (tx_unit_hbd) previously always called the NON-QM highbd quantize kernels and
# passed `iwt: None` to the trellis, so with the fork's default QM on, bd10
# quantized/dequantized WITHOUT matrices while bd8 applied them — every fork
# bd10 cell diverged (measured 0/64 before, 40/64 after). C selects the matrix
# purely from base_qindex (svt_av1_qm_init, md_config_process.c:246-280 — no
# bit-depth term) and routes bd>8 through svt_av1_highbd_quantize_{b,fp}_facade
# (full_loop.c:139-176), which is what qm::quantize_{fp,b}_hbd_qm now port.
# Load-bearing: with SVT_FORK_ENABLE_QM=0 these cells still match, but that is
# a DIFFERENT (QM-off) bitstream — the point is that QM-on now matches too.
#
# VARIANCE BOOST AT BD10 (this landing) adds the six `diag` q12/q20 cells.
# svt_av1_convert_qindex_to_q_fp8 / svt_av1_compute_qdelta_fp (rc_aq.c:24-61)
# are the ONLY two bit-depth entry points in the variance-boost chain — they
# select both a different qlookup table and a different shift per depth (8-bit
# `ac_quant_qtx << 6`, 10-bit `<< 4`). var_boost.rs hardcoded the 8-bit form,
# skewing every boost at bd10. Everything else in the chain is correctly
# bit-depth INVARIANT because C computes variance/mean on the 8-bit MSB luma
# plane at every depth.
CELLS=(
  # --- gradient, single-SB ---
  # q5 was class B — closed by the QUANT-SHARPNESS landing (quant.rs
  # apply_quant_sharpness_factors; see the QUANT-SHARPNESS note below). The
  # fork's `svt_av1_build_quantizer` sharpens the dead-zone zbin/round for
  # blocks whose qindex is below the fixed init base_q_idx=31; a q5 block at
  # qindex 20 (<31) uses the sharpened table, the port's builder did not.
  "gradient 64 64 5 10"
  "gradient 64 64 5 13"
  # q12 was class A (QM-in-PD0 partition near-tie) — closed by the PD0
  # quantization-matrix landing (pd0.rs tx_quant_core); see the QM-IN-PD0 note.
  "gradient 64 64 12 10"
  "gradient 64 64 12 13"
  "gradient 64 64 20 10"
  "gradient 64 64 20 13"
  "gradient 64 64 32 10"
  "gradient 64 64 32 13"
  "gradient 64 64 40 10"
  "gradient 64 64 40 13"
  "gradient 64 64 48 10"
  "gradient 64 64 48 13"
  "gradient 64 64 55 10"
  "gradient 64 64 55 13"
  "gradient 64 64 63 10"
  "gradient 64 64 63 13"
  # --- gradient, multi-SB ---
  # q5 closed by the QUANT-SHARPNESS landing (below).
  "gradient 128 128 5 10"
  "gradient 128 128 5 13"
  "gradient 128 128 12 10"
  "gradient 128 128 12 13"
  "gradient 128 128 20 10"
  "gradient 128 128 20 13"
  "gradient 128 128 32 10"
  "gradient 128 128 32 13"
  # q40 was class A — closed by the QM-in-PD0 landing.
  "gradient 128 128 40 10"
  "gradient 128 128 40 13"
  "gradient 128 128 48 10"
  "gradient 128 128 48 13"
  "gradient 128 128 55 10"
  "gradient 128 128 55 13"
  "gradient 128 128 63 10"
  "gradient 128 128 63 13"
  # --- diag (coded chroma residual: exercises the bd10 CHROMA re-encode's
  #     per-plane QM level, qm_uv[0]/qm_uv[1]) ---
  "diag 64 64 12 10"
  "diag 64 64 12 13"
  "diag 64 64 20 10"
  "diag 64 64 20 13"
  "diag 64 64 32 10"
  "diag 64 64 32 13"
  "diag 64 64 40 10"
  "diag 64 64 40 13"
  # q48 was class A — closed by the QM-in-PD0 landing.
  "diag 64 64 48 10"
  "diag 64 64 48 13"
  "diag 64 64 55 10"
  "diag 64 64 55 13"
  "diag 64 64 63 10"
  "diag 64 64 63 13"
  "diag 128 128 20 10"
  "diag 128 128 20 13"
  "diag 128 128 32 10"
  "diag 128 128 32 13"
  "diag 128 128 40 10"
  "diag 128 128 40 13"
  # q48 is QM-path (matches with SVT_FORK_ENABLE_QM=0, diverges with QM on) —
  # the SAME class-A PD0 near-tie root, just omitted from the docs' original
  # 3-combo class-A enumeration. Closed by the same QM-in-PD0 landing.
  "diag 128 128 48 10"
  "diag 128 128 48 13"
  "diag 128 128 55 10"
  "diag 128 128 55 13"
  "diag 128 128 63 10"
  "diag 128 128 63 13"
)
for cell in "${CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! SVT_HDR_MODE=1 SVTAV1_BD=10 "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[rs-err]"); continue
  fi
  if ! SVT_HDR_MODE=1 SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1)); failed+=("$tag")
  fi
done

# ANTI-VACUITY: fork mode must not be silently degenerating to mainline bytes.
# If the fork oracle and the mainline oracle produced identical output, every
# "fork" match above would be meaningless (it would just be re-proving the
# mainline bd10 gate). Assert the two oracles genuinely differ on a gated cell.
SVT_HDR_MODE=1 SVTAV1_BD=10 "$HERE/identity_run" gradient 64 64 40 10 "$OUT/av" >/dev/null 2>&1
SVT_HDR_MODE=1 SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" 64 64 40 10 "$OUT/av.yuv" "$OUT/av_fork.obu" 10 >/dev/null 2>&1
SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" 64 64 40 10 "$OUT/av.yuv" "$OUT/av_main.obu" 10 >/dev/null 2>&1
if cmp -s "$OUT/av_fork.obu" "$OUT/av_main.obu"; then
  echo "ANTI-VACUITY FAILURE: the SVT_HDR_MODE=ON oracle produced mainline-identical"
  echo "bytes — the fork oracle is not actually a fork build (check that"
  echo "Bin/ReleaseHdr/libSvtAv1Enc.a came from cbuild-static-hdr with -DSVT_HDR_MODE=ON)."
  rm -rf "$OUT"
  exit 1
fi

rm -rf "$OUT"
echo "hdr-fork bd10 identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

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
# KNOWN-OPEN fork x bd10 axes (NOT gated here — each measured by isolating the
# knob on both sides, see docs/HDR-ON-4.2.md):
#   * variance boost at bd10 — the C variance/mean pipeline reads the 8-bit MSB
#     plane at EVERY bit depth (reference_object.c:246 creates the PA reference
#     at EB_EIGHT_BIT; resource_coordination_process.c:1320 aliases y_buffer
#     onto the y8b pool), so every 8-bit-domain constant downstream
#     (delta_var_th 7500, the PQ dark-bias `mean <= 25000`) stays valid at 10
#     bit. Cells that need SVT_FORK_ENABLE_VARIANCE_BOOST=0 to match are
#     tracking that.
#   * loop-filter sharpness=1 at bd10.
#   * a QM residual at a few mid/high-qindex cells.
# Cells in those classes are excluded below rather than weakened.
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
CELLS=(
  # --- gradient, single-SB ---
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
  "gradient 128 128 12 10"
  "gradient 128 128 12 13"
  "gradient 128 128 20 10"
  "gradient 128 128 20 13"
  "gradient 128 128 32 10"
  "gradient 128 128 32 13"
  "gradient 128 128 48 10"
  "gradient 128 128 48 13"
  "gradient 128 128 55 10"
  "gradient 128 128 55 13"
  "gradient 128 128 63 10"
  "gradient 128 128 63 13"
  # --- diag (coded chroma residual: exercises the bd10 CHROMA re-encode's
  #     per-plane QM level, qm_uv[0]/qm_uv[1]) ---
  "diag 64 64 32 10"
  "diag 64 64 32 13"
  "diag 64 64 40 10"
  "diag 64 64 40 13"
  "diag 64 64 55 10"
  "diag 64 64 55 13"
  "diag 64 64 63 10"
  "diag 64 64 63 13"
  "diag 128 128 32 10"
  "diag 128 128 32 13"
  "diag 128 128 40 10"
  "diag 128 128 40 13"
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

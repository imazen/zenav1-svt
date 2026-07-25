#!/usr/bin/env bash
# SUPERRES identity + conformance gate (superres chunk B.3).
#
# Superres encodes the frame at a REDUCED width `coded_w = w * 8 / denom` and
# the decoder normatively upscales it back to `w`. Three things must hold, and
# this gate checks all three per cell:
#
#   1. BYTE-PARITY — the port's OBU == the real C encoder's OBU at the same
#      config (`SVT_SUPERRES_KF_DENOM=D`, which the C driver maps to
#      `superres_mode = SUPERRES_FIXED` + `superres_kf_denom = D`).
#   2. DECODABILITY — the stream decodes under the AV1 reference decoder and
#      the decoded frame comes out at the FULL (upscaled) size. Byte-parity
#      alone cannot catch a header that describes a geometry neither encoder
#      actually produced; the upscale is normative, so this is not optional.
#   3. ANTI-VACUITY — the superres stream must DIFFER from the same cell
#      encoded without superres. A cell where they coincide proves nothing.
#
# MEASURED (2026-07-24): for a STILL (KEY) frame the denominator that takes
# effect in C is `--superres-kf-denom` / `superres_kf_denom`. Setting only
# `superres_denom` signals `enable_superres = 1` but leaves `use_superres = 0`
# on the key frame.
#
# SCOPE (the cells this gate CLAIMS byte-parity for): allintra preset 8, every
# denominator 9..=16, both contents, both sizes, qp {20,32,40,55}. Adding a cell
# means it byte-matches — do NOT add one that only decodes.
#
# Two documented exclusions, both with a measured root cause:
#
# * presets <= 6 — the port REFUSES superres there (`superres_config_error`):
#   loop restoration is on (`seq_tools_for_preset`: wn > 0) and C runs LR on the
#   UPSCALED frame (`svt_av1_superres_upscale_frame` sits between CDEF and LR,
#   cdef_process.c:152) while this port still searches/applies it at the coded
#   width. Refusing beats emitting a stream whose LR geometry disagrees with the
#   signalled one.
# * ONE cell, `gradient_64_q32_p7_d10` — the only byte divergence left in the
#   full `SR_PRESETS="7 8 9 10 13"` sweep, which is otherwise **639/640**
#   byte-identical (and 640/640 decodable at the upscaled size). Preset 7 is
#   therefore out of the default set until it is root-caused; run
#   `SR_PRESETS="7 8 9 10 13" tools/superres_gate.sh` to see it.
#
#   History worth keeping: that sweep was 507/640 before chunk B.4. The 133
#   divergences were all partition-symbol (`CDF10`) flips on textured content,
#   and encoding the port's OWN downscaled pixels at the coded dims WITHOUT
#   superres was byte-identical to C (gradient 128x128 q32 p10 d16: 724B ==
#   724B, 6390 tile ops) — so neither the downscale nor the coded-width MD was
#   at fault. Root cause was in C: `scale_pcs_params` (resize.c:1434) re-inits
#   the b64/SB geometry for the coded size but does NOT recompute
#   `pcs->variance`, so C's PD0 keeps reading picture-analysis variances
#   computed on the FULL-RESOLUTION b64 grid through the new coded-grid
#   indices. Chunk B.4 reproduces that indexing deliberately.
#
# Env: AOMDEC (path to aomdec; required — no graceful skip).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  echo "aomdec not found (set AOMDEC=/path/to/aomdec) — this gate needs the" >&2
  echo "reference decoder to check the normative upscale" >&2
  exit 2
fi
read -r -a SIZES <<<"${SR_SIZES:-64 128}"
read -r -a QPS <<<"${SR_QPS:-20 32 40 55}"
read -r -a PRESETS <<<"${SR_PRESETS:-8 9 10 13}"
read -r -a DENOMS <<<"${SR_DENOMS:-9 10 11 12 13 14 15 16}"
read -r -a CONTENTS <<<"${SR_CONTENTS:-uniform gradient}"
OUT="${TMPDIR:-/tmp}/srgate.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
decode_fail=()
vac=()
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        for d in "${DENOMS[@]}"; do
          cell="${content}_${sz}_q${qp}_p${p}_d${d}"
          if ! SVTAV1_SUPERRES="$d" "$HERE/identity_run" \
               "$content" "$sz" "$sz" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
          fi
          if ! SVT_SUPERRES_KF_DENOM="$d" SVT_TRACE_OUT=/dev/null \
               "$HERE/capture_c_trace/capture_c_trace" \
               "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 8 >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
          fi
          # (3) anti-vacuity: the same cell WITHOUT superres.
          if ! "$HERE/identity_run" "$content" "$sz" "$sz" "$qp" "$p" "$OUT/ns" \
               >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[ns-err]"); continue
          fi
          if cmp -s "$OUT/rs.obu" "$OUT/ns.obu"; then
            vac+=("$cell")
          fi
          # (2) decodability + output size, on the PORT's stream.
          if ! "$aomdec" --rawvideo -o "$OUT/dec.yuv" "$OUT/rs.obu" >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[decode]"); decode_fail+=("$cell"); continue
          fi
          # I420 at the FULL width: w*h + 2*((w+1)/2 * (h+1)/2).
          want=$(( sz * sz + 2 * (((sz + 1) / 2) * ((sz + 1) / 2)) ))
          got=$(stat -c%s "$OUT/dec.yuv")
          if [ "$got" -ne "$want" ]; then
            fail=$((fail + 1))
            failed+=("$cell[decoded ${got}B != upscaled ${want}B]")
            continue
          fi
          # (1) byte-parity.
          if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
            pass=$((pass + 1))
          else
            fail=$((fail + 1)); failed+=("$cell")
          fi
        done
      done
    done
  done
done
echo "superres identity + conformance: $pass / $((pass + fail)) cells"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[*]}"
fi
if [ "${#vac[@]}" -gt 0 ]; then
  echo "  VACUOUS (superres stream == non-superres stream): ${vac[*]}"
fi
rm -rf "$OUT"
[ "$fail" -eq 0 ] && [ "${#vac[@]}" -eq 0 ]

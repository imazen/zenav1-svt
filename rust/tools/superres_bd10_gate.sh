#!/usr/bin/env bash
# SUPERRES bd10 identity + conformance gate (the 10-bit arm of
# `superres_gate.sh`).
#
# C's native-10-bit superres flow (svt_aom_resize_frame, resize.c) is pack ->
# `svt_av1_highbd_resize_plane_horizontal` -> unpack: the u8 canvas the rest
# of the encoder reads is `filtered_u16 >> 2`, NOT the already-truncated u8
# resized. The port mirrors that order (`superres_downscale_420_hbd`), and
# the normative recon upscale runs the u16 twin
# (`highbd_upscale_normative_row`, pinned byte-exact by
# `c_parity_superres::upscale_row_hbd_matches_c_across_denominators`).
#
# Per cell this gate asserts FOUR things:
#
#   1. BYTE-PARITY — the port's OBU == the real C encoder's OBU at the same
#      config (SVT_SUPERRES_KF_DENOM=D, bd 10, the SAME u16 source .yuv).
#   2. DECODABILITY + OUTPUT GEOMETRY — the stream decodes under aomdec and
#      the frame comes out at the FULL (upscaled) size as u16 I420.
#   3. RECON PARITY — the port's `last_recon10_final` (SVTAV1_FINAL_RECON)
#      equals aomdec's decoded output byte-for-byte. This is the leg that
#      pins the u16 normative upscale + the whole 10-bit filter chain at the
#      output geometry; a byte-identical stream already implies it, but the
#      explicit comparison keeps the check meaningful if byte-parity ever
#      regresses first.
#   4. ANTI-VACUITY — the superres stream must DIFFER from the same cell at
#      bd10 without superres.
#
# The source is identity_run's SVTAV1_HBD_SRC content: a REAL 10-bit source
# whose low 2 bits carry signal, so the u16 downscale's extra precision is
# observable in the output bytes (u8<<2 input would let a u8-resize pass as
# "parity" — a vacuous gate).
#
# Env: AOMDEC (path to aomdec; required — no graceful skip). SR_* filters
# match the 8-bit gate's.
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
OUT="${TMPDIR:-/tmp}/srgate10.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
vac=()
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        # (4) anti-vacuity baseline: the same cell WITHOUT superres, still
        # bd10 + native source.
        ns_ok=1
        if ! SVTAV1_BD=10 SVTAV1_HBD_SRC=1 "$HERE/identity_run" \
             "$content" "$sz" "$sz" "$qp" "$p" "$OUT/ns" \
             >/dev/null 2>&1; then
          ns_ok=0
        fi
        for d in "${DENOMS[@]}"; do
          cell="${content}_${sz}_q${qp}_p${p}_d${d}"
          if [ "$ns_ok" -eq 0 ]; then
            fail=$((fail + 1)); failed+=("$cell[ns-err]"); continue
          fi
          if ! SVTAV1_BD=10 SVTAV1_HBD_SRC=1 SVTAV1_SUPERRES="$d" \
               SVTAV1_FINAL_RECON="$OUT/rec.yuv" \
               "$HERE/identity_run" \
               "$content" "$sz" "$sz" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
          fi
          # C reads the SAME u16 .yuv identity_run wrote, at bit depth 10.
          if ! SVT_SUPERRES_KF_DENOM="$d" SVT_TRACE_OUT=/dev/null \
               "$HERE/capture_c_trace/capture_c_trace" \
               "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 \
               >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
          fi
          if cmp -s "$OUT/rs.obu" "$OUT/ns.obu"; then
            vac+=("$cell")
          fi
          # (2) decodability + output size on the PORT's stream. aomdec
          # --rawvideo emits u16 LE at 10-bit: 2 bytes per I420 sample at
          # the FULL (upscaled) width.
          if ! "$aomdec" --rawvideo -o "$OUT/dec.yuv" "$OUT/rs.obu" \
               >/dev/null 2>&1; then
            fail=$((fail + 1)); failed+=("$cell[decode]"); continue
          fi
          want=$(( 2 * (sz * sz + 2 * (((sz + 1) / 2) * ((sz + 1) / 2))) ))
          got=$(wc -c < "$OUT/dec.yuv" | tr -d ' ')
          if [ "$got" -ne "$want" ]; then
            fail=$((fail + 1))
            failed+=("$cell[decoded ${got}B != upscaled ${want}B]")
            continue
          fi
          # (3) recon parity: the port's 10-bit final recon must equal the
          # decoder's output — the u16 normative upscale lands here.
          if ! cmp -s "$OUT/rec.yuv" "$OUT/dec.yuv"; then
            fail=$((fail + 1)); failed+=("$cell[recon!=dec]"); continue
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
echo "superres bd10 identity + conformance + recon: $pass / $((pass + fail)) cells"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[*]}"
fi
if [ "${#vac[@]}" -gt 0 ]; then
  echo "  VACUOUS (superres stream == non-superres stream): ${vac[*]}"
fi
rm -rf "$OUT"
[ "$fail" -eq 0 ] && [ "${#vac[@]}" -eq 0 ]

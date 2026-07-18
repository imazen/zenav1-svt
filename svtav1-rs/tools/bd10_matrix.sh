#!/usr/bin/env bash
# bd10 identity gate: encode uniform content at bit depth 10 with BOTH the Rust
# port (SVTAV1_BD=10 identity_run) and the C reference (capture_c_trace <..> 10)
# and require byte-identical OBUs.
#
# Scope: the bd10-port-map first-cell target — uniform content at preset <= 3
# (M0..M3), where the frame-header params (loop-filter/CDEF levels) are
# bit-depth-independent for flat content, so the coded tile bytes match bd8
# apart from the sequence-header high_bitdepth bit. The port is u8 end-to-end
# (the u16 MD path is chunks 2-4), which is exactly correct for uniform/skip:
# the decoder's DC prediction fills the 10-bit default and the block is skip, so
# no residual is coded and the tile is bit-depth-independent. Faster presets
# (M6/M13) derive LF/CDEF from the bd10 quantizer and are NOT in scope yet.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
read -r -a SIZES <<<"${BD10_SIZES:-64 128}"
read -r -a QPS <<<"${BD10_QPS:-20 40 55}"
read -r -a PRESETS <<<"${BD10_PRESETS:-0 2 3}"
OUT="${TMPDIR:-/tmp}/bd10gate.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()
for sz in "${SIZES[@]}"; do
  for qp in "${QPS[@]}"; do
    for p in "${PRESETS[@]}"; do
      if ! SVTAV1_BD=10 "$HERE/identity_run" uniform "$sz" "$sz" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("u_${sz}_q${qp}_p${p}[rs-err]"); continue
      fi
      if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("u_${sz}_q${qp}_p${p}[c-err]"); continue
      fi
      if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        pass=$((pass + 1))
      else
        fail=$((fail + 1)); failed+=("u_${sz}_q${qp}_p${p}")
      fi
    done
  done
done
rm -rf "$OUT"
echo "bd10 uniform (<=M3) identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

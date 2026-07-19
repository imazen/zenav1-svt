#!/usr/bin/env bash
# bd10 identity gate: encode uniform content at bit depth 10 with BOTH the Rust
# port (SVTAV1_BD=10 identity_run) and the C reference (capture_c_trace <..> 10)
# and require byte-identical OBUs.
#
# Scope: uniform content at ALL tracked presets (M0..M13). The port is u8
# end-to-end (the u16 MD path is a later chunk), which is exactly correct for
# uniform/skip: the decoder's DC prediction fills the 10-bit default and the
# block is skip, so no residual is coded and the tile is bit-depth-independent.
# The one bit-depth-DEPENDENT frame-header param for flat content is the M6+
# LPF_PICK_FROM_Q loop-filter level, now derived at bd10
# (deblock::pick_filter_levels_key_frame's bd10 arm). Non-flat bd10 content
# still needs the u16 MD path (precision-sensitive RD).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
read -r -a SIZES <<<"${BD10_SIZES:-64 128}"
read -r -a QPS <<<"${BD10_QPS:-20 40 55}"
read -r -a PRESETS <<<"${BD10_PRESETS:-0 2 3 6 10 13}"
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
echo "bd10 uniform identity: $pass / $((pass + fail)) byte-identical"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

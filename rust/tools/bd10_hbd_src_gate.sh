#!/usr/bin/env bash
# NATIVE 10-bit SOURCE identity gate (task #6 chunk 2): the port's u16 entry
# points (`try_encode_frame_420_hbd` / `try_encode_frame_hbd`) vs the real C
# encoder, on content whose LOW 2 BITS ARE SET — i.e. a source that is NOT a
# widened 8-bit picture.
#
# Why this is a different gate from `bd10_matrix.sh` / `bd10_nonflat_gate.sh`:
# those feed both encoders `u8 << 2`, so the low 2 bits are zero everywhere and
# a port that silently truncated them would still pass. Here `SVTAV1_HBD_SRC=1`
# makes `identity_run` generate real 10-bit samples, write them to the .yuv the
# C driver reads, AND push the same u16 planes through the port's hbd entry
# point. Byte-identical OBUs then prove the low bits survive end-to-end
# (MD funnel + coded levels + deblock/CDEF/LR searches) in BOTH encoders.
#
# ANTI-VACUITY (enforced below, not just documented): a cell only proves
# something about the u16 path if its real-10-bit stream DIFFERS from the
# widened-u8 stream of the same content. MEASURED 2026-07-24: at qp 55 they
# coincide for every content/size/preset — at that quantizer a +-3/1023
# perturbation is below the quantization step, so the low bits legitimately
# vanish. That is physics, not a port defect, so the rule is per-CONFIGURATION
# rather than per-cell: every (content, size, preset) triple must have AT LEAST
# ONE qp where the low bits change the bitstream. A triple that is vacuous at
# every qp would keep passing with the u16 threading ripped out, and fails the
# gate. Vacuous cells are always listed, never silently counted as coverage.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
read -r -a SIZES <<<"${HBD_SIZES:-64 128}"
read -r -a QPS <<<"${HBD_QPS:-8 20 32 40 55}"
read -r -a PRESETS <<<"${HBD_PRESETS:-6 8 9 10 13}"
read -r -a CONTENTS <<<"${HBD_CONTENTS:-uniform gradient}"
OUT="${TMPDIR:-/tmp}/bd10hbd.$$"
mkdir -p "$OUT"
pass=0
fail=0
vacuous=0
failed=()
vac=()
# Per-(content,size,preset) count of cells whose low bits changed the stream.
declare -A live
for content in "${CONTENTS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        cell="${content}_${sz}_q${qp}_p${p}"
        if ! SVTAV1_BD=10 SVTAV1_HBD_SRC=1 "$HERE/identity_run" \
             "$content" "$sz" "$sz" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
          fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
        fi
        if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
             "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
          fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
        fi
        # The widened-u8 stream of the SAME content, for the anti-vacuity check.
        if ! SVTAV1_BD=10 "$HERE/identity_run" \
             "$content" "$sz" "$sz" "$qp" "$p" "$OUT/w" >/dev/null 2>&1; then
          fail=$((fail + 1)); failed+=("$cell[widen-err]"); continue
        fi
        triple="${content}_${sz}_p${p}"
        : "${live[$triple]:=0}"
        if cmp -s "$OUT/rs.obu" "$OUT/w.obu"; then
          vacuous=$((vacuous + 1)); vac+=("$cell")
        else
          live[$triple]=$(( ${live[$triple]} + 1 ))
        fi
        if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
          pass=$((pass + 1))
        else
          fail=$((fail + 1)); failed+=("$cell")
        fi
      done
    done
  done
done
echo "bd10 NATIVE-10-bit-source identity: $pass / $((pass + fail)) byte-identical"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[*]}"
fi
if [ "$vacuous" -gt 0 ]; then
  echo "  vacuous cells (real-10-bit stream == widened-u8 stream — they verify"
  echo "  parity but do NOT exercise the u16 path): ${vac[*]}"
fi
dead=()
for t in "${!live[@]}"; do
  [ "${live[$t]}" -eq 0 ] && dead+=("$t")
done
if [ "${#dead[@]}" -gt 0 ]; then
  echo "  GATE PREMISE FAILED — these configurations are vacuous at EVERY qp,"
  echo "  so they would pass with the u16 threading removed: ${dead[*]}"
fi
echo "  configurations with at least one low-bits-live qp: $(( ${#live[@]} - ${#dead[@]} )) / ${#live[@]}"
rm -rf "$OUT"
[ "$fail" -eq 0 ] && [ "${#dead[@]}" -eq 0 ]

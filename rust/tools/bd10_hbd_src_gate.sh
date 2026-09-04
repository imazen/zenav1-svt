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
# bash >= 4: this script uses mapfile/readarray/declare -A, which bash 3.2
# (macOS /bin/bash) does not have — there it yields an EMPTY array and the
# gate passes over nothing (docs/WORKING-ON-THIS.md §5). Refuse, loudly.
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
read -r -a SIZES <<<"${HBD_SIZES:-64 128}"
read -r -a QPS <<<"${HBD_QPS:-8 20 32 40 55}"
read -r -a PRESETS <<<"${HBD_PRESETS:-6 8 9 10 13}"
read -r -a CONTENTS <<<"${HBD_CONTENTS:-uniform gradient}"
# PER-ISA cells (docs/SUSPECTED-C-BUGS.md #9). C's own encoded bitstream
# depends on the host ISA, and this gate found a FOURTH corner of that on
# 2026-08-28: three cells where the port matches C-on-x86-64 (green in CI, run
# 33219332305) and differs from C-on-aarch64 by exactly +3 bytes on C's side.
#
#   gradient_64_q8_p6    port 1605 B   C(aarch64) 1608 B
#   gradient_128_q8_p8   port 6120 B   C(aarch64) 6123 B
#   gradient_128_q20_p8  port 3015 B   C(aarch64) 3018 B
#
# The port is NOT the variable side, measured two ways: `tier_invariance.rs`
# holds the port's bytes constant across every archmage dispatch tier, and
# these three cells were re-run against the pre-change tree (bfae1b69) in a
# sibling workspace — byte-identical port output there too, so nothing in the
# port moved. A NEW corner relative to the three already in entry #9: bd10 but
# NOT screen content, NOT preset 7, and the LOW-qp end (8 / 20) rather than 55.
#
# Scoped with `uname -m`, exactly as `screen_palette_bd_gate.sh` scopes its
# two: a flat pin list cannot be right on both hosts — pinning unconditionally
# fails x86-64 (where the cell MATCHES) and not pinning fails aarch64. Pins are
# self-promoting: a pinned cell that starts matching FAILS, so the day C stops
# diverging here the gate says so instead of quietly widening.
KNOWN_DIFF=()
case "$(uname -m)" in
  arm64 | aarch64)
    KNOWN_DIFF+=("gradient_64_q8_p6" "gradient_128_q8_p8" "gradient_128_q20_p8")
    ;;
esac
is_known_diff() {
  local needle=$1 k
  for k in "${KNOWN_DIFF[@]}"; do
    [[ "$k" == "$needle" ]] && return 0
  done
  return 1
}

OUT="${TMPDIR:-/tmp}/bd10hbd.$$"
mkdir -p "$OUT"
pass=0
fail=0
vacuous=0
isa_pinned=0
failed=()
vac=()
isa_pins=()
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
          if is_known_diff "$cell"; then
            # Self-promoting: C stopped diverging on this host, so the pin is
            # stale and must be removed rather than left masking a real cell.
            fail=$((fail + 1)); failed+=("$cell[PER-ISA pin now MATCHES — remove it]")
          else
            pass=$((pass + 1))
          fi
        elif is_known_diff "$cell"; then
          isa_pinned=$((isa_pinned + 1)); isa_pins+=("$cell")
        else
          fail=$((fail + 1)); failed+=("$cell")
        fi
      done
    done
  done
done
# --- PQ tier (corpus-free, so it runs in CI unlike bd10_hbd_pq_gate.sh) ---
#
# The cells above carry a SYNTHETIC low-bit pattern, `(3r + 5c + v) % 4`. These
# carry low bits produced by a REAL transfer curve instead: `SVTAV1_HBD_PQ`
# linearizes the 8-bit luma as sRGB, maps it onto a 1000-nit display, applies
# the SMPTE ST 2084 (PQ) OETF and quantizes to 10-bit LIMITED range; chroma is
# rescaled 8-bit limited -> 10-bit limited. The code-value HISTOGRAM is
# PQ-shaped — dense in the shadows, sparse in the highlights — which a modulo
# pattern cannot produce, and it is the distribution an HDR still actually
# hands an encoder.
#
# WHY IT IS HERE and not only in the photographic gate: no CI runner has the
# image corpora (rust-gates.yml sets ZENAV1_SKIP_CORPUS_TESTS at workflow
# scope), so `tools/bd10_hbd_pq_gate.sh` can only ever run locally. This
# sub-grid is synthetic, therefore corpus-free, therefore the x86-64 reference
# host does see PQ-shaped low bits at every preset band — including preset 6,
# the only one that runs the CDEF strength and Wiener LR searches, where the
# photographic PQ gate has 12 aarch64-scoped pins.
read -r -a PQ_QPS <<<"${HBD_PQ_QPS:-8 20 32}"
read -r -a PQ_PRESETS <<<"${HBD_PQ_PRESETS:-6 8 9}"
for sz in "${SIZES[@]}"; do
  for qp in "${PQ_QPS[@]}"; do
    for p in "${PQ_PRESETS[@]}"; do
      cell="pq_gradient_${sz}_q${qp}_p${p}"
      if ! SVTAV1_BD=10 SVTAV1_HBD_SRC=1 SVTAV1_HBD_PQ=1 "$HERE/identity_run" \
           gradient "$sz" "$sz" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[rs-err]"); continue
      fi
      if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
           "$sz" "$sz" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[c-err]"); continue
      fi
      if ! SVTAV1_BD=10 "$HERE/identity_run" \
           gradient "$sz" "$sz" "$qp" "$p" "$OUT/w" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("$cell[widen-err]"); continue
      fi
      triple="pq_gradient_${sz}_p${p}"
      : "${live[$triple]:=0}"
      if cmp -s "$OUT/rs.obu" "$OUT/w.obu"; then
        vacuous=$((vacuous + 1)); vac+=("$cell")
      else
        live[$triple]=$(( ${live[$triple]} + 1 ))
      fi
      if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        if is_known_diff "$cell"; then
          fail=$((fail + 1)); failed+=("$cell[PER-ISA pin now MATCHES — remove it]")
        else
          pass=$((pass + 1))
        fi
      elif is_known_diff "$cell"; then
        isa_pinned=$((isa_pinned + 1)); isa_pins+=("$cell")
      else
        rs=$(stat -f %z "$OUT/rs.obu" 2>/dev/null || stat -c %s "$OUT/rs.obu")
        cb=$(stat -f %z "$OUT/c.obu" 2>/dev/null || stat -c %s "$OUT/c.obu")
        fail=$((fail + 1)); failed+=("$cell[port=${rs}B C=${cb}B]")
      fi
    done
  done
done

echo "bd10 NATIVE-10-bit-source identity: $pass / $((pass + fail)) byte-identical"
if [ "$fail" -gt 0 ]; then
  printf '  FAILED: %s\n' "${failed[*]}"
fi
if [ "$isa_pinned" -gt 0 ]; then
  echo "  per-ISA pinned (C diverges from itself across hosts, SUSPECTED-C-BUGS #9;"
  echo "  these MATCH C-on-x86-64 in CI): ${isa_pins[*]}"
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

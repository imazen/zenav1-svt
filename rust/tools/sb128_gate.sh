#!/usr/bin/env bash
# SB128 (128x128 superblock) identity gate — task #91.
#
# ============================================================ THE GEOMETRY
# There is NO `super_block_size` field in EbSvtAv1EncConfiguration and no
# `--sb-size` CLI option in SvtAv1EncApp. C DERIVES the superblock size in
# `Globals/enc_handle.c:4071-4111` and the port replays that same rule in
# `sb128_geom::derive_super_block_size`, so both encoders agree with no
# harness flag at all. Two clauses decide every cell:
#
#   1. AREA. `input_resolution == INPUT_SIZE_240p_RANGE` forces 64
#      UNCONDITIONALLY, and that bucket is "aligned luma area <
#      INPUT_SIZE_240p_TH = 0x28500 = 165,120" (Codec/definitions.h:1834,
#      classified on the 8-ALIGNED dims — enc_handle.c:3920 folds the pad in
#      before enc_handle.c:3992 derives the bucket).
#   2. PRESET. In the allintra branch only `enc_mode <= ENC_M1` selects 128.
#
# MEASURED on the real encoder (`Bin/Release/SvtAv1EncApp`, SH bit read back
# with tools/sb128_seqhdr.py):
#      512x384 (196,608 px) preset 0 -> 1     preset 1 -> 1
#      512x384                preset 2 -> 0     preset 3 -> 0
#      256x256  (65,536 px)   preset 0 -> 0    <-- every legacy cell
#      512x320 (163,840 px)   preset 0 -> 0    <-- just under the threshold
#      512x336 (172,032 px)   preset 0 -> 1    <-- just over
#
# So the "cells >= 128px" intuition is WRONG: a 128x128 or 256x256 frame can
# never exercise SB128, because C forces 64 below 165,120 px. Every cell here
# is >= 165,120 aligned luma samples AND preset 0 or 1. Cells are kept just
# over the threshold to hold encode time down.
#
# ============================================================ WHAT IT ASSERTS
#   A. ANTI-VACUITY (hard): every sb128 cell's C oracle really did emit
#      `use_128x128_superblock=1`. Without this the gate could "pass" while
#      silently re-proving the SB64 gate. Mirrors hdr_bd10_gate.sh:145-158.
#   B. CONTROL (hard): a same-preset, same-content cell BELOW the area
#      threshold byte-matches. This is the harness-faithfulness proof — it
#      shows preset 0/1 identity is real, so an sb128 MISMATCH is about the
#      superblock geometry and not about preset 0 being unported.
#   C. BYTE-EXACT (hard): every cell listed in SB128_BYTE_EXACT must `cmp`
#      clean. Adding a cell there means it byte-matches; never add one that
#      merely decodes or falls back.
#   D. PIN (hard, self-promoting): every cell in SB128_PINNED must still
#      DIVERGE. When the SB128 encode path lands, those flip to matching and
#      this gate FAILS — that is the signal to move them into
#      SB128_BYTE_EXACT. Same pattern the sibling aom-rs port uses for its
#      pinned near-ties.
#
# Exit 0 iff A, B, C and D all hold.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/sb128gate.$$"
mkdir -p "$OUT"

pass=0
fail=0
failed=()

# Cells: "content w h qp preset". Every one is >= 165,120 aligned luma px
# and preset <= 1, i.e. SB128 in C (asserted per-cell below, never assumed).
#
#   512x384 = 196,608 px — an EXACT 128 grid (4x3 SBs), no partial SBs.
#   448x384 = 172,032 px — 3.5x3 SBs: a partial 128 COLUMN (448 = 3*128+64),
#                          which is also a whole number of 64s, so it isolates
#                          the SB128 right-edge case from partial-b64 coding.
#   512x448 = 229,376 px — 4x3.5 SBs: a partial 128 ROW.
SB128_CELLS=(
  "uniform  512 384 32 0"
  "uniform  512 384 55 0"
  "uniform  512 384 63 0"
  "uniform  512 384 32 1"
  "gradient 512 384 32 0"
  "gradient 512 384 55 0"
  "gradient 512 384 20 0"
  "gradient 512 384 32 1"
  "diag     512 384 32 0"
  "diag     512 384 55 0"
  "uniform  448 384 32 0"
  "gradient 448 384 32 0"
  "uniform  512 448 32 0"
  "gradient 512 448 32 0"
)

# Cells that BYTE-MATCH today. A cell only ever moves here after `cmp` says
# so. Landed by task #91 chunk 3 (the 128 partition root + the b64
# coding-unit walk):
#   - all four `uniform 512x384` cells (q32/q55/q63, p0 AND p1) — the exact
#     4x3 SB128 grid, no partial SBs;
#   - `diag 512x384 q32 p0` — TEXTURED content, so the forced-SPLIT 128 root
#     is not a uniform-only artefact;
#   - `uniform 448x384 q32 p0` — a partial 128 COLUMN (448 = 3*128 + 64), so
#     the right-edge `has_cols == false` binary partition arm (the
#     H4/V4-free `partition_gather_horz_alike` with is_128) is exercised;
#   - `uniform 512x448 q32 p0` — a partial 128 ROW, the `has_rows == false`
#     vert_alike counterpart.
SB128_BYTE_EXACT=(
  "uniform  512 384 32 0"
  "uniform  512 384 55 0"
  "uniform  512 384 63 0"
  "uniform  512 384 32 1"
  "diag     512 384 32 0"
  "uniform  448 384 32 0"
  "uniform  512 448 32 0"
)

# Cells PINNED as still-diverging. Everything in SB128_CELLS that is not in
# SB128_BYTE_EXACT is implicitly pinned (see the loop) — the pin is what
# makes this gate fail-forward when the port starts matching.

# CONTROL cells: same presets and content, BELOW the area threshold, so C
# codes them at SB64 and the port must byte-match them exactly.
CONTROL_CELLS=(
  "uniform  512 320 32 0"
  "gradient 512 320 32 0"
  "gradient 512 320 32 1"
  "gradient 256 256 32 0"
)

in_list() {
  local needle="$1"; shift
  local e
  for e in "$@"; do [ "$e" = "$needle" ] && return 0; done
  return 1
}

# ---------------------------------------------------------------- CONTROL
echo "--- control cells (below the 165,120px threshold -> C codes SB64) ---"
for cell in "${CONTROL_CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="ctl_${content}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi
  # The control must be SB64 on BOTH sides, or it is not a control.
  sb=$(python3 "$HERE/sb128_seqhdr.py" "$OUT/c.obu" | grep -o 'use_128x128_superblock=[01]' | cut -d= -f2)
  if [ "$sb" != "0" ]; then
    fail=$((fail + 1)); failed+=("$tag[control-is-sb128-not-a-control]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1)); echo "  OK       $tag (sb64)"
  else
    fail=$((fail + 1)); failed+=("$tag[control-MISMATCH]")
    echo "  MISMATCH $tag  <-- harness/preset regression, not an sb128 issue"
  fi
done

# ------------------------------------------------------------- SB128 CELLS
echo "--- sb128 cells (>= 165,120px, preset <= 1 -> C codes SB128) ---"
for cell in "${SB128_CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi

  # (A) ANTI-VACUITY — the oracle must genuinely be an SB128 encode.
  sb=$(python3 "$HERE/sb128_seqhdr.py" "$OUT/c.obu" | grep -o 'use_128x128_superblock=[01]' | cut -d= -f2)
  if [ "$sb" != "1" ]; then
    fail=$((fail + 1)); failed+=("$tag[VACUOUS: C emitted sb64, cell proves nothing]")
    echo "  VACUOUS  $tag"
    continue
  fi

  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    if in_list "$cell" "${SB128_BYTE_EXACT[@]+"${SB128_BYTE_EXACT[@]}"}"; then
      pass=$((pass + 1)); echo "  OK       $tag (sb128 byte-exact)"
    else
      # (D) the self-promoting pin fired: this is GOOD news that must not be
      # silently absorbed. Fail loudly so the cell gets promoted.
      fail=$((fail + 1)); failed+=("$tag[PIN-BROKEN: now byte-exact -> move to SB128_BYTE_EXACT]")
      echo "  PROMOTE  $tag  <-- now byte-exact! add it to SB128_BYTE_EXACT"
    fi
  else
    if in_list "$cell" "${SB128_BYTE_EXACT[@]+"${SB128_BYTE_EXACT[@]}"}"; then
      fail=$((fail + 1)); failed+=("$tag[REGRESSION: was byte-exact]")
      echo "  REGRESS  $tag"
    else
      pass=$((pass + 1))
      cb=$(stat -c%s "$OUT/c.obu"); rb=$(stat -c%s "$OUT/rs.obu")
      echo "  pinned   $tag (C=${cb}B port=${rb}B — sb128 path unported)"
    fi
  fi
done

rm -rf "$OUT"
total=$((pass + fail))
echo
echo "sb128 gate: $pass / $total"
if [ "$fail" -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
fi
[ "$fail" -eq 0 ]

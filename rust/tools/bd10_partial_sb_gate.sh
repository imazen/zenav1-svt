#!/usr/bin/env bash
# bd10 PARTIAL-SUPERBLOCK identity gate — 10-bit encodes whose ALIGNED dims are
# NOT a multiple of 64, byte-compared against the real C encoder at bd10.
#
# WHY THIS GATE EXISTS. Until 2026-08-04 every one of these cells was REFUSED:
#
#   "10-bit requires 64-aligned encode dimensions: no bd10 producer is
#    partial-SB aware, so the encode would be 8-bit-quantized under a 10-bit
#    sequence header"
#
# which is the product case for 10-bit AVIF (real images are not 64-aligned).
# The two existing bd10 gates could not have caught the regression either way:
# `bd10_matrix.sh` sweeps `BD10_SIZES=64 128` and `bd10_nonflat_gate.sh` only
# 64x64/128x128, so NO bd10 gate reached a partial superblock at all. This one
# does nothing else.
#
# WHAT MADE IT WORK (measured, not assumed). The bd10 FULL-RD funnel rides the
# SAME leaf funnel and the SAME partition search as the 8-bit path, which is
# already partial-SB correct (`partial_sb_gate.sh`, 146/146). Specifically it
# inherits: the PD0 edge predicates + forced split, the edge-aware PD1
# depth-refinement walk, the single injected shape at a one-false node, the
# cropped-TX RD distortion (whose bd10 twin `TxRdArgs::crop` was already fed the
# same `blk_crop`/`uv_crop`), the SB-extent-sized recon canvases, and
# `commit_leaf`'s straddle clip — which is applied to the bd10 canvases exactly
# as to the u8 ones. The claim "`tx_unit_hbd` is not partial-SB-aware" that the
# refusal rested on was wrong: that function takes explicit `(w, h, stride,
# off)` and its only geometry term is the crop, which is already wired.
#
# SCOPE — presets 0..8 only. Preset >= 9 (eff-M9) is STILL REFUSED at partial
# SB and that refusal is gated at the bottom of this file, self-promotingly: its
# only level producer is the level-only re-encode post-pass, which is genuinely
# not partial-SB aware (ALIGNED-sized `recon10` buffers, unclipped straddle
# writes, and a fixed `(partition_type, children.len())` child-offset table that
# a pruned / tail-truncated partial-SB child list does not satisfy).
#
# ISA NOTE. Cells are validated on the host that added them. C itself emits
# different bytes for the same input on different architectures for a small
# number of cells (docs/SUSPECTED-C-BUGS.md #9), so a cell here is a
# per-architecture claim, exactly like `partial_sb_gate.sh`'s ISA-scoped cell.
#
# Usage: tools/bd10_partial_sb_gate.sh
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/bd10partialsb.$$"
mkdir -p "$OUT"
pass=0
fail=0
failed=()

# Each cell: "content w h qp preset". Every one has a partial superblock
# (aligned dims not a multiple of 64) and every one BYTE-MATCHES C at bd10.
# The byte count in the comment is the measured stream size — it is what makes
# a vacuous cell visible: a 20-something-byte uniform cell proves the GEOMETRY
# path, a four-figure gradient cell proves the 10-bit LEVEL path.
CELLS=(
  # ---- 96x80: the primary partial-SB milestone (right edge at SB(0,1),
  # bottom edge at SB(1,0), both-false corner at SB(1,1)), now at bd10.
  "uniform 96 80 20 0"     # 24 B
  "uniform 96 80 20 8"     # 28 B
  "uniform 96 80 55 0"     # 24 B
  "uniform 96 80 55 8"     # 28 B
  "gradient 96 80 20 0"    # 1617 B
  "gradient 96 80 20 6"    # 1686 B
  "gradient 96 80 32 0"    # 830 B
  "gradient 96 80 32 4"    # 854 B
  "gradient 96 80 32 6"    # 882 B  <- the anti-vacuity anchor, see below
  "gradient 96 80 55 0"    # 94 B
  "gradient 96 80 55 2"    # 94 B
  "gradient 96 80 55 4"    # 94 B
  "gradient 96 80 55 6"    # 95 B
  # ---- right edge only
  "uniform 96 64 20 0"     # 23 B
  "uniform 96 64 20 8"     # 24 B
  "uniform 96 64 55 0"     # 23 B
  "uniform 96 64 55 8"     # 24 B
  "gradient 96 64 20 0"    # 1284 B
  "gradient 96 64 20 4"    # 1273 B
  "gradient 96 64 20 6"    # 1269 B
  "gradient 96 64 20 8"    # 1266 B
  "gradient 96 64 32 0"    # 715 B
  "gradient 96 64 32 6"    # 776 B
  "gradient 96 64 32 8"    # 772 B
  "gradient 96 64 55 2"    # 82 B
  "gradient 96 64 55 4"    # 82 B
  "gradient 96 64 55 6"    # 88 B
  "gradient 96 64 55 8"    # 91 B
  # ---- bottom edge only
  "uniform 64 80 20 0"     # 23 B
  "uniform 64 80 20 8"     # 26 B
  "uniform 64 80 55 0"     # 23 B
  "uniform 64 80 55 8"     # 26 B
  "gradient 64 80 20 0"    # 1091 B
  "gradient 64 80 20 6"    # 1131 B
  "gradient 64 80 32 0"    # 570 B
  "gradient 64 80 32 2"    # 566 B
  "gradient 64 80 32 4"    # 587 B
  "gradient 64 80 32 6"    # 603 B
  "gradient 64 80 55 0"    # 68 B
  "gradient 64 80 55 2"    # 68 B
  "gradient 64 80 55 4"    # 68 B
  "gradient 64 80 55 6"    # 68 B
  "gradient 64 80 55 8"    # 72 B
  # ---- both edges, 8-aligned (single partial SB, straddling leaves)
  "uniform 72 72 20 0"     # 24 B
  "uniform 72 72 20 8"     # 32 B
  "uniform 72 72 55 0"     # 24 B
  "uniform 72 72 55 8"     # 32 B
  "gradient 72 72 20 0"    # 1087 B
  "gradient 72 72 20 2"    # 1134 B
  "gradient 72 72 20 6"    # 1160 B
  "gradient 72 72 32 0"    # 548 B
  "gradient 72 72 32 2"    # 561 B
  "gradient 72 72 32 6"    # 601 B
  "gradient 72 72 55 0"    # 83 B
  "gradient 72 72 55 2"    # 83 B
  "gradient 72 72 55 4"    # 82 B
  "gradient 72 72 55 6"    # 81 B
  "gradient 72 72 55 8"    # 90 B
  # ---- ODD true dims (65x65 -> aligned 72x72): true-dim seq-header size bits
  # + the ceiling-chroma harness convention, at bd10.
  "uniform 65 65 20 0"     # 24 B
  "uniform 65 65 20 8"     # 32 B
  "uniform 65 65 55 0"     # 24 B
  "uniform 65 65 55 8"     # 32 B
  "gradient 65 65 20 6"    # 986 B
  "gradient 65 65 20 8"    # 998 B
  "gradient 65 65 32 0"    # 533 B
  "gradient 65 65 32 4"    # 531 B
  "gradient 65 65 32 6"    # 583 B
  "gradient 65 65 32 8"    # 585 B
  "gradient 65 65 55 0"    # 73 B
  "gradient 65 65 55 6"    # 80 B
  "gradient 65 65 55 8"    # 87 B
  # ---- sub-64 single partial SB
  "uniform 48 48 20 0"     # 21 B
  "uniform 48 48 20 8"     # 22 B
  "uniform 48 48 55 0"     # 21 B
  "uniform 48 48 55 8"     # 22 B
  "gradient 48 48 20 6"    # 548 B
  "gradient 48 48 32 0"    # 299 B
  "gradient 48 48 32 6"    # 322 B
  "gradient 48 48 32 8"    # 329 B
  "gradient 48 48 55 0"    # 64 B
  "gradient 48 48 55 2"    # 64 B
  "gradient 48 48 55 4"    # 60 B
  "gradient 48 48 55 6"    # 62 B
  "gradient 48 48 55 8"    # 63 B
  # ---- the straddle-WIN geometries (C keeps a straddling boundary block as a
  # leaf) — the u8 gate's cropped-TX anchors, now exercised at 10 bits.
  "uniform 80 88 20 0"     # 24 B
  "uniform 80 88 20 8"     # 29 B
  "uniform 80 88 55 0"     # 24 B
  "uniform 80 88 55 8"     # 29 B
  "gradient 80 88 20 0"    # 1319 B
  "gradient 80 88 20 6"    # 1508 B
  "gradient 80 88 55 0"    # 106 B
  "gradient 80 88 55 6"    # 109 B
  "uniform 72 88 20 0"     # 24 B
  "uniform 72 88 20 8"     # 30 B
  "uniform 72 88 55 0"     # 24 B
  "uniform 72 88 55 8"     # 30 B
  "gradient 72 88 20 0"    # 1204 B
  "gradient 72 88 20 6"    # 1376 B
  "gradient 72 88 55 0"    # 95 B
  "gradient 72 88 55 4"    # 95 B
  "gradient 72 88 55 6"    # 98 B
  # ---- multi-SB
  "uniform 120 120 20 0"   # 24 B
  "uniform 120 120 20 8"   # 25 B
  "uniform 120 120 55 0"   # 24 B
  "uniform 120 120 55 8"   # 25 B
  "gradient 120 120 20 0"  # 2355 B
  "gradient 120 120 20 4"  # 2625 B
  "gradient 120 120 20 6"  # 2773 B
  "uniform 200 120 20 6"   # 27 B
  "uniform 200 120 20 8"   # 33 B
  "uniform 200 120 55 6"   # 27 B
  "uniform 200 120 55 8"   # 33 B
  "gradient 200 120 20 6"  # 4482 B
  # ---- a REAL image size (512x481 -> aligned 512x488, bottom partial row)
  "uniform 512 481 20 6"   # 38 B
  "uniform 512 481 20 8"   # 38 B
  "uniform 512 481 55 6"   # 38 B
  "uniform 512 481 55 8"   # 38 B
  "gradient 512 481 20 6"  # 18021 B
  "gradient 512 481 55 6"  # 1427 B
)

for cell in "${CELLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! SVTAV1_BD=10 "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
      "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("${tag}[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1)); failed+=("$tag")
  fi
done

# --- ANTI-VACUITY --------------------------------------------------------
# A gate that would pass without the feature is a defect. Two checks:
#
# 1. Every cell above has ALIGNED dims that are not a multiple of 64, i.e. a
#    real partial superblock. Assert it here rather than trusting the list —
#    a cell silently edited to 64x64 would otherwise keep this green while
#    testing nothing this file is named for.
# 2. At least 40 cells must produce a stream LARGER than 200 bytes. A flat
#    (uniform) bd10 frame is all-skip and its tile payload is bit-depth
#    independent, so a gate made only of uniform cells would pass with the
#    bd10 level path deleted entirely.
vac=0
for cell in "${CELLS[@]}"; do
  read -r _content w h _qp _p <<<"$cell"
  aw=$(((w + 7) / 8 * 8))
  ah=$(((h + 7) / 8 * 8))
  if [ $((aw % 64)) -eq 0 ] && [ $((ah % 64)) -eq 0 ]; then
    echo "ANTI-VACUITY FAIL: cell '$cell' has NO partial superblock (aligned ${aw}x${ah})"
    vac=1
  fi
done
big=$(printf '%s\n' "${CELLS[@]}" | grep -c '^gradient')
if [ "$big" -lt 40 ]; then
  echo "ANTI-VACUITY FAIL: only $big non-flat cells; a uniform-only bd10 gate is vacuous"
  vac=1
fi

# --- THE STILL-REFUSED BAND (self-promoting pin) --------------------------
# preset >= 9 at a partial SB must STILL be refused (exit 3 from identity_run's
# unwrap_or_refuse). If someone makes the level-only post-pass partial-SB aware,
# these start ENCODING and this block fails until the cells are promoted into
# CELLS above — the same self-promoting discipline identity_full_8bit.sh uses.
REFUSED=(
  "gradient 96 80 32 9"
  "gradient 96 80 32 10"
  "gradient 65 65 32 13"
  "gradient 512 481 20 9"
)
refused_ok=0
refused_bad=()
for cell in "${REFUSED[@]}"; do
  read -r content w h qp p <<<"$cell"
  SVTAV1_BD=10 "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1
  rc=$?
  if [ "$rc" -eq 3 ]; then
    refused_ok=$((refused_ok + 1))
  else
    refused_bad+=("${content}_${w}x${h}_q${qp}_p${p}[rc=$rc]")
  fi
done

rm -rf "$OUT"
echo "bd10 partial-SB identity: $pass / $((pass + fail)) byte-identical"
echo "bd10 partial-SB preset>=9 still refused: $refused_ok / ${#REFUSED[@]}"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "${#refused_bad[@]}" -gt 0 ] && printf 'NO LONGER REFUSED (promote it into CELLS): %s\n' "${refused_bad[@]}"
[ "$fail" -eq 0 ] && [ "$vac" -eq 0 ] && [ "${#refused_bad[@]}" -eq 0 ]

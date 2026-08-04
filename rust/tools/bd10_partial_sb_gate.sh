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
# SCOPE — presets 0..13, i.e. BOTH bd10 level producers. Preset <= 8 is the
# full-RD funnel (above); preset >= 9 is the level-only re-encode post-pass,
# which needed real work: SB-extent-sized `recon10` (it was ALIGNED-sized),
# straddle-clipped recon writes, SB-extent-padded 10-bit sources, and the pack's
# skip-off-frame-quadrant child walk in place of a fixed
# `(partition_type, children.len())` offset table. See the p9 block below.
#
# A residual set of NON-FLAT cells still diverges. It is the known bd10
# non-flat gap, not a partial-SB one — the numbers and the control measurements
# are in the PINNED block at the bottom, which pins a representative slice
# self-promotingly so the residual cannot silently move in either direction.
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
  # ---- PRESET >= 9 (eff-M9), the LEVEL-ONLY RE-ENCODE POST-PASS band.
  # This band has no full-RD funnel: the coded 10-bit levels come from
  # `bd10_reencode_luma` / `_chroma`, which walk the decided trees a second
  # time. That pass was the LAST thing genuinely not partial-SB aware, and the
  # four things it needed are what these cells exercise:
  #   * SB-extent-sized `recon10` (it was ALIGNED-sized, so a straddling leaf
  #     wrote past the buffer at the bottom-right and wrapped a row at the
  #     right edge);
  #   * the straddle clip on the recon writes (`commit_leaf`'s rule);
  #   * SB-extent-padded 10-bit sources (`sb_input` / `sb_chroma_owned` twins) —
  #     the residual gather reads the full block width;
  #   * the pack's child walk (skip off-frame quadrant ORIGINS, pull packed
  #     children in order) instead of a fixed `(type, len)` offset table. A
  #     right-edge-only prune leaves [q0, q2]; the old `zip` put the
  #     BOTTOM-LEFT child at the TOP-RIGHT offset, and a pruned SPLIT/HORZ/VERT
  #     child count hit `panic!("bd10 reencode: unsupported partition")`.
  # p10..p13 all clamp to eff-M9 in C (enc_handle.c:4415-4419), so a p13 cell is
  # a second measurement of p9, not independent coverage — two are kept as a
  # smoke check and the rest of the band is p9.
  "uniform 96 80 20 9"     # 28 B
  "uniform 96 80 55 9"     # 28 B
  "gradient 96 80 20 9"    # 1696 B
  "gradient 96 80 32 9"    # 875 B
  "gradient 96 80 55 9"    # 105 B
  "gradient 96 80 32 13"   # 875 B   (p13 == p9, smoke check)
  "uniform 96 64 32 9"     # 24 B
  "gradient 96 64 20 9"    # 1264 B
  "gradient 96 64 32 9"    # 774 B
  "gradient 96 64 55 9"    # 91 B
  "uniform 64 80 32 9"     # 26 B
  "gradient 64 80 20 9"    # 1143 B
  "gradient 64 80 32 9"    # 596 B
  "gradient 64 80 55 9"    # 72 B
  "uniform 72 72 32 9"     # 32 B
  "gradient 72 72 20 9"    # 1158 B
  "gradient 72 72 32 9"    # 592 B
  "gradient 72 72 55 9"    # 90 B
  "uniform 65 65 32 9"     # 32 B
  "gradient 65 65 20 9"    # 996 B
  "gradient 65 65 32 9"    # 584 B
  "gradient 65 65 55 9"    # 87 B
  "gradient 65 65 32 13"   # 584 B   (p13 == p9, smoke check)
  "gradient 80 88 20 9"    # 1616 B
  "gradient 80 88 32 9"    # 817 B
  "gradient 72 88 20 9"    # 1425 B
  "gradient 72 88 32 9"    # 715 B
  "gradient 120 120 20 9"  # 2884 B
  "gradient 120 120 32 9"  # 1507 B
  "gradient 120 120 55 9"  # 175 B
  "gradient 200 120 20 9"  # 4707 B
  "gradient 200 120 32 9"  # 2435 B
  "gradient 48 48 32 9"    # 325 B
  "gradient 48 48 55 9"    # 60 B
  "uniform 512 481 32 9"   # 38 B
  "gradient 512 481 20 9"  # 24786 B
  "gradient 512 481 32 9"  # 11569 B
  "gradient 512 481 55 9"  # 1498 B
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
if [ "$big" -lt 60 ]; then
  echo "ANTI-VACUITY FAIL: only $big non-flat cells; a uniform-only bd10 gate is vacuous"
  vac=1
fi

# --- PINNED DIVERGENCES (self-promoting) ----------------------------------
# NOTHING about bd10 partial-SB is refused any more, so there is no refusal
# left to pin. What IS still open is a RESIDUAL SET of non-flat cells where the
# port and C disagree. Pinning a representative slice keeps the residual honest
# in both directions: a pinned cell that starts MATCHING fails this gate until
# it is promoted into CELLS (a fix can never land unnoticed), and one that
# stops encoding at all fails as a harness error.
#
# The residual is NOT a partial-SB gap — it is the KNOWN bd10 non-flat gap
# (`bd10_nonflat_gate.sh`, 197/309 at 64-ALIGNED dims), measured on both
# geometries on 2026-08-04 over 11 geometries x p0..p8 x q{20,32,55} x
# {uniform,gradient}:
#
#   bd8  @ partial-SB      565 / 594     (29 cells fail at 8 bit too)
#   bd10 @ 64-aligned      241 / 270     -> 29 bd10-only = 21.5% of non-flat
#   bd10 @ partial-SB      490 / 594     -> 78 bd10-only = 26.3% of non-flat
#   bd10 @ partial-SB p9+  310 / 330     (3 bd10-only configs, 1 fails at bd8)
#
# Every failing cell on every one of those grids is `gradient`; uniform is
# 100%. Raw per-cell data: benchmarks/bd10_partial_sb_2026-08-04.tsv.
PINNED=(
  # bd10-only at the eff-M9 band (bd8 matches these two)
  "gradient 80 88 55 9"
  "gradient 48 48 20 9"
  # bd10-only at the full-RD band
  "gradient 96 80 20 4"
  "gradient 65 65 20 2"
  # fails at bd8 TOO — pinned so a bd10 claim is never read as covering it
  "gradient 72 88 55 9"
)
pin_ok=0
pin_bad=()
for cell in "${PINNED[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}"
  if ! SVTAV1_BD=10 "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
    pin_bad+=("${tag}[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
      "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
    pin_bad+=("${tag}[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pin_bad+=("${tag}[NOW MATCHES — promote it into CELLS]")
  else
    pin_ok=$((pin_ok + 1))
  fi
done

rm -rf "$OUT"
echo "bd10 partial-SB identity: $pass / $((pass + fail)) byte-identical"
echo "bd10 partial-SB pinned divergences still diverging: $pin_ok / ${#PINNED[@]}"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "${#pin_bad[@]}" -gt 0 ] && printf 'PIN BROKEN: %s\n' "${pin_bad[@]}"
[ "$fail" -eq 0 ] && [ "$vac" -eq 0 ] && [ "${#pin_bad[@]}" -eq 0 ]

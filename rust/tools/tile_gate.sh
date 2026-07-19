#!/usr/bin/env bash
# Tile-configuration gate (task #96) — the acceptance-criteria axis
# "every tile configuration". Until this landed there was no tile test of
# any kind: every other gate encodes a single tile, so both halves of the
# tile grid were entirely unmeasured.
#
# Drives BOTH encoders with a matched tile request — the port via
# SVTAV1_TILE_ROWS_LOG2 / SVTAV1_TILE_COLS_LOG2, the C reference via
# SVT_TILE_ROWS / SVT_TILE_COLUMNS (both are the log2 domain of
# cfg.tile_rows / cfg.tile_columns, EbSvtAv1Enc.h:607-611) — and applies
# FIVE asserts per cell:
#
#   (A) ANTI-VACUITY. The C oracle's bytes at this tile request must
#       DIFFER from the C oracle's bytes at rows=cols=0 on the same
#       input. Both log2s are CLAMPED to what the frame geometry supports
#       (svt_aom_set_tile_info), so a request the geometry cannot honour
#       silently produces a single-tile encode — and a cell comparing two
#       single-tile encodes proves nothing about tiling however
#       impressively the request is spelled. This is the check that makes
#       the gate about tiles.
#
#   (E) DECODABILITY. aomdec must accept the PORT's stream, byte-match or
#       not. This is not belt-and-braces: a byte gate is structurally
#       BLIND to corruption among expected-DIFF cells, and this axis shipped
#       exactly that bug — the pre-#96 rows path wrote
#       `context_update_tile_id = (1<<log2)-1` where C writes
#       `NumTiles-1`, so every frame whose SB-row count did not divide by
#       the tile count was REJECTED by conforming decoders ("Invalid
#       context_update_tile"). It was invisible because no gate decoded a
#       multi-tile stream. `512x384 r2` below is that exact cell.
#
#   (C) CONTROL. The single-tile encode of each geometry must still
#       byte-match. Tile_info() sits in the frame header of EVERY cell the
#       whole project encodes, so a tile-syntax regression would land on
#       all 7 other gates at once; this catches it here first.
#
#   (B) BYTE-EXACT cells are asserted byte-identical to C.
#
#   (D) DIVERGING cells are PINNED SELF-PROMOTING: a pinned cell that
#       starts matching FAILS the gate, so the improvement is noticed and
#       the cell gets promoted instead of silently absorbed.
#
# Usage: tile_gate.sh
# Env:   AOMDEC=/path/to/aomdec (autodetected)
#
# The full sweep behind the cell choices is tools/tile_map.sh, whose
# scoreboard lives at benchmarks/tile_map_latest.tsv.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/tilegate.$$"
mkdir -p "$OUT"

pass=0
fail=0
failed=()

aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  for cand in /root/aomdec-build/aomdec /root/aomdec-debug/aomdec \
              /root/aom-rs/upstream/build/aomdec; do
    [ -x "$cand" ] && { aomdec="$cand"; break; }
  done
fi
command -v "$aomdec" >/dev/null 2>&1 || [ -x "$aomdec" ] || {
  echo "WARNING: aomdec not found (set AOMDEC=...) — assert (E) DECODABILITY is SKIPPED" >&2
  aomdec=""
}
[ -n "$aomdec" ] && echo "decodability check: $aomdec"

# Geometries, chosen so the grid is genuinely exercised (a tile is at
# least one SB, and the tile grid is counted in SBs):
#
#   256x256 -> 4x4 SBs   both axes divide by every log2 in range — the
#                        clean power-of-two grid. rows_log2=2 gives 4
#                        tiles of 1 SB row; with cols_log2=2 that is 16
#                        one-SB tiles, the maximum decomposition here.
#   512x384 -> 8x6 SBs   6 does NOT divide by 4: C's algorithm gives a
#                        2-SB tile height and therefore THREE tile rows
#                        at rows_log2=2, not four. This is the geometry
#                        that produced the out-of-range
#                        context_update_tile_id.
#   640x448 -> 10x7 SBs  neither axis divides — ragged on BOTH axes at
#                        once. At log2=2 on both: width ceil(10/4)=3 SBs
#                        -> ceil(10/3)=4 columns, height ceil(7/4)=2 SBs
#                        -> ceil(7/2)=4 rows, with the last tile of each
#                        axis short (1 SB wide, 1 SB high).
#
# Cells: "content w h qp preset rows_log2 cols_log2".
CELLS=(
  # --- 256x256, the clean grid ---
  "gradient 256 256 45 6  0 1"
  "gradient 256 256 45 6  0 2"
  "gradient 256 256 45 6  1 0"
  "gradient 256 256 45 6  1 1"
  "gradient 256 256 45 6  1 2"
  "gradient 256 256 45 6  2 0"
  "gradient 256 256 45 6  2 1"
  "gradient 256 256 45 6  2 2"
  "gradient 256 256 45 10 1 0"
  "gradient 256 256 45 10 2 0"
  "gradient 256 256 45 10 2 2"
  "gradient 256 256 45 13 2 2"
  "gradient 256 256 20 6  1 0"
  "gradient 256 256 20 6  2 2"
  # --- 512x384, the ragged-ROWS geometry (the ex-corruption cell) ---
  "gradient 512 384 45 6  0 1"
  "gradient 512 384 45 6  1 0"
  "gradient 512 384 45 6  2 0"
  "gradient 512 384 45 6  2 2"
  "gradient 512 384 20 6  2 0"
  # --- 640x448, ragged on BOTH axes ---
  "gradient 640 448 45 6  1 1"
  "gradient 640 448 45 6  2 2"
  "gradient 640 448 45 10 2 1"
)

# Cells that BYTE-MATCH the C reference today. A cell only ever moves here
# after `cmp` says so (measured by tools/tile_map.sh, 162-cell sweep).
#
# 18 of the 22 tile cells, across all three geometries and both axes,
# including `512x384 r2` — the ragged-rows geometry whose tile count is 3
# where `1<<log2` is 4, and the exact cell that used to be UNDECODABLE.
BYTE_EXACT=(
  "gradient 256 256 45 6  0 1"
  "gradient 256 256 45 6  0 2"
  "gradient 256 256 45 6  1 0"
  "gradient 256 256 45 6  1 1"
  "gradient 256 256 45 6  1 2"
  "gradient 256 256 45 6  2 0"
  "gradient 256 256 45 6  2 1"
  "gradient 256 256 45 6  2 2"
  "gradient 256 256 45 10 1 0"
  "gradient 256 256 45 10 2 0"
  "gradient 256 256 45 10 2 2"
  "gradient 256 256 45 13 2 2"
  "gradient 256 256 20 6  1 0"
  "gradient 512 384 45 6  0 1"
  "gradient 512 384 45 6  2 0"
  "gradient 512 384 45 6  2 2"
  "gradient 512 384 20 6  2 0"
  "gradient 640 448 45 10 2 1"
)

# ---------------------------------------------------------------------------
# WHY THE REMAINING 4 CELLS DIVERGE — measured, not guessed.
#
# The frame header is NOT the problem, and never was: identity_diff
# classifies every diverging cell's first divergence as `tile payload`,
# never SH or FH. tile_info() — uniform_tile_spacing_flag, both increment
# runs, context_update_tile_id over the ACTUAL tile count,
# tile_size_bytes_minus_1 — and the tile group's size prefixes are
# byte-correct across the entire 162-cell sweep.
#
# The dominant root WAS that the MD search predicted across tile
# boundaries, and that is now fixed: `intra_edge::TileMi` carries the
# tile's mi rect into the leaf funnel's UnitGeom and DrGeom, so all four
# of C's tile-scoped availability predicates (`have_top`, `have_left`,
# `right_available`, `bottom_available`) plus the funnel's neighbour
# extraction and its tx-level canvases now stop at the tile edge instead
# of reading the 128 fill. That promoted 12 of the 22 cells in one change.
#
# What is left is 4 cells, and they are NOT one thing:
#
#   * `512x384 q45 p6 r1c0` — the outlier, and the only LARGE gap
#     (C=3211B vs port=3042B; every other divergence is under 60 bytes).
#     C spends ~170 bytes more than its OWN single-tile encode of the same
#     input (3017B) while the port spends ~25 more, which is the signature
#     of a whole-frame restoration-type flip rather than scattered MD
#     drift. Worth noting what it is NOT: SVT's LR search is entirely
#     tile-independent — `svt_aom_foreach_rest_unit_in_frame` calls
#     `on_tile(0,0)` exactly once over `whole_frame_rect`
#     (restoration.c:1274-1297) and the stripe derivation's tile loop is
#     literally commented out (`for i < 1 /*cm->tile_rows*/`,
#     restoration.c:1699). So the LR CONFIG cannot depend on tiles; the
#     recon fed to the search must still differ somewhere. Unroot-caused.
#
#   * `640x448 r1c1`, `640x448 r2c2`, `256x256 q20 r2c2` — small
#     residuals (7-54 bytes) on the ragged-both-axes geometry and at the
#     low qp where far more blocks are coded. Ordinary near-tie drift of
#     the kind the other gates' pins also carry, plus whatever the
#     remaining non-tile-aware corners of the search are: the per-tile MD
#     recon canvas is still frame-SIZED and 128-filled rather than
#     tile-cropped, and the bd10 re-encode path is explicitly whole-frame
#     (see the PORT-NOTEs in pipeline.rs).
#
# The first DIVERGING op in these cells is usually `lr-taps`, which is a
# SYMPTOM and not the root: LR syntax is written before each SB's
# partition tree, so any recon difference anywhere in the frame reprices
# every unit's taps and surfaces at the very first coded op.
# ---------------------------------------------------------------------------

# CONTROLS: the same geometries at rows=cols=0. These must byte-match —
# they are the harness-faithfulness witness AND the regression guard for
# tile_info() bits that every other gate's cells also carry.
CONTROLS=(
  "gradient 256 256 45 6"
  "gradient 512 384 45 6"
  "gradient 640 448 45 6"
)

in_list() {
  local needle="$1"; shift
  local e
  for e in "$@"; do [ "$e" = "$needle" ] && return 0; done
  return 1
}

# ---------------------------------------------------------------- CONTROL
echo "--- control cells (rows=cols=0 -> single tile; must byte-match) ---"
for cell in "${CONTROLS[@]}"; do
  read -r content w h qp p <<<"$cell"
  tag="ctl_${content}_${w}x${h}_q${qp}_p${p}"
  if ! "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" \
        >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi
  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    pass=$((pass + 1)); echo "  OK       $tag"
  else
    fail=$((fail + 1)); failed+=("$tag[control-MISMATCH]")
    echo "  MISMATCH $tag  <-- single-tile regression, NOT a tile-grid issue"
  fi
done

# ------------------------------------------------------------- TILE CELLS
echo "--- tile cells (rows_log2 x cols_log2) ---"
for cell in "${CELLS[@]}"; do
  read -r content w h qp p r c <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}_r${r}c${c}"

  if ! SVTAV1_TILE_ROWS_LOG2="$r" SVTAV1_TILE_COLS_LOG2="$c" \
       "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" \
       >"$OUT/rs.log" 2>"$OUT/rs.trace"; then
    fail=$((fail + 1)); failed+=("$tag[rs-err]"); continue
  fi
  if ! SVT_TILE_ROWS="$r" SVT_TILE_COLUMNS="$c" SVT_TRACE_OUT=/dev/null \
       "$HERE/capture_c_trace/capture_c_trace" \
       "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" >"$OUT/c.log" 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c-err]"); continue
  fi

  # (A) ANTI-VACUITY — the tile request must have changed the C encode.
  if ! SVT_TILE_ROWS=0 SVT_TILE_COLUMNS=0 SVT_TRACE_OUT=/dev/null \
       "$HERE/capture_c_trace/capture_c_trace" \
       "$w" "$h" "$qp" "$p" "$OUT/rs.yuv" "$OUT/c0.obu" >/dev/null 2>&1; then
    fail=$((fail + 1)); failed+=("$tag[c0-err]"); continue
  fi
  if cmp -s "$OUT/c.obu" "$OUT/c0.obu"; then
    fail=$((fail + 1))
    failed+=("$tag[VACUOUS: C coded it identically to a single tile]")
    echo "  VACUOUS  $tag  <-- geometry clamped the request away; cell proves nothing"
    continue
  fi

  # (E) DECODABILITY — the port's bytes must be a legal AV1 stream.
  if [ -n "$aomdec" ]; then
    if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
      fail=$((fail + 1))
      failed+=("$tag[UNDECODABLE: aomdec rejected the port stream]")
      echo "  CORRUPT  $tag  <-- port stream does not decode"
      continue
    fi
  fi

  if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
    if in_list "$cell" "${BYTE_EXACT[@]+"${BYTE_EXACT[@]}"}"; then
      pass=$((pass + 1)); echo "  OK       $tag (byte-exact)"
    else
      # (D) the self-promoting pin fired — good news that must not be
      # silently absorbed.
      fail=$((fail + 1))
      failed+=("$tag[PIN-BROKEN: now byte-exact -> move it to BYTE_EXACT]")
      echo "  PROMOTE  $tag  <-- now byte-exact! add it to BYTE_EXACT"
    fi
  else
    if in_list "$cell" "${BYTE_EXACT[@]+"${BYTE_EXACT[@]}"}"; then
      fail=$((fail + 1)); failed+=("$tag[REGRESSION: was byte-exact]")
      echo "  REGRESS  $tag"
    else
      pass=$((pass + 1))
      cb=$(stat -c%s "$OUT/c.obu"); rb=$(stat -c%s "$OUT/rs.obu")
      echo "  pinned   $tag (C=${cb}B port=${rb}B, decodes — see the note above)"
    fi
  fi
done

rm -rf "$OUT"
total=$((pass + fail))
echo
echo "tile gate: $pass / $total"
if [ "$fail" -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
fi
[ "$fail" -eq 0 ]

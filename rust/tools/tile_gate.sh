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
#                        once (5 col tiles at cols_log2=2 wait, 10/4->3
#                        wide -> 4 cols; 7/4->2 high -> 4 rows).
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
# The pattern is not arbitrary: every one is 256x256 at qp 45, i.e. the
# geometry where each tile is one or two SBs AND the qp is high enough
# that the blocks on a tile edge quantize to the same decision either way.
# That is exactly what the remaining root predicts — see the note below.
BYTE_EXACT=(
  "gradient 256 256 45 6  1 0"
  "gradient 256 256 45 6  2 0"
  "gradient 256 256 45 6  2 1"
  "gradient 256 256 45 6  2 2"
  "gradient 256 256 45 10 2 2"
  "gradient 256 256 45 13 2 2"
)

# ---------------------------------------------------------------------------
# WHY THE REMAINING CELLS DIVERGE — measured, not guessed.
#
# The frame header is NOT the problem: identity_diff classifies every
# diverging cell's first divergence as `tile payload`, never SH or FH. So
# tile_info() — uniform_tile_spacing_flag, both increment runs,
# context_update_tile_id over the ACTUAL tile count, tile_size_bytes_minus_1
# — and the tile group's size prefixes are byte-correct across the sweep.
#
# The root is that the MD search is not tile-boundary-aware. Each tile's
# search runs on its own frame-sized recon canvas initialised to 128
# (`tile_frame_recon`, pipeline.rs), and the leaf funnel — the MD path for
# every preset this gate uses — reads neighbours through the NON-tiled
# `extract_neighbors` plus raw `abs_tx_x > 0` / `abs_tx_y > 0` tests
# (leaf_funnel.rs:1022, :1745, :5976, :6118) and `intra_edge::dr_predict`'s
# `have_top = mi_row > 0` / `have_left = mi_col > 0`, whose own comment
# says "(single tile)" (intra_edge.rs:882). So a block on a tile edge
# predicts from the 128 fill, where C signals the neighbour UNAVAILABLE and
# takes build_intra_predictors' 127/129/128 fallbacks
# (`xd->up_available = mi_row > tile->mi_row_start`,
# adaptive_mv_pred.c:1058-1059). Different prediction -> different residual
# -> different decision when the block is close to a boundary.
#
# Two corroborations:
#   * the cells that DO match are the ones whose tiles are one or two SBs
#     at high qp — where the edge blocks' decisions are insensitive;
#   * the first DIVERGING op is usually `lr-taps`, which is a SYMPTOM, not
#     the root: LR syntax is written before each SB's partition tree, and
#     the LR search is frame-scoped in C too (`svt_aom_foreach_rest_unit_in
#     _frame` calls on_tile(0,0) exactly once over whole_frame_rect,
#     restoration.c:1274-1297), so ANY recon difference anywhere in the
#     frame reprices every unit's taps and surfaces at the very first op.
#
# Fixing it means threading the tile origin into the funnel's UnitGeom and
# intra_edge::DrGeom and switching those five sites to the tile-scoped
# forms C uses. That is a change to the hottest path in the encoder, so it
# is a separate, separately-verified landing — not a rider on the gate.
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
      echo "  pinned   $tag (C=${cb}B port=${rb}B, decodes — MD not tile-aware)"
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

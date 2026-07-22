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
  # The two big-gap tile-ROW-boundary witnesses (were port -190 / -214 bytes
  # short before the M6 PD0 tile-boundary fix — the strongest proof it works).
  "gradient 512 384 45 6  1 1"
  "gradient 512 384 45 6  1 2"
  # --- 640x448, ragged on BOTH axes ---
  "gradient 640 448 45 6  1 1"
  "gradient 640 448 45 6  2 2"
  "gradient 640 448 45 10 2 1"
  # The two big-gap BOTH-axes q20 witnesses (were port +219 / +180 bytes off).
  "gradient 640 448 20 6  1 2"
  "gradient 640 448 20 6  2 2"
)

# Cells that BYTE-MATCH the C reference today. A cell only ever moves here
# after `cmp` says so (measured by tools/tile_map.sh, 162-cell sweep).
#
# ALL 26 tile cells are byte-exact — the full 162-cell sweep is 162/162 MATCH
# after the M6 PD0 tile-boundary fix (see the note below). This includes
# `512x384 r2` (the ragged-rows geometry, tile count 3 where 1<<log2 is 4, the
# ex-UNDECODABLE cell) and the four big-gap witnesses (512x384 q45 r1c1/r1c2,
# 640x448 q20 r1c2/r2c2) that were 190-219 bytes off before the fix.
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
  "gradient 256 256 20 6  2 2"
  "gradient 512 384 45 6  0 1"
  "gradient 512 384 45 6  1 0"
  "gradient 512 384 45 6  2 0"
  "gradient 512 384 45 6  2 2"
  "gradient 512 384 20 6  2 0"
  "gradient 512 384 45 6  1 1"
  "gradient 512 384 45 6  1 2"
  "gradient 640 448 45 6  1 1"
  "gradient 640 448 45 6  2 2"
  "gradient 640 448 45 10 2 1"
  "gradient 640 448 20 6  1 2"
  "gradient 640 448 20 6  2 2"
)

# ---------------------------------------------------------------------------
# THE MULTI-TILE PRESET-6 RESIDUAL — ROOT-CAUSED AND CLOSED (162/162 MATCH).
#
# The frame header is NOT the problem, and never was: identity_diff
# classifies every diverging cell's first divergence as `tile payload`,
# never SH or FH. tile_info() — uniform_tile_spacing_flag, both increment
# runs, context_update_tile_id over the ACTUAL tile count,
# tile_size_bytes_minus_1 — and the tile group's size prefixes are
# byte-correct across the entire 162-cell sweep.
#
# The MD-search fix (`intra_edge::TileMi` carrying the tile's mi rect into
# the leaf funnel's UnitGeom / DrGeom so C's tile-scoped availability
# predicates and neighbour extraction stop at the tile edge instead of
# reading the 128 fill) promoted 12 of the 22 cells — but it fixed only the
# LEAF-MODE search, not the PARTITION search.
#
# The residual (25 cells, every one PRESET 6) was a SECOND tile-blind
# corner: the M6 PD0 partition search (`pd0_pick_sb_partition_m6_eval` ->
# `Pd0Ctx::lvl1_block_cost_rect`) predicted the DC candidate from
# `extract_neighbors` — the FRAME-edge availability form — so at a
# tile-TOP-ROW / tile-LEFT-COLUMN superblock it read source pixels ACROSS
# the tile boundary. C's `up_available` / `left_available` respect tiles at
# every preset, so C predicts DC from the tile edge (worse prediction ->
# higher residual -> SPLIT into 16x16/8x8) while the port kept the 64x64
# NONE and coded a different tree. Measured at 512x384 q45 r1c0: at the
# tile-row boundary mi_row=48 (pixel 192) C-multi splits to bsize 6/3 where
# C-single (== port) keeps bsize 12 — and the port's multi-tile tree was
# BYTE-IDENTICAL to its own single-tile tree (tile-blind). Fix: thread the
# tile pixel origin into `Pd0Ctx` and use `extract_neighbors_tiled` in the
# LVL_1 leaf cost (byte-inert at origin 0, so single-tile / eff-M9 LVL_5 /
# bd10 LVL_0 are untouched — which is exactly why presets 10/13 were 48/48
# THROUGHOUT: their variance-dominated eff-M9 partition never reacted to the
# boundary-prediction change in the first place).
#
# The `lr-taps` "first divergence" was a faithful SYMPTOM, not the root: LR
# syntax is written before each SB's partition tree, so any recon difference
# anywhere in the frame reprices the whole-frame Wiener taps and surfaces at
# the very first coded op. Both encoders DO pick RESTORE_WIENER; the taps
# just differed because the recon fed to the (genuinely whole-frame /
# tile-independent — restoration.c `foreach_rest_unit_in_frame` uses
# `whole_frame_rect` and calls `on_tile(0,0)` exactly once, tile loop
# hardcoded `< 1` at :1699) LR search differed. The earlier "per-tile RU
# grid" hypothesis (pipeline.rs task-#86 PORT-NOTE) was WRONG — the recon
# difference was the PD0 partition, now fixed.
#
# NOT COVERED by this gate (stated so nobody reads 26/26 as more than it
# is):
#   * SB128 + tiles. Every cell here is SB64 — C picks SB128 only at
#     preset <= 1 and >= 165,120 aligned px, and these presets are 6/10/13.
#     TileGrid::resolve does implement the SB128 limits (max_tile_width_sb
#     halves, max_tile_area_sb quarters) but nothing exercises them.
#   * bd10 + tiles. The bd10 re-encode is post-merge and whole-frame — see
#     the PORT-NOTEs at both UnitGeom sites in pipeline.rs.
#   * Real photographic / screen content with tiles. All cells are
#     `gradient`.
#   * C's enc_settings validation caps (each log2 <= 6, product <= 128,
#     and tile_columns <= 4 — enc_settings.c:373,377) are a hard REJECT in
#     C, where the port clamps geometrically. That is an error-behaviour
#     difference, not a bitstream one, and it is out of reach here anyway:
#     cols_log2 5 needs >= 32 SB columns, i.e. a >= 2048px-wide frame.
#   * Non-uniform tile spacing is not a gap — C itself refuses it
#     ("NON uniform_tile_spacing_flag not supported yet",
#     entropy_coding.c:2427), so uniform is the whole envelope.
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

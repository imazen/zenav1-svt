#!/usr/bin/env bash
# Feature-COMBINATION coverage gate (task: untested-intersection map).
#
# ============================================================ WHY THIS EXISTS
# Every other gate in this repo tests ONE feature in isolation:
#   * tile_gate.sh    — bd8, SB64, multi-tile
#   * sb128_gate.sh   — bd8, single-tile, SB128
#   * bd10_*          — bd10, single-tile
#   * bd10_photo_gate — bd10, real content, single-tile
# The INTERSECTIONS were explicitly documented as UNMEASURED
# (tile_gate.sh:208-217, docs/finishing-survey.md, docs/sb128-port-map.md).
# This gate measures the three of them and turns the result into a MAP:
# byte-MATCH cells become ASSERTED byte-identity cells (a real strengthening
# — it proves the intersection is correct), DIFF cells become PINNED
# self-promoting divergence targets with a first-divergence localization
# recorded in docs/coverage-combos-map.md.
#
#   Axis 1 — SB128 x tiles: a frame large enough that C picks SB128 (aligned
#            luma area >= 165,120 AND preset <= 1) WITH a tile grid. Exercises
#            the SB128 tile limits in TileGrid::resolve (max_tile_width_sb
#            HALVES, max_tile_area_sb QUARTERS) composed with the SB128
#            partition/coding walk.
#   Axis 2 — bd10 x tiles: multi-tile encode at 10-bit. The bd10 re-encode
#            path (bd10_reencode_{luma,chroma}) is WHOLE-FRAME (post per-tile
#            merge); this is where it either composes with tiles or does not.
#   Axis 3 — real content x tiles: CID22 photographic + gb82-sc screen images
#            with tile splits (the tile gate uses only synthetic gradient).
#
# ============================================================ WHAT IT ASSERTS
# Per cell, FOUR encodes are produced and compared:
#   port_tiled   = the port with the tile request      (rs.obu)
#   C_tiled      = C   with the tile request           (c.obu)
#   C_single     = C   at rows=cols=0 (single tile)    (c0.obu)
#   port_single  = the port at rows=cols=0             (rs0.obu)  [the CONTROL]
#
#   (A) ANTI-VACUITY (hard): C_tiled must DIFFER from C_single, i.e. the tile
#       request genuinely changed C's encode. A grid the geometry clamps away
#       silently produces a single-tile encode and the cell would prove
#       nothing about tiling. (Axis 1 also asserts C_tiled is really SB128.)
#   (B) CONTROL (classify, not a hard fail on its own): port_single vs
#       C_single. If the CONTROL does NOT match, the cell has a PRE-EXISTING
#       single-tile divergence (bd10 low-preset content, screen-content tools,
#       a real-content near-tie) that is NOT about tiles — so its tiled result
#       proves nothing about the intersection. Such cells are listed in
#       CONTENT_DIVERGES and reported separately; they are neither promoted nor
#       counted as an intersection finding.
#   (C) DECODABILITY (hard, every cell): aomdec must accept port_tiled — a byte
#       gate is blind to corruption among expected-DIFF cells. Same contract as
#       tile_gate.sh / sb128_gate.sh. Skipped LOUDLY if aomdec is absent.
#   (D) BYTE-EXACT (hard): every cell in <AXIS>_BYTE_EXACT must `cmp` clean AND
#       its control must match. A cell only ever lands there after `cmp` says
#       so.
#   (E) PIN (hard, self-promoting): a cell with a matching CONTROL that is NOT
#       in <AXIS>_BYTE_EXACT is pinned-diverging. If it starts byte-matching,
#       the gate FAILS so the improvement is promoted, never silently absorbed.
#
# Exit 0 iff A, C, D, E hold for every cell.  Env: AOMDEC=/path/to/aomdec.
# SVT_CREF_LIB_DIR must point at the C reference lib (mainline Bin/Release).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-/tmp}/covcombos.$$"
mkdir -p "$OUT"
SCORE="$RS_ROOT/benchmarks/coverage_combos_latest.tsv"
mkdir -p "$RS_ROOT/benchmarks"

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
  echo "WARNING: aomdec not found (set AOMDEC=...) — assert (C) DECODABILITY is SKIPPED" >&2
  aomdec=""
}
[ -n "$aomdec" ] && echo "decodability check: $aomdec"

CID="${CID_CORPUS:-/root/work/codec-corpus/CID22/CID22-512/training}"
SC="${SC_CORPUS:-/root/work/codec-corpus/gb82-sc}"

# ---------------------------------------------------------------------------
# CELLS. Format: "axis bd content w h qp preset rows_log2 cols_log2"
#   axis    = sb128 | bd10 | real   (drives env + which anti-vacuity asserts)
#   bd      = 8 | 10
#   content = uniform | gradient | diag | file:<abs-path.png>
# For `real` cells the content is a file: path; w/h are the image dims rounded
# up to a multiple of 64 (CID22-512 is natively 512x512; screen crops below).
# ---------------------------------------------------------------------------

# --- Axis 1: SB128 x tiles (bd8, preset <= 1, >= 165,120 aligned px) --------
#   512x384 = 196,608px = 4x3 SB128   (cols_log2<=2, rows_log2<=2)
#   512x512 = 262,144px = 4x4 SB128
#   640x512 = 327,680px = 5x4 SB128
SB128_CELLS=(
  "sb128 8 gradient 512 384 32 0 0 1"
  "sb128 8 gradient 512 384 32 0 1 0"
  "sb128 8 gradient 512 384 32 0 1 1"
  "sb128 8 gradient 512 384 32 0 2 2"
  "sb128 8 gradient 512 384 55 0 1 1"
  "sb128 8 uniform  512 384 32 0 1 1"
  "sb128 8 uniform  512 384 32 0 2 2"
  "sb128 8 diag     512 384 32 0 1 1"
  "sb128 8 diag     512 384 32 0 2 2"
  "sb128 8 gradient 512 384 32 1 1 1"
  "sb128 8 gradient 512 512 32 0 2 2"
  "sb128 8 diag     512 512 32 0 1 1"
  "sb128 8 gradient 640 512 32 0 1 1"
  "sb128 8 gradient 640 512 32 0 2 2"
  "sb128 8 gradient 640 512 32 0 1 2"
  "sb128 8 uniform  640 512 32 0 2 2"
)

# --- Axis 2: bd10 x tiles ---------------------------------------------------
#   uniform is bit-depth-independent (skip) -> expected MATCH.
#   gradient/diag exercise the WHOLE-FRAME bd10 re-encode with tiles.
BD10_CELLS=(
  "bd10 10 uniform  128 128 40 10 1 1"
  "bd10 10 uniform  256 256 40 6  2 2"
  "bd10 10 uniform  256 256 40 10 2 2"
  "bd10 10 uniform  256 256 40 13 2 2"
  "bd10 10 gradient 256 256 20 10 1 1"
  "bd10 10 gradient 256 256 40 10 1 1"
  "bd10 10 gradient 256 256 40 10 2 2"
  "bd10 10 gradient 256 256 40 13 2 2"
  "bd10 10 gradient 256 256 40 6  2 2"
  "bd10 10 diag     256 256 40 10 1 1"
  "bd10 10 diag     256 256 40 13 2 2"
  "bd10 10 gradient 128 128 40 10 1 1"
)

# --- Axis 3: real content x tiles (bd8) -------------------------------------
#   CID22-512 photographic (512x512 = 8x8 SB64 at preset >= 6).
#   gb82-sc screen crops rounded up to 64: windows95 640x480->640x512,
#   graph 796x481->832x512.  preset 10 keeps them SB64 (SB128 needs preset<=1).
REAL_CELLS=(
  "real 8 file:$CID/1001682.png 512 512 40 10 1 1"
  "real 8 file:$CID/1001682.png 512 512 40 10 2 2"
  "real 8 file:$CID/1001682.png 512 512 40 6  2 2"
  "real 8 file:$CID/2119713.png 512 512 40 10 2 2"
  "real 8 file:$CID/2119713.png 512 512 40 6  1 1"
  "real 8 file:$CID/4666751.png 512 512 40 10 2 2"
  "real 8 file:$CID/2738653.png 512 512 40 10 1 1"
  "real 8 file:$CID/1484678.png 512 512 20 10 2 2"
  "real 8 file:$SC/windows95.png 640 512 40 10 1 1"
  "real 8 file:$SC/windows95.png 640 512 40 10 2 2"
  "real 8 file:$SC/graph.png     832 512 40 10 1 1"
  "real 8 file:$SC/graph.png     832 512 40 10 2 2"
)

# ---------------------------------------------------------------------------
# BYTE-EXACT lists (measured; a cell moves here only after `cmp` says so, and
# only when its single-tile CONTROL also matches). Keyed by the full cell
# string so the pin is exact.
# ---------------------------------------------------------------------------
BYTE_EXACT=(
  # --- Axis 1: SB128 x tiles — ALL byte-exact (measured 2026-07-22) ---
  "sb128 8 gradient 512 384 32 0 0 1"
  "sb128 8 gradient 512 384 32 0 1 0"
  "sb128 8 gradient 512 384 32 0 1 1"
  "sb128 8 gradient 512 384 32 0 2 2"
  "sb128 8 gradient 512 384 55 0 1 1"
  "sb128 8 uniform  512 384 32 0 1 1"
  "sb128 8 uniform  512 384 32 0 2 2"
  "sb128 8 diag     512 384 32 0 1 1"
  "sb128 8 diag     512 384 32 0 2 2"
  "sb128 8 gradient 512 384 32 1 1 1"
  "sb128 8 gradient 512 512 32 0 2 2"
  "sb128 8 diag     512 512 32 0 1 1"
  "sb128 8 gradient 640 512 32 0 1 1"
  "sb128 8 gradient 640 512 32 0 2 2"
  "sb128 8 gradient 640 512 32 0 1 2"
  "sb128 8 uniform  640 512 32 0 2 2"
  # --- Axis 2: bd10 x tiles — uniform (bit-depth-independent) matches ---
  "bd10 10 uniform  128 128 40 10 1 1"
  "bd10 10 uniform  256 256 40 6  2 2"
  "bd10 10 uniform  256 256 40 10 2 2"
  "bd10 10 uniform  256 256 40 13 2 2"
  # --- Axis 3: real x tiles (measured 2026-07-22) — a MIXED map: some real
  #     cells byte-match with tiles, some DIVERGE (control matches on all —
  #     verified — so every DIFF here is a genuine tile-intersection finding,
  #     not a pre-existing content divergence). The split tracks tile COUNT:
  #     1001682 matches r1c1 but DIVERGES r2c2; graph diverges at BOTH r1c1 and
  #     r2c2. See docs/coverage-combos-map.md.
  "real 8 file:$CID/1001682.png 512 512 40 10 1 1"
  "real 8 file:$CID/1001682.png 512 512 40 6  2 2"
  "real 8 file:$CID/2119713.png 512 512 40 10 2 2"
  "real 8 file:$CID/2119713.png 512 512 40 6  1 1"
  "real 8 file:$CID/2738653.png 512 512 40 10 1 1"
  "real 8 file:$CID/1484678.png 512 512 20 10 2 2"
  "real 8 file:$SC/windows95.png 640 512 40 10 1 1"
  "real 8 file:$SC/windows95.png 640 512 40 10 2 2"
  # PINNED-DIVERGING (control matches, tiled diverges — genuine tile findings):
  #   "real 8 file:$CID/1001682.png 512 512 40 10 2 2"
  #   "real 8 file:$CID/4666751.png 512 512 40 10 2 2"
  #   "real 8 file:$SC/graph.png     832 512 40 10 1 1"
  #   "real 8 file:$SC/graph.png     832 512 40 10 2 2"
)

# Cells whose SINGLE-TILE control does NOT match: a pre-existing content
# divergence, NOT a tile issue. Reported separately, never promoted. (None in
# the current cell set — screen preset-6 which diverges single-tile is kept out
# of the cell lists; add here if a cell's control regresses.)
CONTENT_DIVERGES=()

in_list() {
  local needle="$1"; shift
  local e
  for e in "$@"; do [ "$e" = "$needle" ] && return 0; done
  return 1
}

# encode helper: $1=out-prefix $2=bd $3=content $4=w $5=h $6=qp $7=preset
#                $8=rows_log2 $9=cols_log2  (rows/cols default 0)
port_encode() {
  local pfx=$1 bd=$2 content=$3 w=$4 h=$5 qp=$6 p=$7 r=${8:-0} c=${9:-0}
  SVTAV1_BD=$bd SVTAV1_TILE_ROWS_LOG2=$r SVTAV1_TILE_COLS_LOG2=$c \
    "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$p" "$pfx" >"$pfx.log" 2>"$pfx.trace"
}
c_encode() {
  local yuv=$1 obu=$2 bd=$3 w=$4 h=$5 qp=$6 p=$7 r=${8:-0} c=${9:-0}
  local bdarg=""; [ "$bd" = "10" ] && bdarg="10"
  SVT_TILE_ROWS=$r SVT_TILE_COLUMNS=$c SVT_TRACE_OUT=/dev/null \
    "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$p" "$yuv" "$obu" $bdarg >"$obu.log" 2>&1
}

printf 'axis\tbd\tcontent\tw\th\tqp\tpreset\trows\tcols\tsb128\tvacuity\tcontrol\ttiled\tc_bytes\tport_bytes\n' >"$SCORE"

run_axis() {
  local axis_name="$1"; shift
  local -a cells=("$@")
  local axis_pass=0 axis_diff=0 axis_contentdiv=0
  echo "=================================================================="
  echo "### AXIS: $axis_name"
  echo "=================================================================="
  for cell in "${cells[@]}"; do
    read -r axis bd content w h qp p r c <<<"$cell"
    local short_content="$content"
    case "$content" in file:*) short_content="$(basename "${content#file:}" .png)";; esac
    local tag="${axis}_${short_content}_${w}x${h}_q${qp}_p${p}_r${r}c${c}_bd${bd}"

    # port_tiled + rs.yuv
    if ! port_encode "$OUT/rs" "$bd" "$content" "$w" "$h" "$qp" "$p" "$r" "$c"; then
      fail=$((fail+1)); failed+=("$tag[rs-err]"); echo "  RS-ERR   $tag"; continue
    fi
    # C_tiled
    if ! c_encode "$OUT/rs.yuv" "$OUT/c.obu" "$bd" "$w" "$h" "$qp" "$p" "$r" "$c"; then
      fail=$((fail+1)); failed+=("$tag[c-err]"); echo "  C-ERR    $tag"; continue
    fi
    # C_single (anti-vacuity ref)
    c_encode "$OUT/rs.yuv" "$OUT/c0.obu" "$bd" "$w" "$h" "$qp" "$p" 0 0
    # port_single (the CONTROL). Content is deterministic (same PNG/pattern,
    # same dims, same bd), so rs0.yuv == rs.yuv byte-for-byte — c0.obu (from
    # rs.yuv) and rs0.obu (from rs0.yuv) are apples-to-apples.
    port_encode "$OUT/rs0" "$bd" "$content" "$w" "$h" "$qp" "$p" 0 0

    # (A) anti-vacuity
    local vac="tiled"
    if cmp -s "$OUT/c.obu" "$OUT/c0.obu"; then vac="VACUOUS"; fi
    # (A) SB128 axis: C_tiled must really be SB128
    local sb="-"
    if [ "$axis" = "sb128" ]; then
      sb=$(python3 "$HERE/sb128_seqhdr.py" "$OUT/c.obu" 2>/dev/null | grep -o 'use_128x128_superblock=[01]' | cut -d= -f2)
      [ -z "$sb" ] && sb="?"
    fi
    # (B) control
    local ctlm="ctl-DIFF"
    if cmp -s "$OUT/rs0.obu" "$OUT/c0.obu"; then ctlm="ctl-MATCH"; fi
    # tiled verdict
    local tiled="DIFF"
    if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then tiled="MATCH"; fi

    local cb rb; cb=$(stat -c%s "$OUT/c.obu"); rb=$(stat -c%s "$OUT/rs.obu")
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$axis" "$bd" "$short_content" "$w" "$h" "$qp" "$p" "$r" "$c" "$sb" "$vac" "$ctlm" "$tiled" "$cb" "$rb" >>"$SCORE"

    # ---- hard asserts ----
    if [ "$vac" = "VACUOUS" ]; then
      fail=$((fail+1)); failed+=("$tag[VACUOUS: C coded tiles == single tile]")
      echo "  VACUOUS  $tag"; continue
    fi
    if [ "$axis" = "sb128" ] && [ "$sb" != "1" ]; then
      fail=$((fail+1)); failed+=("$tag[NOT-SB128: C emitted sb=$sb]")
      echo "  NOT-SB128 $tag"; continue
    fi
    if [ -n "$aomdec" ] && ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
      fail=$((fail+1)); failed+=("$tag[UNDECODABLE]")
      echo "  CORRUPT  $tag  <-- port stream does not decode"; continue
    fi

    # (B) content-diverges-single-tile: not a tile finding
    if [ "$ctlm" = "ctl-DIFF" ]; then
      if in_list "$cell" "${CONTENT_DIVERGES[@]+"${CONTENT_DIVERGES[@]}"}"; then
        axis_contentdiv=$((axis_contentdiv+1)); pass=$((pass+1))
        echo "  content  $tag  (single-tile control DIFFERS — pre-existing, not a tile issue)"
      else
        fail=$((fail+1)); failed+=("$tag[CONTROL-REGRESSED: single-tile no longer matches]")
        echo "  CTL-REG  $tag  <-- single-tile control regressed; investigate"
      fi
      continue
    fi

    # control matches from here — the tiled result IS an intersection finding
    if [ "$tiled" = "MATCH" ]; then
      if in_list "$cell" "${BYTE_EXACT[@]+"${BYTE_EXACT[@]}"}"; then
        axis_pass=$((axis_pass+1)); pass=$((pass+1)); echo "  OK       $tag (byte-exact)"
      else
        fail=$((fail+1)); failed+=("$tag[PIN-BROKEN: now byte-exact -> add to BYTE_EXACT]")
        echo "  PROMOTE  $tag  <-- now byte-exact! add it to BYTE_EXACT"
      fi
    else
      if in_list "$cell" "${BYTE_EXACT[@]+"${BYTE_EXACT[@]}"}"; then
        fail=$((fail+1)); failed+=("$tag[REGRESSION: was byte-exact]")
        echo "  REGRESS  $tag"
      else
        axis_diff=$((axis_diff+1)); pass=$((pass+1))
        echo "  pinned   $tag (C=${cb}B port=${rb}B — tile-intersection DIVERGES, control MATCHES)"
      fi
    fi
  done
  echo "--- $axis_name: $axis_pass byte-exact / $axis_diff pinned-diverging / $axis_contentdiv content-diverges ---"
}

run_axis "SB128 x tiles" "${SB128_CELLS[@]}"
run_axis "bd10 x tiles"  "${BD10_CELLS[@]}"
run_axis "real x tiles"  "${REAL_CELLS[@]}"

rm -rf "$OUT"
total=$((pass + fail))
echo
echo "coverage combos gate: $pass / $total"
echo "scoreboard: $SCORE"
if [ "$fail" -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
fi
[ "$fail" -eq 0 ]

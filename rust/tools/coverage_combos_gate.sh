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
#
# CC_AXES (default "sb128 bd10 real") selects which axes run. Axis 3 needs the
# CID22 + gb82-sc corpora, which CI runners do not have; the CI step passes
# CC_AXES="sb128 bd10" so the skip is the CALLER's, visible in the workflow,
# never a silent in-script file-exists check (an axis that is skipped is
# announced on stderr and counted in the final line).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-$HOME/tmp}/covcombos.$$"
mkdir -p "$OUT"
# The scoreboard goes under target/: until 2026-09-26 every run rewrote the
# tracked benchmarks/coverage_combos_latest.tsv (kept as coverage_combos_2026-09-02.tsv), so
# running the gate dirtied the tree.
SCORE="$RS_ROOT/target/coverage_combos_latest.tsv"
mkdir -p "$RS_ROOT/target"


aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  for cand in /root/aomdec-build/aomdec /root/aomdec-debug/aomdec \
              /root/aom-rs/upstream/build/aomdec; do
    [ -x "$cand" ] && { aomdec="$cand"; break; }
  done
fi
command -v "$aomdec" >/dev/null 2>&1 || [ -x "$aomdec" ] || {
  echo "coverage combos gate: aomdec not found (set AOMDEC=...); assert (C) DECODABILITY is required" >&2
  exit 2
}

CID="${CID_CORPUS:-$(corpus_dir codec-corpus/CID22/CID22-512/training)}"
SC="${SC_CORPUS:-$(corpus_dir codec-corpus/gb82-sc)}"

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
  # PROMOTED 2026-09-02 (issue #18): 7 of the 8 gradient/diag cells that had
  # been pinned-diverging since 2026-07-22 became BYTE-EXACT once bd10 intra
  # prediction stopped crossing tile edges (`extract_neighbors_hbd` gained
  # `tile_top`/`tile_left`; `bd10_reencode_{luma,chroma}_node` stopped using
  # `TileMi::whole_frame`). The gate's own PIN-BROKEN check is what demanded
  # the promotion — it fired on all seven.
  #
  # NOTE the earlier measurement this supersedes: threading per-tile bounds
  # into the RE-ENCODE alone was verified byte-inert, and that is still true.
  # The cells moved because the OTHER site — the preset <= 8 funnel's
  # frame-absolute neighbour availability — was never in that experiment.
  "bd10 10 gradient 128 128 40 10 1 1"
  "bd10 10 gradient 256 256 20 10 1 1"
  "bd10 10 gradient 256 256 40 6  2 2"
  "bd10 10 gradient 256 256 40 10 2 2"
  "bd10 10 gradient 256 256 40 13 2 2"
  "bd10 10 diag     256 256 40 10 1 1"
  "bd10 10 diag     256 256 40 13 2 2"
  # STILL PINNED-DIVERGING — the ONE bd10 x tiles cell this fix did not move
  # (C=2250B port=2240B), and the doc's tree_diff analysis of it stands: the
  # eff-M9 partition search picks bsize 9 where C splits to bsize 6 at the
  # y=128 tile-row-boundary SBs. That is a byte root, not a correctness one.
  #   "bd10 10 gradient 256 256 40 10 1 1"
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

# The cells are a list for tools/cellrun.py (plan T3). Each tiled cell names
# the untiled encode of the same content/geometry/qp/preset: C's stream must
# differ from it (VACUITY) and so must the port's; that untiled sibling is
# the CONTROL and must byte-match (a CONTENT_DIVERGES cell's control would be
# pinned DIFFERS instead; none is today). Tiled cells in BYTE_EXACT pin
# IDENTICAL; the rest pin DIFFERS, which is self-promoting. SB128-axis cells
# also require C's sequence header to say 128.
in_list() {
  local needle="$1"; shift
  local e
  for e in "$@"; do [ "$e" = "$needle" ] && return 0; done
  return 1
}
LIST="$OUT/coverage_combos.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv_port\tenv_c\texpect\tcheck\tdiffers_from\tc_differs_from\n' >"$LIST"
declare -A ctl_seen
add_axis() {
  local cell axis bd content w h qp p r c short ctl tag expect check
  for cell in "$@"; do
    read -r axis bd content w h qp p r c <<<"$cell"
    short="$content"
    case "$content" in file:*) short="$(basename "${content#file:}" .png)";; esac
    ctl="ctl_${axis}_${short}_${w}x${h}_q${qp}_p${p}_bd${bd}"
    tag="${axis}_${short}_${w}x${h}_q${qp}_p${p}_r${r}c${c}_bd${bd}"
    if [ -z "${ctl_seen[$ctl]:-}" ]; then
      ctl_seen[$ctl]=1
      expect=IDENTICAL
      in_list "$cell" "${CONTENT_DIVERGES[@]+"${CONTENT_DIVERGES[@]}"}" && expect=DIFFERS
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t\t\t%s\tc\t\t\n' \
        "$ctl" "$content" "$w" "$h" "$qp" "$p" "$bd" "$expect" >>"$LIST"
    fi
    expect=DIFFERS
    in_list "$cell" "${BYTE_EXACT[@]+"${BYTE_EXACT[@]}"}" && expect=IDENTICAL
    check=c,decodes
    [ "$axis" = sb128 ] && check=c,decodes,sb128
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$tag" "$content" "$w" "$h" "$qp" "$p" "$bd" \
      "SVTAV1_TILE_ROWS_LOG2=$r;SVTAV1_TILE_COLS_LOG2=$c" "SVT_TILE_ROWS=$r;SVT_TILE_COLUMNS=$c" \
      "$expect" "$check" "$ctl" "$ctl" >>"$LIST"
  done
}

CC_AXES="${CC_AXES:-sb128 bd10 real}"
skipped_axes=()
axis_selected() { case " $CC_AXES " in *" $1 "*) return 0;; *) return 1;; esac; }
if axis_selected sb128; then add_axis "${SB128_CELLS[@]}"; else skipped_axes+=(sb128); fi
if axis_selected bd10;  then add_axis "${BD10_CELLS[@]}";  else skipped_axes+=(bd10);  fi
if axis_selected real;  then add_axis "${REAL_CELLS[@]}";  else skipped_axes+=(real);  fi
if [ "${#skipped_axes[@]}" -gt 0 ]; then
  echo "WARNING: axes SKIPPED by the caller via CC_AXES='$CC_AXES': ${skipped_axes[*]}" >&2
fi
if [ "$(wc -l <"$LIST")" -le 1 ]; then
  echo "coverage combos gate: no axis selected (CC_AXES='$CC_AXES') — refusing to report 0/0 as a pass" >&2
  exit 2
fi
trap 'rm -rf "$OUT"' EXIT
AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${CC_JOBS:-4}"
rc=$?
cp "$OUT/result.tsv" "$SCORE"
python3 - "$OUT/result.tsv" "${skipped_axes[*]}" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
tiled = [r for r in rows if not r["name"].startswith("ctl_")]
ctl = [r for r in rows if r["name"].startswith("ctl_")]
bad = [r for r in rows if r["ok"] != "yes"]
pinned = sum(r["expect"] == "DIFFERS" and r["ok"] == "yes" for r in tiled)
skip = f"  (axes skipped by caller: {sys.argv[2]})" if sys.argv[2] else ""
print(f"coverage combos gate: {len(rows) - len(bad)} / {len(rows)} "
      f"({len(tiled)} tiled cells, {pinned} pinned diverging; {len(ctl)} controls){skip}")
for r in bad:
    why = "NOW MATCHES — add it to BYTE_EXACT" if r["expect"] == "DIFFERS" and r["verdict"] == "IDENTICAL" else f"{r['verdict']} {r['detail']}; {r['checks']}"
    print(f"FAILED: {r['name']} [{why}]")
PY
echo "scoreboard: $SCORE"
exit "$rc"

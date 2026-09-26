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
# Env:   AOMDEC=/path/to/aomdec (autodetected; required), TILE_JOBS (4)
#
# The full sweep behind the cell choices is tools/tile_map.sh, whose
# scoreboard is written to target/tile_map_latest.tsv (last recorded: benchmarks/tile_map_2026-07-22.tsv).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
OUT="${TMPDIR:-$HOME/tmp}/tilegate.$$"
mkdir -p "$OUT"


aomdec="${AOMDEC:-aomdec}"
if ! command -v "$aomdec" >/dev/null 2>&1; then
  for cand in /root/aomdec-build/aomdec /root/aomdec-debug/aomdec \
              /root/aom-rs/upstream/build/aomdec; do
    [ -x "$cand" ] && { aomdec="$cand"; break; }
  done
fi
# Required: (E) used to be SKIPPED with a warning when aomdec was missing,
# and a gate that silently drops an assert is the one that lets corruption
# through (the pre-#96 undecodable-rows bug was exactly that class).
command -v "$aomdec" >/dev/null 2>&1 || [ -x "$aomdec" ] || {
  echo "tile gate: aomdec not found (set AOMDEC=...); assert (E) DECODABILITY is required" >&2
  exit 2
}

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

# The cells are a list for tools/cellrun.py (plan T3). Every tile cell is
# byte-exact today (the 162-cell tools/tile_map.sh sweep is 162/162), so each
# row pins IDENTICAL; a cell that ever diverges gets `expect DIFFERS`, which
# is self-promoting (a match then fails the gate). Each tile cell names the
# untiled encode of the same (size, qp, preset) twice: C's bytes must differ
# from it (A, the knob reached C) and so must the port's (the port honoured
# it too; new 2026-09-26). The untiled siblings are the controls (C): they
# must byte-match, and there is now one per (size, qp, preset) instead of one
# per size.
LIST="$OUT/tile.cells.tsv"
trap 'rm -rf "$OUT"' EXIT
printf 'name\tcontent\tw\th\tqp\tpreset\tenv_port\tenv_c\texpect\tcheck\tdiffers_from\tc_differs_from\n' >"$LIST"
declare -A seen
for cell in "${CELLS[@]}"; do
  read -r content w h qp p r c <<<"$cell"
  ctl="ctl_${content}_${w}x${h}_q${qp}_p${p}"
  if [ -z "${seen[$ctl]:-}" ]; then
    seen[$ctl]=1
    printf '%s\t%s\t%s\t%s\t%s\t%s\t\t\tIDENTICAL\tc,decodes\t\t\n' \
      "$ctl" "$content" "$w" "$h" "$qp" "$p" >>"$LIST"
  fi
  printf '%s_%sx%s_q%s_p%s_r%sc%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\tIDENTICAL\tc,decodes\t%s\t%s\n' \
    "$content" "$w" "$h" "$qp" "$p" "$r" "$c" "$content" "$w" "$h" "$qp" "$p" \
    "SVTAV1_TILE_ROWS_LOG2=$r;SVTAV1_TILE_COLS_LOG2=$c" "SVT_TILE_ROWS=$r;SVT_TILE_COLUMNS=$c" \
    "$ctl" "$ctl" >>"$LIST"
done
AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${TILE_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
bad = [r for r in rows if r["ok"] != "yes"]
ctl = [r for r in rows if r["name"].startswith("ctl_")]
print(f"tile gate: {len(rows) - len(bad)} / {len(rows)} "
      f"({len(rows) - len(ctl)} tile cells, {len(ctl)} single-tile controls)")
for r in bad:
    print(f"FAILED: {r['name']} [{r['verdict']} {r['detail']}; {r['checks']}]")
PY
exit "$rc"

#!/usr/bin/env bash
# Screen-content palette byte-parity gate — bd8 AND bd10.
#
# WHY THIS EXISTS. Every other synthetic content the identity harness can
# generate (uniform / gradient / diag) is photographic in character, so the
# screen-content detector never arms and NO gate cell could reach the palette
# path. That blind spot let a real defect ship: palette candidates were gated
# out of the bd10 mode-decision funnel entirely (`!bd10_funnel`), so at 10 bits
# the port coded ZERO palette blocks where C codes hundreds. The cost was
# measured, not guessed — `screen 128x128 q32`:
#
#     preset 0   C 327 B   port 664 B    (2.03x)
#     preset 6   C 453 B   port 1110 B   (2.45x)
#
# and on the production corpus it showed up as preset 6 bd10 = 380/515
# byte-identical (vs 515/515 at bd8), with all 135 failures on the eight
# screen-detecting content classes. A gate that cannot reach a feature cannot
# guard it, so this one drives the `screen` content at BOTH depths.
#
# ANTI-VACUITY (enforced in the script, per rust/CLAUDE.md "Gate Discipline"):
# a palette gate that passes because nothing coded a palette is worthless. Each
# cell dumps the port's own partition tree and asserts the frame actually
# CONTAINS palette leaves; a cell that codes none FAILS even if its bytes match.
#
# Usage: screen_palette_bd_gate.sh
# Env:   SP_SIZES SP_QPS SP_PRESETS SP_BDS  (space-separated overrides)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

read -r -a SIZES <<<"${SP_SIZES:-64 128}"
read -r -a QPS <<<"${SP_QPS:-20 32 55}"
# Palette is live at preset <= 7 on sc_class5 content (sc_detect.rs, C
# enc_mode_config.c:2374-2390); above that palette_level is 0, so those presets
# would be vacuous by construction and are deliberately not swept here.
#
# PRESET 7 EARNS ITS PLACE TWICE. It is the only preset where C's CDEF
# use_qp_strength fast path (cdef_search_level == 10, allintra M7+) and screen
# detection (force-disabled at M8+, enc_handle.c:4641-4651) BOTH hold, so it is
# the only default-config preset that exercises the CDEF screen-content
# qp-strength arm. The audit found M7 in no sweep in the repo at all; with the
# arm ported, 10 of these 12 cells byte-match, and with the arm's flag forced
# false 10 of 12 FAIL at an IDENTICAL byte count (the strengths are fixed-width
# frame-header fields, so a length check is structurally blind to them).
read -r -a PRESETS <<<"${SP_PRESETS:-0 2 4 6 7}"
read -r -a BDS <<<"${SP_BDS:-8 10}"
# DEFAULT is `screen` AND `screenrep` since 2026-09-26. screenrep was left
# out because at bd10 it reproduced a separate high-entropy divergence class
# (measured 2026-08-03: p2/q20, p6/{q20,q32,q55} and the p4 pins below). That
# class has since closed: re-measured 2026-09-26 on i265, the full grid with
# both contents is 120/120 byte-identical, so the gate now guards it too.
# `SP_CONTENTS=screen` restores the palette-only grid.
read -r -a CONTENTS <<<"${SP_CONTENTS:-screen screenrep}"

# KNOWN-DIVERGING cells, pinned SELF-PROMOTINGLY (the sb128_gate pattern): a
# cell listed here is expected to DIFFER, and a listed cell that starts MATCHING
# fails the gate until it is moved out. That way a fix cannot land unnoticed and
# a regression cannot hide behind a stale exclusion.
#
# screenrep_*_p4_bd10 is the documented open bd10 preset-4 residual (STATUS.md
# "Remaining bd10 low-preset scope: p4"). This content is a SYNTHETIC repro for
# it -- previously it reproduced only on a photo corpus that is not in-tree.
# Signature: bd8 identical at every preset; bd10 identical at p0/p2/p6 and at
# p4/q55; diverging at p4/q20 and p4/q32 (1 byte at q32). Suspected root, not
# yet confirmed: the NSQ recon-distortion gate keeps the u8 path at p4/p5
# (depth_refine.rs) where C scores it at hbd_md.
# KNOWN_DIFF cells pin `expect DIFFERS` (a match fails as "remove it from
# KNOWN_DIFF"); ARCH_KNOWN_DIFF pins only on aarch64/arm64 (the `arch`
# column), and those cells must MATCH elsewhere.
# The four screenrep p4 bd10 pins were measured MATCHING on 2026-09-26 (both
# this and the pre-cellrun script said "now match — remove") and are gone;
# nobody saw it because the default contents never ran screenrep.
KNOWN_DIFF=()
ARCH_KNOWN_DIFF=("screen_64_q55_p7_bd10" "screen_128_q55_p7_bd10")
in_list() {
  local needle=$1 k; shift
  for k in "$@"; do [[ "$k" == "$needle" ]] && return 0; done
  return 1
}
# The cells are a list for tools/cellrun.py (plan T3), in parallel
# (SP_JOBS, default 4); each screen cell's packed tree goes to @CELL@ and
# must code at least one palette leaf (per-cell anti-vacuity).
OUT="${TMPDIR:-$HOME/tmp}/screenpal.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
LIST="$OUT/screen_palette_bd.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tenv_port\texpect\tcheck\tarch\n' >"$LIST"
for content in "${CONTENTS[@]}"; do
for bd in "${BDS[@]}"; do
  for sz in "${SIZES[@]}"; do
    for qp in "${QPS[@]}"; do
      for p in "${PRESETS[@]}"; do
        cell="${content}_${sz}_q${qp}_p${p}_bd${bd}"
        row() { printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_PACKTREE=@CELL@/rs.ptree\t%s\tc\t%s\n' \
          "$1" "$content" "$sz" "$sz" "$qp" "$p" "$bd" "$2" "$3" >>"$LIST"; }
        if in_list "$cell" "${KNOWN_DIFF[@]+"${KNOWN_DIFF[@]}"}"; then
          row "$cell" DIFFERS ""
        elif in_list "$cell" "${ARCH_KNOWN_DIFF[@]}"; then
          row "$cell" IDENTICAL '!aarch64,!arm64'
          row "${cell}_pin" DIFFERS 'aarch64,arm64'
        else
          row "$cell" IDENTICAL ""
        fi
      done
    done
  done
done
done
python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${SP_JOBS:-4}"
rc=$?
CELLDIR="$RS_ROOT/target/cells/screen_palette_bd.cells.${SVT_ORACLE:-default}"
python3 - "$OUT/result.tsv" "$CELLDIR" <<'PY'
import csv, re, sys
from pathlib import Path
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
celldir = Path(sys.argv[2])
pal = re.compile(r"(?:^|\s)pal=([1-9][0-9]*)")
def palette_leaves(name):
    t = celldir / name / "rs.ptree"
    return sum(1 for l in t.read_text().splitlines() if pal.search(l)) if t.exists() else 0
cells = [r for r in rows if r["expect"] == "IDENTICAL"]
pins = [r for r in rows if r["expect"] == "DIFFERS"]
ok = [r for r in cells if r["ok"] == "yes"]
print(f"screen-content palette identity: {len(ok)} / {len(rows) - len(pins) + sum(r['ok'] != 'yes' for r in pins)} byte-identical "
      f"(+{sum(r['ok'] == 'yes' for r in pins)} pinned known-diff)")
bad = False
promoted = [r["name"] for r in pins if r["verdict"] == "IDENTICAL"]
if promoted:
    print("  PINNED CELLS NOW MATCH — remove them from KNOWN_DIFF:")
    for n in promoted: print(f"    {n}")
    bad = True
for r in cells:
    if r["ok"] != "yes":
        print(f"  FAILED: {r['name']} [{r['verdict']} {r['detail']}]"); bad = True
vac = [r["name"] for r in rows if r["name"].startswith("screen_") and palette_leaves(r["name"]) == 0]
if vac:
    print("  VACUOUS (no palette leaf coded — these cells guard nothing):")
    for n in vac: print(f"    {n}")
    bad = True
sys.exit(1 if bad else 0)
PY
st=$?
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]

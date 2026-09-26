#!/usr/bin/env bash
# Screen-content luma-palette byte-parity gate (task #71 / KB-P29).
#
# The FIRST real-screenshot byte test of the luma palette path. Before the
# #71 fixes only the two crops on which C uses no palette (codec_wiki,
# gmessages) byte-matched; every other screen crop diverged because the port
# mis-decided the per-block palette-vs-regular RD. Two roots were fixed:
#
#   1. INTER-CLASS MDS3 prune (post_mds2_nic_pruning inter-class block,
#      product_coding_loop.c:7993-8008). On the I-slice mds3_class_th is
#      re-floored to MAX(25, scaled*i_mds3_class_th_mult) (:7978-7979) — NOT
#      forced ~0 like mds1/mds2_class_th — so C ZEROES the regular class at
#      MDS3 when its best cost deviates too far from the palette global best.
#      The port kept a regular candidate at MDS3 that then beat palette.
#      (leaf_funnel.rs post_mds2 per-lane prune + FunnelCfg mds3_class_th /
#      mds3_band_cnt / i_mds3_class_th_mult.)
#
#   2. PALETTE COLOR-INDEX MAP rate table. C's MD-side update_palette_cdf
#      (md_rate_estimation.c:733-759) advances ONLY palette_y_mode /
#      palette_y_size — it NEVER touches palette_y_color_index_cdf, so
#      palette_ycolor_fac_bitss stays at its frame-init value for every SB.
#      The port's full-walk chain sim adapted it, drifting the map rate on
#      2nd+ palette blocks. build_md_rates now builds palette_ycolor from the
#      frame-init (default) CDF.
#
# This gate asserts byte-identity on the fully-closed set: every gb82-sc crop
# at preset 6 (nic_level 6) bd8 across the qp grid. It also anti-vacuity
# checks that the streams genuinely CODE palette blocks (else a palette gate
# that never exercises palette proves nothing). Presets 0-5/7 carry OTHER
# (non-palette) near-ties (angular / edge-filter / partition RD) that are not
# in scope here.
#
# Exit non-zero on ANY divergence or if anti-vacuity fails.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
. "$(dirname "$0")/lib_corpus.sh"
# Issue #23: a flat crop makes these gates pass while reaching neither
# palette nor IntraBC. Fail loudly instead of silently (identity_run only
# honours this when set, so synthetic uniform cells are unaffected).
export SVTAV1_ASSERT_NONFLAT=1
RS_ROOT=$(cd "$HERE/.." && pwd)

SCREEN_DIR="${SCREEN_DIR:-$(corpus_dir codec-corpus/gb82-sc)}"
# $RS_ROOT is rust/, so the library lives one level UP (matching
# svtav1-cref/build.rs, which resolves Bin/Release from the REPO root).
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR

read -r -a IMGS <<<"${SP_IMGS:-graph codec_wiki gmessages gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a QPS  <<<"${SP_QPS:-5 20 32 48 63}"
PRESET="${SP_PRESET:-6}"
DIM="${SP_DIM:-512}"

# The cells are a list for tools/cellrun.py (plan T3), run in parallel
# (SP_JOBS, default 4). Each writes its packed tree beside its streams
# (@CELL@), and the summary counts palette-coding cells from those files.
# A missing screenshot FAILS (it used to print SKIP-MISSING and pass).
OUT="${TMPDIR:-$HOME/tmp}/screen_palette.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
LIST="$OUT/screen_palette.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tenv_port\tenv_c\tcheck\n' >"$LIST"
for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { echo "screen palette gate: missing $png (set SCREEN_DIR)" >&2; exit 2; }
  for qp in "${QPS[@]}"; do
    printf '%s_p%s_q%s\t%s\t%s\t%s\t%s\t%s\tSVTAV1_PACKTREE=@CELL@/rs.ptree\tSVT_NO_AUTO_CMAKE=1\tc\n' \
      "$img" "$PRESET" "$qp" "$(screen_crop_spec "$png")" "$DIM" "$DIM" "$qp" "$PRESET" >>"$LIST"
  done
done
python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${SP_JOBS:-4}"
rc=$?
CELLDIR="$RS_ROOT/target/cells/screen_palette.cells.${SVT_ORACLE:-default}"
python3 - "$OUT/result.tsv" "$CELLDIR" "$PRESET" <<'PY'
import csv, re, sys
from pathlib import Path
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
celldir, preset = Path(sys.argv[2]), sys.argv[3]
pal = re.compile(r"pal=[1-9]")
seen = sum(1 for r in rows
           if (celldir / r["name"] / "rs.ptree").exists()
           and pal.search((celldir / r["name"] / "rs.ptree").read_text()))
ok = [r for r in rows if r["verdict"] == "IDENTICAL"]
print(f"screen palette gate (preset {preset} bd8): {len(ok)} / {len(rows)} byte-identical  (palette-coding cells: {seen})")
for r in rows:
    if r["verdict"] != "IDENTICAL":
        print(f"FAILED: {r['name']} [{r['verdict']} {r['detail']}]")
if seen == 0:
    print("ANTI-VACUITY FAIL: no palette block coded on any cell", file=sys.stderr)
    sys.exit(3)
PY
st=$?
[ "$st" -eq 3 ] && exit 3
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]

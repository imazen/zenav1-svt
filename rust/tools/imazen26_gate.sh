#!/usr/bin/env bash
# imazen26 production-corpus byte-identity GATE (coverage task).
#
# A TRACTABLE regression gate over the clean subset discovered by the K300
# discovery sweep (tools/imazen26_sweep.sh). It asserts byte-identity of the
# port vs the C reference on 1-2 representative images per content_class at a
# few axes that the sweep MEASURED byte-identical — the novel content classes
# (bilevel patent scans, government document pages, synthetic plots, AI
# clipart/illustration/product renders, manuscript scans) that no other gate
# corpus exercises, alongside photos / art / screenshots.
#
# CONTRACT (house style): every CELL below is ASSERTED byte-identical — a
# divergence is a REGRESSION (exit 1). A cell only lands here after the sweep
# measured it IDENTICAL; we NEVER assert a non-matching cell. Screen-content
# classes (web/mobile screenshots) carry a KNOWN screen-IBC/palette front at
# preset 0-4 that is out of this gate's scope — those classes are gated ONLY at
# presets >= 6 (IBC off), where they measured clean.
#
# Every port stream is also checked DECODABLE by aomdec (the zero-tolerance
# self-desync class) when aomdec is available.
#
# Corpus: NOT committed (large). Referenced by env-overridable path and the
# gate FAILS LOUD (exit 2) if it or any asserted image is absent.
#   IM26_DIR       default /root/work/imazen26-cache/K300   (the 273 PNGs)
#   IM26_MANIFEST  default $IM26_DIR/../K300.tsv            (basename->class)
#   SVT_CREF_LIB_DIR must point at the C reference lib (Bin/Release).
#   AOMDEC         required for the decodability check; IM26_ALLOW_NO_DECODE=1
#                  skips it (the caller's decision, never this script's).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
. "$(dirname "$0")/lib_corpus.sh"
RS_ROOT=$(cd "$HERE/.." && pwd)

IM26_DIR="${IM26_DIR:-$(corpus_dir imazen26-cache/K300)}"
# NO SILENT SKIP. This corpus is not committed and, until 2026-09-10, was not
# materialised on any host reachable from this repo -- so `corpus_dir` resolved
# to a path that was simply absent and every cell below reported MISSING.
#
# WHERE K300 COMES FROM, so nobody has to do this archaeology again. The
# selection is GIT-TRACKED in imazen/codec-corpus at
#   imazen-26/manifests/imazen26_representatives_K300_2026-06-14.tsv
# -- 300 rows, columns `url  crop_label  content_class  cluster_id
# cluster_size`, a k-means representative pick over imazen-26's 2,160 images
# covering all 20 content classes. Every `url` points at
# https://codec-corpus.r2.imazen.org/imazen-26-png-v3/..., which is public. So
# the whole corpus is reconstructible from that one file with no credentials;
# only the MATERIALISED cache (/root/work/imazen26-cache/K300 on the retired
# `dev-32gb` box) was ever lost, and a derived cache whose recipe is versioned
# is not lost data.
#
# CAVEAT, unresolved: that manifest assigns a PER-IMAGE crop region
# (`crop_label`: c50_bl, c50_center, c25_tl, full, ...). This gate centre-crops
# every image instead, via `crop:` at IM26_DIM. The 40 cells below were measured
# byte-identical under the centre crop and are self-consistent, but they are not
# the regions the representative selection chose.
#
# Fetch the published crops instead.
#
# The published set is the twenty gate images CENTRE-CROPPED to 512x512, which is
# all the gate ever encodes (`IM26_DIM`, below). The originals are 290 MiB; the
# crops are 6.1 MiB. That substitution is bit-exact by construction
# (`examples/crop_png.rs` reuses identity_run's own decode and centre
# arithmetic) and was MEASURED: the gate produced the same byte counts and the
# same 40/40 verdict from both. See the prefix's README.md.
if [ ! -d "$IM26_DIR" ] || [ -z "$(ls -A "$IM26_DIR" 2>/dev/null)" ]; then
  echo "== fetching imazen26 K300 gate images -> $IM26_DIR"
  "$HERE/fetch_r2_assets.sh" imazen26-k300-512/ "$IM26_DIR" || {
    echo "imazen26 gate: could not obtain the K300 images" >&2; exit 2; }
fi
# Declared for the sweep's contract; THIS gate never reads it (the cell list
# below carries its own basenames), which is why the published asset set is 20
# images and no K300.tsv, and why the gate runs 40/40 without one. The full
# 273-image K300 plus its TSV is what tools/imazen26_sweep.sh needs, and that
# discovery sweep is not in CI.
IM26_MANIFEST="${IM26_MANIFEST:-$(dirname "$IM26_DIR")/K300.tsv}"
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR
DIM="${IM26_DIM:-512}"

aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
  for c in aomdec /root/aomdec-debug/aomdec; do command -v "$c" >/dev/null 2>&1 && { aomdec=$c; break; }; done
fi
if [ -n "$aomdec" ]; then
  echo "decodability check: $aomdec"
elif [ "${IM26_ALLOW_NO_DECODE:-0}" = 1 ]; then
  echo "WARNING: aomdec not found; DECODABILITY assert SKIPPED (IM26_ALLOW_NO_DECODE=1)" >&2
else
  echo "imazen26_gate: aomdec not found (set AOMDEC=, or IM26_ALLOW_NO_DECODE=1 to skip the decode assert)" >&2
  exit 2
fi

# ---- fail LOUD if the (uncommitted) corpus is absent -----------------------
[ -d "$IM26_DIR" ] || { echo "imazen26_gate: corpus dir absent: $IM26_DIR (set IM26_DIR=)" >&2; exit 2; }

# ---- CELLS: "basename<TAB>preset qp bd" — every one MEASURED byte-identical
# by tools/imazen26_sweep.sh (benchmarks/imazen26_sweep_2026-07-24.tsv).
# Generated from the map; see benchmarks/imazen26_sweep_2026-07-24.meta.
# @@CELLS_BEGIN@@
CELLS=(
  "1014_general_colorful-glass-spheres_seattle-center-seattle_s23u_iso500-f1p7_20230727-173515_4000x3000.sdr 0 32 8"
  "1014_general_colorful-glass-spheres_seattle-center-seattle_s23u_iso500-f1p7_20230727-173515_4000x3000.sdr 10 32 10"
  "1225_interiors_kitchen-appliances-overhead_minamiizu-japan_s23u_iso200-f1p7_20250307-112014_3000x4000.sdr 0 32 8"
  "1225_interiors_kitchen-appliances-overhead_minamiizu-japan_s23u_iso200-f1p7_20250307-112014_3000x4000.sdr 10 32 10"
  "1421_nature_potted-orchids-table_shinjuku-gyoen-national-garden-shinjuku_s23u_iso16-f1p7_20230702-114223_3000x4000.sdr 0 32 8"
  "1421_nature_potted-orchids-table_shinjuku-gyoen-national-garden-shinjuku_s23u_iso16-f1p7_20230702-114223_3000x4000.sdr 10 32 10"
  "1614_food_dessert-plate-upside-down_valladolid-mexico_s23u_iso640-f1p7_20230917-213342_4000x3000.sdr 0 32 8"
  "1614_food_dessert-plate-upside-down_valladolid-mexico_s23u_iso640-f1p7_20230917-213342_4000x3000.sdr 10 32 10"
  "2001_people_by-anastasia-pivnenko-prng6r1nspq-unsplash_2272x3072.sdr 0 32 8"
  "2001_people_by-anastasia-pivnenko-prng6r1nspq-unsplash_2272x3072.sdr 10 32 10"
  "2409_textures_blue-abstract-texture_by-tim-mossholder-shmrlyrv-s0-unsplash_5504x8256.sdr 10 32 8"
  "2409_textures_blue-abstract-texture_by-tim-mossholder-shmrlyrv-s0-unsplash_5504x8256.sdr 10 32 10"
  "3002_aic_the-irish-question_181777_4000x4860.sdr 10 32 8"
  "3002_aic_the-irish-question_181777_4000x4860.sdr 10 32 10"
  "3318_met_vessel_479496_2505x2596.sdr 10 32 8"
  "3318_met_vessel_479496_2505x2596.sdr 10 32 10"
  "5017_nps_grsm-grsm-trail-map_color_p01_9146x5272.sdr 10 32 8"
  "5017_nps_grsm-grsm-trail-map_color_p01_9146x5272.sdr 10 32 10"
  "5202_epa_climate-impact-2021_fig-es1-six-impacts_p005_2968x3841.sdr 10 32 8"
  "5202_epa_climate-impact-2021_fig-es1-six-impacts_p005_2968x3841.sdr 10 32 10"
  "5329_noaa_nhc-al122024-kirk_p01_2550x3300.sdr 10 32 8"
  "5329_noaa_nhc-al122024-kirk_p01_2550x3300.sdr 10 32 10"
  "6003_scans-patents_lynn-conway-us5046022-1bit_p004_2320x3408.sdr 10 32 8"
  "6003_scans-patents_lynn-conway-us5046022-1bit_p004_2320x3408.sdr 10 32 10"
  "6606_scans-illustrations_haeckel-red-algae_plate0065_5015x7275.sdr 10 32 8"
  "6606_scans-illustrations_haeckel-red-algae_plate0065_5015x7275.sdr 10 32 10"
  "6825_scans-text_redoute-fr-description_p0060_2415x3528.sdr 10 32 8"
  "6825_scans-text_redoute-fr-description_p0060_2415x3528.sdr 10 32 10"
  "7002_plots_line-00118-s306ca2bd_1024x1024.sdr 10 32 8"
  "7002_plots_line-00118-s306ca2bd_1024x1024.sdr 10 32 10"
  "8012_mobile-screenshots_on-screen-keyboard_screenshot-20260526-070935-calculator_1080x2520.sdr 10 32 8"
  "8012_mobile-screenshots_on-screen-keyboard_screenshot-20260526-070935-calculator_1080x2520.sdr 10 32 10"
  "8129_web-screenshots_nasa-news_dpr1_page2_1440x900.sdr 13 32 8"
  "8129_web-screenshots_nasa-news_dpr1_page2_1440x900.sdr 13 32 10"
  "9050_gen_clipart_parrot-tropical_1024x1536.sdr 10 32 8"
  "9050_gen_clipart_parrot-tropical_1024x1536.sdr 10 32 10"
  "9107_gen_illustrations_choir-scene-cathedral_1024x1536.sdr 10 32 8"
  "9107_gen_illustrations_choir-scene-cathedral_1024x1536.sdr 10 32 10"
  "9259_gen_products-baby_bib-multipack-six-prints_flat_p0031_1536x1024.sdr 10 32 8"
  "9259_gen_products-baby_bib-multipack-six-prints_flat_p0031_1536x1024.sdr 10 32 10"
)
# @@CELLS_END@@

# The cells are a list for tools/cellrun.py (plan T3), in parallel
# (IM26_JOBS, default 4). With a decoder, each cell also checks `recon`
# (aomdec's output == the port's final recon; stronger than the old "aomdec
# accepts it", and it held on all 40 when adopted, 2026-09-26).
OUT="${TMPDIR:-$HOME/tmp}/imazen26.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
LIST="$OUT/imazen26.cells.tsv"
printf 'name\tcontent\tw\th\tqp\tpreset\tbd\tcheck\n' >"$LIST"
check=c
[ -n "$aomdec" ] && check=c,recon
missing=()
for cell in "${CELLS[@]}"; do
  read -r base preset qp bd <<<"$cell"
  png="$IM26_DIR/$base.png"
  tag="${base%.png}__p${preset}_q${qp}_bd${bd}"
  if [ ! -f "$png" ]; then missing+=("$tag[missing]"); continue; fi
  printf '%s\tcrop:%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$tag" "$png" "$DIM" "$DIM" "$qp" "$preset" "$bd" "$check" >>"$LIST"
done
AOMDEC="$aomdec" python3 "$HERE/cellrun.py" "$LIST" --out "$OUT/result.tsv" --bytes-only --jobs "${IM26_JOBS:-4}"
rc=$?
python3 - "$OUT/result.tsv" "${missing[@]+"${missing[@]}"}" <<'PY'
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
missing = sys.argv[2:]
ok = [r for r in rows if r["verdict"] == "IDENTICAL" and "FAIL" not in r["checks"]]
print(f"\nimazen26 gate: {len(ok)} / {len(rows) + len(missing)} byte-identical  (clean-subset regression gate)")
bad = [f"{r['name']}[{r['verdict']} {r['detail']}; {r['checks']}]" for r in rows if r not in ok] + missing
if bad:
    print("FAILED: " + "\nFAILED: ".join(bad))
sys.exit(1 if bad else 0)
PY
st=$?
[ "$rc" -eq 0 ] && [ "$st" -eq 0 ]

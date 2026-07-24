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
#   AOMDEC         optional; decodability check skipped LOUDLY if unset+absent.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CT_BIN="$HERE/capture_c_trace/capture_c_trace.bin"
IM26_DIR="${IM26_DIR:-/root/work/imazen26-cache/K300}"
IM26_MANIFEST="${IM26_MANIFEST:-$(dirname "$IM26_DIR")/K300.tsv}"
: "${SVT_CREF_LIB_DIR:=$(cd "$RS_ROOT/.." && pwd)/Bin/Release}"
export SVT_CREF_LIB_DIR
DIM="${IM26_DIM:-512}"

aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
  for c in aomdec /root/aomdec-debug/aomdec; do command -v "$c" >/dev/null 2>&1 && { aomdec=$c; break; }; done
fi
[ -n "$aomdec" ] && echo "decodability check: $aomdec" || \
  echo "WARNING: aomdec not found (set AOMDEC=) — DECODABILITY assert SKIPPED" >&2

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

echo "priming builds..." >&2
( cd "$RS_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c3 \
    cargo build --release -p zenav1-svt --features symtrace --example identity_run ) >&2 \
  || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CT_BIN" ] || { echo "binaries missing" >&2; exit 2; }

OUT="$RS_ROOT/target/imazen26_gate"
mkdir -p "$OUT"
pass=0; fail=0; declare -a failed=()

for cell in "${CELLS[@]}"; do
  read -r base preset qp bd <<<"$cell"
  png="$IM26_DIR/$base.png"
  [ -f "$png" ] || { echo "  MISSING  $base (not in $IM26_DIR)" >&2; fail=$((fail+1)); failed+=("$base[missing]"); continue; }
  tag="${base%.png}__p${preset}_q${qp}_bd${bd}"
  d="$OUT/$tag"; mkdir -p "$d"
  if ! SVTAV1_BD="$bd" nice -n 19 ionice -c3 \
        "$RUN_BIN" "crop:$png" "$DIM" "$DIM" "$qp" "$preset" "$d/rs" >/dev/null 2>"$d/rs.err"; then
    fail=$((fail+1)); failed+=("$tag[rs-encode-err]"); echo "  RS-ERR   $tag"; continue
  fi
  if ! nice -n 19 ionice -c3 \
        "$CT_BIN" "$DIM" "$DIM" "$qp" "$preset" "$d/rs.yuv" "$d/c.obu" "$bd" >/dev/null 2>/dev/null; then
    fail=$((fail+1)); failed+=("$tag[c-encode-err]"); echo "  C-ERR    $tag"; continue
  fi
  # DECODABILITY (self-desync = zero-tolerance)
  if [ -n "$aomdec" ] && ! "$aomdec" --rawvideo -o /dev/null "$d/rs.obu" >/dev/null 2>&1; then
    fail=$((fail+1)); failed+=("$tag[UNDECODABLE]"); echo "  CORRUPT  $tag  <-- port stream does not decode"; continue
  fi
  if cmp -s "$d/rs.obu" "$d/c.obu"; then
    pass=$((pass+1)); echo "  OK       $tag ($(stat -c%s "$d/rs.obu")B)"
  else
    fdiff=$(cmp "$d/rs.obu" "$d/c.obu" 2>/dev/null | awk '{print $NF}')
    fail=$((fail+1)); failed+=("$tag[DIFF@${fdiff} port=$(stat -c%s "$d/rs.obu")B C=$(stat -c%s "$d/c.obu")B]")
    echo "  DIFF     $tag  @${fdiff}"
  fi
  rm -f "$d/rs.yuv"
done

rm -rf "$OUT"
total=$((pass+fail))
echo
echo "imazen26 gate: $pass / $total byte-identical  (clean-subset regression gate)"
if [ "$fail" -gt 0 ]; then printf 'FAILED: %s\n' "${failed[@]}"; fi
[ "$fail" -eq 0 ]

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
RS_ROOT=$(cd "$HERE/.." && pwd)

RUN_BIN="$RS_ROOT/target/release/examples/identity_run"
CT_BIN="$HERE/capture_c_trace/capture_c_trace"
SCREEN_DIR="${SCREEN_DIR:-/root/work/codec-corpus/gb82-sc}"
: "${SVT_CREF_LIB_DIR:=$RS_ROOT/Bin/Release}"
export SVT_CREF_LIB_DIR

read -r -a IMGS <<<"${SP_IMGS:-graph codec_wiki gmessages gui imac_dark imac_g3 imessage terminal windows windows95}"
read -r -a QPS  <<<"${SP_QPS:-5 20 32 48 63}"
PRESET="${SP_PRESET:-6}"
DIM="${SP_DIM:-512}"

echo "priming builds..." >&2
( cd "$RS_ROOT" && CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c3 \
    cargo build --release -p zenav1-svt --features symtrace --example identity_run ) >&2 \
  || { echo "port build failed" >&2; exit 2; }
"$HERE/capture_c_trace/build.sh" >/dev/null 2>&1 || { echo "C driver build failed" >&2; exit 2; }
[ -x "$RUN_BIN" ] && [ -x "$CT_BIN" ] || { echo "binaries missing" >&2; exit 2; }

OUT="$RS_ROOT/target/screen_palette_gate"
mkdir -p "$OUT"
pass=0; fail=0; palette_seen=0; declare -a failed=()

for img in "${IMGS[@]}"; do
  png="$SCREEN_DIR/${img}.png"
  [ -f "$png" ] || { echo "  SKIP-MISSING $img (no $png)"; continue; }
  for qp in "${QPS[@]}"; do
    tag="${img}_p${PRESET}_q${qp}"
    d="$OUT/$tag"; mkdir -p "$d"
    rm -f "$d/rs.ptree"
    if ! SVTAV1_PACKTREE="$d/rs.ptree" SVTAV1_BD=8 nice -n 19 ionice -c3 \
          "$RUN_BIN" "crop:$png" "$DIM" "$DIM" "$qp" "$PRESET" "$d/rs" >/dev/null 2>/dev/null; then
      fail=$((fail+1)); failed+=("$tag[port-encode-error]"); echo "  RS-ERR   $tag"; continue
    fi
    if ! SVT_NO_AUTO_CMAKE=1 nice -n 19 ionice -c3 \
          "$CT_BIN" "$DIM" "$DIM" "$qp" "$PRESET" "$d/rs.yuv" "$d/c.obu" 8 >/dev/null 2>/dev/null; then
      fail=$((fail+1)); failed+=("$tag[c-encode-error]"); echo "  C-ERR    $tag"; continue
    fi
    # anti-vacuity: did the port actually code any palette block on this cell?
    if [ -f "$d/rs.ptree" ] && grep -qE 'pal=[1-9]' "$d/rs.ptree"; then
      palette_seen=$((palette_seen+1))
    fi
    if cmp -s "$d/rs.obu" "$d/c.obu"; then
      pass=$((pass+1)); echo "  OK       $tag ($(stat -c%s "$d/rs.obu")B)"
    else
      fdiff=$(cmp "$d/rs.obu" "$d/c.obu" 2>/dev/null | awk '{print $NF}')
      fail=$((fail+1)); failed+=("$tag[DIFF@${fdiff} port=$(stat -c%s "$d/rs.obu")B C=$(stat -c%s "$d/c.obu")B]")
      echo "  DIFF     $tag  @${fdiff}"
    fi
  done
done

rm -rf "$OUT"
total=$((pass+fail))
echo
echo "screen palette gate (preset ${PRESET} bd8): $pass / $total byte-identical  (palette-coding cells: ${palette_seen})"
if [ "$fail" -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
fi
# Anti-vacuity: a palette gate that codes NO palette anywhere is meaningless.
if [ "$palette_seen" -eq 0 ]; then
  echo "ANTI-VACUITY FAIL: no palette block coded on any cell" >&2
  exit 3
fi
[ "$fail" -eq 0 ]

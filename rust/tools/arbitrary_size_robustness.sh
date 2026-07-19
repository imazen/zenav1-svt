#!/usr/bin/env bash
# Arbitrary-size ROBUSTNESS gate (task #95 "arbitrary sizes working").
#
# Encodes a representative (dim x preset x qp x bit-depth) grid with the Rust
# port and asserts, for EVERY cell:
#   1. the encode is PANIC-FREE, and
#   2. the raw OBU stream DECODES under the AV1 reference decoder (aomdec).
#
# This is NOT a byte-identity gate (that is identity_matrix.sh / partial_sb_gate.sh
# / bd10_*). The bar here is that an ARBITRARY frame size at ANY preset produces a
# valid, decodable stream instead of crashing — the "arbitrary sizes working"
# deliverable. It specifically covers the partial-SB straddle path at presets 0-5
# (M0-M2 CfL always-on) which used to OOB-panic on odd/straddling edge leaves
# (leaf_funnel.rs txb-context span; fixed 2026-07-19).
#
# Cells are chosen to cross every geometry class:
#   - ODD true dims (65, 257): true < aligned, so edge leaves straddle the
#     aligned extent (the panic root).
#   - EVEN partial dims (56, 72, 88, 120, 200): 8-aligned but not 64-aligned.
#   - NON-square (65x64, 72x65, 257x120, 65x257): asymmetric partial SBs.
#   - sub-64 (40): a single partial SB.
#   - presets 0/1/3/5 (funnel M0-M2, CfL always-on) + 6 (detector-gated) +
#     9/13 (LPD0), across bd8 and bd10 and low/high qp.
#
# Env: AOMDEC (path to aomdec; auto-detected from common build dirs otherwise).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

# Locate aomdec.
AOMDEC="${AOMDEC:-}"
if [ -z "$AOMDEC" ]; then
  for c in aomdec /root/aomdec-build/aomdec /root/aomdec-debug/aomdec \
           /root/aom-rs/reference/libaom/build/aomdec; do
    if command -v "$c" >/dev/null 2>&1; then AOMDEC="$c"; break; fi
  done
fi
command -v "$AOMDEC" >/dev/null 2>&1 || { echo "aomdec not found (set AOMDEC=/path/to/aomdec)" >&2; exit 2; }

# Freshness: build the release runner (buffered; loud only on failure).
_bl=$(mktemp)
if ! CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" nice -n 19 ionice -c 3 \
      cargo build --release -p zenav1-svt --example identity_run >"$_bl" 2>&1; then
  cat "$_bl" >&2; rm -f "$_bl"
  echo "arbitrary_size_robustness: BUILD FAILED" >&2; exit 1
fi
rm -f "$_bl"
BIN="$RS_ROOT/target/release/examples/identity_run"
OUT="${TMPDIR:-/tmp}/arbsize.$$"; mkdir -p "$OUT"

# Representative cells: "w h qp preset bd".
CELLS=(
  # --- ODD true dims x CfL-always-on presets (the fixed panic root), bd8+bd10 ---
  "65 65 20 0 8"  "65 65 45 0 10"  "65 65 20 1 8"  "65 65 45 3 10"  "65 65 20 5 8"
  "65 64 45 0 8"  "65 64 20 0 10"  "65 64 45 3 8"  "65 64 20 5 10"
  "257 257 20 0 8" "257 257 45 1 10" "257 257 20 3 8" "257 257 45 5 10"
  "257 120 20 0 8" "257 120 45 3 10"
  "65 257 20 0 8"  "65 257 45 1 10"  "65 257 20 3 8"
  "72 65 45 0 8"   "72 65 20 3 10"
  # --- EVEN partial dims (8-aligned, not 64-aligned) x low presets ---
  "56 56 20 0 8"   "56 56 45 3 10"
  "72 72 20 0 8"   "72 72 45 1 10"  "72 72 20 5 8"
  "88 88 45 0 8"   "88 88 20 3 10"
  "120 120 20 0 8" "120 120 45 3 10" "120 120 20 5 8"
  "200 200 45 1 8" "200 200 20 3 10"
  # --- sub-64 single partial SB ---
  "40 40 20 0 8"   "40 40 45 3 10"
  # --- detector-gated preset 6 + LPD0 9/13 across the geometry classes ---
  "65 65 20 6 8"   "65 65 45 6 10"  "65 65 20 9 8"   "65 65 45 13 10"
  "257 257 20 6 8" "257 257 45 13 10"
  "120 120 20 6 8" "120 120 45 9 10" "120 120 20 13 8"
  "65 64 45 6 8"   "65 257 20 13 8"  "257 120 45 9 8"
  "200 200 20 6 8" "72 72 45 13 10"
)

pass=0; fail=0; failed=()
for cell in "${CELLS[@]}"; do
  read -r w h qp p bd <<<"$cell"
  tag="${w}x${h}_q${qp}_p${p}_bd${bd}"
  pfx="$OUT/$tag"
  if ! SVTAV1_BD="$bd" timeout 180 "$BIN" gradient "$w" "$h" "$qp" "$p" "$pfx" >/dev/null 2>"$pfx.err"; then
    if grep -q "panicked at" "$pfx.err"; then
      fail=$((fail+1)); failed+=("$tag PANIC: $(grep -m1 'panicked at' "$pfx.err" | sed 's/.*panicked at //')")
    else
      fail=$((fail+1)); failed+=("$tag RUNERR")
    fi
    continue
  fi
  if [ ! -s "$pfx.obu" ]; then
    fail=$((fail+1)); failed+=("$tag NO-OUTPUT"); continue
  fi
  if "$AOMDEC" "$pfx.obu" -o /dev/null >/dev/null 2>&1; then
    pass=$((pass+1))
  else
    fail=$((fail+1)); failed+=("$tag DECODE-FAIL")
  fi
  rm -f "$pfx.obu" "$pfx.yuv"
done
rm -rf "$OUT"
echo "arbitrary-size robustness: $pass / $((pass+fail)) panic-free + aomdec-decodable"
if [ "$fail" -gt 0 ]; then printf 'FAILED: %s\n' "${failed[@]}"; exit 1; fi

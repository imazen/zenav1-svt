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
# CONTENT (added 2026-08-03): every cell runs on BOTH `gradient` AND `screen`.
# This gate previously encoded `gradient` only, and `gradient` never arms the
# screen-content detector — so palette and IntraBC were switched OFF in every
# cell and the gate could not observe the code paths they reach. That was not
# hypothetical: `tools/identity_full_8bit.sh` found a real OOB panic in
# `intrabc_hash.rs` on a 32x32 SCREEN frame at preset 0 (a `usize` underflow
# where C uses a signed int), which this gate ran straight past because its
# equivalent cells carried gradient. A panic-freedom gate that cannot arm half
# the encoder's tools is not a panic-freedom gate.
#
# Env: AOMDEC (path to aomdec; auto-detected from common build dirs otherwise).
#      ASR_CONTENTS to override the content list (default "gradient screen").
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib_nice.sh
. "$HERE/lib_nice.sh"
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
if ! CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" $LOWPRI \
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
  # --- SB128-triggering (>=165,120 px @ preset 0/1) + partial/odd SB128 ---
  # These exercise the SB128 encode path (use_128x128_superblock=1) incl. the
  # bd10 x SB128 combo; presets 3/13 at the same size are the SB64 control.
  "512 384 32 0 8"  "512 384 32 1 10"  "448 384 32 0 10"
  "512 512 32 1 8"  "456 392 32 0 8"   "456 392 32 1 10"   # 456x392 = partial SB128
  "513 385 32 0 8"  "520 392 32 1 10"                       # odd + partial SB128
  "512 384 32 3 8"                                          # SB64 control at an SB128 size
)

read -r -a CONTENTS <<<"${ASR_CONTENTS:-gradient screen}"

# Sub-64 dims that specifically reproduce the intrabc_hash underflow class:
# a picture SMALLER than a hash block, with the screen tools armed.
CELLS+=(
  "32 32 20 0 8"   "32 32 45 0 10"   "32 32 20 1 8"
  "48 48 20 0 8"   "48 48 45 1 10"
  "16 16 20 0 8"   "24 24 45 0 8"
)

pass=0; fail=0; refused=0; failed=(); refused_cells=()
for content in "${CONTENTS[@]}"; do
for cell in "${CELLS[@]}"; do
  read -r w h qp p bd <<<"$cell"
  tag="${content}_${w}x${h}_q${qp}_p${p}_bd${bd}"
  pfx="$OUT/$tag"
  SVTAV1_BD="$bd" timeout 180 "$BIN" "$content" "$w" "$h" "$qp" "$p" "$pfx" \
    >/dev/null 2>"$pfx.err"
  rc=$?
  if [ "$rc" -eq 3 ]; then
    # identity_run exit 3 = the encoder REFUSED this configuration with a typed
    # error. That is the CORRECT behaviour for a config outside the verified
    # envelope (rust/CLAUDE.md: refuse, never emit a plausible-but-wrong
    # stream), so it is neither a pass nor a failure here — this gate asks
    # "does it crash or produce an undecodable stream", and a refusal does
    # neither. Counted and listed so a refusal can never be mistaken for
    # coverage.
    refused=$((refused+1)); refused_cells+=("$tag: $(sed -n 's/.*REFUSED by the encoder: //p' "$pfx.err" | head -1)")
    continue
  fi
  if [ "$rc" -ne 0 ]; then
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
done
rm -rf "$OUT"
echo "arbitrary-size robustness: $pass / $((pass + fail)) panic-free + aomdec-decodable" \
     "($refused refused as out-of-envelope)"
if ((refused)); then
  echo "  REFUSED (typed error, NOT a crash — the correct out-of-envelope behaviour):"
  printf '    %s\n' "${refused_cells[@]}"
fi
if [ "$fail" -gt 0 ]; then printf 'FAILED: %s\n' "${failed[@]}"; exit 1; fi

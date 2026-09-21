#!/usr/bin/env bash
# MONOCHROME INTER DECODER-EQUALITY GATE — the witness for the chroma-free
# inter arm (`chroma.is_none()` frames through the normal funnel).
#
# WHAT IT ASSERTS, in order:
#   1. aomdec == dav1d == ENCODER RECON on every frame, byte-for-byte.
#      The refusal this gate replaced cited a mono inter stream that BOTH
#      decoders rejected; two independent decoders agreeing with our own
#      reconstruction is the whole claim — "parses" alone is not.
#   2. REAL INTER USAGE (anti-vacuity): `SVTAV1_INTERDBG` must report at
#      least one IDBG block decision, and the shifted-content legs must
#      produce at least one NONZERO MV. A run that silently coded every
#      frame intra would pass leg 1 and must fail here.
#   3. qp0 mono inter REFUSES — the mono lossless arm has no inter WHT
#      residual path, so a qp0 "inter" stream would be legal but silently
#      all-intra (this gate's own vacuity leg caught exactly that on
#      2026-09-21). The refusal is the correct behavior.
#
# COVERAGE mirrors the measured landing: synthetic diag/screen/gradient
# generators with a 13-px-per-frame shift (nonzero integer MVs), a real
# clip (fourpeople, natural motion), presets {0,6,8,13}, sizes
# 64x64..256x128, sb64 and sb128, a 6-frame inter-on-inter chain, a qp0
# lossless leg and a 10-bit leg.
#
# MEASURED 2026-09-21: all legs byte-identical across the three
# reconstructions. The historical corrupt-stream defect predated the
# format-agnostic inter correctness landings (write-time
# overlappable_neighbors/num_proj_ref derivation, recon-only eob
# preservation); the mono arm inherited those fixes.
#
# Usage: tools/mono_inter_gate.sh
# Env:   AOMDEC DAV1D
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
RUN="$HERE/identity_run"
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
AOMDEC="${AOMDEC:-aomdec}"
DAV1D="${DAV1D:-dav1d}"

for tool in "$AOMDEC" "$DAV1D"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "mono inter gate: $tool not on PATH -- this gate needs TWO" >&2
        echo "  independent decoders; refusing to run with fewer." >&2
        exit 1
    fi
done

W=${TMPDIR:-$HOME/tmp}/mono-inter.$$
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

pass=0; fail=0; declare -a failed

# one mono inter leg: encode, decode with BOTH decoders, compare all three
# reconstructions per frame. args: label content w h qp preset nframes [extra-env]
check() {
    local label=$1 content=$2 w=$3 h=$4 qp=$5 preset=$6 nf=$7
    shift 7
    local out="$W/$label"; mkdir -p "$out"
    if ! env -u SVTAV1_BD SVTAV1_MONO=1 "$@" \
        SVTAV1_FRAMES="$nf" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        SVTAV1_INTERDBG=1 SVTAV1_FINAL_RECON="$out/rec" \
        "$RUN" "$content" "$w" "$h" "$qp" "$preset" "$out/p" \
        >"$out/stdout.txt" 2>"$out/trace.txt"; then
        fail=$((fail+1)); failed+=("$label encode-fail/refused"); return
    fi
    # anti-vacuity: at least one inter block decision...
    if ! grep -q '^IDBG' "$out/trace.txt"; then
        fail=$((fail+1)); failed+=("$label NO-INTER-BLOCKS (vacuous)"); return
    fi
    # ...and on shifted content, at least one NONZERO mv (only checked when
    # the caller shifted the content: near-static clips legitimately code
    # all-(0,0) blocks).
    if [ "${WANT_NONZERO_MV:-0}" = 1 ] \
        && ! grep -qE 'mv=\((-?[1-9][0-9]*|0,-?[1-9][0-9]*)' "$out/trace.txt"; then
        fail=$((fail+1)); failed+=("$label ALL-ZERO-MV (no motion used)"); return
    fi
    if ! "$AOMDEC" --rawvideo -o "$out/aom.yuv" "$out/p.obu" >/dev/null 2>&1; then
        fail=$((fail+1)); failed+=("$label aomdec-reject"); return
    fi
    if ! "$DAV1D" -i "$out/p.obu" -o "$out/dav.yuv" >/dev/null 2>&1; then
        fail=$((fail+1)); failed+=("$label dav1d-reject"); return
    fi
    BDEPTH=${BDEPTH:-8} python3 - "$out" "$w" "$h" "$nf" "$qp" <<'PY'
import sys, os
d, w, h, n, qp = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), \
                 int(sys.argv[4]), int(sys.argv[5])
step = 2 if int(os.environ.get('BDEPTH', '8')) > 8 else 1
fl = w * h * step
aom = open(f'{d}/aom.yuv', 'rb').read()
dav = open(f'{d}/dav.yuv', 'rb').read()
src = open(f'{d}/p.yuv', 'rb').read()
# p.yuv is i420 (u16 samples at bd10): per-frame bytes = luma*3/2 in the
# same byte units `fl` already carries, and the mono luma plane leads it.
src_fl = fl * 3 // 2
if len(aom) < n * fl or len(dav) < n * fl:
    print(f'  decode short: aom={len(aom)} dav={len(dav)} need {n * fl}')
    sys.exit(1)
for i in range(n):
    a = aom[i * fl:(i + 1) * fl]; v = dav[i * fl:(i + 1) * fl]
    rec = open(f'{d}/rec.f{i}', 'rb').read()[:fl]
    if a != v:
        print(f'  f{i}: aomdec != dav1d'); sys.exit(1)
    if a != rec:
        ny = sum(x != y for x, y in zip(a, rec))
        print(f'  f{i}: aomdec != recon ({ny} bytes)'); sys.exit(1)
    if qp == 0:  # lossless oracle: decode must equal SOURCE luma
        sy = src[i * src_fl:i * src_fl + fl]
        if a != sy:
            print(f'  f{i}: qp0 NOT LOSSLESS vs source'); sys.exit(1)
sys.exit(0)
PY
    if [ $? -ne 0 ]; then
        fail=$((fail+1)); failed+=("$label recon/decode mismatch"); return
    fi
    pass=$((pass+1))
}

# synthetic 13px-shift legs — real nonzero motion — x presets x dims
WANT_NONZERO_MV=1 check "diag-128-p0"   diag     128 128 30 0  3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "diag-128-p6"   diag     128 128 30 6  3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "diag-128-p13"  diag     128 128 30 13 3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "screen-64-p0"  screen    64  64 30 0  3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "screen-64-p13" screen    64  64 30 13 3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "grad-256x128-p0"  gradient 256 128 30 0  3 SVTAV1_FRAME_SHIFT=13
WANT_NONZERO_MV=1 check "grad-128x64-p13"  gradient 128  64 30 13 3 SVTAV1_FRAME_SHIFT=13
# sb128
WANT_NONZERO_MV=1 check "diag-sb128"    diag     128 128 30 6  3 SVTAV1_FRAME_SHIFT=13 SVTAV1_SB=128
# inter-on-inter chain, screen content at a mid preset
WANT_NONZERO_MV=1 check "chain6-screen-p8" screen 128 128 30 8 6 SVTAV1_FRAME_SHIFT=13
# qp0 mono inter must REFUSE — the mono lossless arm has no inter WHT path,
# so a qp0 mono "inter" frame is legal but silently all-intra (measured:
# zero inter block decisions). The refusal is the honest answer.
{
    out="$W/qp0-refuse"; mkdir -p "$out"
    env -u SVTAV1_BD SVTAV1_MONO=1 SVTAV1_FRAMES=3 SVTAV1_INTRA_PERIOD=64 \
        SVTAV1_HIER_LEVELS=0 SVTAV1_FRAME_SHIFT=13 \
        "$RUN" diag 128 128 0 6 "$out/p" >"$out/stdout.txt" 2>"$out/trace.txt"
    # a REFUSED encode exits nonzero; the refusal text (stderr) must be the
    # qp0 one
    if grep -q "not implemented outside 8-bit" "$out/stdout.txt" "$out/trace.txt"; then
        pass=$((pass+1))
    else
        fail=$((fail+1)); failed+=("qp0-refuse did-not-refuse")
    fi
}
# 10-bit mono inter
BDEPTH=10 WANT_NONZERO_MV=1 check "bd10-diag-p6" diag 128 128 30 6 3 SVTAV1_FRAME_SHIFT=13 SVTAV1_BD=10
# real content (natural motion; nonzero MVs expected but not pinned)
if [ -f "$ASSETS/fourpeople_128x128_8f.i420" ]; then
    check "fourpeople-p6" "rawseq:$ASSETS/fourpeople_128x128_8f.i420" 128 128 30 6 4
else
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" \
        && check "fourpeople-p6" "rawseq:$ASSETS/fourpeople_128x128_8f.i420" 128 128 30 6 4 \
        || { fail=$((fail+1)); failed+=("fourpeople-p6 no-asset"); }
fi

echo "mono_inter_gate: $pass pass / $fail fail"
for f in "${failed[@]:-}"; do [ -n "$f" ] && echo "  FAIL $f"; done
[ "$fail" -eq 0 ]

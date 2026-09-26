#!/usr/bin/env bash
# QP 0 (CODED-LOSSLESS) INTER GATE — the witness for the 8-bit 4:2:0
# coded-lossless inter arm, which `lossless_config_error` now admits.
#
# WHAT IT ASSERTS, in order:
#   1. DECODE == SOURCE on every frame. At base_q_idx 0 the contract is
#      lossless, so the oracle is the source itself — stronger than
#      recon==aomdec. Any block whose residual does not invert exactly,
#      or that is marked skip over a non-exact prediction, shows here.
#   2. REAL INTER USAGE (anti-vacuity): `SVTAV1_INTERDBG` must report at
#      least one IDBG block decision on every encode. A qp0 run that fell
#      back to all-intra would pass leg 1 and must fail this one — that
#      is exactly what the 4:4:4 arm does today, which is why it stays
#      refused (leg 4 pins it).
#   3. sb128 at 8-bit 4:2:0: same lossless decode.
#   4. REFUSALS hold: 10-bit inter qp0 (per-block vs per-TXB intra
#      prediction divergence — measured: encoder recon == source but
#      aomdec disagrees) and 4:4:4 inter qp0 (no inter WHT arm; stream is
#      legal but silently all-intra) refuse explicitly, at frame 0.
#
# MEASURED 2026-09-21: 8-bit 4:2:0 funnel path coded real inter blocks
# (NearestMv/NewMv/GlobalMv+skip-mode candidates incl. WarpedCausal) with
# WHT residuals; aomdec output byte-identical to source across presets
# 0/6/13, sb64 and sb128. The skip-mode arbitration needed prediction
# dists that coded_lossless had suppressed (mds3 `svt_aom_full_cost`
# arm) — fixed in the same change.
#
# ASSETS are the same public-domain Derf clips `video_selfcheck_gate.sh`
# uses and are fetched the same way; there is no silent skip.
#
# Usage: tools/qp0_inter_gate.sh
# Env:   AOMDEC
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS=$(cd "$HERE/.." && pwd)
RUN="$HERE/identity_run"
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
AOMDEC="${AOMDEC:-aomdec}"

if ! command -v "$AOMDEC" >/dev/null 2>&1; then
    echo "qp0 inter gate: aomdec not on PATH -- this gate needs an" >&2
    echo "  INDEPENDENT decoder and will not fall back to the port's own." >&2
    exit 1
fi
if [ ! -f "$ASSETS/fourpeople_128x128_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "qp0 inter gate: could not obtain assets" >&2; exit 1; }
fi

W=${TMPDIR:-$HOME/tmp}/qp0-inter.$$
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

pass=0; fail=0; declare -a failed

# leg: 8-bit 4:2:0 qp0 multi-frame — decode must equal SOURCE, and the
# trace must contain at least one inter (IDBG) block decision.
check_lossless() { # label size preset nframes [sb]
    local label=$1 size=$2 preset=$3 nf=$4 sb=${5:-}
    local out="$W/$label"; mkdir -p "$out"
    local w=${size%x*} h=${size#*x}
    local clip="$ASSETS/fourpeople_${size}_8f.i420"
    if [ ! -f "$clip" ]; then clip="$ASSETS/johnny_${size}_8f.i420"; fi
    if [ ! -f "$clip" ]; then
        fail=$((fail+1)); failed+=("$label missing-asset"); return
    fi
    local sbenv=()
    [ -n "$sb" ] && sbenv=(SVTAV1_SB="$sb")
    if ! env -u SVTAV1_BD -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM \
        -u SVTAV1_FRAME_ZOOM_DEN "${sbenv[@]}" \
        SVTAV1_FRAMES="$nf" SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        SVTAV1_INTERDBG=1 SVTAV1_FINAL_RECON="$out/rec" \
        "$RUN" "rawseq:$clip" "$w" "$h" 0 "$preset" "$out/p" \
        >"$out/stdout.txt" 2>"$out/trace.txt"; then
        fail=$((fail+1)); failed+=("$label encode-fail/refused"); return
    fi
    if ! grep -q '^IDBG' "$out/trace.txt"; then
        fail=$((fail+1)); failed+=("$label NO-INTER-BLOCKS (vacuous)"); return
    fi
    if ! "$AOMDEC" --rawvideo -o "$out/dec.yuv" "$out/p.obu" >/dev/null 2>&1; then
        fail=$((fail+1)); failed+=("$label aomdec-reject"); return
    fi
    python3 - "$out" "$w" "$h" "$nf" <<'PY'
import sys
d, w, h, n = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
fl = w * h + 2 * (w // 2) * (h // 2)
dec = open(f'{d}/dec.yuv', 'rb').read()
src = open(f'{d}/p.yuv', 'rb').read()
if len(dec) < n * fl:
    print(f'  decode short: {len(dec)} < {n * fl}'); sys.exit(1)
for i in range(n):
    if dec[i * fl:(i + 1) * fl] != src[i * fl:(i + 1) * fl]:
        ny = sum(a != b for a, b in
                 zip(dec[i * fl:i * fl + w * h], src[i * fl:i * fl + w * h]))
        print(f'  f{i}: NOT LOSSLESS (y diffs {ny})'); sys.exit(1)
sys.exit(0)
PY
    if [ $? -ne 0 ]; then
        fail=$((fail+1)); failed+=("$label not-lossless"); return
    fi
    pass=$((pass+1))
}

# leg: refusal check — encode must REFUSE at frame 0 with the qp0-inter
# envelope message.
check_refusal() { # label bd|fmt size
    local label=$1 kind=$2 size=$3
    local out="$W/$label"; mkdir -p "$out"
    local w=${size%x*} h=${size#*x}
    local clip="$ASSETS/fourpeople_${size}_8f.i420"
    if [ "$kind" = bd10 ]; then
        env -u SVTAV1_FRAME_SHIFT SVTAV1_BD=10 \
            SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
            "$RUN" "rawseq:$clip" "$w" "$h" 0 6 "$out/p" \
            >"$out/stdout.txt" 2>"$out/trace.txt"
    else # 444 via the probe
        "$HERE/example" probe_444_dup "$out" "$size" 0 6 shift \
            >"$out/stdout.txt" 2>"$out/trace.txt"
    fi
    if grep -q "not implemented outside 8-bit" "$out/stdout.txt" "$out/trace.txt"; then
        pass=$((pass+1)); return
    fi
    fail=$((fail+1)); failed+=("$label did-not-refuse")
}

for p in 0 6 13; do
    check_lossless "sb64-128-p$p" 128x128 "$p" 4
done
check_lossless "sb128-128-p6" 128x128 6 4 128
check_lossless "sb64-256-p6" 256x256 6 4
check_refusal "refuse-bd10" bd10 128x128
check_refusal "refuse-444" fmt444 128x128

echo "qp0_inter_gate: $pass pass / $fail fail"
for f in "${failed[@]:-}"; do [ -n "$f" ] && echo "  FAIL $f"; done
[ "$fail" -eq 0 ]

#!/usr/bin/env bash
# deep-search-v1 positive-control gate.
#
# What it asserts (anti-vacuity contract):
#   fires-*       — SVTAV1_DEEP_SEARCH=1 CHANGES the stream at preset 8 on
#                   real content (bytes differ). If the arm never reaches
#                   the funnel these rows fail, not pass.
#   decodes-*     — the deep-search stream decodes under aomdec. The first
#                   implementation desynced here (seq enable_filter_intra=0
#                   while the -1 funnel emits use_filter_intra symbols);
#                   the seq-bits patch that fixed it is what this row
#                   keeps alive.
#   recon-*       — the encoder's own final reconstruction
#                   (SVTAV1_FINAL_RECON) is BYTE-IDENTICAL to aomdec's
#                   decode of the deep-search stream: decoder-exact, not
#                   merely parseable.
#   inert-m1      — at preset -1 the arm is a no-op (byte-identical), so
#                   the deepest native tier is untouched.
#   refuse-video  — ZenEnhancements::validate refuses the arm on an inter
#                   (multi-frame) encode: the encode must FAIL with the
#                   env flag set and succeed without it.
#
# A "deep" stream that parses but reconstructs differently from the
# encoder's own picture is a FAIL — the gate compares recon bytes, not
# just decode success.

set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IRUN="$ROOT/tools/identity_run"
AOMDEC=${AOMDEC:-aomdec}
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
pass=0; fail=0
ok()  { pass=$((pass+1)); echo "PASS $1"; }
bad() { fail=$((fail+1)); echo "FAIL $1"; }

IMGROOT="${IMAZEN26:-/home/lilith/work/zen/imazen-26-png-v3/png-v3}"
IMG=$(find "$IMGROOT/1000-lilith-photos-general" -name '*.png' | sort | head -1)
SCR=$(find "$IMGROOT/8000-lilith-mobile-screenshots" -name '*.png' | sort | head -1)
[ -f "$IMG" ] || { echo "deep_search_gate: no photo corpus image"; exit 2; }
[ -f "$SCR" ] || { echo "deep_search_gate: no screenshot corpus image"; exit 2; }

enc() { # env-deep(0|1) out-prefix preset qp src w
    local deep=$1 out=$2 p=$3 qp=$4 src=$5 w=${6:-512}
    if [ "$deep" = 1 ]; then
        SVTAV1_DEEP_SEARCH=1 "$IRUN" "$src" "$w" "$w" "$qp" "$p" "$out" >/dev/null 2>&1
    else
        "$IRUN" "$src" "$w" "$w" "$qp" "$p" "$out" >/dev/null 2>&1
    fi
}

# --- fires + decodes on real content at p8 ---
for spec in "photo crop:$IMG" "screen crop:$SCR"; do
    name=${spec%% *}; src=${spec#* }
    enc 0 "$W/${name}_d0" 8 20 "$src"
    enc 1 "$W/${name}_d1" 8 20 "$src"
    if [ -s "$W/${name}_d1.obu" ] && ! cmp -s "$W/${name}_d0.obu" "$W/${name}_d1.obu"; then
        ok "fires-$name"
    else
        bad "fires-$name"
    fi
    if "$AOMDEC" --rawvideo -o "$W/${name}_d1.i420" "$W/${name}_d1.obu" >/dev/null 2>&1; then
        ok "decodes-$name"
    else
        bad "decodes-$name"
    fi
done

# --- recon == decode on the deep stream ---
SVTAV1_DEEP_SEARCH=1 SVTAV1_FINAL_RECON="$W/recon.i420" \
    "$IRUN" "crop:$IMG" 512 512 20 8 "$W/ph_recon" >/dev/null 2>&1
"$AOMDEC" --rawvideo -o "$W/recon_dec.i420" "$W/ph_recon.obu" >/dev/null 2>&1
if [ -s "$W/recon.i420" ] && cmp -s "$W/recon.i420" "$W/recon_dec.i420"; then
    ok "recon-photo"
else
    bad "recon-photo"
fi

# --- p-1 inertness ---
enc 0 "$W/m1_off" -1 30 "gradient" 128
enc 1 "$W/m1_on"  -1 30 "gradient" 128
if cmp -s "$W/m1_off.obu" "$W/m1_on.obu"; then
    ok "inert-m1"
else
    bad "inert-m1"
fi

# --- video refusal: 2-frame encode must fail WITH the arm, pass without ---
if SVTAV1_FRAMES=2 "$IRUN" "gradient" 128 128 30 8 "$W/vid_off" >/dev/null 2>&1 \
   && ! { SVTAV1_FRAMES=2 SVTAV1_DEEP_SEARCH=1 "$IRUN" "gradient" 128 128 30 8 "$W/vid_on" >/dev/null 2>&1; }; then
    ok "refuse-video"
else
    bad "refuse-video"
fi

# --- mono refusal: no chroma planes -> outside the measured envelope ---
if SVTAV1_MONO=1 "$IRUN" "gradient" 128 128 30 8 "$W/mono_off" >/dev/null 2>&1 \
   && ! { SVTAV1_MONO=1 SVTAV1_DEEP_SEARCH=1 "$IRUN" "gradient" 128 128 30 8 "$W/mono_on" >/dev/null 2>&1; }; then
    ok "refuse-mono"
else
    bad "refuse-mono"
fi

echo "deep_search_gate: $pass pass / $fail fail"
[ "$fail" -eq 0 ]

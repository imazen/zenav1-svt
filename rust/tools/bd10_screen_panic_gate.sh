#!/usr/bin/env bash
# bd10 SCREEN-CONTENT panic-freedom gate.
#
# WHY: the bd10 (u16) leaf funnel does NOT port the luma palette (#71) search —
# a surviving palette candidate reaches the bd10 full-RD `tx_unit_hbd` with a u8
# w*h palette prediction where the hbd path indexes a u16 buffer, panicking with
# an out-of-bounds (leaf_funnel.rs `residual.push(src[..]-pred[..])`). This fires
# on REAL screen content at bd10 (palette is active at preset<=7 via sc_class5),
# i.e. a PANIC ON THE PUBLIC `encode_frame_420` API — discovered by the
# wider-corpus sweep (tools/wider_corpus_sweep.sh, 2026-07-22: 80 rs-err cells,
# all screen+bd10+preset{0,6}). Fixed by gating palette injection out of the bd10
# funnel (leaf_funnel.rs `!bd10_funnel`), so those leaves decide among the ported
# non-palette hbd modes and yield a VALID DECODABLE stream instead of crashing.
#
# This gate PINS that fix: every gb82-sc screenshot, center-cropped to 512x512 and
# encoded at bd10 across the palette-active presets {0,6} and a qp spread, must
# (a) encode without panicking and (b) be decodable by aomdec. It deliberately
# does NOT assert byte-identity to C — the bd10 palette path is unported, so those
# streams legitimately DIFFER (byte-exact bd10 palette is a future #71 port); the
# contract here is panic-freedom + decodability, matching the zenav1-aom panic-
# freedom discipline for the public encode API.
#
# CORPUS: gb82-sc (10 screen/UI/text PNGs). Override with BD10_SCREEN_CORPUS=<dir>.
# If absent the gate FAILS LOUDLY (never a silent skip).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

AOMDEC="${AOMDEC:-}"
if [ -z "$AOMDEC" ]; then
    for c in aomdec /root/aomdec-debug/aomdec; do command -v "$c" >/dev/null 2>&1 && { AOMDEC=$c; break; }; done
fi
command -v "$AOMDEC" >/dev/null 2>&1 || { echo "bd10 screen panic gate: aomdec not found (set AOMDEC=)" >&2; exit 2; }

CORPUS="${BD10_SCREEN_CORPUS:-/root/work/codec-corpus/gb82-sc}"
if [ ! -d "$CORPUS" ]; then
    echo "bd10 screen panic gate: corpus not found at $CORPUS" >&2
    echo "  set BD10_SCREEN_CORPUS=<dir of screen PNGs> to point at it." >&2
    exit 1
fi
mapfile -t IMAGES < <(ls "$CORPUS"/*.png 2>/dev/null | sort)
[ "${#IMAGES[@]}" -gt 0 ] || { echo "bd10 screen panic gate: no PNGs in $CORPUS" >&2; exit 1; }

# Presets 0 and 6 = the palette-active (sc_class5-gated, preset<=7) band that
# reaches the bd10 palette candidate. p10/p13 map to M9 (palette off) and never
# hit the path, so they are not needed here.
PRESETS=(0 6)
QPS=(5 32 63)

OUT="${TMPDIR:-/tmp}/bd10screenpanic.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
pass=0; fail=0; failed=()

for png in "${IMAGES[@]}"; do
    stem=$(basename "$png" .png)
    for p in "${PRESETS[@]}"; do
        for qp in "${QPS[@]}"; do
            tag="${stem}_bd10_p${p}_q${qp}"
            if ! SVTAV1_BD=10 "$HERE/identity_run" "crop:$png" 512 512 "$qp" "$p" "$OUT/rs" \
                    >/dev/null 2>"$OUT/err"; then
                fail=$((fail + 1)); failed+=("${tag}[PANIC: $(grep -oE 'panicked at [^ ]+' "$OUT/err" | head -1)]"); continue
            fi
            if ! "$AOMDEC" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
                fail=$((fail + 1)); failed+=("${tag}[undecodable]"); continue
            fi
            pass=$((pass + 1))
        done
    done
done

total=$((pass + fail))
echo "bd10 screen panic-freedom: $pass / $total encode-without-panic + decodable"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

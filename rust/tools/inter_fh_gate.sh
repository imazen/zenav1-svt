#!/usr/bin/env bash
# INTER frame-header gate — the campaign's first inter-frame scoreboard cell.
#
# WHAT IT ASSERTS, and why it is shaped this way.
#
# The inter frame HEADER is derived (reference structure from
# `port_picstruct`, tool ladders from `sig_deriv_mode_decision_config_default`);
# the inter TILE is not ported. So the frame's BYTES cannot match C yet, and a
# byte gate would just be a permanently red cell that tells nobody anything.
#
# What CAN be asserted, and is: the frame header's FIELD LAYOUT. Every field of
# frame 1's `uncompressed_header()` must equal C's, with the exception of an
# explicitly listed OPEN set. A gate that simply pinned today's byte string
# would go RED when someone closes an open field — so the rule is a SUBSET
# test: the differing fields must be a subset of $OPEN. Closing one keeps the
# gate green; a NEW divergence, or a field whose presence changed (which shifts
# every field after it), turns it red.
#
# Open set: EMPTY as of 2026-09-01 (docs/INTER-ENCODE-PLAN.md §1r) — the inter
# frame header is byte-identical to C's on this cell, so the subset test is
# currently a plain field-identity assertion. The mechanism is kept rather than
# replaced by a byte compare because the NEXT open field, whenever one appears,
# should be nameable and listable here instead of turning the cell red for
# months. Add a field to $INTER_FH_OPEN only with the measurement that says why.
#
# Frame 0 (the VIDEO-MODE key frame) must stay byte-identical; that half is a
# hard byte assertion.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
W=${1:-64}; H=${2:-64}; QP=${3:-40}; PRESET=${4:-6}; CONTENT=${5:-gradient}
OPEN_DEFAULT=""
OPEN=${INTER_FH_OPEN:-$OPEN_DEFAULT}

OUT="$HERE/../target/inter-fh-gate/${CONTENT}_${W}x${H}_q${QP}_p${PRESET}"
rm -rf "$OUT"; mkdir -p "$OUT"

st=0
SVTAV1_INTER_EXPERIMENTAL=1 \
    "$HERE/identity_diff_inter.sh" "$W" "$H" "$QP" "$PRESET" 2 "$CONTENT" "$OUT" \
    >"$OUT/diff.txt" 2>&1 || st=$?
if [[ $st -eq 3 ]]; then
    echo "inter fh gate: the encoder REFUSED frame 1 — SVTAV1_INTER_EXPERIMENTAL did not reach it" >&2
    cat "$OUT/diff.txt" >&2
    exit 3
fi

# Frame 0 is a hard byte assertion.
if ! cmp -s "$OUT/c.obu.pts0" "$OUT/rs.obu.f0"; then
    echo "inter fh gate: FRAME 0 (video-mode key frame) is no longer byte-identical" >&2
    cat "$OUT/diff.txt" >&2
    exit 1
fi
echo "frame 0: IDENTICAL ($(wc -c <"$OUT/c.obu.pts0" | tr -d ' ') B)"

# Frame 1: the field walk.
python3 "$HERE/fh_fields.py" --index 1 "$OUT/c.obu" "$OUT/rs.obu" >"$OUT/fields.txt"
# NB: `mapfile`/`readarray` do NOT exist in bash 3.2, which is what macOS ships
# and what `identity_full_8bit.sh` was once silently broken by. Plain
# word-splitting only.
diffs=$(grep -- '<-- DIFFERS' "$OUT/fields.txt" | awk '{print $1}' || true)

fail=0
for f in $diffs; do
    found=0
    for o in $OPEN; do [ "$f" = "$o" ] && found=1; done
    if [ $found -eq 0 ]; then
        echo "inter fh gate: UNEXPECTED frame-header divergence: $f" >&2
        fail=1
    fi
done
if [ $fail -ne 0 ]; then
    cat "$OUT/fields.txt" >&2
    exit 1
fi

n=$(printf '%s\n' $diffs | grep -c . || true)
echo "frame 1 header: field-exact except ${n} known-open field(s): ${diffs:-none}"
echo "inter fh gate: PASS  ($OUT/fields.txt)"

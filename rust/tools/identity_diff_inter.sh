#!/usr/bin/env bash
# INTER-frame bitstream-identity harness (campaign chunk C0).
#
# The still-picture sibling is identity_diff.sh; this one encodes a SEQUENCE on
# both sides and compares FRAME BY FRAME, because in a multi-frame stream the
# first divergence of a concatenation is uninformative — frame 0 getting one
# byte longer shifts everything after it.
#
# Usage: identity_diff_inter.sh <width> <height> <cli_qp 0..63> <preset> [frames] [content] [outdir]
#   frames:  default 2 (key + one inter frame — the smallest inter cell)
#   content: uniform | gradient | diag | screen | screenrep | file:<png> (default gradient)
#
# Both sides consume the ONE .yuv the Rust side writes, exactly like the still
# harness. Motion is a horizontal translation of frame 0 (SVTAV1_FRAME_SHIFT,
# default 3 px/frame): a single global integer MV is the right answer for
# nearly every block, so this cell tests the inter PLUMBING, not the search.
#
# GOP: low-delay P, flat (no pyramid), only frame 0 a key frame. Both sides are
# given the same shape (C: SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0
# SVT_PRED_STRUCT=1; Rust: SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0).
#
# Exit status: 0 iff every frame is byte-identical. 3 iff the port REFUSED a
# frame (a refusal is not a crash and not a byte divergence — the gate says so
# in those words, because conflating them is how a missing feature gets read as
# a corruption bug). 4 iff the port CRASHED — see below. 1 for a byte
# divergence, 2 for a usage error.
#
# WHY 4 EXISTS, MEASURED 2026-09-02. A Rust panic exits 101, and this script
# used to propagate that raw status. Every consumer keyed on "3 = refusal" and
# on whether `rs.obu.f0` exists — and frame 0 IS written before a frame-1
# panic, so a crash sailed through both checks and was classified as an
# ordinary frame-1 byte divergence. `inter_byte_matrix.sh` reported EIGHTEEN
# panicking 72x72 cells as F1DIFF, and `docs/INTER-ENCODE-PLAN.md` §1z15's
# "55 F1DIFF cells" therefore counted 18 crashes as divergences for a whole
# chunk. A crash and a wrong byte are not the same defect and must never share
# a verdict.
set -euo pipefail

if [[ $# -lt 4 ]]; then
    echo "usage: $0 <width> <height> <cli_qp 0..63> <preset> [frames] [content] [outdir]" >&2
    exit 2
fi
W=$1; H=$2; QP=$3; PRESET=$4
FRAMES="${5:-2}"
CONTENT="${6:-gradient}"

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
OUTDIR="${7:-$RS_ROOT/target/identity-inter/${CONTENT}_${W}x${H}_q${QP}_p${PRESET}_f${FRAMES}}"
mkdir -p "$OUTDIR"

# 1. Rust side: writes <out>.yuv (N frames), <out>.obu (concatenated) and
#    <out>.obu.f<i> per frame. Exit 3 = the encoder refused a frame.
rs_status=0
SVTAV1_FRAMES="$FRAMES" \
SVTAV1_INTRA_PERIOD="${SVTAV1_INTRA_PERIOD:-64}" \
SVTAV1_HIER_LEVELS="${SVTAV1_HIER_LEVELS:-0}" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$OUTDIR/rs" \
    2>"$OUTDIR/rs.trace" || rs_status=$?

# 2. C side: the SAME .yuv, matched GOP shape. Always run it, even when the
#    port refused — the reference stream is what the next chunk is aimed at,
#    and having it on disk is half the value of this harness.
SVT_FRAMES="$FRAMES" \
SVT_INTRA_PERIOD="${SVT_INTRA_PERIOD:--1}" \
SVT_HIER_LEVELS="${SVT_HIER_LEVELS:-0}" \
SVT_PRED_STRUCT="${SVT_PRED_STRUCT:-1}" \
SVT_TRACE_OUT="${SVT_TRACE_OUT:-/dev/null}" \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$PRESET" \
    "$OUTDIR/rs.yuv" "$OUTDIR/c.obu" 2>"$OUTDIR/c.stderr"

{
    echo "cell: ${CONTENT} ${W}x${H} q${QP} p${PRESET} frames=${FRAMES}"
    echo "gop:  low-delay P, flat, key frame 0 only"
    echo
} > "$OUTDIR/report.txt"

if [[ $rs_status -eq 3 ]]; then
    {
        echo "PORT REFUSED — this is a refusal, not a divergence and not a crash."
        grep -h "REFUSED" "$OUTDIR/rs.trace" || true
        echo
        echo "C reference frames (the target):"
        for f in "$OUTDIR"/c.obu.pts*; do
            [[ -e $f ]] && echo "  $(basename "$f"): $(wc -c <"$f" | tr -d ' ') bytes"
        done
        echo
        echo "Port frames encoded before the refusal:"
        for f in "$OUTDIR"/rs.obu.f*; do
            [[ -e $f ]] && echo "  $(basename "$f"): $(wc -c <"$f" | tr -d ' ') bytes"
        done
    } >> "$OUTDIR/report.txt"
    cat "$OUTDIR/report.txt"
    exit 3
fi
# Any other nonzero status is a CRASH (a Rust panic is 101), and it gets its
# OWN exit code so no caller can classify it as a byte divergence. See the
# status list at the top of this file.
if [[ $rs_status -ne 0 ]]; then
    {
        echo "PORT CRASHED — this is a crash, not a refusal and not a byte divergence."
        echo "identity_run exited $rs_status. The panic, if any:"
        grep -h "panicked at" "$OUTDIR/rs.trace" || echo "  (no panic line in rs.trace)"
    } | tee -a "$OUTDIR/report.txt" >&2
    exit 4
fi

# 3. Per-frame byte comparison.
ok=1
for ((i = 0; i < FRAMES; i++)); do
    c="$OUTDIR/c.obu.pts$i"
    r="$OUTDIR/rs.obu.f$i"
    if [[ ! -e $c || ! -e $r ]]; then
        echo "frame $i: MISSING ($(basename "$c") or $(basename "$r"))" >> "$OUTDIR/report.txt"
        ok=0
        continue
    fi
    cb=$(wc -c <"$c" | tr -d ' ')
    rb=$(wc -c <"$r" | tr -d ' ')
    if cmp -s "$c" "$r"; then
        echo "frame $i: IDENTICAL ($cb B)" >> "$OUTDIR/report.txt"
    else
        off=$(cmp "$c" "$r" 2>&1 | sed 's/.*char \([0-9]*\).*/\1/' || echo "?")
        echo "frame $i: DIFFERS (C=${cb}B Rust=${rb}B, first differing byte $off)" >> "$OUTDIR/report.txt"
        ok=0
    fi
done

cat "$OUTDIR/report.txt"
[[ $ok -eq 1 ]] || exit 1

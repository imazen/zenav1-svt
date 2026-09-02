#!/usr/bin/env bash
# CDF-CONTINUATION gate: does the port SAVE the same end-of-frame FRAME_CONTEXT
# the C encoder saves onto its reference?
#
# A frame whose header names a `primary_ref_frame` starts its tile CDFs from
# the referenced frame's end-of-frame state (`ec_process.c:101-112`), so a
# saved context that differs from C's makes every tile symbol after the first
# wrong — and none of that is visible in a byte count until the inter mode
# decision also matches. This gate compares the saved state DIRECTLY, so the
# store can be proven right long before anything consumes it.
#
# Both dumps are taken AFTER `svt_av1_reset_cdf_symbol_counters`, i.e. exactly
# the bytes that land in `EbReferenceObject::frame_context`:
#   C     tools/capture_c_trace/wrap_recon.c's
#         __wrap_svt_av1_reset_cdf_symbol_counters   (SVT_FCTX_OUT)
#   port  crate::port_frame_cdf::FrameCdfs::dump_to  (SVTAV1_FCTX_OUT)
#
# Usage: fctx_gate.sh [width] [height] [cli_qp] [preset] [frames] [content]
#        (defaults: the inter campaign's reference cell, 64 64 40 6 2 gradient)
#
# REQUIRES DOCKER, and says so rather than skipping: the C side needs
# `-Wl,--wrap`, which Apple's ld64 lacks, so the oracle runs in the Linux
# container (docs/WORKING-ON-THIS.md §5). A missing docker daemon is a HARNESS
# FAILURE, never a pass — a gate that silently skips is the trap that section
# was written about.
set -euo pipefail

W=${1:-64}
H=${2:-64}
QP=${3:-40}
PRESET=${4:-6}
FRAMES=${5:-2}
CONTENT=${6:-gradient}

HERE=$(cd "$(dirname "$0")" && pwd)
WORK="${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}/fctx-gate"
mkdir -p "$WORK"

if ! command -v docker >/dev/null 2>&1; then
    echo "fctx gate: FAIL — no docker. The C oracle needs -Wl,--wrap, which" >&2
    echo "           Apple ld64 lacks, so it runs in tools/ctrace-linux." >&2
    echo "           This is a harness failure, not a parity result." >&2
    exit 2
fi

rm -f "$WORK/c.fctx" "$WORK/rs.fctx"

# 1. Port side. It writes the .yuv both sides consume, so it runs first.
SVTAV1_FCTX_OUT="$WORK/rs.fctx" \
    SVTAV1_INTER_EXPERIMENTAL=1 \
    SVTAV1_FRAMES="$FRAMES" \
    SVTAV1_INTRA_PERIOD="${SVTAV1_INTRA_PERIOD:-64}" \
    SVTAV1_HIER_LEVELS="${SVTAV1_HIER_LEVELS:-0}" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$WORK/rs" \
    >"$WORK/rs.trace" 2>&1

# 2. C side, on the SAME .yuv and the same GOP shape.
SVT_FRAMES="$FRAMES" \
    SVT_INTRA_PERIOD="${SVT_INTRA_PERIOD:--1}" \
    SVT_HIER_LEVELS="${SVT_HIER_LEVELS:-0}" \
    SVT_PRED_STRUCT="${SVT_PRED_STRUCT:-1}" \
    SVT_FCTX_OUT="$WORK/c.fctx" \
    "$HERE/ctrace-linux/run.sh" "$W" "$H" "$QP" "$PRESET" \
    "$WORK/rs.yuv" "$WORK/c.obu" 8 >"$WORK/c.log" 2>&1

for f in "$WORK/c.fctx" "$WORK/rs.fctx"; do
    if [[ ! -s "$f" ]]; then
        echo "fctx gate: FAIL — $f is empty; the dump never fired." >&2
        echo "           (Anti-vacuity: an absent dump and an equal one must" >&2
        echo "            never look the same.)" >&2
        exit 2
    fi
done

# Frame 0 is the one a later frame restores FROM, so it is the frame under
# test. Frame 1's saved context can only match once the inter tile does.
echo "== CDF-continuation gate: $CONTENT ${W}x${H} q$QP p$PRESET frames=$FRAMES =="
# `set -e` would abort on a nonzero exit before `rc=$?` could read it, so the
# comparison runs in an `if` (which suspends errexit) rather than bare.
rc=0
if ! python3 "$HERE/fctx_diff.py" "$WORK/c.fctx" "$WORK/rs.fctx" --frame=0 --max-fields=20; then
    rc=1
fi
if [[ $rc -eq 0 ]]; then
    echo "fctx gate: PASS  ($WORK)"
else
    echo "fctx gate: FAIL  ($WORK)"
fi
exit $rc

#!/usr/bin/env bash
# PERCEPTUAL RD-EQUIVALENCE diagnostic for a byte-diverging inter cell.
#
# WHAT THIS MEASURES — and what it does not. The byte census
# (`tools/video_census_gate.sh`) is a strict ratchet: any divergence from C's
# stream fails the cell. That is the right parity contract, but it cannot
# distinguish "the port picked a different-but-equivalent winner" from "the
# port lost rate-distortion". This script encodes the same sequence with both
# encoders and scores the decoded streams against the uncompressed source
# with cvvdp's VIDEO path (temporal channels, pycvvdp v0.5.7 parity) via the
# `tools/cvvdp_eval` sidecar binary — a diverging cell whose JOD delta is
# within noise at matched bytes is benign MD divergence; a cell where the
# port's JOD is materially LOWER is a real quality regression worth fixing
# before cosmetic byte-chasing.
#
# LIMITS. cvvdp is still-image-plus-temporal scoring; at the 8-frame census
# clips the 9-tap FIR (fps=30) is barely exercised. The YUV->sRGB conversion
# is fixed (BT.601 limited, nearest chroma) for both sides, so only the
# port-minus-C delta is meaningful. Triage evidence, never a parity proof.
#
# Usage: tools/cvvdp_equiv.sh <clip|file.i420> <size> <qp> <preset> [frames]
#   clip:  a name resolvable under $ZENAV1_VIDEO_ASSETS as
#          `<clip>_<W>x<H>_8f.i420`, or a literal .i420 path.
#   size:  W or WxH.
# Env:   CVVDP_EVAL — prebuilt sidecar binary (default: builds
#          tools/cvvdp_eval on demand).
#        CVVDP_DISPLAY — cvvdp display preset (default standard_fhd).
#        CVVDP_LOW_MEM — set to use the scorer's u8 ring window
#          (bit-identical scores, ~4x less RSS at large sizes).
#        CVVDP_KEEP — keep the work directory.
# Output: clip-level cvvdp-video JOD per side + per-frame stills table +
# byte totals. Exit 0 — measurement, not a gate.
set -euo pipefail

if [[ $# -lt 4 ]]; then
    echo "usage: $0 <clip|file.i420> <size> <qp> <preset> [frames]" >&2
    exit 2
fi
CLIP=$1; SIZE=$2; QP=$3; PRESET=$4; FRAMES="${5:-8}"

HERE=$(cd "$(dirname "$0")" && pwd)

if [[ $SIZE == *x* ]]; then
    W=${SIZE%x*}; H=${SIZE#*x}
else
    W=$SIZE; H=$SIZE
fi
if [[ -f $CLIP ]]; then
    YUV=$CLIP
else
    ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
    YUV="$ASSETS/${CLIP}_${W}x${H}_8f.i420"
fi
[[ -f $YUV ]] || { echo "cvvdp_equiv: no input: $YUV" >&2; exit 2; }

EVAL="${CVVDP_EVAL:-}"
if [[ -z $EVAL ]]; then
    EVAL="$HERE/cvvdp_eval/target/release/cvvdp_eval"
    if [[ ! -x $EVAL ]]; then
        ( cd "$HERE/cvvdp_eval" && cargo build --release -q ) \
            || { echo "cvvdp_equiv: build the sidecar first — cd tools/cvvdp_eval && cargo build --release" >&2; exit 2; }
    fi
fi

WORK="${TMPDIR:-$HOME/tmp}/cvvdp-equiv-$$"
mkdir -p "$WORK"
trap '[[ -z ${CVVDP_KEEP:-} ]] && rm -rf "$WORK" || true' EXIT

# Encode both sides (same bytes on the wire is NOT asserted here).
SVTAV1_FRAMES=$FRAMES SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
    CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} \
    "$HERE/identity_run" "rawseq:$YUV" "$W" "$H" "$QP" "$PRESET" "$WORK/rs" \
    2>"$WORK/rs.trace"
SVT_FRAMES=$FRAMES SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$PRESET" \
    "$WORK/rs.yuv" "$WORK/c.obu" 8 2>"$WORK/c.trace"

# Decode + convert + score — all in the sidecar (aomdec inside it).
"$EVAL" "$WORK/rs.yuv" "${W}x${H}" "$WORK/c.obu" "$WORK/rs.obu" \
    --display "${CVVDP_DISPLAY:-standard_fhd}" --fps 30 \
    ${CVVDP_LOW_MEM:+--low-memory}

echo "work dir: $WORK (set CVVDP_KEEP=1 to keep frames/traces)"

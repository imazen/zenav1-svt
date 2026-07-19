#!/usr/bin/env bash
# Bitstream-identity harness: run Rust EncodePipeline and the C reference
# library on IDENTICAL input at a matched still-picture/AVIF CQP config,
# capture both arithmetic-coder op traces, and diff streams + traces.
#
# Usage: identity_diff.sh <width> <height> <cli_qp 0..63> <preset> [content] [outdir]
#   content: uniform (default) | gradient
#
# Outputs under <outdir> (default rust/target/identity/<case>):
#   rs.yuv rs.obu rs.trace   — Rust side (identity_run, symtrace stderr)
#   c.obu c.trace c.stderr   — C side (capture_c_trace, --wrap trace)
#   report.txt               — identity_diff.py output
# Exit status: 0 iff the two streams are byte-identical.
set -euo pipefail

if [[ $# -lt 4 ]]; then
    echo "usage: $0 <width> <height> <cli_qp 0..63> <preset> [uniform|gradient] [outdir]" >&2
    exit 2
fi
W=$1
H=$2
QP=$3
PRESET=$4
CONTENT="${5:-uniform}"

HERE=$(cd "$(dirname "$0")" && pwd)           # rust/tools
RS_ROOT=$(cd "$HERE/.." && pwd)               # rust
OUTDIR="${6:-$RS_ROOT/target/identity/${CONTENT}_${W}x${H}_q${QP}_p${PRESET}}"
mkdir -p "$OUTDIR"

# 1-2. Builds are NOT done here any more: both `capture_c_trace` and
# `identity_run` are wrapper scripts that force their own freshness check (C lib
# from Source/ + relink; cargo build) before exec'ing the real binary. That is
# deliberate — it makes it structurally impossible for this harness, or anyone
# at a shell, to compare against a stale C driver or a stale Rust encoder.
# Do not "optimize" by calling the raw binaries directly.

# 3. Rust encode: writes rs.yuv (shared input) + rs.obu; trace on stderr.
"$HERE/identity_run" \
    "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$OUTDIR/rs" 2>"$OUTDIR/rs.trace"

# 4. C encode of the SAME yuv bytes, AT THE SAME BIT DEPTH.
#
# `SVTAV1_BD` selects the port's depth (identity_run reads it); the C driver
# takes it as argv[7]. Passing it here is not optional: without it the C side
# silently encoded 8-bit while the port encoded 10, and every bd10 report came
# back "STAGE: SH | high_bitdepth C=0 Rust=1" with a nonsense byte delta — a
# comparison of two different configurations dressed up as a divergence. That
# made this differ unusable for the whole bd10 track, which is why bd10
# root-causing has been done with ad-hoc scripts instead.
SVT_TRACE_OUT="$OUTDIR/c.trace" "$HERE/capture_c_trace/capture_c_trace" \
    "$W" "$H" "$QP" "$PRESET" "$OUTDIR/rs.yuv" "$OUTDIR/c.obu" \
    "${SVTAV1_BD:-8}" 2>"$OUTDIR/c.stderr"

# 5. Diff. Concise by default (STAGE + VERDICT); set IDENTITY_VERBOSE=1 for
#    the full field walks + op-context dumps when diagnosing.
set +e
verbose_flag=()
[[ -n "${IDENTITY_VERBOSE:-}" ]] && verbose_flag=(--verbose)
python3 "$HERE/identity_diff.py" \
    --c-obu "$OUTDIR/c.obu" --rust-obu "$OUTDIR/rs.obu" \
    --c-trace "$OUTDIR/c.trace" --rust-trace "$OUTDIR/rs.trace" \
    "${verbose_flag[@]}" \
    | tee "$OUTDIR/report.txt"
rc=${PIPESTATUS[0]}
set -e
echo "artifacts: $OUTDIR" >&2
exit "$rc"

#!/usr/bin/env bash
# Bitstream-identity harness: run Rust EncodePipeline and the C reference
# library on IDENTICAL input at a matched still-picture/AVIF CQP config,
# capture both arithmetic-coder op traces, and diff streams + traces.
#
# Usage: identity_diff.sh <width> <height> <cli_qp 0..63> <preset> [content] [outdir]
#   content: uniform (default) | gradient
#
# Outputs under <outdir> (default svtav1-rs/target/identity/<case>):
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

HERE=$(cd "$(dirname "$0")" && pwd)           # svtav1-rs/tools
RS_ROOT=$(cd "$HERE/.." && pwd)               # svtav1-rs
OUTDIR="${6:-$RS_ROOT/target/identity/${CONTENT}_${W}x${H}_q${QP}_p${PRESET}}"
mkdir -p "$OUTDIR"

# 1. C driver (external build, never part of the cargo workspace).
"$HERE/capture_c_trace/build.sh" >&2

# 2. Rust runner with the symtrace feature.
(cd "$RS_ROOT" && cargo build --release -p svtav1 --features symtrace --example identity_run) >&2

# 3. Rust encode: writes rs.yuv (shared input) + rs.obu; trace on stderr.
"$RS_ROOT/target/release/examples/identity_run" \
    "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$OUTDIR/rs" 2>"$OUTDIR/rs.trace"

# 4. C encode of the SAME yuv bytes.
SVT_TRACE_OUT="$OUTDIR/c.trace" "$HERE/capture_c_trace/capture_c_trace" \
    "$W" "$H" "$QP" "$PRESET" "$OUTDIR/rs.yuv" "$OUTDIR/c.obu" 2>"$OUTDIR/c.stderr"

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

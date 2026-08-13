#!/usr/bin/env bash
# Op-trace localization for ONE cell, with the C side run in the Linux
# container (./run.sh) so the `-Wl,--wrap` interposers exist.
#
# This is `tools/identity_diff.sh` with two differences:
#   * the C driver is the containerised one, so the op trace is real on a
#     macOS host instead of degrading to a byte-only comparison;
#   * `content` is passed through verbatim, so `crop:<png>` / `file:<png>` /
#     `raw:<yuv>` work — issue #15 only reproduces on REAL content, which
#     identity_diff.sh's `uniform|gradient` cases cannot express.
#
# Usage: diff_cell.sh <w> <h> <qp> <preset> <content> [outdir-name]
#   content: uniform | gradient | crop:<png> | file:<png> | raw:<yuv>
# Env: CTRACE_WORK (default ~/tmp/zenav1-ctrace) — outdir lives under it,
#      because the container can only see paths inside that mount.
#      IDENTITY_VERBOSE=1 for the full field walk / op context.
# Exit status: 0 iff the two streams are byte-identical.
set -uo pipefail

[[ $# -ge 5 ]] || {
    echo "usage: $0 <w> <h> <qp> <preset> <content> [outdir-name]" >&2
    exit 2
}
W=$1 H=$2 QP=$3 PRESET=$4 CONTENT=$5
HERE=$(cd "$(dirname "$0")" && pwd)
RS_TOOLS=$(cd "$HERE/.." && pwd)
WORK="${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}"
# Default cell name: content basename, sanitised (a `crop:/a/b.png` cannot be
# a directory component).
NAME="${6:-$(basename "${CONTENT#*:}" .png)_${W}x${H}_q${QP}_p${PRESET}}"
OUT="$WORK/$NAME"
mkdir -p "$OUT"

"$RS_TOOLS/identity_run" "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$OUT/rs" 2>"$OUT/rs.trace" || {
    echo "PORTFAIL" >&2
    exit 3
}

SVT_TRACE_OUT="$OUT/c.trace" "$HERE/run.sh" \
    "$W" "$H" "$QP" "$PRESET" "$OUT/rs.yuv" "$OUT/c.obu" "${SVTAV1_BD:-8}" \
    2>"$OUT/c.stderr" >"$OUT/c.stdout" || {
    tail -20 "$OUT/c.stderr" >&2
    echo "CFAIL" >&2
    exit 4
}

verbose_flag=()
[[ -n "${IDENTITY_VERBOSE:-}" ]] && verbose_flag=(--verbose)
python3 "$RS_TOOLS/identity_diff.py" \
    --c-obu "$OUT/c.obu" --rust-obu "$OUT/rs.obu" \
    --c-trace "$OUT/c.trace" --rust-trace "$OUT/rs.trace" \
    "${verbose_flag[@]}" | tee "$OUT/report.txt"
rc=${PIPESTATUS[0]}
echo "artifacts: $OUT" >&2
exit "$rc"

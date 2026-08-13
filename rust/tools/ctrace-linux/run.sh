#!/usr/bin/env bash
# Run the C reference's `-Wl,--wrap` trace driver inside a Linux container.
#
# Drop-in for `tools/capture_c_trace/capture_c_trace`, with the same argv, on a
# host whose linker has no `--wrap` (Apple ld64). See ./Dockerfile for why.
#
# Usage: run.sh <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu> [bd]
# Env:
#   SVT_TRACE_OUT   host path for the op trace (must live under $CTRACE_WORK)
#   CTRACE_WORK     host scratch dir bind-mounted at /work (default ~/tmp/zenav1-ctrace)
#   CTRACE_PLATFORM docker platform (default: linux/<host arch>)
#
# The .yuv/.obu/.trace paths you pass are HOST paths and must be inside
# $CTRACE_WORK; the container sees them under /work. This is enforced, not
# assumed — a path outside the mount would silently produce a file the host
# never sees.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
IMAGE="${CTRACE_IMAGE:-zenav1-svt-ctrace:1}"
WORK="${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}"
mkdir -p "$WORK"

case "$(uname -m)" in
arm64 | aarch64) HOST_PLATFORM=linux/arm64 ;;
x86_64 | amd64) HOST_PLATFORM=linux/amd64 ;;
*) HOST_PLATFORM="" ;;
esac
PLATFORM="${CTRACE_PLATFORM:-$HOST_PLATFORM}"
[[ -n "$PLATFORM" ]] || {
    echo "error: unknown host arch $(uname -m) — set CTRACE_PLATFORM" >&2
    exit 2
}

# Map a host path under $WORK to its /work equivalent; refuse anything else.
map() {
    local p=$1 abs
    abs=$(cd "$(dirname "$p")" 2>/dev/null && pwd)/$(basename "$p") || {
        echo "error: cannot resolve $p" >&2
        exit 2
    }
    case "$abs" in
    "$WORK"/*) printf '/work/%s\n' "${abs#"$WORK"/}" ;;
    *)
        echo "error: $p is outside CTRACE_WORK ($WORK); the container cannot see it" >&2
        exit 2
        ;;
    esac
}

[[ $# -ge 6 ]] || {
    echo "usage: $0 <w> <h> <qp> <preset> <in.yuv> <out.obu> [bd]" >&2
    exit 2
}
W=$1 H=$2 QP=$3 PRESET=$4
IN=$(map "$5")
OUT=$(map "$6")
BD="${7:-8}"

TRACE_ARG=/dev/null
if [[ -n "${SVT_TRACE_OUT:-}" && "${SVT_TRACE_OUT}" != /dev/null ]]; then
    : >"$SVT_TRACE_OUT"
    TRACE_ARG=$(map "$SVT_TRACE_OUT")
fi

docker build --platform "$PLATFORM" -t "$IMAGE" "$HERE" >"$WORK/docker-build.log" 2>&1 || {
    cat "$WORK/docker-build.log" >&2
    exit 1
}

exec docker run --rm --platform "$PLATFORM" \
    -v "$REPO":/repo:ro \
    -v "$WORK":/work \
    -v zenav1-svt-ctrace-cbuild:/cbuild \
    -e SVT_TRACE_OUT="$TRACE_ARG" \
    -e SVT_BUILD_JOBS="${SVT_BUILD_JOBS:-6}" \
    "$IMAGE" bash /repo/rust/tools/ctrace-linux/incontainer.sh \
    "$W" "$H" "$QP" "$PRESET" "$IN" "$OUT" "$BD"

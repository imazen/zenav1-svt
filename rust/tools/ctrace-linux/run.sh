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
#
# You need a docker daemon of the HOST's architecture, because C's kernels are
# runtime-dispatched and an x86 container would be a DIFFERENT oracle, not the
# same one. On Apple silicon with colima that is a native (vz) arm64 profile —
# an x86_64 qemu profile will not do:
#
#   colima start --profile arm --arch aarch64 --vm-type vz \
#       --cpu 6 --memory 8 --disk 24 --mount-type virtiofs
#   docker context use colima-arm
#
# The C lib + wrap driver are cached in the `zenav1-svt-ctrace-cbuild` docker
# volume, so only the first run pays the ~1 min build.
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

# The `wrap_recon.c` interposers are driven by their own env vars, split into
# two kinds: those that name an OUTPUT PATH (which must be remapped into the
# container's /work view) and those that are plain SELECTORS (passed through).
#
# Keep `PATH_ENV` in sync with `getenv("SVT_..._OUT")` / `SVT_RECON_BIN` in
# `tools/capture_c_trace/wrap_recon.c` — an unmapped path var is not an error,
# it just silently writes inside the container and the host sees nothing, which
# reads as "the interposer produced no data". (That is how the deblock-level
# investigation first came up empty: SVT_RECON_OUT/SVT_RECON_BIN were absent
# from this list.)
#
# The file dumps all APPEND, so truncate first — a stale dump read as fresh is
# the `SVTAV1_PACKTREE` trap in rust/CLAUDE.md, from the other side.
DUMP_ENV=()
for v in SVT_CTREE_OUT SVT_PICKPART_OUT SVT_QLEVELS_OUT SVT_CCOEF_OUT \
    SVT_CCOST_OUT SVT_PART_OUT SVT_SEED_OUT SVT_RECON_OUT SVT_RECON_BIN \
    SVT_CEDGE_OUT SVT_FASTCOST_OUT SVT_FULLCOST_OUT SVT_UVLOOP_OUT \
    SVT_UVRATE_OUT SVT_PD0COST_OUT SVT_LFRECON_OUT SVT_LFRECON_BIN; do
    if [[ -n "${!v:-}" ]]; then
        # The *_BIN vars are PREFIXES (the interposer appends `.p<plane>`), so
        # they have no file of their own to truncate.
        [[ "$v" == *_BIN ]] || : >"${!v}"
        DUMP_ENV+=(-e "$v=$(map "${!v}")")
    fi
done
for v in SVT_PICKPART_MIROW SVT_PICKPART_MICOL SVT_CCOEF_XY SVT_QLEVELS_XY \
    SVT_QLEVELS_COMP SVT_PART_MI SVT_CEDGE_XY SVT_FASTCOST_XY \
    SVT_FULLCOST_XY SVT_UVLOOP_XY SVT_UVRATE_XY SVT_PD0COST_SBY; do
    [[ -n "${!v:-}" ]] && DUMP_ENV+=(-e "$v=${!v}")
done

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
    "${DUMP_ENV[@]+"${DUMP_ENV[@]}"}" \
    "$IMAGE" bash /repo/rust/tools/ctrace-linux/incontainer.sh \
    "$W" "$H" "$QP" "$PRESET" "$IN" "$OUT" "$BD"

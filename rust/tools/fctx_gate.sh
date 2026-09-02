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
# THE C ORACLE NEEDS `-Wl,--wrap`, so WHERE it runs depends on the linker.
# On a GNU-ld host (every Linux, including CI) the host driver
# `tools/capture_c_trace/capture_c_trace` IS a wrap build and is used directly;
# on Apple's ld64 it is not, and the oracle runs in the Linux container
# (docs/WORKING-ON-THIS.md §5). Neither arm may SKIP: a missing docker daemon
# on the container arm is a HARNESS FAILURE, never a pass.
#
# MEASURED 2026-09-02, and it is why the probe exists: this gate used to call
# `ctrace-linux/run.sh` unconditionally. On Linux with a docker CLI but no
# daemon, `set -e` aborted at that call with every diagnostic already
# redirected into `$WORK/c.log` — **rc=1 and ZERO output**, on a host where the
# host driver would have worked. That is the silent-harness trap from the
# inside, and it is the reason this gate was never wired into CI.
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

# Does THIS host's linker support `-Wl,--wrap`? Probed exactly the way
# `capture_c_trace/build.sh` probes it, so the two can never disagree about
# which driver got built.
probe=$(mktemp -d)
printf 'void __wrap_probe_fn(void){} int probe_fn(void); int main(void){return 0;}\n' \
    >"$probe/p.c"
if cc -o "$probe/p" "$probe/p.c" -Wl,--wrap=probe_fn >/dev/null 2>&1; then
    HOST_WRAP=1
else
    HOST_WRAP=0
fi
rm -rf "$probe"

if [[ $HOST_WRAP -eq 0 ]] && ! command -v docker >/dev/null 2>&1; then
    echo "fctx gate: FAIL — this linker has no -Wl,--wrap (Apple ld64) and there" >&2
    echo "           is no docker to run tools/ctrace-linux in." >&2
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

# 2. C side, on the SAME .yuv and the same GOP shape. `run.sh` is a drop-in
#    for the host driver's argv, so the only difference is which one runs.
#    The failure is REPORTED here rather than left to `set -e`, because
#    everything the driver says is already redirected into `$WORK/c.log` and an
#    errexit abort at this line prints nothing at all.
if [[ $HOST_WRAP -eq 1 ]]; then
    C_DRIVER="$HERE/capture_c_trace/capture_c_trace"
else
    C_DRIVER="$HERE/ctrace-linux/run.sh"
fi
if ! SVT_FRAMES="$FRAMES" \
    SVT_INTRA_PERIOD="${SVT_INTRA_PERIOD:--1}" \
    SVT_HIER_LEVELS="${SVT_HIER_LEVELS:-0}" \
    SVT_PRED_STRUCT="${SVT_PRED_STRUCT:-1}" \
    SVT_FCTX_OUT="$WORK/c.fctx" \
    "$C_DRIVER" "$W" "$H" "$QP" "$PRESET" \
    "$WORK/rs.yuv" "$WORK/c.obu" 8 >"$WORK/c.log" 2>&1; then
    echo "fctx gate: FAIL — the C oracle ($C_DRIVER) did not run:" >&2
    tail -20 "$WORK/c.log" >&2
    exit 2
fi

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

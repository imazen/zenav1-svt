#!/usr/bin/env bash
# In-container half of `run.sh` — never invoke this from the host.
#
# Builds the C reference static lib and the `-Wl,--wrap` trace driver into
# /cbuild (a docker volume, so it persists between runs and NEVER writes into
# the repo, which is mounted read-only), then execs the driver.
#
# argv is passed straight through to `capture_c_trace.bin`:
#   <width> <height> <cli_qp 0..63> <preset> <in.yuv> <out.obu> [bit_depth]
# Env: SVT_TRACE_OUT — where the driver writes the op trace.
set -euo pipefail

REPO=/repo
C_ROOT="$REPO/reference/svt-av1"
BUILD=/cbuild/cbuild-static
LIBDIR=/cbuild/Bin/Release
DRIVER=/cbuild/capture_c_trace.bin

[[ -d "$C_ROOT/Source" ]] || {
    echo "error: $C_ROOT/Source missing — did you 'git submodule update --init'?" >&2
    exit 2
}

# Configure once; `cmake --build` below is the per-run staleness check (a fast
# no-op when current), matching the host's cbuild-static settings exactly
# (BUILD_SHARED_LIBS=OFF, BUILD_TESTING=OFF, SVT_AV1_LTO=OFF, NATIVE=OFF).
# NATIVE=OFF matters: a -march=native build would pin the ISA at compile time
# and defeat the runtime RTCD dispatch the host oracle uses.
if [[ ! -f "$BUILD/CMakeCache.txt" ]]; then
    cmake -S "$C_ROOT" -B "$BUILD" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_SHARED_LIBS=OFF \
        -DBUILD_APPS=OFF \
        -DBUILD_TESTING=OFF \
        -DSVT_AV1_LTO=OFF \
        -DNATIVE=OFF \
        -DSVT_HDR_MODE=OFF \
        -DCMAKE_OUTPUT_DIRECTORY="$LIBDIR/" >/cbuild/cmake-configure.log 2>&1 ||
        { cat /cbuild/cmake-configure.log >&2; exit 1; }
fi
cmake --build "$BUILD" -j "${SVT_BUILD_JOBS:-6}" >/cbuild/cmake-build.log 2>&1 ||
    { tail -40 /cbuild/cmake-build.log >&2; exit 1; }

# Link the wrap driver. An explicit argv[1] is REQUIRED here: without it
# build.sh would write both the binary and its `.selected.<mode>` sidecar into
# the repo tree, and the host's `capture_c_trace` wrapper would then exec a
# Linux ELF. (The repo is mounted read-only, so that attempt fails loudly
# rather than silently — belt and braces.)
SVT_CREF_LIB_DIR="$LIBDIR" SVT_NO_AUTO_CMAKE=1 \
    bash "$REPO/rust/tools/capture_c_trace/build.sh" "$DRIVER" >/cbuild/driver-build.log 2>&1 ||
    { cat /cbuild/driver-build.log >&2; exit 1; }

# Anti-vacuity: the whole point of this container is the interposers. If the
# link silently produced the byte-only driver we would get an EMPTY trace and
# read it as "C never called this", which is the exact failure mode
# rust/CLAUDE.md warns about. Prove the wrap symbols are present.
# (`grep -q` is deliberately NOT used here: it exits on first match, SIGPIPEs
# `nm`, and under `set -o pipefail` that 141 fails the pipeline — reporting
# "no wrap symbols" on a driver that has all 18 of them. Count instead.)
nm "$DRIVER" >/cbuild/driver-syms.txt 2>/dev/null || true
if [[ "$(grep -c '__wrap_svt_od_ec_encode_cdf_q15' /cbuild/driver-syms.txt)" == 0 ]]; then
    echo "error: $DRIVER has no __wrap_ symbols — the --wrap link did not take." >&2
    echo "       Refusing to run: an empty op trace is worse than no trace." >&2
    exit 3
fi

exec "$DRIVER" "$@"

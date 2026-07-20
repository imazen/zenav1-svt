#!/usr/bin/env bash
# Build the C-side ENCODE-TIMING driver (tools/perf_c_encode) against the
# in-tree static reference library. The timing sibling of capture_c_trace,
# but with NO -Wl,--wrap= interposers (they would dominate the measurement)
# so it links the plain libSvtAv1Enc.a and reflects real encode speed.
#
# NOT part of the cargo workspace build.
#
# Usage: build.sh [output-binary]
# Env:   SVT_CREF_LIB_DIR — dir with libSvtAv1Enc.a (default <repo>/Bin/Release)
#        SVT_BUILD_JOBS   — parallelism for the C lib rebuild (default 8)
#        SVT_NO_AUTO_CMAKE=1 — skip the automatic C-lib rebuild
#
# STALENESS CONTRACT (same shape as capture_c_trace/build.sh — a stale binary
# silently lies about speed the same way a stale trace binary lies about ops):
#   1. Source/*.c edited, lib not rebuilt      -> `cmake --build` below.
#   2. lib rebuilt, driver not relinked        -> ("$OUT" -nt "$LIB") guard.
#   3. THIS script or the .c edited, not relinked -> guards on both.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd) # <repo> (rust/tools/perf_c_encode -> repo root)

LIB_DIR="${SVT_CREF_LIB_DIR:-$ROOT/Bin/Release}"
LIB="$LIB_DIR/libSvtAv1Enc.a"
CMAKE_DIR="$ROOT/cbuild-static"
OUT="${1:-$HERE/perf_c_encode}"

# Keep the static lib current with Source/ (only for the in-tree default lib;
# an explicit SVT_CREF_LIB_DIR is the caller's artifact — do not build into it).
if [[ -z "${SVT_CREF_LIB_DIR:-}" && -z "${SVT_NO_AUTO_CMAKE:-}" && -d "$CMAKE_DIR" ]]; then
    if ! cmake --build "$CMAKE_DIR" -j "${SVT_BUILD_JOBS:-8}" >/dev/null 2>&1; then
        echo "error: 'cmake --build $CMAKE_DIR' FAILED — refusing to run against a" >&2
        echo "       possibly stale $LIB. Fix the C build first, or re-run with" >&2
        echo "       SVT_NO_AUTO_CMAKE=1 if you know the lib is current." >&2
        exit 1
    fi
fi

if [[ ! -f "$LIB" ]]; then
    echo "error: $LIB not found. Build the C reference first:" >&2
    echo "  cmake -S $ROOT -B $CMAKE_DIR -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \\" >&2
    echo "        -DBUILD_APPS=OFF -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF \\" >&2
    echo "        -DCMAKE_OUTPUT_DIRECTORY=$LIB_DIR/ && cmake --build $CMAKE_DIR -j" >&2
    exit 1
fi

# Skip relink when up to date (sources + lib + this script older than binary).
if [[ -x "$OUT" && "$OUT" -nt "$HERE/perf_c_encode.c" && "$OUT" -nt "$HERE/build.sh" &&
    "$OUT" -nt "$LIB" ]]; then
    echo "perf_c_encode: up to date ($OUT)"
    exit 0
fi

# -O2 matches the reference lib's build type (Release). No -march=native — the
# reference lib ships runtime SIMD dispatch, and the perf gate forbids native.
cc -O2 -o "$OUT" \
    "$HERE/perf_c_encode.c" \
    -I"$ROOT/Source/API" \
    -I"$ROOT/Source/Lib/Codec" \
    -I"$ROOT/Source/Lib/Globals" \
    -I"$ROOT/Source/Lib/C_DEFAULT" \
    "$LIB" -lpthread -lm

echo "perf_c_encode: built $OUT (lib=$LIB)"

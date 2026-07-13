#!/usr/bin/env bash
# Build the C-side trace driver against the in-tree static reference library.
# NOT part of the cargo workspace build — identity_diff.sh calls this on demand.
#
# Usage: build.sh [output-binary]
# Env:   SVT_CREF_LIB_DIR — dir containing libSvtAv1Enc.a (default <repo>/Bin/Release)
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd) # <repo> (svtav1-rs/tools/capture_c_trace -> repo root)
OUT="${1:-$HERE/capture_c_trace}"
LIB_DIR="${SVT_CREF_LIB_DIR:-$ROOT/Bin/Release}"
LIB="$LIB_DIR/libSvtAv1Enc.a"

if [[ ! -f "$LIB" ]]; then
    echo "error: $LIB not found. Build the C reference first:" >&2
    echo "  cmake -S $ROOT -B $ROOT/cbuild-static -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF && cmake --build $ROOT/cbuild-static -j" >&2
    exit 1
fi

# Skip rebuild when up to date (sources + lib older than binary).
if [[ -x "$OUT" && "$OUT" -nt "$HERE/capture_c_trace.c" && "$OUT" -nt "$HERE/wrap_odec.c" && "$OUT" -nt "$LIB" ]]; then
    echo "capture_c_trace: up to date ($OUT)"
    exit 0
fi

cc -O2 -g -o "$OUT" \
    "$HERE/capture_c_trace.c" \
    "$HERE/wrap_odec.c" \
    -I"$ROOT/Source/API" \
    -I"$ROOT/Source/Lib/Codec" \
    -I"$ROOT/Source/Lib/Globals" \
    -I"$ROOT/Source/Lib/C_DEFAULT" \
    -Wl,--wrap=svt_od_ec_encode_cdf_q15 \
    -Wl,--wrap=svt_od_ec_encode_bool_q15 \
    -Wl,--wrap=svt_od_ec_encode_bool_eq_q15 \
    -Wl,--wrap=svt_od_ec_enc_init \
    -Wl,--wrap=svt_od_ec_enc_reset \
    -Wl,--wrap=svt_od_ec_enc_done \
    "$LIB" -lpthread -lm

echo "capture_c_trace: built $OUT"

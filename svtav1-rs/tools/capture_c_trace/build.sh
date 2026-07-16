#!/usr/bin/env bash
# Build the C-side trace driver against the in-tree static reference library.
# NOT part of the cargo workspace build.
#
# You normally do NOT call this directly — run the `capture_c_trace` wrapper,
# which calls this first and then execs the binary, so a stale driver can never
# be used. See the STALENESS CONTRACT below.
#
# Usage: build.sh [output-binary]
# Env:   SVT_CREF_LIB_DIR — dir containing libSvtAv1Enc.a (default <repo>/Bin/Release)
#        SVT_BUILD_JOBS   — parallelism for the C lib rebuild (default 8)
#        SVT_NO_AUTO_CMAKE=1 — skip the automatic C-lib rebuild (see below)
#
# STALENESS CONTRACT (do not weaken — this exists because it bit us):
#   Every C-vs-Rust comparison is only meaningful if the C driver reflects the
#   CURRENT Source/ tree. There are three ways a stale binary can silently lie:
#     1. Source/*.c edited but libSvtAv1Enc.a not rebuilt  -> handled by the
#        `cmake --build` below (a ~0.5s no-op when already current).
#     2. libSvtAv1Enc.a rebuilt but the driver not relinked -> handled by the
#        mtime guard further down ("$OUT" -nt "$LIB").
#     3. THIS SCRIPT edited (new wrapper source, new -Wl,--wrap= flag) but the
#        driver not relinked -> handled by ("$OUT" -nt "$HERE/build.sh").
#        Added 2026-07-16: wrap_recon.c and its --wrap flag were added here,
#        the guard watched only the .c files (all older than the binary), so no
#        relink fired and `nm` showed no __wrap_ symbol — the interposer would
#        have silently dumped nothing. Guard the recipe, not just its
#        ingredients.
#   All must hold. On 2026-07-15 (2) silently produced an EMPTY instrumentation
#   dump for a whole debug cycle: the C lib had been rebuilt with new fprintf
#   dumps, but capture_c_trace still linked the previous archive, so the dump
#   printed nothing and the (wrong) conclusion drawn was "C never calls this
#   function". Nothing failed loudly; the binary just quietly predated the lib.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd) # <repo> (svtav1-rs/tools/capture_c_trace -> repo root)
OUT="${1:-$HERE/capture_c_trace.bin}"
LIB_DIR="${SVT_CREF_LIB_DIR:-$ROOT/Bin/Release}"
LIB="$LIB_DIR/libSvtAv1Enc.a"

# Hole #1: make the static lib itself current with Source/. Only for the
# in-tree default — an explicit SVT_CREF_LIB_DIR is the caller's own artifact
# and we must not build into it.
if [[ -z "${SVT_CREF_LIB_DIR:-}" && -z "${SVT_NO_AUTO_CMAKE:-}" && -d "$ROOT/cbuild-static" ]]; then
    if ! cmake --build "$ROOT/cbuild-static" -j "${SVT_BUILD_JOBS:-8}" >/dev/null 2>&1; then
        echo "error: 'cmake --build $ROOT/cbuild-static' FAILED — refusing to run against a" >&2
        echo "       possibly stale $LIB. Fix the C build first, or re-run with" >&2
        echo "       SVT_NO_AUTO_CMAKE=1 if you know the lib is current." >&2
        exit 1
    fi
fi

if [[ ! -f "$LIB" ]]; then
    echo "error: $LIB not found. Build the C reference first:" >&2
    echo "  cmake -S $ROOT -B $ROOT/cbuild-static -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF && cmake --build $ROOT/cbuild-static -j" >&2
    exit 1
fi

# Skip rebuild when up to date (sources + lib older than binary).
if [[ -x "$OUT" && "$OUT" -nt "$HERE/capture_c_trace.c" && "$OUT" -nt "$HERE/wrap_odec.c" &&
    "$OUT" -nt "$HERE/wrap_recon.c" && "$OUT" -nt "$HERE/build.sh" && "$OUT" -nt "$LIB" ]]; then
    echo "capture_c_trace: up to date ($OUT)"
    exit 0
fi

cc -O2 -g -o "$OUT" \
    "$HERE/capture_c_trace.c" \
    "$HERE/wrap_odec.c" \
    "$HERE/wrap_recon.c" \
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
    -Wl,--wrap=svt_av1_loop_filter_init \
    -Wl,--wrap=svt_aom_txb_estimate_coeff_bits \
    -Wl,--wrap=svt_aom_partition_rate_cost \
    -Wl,--wrap=svt_aom_pick_partition \
    "$LIB" -lpthread -lm

echo "capture_c_trace: built $OUT"

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
ROOT=$(cd "$HERE/../../.." && pwd) # <repo> (rust/tools/capture_c_trace -> repo root)
C_ROOT="$ROOT/reference/svt-av1" # the C reference submodule (imazen/svt-av1-ref)

# ---------------------------------------------------------------------------
# WHICH C ORACLE — resolved through the registry (rust/oracles/oracles.tsv,
# rust/docs/ORACLES.md): $SVT_ORACLE names it; unset, the legacy switch
# SVT_HDR_MODE=1 means hybrid-3115-hdr and anything else hybrid-3115, so every
# pre-existing caller keeps byte-for-byte the same oracle.
#
# Every oracle MUST have its own lib dir AND its own driver binary. Both halves
# are load-bearing:
#   * distinct lib dirs — the cmake output dir is CMAKE_OUTPUT_DIRECTORY, which
#     defaults to Bin/${CMAKE_BUILD_TYPE} for EVERY config; an HDR build left at
#     the default once silently OVERWROTE the mainline libSvtAv1Enc.a, and every
#     "mainline" gate then compared against the fork oracle. tools/oracle gives
#     each oracle its own lib dir.
#   * distinct binaries — the staleness guard below is a set of mtime
#     comparisons against ONE $LIB. Sharing a binary across oracles defeats it
#     in the most dangerous direction: after linking oracle B, switching back to
#     A finds the binary NEWER than A's (older) lib, so no relink fires and A
#     silently runs B's code. Per-oracle $OUT keeps each guard chain independent.
#
# All oracles agree on SVT_AV1_LTO=OFF and NATIVE=OFF: a bit-identity oracle may
# not differ from its counterpart by optimisation level.
# ---------------------------------------------------------------------------
ORACLE_TOOL="$HERE/../oracle"
ORACLE=$("$ORACLE_TOOL" resolve)
ORACLE_MODE=$("$ORACLE_TOOL" field "$ORACLE" mode)
ORACLE_API=$("$ORACLE_TOOL" field "$ORACLE" api)
C_ROOT=$("$ORACLE_TOOL" srcdir "$ORACLE")
DEFAULT_LIB_DIR=$("$ORACLE_TOOL" libdir "$ORACLE")
case "$ORACLE" in
    hybrid-3115) DEFAULT_OUT="$HERE/capture_c_trace.bin" CMAKE_DIR="$ROOT/cbuild-static" ;;
    hybrid-3115-hdr) DEFAULT_OUT="$HERE/capture_c_trace.hdr.bin" CMAKE_DIR="$ROOT/cbuild-static-hdr" ;;
    *) DEFAULT_OUT="$HERE/capture_c_trace.$ORACLE.bin" CMAKE_DIR="" ;;
esac
# C API differences the driver must bridge, declared per oracle in the registry
# (`driver_defs`). ZEN_ORACLE_MAINLINE_API: no svt-av1-hdr config fields, so
# those FORK_SET lines compile out and a request for one REFUSES.
read -r -a API_DEFS <<<"$("$ORACLE_TOOL" field "$ORACLE" driver_defs)"
if [[ "$ORACLE_API" == "mainline" && " ${API_DEFS[*]} " != *" -DZEN_ORACLE_MAINLINE_API=1 "* ]]; then
    echo "error: oracle $ORACLE has api=mainline but no -DZEN_ORACLE_MAINLINE_API=1 in driver_defs" >&2
    exit 1
fi
# Kept for the log lines below and for callers that still print it.
HDR_MODE="${SVT_HDR_MODE:-0}"

OUT="${1:-$DEFAULT_OUT}"
LIB_DIR="${SVT_CREF_LIB_DIR:-$DEFAULT_LIB_DIR}"
LIB="$LIB_DIR/libSvtAv1Enc.a"

# Hole #1: make the static lib itself current with Source/. Only for the
# in-tree default — an explicit SVT_CREF_LIB_DIR is the caller's own artifact
# and we must not build into it.
if [[ -z "${SVT_CREF_LIB_DIR:-}" && -z "${SVT_NO_AUTO_CMAKE:-}" && "$ORACLE_MODE" == "pinned" ]]; then
    # Pinned oracles are materialised and built by the registry tool; it is a
    # no-op when the stamped build for this commit already exists.
    "$ORACLE_TOOL" build "$ORACLE" >/dev/null
fi
if [[ -z "${SVT_CREF_LIB_DIR:-}" && -z "${SVT_NO_AUTO_CMAKE:-}" && -n "$CMAKE_DIR" && -d "$CMAKE_DIR" ]]; then
    if ! cmake --build "$CMAKE_DIR" -j "${SVT_BUILD_JOBS:-8}" >/dev/null 2>&1; then
        echo "error: 'cmake --build $CMAKE_DIR' FAILED — refusing to run against a" >&2
        echo "       possibly stale $LIB. Fix the C build first, or re-run with" >&2
        echo "       SVT_NO_AUTO_CMAKE=1 if you know the lib is current." >&2
        exit 1
    fi
fi

if [[ ! -f "$LIB" ]]; then
    echo "error: $LIB not found (oracle $ORACLE). Build it first: rust/tools/oracle build $ORACLE —" >&2
    echo "  cargo does it (both variants, SHA-stamped; rust/crates/svtav1-cref/build.rs):" >&2
    echo "    (cd $ROOT/rust && cargo build -p zenav1-svt-cref)" >&2
    echo "  or by hand, with the same flags:" >&2
    echo "    cmake -S $C_ROOT -B $CMAKE_DIR -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \\" >&2
    echo "          -DBUILD_APPS=OFF -DBUILD_TESTING=OFF -DSVT_AV1_LTO=OFF -DNATIVE=OFF $CMAKE_HDR_FLAG \\" >&2
    echo "          -DCMAKE_OUTPUT_DIRECTORY=$DEFAULT_LIB_DIR/ && cmake --build $CMAKE_DIR -j" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Linker capability probe: `-Wl,--wrap=` is a GNU-ld/lld extension. Apple's ld64
# rejects it outright ("ld: unknown options: --wrap=..."), which used to make
# this script — and therefore EVERY byte-parity gate in tools/ — unbuildable on
# macOS. The gates do not actually need the interposers: all but identity_diff
# and tile_map pass `SVT_TRACE_OUT=/dev/null` and compare OBU bytes only.
#
# So probe the capability (never the OS name — a Linux host with ld64-like
# tooling, or a macOS host with GNU ld installed, should both do the right
# thing) and fall back to a byte-only driver. The fallback gets its OWN $OUT so
# the staleness contract above stays per-mode: a wrap build and a no-wrap build
# can coexist and neither can silently masquerade as the other.
# ---------------------------------------------------------------------------
WRAP_PROBE=$(mktemp -d)
trap 'rm -rf "$WRAP_PROBE"' EXIT
printf 'void __wrap_probe_fn(void){} int probe_fn(void); int main(void){return 0;}\n' \
    >"$WRAP_PROBE/p.c"
if cc -o "$WRAP_PROBE/p" "$WRAP_PROBE/p.c" -Wl,--wrap=probe_fn >/dev/null 2>&1; then
    WRAP_SUPPORTED=1
else
    WRAP_SUPPORTED=0
    OUT="${1:-${DEFAULT_OUT%.bin}.nowrap.bin}"
fi

# Publish the resolved binary path so the `capture_c_trace` wrapper execs the
# one this script actually produced, instead of re-deriving it (and drifting).
# Per-mode, like $OUT itself. Only for the default invocation — an explicit
# argv[1] is the caller's own artifact and must not move the wrapper's target.
if [[ -z "${1:-}" ]]; then
    printf '%s\n' "$OUT" >"$HERE/.selected.$ORACLE"
fi

# Skip rebuild when up to date (sources + lib older than binary).
if [[ -x "$OUT" && "$OUT" -nt "$HERE/capture_c_trace.c" && "$OUT" -nt "$HERE/wrap_odec.c" &&
    "$OUT" -nt "$HERE/wrap_recon.c" && "$OUT" -nt "$HERE/build.sh" && "$OUT" -nt "$LIB" &&
    "$OUT" -nt "$HERE/../../oracles/oracles.tsv" ]]; then
    echo "capture_c_trace: up to date ($OUT)"
    exit 0
fi

if [[ "$WRAP_SUPPORTED" == "0" ]]; then
    # Byte-only driver: the real encoder, real OBU bytes, no op trace. The
    # wrap_*.c translation units are EXCLUDED (they reference `__real_*`
    # symbols that only the --wrap flag defines), and the driver is compiled
    # with -DSVT_NO_WRAP_TRACE so it refuses a real SVT_TRACE_OUT request
    # instead of writing an empty trace a differ would misread.
    echo "capture_c_trace: this linker does not support -Wl,--wrap — building the BYTE-ONLY" >&2
    echo "                 driver. Byte-parity gates (identity_matrix, bd10_*, tile_gate," >&2
    echo "                 sb128_gate, partial_sb_gate, ...) work; op-level localization" >&2
    echo "                 (identity_diff.sh's symbol trace, tile_map.sh) needs a GNU-ld host." >&2
    cc -O2 -g -o "$OUT" \
        -DSVT_NO_WRAP_TRACE=1 "${API_DEFS[@]}" \
        "$HERE/capture_c_trace.c" \
        -I"$C_ROOT/Source/API" \
        -I"$C_ROOT/Source/Lib/Codec" \
        -I"$C_ROOT/Source/Lib/Globals" \
        -I"$C_ROOT/Source/Lib/C_DEFAULT" \
        "$LIB" -lpthread -lm
    echo "capture_c_trace: built $OUT (byte-only, oracle=$ORACLE, lib=$LIB)"
    exit 0
fi

cc -O2 -g -o "$OUT" "${API_DEFS[@]}" \
    "$HERE/capture_c_trace.c" \
    "$HERE/wrap_odec.c" \
    "$HERE/wrap_recon.c" \
    -I"$C_ROOT/Source/API" \
    -I"$C_ROOT/Source/Lib/Codec" \
    -I"$C_ROOT/Source/Lib/Globals" \
    -I"$C_ROOT/Source/Lib/C_DEFAULT" \
    -Wl,--wrap=svt_od_ec_encode_cdf_q15 \
    -Wl,--wrap=svt_od_ec_encode_bool_q15 \
    -Wl,--wrap=svt_od_ec_encode_bool_eq_q15 \
    -Wl,--wrap=svt_od_ec_enc_init \
    -Wl,--wrap=svt_od_ec_enc_reset \
    -Wl,--wrap=svt_od_ec_enc_done \
    -Wl,--wrap=svt_av1_loop_filter_init \
    -Wl,--wrap=svt_av1_loop_filter_frame \
    -Wl,--wrap=svt_aom_txb_estimate_coeff_bits \
    -Wl,--wrap=svt_aom_partition_rate_cost \
    -Wl,--wrap=svt_aom_pick_partition \
    -Wl,--wrap=svt_aom_pick_partition_pd0 \
    -Wl,--wrap=svt_aom_estimate_syntax_rate \
    -Wl,--wrap=svt_aom_intra_fast_cost \
    -Wl,--wrap=svt_aom_inter_fast_cost \
    -Wl,--wrap=svt_aom_update_mi_map \
    -Wl,--wrap=svt_aom_update_stats \
    -Wl,--wrap=svt_aom_full_loop_uv \
    -Wl,--wrap=svt_av1_intra_prediction \
    -Wl,--wrap=svt_aom_get_intra_uv_fast_rate \
    -Wl,--wrap=svt_aom_full_cost \
    -Wl,--wrap=svt_aom_full_cost_pd0 \
    -Wl,--wrap=svt_aom_quantize_inv_quantize \
    -Wl,--wrap=svt_aom_sig_deriv_enc_dec_pd0 \
    -Wl,--wrap=svt_av1_reset_cdf_symbol_counters \
    -Wl,--wrap=svt_aom_motion_estimation_b64 \
    -Wl,--wrap=svt_av1_find_best_sub_pixel_tree_pruned \
    -Wl,--wrap=svt_av1_compute_qdelta_by_rate \
    -Wl,--wrap=svt_aom_inter_pu_prediction_av1 \
    -Wl,--wrap=svt_aom_global_motion_estimation \
    -Wl,--wrap=svt_aom_gm_get_params_cost \
    -Wl,--wrap=svt_av1_refine_integerized_param \
    -Wl,--wrap=svt_av1_warp_error \
    -Wl,--wrap=svt_aom_wm_motion_refinement \
    -Wl,--wrap=svt_aom_estimate_transform \
    -Wl,--wrap=svt_aom_encode_sb \
    -Wl,--wrap=svt_aom_estimate_coefficients_rate \
    -Wl,--wrap=svt_aom_write_modes_sb \
    -Wl,--wrap=svt_aom_generate_av1_mvp_table \
    -Wl,--wrap=svt_aom_update_part_stats \
    -Wl,--wrap=svt_aom_quantize_inv_quantize_light \
    -Wl,--wrap=svt_aom_inv_transform_recon_wrapper \
    -Wl,--wrap=svt_aom_product_full_mode_decision \
    -Wl,--wrap=svt_av1_get_q_index_from_qstep_ratio \
    -Wl,--wrap=svt_av1_rc_init_sb_qindex \
    -Wl,--wrap=svt_av1_rc_calc_qindex_crf_cqp \
    -Wl,--wrap=svt_aom_set_tuned_blk_lambda \
    "$LIB" -lpthread -lm

echo "capture_c_trace: built $OUT (oracle=$ORACLE, lib=$LIB)"

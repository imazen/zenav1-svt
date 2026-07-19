#!/usr/bin/env bash
# bd10 PHOTOGRAPHIC identity gate (task #94, low-preset/real-content axis).
#
# Real 10-bit photographic content, byte-identical to the real C encoder. Every
# cell below was verified with `cmp` at the commit that added it.
#
# WHY THIS GATE EXISTS: every other bd10 gate feeds SYNTHETIC content
# (uniform/gradient/diag). Synthetic patterns exercise a narrow slice of the
# mode/tx/partition space, so a port can be synthetic-green and still diverge on
# the content users actually encode — which is exactly what happened here:
# docs/bd10-port-map.md recorded an 18/18 photographic FAILURE ("on real content
# the partition tree ALWAYS diverges at bd10"). That measurement predated the
# PD0_LVL_0 partition fix, the TXS-coupling gate, the bd10 chroma re-encode and
# the AVX2-hadamard fix; with all four landed, photographic bd10 at eff-M9 is
# byte-identical. This gate pins that so it cannot silently regress.
#
# SCOPE — the eff-M9 band (presets 9..13) PLUS presets 7-8 (group D below).
#
# Presets 7-8 were closed 2026-07-19 by the TXT rate-cost gate lambda fix
# (leaf_funnel.rs `txt_search`): C prices that gate with the SAME `full_lambda`
# it prices the tx-type cost with — `hbd_md ? full_lambda_md[EB_10_BIT_MD] :
# [EB_8_BIT_MD]` (product_coding_loop.c:4590/4714/4944) — while the port had the
# cost on the bd10 lambda and the gate on the u8 one, so at bd10 the gate
# under-fired and the port picked tx types C prunes before quantizing them.
# Photographic p6-p8 went 2/27 -> 18/27 (all of p7+p8).
#
# Preset 6 CLOSED 2026-07-19 (group E below): its residual was the CDEF and
# Wiener-LR SEARCHES still running at 8 bits — p6 is the only preset in this
# gate that runs either. Both now run at true 10 bits. Presets 0-5 still
# diverge and are NOT listed: their first divergence is the DEBLOCK-LEVEL full
# search (`pick_filter_levels_full_search`, used at preset <= 5), a third
# post-MD search that is still 8-bit at bd10 — measured on 2119713 q32 p0 as
# `loop_filter_level[2] C=6 Rust=4`. See docs/bd10-port-map.md.
#
# CORPUS: CID22-512 (250 real 512x512 photographic PNGs, natively 64-aligned).
# Override with BD10_PHOTO_CORPUS=<dir>. If the corpus is absent this gate FAILS
# LOUDLY — it never skips silently, because a gate that quietly passes without
# encoding anything is worse than one that fails.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

# Reference decoder for the DECODABILITY assert in run_cell. Required, never
# skipped: byte-identity alone is blind to a cell that regresses OUT of the
# lists below, so every port OBU this gate produces must also be provably
# decodable. Same contract as bd10_nonflat_gate.sh. Override with AOMDEC=.
aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
    for _c in aomdec /root/aomdec-debug/aomdec; do
        if command -v "$_c" >/dev/null 2>&1; then aomdec=$_c; break; fi
    done
fi
command -v "$aomdec" >/dev/null 2>&1 || {
    echo "bd10 photo gate: aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}

CORPUS="${BD10_PHOTO_CORPUS:-/root/work/codec-corpus/CID22/CID22-512/training}"
if [ ! -d "$CORPUS" ]; then
    echo "bd10 photo gate: corpus not found at $CORPUS" >&2
    echo "  set BD10_PHOTO_CORPUS=<dir of 512x512 PNGs> to point at it." >&2
    exit 1
fi

# Images verified byte-identical at every qp x preset in the grids below.
# Named explicitly (NOT selected by index into `ls`) so the gate is stable
# against corpus additions/removals.
IMAGES_A=(1001682 1484678 2119713 2738653 4666751)
QPS_A=(12 32 55)
IMAGES_B=(1080721 1454613116 167491 2208891 2666598 3571065 708587 pexels-photo-3214683)
QPS_B=(5 20 40 63)
PRESETS=(10 13)
# The rest of the eff-M9 band. Presets 9/11/12 share the closed envelope but
# were never gated (the synthetic gate was pinned on 10 and 13 only); measured
# 18/18 byte-identical on the group-C images below.
IMAGES_C=(1001682 2119713 4666751)
QPS_C=(12 40)
PRESETS_C=(9 11 12)
# Group D — presets 7-8, closed by the TXT rate-cost gate lambda fix (header).
# These are the first photographic cells below the eff-M9 band to be
# byte-identical: at p7/p8 `bd10_full_rd_supported` is TRUE, so unlike groups
# A-C they exercise the whole bd10 full-RD leaf (10-bit TXT search, tx-depth
# loop, chroma full loop and the bd10 CfL arm), not just the level re-encode.
# 18/18 verified with `cmp` at the commit that added them.
IMAGES_D=(1001682 2119713 4666751)
QPS_D=(12 32 55)
PRESETS_D=(7 8)
# Group E — preset 6, closed 2026-07-19 by running the CDEF strength search
# AND the Wiener loop-restoration search at TRUE 10 bits.
#
# p6 is the ONLY preset in this gate that runs those two searches at all:
# `cdef::allintra_preset_uses_cdef_search` is `preset <= 6` (presets 7-13 take
# the closed-form qp picker, already bd10-aware) and
# `restoration::wn_filter_ctrls_allintra` disables LR above preset 6. Both
# searches read the reconstructed frame and write frame-header syntax, so
# running them on the u8 recon at bd10 was a bitstream divergence, not a recon
# approximation — measured as exactly two things on 2119713 q32 p6:
# `cdef_y_sec_strength[1] C=2 Rust=0` plus the Wiener taps
# (docs/bd10-port-map.md, "p6's residual is the 8-bit CDEF/LR SEARCH").
# They now run on the 10-bit post-deblock / post-CDEF canvas with
# coeff_shift = 2 and the bd10 lambdas. 9/9 verified with `cmp`.
IMAGES_E=(1001682 2119713 4666751)
QPS_E=(12 32 55)
PRESETS_E=(6)
# Group F — preset 5, closed 2026-07-19 by running the M2..M5 CHROMA mode
# decision at TRUE 10 bits. `search_best_mds3_uv_mode` (the M2..M5 uv-mode
# search) and `check_best_indepedant_cfl` (the ind-uv CfL arbitration) ran
# ENTIRELY at 8 bits at bd10: the port priced the uv full loop with the u8
# `chroma_eval` + u8 lambda and gated the CfL arm on `bd10_rd.is_none()`. C
# runs BOTH at hbd_md — `full_lambda_md[EB_10_BIT_MD]` with 10-bit prediction /
# residual / full-loop distortion (product_coding_loop.c:7307/7397/7443,
# :3899). Deciding the uv mode / CfL on u8 chroma flipped near-ties (uv V-vs-DC
# at block (0,0), CfL-vs-non-CfL at (16,80) on 1001682 q12 p5) which cascaded
# into partition/mode divergence across the frame. Fixed by running the M2..M5
# uv search + the ind-uv CfL arbitration on `chroma_eval10` + `b.lambda` at
# bd10 (leaf_funnel.rs). ALL 5 group-A images x q{12,32,55} at p5 are now
# byte-identical (15/15, verified `cmp`). p4 is 11/15 and p2/p3 diverge on a
# SEPARATE, still-open LUMA partition RD near-tie (docs/bd10-port-map.md).
IMAGES_F=(1001682 1484678 2119713 2738653 4666751)
QPS_F=(12 32 55)
PRESETS_F=(5)

OUT="${TMPDIR:-/tmp}/bd10photo.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
pass=0
fail=0
missing=0
failed=()

run_cell() {
    local stem=$1 qp=$2 p=$3
    local png="$CORPUS/$stem.png"
    local tag="${stem}_q${qp}_p${p}"
    if [ ! -f "$png" ]; then
        missing=$((missing + 1))
        failed+=("${tag}[no-png]")
        return
    fi
    if ! SVTAV1_BD=10 "$HERE/identity_run" "file:$png" 512 512 "$qp" "$p" "$OUT/rs" \
        >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[rs-err]")
        return
    fi
    if ! SVT_TRACE_OUT=/dev/null "$HERE/capture_c_trace/capture_c_trace" \
        512 512 "$qp" "$p" "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[c-err]")
        return
    fi
    # Decodability BEFORE byte-identity: a stream aomdec rejects is a failure
    # regardless of what it compares equal to.
    if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
        fail=$((fail + 1))
        failed+=("${tag}[undecodable]")
        return
    fi
    if cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
        failed+=("$tag")
    fi
}

for stem in "${IMAGES_A[@]}"; do
    for qp in "${QPS_A[@]}"; do
        for p in "${PRESETS[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_B[@]}"; do
    for qp in "${QPS_B[@]}"; do
        for p in "${PRESETS[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_C[@]}"; do
    for qp in "${QPS_C[@]}"; do
        for p in "${PRESETS_C[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_D[@]}"; do
    for qp in "${QPS_D[@]}"; do
        for p in "${PRESETS_D[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_E[@]}"; do
    for qp in "${QPS_E[@]}"; do
        for p in "${PRESETS_E[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done
for stem in "${IMAGES_F[@]}"; do
    for qp in "${QPS_F[@]}"; do
        for p in "${PRESETS_F[@]}"; do run_cell "$stem" "$qp" "$p"; done
    done
done

total=$((pass + fail + missing))
echo "bd10 photographic identity: $pass / $total byte-identical"
[ "$((fail + missing))" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$((fail + missing))" -eq 0 ]

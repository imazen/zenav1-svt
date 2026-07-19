#!/usr/bin/env bash
# bd10 RECON-PARITY gate — the port's 10-bit reconstruction must be
# byte-identical to the real C encoder's, not merely produce identical bytes.
#
# WHY THIS GATE EXISTS
# --------------------
# Byte-identity of the OBU is necessary but NOT sufficient once the encoder
# grew post-MD searches that READ the reconstruction: the deblock-level search
# (preset <= 5), the CDEF strength search and the Wiener-LR search all pick
# frame-header syntax by measuring the recon against the source. A port can
# emit C's exact bytes while holding a WRONG 10-bit recon — the searches just
# happen not to be reached, or happen to hill-climb to the same answer — and
# then silently diverge the moment a cell reaches one of them.
#
# That is not hypothetical. It is what shipped: the bd10 level-only re-encode
# post-pass (`bd10_reencode_luma`) ran ON TOP of the FULL-RD funnel that had
# already produced the coded 10-bit levels, re-quantizing them with the RDOQ
# entropy contexts hardcoded to 0/0. `bd10_full_rd_supported`'s doc comment
# stated the invariant ("the winner's 10-bit levels ARE the coded ones, so the
# level-only re-encode post-pass is skipped") but the gate never implemented
# it. Every byte gate stayed green while the 10-bit canvas the searches read
# disagreed with the bitstream by 8-12 KB per frame.
#
# A previous investigation looked straight at that canvas, compared it against
# the EIGHT-BIT recon, found `recon10 ~= 4*u8recon + 24`, and recorded a
# systematic-offset defect. The x4 is just the bit-depth scale: those exact
# `recon10` values are C's, byte for byte. Comparing a 10-bit plane against an
# 8-bit one cannot answer the question. Comparing it against C's plane can, and
# that is all this gate does.
#
# HOW
# ---
# C's `svt_av1_loop_filter_init` interposer (tools/capture_c_trace/wrap_recon.c)
# dumps the recon picture C is about to run the filter chain on — post-MD,
# PRE-deblock — as packed u16 LE. The port's `SVTAV1_BD10_RECON` dumps
# `last_recon10_y` at the same point (the post-filter chain works on a clone).
# The two files must be identical.
#
# SCOPE: preset <= 5 ONLY, and that restriction is load-bearing. At preset >= 6
# C's single `loop_filter_init` call comes from the sb-based DLF path in
# enc_dec_process.c, BEFORE the frame is fully reconstructed, so call 0 holds
# only SB(0,0) and the dump is a mid-frame snapshot. At preset <= 5 the call
# comes from dlf_process.c after the full-image recon is final. A partial
# snapshot can only ever cause a FALSE FAILURE here, never a false pass, but
# the restriction keeps the gate honest rather than noisy.
#
# Cells are 64-aligned (the bd10 envelope) and every one is asserted on BOTH
# axes: the OBU must byte-match AND the recon must byte-match. So this gate is
# a strict superset of a byte gate over the same cells.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

aomdec="${AOMDEC:-}"
if [ -z "$aomdec" ]; then
    for _c in aomdec /root/aomdec-debug/aomdec; do
        if command -v "$_c" >/dev/null 2>&1; then aomdec=$_c; break; fi
    done
fi
command -v "$aomdec" >/dev/null 2>&1 || {
    echo "bd10 recon-parity gate: aomdec not found (set AOMDEC=/path/to/aomdec)" >&2
    exit 2
}

# content w h qp preset. Chosen from the p0..p5 x q{5..63} x {gradient,diag,
# uniform} x {64,128} sweep: every cell here was verified byte-identical on
# BOTH axes with `cmp` at the commit that added it. The 128x128 gradient cells
# at p3/p5 are the regression witnesses for the post-pass defect above — they
# were OBU DIFF + recon DIFF (8194-11766 B) before it was fixed.
CELLS=(
  "gradient 128 128 12 3"
  "gradient 128 128 12 5"
  "gradient 128 128 32 5"
  "gradient 128 128 55 5"
  "gradient 128 128 40 4"
  "gradient 128 128 20 2"
  "gradient 64 64 12 5"
  "gradient 64 64 55 3"
  "gradient 64 64 12 2"
  "gradient 64 64 12 4"
  "uniform 128 128 32 5"
  "uniform 128 128 55 3"
  "uniform 64 64 12 0"
)

OUT="${TMPDIR:-/tmp}/bd10recon.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
pass=0
fail=0
failed=()

for cell in "${CELLS[@]}"; do
    # shellcheck disable=SC2086
    set -- $cell
    content=$1 w=$2 h=$3 qp=$4 p=$5
    tag="${content}_${w}x${h}_q${qp}_p${p}"

    if ! SVTAV1_BD=10 SVTAV1_BD10_RECON="$OUT/rs.r10" "$HERE/identity_run" \
        "$content" "$w" "$h" "$qp" "$p" "$OUT/rs" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("${tag}[rs-err]"); continue
    fi
    rm -f "$OUT/c.r10.p0"
    if ! SVT_TRACE_OUT=/dev/null SVT_RECON_OUT=/dev/null SVT_RECON_BIN="$OUT/c.r10" \
        "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$p" \
        "$OUT/rs.yuv" "$OUT/c.obu" 10 >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("${tag}[c-err]"); continue
    fi
    # Decodability first: a stream aomdec rejects fails regardless of what it
    # compares equal to. Same contract as the other bd10 gates.
    if ! "$aomdec" --rawvideo -o /dev/null "$OUT/rs.obu" >/dev/null 2>&1; then
        fail=$((fail + 1)); failed+=("${tag}[undecodable]"); continue
    fi
    if [ ! -s "$OUT/rs.r10" ]; then
        # The port produced no 10-bit canvas at all. Silently "passing" that
        # would make the gate vacuous, so it is an explicit failure.
        fail=$((fail + 1)); failed+=("${tag}[no-port-recon]"); continue
    fi
    if [ ! -s "$OUT/c.r10.p0" ]; then
        fail=$((fail + 1)); failed+=("${tag}[no-c-recon]"); continue
    fi
    if ! cmp -s "$OUT/rs.obu" "$OUT/c.obu"; then
        fail=$((fail + 1)); failed+=("${tag}[obu]"); continue
    fi
    if ! cmp -s "$OUT/rs.r10" "$OUT/c.r10.p0"; then
        nbytes=$(cmp -l "$OUT/rs.r10" "$OUT/c.r10.p0" 2>/dev/null | wc -l)
        fail=$((fail + 1)); failed+=("${tag}[recon:${nbytes}B]"); continue
    fi
    pass=$((pass + 1))
    set --
done

total=$((pass + fail))
echo "bd10 recon parity (OBU + 10-bit recon vs real C): $pass / $total"
[ "$fail" -gt 0 ] && printf 'FAILED: %s\n' "${failed[@]}"
[ "$fail" -eq 0 ]

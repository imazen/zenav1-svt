#!/usr/bin/env bash
# INTER-frame BYTE gate — the cells where BOTH frames of a 2-frame low-delay P
# encode are byte-identical to C.
#
# WHY IT IS SEPARATE FROM `inter_fh_gate.sh`. That gate asserts the frame
# HEADER's field layout, which is what could be asserted while the tile was
# unported. This one asserts the whole stream, which became possible on
# 2026-09-01 (docs/INTER-ENCODE-PLAN.md §1z, the first byte-identical inter
# frame). Keeping both means a header regression still names itself as a header
# regression instead of hiding inside a byte mismatch.
#
# SHAPE, and it is the same one `inter_decode_gate.sh` uses for the same
# reason: PASS_CELLS must be byte-identical on BOTH frames; OPEN_CELLS are
# known not to be and are listed with the measured reason, so the gate states a
# frontier rather than hiding one. A cell moves OPEN -> PASS to record
# progress; a PASS cell regressing fails.
#
# ANTI-VACUITY: a cell only belongs in PASS_CELLS if it was measured
# byte-identical. The gate additionally refuses to run with an EMPTY pass list,
# because "0 / 0 identical" is the failure mode `docs/WORKING-ON-THIS.md` §5
# records for the corpus gates.
#
# TEETH, measured rather than asserted: reverting §1z's fix (letting the
# homegrown temporal filter run over an inter frame's MD source again) fails
# **2 of the 6** required cells — `gradient 64x64 q40 p6` and
# `gradient 16x16 q40 p6`. The other four stay green because at q55, and on
# `screen` 16x16, the filtered source still quantizes to the same decision.
# That is the honest number: this gate WITNESSES the temporal-filter defect,
# and four of its cells do not.
#
# Usage: tools/inter_byte_gate.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)

# "<content> <w> <h> <qp> <preset> <frames> <shift>"
PASS_CELLS=(
    "gradient 64 64 40 6 2 3"   # §1z's reference cell: one 64x64 NEWMV skip block
    "gradient 64 64 55 6 2 3"
    "gradient 16 16 40 6 2 3"
    "gradient 16 16 55 6 2 3"
    "screen 16 16 20 6 2 3"
    "screen 16 16 40 6 2 3"
)
# The frontier, measured 2026-09-01 over 96 cells
# ({uniform,gradient,diag,screen} x {16,64,72,128} x {q20,q40,q55} x {p6,p8}).
# Two named shapes, and they are different problems:
#   * `uniform` at 16/64 matches C's LENGTH exactly and differs at byte 15,
#     which is inside the frame HEADER — that is §1x's recorded
#     `cdef_damping_minus_3` finding (C 1, port 0) showing up as a byte.
#   * preset 8 diverges far more widely than preset 6 (873 B against C's 22 on
#     `gradient 64 q20 p8`): the video arm's p8 mode-decision ladder has never
#     been exercised on an inter frame.
OPEN_CELLS=(
    "uniform 64 64 40 6 2 3"    # header: cdef_damping_minus_3, §1x
    "gradient 64 64 20 6 2 3"   # tile 25 B vs C's 22
    "gradient 64 64 40 8 2 3"   # preset-8 ladder
)

if [[ ${#PASS_CELLS[@]} -eq 0 ]]; then
    echo "inter byte gate: PASS_CELLS is empty — a gate with nothing to assert" >&2
    exit 1
fi

work="${TMPDIR:-$HOME/tmp}/inter-byte-gate.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

# echoes "<f0>/<f1>" as 1 (identical) or 0, or "ERR"
run_cell() {
    local content=$1 w=$2 h=$3 qp=$4 preset=$5 frames=$6 shift_px=$7
    local out="$work/${content}_${w}x${h}_q${qp}_p${preset}"
    mkdir -p "$out"
    SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="$shift_px" \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$qp" "$preset" "$frames" "$content" "$out" \
        >"$out/diff.txt" 2>&1
    local st=$?
    if [[ $st -eq 3 ]]; then echo "ERR"; return; fi
    if [[ ! -s "$out/c.obu.pts0" || ! -s "$out/rs.obu.f0" ]]; then echo "ERR"; return; fi
    local a=0 b=0
    cmp -s "$out/c.obu.pts0" "$out/rs.obu.f0" && a=1
    cmp -s "$out/c.obu.pts1" "$out/rs.obu.f1" && b=1
    echo "$a/$b"
}

fail=0; err=0; promoted=0
echo "== inter byte gate =="
for spec in "${PASS_CELLS[@]}"; do
    # shellcheck disable=SC2086
    got=$(run_cell $spec)
    if [[ "$got" == "ERR" ]]; then
        echo "  HARNESS  $spec  (the encoder refused, or produced no stream)"
        err=$((err + 1))
    elif [[ "$got" == "1/1" ]]; then
        echo "  PASS  $spec"
    else
        echo "  FAIL  $spec  (frame0/frame1 identical = $got)"
        fail=$((fail + 1))
    fi
done
echo "-- known-open cells --"
for spec in "${OPEN_CELLS[@]}"; do
    # shellcheck disable=SC2086
    got=$(run_cell $spec)
    if [[ "$got" == "1/1" ]]; then
        echo "  PROMOTED  $spec  (now byte-identical — move it to PASS_CELLS)"
        promoted=$((promoted + 1))
    else
        echo "  open  $spec  (frame0/frame1 identical = $got, known)"
    fi
done

echo
echo "inter byte gate: ${#PASS_CELLS[@]} required, $fail failed, ${#OPEN_CELLS[@]} known-open ($promoted now identical), $err harness errors"
if [[ $err -gt 0 ]]; then
    echo "inter byte gate: HARNESS FAILURE — not a parity result" >&2
    exit 2
fi
if [[ $fail -gt 0 ]]; then exit 1; fi
if [[ $promoted -gt 0 ]]; then
    echo "A known-open cell is now byte-identical. Move it to PASS_CELLS in this script." >&2
    exit 1
fi
echo "inter byte gate: PASS"

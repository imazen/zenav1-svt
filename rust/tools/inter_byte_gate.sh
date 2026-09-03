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
# TEETH, measured rather than asserted: reverting §1z's temporal-filter fix
# (letting the homegrown filter run over an inter frame's MD source again)
# failed 2 of the 6 cells the gate had at the time — `gradient 64x64 q40 p6`
# and `gradient 16x16 q40 p6`. The rest stayed green because at q55, and on
# `screen` 16x16, the filtered source still quantizes to the same decision.
# That is the honest number: this gate WITNESSES that defect on some of its
# cells and not on others.
#
# The four q55 p8 cells added on 2026-09-02 witness §1z'''''s missing
# `md_disallow_nsq_search` conjunct: with the one-term gate restored, each of
# them takes the fixed-tree path and codes squares where C codes an NSQ shape
# (`gradient 64x64 q55 p8`: 418 B against C's 295 on frame 0 alone).
#
# The five cells added on 2026-09-02 for §1z⁹ witness PD0's missing INTER
# compensation: with the `Pd0InterRef` arm removed, each one's frame 1 goes back
# to a PD0 tree of 8x8s decided from an INTRA DC prediction of a translated
# frame (`diag 64x64 q40 p8`: 35 B against C's 22).
#
# The TWENTY-TWO cells promoted on 2026-09-02 for §1z22 witness PD0's INTER
# ARM on the REFINEMENT path. `pipeline.rs` routed every `refined` superblock
# — which is every preset <= 6 on both arms — to `pd0_pick_sb_partition_m6_eval`,
# the ALLINTRA entry point, so an inter frame's PD0 predicted a DC block from
# its own recon, priced it with the KEY-frame lambda and descended to 8x8. On
# `gradient 64x64 q20 p6` frame 1 that is 80 evaluated nodes against C's five,
# and a 64x64 PART_N distortion of 2_045_904 against C's 50_800. With the
# `inter` argument reverted to `None` each of the twenty-two goes back to
# F1DIFF. Fourteen are `screen`, five `diag`, three `gradient`; nineteen are
# p6-or-p8 pairs of the same geometry, which is the tell that the defect was
# structural rather than content-keyed.
#
# The TWELVE cells promoted on 2026-09-02 for §1z21 witness the DLF VIDEO
# ARM. `pipeline.rs` handed every non-key frame `LfLevels::default()`, so the
# port signalled `loop_filter_level = 0` on every inter frame while C signalled
# 8/9/12/16/20/24. With `dlf_arm` reverted to that constant each of the twelve
# goes back to a frame-1 header that differs at `loop_filter_level[0]` —
# measured, not asserted. They cover BOTH arms of the ladder on purpose:
# the seven p6 cells (`diag 16x16 q20`, `diag 16x16 q40`, `diag 64x64 q55`,
# `diag 128x128 q55`) exercise dlf_level 3, whose levels are COPIED from the
# reference with no search; the five p8 ones (`gradient 16x16 q40/q55`,
# `screen 16x16 q40/q55`, `diag 72x72 q40`) exercise dlf_level 6's by-q closed
# form with the INTER slope, and `screen 16x16 q40 p8` in particular pins
# `me_based_dlf_skip`'s SEPARATE luma and chroma thresholds — C writes luma 9
# with chroma 0 there, which a single-threshold implementation cannot produce.
#
# The `uniform` cells are the ones that witness §1z''s intra-rate defect:
# every one of the six p6 ones was a DIFFERS before it and is byte-identical
# after. The eight p8 cells added on 2026-09-02 witness §1z''''s dropped inter
# arm: reverting that one-line wiring puts every one of them back to an
# intra-only candidate set and a frame 1 an order of magnitude too large
# (`gradient 64x64 q40 p8`: 291 B against C's 22).
#
# Usage: tools/inter_byte_gate.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)

# "<content> <w> <h> <qp> <preset> <frames> <shift>"
PASS_CELLS=(
    # 91 of a 96-cell sweep ({uniform,gradient,diag,screen} x {16,64,72,128}
    # x {q20,q40,q55} x {p6,p8}, all frames=2 low-delay P) are byte-identical
    # on BOTH frames as of docs/INTER-ENCODE-PLAN.md §1z24. The envelope this
    # campaign: 40 (§1z15) -> 49 (§1z17, MD's `is_inter_ctx` was reading an
    # INVERTED context table) -> 55 (§1z19, `av1_find_samples` ported so
    # `num_proj_ref` is real and the motion-mode ALPHABET matches C's) ->
    # 67 (§1z21, the DLF video arm) -> 89 (§1z22, PD0's INTER arm on the
    # REFINEMENT path — the port ran the ALLINTRA PD0 on every inter frame at
    # preset <= 6, with a DC prediction, the KEY-frame lambda and `min_sq` 8)
    # -> 91 (§1z24, the PER-SUPERBLOCK MD lambda: C's `full_lambda_md` /
    # `fast_lambda_md` come from `svt_aom_get_me_qindex(sb)`, so a flat frame
    # lambda priced every superblock's partition search wrong — 6633 against
    # C's 5182 / 5182 / 5182 / 7773 on `diag 72x72 q40 p6` frame 1).
    #
    # Listed in full, and regenerated wholesale rather than appended to: a
    # gate that samples its own frontier reports a smaller regression than it
    # should, and a hand-appended list drifts from the sweep it claims to
    # assert. `tools/inter_byte_matrix.sh` is that sweep.
    # ------------------------------------------------------------------
    # ABOVE 360p — added 2026-09-03, and they are the FIRST cells this gate
    # has had there. Every cell above is 16/64/72/128 square, i.e. C's
    # `input_resolution` R240p, where `mfmv_level` is 1. At 568 px and up it
    # is 2 (`sig_deriv_mode_decision_config_default`: `m <= M8 && res >
    # R360p`), and `inter_hdr_arm` REFUSED that — so the whole resolution
    # class was invisible to every byte gate in this repo, and showed up only
    # as twelve REFUSED cells in `inter_completion_scan.sh`.
    #
    # 576 is the smallest 64-ALIGNED size past the R360p threshold
    # (0x4CE00 = 314,880 luma samples; 552x552 = 304,704 is under it,
    # 568x568 = 322,624 is over). Both frames byte-identical, MEASURED
    # 2026-09-03: p6 frame 0 41,537 B / frame 1 35 B, p8 the same.
    # 568x568 is deliberately NOT here: it is a PARTIAL superblock and its
    # frame 1 is 55 B against C's 53, which is the pre-existing partial-SB
    # frontier (`inter_completion_scan` has 0 of 33 partial-SB cells
    # byte-identical), not an mfmv effect.
    #
    # TEETH: reverting the `mfmv_controls` wiring turns both into HARNESS
    # ("the encoder refused"), not FAIL — which is why they belong in a gate
    # that distinguishes the two.
    "gradient 576 576 32 6 2 3"
    "gradient 576 576 32 8 2 3"
    "uniform 16 16 20 6 2 3"
    "uniform 16 16 20 8 2 3"
    "uniform 16 16 40 6 2 3"
    "uniform 16 16 40 8 2 3"
    "uniform 16 16 55 6 2 3"
    "uniform 16 16 55 8 2 3"
    "uniform 64 64 20 6 2 3"
    "uniform 64 64 20 8 2 3"
    "uniform 64 64 40 6 2 3"
    "uniform 64 64 40 8 2 3"
    "uniform 64 64 55 6 2 3"
    "uniform 64 64 55 8 2 3"
    "uniform 72 72 20 6 2 3"
    "uniform 72 72 20 8 2 3"
    "uniform 72 72 40 6 2 3"
    "uniform 72 72 40 8 2 3"
    "uniform 72 72 55 6 2 3"
    "uniform 72 72 55 8 2 3"
    "uniform 128 128 20 6 2 3"
    "uniform 128 128 20 8 2 3"
    "uniform 128 128 40 6 2 3"
    "uniform 128 128 40 8 2 3"
    "uniform 128 128 55 6 2 3"
    "uniform 128 128 55 8 2 3"
    "gradient 16 16 20 6 2 3"
    "gradient 16 16 20 8 2 3"
    "gradient 16 16 40 6 2 3"
    "gradient 16 16 40 8 2 3"
    "gradient 16 16 55 6 2 3"
    "gradient 16 16 55 8 2 3"
    "gradient 64 64 20 6 2 3"
    "gradient 64 64 20 8 2 3"
    "gradient 64 64 40 6 2 3"
    "gradient 64 64 40 8 2 3"
    "gradient 64 64 55 6 2 3"
    "gradient 64 64 55 8 2 3"
    "gradient 72 72 20 6 2 3"
    "gradient 72 72 20 8 2 3"
    "gradient 72 72 40 6 2 3"
    "gradient 72 72 40 8 2 3"
    "gradient 72 72 55 6 2 3"
    "gradient 72 72 55 8 2 3"
    "gradient 128 128 20 6 2 3"
    "gradient 128 128 40 6 2 3"
    "gradient 128 128 40 8 2 3"
    "gradient 128 128 55 6 2 3"
    "gradient 128 128 55 8 2 3"
    "diag 16 16 20 6 2 3"
    "diag 16 16 20 8 2 3"
    "diag 16 16 40 6 2 3"
    "diag 16 16 40 8 2 3"
    "diag 16 16 55 6 2 3"
    "diag 16 16 55 8 2 3"
    "diag 64 64 20 6 2 3"
    "diag 64 64 20 8 2 3"
    "diag 64 64 40 6 2 3"
    "diag 64 64 40 8 2 3"
    "diag 64 64 55 6 2 3"
    "diag 64 64 55 8 2 3"
    "diag 72 72 20 6 2 3"
    "diag 72 72 20 8 2 3"
    "diag 72 72 40 8 2 3"
    "diag 128 128 20 6 2 3"
    "diag 128 128 40 6 2 3"
    "diag 128 128 40 8 2 3"
    "diag 128 128 55 6 2 3"
    "diag 128 128 55 8 2 3"
    "screen 16 16 20 6 2 3"
    "screen 16 16 20 8 2 3"
    "screen 16 16 40 6 2 3"
    "screen 16 16 40 8 2 3"
    "screen 16 16 55 6 2 3"
    "screen 16 16 55 8 2 3"
    "screen 64 64 20 6 2 3"
    "screen 64 64 20 8 2 3"
    "screen 64 64 40 6 2 3"
    "screen 64 64 40 8 2 3"
    "screen 64 64 55 6 2 3"
    "screen 64 64 55 8 2 3"
    "screen 72 72 20 6 2 3"
    "screen 72 72 20 8 2 3"
    "screen 72 72 40 6 2 3"
    "screen 72 72 40 8 2 3"
    "screen 72 72 55 6 2 3"
    "screen 72 72 55 8 2 3"
    "screen 128 128 20 6 2 3"
    "screen 128 128 20 8 2 3"
    "screen 128 128 40 6 2 3"
    "screen 128 128 40 8 2 3"
    "screen 128 128 55 6 2 3"
    "screen 128 128 55 8 2 3"
)
# Read below as `${OPEN_CELLS[@]+"${OPEN_CELLS[@]}"}` — see the same note in
# `inter_decode_gate.sh`: on bash < 4.4 (`/bin/bash` on macOS is 3.2.57)
# expanding an EMPTY array under `set -u` aborts the script, so a gate whose
# last open cell gets promoted would stop being able to report PASS.
#
# THE THREE 72x72 CELLS ARE CRASH-REGRESSION CELLS, and they meet
# `docs/WORKING-ON-THIS.md` §3's rule in the CRASH column rather than the byte
# one: each PANICKED before the fix that landed with them
# (`md_search.rs`'s source gather, off the end of an unpadded 72x72 plane) and
# each ENCODES after it. They are still open on bytes, which is exactly why
# they belong here and not in PASS_CELLS — and it is why `run_cell` had to
# learn to say CRASH first: as plain open cells they reported
# "open ... known" through the whole defect. One per panicking content class
# (gradient's six 72x72 cells never panicked).
OPEN_CELLS=(
    # FOUR cells, re-derived from `inter_byte_matrix.sh` rather than carried
    # forward. §1z24 (the per-superblock MD lambda) promoted two of the six
    # this list held — `gradient 72x72 q20 p6` and `diag 72x72 q20 p8` — and
    # they are now in PASS_CELLS.
    #
    # THREE OF THE FOUR ARE STILL 72x72, a PARTIAL superblock. What changed
    # is WHAT is left there: on `diag 72x72 q40 p6` the partition tree now
    # matches C's exactly (five inter blocks, `mi=(8,16)` included), and the
    # residual byte is a MODE — the port codes NEWMV where C codes NEARMV at
    # `mi=(8,16)`, same MV `(24,0)`. So §1z22's "the port stops the edge
    # descent one depth too early" is CLOSED and the remaining defect is in
    # the candidate/MVP lane, not in the partition cost model.
    "diag 72 72 40 6 2 3"   # frame 1 29 B vs C's 28
    "diag 72 72 55 6 2 3"   # frame 1 31 B vs C's 29
    "diag 72 72 55 8 2 3"   # frame 1 30 B vs C's 29
    "diag 128 128 20 8 2 3"   # frame 1 26 B vs C's 25
)

if [[ ${#PASS_CELLS[@]} -eq 0 ]]; then
    echo "inter byte gate: PASS_CELLS is empty — a gate with nothing to assert" >&2
    exit 1
fi

work="${TMPDIR:-$HOME/tmp}/inter-byte-gate.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

# echoes "<f0>/<f1>" as 1 (identical) or 0, or "ERR", or "CRASH".
#
# CRASH IS ITS OWN ANSWER. MEASURED 2026-09-02: a frame-1 panic leaves
# `rs.obu.f0` on disk, so the two checks below (status 3, missing files) both
# passed and the cell scored "0/0" — a PASS cell would have failed loudly, but
# a KNOWN-OPEN cell read as "open ... known", indistinguishable from the byte
# divergence it is not. Eighteen 72x72 cells were in exactly that state.
run_cell() {
    local content=$1 w=$2 h=$3 qp=$4 preset=$5 frames=$6 shift_px=$7
    local out="$work/${content}_${w}x${h}_q${qp}_p${preset}"
    mkdir -p "$out"
    SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="$shift_px" \
        "$HERE/identity_diff_inter.sh" "$w" "$h" "$qp" "$preset" "$frames" "$content" "$out" \
        >"$out/diff.txt" 2>&1
    local st=$?
    if [[ $st -eq 4 ]]; then echo "CRASH"; return; fi
    if [[ $st -eq 3 ]]; then echo "ERR"; return; fi
    if [[ ! -s "$out/c.obu.pts0" || ! -s "$out/rs.obu.f0" ]]; then echo "ERR"; return; fi
    local a=0 b=0
    cmp -s "$out/c.obu.pts0" "$out/rs.obu.f0" && a=1
    cmp -s "$out/c.obu.pts1" "$out/rs.obu.f1" && b=1
    echo "$a/$b"
}

fail=0; err=0; promoted=0; crashed=0
echo "== inter byte gate =="
for spec in "${PASS_CELLS[@]}"; do
    # shellcheck disable=SC2086
    got=$(run_cell $spec)
    if [[ "$got" == "CRASH" ]]; then
        echo "  CRASH  $spec  (the encoder PANICKED — not a byte divergence)"
        crashed=$((crashed + 1))
    elif [[ "$got" == "ERR" ]]; then
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
for spec in ${OPEN_CELLS[@]+"${OPEN_CELLS[@]}"}; do
    # shellcheck disable=SC2086
    got=$(run_cell $spec)
    if [[ "$got" == "CRASH" ]]; then
        echo "  CRASH  $spec  (the encoder PANICKED — a known-open cell may DIFFER, never crash)"
        crashed=$((crashed + 1))
    elif [[ "$got" == "1/1" ]]; then
        echo "  PROMOTED  $spec  (now byte-identical — move it to PASS_CELLS)"
        promoted=$((promoted + 1))
    else
        echo "  open  $spec  (frame0/frame1 identical = $got, known)"
    fi
done

echo
echo "inter byte gate: ${#PASS_CELLS[@]} required, $fail failed, ${#OPEN_CELLS[@]} known-open ($promoted now identical), $crashed crashed, $err harness errors"
# A crash fails the gate from EITHER list. A known-open cell is allowed to
# produce different bytes; it is never allowed to panic.
if [[ $crashed -gt 0 ]]; then
    echo "inter byte gate: FAIL — $crashed cell(s) PANICKED. A panic is a defect," >&2
    echo "  not a frontier state, and it is not what 'known-open' means." >&2
    exit 1
fi
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

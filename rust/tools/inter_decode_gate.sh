#!/usr/bin/env bash
# TIER-3 gate (docs/WORKING-ON-THIS.md §4) for the INTER path: a real decoder
# must accept the port's multi-frame stream.
#
# Why this exists, and why it is not redundant with the byte gates. A byte gate
# says "these bytes differ from C's"; it says NOTHING about whether the port's
# own bytes are a bitstream at all. Six defects in the pack's inter arm were
# invisible to every byte count in this repo and visible IMMEDIATELY to
# `dav1d`, because each one wrote a symbol a decoder does not read (or read one
# it does not write) and the difference showed up only as a desync:
#
#   * `write_is_inter` used a CONSTANT context where C computes a 4-valued one;
#   * an INTER block wrote the intra `uv_mode` symbol (C codes none), behind a
#     `debug_assert!` that RELEASE builds compile out;
#   * `av1_code_tx_size` picked its arm on `use_intrabc` instead of
#     `is_inter_block`, coding a `tx_size` depth symbol C omits;
#   * the luma coefficient writer picked the tx-type CDF ROWS the same wrong
#     way, at two separate call sites (tx_depth 0 and > 0);
#   * the chroma tx type followed `uv_mode` instead of the luma type on an
#     inter block, which changes the SCAN ORDER;
#   * the mi grid stamped `DC_PRED` as an inter neighbour's mode, which moves
#     `mode_context` and therefore the `newmv` CDF row.
#
# Usage: inter_decode_gate.sh [decoder]   (default: dav1d, then aomdec)
#
# The cells are split into TWO lists on purpose. `PASS_CELLS` must decode
# COMPLETELY; `OPEN_CELLS` are known not to, and are listed with the measured
# reason so the gate is a statement about a known frontier rather than a filter
# that hides one. Moving a cell from OPEN to PASS is how this gate records
# progress; a PASS cell regressing fails the gate.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
DEC="${1:-}"
if [[ -z "$DEC" ]]; then
    if command -v dav1d >/dev/null 2>&1; then DEC=dav1d
    elif command -v aomdec >/dev/null 2>&1; then DEC=aomdec
    else
        echo "inter decode gate: NO DECODER — install dav1d or aomdec." >&2
        echo "This is a HARNESS FAILURE, not a parity result." >&2
        exit 2
    fi
fi

# content w h qp preset frames shift
PASS_CELLS=(
    "uniform 64 64 40 6 2 3"    # one 64x64 all-skip inter block
    "gradient 64 64 40 6 2 0"   # zero motion: one all-skip inter block
    "gradient 16 16 50 6 2 1"   # one 8x16 all-skip inter block
    # PROMOTED 2026-09-01 by docs/INTER-ENCODE-PLAN.md §1y, which is also the
    # answer to §1x's open defect. Both cells refused frame 1 under the
    # PRE-CAMPAIGN recursion, which stamps no mode-info map at all, so every
    # neighbour-dependent inter context it wrote was derived from an
    # all-intra grid. §1s item 1 put those frames on the C-exact PD0 + leaf
    # funnel, whose `commit_leaf` stamps the real `mode` / `ref_frame` / `mv`
    # / `interp_filters` per committed block — and both decode 2/2.
    #
    # §1x's search space named three candidates
    # (`collect_neighbors_ref_counts` -> `write_ref_frames`,
    # `av1_get_pred_context_switchable_interp`, `mode_context`); all three
    # read that grid, so the fix is upstream of the choice between them and
    # the question of WHICH one was wrong is now moot rather than answered.
    "gradient 16 16 44 6 2 1"   # TWO 8x16 all-skip NEWMV blocks (§1x's repro)
    "gradient 64 64 40 6 2 3"   # the campaign's reference cell, 3px shift
)
# Empty on purpose, and that is a claim: every cell this gate knows about
# decodes completely. A NEW open cell goes here with its measured reason, so
# the gate keeps stating a frontier instead of hiding one.
OPEN_CELLS=()

work="${TMPDIR:-/tmp}/inter-decode-gate.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

decode_frames() {
    # echoes "<decoded>/<total>" or "ERR"
    local obu=$1
    case "$DEC" in
        *dav1d*)
            "$DEC" -i "$obu" -o /dev/null 2>&1 |
                grep -o 'Decoded [0-9]*/[0-9]*' | tail -1 | sed -E 's#Decoded ([0-9]*)/.*#\1#' ;;
        *)
            "$DEC" -o /dev/null --i420 "$obu" >/dev/null 2>"$work/err"
            if grep -q 'Failed to decode' "$work/err"; then echo "ERR"; else echo "ok"; fi ;;
    esac
}

run_cell() {
    local spec=$1 want=$2
    set -- $spec
    local c=$1 w=$2 h=$3 q=$4 p=$5 f=$6 sh=$7
    local out="$work/${c}_${w}x${h}_q${q}_p${p}_s${sh}"
    if ! SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAMES="$f" SVTAV1_FRAME_SHIFT="$sh" \
        SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        "$HERE/identity_run" "$c" "$w" "$h" "$q" "$p" "$out" >/dev/null 2>"$work/enc"; then
        echo "  HARNESS: $spec — the port failed to encode (check for a concurrent cargo)"
        return 2
    fi
    local got
    got=$(decode_frames "$out.obu")
    if [[ "$want" == pass ]]; then
        if [[ "$got" == "$f" || "$got" == ok ]]; then
            echo "  PASS  $spec  (decoded $got/$f)"
            return 0
        fi
        echo "  FAIL  $spec  (decoded $got, wanted $f)"
        return 1
    else
        if [[ "$got" == "$f" || "$got" == ok ]]; then
            echo "  PROMOTED  $spec  (decoded $got/$f — move it to PASS_CELLS)"
            return 3
        fi
        echo "  open  $spec  (decoded $got/$f, known)"
        return 0
    fi
}

echo "== inter decode gate ($DEC) =="
fail=0 promoted=0 harness=0
for c in "${PASS_CELLS[@]}"; do
    run_cell "$c" pass; rc=$?
    ((rc == 1)) && fail=$((fail + 1))
    ((rc == 2)) && harness=$((harness + 1))
done
echo "-- known-open cells --"
for c in "${OPEN_CELLS[@]}"; do
    run_cell "$c" open; rc=$?
    ((rc == 3)) && promoted=$((promoted + 1))
    ((rc == 2)) && harness=$((harness + 1))
done

echo
echo "inter decode gate: ${#PASS_CELLS[@]} required, $fail failed, ${#OPEN_CELLS[@]} known-open ($promoted now decode), $harness harness errors"
if ((harness)); then
    echo "HARNESS ERRORS — not a parity result." >&2
    exit 2
fi
if ((fail)); then exit 1; fi
if ((promoted)); then
    echo "A known-open cell now decodes. Move it to PASS_CELLS in this script." >&2
    exit 1
fi
echo "inter decode gate: PASS"

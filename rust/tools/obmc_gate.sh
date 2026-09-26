#!/usr/bin/env bash
# OBMC: the port selects it where C does, and its reconstruction is a decoder's.
#
# WHY THIS GATE EXISTS. `benchmarks/obmc_census_2026-09-10.meta` measured C
# coding OBMC on 22.5 % of every coded inter block at preset 0 (987 of 4382
# over twelve real-video cells), 22.2 % at MR and 27.0 % at preset 1, and
# EXACTLY ZERO from preset 2 up. The port injected no OBMC candidate at all,
# on the strength of a census that had only ever run at presets 6 and 8.
#
# WHAT IT ASSERTS, and the order matters:
#
#   1. The port SELECTS OBMC where C does — a positive control. A wiring that
#      builds the candidate but never wins with it would pass every byte gate
#      in this repo unchanged and be worth nothing.
#   2. Its RECONSTRUCTION is byte-identical to an independent decoder on every
#      frame. This is the load-bearing assertion. OBMC's blend depends on the
#      NEIGHBOURS' motion, which is not re-transmitted — the decoder re-derives
#      it from the same mi grid — so a wrong neighbour, a wrong pairing, a
#      wrong edge or a wrong blend depth shows up HERE and nowhere in a size
#      comparison. The 4-wide pairing rule (C takes a 4-wide neighbour's
#      GEOMETRY from the start of the pair and its MODE INFO from the pair's
#      second half) cost 120 luma samples on vidyo3 256x256 p0 and was
#      invisible to every other gate.
#   3. Cells where C selects NO OBMC still code none, so the injection cannot
#      have widened the candidate set where C's own ladder closes it. That is
#      what the preset-2 cells are for: `svt_aom_get_obmc_level` gives level 5
#      there and C picks OBMC on zero blocks.
#
# It does NOT assert byte-identity with C. The MDS-stage MV refinement IS wired
# (`opt_non_translation_motion_mode_obmc` -> `obmc_motion_refinement` ->
# `single_motion_search`), which is what these counts include; the
# INJECTION-time refinement (`obmc_ctrls.refine_level == 0`, obmc_level 1,
# preset MR only) is not. See `benchmarks/obmc_wiring_2026-09-11.meta`.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

CELLS=(
    "vidyo3 256x256 0 118"
    "vidyo1 256x256 0 58"
    "johnny 256x256 0 12"
    "vidyo3 128x128 0 40"
    "vidyo3 256x256 2 0"
    "vidyo1 256x256 2 0"
)

# The run is tools/mode_count_gate.py (cellrun, in parallel; counts
# mm=ObmcCausal per cell against the floor, recon == aomdec and dav1d ==
# aomdec on every frame).
exec python3 "$HERE/mode_count_gate.py" --mode ObmcCausal --label "obmc gate" \
    --frames "${OBMC_FRAMES:-2}" --qp "${OBMC_QP:-40}" \
    --env "SVTAV1_INTRA_PERIOD=64;SVTAV1_HIER_LEVELS=0" "${CELLS[@]}"

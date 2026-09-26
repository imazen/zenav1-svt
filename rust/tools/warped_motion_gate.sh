#!/usr/bin/env bash
# WARPED MOTION: the port selects it, and its reconstruction is a decoder's.
#
# WHY THIS GATE EXISTS. Until 2026-09-10 `inter_md_arm` injected no candidate
# whose `motion_mode != SimpleTranslation`, and `CONTEXT-HANDOFF.md` justified
# that with "if C selected these tools in the tested envelope the bytes would
# already differ, and they do not". `benchmarks/c_motion_mode_census_2026-09-10.meta`
# measured the premise false: C codes **88 of 1158** inter blocks WARPED_CAUSAL
# over the 24-cell video gate, all at preset 6.
#
# WHAT IT ASSERTS, and the order matters:
#
#   1. The port SELECTS warped motion where C does — a positive control. A
#      wiring that injects the candidate but never wins with it would pass
#      every byte gate in this repo unchanged and be worth nothing.
#   2. Its RECONSTRUCTION is byte-identical to an independent decoder on every
#      frame. This is the load-bearing assertion: the warp parameters are NOT
#      in the bitstream — the decoder re-derives them from the same neighbour
#      scan — so a wrong derivation, a wrong sample set, a wrong band gate or a
#      wrong chroma fallback all show up here and NOWHERE in a size comparison.
#   3. Cells where C selects NO warped block still code zero of them, so the
#      injection cannot have widened the candidate set where C's own gate is
#      closed.
#
# It does NOT assert byte-identity with C. The MDS1 warp MV refinement
# (`opt_non_translation_motion_mode_warp`, wm_level 3 -> refine_level 1) is not
# wired, so the port's warp candidates carry the unrefined MV. See
# `benchmarks/warped_motion_2026-09-10.meta`.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CELLS=(
    "vidyo3         256x256 6 20"
    "vidyo1         256x256 6 5"
    "vidyo3         128x128 6 1"
    "johnny         256x256 6 1"
    "fourpeople     128x128 6 0"
    "kristenandsara 128x128 8 0"
    "vidyo3         256x256 8 0"
    "vidyo1         128x128 8 0"
)

# The run is tools/mode_count_gate.py (cellrun, in parallel; counts
# mm=WarpedCausal per cell against the floor, recon == aomdec and dav1d ==
# aomdec on every frame).
exec python3 "$HERE/mode_count_gate.py" --mode WarpedCausal --label "warped motion gate" \
    --frames "${WMG_FRAMES:-8}" --qp "${WMG_QP:-40}" --env "SVTAV1_HIER_LEVELS=0" "${CELLS[@]}"

#!/usr/bin/env bash
# Regenerate the random-access reference-structure captures used by
# `tests/c_parity_picstruct_ra_rps.rs`.
#
# Each capture is a REAL C-encoder bitstream. The test reads
# `refresh_frame_flags`, `ref_frame_idx[]`, `show_frame` and
# `frame_to_show_map_idx` straight out of its uncompressed frame headers and
# compares them against `port_picstruct_ra` — evidence tier 2
# (`docs/WORKING-ON-THIS.md` §4), which is the strongest tier reachable for a
# `static` C function with no exported symbol.
#
#   tools/gen_ra_rps_captures.sh [outdir]
#
# Two presets, because ONE is not enough and the reason is measured:
#
#   preset 8 -> mrp level 6: list caps 3 and 2, so `prune_refs` folds GOLD onto
#               LAST and ALT onto BWD on every frame and those columns cannot
#               witness their table entries.
#   preset 4 -> mrp level 4: caps 4 and 3 (nothing folded on most frames),
#               `referencing_scheme = 1` (top-layer pictures become references,
#               and two entries of the HL1 layer-1 row move) and
#               `more_5L_refs = 1` (six entries across HL4).
#
# FRAME COUNTS ARE A HARNESS LIMIT, not a choice. The C driver's ST-mode object
# pool exhausts above 7 / 9 / 17 / 25 / 41 frames at HL1..HL5 ("empty object
# pool exhausted after pumping dispatcher"), and the counts below are the
# largest WHOLE number of mini-GOPs under that ceiling. Raising them needs a
# driver change, not a bigger number here.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
OUT="${1:-$RS_ROOT/crates/svtav1-encoder/tests/data/picstruct_ra}"
WORK="${TMPDIR:-$HOME/tmp}/ra_rps_captures"
mkdir -p "$OUT" "$WORK"

# hierarchical_levels:frame_count — one key frame plus a whole number of
# mini-GOPs of 2^H pictures.
CELLS="1:7 2:9 3:17 4:17 5:33"

for cell in $CELLS; do
    n=${cell##*:}
    if [[ ! -f "$WORK/src$n.yuv" ]]; then
        SVTAV1_FRAMES="$n" SVTAV1_INTRA_PERIOD=128 SVTAV1_HIER_LEVELS=0 \
            "$HERE/identity_run" gradient 64 64 35 8 "$WORK/src$n" >/dev/null 2>&1 || true
    fi
    [[ -s "$WORK/src$n.yuv" ]] || { echo "could not produce $WORK/src$n.yuv" >&2; exit 1; }
done

for preset in 8 4; do
    prefix=$([[ $preset == 8 ]] && echo "ra_hl" || echo "ra_p${preset}_hl")
    for cell in $CELLS; do
        h=${cell%%:*}; n=${cell##*:}
        out="$OUT/${prefix}${h}.obu"
        SVT_FRAMES="$n" SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS="$h" SVT_PRED_STRUCT=2 \
        SVT_TRACE_OUT=/dev/null \
            "$HERE/capture_c_trace/capture_c_trace" 64 64 35 "$preset" \
            "$WORK/src$n.yuv" "$out" >/dev/null 2>&1
        # capture_c_trace also writes one .ptsN file per packet; they are
        # driver debris, not part of the fixture.
        rm -f "$out".pts*
        echo "preset $preset HL$h: $n frames -> $out ($(wc -c <"$out" | tr -d ' ') bytes)"
    done
done

echo
echo "Inspect one with:  tools/ra_rps_oracle.py $OUT/ra_hl5.obu"

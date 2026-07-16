#!/usr/bin/env bash
# One-command C-vs-port divergence drill for a single encode cell.
#
#   drill_cell.sh <png> <w> <h> <cli_qp 0..63> <preset> [outdir]
#
# Does, in order (reusing artifacts in outdir):
#   1. Port encode with recon-plane dump (writes the shared rs.yuv).
#   2. C encode of the same yuv with the --wrap recon-plane dump.
#   3. Byte-compare; if identical, report and stop.
#   4. Locate the FIRST divergent recon pixel -> owning superblock.
#   5. Re-dump both sides FILTERED to that SB (port SVTAV1_DBG_MI, C
#      SVT_PICKPART_MIROW/MICOL) — filtered dumps are ~50 lines, not 45 MB.
#   6. Join the two chosen trees: structural flips, leaf mode flips, RD deltas.
#
# Replaces the ~15 manual steps this took before (encode x2, python pixel
# diff, grep sweeps, hand-joins). Total runtime ~= 4 encodes of the cell.
# If step 4 reports the recons identical, the divergence is post-recon
# (filter search) — use identity_diff.sh / the RECON_SSE probe instead.
set -euo pipefail

if [[ $# -lt 5 ]]; then
    echo "usage: $0 <png|uniform|gradient> <w> <h> <cli_qp 0..63> <preset> [outdir]" >&2
    exit 2
fi
IMG=$1 W=$2 H=$3 QP=$4 P=$5
HERE=$(cd "$(dirname "$0")" && pwd)
# identity_run content arg: synthetic generators pass through verbatim;
# anything else is a PNG path.
case "$IMG" in
uniform | gradient) CONTENT=$IMG NAME=$IMG ;;
*) CONTENT="file:$IMG" NAME=$(basename "$IMG" .png) ;;
esac
D="${6:-$HERE/../target/drill/${NAME}_${W}x${H}_q${QP}_p${P}}"
mkdir -p "$D"

# 1+2: encodes with recon dumps (wrappers force freshness). The port's stderr
# IS its symtrace op stream; C's comes from SVT_TRACE_OUT — both captured here
# so the op-level differ below costs no extra encodes.
SVTAV1_RECONDBG=1 SVTAV1_RECON_BIN="$D/p" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$P" "$D/rs" >/dev/null 2>"$D/rs.trace"
SVT_RECON_OUT="$D/c.sse" SVT_RECON_BIN="$D/c" SVT_TRACE_OUT="$D/c.trace" \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$P" "$D/rs.yuv" "$D/c.obu" >/dev/null 2>"$D/c.log"

# 3: byte compare.
if cmp -s "$D/rs.obu" "$D/c.obu"; then
    echo "IDENTICAL ($(stat -c%s "$D/rs.obu") bytes)"
    exit 0
fi
echo "DIFFERS: port=$(stat -c%s "$D/rs.obu")B C=$(stat -c%s "$D/c.obu")B first=$(cmp "$D/rs.obu" "$D/c.obu" 2>/dev/null | awk '{print $NF}' || true)"

# 4: first divergent recon pixel -> SB root mi.
set +e
MI=$(python3 "$HERE/drill_join.py" --locate "$D" "$W" "$H")
rc=$?
set -e
if [[ $rc -eq 2 ]]; then
    echo "recon planes MISSING (rc 2) — the port recon dump (SVTAV1_RECONDBG in"
    echo "pick_filter_levels_full_search) only fires on the full-DLF-search path;"
    echo "at presets whose dlf level uses the cheaper search it never runs."
    exit 2
fi
if [[ $rc -eq 3 || -z "$MI" ]]; then
    echo "recon planes IDENTICAL -> divergence is post-recon (LF/CDEF/LR search); use identity_diff.sh"
    exit 3
fi
read -r MIROW MICOL <<<"$MI"
echo "drilling SB root mi=($MIROW,$MICOL)"

# 5: SB-filtered decision dumps.
SVTAV1_NSQDBG=1 SVTAV1_DBG_MI="$MIROW,$MICOL" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$P" "$D/rs2" >/dev/null 2>"$D/rs.nsqraw"
grep -E 'NSQDBG (TS|BLK|SHAPE|SKIP|TSX)' "$D/rs.nsqraw" >"$D/rs.sbdump" || true
SVT_PICKPART_OUT="$D/c.pickpart" SVT_PICKPART_MIROW="$MIROW" SVT_PICKPART_MICOL="$MICOL" \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$P" "$D/rs.yuv" "$D/c.obu" >/dev/null 2>&1

# 6: joined report.
python3 "$HERE/drill_join.py" --join "$D/c.pickpart" "$D/rs.sbdump"

# 7: op-level first divergence (identity_diff on the ALREADY-captured traces —
# the causal ground truth for where the coded streams split; a first-divergent
# op EARLIER than the recon SB means a recon-invisible symbol/context split).
python3 "$HERE/identity_diff.py" --c-obu "$D/c.obu" --rust-obu "$D/rs.obu" \
    --c-trace "$D/c.trace" --rust-trace "$D/rs.trace" 2>/dev/null |
    grep -E 'STAGE|first|op |VERDICT' | head -12 || true
echo "artifacts: $D (c.pickpart, rs.sbdump, traces, recon planes)"

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
rm -f "$D/rs.ptree" # PACKTREE appends; stale rows would poison the join
SVTAV1_RECONDBG=1 SVTAV1_RECON_BIN="$D/p" SVTAV1_PACKTREE="$D/rs.ptree" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$P" "$D/rs" >/dev/null 2>"$D/rs.trace"
SVT_RECON_OUT="$D/c.sse" SVT_RECON_BIN="$D/c" SVT_TRACE_OUT="$D/c.trace" SVT_CTREE_OUT="$D/c.ctree" \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$P" "$D/rs.yuv" "$D/c.obu" >/dev/null 2>"$D/c.log"

# 3: byte compare.
if cmp -s "$D/rs.obu" "$D/c.obu"; then
    echo "IDENTICAL ($(stat -c%s "$D/rs.obu") bytes)"
    exit 0
fi
echo "DIFFERS: port=$(stat -c%s "$D/rs.obu")B C=$(stat -c%s "$D/c.obu")B first=$(cmp "$D/rs.obu" "$D/c.obu" 2>/dev/null | awk '{print $NF}' || true)"

# 4: first divergent DECODED pixel -> SB root mi, via the aom-decoder-rs
# oracle (tools/decode_diff). Decoding both OBUs is preset-independent
# ground truth — the internal recon dumps' C-side hook is only valid at
# presets <= M5 (at M6+ C deblocks per-SB during the walk, so its
# loop_filter_init-time buffer is already filtered; #90). Falls back to
# the raw dump compare only if the decoder tool is missing.
DD="$HERE/decode_diff/target/release/decode-diff"
if [[ ! -x "$DD" ]]; then
    echo "building decode-diff (first run)..."
    (cd "$HERE/decode_diff" && cargo build --release -q)
fi
set +e
DIFFOUT=$("$DD" "$D/c.obu" "$D/rs.obu")
rc=$?
set -e
# Oracle history note: the ORIGINAL backend (/root/work/aom-decoder-rs)
# silently desynced on Wiener-active streams; decode_diff now uses the
# aom-rs `aom-decode` crate (Gate-1 conformance-verified), validated on
# Wiener-active SVT streams via --vs-raw-prefilter (self-check == exact).
# The in-tool 128-gray guard remains as cheap insurance.
if [[ $rc -eq 0 ]]; then
    echo "decoded outputs IDENTICAL -> streams differ only in signaling that"
    echo "does not change pixels (recon-invisible symbol/context split); use"
    echo "identity_diff.sh / the op differ."
    exit 3
fi
if [[ $rc -eq 3 ]]; then
    echo "$DIFFOUT"
    echo "oracle unusable on this stream (Wiener-active) — falling back to the"
    echo "final-tree diff below for decision-level localization; the raw recon"
    echo "dump compare (p.p*/c.p*) is valid at presets <= M5."
    MIROW=0
    MICOL=0
elif [[ $rc -ne 1 ]]; then
    echo "decode-diff failed (rc $rc):"
    echo "$DIFFOUT"
    exit 2
fi
echo "$DIFFOUT" | grep -E "DIFF |NDIFF"
MI=$(echo "$DIFFOUT" | awk '/^SB /{print $2, $3}' | sed 's/mi_row=//; s/mi_col=//')
read -r MIROW MICOL <<<"$MI"
echo "drilling SB root mi=($MIROW,$MICOL)"

# 5: SB-filtered decision dumps.
SVTAV1_NSQDBG=1 SVTAV1_DBG_MI="$MIROW,$MICOL" \
    "$HERE/identity_run" "$CONTENT" "$W" "$H" "$QP" "$P" "$D/rs2" >/dev/null 2>"$D/rs.nsqraw"
grep -E 'NSQDBG (TS|BLK|SHAPE|SKIP|TSX)' "$D/rs.nsqraw" >"$D/rs.sbdump" || true
SVT_PICKPART_OUT="$D/c.pickpart" SVT_PICKPART_MIROW="$MIROW" SVT_PICKPART_MICOL="$MICOL" \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$P" "$D/rs.yuv" "$D/c.obu" >/dev/null 2>&1

# 6: FINAL-TREE diff (every preset; C side = the update_mi_map wrap) —
# flips-only, bounded output. The most direct decision-level localizer.
python3 "$HERE/tree_diff.py" "$D/c.ctree" "$D/rs.ptree" || true

# 6b: joined SEARCH-side report (<= M5 walk only; empty at M6+).
python3 "$HERE/drill_join.py" --join "$D/c.pickpart" "$D/rs.sbdump"

# 7: op-level first divergence (identity_diff on the ALREADY-captured traces —
# the causal ground truth for where the coded streams split; a first-divergent
# op EARLIER than the recon SB means a recon-invisible symbol/context split).
python3 "$HERE/identity_diff.py" --c-obu "$D/c.obu" --rust-obu "$D/rs.obu" \
    --c-trace "$D/c.trace" --rust-trace "$D/rs.trace" 2>/dev/null |
    grep -E 'STAGE|first|op |VERDICT' | head -12 || true
echo "artifacts: $D (c.pickpart, rs.sbdump, traces, recon planes)"

#!/usr/bin/env bash
# NSQ-gate REACH census on INTER frames — which of `depth_refine.rs`'s four
# NSQ skip gates actually run on a P frame, per cell, counted from the port's
# SVTAV1_NSQDBG dump. Written 2026-09-04 for docs/INTER-ENCODE-PLAN.md §1z35:
# the parent-SQ-mode modulation inside `skip_by_recon_dist` read the intra
# y_mode instead of C's unified `block_mi.mode`, and the question "does that
# fix move bytes on the campaign grid" reduces to "is the recon-dist gate
# ever ENTERED on an inter frame". This answers it per cell; it is the port's
# side of the differential (C's copy of the gate is `static`, so it cannot be
# `--wrap`ed — see §1z35 for why the C side needs no probe here).
#
# Columns: SPENTRY = shapes that entered `skip_processing_nsq` (sl=1 only),
# SKIP1..4 = shapes killed by gate 1 (split rate) / 2 (sq txs) / 3 (recon
# dist) / 4 (shapes), RDENTRY = entries into `skip_by_recon_dist`, RECONDIST =
# entries that got past its `max_dev == 0` / no-quad early-outs (the point
# where the parent mode is read), MODEDIFF = of those, how many had
# ymode != block_mi.mode (i.e. an inter parent — the only rows the fix can
# change). All counts are inter frames only (`sl=1`).
#
# Usage: tools/nsq_inter_reach_census.sh [out.tsv]
# Env:   NRC_CONTENT / NRC_SIZES / NRC_QPS / NRC_PRESETS / NRC_FRAMES (default
#        the 96-cell grid's axes at frames=2), NRC_SHIFT (SVTAV1_FRAME_SHIFT,
#        default 3 = the grid's). Sets SVTAV1_INTER_EXPERIMENTAL=1 like every
#        inter matrix script — without it the port REFUSES frame 1.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:-/dev/stdout}
CONTENT=${NRC_CONTENT:-"uniform gradient diag screen"}
SIZES=${NRC_SIZES:-"16 64 72 128"}
QPS=${NRC_QPS:-"20 40 55"}
PRESETS=${NRC_PRESETS:-"6 8"}
FRAMES=${NRC_FRAMES:-2}
D=$(mktemp -d "${TMPDIR:-$HOME/tmp}/nrc.XXXXXX")
printf 'content\tsize\tqp\tpreset\tframes\tshift\tverdict\tSPENTRY\tSKIP1\tSKIP2\tSKIP3\tSKIP4\tRDENTRY\tRECONDIST\tMODEDIFF\n' > "$OUT"
for c in $CONTENT; do for s in $SIZES; do for q in $QPS; do for p in $PRESETS; do
    d="$D/${c}_${s}_${q}_${p}"
    SVTAV1_NSQDBG=1 SVTAV1_INTER_EXPERIMENTAL=1 SVTAV1_FRAME_SHIFT="${NRC_SHIFT:-3}" \
        "$HERE/identity_diff_inter.sh" "$s" "$s" "$q" "$p" "$FRAMES" "$c" "$d" > "$d.report" 2>&1
    rc=$?
    case $rc in 0) v=BOTH;; 1) v=DIFF;; 3) v=REFUSED;; *) v=CRASH$rc;; esac
    t="$d/rs.trace"
    sp=$(grep -c 'NSQDBG SPENTRY sl=1' "$t"); rd=$(grep -c 'NSQDBG RDENTRY sl=1' "$t")
    s1=$(grep -c 'NSQDBG SKIP sl=1 .* gate=1$' "$t"); s2=$(grep -c 'NSQDBG SKIP sl=1 .* gate=2$' "$t")
    s3=$(grep -c 'NSQDBG SKIP sl=1 .* gate=3$' "$t"); s4=$(grep -c 'NSQDBG SKIP sl=1 .* gate=4$' "$t")
    rc_=$(grep -c 'NSQDBG RECONDIST sl=1' "$t")
    md=$(grep 'NSQDBG RECONDIST sl=1' "$t" | awk '{y="";b="";for(i=1;i<=NF;i++){if($i~/^ymode=/)y=substr($i,7); if($i~/^bmm=/)b=substr($i,5)}; if(y!=b)n++} END{print n+0}')
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$c" "$s" "$q" "$p" "$FRAMES" "${NRC_SHIFT:-3}" "$v" "$sp" "$s1" "$s2" "$s3" "$s4" "$rd" "$rc_" "$md" >> "$OUT"
done; done; done; done
rm -rf "$D"

#!/usr/bin/env bash
# Recon-parity gate for the MAINLINE variance-boost per-SB delta-q path:
# the encoder's reconstruction must equal aomdec's output bit-exactly, and
# every planned SB qindex must be congruent to the signalled frame base mod
# delta_q_res (what C's svt_av1_normalize_sb_delta_q guarantees, rc_aq.c:830).
#
# SVTAV1_VB_DUMP is set HERE, not inside the example: the anti-vacuity
# accounting depends on it, so the skip decision stays visible in the chain.
set -u
cd "$(dirname "$0")/.."
outdir="${1:-target/variance_boost_recon}"
mkdir -p "$outdir"
export SVTAV1_VB_DUMP="$outdir/plan.txt"
export AOMDEC="${AOMDEC:-/opt/homebrew/bin/aomdec}"
exec cargo run --release -p zenav1-svt --example variance_boost_recon -- "$outdir"

#!/usr/bin/env bash
# VIDEO-MODE op-trace localization for ONE cell, C side in the Linux container.
#
# `diff_cell.sh` is the STILL sibling: it encodes one frame with the driver's
# default (allintra) configuration, so it cannot express the video-mode cells
# the inter campaign lives on — and on those the byte verdict alone names
# nothing (`docs/INTER-ENCODE-PLAN.md` §1j: two residuals whose coded tree,
# every leaf field, every luma level and the whole pre-deblock recon already
# equalled C's, with the divergence purely in the entropy layer).
#
# Three differences from `diff_cell.sh`, all forced by the video GOP:
#   * both sides get the low-delay-P shape `identity_diff_inter.sh` uses
#     (C: SVT_FRAMES/SVT_INTRA_PERIOD/SVT_HIER_LEVELS/SVT_PRED_STRUCT;
#      Rust: SVTAV1_FRAMES/SVTAV1_INTRA_PERIOD/SVTAV1_HIER_LEVELS);
#   * the port REFUSES frame 1 (`pipeline.rs`'s is_key guard) and exits 3.
#     That is the expected state of the campaign, not a failure, so this script
#     does not treat it as one — it diffs FRAME 0;
#   * the OBUs compared are the per-frame files (`c.obu.pts0` / `rs.obu.f0`),
#     never the concatenation: in a multi-frame stream the first divergence of
#     a concatenation is uninformative.
#
# Usage: vdiff_cell.sh <w> <h> <qp> <preset> <content> [frames] [outdir-name]
#   content: uniform | gradient | diag | screen | screenrep | file:<png>
# Env: CTRACE_WORK (default ~/tmp/zenav1-ctrace) — the outdir lives under it,
#      because the container can only see paths inside that mount.
#      IDENTITY_VERBOSE=1 for the full field walk / op context.
#
# Reading the report. `identity_diff.py` gives the BYTE verdict, which is the
# authoritative one. Its op INDEX is not reliable on a video cell — its
# alignment assumes one frame per trace and the still driver's prologue — so
# this script also runs `optrace_first_diff.py`, which splits both traces on
# `W RESET`, compares C frame 0 against the port's real pack writer, and
# normalizes C's BOOL / BOOLEQ spellings against the port's 2-symbol CDF
# writes. THAT is the localization. Its positive control is any byte-identical
# video cell (`gradient 72x88 q40 p4`: "op streams identical").
#
# When it names an op, grep the printed `icdf` value in
# `crates/svtav1-encoder/src/entropy/default_cdfs.rs` — that names the CDF
# table, and the table names the syntax element. That is how the `TX_SIZE_CDF`
# symbol coded under TX_MODE_LARGEST was found (`docs/INTER-ENCODE-PLAN.md`
# §1j).
#
# Exit status: 0 iff frame 0 is byte-identical.
set -uo pipefail

[[ $# -ge 5 ]] || {
    echo "usage: $0 <w> <h> <qp> <preset> <content> [frames] [outdir-name]" >&2
    exit 2
}
W=$1 H=$2 QP=$3 PRESET=$4 CONTENT=$5
FRAMES="${6:-2}"
HERE=$(cd "$(dirname "$0")" && pwd)
RS_TOOLS=$(cd "$HERE/.." && pwd)
WORK="${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}"
NAME="${7:-vdiff_$(basename "${CONTENT#*:}" .png)_${W}x${H}_q${QP}_p${PRESET}}"
OUT="$WORK/$NAME"
mkdir -p "$OUT"

# The port writes rs.yuv (N frames), rs.obu and rs.obu.f<i>. Exit 3 is the
# INTER refusal — expected while the campaign is on key frames, so it is not
# an error here; anything else is.
rs_status=0
SVTAV1_FRAMES="$FRAMES" \
    SVTAV1_INTRA_PERIOD="${SVTAV1_INTRA_PERIOD:-64}" \
    SVTAV1_HIER_LEVELS="${SVTAV1_HIER_LEVELS:-0}" \
    "$RS_TOOLS/identity_run" "$CONTENT" "$W" "$H" "$QP" "$PRESET" "$OUT/rs" \
    2>"$OUT/rs.trace" || rs_status=$?
if [[ $rs_status -ne 0 && $rs_status -ne 3 ]]; then
    echo "PORTFAIL (status $rs_status)" >&2
    exit 3
fi

SVT_FRAMES="$FRAMES" \
    SVT_INTRA_PERIOD="${SVT_INTRA_PERIOD:--1}" \
    SVT_HIER_LEVELS="${SVT_HIER_LEVELS:-0}" \
    SVT_PRED_STRUCT="${SVT_PRED_STRUCT:-1}" \
    SVT_TRACE_OUT="$OUT/c.trace" \
    "$HERE/run.sh" "$W" "$H" "$QP" "$PRESET" "$OUT/rs.yuv" "$OUT/c.obu" \
    "${SVTAV1_BD:-8}" 2>"$OUT/c.stderr" >"$OUT/c.stdout" || {
    tail -20 "$OUT/c.stderr" >&2
    echo "CFAIL" >&2
    exit 4
}

# `set -u` + an EMPTY array is an unbound-variable error on bash 3.2 (the
# macOS system bash this repo's scripts run under), so the flag is a plain
# string, not an array — `diff_cell.sh` gets away with the array only because
# it never runs with the variable empty on that shell.
verbose_flag=""
[[ -n "${IDENTITY_VERBOSE:-}" ]] && verbose_flag="--verbose"
# shellcheck disable=SC2086  # deliberate word-splitting of the optional flag
python3 "$RS_TOOLS/identity_diff.py" \
    --c-obu "$OUT/c.obu.pts0" --rust-obu "$OUT/rs.obu.f0" \
    --c-trace "$OUT/c.trace" --rust-trace "$OUT/rs.trace" \
    $verbose_flag | tee "$OUT/report.txt"
rc=${PIPESTATUS[0]}
echo
python3 "$HERE/optrace_first_diff.py" "$OUT/c.trace" "$OUT/rs.trace" |
    tee -a "$OUT/report.txt"
echo "artifacts: $OUT" >&2
exit "$rc"

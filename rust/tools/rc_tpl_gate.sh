#!/usr/bin/env bash
# TPL (random access, aq_mode 2) and low-delay CBR against mainline C.
#
# WHY THIS GATE EXISTS. Both paths are live — `aq_mode 2` turns TPL on under
# random access (inter_hdr_arm::scs_tpl) and `RcMode::Cbr` is accepted under
# low delay — and both had byte-identity claims only in commit messages
# (6f263c03, 21c6eb56), with no gate behind them. Measured 2026-09-26 on
# i265 against mainline-4.2.0: TPL is byte-identical on 5 of 8 synthetic
# RA cells; CBR on NONE of 34, every one diverging in the KEY frame, where
# C spends more bits (lower qp) than the port — the CBR commit's cells were
# most likely measured against the hybrid oracle, which was the default
# then.
#
# WHAT IT ASSERTS, per cell (tools/cellrun.py, in parallel):
#   - the C verdict pinned in `expect` — IDENTICAL or DIFFERS, so a cell
#     that regresses fails AND a cell that starts matching fails until the
#     pin moves with it (an out-of-date measurement is the thing this repo
#     keeps getting burned by);
#   - recon == aomdec and dav1d == aomdec on every frame (the stream is
#     correct whatever C does);
#   - anti-vacuity: the aq_mode-2 cell's streams differ from an aq_mode-0
#     twin on BOTH sides (TPL really ran), and a 200 kbps CBR cell's differ
#     from its 50 kbps twin (the target reached the rate control).
#
# Usage: tools/rc_tpl_gate.sh     Env: RTG_JOBS (4), AOMDEC, DAV1D
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"
. "$HERE/lib_corpus.sh"

ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
if [ ! -f "$ASSETS/johnny_256x256_8f.i420" ]; then
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || { echo "rc_tpl_gate: could not fetch the derf clips" >&2; exit 2; }
fi

work="${TMPDIR:-$HOME/tmp}/rc-tpl.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT
LIST="$work/rc_tpl.cells.tsv"

RA_P="SVTAV1_FRAMES=9;SVTAV1_INTRA_PERIOD=64;SVTAV1_HIER_LEVELS=3;SVT_PRED_STRUCT=2;SVTAV1_FRAME_SHIFT=3"
RA_C="SVT_FRAMES=9;SVT_INTRA_PERIOD=64;SVT_HIER_LEVELS=3;SVT_PRED_STRUCT=2"
# C refuses rate control with adaptive quantisation off ("Adaptive
# quantization can not be turned OFF when RC ON"), so CBR runs aq_mode 2 on
# both sides; under low delay that does not turn TPL on.
CBR_P="SVTAV1_FRAMES=8;SVTAV1_INTRA_PERIOD=64;SVTAV1_HIER_LEVELS=0;SVTAV1_RC_MODE=2;SVT_AQ_MODE=2"
CBR_C="SVT_FRAMES=8;SVT_INTRA_PERIOD=-1;SVT_HIER_LEVELS=0;SVT_PRED_STRUCT=1;SVT_RC_MODE=2;SVT_AQ_MODE=2"

{
printf 'name\tcontent\tw\th\tqp\tpreset\tenv_port\tenv_c\texpect\tcheck\tdiffers_from\tc_differs_from\n'
# name content preset aq expect differs_from
while read -r name content preset aq expect twin; do
    [ -n "$name" ] || continue
    [ "$twin" = "-" ] && twin=""
    printf '%s\t%s\t256\t256\t40\t%s\t%s;SVT_AQ_MODE=%s\t%s;SVT_AQ_MODE=%s\t%s\tc,recon,dav1d\t%s\t%s\n' \
        "$name" "$content" "$preset" "$RA_P" "$aq" "$RA_C" "$aq" "$expect" "$twin" "$twin"
done <<'EOF'
ra_aq0_gradient_p6   gradient 6 0 IDENTICAL -
ra_aq2_gradient_p6   gradient 6 2 IDENTICAL ra_aq0_gradient_p6
ra_aq2_gradient_p8   gradient 8 2 DIFFERS   -
ra_aq2_uniform_p6    uniform  6 2 IDENTICAL -
ra_aq2_uniform_p8    uniform  8 2 IDENTICAL -
ra_aq2_screen_p6     screen   6 2 IDENTICAL -
ra_aq2_screen_p8     screen   8 2 IDENTICAL -
ra_aq2_diag_p6       diag     6 2 DIFFERS   -
ra_aq2_diag_p8       diag     8 2 DIFFERS   -
EOF
# name content preset tbr expect differs_from
while read -r name content preset tbr expect twin; do
    [ -n "$name" ] || continue
    [ "$twin" = "-" ] && twin=""
    case "$content" in
        johnny|vidyo3) src="rawseq:$ASSETS/${content}_256x256_8f.i420"; shift_env="" ;;
        *) src="$content"; shift_env=";SVTAV1_FRAME_SHIFT=3" ;;
    esac
    printf '%s\t%s\t256\t256\t40\t%s\t%s;SVTAV1_TBR=%s%s\t%s;SVT_TBR=%s\t%s\tc,recon,dav1d\t%s\t%s\n' \
        "$name" "$src" "$preset" "$CBR_P" "$tbr" "$shift_env" "$CBR_C" "$tbr" "$expect" "$twin" "$twin"
done <<'EOF'
cbr_gradient_t50     gradient 6 50  DIFFERS -
cbr_gradient_t200    gradient 6 200 DIFFERS cbr_gradient_t50
cbr_diag_t50         diag     6 50  DIFFERS -
cbr_diag_t200        diag     6 200 DIFFERS cbr_diag_t50
cbr_johnny_t50       johnny   6 50  DIFFERS -
cbr_johnny_t200      johnny   6 200 DIFFERS cbr_johnny_t50
cbr_vidyo3_t50       vidyo3   6 50  DIFFERS -
cbr_vidyo3_t200      vidyo3   6 200 DIFFERS cbr_vidyo3_t50
EOF
} >"$LIST"

env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
    python3 "$HERE/cellrun.py" "$LIST" --out "$work/result.tsv" --bytes-only --jobs "${RTG_JOBS:-4}"
rc=$?
echo "== rc/tpl gate (TPL under random access, low-delay CBR; mainline C) =="
column -t -s$'\t' < <(cut -f1,2,5,6,7 "$work/result.tsv")
[ "$rc" -eq 0 ] && echo "rc/tpl gate: OK"
exit "$rc"

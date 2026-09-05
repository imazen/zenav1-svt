#!/usr/bin/env bash
# The port's frame-level GLOBAL-MOTION derivation against C's, per cell.
#
# WHAT IT ASSERTS. For every cell, the port's `GMPORT` line
# (`SVTAV1_GMDBG`, `crate::port_global_me::global_motion_estimation`) and C's
# `GMFRAME` line (`SVT_GM_OUT`, the `--wrap` interposer on the real
# `svt_aom_global_motion_estimation`) must agree on ALL of:
#
#   b64  total_me_sad  avg_me_sad  total_gm_sbs  ds(ownsample level)
#
# and the port's `all_identity` must equal `!is_gm_on` — i.e. when C's search
# leaves every reference IDENTITY the port must say so, and when C fits a
# model the port must NOT claim identity. That last pair is the one the
# pipeline's refusal turns on, so a disagreement there is a
# silently-wrong-bitstream bug, not a cosmetic one.
#
# WHY IT EXISTS. `svt_aom_derive_gm_level` is non-zero for EVERY inter frame at
# preset <= 4, and the pipeline used to refuse on that alone — a fact about the
# PRESET. Whether C actually fits a model is
# `svt_aom_global_motion_estimation`'s own `average_me_sad` gate, and nothing
# in this repo could see it. This gate is that eye. It found, on its first run,
# that the port's `rc_me_allow_gm` was 0 on every b64 where C's was 1
# (`MePicParams::gm_enabled` was hard-coded `false`).
#
# ANTI-VACUITY, and it is the whole point here. A gate that only ever sees
# `avg_me_sad = 0` proves nothing about the branch that matters, because a
# pure translation — which is all `SVTAV1_FRAME_SHIFT` produces — is exactly
# what open-loop ME cancels, so the residual floors the integer divide to 0 on
# EVERY synthetic cell and on real photo content too (measured: gradient/diag/
# screen at 64..512 and `crop:` CID22 at shifts 3/13/37, all `avg_me_sad=0`,
# `is_gm_on=0`). So the cell list MUST contain at least one cell where C
# reaches a NON-IDENTITY model, and the gate FAILS if it does not. That is what
# `SVTAV1_FRAME_ZOOM_NUM/_DEN` is for: a zoom about the frame centre is a
# ROTZOOM the ME cannot cancel with an integer MV, and C fits one
# (`wmtype=2`, `is_global=1`).
#
# REQUIRES a `-Wl,--wrap` linker for the C side. On macOS ld64 there is none,
# so the C half runs through `tools/ctrace-linux/run.sh` (docker, HOST arch) —
# set GMJ_CTRACE=1 for that. A missing driver is exit 2, never a pass.
#
# Usage: tools/gm_join_gate.sh [outdir]
# Env:
#   GMJ_CELLS   — one spec per line: "<content> <w> <h> <qp> <preset> <shift> <zoomnum> <zoomden>"
#   GMJ_CTRACE  — 1 to drive the C side through tools/ctrace-linux/run.sh
set -uo pipefail
# bash >= 4: mapfile/declare -A. bash 3.2 (macOS /bin/bash) silently yields an
# EMPTY array and the gate passes over nothing (docs/WORKING-ON-THIS.md §5).
[[ ${BASH_VERSINFO[0]} -ge 4 ]] || { echo "FATAL: needs bash >= 4 (got $BASH_VERSION); run under a newer bash" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
REPO=$(cd "$RS_ROOT/.." && pwd)
cd "$RS_ROOT"
. "$HERE/lib_corpus.sh"

OUT="${1:-$RS_ROOT/target/gm-join}"
mkdir -p "$OUT"

PHOTO_DIR=$(corpus_dir codec-corpus/CID22/CID22-512/training) || true
PHOTO="$PHOTO_DIR/3571065.png"

# The default cell list covers all THREE branches C's chain has, because the
# refusal turns on which one a frame takes:
#   * the search never runs (`avg_me_sad = 0`)  — the shift cells;
#   * the search RUNS and returns IDENTITY      — the 16x16 cells, where
#     `avg_me_sad` is 102 / 104 but RANSAC has too few correspondences to fit
#     anything (measured: C emits no `GMCOST`/`GMREFINE` line at all there);
#   * the search runs and FITS a model          — the two zoom cells.
# The last of the three is the anti-vacuity load: the same real image with a
# 33/32 and a 9/8 zoom, where C fits a ROTZOOM per list.
DEFAULT_CELLS="\
gradient 16 16 40 2 3 1 1
diag 16 16 40 0 3 1 1
gradient 64 64 40 2 3 1 1
gradient 128 128 40 0 3 1 1
diag 256 256 40 2 3 1 1
screen 128 128 40 4 3 1 1
crop:$PHOTO 128 128 40 2 3 1 1
crop:$PHOTO 256 256 40 2 3 1 1
crop:$PHOTO 256 256 40 2 0 33 32
crop:$PHOTO 512 512 40 2 0 9 8"
CELLS="${GMJ_CELLS:-$DEFAULT_CELLS}"

if [[ ! -r "$PHOTO" ]]; then
    echo "FATAL: corpus image missing: $PHOTO" >&2
    echo "  (codec-corpus/CID22/CID22-512/training — see tools/lib_corpus.sh)" >&2
    exit 2
fi

CTRACE="${GMJ_CTRACE:-0}"
if [[ "$CTRACE" == 1 ]]; then
    CWORK="${CTRACE_WORK:-$HOME/tmp/zenav1-ctrace}"
    mkdir -p "$CWORK/gmjoin"
fi

rows=0; bad=0; nonident=0; models_joined=0; searched=0
: > "$OUT/join.tsv"
printf 'cell\tfield\tC\tport\n' > "$OUT/mismatch.tsv"

while read -r content w h qp preset shift zn zd; do
    [[ -n "${content:-}" ]] || continue
    tag="$(printf '%s' "$content" | tr -c 'A-Za-z0-9' '_')_${w}x${h}_q${qp}_p${preset}_s${shift}_z${zn}-${zd}"
    d="$OUT/$tag"
    mkdir -p "$d"

    # 1. PORT: encode with the GM derivation dumped. The encoder may REFUSE
    #    (exit 3) on a cell where C fits a model — that is the shipped
    #    behaviour and is not a gate failure; the GMPORT line is printed
    #    before the refusal, which is why it is still joinable.
    SVTAV1_GMDBG=1 SVTAV1_INTER_EXPERIMENTAL=1 \
    SVTAV1_FRAME_SHIFT="$shift" SVTAV1_FRAME_ZOOM_NUM="$zn" SVTAV1_FRAME_ZOOM_DEN="$zd" \
    SVTAV1_FRAMES=2 SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
        "$HERE/identity_run" "$content" "$w" "$h" "$qp" "$preset" "$d/rs" \
        >"$d/rs.out" 2>"$d/rs.trace" || true
    port=$(grep -m1 '^GMPORT ' "$d/rs.trace" || true)
    if [[ -z "$port" ]]; then
        echo "FATAL: no GMPORT line for $tag — the port's derivation never ran" >&2
        echo "  (SVTAV1_GMDBG unset in the binary? frame 1 refused before ME?)" >&2
        exit 2
    fi

    # 2. C: the same .yuv, matched GOP, with SVT_GM_OUT.
    if [[ "$CTRACE" == 1 ]]; then
        cp "$d/rs.yuv" "$CWORK/gmjoin/$tag.yuv"
        SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
        SVT_GM_OUT="$CWORK/gmjoin/$tag.gm" \
            "$HERE/ctrace-linux/run.sh" "$w" "$h" "$qp" "$preset" \
            "$CWORK/gmjoin/$tag.yuv" "$CWORK/gmjoin/$tag.obu" 8 \
            >"$d/c.log" 2>&1 || { echo "FATAL: ctrace run failed for $tag (see $d/c.log)" >&2; exit 2; }
        cp "$CWORK/gmjoin/$tag.gm" "$d/c.gm"
    else
        SVT_FRAMES=2 SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
        SVT_GM_OUT="$d/c.gm" \
            "$HERE/capture_c_trace/capture_c_trace" "$w" "$h" "$qp" "$preset" \
            "$d/rs.yuv" "$d/c.obu" >"$d/c.log" 2>&1 ||
            { echo "FATAL: capture_c_trace failed for $tag (see $d/c.log)" >&2; exit 2; }
    fi
    cline=$(grep -m1 '^GMFRAME ' "$d/c.gm" 2>/dev/null || true)
    if [[ -z "$cline" ]]; then
        echo "FATAL: no GMFRAME line for $tag — the SVT_GM_OUT interposer never fired" >&2
        echo "  (no -Wl,--wrap linker? try GMJ_CTRACE=1. An empty dump is NOT a pass.)" >&2
        exit 2
    fi

    kv() { sed -n "s/.*[[:space:]]$2=\([^[:space:]]*\).*/\1/p" <<<"$1"; }
    declare -A C P
    for f in b64 total_me_sad avg_me_sad total_gm_sbs ds; do
        C[$f]=$(kv "$cline" "$f"); P[$f]=$(kv "$port" "$f")
    done

    # THE MODEL ITSELF, per (list, ref). C's `GMREF` lines carry `wmtype` and
    # `wmmat`; the port's `GMPORTREF` lines carry the same two from
    # `port_global_me::compute_global_motion`. Only NON-IDENTITY models produce
    # a line on either side (C skips the initialised-and-untouched majority),
    # so the two sets must match exactly — a port that found a model C did not,
    # or missed one C found, shows up as an unmatched key.
    #
    # This is the assertion the whole search port rests on: the derivation
    # deciding "C would search" is only half the fact, and the other half is
    # what the search RETURNS.
    c_models=$(sed -n 's/^GMREF .*list=\([0-9]*\) ref=\([0-9]*\) wmtype=\([0-9-]*\) is_global=[0-9]* wmmat=\([^ ]*\).*/\1,\2,\3,\4/p' "$d/c.gm" | sort)
    p_models=$(grep '^GMPORTREF ' "$d/rs.trace" 2>/dev/null |
        sed -n 's/.*list=\([0-9]*\) ref=\([0-9]*\) wmtype=\([0-9-]*\) *wmmat=\[\([^]]*\)\].*/\1,\2,\3,\4/p' |
        tr -d ' ' | sort)
    if [[ "$c_models" != "$p_models" ]]; then
        printf '%s\t%s\t%s\t%s\n' "$tag" "models" "${c_models//$'\n'/;}" "${p_models//$'\n'/;}" >> "$OUT/mismatch.tsv"
        bad=$((bad + 1))
    fi
    [[ -n "$c_models" ]] && models_joined=$((models_joined + $(printf '%s\n' "$c_models" | wc -l | tr -d ' ')))
    # C's `is_gm_on` is 1 iff some reference ended non-IDENTITY, and the port's
    # counterpart is `GMPORTMODELS`' own `is_gm_on` — the SEARCH's verdict.
    # NOT `GMPORT`'s `all_identity`, which is the DERIVATION's: on a cell where
    # C runs the search and RANSAC fits nothing (the 16x16 cells) those two
    # disagree by design, and comparing the wrong one made this gate red on a
    # port that was right.
    c_is_gm_on=$(kv "$cline" is_gm_on)
    pmodels=$(grep -m1 '^GMPORTMODELS ' "$d/rs.trace" 2>/dev/null || true)
    if [[ -z "$pmodels" ]]; then
        echo "FATAL: no GMPORTMODELS line for $tag" >&2
        exit 2
    fi
    p_is_gm_on=$(kv "$pmodels" is_gm_on)
    [[ "$c_is_gm_on" == 1 ]] && nonident=$((nonident + 1))
    [[ "$(kv "$pmodels" searched)" == 1 ]] && searched=$((searched + 1))

    rows=$((rows + 1))
    printf '%s\tC:%s\tPORT:%s\n' "$tag" "$cline" "$port" >> "$OUT/join.tsv"
    for f in b64 total_me_sad avg_me_sad total_gm_sbs ds; do
        if [[ "${C[$f]}" != "${P[$f]}" ]]; then
            printf '%s\t%s\t%s\t%s\n' "$tag" "$f" "${C[$f]}" "${P[$f]}" >> "$OUT/mismatch.tsv"
            bad=$((bad + 1))
        fi
    done
    if [[ "$c_is_gm_on" != "$p_is_gm_on" ]]; then
        printf '%s\t%s\t%s\t%s\n' "$tag" "is_gm_on" "$c_is_gm_on" "$p_is_gm_on" >> "$OUT/mismatch.tsv"
        bad=$((bad + 1))
    fi
    echo "  $tag  C[$cline]  PORT[$port]"
done <<< "$CELLS"

echo
echo "gm join gate: $rows cell(s) joined, $bad mismatching field(s)"
echo "  cells where C fitted a NON-IDENTITY model: $nonident"
echo "  non-IDENTITY (list, ref) models joined field for field: $models_joined"
echo "  cells where the port RAN C's search: $searched"
if [[ $rows -eq 0 ]]; then
    echo "ANTI-VACUITY FAIL: zero cells joined" >&2
    exit 1
fi
if [[ $searched -eq 0 ]]; then
    echo "ANTI-VACUITY FAIL: the port never RAN C's per-reference search on any cell —" >&2
    echo "  every cell short-circuited at the average_me_sad gate, so compute_global_motion" >&2
    echo "  was not exercised at all." >&2
    exit 1
fi
if [[ $models_joined -eq 0 ]]; then
    echo "ANTI-VACUITY FAIL: no non-IDENTITY model was joined; the search's OUTPUT" >&2
    echo "  was never compared against C's, only the decision to run it." >&2
    exit 1
fi
if [[ $nonident -eq 0 ]]; then
    echo "ANTI-VACUITY FAIL: no cell reached a non-IDENTITY global-motion model." >&2
    echo "  Every cell had avg_me_sad below C's gate, so the branch this gate exists" >&2
    echo "  to cover was never taken. Add a zoom cell (SVTAV1_FRAME_ZOOM_NUM/_DEN)." >&2
    exit 1
fi
if [[ $bad -ne 0 ]]; then
    echo "FAIL — see $OUT/mismatch.tsv" >&2
    cat "$OUT/mismatch.tsv" >&2
    exit 1
fi
echo "PASS"

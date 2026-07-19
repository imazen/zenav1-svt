#!/usr/bin/env bash
# bd10 low-preset failure CLASSIFIER (task #94, low-preset axis).
#
#   bd10_classify.sh [-o out.tsv] [-c "gradient diag"] [-d "64 128"] \
#                    [-q "12 20 32 40 55"] [-p "0 3 6"]
#
# For each cell it runs the port (SVTAV1_BD=10 + SVTAV1_PACKTREE) and the real C
# encoder (capture_c_trace .. 10 + SVT_CTREE_OUT) on the SAME yuv, `cmp`s the
# OBUs, and — when they differ — classifies the divergence into the three known
# bd10 low-preset axes so the highest-yield one can be fixed first:
#
#   PART   the coded partition GEOMETRY differs (port-only mi keys, or a bsize
#          flip). Axis 1: PD1 depth-refine + NSQ.
#   MODE   geometry identical, a per-leaf mode/uv/fi/txd/angle field flips.
#          Axis 2: the bd10 leaf mode decision (filter-intra / CfL / mode RD).
#   FH     TREES IDENTICAL *and* the tile payload is byte-identical — the ONLY
#          divergence is a frame-header post-filter value. Axis 3: the bd10
#          CDEF-search / Wiener-LR search. This is the strongest possible
#          evidence for axis 3, because nothing upstream moved.
#   COEFF  trees identical, tile payload differs — a coded-level divergence
#          inside the bd10 re-encode envelope.
#
# WHY THE ORDER MATTERS (do not "simplify" this into a first-divergence label):
# a PART/MODE flip changes the recon, which changes what the CDEF/LR searches
# see, so such cells ALSO show a frame-header difference. That FH difference is
# a DOWNSTREAM SYMPTOM, not independent evidence of an axis-3 bug. Only a cell
# whose tree AND tile payload are identical proves the post-filter search itself
# diverges. The script therefore reports the primary axis (the upstream-most
# one) plus, in the `also` column, every co-occurring signal, so a cell needing
# two fixes is never miscounted as needing one.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)
cd "$RS_ROOT"

OUT=""
CONTENTS="gradient diag"
DIMS="64 128"
QPS="12 20 32 40 55"
PRESETS="0 3 6"
while getopts "o:c:d:q:p:" opt; do
    case $opt in
    o) OUT=$OPTARG ;;
    c) CONTENTS=$OPTARG ;;
    d) DIMS=$OPTARG ;;
    q) QPS=$OPTARG ;;
    p) PRESETS=$OPTARG ;;
    *)
        echo "bad flag" >&2
        exit 2
        ;;
    esac
done

W="${TMPDIR:-/tmp}/bd10cls.$$"
mkdir -p "$W"
trap 'rm -rf "$W"' EXIT

emit() { if [ -n "$OUT" ]; then printf '%s\n' "$*" >>"$OUT"; fi; printf '%s\n' "$*"; }
[ -n "$OUT" ] && : >"$OUT"
emit $'cell\tcontent\tw\tqp\tpreset\tverdict\taxis\talso\tdetail'

for content in $CONTENTS; do
    for d in $DIMS; do
        for qp in $QPS; do
            for p in $PRESETS; do
                # `content` may be `file:/path/to/x.png` — strip the dir/ext so
                # the tag stays a single filename-safe token.
                cshort=${content##*/}
                cshort=${cshort%.png}
                tag="${cshort}_${d}x${d}_q${qp}_p${p}"
                rm -f "$W/rs.ptree"
                if ! SVTAV1_BD=10 SVTAV1_PACKTREE="$W/rs.ptree" \
                    "$HERE/identity_run" "$content" "$d" "$d" "$qp" "$p" "$W/rs" \
                    >/dev/null 2>"$W/rs.err"; then
                    emit "$tag	$content	$d	$qp	$p	RSERR	-	-	port encode failed"
                    continue
                fi
                if ! SVT_TRACE_OUT=/dev/null SVT_CTREE_OUT="$W/c.ctree" \
                    "$HERE/capture_c_trace/capture_c_trace" "$d" "$d" "$qp" "$p" \
                    "$W/rs.yuv" "$W/c.obu" 10 >/dev/null 2>"$W/c.err"; then
                    emit "$tag	$content	$d	$qp	$p	CERR	-	-	C encode failed"
                    continue
                fi
                if cmp -s "$W/rs.obu" "$W/c.obu"; then
                    emit "$tag	$content	$d	$qp	$p	MATCH	-	-	byte-identical"
                    continue
                fi

                # --- classify -------------------------------------------------
                tre=$(python3 "$HERE/tree_diff.py" "$W/c.ctree" "$W/rs.ptree" --max 4 2>&1)
                # -v so the definitive "tile payload: C NB, Rust MB -> IDENTICAL"
                # line is available; the quiet form omits it entirely when the
                # payloads match, which is exactly the case we must detect.
                idfv=$(python3 "$HERE/identity_diff.py" -v --c-obu "$W/c.obu" \
                    --rust-obu "$W/rs.obu" 2>&1)
                idf=$(tail -4 <<<"$idfv")

                # geometry: "geometry: N C-only / M port-only"; M>0 == real
                # geometry mismatch (C stamps finer sub-keys even when equal).
                portonly=$(sed -n 's/.*geometry: [0-9]* C-only \/ \([0-9]*\) port-only.*/\1/p' <<<"$tre")
                portonly=${portonly:-0}
                flipcounts=$(sed -n 's/^  flip counts: //p' <<<"$tre")
                bsizeflip=0
                grep -q 'bsize=' <<<"$flipcounts" && bsizeflip=1
                otherflip=0
                grep -qE '(mode|uv|fi|txd|ady|aduv|cflidx|cflsgn)=' <<<"$flipcounts" && otherflip=1
                skipflips=$(sed -n 's/.*, \([0-9]*\) skip flips.*/\1/p' <<<"$tre")
                skipflips=${skipflips:-0}

                # tile payload equality + the FH field, from identity_diff.
                stage=$(sed -n 's/^STAGE: //p' <<<"$idf")
                tile_same=0
                grep -qE 'tile payload:.*-> IDENTICAL' <<<"$idfv" && tile_same=1

                sigs=""
                [ "$portonly" -gt 0 ] || [ "$bsizeflip" -eq 1 ] && sigs="${sigs}PART,"
                [ "$otherflip" -eq 1 ] && sigs="${sigs}MODE,"
                [ "$skipflips" -gt 0 ] && sigs="${sigs}SKIP,"
                grep -q '^FH' <<<"$stage" && sigs="${sigs}FH,"
                sigs=${sigs%,}

                if [ "$portonly" -gt 0 ] || [ "$bsizeflip" -eq 1 ]; then
                    axis=PART
                elif [ "$otherflip" -eq 1 ] || [ "$skipflips" -gt 0 ]; then
                    axis=MODE
                elif [ "$tile_same" -eq 1 ]; then
                    axis=FH
                else
                    axis=COEFF
                fi
                detail=$(tr '\t' ' ' <<<"$stage")
                emit "$tag	$content	$d	$qp	$p	DIFF	$axis	${sigs:--}	$detail"
            done
        done
    done
done

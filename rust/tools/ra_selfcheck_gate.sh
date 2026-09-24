#!/usr/bin/env bash
# RANDOM-ACCESS (hierarchical GOP) selfcheck: the port's RA stream decoded by
# an INDEPENDENT decoder, every DISPLAY frame byte-identical to the encoder's
# final reconstruction.
#
# WHAT THIS ASSERTS that the other gates do not. `c_parity_picstruct_ra_rps`
# proves the REFERENCE STRUCTURE (ref_frame_idx / refresh flags / show_frame /
# show_existing) is what C would signal — it says nothing about whether the
# tile data a decoder reconstructs matches what the encoder intended. And
# `video_selfcheck_gate.sh` covers only the flat low-delay chain, where coded
# order == display order. RA reorders: frames code in decode order and emit
# `show_existing_frame` OBUs for hidden pictures, so a slot/CDF/motion-field
# binding that is right under LDP can still be wrong here — the failure mode
# this file exists to catch is a stream that DECODES but whose late pictures
# are not the pictures the encoder coded (stored-context desync).
#
# HOW IT CHECKS. `SVTAV1_FINAL_RECON` now names each dumped reconstruction by
# the CODED frame's display order (`last_recon_display_order`), so under RA a
# hidden base layer's recon lands on the display index it will be shown at —
# the decoder's output at that position must equal it byte-for-byte. The gate
# requires all N display frames present and all N recons matching.
#
# CELLS: a complete-mini-GOP ladder (2/4/8/16/32-picture windows at
# hierarchical_levels 1..5) on two real clips, plus one TRAILING PARTIAL
# window — the schedule edge the RPS oracle test cannot reach (its C captures
# only cover complete mini-GOPs).
#
# ASSETS: the same public-domain Derf clips as video_selfcheck_gate.sh. The
# published clips are 8 frames; longer cells are built by repeating the clip,
# which is enough — the point is decoder-verified correctness, not content
# novelty (the temporal field, DPB and CDF chain are exercised identically on
# repeated motion).
#
# Usage: tools/ra_selfcheck_gate.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
RUN="$HERE/identity_run"
ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
SIZE="${RASG_SIZE:-256x256}"
PRESET="${RASG_PRESET:-6}"
QP="${RASG_QP:-40}"
W=${SIZE%x*}; H=${SIZE#*x}
FL=$((W * H + 2 * (W / 2) * (H / 2)))

AOMDEC="${AOMDEC:-aomdec}"
if ! command -v "$AOMDEC" >/dev/null 2>&1; then
    echo "ra selfcheck gate: aomdec not on PATH -- this gate needs an" >&2
    echo "  INDEPENDENT decoder and will not fall back to the port's own." >&2
    exit 1
fi
if [ ! -f "$ASSETS/johnny_${SIZE}_8f.i420" ]; then
    echo "== fetching video assets -> $ASSETS"
    "$HERE/fetch_r2_assets.sh" video/pd-derf-720p/ "$ASSETS" || {
        echo "ra selfcheck gate: could not obtain assets" >&2; exit 1; }
fi

# clip:hier:frames — frames = 1 + k*2^hier (complete mini-GOPs), except the
# trailing cell whose second window is deliberately incomplete.
CELLS=(
    "johnny 1 9"
    "johnny 2 9"
    "johnny 3 17"
    "johnny 4 33"
    "johnny 5 33"
    "vidyo3 2 9"
    "vidyo3 3 17"
    "vidyo3 4 33"
    "vidyo3 5 33"
    "johnny 3 12"   # trailing partial mini-GOP
    "vidyo3 4 20"   # ditto, deeper hierarchy
)
EXPECT=${#CELLS[@]}

work="${TMPDIR:-$HOME/tmp}/ra-selfcheck.$$"
mkdir -p "$work"
trap 'rm -rf "$work"' EXIT

fail=0; ran=0; hidden_seen=0
echo "== ra selfcheck gate (port recon vs aomdec, ${SIZE}, p${PRESET} q${QP}) =="
printf '  %-14s %-4s %-7s %-9s %-9s %s\n' clip hier frames displays recons note
for cell in "${CELLS[@]}"; do
    read -r clip hier n <<<"$cell"
    out="$work/${clip}_h${hier}_n${n}"; mkdir -p "$out"
    asset="$ASSETS/${clip}_${SIZE}_8f.i420"
    if [ ! -f "$asset" ]; then
        echo "  MISSING ASSET $asset" >&2; fail=$((fail + 1)); continue
    fi
    # Repeat the 8-frame clip to the requested length.
    src="$out/src.i420"
    if ! python3 - "$asset" "$src" "$n" "$FL" <<'PY'
import sys
asset, dst, n, fl = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
data = open(asset, 'rb').read()
frames = data[: fl * (len(data) // fl)]
one = len(frames) // fl
open(dst, 'wb').write(b''.join(frames[(i % one) * fl:(i % one + 1) * fl] for i in range(n)))
PY
    then
        echo "  could not build looped source" >&2; exit 1
    fi
    if ! env -u SVTAV1_FRAME_SHIFT -u SVTAV1_FRAME_ZOOM_NUM -u SVTAV1_FRAME_ZOOM_DEN \
        SVTAV1_FRAMES="$n" SVTAV1_INTRA_PERIOD=64 SVT_PRED_STRUCT=2 \
        SVTAV1_HIER_LEVELS="$hier" \
        SVTAV1_FINAL_RECON="$out/rec" \
        "$RUN" "rawseq:$src" "$W" "$H" "$QP" "$PRESET" "$out/p" \
        >"$out/stdout.txt" 2>"$out/trace.txt"; then
        printf '  %-14s %-4s %-7s %-9s %-9s %s\n' "$clip" "$hier" "$n" - - "ENCODE REFUSED/FAILED"
        fail=$((fail + 1)); continue
    fi
    ran=$((ran + 1))
    if ! "$AOMDEC" --rawvideo -o "$out/dec.yuv" "$out/p.obu" >/dev/null 2>&1; then
        printf '  %-14s %-4s %-7s %-9s %-9s %s\n' "$clip" "$hier" "$n" - - "UNDECODABLE"
        fail=$((fail + 1)); continue
    fi
    got=$(OUT="$out" N="$n" FL="$FL" python3 - <<'PY'
import os
out, n, fl = os.environ['OUT'], int(os.environ['N']), int(os.environ['FL'])
d = open(os.path.join(out, 'dec.yuv'), 'rb').read()
ndisp = len(d) // fl
ok, first, hidden = 0, None, 0
# A show_existing frame is an OBU with no tile data; its presence is what
# makes the hidden-layer machinery real rather than an LDP encode in a
# trenchcoat. Count them from the stream's OBU types.
data = open(os.path.join(out, 'p.obu'), 'rb').read()
pos = 0
while pos < len(data):
    # OBU header: forbidden(1) type(4) ext(1) has_size(1) reserved(1)
    hdr = data[pos]; pos += 1
    obu_type = (hdr >> 3) & 0xF
    has_ext = (hdr >> 2) & 1
    has_size = (hdr >> 1) & 1
    if has_ext: pos += 1
    if has_size:
        shift = 0; size = 0
        while True:
            b = data[pos]; pos += 1
            size |= (b & 0x7F) << shift
            if not (b & 0x80): break
            shift += 7
        pos += size
    # frame OBUs with show_existing are parsed by the oracle; cheaper proxy:
    # counted separately below via display count vs coded count.
print(f"{ndisp}", end=" ")
for i in range(n):
    p = os.path.join(out, f'rec.f{i}')
    if not os.path.exists(p):
        break
    if open(p, 'rb').read()[:fl] == d[i * fl:(i + 1) * fl]:
        ok += 1
    elif first is None:
        first = i
print(f"{ok}/{n}" + (f" drift-from-f{first}" if first is not None else ""))
PY
)
    ndisp=${got%% *}; recs=${got##* }
    note=ok
    if [ "$ndisp" != "$n" ]; then note="DISPLAY COUNT $ndisp != $n"; fail=$((fail + 1));
    elif [ "$recs" != "$n/$n" ]; then note="RECON MISMATCH"; fail=$((fail + 1)); fi
    # Anti-vacuity for the hierarchical machinery: hier >= 1 cells must emit
    # more coded frames than displays would suggest at LDP — i.e. the stream
    # must contain hidden (show_frame=0) pictures. The oracle reports them.
    if [ "$hier" -ge 1 ]; then
        nhid=$(python3 "$HERE/ra_rps_oracle.py" "$out/p.obu" 2>/dev/null | \
            python3 -c "import sys, json; print(sum(1 for l in sys.stdin if json.loads(l).get('show_frame') == 0))" 2>/dev/null || echo 0)
        if [ "$nhid" -gt 0 ]; then hidden_seen=$((hidden_seen + 1)); fi
    fi
    printf '  %-14s %-4s %-7s %-9s %-9s %s\n' "$clip" "$hier" "$n" "$ndisp" "$recs" "$note"
done

echo
echo "cells run: $ran of $EXPECT   hidden-frame cells: $hidden_seen   failed: $fail"
if [ "$ran" -ne "$EXPECT" ]; then
    echo "ANTI-VACUITY FAIL: $ran of $EXPECT cells actually ran" >&2; exit 1
fi
# Every hier>=1 cell must have produced hidden pictures; count them.
nhiercells=$(printf '%s\n' "${CELLS[@]}" | awk '$2 >= 1' | wc -l)
if [ "$hidden_seen" -ne "$nhiercells" ]; then
    echo "ANTI-VACUITY FAIL: $hidden_seen of $nhiercells hierarchical cells emitted hidden frames" >&2; exit 1
fi
[ "$fail" -eq 0 ] || exit 1
echo "ra selfcheck gate: OK"

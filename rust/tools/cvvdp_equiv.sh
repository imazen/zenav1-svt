#!/usr/bin/env bash
# PERCEPTUAL RD-EQUIVALENCE diagnostic for a byte-diverging inter cell.
#
# WHAT THIS MEASURES — and what it does not. The byte census
# (`tools/video_census_gate.sh`) is a strict ratchet: any divergence from C's
# stream fails the cell. That is the right parity contract, but it cannot
# distinguish "the port picked a different-but-equivalent winner" from "the
# port lost rate-distortion". This script encodes the same sequence with both
# encoders, decodes each with `aomdec`, and scores every frame against the
# uncompressed source with cvvdp (the CPU port in zenmetrics). A diverging
# cell whose JOD deltas are within noise at matched bytes is benign MD
# divergence; a cell where the port's JOD is materially LOWER is a real
# quality regression worth fixing before cosmetic byte-chasing.
#
# LIMITS. cvvdp is a still-image metric applied per frame — no temporal
# pooling, so it sees inter distortion only as it survives into each decoded
# frame. The YUV->sRGB conversion is fixed (BT.601 limited, nearest chroma)
# for BOTH sides, so absolute JODs are not meaningful — only the port-minus-C
# delta per frame is. Use it as triage evidence, never as a proof of parity.
#
# Usage: tools/cvvdp_equiv.sh <clip|file.i420> <size> <qp> <preset> [frames]
#   clip:  a name resolvable under $ZENAV1_VIDEO_ASSETS as
#          `<clip>_<size>_8f.i420`, or a literal .i420 path.
#   size:  WxH — e.g. 128 or 128x128.
# Env:   ZENMETRICS — zenmetrics binary (default: PATH, then
#          $HOME/work/zen/zenmetrics/target/release/zenmetrics).
#        AOMDEC — decoder (default: aomdec).
#        CVVDP_KEEP — set to keep the work directory instead of deleting it.
#
# Output: per-frame (bytes_C, bytes_port, JOD_C, JOD_port, dJOD) table plus a
# verdict line. Exit 0 always — this is a measurement, not a gate.
set -euo pipefail

if [[ $# -lt 4 ]]; then
    echo "usage: $0 <clip|file.i420> <size> <qp> <preset> [frames]" >&2
    exit 2
fi
CLIP=$1; SIZE=$2; QP=$3; PRESET=$4; FRAMES="${5:-8}"

HERE=$(cd "$(dirname "$0")" && pwd)
RS_ROOT=$(cd "$HERE/.." && pwd)

# Resolve the input.
if [[ $SIZE == *x* ]]; then
    W=${SIZE%x*}; H=${SIZE#*x}
else
    W=$SIZE; H=$SIZE
fi
if [[ -f $CLIP ]]; then
    YUV=$CLIP
else
    ASSETS="${ZENAV1_VIDEO_ASSETS:-${ZENAV1_CORPUS_ROOT:-$HOME/work/zen}/video/pd-derf-720p}"
    YUV="$ASSETS/${CLIP}_${W}x${H}_8f.i420"
fi
[[ -f $YUV ]] || { echo "cvvdp_equiv: no input: $YUV" >&2; exit 2; }

AOMDEC="${AOMDEC:-aomdec}"
ZM="${ZENMETRICS:-}"
if [[ -z $ZM ]]; then
    if command -v zenmetrics >/dev/null 2>&1; then ZM=zenmetrics;
    elif [[ -x $HOME/work/zen/zenmetrics/target/release/zenmetrics ]]; then
        ZM="$HOME/work/zen/zenmetrics/target/release/zenmetrics"
    fi
fi
[[ -n $ZM && -x $(command -v "$ZM" 2>/dev/null || echo "$ZM") ]] || {
    echo "cvvdp_equiv: no zenmetrics binary — build zenmetrics-cli or set ZENMETRICS" >&2; exit 2; }

WORK="${TMPDIR:-$HOME/tmp}/cvvdp-equiv-$$"
mkdir -p "$WORK/frames"
trap '[[ -z ${CVVDP_KEEP:-} ]] && rm -rf "$WORK" || true' EXIT

# 1. Encode both sides (same bytes on the wire is NOT asserted here).
SVTAV1_FRAMES=$FRAMES SVTAV1_INTRA_PERIOD=64 SVTAV1_HIER_LEVELS=0 \
    CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} \
    "$HERE/identity_run" "rawseq:$YUV" "$W" "$H" "$QP" "$PRESET" "$WORK/rs" \
    2>"$WORK/rs.trace"
SVT_FRAMES=$FRAMES SVT_INTRA_PERIOD=-1 SVT_HIER_LEVELS=0 SVT_PRED_STRUCT=1 \
    "$HERE/capture_c_trace/capture_c_trace" "$W" "$H" "$QP" "$PRESET" \
    "$WORK/rs.yuv" "$WORK/c.obu" 8 2>"$WORK/c.trace"

# 2. Decode both streams.
"$AOMDEC" --output="$WORK/c.dec.y4m" "$WORK/c.obu"  >/dev/null 2>&1
"$AOMDEC" --output="$WORK/rs.dec.y4m" "$WORK/rs.obu" >/dev/null 2>&1

# 3. Convert every frame to sRGB PNG with the repo's fixed conversion
#    (tools/i444_png.py coefficients — BT.601 limited, nearest chroma).
python3 - "$WORK" "$W" "$H" <<'PY'
import struct, zlib, sys
work, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
def clip8(x): return max(0, min(255, x))
def write_png(path, rgb):
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))
    raw = b"".join(b"\x00" + bytes(rgb[r*w*3:(r+1)*w*3]) for r in range(h))
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    open(path, "wb").write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
                           + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
def i420_to_rgb(data, off):
    cw = w//2
    y = data[off:off+w*h]
    u = data[off+w*h:off+w*h+cw*(h//2)]
    v = data[off+w*h+cw*(h//2):off+w*h*3//2]
    rgb = bytearray(w*h*3)
    for j in range(h):
        uj = u[(j//2)*cw:(j//2)*cw+cw]; vj = v[(j//2)*cw:(j//2)*cw+cw]
        for i in range(w):
            yy = y[j*w+i]-16; uu = uj[i//2]-128; vv = vj[i//2]-128
            p = (j*w+i)*3
            rgb[p]   = clip8((298*yy + 409*vv + 128) >> 8)
            rgb[p+1] = clip8((298*yy - 100*uu - 208*vv + 128) >> 8)
            rgb[p+2] = clip8((298*yy + 516*uu + 128) >> 8)
    return rgb
def y4m_frames(path):
    data = open(path, "rb").read()
    pos = data.find(b"\n") + 1
    offs = []
    while pos < len(data):
        assert data[pos:pos+6] == b"FRAME\n", data[pos:pos+10]
        pos += 6; offs.append(pos); pos += w*h*3//2
    return data, offs
src = open(f"{work}/rs.yuv", "rb").read()
n = len(src)//(w*h*3//2)
for f in range(n):
    write_png(f"{work}/frames/src_f{f}.png", i420_to_rgb(src, f*w*h*3//2))
for name in ("c.dec", "rs.dec"):
    data, offs = y4m_frames(f"{work}/{name}.y4m")
    for f, off in enumerate(offs):
        write_png(f"{work}/frames/{name}_f{f}.png", i420_to_rgb(data, off))
PY

# 4. Organise frames into per-clip dirs (sorted names) — needed by
#    zenmetrics' `score-video` (the real temporal-cvvdp path) as well as
#    the per-frame stills loop below. Source frames first.
python3 - "$WORK" <<'PY'
import os, sys
work = sys.argv[1]
for d in ("src", "c.dec", "rs.dec"):
    os.makedirs(f"{work}/vdir/{d}", exist_ok=True)
    for f in sorted(os.listdir(f"{work}/frames")):
        if f.startswith(d.rstrip('.').replace('.', '') + "_f") or \
           (d == "src" and f.startswith("src_f")) or \
           (d == "c.dec" and f.startswith("c.dec_f")) or \
           (d == "rs.dec" and f.startswith("rs.dec_f")):
            n = f.split("_f")[1].split(".")[0]
            dst = f"{work}/vdir/{d}/f{int(n):04d}.png"
            if not os.path.exists(dst):
                os.link(f"{work}/frames/{f}", dst)
PY

# 5. Headline: whole-clip cvvdp VIDEO score per side — the real temporal
#    path (sustained + transient channels over the causal FIR window),
#    pycvvdp v0.5.7 parity. Requires a zenmetrics build ≥ the
#    `score-video` subcommand; absent it, the per-frame stills table
#    below still runs (that's the pre-video fallback).
DISPLAY="${CVVDP_DISPLAY:-standard_fhd}"
if "$ZM" score-video --help >/dev/null 2>&1 && "$ZM" score-video --help 2>&1 | grep -q "display-model"; then
    c_jod=$("$ZM" score-video --reference-dir "$WORK/vdir/src" \
            --distorted-dir "$WORK/vdir/c.dec" --fps 30 --display-model "$DISPLAY" \
            2>/dev/null | sed -n 's/.*jod=\([0-9.]*\).*/\1/p')
    rs_jod=$("$ZM" score-video --reference-dir "$WORK/vdir/src" \
             --distorted-dir "$WORK/vdir/rs.dec" --fps 30 --display-model "$DISPLAY" \
             2>/dev/null | sed -n 's/.*jod=\([0-9.]*\).*/\1/p')
    if [[ -n $c_jod && -n $rs_jod ]]; then
        printf "cvvdp-video (%s, %sfps): JOD_C=%s JOD_port=%s dJOD=%s (clip-level, temporal)\n" \
            "$DISPLAY" 30 "$c_jod" "$rs_jod" "$(echo "$rs_jod - $c_jod" | bc)"
    fi
else
    echo "note: this zenmetrics lacks score-video — per-frame stills only" >&2
fi

# 6. Per-frame cvvdp vs source; print the table.
python3 - "$WORK" "$ZM" "$DISPLAY" "$WORK/c.obu" "$WORK/rs.obu" <<'PY'
import re, struct, subprocess, sys
work, zm, display, c_obu, rs_obu = sys.argv[1:6]
def obu_frame_sizes(path):
    data = open(path, "rb").read()
    out, i, cur = [], 0, 0
    while i < len(data):
        hdr = data[i]; ot = (hdr >> 3) & 0xF; j = i + 1 + ((hdr >> 2) & 1)
        if (hdr >> 1) & 1:
            v = 0; sh = 0
            while True:
                b = data[j]; j += 1; v |= (b & 0x7f) << sh; sh += 7
                if not (b & 0x80): break
            size = v
        else:
            size = len(data) - j
        if ot == 2 and cur:  # temporal delimiter starts a new TU
            out.append(cur); cur = 0
        cur += j - i + size
        i = j + size
    if cur: out.append(cur)
    return out
c_bytes = obu_frame_sizes(c_obu); rs_bytes = obu_frame_sizes(rs_obu)
n = min(len(c_bytes), len(rs_bytes))
def jod(ref, dis):
    # Newer zenmetrics builds require --display-model; older ones did not
    # accept it. Probe once, then reuse.
    global _need_display
    args = [zm, "score", "--metric", "cvvdp", "--reference", ref, "--distorted", dis]
    for extra in (["--display-model", display], []) if _need_display is None else (
            [["--display-model", display]] if _need_display else [[]]):
        out = subprocess.run(args[:4] + extra + args[4:],
                             capture_output=True, text=True)
        m = re.search(r"cvvdp[^=]*=([0-9.]+)", out.stdout)
        if m:
            if _need_display is None: _need_display = bool(extra)
            return float(m.group(1))
    return float("nan")
_need_display = None
print(f"frame  bytes_C  bytes_port  JOD_C     JOD_port  dJOD")
tot_d = 0.0
for f in range(n):
    jc = jod(f"{work}/frames/src_f{f}.png", f"{work}/frames/c.dec_f{f}.png")
    jr = jod(f"{work}/frames/src_f{f}.png", f"{work}/frames/rs.dec_f{f}.png")
    tot_d += jr - jc
    print(f"f{f}     {c_bytes[f]:>7} {rs_bytes[f]:>11} {jc:>9.6f} {jr:>10.6f} {jr-jc:+.4f}")
print(f"totals bytes: C={sum(c_bytes[:n])} port={sum(rs_bytes[:n])} "
      f"mean dJOD={tot_d/n:+.4f} (port-C, positive = port better)")
PY

echo "work dir: $WORK (set CVVDP_KEEP=1 to keep frames/traces)"

#!/usr/bin/env python3
"""Cut gate-sized multi-frame I420 sequences out of a 4:2:0 8-bit y4m.

WHY THIS EXISTS. Every inter/video gate in this repo encodes SYNTHETIC content
-- `inter_byte_gate.sh`'s 96 cells are {uniform,gradient,diag,screen}, and
`video_key_matrix.sh` defaults to `gradient diag screen screenrep uniform`.
Real motion is a different regime: `identity_run`'s multi-frame path warps
frame 0 by a global integer translation, which open-loop ME finds exactly, so
the residual SAD floors to zero (measured: `avg_me_sad=0`, `is_gm_on=0` on all
of {gradient,diag,screen} x {64,128,256,512}; see INTER-ENCODE-PLAN.md). No
synthetic cell can present a motion field that C's search has to work for.

CROP SELECTION IS THE WHOLE POINT. A 128x128 window of a 720p conference clip
is very often a static wall: flat, and identical in every frame. Encoding that
exercises nothing, and it fails SILENTLY -- the cell passes, byte-identically,
while testing neither intra texture nor inter search. This repo has already
shipped that exact bug once: 26 screen-content cells were byte-identical to C
and structurally unable to reach the palette/IntraBC code they existed to
guard, because `crop:` took the centre of `gmessages.png` and the centre was
262144 pixels of one colour (issue #23, see `lib_corpus.sh::screen_crop_spec`).

So this tool does not crop the centre. It scores a grid of candidate origins on
two independent axes and takes the best:

  texture -- mean |luma - blurred luma| over frame 0, a flatness detector. A
             wall scores ~0 and cannot exercise intra prediction.
  motion  -- the MINIMUM over consecutive pairs of mean |luma(f)-luma(f-1)|
             at the SAME window. A tripod-mounted background scores ~0 and
             cannot exercise ME -- and so does one repeated frame, which is why
             this is a minimum rather than a mean (see `motion()`).

Both are recorded in the sidecar and both are asserted non-trivial, so a future
re-aim onto flat or static content fails loudly here rather than silently in a
gate months later.

Output per (clip, size): `<name>_<W>x<H>_<N>f.i420`, N frames of planar I420
concatenated -- exactly what `identity_run`'s `rawseq:` content mode reads --
plus a `.json` sidecar carrying the crop origin, both scores, the source URL
and the license.

Usage:
    mk_video_assets.py <in.y4m> <out-dir> --name NAME --sizes 128x128,256x256
                       --frames 16 --source-url URL --license "public domain"
"""
import argparse, json, os, sys

def parse_y4m(path):
    """(header dict, [frame bytes]) for an 8-bit 4:2:0 y4m."""
    with open(path, "rb") as f:
        data = f.read()
    nl = data.index(b"\n")
    hdr = data[:nl].decode("ascii")
    if not hdr.startswith("YUV4MPEG2"):
        sys.exit(f"{path}: not a y4m")
    w = h = None
    for tok in hdr.split()[1:]:
        if tok[0] == "W": w = int(tok[1:])
        elif tok[0] == "H": h = int(tok[1:])
        elif tok[0] == "C" and not tok.startswith(("C420jpeg", "C420mpeg2", "C420paldv", "C420")):
            sys.exit(f"{path}: chroma {tok} is not 4:2:0 8-bit")
    if w is None or h is None:
        sys.exit(f"{path}: header names no W/H: {hdr!r}")
    fsz = w * h * 3 // 2
    frames, pos = [], nl + 1
    while pos < len(data):
        e = data.find(b"\n", pos)
        if e < 0 or not data[pos:pos + 5] == b"FRAME":
            break
        pos = e + 1
        if pos + fsz > len(data):
            break                      # truncated tail of a ranged fetch
        frames.append(data[pos:pos + fsz])
        pos += fsz
    return {"w": w, "h": h}, frames

def luma_crop(frame, w, h, ox, oy, cw, ch):
    return [frame[(oy + r) * w + ox : (oy + r) * w + ox + cw] for r in range(ch)]

# The two scores are pure Python over raw bytes (no numpy on the CI image or
# this box), so an exhaustive full-resolution search over every candidate
# origin is minutes per clip. Both take a `stride`: the SEARCH runs subsampled
# to rank origins cheaply, and the winner is then re-scored at stride 1 so the
# number recorded in the sidecar -- and checked against the anti-vacuity floor
# -- is the exact one, never the estimate.
def texture(rows, cw, ch, stride=1):
    """Mean |px - mean(3x3 neighbourhood)|; a flat window scores 0."""
    tot = n = 0
    for r in range(1, ch - 1, stride):
        a, b, c = rows[r - 1], rows[r], rows[r + 1]
        for x in range(1, cw - 1, stride):
            s = (a[x-1]+a[x]+a[x+1] + b[x-1]+b[x]+b[x+1] + c[x-1]+c[x]+c[x+1])
            tot += abs(b[x] * 9 - s); n += 9
    return tot / n if n else 0.0

def motion(frames, w, h, ox, oy, cw, ch, stride=1, fstride=1):
    """MINIMUM over consecutive pairs of mean |luma(f) - luma(f-1)|.

    The minimum, not the mean, and that is the whole point. MEASURED
    2026-09-10: vidyo3's first cell scored mean motion 20.4 and looked ideal,
    but `|f1-f0|` was EXACTLY 0.00 -- the clip is 30 fps stored at 60, so every
    frame is doubled. C and the port both coded that frame to an identical 24
    bytes, and the cell would have shipped as "byte-identical inter parity on
    real video" while asserting only that two encoders agree a repeated frame
    is a skip. A mean over the sequence hides a zero pair; a minimum cannot.
    """
    worst = None
    prev = None
    for fr in frames[::fstride]:
        cur = luma_crop(fr, w, h, ox, oy, cw, ch)
        if prev is not None:
            tot = n = 0
            for r in range(0, ch, stride):
                pr, cr = prev[r], cur[r]
                for x in range(0, cw, stride):
                    tot += abs(cr[x] - pr[x])
                    n += 1
            d = tot / n if n else 0.0
            worst = d if worst is None else min(worst, d)
        prev = cur
    return worst if worst is not None else 0.0

def dedupe_stride(frames, w, h):
    """Smallest k such that no two frames k apart are byte-identical.

    Conference clips in the derf collection are commonly 30 fps material
    published at 60, i.e. every frame doubled. Encoding the doubled sequence
    makes every second frame a trivial skip.
    """
    for k in (1, 2, 3, 4):
        if all(frames[i] != frames[i - k] for i in range(k, len(frames), 1)):
            return k
    return 0

def crop_i420(frame, w, h, ox, oy, cw, ch):
    out = bytearray()
    for r in range(ch):
        o = (oy + r) * w + ox
        out += frame[o : o + cw]
    ysz, cw2, ch2 = w * h, w // 2, h // 2
    ox2, oy2, cwc, chc = ox // 2, oy // 2, cw // 2, ch // 2
    for pl in range(2):
        base = ysz + pl * (cw2 * ch2)
        for r in range(chc):
            o = base + (oy2 + r) * cw2 + ox2
            out += frame[o : o + cwc]
    return bytes(out)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("y4m"); ap.add_argument("outdir")
    ap.add_argument("--name", required=True)
    ap.add_argument("--sizes", default="128x128,256x256")
    ap.add_argument("--frames", type=int, default=16)
    ap.add_argument("--source-url", required=True)
    ap.add_argument("--license", required=True)
    ap.add_argument("--min-texture", type=float, default=2.0)
    ap.add_argument("--min-motion", type=float, default=0.5)
    ap.add_argument("--decimate", type=int, default=0,
                    help="keep every Nth frame; 0 = detect duplicated frames")
    a = ap.parse_args()

    meta, frames = parse_y4m(a.y4m)
    w, h = meta["w"], meta["h"]
    dec = a.decimate or dedupe_stride(frames, w, h)
    if dec == 0:
        sys.exit(f"ANTI-VACUITY FAIL: {a.name} still has byte-identical frames "
                 f"4 apart -- no decimation makes this clip move")
    frames = frames[::dec]
    if len(frames) < a.frames:
        sys.exit(f"{a.y4m}: {len(frames)} whole frames after decimate={dec}, need {a.frames}")
    frames = frames[: a.frames]
    for i in range(1, len(frames)):
        if frames[i] == frames[i - 1]:
            sys.exit(f"ANTI-VACUITY FAIL: {a.name} frames {i-1}/{i} are byte-identical "
                     f"after decimate={dec}")
    print(f"{a.name}: decimate={dec} ({len(frames)} frames kept)")
    os.makedirs(a.outdir, exist_ok=True)

    for spec in a.sizes.split(","):
        cw, ch = (int(v) for v in spec.split("x"))
        # Even origins only: the chroma crop must land on a chroma sample.
        step = 64
        # Rank on the WEAKER of the two normalised axes, so a window that is
        # busy but static (a poster on a wall) or moving but flat (a shadow
        # crossing paint) cannot win: a gate cell needs BOTH.
        rank = lambda t, m: min(t / a.min_texture, m / a.min_motion)
        ranked = []
        for oy in range(0, h - ch + 1, step):
            for ox in range(0, w - cw + 1, step):
                m = motion(frames, w, h, ox, oy, cw, ch, stride=4, fstride=4)
                t = texture(luma_crop(frames[0], w, h, ox, oy, cw, ch), cw, ch, stride=4)
                ranked.append((rank(t, m), ox, oy))
        ranked.sort(reverse=True)
        best = None
        for _, ox, oy in ranked[:6]:      # re-score the shortlist exactly
            m = motion(frames, w, h, ox, oy, cw, ch)
            t = texture(luma_crop(frames[0], w, h, ox, oy, cw, ch), cw, ch)
            if best is None or rank(t, m) > best[0]:
                best = (rank(t, m), ox, oy, t, m)
        score, ox, oy, t, m = best
        if t < a.min_texture or m < a.min_motion:
            sys.exit(f"ANTI-VACUITY FAIL: best {cw}x{ch} window of {a.name} is "
                     f"texture={t:.2f} (min {a.min_texture}) motion={m:.2f} "
                     f"(min {a.min_motion}) -- this clip cannot feed a gate")
        stem = f"{a.name}_{cw}x{ch}_{a.frames}f"
        raw = os.path.join(a.outdir, stem + ".i420")
        with open(raw, "wb") as f:
            for fr in frames:
                f.write(crop_i420(fr, w, h, ox, oy, cw, ch))
        with open(os.path.join(a.outdir, stem + ".json"), "w") as f:
            json.dump({
                "name": a.name, "width": cw, "height": ch, "frames": a.frames,
                "format": "i420-8bit-planar-concatenated",
                "crop_origin": [ox, oy], "source_dimensions": [w, h],
                "decimate": dec,
                "texture": round(t, 4), "min_pair_motion": round(m, 4),
                "source_url": a.source_url, "license": a.license,
                "tool": "rust/tools/mk_video_assets.py",
            }, f, indent=2, sort_keys=True)
            f.write("\n")
        print(f"{stem}: origin=({ox},{oy}) texture={t:.2f} min-pair-motion={m:.2f} "
              f"{os.path.getsize(raw)} bytes")

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Edge-replicate an I420 .yuv from (w,h) to (aw,ah) — C `pad_input_picture`.

WHY: the CONTROLLED A/B for a "true != aligned" divergence. Encoding the same
PNG at 188x256 and at 192x256 with `crop:` is NOT a controlled comparison — the
centre-crop takes a different source window, so two things changed at once.
This produces the frame the 188-wide encode ACTUALLY sees after its internal
TRUE->ALIGNED padding, so it can be fed back as a genuinely 192-wide frame:
identical pixels, `true == aligned`. Whatever still differs is not alignment.

Chroma is CEILING ((w+1)//2) on input and (aw//2) on output, matching
identity_run's .yuv layout and the pipeline's aligned chroma.

Usage: pad_yuv.py <in.yuv> <w> <h> <aw> <ah> <out.yuv>
"""

import sys


def pad(plane, w, h, aw, ah):
    out = bytearray(aw * ah)
    for r in range(ah):
        sr = min(r, h - 1)
        row = plane[sr * w : sr * w + w]
        out[r * aw : r * aw + w] = row
        if aw > w:
            out[r * aw + w : (r + 1) * aw] = bytes([row[-1]]) * (aw - w)
    return bytes(out)


def main():
    if len(sys.argv) != 7:
        sys.exit(__doc__)
    src, w, h, aw, ah, dst = (
        sys.argv[1],
        int(sys.argv[2]),
        int(sys.argv[3]),
        int(sys.argv[4]),
        int(sys.argv[5]),
        sys.argv[6],
    )
    assert aw >= w and ah >= h, "padding only grows the frame"
    cw, ch = (w + 1) // 2, (h + 1) // 2
    acw, ach = aw // 2, ah // 2
    data = open(src, "rb").read()
    need = w * h + 2 * cw * ch
    assert len(data) >= need, f"{src}: {len(data)} < {need}"
    y = data[: w * h]
    u = data[w * h : w * h + cw * ch]
    v = data[w * h + cw * ch : need]
    with open(dst, "wb") as f:
        f.write(pad(y, w, h, aw, ah))
        f.write(pad(u, cw, ch, acw, ach))
        f.write(pad(v, cw, ch, acw, ach))
    print(f"{src} {w}x{h} -> {dst} {aw}x{ah} ({aw * ah + 2 * acw * ach} B)")


if __name__ == "__main__":
    main()

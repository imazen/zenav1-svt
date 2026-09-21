#!/usr/bin/env python3
"""I444 <-> PNG utilities + per-plane PSNR, dependency-free (zlib only).

  i444_png.py png <in.yuv> <w> <h> <out.png>
      I444 planar -> 8-bit RGB PNG via BT.601 studio-swing -> full-range RGB
      (the inverse of probe_444_file's forward matrix).

  i444_png.py psnr <a.yuv> <b.yuv> <w> <h>
      Per-plane PSNR of two I444 files: prints "Y U V" in dB
      (inf for identical planes).

  i444_png.py gray <in.y> <w> <h> <out.png>
      Luma-only (BT.601 limited) -> full-range grayscale RGB PNG.

  i444_png.py ypsnr <a.y> <b.y> <w> <h>
      Luma-only PSNR in dB.

  i444_png.py up444 <in.i420> <w> <h> <out.yuv>
      I420 (ceiling chroma) -> I444 by nearest-neighbour 2x chroma upsample —
      lets a decoded 4:2:0 frame be scored against an I444 source.
"""
import sys, math, zlib, struct


def read_i444(path, w, h):
    d = open(path, "rb").read()
    n = w * h
    assert len(d) >= 3 * n, f"{path}: {len(d)} < {3*n}"
    return d[:n], d[n:2*n], d[2*n:3*n]


def clip8(x):
    return 0 if x < 0 else (255 if x > 255 else x)


def write_png(path, w, h, rgb):
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))
    raw = b"".join(b"\x00" + bytes(rgb[r*w*3:(r+1)*w*3]) for r in range(h))
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
           + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))
    open(path, "wb").write(png)


def to_png(yuv_path, w, h, png_path):
    y, u, v = read_i444(yuv_path, w, h)
    rgb = bytearray(w * h * 3)
    for i in range(w * h):
        yy = y[i] - 16
        uu = u[i] - 128
        vv = v[i] - 128
        rgb[3*i]   = clip8((298 * yy + 409 * vv + 128) >> 8)
        rgb[3*i+1] = clip8((298 * yy - 100 * uu - 208 * vv + 128) >> 8)
        rgb[3*i+2] = clip8((298 * yy + 516 * uu + 128) >> 8)
    write_png(png_path, w, h, rgb)


def to_gray(y_path, w, h, png_path):
    y = open(y_path, "rb").read()
    assert len(y) >= w * h, f"{y_path}: {len(y)} < {w*h}"
    rgb = bytearray(w * h * 3)
    for i in range(w * h):
        g = clip8((298 * (y[i] - 16) + 128) >> 8)
        rgb[3*i] = g
        rgb[3*i+1] = g
        rgb[3*i+2] = g
    write_png(png_path, w, h, rgb)


def psnr_plane(a, b):
    n = len(a)
    se = 0
    for i in range(n):
        d = a[i] - b[i]
        se += d * d
    if se == 0:
        return float("inf")
    return 10.0 * math.log10(255.0 * 255.0 * n / se)


def psnr(a_path, b_path, w, h):
    ay, au, av = read_i444(a_path, w, h)
    by, bu, bv = read_i444(b_path, w, h)
    print(f"{psnr_plane(ay, by):.3f} {psnr_plane(au, bu):.3f} {psnr_plane(av, bv):.3f}")


def ypsnr(a_path, b_path, w, h):
    ay = open(a_path, "rb").read()[: w * h]
    by = open(b_path, "rb").read()[: w * h]
    print(f"{psnr_plane(ay, by):.3f}")


def up444(i420_path, w, h, out_path):
    d = open(i420_path, "rb").read()
    n = w * h
    cw, ch = (w + 1) // 2, (h + 1) // 2
    assert len(d) >= n + 2 * cw * ch, f"{i420_path}: {len(d)} < {n + 2*cw*ch}"
    y = d[:n]
    us, vs = d[n:n + cw * ch], d[n + cw * ch:n + 2 * cw * ch]
    u = bytearray(n)
    v = bytearray(n)
    for r in range(h):
        for c in range(w):
            u[r * w + c] = us[(r // 2) * cw + c // 2]
            v[r * w + c] = vs[(r // 2) * cw + c // 2]
    open(out_path, "wb").write(y + bytes(u) + bytes(v))


if __name__ == "__main__":
    if len(sys.argv) == 6 and sys.argv[1] == "png":
        to_png(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5])
    elif len(sys.argv) == 6 and sys.argv[1] == "psnr":
        psnr(sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]))
    elif len(sys.argv) == 6 and sys.argv[1] == "gray":
        to_gray(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5])
    elif len(sys.argv) == 6 and sys.argv[1] == "ypsnr":
        ypsnr(sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]))
    elif len(sys.argv) == 6 and sys.argv[1] == "up444":
        up444(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5])
    else:
        sys.exit(__doc__)

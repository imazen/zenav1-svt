#!/usr/bin/env python3
"""imazen-26 subset tune sweep: size / SSIMULACRA2 / encode-time per arm.

Per cell: identity_run crop:<png> 512x512 qp p6 -> obu (+ input i420 dump),
aomdec -> y4m -> i420, both converted to PNG (BT.601 full-range inverse of
identity_run's rgb_to_i420_bt601) and scored by fast-ssim2-cli image.
"""
import os, re, struct, subprocess, sys, time, zlib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

RS = "/home/lilith/work/zen/zenav1-svt/rust"
RUN = f"{RS}/target/release/examples/identity_run"
AOMDEC = "aomdec"
SSIM2 = "/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli"
SUBSET = "/home/lilith/tmp/tune_subset.tsv"
OUT = Path("/home/lilith/tmp/tune_sweep")
W = H = 512
PRESET = 6
QPS = [20, 32, 44]

ARMS = {
    "st_oct8":  {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "8"},
    "st_oct7_vb4": {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "7",
                    "SVT_FORK_VARIANCE_BOOST_STRENGTH": "4"},
    "st_oct8_vb4": {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "8",
                    "SVT_FORK_VARIANCE_BOOST_STRENGTH": "4"},
    "st_oct7_ac10": {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "7",
                     "SVT_FORK_AC_BIAS": "1.0"},
    "st_oct7_ac20": {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "7",
                     "SVT_FORK_AC_BIAS": "2.0"},
    "st_oct8_ac10": {"SVTAV1_STILL_TUNE": "1", "SVT_FORK_VARIANCE_OCTILE": "8",
                     "SVT_FORK_AC_BIAS": "1.0"},
}

def i420_to_png(yuv: bytes, w: int, h: int, path: str):
    """BT.601 LIMITED-range I420 -> RGB PNG (inverse of identity_run's
    rgb_to_i420_bt601: Y=(66R+129G+25B)/256+16, Cb/Cr 224-scale +128)."""
    ys = w * h
    u = yuv[ys:ys + ys // 4]
    v = yuv[ys + ys // 4:]
    rgb = bytearray(w * h * 3)
    for r in range(h):
        for c in range(w):
            yl = (yuv[r * w + c] - 16) * (255.0 / 219.0)
            uu = (u[(r // 2) * (w // 2) + c // 2] - 128) * (255.0 / 224.0)
            vv = (v[(r // 2) * (w // 2) + c // 2] - 128) * (255.0 / 224.0)
            rr = yl + 1.402 * vv
            gg = yl - 0.344136 * uu - 0.714136 * vv
            bb = yl + 1.772 * uu
            o = (r * w + c) * 3
            rgb[o] = max(0, min(255, round(rr)))
            rgb[o + 1] = max(0, min(255, round(gg)))
            rgb[o + 2] = max(0, min(255, round(bb)))
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    raw = b"".join(b"\x00" + bytes(rgb[r * w * 3:(r + 1) * w * 3]) for r in range(h))
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw, 6))
           + chunk(b"IEND", b""))
    Path(path).write_bytes(png)

def y4m_to_i420(data: bytes):
    end = data.index(b"\n") + 1
    return data[end:end + W * H * 3 // 2]

def run_cell(args):
    cls, png_rel, arm, qp = args
    img = os.path.join("/home/lilith/work/zen", png_rel)
    cell = OUT / "cells" / cls / Path(png_rel).stem
    cell.mkdir(parents=True, exist_ok=True)
    src_png = cell / "src.png"
    src_yuv = cell / "src.yuv"
    tag = f"{arm}_q{qp}"
    obu = cell / f"{tag}.obu"
    dec_yuv = cell / f"{tag}.dec.yuv"
    dec_png = cell / f"{tag}.dec.png"

    env = dict(os.environ, **ARMS[arm])
    t0 = time.monotonic()
    r = subprocess.run([RUN, f"crop:{img}", str(W), str(H), str(qp), str(PRESET),
                        str(cell / tag)],
                       env=env, capture_output=True)
    ms = (time.monotonic() - t0) * 1000
    if r.returncode != 0 or not obu.exists():
        return (cls, png_rel, arm, qp, -1, ms, "ENC-ERR", r.stderr.decode()[-200:])
    if not src_yuv.exists():
        # identity_run writes <prefix>.yuv = the exact I420 fed to the encoder
        os.replace(str(cell / f"{tag}.yuv"), src_yuv)
    else:
        os.remove(str(cell / f"{tag}.yuv"))
    if not src_png.exists():
        i420_to_png(src_yuv.read_bytes(), W, H, str(src_png))

    d = subprocess.run([AOMDEC, "-o", str(cell / f"{tag}.y4m"), str(obu)],
                       capture_output=True)
    if d.returncode != 0:
        return (cls, png_rel, arm, qp, obu.stat().st_size, ms, "DEC-ERR", "")
    dec = y4m_to_i420((cell / f"{tag}.y4m").read_bytes())
    dec_yuv.write_bytes(dec)
    os.remove(cell / f"{tag}.y4m")
    i420_to_png(dec, W, H, str(dec_png))
    s = subprocess.run([SSIM2, "image", str(src_png), str(dec_png)], capture_output=True)
    m = re.search(rb"Score:\s*([0-9.\-]+)", s.stdout)
    score = float(m.group(1)) if m else -1
    dec_png.unlink()
    return (cls, png_rel, arm, qp, obu.stat().st_size, ms, f"{score:.3f}", "")

def main():
    rows = [l.split("\t") for l in open(SUBSET).read().splitlines()]
    jobs = [(cls, p, arm, qp) for cls, p in rows for arm in ARMS for qp in QPS]
    print(f"{len(jobs)} cells", file=sys.stderr)
    out = open(OUT / "results_r2.tsv", "w")
    out.write("class\timage\tarm\tqp\tbytes\tenc_ms\tssim2\tnote\n")
    done = 0
    with ThreadPoolExecutor(12) as ex:
        for res in ex.map(run_cell, jobs):
            out.write("\t".join(map(str, res)) + "\n")
            out.flush()
            done += 1
            if done % 50 == 0:
                print(f"{done}/{len(jobs)}", file=sys.stderr)

if __name__ == "__main__":
    main()

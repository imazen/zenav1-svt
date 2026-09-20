#!/usr/bin/env python3
"""imazen-26 tune sweep by PRESET ZONE: bytes / SSIMULACRA2 / wall+user time.

Per cell: encode with identity_run at (preset, arm env, qp), time it with
monotonic wall + RUSAGE_CHILDREN deltas (user+sys CPU), aomdec-decode,
score decoded vs the encoder's exact I420 input via fast-ssim2-cli.

Cell/tag layout identical to sweep_tune.py so the src.yuv/src.png caches
under tune_sweep/cells/ are shared.
"""
import os
import re
import resource
import subprocess
import sys
import time
import zlib
import struct
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SUBSET = "/home/lilith/tmp/tune_subset.tsv"
OUT = Path("/home/lilith/tmp/tune_sweep")
RUN = "/home/lilith/work/zen/zenav1-svt/rust/target/release/examples/identity_run"
AOMDEC = "/home/lilith/bin/aomdec" if Path("/home/lilith/bin/aomdec").exists() else "aomdec"
SSIM2 = "/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli"
W = H = 512
QPS = [20, 32, 44]
PRESETS = [-1, 2, 4, 6, 8, 10, 12]

ARMS = {
    "t1": {},
    "t2": {"SVTAV1_TUNE": "2"},
    "t3": {"SVTAV1_TUNE": "3"},
    "t4": {"SVTAV1_TUNE": "4"},
    "st": {"SVTAV1_STILL_TUNE": "1"},
}


def i420_to_png(yuv: bytes, w: int, h: int, path: str):
    ys = w * h
    u = yuv[ys:ys + ys // 4]
    v = yuv[ys + ys // 4:]
    rgb = bytearray(w * h * 3)
    for r in range(h):
        for c in range(w):
            yl = (yuv[r * w + c] - 16) * (255.0 / 219.0)
            uu = (u[(r // 2) * (w // 2) + c // 2] - 128) * (255.0 / 224.0)
            vv = (v[(r // 2) * (w // 2) + c // 2] - 128) * (255.0 / 224.0)
            i = (r * w + c) * 3
            rgb[i] = max(0, min(255, round(yl + 1.402 * vv)))
            rgb[i + 1] = max(0, min(255, round(yl - 0.344136 * uu - 0.714136 * vv)))
            rgb[i + 2] = max(0, min(255, round(yl + 1.772 * uu)))

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)

    raw = b"".join(b"\x00" + bytes(rgb[r * w * 3:(r + 1) * w * 3]) for r in range(h))
    Path(path).write_bytes(b"\x89PNG\r\n\x1a\n"
                           + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                           + chunk(b"IDAT", zlib.compress(raw, 6))
                           + chunk(b"IEND", b""))


def y4m_to_i420(data: bytes) -> bytes:
    eoh = data.index(b"\n") + 1
    hdr = data[:eoh]
    m = re.search(rb"W(\d+)\s+H(\d+)", hdr)
    fw, fh = int(m.group(1)), int(m.group(2))
    frame_sz = fw * fh * 3 // 2
    pos = eoh
    if data[pos:pos + 5] == b"FRAME":
        pos = data.index(b"\n", pos) + 1
    yuv = data[pos:pos + frame_sz]
    assert len(yuv) == frame_sz, f"y4m frame short: {len(yuv)} < {frame_sz}"
    return yuv


def run_cell(job):
    cls, png_rel, arm, preset, qp = job
    cell = OUT / "cells" / cls / Path(png_rel).stem
    cell.mkdir(parents=True, exist_ok=True)
    img = Path("/home/lilith/work/zen") / png_rel
    src_yuv = cell / "src.yuv"
    src_png = cell / "src.png"
    tag = f"{arm}_p{preset}_q{qp}"
    obu = cell / f"{tag}.obu"
    dec_yuv = cell / f"{tag}.dec.yuv"
    dec_png = cell / f"{tag}.dec.png"

    env = dict(os.environ, **ARMS[arm])
    ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.monotonic()
    r = subprocess.run([RUN, f"crop:{img}", str(W), str(H), str(qp), str(preset),
                        str(cell / tag)],
                       env=env, capture_output=True)
    wall_ms = (time.monotonic() - t0) * 1000
    ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu_ms = (ru1.ru_utime + ru1.ru_stime - ru0.ru_utime - ru0.ru_stime) * 1000
    if r.returncode != 0 or not obu.exists():
        return (cls, png_rel, arm, preset, qp, -1, wall_ms, cpu_ms, "ENC-ERR",
                r.stderr.decode()[-160:])
    if not src_yuv.exists():
        os.replace(str(cell / f"{tag}.yuv"), src_yuv)
    else:
        os.remove(str(cell / f"{tag}.yuv"))
    if not src_png.exists():
        i420_to_png(src_yuv.read_bytes(), W, H, str(src_png))

    d = subprocess.run([AOMDEC, "-o", str(cell / f"{tag}.y4m"), str(obu)],
                       capture_output=True)
    if d.returncode != 0:
        return (cls, png_rel, arm, preset, qp, obu.stat().st_size, wall_ms,
                cpu_ms, "DEC-ERR", "")
    dec = y4m_to_i420((cell / f"{tag}.y4m").read_bytes())
    dec_yuv.write_bytes(dec)
    os.remove(cell / f"{tag}.y4m")
    i420_to_png(dec, W, H, str(dec_png))
    s = subprocess.run([SSIM2, "image", str(src_png), str(dec_png)], capture_output=True)
    m = re.search(rb"Score:\s*([0-9.\-]+)", s.stdout)
    score = float(m.group(1)) if m else -1
    dec_png.unlink()
    return (cls, png_rel, arm, preset, qp, obu.stat().st_size, wall_ms,
            cpu_ms, f"{score:.3f}", "")


def main():
    rows = [l.split("\t") for l in open(SUBSET).read().splitlines()]
    jobs = [(cls, p, arm, preset, qp)
            for cls, p in rows for arm in ARMS for preset in PRESETS for qp in QPS]
    print(f"{len(jobs)} cells", file=sys.stderr)
    out = open(OUT / "results_zones.tsv", "w")
    out.write("class\timage\tarm\tpreset\tqp\tbytes\twall_ms\tcpu_ms\tssim2\tnote\n")
    done = 0
    with ThreadPoolExecutor(12) as ex:
        for res in ex.map(run_cell, jobs):
            out.write("\t".join(map(str, res)) + "\n")
            out.flush()
            done += 1
            if done % 100 == 0:
                print(f"{done}/{len(jobs)}", file=sys.stderr)


if __name__ == "__main__":
    main()

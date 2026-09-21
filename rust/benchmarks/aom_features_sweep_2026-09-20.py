#!/usr/bin/env python3
"""Combination validation: ZenEnhancement::AomAdaptive*/AomDeltaQLf under tune
IQ on the 42-image subset + zenav1-aom reference cells (psnr/iq) for the
vs-libaom comparison. Port cells reuse the sweep_tune layout; aom cells read
the cached src.yuv. Every cell aomdec-decoded, scored ssim2 vs the same
src.png.
"""
import os, re, resource, subprocess, sys, time, zlib, struct
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SUBSET = "/home/lilith/tmp/tune_subset.tsv"
OUT = Path("/home/lilith/tmp/tune_sweep")
RUN = "/home/lilith/work/zen/zenav1-svt/rust/target/release/examples/identity_run"
AOMENC = "/tmp/aomenc/target/release/aomenc"
AOMDEC = "aomdec"
SSIM2 = "/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli"
W = H = 512
QPS = [20, 32, 44]
PORT_PRESETS = [6, 10]
AOM_CPUS = [6, 8]
ARMS = {
    "t1":        {},
    "t3":        {"SVTAV1_TUNE": "3"},
    "iq_adef":   {"SVTAV1_TUNE": "3", "SVTAV1_ADAPTIVE_CDEF": "1"},
    "iq_asharp": {"SVTAV1_TUNE": "3", "SVTAV1_ADAPTIVE_SHARPNESS": "1"},
    "iq_both":   {"SVTAV1_TUNE": "3", "SVTAV1_ADAPTIVE_CDEF": "1",
                  "SVTAV1_ADAPTIVE_SHARPNESS": "1"},
    "adef":      {"SVTAV1_ADAPTIVE_CDEF": "1"},
    "asharp":    {"SVTAV1_ADAPTIVE_SHARPNESS": "1"},
    "iq_dlf":    {"SVTAV1_TUNE": "3", "SVTAV1_DELTA_QLF": "1"},
    "iq_all":    {"SVTAV1_TUNE": "3", "SVTAV1_ADAPTIVE_CDEF": "1",
                  "SVTAV1_ADAPTIVE_SHARPNESS": "1", "SVTAV1_DELTA_QLF": "1"},
}
AOM_TUNES = ["psnr", "iq"]

def i420_to_png(yuv, w, h, path):
    ys = w * h
    u = yuv[ys:ys + ys // 4]; v = yuv[ys + ys // 4:]
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
        + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))

def y4m_to_i420(data):
    eoh = data.index(b"\n") + 1
    hdr = data[:eoh]
    m = re.search(rb"W(\d+)\s+H(\d+)", hdr)
    fw, fh = int(m.group(1)), int(m.group(2))
    frame_sz = fw * fh * 3 // 2
    pos = eoh
    if data[pos:pos + 5] == b"FRAME":
        pos = data.index(b"\n", pos) + 1
    yuv = data[pos:pos + frame_sz]
    assert len(yuv) == frame_sz, f"y4m short {len(yuv)}"
    return yuv

def score(src_png, dec_png):
    s = subprocess.run([SSIM2, "image", str(src_png), str(dec_png)], capture_output=True)
    m = re.search(rb"(\d+\.?\d*)", s.stdout)
    return float(m.group(1)) if m else float("nan")

def run_port(job):
    cls, png_rel, arm, preset, qp = job
    cell = OUT / "cells" / cls / Path(png_rel).stem
    img = Path("/home/lilith/work/zen") / png_rel
    src_yuv = cell / "src.yuv"; src_png = cell / "src.png"
    tag = f"{arm}_p{preset}_q{qp}"
    obu = cell / f"{tag}.obu"
    env = dict(os.environ, **ARMS[arm])
    r = subprocess.run([RUN, f"crop:{img}", str(W), str(H), str(qp), str(preset),
                        str(cell / tag)], env=env, capture_output=True)
    if r.returncode != 0 or not obu.exists():
        return (cls, png_rel, arm, preset, qp, -1, "ENC-ERR", r.stderr.decode()[-120:])
    if not src_yuv.exists():
        os.replace(str(cell / f"{tag}.yuv"), src_yuv)
    else:
        os.remove(str(cell / f"{tag}.yuv"))
    if not src_png.exists():
        i420_to_png(src_yuv.read_bytes(), W, H, str(src_png))
    d = subprocess.run([AOMDEC, "-o", str(cell / f"{tag}.y4m"), str(obu)], capture_output=True)
    if d.returncode != 0:
        return (cls, png_rel, arm, preset, qp, obu.stat().st_size, "DEC-ERR", "")
    dec = y4m_to_i420((cell / f"{tag}.y4m").read_bytes())
    dec_png = cell / f"{tag}.dec.png"
    i420_to_png(dec, W, H, str(dec_png))
    os.remove(cell / f"{tag}.y4m")
    sc = score(src_png, dec_png)
    os.remove(dec_png)
    return (cls, png_rel, arm, preset, qp, obu.stat().st_size, f"{sc:.4f}", "")

def run_aom(job):
    cls, png_rel, tune, cpu, cq = job
    cell = OUT / "cells" / cls / Path(png_rel).stem
    src_yuv = cell / "src.yuv"; src_png = cell / "src.png"
    tag = f"aom_{tune}_c{cpu}_q{cq}"
    obu = cell / f"{tag}.obu"
    r = subprocess.run([AOMENC, str(src_yuv), str(W), str(H), str(cq), str(cpu),
                        tune, str(obu)], capture_output=True)
    if r.returncode != 0 or not obu.exists():
        return (cls, png_rel, tag, cpu, cq, -1, "ENC-ERR", r.stderr.decode()[-120:])
    d = subprocess.run([AOMDEC, "-o", str(cell / f"{tag}.y4m"), str(obu)], capture_output=True)
    if d.returncode != 0:
        return (cls, png_rel, tag, cpu, cq, obu.stat().st_size, "DEC-ERR", "")
    dec = y4m_to_i420((cell / f"{tag}.y4m").read_bytes())
    dec_png = cell / f"{tag}.dec.png"
    i420_to_png(dec, W, H, str(dec_png))
    os.remove(cell / f"{tag}.y4m")
    sc = score(src_png, dec_png)
    os.remove(dec_png)
    return (cls, png_rel, tag, cpu, cq, obu.stat().st_size, f"{sc:.4f}", "")

def main():
    rows = [l.split("\t") for l in Path(SUBSET).read_text().splitlines() if l.strip()]
    jobs_p, jobs_a = [], []
    for cls, png_rel in rows:
        cell = OUT / "cells" / cls / Path(png_rel).stem
        cell.mkdir(parents=True, exist_ok=True)
        for arm in ARMS:
            for p in PORT_PRESETS:
                for qp in QPS:
                    if not (cell / f"{arm}_p{p}_q{qp}.obu").exists() or arm in ("iq_adef","iq_asharp","iq_both","adef","asharp"):
                        jobs_p.append((cls, png_rel, arm, p, qp))
        for tune in AOM_TUNES:
            for cpu in AOM_CPUS:
                for cq in QPS:
                    if not (cell / f"aom_{tune}_c{cpu}_q{cq}.obu").exists():
                        jobs_a.append((cls, png_rel, tune, cpu, cq))
    print(f"{len(jobs_p)} port cells, {len(jobs_a)} aom cells", flush=True)
    out_f = OUT / "aomfeat_results.csv"
    done = set()
    if out_f.exists():
        for l in out_f.read_text().splitlines()[1:]:
            f = l.split(",")
            done.add((f[1], f[2], f[3], f[4]))
    with out_f.open("a") as fh:
        if not done:
            fh.write("cls,png,arm,preset,qp,bytes,ssim2,err\n")
        with ThreadPoolExecutor(16) as ex:
            for res in ex.map(run_port, jobs_p):
                cls, png, arm, p, qp, by, sc, err = res
                fh.write(f"{cls},{png},{arm},{p},{qp},{by},{sc},{err}\n"); fh.flush()
            for res in ex.map(run_aom, jobs_a):
                cls, png, tag, cpu, cq, by, sc, err = res
                fh.write(f"{cls},{png},{tag},{cpu},{cq},{by},{sc},{err}\n"); fh.flush()
    print("done", flush=True)

if __name__ == "__main__":
    main()

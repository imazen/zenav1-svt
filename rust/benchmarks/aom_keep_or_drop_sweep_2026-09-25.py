#!/usr/bin/env python3
"""Keep-or-drop RD measurement for the three unmeasured libaom-derived
ZenEnhancement variants: AomIntraEdgeFilter (ie), AomRestorationUnitSearch
(ru) and AomScreenTools (st). Same cell protocol as
aom_features_sweep_2026-09-20.py: identity_run crop: -> .obu, aomdec
decode, fast-ssim2 vs the inverse-BT.601 PNG; PSNR-Y is taken on the raw
Y planes. Every .obu is persisted by sha256 under STREAMS so cells never
have to be re-encoded.

Arms (env hooks on identity_run; `both` = ie+ru):
  photo subset (42 imgs): p-1 {base,ie,ru,both,st}, p6 {base,st}
  gb82-sc (10 imgs):      p-1 {base,ie,ru,st}, p4 {base,st}, p6 {base,st}
  x tune {t1 default, t3 SVTAV1_TUNE=3} x qp {8,16,24,32,40,46,52,58,63}

Usage:
  aom_keep_or_drop_sweep_2026-09-25.py           # run missing cells
  aom_keep_or_drop_sweep_2026-09-25.py analyze   # BD-rate table from CSV
"""
import hashlib
import os
import re
import struct
import subprocess
import sys
import zlib
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from math import exp, log
from pathlib import Path

HERE = Path(__file__).resolve().parent
ZEN = Path("/home/lilith/work/zen")
SUBSET = HERE / "still_image_tune_v1_subset_2026-09-19.tsv"
SCREEN_DIR = ZEN / "codec-corpus/gb82-sc"
OUT = Path.home() / "tmp" / "aom_kod_cells_2026-09-25"
STREAMS = Path.home() / "tmp" / "aom-keep-or-drop-2026-09-25"
CSV = HERE / "aom_keep_or_drop_2026-09-25.csv"
RUN = HERE.parent / "target/release/examples/identity_run"
AOMDEC = "/usr/bin/aomdec"
GTIME = "/usr/bin/time"
SSIM2 = ZEN / "fast-ssim2/target/release/fast-ssim2-cli"
W = H = 512
QPS = [8, 16, 24, 32, 40, 46, 52, 58, 63]
TUNES = {"t1": {}, "t3": {"SVTAV1_TUNE": "3"}}
ARMS = {
    "base": {},
    "ie": {"SVTAV1_ZEN_INTRA_EDGE_FILTER": "1"},
    "ru": {"SVTAV1_ZEN_RESTORATION_UNIT_SEARCH": "1"},
    "both": {"SVTAV1_ZEN_INTRA_EDGE_FILTER": "1",
             "SVTAV1_ZEN_RESTORATION_UNIT_SEARCH": "1"},
    "st": {"SVTAV1_SCREEN_TOOLS": "1"},
}
PHOTO_GRID = {"-1": ["base", "ie", "ru", "both", "st"], "6": ["base", "st"]}
SCREEN_GRID = {"-1": ["base", "ie", "ru", "st"],
               "4": ["base", "st"], "6": ["base", "st"],
               # Supplemental (beyond the briefed grid): at p<=7 the allintra
               # ladders keep at least one screen tool live, so the
               # AomScreenTools fill (which requires BOTH dead) never fires
               # and cells are byte-identical. p8 is the first preset in its
               # active zone.
               "8": ["base", "st"]}
# Everything that could leak in from the caller's environment and silently
# change an arm. Arms re-add only what they arm.
ENV_STRIP = re.compile(r"^(SVTAV1_|SVT_|SVT$)")


def i420_to_png(yuv, w, h, path):
    ys = w * h
    u = yuv[ys:ys + ((w + 1) // 2) * ((h + 1) // 2)]
    v = yuv[ys + len(u):]
    cw = (w + 1) // 2
    rgb = bytearray(w * h * 3)
    for r in range(h):
        for c in range(w):
            yl = (yuv[r * w + c] - 16) * (255.0 / 219.0)
            uu = (u[(r // 2) * cw + c // 2] - 128) * (255.0 / 224.0)
            vv = (v[(r // 2) * cw + c // 2] - 128) * (255.0 / 224.0)
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
    return fw, fh, yuv


def psnr_y(src_yuv, dec_yuv, w, h):
    ys = w * h
    a, b = src_yuv[:ys], dec_yuv[:ys]
    mse = sum((x - y) * (x - y) for x, y in zip(a, b)) / ys
    if mse == 0:
        return 99.9
    import math
    return 10.0 * math.log10(255.0 * 255.0 / mse)


def score(src_png, dec_png):
    s = subprocess.run([str(SSIM2), "image", str(src_png), str(dec_png)],
                       capture_output=True)
    m = re.search(rb"(\d+\.?\d*)", s.stdout)
    return float(m.group(1)) if m else float("nan")


def persist(obu):
    sha = hashlib.sha256(obu.read_bytes()).hexdigest()
    dst = STREAMS / f"{sha}.obu"
    if not dst.exists():
        try:
            os.link(obu, dst)
        except OSError:
            dst.write_bytes(obu.read_bytes())
    return sha


IMG_CACHE = Path.home() / "tmp" / "aom_kod_images"


def resolve_img(img):
    """imazen-26-png-v3 is mostly git-LFS pointers on this host. The objects
    were fetched (sha256-verified) to IMG_CACHE/<oid>.png; a row whose target
    is still a pointer resolves to the cached object."""
    p = Path(img)
    head = p.open("rb").read(64)
    if head.startswith(b"version https://git-lfs"):
        m = re.search(rb"oid sha256:(\w+)", p.open("rb").read(256))
        cached = IMG_CACHE / f"{m.group(1).decode()}.png"
        if not cached.exists():
            raise FileNotFoundError(f"LFS pointer {img} not in {IMG_CACHE}")
        return cached
    return p


def content_arg(img):
    img = resolve_img(img)
    # Same override as tools/lib_corpus.sh: the centre 512x512 of
    # gb82-sc/gmessages.png is one flat colour, so it is cropped at an
    # explicit content offset instead.
    if Path(img).name == "gmessages.png":
        return f"crop@384,384:{img}"
    return f"crop:{img}"


def run_cell(job):
    cls, png_rel, tune, arm, preset, qp = job
    cell = OUT / "cells" / cls / Path(png_rel).stem
    img = ZEN / png_rel
    src_yuv = cell / "src.yuv"
    src_png = cell / "src.png"
    tag = f"{tune}_{arm}_p{preset}_q{qp}"
    obu = cell / f"{tag}.obu"
    env = {k: v for k, v in os.environ.items() if not ENV_STRIP.match(k)}
    env.update(TUNES[tune])
    env.update(ARMS[arm])
    try:
        carg = content_arg(img)
    except FileNotFoundError as e:
        return (cls, png_rel, tune, arm, preset, qp, -1, -1, -1,
                "", "", "", f"SRC-ERR:{e}")
    r = subprocess.run(
        [GTIME, "-f", "__T__%U %S %e", str(RUN), carg,
         str(W), str(H), str(qp), str(preset), str(cell / tag)],
        env=env, capture_output=True)
    tm = re.search(rb"__T__(\d+\.?\d*) (\d+\.?\d*) (\d+\.?\d*)", r.stderr)
    cpu_ms = int(round((float(tm.group(1)) + float(tm.group(2))) * 1000)) if tm else -1
    wall_ms = int(round(float(tm.group(3)) * 1000)) if tm else -1
    if r.returncode != 0 or not obu.exists():
        tail = (r.stderr.decode(errors="replace")
                .replace("\n", " ").replace(",", ";")[-140:])
        return (cls, png_rel, tune, arm, preset, qp, -1, wall_ms, cpu_ms,
                "", "", "", f"ENC-ERR{':' + tail if tail else ''}")
    if not src_yuv.exists():
        os.replace(str(cell / f"{tag}.yuv"), src_yuv)
    else:
        os.remove(str(cell / f"{tag}.yuv"))
    if not src_png.exists():
        i420_to_png(src_yuv.read_bytes(), W, H, str(src_png))
    sha = persist(obu)
    d = subprocess.run([AOMDEC, "-o", str(cell / f"{tag}.y4m"), str(obu)],
                       capture_output=True)
    if d.returncode != 0:
        return (cls, png_rel, tune, arm, preset, qp, obu.stat().st_size,
                wall_ms, cpu_ms, "", "", sha, "DEC-ERR")
    fw, fh, dec = y4m_to_i420((cell / f"{tag}.y4m").read_bytes())
    os.remove(cell / f"{tag}.y4m")
    if (fw, fh) != (W, H):
        return (cls, png_rel, tune, arm, preset, qp, obu.stat().st_size,
                wall_ms, cpu_ms, "", "", sha, f"DIM-ERR:{fw}x{fh}")
    dec_png = cell / f"{tag}.dec.png"
    i420_to_png(dec, W, H, str(dec_png))
    sc = score(src_png, dec_png)
    py = psnr_y(src_yuv.read_bytes(), dec, W, H)
    os.remove(dec_png)
    return (cls, png_rel, tune, arm, preset, qp, obu.stat().st_size,
            wall_ms, cpu_ms, f"{sc:.5f}", f"{py:.3f}", sha, "")


def jobs_for(rows, grid, cls_override=None):
    out = []
    for cls, png_rel in rows:
        c = cls_override or cls
        cell = OUT / "cells" / c / Path(png_rel).stem
        cell.mkdir(parents=True, exist_ok=True)
        for tune in TUNES:
            for preset, arms in grid.items():
                for arm in arms:
                    for qp in QPS:
                        out.append((c, png_rel, tune, arm, int(preset), qp))
    return out


def main():
    STREAMS.mkdir(parents=True, exist_ok=True)
    rows = [l.split("\t") for l in SUBSET.read_text().splitlines() if l.strip()]
    srows = [("gb82-sc", f"codec-corpus/gb82-sc/{p.name}")
             for p in sorted(SCREEN_DIR.glob("*.png"))]
    jobs = jobs_for(rows, PHOTO_GRID) + jobs_for(srows, SCREEN_GRID)
    done = set()
    if CSV.exists():
        for l in CSV.read_text().splitlines()[1:]:
            f = l.split(",")
            # Only clean cells count as done; an ENC-ERR/DEC-ERR row is
            # retried on the next run (analyze keeps the non-error row).
            if len(f) > 12 and not f[12]:
                done.add((f[1], f[2], f[3], f[4], int(f[5])))
    jobs = [j for j in jobs
            if (j[1], j[2], j[3], str(j[4]), j[5]) not in done]
    print(f"{len(jobs)} cells to run "
          f"({len(rows)} photo imgs, {len(srows)} screen imgs)", flush=True)
    n = 0
    with CSV.open("a") as fh:
        if not done:
            fh.write("cls,png,tune,arm,preset,qp,bytes,wall_ms,cpu_ms,"
                     "ssim2,psnr_y,sha256,err\n")
        with ThreadPoolExecutor(6) as ex:
            for res in ex.map(run_cell, jobs):
                (cls, png, tune, arm, p, qp, by, wl, cp, sc, py, sha, err) = res
                fh.write(f"{cls},{png},{tune},{arm},{p},{qp},{by},{wl},{cp},"
                         f"{sc},{py},{sha},{err}\n")
                n += 1
                if n % 50 == 0:
                    fh.flush()
                    print(f"  {n}/{len(jobs)}", flush=True)
    print("done", flush=True)


# ---------------- analysis ----------------

def pct(v, p):
    v = sorted(v)
    i = (len(v) - 1) * p / 100
    lo = int(i)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (i - lo)


def curve(pts, axis):
    return sorted((pts[q][axis], log(pts[q]["b"])) for q in pts)


def interp(curve_pts, x):
    xs = [c[0] for c in curve_pts]
    if x < xs[0] or x > xs[-1]:
        return None
    for i in range(len(curve_pts) - 1):
        x0, y0 = curve_pts[i]
        x1, y1 = curve_pts[i + 1]
        if x0 <= x <= x1:
            if x1 == x0:
                return y0
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    return curve_pts[-1][1]


def bdrate(a_pts, b_pts, axis):
    ca, cb = curve(a_pts, axis), curve(b_pts, axis)
    lo = max(ca[0][0], cb[0][0])
    hi = min(ca[-1][0], cb[-1][0])
    if hi <= lo:
        return None
    n = 40
    step = (hi - lo) / n
    diffs = []
    for i in range(n):
        x = lo + (i + 0.5) * step
        ya, yb = interp(ca, x), interp(cb, x)
        if ya is not None and yb is not None:
            diffs.append(ya - yb)
    if not diffs:
        return None
    return (exp(sum(diffs) / len(diffs)) - 1) * 100


def analyze():
    # A cell may appear twice (an err row then a clean rerun) — keep the
    # last row per cell; the dedupe collapses retries.
    dedup = {}
    for line in CSV.read_text().splitlines()[1:]:
        f = line.split(",")
        dedup[(f[0], f[1], f[2], f[3], f[4], f[5])] = f
    rows = []
    for f in dedup.values():
        rows.append(dict(cls=f[0], img=f[1], tune=f[2], arm=f[3],
                         p=int(f[4]), qp=int(f[5]), b=int(f[6]),
                         wall=float(f[7]), cpu=float(f[8]),
                         s=float(f[9]) if f[9] else float("nan"),
                         py=float(f[10]) if f[10] else float("nan"),
                         err=f[12] if len(f) > 12 else ""))
    good = [r for r in rows if not r["err"]]
    bad = [r for r in rows if r["err"]]
    klass = {"gb82-sc": "screen"}
    for r in rows:
        r["k"] = klass.get(r["cls"], "photo")
    g = defaultdict(dict)
    for r in good:
        g[(r["k"], r["img"], r["tune"], r["arm"], r["p"])][r["qp"]] = r

    combos = sorted({(k[0], k[2], k[4], k[3]) for k in g if k[3] != "base"})
    print(f"{'class':>6} {'tune':>4} {'p':>3} {'arm':>5} "
          f"{'BDs2 mean':>9} {'p25':>7} {'p50':>7} {'p75':>7} "
          f"{'BDpy mean':>9} {'p25':>7} {'p50':>7} {'p75':>7} "
          f"{'n':>3} {'win':>4} {'loss':>4} {'cpuRatio':>9} {'err':>4}")
    for k, tune, p, arm in combos:
        bd_s, bd_p, ratio, win, loss = [], [], [], 0, 0
        imgs = sorted({key[1] for key in g if key[0] == k and key[2] == tune
                       and key[4] == p})
        for img in imgs:
            a = g.get((k, img, tune, arm, p))
            b = g.get((k, img, tune, "base", p))
            if not a or not b or len(a) < 3 or len(b) < 3:
                continue
            v = bdrate(a, b, "s")
            if v is None:
                continue
            bd_s.append(v)
            vp = bdrate(a, b, "py")
            if vp is not None:
                bd_p.append(vp)
            win += v < -0.3
            loss += v > 0.3
            ca = sum(a[q]["cpu"] for q in a)
            cb = sum(b[q]["cpu"] for q in b)
            if cb > 0:
                ratio.append(ca / cb)
        derr = sum(1 for r in bad if r["k"] == k and r["tune"] == tune
                   and r["p"] == p and r["arm"] == arm)
        if not bd_s:
            print(f"{k:>6} {tune:>4} {p:>3} {arm:>5}  (no overlapping curve) "
                  f"{'':>50} {derr:>6}")
            continue
        print(f"{k:>6} {tune:>4} {p:>3} {arm:>5} "
              f"{sum(bd_s)/len(bd_s):9.3f} {pct(bd_s,25):7.3f} "
              f"{pct(bd_s,50):7.3f} {pct(bd_s,75):7.3f} "
              f"{sum(bd_p)/len(bd_p):9.3f} {pct(bd_p,25):7.3f} "
              f"{pct(bd_p,50):7.3f} {pct(bd_p,75):7.3f} "
              f"{len(bd_s):>3} {win:>4} {loss:>4} "
              f"{sum(ratio)/len(ratio):9.3f} {derr:>4}")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "analyze":
        analyze()
    else:
        main()

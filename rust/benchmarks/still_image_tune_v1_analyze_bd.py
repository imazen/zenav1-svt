#!/usr/bin/env python3
"""BD-rate per (preset, arm) vs a baseline arm, ssim2 as quality axis.

For each (image, preset, arm): rate-quality points at qp {20,32,44}.
BD-rate = mean relative bitrate diff over the overlapping quality range,
log-rate linear interpolation between qp points (robust to 3-point curves).
Negative = fewer bytes at equal quality.
"""
import sys
from collections import defaultdict
from math import exp, log

FILES = sys.argv[1:] or ["/home/lilith/tmp/tune_sweep/results_backfill.tsv"]
rows = []
for path in FILES:
    for line in open(path).read().splitlines()[1:]:
        f = line.split("\t")
        try:
            rows.append(dict(cls=f[0], img=f[1], arm=f[2], p=int(f[3]), qp=int(f[4]),
                             b=int(f[5]), wall=float(f[6]), cpu=float(f[7]),
                             s=float(f[8]), note=f[9] if len(f) > 9 else ""))
        except (ValueError, IndexError):
            pass

g = defaultdict(dict)
for r in rows:
    if r["note"]:
        continue
    g[(r["img"], r["arm"], r["p"])][r["qp"]] = r

imgs = sorted({r["img"] for r in rows})
arms = sorted({r["arm"] for r in rows})
presets = sorted({r["p"] for r in rows})
qps = sorted({r["qp"] for r in rows})


def pct(v, p):
    v = sorted(v)
    i = (len(v) - 1) * p / 100
    lo = int(i)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (i - lo)


def curve(pts):
    """(ssim2, log bytes) sorted by ssim2."""
    return sorted((pts[q]["s"], log(pts[q]["b"])) for q in pts)


def interp(curve_pts, x):
    """log-rate at quality x via linear interp; None outside range."""
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


def bdrate(a_pts, b_pts):
    """BD-rate % of a vs b over common quality range."""
    ca, cb = curve(a_pts), curve(b_pts)
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


def analyze(base_arm, arm_filter=None, title=""):
    print(f"=== BD-rate vs {base_arm} {title} ===")
    print(f"{'preset':>7} {'arm':<6} {'BD%p25':>7} {'BD%p50':>7} {'BD%p75':>7} "
          f"{'n':>4} {'win':>4} {'loss':>4} {'wall_ms':>8} {'cpu_ms':>8}")
    for p in presets:
        for arm in arms:
            if arm == base_arm or (arm_filter and arm not in arm_filter):
                continue
            bd, wl, cp = [], [], []
            win = loss = 0
            for img in imgs:
                a = g.get((img, arm, p))
                b = g.get((img, base_arm, p))
                if not a or not b or len(a) < 3 or len(b) < 3:
                    continue
                v = bdrate(a, b)
                if v is None:
                    continue
                bd.append(v)
                win += v < -0.3
                loss += v > 0.3
                wl.append(sum(a[q]["wall"] for q in a) / len(a))
                cp.append(sum(a[q]["cpu"] for q in a) / len(a))
            if not bd:
                continue
            print(f"{p:>7} {arm:<6} {pct(bd,25):7.2f} {pct(bd,50):7.2f} {pct(bd,75):7.2f} "
                  f"{len(bd):>4} {win:>4} {loss:>4} {pct(wl,50):8.0f} {pct(cp,50):8.0f}")
        print()


if __name__ == "__main__":
    analyze("t1", title="(baseline = default tune)")
    analyze("t3", arm_filter={"st", "s0", "s3", "s5", "qm2", "qm6", "ac10",
                              "vs2", "cv1", "o6", "o7", "o8", "o7ac", "t4"},
            title="(extras/bundle vs tune IQ)")

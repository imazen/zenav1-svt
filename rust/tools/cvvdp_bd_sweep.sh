#!/usr/bin/env bash
# RATE-QUALITY sweep companion to tools/cvvdp_equiv.sh: encode a cell at a
# ladder of QPs on both encoders, score every decoded frame vs the source
# with cvvdp, and report a Bjontegaard-style delta between the two
# (log-rate, JOD) curves — the "does the diverging port still track C's RD
# frontier" question the byte census cannot answer.
#
# THIS IS A DIAGNOSTIC, NOT A GATE. cvvdp is a still-image metric applied
# per frame (mean over frames); the sweep tells you whether the port's RD
# operating points sit on the same frontier as C's, within metric noise.
# It does not assert bytes, and a small delta is not proof of parity —
# use it to prioritize which diverging cells are quality bugs vs cosmetic.
#
# Usage: tools/cvvdp_bd_sweep.sh <clip|file.i420> <size> <preset> [frames]
# Env:   QPS — space-separated ladder (default "20 32 44 55" — spans the
#          census's qp20/40/55 anchors plus one midpoint so the cubic fit
#          has real shape).
#        ZENMETRICS, AOMDEC, CVVDP_KEEP — as in cvvdp_equiv.sh.
#
# Output: per-QP (bytes, mean JOD) for each side, then
#   "BD-JOD delta: +X.XXX JOD (port vs C, positive = better at same rate)"
# computed by the standard Bjontegaard integration: cubic fit of JOD to
# log-rate per side over the overlapping log-rate range, difference of the
# two fitted curves integrated then divided by range width.
set -euo pipefail

if [[ $# -lt 3 ]]; then
    echo "usage: $0 <clip|file.i420> <size> <preset> [frames]" >&2
    exit 2
fi
CLIP=$1; SIZE=$2; PRESET=$3; FRAMES="${4:-8}"
QPS="${QPS:-20 32 44 55}"
HERE=$(cd "$(dirname "$0")" && pwd)

OUT="${TMPDIR:-$HOME/tmp}/cvvdp-bd-$$"
mkdir -p "$OUT"
trap '[[ -z ${CVVDP_KEEP:-} ]] && rm -rf "$OUT" || true' EXIT

for q in $QPS; do
    echo "== qp $q ==" >&2
    CVVDP_KEEP=1 TMPDIR="$OUT" "$HERE/cvvdp_equiv.sh" "$CLIP" "$SIZE" "$q" "$PRESET" "$FRAMES" \
        > "$OUT/qp$q.txt" 2>"$OUT/qp$q.log" || { echo "qp $q: failed (see $OUT/qp$q.log)" >&2; exit 1; }
done

python3 - "$OUT" $QPS <<'PY'
import math, re, sys
work = sys.argv[1]
qps = sys.argv[2:]

def parse(path):
    tot = None; jods = {"c": [], "rs": []}
    for ln in open(path):
        m = re.match(r"f(\d+)\s+(\d+)\s+(\d+)\s+([0-9.]+)\s+([0-9.]+)\s+([+-][0-9.]+)", ln)
        if m:
            jods["c"].append(float(m.group(4))); jods["rs"].append(float(m.group(5)))
        m2 = re.search(r"totals bytes: C=(\d+) port=(\d+)", ln)
        if m2: tot = (int(m2.group(1)), int(m2.group(2)))
    return tot, jods

pts = {"c": [], "rs": []}   # list of (bytes, meanJOD)
print(f"{'qp':>4} {'bytes_C':>9} {'bytes_port':>11} {'JOD_C':>9} {'JOD_port':>9} {'dJOD':>8}")
for q in qps:
    tot, jods = parse(f"{work}/qp{q}.txt")
    if tot is None or not jods["c"]:
        print(f"{q:>4}  (no data)"); continue
    mc = sum(jods["c"]) / len(jods["c"])
    mr = sum(jods["rs"]) / len(jods["rs"])
    pts["c"].append((tot[0], mc)); pts["rs"].append((tot[1], mr))
    print(f"{q:>4} {tot[0]:>9} {tot[1]:>11} {mc:>9.4f} {mr:>9.4f} {mr-mc:+8.4f}")

def cubic_interp(points):
    """Least-squares cubic JOD = f(log rate). numpy-free polyfit."""
    xs = [math.log(b) for b, _ in points]
    ys = [j for _, j in points]
    n = len(xs)
    if n < 4: return None
    # normal equations for cubic
    A = [[sum(x**(i+j) for x in xs) for j in range(4)] for i in range(4)]
    B = [sum(y * x**i for x, y in zip(xs, ys)) for i in range(4)]
    # gaussian elimination
    for i in range(4):
        piv = A[i][i]
        if abs(piv) < 1e-12:
            for k in range(i+1, 4):
                if abs(A[k][i]) > abs(piv):
                    A[i], A[k] = A[k], A[k]; B[i], B[k] = B[k], B[k]; piv = A[i][i]
        for k in range(4):
            if k != i:
                f = A[k][i] / piv
                for j in range(4): A[k][j] -= f * A[i][j]
                B[k] -= f * B[i]
    return [B[i] / A[i][i] for i in range(4)]

def integ(coef, lo, hi):
    c0, c1, c2, c3 = coef
    F = lambda x: c0*x + c1*x**2/2 + c2*x**3/3 + c3*x**4/4
    return F(hi) - F(lo)

cc, rc = cubic_interp(pts["c"]), cubic_interp(pts["rs"])
if cc and rc:
    lo = max(min(math.log(b) for b, _ in pts["c"]), min(math.log(b) for b, _ in pts["rs"]))
    hi = min(max(math.log(b) for b, _ in pts["c"]), max(math.log(b) for b, _ in pts["rs"]))
    if hi > lo:
        d = (integ(rc, lo, hi) - integ(cc, lo, hi)) / (hi - lo)
        print(f"BD-JOD delta over log-rate [{math.exp(lo):.0f}..{math.exp(hi):.0f}] bytes: "
              f"{d:+.4f} JOD (port-C; positive = better quality at same rate)")
    else:
        print("BD-JOD: rate ranges do not overlap")
PY
echo "sweep dir: $OUT (set CVVDP_KEEP=1 to keep)"

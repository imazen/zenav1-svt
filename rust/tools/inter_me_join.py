#!/usr/bin/env python3
"""Join C's SVT_SUBPEL_OUT stage-0 `start=` against the port's PMEDBG `fpme=`.

C's stage-0 `start_mv` is the MV `read_refine_me_mvs` hands the sub-pel tree,
i.e. the full-pel chain's output INCLUDING `md_nsq_motion_search`; the port's
`fpme` is `inter_search_arm`'s `fp_me_mv` at the same point. The key is
(org_x, org_y, bwidth, bheight, list_idx, ref_idx) — the SHAPE is in the key
on purpose, because a 16x32 and an 8x16 at the same origin are different
searches and joining them would invent an agreement.

Prints "<joined> <nsq> <differ>"; --verbose lists the disagreements, one per
line, in a STABLE machine form the gate diffs against its pinned set:

    SQ|NSQ org_x org_y bw bh li ri Cy Cx Py Px

Stable means sorted and fully specified — the gate compares the whole set, so
a row that starts agreeing fails just as loudly as a new row that stops.
See tools/inter_me_join_gate.sh for what this is FOR.
"""
import re
import sys

C_RE = re.compile(
    r"SUBPEL stage=(\d+) org=\((\d+),(\d+)\) bsize=\d+ bw=(\d+) bh=(\d+) "
    r"sq=(\d+) li=(\d+) ri=(\d+) start=\((-?\d+),(-?\d+)\)")
P_RE = re.compile(
    r"PMEDBG org=\((\d+),(\d+)\) (\d+)x(\d+) li=(\d+) ri=(\d+) .*?fpme=\((-?\d+),(-?\d+)\)")


def main() -> int:
    cpath, ppath = sys.argv[1], sys.argv[2]
    verbose = "--verbose" in sys.argv[3:]
    c = {}
    for line in open(cpath, errors="replace"):
        m = C_RE.search(line)
        if not m or m.group(1) != "0":
            continue
        _, x, y, bw, bh, _sq, li, ri, sy, sx = m.groups()
        c[(int(x), int(y), int(bw), int(bh), int(li), int(ri))] = (int(sy), int(sx))
    p = {}
    for line in open(ppath, errors="replace"):
        m = P_RE.search(line)
        if not m:
            continue
        x, y, bw, bh, li, ri, fy, fx = m.groups()
        p[(int(x), int(y), int(bw), int(bh), int(li), int(ri))] = (int(fy), int(fx))
    keys = sorted(set(c) & set(p))
    nsq = sum(1 for k in keys if k[2] != k[3])
    bad = [k for k in keys if c[k] != p[k]]
    if verbose:
        for k in bad:
            shape = "SQ" if k[2] == k[3] else "NSQ"
            print(f"{shape} {k[0]} {k[1]} {k[2]} {k[3]} {k[4]} {k[5]} "
                  f"{c[k][0]} {c[k][1]} {p[k][0]} {p[k][1]}")
    else:
        print(f"{len(keys)} {nsq} {len(bad)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

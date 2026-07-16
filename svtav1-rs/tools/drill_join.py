#!/usr/bin/env python3
"""Drill helpers for C-vs-port divergence localization (used by drill_cell.sh).

Two modes:

  --locate <dir> <w> <h>
      Read the raw recon planes dumped by both sides (c.p0..p2 from the C
      --wrap, p.p0..p2 from SVTAV1_RECON_BIN) and print the SB-root mi of the
      FIRST divergent pixel as "mi_row mi_col" on stdout (empty + rc 3 if the
      recons are byte-identical -> the divergence is post-recon). Human summary
      goes to stderr.

  --join <c.pickpart> <rs.sbdump>
      Join C's chosen partition tree (PICKPART/CLEAF lines) against the port's
      SB-filtered NSQDBG dump (TS/BLK lines). Prints only actionable signal:
      structural flips (C parent-vs-split != port), leaf mode/uv/txd flips,
      and the top RD deltas (constant-offset fingerprints jump out here).
"""

import re
import sys


def locate(d, w, h):
    """First divergent superblock in SB RASTER (== encode/causal) order.

    Pixel-raster first-diff is WRONG here: an earlier-in-encode-order SB may
    first diverge at a lower pixel row, so a later SB's top row wins the pixel
    race and the drill targets a downstream cascade instead of the root. Scan
    SB by SB (all three planes) and report the first SB with any diff.
    """
    planes = []
    for plane, (pw, ph) in enumerate([(w, h), (w // 2, h // 2), (w // 2, h // 2)]):
        try:
            c = open(f"{d}/c.p{plane}", "rb").read()
            p = open(f"{d}/p.p{plane}", "rb").read()
        except FileNotFoundError as e:
            print(f"locate: missing {e.filename}", file=sys.stderr)
            return 2
        planes.append((c, p, pw, ph, 64 if plane == 0 else 32))
        nd = sum(1 for i in range(min(len(c), len(p))) if c[i] != p[i])
        print(f"plane{plane}: {nd} byte diffs", file=sys.stderr)
    for sby in range((h + 63) // 64):
        for sbx in range((w + 63) // 64):
            for pi, (c, p, pw, ph, sbsz) in enumerate(planes):
                x0, y0 = sbx * sbsz, sby * sbsz
                for y in range(y0, min(y0 + sbsz, ph)):
                    row = slice(y * pw + x0, y * pw + min(x0 + sbsz, pw))
                    if c[row] != p[row]:
                        x = next(i for i in range(row.start, row.stop) if c[i] != p[i]) - y * pw
                        print(
                            f"first divergent SB in encode order: SB({sby},{sbx}) "
                            f"plane{pi} at ({x},{y}) c={c[y*pw+x]} p={p[y*pw+x]}",
                            file=sys.stderr,
                        )
                        print(sby * 16, sbx * 16)
                        return 0
    return 3  # all planes identical


# C PartitionType -> port funnel-shape id (depth_refine.rs c_part).
C_PART_TO_SHAPE = {0: 0, 1: 1, 2: 2, 8: 3, 9: 4}
PART_NAME = {0: "NONE", 1: "HORZ", 2: "VERT", 3: "SPLIT", 4: "HORZ_A",
             5: "HORZ_B", 6: "VERT_A", 7: "VERT_B", 8: "HORZ_4", 9: "VERT_4"}


def join(c_path, p_path):
    c_nodes, c_leaves = {}, {}
    for line in open(c_path):
        m = re.match(r"PICKPART mi=\((\d+),(\d+)\) bsize=(\d+) partition=(\d+) rd=(-?\d+)", line)
        if m:
            r, c, b, part, rd = map(int, m.groups())
            c_nodes[(r, c, b)] = (part, rd)
        m = re.match(r"CLEAF mi=\((\d+),(\d+)\) bsize=(\d+) shape=(\d+) nsi=(\d+) mode=(\d+) uv=(\d+) txd=(\d+)"
                     r".*? txt=\[([\d,-]+)\] ye=\[([\d,]+)\] ue=(\d+) ve=(\d+)", line)
        if m:
            g = m.groups()
            r, c, b = int(g[0]), int(g[1]), int(g[2])
            c_leaves[(r, c, b)] = dict(mode=int(g[5]), uv=int(g[6]), txd=int(g[7]),
                                       txt=g[8].split(","), ye=g[9].split(","), ue=g[10], ve=g[11])

    p_ts, p_blk, p_shape = {}, {}, {}
    for line in open(p_path):
        m = re.match(r"NSQDBG SHAPE mi=\((\d+),(\d+)\) bsize=(\d+) shape=(\d+) valid=(\d+) part_cost=(\d+)", line)
        if m:
            r, c, b, shape, valid, pc = map(int, m.groups())
            p_shape[(r, c, b, shape)] = pc
        m = re.match(
            r"NSQDBG TS mi=\((\d+),(\d+)\) bsize=(\d+) parent_valid=(\d+) parent=(\d+) split=(\d+)"
            r"(?: sr=(\d+) c=\[(\d+),(\d+),(\d+),(\d+)\])? chose=(\w+)", line)
        if m:
            g = m.groups()
            r, c, b = int(g[0]), int(g[1]), int(g[2])
            p_ts[(r, c, b)] = dict(pv=int(g[3]), parent=int(g[4]), split=int(g[5]),
                                   sr=g[6], ch=g[7:11], chose=g[11])
        m = re.match(
            r"NSQDBG BLK mi=\((\d+),(\d+)\) bsize=(\d+) shape=(\d+) nsi=(\d+) cost=(\d+) rate=(\d+)"
            r" dist=(\d+) mode=(\d+) coeff=(\d+) nz=(\d+) txd=(\d+) uv=(\d+)"
            r"(?: txt=\[([\d,]*)\] ye=\[([\d,]*)\] ue=(\d+) ve=(\d+))?", line)
        if m:
            g = m.groups()
            v = list(map(int, g[:13]))
            p_blk[(v[0], v[1], v[2], v[3], v[4])] = dict(
                cost=v[5], rate=v[6], dist=v[7], mode=v[8], txd=v[11], uv=v[12],
                txt=(g[13] or "").split(",") if g[13] else None,
                ye=(g[14] or "").split(",") if g[14] else None, ue=g[15], ve=g[16])

    # 1. Structural flips: C's parent-vs-split choice per square node vs port TS.
    flips, matches = [], 0
    for (r, c, b), (part, rd) in sorted(c_nodes.items(), key=lambda kv: (-kv[0][2], kv[0][0], kv[0][1])):
        ts = p_ts.get((r, c, b))
        if ts is None:
            continue  # port had no TS there (e.g. forced depth) — leaf compare covers it
        c_split = part == 3
        p_split = ts["chose"] == "split"
        if c_split != p_split:
            flips.append(f"  mi=({r},{c}) {b}: C={PART_NAME.get(part,part)} rd={rd} | "
                         f"port chose={ts['chose']} parent={ts['parent']} split={ts['split']} sr={ts['sr']}")
        else:
            matches += 1
    print(f"STRUCTURE: {matches} square nodes agree, {len(flips)} FLIP(s)")
    for f in flips[:10]:
        print(f)

    # 2. Leaf decisions + RD deltas on structure-matching NONE leaves.
    deltas, mode_flips, coeff_flips = [], [], []
    for (r, c, b), (part, rd) in c_nodes.items():
        if part != 0:
            continue
        cl = c_leaves.get((r, c, b))
        pb = p_blk.get((r, c, b, 0, 0))
        if not cl or not pb:
            continue
        # C's node rd INCLUDES the partition-rate term; the port's BLK cost
        # does not, but its SHAPE part_cost does — compare like with like.
        pcost = p_shape.get((r, c, b, 0), pb["cost"])
        d = pcost - rd
        deltas.append((abs(d), d, r, c, b))
        if (pb["mode"], pb["uv"], pb["txd"]) != (cl["mode"], cl["uv"], cl["txd"]):
            mode_flips.append(f"  mi=({r},{c}) {b}: C mode/uv/txd=({cl['mode']},{cl['uv']},{cl['txd']}) | "
                              f"port=({pb['mode']},{pb['uv']},{pb['txd']}) Crd={rd} Pcost={pb['cost']}")
        elif pb["txt"] is not None:
            # Same mode/uv/txd: compare coeff-level state on the used txbs.
            n = len(pb["txt"])
            if (pb["txt"] != cl["txt"][:n] or pb["ye"] != cl["ye"][:n]
                    or pb["ue"] != cl["ue"] or pb["ve"] != cl["ve"]):
                coeff_flips.append(
                    f"  mi=({r},{c}) {b}: C txt={cl['txt'][:n]} ye={cl['ye'][:n]} ue={cl['ue']} ve={cl['ve']} | "
                    f"port txt={pb['txt']} ye={pb['ye']} ue={pb['ue']} ve={pb['ve']}")
    print(f"LEAF MODES: {len(mode_flips)} flip(s) of {len(deltas)} compared NONE leaves")
    for f in mode_flips[:10]:
        print(f)
    print(f"COEFF-LEVEL (same mode, differing tx_type/eob): {len(coeff_flips)}")
    for f in coeff_flips[:10]:
        print(f)
    deltas.sort(reverse=True)
    nz = [x for x in deltas if x[0] != 0]
    print(f"RD DELTAS (port-C) nonzero on {len(nz)}/{len(deltas)} leaves; top:")
    for _, d, r, c, b in deltas[:8]:
        print(f"  mi=({r},{c}) bsize={b}: {d:+d}")
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--locate":
        sys.exit(locate(sys.argv[2], int(sys.argv[3]), int(sys.argv[4])))
    if len(sys.argv) >= 2 and sys.argv[1] == "--join":
        sys.exit(join(sys.argv[2], sys.argv[3]))
    print(__doc__, file=sys.stderr)
    sys.exit(2)

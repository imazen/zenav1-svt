#!/usr/bin/env python3
"""tree_diff: join C's final coded tree (CTREE, the svt_aom_update_mi_map
--wrap dump — valid at EVERY preset) against the port's (PTREE, the
SVTAV1_PACKTREE pack-time dump) and print ONLY the flips.

  tree_diff.py <c_ctree_file> <rs_ptree_file> [--max N]

Token-frugal contract: bounded output (default 12 flip lines + a 4-line
summary), no full-tree dumps — pipe-friendly for drill_cell.sh and agents.

Comparable fields: bsize, mode, uv, fi, ady, aduv, txd, cflidx, cflsgn,
pal (port side has no palette yet -> a C pal>0 row is reported as a
palette flip, not a field mismatch), and skip (C dumps the all-plane skip
bit; the port dumps yeob/ueob/veob — skip flips only when definite:
C skip=1 with any port eob>0, or C skip=0 with all port eobs 0).
`part` is contextual (stamped from the parent split) and compared but
reported separately — a part flip with equal geometry usually means the
PARENT tree differs, not this leaf.

Both dumps may stamp a block more than once (MD re-stamps at depth
changes on the C side); last record wins.
"""
import sys
import re

ROW = re.compile(r"mi=\((\d+),(\d+)\)\s+(.*)")


def parse(path, tag):
    """-> {(mi_row, mi_col): {field: int}} — last record per key wins."""
    out = {}
    with open(path) as f:
        for line in f:
            if not line.startswith(tag):
                continue
            m = ROW.search(line)
            if not m:
                continue
            key = (int(m.group(1)), int(m.group(2)))
            fields = {}
            for kv in m.group(3).split():
                if "=" in kv:
                    k, v = kv.split("=", 1)
                    try:
                        fields[k] = int(v)
                    except ValueError:
                        pass
            out[key] = fields
    return out


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    max_flips = 12
    if "--max" in sys.argv:
        max_flips = int(sys.argv[sys.argv.index("--max") + 1])
    c = parse(sys.argv[1], "CTREE")
    r = parse(sys.argv[2], "PTREE")
    # --at row,col: print both sides' full record for one mi and exit —
    # the bounded way to inspect a single block (2 lines).
    if "--at" in sys.argv:
        row, col = map(int, sys.argv[sys.argv.index("--at") + 1].split(","))
        k = (row, col)
        print(f"C    {k}: {c.get(k, 'ABSENT')}")
        print(f"port {k}: {r.get(k, 'ABSENT')}")
        sys.exit(0)
    if not c or not r:
        print(f"TREE: no records (C={len(c)} port={len(r)}) — check the dumps ran")
        sys.exit(2)

    only_c = sorted(set(c) - set(r))
    only_r = sorted(set(r) - set(c))
    both = sorted(set(c) & set(r))

    # Calibrated on a byte-identical cell (1147124 p2 q20): C's mi grid
    # retains STALE values from losing MD candidates for fields the writer
    # only codes conditionally — cfl alphas when uv != CFL(13), angle
    # deltas when the mode is non-directional, `part` re-stamped per
    # recursion. Compare each field only where the stream actually codes
    # it, or an identical stream reports thousands of phantom flips.
    flips = []          # (key, field, cval, rval)
    part_flips = 0
    pal_blocks = []
    skip_flips = []
    field_counts = {}
    for k in both:
        cf, rf = c[k], r[k]
        checks = [("bsize", True), ("mode", True), ("uv", True), ("fi", True), ("txd", True)]
        # angle deltas: only coded for directional modes (1..=8).
        checks.append(("ady", 1 <= cf.get("mode", 0) <= 8))
        checks.append(("aduv", 1 <= cf.get("uv", 0) <= 8))
        # CfL alphas: only coded when uv == UV_CFL (13).
        cfl_live = cf.get("uv", 0) == 13
        checks.append(("cflidx", cfl_live))
        checks.append(("cflsgn", cfl_live))
        for fld, live in checks:
            if live and fld in cf and fld in rf and cf[fld] != rf[fld]:
                flips.append((k, fld, cf[fld], rf[fld]))
                field_counts[fld] = field_counts.get(fld, 0) + 1
        if cf.get("part") is not None and rf.get("part") is not None and cf["part"] != rf["part"]:
            part_flips += 1
        if cf.get("pal", 0) > 0:
            pal_blocks.append(k)
        cskip = cf.get("skip")
        if cskip is not None:
            r_all0 = rf.get("yeob", 0) == 0 and rf.get("ueob", 0) == 0 and rf.get("veob", 0) == 0
            if (cskip == 1 and not r_all0) or (cskip == 0 and r_all0):
                skip_flips.append((k, cskip, rf.get("yeob", 0), rf.get("ueob", 0), rf.get("veob", 0)))

    # C stamps finer sub-keys than the port's min-8x8 leaves even on
    # byte-identical streams (920 C-only keys on the calibration cell) —
    # C-only keys are only alarming when port-only keys ALSO exist
    # (mutual geometry mismatch = genuinely different trees).
    geom_alarm = bool(only_r)
    print(
        f"TREE: {len(both)} blocks joined, {len(flips)} field flips, "
        f"{len(skip_flips)} skip flips, {len(pal_blocks)} C palette blocks, "
        f"geometry: {len(only_c)} C-only / {len(only_r)} port-only"
        f"{' <-- TREES DIFFER' if geom_alarm else ' (C sub-key granularity; benign unless port-only > 0)'}"
        f" [part re-stamps not compared: {part_flips} raw diffs]"
    )
    if geom_alarm:
        print(f"  first C-only mi: {only_c[:3]}  first port-only mi: {only_r[:3]}")
    if field_counts:
        print("  flip counts: " + " ".join(f"{k}={v}" for k, v in sorted(field_counts.items())))
    for k, fld, cv, rv in flips[:max_flips]:
        print(f"  FLIP mi={k} {fld}: C={cv} port={rv}")
    if len(flips) > max_flips:
        print(f"  ... {len(flips) - max_flips} more flips suppressed (--max)")
    for k, cs, ye, ue, ve in skip_flips[:4]:
        print(f"  SKIPFLIP mi={k} C_skip={cs} port eobs y={ye} u={ue} v={ve}")
    if pal_blocks:
        print(f"  C palette blocks (port codes none): first {pal_blocks[:6]}")
    sys.exit(0 if not flips and not only_r and not skip_flips else 1)


if __name__ == "__main__":
    main()

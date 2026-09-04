#!/usr/bin/env python3
"""Print the CALLER edges of every function matching a regex, from a
`callgrind_annotate --tree=caller` dump — the edge-level numbers the
callcount_* records quote ("tx_type_search -> svt_aom_quantize_inv_quantize
(270,415x)").

Usage: tree_callers.py <tree file> <regex> [<regex>...]

The tree format is, for each function, a run of `< caller (Nx)` lines with
that edge's INCLUSIVE Ir and share, followed by the `* function` line. This
prints each matching `*` block as:

  * <function>  <incl Ir> (<pct>)
      <N>x  <incl Ir>  (<pct>)  <caller>

Counts are exact call counts of that edge; Ir is the inclusive cost of the
callee when called from that caller (callgrind's attribution).
"""
import re
import sys

STAR = re.compile(r"^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s+\*\s+(\S+?):(.+?)(?:\s+\[.*\])?\s*$")
CALLER = re.compile(r"^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s+<\s+(\S+?):(.+?)\s+\(([\d,]+)x\)(?:\s+\[.*\])?\s*$")


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    path = sys.argv[1]
    rxs = [re.compile(r) for r in sys.argv[2:]]
    pending = []
    with open(path, errors="replace") as f:
        for line in f:
            m = CALLER.match(line)
            if m:
                pending.append((int(m.group(5).replace(",", "")),
                                int(m.group(1).replace(",", "")),
                                m.group(2), m.group(4).strip()))
                continue
            m = STAR.match(line)
            if m:
                fn = m.group(4).strip()
                if any(r.search(fn) for r in rxs):
                    print(f"* {fn}  {m.group(1)} ({m.group(2)}%)")
                    for calls, ir, pct, caller in sorted(pending, reverse=True):
                        print(f"    {calls:>12,}x  {ir:>16,}  ({pct:>5}%)  {caller}")
                pending = []
                continue
            if line.strip() == "":
                pending = []


if __name__ == "__main__":
    main()

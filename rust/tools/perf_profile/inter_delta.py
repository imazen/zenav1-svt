#!/usr/bin/env python3
"""Per-function cost of the INTER frame: the N=2 cell minus the N=1 cell.

`tools/perf_gate.sh` prices the inter frame by differencing WALL CLOCK medians
(frames=2 minus frames=1, video mode both), which cannot resolve C's side --
its p25/p75 spread is the size of the difference. Callgrind is deterministic,
so the SAME subtraction on instruction counts is exact and resolves per
symbol. This script does that subtraction over the files
`callcount_cells.sh` writes for two cells profiled under the same binaries:

  self_<side>_<cell>_p<P>.txt   callgrind_annotate --threshold=100 (self Ir)
  incl_<side>_<cell>_p<P>.txt   callgrind_annotate --inclusive=yes --threshold=100
  cc_<side>_<cell>_p<P>.tsv     callcount.py --demangle (calls into each fn)

and prints, per function, self/inclusive Ir and call count for the N=1 cell,
the N=2 cell, and their difference, ranked by self-Ir difference, with each
row's share of the whole-process Ir difference.

Usage:
  inter_delta.py <cells_dir> --n1 <cell> --n2 <cell> --preset <P>
                 [--side port|c] [--tsv out.tsv] [--top N] [--min-share PCT]

Reading rules (they are the traps):
  * A DIFFERENCE of two profiles: a function whose count is the same in both
    cells reads 0 here even if it is huge. Frame-0 work cancels; setup and
    teardown cancel; what remains is what the second (inter) frame ADDED, plus
    any frame-0 work whose cost CHANGED because a second frame exists (a
    reference-picture copy, a larger DPB, a different filter level). A
    negative delta is real and means the N=2 encode did LESS of that than the
    N=1 encode.
  * `share` is self-delta / total-process-delta. Rows can be negative and the
    shares of the top rows can exceed 100 % between them.
  * Self Ir is misleading where one side inlines and the other does not; the
    inclusive column is beside it for that reason. Rank by self, read both.
  * callgrind charges `rep stosb` one Ir per byte: a memset/calloc row is an
    order of magnitude heavier here than on hardware (WORKING-ON-THIS.md §5).
  * The two sides' symbol names are NOT joined here; join by C-comment
    citation + call-graph position, as callcount_join.py does, never by name.
"""
import argparse
import os
import re
import sys

ROW = re.compile(r"^\s*([\d,]+)\s+\(\s*-?[\d.]+%\)\s+(\S+?):(.*?)(?:\s+\[([^\]]*)\])?\s*$")
TOTAL = re.compile(r"^\s*([\d,]+)\s+\(100\.0%\)\s+PROGRAM TOTALS")


def parse_annotate(path):
    """-> (total_ir, {function: ir}) from a callgrind_annotate dump."""
    total = None
    cost = {}
    in_table = False
    with open(path, errors="replace") as f:
        for line in f:
            if line.startswith("-- Auto-annotated") or line.startswith("-- line"):
                break
            m = TOTAL.match(line)
            if m:
                total = int(m.group(1).replace(",", ""))
                continue
            if re.match(r"^Ir\s+file:function", line):
                in_table = True
                continue
            if not in_table:
                continue
            m = ROW.match(line)
            if not m:
                continue
            fn = m.group(3).strip()
            obj = os.path.basename(m.group(4)) if m.group(4) else ""
            # libc-internal frames share names with the binary's (`(below main)`);
            # keep the object on those so two different frames never merge.
            key = fn if obj.endswith(("perf_encode", "perf_c_encode")) or not obj else f"{fn} [{obj}]"
            cost[key] = cost.get(key, 0) + int(m.group(1).replace(",", ""))
    if total is None:
        sys.exit(f"{path}: no PROGRAM TOTALS line")
    return total, cost


def parse_calls(path):
    """-> {function: calls} from callcount.py output."""
    calls = {}
    with open(path, errors="replace") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t", 1)
            if len(parts) != 2 or not parts[0].isdigit():
                continue
            calls[parts[1].strip()] = calls.get(parts[1].strip(), 0) + int(parts[0])
    return calls


def calls_for(key, calls):
    """The callcount table is keyed on rustfilt's spelling; the annotate table
    on valgrind's. They agree on every symbol seen so far; where they do not,
    strip the `[object]` suffix and try again, else report 'na'."""
    if key in calls:
        return calls[key]
    bare = re.sub(r" \[[^\]]*\]$", "", key)
    return calls.get(bare, "na")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cells_dir")
    ap.add_argument("--n1", required=True, help="cell name of the FRAMES=1 VIDEO=1 run")
    ap.add_argument("--n2", required=True, help="cell name of the FRAMES=2 run")
    ap.add_argument("--preset", required=True)
    ap.add_argument("--side", default="port", choices=["port", "c"])
    ap.add_argument("--tsv", help="write the full ranked table here")
    ap.add_argument("--top", type=int, default=40)
    ap.add_argument("--min-share", type=float, default=0.0, help="print rows at or above this %% share (abs)")
    a = ap.parse_args()

    d, s, p = a.cells_dir, a.side, a.preset
    def fp(kind, cell, ext):
        path = os.path.join(d, f"{kind}_{s}_{cell}_p{p}.{ext}")
        if not os.path.exists(path):
            sys.exit(f"missing {path}")
        return path

    t1, self1 = parse_annotate(fp("self", a.n1, "txt"))
    t2, self2 = parse_annotate(fp("self", a.n2, "txt"))
    _, incl1 = parse_annotate(fp("incl", a.n1, "txt"))
    _, incl2 = parse_annotate(fp("incl", a.n2, "txt"))
    c1 = parse_calls(fp("cc", a.n1, "tsv"))
    c2 = parse_calls(fp("cc", a.n2, "tsv"))

    total_delta = t2 - t1
    keys = set(self1) | set(self2)
    rows = []
    for k in keys:
        s1, s2 = self1.get(k, 0), self2.get(k, 0)
        i1, i2 = incl1.get(k, 0), incl2.get(k, 0)
        k1, k2 = calls_for(k, c1), calls_for(k, c2)
        kd = (k2 - k1) if isinstance(k1, int) and isinstance(k2, int) else "na"
        rows.append((s2 - s1, k, s1, s2, i1, i2, i2 - i1, k1, k2, kd))
    rows.sort(key=lambda r: -r[0])

    # The sum of the per-function self deltas must equal the process delta:
    # this is the check that the parser read every row on both sides.
    sum_self = sum(r[0] for r in rows)
    hdr = ("rank\tside\tfunction\tself_n1\tself_n2\tself_delta\tshare_pct\t"
           "incl_n1\tincl_n2\tincl_delta\tcalls_n1\tcalls_n2\tcalls_delta")
    if a.tsv:
        with open(a.tsv, "w") as out:
            out.write(f"# inter_delta.py side={s} n1={a.n1} n2={a.n2} preset={p}\n")
            out.write(f"# total Ir n1={t1} n2={t2} delta={total_delta} sum_of_self_deltas={sum_self}\n")
            out.write(hdr + "\n")
            for i, r in enumerate(rows, 1):
                share = 100.0 * r[0] / total_delta if total_delta else 0.0
                out.write(f"{i}\t{s}\t{r[1]}\t{r[2]}\t{r[3]}\t{r[0]}\t{share:.3f}\t"
                          f"{r[4]}\t{r[5]}\t{r[6]}\t{r[7]}\t{r[8]}\t{r[9]}\n")

    print(f"== {s}: {a.n2} - {a.n1} @ p{p}")
    print(f"   total Ir  n1={t1:,}  n2={t2:,}  delta={total_delta:,}  "
          f"(sum of self deltas {sum_self:,}; {'EXACT' if sum_self == total_delta else 'MISMATCH — parser dropped rows'})")
    print(f"{'self_delta':>15} {'share':>7} {'calls n1':>10} {'calls n2':>10} {'incl_delta':>15}  function")
    shown = 0
    for r in rows:
        share = 100.0 * r[0] / total_delta if total_delta else 0.0
        if abs(share) < a.min_share:
            continue
        print(f"{r[0]:>15,} {share:>6.2f}% {str(r[7]):>10} {str(r[8]):>10} {r[6]:>15,}  {r[1]}")
        shown += 1
        if shown >= a.top:
            break


if __name__ == "__main__":
    main()

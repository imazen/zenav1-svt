#!/usr/bin/env python3
"""List functions that nothing in the product code calls.

Runs on prep.py's mirror tree (test code blanked, line numbers kept), so a
function that only tests call is reported as unreferenced — which is the point:
product code that only a test keeps alive is either dead or a parallel
implementation of something the live path does another way.

With --transitive the scan repeats with dead fn bodies blanked, so helpers that
only dead code calls are reported too (the `round` column says which pass).

A name counts as referenced if it appears as a whole word anywhere other than
its own definition line, in any file of the mirror (method calls, paths, fn
pointers, macro bodies all match). That makes the report conservative: a
common name ("new", "len") shared by an unrelated item hides a dead fn, but a
reported fn really has no textual reference left.

    deadfns.py --src ~/tmp/review/keep/mirror [--min-lines 5] > dead.tsv
"""

import argparse
import re
from collections import defaultdict
from pathlib import Path

# archmage `incant!(name(..), [v3, neon, ...])` calls `name_v3`, `name_neon`, ...
# so a tiered fn is referenced when its stem is.
TIER = re.compile(r"_(?:v1|v2|v3|v4|v4x|neon|scalar|wasm128|avx512|sse2|sse4|avx2)$")
FN = re.compile(r"^\s*(pub(?:\([a-z:]+\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([a-z_][a-z0-9_]*)")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", required=True)
    ap.add_argument("--min-lines", type=int, default=1, help="only report fns at least this long")
    ap.add_argument("--transitive", action="store_true", help="repeat until fns only dead code calls are found too")
    a = ap.parse_args()
    root = Path(a.src)
    files = sorted(root.rglob("*.rs"))
    texts = {f: f.read_text(errors="replace") for f in files}

    def scan(texts):
        defs = []  # (file, line_no, name, length)
        for f, t in texts.items():
            lines = t.split("\n")
            for i, line in enumerate(lines):
                m = FN.match(line)
                if not m:
                    continue
                # Length: until the brace that closes the fn body.
                depth, started, n = 0, False, 0
                for j in range(i, len(lines)):
                    s_ = lines[j]
                    depth += s_.count("{") - s_.count("}")
                    started |= "{" in s_
                    n += 1
                    if started and depth <= 0:
                        break
                    if not started and s_.rstrip().endswith(";"):
                        break  # trait method declaration without body
                defs.append((f, i + 1, m.group(2), n, (m.group(1) or "priv").strip()))
        counts = defaultdict(int)
        names = {d[2] for d in defs} | {TIER.sub("", d[2]) for d in defs}
        word = re.compile(r"\b(" + "|".join(sorted(names, key=len, reverse=True)) + r")\b")
        def_lines = {(d[0], d[1]) for d in defs}
        for f, t in texts.items():
            for i, line in enumerate(t.split("\n")):
                if (f, i + 1) in def_lines:
                    m = FN.match(line)
                    line = line[m.end():] if m else line
                for w in word.findall(line):
                    counts[w] += 1
        dead = [d[:4] + (d[4],) for d in defs
                if counts[d[2]] == 0 and (TIER.sub("", d[2]) == d[2] or counts[TIER.sub("", d[2])] == 0)
                and d[2] != "main"]
        return dead

    # Round 1 = no reference at all. With --transitive, blank the bodies of dead
    # fns and rescan: helpers only a dead fn called become dead too.
    rounds = []
    dead_all = {}
    while True:
        dead = [d for d in scan(texts) if (d[0], d[1]) not in dead_all]
        if not dead:
            break
        rounds.append(len(dead))
        for d in dead:
            dead_all[(d[0], d[1])] = (len(rounds), d)
        if not a.transitive:
            break
        by = defaultdict(list)
        for d in dead:
            by[d[0]].append(d)
        for f, ds in by.items():
            lines = texts[f].split("\n")
            for _, ln, _, n, _ in ds:
                for k in range(ln - 1, min(ln - 1 + n, len(lines))):
                    lines[k] = ""
            texts[f] = "\n".join(lines)
    defs = [v[1] for v in dead_all.values()]
    rounds_of = {(v[1][0], v[1][1]): v[0] for v in dead_all.values()}

    print("file\tline\tfn\tlines\tround\tvis")
    total = 0
    by_file = defaultdict(int)
    for f, ln, name, n, vis in sorted(defs, key=lambda d: (str(d[0]), d[1])):
        if n >= a.min_lines:
            rel = f.relative_to(root)
            print(f"{rel}\t{ln}\t{name}\t{n}\t{rounds_of[(f, ln)]}\t{vis}")
            total += n
            by_file[str(rel)] += n
    import sys
    print(f"# rounds {rounds}; {len(defs)} fns in {len(by_file)} files; "
          f"{total} lines; top files: " +
          ", ".join(f"{k}:{v}" for k, v in sorted(by_file.items(), key=lambda kv: -kv[1])[:8]),
          file=sys.stderr)


if __name__ == "__main__":
    main()

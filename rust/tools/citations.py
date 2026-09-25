#!/usr/bin/env python3
"""Track `file.c:N` / `file.c:N-M` C citations across C oracles.

The Rust port cites the C reference by line (~6,900 citations in Rust source
and in the svtav1-cref shims, which also hold COPIES of static C bodies). Line
numbers go stale on any oracle bump, and a copied body silently keeps testing
against the old C. This tool reads each cited span from the BASE oracle (the
one the code was written against) and looks for the same text in a TARGET
oracle:

  same     the span is unchanged at the same lines
  moved    the identical text sits at other lines (unique match) — remappable
  ambig    the identical text occurs more than once in the target file
  changed  the text is gone from the target file: the C behind this citation
           CHANGED — the Rust mirroring it (or the shim copying it) needs review
  unresolved  the cited file is not in the base oracle (a typo, or a file the
           base does not carry)

    tools/citations.py check --to ghost-robot            # summary + changed list
    tools/citations.py check --to ghost-robot --tsv out.tsv
    tools/citations.py remap --to ghost-robot            # rewrite `moved` line numbers

Oracles are names from rust/oracles/oracles.tsv (`--base` defaults to
hybrid-3115, the source every existing citation was written against). A single
line citation is fingerprinted as that line plus the next two, so a lone `}`
cannot match everywhere; whitespace is normalised. `remap` edits files in
place and only touches `moved` citations, so run it in a clean tree and review
the diff.
"""
import argparse
import collections
import os
import pathlib
import re
import subprocess
import sys

RUST = pathlib.Path(__file__).resolve().parents[1]
SCAN_DIRS = [RUST / "crates", RUST / "svtav1"]
SCAN_EXT = {".rs", ".c", ".h"}
CITE = re.compile(r"\b([A-Za-z_][A-Za-z_0-9]*\.[ch]):(\d+)(?:-(\d+))?\b")
SINGLE_WINDOW = 3


def oracle_src(name: str) -> pathlib.Path:
    out = subprocess.run([str(RUST / "tools/oracle"), "srcdir", name], check=True,
                         capture_output=True, text=True).stdout.strip()
    src = pathlib.Path(out)
    if not (src / "Source").is_dir():
        sys.exit(f"citations: {name} source not materialised at {src} (run tools/oracle build {name})")
    return src


class Oracle:
    def __init__(self, name: str):
        self.name = name
        self.root = oracle_src(name)
        self.by_base = collections.defaultdict(list)
        for p in (self.root / "Source").rglob("*"):
            if p.suffix in (".c", ".h"):
                self.by_base[p.name].append(p)
        self._lines = {}

    def lines(self, path: pathlib.Path):
        if path not in self._lines:
            self._lines[path] = path.read_text(errors="replace").splitlines()
        return self._lines[path]

    def resolve(self, base: str):
        paths = self.by_base.get(base, [])
        if len(paths) > 1:
            # Prefer the scalar library over SIMD/app/test copies of a name.
            lib = [p for p in paths if "/Source/Lib/" in str(p) and "/ASM_" not in str(p)]
            paths = lib or paths
        return paths[0] if len(paths) == 1 else (paths[0] if paths else None)


def norm(lines):
    return tuple(" ".join(l.split()) for l in lines)


def span(n: int, m):
    return (n, m) if m else (n, n + SINGLE_WINDOW - 1)


def find_all(hay, needle):
    k = len(needle)
    if k == 0:
        return []
    first = needle[0]
    return [i for i in range(len(hay) - k + 1) if hay[i] == first and tuple(hay[i:i + k]) == needle]


def scan_files():
    for d in SCAN_DIRS:
        for p in d.rglob("*"):
            if p.suffix in SCAN_EXT and "/target/" not in str(p) and p.is_file():
                yield p


def classify(base: Oracle, target: Oracle, fname: str, n: int, m):
    bp, tp = base.resolve(fname), target.resolve(fname)
    if bp is None:
        return "unresolved", None
    blines = base.lines(bp)
    a, b = span(n, m)
    if a < 1 or b > len(blines) or a > b:
        return "unresolved", None
    text = norm(blines[a - 1:b])
    if tp is None:
        return "changed", None
    tl = norm(target.lines(tp))
    if tuple(tl[a - 1:b]) == text:
        return "same", (a, b)
    hits = find_all(tl, text)
    if len(hits) == 1:
        s = hits[0] + 1
        return "moved", (s, s + (b - a))
    if hits:
        return "ambig", None
    return "changed", None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cmd", choices=["check", "remap"])
    ap.add_argument("--base", default="hybrid-3115")
    ap.add_argument("--to", required=True)
    ap.add_argument("--tsv", help="write every citation's status here")
    args = ap.parse_args()
    base, target = Oracle(args.base), Oracle(args.to)
    counts = collections.Counter()
    by_file = collections.Counter()
    rows = []
    edits = collections.defaultdict(list)
    for path in scan_files():
        text = path.read_text(errors="replace")
        for mo in CITE.finditer(text):
            fname, n, m = mo.group(1), int(mo.group(2)), mo.group(3) and int(mo.group(3))
            status, new = classify(base, target, fname, n, m)
            counts[status] += 1
            line = text.count("\n", 0, mo.start()) + 1
            rel = path.relative_to(RUST)
            rows.append((status, str(rel), line, mo.group(0), f"{new[0]}-{new[1]}" if new and status == "moved" else ""))
            if status == "changed":
                by_file[fname] += 1
            if status == "moved":
                repl = f"{fname}:{new[0]}" + (f"-{new[1]}" if m else "")
                edits[path].append((mo.start(), mo.end(), repl))
    total = sum(counts.values())
    print(f"citations {args.base} -> {args.to}: {total} total")
    for k in ["same", "moved", "ambig", "changed", "unresolved"]:
        print(f"  {k:<10} {counts[k]:>6}  ({100.0 * counts[k] / max(total, 1):.1f}%)")
    if by_file:
        print("changed citations by cited C file (the C behind these moved on):")
        for f, c in by_file.most_common(15):
            print(f"  {c:>5}  {f}")
    if args.tsv:
        with open(args.tsv, "w") as fh:
            fh.write("status\tfile\tline\tcitation\tnew_span\n")
            for r in rows:
                fh.write("\t".join(map(str, r)) + "\n")
        print(f"wrote {len(rows)} rows to {args.tsv}")
    if args.cmd == "remap":
        n = 0
        for path, es in edits.items():
            text = path.read_text()
            for s, e, repl in sorted(es, reverse=True):
                text = text[:s] + repl + text[e:]
                n += 1
            path.write_text(text)
        print(f"remapped {n} moved citations in {len(edits)} files (review the diff)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

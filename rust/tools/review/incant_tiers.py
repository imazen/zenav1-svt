#!/usr/bin/env python3
"""List every archmage `incant!` dispatch site and the SIMD tiers it names.

    tools/review/incant_tiers.py [--root rust/] [--missing]

A site whose tier list lacks both `v3` and `v4` runs its scalar arm on every
x86 machine, however wide its vector unit; one lacking `neon` does the same on
aarch64. `--missing` prints only those sites. Output is TSV
(file, line, kernel, tiers, missing) so it sorts and diffs cleanly.

The parse is textual: it reads the balanced parentheses after `incant!` and
takes the LAST `[...]` group as the tier list, which is how every call in this
tree is written. A site it cannot parse is reported with tiers `?` rather than
skipped, so a change in call shape shows up instead of silently vanishing.
"""
import argparse
import pathlib
import re
import sys


def strip_comments(text):
    # Blank `//` comments (doc comments quote `incant!(...)` in prose) while
    # keeping every newline, so line numbers still match the source.
    return re.sub(r"//[^\n]*", lambda m: " " * len(m.group()), text)


def sites(text):
    text = strip_comments(text)
    for m in re.finditer(r"\bincant!\s*\(", text):
        depth, i = 1, m.end()
        while i < len(text) and depth:
            depth += {"(": 1, ")": -1}.get(text[i], 0)
            i += 1
        body = text[m.end():i - 1]
        line = text.count("\n", 0, m.start()) + 1
        kernel = re.match(r"\s*([A-Za-z_][\w:]*)", body)
        groups = re.findall(r"\[([^\[\]]*)\]\s*,?\s*$", body)
        tiers = [t.strip() for t in groups[-1].split(",") if t.strip()] if groups else None
        yield line, kernel.group(1) if kernel else "?", tiers


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=str(pathlib.Path(__file__).resolve().parents[2]))
    ap.add_argument("--missing", action="store_true", help="only sites missing an x86 or neon tier")
    a = ap.parse_args()
    root = pathlib.Path(a.root)
    rows = []
    for path in sorted(root.glob("crates/*/src/**/*.rs")):
        text = path.read_text(encoding="utf-8")
        for line, kernel, tiers in sites(text):
            if tiers is None:
                missing = "unparsed"
            else:
                gaps = []
                if not {"v3", "v4"} & set(tiers):
                    gaps.append("x86")
                if not any(t.startswith("neon") for t in tiers):
                    gaps.append("neon")
                missing = ",".join(gaps)
            if a.missing and not missing:
                continue
            rows.append((str(path.relative_to(root)), line, kernel,
                         ",".join(tiers) if tiers else "?", missing))
    w = sys.stdout.write
    w("file\tline\tkernel\ttiers\tmissing\n")
    for r in rows:
        w("\t".join(map(str, r)) + "\n")
    total = sum(1 for p in root.glob("crates/*/src/**/*.rs")
                for _ in sites(p.read_text(encoding="utf-8")))
    sys.stderr.write(f"{len(rows)} of {total} incant! sites listed\n")


if __name__ == "__main__":
    main()

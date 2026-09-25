#!/usr/bin/env python3
"""Remove `use super::*;` lines that rustc reports unused.

    cargo check --workspace --all-targets --message-format short 2>&1 | tools/drop_unused_super_glob.py

A child made by tools/move_items.py always starts with `use super::*;`, and a
child that needs nothing from its parent then warns. This reads the warnings
on stdin and deletes exactly the reported lines; nothing else is touched.
"""
import re
import sys

hits = set()
for line in sys.stdin:
    m = re.match(r"^(\S+\.rs):(\d+):\d+: warning: unused import: `super::\*`", line)
    if m:
        hits.add((m.group(1), int(m.group(2))))
by_file = {}
for f, n in hits:
    by_file.setdefault(f, []).append(n)
for f, ns in by_file.items():
    lines = open(f).read().split("\n")
    for n in sorted(ns, reverse=True):
        if lines[n - 1].strip() == "use super::*;":
            del lines[n - 1]
            if n - 1 < len(lines) and lines[n - 1] == "":
                del lines[n - 1]
    open(f, "w").write("\n".join(lines))
    print(f"{f}: removed {len(ns)}")

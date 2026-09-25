#!/usr/bin/env python3
"""Fail if any Rust source file is over the size limit (default 3,000 lines).

    python3 tools/file_size_check.py [--max N]

The target is 2-3 kloc per file (user, 2026-09-25). A file past it gets split
along real seams, as pure moves that leave the output pins unchanged:
tools/split_inline_mod.py for inline `mod x { .. }` blocks,
tools/move_items.py for runs of top-level items, tools/move_methods.py for
methods of a big `impl`, and tools/ra_extract.py (rust-analyzer's "Extract into
function") to cut a long function into stages. Generated tables are split by
their generators (xtask/transcribe_qm.py, cref's gen_default_cdfs), not by hand.
"""
import argparse
import pathlib
import sys

RUST = pathlib.Path(__file__).resolve().parents[1]
ROOTS = ["crates", "svtav1", "tools", "xtask"]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--max", type=int, default=3000)
    args = ap.parse_args()
    over = []
    for root in ROOTS:
        for p in (RUST / root).rglob("*.rs"):
            if "/target/" in str(p):
                continue
            n = sum(1 for _ in p.open(errors="replace"))
            if n > args.max:
                over.append((n, p.relative_to(RUST)))
    if over:
        print(f"file_size_check: {len(over)} file(s) over {args.max} lines:")
        for n, p in sorted(over, reverse=True):
            print(f"  {n:6}  {p}")
        print("Split along real seams with the tools named in this script's docstring.")
        return 1
    print(f"file_size_check: every .rs file is at most {args.max} lines")
    return 0


if __name__ == "__main__":
    sys.exit(main())

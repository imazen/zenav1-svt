#!/usr/bin/env python3
"""Move an inline top-level `mod NAME { ... }` out of a Rust file, unchanged.

    tools/split_inline_mod.py crates/svtav1-encoder/src/pipeline.rs tests

replaces the block with `mod NAME;` (attributes and doc comments above it stay
in the parent) and writes the body to `<file-stem>/NAME.rs` — or `NAME.rs`
beside the file when the parent is `mod.rs` / `lib.rs` / `main.rs`. It is a
pure move: the body is copied verbatim and then run through rustfmt, which
re-indents code but never touches the contents of string literals (a hand
dedent would). Refuses if the target file exists or the module is not a
top-level inline block.

This is how files are brought under the 2-3 kloc target without editing
logic; the output pins must be unchanged after it (`just pins`).
"""
import pathlib
import re
import subprocess
import sys


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    path, name = pathlib.Path(sys.argv[1]), sys.argv[2]
    lines = path.read_text().split("\n")
    head = re.compile(r"^((?:pub(?:\([^)]*\))? )?mod " + re.escape(name) + r") \{\s*$")
    starts = [i for i, l in enumerate(lines) if head.match(l)]
    if len(starts) != 1:
        sys.exit(f"split_inline_mod: expected one top-level `mod {name} {{` in {path}, found {len(starts)}")
    a = starts[0]
    b = next((j for j in range(a + 1, len(lines)) if lines[j] == "}"), None)
    if b is None:
        sys.exit(f"split_inline_mod: no column-0 closing brace for mod {name}")
    if path.stem in ("mod", "lib", "main"):
        out = path.parent / f"{name}.rs"
    else:
        out = path.parent / path.stem / f"{name}.rs"
    if out.exists():
        sys.exit(f"split_inline_mod: {out} already exists")
    out.parent.mkdir(exist_ok=True)
    out.write_text("\n".join(lines[a + 1:b]).rstrip("\n") + "\n")
    lines[a:b + 1] = [head.match(lines[a]).group(1) + ";"]
    path.write_text("\n".join(lines))
    subprocess.run(["rustfmt", "--edition", "2024", str(out)], check=True)
    print(f"moved mod {name}: {b - a - 1} lines -> {out}")


if __name__ == "__main__":
    main()

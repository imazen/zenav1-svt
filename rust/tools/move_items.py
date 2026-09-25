#!/usr/bin/env python3
"""Move a contiguous run of top-level items from a Rust file into a child module.

    tools/move_items.py <file.rs> <child> <first_item_regex> <end_item_regex>

The regexes each match exactly one line (searched with `re.match`, so they
anchor at column 0): the first item to move, and the first item to KEEP
(`EOF` moves through the end of the file). Doc comments and attributes move
with the item they sit on. Matching on signatures rather than line numbers
means a sequence of moves never depends on arithmetic from an earlier one.

The run is written to `<file-stem>/<child>.rs` headed by `use super::*;`, and
the parent gains `mod <child>;` + `pub use <child>::*;` (narrowed to
`pub(crate)` or private to match what the child holds) at the END of the file (so
every `macro_rules!` the parent defines is already in textual scope for the
child). Privacy is the only thing edited, because moving code into a child
module makes its private items private to the child:

  - top-level items with no visibility get `pub(super)`;
  - fields of moved structs with no visibility get `pub(super)`;
  - `fn`s inside moved `impl` blocks with no visibility get `pub(super)`.

`pub(super)` from the child is exactly "visible to the parent and its other
children", which is what the item had before the move. Nothing else changes,
so the output pins must stay byte-identical (`just pins`).
"""
import pathlib
import re
import subprocess
import sys

ITEM = re.compile(r"^(pub(\([^)]*\))? )?(const |async |unsafe )*(fn|struct|enum|impl|const|static|type|trait|union|use|mod|macro_rules!)\b")
VIS = re.compile(r"^\s*pub(\([^)]*\))?\s")


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    path, child = pathlib.Path(sys.argv[1]), sys.argv[2]
    lines = path.read_text().split("\n")

    def find(rx):
        hits = [i for i, l in enumerate(lines) if re.match(rx, l)]
        if len(hits) != 1:
            sys.exit(f"move_items: {rx!r} matches {len(hits)} lines, need exactly 1")
        return hits[0]

    first = find(sys.argv[3])
    end = len(lines) if sys.argv[4] == "EOF" else find(sys.argv[4])
    if end <= first:
        sys.exit("move_items: end item is not after the first item")
    if not ITEM.match(lines[first]):
        sys.exit(f"move_items: line {first + 1} is not a top-level item: {lines[first]!r}")
    if end < len(lines) and lines[end].strip() and not ITEM.match(lines[end]) and not lines[end].startswith(("///", "#[", "//")):
        sys.exit(f"move_items: end line {end + 1} is not an item boundary: {lines[end]!r}")
    start = first
    while start > 0 and lines[start - 1].startswith(("///", "#[", "//")):
        start -= 1
    stop = end
    while stop > start and lines[stop - 1].startswith(("///", "#[")):
        stop -= 1  # docs/attrs of the item at `end` stay with it
    body = lines[start:stop]

    out_lines = []
    in_struct = in_impl = False
    for l in body:
        if ITEM.match(l) and not VIS.match(l) and not l.startswith(("impl", "use ", "mod ", "macro_rules!")):
            l = "pub(super) " + l
        if re.match(r"^(pub(\([^)]*\))? )?struct \w+.*\{\s*$", l):
            in_struct = True
        elif re.match(r"^impl\b", l):
            in_impl = True
        elif l.startswith("}"):
            in_struct = in_impl = False
        elif in_struct and re.match(r"^    [a-z_][a-z0-9_]*\s*:", l):
            l = "    pub(super) " + l[4:]
        elif in_impl and re.match(r"^    (const |async |unsafe )*fn\b", l):
            l = "    pub(super) " + l[4:]
        out_lines.append(l)

    out = path.parent / (path.stem if path.stem not in ("mod", "lib", "main") else "") / f"{child}.rs"
    out = pathlib.Path(str(out).replace("//", "/"))
    if out.exists():
        sys.exit(f"move_items: {out} already exists")
    out.parent.mkdir(exist_ok=True)
    out.write_text("use super::*;\n\n" + "\n".join(out_lines).strip("\n") + "\n")
    rest = lines[:start] + lines[stop:]
    while rest and rest[-1] == "":
        rest.pop()
    # `pub use` only when the child has a `pub` item, else rustc warns that
    # the glob re-exports nothing publicly.
    if any(re.match(r"^pub (?!\()", l) for l in out_lines):
        vis = "pub "
    elif any(l.startswith("pub(crate) ") for l in out_lines):
        vis = "pub(crate) "
    else:
        vis = ""
    rest += ["", f"mod {child};", f"{vis}use {child}::*;", ""]
    path.write_text("\n".join(rest))
    subprocess.run(["rustfmt", "--edition", "2024", str(out)], check=True)
    print(f"moved lines {start + 1}..{stop} ({stop - start}) -> {out}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Move a contiguous run of top-level items from a Rust file into a child module.

    tools/move_items.py <file.rs> <child> <first_item_regex> <end_item_regex>
    tools/move_items.py <file.rs> <child> @<first_line> @<end_line>

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

`mod x;` declarations and `use` imports inside the run stay in the parent
(with their attributes): moving a `mod` would orphan its file, and children
see the parent's imports through `use super::*` anyway.

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

    # `@N` gives a 1-based line directly (e.g. a section banner). Safe when
    # applied bottom-up: a move only edits lines at and after its own range.
    by_line = sys.argv[3].startswith("@")
    first = int(sys.argv[3][1:]) - 1 if by_line else find(sys.argv[3])
    if sys.argv[4] == "EOF":
        end = len(lines)
    elif sys.argv[4].startswith("@"):
        end = int(sys.argv[4][1:]) - 1
    else:
        end = find(sys.argv[4])
    if end <= first:
        sys.exit("move_items: end item is not after the first item")
    if not by_line and not ITEM.match(lines[first]):
        sys.exit(f"move_items: line {first + 1} is not a top-level item: {lines[first]!r}")
    if end < len(lines) and lines[end].strip() and not ITEM.match(lines[end]) and not lines[end].startswith(("///", "#[", "//")):
        sys.exit(f"move_items: end line {end + 1} is not an item boundary: {lines[end]!r}")
    start = first
    while not by_line and start > 0 and lines[start - 1].startswith(("///", "#[", "//")):
        start -= 1
    stop = end
    while not sys.argv[4].startswith("@") and stop > start and lines[stop - 1].startswith(("///", "#[", "//")):
        stop -= 1  # docs, attrs and comments of the item at `end` stay with it
    body = lines[start:stop]
    # `mod x;` declarations and `use` imports stay in the parent: moving a
    # `mod` would change the submodule's path and orphan its file, and a moved
    # `use` would vanish from the parent (children see the parent's imports
    # through `use super::*`). Their attributes stay with them.
    kept_mods, filtered = [], []
    for l in body:
        if re.match(r"^(pub(\([^)]*\))? )?(mod \w+;|use )", l):
            attrs = []
            while filtered and filtered[-1].startswith(("#[", "///", "//")):
                attrs.insert(0, filtered.pop())
            kept_mods += attrs + [l]
        else:
            filtered.append(l)
    body = filtered

    out_lines = []
    in_struct = in_impl = in_extern = False
    for l in body:
        if re.match(r'^(unsafe )?extern "C" \{', l):
            in_extern = True
        elif in_extern and l.startswith("}"):
            in_extern = False
        elif in_extern and re.match(r"^    (safe |unsafe )?(fn|static)\b", l):
            l = "    pub(super) " + l[4:]
        if ITEM.match(l) and not VIS.match(l) and not l.startswith(("impl", "use ", "mod ", "macro_rules!")):
            l = "pub(super) " + l
        if re.match(r"^(pub(\([^)]*\))? )?struct \w+.*\{\s*$", l):
            in_struct = True
        elif re.match(r"^impl\b", l):
            # Only inherent impls: a trait impl's methods take no visibility.
            in_impl = not re.search(r"\bfor\b", l.split("{")[0])
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
    # Format BEFORE touching the parent: if the child does not parse, nothing
    # has been removed from the parent yet.
    if subprocess.run(["rustfmt", "--edition", "2024", str(out)]).returncode != 0:
        out.unlink()
        sys.exit(f"move_items: {out} does not parse; parent left unchanged")
    rest = lines[:start] + kept_mods + lines[stop:]
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
    print(f"moved lines {start + 1}..{stop} ({stop - start}) -> {out}")


if __name__ == "__main__":
    main()

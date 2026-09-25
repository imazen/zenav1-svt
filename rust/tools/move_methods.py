#!/usr/bin/env python3
"""Move named methods out of a big `impl` block into a child module's own impl.

    tools/move_methods.py <file.rs> '<impl header regex>' <child> <method>...

e.g. tools/move_methods.py crates/svtav1-encoder/src/pipeline.rs \
         '^impl EncodePipeline \\{$' ra run_picture_decision encode_ra_window

Each method (found at 4-space indent, with the doc comments and attributes
above it) moves verbatim, in source order, into `<file-stem>/<child>.rs` as

    use super::*;

    impl <Type> { ...methods... }

and the parent gains `mod <child>;` at the end of the file. A method in a
child module's impl keeps full access to the type's private fields (privacy
follows the module tree), so the only edit is visibility: a method with no
`pub`/`pub(..)` becomes `pub(super)` — the reach it had before, the parent
and its children. Nothing else changes; the output pins must be identical.
"""
import pathlib
import re
import subprocess
import sys


def main():
    if len(sys.argv) < 5:
        sys.exit(__doc__)
    path, head_rx, child, names = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4:]
    lines = path.read_text().split("\n")
    heads = [i for i, l in enumerate(lines) if re.match(head_rx, l)]
    if len(heads) != 1:
        sys.exit(f"move_methods: impl header {head_rx!r} matches {len(heads)} lines")
    h = heads[0]
    close = next(j for j in range(h + 1, len(lines)) if lines[j] == "}")
    header = lines[h].rstrip()

    spans = []
    for name in names:
        rx = re.compile(r"^    (pub(\([^)]*\))? )?(const )?fn " + re.escape(name) + r"\b")
        hits = [j for j in range(h + 1, close) if rx.match(lines[j])]
        if len(hits) != 1:
            sys.exit(f"move_methods: method {name!r} found {len(hits)} times in the impl")
        s = hits[0]
        e = next((j for j in range(s, close) if lines[j] == "    }"), None)
        if e is None:
            sys.exit(f"move_methods: no end for {name}")
        while lines[s - 1].startswith(("    ///", "    #[")):
            s -= 1
        spans.append((s, e + 1, name))
    spans.sort()
    for (a, b, n), (c, _, m) in zip(spans, spans[1:]):
        if c < b:
            sys.exit(f"move_methods: {n} and {m} overlap")

    body = []
    for s, e, _ in spans:
        chunk = lines[s:e]
        chunk = [
            ("    pub(super) " + l[4:]) if re.match(r"^    (const )?fn ", l) else l
            for l in chunk
        ]
        body += chunk + [""]
    out = path.parent / (path.stem if path.stem not in ("mod", "lib", "main") else "") / f"{child}.rs"
    out = pathlib.Path(str(out).replace("//", "/"))
    if out.exists():
        sys.exit(f"move_methods: {out} already exists")
    out.parent.mkdir(exist_ok=True)
    out.write_text("use super::*;\n\n" + header + "\n" + "\n".join(body).rstrip("\n") + "\n}\n")

    keep = []
    cut = set()
    for s, e, _ in spans:
        cut.update(range(s, e))
        # swallow one blank line after each removed method
        if e < len(lines) and lines[e] == "" and e not in cut:
            cut.add(e)
    keep = [l for i, l in enumerate(lines) if i not in cut]
    while keep and keep[-1] == "":
        keep.pop()
    keep += ["", f"mod {child};", ""]
    path.write_text("\n".join(keep))
    subprocess.run(["rustfmt", "--edition", "2024", str(out)], check=True)
    print(f"moved {len(spans)} methods ({sum(e - s for s, e, _ in spans)} lines) -> {out}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Prepare the product Rust source for review: strip test code, shrink it for reading.

Uses tree-sitter (a real parser, not regexes), so a brace inside a string or a
comment cannot end a test module early. Needs the review venv:

    uv venv ~/.local/share/venvs/review
    VIRTUAL_ENV=~/.local/share/venvs/review uv pip install tree-sitter tree-sitter-rust

What counts as test code (removed):
  * any item carrying `#[cfg(test)]`, `#[cfg(all(test, ..))]`, `#[test]`, or `#[bench]`,
    together with its attributes and the doc comments directly above it;
  * whole files that are test-only: a file-level `#![cfg(test)]`, or a file that
    a parent pulls in with `#[cfg(test)] mod x;`;
  * `tests/`, `benches/` and `examples/` directories (never walked).
`#[cfg(any(test, ...))]` is NOT removed: that code also builds outside tests.

Outputs, under --out:
  mirror/   the same tree with test code blanked OUT BUT LINE NUMBERS KEPT, so
            jscpd / lizard / rust-code-analysis findings point at real source lines.
  bundle/   compact reading copies, one file per crate area, split into chunks of
            about --chunk-tokens. Large literal tables are replaced by a one-line
            stub. With --comments=drop, comments are removed too. Every gap is
            marked `//@L<line>` (the path is in the `//// FILE` header above it) so a finding in a bundle maps back to source.
  manifest.tsv  per file: original lines, kept lines, removed test lines,
            elided table lines, bundle tokens (approx, bytes/4).
"""

import argparse
import os
import re
import sys
from pathlib import Path

import tree_sitter_rust
from tree_sitter import Language, Parser

RUST = Language(tree_sitter_rust.language())
PARSER = Parser(RUST)

TEST_ATTR = re.compile(r"^#\s*\[\s*(?:test|bench|cfg\s*\(\s*(?:test|all\s*\(\s*test\b)[^\]]*\))\s*\]$", re.S)
PATH_ATTR = re.compile(r'^#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]$')
INNER_TEST_ATTR = re.compile(r"^#!\s*\[\s*cfg\s*\(\s*(?:test|all\s*\(\s*test\b)", re.S)
LITERALS = {"integer_literal", "float_literal", "boolean_literal", "char_literal",
            "string_content", "string_literal", "negative_literal"}
# Nodes whose presence means an array literal is code, not a data table.
CODE_NODES = {
    "call_expression", "closure_expression", "block", "macro_invocation",
    "if_expression", "match_expression", "method_call_expression", "loop_expression",
    "for_expression", "while_expression",
}


def text(src, node):
    return src[node.start_byte:node.end_byte].decode("utf8", "replace")


def is_comment(node):
    return node.type in ("line_comment", "block_comment")


def test_ranges(src, tree):
    """Byte ranges of test-only items, and names of `#[cfg(test)] mod x;` children."""
    ranges, test_mods = [], []

    def walk(node):
        kids = node.children
        i = 0
        while i < len(kids):
            k = kids[i]
            if k.type == "attribute_item" and TEST_ATTR.match(text(src, k)):
                # Extend back over doc comments / other attributes directly above.
                start = i
                while start > 0 and (kids[start - 1].type == "attribute_item" or is_comment(kids[start - 1])):
                    if is_comment(kids[start - 1]) and not text(src, kids[start - 1]).startswith("///"):
                        break
                    start -= 1
                # Forward to the item the attribute decorates.
                j = i + 1
                while j < len(kids) and (kids[j].type == "attribute_item" or is_comment(kids[j])):
                    j += 1
                if j < len(kids):
                    item = kids[j]
                    if item.type == "mod_item" and item.child_by_field_name("body") is None:
                        path_attr = None
                        for a in kids[start:j]:
                            m = PATH_ATTR.match(text(src, a))
                            if m:
                                path_attr = m.group(1)
                        name = item.child_by_field_name("name")
                        if path_attr is not None:
                            test_mods.append(("path", path_attr))
                        elif name is not None:
                            test_mods.append(text(src, name))
                    ranges.append((kids[start].start_byte, item.end_byte))
                    i = j + 1
                    continue
            if k.type in ("mod_item", "impl_item", "trait_item"):
                for c in k.children:
                    if c.type == "declaration_list":
                        walk(c)
            i += 1

    walk(tree.root_node)
    return ranges, test_mods


def file_is_test(src, tree):
    for k in tree.root_node.children:
        if k.type == "inner_attribute_item" and INNER_TEST_ATTR.match(text(src, k)):
            return True
        if k.type not in ("inner_attribute_item", "line_comment", "block_comment"):
            break
    return False


def table_ranges(src, tree, min_bytes):
    """Byte ranges of large literal array expressions (data tables)."""
    out = []

    def is_table(node):
        # A table is literals all the way down: at least 90% of the named leaves
        # are literals, and nothing computes. An array of variables is code.
        stack, leaves, lits = [node], 0, 0
        while stack:
            n = stack.pop()
            if n.type in CODE_NODES:
                return False
            if n.named_child_count == 0 and n.is_named:
                leaves += 1
                lits += n.type in LITERALS
            stack.extend(n.children)
        return leaves > 0 and lits >= 0.9 * leaves

    def walk(node):
        if node.type == "array_expression" and node.end_byte - node.start_byte >= min_bytes:
            if is_table(node):
                out.append((node.start_byte, node.end_byte, node.named_child_count))
                return
        for c in node.children:
            walk(c)

    walk(tree.root_node)
    return out


def comment_ranges(src, tree):
    out = []

    def walk(node):
        if is_comment(node):
            out.append((node.start_byte, node.end_byte))
            return
        for c in node.children:
            walk(c)

    walk(tree.root_node)
    return out


def merge(ranges):
    ranges = sorted(ranges)
    out = []
    for a, b in ranges:
        if out and a <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        else:
            out.append((a, b))
    return out


def blank_bytes(src, ranges):
    """Replace ranges with spaces, keeping newlines (line numbers survive)."""
    buf = bytearray(src)
    for a, b in ranges:
        for p in range(a, b):
            if buf[p] != 0x0A:
                buf[p] = 0x20
    return bytes(buf)


def resolve_child_mod(path, name):
    """Where `mod name;` in `path` lives (edition-2018 rules; ("path", p) for #[path])."""
    if isinstance(name, tuple):
        cand = path.parent / name[1]
        return cand if cand.exists() else None
    if path.name in ("mod.rs", "lib.rs", "main.rs"):
        base = path.parent
    else:
        base = path.parent / path.stem
    for cand in (base / f"{name}.rs", base / name / "mod.rs"):
        if cand.exists():
            return cand
    return None


def bundle_lines(rel, src_text, removed_lines, tables, code_only_text=None):
    """Compact lines, with `//@L<n>` markers wherever source lines are skipped.

    With code_only_text (the source with comment bytes blanked), comments are
    removed inside lines too, and a line is dropped only when no code is left
    on it: `/*name=*/ None,` keeps its `None,`.
    """
    lines = src_text.split("\n")
    code_lines = code_only_text.split("\n") if code_only_text is not None else None
    table_by_start = {s: (e, n) for s, e, n in tables}
    out = [f"//// FILE {rel}"]
    expect = 1
    ln = 1
    total = len(lines)
    while ln <= total:
        if ln in removed_lines:
            ln += 1
            continue
        if ln in table_by_start:
            end, n = table_by_start[ln]
            head = (code_lines or lines)[ln - 1].rstrip()
            if ln != expect:
                out.append(f"//@L{ln}")
            out.append(f"{head}  /* ... {n}-entry table elided, lines {ln}-{end} */")
            ln = end + 1
            expect = -1
            continue
        line = lines[ln - 1].rstrip()
        if code_lines is not None:
            code = code_lines[ln - 1].rstrip()
            if line.strip() and not code.strip():
                ln += 1  # comment-only line
                continue
            line = code
        if not line.strip() and (not out or not out[-1].strip()):
            ln += 1
            continue
        if ln != expect:
            out.append(f"//@L{ln}")
        out.append(line)
        expect = ln + 1
        ln += 1
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=str(Path(__file__).resolve().parents[2]), help="rust/ workspace root")
    ap.add_argument("--out", required=True)
    ap.add_argument("--src", action="append", default=None,
                    help="src dirs relative to root (default: every crate's src except svtav1-cref)")
    ap.add_argument("--comments", choices=["keep", "drop"], default="keep")
    ap.add_argument("--table-bytes", type=int, default=600, help="elide literal arrays at least this large")
    ap.add_argument("--chunk-tokens", type=int, default=60000)
    args = ap.parse_args()

    root = Path(args.root)
    out = Path(args.out)
    srcs = args.src or sorted(
        str(p.relative_to(root)) for p in list(root.glob("crates/*/src")) + [root / "svtav1/src"]
        if p.is_dir() and "svtav1-cref" not in str(p)
    )

    files = []
    for s in srcs:
        files.extend(sorted((root / s).rglob("*.rs")))
    files = [f for f in files if not any(part in ("tests", "benches", "examples", "bin") for part in f.relative_to(root).parts[3:])]

    parsed = {}
    test_files = set()
    for f in files:
        src = f.read_bytes()
        tree = PARSER.parse(src)
        parsed[f] = (src, tree)
        if file_is_test(src, tree):
            test_files.add(f)
    # Files pulled in by `#[cfg(test)] mod x;` (transitively).
    changed = True
    tr_cache = {}
    while changed:
        changed = False
        for f, (src, tree) in parsed.items():
            if f not in tr_cache:
                tr_cache[f] = test_ranges(src, tree)
            for name in tr_cache[f][1]:
                child = resolve_child_mod(f, name)
                if child is not None and child in parsed and child not in test_files:
                    test_files.add(child)
                    changed = True
        # Children of test files are test files.
        for f in list(test_files):
            src, tree = parsed[f]
            for k in tree.root_node.children:
                if k.type == "mod_item" and k.child_by_field_name("body") is None:
                    child = resolve_child_mod(f, text(src, k.child_by_field_name("name")))
                    if child is not None and child in parsed and child not in test_files:
                        test_files.add(child)
                        changed = True

    (out / "mirror").mkdir(parents=True, exist_ok=True)
    (out / "bundle").mkdir(parents=True, exist_ok=True)
    for old in (out / "bundle").glob("*.rs"):
        old.unlink()

    manifest = ["path\tlines\tkept\ttest_lines\ttable_lines\tbundle_tokens"]
    groups = {}
    for f, (src, tree) in parsed.items():
        rel = str(f.relative_to(root))
        nlines = src.count(b"\n") + 1
        mirror = out / "mirror" / rel
        mirror.parent.mkdir(parents=True, exist_ok=True)
        if f in test_files:
            mirror.write_bytes(b"\n" * (nlines - 1))
            manifest.append(f"{rel}\t{nlines}\t0\t{nlines}\t0\t0")
            continue
        tranges = merge(tr_cache[f][0])
        blanked = blank_bytes(src, tranges)
        mirror.write_bytes(blanked)

        def line_of(b):
            return src.count(b"\n", 0, b) + 1

        removed = set()
        for a, b in tranges:
            removed.update(range(line_of(a), line_of(b) + 1))
        tables = []
        for a, b, n in table_ranges(src, tree, args.table_bytes):
            la, lb = line_of(a), line_of(b)
            if la in removed or lb - la < 3:
                continue
            tables.append((la, lb, n))
        code_only = None
        if args.comments == "drop":
            code_only = blank_bytes(src, comment_ranges(src, tree)).decode("utf8", "replace")
        lines = bundle_lines(rel, src.decode("utf8", "replace"), removed, tables, code_only)
        body = "\n".join(lines) + "\n"
        tok = len(body.encode()) // 4
        table_lines = sum(b - a + 1 for a, b, _ in tables)
        manifest.append(f"{rel}\t{nlines}\t{len(lines) - 1}\t{len(removed)}\t{table_lines}\t{tok}")
        parts = Path(rel).parts
        # Group: crate, plus the first src subdirectory (or the file) for big crates.
        key = parts[1] if parts[0] == "crates" else parts[0]
        groups.setdefault(key, []).append((rel, body, tok))

    total = 0
    for key, items in sorted(groups.items()):
        chunk, ctok, idx = [], 0, 0
        for rel, body, tok in sorted(items):
            if chunk and ctok + tok > args.chunk_tokens:
                (out / "bundle" / f"{key}.{idx:02}.rs").write_text("".join(chunk))
                chunk, ctok, idx = [], 0, idx + 1
            chunk.append(body)
            ctok += tok
        if chunk:
            (out / "bundle" / f"{key}.{idx:02}.rs").write_text("".join(chunk))
        total += sum(t for _, _, t in items)
    (out / "manifest.tsv").write_text("\n".join(manifest) + "\n")
    tl = sum(int(r.split("\t")[3]) for r in manifest[1:])
    el = sum(int(r.split("\t")[4]) for r in manifest[1:])
    print(f"{len(parsed)} files, {len(test_files)} test-only files, {tl} test lines removed, "
          f"{el} table lines elided, bundle ~{total // 1000}k tokens -> {out}", file=sys.stderr)


if __name__ == "__main__":
    main()

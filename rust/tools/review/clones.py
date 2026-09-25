#!/usr/bin/env python3
"""Find near-duplicate Rust code: copies that differ only in names, literals and types.

jscpd finds exact copies. The port's typical duplication is an 8-bit function
copied into a 10-bit twin (`u8` -> `u16`, `_hbd` suffixes, different constants),
which jscpd cannot see. This tokenizes with tree-sitter and normalizes every
identifier to `I`, every type name to `T` and every literal to `L`, then finds
runs of at least --window identical normalized tokens in two places.

Run it on prep.py's mirror tree so test code is excluded and line numbers are real:

    clones.py --src ~/tmp/review/keep/mirror --out ~/tmp/review/clones

Writes clones.tsv (one row per clone pair, longest first) and file_pairs.tsv
(cloned lines summed per pair of files, which surfaces whole-module twins).
Long literal tables are skipped (a window that is mostly `L ,` is not code).
"""

import argparse
from collections import defaultdict
from pathlib import Path

import tree_sitter_c
import tree_sitter_rust
from tree_sitter import Language, Parser

PARSERS = {
    "rust": (Parser(Language(tree_sitter_rust.language())), ("*.rs",)),
    "c": (Parser(Language(tree_sitter_c.language())), ("*.c", "*.h")),
}
IDENT = {"identifier", "field_identifier", "shorthand_field_identifier", "label", "metavariable",
         "statement_identifier"}
TYPE = {"type_identifier", "primitive_type", "sized_type_specifier"}
LIT = {"integer_literal", "float_literal", "string_content", "char_literal", "boolean_literal",
       "escape_sequence", "raw_string_literal", "number_literal", "true", "false", "null",
       "string_literal", "concatenated_string"}
SKIP = {"line_comment", "block_comment", "doc_comment", "inner_doc_comment_marker",
        "outer_doc_comment_marker", "comment", "attribute_item", "inner_attribute_item",
        "preproc_include"}


def tokens(parser, src):
    """(normalized_token, raw_text, line) for each leaf; comments/attributes dropped."""
    out = []
    tree = parser.parse(src)
    stack = [tree.root_node]
    while stack:
        n = stack.pop()
        if n.type in SKIP:
            continue
        raw = src[n.start_byte:n.end_byte]
        if n.type in LIT:
            out.append(("L", raw, n.start_point[0] + 1))
            continue
        if n.child_count == 0:
            t = n.type
            if t in IDENT:
                tok = "I"
            elif t in TYPE:
                tok = "T"
            else:
                tok = t
            out.append((tok, raw, n.start_point[0] + 1))
            continue
        stack.extend(reversed(n.children))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--src", required=True, action="append", help="repeatable")
    ap.add_argument("--lang", choices=sorted(PARSERS), default="rust")
    ap.add_argument("--out", required=True)
    ap.add_argument("--window", type=int, default=70, help="minimum clone length in tokens")
    ap.add_argument("--min-lines", type=int, default=10)
    ap.add_argument("--max-bucket", type=int, default=24, help="ignore windows seen more often (boilerplate)")
    a = ap.parse_args()
    parser, globs = PARSERS[a.lang]
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    files, toks, raws, lines, fid, nonblank = [], [], [], [], [], []
    for root in map(Path, a.src):
        paths = sorted(p for g in globs for p in root.rglob(g))
        for f in paths:
            src = f.read_bytes()
            t = tokens(parser, src)
            if not t:
                continue
            files.append(str(f.relative_to(root.parent)) if len(a.src) > 1 else str(f.relative_to(root)))
            nonblank.append(len({ln for _, _, ln in t}))
            for tok, raw, ln in t:
                toks.append(tok)
                raws.append(raw)
                lines.append(ln)
                fid.append(len(files) - 1)
    ids = {}
    seq = [ids.setdefault(t, len(ids)) for t in toks]
    lit, comma = ids.get("L", -1), ids.get(",", -1)
    W = a.window
    n = len(seq)

    # Rolling hash of each window that stays inside one file and is not a literal table.
    B, M = 1_000_003, (1 << 61) - 1
    pw = pow(B, W, M)
    buckets = defaultdict(list)
    h = 0
    litcount = 0
    for i in range(n):
        h = (h * B + seq[i] + 1) % M
        litcount += seq[i] in (lit, comma)
        if i >= W:
            h = (h - (seq[i - W] + 1) * pw) % M
            litcount -= seq[i - W] in (lit, comma)
        if i >= W - 1:
            s = i - W + 1
            if fid[s] == fid[i] and litcount < 0.6 * W:
                buckets[h].append(s)

    # Seeds on the same diagonal (j - i) with overlapping windows form one clone.
    diag = defaultdict(list)
    for pos in buckets.values():
        if len(pos) < 2 or len(pos) > a.max_bucket:
            continue
        for x in range(len(pos)):
            for y in range(x + 1, len(pos)):
                i, j = pos[x], pos[y]
                if j - i < W:  # self-overlapping repetition (unrolled code)
                    continue
                diag[j - i].append(i)
    clones = []
    for d, starts in diag.items():
        starts.sort()
        run_s = prev = starts[0]
        for s in starts[1:] + [None]:
            if s is not None and s <= prev + W:
                prev = s
                continue
            i0, i1 = run_s, prev + W - 1
            j0, j1 = i0 + d, i1 + d
            if fid[i0] == fid[i1] and fid[j0] == fid[j1]:
                la = (lines[i0], lines[i1])
                lb = (lines[j0], lines[j1])
                nl = min(la[1] - la[0], lb[1] - lb[0]) + 1
                # Same file with overlapping line ranges is unrolled repetition
                # (transform butterflies), not a copied function.
                overlap = fid[i0] == fid[j0] and lb[0] <= la[1]
                if nl >= a.min_lines and not overlap:
                    exact = raws[i0:i1 + 1] == raws[j0:j1 + 1]
                    clones.append((nl, i1 - i0 + 1, files[fid[i0]], la, files[fid[j0]], lb, exact))
            if s is not None:
                run_s = prev = s

    # Drop clones nested inside a larger clone of the same pair of files.
    clones.sort(key=lambda c: -c[0])
    kept = []
    cover = defaultdict(list)
    for c in clones:
        key = (c[2], c[4])
        if any(a0 <= c[3][0] and c[3][1] <= a1 and b0 <= c[5][0] and c[5][1] <= b1
               for (a0, a1), (b0, b1) in cover[key]):
            continue
        cover[key].append((c[3], c[5]))
        kept.append(c)

    with open(out / "clones.tsv", "w") as fh:
        fh.write("lines\ttokens\tidentical_names\tfile_a\tlines_a\tfile_b\tlines_b\n")
        for nl, nt, fa, la, fb, lb, ex in kept:
            fh.write(f"{nl}\t{nt}\t{int(ex)}\t{fa}\t{la[0]}-{la[1]}\t{fb}\t{lb[0]}-{lb[1]}\n")
    pairs = defaultdict(lambda: [0, 0])
    for nl, _, fa, _, fb, _, _ in kept:
        k = tuple(sorted((fa, fb)))
        pairs[k][0] += nl
        pairs[k][1] += 1
    with open(out / "file_pairs.tsv", "w") as fh:
        fh.write("cloned_lines\tclones\tfile_a\tfile_b\n")
        for (fa, fb), (nl, k) in sorted(pairs.items(), key=lambda kv: -kv[1][0]):
            fh.write(f"{nl}\t{k}\t{fa}\t{fb}\n")
    # Per file: distinct lines that sit inside any clone (either side).
    covered = defaultdict(set)
    for nl, _, fa, la, fb, lb, _ in kept:
        covered[fa].update(range(la[0], la[1] + 1))
        covered[fb].update(range(lb[0], lb[1] + 1))
    with open(out / "files.tsv", "w") as fh:
        fh.write("file\tcode_lines\tcloned_lines\tcloned_share\n")
        for i, f in sorted(enumerate(files), key=lambda kv: -len(covered[kv[1]])):
            c = len(covered[f])
            fh.write(f"{f}\t{nonblank[i]}\t{c}\t{c / max(nonblank[i], 1):.2f}\n")
    tot_code = sum(nonblank)
    tot_cov = sum(len(v) for v in covered.values())
    renamed = sum(c[0] for c in kept if not c[6])
    print(f"{len(files)} files, {tot_code} code lines, {n} tokens; {len(kept)} clone pairs; "
          f"{tot_cov} distinct lines inside a clone ({tot_cov / max(tot_code, 1):.1%}); "
          f"pair-lines {sum(c[0] for c in kept)} of which renamed/retyped {renamed}")


if __name__ == "__main__":
    main()

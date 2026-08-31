#!/usr/bin/env python3
"""Inventory of the SVT-AV1 C encoder surface vs what this port has translated.

Answers ONE question: which C functions have no Rust counterpart yet. It does
that by NAME, which is a heuristic and is stated as one — the port deliberately
names its functions after C's, so a name hit is good evidence of a port and a
name miss is good evidence of a gap, but neither is proof. Treat the output as
a work queue, not a coverage claim.

  tools/c_surface_inventory.py [--tsv out.tsv]

Counts every function DEFINITION in Source/Lib/{Codec, Globals, C_DEFAULT},
then looks for the name in the Rust tree under rust/crates + rust/svtav1.

SCOPE, and why:
  Codec, Globals  — the encoder proper.
  C_DEFAULT       — the SCALAR reference kernels. These are the semantics every
                    SIMD variant must reproduce, so they are exactly what a
                    byte-exact port has to match, and leaving them out
                    understated the surface. (They were missing from this
                    tool's first run on 2026-08-31: 46 functions, 25 unmatched,
                    including obmc_variance, obmc_sad, the compound
                    diffwtd-mask builders and the whole 10-bit pack/unpack
                    family.)
  ASM_*           — DELIBERATELY EXCLUDED. Those ~212 .c files are hand-written
                    AVX2/AVX512/NEON/SVE/SSE implementations of kernels whose
                    semantics live in C_DEFAULT. This port reaches the same
                    semantics through archmage dispatch and Rust SIMD, so they
                    are alternative implementations, not untranslated surface.
                    Excluding them is a scope judgement, not an oversight — a
                    PERF coverage map would be a different tool.
"""
import os, re, subprocess, sys, json

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))          # rust/
ROOT = os.path.dirname(REPO)
CSRC = os.path.join(ROOT, "reference", "svt-av1", "Source", "Lib")
RSRC = [os.path.join(REPO, "crates"), os.path.join(REPO, "svtav1")]

# A C function definition at column 0: <type...> name(args) {   — deliberately
# conservative; it misses macro-generated and multi-line-signature functions,
# which is why the totals are a LOWER BOUND on the surface.
DEF = re.compile(r'^(?:static\s+)?(?:const\s+)?[A-Za-z_][\w \*]*?\b([a-z_][a-z0-9_]*)\s*\([^;]*?\)\s*\{', re.M)

def c_functions():
    out = {}
    for sub in ("Codec", "Globals", "C_DEFAULT"):
        d = os.path.join(CSRC, sub)
        if not os.path.isdir(d):
            continue
        for fn in sorted(os.listdir(d)):
            if not fn.endswith(".c"):
                continue
            path = os.path.join(d, fn)
            text = open(path, errors="ignore").read()
            for m in DEF.finditer(text):
                name = m.group(1)
                if name in ("if", "for", "while", "switch", "return", "sizeof"):
                    continue
                out.setdefault(f"{sub}/{fn}", []).append(name)
    return out

def rust_blob():
    parts = []
    for base in RSRC:
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in ("target", ".git", "vendor")]
            for f in filenames:
                if f.endswith((".rs", ".c")):
                    parts.append(open(os.path.join(dirpath, f), errors="ignore").read())
    return "\n".join(parts)

def main():
    cfns = c_functions()
    blob = rust_blob()
    rows = []
    for path, names in cfns.items():
        for n in sorted(set(names)):
            # A port may drop the svt_aom_ / svt_av1_ / svt_ prefix.
            stems = {n}
            for p in ("svt_aom_", "svt_av1_", "svt_"):
                if n.startswith(p):
                    stems.add(n[len(p):])
            hit = any(s in blob for s in stems)
            rows.append((path, n, "ported" if hit else "MISSING"))

    per_file = {}
    for path, n, st in rows:
        d = per_file.setdefault(path, [0, 0, []])
        d[0] += 1
        if st == "ported":
            d[1] += 1
        else:
            d[2].append(n)

    total = len(rows)
    ported = sum(1 for r in rows if r[2] == "ported")
    print(f"C functions found: {total}   name-matched in the Rust tree: {ported}"
          f"   no match: {total - ported}   ({100.0 * ported / total:.1f}% matched)")
    print()
    print(f"{'file':44} {'total':>6} {'matched':>8} {'gap':>5}")
    for path in sorted(per_file, key=lambda p: -(per_file[p][0] - per_file[p][1])):
        t, p, miss = per_file[path]
        if t - p == 0:
            continue
        print(f"{path:44} {t:>6} {p:>8} {t - p:>5}")

    if "--tsv" in sys.argv:
        out = sys.argv[sys.argv.index("--tsv") + 1]
        with open(out, "w") as fh:
            fh.write("file\tfunction\tstatus\n")
            for path, n, st in rows:
                fh.write(f"{path}\t{n}\t{st}\n")
        print(f"\nwrote {out}")

main()

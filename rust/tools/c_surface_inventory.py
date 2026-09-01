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
#
# The name class is `[A-Za-z_]\w*`, NOT `[a-z_][a-z0-9_]*`. It was the latter
# until 2026-08-31, which made every C function with an uppercase letter in
# its name INVISIBLE to this tool — not unmatched, absent. In Codec/transforms.c
# alone that hid 76 of 181 definitions: the 55 `_N2_c` / `_N4_c` 1-D kernels,
# the 38 `_N2_c` / `_N4_c` 2-D wrappers and the 5
# `svt_handle_transform*_N2_N4_c` entries, so the file reported a surface of
# 105. Tree-wide the fix raises the count from 2,673 to 2,756. A probe that
# silently sees nothing is indistinguishable from an absence
# (docs/WORKING-ON-THIS.md §5), which is exactly what this was.
DEF = re.compile(r'^(?:static\s+)?(?:const\s+)?[A-Za-z_][\w \*]*?\b([A-Za-z_]\w*)\s*\([^;]*?\)\s*\{', re.M)

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

def rust_defs():
    """Every function NAME defined in port source.

    STRICT by construction, and it has to be. The first version of this tool
    asked `any(stem in blob)` over a concatenation of every .rs and .c file
    including tests and cref shims — so a name mentioned ANYWHERE counted as
    ported. Measured consequences on the 2026-08-31 wave, by the completeness
    audit:

      * >=79 rows flipped to "ported" because a lane's module doc listed them
        as NOT ported. `cyclic_refresh_init`, `rtc_cyclic_refresh_init` and
        `kf_group_rate_assingment` each appear in exactly one place in the
        tree: a `//!` line enumerating what was left out. The tool read "we
        did not port X" as evidence that X is ported, converting a lane's
        honesty into credit.
      * 30 more flipped on a cref shim or a test mention alone, with no port
        function at all.

    So: only `fn <name>` in NON-TEST, NON-CREF Rust source counts. cref is
    excluded because it is the C oracle's binding layer — a name there is
    evidence the C function was CALLED, which is the opposite of ported.

    This still UNDER-credits real work in one known way, and that is the safer
    direction: a lane that parameterizes N C functions into one table (as
    wp-transforms did for the 54 `highbd_fwd_txfm_WxH*` entries) reads as N
    misses. Such collapses are disclosed in the lanes' port maps; check there
    before treating a MISSING row as untouched.

    Worked example of how big that effect is, so nobody reads a row as a
    coverage number: `docs/transforms-port-map.md` audits
    Codec/transforms.c + Codec/inv_transforms.c function by function and finds
    0 of 256 definitions unported — while this tool reports 7/181 and 30/75
    matched, because 174 of them sit behind nine deliberate family collapses
    and 2 have no expressible Rust counterpart at all.
    """
    defs = set()
    fn_re = re.compile(r"\bfn\s+([a-z_][a-z0-9_]*)")
    for base in RSRC:
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = [d for d in dirnames if d not in ("target", ".git", "vendor")]
            if os.sep + "tests" in dirpath or "svtav1-cref" in dirpath:
                continue
            for f in filenames:
                if not f.endswith(".rs"):
                    continue
                text = open(os.path.join(dirpath, f), errors="ignore").read()
                defs.update(fn_re.findall(text))
    return defs

def main():
    cfns = c_functions()
    defs = rust_defs()
    rows = []
    for path, names in cfns.items():
        for n in sorted(set(names)):
            # A port may drop the svt_aom_ / svt_av1_ / svt_ prefix.
            stems = {n}
            for p in ("svt_aom_", "svt_av1_", "svt_"):
                if n.startswith(p):
                    stems.add(n[len(p):])
            hit = any(st in defs for st in stems)
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
    print(f"C functions found: {total}   with a matching `fn` in port source: {ported}"
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

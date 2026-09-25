#!/usr/bin/env python3
"""Rank where the Rust port is disproportionately larger than the C it ports.

Inputs are rust-code-analysis JSON trees (`rust-code-analysis-cli -m -O json`):
one for the test-stripped Rust mirror (from prep.py) and one for the C encoder
(Source/Lib/{Codec,C_DEFAULT,Globals}; the per-ISA ASM_* trees are left out
because the port has one archmage kernel where C has one file per ISA).

Sizes are rust-code-analysis `ploc` (lines holding code; comments and blanks
excluded). Three views, each written as a TSV under --out:

  fn_pairs.tsv     C functions matched to Rust functions by normalized name
                   (svt_aom_/svt_av1_/svt_/aom_/av1_/eb_ prefixes and a `_c`
                   kernel suffix removed). A C function with several Rust
                   definitions is a duplication candidate; a big rust/c ratio
                   is a bloat candidate. Name matching misses renamed ports,
                   so an unmatched function is NOT proof of anything.
  file_cites.tsv   each Rust file against the C files its comments cite
                   (`foo.c` / `foo.h` mentions). Rust ploc vs the ploc of the
                   cited C files, split by how many other Rust files cite them.
  c_modules.tsv    per C .c file: its ploc against the Rust ploc attributed to
                   it. Each Rust file's ploc is split across the .c files it
                   cites, weighted by citation count. C's per-ISA SIMD files are
                   not in the C total, so dsp kernels read high by construction.
  rust_fns.tsv     every Rust function, largest first, with cyclomatic and
                   cognitive complexity and whether a C function shares its name.
  rust_files.tsv   per Rust file: ploc, share of ploc in name-matched functions,
                   comment lines per code line, max cognitive complexity.

The ratios are pointers for a human read, not verdicts: 8-bit and 10-bit paths,
safe-Rust bounds plumbing and archmage dispatch legitimately cost lines.
"""

import argparse
import json
import re
from collections import defaultdict
from pathlib import Path

PREFIX = re.compile(r"^(?:svt_aom_|svt_av1_|svt_|aom_|av1_|eb_)+")
CITE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*\.[ch])\b")


def norm(name):
    n = PREFIX.sub("", name.lower())
    if n.endswith("_c") and len(n) > 3:
        n = n[:-2]
    return n


def functions(rca_dir, strip_prefix):
    """(file, name, start, end, ploc, cyclomatic, cognitive) for every function space."""
    out = []
    for j in Path(rca_dir).rglob("*.json"):
        d = json.loads(j.read_text())
        path = d["name"]
        if strip_prefix and path.startswith(strip_prefix):
            path = path[len(strip_prefix):]

        def walk(s, top):
            if s["kind"] == "function" and s["name"] and top:
                m = s["metrics"]
                out.append((path, s["name"], s["start_line"], s["end_line"], m["loc"]["ploc"],
                            m["cyclomatic"]["sum"], m["cognitive"]["sum"]))
                # Nested fns/closures are counted inside their parent.
                return
            for c in s["spaces"]:
                walk(c, top)

        walk(d, True)
    return out


def units(rca_dir, strip_prefix):
    out = {}
    for j in Path(rca_dir).rglob("*.json"):
        d = json.loads(j.read_text())
        path = d["name"]
        if strip_prefix and path.startswith(strip_prefix):
            path = path[len(strip_prefix):]
        m = d["metrics"]
        out[path] = (m["loc"]["ploc"], m["loc"]["cloc"], m["cognitive"]["max"])
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rust-rca", required=True)
    ap.add_argument("--rust-prefix", default="", help="prefix to strip from Rust paths (the mirror dir)")
    ap.add_argument("--rust-src", required=True, help="the mirror tree, for comment citations")
    ap.add_argument("--c-rca", required=True)
    ap.add_argument("--c-prefix", default="")
    ap.add_argument("--out", required=True)
    ap.add_argument("--top", type=int, default=40)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    rf = functions(a.rust_rca, a.rust_prefix)
    cf = functions(a.c_rca, a.c_prefix)
    ru = units(a.rust_rca, a.rust_prefix)
    cu = units(a.c_rca, a.c_prefix)

    c_by = defaultdict(list)
    for f in cf:
        c_by[norm(f[1])].append(f)
    r_by = defaultdict(list)
    for f in rf:
        r_by[norm(f[1])].append(f)

    # --- function pairs -------------------------------------------------------
    rows = []
    matched_r = set()
    for key, cs in c_by.items():
        rs = r_by.get(key)
        if not rs or len(key) < 6:  # short names ("init", "get") match noise
            continue
        cp = max(c[4] for c in cs)
        rp = sum(r[4] for r in rs)
        for r in rs:
            matched_r.add((r[0], r[2]))
        rows.append((key, len(rs), cp, rp, rp / max(cp, 1), rp - cp,
                     f"{cs[0][0]}:{cs[0][2]}", " ".join(f"{r[0]}:{r[2]}" for r in rs)))
    rows.sort(key=lambda r: -r[5])
    with open(out / "fn_pairs.tsv", "w") as fh:
        fh.write("fn\trust_defs\tc_ploc\trust_ploc\tratio\texcess\tc_site\trust_sites\n")
        for r in rows:
            fh.write("\t".join(str(round(x, 2)) if isinstance(x, float) else str(x) for x in r) + "\n")

    # --- citations: Rust file -> cited C files --------------------------------
    c_file_ploc = defaultdict(int)
    for p, (ploc, _, _) in cu.items():
        c_file_ploc[Path(p).name] += ploc
    cites = {}
    cited_by = defaultdict(set)
    for p in ru:
        src = Path(a.rust_src) / p
        if not src.exists():
            continue
        names = {n for n in CITE.findall(src.read_text(errors="replace")) if n in c_file_ploc}
        cites[p] = names
        for n in names:
            cited_by[n].add(p)
    frows = []
    for p, names in cites.items():
        if not names:
            continue
        # Share each C file's ploc across the Rust files that cite it.
        c_share = sum(c_file_ploc[n] / len(cited_by[n]) for n in names)
        frows.append((p, ru[p][0], round(c_share), ru[p][0] / max(c_share, 1),
                      " ".join(sorted(names, key=lambda n: -c_file_ploc[n])[:6])))
    frows.sort(key=lambda r: -(r[1] - r[2]))
    with open(out / "file_cites.tsv", "w") as fh:
        fh.write("rust_file\trust_ploc\tcited_c_ploc_share\tratio\tcited_c_files\n")
        for r in frows:
            fh.write("\t".join(str(round(x, 2)) if isinstance(x, float) else str(x) for x in r) + "\n")

    # --- citation-weighted attribution to C modules ---------------------------
    attr = defaultdict(float)
    attr_files = defaultdict(list)
    unattributed = 0.0
    for p in ru:
        src = Path(a.rust_src) / p
        counts = defaultdict(int)
        if src.exists():
            for n in CITE.findall(src.read_text(errors="replace")):
                if n.endswith(".c") and n in c_file_ploc:
                    counts[n] += 1
        tot = sum(counts.values())
        if not tot:
            unattributed += ru[p][0]
            continue
        for n, k in counts.items():
            share = ru[p][0] * k / tot
            attr[n] += share
            attr_files[n].append((share, p))
    mrows = []
    for n in set(attr) | {Path(p).name for p in cu if p.endswith(".c")}:
        rp, cp = attr.get(n, 0.0), c_file_ploc.get(n, 0)
        top = " ".join(f"{Path(q).name}:{s:.0f}" for s, q in sorted(attr_files.get(n, []), reverse=True)[:5])
        mrows.append((n, cp, round(rp), rp / max(cp, 1), round(rp - cp), top))
    mrows.sort(key=lambda r: -r[4])
    with open(out / "c_modules.tsv", "w") as fh:
        fh.write("c_file\tc_ploc\trust_ploc_attributed\tratio\texcess\ttop_rust_contributors\n")
        for r in mrows:
            fh.write("\t".join(str(round(x, 2)) if isinstance(x, float) else str(x) for x in r) + "\n")
    with open(out / "rust_fns.tsv", "w") as fh:
        fh.write("rust_file\tline\tfn\tploc\tcyclomatic\tcognitive\tc_name_match\n")
        for f in sorted(rf, key=lambda f: -f[4]):
            fh.write(f"{f[0]}\t{f[2]}\t{f[1]}\t{f[4]:.0f}\t{f[5]:.0f}\t{f[6]:.0f}\t{int((f[0], f[2]) in matched_r)}\n")

    # --- per Rust file -----------------------------------------------------------
    per = defaultdict(lambda: [0, 0])
    for f in rf:
        per[f[0]][0] += f[4]
        if (f[0], f[2]) in matched_r:
            per[f[0]][1] += f[4]
    with open(out / "rust_files.tsv", "w") as fh:
        fh.write("rust_file\tploc\tfn_ploc\tmatched_share\tcomment_per_code\tmax_cognitive\n")
        for p, (ploc, cloc, cogmax) in sorted(ru.items(), key=lambda kv: -kv[1][0]):
            fp, mp = per[p]
            fh.write(f"{p}\t{ploc:.0f}\t{fp:.0f}\t{mp / max(fp, 1):.2f}\t{cloc / max(ploc, 1):.2f}\t{cogmax:.0f}\n")

    # --- summary ----------------------------------------------------------------
    tr = sum(v[0] for v in ru.values())
    tc = sum(v[0] for v in cu.values())
    trc = sum(v[1] for v in ru.values())
    tcc = sum(v[1] for v in cu.values())
    print(f"Rust ploc {tr:.0f} (comment lines {trc:.0f}, {trc / tr:.2f}/code line); "
          f"C ploc {tc:.0f} (comment lines {tcc:.0f}, {tcc / tc:.2f}/code line)")
    print(f"C functions {len(cf)}, Rust functions {len(rf)}, name-matched C fns {len(rows)}, "
          f"matched Rust ploc {sum(r[3] for r in rows):.0f} vs C {sum(r[2] for r in rows):.0f}")
    multi = [r for r in rows if r[1] > 1]
    print(f"C fns with >1 Rust definition: {len(multi)} "
          f"(Rust ploc {sum(r[3] for r in multi):.0f} vs C {sum(r[2] for r in multi):.0f})")
    def dist(fs, label):
        big = [f for f in fs if f[4] >= 200]
        cog = [f for f in fs if f[6] >= 50]
        total = sum(f[4] for f in fs)
        print(f"  {label:5} fns={len(fs):5}  ploc>=200: {len(big):4} fns holding {sum(f[4] for f in big) / total:5.1%} of fn code;"
              f"  cognitive>=50: {len(cog):4};  max ploc {max(f[4] for f in fs):.0f}, max cognitive {max(f[6] for f in fs):.0f}")
    print("\nFunction size / complexity distribution:")
    dist(cf, "C")
    dist(rf, "Rust")
    print(f"\nTop {a.top} Rust functions by cognitive complexity:")
    for f in sorted(rf, key=lambda f: -f[6])[:a.top]:
        print(f"  cog={f[6]:5.0f} cyc={f[5]:4.0f} ploc={f[4]:5.0f}  {f[0]}:{f[2]} {f[1]}")
    print(f"\nTop 15 C functions by cognitive complexity (for scale):")
    for f in sorted(cf, key=lambda f: -f[6])[:15]:
        print(f"  cog={f[6]:5.0f} cyc={f[5]:4.0f} ploc={f[4]:5.0f}  {f[0]}:{f[2]} {f[1]}")
    print(f"\nTop {a.top} function pairs by excess Rust ploc (rust - c):")
    for r in rows[:a.top]:
        print(f"  {r[0]:40} defs={r[1]:2} c={r[2]:5.0f} rust={r[3]:5.0f} x{r[4]:4.1f}  {r[6]}")
    print(f"\nRust ploc citing no .c file at all: {unattributed:.0f}")
    print(f"\nTop {a.top} C modules by attributed Rust ploc above C ploc:")
    for r in mrows[:a.top]:
        print(f"  {r[0]:34} c={r[1]:6.0f} rust={r[2]:6} x{r[3]:5.1f}  {r[5]}")
    print(f"\nC modules the port barely touches (c_ploc >= 300, ratio < 0.25):")
    for r in sorted(mrows, key=lambda r: r[3]):
        if r[1] >= 300 and r[3] < 0.25:
            print(f"  {r[0]:34} c={r[1]:6.0f} rust={r[2]:6}")
    print(f"\nTop {a.top} Rust files by ploc above their cited-C share:")
    for r in frows[:a.top]:
        print(f"  {r[0]:62} rust={r[1]:6.0f} c_share={r[2]:6} x{r[3]:5.1f}  {r[4]}")


if __name__ == "__main__":
    main()

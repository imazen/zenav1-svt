#!/usr/bin/env python3
"""Consolidate every committed 8-bit sweep into ONE per-preset coverage matrix.

    python3 tools/coverage_matrix.py [--csv]

WHY THIS EXISTS. The 8-bit evidence lives in several `benchmarks/
identity_full_8bit*.tsv` files, one per sweep, and a preset can look "covered"
because it appears in *a* sweep while the axis that actually exercises its
unique feature is missing. That is not hypothetical: presets 1/2/3 were covered
at 64/128px synthetic and nowhere else, and IntraBC — the ONLY thing that
distinguishes their `intrabc_level` 4/5/6 — never wins a block on synthetic
content, so levels 4/5/6 had no byte-parity coverage at all despite every
preset showing green.

This prints cells-per-(preset x axis) so an EMPTY axis is visible as `--`
rather than hiding behind another axis's pass rate. Read the `--` first; a
missing cell count is a coverage claim nobody has tested, and it is strictly
more dangerous than a failing one.

Axes:
  synth          64/128px synthetic content (uniform/gradient/diag/screen)
  dims-aligned   64-aligned geometry from the dims tier
  dims-partial   NON-64-aligned geometry (partial SB, odd, tiny)
  real-photo     CID22 + gb82 photographic corpora, 512x512 centre crop
  real-screen    gb82-sc screen corpus, 512x512 centre crop

PINNED cells count as covered (they are known-diverging and tracked); only
IDENTICAL and PINNED are counted as pass, so RS_ERR / C_ERR show up as a gap.
"""
import csv, glob, collections, os, sys

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "benchmarks")


def tier(fn):
    b = os.path.basename(fn)
    if "_dims" in b:
        return "dims"
    if "_real" in b:
        return "real"
    return "synthetic"


def corpus(path):
    # gb82-sc must be tested BEFORE gb82 — the screen corpus path contains both.
    if "gb82-sc" in path:
        return "screen"
    if "CID22" in path or "gb82" in path:
        return "photo"
    return "photo"


def bucket(r):
    t = r["_tier"]
    if t == "synthetic":
        return "synth"
    if t == "real":
        return "real-" + corpus(r["content"])
    aligned = int(r["width"]) % 64 == 0 and int(r["height"]) % 64 == 0
    return "dims-aligned" if aligned else "dims-partial"


def main():
    files = sorted(glob.glob(os.path.join(BASE, "identity_full_8bit*.tsv")))
    rows = []
    for fn in files:
        with open(fn) as fh:
            for r in csv.DictReader(fh, delimiter="\t"):
                r["_tier"] = tier(fn)
                rows.append(r)
    if not rows:
        sys.exit(f"no identity_full_8bit*.tsv under {BASE}")

    ok = lambda r: r["verdict"] in ("IDENTICAL", "PINNED")
    cols = ["synth", "dims-aligned", "dims-partial", "real-photo", "real-screen"]
    g = collections.defaultdict(lambda: [0, 0])
    for r in rows:
        k = (int(r["preset"]), bucket(r))
        g[k][1] += 1
        if ok(r):
            g[k][0] += 1

    if "--csv" in sys.argv:
        w = csv.writer(sys.stdout)
        w.writerow(["preset"] + cols)
        for p in range(14):
            w.writerow([p] + [f"{g[(p,c)][0]}/{g[(p,c)][1]}" if g[(p, c)][1] else "" for c in cols])
        return

    print(f"8-BIT COVERAGE MATRIX — {len(rows)} cells across {len(files)} sweeps\n")
    print("      " + "".join(c.center(16) for c in cols))
    gaps = []
    for p in range(14):
        line = f"  p{p:<3} "
        for c in cols:
            i, t = g[(p, c)]
            if t == 0:
                line += "--".center(16)
                gaps.append(f"p{p} {c}")
            else:
                line += f"{i}/{t}".center(16)
        print(line)
    print("\n  '--' = NO CELLS. An untested axis is a coverage claim nobody checked;")
    print("  read these before reading any pass rate.")
    if gaps:
        print(f"\n  {len(gaps)} uncovered (preset, axis) pairs:")
        for gp in gaps:
            print(f"    {gp}")


if __name__ == "__main__":
    main()

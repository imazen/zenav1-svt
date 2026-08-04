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
    """Classify a row by WHAT IT IS, not by which file it came from.

    The tier used to be derived from the filename (`_dims` -> dims, `_real` ->
    real, else synthetic). That silently mis-filed every row of a FULL sweep,
    whose filename carries no tier marker: its dims geometries landed in the
    `synth` column and the dims columns read `--` for every preset the full gate
    covers. The cell knows what it is — a 512x512 corpus crop is real content
    and a 65x65 frame is partial-SB geometry — so ask the cell.
    """
    content = r["content"]
    if "/" in content or content.startswith(("crop:", "file:", "raw:")):
        return "real-" + corpus(content)
    w, h = int(r["width"]), int(r["height"])
    aligned = w % 64 == 0 and h % 64 == 0
    # The synthetic tier is exactly {64,128} squares; every other geometry
    # belongs to the dims sweep. A 64x64 or 128x128 cell appears in both tiers
    # and is the same cell either way, so counting it once as `synth` is right.
    if w == h and w in (64, 128):
        return "synth"
    return "dims-aligned" if aligned else "dims-partial"


def main():
    # Oldest first, so a later sweep's verdict for the SAME cell overwrites an
    # earlier one. Without this the matrix is cumulative-over-history: a cell
    # that failed in April and passes today counts as both, and a preset whose
    # bugs were fixed still reads as failing forever. (Measured 2026-08-04: p0
    # dims-partial showed 67/108 the day it became 36/36.) Coverage is about
    # WHICH cells exist; the pass rate beside it has to reflect the CURRENT
    # tree or nobody will trust either number.
    # `identity_full_8bit_latest.tsv` is INCLUDED deliberately: it is the
    # scratch pointer the last run wrote, which makes it the newest evidence
    # there is. Excluding it blanked p6..p13's dims columns to `--`, because
    # those presets' most recent verdicts live only there. It is gitignored, so
    # a fresh clone simply has one fewer source.
    files = sorted(
        glob.glob(os.path.join(BASE, "identity_full_8bit*.tsv")),
        key=lambda f: (os.path.getmtime(f), f),
    )
    latest = {}
    for fn in files:
        with open(fn) as fh:
            for r in csv.DictReader(fh, delimiter="\t"):
                latest[(r["content"], r["width"], r["height"], r["qp"], r["preset"])] = r
    rows = list(latest.values())
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

    print(f"8-BIT COVERAGE MATRIX — {len(rows)} distinct cells, newest verdict per cell, from {len(files)} sweeps\n")
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

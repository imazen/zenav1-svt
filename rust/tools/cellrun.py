#!/usr/bin/env python3
"""One runner for byte-identity cells (plan T3, chunk 1).

Each cell encodes with the port (`tools/identity_run`) and with the C oracle
named by `SVT_ORACLE` (`tools/capture_c_trace/capture_c_trace`), then
`tools/identity_diff.py` classifies the pair: the same three steps
`tools/identity_diff.sh` runs, driven by a cell list instead of a shell loop,
so two cells can differ only in what they ask for.

A cell list is a TSV file with a header row. Columns:
  name      unique cell name (also its artifact directory)
  content   anything identity_run accepts (gradient, crop:<png>, raw:<yuv>, ...)
  w h qp preset
  bd        bit depth, 8 or 10 (optional, default 8)
  env       extra variables for BOTH encoders, `K=V;K=V` (optional)
  expect    pinned verdict, IDENTICAL or DIFFERS (optional)
Blank lines and lines starting with `#` are ignored.

Output: a TSV with one row per cell (name, verdict, stage, detail, expect,
ok), written to --out and summarised on stderr. Exit status: 0 when every
cell that pins an `expect` matches it, 1 when one does not, 2 when a cell
could not be run (encoder refused, driver missing, timeout).

Usage: tools/cellrun.py CELLS.tsv [--out OUT.tsv] [--jobs N] [--timeout S]
Run it under run-heavy; the oracle comes from SVT_ORACLE (default: the
registry's default row, as for every other tool).
"""

import argparse
import concurrent.futures
import csv
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RS_ROOT = HERE.parent


def read_cells(path):
    rows = []
    with open(path, newline="") as f:
        lines = [l for l in f if l.strip() and not l.lstrip().startswith("#")]
    for row in csv.DictReader(lines, delimiter="\t"):
        for col in ("name", "content", "w", "h", "qp", "preset"):
            if not row.get(col):
                sys.exit(f"cellrun: {path}: a row lacks '{col}': {row}")
        rows.append(row)
    names = [r["name"] for r in rows]
    if len(set(names)) != len(names):
        sys.exit(f"cellrun: {path}: duplicate cell names")
    return rows


def cell_env(row):
    env = dict(os.environ)
    env["SVTAV1_BD"] = row.get("bd") or "8"
    for kv in filter(None, (row.get("env") or "").split(";")):
        k, _, v = kv.partition("=")
        env[k.strip()] = v.strip()
    return env


def run_cell(row, root, timeout):
    """Returns (verdict, stage, detail). verdict is IDENTICAL, DIFFERS or ERROR."""
    d = root / row["name"]
    d.mkdir(parents=True, exist_ok=True)
    env = cell_env(row)
    w, h, qp, preset = row["w"], row["h"], row["qp"], row["preset"]
    try:
        with open(d / "rs.trace", "wb") as err:
            r = subprocess.run(
                [str(HERE / "identity_run"), row["content"], w, h, qp, preset, str(d / "rs")],
                env=env, stdout=subprocess.DEVNULL, stderr=err, timeout=timeout)
        if r.returncode != 0:
            return "ERROR", "PORT", f"identity_run exited {r.returncode} (see {d}/rs.trace)"
        # The byte-only driver (no --wrap on this linker) has no op trace; the
        # verdict is unaffected, only the localization is.
        sel = HERE / "capture_c_trace" / f".selected.{env.get('SVT_HDR_MODE', '0')}"
        nowrap = sel.exists() and sel.read_text().strip().endswith(".nowrap.bin")
        cenv = dict(env)
        cenv["SVT_TRACE_OUT"] = os.devnull if nowrap else str(d / "c.trace")
        with open(d / "c.stderr", "wb") as err:
            r = subprocess.run(
                [str(HERE / "capture_c_trace" / "capture_c_trace"), w, h, qp, preset,
                 str(d / "rs.yuv"), str(d / "c.obu"), env["SVTAV1_BD"]],
                env=cenv, stdout=subprocess.DEVNULL, stderr=err, timeout=timeout)
        if r.returncode != 0:
            return "ERROR", "C", f"capture_c_trace exited {r.returncode} (see {d}/c.stderr)"
        args = ["python3", str(HERE / "identity_diff.py"),
                "--c-obu", str(d / "c.obu"), "--rust-obu", str(d / "rs.obu")]
        if not nowrap:
            args += ["--c-trace", str(d / "c.trace"), "--rust-trace", str(d / "rs.trace")]
        r = subprocess.run(args, env=env, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return "ERROR", "TIMEOUT", f"over {timeout}s"
    (d / "report.txt").write_text(r.stdout)
    if r.returncode == 0:
        return "IDENTICAL", "-", "-"
    for line in r.stdout.splitlines():
        if line.startswith("STAGE: "):
            rest = line[len("STAGE: "):]
            stage, _, detail = rest.partition(" | ")
            return "DIFFERS", stage, detail or "-"
    return "DIFFERS", "-", "-"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cells")
    ap.add_argument("--out", help="result TSV (default: <cells stem>.result.tsv beside the artifacts)")
    ap.add_argument("--jobs", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=600, help="seconds per step")
    a = ap.parse_args()

    rows = read_cells(a.cells)
    oracle = os.environ.get("SVT_ORACLE", "default")
    root = RS_ROOT / "target" / "cells" / f"{Path(a.cells).stem}.{oracle}"
    out = Path(a.out) if a.out else root / "result.tsv"
    out.parent.mkdir(parents=True, exist_ok=True)

    # Build both drivers once, up front, so parallel cells do not race cargo.
    subprocess.run([str(HERE / "identity_run"), "--version"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as ex:
        results = list(ex.map(lambda r: run_cell(r, root, a.timeout), rows))

    bad = errors = 0
    with open(out, "w", newline="") as f:
        wr = csv.writer(f, delimiter="\t", lineterminator="\n")
        wr.writerow(["name", "verdict", "stage", "detail", "expect", "ok"])
        for row, (verdict, stage, detail) in zip(rows, results):
            expect = row.get("expect") or ""
            ok = "-" if not expect else ("yes" if expect == verdict else "NO")
            bad += ok == "NO"
            errors += verdict == "ERROR"
            wr.writerow([row["name"], verdict, stage, detail, expect or "-", ok])
    ident = sum(v == "IDENTICAL" for v, _, _ in results)
    print(f"cellrun: {oracle}: {ident}/{len(rows)} identical, {errors} errors, "
          f"{bad} differ from their pinned verdict -> {out}", file=sys.stderr)
    return 2 if errors else (1 if bad else 0)


if __name__ == "__main__":
    sys.exit(main())

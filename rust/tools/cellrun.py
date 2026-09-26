#!/usr/bin/env python3
"""One runner for identity cells (plan T3).

Each cell encodes with the port (`tools/identity_run`), then runs the checks
its `check` column names:
  c         byte identity with the C oracle named by `SVT_ORACLE`
            (`tools/capture_c_trace/capture_c_trace`), classified by
            `tools/identity_diff.py` (the default check);
  lossless  aomdec's output of the port's stream == the source .yuv;
  recon     aomdec's output == the port's final recon (`SVTAV1_FINAL_RECON`,
            set by the runner), the encoder/decoder alignment check;
  dav1d     dav1d's output == aomdec's (implies a decode);
  decodes   aomdec accepts the port's stream (no pixel comparison; implied
            by lossless/recon/dav1d);
  none      the port encode alone (e.g. the sibling a `differs_from` names).
so cells differ only in what they ask for, never in how they are run.

A cell list is a TSV file with a header row. Columns:
  name          unique cell name (also its artifact directory)
  content       anything identity_run accepts (gradient, crop:<png>, ...)
  w h qp preset
  bd            bit depth, 8 or 10 (optional, default 8)
  env           extra variables for BOTH encoders, `K=V;K=V` (optional)
  env_port      variables for the port only (e.g. SVTAV1_Y_STRIDE, or the
                port's name for a knob the C driver spells differently)
  env_c         variables for the C driver only (e.g. SVT_TILE_ROWS)
  expect        pinned C verdict, IDENTICAL or DIFFERS (optional)
  check         comma list from the table above (optional, default `c`)
  differs_from  another cell whose port stream this one's must NOT equal
                (anti-vacuity: the feature under test changed the output)
  c_differs_from  another cell whose C stream this one's must NOT equal
                (anti-vacuity on the C side: the knob reached C's encoder)
  arch          comma list of machine arches (`uname -m`) the cell runs on,
                e.g. `aarch64,arm64` (Linux and macOS spell ARM differently),
                or ones it skips (`!aarch64,!arm64`); for arch-specific cells
                and pins
Blank lines and lines starting with `#` are ignored.

Output: a TSV with one row per cell (name, verdict, stage, detail, expect,
checks, ok), written to --out and summarised on stderr. `verdict` is the C
comparison (IDENTICAL, DIFFERS, ERROR, or `-` without `c`); `checks` lists the
other checks as `name=ok|FAIL`. Exit status: 0 when every pinned `expect`
matches and every other check passes, 1 when one does not, 2 when a cell
could not be run (encoder refused, driver or decoder missing, timeout).

--bytes-only compares the C stream with `cmp` instead of capturing and
diffing symbol traces: the verdict is the same, only localisation is lost.

Usage: tools/cellrun.py CELLS.tsv [--out OUT.tsv] [--jobs N] [--timeout S]
                        [--bytes-only]
Run it under run-heavy; the oracle comes from SVT_ORACLE (default: the
registry's default row, as for every other tool). Decoders: AOMDEC, DAV1D.
"""

import argparse
import concurrent.futures
import csv
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RS_ROOT = HERE.parent
CHECKS = {"c", "lossless", "recon", "dav1d", "decodes", "none"}


def read_cells(path):
    rows = []
    with open(path, newline="") as f:
        lines = [l for l in f if l.strip() and not l.lstrip().startswith("#")]
    for row in csv.DictReader(lines, delimiter="\t"):
        for col in ("name", "content", "w", "h", "qp", "preset"):
            if not row.get(col):
                sys.exit(f"cellrun: {path}: a row lacks '{col}': {row}")
        checks = set(filter(None, (row.get("check") or "c").split(",")))
        if not checks <= CHECKS:
            sys.exit(f"cellrun: {path}: {row['name']}: unknown check(s) {checks - CHECKS}")
        row["checks"] = checks - {"none"}
        rows.append(row)
    arch = os.uname().machine
    def applies(r):
        toks = [t.strip() for t in (r.get("arch") or "").split(",") if t.strip()]
        pos = [t for t in toks if not t.startswith("!")]
        neg = [t[1:] for t in toks if t.startswith("!")]
        return (not pos or arch in pos) and arch not in neg
    rows = [r for r in rows if applies(r)]
    names = {r["name"] for r in rows}
    if len(names) != len(rows):
        sys.exit(f"cellrun: {path}: duplicate cell names")
    for r in rows:
        for col in ("differs_from", "c_differs_from"):
            if r.get(col) and r[col] not in names:
                sys.exit(f"cellrun: {path}: {r['name']}: {col} names no cell")
    return rows


def cell_env(row, side=None):
    """The environment for one side (`port`, `c`) or for both (None)."""
    env = dict(os.environ)
    env["SVTAV1_BD"] = row.get("bd") or "8"
    cols = ["env"] + ([f"env_{side}"] if side else [])
    for col in cols:
        for kv in filter(None, (row.get(col) or "").split(";")):
            k, _, v = kv.partition("=")
            env[k.strip()] = v.strip()
    return env


def decoder(var, default):
    exe = os.environ.get(var) or shutil.which(default)
    if not exe or not (Path(exe).is_file() or shutil.which(exe)):
        return None
    return exe


def recon_bytes(d):
    """The port's final recon: one file for a still, `.f<i>` per frame."""
    if (d / "recon").exists():
        return (d / "recon").read_bytes()
    frames = sorted(d.glob("recon.f*"), key=lambda p: int(p.suffix[2:]))
    return b"".join(p.read_bytes() for p in frames) if frames else None


def last_line(path):
    """A log's most useful line for an ERROR row: a panic's message, else the
    last line that is not the backtrace hint."""
    try:
        lines = [l for l in path.read_text(errors="replace").splitlines() if l.strip()]
    except OSError:
        return "(no log)"
    for i, l in enumerate(lines):
        if "panicked at" in l and i + 1 < len(lines):
            return lines[i + 1][:200]
    lines = [l for l in lines if not l.startswith("note: run with")]
    return lines[-1][:200] if lines else "(empty log)"


def run_checks(row, d, env, timeout):
    """The decoder checks. Returns [(name, ok)], or raises RuntimeError."""
    want = row["checks"] & {"lossless", "recon", "dav1d", "decodes"}
    if not want:
        return []
    aomdec = decoder("AOMDEC", "aomdec")
    if not aomdec:
        raise RuntimeError("aomdec not found (set AOMDEC)")
    depth = ["--output-bit-depth=10"] if row.get("bd") == "10" else []
    r = subprocess.run([aomdec, "--rawvideo", *depth, "-o", str(d / "dec.yuv"), str(d / "rs.obu")],
                       env=env, capture_output=True, timeout=timeout)
    if r.returncode != 0:
        return [(c, False) for c in sorted(want)]
    dec = (d / "dec.yuv").read_bytes()
    out = [("decodes", True)] if "decodes" in want else []
    if "lossless" in want:
        out.append(("lossless", dec == (d / "rs.yuv").read_bytes()))
    if "recon" in want:
        rec = recon_bytes(d)
        out.append(("recon", rec is not None and dec == rec))
    if "dav1d" in want:
        dav1d = decoder("DAV1D", "dav1d")
        if not dav1d:
            raise RuntimeError("dav1d not found (set DAV1D)")
        r = subprocess.run([dav1d, "-q", "-i", str(d / "rs.obu"), "-o", str(d / "dav1d.yuv")],
                           env=env, capture_output=True, timeout=timeout)
        out.append(("dav1d", r.returncode == 0 and (d / "dav1d.yuv").read_bytes() == dec))
    return out


def run_c(row, d, timeout, bytes_only):
    """Returns (verdict, stage, detail) for the C comparison."""
    env = cell_env(row, "c")
    oracle = subprocess.run([str(HERE / "oracle"), "resolve"], env=env,
                            capture_output=True, text=True).stdout.strip()
    sel = HERE / "capture_c_trace" / f".selected.{oracle}"
    # The byte-only driver (no --wrap on this linker) has no op trace; the
    # verdict is unaffected, only the localization is.
    nowrap = sel.exists() and sel.read_text().strip().endswith(".nowrap.bin")
    traced = not (bytes_only or nowrap)
    cenv = dict(env)
    cenv["SVT_TRACE_OUT"] = str(d / "c.trace") if traced else os.devnull
    w, h, qp, preset = row["w"], row["h"], row["qp"], row["preset"]
    with open(d / "c.stderr", "wb") as err:
        r = subprocess.run(
            [str(HERE / "capture_c_trace" / "capture_c_trace"), w, h, qp, preset,
             str(d / "rs.yuv"), str(d / "c.obu"), env["SVTAV1_BD"]],
            env=cenv, stdout=subprocess.DEVNULL, stderr=err, timeout=timeout)
    if r.returncode != 0:
        return ("ERROR", "C", f"capture_c_trace exited {r.returncode}: "
                f"{last_line(d / 'c.stderr')} ({d}/c.stderr)")
    rs, c = (d / "rs.obu").read_bytes(), (d / "c.obu").read_bytes()
    if bytes_only:
        return ("IDENTICAL", "-", "-") if rs == c else ("DIFFERS", "-", f"port {len(rs)} B, C {len(c)} B")
    args = ["python3", str(HERE / "identity_diff.py"),
            "--c-obu", str(d / "c.obu"), "--rust-obu", str(d / "rs.obu")]
    if traced:
        args += ["--c-trace", str(d / "c.trace"), "--rust-trace", str(d / "rs.trace")]
    r = subprocess.run(args, env=env, capture_output=True, text=True, timeout=timeout)
    (d / "report.txt").write_text(r.stdout)
    if r.returncode == 0:
        return "IDENTICAL", "-", "-"
    for line in r.stdout.splitlines():
        if line.startswith("STAGE: "):
            stage, _, detail = line[len("STAGE: "):].partition(" | ")
            return "DIFFERS", stage, detail or "-"
    return "DIFFERS", "-", "-"


def run_cell(row, root, timeout, bytes_only):
    """Returns (verdict, stage, detail, checks)."""
    d = root / row["name"]
    if d.exists():
        shutil.rmtree(d)
    d.mkdir(parents=True)
    env = cell_env(row, "port")
    if "recon" in row["checks"]:
        env["SVTAV1_FINAL_RECON"] = str(d / "recon")
    try:
        with open(d / "rs.trace", "wb") as err:
            r = subprocess.run(
                [str(HERE / "identity_run"), row["content"], row["w"], row["h"], row["qp"],
                 row["preset"], str(d / "rs")],
                env=env, stdout=subprocess.DEVNULL, stderr=err, timeout=timeout)
        if r.returncode != 0:
            return ("ERROR", "PORT", f"identity_run exited {r.returncode}: "
                    f"{last_line(d / 'rs.trace')} ({d}/rs.trace)", [])
        checks = run_checks(row, d, env, timeout)
        if "c" in row["checks"]:
            verdict, stage, detail = run_c(row, d, timeout, bytes_only)
        else:
            verdict, stage, detail = "-", "-", "-"
    except subprocess.TimeoutExpired:
        return "ERROR", "TIMEOUT", f"over {timeout}s", []
    except RuntimeError as e:
        return "ERROR", "DECODER", str(e), []
    return verdict, stage, detail, checks


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cells")
    ap.add_argument("--out", help="result TSV (default: <cells stem>.result.tsv beside the artifacts)")
    ap.add_argument("--jobs", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=600, help="seconds per step")
    ap.add_argument("--bytes-only", action="store_true",
                    help="compare C bytes with cmp; skip the symbol traces")
    a = ap.parse_args()

    rows = read_cells(a.cells)
    oracle = os.environ.get("SVT_ORACLE", "default")
    root = RS_ROOT / "target" / "cells" / f"{Path(a.cells).stem}.{oracle}"
    out = Path(a.out) if a.out else root / "result.tsv"
    out.parent.mkdir(parents=True, exist_ok=True)

    # Build both drivers once, up front, so parallel cells do not race cargo
    # or the C relink: a cell that execs the driver while another cell's
    # build.sh rewrites it fails with "Text file busy" (seen 2026-09-26 after
    # an oracles.tsv change). One C build per oracle the cells resolve to.
    subprocess.run([str(HERE / "identity_run"), "--version"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    built = set()
    for row in rows:
        if "c" not in row["checks"]:
            continue
        env = cell_env(row, "c")
        oracle_name = subprocess.run([str(HERE / "oracle"), "resolve"], env=env,
                                     capture_output=True, text=True).stdout.strip()
        if oracle_name in built:
            continue
        built.add(oracle_name)
        r = subprocess.run([str(HERE / "capture_c_trace" / "build.sh")], env=env,
                           capture_output=True, text=True)
        if r.returncode != 0:
            sys.exit(f"cellrun: C driver build failed for {oracle_name}:\n{r.stdout}{r.stderr}")

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as ex:
        results = list(ex.map(lambda r: run_cell(r, root, a.timeout, a.bytes_only), rows))

    # Anti-vacuity, once every cell's stream exists. A sibling that could
    # not be encoded FAILS the check: skipping it would pass the cell with its
    # premise unverified.
    by_name = {r["name"]: res for r, res in zip(rows, results)}
    for row, res in zip(rows, results):
        other = row.get("differs_from")
        if not other or res[0] == "ERROR":
            continue
        if by_name[other][0] == "ERROR":
            res[3].append((f"differs_from:{other}(errored)", False))
            continue
        same = (root / row["name"] / "rs.obu").read_bytes() == \
            (root / other / "rs.obu").read_bytes()
        res[3].append((f"differs_from:{other}", not same))
    for row, res in zip(rows, results):
        other = row.get("c_differs_from")
        if not other or res[0] == "ERROR":
            continue
        a, b = root / row["name"] / "c.obu", root / other / "c.obu"
        if by_name[other][0] == "ERROR" or not a.exists() or not b.exists():
            res[3].append((f"c_differs_from:{other}(missing)", False))
            continue
        res[3].append((f"c_differs_from:{other}", a.read_bytes() != b.read_bytes()))

    bad = errors = 0
    with open(out, "w", newline="") as f:
        wr = csv.writer(f, delimiter="\t", lineterminator="\n")
        wr.writerow(["name", "verdict", "stage", "detail", "expect", "checks", "ok"])
        for row, (verdict, stage, detail, checks) in zip(rows, results):
            expect = row.get("expect") or ""
            failed = [n for n, ok in checks if not ok]
            ok = (not expect or expect == verdict) and not failed
            pinned = bool(expect) or bool(checks)
            bad += pinned and not ok and verdict != "ERROR"
            errors += verdict == "ERROR"
            wr.writerow([row["name"], verdict, stage, detail, expect or "-",
                         " ".join(f"{n}={'ok' if o else 'FAIL'}" for n, o in checks) or "-",
                         "ERROR" if verdict == "ERROR" else
                         ("yes" if ok else "NO") if pinned else "-"])
    for row, (verdict, stage, detail, _) in zip(rows, results):
        if verdict == "ERROR":
            print(f"cellrun: ERROR {row['name']} [{stage}] {detail}", file=sys.stderr)
    ident = sum(r[0] == "IDENTICAL" for r in results)
    ran_c = sum("c" in r["checks"] for r in rows)
    print(f"cellrun: {oracle}: {ident}/{ran_c} identical to C, {errors} errors, "
          f"{bad} failing a pinned verdict or check -> {out}", file=sys.stderr)
    return 2 if errors else (1 if bad else 0)


if __name__ == "__main__":
    sys.exit(main())

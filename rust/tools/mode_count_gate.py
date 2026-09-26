#!/usr/bin/env python3
"""Motion-mode gates on real video: the port must SELECT a mode where C does,
and its reconstruction must be a decoder's (plan T3).

Shared by warped_motion_gate.sh and obmc_gate.sh. Each cell encodes a
public-domain derf clip with the port alone (tools/cellrun.py, in parallel)
under SVTAV1_INTERDBG=1, then:
  - counts `mm=<MODE>` lines in the port's stderr (the cell's rs.trace) and
    compares them with the cell's FLOOR: at least FLOOR blocks, and a floor of
    0 means C selects none there, so the port must code none either;
  - requires recon == aomdec and dav1d == aomdec on every frame (cellrun's
    `recon` and `dav1d` checks; stronger than the old "dav1d decodes, then
    compare the recon with dav1d").

Usage: mode_count_gate.py --mode WarpedCausal --label "warped motion gate"
           --frames 8 --qp 40 --env 'SVTAV1_HIER_LEVELS=0' CELL...
       CELL = "clip size preset floor", e.g. "vidyo3 256x256 6 20"
Env:   ZENAV1_VIDEO_ASSETS (derf clips), AOMDEC, DAV1D, MODE_JOBS (4).
"""

import argparse
import csv
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", required=True, help="the mm= value to count")
    ap.add_argument("--label", required=True)
    ap.add_argument("--frames", type=int, required=True)
    ap.add_argument("--qp", type=int, required=True)
    ap.add_argument("--env", default="", help="extra port variables, K=V;K=V")
    ap.add_argument("cells", nargs="+")
    a = ap.parse_args()

    assets = Path(os.environ.get("ZENAV1_VIDEO_ASSETS")
                  or Path(os.environ.get("ZENAV1_CORPUS_ROOT", Path.home() / "work/zen"))
                  / "video/pd-derf-720p")
    if not (assets / "vidyo3_128x128_8f.i420").exists():
        print(f"== fetching video assets -> {assets}")
        r = subprocess.run([str(HERE / "fetch_r2_assets.sh"), "video/pd-derf-720p/", str(assets)])
        if r.returncode != 0:
            sys.exit(f"{a.label}: could not obtain assets")

    work = Path(tempfile.mkdtemp(prefix="mode-gate.", dir=os.environ.get("TMPDIR", Path.home() / "tmp")))
    cells, floors, missing = [], {}, []
    env = ";".join(filter(None, [f"SVTAV1_FRAMES={a.frames}", "SVTAV1_INTERDBG=1", a.env]))
    for spec in a.cells:
        clip, size, preset, floor = spec.split()
        w, h = size.split("x")
        asset = assets / f"{clip}_{size}_8f.i420"
        if not asset.exists():
            missing.append(str(asset))
            continue
        name = f"{clip}_{size}_p{preset}"
        floors[name] = int(floor)
        cells.append([name, f"rawseq:{asset}", w, h, str(a.qp), preset, env, "recon,dav1d,inter"])
    lst = work / "mode.cells.tsv"
    with open(lst, "w", newline="") as f:
        wr = csv.writer(f, delimiter="\t", lineterminator="\n")
        wr.writerow(["name", "content", "w", "h", "qp", "preset", "env_port", "check"])
        wr.writerows(cells)
    root = work / "cells"
    run_env = {k: v for k, v in os.environ.items()
               if k not in ("SVTAV1_FRAME_SHIFT", "SVTAV1_FRAME_ZOOM_NUM", "SVTAV1_FRAME_ZOOM_DEN")}
    subprocess.run([sys.executable, str(HERE / "cellrun.py"), str(lst), "--root", str(root),
                    "--out", str(work / "result.tsv"), "--jobs", os.environ.get("MODE_JOBS", "4")],
                   env=run_env, stderr=subprocess.DEVNULL)
    rows = {r["name"]: r for r in csv.DictReader(open(work / "result.tsv"), delimiter="\t")}

    print(f"== {a.label} (public-domain derf clips, qp {a.qp}, {a.frames} frames) ==")
    print(f"{'clip':<26} {'count':<8} {'min':<6} {'checks':<34} note")
    fail, ran = len(missing), 0
    for m in missing:
        print(f"  MISSING ASSET {m}")
    for name, floor in floors.items():
        r = rows[name]
        if r["verdict"] == "ERROR":
            print(f"  {name:<24} ERR      {floor:<6} {'-':<34} {r['detail']}")
            fail += 1
            continue
        ran += 1
        trace = root / name / "rs.trace"
        n = sum(1 for l in open(trace, errors="replace") if f"mm={a.mode}" in l) if trace.exists() else 0
        note = "ok"
        if r["ok"] != "yes":
            note = "RECON/DECODE MISMATCH"
        elif floor == 0 and n != 0:
            note = f"{a.mode} where C selects NONE"
        elif n < floor:
            note = f"selects FEWER {a.mode} blocks than pinned"
        fail += note != "ok"
        print(f"  {name:<24} {n:<8} {floor:<6} {r['checks']:<34} {note}")
    print(f"\ncells run: {ran} of {len(a.cells)}   failed: {fail}")
    if ran != len(a.cells):
        sys.exit(f"ANTI-VACUITY FAIL: {ran} of {len(a.cells)} cells actually ran")
    if fail:
        sys.exit(1)
    print(f"{a.label}: OK")


if __name__ == "__main__":
    main()

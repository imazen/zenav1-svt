#!/usr/bin/env python3
"""Paired still-image timings from prebuilt, internally timed API drivers.

Cells TSV: name, yuv, width, height, qp, preset (tab-separated header).
No build occurs here. Every sample's output is byte-compared, and failed or
missing samples fail the run instead of disappearing from its statistics.
Use --reference-kind port for a same-binary control or a port A/B comparison.
Use --port-kind c --reference-kind c to measure two C drivers. The historical
port/reference column names denote slots; metadata identifies each driver kind.
The ratio is always port/reference; output metadata records the exact binaries.
"""

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import re
import statistics
import subprocess
import time


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", type=Path, required=True)
    ap.add_argument("--port-kind", choices=["port", "c"], default="port")
    ap.add_argument("--reference", type=Path, required=True)
    ap.add_argument("--reference-kind", choices=["c", "port"], default="c")
    ap.add_argument("--cells", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--rounds", type=int, default=21)
    ap.add_argument("--cpu", type=int, required=True)
    ap.add_argument("--seed", type=int, default=20260906)
    args = ap.parse_args()
    if args.rounds < 4:
        ap.error("at least four paired rounds are required")
    os.sched_setaffinity(0, {args.cpu})
    # No ambient diagnostic/tuning override may change one encoder's workload.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SVT_", "SVTAV1_"))}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    work = args.out.parent / (args.out.stem + "-artifacts")
    work.mkdir(exist_ok=False)
    rng = random.Random(args.seed)
    cells = list(csv.DictReader(args.cells.open(), delimiter="\t"))
    if not cells:
        ap.error("empty cell manifest")
    meta = {
        "date_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "host": platform.node(), "platform": platform.platform(),
        "affinity": sorted(os.sched_getaffinity(0)), "nice": os.nice(0),
        "rounds": args.rounds, "seed": args.seed,
        "port_kind": args.port_kind, "reference_kind": args.reference_kind,
        "port": str(args.port.resolve()), "port_sha256": sha(args.port),
        "reference": str(args.reference.resolve()),
        "reference_sha256": sha(args.reference),
        "cells": cells, "input_sha256": {c["yuv"]: sha(c["yuv"]) for c in cells},
        "timing": "driver ENCODE_NS; setup and file I/O excluded; one warmup",
        "source_commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True).strip(),
        "source_diff": subprocess.check_output(["git", "diff", "--stat"], text=True),
    }
    args.out.with_suffix(".meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    with args.out.with_suffix(".raw.tsv").open("x") as raw, args.out.open("x") as summary:
        rw = csv.writer(raw, delimiter="\t", lineterminator="\n")
        sw = csv.writer(summary, delimiter="\t", lineterminator="\n")
        rw.writerow(["name", "round", "first", "port_ns", "reference_ns", "ident"])
        sw.writerow(["name", "n", "port_ms", "reference_ms", "ratio", "p25", "p75", "ident"])
        for cell in cells:
            w, h, qp, preset = [cell[k] for k in ("width", "height", "qp", "preset")]
            if Path(cell["yuv"]).stat().st_size != int(w) * int(h) * 3 // 2:
                raise ValueError(f"input size mismatch for {cell['name']}")
            samples = []
            for round_index in range(args.rounds):
                order = ["port", "reference"]
                rng.shuffle(order)
                ns, outputs = {}, {}
                for arm in order:
                    prefix = work / f"{cell['name']}-{arm}"
                    is_port = (args.port_kind if arm == "port" else args.reference_kind) == "port"
                    binary = args.port if arm == "port" else args.reference
                    argv = ([str(binary), "raw:" + cell["yuv"], w, h, qp, preset,
                             str(prefix), "1"] if is_port else
                            [str(binary), w, h, qp, preset, cell["yuv"],
                             str(prefix) + ".obu", "1"])
                    with (work / f"{cell['name']}-{arm}.log").open("a") as log:
                        result = subprocess.run(argv, env=env, text=True,
                                                stdout=subprocess.PIPE, stderr=log)
                        log.write(result.stdout)
                    result.check_returncode()
                    match = re.fullmatch(r"ENCODE_NS=(\d+) BYTES=(\d+)(?: FRAMES=1)?\s*", result.stdout)
                    if not match or int(match[1]) == 0:
                        raise RuntimeError(f"invalid timing output: {result.stdout!r}")
                    ns[arm] = int(match[1])
                    outputs[arm] = Path(str(prefix) + ".obu").read_bytes()
                    if len(outputs[arm]) != int(match[2]) or not outputs[arm]:
                        raise RuntimeError("output size does not match driver report")
                identical = outputs["port"] == outputs["reference"]
                rw.writerow([cell["name"], round_index, order[0], ns["port"],
                             ns["reference"], "Y" if identical else "N"])
                raw.flush()
                if not identical:
                    raise RuntimeError(f"{cell['name']}: differing outputs; no performance claim")
                samples.append((ns["port"], ns["reference"]))
                print(f"{cell['name']} round {round_index + 1}/{args.rounds} ident=Y", flush=True)
            ratios = [p / c for p, c in samples]
            quartiles = statistics.quantiles(ratios, n=4, method="inclusive")
            row = [cell["name"], len(samples), statistics.median(p for p, _ in samples) / 1e6,
                   statistics.median(c for _, c in samples) / 1e6,
                   statistics.median(ratios), quartiles[0], quartiles[2], "Y"]
            sw.writerow(row)
            summary.flush()
            print(f"RESULT {cell['name']}: port/reference={row[4]:.4f} "
                  f"[{row[5]:.4f}, {row[6]:.4f}]", flush=True)


if __name__ == "__main__":
    main()

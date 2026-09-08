#!/usr/bin/env python3
"""Compare retained default identity cells with an independently built C app.

Run under run-heavy. This is a correctness check, never a performance benchmark.
Inputs must come from identity_full_8bit.sh with no coding-setting environment
overrides: its settings.txt records only the six axes accepted here. The source
and build of --app must be recorded separately; a binary hash identifies the
actual executable but does not prove that its source was pristine.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    cells = sorted(args.artifacts.resolve(strict=True).glob("cell.*/settings.txt"))
    if not cells:
        parser.error("no retained identity cells found")
    # A new directory prevents stale encoded outputs from passing a failed run.
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    (output / "provenance.json").write_text(json.dumps({
        "schema": 1, "app": str(app), "app_sha256": digest(app),
        "artifacts": str(args.artifacts.resolve()), "cells": len(cells),
        "scope": "default still CQP 420; no unrecorded coding overrides",
        "timing_claim": False,
    }, indent=2) + "\n")
    failures = 0
    # Avoid accidental trace/debug or fork environment settings in this run.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SVT_", "SVTAV1_"))}
    with (output / "results.jsonl").open("w") as rows:
        for index, settings_path in enumerate(cells, 1):
            settings = dict(line.split("=", 1)
                            for line in settings_path.read_text().splitlines())
            if set(settings) != {"content", "width", "height", "qp", "preset", "bit_depth"}:
                raise ValueError(f"unrecognized settings in {settings_path}")
            cell = settings_path.parent
            dest = output / cell.name
            dest.mkdir()
            encoded = dest / "pristine.obu"
            argv = [str(app), "-i", str(cell / "rs.yuv"), "-b", str(encoded),
                    "-w", settings["width"], "-h", settings["height"], "-n", "1",
                    "--input-depth", settings["bit_depth"], "--preset", settings["preset"],
                    "--rc", "0", "--aq-mode", "0", "--qp", settings["qp"],
                    "--avif", "1", "--lp", "1", "--fps-num", "30", "--fps-denom", "1",
                    "--progress", "0"]
            row = {"cell": cell.name, "settings": settings, "argv": argv,
                   "input_sha256": digest(cell / "rs.yuv"),
                   "hybrid_sha256": digest(cell / "c.obu"),
                   "rust_sha256": digest(cell / "rs.obu")}
            with (dest / "encode.log").open("wb") as log:
                try:
                    result = subprocess.run(argv, env=env, stdout=log, stderr=log,
                                            timeout=args.timeout, check=False)
                    row["exit_code"] = result.returncode
                    valid = result.returncode == 0 and encoded.is_file() and encoded.stat().st_size > 0
                    row["verdict"] = "ERROR"
                    if valid:
                        row["pristine_sha256"] = digest(encoded)
                        row["bytes"] = encoded.stat().st_size
                        row["verdict"] = "IDENTICAL" if (
                            row["pristine_sha256"] == row["hybrid_sha256"] == row["rust_sha256"]
                        ) else "DIFF"
                except subprocess.TimeoutExpired:
                    row["verdict"] = "TIMEOUT"
            failures += row["verdict"] != "IDENTICAL"
            rows.write(json.dumps(row, sort_keys=True) + "\n")
            rows.flush()
            if index % 50 == 0 or row["verdict"] != "IDENTICAL":
                print(f"{index}/{len(cells)} compared, {failures} failures", flush=True)
    print(f"{len(cells) - failures}/{len(cells)} pristine/hybrid/Rust byte-identical", flush=True)
    return int(failures != 0)


if __name__ == "__main__":
    raise SystemExit(main())

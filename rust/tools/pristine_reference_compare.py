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
    parser.add_argument("--rust-runner", type=Path,
                        help="always-fresh identity_run wrapper; rerun Rust with pristine source identity")
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    cells = sorted(args.artifacts.resolve(strict=True).glob("cell.*/settings.txt"))
    if not cells:
        parser.error("no retained identity cells found")
    # A new directory prevents stale encoded outputs from passing a failed run.
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    provenance = {
        "schema": 1, "app": str(app), "app_sha256": digest(app),
        "artifacts": str(args.artifacts.resolve()), "cells": len(cells),
        "scope": "default still CQP 420; no unrecorded coding overrides",
        "timing_claim": False,
        "rust_runner": str(args.rust_runner.resolve(strict=True)) if args.rust_runner else None,
        "rust_reference": "svt-mainline-4.2.0-9292ec8e32bce26f781f277ec8739b53426c4300" if args.rust_runner else None,
    }
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
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
            if args.rust_runner:
                runner = args.rust_runner.resolve(strict=True)
                rust_argv = [str(runner), settings["content"], settings["width"], settings["height"],
                             settings["qp"], settings["preset"], str(dest / "rs")]
                row["rust_argv"] = rust_argv
                row["baseline_rust_sha256"] = row.pop("rust_sha256")
                rust_env = dict(env, SVTAV1_BD=settings["bit_depth"],
                                SVTAV1_REFERENCE="svt-mainline-4.2.0-9292ec8e32bce26f781f277ec8739b53426c4300")
                with (dest / "rust.log").open("wb") as log:
                    try:
                        result = subprocess.run(rust_argv, env=rust_env, cwd=runner.parent.parent,
                                                stdout=log, stderr=log, timeout=args.timeout, check=False)
                        row["rust_exit_code"] = result.returncode
                        if result.returncode != 0:
                            row["rust_error"] = "encode failed"
                        elif digest(dest / "rs.yuv") != row["input_sha256"]:
                            row["rust_error"] = "regenerated input differs from retained input"
                        else:
                            row["rust_sha256"] = digest(dest / "rs.obu")
                            # identity_run always executes this path after its
                            # Cargo freshness check. Hash the actual binary,
                            # not merely the wrapper script or a commit name.
                            binary = runner.parent.parent / "target/release/examples/identity_run"
                            binary_hash = digest(binary)
                            if "rust_binary_sha256" not in provenance:
                                provenance["rust_binary_sha256"] = binary_hash
                                (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
                            elif binary_hash != provenance["rust_binary_sha256"]:
                                row["rust_error"] = "Rust binary changed during comparison"
                    except subprocess.TimeoutExpired:
                        row["rust_error"] = "timeout"
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
                        matched = row["pristine_sha256"] == row.get("rust_sha256")
                        if not args.rust_runner:
                            matched &= row["pristine_sha256"] == row["hybrid_sha256"]
                        row["verdict"] = "IDENTICAL" if matched else "DIFF"
                        if row.get("rust_error"):
                            row["verdict"] = "RUST_ERROR"
                except subprocess.TimeoutExpired:
                    row["verdict"] = "TIMEOUT"
            failures += row["verdict"] != "IDENTICAL"
            rows.write(json.dumps(row, sort_keys=True) + "\n")
            rows.flush()
            if index % 50 == 0 or row["verdict"] != "IDENTICAL":
                print(f"{index}/{len(cells)} compared, {failures} failures", flush=True)
    comparison = "pristine/Rust" if args.rust_runner else "pristine/hybrid/Rust"
    print(f"{len(cells) - failures}/{len(cells)} {comparison} byte-identical", flush=True)
    return int(failures != 0)


if __name__ == "__main__":
    raise SystemExit(main())

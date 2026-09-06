#!/usr/bin/env python3
"""Build baseline and PGO still drivers from one frozen source tree.

Run under run-heavy. Training cells use still_pairs.py's TSV schema.
Every instrumented training output must equal the ordinary baseline output.
Evaluate the resulting binaries separately on held-out images.
"""

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--cells", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--cpu", required=True, type=int)
    args = ap.parse_args()
    workspace = Path(__file__).resolve().parents[2]
    out = args.out.resolve()
    cells = list(csv.DictReader(args.cells.open(), delimiter="\t"))
    if not cells:
        ap.error("training manifest is empty")
    names = [c["name"] for c in cells]
    if len(names) != len(set(names)):
        ap.error("duplicate cell names")
    for c in cells:
        if Path(c["name"]).name != c["name"]:
            ap.error("cell names must be plain file names")
        if Path(c["yuv"]).stat().st_size != int(c["width"]) * int(c["height"]) * 3 // 2:
            ap.error(f"bad I420 size: {c['name']}")
    out.mkdir(parents=True, exist_ok=False)
    profiles = out / "profiles"
    profiles.mkdir()
    artifacts = out / "training"
    artifacts.mkdir()
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SVT_", "SVTAV1_", "CARGO_PROFILE_"))
           and k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "LLVM_PROFILE_FILE")}
    env.update(CARGO_TARGET_DIR=str(out / "target"), CARGO_INCREMENTAL="0",
               CARGO_PROFILE_RELEASE_OPT_LEVEL="3",
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS="16",
               CARGO_PROFILE_RELEASE_LTO="false", CARGO_BUILD_JOBS="4")
    target = "x86_64-unknown-linux-gnu"
    commands = []
    metadata = {
        "date_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "source_commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=workspace, text=True).strip(),
        "source_diff": subprocess.check_output(
            ["git", "diff", "--stat"], cwd=workspace, text=True),
        "rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
        "target": target, "target_cpu": "baseline", "cpu": args.cpu,
        "opt_level": 3, "codegen_units": 16, "lto": False,
        "cells": cells, "input_sha256": {c["yuv"]: sha(c["yuv"]) for c in cells},
        "commands": commands,
    }

    def save():
        (out / "meta.json").write_text(json.dumps(metadata, indent=2) + "\n")

    def run(argv, label, extra=None):
        call_env = env | (extra or {})
        commands.append({"argv": list(map(str, argv)), "label": label,
                         "RUSTFLAGS": call_env.get("RUSTFLAGS", ""),
                         "LLVM_PROFILE_FILE": call_env.get("LLVM_PROFILE_FILE")})
        save()
        print(label, flush=True)
        with (out / (label + ".log")).open("w") as log:
            subprocess.run(argv, cwd=workspace, env=call_env,
                           stdout=log, stderr=subprocess.STDOUT, check=True)

    build = ["cargo", "build", "--locked", "--release", "--target", target,
             "-p", "zenav1-svt", "--example", "perf_encode", "-j", "4"]
    executable = out / "target" / target / "release/examples/perf_encode"
    run(build, "build-baseline")
    baseline = out / "perf_encode.baseline"
    shutil.copy2(executable, baseline)
    run(build, "build-instrumented", {"RUSTFLAGS": f"-Cprofile-generate={profiles}"})
    instrumented = out / "perf_encode.instrumented"
    shutil.copy2(executable, instrumented)
    outputs = []
    for index, cell in enumerate(cells):
        params = [cell[k] for k in ("width", "height", "qp", "preset")]
        encoded = []
        for arm, binary in (("baseline", baseline), ("instrumented", instrumented)):
            prefix = artifacts / (cell["name"] + "-" + arm)
            argv = ["taskset", "-c", str(args.cpu), str(binary),
                    "raw:" + cell["yuv"], *params, str(prefix), "0"]
            run(argv, f"train-{index:03d}-{arm}",
                {"LLVM_PROFILE_FILE": str(profiles / "%m-%p.profraw")})
            encoded.append(Path(str(prefix) + ".obu"))
        if encoded[0].read_bytes() != encoded[1].read_bytes():
            raise RuntimeError(f"instrumentation changed bytes: {cell['name']}")
        outputs.append({"name": cell["name"], "obu_sha256": sha(encoded[0])})
        print(f"training {index + 1}/{len(cells)} ident=Y", flush=True)
    raw = sorted(profiles.glob("*.profraw"))
    if not raw or any(p.stat().st_size == 0 for p in raw):
        raise RuntimeError("missing or empty profile files")
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    profdata = sysroot / "lib/rustlib" / target / "bin/llvm-profdata"
    merged = out / "merged.profdata"
    run([str(profdata), "merge", "-o", str(merged), *map(str, raw)], "merge-profiles")
    run([str(profdata), "show", "--all-functions", "--counts", str(merged)], "profile-counts")
    run(build, "build-pgo", {"RUSTFLAGS":
        f"-Cprofile-use={merged} -Cllvm-args=-pgo-warn-missing-function"})
    optimized = out / "perf_encode.pgo"
    shutil.copy2(executable, optimized)
    metadata.update(training_outputs=outputs, profile_sha256=sha(merged),
                    raw_profiles=len(raw), baseline_sha256=sha(baseline),
                    instrumented_sha256=sha(instrumented), pgo_sha256=sha(optimized))
    save()
    print(f"READY {baseline} {optimized}", flush=True)


if __name__ == "__main__":
    main()

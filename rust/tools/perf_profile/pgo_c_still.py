#!/usr/bin/env python3
"""Train GCC PGO for the pinned C oracle on the same still cells as Rust.

Run under run-heavy. Builds and profile data stay in a new output directory.
The ordinary reference library and submodule source remain untouched.
"""

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--cells", type=Path, required=True)
    ap.add_argument("--baseline", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--cpu", type=int, required=True)
    args = ap.parse_args()
    root = Path(__file__).resolve().parents[3]
    source = root / "reference/svt-av1"
    out = args.out.resolve()
    cells = list(csv.DictReader(args.cells.open(), delimiter="\t"))
    if not cells:
        ap.error("empty training manifest")
    names = [c["name"] for c in cells]
    if len(names) != len(set(names)) or any(Path(n).name != n for n in names):
        ap.error("training cell names must be unique plain file names")
    out.mkdir(parents=True, exist_ok=False)
    profiles, artifacts = out / "profiles", out / "training"
    profiles.mkdir()
    artifacts.mkdir()
    baseline = out / "perf_c_encode.baseline"
    shutil.copy2(args.baseline, baseline)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SVT_", "SVTAV1_"))
           and k not in ("CFLAGS", "CXXFLAGS", "LDFLAGS", "GCOV_PREFIX", "GCOV_PREFIX_STRIP")}
    metadata = {
        "c_source": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=source, text=True).strip(),
        "compiler": subprocess.check_output(["cc", "--version"], text=True),
        "baseline_sha256": sha(baseline), "cells": cells,
        "input_sha256": {c["yuv"]: sha(c["yuv"]) for c in cells},
        "commands": [], "training_outputs": [],
    }

    def save():
        (out / "meta.json").write_text(json.dumps(metadata, indent=2) + "\n")

    def run(argv, label):
        metadata["commands"].append({"argv": list(map(str, argv)), "label": label})
        save()
        print(label, flush=True)
        with (out / (label + ".log")).open("w") as log:
            subprocess.run(argv, cwd=root, env=env, stdout=log,
                           stderr=subprocess.STDOUT, check=True)

    executable = out / "perf_c_encode"

    def build(phase, flags):
        run(["cmake", "-S", str(source), "-B", str(out / "build"), "-G", "Ninja",
             "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_C_COMPILER=/usr/bin/cc",
             "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_APPS=OFF", "-DBUILD_TESTING=OFF",
             "-DNATIVE=OFF", "-DENABLE_AVX512=ON", "-DSVT_AV1_LTO=OFF",
             "-DSVT_HDR_MODE=OFF", f"-DCMAKE_OUTPUT_DIRECTORY={out / 'lib'}",
             "-DCMAKE_C_FLAGS=" + " ".join(flags)], "configure-" + phase)
        run(["cmake", "--build", str(out / "build"), "-j", "4"], "build-" + phase)
        includes = ["API", "Lib/Codec", "Lib/Globals", "Lib/C_DEFAULT"]
        run(["cc", "-O2", *flags, "-o", str(executable),
             str(root / "rust/tools/perf_c_encode/perf_c_encode.c"),
             *["-I" + str(source / "Source" / p) for p in includes],
             str(out / "lib/libSvtAv1Enc.a"), "-lpthread", "-lm"], "link-" + phase)

    build("instrumented", [f"-fprofile-generate={profiles}", "-fprofile-update=atomic"])
    instrumented = out / "perf_c_encode.instrumented"
    shutil.copy2(executable, instrumented)
    for i, cell in enumerate(cells):
        params = [cell[k] for k in ("width", "height", "qp", "preset", "yuv")]
        outputs = []
        for arm, binary in (("baseline", baseline), ("instrumented", instrumented)):
            obu = artifacts / (cell["name"] + "-" + arm + ".obu")
            run(["taskset", "-c", str(args.cpu), str(binary), *params, str(obu), "0"],
                f"train-{i:03d}-{arm}")
            outputs.append(obu)
        if outputs[0].read_bytes() != outputs[1].read_bytes():
            raise RuntimeError(f"C instrumentation changed bytes: {cell['name']}")
        metadata["training_outputs"].append({"name": cell["name"], "obu_sha256": sha(outputs[0])})
        save()
        print(f"training {i + 1}/{len(cells)} ident=Y", flush=True)
    raw = sorted(profiles.rglob("*.gcda"))
    if not raw or any(p.stat().st_size == 0 for p in raw):
        raise RuntimeError("missing or empty GCC profiles")
    metadata["profiles"] = {str(p): sha(p) for p in raw}
    run(["gcov-dump", "-l", *map(str, raw)], "profile-counts")
    build("pgo", [f"-fprofile-use={profiles}"])
    optimized = out / "perf_c_encode.pgo"
    shutil.copy2(executable, optimized)
    metadata.update(instrumented_sha256=sha(instrumented), pgo_sha256=sha(optimized))
    save()
    print(f"READY {baseline} {optimized}", flush=True)


if __name__ == "__main__":
    main()

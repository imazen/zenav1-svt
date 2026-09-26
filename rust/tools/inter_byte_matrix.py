#!/usr/bin/env python3
"""The inter campaign's byte-identity FRONTIER sweep against C (plan T3 port).

`inter_byte_gate.sh` pins the cells that are byte-identical; this sweeps the
grid behind it and reports, per cell, the FIRST temporal unit where the
port's stream leaves C's. Cells run in parallel through tools/cellrun.py
(port + C, byte comparison); both streams are then split on temporal
delimiters, so the verdict names a frame whatever the frame count:

  IDENTICAL   every temporal unit byte-identical
  TU<k>       units 0..k-1 identical, unit k differs. TU0 is a KEY-frame
              defect, and every later reading behind it is meaningless;
              TU1+ is an inter-decision defect
  ERROR       the port or C did not encode (a port panic is a defect, not a
              frontier state: the sweep exits 1 on one)

Content is a synthetic class (uniform gradient diag screen screenrep; later
frames are frame 0 shifted by IBM_SHIFT px, the motion open-loop ME finds
exactly) or a public-domain derf clip (fourpeople johnny kristenandsara
vidyo1 vidyo3 vidyo4; 8-frame I420 at 128x128 or 256x256 from the R2 prefix
video/pd-derf-720p/, fetched when missing) — real motion, which is where the
port and C part.

Usage: tools/inter_byte_matrix.sh [outdir]
Env:   IBM_CONTENT  classes and/or clips (default: uniform gradient diag screen)
       IBM_SIZES    square sizes (default 16 64 72 128; clips: 128 and 256)
       IBM_QPS      (default 20 40 55)
       IBM_PRESETS  (default 6 8)
       IBM_FRAMES   frames per cell, low delay (default 2; clips have 8)
       IBM_SHIFT    synthetic px/frame (default 3)
       IBM_JOBS     parallel cells (default 4)
Output: TSV on stdout (content size qp preset frames c_bytes port_bytes
verdict), cell artifacts under outdir (default target/inter-byte-matrix),
and a summary line.
"""

import csv
import os
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
RS_ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import sb128_seqhdr  # noqa: E402

CLIPS = {"fourpeople", "johnny", "kristenandsara", "vidyo1", "vidyo3", "vidyo4"}
OBU_TEMPORAL_DELIMITER = 2


def env_list(name, default):
    return os.environ.get(name, default).split()


def temporal_units(data):
    """The stream split at each temporal delimiter OBU, as byte strings."""
    units, start, i = [], 0, 0
    while i < len(data):
        h = data[i]
        obu_type = (h >> 3) & 0xF
        j = i + 1 + ((h >> 2) & 1)
        if (h >> 1) & 1:
            size, n = sb128_seqhdr.leb128(data, j)
            j += n
        else:
            size = len(data) - j
        if obu_type == OBU_TEMPORAL_DELIMITER and i > start:
            units.append(data[start:i])
            start = i
        i = j + size
    if start < len(data):
        units.append(data[start:])
    return units


def main():
    out = Path(sys.argv[1] if len(sys.argv) > 1 else RS_ROOT / "target/inter-byte-matrix")
    contents = env_list("IBM_CONTENT", "uniform gradient diag screen")
    sizes = env_list("IBM_SIZES", "16 64 72 128")
    qps = env_list("IBM_QPS", "20 40 55")
    presets = env_list("IBM_PRESETS", "6 8")
    frames = int(os.environ.get("IBM_FRAMES", "2"))
    shift = os.environ.get("IBM_SHIFT", "3")

    assets = Path(os.environ.get("ZENAV1_VIDEO_ASSETS")
                  or Path(os.environ.get("ZENAV1_CORPUS_ROOT", Path.home() / "work/zen"))
                  / "video/pd-derf-720p")
    if any(c in CLIPS for c in contents) and not (assets / "vidyo3_128x128_8f.i420").exists():
        # stdout is the TSV: the fetch's progress goes to stderr (on a fresh CI
        # runner it once landed above the header and broke every consumer).
        r = subprocess.run([str(HERE / "fetch_r2_assets.sh"), "video/pd-derf-720p/", str(assets)],
                           stdout=sys.stderr)
        if r.returncode != 0:
            sys.exit("inter_byte_matrix: could not fetch the derf clips")

    port_env = f"SVTAV1_FRAMES={frames};SVTAV1_INTRA_PERIOD=64;SVTAV1_HIER_LEVELS=0"
    c_env = f"SVT_FRAMES={frames};SVT_INTRA_PERIOD=-1;SVT_HIER_LEVELS=0;SVT_PRED_STRUCT=1"
    cells, meta = [], {}
    for content in contents:
        for s in sizes:
            if content in CLIPS:
                if frames > 8:
                    sys.exit("inter_byte_matrix: the derf clips have 8 frames")
                asset = assets / f"{content}_{s}x{s}_8f.i420"
                if not asset.exists():
                    continue  # the clips exist at 128 and 256 only
                src, penv = f"rawseq:{asset}", port_env
            else:
                src, penv = content, f"{port_env};SVTAV1_FRAME_SHIFT={shift}"
            for q in qps:
                for p in presets:
                    name = f"{content}_{s}x{s}_q{q}_p{p}"
                    meta[name] = (content, s, q, p)
                    cells.append([name, src, s, s, q, p, penv, c_env, "c"])
    if not cells:
        sys.exit("inter_byte_matrix: no cell to run (clips exist at 128 and 256 only)")

    work = Path(tempfile.mkdtemp(prefix="ibm.", dir=os.environ.get("TMPDIR", Path.home() / "tmp")))
    lst = work / "inter_byte_matrix.cells.tsv"
    with open(lst, "w", newline="") as f:
        w = csv.writer(f, delimiter="\t", lineterminator="\n")
        w.writerow(["name", "content", "w", "h", "qp", "preset", "env_port", "env_c", "check"])
        w.writerows(cells)
    subprocess.run([sys.executable, str(HERE / "cellrun.py"), str(lst), "--root", str(out),
                    "--out", str(work / "result.tsv"), "--bytes-only",
                    "--jobs", os.environ.get("IBM_JOBS", "4")], stderr=subprocess.DEVNULL)
    rows = {r["name"]: r for r in csv.DictReader(open(work / "result.tsv"), delimiter="\t")}

    print("content\tsize\tqp\tpreset\tframes\tc_bytes\tport_bytes\tverdict")
    tally, port_errors = Counter(), 0
    for name, (content, s, q, p) in meta.items():
        r, d = rows[name], out / name
        rs, c = d / "rs.obu", d / "c.obu"
        if r["verdict"] == "ERROR" or not rs.exists() or not c.exists():
            v = "ERROR"
            port_errors += r.get("stage") == "PORT"
            cb = pb = "-"
            print(f"{content}\t{s}\t{q}\t{p}\t{frames}\t{cb}\t{pb}\t{v}\t{r['detail']}")
        else:
            a, b = rs.read_bytes(), c.read_bytes()
            ua, ub = temporal_units(a), temporal_units(b)
            k = next((i for i in range(max(len(ua), len(ub)))
                      if i >= len(ua) or i >= len(ub) or ua[i] != ub[i]), None)
            v = "IDENTICAL" if k is None else f"TU{k}"
            print(f"{content}\t{s}\t{q}\t{p}\t{frames}\t{len(b)}\t{len(a)}\t{v}")
        tally[v] += 1
    total = sum(tally.values())
    print("# " + " / ".join(f"{n} {v}" for v, n in sorted(tally.items())) + f" of {total}")
    sys.exit(1 if port_errors else 0)


if __name__ == "__main__":
    main()

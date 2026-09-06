#!/usr/bin/env python3
"""Reproduce the i265 still evaluation or disjoint PGO training crops.

ImageMagick decodes center crops to RGB8. Integer RGB-to-I420 conversion
matches identity_run. No resizing, padding, or synthetic upscaling occurs.
"""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import struct
import subprocess

DATASETS = {
    "evaluation": [
        ("photo_cid", "CID22/CID22-512/training/3571065.png", [256, 512]),
        ("photo_clic", "clic2025/training/ef576c4ed599d75d72145a8f34b58ccb.png", [256, 512, 1024]),
        ("screen_terminal", "gb82-sc/terminal.png", [256, 512, 1024]),
        ("photo_waves", "gb82/waves-lossless.png", [256, 512]),
        ("photo_nyc", "gb82/nyc-lossless.png", [256, 512]),
        ("screen_wiki", "gb82-sc/codec_wiki.png", [256, 512, 1024]),
    ],
    "training": [
        ("train_rain", "gb82/rain-lossless.png", [64, 256, 512]),
        ("train_sunset", "gb82/sunset-lossless.png", [64, 256, 512]),
        ("train_night", "gb82/night-lossless.png", [64, 256, 512]),
        ("train_windows95", "gb82-sc/windows95.png", [64, 256, 480]),
    ],
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def i420(rgb, w, h):
    assert w > 0 and h > 0 and w % 2 == h % 2 == 0
    assert len(rgb) == w * h * 3
    y = bytearray(w * h)
    u = bytearray(w * h // 4)
    v = bytearray(w * h // 4)
    for i in range(w * h):
        rr, gg, bb = rgb[3 * i:3 * i + 3]
        y[i] = ((66 * rr + 129 * gg + 25 * bb + 128) >> 8) + 16
    for r in range(0, h, 2):
        for c in range(0, w, 2):
            i = 3 * (r * w + c)
            offsets = (i, i + 3, i + 3 * w, i + 3 * w + 3)
            rr, gg, bb = ((sum(rgb[j + k] for j in offsets) + 2) // 4 for k in range(3))
            n = (r // 2) * (w // 2) + c // 2
            u[n] = ((-38 * rr - 74 * gg + 112 * bb + 128) >> 8) + 128
            v[n] = ((112 * rr - 94 * gg - 18 * bb + 128) >> 8) + 128
    return bytes(y + u + v)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dataset", choices=DATASETS, required=True)
    ap.add_argument("--corpus", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--verify-initial", type=Path,
                    help="directory containing the three original Rust-generated 512 YUVs")
    args = ap.parse_args()
    sources = []
    for name, relative, sizes in DATASETS[args.dataset]:
        source = (args.corpus / relative).resolve()
        png = source.read_bytes()
        assert png[:8] == b"\x89PNG\r\n\x1a\n" and png[12:16] == b"IHDR"
        pw, ph = struct.unpack(">II", png[16:24])
        assert min(pw, ph) >= max(sizes), (source, pw, ph, sizes)
        sources.append((name, source, sizes, png, pw, ph))
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    metadata, cells = [], []
    for name, source, sizes, png, pw, ph in sources:
        for size in sizes:
            x, y = (pw - size) // 2, (ph - size) // 2
            command = ["magick", str(source), "-crop", f"{size}x{size}+{x}+{y}",
                       "+repage", "-alpha", "off", "-depth", "8", "rgb:-"]
            rgb = subprocess.run(command, check=True, stdout=subprocess.PIPE).stdout
            raw = i420(rgb, size, size)
            if args.verify_initial and size == 512 and name in (
                    "photo_cid", "photo_clic", "screen_terminal"):
                assert raw == (args.verify_initial / f"{name}.yuv").read_bytes(), name
            destination = out / f"{name}-{size}.yuv"
            destination.write_bytes(raw)
            metadata.append({"name": name, "source": str(source), "source_sha256": digest(png),
                             "source_dimensions": [pw, ph], "crop": [x, y, size, size],
                             "yuv": str(destination), "yuv_sha256": digest(raw),
                             "rgb_decode_command": command, "bytes": len(raw)})
            print(name, size, digest(raw), flush=True)
            for qp in (20, 40, 60):
                for preset in (2, 6, 8):
                    cells.append([f"{name}-{size}-q{qp}-p{preset}", str(destination),
                                  size, size, qp, preset])
    assert len(cells) == {"evaluation": 135, "training": 108}[args.dataset]
    with (out / "cells.tsv").open("x") as f:
        writer = csv.writer(f, delimiter="\t", lineterminator="\n")
        writer.writerow(["name", "yuv", "width", "height", "qp", "preset"])
        writer.writerows(cells)
    (out / "inputs.meta.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()

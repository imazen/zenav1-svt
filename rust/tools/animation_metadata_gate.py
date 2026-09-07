#!/usr/bin/env python3
"""Independent libavif metadata gate. Build animation_probe and avif_metadata_probe.c first.

Run under scripts/run-heavy; arguments are the fresh Rust example, C probe, and
an ICC profile fixture. All artifacts go in a temporary directory.
"""
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


def metadata_boxes(data):
    # Walk box boundaries, including full-box and visual-sample-entry headers.
    # Never search compressed payloads for four-character strings.
    containers = {b"moov": 0, b"trak": 0, b"mdia": 0, b"minf": 0,
                  b"stbl": 0, b"stsd": 8, b"av01": 78,
                  b"meta": 4, b"iprp": 0, b"ipco": 0}
    def walk(start, end, path):
        while start < end:
            if end - start < 8:
                raise AssertionError("truncated box header")
            size, kind = struct.unpack_from(">I4s", data, start)
            if size < 8 or start + size > end:
                raise AssertionError("invalid box extent")
            here = path + (kind,)
            if kind == b"mdcv":
                yield here, data[start + 8:start + size]
            if kind in containers:
                yield from walk(start + 8 + containers[kind], start + size, here)
            start += size
    return list(walk(0, len(data), ()))


def main():
    encoder, decoder, profile = (str(Path(p).resolve()) for p in sys.argv[1:])
    expected = {
        "icc": Path(profile).read_bytes().hex(),
        "exif": b"II*\0\x08\0\0\0\0\0\0\0\0\0".hex(),
        "xmp": b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><probe>animation</probe></x:xmpmeta>'.hex(),
        "cicp": "1,13,1,0", "clli": "1000,400",
    }
    count = 0
    with tempfile.TemporaryDirectory(prefix="avif-metadata-") as directory:
        for repeat in ["0", "2", "infinite"]:
            for has_alpha, premultiplied in [(False, False), (True, False), (True, True)]:
                env = os.environ.copy()
                for key in ["AVIF_REPEAT", "AVIF_PREMULTIPLIED", "AVIF_METADATA", "AVIF_ICC", "AVIF_NO_ALPHA"]:
                    env.pop(key, None)
                env.update(AVIF_REPEAT=repeat, AVIF_METADATA="1", AVIF_ICC=profile)
                if not has_alpha:
                    env["AVIF_NO_ALPHA"] = "1"
                if premultiplied:
                    env["AVIF_PREMULTIPLIED"] = "1"
                output = str(Path(directory) / "animation.avif")
                subprocess.run([encoder, output], env=env, check=True, capture_output=True)
                mastering = metadata_boxes(Path(output).read_bytes())
                expected_mdcv = struct.pack(">8H2I", 13250, 34500, 7500, 3000,
                                            34000, 16000, 15635, 16450, 10000000, 50)
                expected_paths = {
                    (b"meta", b"iprp", b"ipco", b"mdcv"),
                    (b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01", b"mdcv"),
                }
                if len(mastering) != 2 or {p for p, _ in mastering} != expected_paths:
                    raise AssertionError("MDCV must appear on both color poster and track")
                if any(payload != expected_mdcv for _, payload in mastering):
                    raise AssertionError("MDCV payload mismatch")
                for poster in [False, True]:
                    args = [decoder, output] + (["poster"] if poster else [])
                    result = subprocess.run(args, check=True, capture_output=True, text=True)
                    actual = dict(line.split("=", 1) for line in result.stdout.splitlines())
                    wanted = dict(expected, alpha=str(int(has_alpha)), frames="1" if poster else "3",
                                  repeat="0" if poster else ("-1" if repeat == "infinite" else repeat),
                                  premultiplied=str(int(premultiplied)))
                    for key, value in wanted.items():
                        if actual.get(key) != value:
                            raise AssertionError(f"repeat={repeat} premultiplied={premultiplied} poster={poster}: {key} mismatch")
                    count += 1
    print(f"animation metadata: {count}/{count} independent track/poster checks passed")


if __name__ == "__main__":
    main()

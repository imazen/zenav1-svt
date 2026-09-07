#!/usr/bin/env python3
"""Independent libavif metadata gate. Build animation_probe and avif_metadata_probe.c first.

Run under scripts/run-heavy; arguments are the fresh Rust example, C probe, and
an ICC profile fixture. All artifacts go in a temporary directory.
"""
import os
from itertools import product
from pathlib import Path
import struct
import subprocess
import sys
import tempfile


def metadata_boxes(data, wanted=b"mdcv"):
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
            if kind == wanted:
                yield here, data[start + 8:start + size]
            if kind in containers:
                yield from walk(start + 8 + containers[kind], start + size, here)
            start += size
    return list(walk(0, len(data), ()))


def verify_orientation_boxes(data, rotation, mirror):
    for kind, value in [(b"irot", rotation), (b"imir", mirror)]:
        found = metadata_boxes(data, kind)
        if value is None:
            assert not found, f"unexpected {kind!r}"
        else:
            expected_paths = {
                (b"meta", b"iprp", b"ipco", kind),
                (b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01", kind),
            }
            assert len(found) == 2 and {p for p, _ in found} == expected_paths
            assert all(payload == bytes([value]) for _, payload in found)
    # Independently inspect poster associations: transformations must be
    # essential and ordered rotation -> mirror, only on the color item.
    ipco = metadata_boxes(data, b"ipco")
    ipma = metadata_boxes(data, b"ipma")
    assert len(ipco) == len(ipma) == 1
    properties, cursor = [], 0
    payload = ipco[0][1]
    while cursor < len(payload):
        size, kind = struct.unpack_from(">I4s", payload, cursor)
        assert size >= 8 and cursor + size <= len(payload)
        properties.append(kind)
        cursor += size
    payload = ipma[0][1]
    assert payload[:4] == b"\0\0\0\0"  # version 0, 7-bit property indexes
    entries = struct.unpack_from(">I", payload, 4)[0]
    cursor = 8
    for _ in range(entries):
        item_id, count = struct.unpack_from(">HB", payload, cursor)
        cursor += 3
        transforms = []
        for association in payload[cursor:cursor + count]:
            kind = properties[(association & 0x7f) - 1]
            if kind in (b"irot", b"imir"):
                assert association & 0x80 and item_id == 1
                transforms.append(kind)
        expected = ([b"irot"] if rotation is not None else []) + ([b"imir"] if mirror is not None else [])
        assert transforms == (expected if item_id == 1 else [])
        cursor += count
    assert cursor == len(payload)


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
        for pasp, repeat, (has_alpha, premultiplied), rotation, mirror in product(
            [None, "1,1", "2,2", "4294967295,4294967295"],
            ["0", "2", "infinite"],
            [(False, False), (True, False), (True, True)],
            [None, 0, 1, 2, 3], [None, 0, 1],
        ):
            env = os.environ.copy()
            for key in ["AVIF_REPEAT", "AVIF_PREMULTIPLIED", "AVIF_METADATA", "AVIF_ICC", "AVIF_NO_ALPHA", "AVIF_PASP", "AVIF_ROTATION", "AVIF_MIRROR"]:
                env.pop(key, None)
            env.update(AVIF_REPEAT=repeat, AVIF_METADATA="1", AVIF_ICC=profile)
            if rotation is not None:
                env["AVIF_ROTATION"] = str(rotation)
            if mirror is not None:
                env["AVIF_MIRROR"] = str(mirror)
            if pasp is not None:
                env["AVIF_PASP"] = pasp
            if not has_alpha:
                env["AVIF_NO_ALPHA"] = "1"
            if premultiplied:
                env["AVIF_PREMULTIPLIED"] = "1"
            output = str(Path(directory) / "animation.avif")
            subprocess.run([encoder, output], env=env, check=True, capture_output=True)
            data = Path(output).read_bytes()
            verify_orientation_boxes(data, rotation, mirror)
            mastering = metadata_boxes(data)
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
                              premultiplied=str(int(premultiplied)), pasp=pasp or "none",
                              rotation="none" if rotation is None else str(rotation),
                              mirror="none" if mirror is None else str(mirror))
                for key, value in wanted.items():
                    if actual.get(key) != value:
                        raise AssertionError(f"repeat={repeat} premultiplied={premultiplied} poster={poster}: {key} mismatch")
                count += 1
    print(f"animation metadata: {count}/{count} independent track/poster checks passed")


if __name__ == "__main__":
    main()

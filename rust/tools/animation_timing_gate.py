#!/usr/bin/env python3
"""Check exact container ticks and libavif sequential/seek pixels.

Arguments: freshly built animation_probe, avif_metadata_probe. Run with run-heavy.
"""
from itertools import groupby, product
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from animation_metadata_gate import metadata_boxes


MAX = 2**32 - 1


def run(args, env=None):
    result = subprocess.run(args, env=env, capture_output=True, text=True)
    if result.returncode:
        raise AssertionError(f"{args}: {result.stderr}")
    return result.stdout


def verify_boxes(data, timescale, durations, repeat, alpha):
    tracks = 2 if alpha else 1
    duration = sum(durations)
    presentation = 2**64 - 1 if repeat == "infinite" else duration * (int(repeat) + 1)
    for kind, number in [(b"mvhd", 1), (b"mdhd", tracks)]:
        boxes = metadata_boxes(data, kind)
        assert len(boxes) == number
        for _, payload in boxes:
            assert payload[:4] == b"\x01\0\0\0"
            scale, ticks = struct.unpack_from(">IQ", payload, 20)
            assert scale == timescale and ticks == (duration if kind == b"mdhd" else presentation)
    boxes = metadata_boxes(data, b"tkhd")
    assert len(boxes) == tracks
    for _, payload in boxes:
        assert payload[0] == 1 and struct.unpack_from(">Q", payload, 28)[0] == presentation
    boxes = metadata_boxes(data, b"elst")
    assert len(boxes) == tracks
    for _, payload in boxes:
        assert payload[:4] == bytes([1, 0, 0, int(repeat != "0")])
        assert struct.unpack_from(">IQQhh", payload, 4) == (1, duration, 0, 1, 0)
    expected_runs = [(len(list(values)), delta) for delta, values in groupby(durations)]
    for kind in (b"stts", b"stss"):
        boxes = metadata_boxes(data, kind)
        assert len(boxes) == tracks
        for _, payload in boxes:
            assert payload[:4] == b"\0\0\0\0"
            count = struct.unpack_from(">I", payload, 4)[0]
            if kind == b"stts":
                assert len(payload) == 8 + count*8
                assert list(struct.iter_unpack(">II", payload[8:])) == expected_runs
            else:
                assert len(payload) == 8 + count*4
                assert list(struct.unpack_from(f">{count}I", payload, 8)) == list(range(1, len(durations)+1))


def main():
    encoder, decoder = (str(Path(p).resolve()) for p in sys.argv[1:])
    cases = [
        (1000, [1], "0"),
        (1000, [100, 100, 100, 200, 200, 1], "2"),
        (1, [MAX, MAX, 1], "0"),
        (MAX, [MAX, MAX-1, MAX], "2"),
        (1000, [1, MAX, 1, MAX], "1"),
        (1000, [1, 2, 3], str(2**31-1)),
        (1000, [1, 2, 3], str(MAX)),
        (1, [MAX]*11, "infinite"),
        (1000, [1 + i % 5 for i in range(257)], "2"),
    ]
    count = 0
    with tempfile.TemporaryDirectory(prefix="avif-timing-") as directory:
        output = str(Path(directory) / "timing.avif")
        for (timescale, durations, repeat), (alpha, premultiplied) in product(
                cases, [(False, False), (True, False), (True, True)]):
            env = {k: v for k, v in os.environ.items() if not k.startswith("AVIF_")}
            env.update(AVIF_TIMESCALE=str(timescale), AVIF_DURATIONS=",".join(map(str, durations)), AVIF_REPEAT=repeat)
            if not alpha:
                env["AVIF_NO_ALPHA"] = "1"
            if premultiplied:
                env["AVIF_PREMULTIPLIED"] = "1"
            run([encoder, output], env)
            verify_boxes(Path(output).read_bytes(), timescale, durations, repeat, alpha)
            actual = dict(line.split("=", 1) for line in run([decoder, output, "timing"]).splitlines())
            assert actual["timescale"] == str(timescale) and actual["duration"] == str(sum(durations))
            assert actual["frames"] == str(len(durations)) and actual["seek"] == "exact"
            assert actual["alpha"] == str(int(alpha)) and actual["premultiplied"] == str(int(premultiplied))
            # libavif intentionally reports counts above INT_MAX as infinite.
            # The exact finite presentation duration is asserted in the boxes.
            observer_repeat = "-1" if repeat == "infinite" or int(repeat) > 2**31-1 else repeat
            assert actual["repeat"] == observer_repeat
            pts = 0
            for i, duration in enumerate(durations):
                assert actual[f"frame{i}"] == f"{pts},{duration}"
                pts += duration
            count += 1
    print(f"animation timing: {count}/{count} exact timing and pixel-seek cases passed")


if __name__ == "__main__":
    main()

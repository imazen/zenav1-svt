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


def metadata_boxes(data, wanted=b"mdcv", offsets=False):
    # Walk box boundaries, including full-box and visual-sample-entry headers.
    # Never search compressed payloads for four-character strings.
    containers = {b"edts": 0, b"moov": 0, b"trak": 0, b"mdia": 0, b"minf": 0,
                  b"stbl": 0, b"stsd": 8, b"av01": 78,
                  b"meta": 4, b"iprp": 0, b"ipco": 0, b"iinf": 6, b"iref": 4}
    def walk(start, end, path):
        while start < end:
            if end - start < 8:
                raise AssertionError("truncated box header")
            size, kind = struct.unpack_from(">I4s", data, start)
            if size < 8 or start + size > end:
                raise AssertionError("invalid box extent")
            here = path + (kind,)
            if kind == wanted:
                yield (here, start + 8) if offsets else (here, data[start + 8:start + size])
            if kind in containers:
                yield from walk(start + 8 + containers[kind], start + size, here)
            start += size
    return list(walk(0, len(data), ()))


def poster_properties(data):
    ipco = metadata_boxes(data, b"ipco")
    ipma = metadata_boxes(data, b"ipma")
    assert len(ipco) == len(ipma) == 1
    properties, cursor = [], 0
    payload = ipco[0][1]
    while cursor < len(payload):
        size, kind = struct.unpack_from(">I4s", payload, cursor)
        assert size >= 8 and cursor + size <= len(payload)
        properties.append((kind, payload[cursor + 8:cursor + size]))
        cursor += size
    payload = ipma[0][1]
    assert payload[:4] == b"\0\0\0\0"
    entries = struct.unpack_from(">I", payload, 4)[0]
    cursor, result = 8, {}
    for _ in range(entries):
        item_id, count = struct.unpack_from(">HB", payload, cursor)
        cursor += 3
        assert item_id not in result and cursor + count <= len(payload)
        result[item_id] = []
        for association in payload[cursor:cursor + count]:
            index = association & 0x7f
            assert 0 < index <= len(properties)
            kind, value = properties[index - 1]
            result[item_id].append((kind, value, bool(association & 0x80)))
        cursor += count
    assert cursor == len(payload)
    return result


def verify_spatial_boxes(data, crop, rotation, mirror, has_alpha, premultiplied):
    clap = None
    if crop is not None:
        x, y, w, h = crop
        clap = struct.pack(">4IiIiI", w, 1, h, 1, 2*x+w-64, 2, 2*y+h-80, 2)
    values = [(b"clap", clap), (b"irot", None if rotation is None else bytes([rotation])),
              (b"imir", None if mirror is None else bytes([mirror]))]
    for kind, value in values:
        found = metadata_boxes(data, kind)
        if value is None:
            assert not found, f"unexpected {kind!r}"
        else:
            expected_paths = {
                (b"meta", b"iprp", b"ipco", kind),
                (b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01", kind),
            }
            assert len(found) == 2 and {p for p, _ in found} == expected_paths
            assert all(payload == value for _, payload in found)
    # Check ordering in both visual sample entries, including absence of a
    # second transform on the alpha track.
    entries = metadata_boxes(data, b"av01")
    assert len(entries) == 1 + int(has_alpha)
    for _, entry in entries:
        cursor, kinds = 78, []
        while cursor < len(entry):
            size, kind = struct.unpack_from(">I4s", entry, cursor)
            assert size >= 8 and cursor + size <= len(entry)
            kinds.append(kind)
            cursor += size
        expected = [] if b"auxi" in kinds else [kind for kind, value in values if value is not None]
        assert [kind for kind in kinds if kind in (b"clap", b"irot", b"imir")] == expected
    # All transforms are essential and ordered crop -> rotation -> mirror;
    # neither the alpha item nor the full secondary image repeats transforms.
    properties = poster_properties(data)
    assert set(properties) == ({1} | ({2} if has_alpha else set()) | ({5} if crop else set()) | ({6} if crop and has_alpha else set()))
    for item_id, props in properties.items():
        transforms = [(kind, value) for kind, value, essential in props if kind in (b"clap", b"irot", b"imir")]
        assert all(essential for kind, _, essential in props if kind in (b"clap", b"irot", b"imir"))
        assert transforms == ([(kind, value) for kind, value in values if value is not None] if item_id == 1 else [])
    if crop is None:
        return
    # The secondary item is visible and shares precisely the first color
    # sample's extent and descriptive properties. No duplicate encoding.
    assert properties[5] == [p for p in properties[1] if p[0] not in (b"clap", b"irot", b"imir")]
    infe = [v for path, v in metadata_boxes(data, b"infe") if path == (b"meta", b"iinf", b"infe")]
    secondary = [v for v in infe if struct.unpack_from(">H", v, 4)[0] == 5]
    assert len(secondary) == 1 and secondary[0][:4] == b"\x02\0\0\0" and secondary[0][8:12] == b"av01"
    iloc = [v for path, v in metadata_boxes(data, b"iloc") if path == (b"meta", b"iloc")][0]
    assert iloc[:6] == b"\x01\0\0\0\x44\0"
    n = struct.unpack_from(">H", iloc, 6)[0]
    extents = {}
    for i in range(n):
        item, method, reference, count, offset, size = struct.unpack_from(">4H2I", iloc, 8 + i*16)
        assert reference == 0 and count == 1 and item not in extents
        extents[item] = (method, offset, size)
    assert len(iloc) == 8 + n*16 and extents[1] == extents[5] and extents[5][0] == 0
    if has_alpha:
        assert extents[2] == extents[6] and properties[2] == properties[6]
        alpha_infe = [v for v in infe if struct.unpack_from(">H", v, 4)[0] in (2, 6)]
        assert len(alpha_infe) == 2 and all(v[:4] == b"\x02\0\0\x01" for v in alpha_infe)
    for kind, expected in [(b"auxl", {2: [1], 6: [5]} if has_alpha else {}),
                           (b"prem", {1: [2], 5: [6]} if premultiplied else {}),
                           (b"cdsc", {3: [1], 4: [1], 7: [5], 8: [5]})]:
        references = {}
        for path, value in metadata_boxes(data, kind):
            assert path == (b"meta", b"iref", kind)
            source, count = struct.unpack_from(">HH", value)
            assert len(value) == 4 + 2*count and source not in references
            references[source] = list(struct.unpack_from(f">{count}H", value, 4))
        assert references == expected, (kind, references, expected)


def select_uncropped_poster(data):
    # Only change which real item is selected as primary, so libavif can decode
    # the independently serialized secondary through its primary-item API.
    found = metadata_boxes(data, b"pitm", offsets=True)
    assert len(found) == 1 and found[0][0] == (b"meta", b"pitm")
    start = found[0][1]
    assert data[start:start+6] == b"\0\0\0\0\0\x01"
    result = bytearray(data)
    struct.pack_into(">H", result, start + 4, 5)
    return result


def main():
    encoder, decoder, profile = (str(Path(p).resolve()) for p in sys.argv[1:])
    expected = {
        "icc": Path(profile).read_bytes().hex(),
        "exif": b"II*\0\x08\0\0\0\0\0\0\0\0\0".hex(),
        "xmp": b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><probe>animation</probe></x:xmpmeta>'.hex(),
        "cicp": "1,13,1,0", "clli": "1000,400", "dimensions": "64,80",
    }
    count = 0
    with tempfile.TemporaryDirectory(prefix="avif-metadata-") as directory:
        for pasp, repeat, (has_alpha, premultiplied), rotation, mirror, crop in product(
            [None, "1,1", "2,2", "4294967295,4294967295"],
            ["0", "2", "infinite"],
            [(False, False), (True, False), (True, True)],
            [None, 0, 1, 2, 3], [None, 0, 1],
            [None, (0, 0, 48, 56), (8, 12, 48, 56), (1, 3, 61, 75), (63, 79, 1, 1)],
        ):
            env = os.environ.copy()
            for key in ["AVIF_REPEAT", "AVIF_PREMULTIPLIED", "AVIF_METADATA", "AVIF_ICC", "AVIF_NO_ALPHA", "AVIF_PASP", "AVIF_ROTATION", "AVIF_MIRROR", "AVIF_CROP", "AVIF_DURATIONS", "AVIF_TIMESCALE"]:
                env.pop(key, None)
            env.update(AVIF_REPEAT=repeat, AVIF_METADATA="1", AVIF_ICC=profile)
            if crop is not None:
                env["AVIF_CROP"] = ",".join(map(str, crop))
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
            verify_spatial_boxes(data, crop, rotation, mirror, has_alpha, premultiplied)
            mastering = metadata_boxes(data)
            expected_mdcv = struct.pack(">8H2I", 13250, 34500, 7500, 3000,
                                        34000, 16000, 15635, 16450, 10000000, 50)
            expected_paths = {
                (b"meta", b"iprp", b"ipco", b"mdcv"),
                (b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01", b"mdcv"),
            }
            if len(mastering) != (3 if crop else 2) or {p for p, _ in mastering} != expected_paths:
                raise AssertionError("MDCV must appear on both color poster and track")
            if any(payload != expected_mdcv for _, payload in mastering):
                raise AssertionError("MDCV payload mismatch")
            secondary_output = str(Path(directory) / "uncropped.avif")
            if crop:
                Path(secondary_output).write_bytes(select_uncropped_poster(data))
            for source in (["track", "poster", "secondary"] if crop else ["track", "poster"]):
                poster = source != "track"
                args = [decoder, secondary_output if source == "secondary" else output] + (["poster"] if poster else [])
                result = subprocess.run(args, check=True, capture_output=True, text=True)
                actual = dict(line.split("=", 1) for line in result.stdout.splitlines())
                wanted = dict(expected, alpha=str(int(has_alpha)), frames="1" if poster else "3",
                              repeat="0" if poster else ("-1" if repeat == "infinite" else repeat),
                              premultiplied=str(int(premultiplied)), pasp=pasp or "none",
                              rotation="none" if rotation is None else str(rotation),
                              mirror="none" if mirror is None else str(mirror),
                              crop=",".join(map(str, crop)) if crop else "none")
                if source == "secondary":
                    wanted.update(crop="none", rotation="none", mirror="none")
                for key, value in wanted.items():
                    if actual.get(key) != value:
                        raise AssertionError(f"crop={crop} rotation={rotation} mirror={mirror} repeat={repeat} premultiplied={premultiplied} source={source}: {key}={actual.get(key)!r}, expected {value!r}")
                count += 1
    print(f"animation metadata: {count}/{count} independent track/poster checks passed")


if __name__ == "__main__":
    main()

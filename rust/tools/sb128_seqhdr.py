#!/usr/bin/env python3
"""Print `use_128x128_superblock` (AV1 spec 5.5.1) from a raw OBU stream.

The sb128 gate's anti-vacuity check: an "sb128 cell" is only meaningful if
the C oracle ACTUALLY emitted 128px superblocks for it. There is no
`super_block_size` field in EbSvtAv1EncConfiguration — C derives the value
(Globals/enc_handle.c:4071-4111) from frame area, preset, and the force-64
clauses — so the only way to know what the oracle did is to read the bit
back out of the bitstream it produced.

Usage:  sb128_seqhdr.py <stream.obu> [...]
Prints one line per file: `<path> use_128x128_superblock=<0|1> <WxH>`.
Exit 0 on success, 2 if any file has no sequence header.

Input is a RAW OBU stream (what tools/capture_c_trace and identity_run
write), not IVF.
"""
import sys


class BitReader:
    def __init__(self, buf):
        self.buf = buf
        self.pos = 0

    def f(self, n):
        v = 0
        for _ in range(n):
            byte = self.buf[self.pos >> 3]
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1)
            self.pos += 1
        return v


def leb128(buf, i):
    v = 0
    n = 0
    while True:
        b = buf[i + n]
        v |= (b & 0x7F) << (n * 7)
        n += 1
        if not (b & 0x80):
            return v, n


def iter_obus(data):
    """Yield (obu_type, payload) for each OBU in a raw stream."""
    i = 0
    while i < len(data):
        h = data[i]
        obu_type = (h >> 3) & 0xF
        has_ext = (h >> 2) & 1
        has_size = (h >> 1) & 1
        j = i + 1 + (1 if has_ext else 0)
        if has_size:
            size, n = leb128(data, j)
            j += n
        else:
            size = len(data) - j
        yield obu_type, data[j:j + size]
        i = j + size


def parse_sequence_header(payload):
    """AV1 spec 5.5.1 sequence_header_obu(), up to use_128x128_superblock."""
    r = BitReader(payload)
    r.f(3)                      # seq_profile
    r.f(1)                      # still_picture
    reduced = r.f(1)            # reduced_still_picture_header
    if reduced:
        r.f(5)                  # seq_level_idx[0]
    else:
        decoder_model_info_present = 0
        buffer_delay_length = 0
        if r.f(1):              # timing_info_present_flag
            r.f(32)             # num_units_in_display_tick
            r.f(32)             # time_scale
            if r.f(1):          # equal_picture_interval
                lz = 0          # uvlc num_ticks_per_picture_minus_1
                while lz < 32 and r.f(1) == 0:
                    lz += 1
                if lz < 32:
                    r.f(lz)
            decoder_model_info_present = r.f(1)
            if decoder_model_info_present:
                buffer_delay_length = r.f(5) + 1
                r.f(32)         # num_units_in_decoding_tick
                r.f(5)          # buffer_removal_time_length_minus_1
                r.f(5)          # frame_presentation_time_length_minus_1
        initial_display_delay_present = r.f(1)
        for _ in range(r.f(5) + 1):          # operating_points_cnt_minus_1
            r.f(12)                          # operating_point_idc
            if r.f(5) > 7:                   # seq_level_idx
                r.f(1)                       # seq_tier
            if decoder_model_info_present and r.f(1):
                r.f(buffer_delay_length)
                r.f(buffer_delay_length)
                r.f(1)
            if initial_display_delay_present and r.f(1):
                r.f(4)
    # Spec order: BOTH bit-widths, THEN both dimensions. Do not fold these
    # into one expression each — that interleaves them and desyncs the
    # reader (every field after it, including the bit this tool exists to
    # report, then comes out of a wrong bit position).
    w_bits = r.f(4) + 1         # frame_width_bits_minus_1
    h_bits = r.f(4) + 1         # frame_height_bits_minus_1
    w = r.f(w_bits) + 1         # max_frame_width_minus_1
    h = r.f(h_bits) + 1         # max_frame_height_minus_1
    if not reduced and r.f(1):  # frame_id_numbers_present_flag
        r.f(4)                  # delta_frame_id_length_minus_2
        r.f(3)                  # additional_frame_id_length_minus_1
    return r.f(1), w, h         # use_128x128_superblock


def main(argv):
    rc = 0
    for path in argv:
        data = open(path, "rb").read()
        found = False
        for obu_type, payload in iter_obus(data):
            if obu_type == 1:                # OBU_SEQUENCE_HEADER
                sb128, w, h = parse_sequence_header(payload)
                print(f"{path} use_128x128_superblock={sb128} {w}x{h}")
                found = True
                break
        if not found:
            print(f"{path} NO-SEQUENCE-HEADER")
            rc = 2
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

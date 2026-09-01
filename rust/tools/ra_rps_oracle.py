#!/usr/bin/env python3
"""Read the REFERENCE STRUCTURE out of a real SVT-AV1 bitstream.

Answers one question with evidence rather than inference: for each coded frame
the C encoder emitted, which DPB slots did it point its seven references at,
which slots did it refresh, and which slot did it re-display? Those are
`ref_frame_idx[]`, `refresh_frame_flags` and `frame_to_show_map_idx` — the
written form of everything `av1_generate_rps_info` computes.

That makes this the oracle for `port_picstruct_ra`: the port's tables are
tier-4 hand transcriptions of `pd_process.c`, and comparing them against the
encoder's own output is a tier-2 check (`docs/WORKING-ON-THIS.md` §4) that no
re-reading of the C can give.

  ra_rps_oracle.py <stream.obu>            # one JSON object per line
  ra_rps_oracle.py <stream.obu> --json     # a single JSON document

Parses the uncompressed frame header per AV1 spec 5.9.2 only as far as
`ref_frame_idx[]`, which is all this question needs, and stops. It therefore
does NOT validate the rest of the header — a parse that runs off the end will
raise rather than return a plausible-looking wrong answer.
"""

import json
import sys

KEY_FRAME, INTER_FRAME, INTRA_ONLY_FRAME, SWITCH_FRAME = 0, 1, 2, 3
OBU_SEQUENCE_HEADER, OBU_TD, OBU_FRAME_HEADER = 1, 2, 3
OBU_TILE_GROUP, OBU_METADATA, OBU_FRAME = 4, 5, 6
OBU_REDUNDANT_FRAME_HEADER, OBU_PADDING = 7, 15
SELECT_SCREEN_CONTENT_TOOLS = 2
SELECT_INTEGER_MV = 2
NUM_REF_FRAMES = 8
REFS_PER_FRAME = 7


class Bits:
    """MSB-first bit cursor. Reading past the end raises, never wraps."""

    def __init__(self, data):
        self.data = data
        self.pos = 0

    def f(self, n):
        v = 0
        for _ in range(n):
            if self.pos >> 3 >= len(self.data):
                raise EOFError("frame header parse ran past the end of the OBU")
            byte = self.data[self.pos >> 3]
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1)
            self.pos += 1
        return v

    def uvlc(self):
        leading = 0
        while True:
            if self.f(1):
                break
            leading += 1
            if leading >= 32:
                return (1 << 32) - 1
        if leading == 0:
            return 0
        return self.f(leading) + (1 << leading) - 1


def leb128(data, i):
    value, shift = 0, 0
    while True:
        b = data[i]
        i += 1
        value |= (b & 0x7F) << shift
        shift += 7
        if not (b & 0x80):
            return value, i


def parse_sequence_header(payload):
    """AV1 spec 5.5.1, far enough to decode any frame header that follows."""
    b = Bits(payload)
    sh = {}
    sh["seq_profile"] = b.f(3)
    b.f(1)  # still_picture
    reduced = b.f(1)
    sh["reduced_still_picture_header"] = reduced

    if reduced:
        b.f(5)  # seq_level_idx[0]
        sh["decoder_model_info_present_flag"] = 0
        sh["equal_picture_interval"] = 1
    else:
        timing_info_present = b.f(1)
        equal_picture_interval = 1
        decoder_model_info_present = 0
        if timing_info_present:
            b.f(32)  # num_units_in_display_tick
            b.f(32)  # time_scale
            equal_picture_interval = b.f(1)
            if equal_picture_interval:
                b.uvlc()  # num_ticks_per_picture_minus_1
            decoder_model_info_present = b.f(1)
            if decoder_model_info_present:
                sh["buffer_delay_length_minus_1"] = b.f(5)
                b.f(32)  # num_units_in_decoding_tick
                sh["buffer_removal_time_length_minus_1"] = b.f(5)
                sh["frame_presentation_time_length_minus_1"] = b.f(5)
        sh["decoder_model_info_present_flag"] = decoder_model_info_present
        sh["equal_picture_interval"] = equal_picture_interval

        initial_display_delay_present = b.f(1)
        ops = b.f(5) + 1
        sh["operating_points_cnt"] = ops
        for _ in range(ops):
            b.f(12)  # operating_point_idc
            level = b.f(5)
            if level > 7:
                b.f(1)  # seq_tier
            if decoder_model_info_present:
                if b.f(1):  # decoder_model_present_for_this_op
                    n = sh["buffer_delay_length_minus_1"] + 1
                    b.f(n)  # decoder_buffer_delay
                    b.f(n)  # encoder_buffer_delay
                    b.f(1)  # low_delay_mode_flag
            if initial_display_delay_present:
                if b.f(1):
                    b.f(4)

    frame_width_bits = b.f(4) + 1
    frame_height_bits = b.f(4) + 1
    b.f(frame_width_bits)  # max_frame_width_minus_1
    b.f(frame_height_bits)  # max_frame_height_minus_1

    if reduced:
        sh["frame_id_numbers_present_flag"] = 0
    else:
        sh["frame_id_numbers_present_flag"] = b.f(1)
    if sh["frame_id_numbers_present_flag"]:
        sh["delta_frame_id_length_minus_2"] = b.f(4)
        sh["additional_frame_id_length_minus_1"] = b.f(3)

    b.f(1)  # use_128x128_superblock
    b.f(1)  # enable_filter_intra
    b.f(1)  # enable_intra_edge_filter

    if reduced:
        sh["enable_order_hint"] = 0
        sh["order_hint_bits"] = 0
        sh["seq_force_screen_content_tools"] = SELECT_SCREEN_CONTENT_TOOLS
        sh["seq_force_integer_mv"] = SELECT_INTEGER_MV
    else:
        b.f(1)  # enable_interintra_compound
        b.f(1)  # enable_masked_compound
        b.f(1)  # enable_warped_motion
        b.f(1)  # enable_dual_filter
        enable_order_hint = b.f(1)
        sh["enable_order_hint"] = enable_order_hint
        if enable_order_hint:
            b.f(1)  # enable_jnt_comp
            b.f(1)  # enable_ref_frame_mvs
        if b.f(1):  # seq_choose_screen_content_tools
            sctools = SELECT_SCREEN_CONTENT_TOOLS
        else:
            sctools = b.f(1)
        sh["seq_force_screen_content_tools"] = sctools
        if sctools > 0:
            if b.f(1):  # seq_choose_integer_mv
                sh["seq_force_integer_mv"] = SELECT_INTEGER_MV
            else:
                sh["seq_force_integer_mv"] = b.f(1)
        else:
            sh["seq_force_integer_mv"] = SELECT_INTEGER_MV
        sh["order_hint_bits"] = (b.f(3) + 1) if enable_order_hint else 0

    sh["enable_superres"] = b.f(1)
    b.f(1)  # enable_cdef
    b.f(1)  # enable_restoration
    return sh


def parse_frame_header(payload, sh, ref_frame_type):
    """AV1 spec 5.9.2, up to and including `ref_frame_idx[]`."""
    b = Bits(payload)
    out = {}

    if sh["reduced_still_picture_header"]:
        raise ValueError("this oracle is for multi-frame streams, not still pictures")

    show_existing = b.f(1)
    if show_existing:
        idx = b.f(3)
        out.update(
            kind="show_existing",
            frame_to_show_map_idx=idx,
            refresh_frame_flags=0xFF if ref_frame_type[idx] == KEY_FRAME else 0,
        )
        return out

    frame_type = b.f(2)
    frame_is_intra = frame_type in (KEY_FRAME, INTRA_ONLY_FRAME)
    show_frame = b.f(1)
    if show_frame and sh["decoder_model_info_present_flag"] and not sh["equal_picture_interval"]:
        b.f(sh["frame_presentation_time_length_minus_1"] + 1)
    if not show_frame:
        b.f(1)  # showable_frame
    if frame_type == SWITCH_FRAME or (frame_type == KEY_FRAME and show_frame):
        error_resilient = 1
    else:
        error_resilient = b.f(1)

    b.f(1)  # disable_cdf_update
    if sh["seq_force_screen_content_tools"] == SELECT_SCREEN_CONTENT_TOOLS:
        allow_sc = b.f(1)
    else:
        allow_sc = sh["seq_force_screen_content_tools"]
    if allow_sc and sh["seq_force_integer_mv"] == SELECT_INTEGER_MV:
        b.f(1)  # force_integer_mv

    if sh["frame_id_numbers_present_flag"]:
        id_len = (
            sh["additional_frame_id_length_minus_1"]
            + 1
            + sh["delta_frame_id_length_minus_2"]
            + 2
        )
        b.f(id_len)  # current_frame_id

    if frame_type == SWITCH_FRAME:
        frame_size_override = 1
    else:
        frame_size_override = b.f(1)

    order_hint = b.f(sh["order_hint_bits"])
    if frame_is_intra or error_resilient:
        primary_ref_frame = 7
    else:
        primary_ref_frame = b.f(3)

    if sh["decoder_model_info_present_flag"]:
        if b.f(1):  # buffer_removal_time_present_flag
            raise ValueError("buffer_removal_time is not parsed by this oracle")

    if frame_type == SWITCH_FRAME or (frame_type == KEY_FRAME and show_frame):
        refresh = 0xFF
    else:
        refresh = b.f(8)

    if (not frame_is_intra or refresh != 0xFF) and error_resilient and sh["enable_order_hint"]:
        for _ in range(NUM_REF_FRAMES):
            b.f(sh["order_hint_bits"])  # ref_order_hint[i]

    out.update(
        kind="coded",
        frame_type=frame_type,
        show_frame=show_frame,
        error_resilient=error_resilient,
        order_hint=order_hint,
        primary_ref_frame=primary_ref_frame,
        refresh_frame_flags=refresh,
    )

    if frame_is_intra:
        out["ref_frame_idx"] = None
        return out

    if not sh["enable_order_hint"]:
        short_signaling = 0
    else:
        short_signaling = b.f(1)
        if short_signaling:
            b.f(3)  # last_frame_idx
            b.f(3)  # gold_frame_idx
    out["frame_refs_short_signaling"] = short_signaling

    refs = []
    for _ in range(REFS_PER_FRAME):
        refs.append(None if short_signaling else b.f(3))
        if sh["frame_id_numbers_present_flag"]:
            b.f(sh["delta_frame_id_length_minus_2"] + 2)
    out["ref_frame_idx"] = refs
    return out


def parse_stream(data):
    """Walk the OBUs, returning one record per frame header, in decode order."""
    i = 0
    sh = None
    ref_frame_type = [KEY_FRAME] * NUM_REF_FRAMES
    frames = []
    while i < len(data):
        first = data[i]
        if first & 0x80:
            raise ValueError(f"obu_forbidden_bit set at byte {i}")
        obu_type = (first >> 3) & 0xF
        extension = (first >> 2) & 1
        has_size = (first >> 1) & 1
        i += 1
        temporal_id = spatial_id = 0
        if extension:
            temporal_id = (data[i] >> 5) & 0x7
            spatial_id = (data[i] >> 3) & 0x3
            i += 1
        if not has_size:
            raise ValueError("obu_has_size_field must be set in these streams")
        size, i = leb128(data, i)
        payload = data[i : i + size]
        i += size

        if obu_type == OBU_SEQUENCE_HEADER:
            sh = parse_sequence_header(payload)
        elif obu_type in (OBU_FRAME_HEADER, OBU_FRAME):
            if sh is None:
                raise ValueError("frame header before any sequence header")
            rec = parse_frame_header(payload, sh, ref_frame_type)
            rec["temporal_id"] = temporal_id
            rec["spatial_id"] = spatial_id
            frames.append(rec)
            ft = rec.get("frame_type", KEY_FRAME)
            for s in range(NUM_REF_FRAMES):
                if (rec["refresh_frame_flags"] >> s) & 1:
                    ref_frame_type[s] = ft
    return sh, frames


def main(argv):
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    data = open(argv[1], "rb").read()
    sh, frames = parse_stream(data)
    if "--json" in argv:
        json.dump({"sequence_header": sh, "frames": frames}, sys.stdout, indent=1)
        print()
    else:
        for rec in frames:
            print(json.dumps(rec, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

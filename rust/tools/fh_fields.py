#!/usr/bin/env python3
"""Field-level FRAME-header differ for two raw OBU streams.

The sequence-header sibling is `sh_fields.py`; this one walks
`uncompressed_header()` (AV1 spec 5.9.2) of the first frame OBU in each stream
and prints the fields side by side, so a divergence names the FIELD instead of
a byte offset.

    tools/fh_fields.py <c.obu> <rust.obu>            # first frame OBU of each
    tools/fh_fields.py --index 1 <c.obu> <rust.obu>  # the 2nd frame OBU
    tools/fh_fields.py --seq <stream-with-sh> <a.obu> <b.obu>

Why it exists (inter campaign, docs/INTER-ENCODE-PLAN.md): on a macOS host the
op-trace differ degrades to "the bytes differ", and in a frame header one wrong
field shifts every following one, so the byte offset names nothing. The video
-mode key frame writes ~20 fields the still (reduced) header never writes.

CAVEAT — the same one `sh_fields.py` and `identity_diff.py` carry: this walks
the header with its own model of the layout, parameterised by each stream's OWN
sequence header. If an EARLIER field's value changes a later field's presence,
the names after that point can be wrong. **The first DIFFERS line is the fact;
everything after it is a hint.** Stop reading at the first one.

No dependencies, no C library, no decoder.
"""
import sys

# ---------------------------------------------------------------- OBU layer

LAST_FRAME = 1
ALTREF_FRAME = 7
REFS_PER_FRAME_ = 7


def get_relative_dist(s, a, b):
    """Spec 5.9.3 get_relative_dist()."""
    if not s['enable_order_hint']:
        return 0
    diff = a - b
    m = 1 << (s['OrderHintBits'] - 1)
    return (diff & (m - 1)) - (diff & m)


OBU_SEQUENCE_HEADER = 1
OBU_TEMPORAL_DELIMITER = 2
OBU_FRAME_HEADER = 3
OBU_FRAME = 6
OBU_REDUNDANT_FRAME_HEADER = 7


def leb128(b, i):
    v = 0
    s = 0
    while True:
        x = b[i]
        i += 1
        v |= (x & 0x7F) << s
        s += 7
        if not (x & 0x80):
            return v, i


def obus(b):
    """[(type, temporal_id, spatial_id, payload)] over a raw OBU stream."""
    i = 0
    out = []
    while i < len(b):
        h = b[i]
        t = (h >> 3) & 0xF
        ext = (h >> 2) & 1
        has_size = (h >> 1) & 1
        j = i + 1
        tid = sid = 0
        if ext:
            tid = (b[j] >> 5) & 0x7
            sid = (b[j] >> 3) & 0x3
            j += 1
        if has_size:
            sz, j = leb128(b, j)
        else:
            sz = len(b) - j
        out.append((t, tid, sid, b[j:j + sz]))
        i = j + sz
    return out


class BR:
    """MSB-first bit reader with the spec's f(n) / su(n) / uvlc() / le(n)."""

    def __init__(self, b):
        self.b = b
        self.p = 0

    def f(self, n):
        v = 0
        for _ in range(n):
            if (self.p >> 3) >= len(self.b):
                raise EOFError("ran off the end of the OBU payload")
            v = (v << 1) | ((self.b[self.p >> 3] >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v

    def su(self, n):
        v = self.f(n)
        sign = 1 << (n - 1)
        return v - 2 * sign if v & sign else v

    def uvlc(self):
        lz = 0
        while True:
            if self.f(1):
                break
            lz += 1
        if lz >= 32:
            return (1 << 32) - 1
        return self.f(lz) + (1 << lz) - 1

    def byte_align(self):
        while self.p & 7:
            self.p += 1


# ------------------------------------------------------- sequence header

SELECT_SCREEN_CONTENT_TOOLS = 2
SELECT_INTEGER_MV = 2
NUM_REF_FRAMES = 8
REFS_PER_FRAME = 7
PRIMARY_REF_NONE = 7
KEY_FRAME, INTER_FRAME, INTRA_ONLY_FRAME, SWITCH_FRAME = 0, 1, 2, 3
SUPERRES_DENOM_BITS = 3
MAX_TILE_WIDTH = 4096
MAX_TILE_AREA = 4096 * 2304
MAX_TILE_COLS = 64
MAX_TILE_ROWS = 64
MAX_SEGMENTS = 8
SEG_LVL_MAX = 8
SEG_FEATURE_BITS = [8, 6, 6, 6, 6, 3, 0, 0]
SEG_FEATURE_SIGNED = [1, 1, 1, 1, 1, 0, 0, 0]
TOTAL_REFS_PER_FRAME = 8
RESTORE_NONE = 0


def parse_seq_header(payload):
    """Return the SH state `uncompressed_header()` needs. Fields are also
    returned for reference but sh_fields.py is the differ for those."""
    r = BR(payload)
    s = {}
    s['seq_profile'] = r.f(3)
    s['still_picture'] = r.f(1)
    s['reduced_still_picture_header'] = r.f(1)
    if s['reduced_still_picture_header']:
        s['timing_info_present_flag'] = 0
        s['decoder_model_info_present_flag'] = 0
        s['initial_display_delay_present_flag'] = 0
        s['operating_points_cnt_minus_1'] = 0
        s['operating_point_idc'] = [0]
        r.f(5)  # seq_level_idx[0]
    else:
        s['timing_info_present_flag'] = r.f(1)
        if s['timing_info_present_flag']:
            r.f(32)  # num_units_in_display_tick
            r.f(32)  # time_scale
            equal_picture_interval = r.f(1)
            if equal_picture_interval:
                r.uvlc()  # num_ticks_per_picture_minus_1
            s['equal_picture_interval'] = equal_picture_interval
            s['decoder_model_info_present_flag'] = r.f(1)
            if s['decoder_model_info_present_flag']:
                s['buffer_delay_length_minus_1'] = r.f(5)
                r.f(32)  # num_units_in_decoding_tick
                s['buffer_removal_time_length_minus_1'] = r.f(5)
                s['frame_presentation_time_length_minus_1'] = r.f(5)
        else:
            s['equal_picture_interval'] = 0
            s['decoder_model_info_present_flag'] = 0
        s['initial_display_delay_present_flag'] = r.f(1)
        cnt = r.f(5)
        s['operating_points_cnt_minus_1'] = cnt
        s['operating_point_idc'] = []
        s['decoder_model_present_for_this_op'] = []
        for _i in range(cnt + 1):
            s['operating_point_idc'].append(r.f(12))
            lvl = r.f(5)
            if lvl > 7:
                r.f(1)  # seq_tier
            if s['decoder_model_info_present_flag']:
                present = r.f(1)
                s['decoder_model_present_for_this_op'].append(present)
                if present:
                    n = s['buffer_delay_length_minus_1'] + 1
                    r.f(n)  # decoder_buffer_delay
                    r.f(n)  # encoder_buffer_delay
                    r.f(1)  # low_delay_mode_flag
            else:
                s['decoder_model_present_for_this_op'].append(0)
            if s['initial_display_delay_present_flag']:
                if r.f(1):
                    r.f(4)
    s['frame_width_bits_minus_1'] = r.f(4)
    s['frame_height_bits_minus_1'] = r.f(4)
    s['max_frame_width_minus_1'] = r.f(s['frame_width_bits_minus_1'] + 1)
    s['max_frame_height_minus_1'] = r.f(s['frame_height_bits_minus_1'] + 1)
    if s['reduced_still_picture_header']:
        s['frame_id_numbers_present_flag'] = 0
    else:
        s['frame_id_numbers_present_flag'] = r.f(1)
    if s['frame_id_numbers_present_flag']:
        s['delta_frame_id_length_minus_2'] = r.f(4)
        s['additional_frame_id_length_minus_1'] = r.f(3)
    s['use_128x128_superblock'] = r.f(1)
    s['enable_filter_intra'] = r.f(1)
    s['enable_intra_edge_filter'] = r.f(1)
    if s['reduced_still_picture_header']:
        s['enable_warped_motion'] = 0
        s['enable_order_hint'] = 0
        s['enable_ref_frame_mvs'] = 0
        s['seq_force_screen_content_tools'] = SELECT_SCREEN_CONTENT_TOOLS
        s['seq_force_integer_mv'] = SELECT_INTEGER_MV
        s['OrderHintBits'] = 0
    else:
        r.f(1)  # enable_interintra_compound
        r.f(1)  # enable_masked_compound
        s['enable_warped_motion'] = r.f(1)
        r.f(1)  # enable_dual_filter
        s['enable_order_hint'] = r.f(1)
        if s['enable_order_hint']:
            r.f(1)  # enable_jnt_comp
            s['enable_ref_frame_mvs'] = r.f(1)
        else:
            s['enable_ref_frame_mvs'] = 0
        if r.f(1):  # seq_choose_screen_content_tools
            s['seq_force_screen_content_tools'] = SELECT_SCREEN_CONTENT_TOOLS
        else:
            s['seq_force_screen_content_tools'] = r.f(1)
        if s['seq_force_screen_content_tools'] > 0:
            if r.f(1):  # seq_choose_integer_mv
                s['seq_force_integer_mv'] = SELECT_INTEGER_MV
            else:
                s['seq_force_integer_mv'] = r.f(1)
        else:
            s['seq_force_integer_mv'] = SELECT_INTEGER_MV
        s['OrderHintBits'] = (r.f(3) + 1) if s['enable_order_hint'] else 0
    s['enable_superres'] = r.f(1)
    s['enable_cdef'] = r.f(1)
    s['enable_restoration'] = r.f(1)
    # color_config()
    high_bitdepth = r.f(1)
    if s['seq_profile'] == 2 and high_bitdepth:
        s['BitDepth'] = 12 if r.f(1) else 10
    else:
        s['BitDepth'] = 10 if high_bitdepth else 8
    s['mono_chrome'] = 0 if s['seq_profile'] == 1 else r.f(1)
    s['NumPlanes'] = 1 if s['mono_chrome'] else 3
    if r.f(1):  # color_description_present_flag
        cp, tc, mc = r.f(8), r.f(8), r.f(8)
    else:
        cp, tc, mc = 2, 2, 2
    if s['mono_chrome']:
        r.f(1)  # color_range
        s['subsampling_x'] = s['subsampling_y'] = 1
        s['separate_uv_delta_q'] = 0
    elif cp == 1 and tc == 13 and mc == 0:
        s['subsampling_x'] = s['subsampling_y'] = 0
        s['separate_uv_delta_q'] = r.f(1)
    else:
        r.f(1)  # color_range
        if s['seq_profile'] == 0:
            s['subsampling_x'] = s['subsampling_y'] = 1
        elif s['seq_profile'] == 1:
            s['subsampling_x'] = s['subsampling_y'] = 0
        else:
            if s['BitDepth'] == 12:
                s['subsampling_x'] = r.f(1)
                s['subsampling_y'] = r.f(1) if s['subsampling_x'] else 0
            else:
                s['subsampling_x'] = 1
                s['subsampling_y'] = 0
        if s['subsampling_x'] and s['subsampling_y']:
            r.f(2)  # chroma_sample_position
        s['separate_uv_delta_q'] = r.f(1)
    s['film_grain_params_present'] = r.f(1)
    return s


# --------------------------------------------------------- frame header

def tile_log2(blkSize, target):
    k = 0
    while (blkSize << k) < target:
        k += 1
    return k


def decode_frame_header(payload, s, obu_type, dpb=None, upd=None):
    """Walk uncompressed_header(). Returns [(name, value)] in bit order.

    `dpb` is the decoder's `RefOrderHint[NUM_REF_FRAMES]`, threaded across the
    frames of the stream by the caller. It is what `skip_mode_params()` needs
    (spec 5.9.2 -> `skip_mode_params`): without it the walk cannot know whether
    `skip_mode_present` is even PRESENT, and a wrong presence decision shifts
    every field after it. Pass `None` only when no earlier frame is available;
    the walk then says so instead of guessing.
    """
    r = BR(payload)
    o = []
    if upd is None:
        upd = {}      # what the caller must apply to `dpb` after this frame

    def rd(name, n):
        v = r.f(n)
        o.append((name, v))
        return v

    def rd_su(name, n):
        v = r.su(n)
        o.append((name, v))
        return v

    def note(name, v):
        o.append((name, v))
        return v

    if s['reduced_still_picture_header']:
        frame_type = KEY_FRAME
        FrameIsIntra = 1
        show_frame = 1
        showable_frame = 0
        error_resilient_mode = 0
        note('frame_type*', frame_type)
    else:
        show_existing = rd('show_existing_frame', 1)
        if show_existing:
            upd['show_existing'] = rd('frame_to_show_map_idx', 3)
            upd['refresh_frame_flags'] = 0
            return o
        frame_type = rd('frame_type', 2)
        FrameIsIntra = 1 if frame_type in (KEY_FRAME, INTRA_ONLY_FRAME) else 0
        show_frame = rd('show_frame', 1)
        if show_frame and s['decoder_model_info_present_flag'] and not s.get('equal_picture_interval', 0):
            rd('frame_presentation_time', s['frame_presentation_time_length_minus_1'] + 1)
        if show_frame:
            showable_frame = 1 if frame_type != KEY_FRAME else 0
        else:
            showable_frame = rd('showable_frame', 1)
        if frame_type == SWITCH_FRAME or (frame_type == KEY_FRAME and show_frame):
            error_resilient_mode = note('error_resilient_mode*', 1)
        else:
            error_resilient_mode = rd('error_resilient_mode', 1)

    disable_cdf_update = rd('disable_cdf_update', 1)
    if s['seq_force_screen_content_tools'] == SELECT_SCREEN_CONTENT_TOOLS:
        allow_screen_content_tools = rd('allow_screen_content_tools', 1)
    else:
        allow_screen_content_tools = note('allow_screen_content_tools*',
                                          s['seq_force_screen_content_tools'])
    if allow_screen_content_tools:
        if s['seq_force_integer_mv'] == SELECT_INTEGER_MV:
            rd('force_integer_mv', 1)
    if s['frame_id_numbers_present_flag']:
        idLen = (s['additional_frame_id_length_minus_1'] + 1 +
                 s['delta_frame_id_length_minus_2'] + 2)
        rd('current_frame_id', idLen)
    if frame_type == SWITCH_FRAME:
        frame_size_override_flag = note('frame_size_override_flag*', 1)
    elif s['reduced_still_picture_header']:
        frame_size_override_flag = note('frame_size_override_flag*', 0)
    else:
        frame_size_override_flag = rd('frame_size_override_flag', 1)
    OrderHint = 0
    if s['OrderHintBits']:
        OrderHint = rd('order_hint', s['OrderHintBits'])
    upd['OrderHint'] = OrderHint
    if FrameIsIntra or error_resilient_mode:
        primary_ref_frame = note('primary_ref_frame*', PRIMARY_REF_NONE)
    else:
        primary_ref_frame = rd('primary_ref_frame', 3)
    if s['decoder_model_info_present_flag']:
        if rd('buffer_removal_time_present_flag', 1):
            for opNum in range(s['operating_points_cnt_minus_1'] + 1):
                if s['decoder_model_present_for_this_op'][opNum]:
                    idc = s['operating_point_idc'][opNum]
                    # temporal/spatial applicability is not modelled; assume it
                    # applies (the SVT configs never enable the decoder model).
                    if idc == 0:
                        rd(f'buffer_removal_time[{opNum}]',
                           s['buffer_removal_time_length_minus_1'] + 1)
    if frame_type == SWITCH_FRAME or (frame_type == KEY_FRAME and show_frame):
        refresh_frame_flags = note('refresh_frame_flags*', 255)
    else:
        refresh_frame_flags = rd('refresh_frame_flags', 8)
    upd['refresh_frame_flags'] = refresh_frame_flags
    if (not FrameIsIntra) or refresh_frame_flags != 255:
        if error_resilient_mode and s['enable_order_hint']:
            for i in range(NUM_REF_FRAMES):
                rd(f'ref_order_hint[{i}]', s['OrderHintBits'])

    allow_intrabc = 0
    allow_high_precision_mv = 0
    is_motion_mode_switchable = 0
    use_ref_frame_mvs = 0

    def frame_size():
        if frame_size_override_flag:
            w = rd('frame_width_minus_1', s['frame_width_bits_minus_1'] + 1) + 1
            h = rd('frame_height_minus_1', s['frame_height_bits_minus_1'] + 1) + 1
        else:
            w = s['max_frame_width_minus_1'] + 1
            h = s['max_frame_height_minus_1'] + 1
        # superres_params()
        if s['enable_superres']:
            use_superres = rd('use_superres', 1)
        else:
            use_superres = 0
        if use_superres:
            rd('coded_denom', SUPERRES_DENOM_BITS)
        return w, h

    def render_size():
        if rd('render_and_frame_size_different', 1):
            rd('render_width_minus_1', 16)
            rd('render_height_minus_1', 16)

    if FrameIsIntra:
        FrameWidth, FrameHeight = frame_size()
        render_size()
        if allow_screen_content_tools and True:  # UpscaledWidth == FrameWidth
            allow_intrabc = rd('allow_intrabc', 1)
    else:
        frame_refs_short_signaling = 0
        if s['enable_order_hint']:
            frame_refs_short_signaling = rd('frame_refs_short_signaling', 1)
            if frame_refs_short_signaling:
                rd('last_frame_idx', 3)
                rd('gold_frame_idx', 3)
        ref_frame_idx = [0] * REFS_PER_FRAME
        for i in range(REFS_PER_FRAME):
            if not frame_refs_short_signaling:
                ref_frame_idx[i] = rd(f'ref_frame_idx[{i}]', 3)
            if s['frame_id_numbers_present_flag']:
                rd(f'delta_frame_id_minus_1[{i}]',
                   s['delta_frame_id_length_minus_2'] + 2)
        if frame_size_override_flag and not error_resilient_mode:
            # frame_size_with_refs()
            found = 0
            for i in range(REFS_PER_FRAME):
                found = rd(f'found_ref[{i}]', 1)
                if found:
                    break
            if not found:
                FrameWidth, FrameHeight = frame_size()
                render_size()
            else:
                if s['enable_superres']:
                    if rd('use_superres', 1):
                        rd('coded_denom', SUPERRES_DENOM_BITS)
                FrameWidth = s['max_frame_width_minus_1'] + 1
                FrameHeight = s['max_frame_height_minus_1'] + 1
        else:
            FrameWidth, FrameHeight = frame_size()
            render_size()
        if s['seq_force_integer_mv'] == SELECT_INTEGER_MV or not FrameIsIntra:
            pass
        allow_high_precision_mv = 0
        # force_integer_mv is 0 unless signalled; SVT never forces it here.
        allow_high_precision_mv = rd('allow_high_precision_mv', 1)
        # read_interpolation_filter()
        if rd('is_filter_switchable', 1) == 0:
            rd('interpolation_filter', 2)
        is_motion_mode_switchable = rd('is_motion_mode_switchable', 1)
        if not (error_resilient_mode or not s['enable_ref_frame_mvs']):
            use_ref_frame_mvs = rd('use_ref_frame_mvs', 1)

    if s['reduced_still_picture_header'] or disable_cdf_update:
        note('disable_frame_end_update_cdf*', 1)
    else:
        rd('disable_frame_end_update_cdf', 1)

    # ---- tile_info() -------------------------------------------------
    MiCols = 2 * ((FrameWidth + 7) >> 3)
    MiRows = 2 * ((FrameHeight + 7) >> 3)
    sbShift = 5 if s['use_128x128_superblock'] else 4
    sbSize = sbShift + 2
    sbCols = (MiCols + 31) >> 5 if s['use_128x128_superblock'] else (MiCols + 15) >> 4
    sbRows = (MiRows + 31) >> 5 if s['use_128x128_superblock'] else (MiRows + 15) >> 4
    maxTileWidthSb = MAX_TILE_WIDTH >> sbSize
    maxTileAreaSb = MAX_TILE_AREA >> (2 * sbSize)
    minLog2TileCols = tile_log2(maxTileWidthSb, sbCols)
    maxLog2TileCols = tile_log2(1, min(sbCols, MAX_TILE_COLS))
    maxLog2TileRows = tile_log2(1, min(sbRows, MAX_TILE_ROWS))
    minLog2Tiles = max(minLog2TileCols, tile_log2(maxTileAreaSb, sbRows * sbCols))
    if rd('uniform_tile_spacing_flag', 1):
        TileColsLog2 = minLog2TileCols
        while TileColsLog2 < maxLog2TileCols:
            if rd('increment_tile_cols_log2', 1):
                TileColsLog2 += 1
            else:
                break
        minLog2TileRows = max(minLog2Tiles - TileColsLog2, 0)
        TileRowsLog2 = minLog2TileRows
        while TileRowsLog2 < maxLog2TileRows:
            if rd('increment_tile_rows_log2', 1):
                TileRowsLog2 += 1
            else:
                break
    else:
        widestTileSb = 0
        startSb = 0
        i = 0
        while startSb < sbCols:
            maxWidth = min(sbCols - startSb, maxTileWidthSb)
            w = rd(f'width_in_sbs_minus_1[{i}]', (maxWidth - 1).bit_length()) + 1
            widestTileSb = max(w, widestTileSb)
            startSb += w
            i += 1
        TileColsLog2 = tile_log2(1, i)
        startSb = 0
        i = 0
        maxTileAreaSb2 = maxTileAreaSb // widestTileSb if widestTileSb else maxTileAreaSb
        while startSb < sbRows:
            maxHeight = min(sbRows - startSb, maxTileAreaSb2)
            h = rd(f'height_in_sbs_minus_1[{i}]', (maxHeight - 1).bit_length()) + 1
            startSb += h
            i += 1
        TileRowsLog2 = tile_log2(1, i)
    if TileColsLog2 > 0 or TileRowsLog2 > 0:
        rd('context_update_tile_id', TileRowsLog2 + TileColsLog2)
        rd('tile_size_bytes_minus_1', 2)

    # ---- quantization_params() ---------------------------------------
    base_q_idx = rd('base_q_idx', 8)

    def read_delta_q(tag):
        if rd(f'{tag}_coded', 1):
            return rd_su(tag, 7)
        return 0

    DeltaQYDc = read_delta_q('delta_q_y_dc')
    diff_uv_delta = 0
    DeltaQUDc = DeltaQUAc = DeltaQVDc = DeltaQVAc = 0
    if s['NumPlanes'] > 1:
        if s['separate_uv_delta_q']:
            diff_uv_delta = rd('diff_uv_delta', 1)
        DeltaQUDc = read_delta_q('delta_q_u_dc')
        DeltaQUAc = read_delta_q('delta_q_u_ac')
        if diff_uv_delta:
            DeltaQVDc = read_delta_q('delta_q_v_dc')
            DeltaQVAc = read_delta_q('delta_q_v_ac')
        else:
            DeltaQVDc, DeltaQVAc = DeltaQUDc, DeltaQUAc
    if rd('using_qmatrix', 1):
        rd('qm_y', 4)
        rd('qm_u', 4)
        if s['separate_uv_delta_q']:
            rd('qm_v', 4)

    # ---- segmentation_params() ---------------------------------------
    seg_qindex_deltas = []
    if rd('segmentation_enabled', 1):
        if primary_ref_frame == PRIMARY_REF_NONE:
            segmentation_update_data = 1
        else:
            if rd('segmentation_update_map', 1):
                rd('segmentation_temporal_update', 1)
            segmentation_update_data = rd('segmentation_update_data', 1)
        if segmentation_update_data:
            for i in range(MAX_SEGMENTS):
                for j in range(SEG_LVL_MAX):
                    if rd(f'feature_enabled[{i}][{j}]', 1):
                        bits = SEG_FEATURE_BITS[j]
                        if SEG_FEATURE_SIGNED[j]:
                            v = rd_su(f'feature_value[{i}][{j}]', 1 + bits)
                        else:
                            v = rd(f'feature_value[{i}][{j}]', bits) if bits else 0
                        if j == 0:
                            seg_qindex_deltas.append(v)
        SegQMDeltas = seg_qindex_deltas
    else:
        SegQMDeltas = []

    # ---- delta_q_params() / delta_lf_params() ------------------------
    delta_q_present = 0
    if base_q_idx > 0:
        delta_q_present = rd('delta_q_present', 1)
    if delta_q_present:
        rd('delta_q_res', 2)
        if not allow_intrabc:
            if rd('delta_lf_present', 1):
                rd('delta_lf_res', 2)
                rd('delta_lf_multi', 1)

    # CodedLossless: every segment's qindex 0 and all deltas 0.
    qidx = [max(0, min(255, base_q_idx + d)) for d in SegQMDeltas] or [base_q_idx]
    CodedLossless = all(q == 0 for q in qidx) and DeltaQYDc == 0 and \
        DeltaQUAc == 0 and DeltaQUDc == 0 and DeltaQVAc == 0 and DeltaQVDc == 0
    AllLossless = CodedLossless  # UpscaledWidth == FrameWidth (no superres here)

    # ---- loop_filter_params() ----------------------------------------
    if CodedLossless or allow_intrabc:
        note('loop_filter_level[0]*', 0)
        note('loop_filter_level[1]*', 0)
    else:
        lvl0 = rd('loop_filter_level[0]', 6)
        lvl1 = rd('loop_filter_level[1]', 6)
        if s['NumPlanes'] > 1 and (lvl0 or lvl1):
            rd('loop_filter_level[2]', 6)
            rd('loop_filter_level[3]', 6)
        rd('loop_filter_sharpness', 3)
        if rd('loop_filter_delta_enabled', 1):
            if rd('loop_filter_delta_update', 1):
                for i in range(TOTAL_REFS_PER_FRAME):
                    if rd(f'update_ref_delta[{i}]', 1):
                        rd_su(f'loop_filter_ref_deltas[{i}]', 7)
                for i in range(2):
                    if rd(f'update_mode_delta[{i}]', 1):
                        rd_su(f'loop_filter_mode_deltas[{i}]', 7)

    # ---- cdef_params() -----------------------------------------------
    if not (CodedLossless or allow_intrabc or not s['enable_cdef']):
        rd('cdef_damping_minus_3', 2)
        cdef_bits = rd('cdef_bits', 2)
        for i in range(1 << cdef_bits):
            rd(f'cdef_y_pri_strength[{i}]', 4)
            rd(f'cdef_y_sec_strength[{i}]', 2)
            if s['NumPlanes'] > 1:
                rd(f'cdef_uv_pri_strength[{i}]', 4)
                rd(f'cdef_uv_sec_strength[{i}]', 2)

    # ---- lr_params() -------------------------------------------------
    if not (AllLossless or allow_intrabc or not s['enable_restoration']):
        UsesLr = 0
        usesChromaLr = 0
        for i in range(s['NumPlanes']):
            t = rd(f'lr_type[{i}]', 2)
            if t != RESTORE_NONE:
                UsesLr = 1
                if i > 0:
                    usesChromaLr = 1
        if UsesLr:
            if s['use_128x128_superblock']:
                rd('lr_unit_shift', 1)
            else:
                if rd('lr_unit_shift', 1):
                    rd('lr_unit_extra_shift', 1)
            if s['subsampling_x'] and s['subsampling_y'] and usesChromaLr:
                rd('lr_uv_shift', 1)

    # ---- read_tx_mode() ----------------------------------------------
    if CodedLossless:
        note('tx_mode*', 0)
    else:
        rd('tx_mode_select', 1)

    # ---- frame_reference_mode() / skip_mode_params() -----------------
    if FrameIsIntra:
        reference_select = note('reference_select*', 0)
    else:
        reference_select = rd('reference_select', 1)
    # skip_mode_params() — spec 5.9.2. The PRESENCE of `skip_mode_present`
    # depends on the reference ORDER HINTS, so it needs the decoder's
    # RefOrderHint[] as of this frame.
    #
    # This used to be approximated as "1 whenever reference_select is set",
    # with a comment admitting the real rule needs the order hints. That
    # approximation is WRONG on the inter campaign's own 2-frame cell
    # (MEASURED 2026-09-01, gradient 64x64 q40 p6): every DPB slot still holds
    # the key frame, so there is no second distinct forward reference and C
    # writes NO skip_mode_present bit — but the tool read one, and then
    # reported `allow_warped_motion = 0` when the stream says 1. Every field
    # from `skip_mode_present` on was off by one bit, and the printout gave no
    # sign of it because the shifted values were all zeros.
    skipModeAllowed = 0
    if FrameIsIntra or not reference_select or not s['enable_order_hint']:
        skipModeAllowed = 0
    elif dpb is None:
        note('skip_mode_present?', -1)
        o.append(('(no DPB: RefOrderHint unknown, walk stops here)', -1))
        return o
    else:
        OrderHints = [0] * (ALTREF_FRAME + 1)
        for i in range(REFS_PER_FRAME):
            OrderHints[LAST_FRAME + i] = dpb[ref_frame_idx[i]]
        forwardIdx = -1
        backwardIdx = -1
        forwardHint = 0
        backwardHint = 0
        for i in range(REFS_PER_FRAME):
            refHint = OrderHints[LAST_FRAME + i]
            if get_relative_dist(s, refHint, OrderHint) < 0:
                if forwardIdx < 0 or get_relative_dist(s, refHint, forwardHint) > 0:
                    forwardIdx = i
                    forwardHint = refHint
            elif get_relative_dist(s, refHint, OrderHint) > 0:
                if backwardIdx < 0 or get_relative_dist(s, refHint, backwardHint) < 0:
                    backwardIdx = i
                    backwardHint = refHint
        if forwardIdx < 0:
            skipModeAllowed = 0
        elif backwardIdx >= 0:
            skipModeAllowed = 1
        else:
            secondForwardIdx = -1
            secondForwardHint = 0
            for i in range(REFS_PER_FRAME):
                refHint = OrderHints[LAST_FRAME + i]
                if get_relative_dist(s, refHint, forwardHint) < 0:
                    if (secondForwardIdx < 0
                            or get_relative_dist(s, refHint, secondForwardHint) > 0):
                        secondForwardIdx = i
                        secondForwardHint = refHint
            skipModeAllowed = 1 if secondForwardIdx >= 0 else 0
    if skipModeAllowed:
        rd('skip_mode_present', 1)

    if FrameIsIntra or error_resilient_mode or not s['enable_warped_motion']:
        note('allow_warped_motion*', 0)
    else:
        rd('allow_warped_motion', 1)
    rd('reduced_tx_set', 1)
    return o


def frame_obu_payloads(path):
    """(seq_header_state_or_None, [frame-header payloads in stream order])."""
    data = open(path, 'rb').read()
    seq = None
    frames = []
    for t, _tid, _sid, p in obus(data):
        if t == OBU_SEQUENCE_HEADER and seq is None:
            seq = parse_seq_header(p)
        elif t in (OBU_FRAME, OBU_FRAME_HEADER):
            frames.append((t, p))
    return seq, frames


def main(argv):
    index = 0
    seq_src = None
    args = []
    i = 1
    while i < len(argv):
        if argv[i] == '--index':
            index = int(argv[i + 1])
            i += 2
        elif argv[i] == '--seq':
            seq_src = argv[i + 1]
            i += 2
        else:
            args.append(argv[i])
            i += 1
    if len(args) != 2:
        print(__doc__)
        return 2
    out = []
    for path in args:
        seq, frames = frame_obu_payloads(path)
        if seq is None and seq_src:
            seq, _ = frame_obu_payloads(seq_src)
        if seq is None:
            print(f"{path}: no sequence header OBU and no --seq given", file=sys.stderr)
            return 2
        if index >= len(frames):
            print(f"{path}: only {len(frames)} frame OBU(s), wanted index {index}",
                  file=sys.stderr)
            return 2
        # Walk EVERY frame up to `index`, maintaining the decoder's
        # RefOrderHint[] — `skip_mode_params()` reads it, so frame N cannot be
        # parsed correctly without the frames before it.
        # Walk EVERY frame up to `index`, maintaining the decoder's
        # RefOrderHint[] — `skip_mode_params()` reads it, so frame N cannot be
        # parsed correctly without the frames before it.
        dpb = None
        fields = None
        try:
            for k in range(index + 1):
                t, p = frames[k]
                upd = {}
                fields = decode_frame_header(p, seq, t, dpb, upd)
                if dpb is None:
                    dpb = [0] * NUM_REF_FRAMES
                mask = upd.get('refresh_frame_flags', 0)
                for slot in range(NUM_REF_FRAMES):
                    if mask & (1 << slot):
                        dpb[slot] = upd.get('OrderHint', 0)
        except EOFError as e:
            print(f"{path}: {e}", file=sys.stderr)
            return 2
        out.append(fields)
    a, b = out
    print(f"{'field':38} {'A':>8} {'B':>8}")
    first = True
    for (na, va), (nb, vb) in zip(a, b):
        same = (na == nb and va == vb)
        flag = ''
        if not same:
            flag = '   <-- DIFFERS' + ('  (FIRST)' if first else '')
            first = False
        # Once the walks disagree on a NAME the rows are no longer the same
        # field, so print both names rather than implying one label for two
        # different syntax elements.
        label = na if na == nb else f'{na} | {nb}'
        print(f"{label:38} {va:>8} {vb:>8}{flag}")
    if len(a) != len(b):
        print(f"(field counts differ: A={len(a)} B={len(b)} — "
              f"the walk diverged, read only up to the first DIFFERS)")
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Bitstream-identity differ: Rust EncodePipeline vs C SVT-AV1.

Compares two raw OBU streams (TD + SH + Frame) byte-wise with field-level
decode hints for SH / FH, and two arithmetic-coder op traces (the Rust
`symtrace` stderr lines vs the C `--wrap` capture from
tools/capture_c_trace) after canonicalization.

Canonical op forms (arithmetic-equivalent encodings unify):
  BOOLEQ val            == BOOL val f=16384        (C aom_write_bit)
  CDF nsyms=2 s icdf=[f] == BOOL s f               (Rust writes 2-symbol CDFs
                                                    through the generic path;
                                                    C routes them to the bool
                                                    coder with f = icdf[0] —
                                                    provably identical
                                                    arithmetic, see
                                                    bitstream_unit.h)
`rng` (coder range before the op) rides along as a state checksum: if every
prior canonical op matched but rng differs, an op escaped one of the traces.

Usage:
  identity_diff.py --c-obu C.obu --rust-obu R.obu \
                   --c-trace C.trace --rust-trace R.trace [--context 8]
"""

import argparse
import re
import sys
from collections import Counter

# ----------------------------------------------------------------------------
# Bit reader + field logging
# ----------------------------------------------------------------------------


class Bits:
    """MSB-first bit cursor over bytes, recording named fields as it goes."""

    def __init__(self, data):
        self.data = data
        self.pos = 0  # bit position
        self.fields = []  # (bitpos, nbits, name, value)

    def f(self, n, name):
        v = 0
        start = self.pos
        for _ in range(n):
            byte = self.data[self.pos >> 3]
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1)
            self.pos += 1
        self.fields.append((start, n, name, v))
        return v

    def su(self, n, name):
        """Signed: value f(n), sign f(1) -> value = sign ? -v : v (spec su())."""
        v = self.f(n, name + ".val")
        s = self.f(1, name + ".sign")
        return -v if s else v


def leb128(data, off):
    v = 0
    for i in range(8):
        b = data[off + i]
        v |= (b & 0x7F) << (7 * i)
        if not (b & 0x80):
            return v, i + 1
    raise ValueError("leb128 too long")


OBU_NAMES = {
    1: "SEQUENCE_HEADER",
    2: "TEMPORAL_DELIMITER",
    3: "FRAME_HEADER",
    4: "TILE_GROUP",
    5: "METADATA",
    6: "FRAME",
    7: "REDUNDANT_FRAME_HEADER",
    15: "PADDING",
}


def parse_obus(data, label):
    """-> list of dicts {type, name, hdr_len, size, payload, offset}."""
    out = []
    off = 0
    while off < len(data):
        hdr = data[off]
        forbidden = hdr >> 7
        obu_type = (hdr >> 3) & 0xF
        ext = (hdr >> 2) & 1
        has_size = (hdr >> 1) & 1
        if forbidden:
            raise ValueError(f"{label}: forbidden bit set at byte {off}")
        hlen = 1 + ext
        if not has_size:
            raise ValueError(f"{label}: OBU without size field at byte {off} unsupported")
        size, slen = leb128(data, off + hlen)
        hlen += slen
        payload = data[off + hlen : off + hlen + size]
        out.append(
            dict(
                type=obu_type,
                name=OBU_NAMES.get(obu_type, f"type{obu_type}"),
                hdr_len=hlen,
                size=size,
                payload=payload,
                offset=off,
            )
        )
        off += hlen + size
    return out


# ----------------------------------------------------------------------------
# Sequence header field walk (reduced-still-picture path, profile 0)
# ----------------------------------------------------------------------------


def decode_sh(payload):
    b = Bits(payload)
    profile = b.f(3, "seq_profile")
    b.f(1, "still_picture")
    reduced = b.f(1, "reduced_still_picture_header")
    if not reduced:
        raise ValueError("non-reduced SH walk not implemented")
    b.f(5, "seq_level_idx[0]")
    wbits = b.f(4, "frame_width_bits_minus_1") + 1
    hbits = b.f(4, "frame_height_bits_minus_1") + 1
    width = b.f(wbits, "max_frame_width_minus_1") + 1
    height = b.f(hbits, "max_frame_height_minus_1") + 1
    # reduced -> no frame ids
    use_128 = b.f(1, "use_128x128_superblock")
    b.f(1, "enable_filter_intra")
    b.f(1, "enable_intra_edge_filter")
    # reduced -> inter tools all 0, no order hint
    enable_superres = b.f(1, "enable_superres")
    b.f(1, "enable_cdef")
    b.f(1, "enable_restoration")
    # color_config()
    high_bd = b.f(1, "high_bitdepth")
    if profile == 2 and high_bd:
        b.f(1, "twelve_bit")
    mono = 0
    if profile != 1:
        mono = b.f(1, "mono_chrome")
    desc = b.f(1, "color_description_present_flag")
    cp = tc = mc = None
    if desc:
        cp = b.f(8, "color_primaries")
        tc = b.f(8, "transfer_characteristics")
        mc = b.f(8, "matrix_coefficients")
    if mono:
        b.f(1, "color_range")
    elif desc and cp == 1 and tc == 13 and mc == 0:
        pass  # sRGB: full range + 4:4:4 implied
    else:
        b.f(1, "color_range")
        # profile 0 -> 4:2:0 (subsampling 1,1)
        if profile == 0:
            b.f(2, "chroma_sample_position")
    if not mono:
        b.f(1, "separate_uv_delta_q")
    b.f(1, "film_grain_params_present")
    return b, dict(
        enable_superres=enable_superres,
        mono=mono,
        profile=profile,
        width=width,
        height=height,
        use_128=use_128,
    )


# ----------------------------------------------------------------------------
# Frame header field walk (key frame under reduced-still SH)
# ----------------------------------------------------------------------------


def decode_fh(payload, sh_info, num_planes):
    b = Bits(payload)
    # reduced_still_picture_header: frame_type=KEY, show_frame=1 (implied)
    b.f(1, "disable_cdf_update")
    asct = b.f(1, "allow_screen_content_tools")
    # FrameIsIntra -> force_integer_mv = 1 implied (not coded)
    # frame_size(): reduced -> frame_size_override_flag = 0, dims from SH
    if sh_info.get("enable_superres"):
        b.f(1, "use_superres")
    rframe = b.f(1, "render_and_frame_size_different")
    if rframe:
        b.f(16, "render_width_minus_1")
        b.f(16, "render_height_minus_1")
    allow_intrabc = 0
    if asct:
        allow_intrabc = b.f(1, "allow_intrabc")
    # key + show_frame -> refresh_frame_flags not coded
    # intra frame -> no ref frame syntax, no interpolation filter
    # reduced_still_picture_header -> disable_frame_end_update_cdf implied 1?
    # NO: implied only via disable_cdf_update; spec 5.9.2:
    #   if (reduced_still_picture_header || disable_cdf_update)
    #       disable_frame_end_update_cdf = 1  (not coded)
    # tile_info() — spec 5.9.15. Once a frame has more than one SB per
    # direction, uniform spacing is followed by increment_tile_cols/rows_log2
    # bits (one per possible doubling, terminated by a 0 bit or the max).
    uniform = b.f(1, "uniform_tile_spacing_flag")
    mi_cols = 2 * ((sh_info.get("width", 64) + 7) >> 3)
    mi_rows = 2 * ((sh_info.get("height", 64) + 7) >> 3)
    if sh_info.get("use_128"):
        sb_cols = (mi_cols + 31) >> 5
        sb_rows = (mi_rows + 31) >> 5
        sb_size_log2 = 7
    else:
        sb_cols = (mi_cols + 15) >> 4
        sb_rows = (mi_rows + 15) >> 4
        sb_size_log2 = 6
    max_tile_width_sb = 4096 >> sb_size_log2
    max_tile_area_sb = (4096 * 2304) >> (2 * sb_size_log2)

    def tl2(a, target):
        k = 0
        while (a << k) < target:
            k += 1
        return k

    min_log2_tile_cols = tl2(max_tile_width_sb, sb_cols)
    max_log2_tile_cols = tl2(1, min(sb_cols, 64))
    max_log2_tile_rows = tl2(1, min(sb_rows, 64))
    min_log2_tiles = max(min_log2_tile_cols, tl2(max_tile_area_sb, sb_rows * sb_cols))
    if not uniform:
        raise ValueError("non-uniform tile spacing walk not implemented")
    tile_cols_log2 = min_log2_tile_cols
    while tile_cols_log2 < max_log2_tile_cols:
        if b.f(1, f"increment_tile_cols_log2[{tile_cols_log2}]"):
            tile_cols_log2 += 1
        else:
            break
    min_log2_tile_rows = max(min_log2_tiles - tile_cols_log2, 0)
    tile_rows_log2 = min_log2_tile_rows
    while tile_rows_log2 < max_log2_tile_rows:
        if b.f(1, f"increment_tile_rows_log2[{tile_rows_log2}]"):
            tile_rows_log2 += 1
        else:
            break
    if tile_cols_log2 > 0 or tile_rows_log2 > 0:
        b.f(tile_cols_log2 + tile_rows_log2, "context_update_tile_id")
        b.f(2, "tile_size_bytes_minus_1")
    # quantization_params()
    base_q = b.f(8, "base_q_idx")
    coded_lossless_possible = base_q == 0
    if b.f(1, "delta_q_y_dc.coded"):
        b.su(6, "delta_q_y_dc")
        coded_lossless_possible = False
    if num_planes > 1:
        diff_uv = 0
        if sh_info.get("separate_uv_delta_q"):
            diff_uv = b.f(1, "diff_uv_delta")
        if b.f(1, "delta_q_u_dc.coded"):
            b.su(6, "delta_q_u_dc")
        if b.f(1, "delta_q_u_ac.coded"):
            b.su(6, "delta_q_u_ac")
        if diff_uv:
            if b.f(1, "delta_q_v_dc.coded"):
                b.su(6, "delta_q_v_dc")
            if b.f(1, "delta_q_v_ac.coded"):
                b.su(6, "delta_q_v_ac")
    b.f(1, "using_qmatrix")
    # segmentation_params()
    seg = b.f(1, "segmentation_enabled")
    if seg:
        raise ValueError("segmentation walk not implemented")
    # delta_q_params()
    delta_q_present = 0
    if base_q > 0:
        delta_q_present = b.f(1, "delta_q_present")
    if delta_q_present:
        b.f(2, "delta_q_res")
        if not allow_intrabc:
            dlf = b.f(1, "delta_lf_present")
            if dlf:
                b.f(2, "delta_lf_res")
                b.f(1, "delta_lf_multi")
    # CodedLossless assumed false for base_q>0 configs the harness uses.
    # loop_filter_params()
    l0 = b.f(6, "loop_filter_level[0]")
    l1 = b.f(6, "loop_filter_level[1]")
    if num_planes > 1 and (l0 or l1):
        b.f(6, "loop_filter_level[2]")
        b.f(6, "loop_filter_level[3]")
    b.f(3, "loop_filter_sharpness")
    if b.f(1, "loop_filter_delta_enabled"):
        if b.f(1, "loop_filter_delta_update"):
            raise ValueError("loop filter delta update walk not implemented")
    # cdef_params() — only when SH.enable_cdef and not (lossless/intrabc)
    if sh_info.get("enable_cdef") and not allow_intrabc:
        b.f(2, "cdef_damping_minus_3")
        cbits = b.f(2, "cdef_bits")
        for i in range(1 << cbits):
            b.f(4, f"cdef_y_pri_strength[{i}]")
            b.f(2, f"cdef_y_sec_strength[{i}]")
            if num_planes > 1:
                b.f(4, f"cdef_uv_pri_strength[{i}]")
                b.f(2, f"cdef_uv_sec_strength[{i}]")
    # lr_params() — only when SH.enable_restoration
    if sh_info.get("enable_restoration") and not allow_intrabc:
        uses = 0
        uses_chroma_lr = 0
        for i in range(num_planes):
            t = b.f(2, f"lr_type[{i}]")
            uses |= t != 0
            if i > 0:
                uses_chroma_lr |= t != 0
        if uses:
            # spec 5.9.20 lr unit size
            if sh_info.get("use_128"):
                b.f(1, "lr_unit_shift")  # shift = bit + 1
            else:
                if b.f(1, "lr_unit_shift"):
                    b.f(1, "lr_unit_extra_shift")
            if num_planes > 1 and uses_chroma_lr:
                # profile 0 -> subsampling_x = subsampling_y = 1
                b.f(1, "lr_uv_shift")
    # read_tx_mode()
    b.f(1, "tx_mode_select")
    # intra frame: no reference_select / skip_mode / warped motion
    b.f(1, "reduced_tx_set")
    # global motion: intra -> none; film grain: present flag off in harness
    return b


def bit_diff_pos(a, b):
    n = min(len(a), len(b))
    for i in range(n):
        if a[i] != b[i]:
            x = a[i] ^ b[i]
            bit = 0
            while not (x & (0x80 >> bit)):
                bit += 1
            return i, bit
    return (n, 0) if len(a) != len(b) else None


def hexctx(data, off, back=4, fwd=8):
    lo = max(0, off - back)
    hi = min(len(data), off + fwd)
    parts = []
    for i in range(lo, hi):
        s = f"{data[i]:02x}"
        parts.append(f"[{s}]" if i == off else s)
    return " ".join(parts)


def field_at(fields, bitpos):
    for start, n, name, v in fields:
        if start <= bitpos < start + n:
            return (start, n, name, v)
    return None


def print_field_walk_diff(name, cw, rw):
    """Side-by-side field walk comparison; returns True if all equal."""
    same = True
    print(f"    {name} field walk (C | Rust):")
    n = max(len(cw.fields), len(rw.fields))
    for i in range(n):
        cf = cw.fields[i] if i < len(cw.fields) else None
        rf = rw.fields[i] if i < len(rw.fields) else None
        if cf and rf and cf[2] == rf[2] and cf[3] == rf[3]:
            continue  # matching field, matching value: stay quiet
        same = False
        cs = f"@{cf[0]:<4} {cf[2]}={cf[3]}" if cf else "(end)"
        rs = f"@{rf[0]:<4} {rf[2]}={rf[3]}" if rf else "(end)"
        print(f"      DIFF #{i}: C {cs:<44} | R {rs}")
    if same:
        print("      all decoded fields identical "
              f"({len(cw.fields)} fields, {cw.pos} bits)")
    return same


# ----------------------------------------------------------------------------
# Trace canonicalization
# ----------------------------------------------------------------------------

RE_CDF = re.compile(r"^W CDF nsyms=(\d+) s=(\d+) icdf=\[(\d+),(\d+),(\d+)\](?: rng=(\d+))?")
RE_BOOL = re.compile(r"^W BOOL val=(\d+) f=(\d+)(?: rng=(\d+))?")
RE_BOOLEQ = re.compile(r"^W BOOLEQ val=(\d+)(?: rng=(\d+))?")


def parse_trace(path):
    """-> (ops, markers). ops: list of (canon_tuple, rng, raw_line, lineno)."""
    ops = []
    markers = []
    with open(path, "r", errors="replace") as f:
        for lineno, line in enumerate(f, 1):
            line = line.rstrip("\n")
            m = RE_CDF.match(line)
            if m:
                nsyms, s, i0, i1, i2, rng = (int(x) if x is not None else None for x in m.groups())
                if nsyms == 2:
                    canon = ("B", s, i0)
                else:
                    canon = ("C", nsyms, s, i0, i1, i2)
                ops.append((canon, rng, line, lineno))
                continue
            m = RE_BOOL.match(line)
            if m:
                v, fq, rng = (int(x) if x is not None else None for x in m.groups())
                ops.append((("B", v, fq), rng, line, lineno))
                continue
            m = RE_BOOLEQ.match(line)
            if m:
                v, rng = (int(x) if x is not None else None for x in m.groups())
                ops.append((("B", v, 16384), rng, line, lineno))
                continue
            if line.startswith("W INIT") or line.startswith("W RESET") or line.startswith("W DONE"):
                markers.append((len(ops), line, lineno))
    return ops, markers


def op_kind(canon):
    return f"CDF{canon[1]}" if canon[0] == "C" else "B"


def diff_traces(c_ops, r_ops, ctx):
    n = min(len(c_ops), len(r_ops))
    div = None
    rng_only = False
    for i in range(n):
        if c_ops[i][0] != r_ops[i][0]:
            div = i
            break
        cr, rr = c_ops[i][1], r_ops[i][1]
        if cr is not None and rr is not None and cr != rr:
            div = i
            rng_only = True
            break
    if div is None and len(c_ops) != len(r_ops):
        div = n  # one side ran out
    hist_side = c_ops if div is not None else c_ops
    upto = div if div is not None else len(c_ops)
    hist = Counter(op_kind(op[0]) for op in hist_side[:upto])
    return div, rng_only, hist


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--c-obu", required=True)
    ap.add_argument("--rust-obu", required=True)
    ap.add_argument("--c-trace")
    ap.add_argument("--rust-trace")
    ap.add_argument("--context", type=int, default=8)
    args = ap.parse_args()

    c_data = open(args.c_obu, "rb").read()
    r_data = open(args.rust_obu, "rb").read()

    print("=" * 78)
    print(f"OBU-LEVEL COMPARISON   C={len(c_data)}B  Rust={len(r_data)}B")
    print("=" * 78)
    c_obus = parse_obus(c_data, "C")
    r_obus = parse_obus(r_data, "Rust")
    print(f"  C stream:    {[(o['name'], o['size']) for o in c_obus]}")
    print(f"  Rust stream: {[(o['name'], o['size']) for o in r_obus]}")

    identical_stream = c_data == r_data
    print(f"  streams byte-identical: {identical_stream}")

    npairs = max(len(c_obus), len(r_obus))
    sh_info_c = sh_info_r = None
    for i in range(npairs):
        co = c_obus[i] if i < len(c_obus) else None
        ro = r_obus[i] if i < len(r_obus) else None
        if co is None or ro is None:
            print(f"\n[OBU {i}] only present on one side: "
                  f"C={co['name'] if co else '-'} Rust={ro['name'] if ro else '-'}")
            continue
        tag = f"[OBU {i}] {co['name']}"
        if co["name"] != ro["name"]:
            print(f"\n{tag}: TYPE MISMATCH C={co['name']} Rust={ro['name']}")
            continue
        cpay, rpay = co["payload"], ro["payload"]
        status = "IDENTICAL" if cpay == rpay else "DIFFERS"
        print(f"\n{tag}: C {co['size']}B, Rust {ro['size']}B -> {status}")
        walked = None
        # Field walks (also needed for FH tile-offset even when identical)
        if co["type"] == 1:  # SH
            try:
                cw, ci = decode_sh(cpay)
                rw, ri = decode_sh(rpay)
                sh_info_c, sh_info_r = ci, ri
                # stash extra SH info for FH walk
                for w_, i_ in ((cw, ci), (rw, ri)):
                    for st, nb, nm, v in w_.fields:
                        if nm in ("separate_uv_delta_q", "enable_cdef", "enable_restoration"):
                            i_[nm] = v
                walked = (cw, rw)
            except Exception as e:  # noqa: BLE001 — report and continue
                print(f"    (SH field walk failed: {e})")
        elif co["type"] == 6 and sh_info_c is not None:  # FRAME
            try:
                np_c = 1 if sh_info_c.get("mono") else 3
                np_r = 1 if sh_info_r.get("mono") else 3
                cw = decode_fh(cpay, sh_info_c, np_c)
                rw = decode_fh(rpay, sh_info_r, np_r)
                walked = (cw, rw)
            except Exception as e:  # noqa: BLE001
                print(f"    (FH field walk failed: {e})")
        if walked:
            cw, rw = walked
            print_field_walk_diff(co["name"], cw, rw)
            if co["type"] == 6:
                # Tile payload = FH bits rounded up to byte boundary, rest of OBU
                c_tile_off = (cw.pos + 7) // 8
                r_tile_off = (rw.pos + 7) // 8
                ct, rt = cpay[c_tile_off:], rpay[r_tile_off:]
                print(f"    FH decoded length: C {cw.pos} bits ({c_tile_off}B), "
                      f"Rust {rw.pos} bits ({r_tile_off}B)")
                tstat = "IDENTICAL" if ct == rt else "DIFFERS"
                print(f"    tile payload: C {len(ct)}B, Rust {len(rt)}B -> {tstat}")
                if ct != rt:
                    d = bit_diff_pos(ct, rt)
                    if d and d[0] < min(len(ct), len(rt)):
                        print(f"      first tile byte diff at +{d[0]} (bit {d[1]}):")
                        print(f"        C:    {hexctx(ct, d[0])}")
                        print(f"        Rust: {hexctx(rt, d[0])}")
        if cpay != rpay:
            d = bit_diff_pos(cpay, rpay)
            if d and d[0] < min(len(cpay), len(rpay)):
                off, bit = d
                bitpos = off * 8 + bit
                print(f"    first payload diff: byte +{off} bit {bit} (bitpos {bitpos})")
                print(f"      C:    {hexctx(cpay, off)}")
                print(f"      Rust: {hexctx(rpay, off)}")
                if walked:
                    for side, w_ in (("C", walked[0]), ("Rust", walked[1])):
                        fa = field_at(w_.fields, bitpos)
                        if fa:
                            print(f"      {side} field at that bit: {fa[2]}={fa[3]} "
                                  f"(bits {fa[0]}..{fa[0]+fa[1]-1})")
            else:
                print(f"    payloads share a {min(len(cpay), len(rpay))}B prefix; "
                      f"sizes differ")

    # ------------------------------------------------------------------
    if args.c_trace and args.rust_trace:
        print()
        print("=" * 78)
        print("TILE OP-TRACE COMPARISON (canonicalized arithmetic-coder ops)")
        print("=" * 78)
        c_ops, c_marks = parse_trace(args.c_trace)
        r_ops, r_marks = parse_trace(args.rust_trace)
        print(f"  op counts: C={len(c_ops)}  Rust={len(r_ops)}")
        c_done = [m for m in c_marks if "DONE" in m[1]]
        r_done = [m for m in r_marks if "DONE" in m[1]]
        print(f"  markers: C {len(c_marks)} (DONE: {[m[1].split(' ec=')[0] for m in c_done]})"
              f" | Rust {len(r_marks)} (DONE: {[m[1].split(' head=')[0] for m in r_done]})")

        div, rng_only, hist = diff_traces(c_ops, r_ops, args.context)
        if div is None:
            print(f"  RESULT: traces IDENTICAL for all {len(c_ops)} ops (incl. rng state)")
        else:
            both = div < min(len(c_ops), len(r_ops))
            if not both:
                print(f"  RESULT: identical for {div} ops, then op-count mismatch "
                      f"(C={len(c_ops)}, Rust={len(r_ops)})")
            elif rng_only:
                print(f"  RESULT: first divergence at op {div}: SAME op fields but rng "
                      f"differs (C rng={c_ops[div][1]}, Rust rng={r_ops[div][1]}) — an op "
                      f"escaped one trace or engines diverge; INVESTIGATE")
            else:
                print(f"  RESULT: first divergence at op {div}")
            print(f"  op-kind histogram up to divergence: "
                  f"{dict(sorted(hist.items()))}")
            ctx = args.context
            lo = max(0, div - ctx)
            print(f"\n  C ops [{lo}..{min(div + ctx, len(c_ops)) - 1}]:")
            for i in range(lo, min(div + ctx, len(c_ops))):
                mark = " <-- DIVERGENCE" if i == div else ""
                print(f"    {i:>5}: {c_ops[i][2]}{mark}")
            print(f"\n  Rust ops [{lo}..{min(div + ctx, len(r_ops)) - 1}]:")
            for i in range(lo, min(div + ctx, len(r_ops))):
                mark = " <-- DIVERGENCE" if i == div else ""
                print(f"    {i:>5}: {r_ops[i][2]}{mark}")

    print()
    print("=" * 78)
    verdict = "IDENTICAL" if identical_stream else "NOT IDENTICAL"
    print(f"VERDICT: streams {verdict}")
    print("=" * 78)
    return 0 if identical_stream else 1


if __name__ == "__main__":
    sys.exit(main())

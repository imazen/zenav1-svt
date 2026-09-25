//! hdr_diff: parse the uncompressed frame header of every FRAME/FRAME_HDR OBU
//! in each input stream with the aom-dsp reader and print every differing
//! field. Drill tool for header-level bitstream divergence.
//!
//!   hdr_diff <a.obu> <b.obu>

use aom_dsp::entropy::header::{
    FrameHeaderObu, FrameHeaderPrefix, FrameSizeHeader, SequenceHeaderObu, TileInfoHeader,
    read_sequence_header_obu, read_uncompressed_header,
};
use aom_dsp::entropy::leb128::uleb_decode;
use aom_dsp::entropy::obu::read_obu_header;
use aom_dsp::entropy::rb::ReadBitBuffer;

fn tile_log2(blk_size: i32, target: i32) -> i32 {
    let mut k = 0;
    while blk_size << k < target {
        k += 1;
    }
    k
}

fn mi_dim(px: i32) -> i32 {
    ((px + 7) & !7) >> 2
}

fn reader_cfg(seq: &SequenceHeaderObu) -> FrameHeaderObu {
    let s = &seq.seq_header;
    let c = &seq.color_config;
    let mib_size_log2 = if s.sb_size_128 { 5u32 } else { 4u32 };
    let mi_cols = mi_dim(s.max_frame_width);
    let mi_rows = mi_dim(s.max_frame_height);
    let sb_cols = (mi_cols + (1 << mib_size_log2) - 1) >> mib_size_log2;
    let sb_rows = (mi_rows + (1 << mib_size_log2) - 1) >> mib_size_log2;
    let sb_size_log2 = mib_size_log2 as i32 + 2;
    let max_width_sb = 4096 >> sb_size_log2;
    let max_tile_area_sb = (4096 * 2304) >> (2 * sb_size_log2);
    let min_log2_cols = tile_log2(max_width_sb, sb_cols);
    let max_log2_cols = tile_log2(1, sb_cols.min(64));
    let max_log2_rows = tile_log2(1, sb_rows.min(64));
    let min_log2_tiles =
        tile_log2(max_tile_area_sb, sb_cols * sb_rows).max(min_log2_cols);
    let mut cfg = FrameHeaderObu {
        prefix: FrameHeaderPrefix {
            reduced_still_picture_hdr: seq.reduced_still_picture_hdr,
            decoder_model_info_present_flag: seq.decoder_model_info_present_flag,
            equal_picture_interval: seq.timing_info.equal_picture_interval,
            frame_presentation_time_length: seq
                .decoder_model_info
                .frame_presentation_time_length as u32,
            frame_id_numbers_present_flag: s.frame_id_numbers_present_flag,
            frame_id_length: s.frame_id_length as u32,
            force_screen_content_tools: s.force_screen_content_tools,
            force_integer_mv: s.force_integer_mv,
            max_frame_width: s.max_frame_width,
            max_frame_height: s.max_frame_height,
            enable_order_hint: s.enable_order_hint,
            order_hint_bits_minus_1: s.order_hint_bits_minus_1,
            operating_points_cnt_minus_1: seq.operating_points_cnt_minus_1,
            operating_point_idc: seq.operating_point_idc,
            op_decoder_model_param_present: seq.op_decoder_model_param_present,
            buffer_removal_time_length: seq
                .decoder_model_info
                .buffer_removal_time_length as u32,
            ..Default::default()
        },
        frame_size: FrameSizeHeader {
            num_bits_width: s.num_bits_width,
            num_bits_height: s.num_bits_height,
            superres_upscaled_width: s.max_frame_width,
            superres_upscaled_height: s.max_frame_height,
            enable_superres: s.enable_superres,
            ..Default::default()
        },
        tile_info: TileInfoHeader {
            mi_cols,
            mi_rows,
            mib_size_log2,
            min_log2_cols,
            max_log2_cols,
            min_log2_rows: (min_log2_tiles - min_log2_cols).max(0),
            max_log2_rows,
            max_width_sb,
            max_height_sb: (max_tile_area_sb / max_width_sb.max(1)).max(1),
            ..Default::default()
        },
        num_planes: if c.monochrome { 1 } else { 3 },
        separate_uv_delta_q: c.separate_uv_delta_q,
        film_grain_params_present: seq.film_grain_params_present,
        cdef: aom_dsp::entropy::header::CdefHeader {
            enable_cdef: s.enable_cdef,
            ..Default::default()
        },
        restoration: aom_dsp::entropy::header::RestorationHeader {
            enable_restoration: s.enable_restoration,
            sb_size_128: s.sb_size_128,
            ..Default::default()
        },
        ..Default::default()
    };
    cfg.might_allow_ref_frame_mvs = s.enable_ref_frame_mvs && s.enable_order_hint;
    cfg.might_allow_warped_motion = s.enable_warped_motion;
    cfg
}

fn headers(path: &str) -> Vec<FrameHeaderObu> {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut out = Vec::new();
    let mut seq: Option<SequenceHeaderObu> = None;
    let mut pos = 0usize;
    while pos < data.len() {
        let h = read_obu_header(&data[pos..]).expect("obu header");
        let (size, size_len) = uleb_decode(&data[pos + h.header_len..]).expect("leb128");
        let body = pos + h.header_len + size_len;
        let payload = &data[body..body + size as usize];
        match h.obu_type {
            1 => {
                let mut rb = ReadBitBuffer::new(payload);
                seq = Some(read_sequence_header_obu(&mut rb));
            }
            3 | 6 | 7 => {
                let cfg = reader_cfg(seq.as_ref().expect("frame before SH"));
                let mut rb = ReadBitBuffer::new(payload);
                out.push(read_uncompressed_header(&mut rb, &cfg));
                eprintln!(
                    "frame {} header consumed {} bits; cdef_bits={}",
                    out.len() - 1,
                    rb.bit_position(),
                    out.last().map(|f| f.cdef.cdef_bits).unwrap_or(-1),
                );
            }
            _ => {}
        }
        pos = body + size as usize;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: hdr_diff <a.obu> <b.obu>");
        std::process::exit(2);
    }
    let a = headers(&args[1]);
    let b = headers(&args[2]);
    println!("{} frames vs {} frames", a.len(), b.len());
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        let xs = format!("{x:#?}");
        let ys = format!("{y:#?}");
        if std::env::var_os("HDR_DUMP").is_some() {
            let xl: Vec<&str> = xs.lines().collect();
            let yl: Vec<&str> = ys.lines().collect();
            for (l, (lx, ly)) in xl.iter().zip(yl.iter()).enumerate() {
                if lx != ly {
                    let lo = l.saturating_sub(6);
                    for (w, (wx, wy)) in
                        xl.iter().zip(yl.iter()).enumerate().skip(lo).take(13)
                    {
                        println!("    {w:4}: C={wx}  RUST={wy}");
                    }
                    println!("    ----");
                }
            }
            continue;
        }
        if std::env::var_os("HDR_FULL").is_some() {
            println!("frame {i} C:\n{xs}");
        }
        if xs == ys {
            println!("frame {i}: headers identical");
            continue;
        }
        println!("frame {i}: HEADER DIFFS");
        let xl: Vec<&str> = xs.lines().collect();
        let yl: Vec<&str> = ys.lines().collect();
        for (l, (lx, ly)) in xl.iter().zip(yl.iter()).enumerate() {
            if lx != ly {
                println!("  line {l}: C={lx}  RUST={ly}");
            }
        }
    }
}

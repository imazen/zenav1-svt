//! decode-diff: decode two raw AV1 OBU streams with the aom-rs decode
//! oracle (`aom-decode`, the Gate-1 byte-identical sibling port) and report
//! the first differing pixel + its superblock, in ENCODE (SB raster) order.
//!
//!   decode_diff <c.obu> <rs.obu> [sb_size (default 64)]
//!   decode_diff --vs-raw <stream.obu> <plane_prefix>
//!
//! `--vs-raw` compares the decode against `<prefix>.p{0,1,2}` raw
//! tightly-packed planes — oracle absolute validation against an
//! encoder-internal dump on a cell whose in-loop filters are no-ops (the
//! decoded output is post-all-filters; with nonzero LF/CDEF/LR expect
//! bounded filter deltas, not equality).
//!
//! Exit codes: 0 = identical; 1 = differ (details on stdout); 2 = decode
//! error. The oracle fails LOUDLY (Result) on anything it cannot decode —
//! unlike the retired aom-decoder-rs backend, which silently fabricated
//! gray frames on Wiener-active streams (#92).
//!
//! Output on difference (machine-parseable, one line each):
//!   DIFF plane=<p> x=<x> y=<y> c=<val> r=<val>       (first in SB order)
//!   SB mi_row=<r> mi_col=<c>                          (owning SB root mi)
//!   NDIFF plane0=<count> ...                          (per-plane totals)

use aom_decode::frame::{decode_frame_obus, FrameDecode};

fn decode(path: &str) -> FrameDecode {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(2);
    });
    match decode_frame_obus(&data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path}: decode error: {e}");
            std::process::exit(2);
        }
    }
}

/// (plane, data, width, height) triples — tight strides (stride == width).
fn planes(f: &FrameDecode) -> Vec<(usize, &[u16], usize, usize)> {
    let mut v = vec![(0usize, f.y.as_slice(), f.width, f.height)];
    if !f.monochrome {
        v.push((1, f.u.as_slice(), f.width_uv, f.height_uv));
        v.push((2, f.v.as_slice(), f.width_uv, f.height_uv));
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: decode_diff <c.obu> <rs.obu> [sb_size]");
        eprintln!("       decode_diff --vs-raw <stream.obu> <plane_prefix>");
        std::process::exit(2);
    }

    // --ibc-debug <stream.obu> <plane_prefix>: prefilter-decode, list every
    // IntraBC block (mi position, bsize, DV, skip, tx_size), and walk the
    // per-block decode records IN DECODE ORDER to report the FIRST block
    // whose luma footprint contains a decoded-vs-raw diff (the encoder's
    // internal recon). The first corrupt block in coding order is where the
    // pack-vs-search desync STARTS (later diffs are usually cascade).
    if args[1] == "--ibc-debug" {
        let data = std::fs::read(&args[2]).unwrap_or_else(|e| {
            eprintln!("read {}: {e}", args[2]);
            std::process::exit(2);
        });
        let (td, _cfg, _fh) = aom_decode::frame::decode_frame_obus_prefilter(&data)
            .unwrap_or_else(|e| {
                eprintln!("{}: prefilter decode error: {e}", args[2]);
                std::process::exit(2);
            });
        if let Some(c) = &td.corrupt {
            println!("DECODER-CORRUPT: {c}");
        }
        // "-" prefix: census-only mode (no raw planes to correlate).
        let raw_y = if args[3] == "-" {
            Vec::new()
        } else {
            std::fs::read(format!("{}.p0", args[3])).expect("raw p0")
        };
        // Per-block IBC census in decode order.
        let mut n_ibc = 0usize;
        for b in &td.blocks {
            if b.info.use_intrabc != 0 {
                n_ibc += 1;
                println!(
                    "IBC mi=({},{}) bsize={} dv=({},{}) skip={} tx_size={} txbs(eob,tt)={:?} uv={:?}",
                    b.mi_row, b.mi_col, b.bsize, b.info.dv_row, b.info.dv_col,
                    b.info.skip, b.tx_size, b.txbs, b.txbs_uv
                );
            }
        }
        println!("IBC-TOTAL {n_ibc} of {} blocks", td.blocks.len());
        // Block dims from the C BlockSize index.
        const BDIM: [(usize, usize); 22] = [
            (4, 4), (4, 8), (8, 4), (8, 8), (8, 16), (16, 8), (16, 16),
            (16, 32), (32, 16), (32, 32), (32, 64), (64, 32), (64, 64),
            (64, 128), (128, 64), (128, 128), (4, 16), (16, 4), (8, 32),
            (32, 8), (16, 64), (64, 16),
        ];
        let mut first_bad: Option<usize> = None;
        let mut n_bad = 0usize;
        if raw_y.is_empty() {
            std::process::exit(0);
        }
        for (bi, b) in td.blocks.iter().enumerate() {
            let (bw, bh) = BDIM[b.bsize];
            let x0 = (b.mi_col as usize) * 4;
            let y0 = (b.mi_row as usize) * 4;
            let mut bad = false;
            'scan: for y in y0..(y0 + bh).min(td.height) {
                for x in x0..(x0 + bw).min(td.width) {
                    if td.recon.px(y * td.stride + x) != u16::from(raw_y[y * td.width + x]) {
                        bad = true;
                        break 'scan;
                    }
                }
            }
            if bad {
                n_bad += 1;
                if first_bad.is_none() {
                    first_bad = Some(bi);
                    println!(
                        "FIRST-BAD block #{bi}: mi=({},{}) {}x{} use_intrabc={} dv=({},{}) \
                         y_mode={} skip={} tx_size={} ntxb={} palette={} fi={}",
                        b.mi_row, b.mi_col, bw, bh, b.info.use_intrabc,
                        b.info.dv_row, b.info.dv_col, b.info.y_mode, b.info.skip,
                        b.tx_size, b.txbs.len(), b.info.palette_size[0],
                        b.info.use_filter_intra
                    );
                    // context: dump the preceding 4 blocks
                    for pi in bi.saturating_sub(4)..bi {
                        let pb = &td.blocks[pi];
                        let (pw, ph) = BDIM[pb.bsize];
                        println!(
                            "  prev #{pi}: mi=({},{}) {}x{} ibc={} dv=({},{}) mode={} skip={}",
                            pb.mi_row, pb.mi_col, pw, ph, pb.info.use_intrabc,
                            pb.info.dv_row, pb.info.dv_col, pb.info.y_mode, pb.info.skip
                        );
                    }
                }
            }
        }
        println!("BAD-LUMA-BLOCKS {n_bad} of {}", td.blocks.len());
        std::process::exit(if n_bad > 0 { 1 } else { 0 });
    }

    // --first-block-diff <a.obu> <b.obu>: prefilter-decode both streams and
    // report the FIRST per-block decode record (in decode order) where the
    // two disagree — position/bsize (tree divergence) or mode info
    // (mode/DV/skip/tx divergence at the same block). The chunk-10
    // localization tool: classifies WHERE the encoders' decisions split.
    if args[1] == "--first-block-diff" {
        let mut tds = Vec::new();
        for p in [&args[2], &args[3]] {
            let data = std::fs::read(p).unwrap_or_else(|e| {
                eprintln!("read {p}: {e}");
                std::process::exit(2);
            });
            let (td, _cfg, _fh) = aom_decode::frame::decode_frame_obus_prefilter(&data)
                .unwrap_or_else(|e| {
                    eprintln!("{p}: prefilter decode error: {e}");
                    std::process::exit(2);
                });
            tds.push(td);
        }
        let (a, b) = (&tds[0], &tds[1]);
        let n = a.blocks.len().min(b.blocks.len());
        let mut diverged = false;
        for i in 0..n {
            let (ba, bb) = (&a.blocks[i], &b.blocks[i]);
            let pos_diff = ba.mi_row != bb.mi_row || ba.mi_col != bb.mi_col || ba.bsize != bb.bsize;
            let ia = &ba.info;
            let ib = &bb.info;
            let mode_diff = ia.use_intrabc != ib.use_intrabc
                || ia.dv_row != ib.dv_row
                || ia.dv_col != ib.dv_col
                || ia.y_mode != ib.y_mode
                || ia.uv_mode != ib.uv_mode
                || ia.skip != ib.skip
                || ia.angle_delta_y != ib.angle_delta_y
                || ia.palette_size != ib.palette_size
                || ia.use_filter_intra != ib.use_filter_intra
                || ba.tx_size != bb.tx_size
                || ba.txbs != bb.txbs;
            if pos_diff || mode_diff {
                let kind = if pos_diff { "TREE" } else { "MODE" };
                println!(
                    "FIRST-BLOCK-DIFF #{i} kind={kind}\n  A mi=({},{}) bsize={} ibc={} dv=({},{}) mode={} uv={} skip={} pal={} fi={} tx={} txbs={:?}\n  B mi=({},{}) bsize={} ibc={} dv=({},{}) mode={} uv={} skip={} pal={} fi={} tx={} txbs={:?}",
                    ba.mi_row, ba.mi_col, ba.bsize, ia.use_intrabc, ia.dv_row, ia.dv_col,
                    ia.y_mode, ia.uv_mode, ia.skip, ia.palette_size[0], ia.use_filter_intra,
                    ba.tx_size, ba.txbs,
                    bb.mi_row, bb.mi_col, bb.bsize, ib.use_intrabc, ib.dv_row, ib.dv_col,
                    ib.y_mode, ib.uv_mode, ib.skip, ib.palette_size[0], ib.use_filter_intra,
                    bb.tx_size, bb.txbs,
                );
                diverged = true;
                break;
            }
        }
        if !diverged {
            if a.blocks.len() != b.blocks.len() {
                println!(
                    "BLOCK-COUNT-DIFF a={} b={} (first {n} identical)",
                    a.blocks.len(),
                    b.blocks.len()
                );
                diverged = true;
            } else {
                println!("BLOCKS-IDENTICAL {n}");
            }
        }
        std::process::exit(if diverged { 1 } else { 0 });
    }

    // --vs-raw-prefilter: decode to the PRE-FILTER reconstruction
    // (aom-decode's decode_frame_obus_prefilter) and compare against the
    // encoder's pre-DLF dump — an EXACT self-consistency check at every
    // preset: any mismatch proves the encoder's internal recon desynced
    // from its own coded stream (the pack-vs-search class), with no
    // filter-delta ambiguity.
    if args[1] == "--vs-raw-prefilter" {
        let data = std::fs::read(&args[2]).unwrap_or_else(|e| {
            eprintln!("read {}: {e}", args[2]);
            std::process::exit(2);
        });
        let (td, _cfg, _fh) = aom_decode::frame::decode_frame_obus_prefilter(&data)
            .unwrap_or_else(|e| {
                eprintln!("{}: prefilter decode error: {e}", args[2]);
                std::process::exit(2);
            });
        // KfTileDecode carries the (possibly mi-aligned) dims + strides.
        let mut bad = false;
        let plist: [(usize, &aom_decode::plane::ReconPlane, usize, usize, usize); 3] = [
            (0, &td.recon, td.width, td.height, td.stride),
            (1, &td.recon_u, td.width_uv, td.height_uv, td.stride_uv),
            (2, &td.recon_v, td.width_uv, td.height_uv, td.stride_uv),
        ];
        for (p, dp, pw, ph, st) in plist {
            if dp.is_empty() {
                continue;
            }
            let raw = match std::fs::read(format!("{}.p{p}", args[3])) {
                Ok(r) => r,
                Err(e) => {
                    println!("plane{p}: raw read failed: {e}");
                    bad = true;
                    continue;
                }
            };
            if raw.len() != pw * ph {
                println!("plane{p}: raw size {} != {}x{}", raw.len(), pw, ph);
                bad = true;
                continue;
            }
            let mut first = None;
            let mut n = 0u64;
            for y in 0..ph {
                for x in 0..pw {
                    if dp.px(y * st + x) != raw[y * pw + x] as u16 {
                        n += 1;
                        if first.is_none() {
                            first = Some((x, y, raw[y * pw + x], dp.px(y * st + x) as u8));
                        }
                    }
                }
            }
            match first {
                None => println!("plane{p}: prefilter decode == raw ({pw}x{ph})"),
                Some((x, y, r, d)) => {
                    println!(
                        "plane{p}: {n} SELF-DESYNC diffs, first at ({x},{y}) raw={r} decoded={d}"
                    );
                    bad = true;
                }
            }
        }
        std::process::exit(if bad { 1 } else { 0 });
    }

    // --r10 <obu> <recon10_le_u16_file>: decode <obu> PREFILTER and compare
    // its LUMA recon (u16) against a raw u16-LE file (the bd10 encoder's
    // internal recon10 dump). Used to (a) self-consistency check the encoder's
    // recon10 vs decode(own OBU), and (b) compare recon10 vs decode(C OBU).
    // Also `--r10 <obuA> --vs <obuB>` diffs two prefilter LUMA recons directly.
    if args[1] == "--r10" {
        let da = std::fs::read(&args[2]).unwrap();
        let (ta, _, _) = aom_decode::frame::decode_frame_obus_prefilter(&da).unwrap_or_else(|e| {
            eprintln!("{}: {e}", args[2]);
            std::process::exit(2);
        });
        let (pw, ph, st) = (ta.width, ta.height, ta.stride);
        let ref_y: Vec<u16> = if args.get(3).map(|s| s.as_str()) == Some("--vs") {
            let db = std::fs::read(&args[4]).unwrap();
            let (tb, _, _) =
                aom_decode::frame::decode_frame_obus_prefilter(&db).unwrap_or_else(|e| {
                    eprintln!("{}: {e}", args[4]);
                    std::process::exit(2);
                });
            let mut v = vec![0u16; pw * ph];
            for y in 0..ph {
                for x in 0..pw {
                    v[y * pw + x] = tb.recon.px(y * tb.stride + x);
                }
            }
            v
        } else {
            let raw = std::fs::read(&args[3]).unwrap();
            raw.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect()
        };
        let mut n = 0u64;
        let mut first = None;
        for y in 0..ph {
            for x in 0..pw {
                let d = ta.recon.px(y * st + x);
                let r = ref_y[y * pw + x];
                if d != r {
                    n += 1;
                    if first.is_none() {
                        first = Some((x, y, r, d));
                    }
                }
            }
        }
        match first {
            None => println!("prefilter luma == ref ({pw}x{ph})"),
            Some((x, y, r, d)) => {
                println!("{n} luma diffs, first at ({x},{y}) ref={r} decoded={d}")
            }
        }
        std::process::exit(if n == 0 { 0 } else { 1 });
    }

    // --blocks <c.obu> <rs.obu> [mi_row,mi_col]: diff the DECODER'S OWN
    // per-block records (DecodedBlockKf — stream truth, bypassing every
    // encoder-side dump-fidelity question). Without a mi filter: prints
    // the first N differing block records; with one: both sides' records
    // for that mi (2 lines).
    if args[1] == "--blocks" {
        let da = std::fs::read(&args[2]).unwrap();
        let db = std::fs::read(&args[3]).unwrap();
        let (ta, _, _) = aom_decode::frame::decode_frame_obus_prefilter(&da).unwrap_or_else(|e| {
            eprintln!("{}: {e}", args[2]);
            std::process::exit(2);
        });
        let (tb, _, _) = aom_decode::frame::decode_frame_obus_prefilter(&db).unwrap_or_else(|e| {
            eprintln!("{}: {e}", args[3]);
            std::process::exit(2);
        });
        let fmt = |b: &aom_decode::DecodedBlockKf| {
            format!(
                "bsize={} part={} tx_size={} txbs={:?} txbs_uv={:?} info={:?}",
                b.bsize, b.partition, b.tx_size, b.txbs, b.txbs_uv, b.info
            )
        };
        if let Some(at) = args.get(4) {
            let rc: Vec<i32> = at.split(',').filter_map(|s| s.parse().ok()).collect();
            let fa = ta.blocks.iter().find(|b| b.mi_row == rc[0] && b.mi_col == rc[1]);
            let fb = tb.blocks.iter().find(|b| b.mi_row == rc[0] && b.mi_col == rc[1]);
            println!("C    ({},{}): {}", rc[0], rc[1], fa.map(&fmt).unwrap_or("ABSENT".into()));
            println!("port ({},{}): {}", rc[0], rc[1], fb.map(&fmt).unwrap_or("ABSENT".into()));
            std::process::exit(0);
        }
        let mut shown = 0;
        for (i, (a, b)) in ta.blocks.iter().zip(tb.blocks.iter()).enumerate() {
            let (sa, sb) = (fmt(a), fmt(b));
            if a.mi_row != b.mi_row || a.mi_col != b.mi_col || sa != sb {
                println!("BLOCK[{i}] C    mi=({},{}) {}", a.mi_row, a.mi_col, sa);
                println!("BLOCK[{i}] port mi=({},{}) {}", b.mi_row, b.mi_col, sb);
                shown += 1;
                if shown >= 4 {
                    break;
                }
            }
        }
        if shown == 0 {
            println!(
                "all {} vs {} block records identical (in lockstep order)",
                ta.blocks.len(),
                tb.blocks.len()
            );
        }
        std::process::exit(if shown == 0 { 0 } else { 1 });
    }

    if args[1] == "--vs-raw" {
        let f = decode(&args[2]);
        let mut bad = false;
        for (p, dp, pw, ph) in planes(&f) {
            let raw = match std::fs::read(format!("{}.p{p}", args[3])) {
                Ok(r) => r,
                Err(e) => {
                    println!("plane{p}: raw read failed: {e}");
                    bad = true;
                    continue;
                }
            };
            if raw.len() != pw * ph {
                println!("plane{p}: raw size {} != {}x{}", raw.len(), pw, ph);
                bad = true;
                continue;
            }
            let mut first = None;
            let mut n = 0u64;
            for i in 0..pw * ph {
                if dp[i] != raw[i] as u16 {
                    n += 1;
                    if first.is_none() {
                        first = Some((i % pw, i / pw, raw[i], dp[i]));
                    }
                }
            }
            match first {
                None => println!("plane{p}: decode == raw ({pw}x{ph})"),
                Some((x, y, r, d)) => {
                    println!("plane{p}: {n} diffs, first at ({x},{y}) raw={r} decoded={d}");
                    bad = true;
                }
            }
        }
        std::process::exit(if bad { 1 } else { 0 });
    }

    // --prefilter <c.obu> <rs.obu>: pairwise diff of the PRE-FILTER
    // reconstructions — the pure causal locator (a pre-filter pixel diff
    // can only come from the coded symbols of its own block + prediction
    // ancestry; no DLF/CDEF/LR cascades).
    if args[1] == "--prefilter" {
        let da = std::fs::read(&args[2]).unwrap();
        let db = std::fs::read(&args[3]).unwrap();
        let (ta, _, _) = aom_decode::frame::decode_frame_obus_prefilter(&da).unwrap_or_else(|e| {
            eprintln!("{}: {e}", args[2]);
            std::process::exit(2);
        });
        let (tb, _, _) = aom_decode::frame::decode_frame_obus_prefilter(&db).unwrap_or_else(|e| {
            eprintln!("{}: {e}", args[3]);
            std::process::exit(2);
        });
        let plist = [
            (0usize, &ta.recon, &tb.recon, ta.width, ta.height, ta.stride, tb.stride),
            (1, &ta.recon_u, &tb.recon_u, ta.width_uv, ta.height_uv, ta.stride_uv, tb.stride_uv),
            (2, &ta.recon_v, &tb.recon_v, ta.width_uv, ta.height_uv, ta.stride_uv, tb.stride_uv),
        ];
        let mut any = false;
        for (p, ca, cb, pw, ph, sa, sbst) in plist {
            if ca.is_empty() {
                continue;
            }
            let mut first = None;
            let mut n = 0u64;
            for y in 0..ph {
                for x in 0..pw {
                    if ca.px(y * sa + x) != cb.px(y * sbst + x) {
                        n += 1;
                        if first.is_none() {
                            first = Some((x, y, ca.px(y * sa + x), cb.px(y * sbst + x)));
                        }
                    }
                }
            }
            if let Some((x, y, a, b)) = first {
                println!("PREFILTER plane={p} first=({x},{y}) c={a} r={b} ndiff={n}");
                any = true;
            } else {
                println!("PREFILTER plane={p} identical");
            }
        }
        std::process::exit(if any { 1 } else { 0 });
    }

    let sb: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    let c = decode(&args[1]);
    let r = decode(&args[2]);
    if (c.width, c.height, c.bit_depth, c.monochrome)
        != (r.width, r.height, r.bit_depth, r.monochrome)
    {
        println!(
            "DIMS c={}x{}@{} r={}x{}@{}",
            c.width, c.height, c.bit_depth, r.width, r.height, r.bit_depth
        );
        std::process::exit(1);
    }
    let w = c.width;
    let h = c.height;
    let cp = planes(&c);
    let rp = planes(&r);

    let mut ndiff = [0u64; 3];
    for ((p, cd, pw, ph), (_, rd, _, _)) in cp.iter().zip(rp.iter()) {
        for i in 0..pw * ph {
            if cd[i] != rd[i] {
                ndiff[*p] += 1;
            }
        }
    }
    if ndiff.iter().all(|&n| n == 0) {
        println!("IDENTICAL decoded output ({w}x{h})");
        std::process::exit(0);
    }

    // First differing pixel in SB (encode) order: iterate SB rows/cols,
    // luma then chroma within each SB — the CAUSAL first divergence, not
    // the pixel-raster one (pixel raster points at downstream cascades).
    'sb_scan: for sb_y in (0..h).step_by(sb) {
        for sb_x in (0..w).step_by(sb) {
            for ((p, cd, pw, ph), (_, rd, _, _)) in cp.iter().zip(rp.iter()) {
                let (bx, by, bw, bh) = if *p == 0 {
                    (sb_x, sb_y, sb, sb)
                } else {
                    (
                        sb_x >> c.subsampling_x,
                        sb_y >> c.subsampling_y,
                        sb >> c.subsampling_x,
                        sb >> c.subsampling_y,
                    )
                };
                for y in by..(by + bh).min(*ph) {
                    for x in bx..(bx + bw).min(*pw) {
                        if cd[y * pw + x] != rd[y * pw + x] {
                            println!(
                                "DIFF plane={p} x={x} y={y} c={} r={}",
                                cd[y * pw + x],
                                rd[y * pw + x]
                            );
                            println!("SB mi_row={} mi_col={}", sb_y / 4, sb_x / 4);
                            break 'sb_scan;
                        }
                    }
                }
            }
        }
    }
    println!(
        "NDIFF plane0={} plane1={} plane2={}",
        ndiff[0], ndiff[1], ndiff[2]
    );
    std::process::exit(1);
}

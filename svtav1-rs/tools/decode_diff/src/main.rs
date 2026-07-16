//! decode-diff: decode two raw AV1 OBU streams with the bit-exact
//! aom-decoder-rs oracle and report the first differing pixel + its
//! superblock, in ENCODE (SB raster) order.
//!
//!   decode_diff <c.obu> <rs.obu> [sb_size (default 64)]
//!
//! Exit codes: 0 = decoded outputs identical; 1 = differ (details on
//! stdout); 2 = decode error.
//!
//! Output on difference (machine-parseable, one line each):
//!   DIFF plane=<p> x=<x> y=<y> c=<val> r=<val>       (first in SB order)
//!   SB mi_row=<r> mi_col=<c>                          (owning SB root mi)
//!   NDIFF plane<p>=<count> ...                        (per-plane totals)

use aom_decoder_rs::{Decoder, Settings};

fn decode(path: &str) -> aom_decoder_rs::DecodedFrame {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(2);
    });
    let mut dec = Decoder::new(Settings::default());
    match dec.decode(&data) {
        Ok(Some(f)) => f,
        Ok(None) => {
            eprintln!("{path}: no frame decoded");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("{path}: decode error: {e:?}");
            std::process::exit(2);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: decode_diff <c.obu> <rs.obu> [sb_size]");
        eprintln!("       decode_diff --vs-raw <stream.obu> <plane_prefix>");
        eprintln!("         (compares the decode against <prefix>.p{{0,1,2}} raw");
        eprintln!("          tightly-packed planes — ORACLE ABSOLUTE VALIDATION");
        eprintln!("          against an encoder-internal dump on a cell whose");
        eprintln!("          in-loop filters are no-ops)");
        std::process::exit(2);
    }
    if args[1] == "--vs-raw" {
        let f = decode(&args[2]);
        let w = f.width as usize;
        let h = f.height as usize;
        let cw = w >> f.subsampling_x;
        let ch = h >> f.subsampling_y;
        let planes: [(&[u16], usize, usize, usize); 3] = [
            (&f.y_plane, w, h, f.y_stride),
            (&f.cb_plane, cw, ch, f.c_stride),
            (&f.cr_plane, cw, ch, f.c_stride),
        ];
        let mut bad = false;
        for (p, (dp, pw, ph, st)) in planes.iter().enumerate() {
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
            for y in 0..*ph {
                for x in 0..*pw {
                    if dp[y * st + x] != raw[y * pw + x] as u16 {
                        n += 1;
                        if first.is_none() {
                            first = Some((x, y, raw[y * pw + x], dp[y * st + x]));
                        }
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
    let sb: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    let c = decode(&args[1]);
    let r = decode(&args[2]);
    if (c.width, c.height, c.bit_depth) != (r.width, r.height, r.bit_depth) {
        println!(
            "DIMS c={}x{}@{} r={}x{}@{}",
            c.width, c.height, c.bit_depth, r.width, r.height, r.bit_depth
        );
        std::process::exit(1);
    }
    let w = c.width as usize;
    let h = c.height as usize;
    let cw = w >> c.subsampling_x;
    let ch = h >> c.subsampling_y;

    // Per-plane diff counts + the first differing pixel in SB (encode)
    // order: iterate SB rows/cols, luma first then chroma within each SB —
    // the CAUSAL first divergence, not the pixel-raster one (a lesson from
    // the earlier drill: pixel raster order points at downstream cascades).
    let planes: [(&[u16], &[u16], usize, usize, usize, usize); 3] = [
        (&c.y_plane, &r.y_plane, w, h, c.y_stride, r.y_stride),
        (&c.cb_plane, &r.cb_plane, cw, ch, c.c_stride, r.c_stride),
        (&c.cr_plane, &r.cr_plane, cw, ch, c.c_stride, r.c_stride),
    ];
    // ORACLE GUARD: aom-decoder-rs (as of 2026-07-16) silently mis-decodes
    // streams with ACTIVE Wiener restoration units — the tile desyncs and
    // the luma plane comes back overwhelmingly 128-gray from the origin
    // (flat/none-LR SVT streams decode bit-exactly; every Wiener-active
    // stream tested decodes gray). Refuse to report a locate when either
    // side shows that signature: a false locate costs a drilling session.
    for (side, (yp, st)) in [("C", (&c.y_plane, c.y_stride)), ("port", (&r.y_plane, r.y_stride))] {
        let mut gray = 0u64;
        for y in 0..h {
            for x in 0..w {
                gray += u64::from(yp[y * st + x] == 128);
            }
        }
        if gray * 100 > (w as u64 * h as u64) * 40 {
            println!(
                "ORACLE-SUSPECT side={side}: {}% of luma decoded exactly 128 — known \
                 aom-decoder-rs failure on Wiener-active streams; DO NOT trust this \
                 locate (use tree_diff / the recon dumps instead)",
                gray * 100 / (w as u64 * h as u64)
            );
            std::process::exit(3);
        }
    }
    let mut ndiff = [0u64; 3];
    for (p, (cp, rp, pw, ph, cs, rs)) in planes.iter().enumerate() {
        for y in 0..*ph {
            for x in 0..*pw {
                if cp[y * cs + x] != rp[y * rs + x] {
                    ndiff[p] += 1;
                }
            }
        }
    }
    if ndiff.iter().all(|&n| n == 0) {
        println!("IDENTICAL decoded output ({w}x{h})");
        std::process::exit(0);
    }

    'sb_scan: for sb_y in (0..h).step_by(sb) {
        for sb_x in (0..w).step_by(sb) {
            // Luma block of this SB, then its chroma blocks — the order the
            // encoder codes them (interleaved per block, but SB granularity
            // is enough to identify the first divergent SB).
            for (p, (cp, rp, pw, ph, cs, rs)) in planes.iter().enumerate() {
                let (bx, by, bw, bh) = if p == 0 {
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
                        if cp[y * cs + x] != rp[y * rs + x] {
                            println!(
                                "DIFF plane={p} x={x} y={y} c={} r={}",
                                cp[y * cs + x],
                                rp[y * rs + x]
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

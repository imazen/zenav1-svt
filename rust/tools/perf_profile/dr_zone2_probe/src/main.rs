#![forbid(unsafe_code)]
use archmage::prelude::*;
use std::hint::black_box;
use std::time::Instant;
use svtav1_dsp::intra_pred as ip;
type Kernel = fn(X64V3Token, &mut [u8], usize, &[u8], &[u8], usize, bool, bool, usize, usize, i32);
fn baseline(
    _: X64V3Token,
    d: &mut [u8],
    s: usize,
    a: &[u8],
    l: &[u8],
    o: usize,
    ua: bool,
    ul: bool,
    w: usize,
    h: usize,
    angle: i32,
) {
    ip::dr_predictor_edged(d, s, a, l, o, ua, ul, w, h, angle);
}
fn main() {
    let token = X64V3Token::summon().expect("native AVX2 probe");
    let mut seed = 20260906u64;
    let mut a = [0u8; 160];
    let mut l = [0u8; 160];
    for i in 0..160 {
        a[i] = (i * 31 + 79) as u8;
        l[i] = (i * 53 + 19) as u8;
    }
    l[15] = a[15];
    let kernels: [(&str, Kernel); 2] = [("baseline", baseline), ("row_split", dr_probe::row_split)];
    println!("size\tangle\tround\torder\tkernel\tns\tcalls\tchecksum");
    for w in [4usize, 8, 16, 32, 64] {
        for angle in [104, 113, 135, 157, 166] {
            let ua = ip::use_intra_edge_upsample(w as i32, w as i32, angle - 90, 0);
            let ul = ip::use_intra_edge_upsample(w as i32, w as i32, angle - 180, 0);
            let mut d = vec![0u8; w * w];
            let calls = (1_048_576 / (w * w)).clamp(256, 8192);
            for round in 0..22 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let first = seed as usize & 1;
                for order in 0..2 {
                    let (name, kernel) = kernels[first ^ order];
                    let start = Instant::now();
                    for _ in 0..calls {
                        kernel(
                            token,
                            black_box(&mut d),
                            w,
                            black_box(&a),
                            black_box(&l),
                            16,
                            ua,
                            ul,
                            w,
                            w,
                            angle,
                        );
                    }
                    let ns = start.elapsed().as_nanos();
                    let checksum = black_box(d.iter().map(|&x| u64::from(x)).sum::<u64>());
                    if round > 0 {
                        println!(
                            "{w}\t{angle}\t{round}\t{order}\t{name}\t{ns}\t{calls}\t{checksum}"
                        );
                    }
                }
            }
        }
    }
}

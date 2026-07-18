//! Differential parity: the HIGH-BIT-DEPTH deblocking loop-filter kernels vs
//! the C reference (`svt_aom_highbd_lpf_*_c`) at bd 10 and 12.
//!
//! The bd8 twin (`c_parity_lpf.rs`) pins the 8-bit kernels; this pins the
//! `hbd::lpf_*_hbd` family, which is what the 10-bit decoder/encoder runs on
//! every transform edge. The risky delta these kernels carry vs bd8 is the
//! `<< (bd - 8)` threshold shift inside every mask/hev helper
//! (`highbd_filter_mask2`, `highbd_hev_mask`, etc.) and the wider
//! `signed_char_clamp_high` range — a single wrong shift diverges recon
//! frame-wide at 10-bit. Fuzzes the full (level, sharpness) space the frame
//! header can derive, over edge / flat / random content, at both bit depths.

use svtav1_cref as cref;
use svtav1_dsp::hbd;
use svtav1_dsp::loop_filter as lf;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const SIZE: usize = 16;

/// (kind, our hbd kernel fn, edge offset in a 16x16 buffer).
type OurKernel = fn(&mut [u16], usize, usize, lf::LfThresh, u8);

fn kernels() -> [(cref::LpfKind, OurKernel, usize); 8] {
    let v_off = 2 * SIZE + 8; // vertical: edge between cols 7|8, rows 2..6
    let h_off = 8 * SIZE + 3; // horizontal: edge between rows 7|8, cols 3..7
    [
        (cref::LpfKind::H4, hbd::lpf_horizontal_4_hbd as OurKernel, h_off),
        (cref::LpfKind::V4, hbd::lpf_vertical_4_hbd as OurKernel, v_off),
        (cref::LpfKind::H6, hbd::lpf_horizontal_6_hbd as OurKernel, h_off),
        (cref::LpfKind::V6, hbd::lpf_vertical_6_hbd as OurKernel, v_off),
        (cref::LpfKind::H8, hbd::lpf_horizontal_8_hbd as OurKernel, h_off),
        (cref::LpfKind::V8, hbd::lpf_vertical_8_hbd as OurKernel, v_off),
        (cref::LpfKind::H14, hbd::lpf_horizontal_14_hbd as OurKernel, h_off),
        (cref::LpfKind::V14, hbd::lpf_vertical_14_hbd as OurKernel, v_off),
    ]
}

/// Content at `bd` bits: flat+noise (flat branch), step edge (outer-edge/hev
/// branches), or full-range random (mask-off). Pixel values span the full
/// `0..(1<<bd)` range so the wider thresholds are actually exercised.
fn fill(content: u32, rng: &mut Rng, vertical: bool, buf: &mut [u16], bd: u8) {
    let maxv = (1u32 << bd) - 1;
    match content {
        0 => {
            let base = (rng.range(maxv as u64 + 1)) as i32;
            let amp = rng.range(6) as i32;
            for px in buf.iter_mut() {
                let n = if amp == 0 { 0 } else { rng.range(2 * amp as u64 + 1) as i32 - amp };
                *px = (base + n).clamp(0, maxv as i32) as u16;
            }
        }
        1 => {
            let a = rng.range(maxv as u64 + 1) as i32;
            let b = rng.range(maxv as u64 + 1) as i32;
            let amp = rng.range(4) as i32;
            for r in 0..SIZE {
                for c in 0..SIZE {
                    let coord = if vertical { c } else { r };
                    let base = if coord < 8 { a } else { b };
                    let n = if amp == 0 { 0 } else { rng.range(2 * amp as u64 + 1) as i32 - amp };
                    buf[r * SIZE + c] = (base + n).clamp(0, maxv as i32) as u16;
                }
            }
        }
        _ => {
            for px in buf.iter_mut() {
                *px = (rng.next() as u32 & maxv) as u16;
            }
        }
    }
}

#[test]
fn hbd_lpf_kernels_match_c_over_level_sharpness_space() {
    let mut rng = Rng(0x108D_1F17_2026_0718);
    for bd in [10u8, 12] {
        for (kind, ours, off) in kernels() {
            let (_, vertical) = kind.geometry();
            for sharpness in [0u8, 1, 4, 7] {
                for level in 0..=63u8 {
                    let t = lf::lf_thresholds(level, sharpness);
                    for content in 0..3u32 {
                        let mut buf = vec![0u16; SIZE * SIZE];
                        fill(content, &mut rng, vertical, &mut buf, bd);
                        let mut c_buf = buf.clone();

                        ours(&mut buf, off, SIZE, t, bd);
                        cref::lpf_hbd(kind, &mut c_buf, off, SIZE, t.mblim, t.lim, t.hev_thr, bd as i32);

                        if buf != c_buf {
                            let i = buf.iter().zip(c_buf.iter()).position(|(a, b)| a != b).unwrap();
                            panic!(
                                "{kind:?} bd{bd} level {level} sharp {sharpness} content {content}: \
                                 first diff at (r{} c{}): ours={} c={}",
                                i / SIZE,
                                i % SIZE,
                                buf[i],
                                c_buf[i]
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Robustness: arbitrary (mblim, lim, hev) triples over random content at both
/// bit depths — the kernels must still agree bit-for-bit.
#[test]
fn hbd_lpf_kernels_match_c_random_params() {
    let mut rng = Rng(0x5EED_108D_7E57_0002);
    for bd in [10u8, 12] {
        for (kind, ours, off) in kernels() {
            let (_, vertical) = kind.geometry();
            for _ in 0..200 {
                let t = lf::LfThresh {
                    mblim: (rng.next() >> 32) as u8,
                    lim: (rng.next() >> 32) as u8,
                    hev_thr: (rng.next() >> 32) as u8,
                };
                let content = rng.range(3) as u32;
                let mut buf = vec![0u16; SIZE * SIZE];
                fill(content, &mut rng, vertical, &mut buf, bd);
                let mut c_buf = buf.clone();

                ours(&mut buf, off, SIZE, t, bd);
                cref::lpf_hbd(kind, &mut c_buf, off, SIZE, t.mblim, t.lim, t.hev_thr, bd as i32);
                assert_eq!(buf, c_buf, "{kind:?} bd{bd} random-params divergence");
            }
        }
    }
}

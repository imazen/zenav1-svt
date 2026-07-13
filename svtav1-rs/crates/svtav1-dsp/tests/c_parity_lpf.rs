//! Differential parity: deblocking loop-filter kernels + threshold tables
//! vs the C reference (`svt_aom_lpf_*_c`, `svt_aom_update_sharpness`).
//!
//! The kernels are what the DECODER runs on every transform edge; a single
//! bit of divergence breaks recon parity frame-wide. Fuzzes the full
//! (level, sharpness) parameter space the decoder can derive from a frame
//! header, over edge-shaped, smooth, and random content.

use svtav1_cref as cref;
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
    fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
    fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Thresholds must match C for every (sharpness, level) pair. hev_thr is
/// `level >> 4` by construction on both sides (svt_av1_loop_filter_init /
/// libaom av1_loop_filter_init), so lim/mblim are the differential part.
#[test]
fn lf_thresholds_match_c() {
    for sharpness in 0..=7u8 {
        let (c_lim, c_mblim) = cref::lf_limits(sharpness);
        for level in 0..=63u8 {
            let ours = lf::lf_thresholds(level, sharpness);
            assert_eq!(
                (ours.lim, ours.mblim),
                (c_lim[level as usize], c_mblim[level as usize]),
                "lim/mblim diverge at level {level} sharpness {sharpness}"
            );
            assert_eq!(ours.hev_thr, level >> 4);
        }
    }
}

const SIZE: usize = 16;

/// (kind, our kernel fn, vertical?, edge offset in a 16x16 buffer)
type OurKernel = fn(&mut [u8], usize, usize, lf::LfThresh);

fn kernels() -> [(cref::LpfKind, OurKernel, usize); 8] {
    // Vertical kernels: edge between columns 7|8, lines = rows 2..6.
    let v_off = 2 * SIZE + 8;
    // Horizontal kernels: edge between rows 7|8, lines = columns 3..7.
    let h_off = 8 * SIZE + 3;
    [
        (cref::LpfKind::H4, lf::lpf_horizontal_4 as OurKernel, h_off),
        (cref::LpfKind::V4, lf::lpf_vertical_4 as OurKernel, v_off),
        (cref::LpfKind::H6, lf::lpf_horizontal_6 as OurKernel, h_off),
        (cref::LpfKind::V6, lf::lpf_vertical_6 as OurKernel, v_off),
        (cref::LpfKind::H8, lf::lpf_horizontal_8 as OurKernel, h_off),
        (cref::LpfKind::V8, lf::lpf_vertical_8 as OurKernel, v_off),
        (
            cref::LpfKind::H14,
            lf::lpf_horizontal_14 as OurKernel,
            h_off,
        ),
        (cref::LpfKind::V14, lf::lpf_vertical_14 as OurKernel, v_off),
    ]
}

/// Content generators. The masks (filter/flat/hev) key off local
/// differences, so cover: small-noise-flat (flat branch), step edges of
/// varying height (outer-edge + hev branches), and full random (mask-off).
fn fill(content: u32, rng: &mut Rng, vertical: bool, buf: &mut [u8]) {
    match content {
        // Flat base with +/- noise of random small amplitude.
        0 => {
            let base = 40 + rng.range(176) as i32;
            let amp = rng.range(6) as i32; // 0..=5
            for px in buf.iter_mut() {
                let n = if amp == 0 {
                    0
                } else {
                    rng.range(2 * amp as u64 + 1) as i32 - amp
                };
                *px = (base + n).clamp(0, 255) as u8;
            }
        }
        // Step edge across the filtered boundary + small noise.
        1 => {
            let a = rng.range(256) as i32;
            let b = rng.range(256) as i32;
            let amp = rng.range(4) as i32;
            for r in 0..SIZE {
                for c in 0..SIZE {
                    let coord = if vertical { c } else { r };
                    let base = if coord < 8 { a } else { b };
                    let n = if amp == 0 {
                        0
                    } else {
                        rng.range(2 * amp as u64 + 1) as i32 - amp
                    };
                    buf[r * SIZE + c] = (base + n).clamp(0, 255) as u8;
                }
            }
        }
        // Full-range random.
        _ => {
            for px in buf.iter_mut() {
                *px = rng.byte();
            }
        }
    }
}

#[test]
fn lpf_kernels_match_c_over_level_sharpness_space() {
    let mut rng = Rng(0xDEB1_0C1C_2026_0713);
    for (kind, ours, off) in kernels() {
        let (_, vertical) = kind.geometry();
        for sharpness in [0u8, 1, 4, 7] {
            for level in 0..=63u8 {
                let t = lf::lf_thresholds(level, sharpness);
                for content in 0..3u32 {
                    let mut buf = vec![0u8; SIZE * SIZE];
                    fill(content, &mut rng, vertical, &mut buf);
                    let mut c_buf = buf.clone();

                    ours(&mut buf, off, SIZE, t);
                    cref::lpf(kind, &mut c_buf, off, SIZE, t.mblim, t.lim, t.hev_thr);

                    if buf != c_buf {
                        let i = buf
                            .iter()
                            .zip(c_buf.iter())
                            .position(|(a, b)| a != b)
                            .unwrap();
                        panic!(
                            "{kind:?} level {level} sharp {sharpness} content {content}: \
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

/// Robustness: arbitrary (mblim, lim, hev) triples outside the reachable
/// derivation, over random content — the kernels must still agree.
#[test]
fn lpf_kernels_match_c_random_params() {
    let mut rng = Rng(0x5EED_1F17_7E57_0001);
    for (kind, ours, off) in kernels() {
        let (_, vertical) = kind.geometry();
        for _ in 0..300 {
            let t = lf::LfThresh {
                mblim: rng.byte(),
                lim: rng.byte(),
                hev_thr: rng.byte(),
            };
            let content = (rng.range(3)) as u32;
            let mut buf = vec![0u8; SIZE * SIZE];
            fill(content, &mut rng, vertical, &mut buf);
            let mut c_buf = buf.clone();

            ours(&mut buf, off, SIZE, t);
            cref::lpf(kind, &mut c_buf, off, SIZE, t.mblim, t.lim, t.hev_thr);
            assert_eq!(buf, c_buf, "{kind:?} random-params divergence");
        }
    }
}

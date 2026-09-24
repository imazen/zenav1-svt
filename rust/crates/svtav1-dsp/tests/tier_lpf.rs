//! Dispatch-tier lock for `lpf_{horizontal,vertical}_{6,8,14}`.
//!
//! The v3 arms port C's `dlf_intrin_sse2.c` kernels. They reproduce `_sse2`
//! on every threshold the encoder can derive (mblim <= 193, lim <= 63) and
//! `_c` everywhere — including the blimit/limit=255 corners where C's own
//! _sse2 saturating sum and flag fold diverge from `~mask`. Every dispatch
//! tier is pinned against the linked C `_c` reference across flat, step-edge,
//! and random content, including threshold triples outside the derivable
//! space.

use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
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

const SIZE: usize = 16;

fn fill(content: u32, rng: &mut Rng, vertical: bool, buf: &mut [u8]) {
    match content {
        // Flat base with +/- noise — exercises flat/flat2 paths.
        0 => {
            let base = 40 + rng.range(176) as i32;
            let amp = rng.range(6) as i32;
            for px in buf.iter_mut() {
                let n = if amp == 0 {
                    0
                } else {
                    rng.range(2 * amp as u64 + 1) as i32 - amp
                };
                *px = (base + n).clamp(0, 255) as u8;
            }
        }
        // Step edge across the filtered boundary.
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
        _ => {
            for px in buf.iter_mut() {
                *px = rng.byte();
            }
        }
    }
}

#[test]
fn lpf_all_tiers_match_c() {
    let mut rng = Rng(0x14F1_17E7_2026_0917);
    // Edge geometry matches tests/c_parity_lpf.rs: H edge between rows 7|8
    // filtering columns 3..6; V edge between cols 7|8 filtering rows 2..5.
    let h_off = 8 * SIZE + 3;
    let v_off = 2 * SIZE + 8;
    let kinds = [
        cref::LpfKind::H6,
        cref::LpfKind::V6,
        cref::LpfKind::H8,
        cref::LpfKind::V8,
        cref::LpfKind::H14,
        cref::LpfKind::V14,
    ];
    for iter in 0..600 {
        let kind = kinds[(iter % kinds.len() as usize) as usize];
        let (_, vertical) = kind.geometry();
        let off = if vertical { v_off } else { h_off };
        // Mix derivable thresholds with adversarial triples — the 255
        // corners are where C's own _sse2 kernel diverges from _c.
        let t = if iter % 5 == 0 {
            lf::LfThresh {
                mblim: 255,
                lim: 255,
                hev_thr: rng.byte(),
            }
        } else {
            lf::LfThresh {
                mblim: rng.byte(),
                lim: rng.byte(),
                hev_thr: rng.byte(),
            }
        };
        let mut buf = vec![0u8; SIZE * SIZE];
        fill((rng.range(3)) as u32, &mut rng, vertical, &mut buf);
        let mut c_buf = buf.clone();
        cref::lpf(kind, &mut c_buf, off, SIZE, t.mblim, t.lim, t.hev_thr);

        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut ours = buf.clone();
            match kind {
                cref::LpfKind::H6 => lf::lpf_horizontal_6(&mut ours, off, SIZE, t),
                cref::LpfKind::V6 => lf::lpf_vertical_6(&mut ours, off, SIZE, t),
                cref::LpfKind::H8 => lf::lpf_horizontal_8(&mut ours, off, SIZE, t),
                cref::LpfKind::V8 => lf::lpf_vertical_8(&mut ours, off, SIZE, t),
                cref::LpfKind::H14 => lf::lpf_horizontal_14(&mut ours, off, SIZE, t),
                cref::LpfKind::V14 => lf::lpf_vertical_14(&mut ours, off, SIZE, t),
                other => panic!("unexpected kind {other:?}"),
            }
            assert_eq!(ours, c_buf, "{kind:?} iter {iter} t={t:?}");
        });
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.permutations_run >= 2, "{report:?}");
    }
}

/// Regression (2026-09-24, found by zenavif's 100x37 partial-SB roundtrip at
/// speed 4): the SIMD `lpf_vertical_6` arms load 8 bytes per row at `s-3`
/// where the filter touches 6, so on the last row of an unpadded plane the
/// load ran past the buffer and panicked. Every tier must filter a buffer
/// that ends exactly after the scalar kernel's last touched byte, and match
/// C run on a padded copy.
#[test]
fn lpf_vertical_6_at_the_end_of_an_unpadded_plane() {
    let mut rng = Rng(0x0DD3_7202_6092_4000);
    let v_off = 2 * SIZE + 8;
    // Last filtered row starts at v_off + 3*SIZE; the scalar footprint there
    // is s-3..=s+2, so the buffer ends at s+3 (exclusive).
    let end = v_off + 3 * SIZE + 3;
    for iter in 0..60 {
        let t = lf::LfThresh {
            mblim: rng.byte(),
            lim: rng.byte(),
            hev_thr: rng.byte(),
        };
        let mut padded = vec![0u8; SIZE * SIZE];
        fill(rng.range(3) as u32, &mut rng, true, &mut padded);
        let mut c_buf = padded.clone();
        cref::lpf(
            cref::LpfKind::V6,
            &mut c_buf,
            v_off,
            SIZE,
            t.mblim,
            t.lim,
            t.hev_thr,
        );
        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let mut ours = padded[..end].to_vec();
            lf::lpf_vertical_6(&mut ours, v_off, SIZE, t);
            assert_eq!(ours[..], c_buf[..end], "V6 unpadded iter {iter} t={t:?}");
        });
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.permutations_run >= 2, "{report:?}");
    }
}

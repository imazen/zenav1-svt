//! Differential parity: the HIGH-BIT-DEPTH intra predictors vs the C
//! reference (`svt_aom_highbd_*_predictor_WxH_c`) at bd 10 and 12.
//!
//! These are the prediction step for every intra block on the 10/12-bit path —
//! `hbd::predict_{v,h,paeth,dc,smooth,smooth_v,smooth_h}_hbd`. The whole `hbd`
//! module was a bulk source translation (task #94) and every predictor still
//! carried a `PORT-NOTE(unverified)` marker: verbatim transcription is this
//! project's weakest evidence tier, and these kernels concentrate exactly the
//! subtle divergences that survive a careful read:
//!
//! - **Paeth tie-break order.** The module doc records that the already-wired
//!   u8 `predict_paeth_core` checks TOP first, disagreeing with the real C
//!   `paeth_predictor_single` (LEFT first) whenever `p_top == p_left`. The hbd
//!   port claims to fix this (left-first). Full-range random neighbours hit the
//!   `p_top == p_left` tie often — this test *proves* the hbd paeth matches C's
//!   actual order rather than reproducing the u8 bug.
//! - **DC variant + rounding.** C ships four distinct DC wrappers (both / top /
//!   left / 128); the port folds them into one `(has_above, has_left)` branch.
//!   Only the `(false,false)` arm is bd-dependent (`128 << (bd - 8)`) — a wrong
//!   shift or a wrong `(sum + count/2)/count` rounding scales a flat block.
//! - **Smooth `divide_round` shift + duplicated weight tables.** `predict_smooth*`
//!   re-implement `divide_round(_, 8|9)` and carry a private copy of the
//!   `sm_weight_arrays` sub-tables; a single wrong weight or shift skews the
//!   blend.
//!
//! C exposes one sized wrapper per (mode, W, H); the port is a generic W×H form,
//! so a width/height/stride marshalling bug is invisible until a real hbd
//! encode. Fuzzed over the full sized family (5 square + 14 rectangular = 19
//! shapes), full-range random neighbour content, tight and padded destination
//! strides, at bd 10 and 12. The four DC split variants are driven through the
//! port's flag pair, so every C wrapper in the family is exercised.

use svtav1_cref as cref;
use svtav1_dsp::hbd;

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
    /// A full-range sample at `bd` bits (`0..(1 << bd)`).
    fn pix(&mut self, bd: u8) -> u16 {
        (self.next() as u32 & ((1u32 << bd) - 1)) as u16
    }
}

/// Every predictor mode the port implements, in the cref enum's discriminant
/// order. `Dc*` are C's four separate DC wrappers, driven below through the
/// port's single `predict_dc_hbd(.., has_above, has_left, ..)` flag pair.
const MODES: [cref::HbdIntraPred; 10] = [
    cref::HbdIntraPred::Dc,
    cref::HbdIntraPred::DcTop,
    cref::HbdIntraPred::DcLeft,
    cref::HbdIntraPred::Dc128,
    cref::HbdIntraPred::V,
    cref::HbdIntraPred::H,
    cref::HbdIntraPred::Paeth,
    cref::HbdIntraPred::Smooth,
    cref::HbdIntraPred::SmoothV,
    cref::HbdIntraPred::SmoothH,
];

/// The exact (W, H) set the C `intra_pred_highbd_sized` macro instantiates:
/// 5 square + 14 rectangular AV1 transform shapes.
const SIZES: &[(usize, usize)] = &[
    (4, 4),
    (8, 8),
    (16, 16),
    (32, 32),
    (64, 64),
    (4, 8),
    (4, 16),
    (8, 4),
    (8, 16),
    (8, 32),
    (16, 4),
    (16, 8),
    (16, 32),
    (16, 64),
    (32, 8),
    (32, 16),
    (32, 64),
    (64, 16),
    (64, 32),
];

/// Run the port predictor for `mode` into `dst`. The `Dc*` variants select the
/// `(has_above, has_left)` pair that C's four DC wrappers correspond to; `V`/`H`
/// ignore the unused neighbour, `Paeth` takes the corner separately.
#[allow(clippy::too_many_arguments)]
fn port_predict(
    mode: cref::HbdIntraPred,
    dst: &mut [u16],
    stride: usize,
    above: &[u16],
    left: &[u16],
    top_left: u16,
    w: usize,
    h: usize,
    bd: u8,
) {
    use cref::HbdIntraPred as M;
    match mode {
        M::Dc => hbd::predict_dc_hbd(dst, stride, above, left, w, h, true, true, bd),
        M::DcTop => hbd::predict_dc_hbd(dst, stride, above, left, w, h, true, false, bd),
        M::DcLeft => hbd::predict_dc_hbd(dst, stride, above, left, w, h, false, true, bd),
        M::Dc128 => hbd::predict_dc_hbd(dst, stride, above, left, w, h, false, false, bd),
        M::V => hbd::predict_v_hbd(dst, stride, above, w, h),
        M::H => hbd::predict_h_hbd(dst, stride, left, w, h),
        M::Paeth => hbd::predict_paeth_hbd(dst, stride, above, left, top_left, w, h),
        M::Smooth => hbd::predict_smooth_hbd(dst, stride, above, left, w, h),
        M::SmoothV => hbd::predict_smooth_v_hbd(dst, stride, above, left, w, h),
        M::SmoothH => hbd::predict_smooth_h_hbd(dst, stride, above, left, w, h),
    }
}

#[test]
fn hbd_intra_predictors_match_c() {
    let mut rng = Rng(0x1D2A_2026_0718_0001);
    for bd in [10u8, 12] {
        for &mode in &MODES {
            for &(w, h) in SIZES {
                for iter in 0..20 {
                    // Alternate tight / padded destination stride so both the
                    // contiguous fast path and the offset arithmetic are hit.
                    let stride = if iter % 2 == 0 { w } else { w + 3 };
                    let above: Vec<u16> = (0..w).map(|_| rng.pix(bd)).collect();
                    let left: Vec<u16> = (0..h).map(|_| rng.pix(bd)).collect();
                    let top_left = rng.pix(bd);

                    let mut ours = vec![0u16; h * stride];
                    port_predict(mode, &mut ours, stride, &above, &left, top_left, w, h, bd);

                    let c = cref::highbd_intra_pred(mode, stride, &above, &left, top_left, w, h, bd as i32);

                    for row in 0..h {
                        let a = &ours[row * stride..row * stride + w];
                        let b = &c[row * stride..row * stride + w];
                        if a != b {
                            let col = a.iter().zip(b).position(|(x, y)| x != y).unwrap();
                            panic!(
                                "{mode:?} {w}x{h} bd{bd} stride{stride} iter{iter}: \
                                 first diff at (r{row} c{col}): ours={} c={}",
                                a[col], b[col]
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Harness-soundness / non-vacuous guard: prove the C oracle actually runs and
/// is marshalled correctly (rather than both sides silently producing zeros),
/// via three known-answer cases whose results are fixed by definition — and
/// confirm the `bd` argument is threaded (DC128 = `128 << (bd - 8)` differs per
/// depth: 128 / 512 / 2048). If this ever failed, the differential sweep above
/// could be comparing two identically-broken paths.
#[test]
fn hbd_intra_harness_known_answers() {
    let (w, h, stride) = (8usize, 8usize, 11usize);
    for bd in [8i32, 10, 12] {
        let above: Vec<u16> = (0..w as u16).map(|i| i * 7 + 3).collect();
        let left: Vec<u16> = (0..h as u16).map(|i| i * 5 + 1).collect();

        // V replicates the above row down every column.
        let v = cref::highbd_intra_pred(cref::HbdIntraPred::V, stride, &above, &left, 0, w, h, bd);
        for row in 0..h {
            assert_eq!(&v[row * stride..row * stride + w], &above[..], "V row {row} bd{bd}");
        }

        // H replicates each left sample across its row.
        let hpred = cref::highbd_intra_pred(cref::HbdIntraPred::H, stride, &above, &left, 0, w, h, bd);
        for row in 0..h {
            assert!(
                hpred[row * stride..row * stride + w].iter().all(|&p| p == left[row]),
                "H row {row} bd{bd}"
            );
        }

        // DC128 fills 128 << (bd - 8) regardless of neighbours.
        let d128 = cref::highbd_intra_pred(cref::HbdIntraPred::Dc128, stride, &above, &left, 0, w, h, bd);
        let expect = 128u16 << (bd as u16 - 8);
        for row in 0..h {
            assert!(
                d128[row * stride..row * stride + w].iter().all(|&p| p == expect),
                "DC128 bd{bd} expected {expect}"
            );
        }
    }
}

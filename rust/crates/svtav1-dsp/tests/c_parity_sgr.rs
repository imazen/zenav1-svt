//! Differential (evidence tier 1): the self-guided (SGR) restoration chain
//! against the REAL exported symbols of `Codec/restoration.c`.
//!
//! | C symbol                            | port fn                        | also covers (static in C) |
//! |-------------------------------------|--------------------------------|---------------------------|
//! | `svt_aom_eb_sgr_params`             | `tables::SGR_PARAMS`           | — (table pin)             |
//! | `svt_aom_eb_x_by_xplus1`            | `tables::X_BY_XPLUS1`          | — (table pin)             |
//! | `svt_aom_eb_one_by_x`               | `tables::ONE_BY_X`             | — (table pin)             |
//! | `svt_decode_xq`                     | `decode_xq`                    | —                         |
//! | `svt_av1_selfguided_restoration_c`  | `selfguided_restoration`       | `boxsum`, `boxsum1`, `boxsum2`, `selfguided_restoration_internal`, `selfguided_restoration_fast_internal` |
//! | `svt_apply_selfguided_restoration_c`| `apply_selfguided_restoration` | all of the above + `svt_decode_xq` |
//!
//! `sgrproj_filter_stripe` / `_highbd` are `static` with no exported driver
//! reachable from here; they are thin `AOMMIN` loops over
//! `apply_selfguided_restoration`, and the tests below drive that loop shape
//! explicitly against C by calling the oracle per proc-unit.

use svtav1_dsp::port_sgr::{
    self, ONE_BY_X, RESTORATION_UNITPELS_MAX, SGR_PARAMS, SGRPROJ_BORDER_HORZ, SGRPROJ_BORDER_VERT,
    SGRPROJ_PARAMS, SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1, SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1, SgrDst,
    SgrSrc, X_BY_XPLUS1,
};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 32) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u32() % ((hi - lo + 1) as u32)) as i32
    }
}

// ---------------------------------------------------------------------------
// 1. The constant tables
// ---------------------------------------------------------------------------

#[test]
fn sgr_params_table_matches_c() {
    for ep in 0..SGRPROJ_PARAMS {
        let c = svtav1_cref::sgr_params(ep as i32);
        let r = SGR_PARAMS[ep];
        assert_eq!(
            [r.r[0], r.r[1], r.s[0], r.s[1]],
            c,
            "svt_aom_eb_sgr_params[{ep}] mismatch"
        );
    }
}

#[test]
fn x_by_xplus1_and_one_by_x_match_c() {
    assert_eq!(X_BY_XPLUS1, svtav1_cref::sgr_x_by_xplus1());
    assert_eq!(ONE_BY_X, svtav1_cref::sgr_one_by_x());
    // The special case C calls out: 0 maps to 1, not 0.
    assert_eq!(X_BY_XPLUS1[0], 1);
}

// ---------------------------------------------------------------------------
// 2. svt_decode_xq
// ---------------------------------------------------------------------------

/// Exhaustive over every `ep` and the whole signalled `xqd` range
/// (`SGRPROJ_PRJ_MIN0..=MAX0` x `MIN1..=MAX1`) — 16 * 128 * 128 = 262144 cells.
#[test]
fn decode_xq_matches_c_exhaustively() {
    let mut arm0 = 0usize;
    let mut arm1 = 0usize;
    let mut arm2 = 0usize;
    for ep in 0..SGRPROJ_PARAMS {
        let p = SGR_PARAMS[ep];
        if p.r[0] == 0 {
            arm0 += 1;
        } else if p.r[1] == 0 {
            arm1 += 1;
        } else {
            arm2 += 1;
        }
        for a in SGRPROJ_PRJ_MIN0..=SGRPROJ_PRJ_MAX0 {
            for b in SGRPROJ_PRJ_MIN1..=SGRPROJ_PRJ_MAX1 {
                let xqd = [a, b];
                let c = svtav1_cref::decode_xq(&xqd, ep as i32);
                let r = port_sgr::decode_xq(&xqd, &p);
                assert_eq!(r, c, "decode_xq mismatch at ep {ep} xqd {xqd:?}");
            }
        }
    }
    // Anti-vacuity: all three branches of the C function exist in the table.
    assert!(arm0 > 0 && arm1 > 0 && arm2 > 0, "not every arm reached");
}

// ---------------------------------------------------------------------------
// 3. svt_av1_selfguided_restoration_c
//    (covers boxsum / boxsum1 / boxsum2 / both internals)
// ---------------------------------------------------------------------------

/// Build an extended 8-bit plane: `stride = width + 2*BORDER_HORZ + slack`,
/// with `BORDER_VERT` rows above and below. Returns `(plane, origin, stride)`.
fn extended_plane_u8(
    width: i32,
    height: i32,
    seed: u64,
    flat: Option<u8>,
) -> (Vec<u8>, usize, usize) {
    let bh = SGRPROJ_BORDER_HORZ as usize;
    let bv = SGRPROJ_BORDER_VERT as usize;
    let stride = width as usize + 2 * bh + 5; // deliberate non-tight stride
    let rows = height as usize + 2 * bv;
    let mut rng = Rng::new(seed);
    let plane: Vec<u8> = (0..stride * rows)
        .map(|_| match flat {
            Some(v) => v,
            None => rng.range(0, 255) as u8,
        })
        .collect();
    (plane, bv * stride + bh, stride)
}

fn extended_plane_u16(
    width: i32,
    height: i32,
    seed: u64,
    bd: i32,
    flat: Option<u16>,
) -> (Vec<u16>, usize, usize) {
    let bh = SGRPROJ_BORDER_HORZ as usize;
    let bv = SGRPROJ_BORDER_VERT as usize;
    let stride = width as usize + 2 * bh + 5;
    let rows = height as usize + 2 * bv;
    let maxv = (1i32 << bd) - 1;
    let mut rng = Rng::new(seed);
    let plane: Vec<u16> = (0..stride * rows)
        .map(|_| match flat {
            Some(v) => v,
            None => rng.range(0, maxv) as u16,
        })
        .collect();
    (plane, bv * stride + bh, stride)
}

/// Every `ep` preset x several unit shapes, on textured content. This is the
/// test that covers `boxsum1` (r = 1), `boxsum2` (r = 2), both `sqr` arms and
/// both internals.
#[test]
fn selfguided_restoration_matches_c_over_every_ep() {
    let shapes = [
        (8i32, 8i32),
        (16, 16),
        (64, 64),
        (64, 8),
        (12, 20),
        (37, 13),
    ];
    let mut used_fast = 0usize;
    let mut used_full = 0usize;

    for ep in 0..SGRPROJ_PARAMS {
        let p = SGR_PARAMS[ep];
        for (si, &(w, h)) in shapes.iter().enumerate() {
            let (plane, origin, stride) = extended_plane_u8(w, h, (ep * 31 + si) as u64, None);

            let mut c_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut c_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            svtav1_cref::selfguided_restoration_lbd(
                &plane,
                origin,
                w,
                h,
                stride,
                &mut c_flt0,
                &mut c_flt1,
                w as usize,
                ep as i32,
                8,
            );

            let mut r_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut r_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            port_sgr::selfguided_restoration(
                SgrSrc::Lowbd(&plane),
                origin,
                w,
                h,
                stride,
                &mut r_flt0,
                &mut r_flt1,
                w as usize,
                ep,
                8,
            );

            let n = (w * h) as usize;
            if p.r[0] > 0 {
                assert_eq!(
                    r_flt0[..n],
                    c_flt0[..n],
                    "flt0 mismatch at ep {ep} shape {w}x{h}"
                );
                used_fast += 1;
            }
            if p.r[1] > 0 {
                assert_eq!(
                    r_flt1[..n],
                    c_flt1[..n],
                    "flt1 mismatch at ep {ep} shape {w}x{h}"
                );
                used_full += 1;
            }
        }
    }
    // Anti-vacuity: both internals actually ran.
    assert!(used_fast > 0, "the r=2 fast internal never ran");
    assert!(used_full > 0, "the r=1 full internal never ran");
}

/// Flat content is the case C's own comments warn about — `z == 0`, where
/// `A[k]` is saturated to 1 rather than 0 and `B[k]` can otherwise overflow.
/// A transcription that dropped that saturation passes on textured content and
/// fails here.
#[test]
fn selfguided_restoration_matches_c_on_flat_content() {
    for ep in 0..SGRPROJ_PARAMS {
        for &v in &[0u8, 1, 128, 255] {
            let (w, h) = (16i32, 16i32);
            let (plane, origin, stride) = extended_plane_u8(w, h, 1, Some(v));
            let mut c_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut c_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            svtav1_cref::selfguided_restoration_lbd(
                &plane,
                origin,
                w,
                h,
                stride,
                &mut c_flt0,
                &mut c_flt1,
                w as usize,
                ep as i32,
                8,
            );
            let mut r_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut r_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            port_sgr::selfguided_restoration(
                SgrSrc::Lowbd(&plane),
                origin,
                w,
                h,
                stride,
                &mut r_flt0,
                &mut r_flt1,
                w as usize,
                ep,
                8,
            );
            let n = (w * h) as usize;
            let p = SGR_PARAMS[ep];
            if p.r[0] > 0 {
                assert_eq!(r_flt0[..n], c_flt0[..n], "flat flt0 ep {ep} v {v}");
            }
            if p.r[1] > 0 {
                assert_eq!(r_flt1[..n], c_flt1[..n], "flat flt1 ep {ep} v {v}");
            }
        }
    }
}

/// The high-bit-depth arm: `bit_depth = 10` shifts `a` and `b` before the
/// variance, which is where the `a * n < b * b` saturation actually fires.
#[test]
fn selfguided_restoration_matches_c_at_bd10() {
    for ep in 0..SGRPROJ_PARAMS {
        for &(w, h) in &[(16i32, 16i32), (64, 32)] {
            let (plane, origin, stride) = extended_plane_u16(w, h, ep as u64 + 7, 10, None);
            let mut c_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut c_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            svtav1_cref::selfguided_restoration_hbd(
                &plane,
                origin,
                w,
                h,
                stride,
                &mut c_flt0,
                &mut c_flt1,
                w as usize,
                ep as i32,
                10,
            );
            let mut r_flt0 = vec![0i32; RESTORATION_UNITPELS_MAX];
            let mut r_flt1 = vec![0i32; RESTORATION_UNITPELS_MAX];
            port_sgr::selfguided_restoration(
                SgrSrc::Highbd(&plane),
                origin,
                w,
                h,
                stride,
                &mut r_flt0,
                &mut r_flt1,
                w as usize,
                ep,
                10,
            );
            let n = (w * h) as usize;
            let p = SGR_PARAMS[ep];
            if p.r[0] > 0 {
                assert_eq!(r_flt0[..n], c_flt0[..n], "bd10 flt0 ep {ep} {w}x{h}");
            }
            if p.r[1] > 0 {
                assert_eq!(r_flt1[..n], c_flt1[..n], "bd10 flt1 ep {ep} {w}x{h}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. svt_apply_selfguided_restoration_c — the whole filter as the decoder runs it
// ---------------------------------------------------------------------------

#[test]
fn apply_selfguided_restoration_matches_c_over_every_ep_and_xqd() {
    let mut rng = Rng::new(0xA51);
    let mut nondegenerate = 0usize;

    for ep in 0..SGRPROJ_PARAMS {
        for trial in 0..6 {
            let (w, h) = [(8i32, 8i32), (16, 16), (64, 64), (31, 17), (64, 8), (4, 4)][trial];
            let (plane, origin, stride) = extended_plane_u8(w, h, (ep * 97 + trial) as u64, None);
            let xqd = [
                rng.range(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0),
                rng.range(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
            ];
            let dst_stride = w as usize + 3;
            let dst_len = dst_stride * h as usize;

            let mut c_dst = vec![0u8; dst_len];
            svtav1_cref::apply_selfguided_restoration_lbd(
                &plane, origin, w, h, stride, ep as i32, &xqd, &mut c_dst, 0, dst_stride, 8,
            );

            let mut r_buf = vec![0u8; dst_len];
            let mut r_dst = SgrDst::Lowbd(&mut r_buf);
            port_sgr::apply_selfguided_restoration(
                SgrSrc::Lowbd(&plane),
                origin,
                w,
                h,
                stride,
                ep,
                &xqd,
                &mut r_dst,
                0,
                dst_stride,
                8,
            );
            assert_eq!(
                r_buf, c_dst,
                "apply mismatch at ep {ep} shape {w}x{h} xqd {xqd:?}"
            );
            if c_dst.iter().any(|&v| v != c_dst[0]) {
                nondegenerate += 1;
            }
        }
    }
    assert!(
        nondegenerate > 50,
        "only {nondegenerate} non-degenerate outputs — the oracle may be flat"
    );
}

/// The extremes of the signalled weight range, which is where the `int16_t`
/// narrowing in C's `w` can actually truncate.
#[test]
fn apply_selfguided_restoration_matches_c_at_xqd_extremes() {
    let corners = [
        [SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MIN1],
        [SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX1],
        [SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MIN1],
        [SGRPROJ_PRJ_MAX0, SGRPROJ_PRJ_MAX1],
        [0, 0],
    ];
    for ep in 0..SGRPROJ_PARAMS {
        for xqd in corners {
            let (w, h) = (24i32, 24i32);
            // High-contrast content maximises |flt - u| and therefore |v|.
            let bh = SGRPROJ_BORDER_HORZ as usize;
            let bv = SGRPROJ_BORDER_VERT as usize;
            let stride = w as usize + 2 * bh + 5;
            let rows = h as usize + 2 * bv;
            let plane: Vec<u8> = (0..stride * rows)
                .map(|i| if (i / 3) % 2 == 0 { 0 } else { 255 })
                .collect();
            let origin = bv * stride + bh;
            let dst_stride = w as usize;

            let mut c_dst = vec![0u8; dst_stride * h as usize];
            svtav1_cref::apply_selfguided_restoration_lbd(
                &plane, origin, w, h, stride, ep as i32, &xqd, &mut c_dst, 0, dst_stride, 8,
            );
            let mut r_buf = vec![0u8; dst_stride * h as usize];
            let mut r_dst = SgrDst::Lowbd(&mut r_buf);
            port_sgr::apply_selfguided_restoration(
                SgrSrc::Lowbd(&plane),
                origin,
                w,
                h,
                stride,
                ep,
                &xqd,
                &mut r_dst,
                0,
                dst_stride,
                8,
            );
            assert_eq!(r_buf, c_dst, "apply extreme mismatch ep {ep} xqd {xqd:?}");
        }
    }
}

#[test]
fn apply_selfguided_restoration_matches_c_at_bd10() {
    let mut rng = Rng::new(0xBD10);
    for ep in 0..SGRPROJ_PARAMS {
        for &(w, h) in &[(16i32, 16i32), (64, 24)] {
            let (plane, origin, stride) = extended_plane_u16(w, h, ep as u64 * 13 + 3, 10, None);
            let xqd = [
                rng.range(SGRPROJ_PRJ_MIN0, SGRPROJ_PRJ_MAX0),
                rng.range(SGRPROJ_PRJ_MIN1, SGRPROJ_PRJ_MAX1),
            ];
            let dst_stride = w as usize + 2;
            let mut c_dst = vec![0u16; dst_stride * h as usize];
            svtav1_cref::apply_selfguided_restoration_hbd(
                &plane, origin, w, h, stride, ep as i32, &xqd, &mut c_dst, 0, dst_stride, 10,
            );
            let mut r_buf = vec![0u16; dst_stride * h as usize];
            let mut r_dst = SgrDst::Highbd(&mut r_buf);
            port_sgr::apply_selfguided_restoration(
                SgrSrc::Highbd(&plane),
                origin,
                w,
                h,
                stride,
                ep,
                &xqd,
                &mut r_dst,
                0,
                dst_stride,
                10,
            );
            assert_eq!(r_buf, c_dst, "bd10 apply mismatch ep {ep} {w}x{h}");
        }
    }
}

// ---------------------------------------------------------------------------
// 5. sgrproj_filter_stripe — the proc-unit loop
// ---------------------------------------------------------------------------

/// `sgrproj_filter_stripe` is `static` in C, so this drives the SAME loop
/// against the exported per-unit oracle: a stripe wider than `procunit_width`
/// must decompose into `AOMMIN(procunit_width, stripe_width - j)` calls, and a
/// port that filtered the stripe in one call would differ (the SGR filter is
/// not separable across proc-unit boundaries — each unit re-derives its own
/// A/B from its own borders).
#[test]
fn sgrproj_filter_stripe_matches_the_c_per_unit_decomposition() {
    let procunit_width = 64i32;
    for &(stripe_width, stripe_height) in &[(64i32, 8i32), (128, 8), (150, 16), (200, 4)] {
        for ep in [0usize, 5, 10, 14] {
            let xqd = [31, -7];
            let (plane, origin, stride) =
                extended_plane_u8(stripe_width, stripe_height, ep as u64 + 77, None);
            let dst_stride = stripe_width as usize + 4;
            let dst_len = dst_stride * stripe_height as usize;

            // C's decomposition, driven through the exported per-unit oracle.
            let mut c_dst = vec![0u8; dst_len];
            let mut j = 0i32;
            while j < stripe_width {
                let w = procunit_width.min(stripe_width - j);
                svtav1_cref::apply_selfguided_restoration_lbd(
                    &plane,
                    origin + j as usize,
                    w,
                    stripe_height,
                    stride,
                    ep as i32,
                    &xqd,
                    &mut c_dst,
                    j as usize,
                    dst_stride,
                    8,
                );
                j += procunit_width;
            }

            let mut r_dst = vec![0u8; dst_len];
            port_sgr::sgrproj_filter_stripe(
                ep,
                &xqd,
                stripe_width,
                stripe_height,
                procunit_width,
                &plane,
                origin,
                stride,
                &mut r_dst,
                0,
                dst_stride,
            );
            assert_eq!(
                r_dst, c_dst,
                "sgrproj_filter_stripe mismatch: {stripe_width}x{stripe_height} ep {ep}"
            );
        }
    }
}

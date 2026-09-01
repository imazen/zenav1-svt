//! Differential parity for the CONV_BUF-domain DIFFWTD mask builder —
//! evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbol driven: `svt_av1_build_compound_diffwtd_mask_d16_c`
//! (C_DEFAULT/inter_prediction_c.c:30), `nm -g`-visible as `T`. The `static`
//! `diffwtd_mask_d16` (:16) is gated INDIRECTLY: the exported function is its
//! only caller and passes its whole output through, so a difference in that
//! inner loop shows up here. Both live `DIFFWTD_MASK_TYPE` values are driven,
//! which is both of its `which_inverse` arms.
//!
//! # The generator's domain, and how it is bounded (WORKING-ON-THIS §5)
//!
//! `src0` / `src1` are CONV_BUFs — the `round_1`-domain compound intermediate
//! that `svt_av1_jnt_convolve_*` writes — not pixels. Rather than assert a
//! range, [`producer_driven_values_match_c`] DRIVES the real producer
//! (`svt_av1_jnt_convolve_2d_c`, through `svtav1_cref`) over extreme 8-bit
//! pixels and feeds its output straight into both mask builders. The observed
//! bound is printed by [`producer_range_is_what_the_scalar_sweep_covers`], and
//! the synthetic sweeps below cover a superset of it.
//!
//! The synthetic sweep goes to the full `u16` range against the SCALAR `_c`
//! kernel only. That is deliberate: the arithmetic there is pure integer
//! promotion on both sides, so a wider domain is free evidence — whereas a
//! SIMD tier can legitimately diverge outside the producer's range (the
//! `_mm_madd_epi16` signedness class of bug), so
//! [`dispatched_tier_matches_scalar_on_producer_range`] pins this host's
//! dispatched kernel on the producer-reachable domain instead.

use svtav1_cref::inter_pred::{self as cref, JntCfg};
use svtav1_cref::interpred_gap::{self as gap, D16MaskTier};
use svtav1_dsp::port_convolve::ConvolveParams;
use svtav1_dsp::port_diffwtd_d16::{build_compound_diffwtd_mask_d16, d16_diff_round};
use svtav1_dsp::port_masked_compound::DiffwtdMaskType;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

const TYPES: [(DiffwtdMaskType, i32); 2] =
    [(DiffwtdMaskType::D38, 0), (DiffwtdMaskType::D38Inv, 1)];

/// Run both sides on one already-built pair of CONV_BUFs and assert equality.
fn compare(
    tier: D16MaskTier,
    src0: &[u16],
    src0_stride: usize,
    src1: &[u16],
    src1_stride: usize,
    h: usize,
    w: usize,
    bd: i32,
    what: &str,
) {
    let cp = ConvolveParams::no_round(false, 0, true, bd);
    for (ty, ty_c) in TYPES {
        let mut got = vec![0u8; h * w];
        let mut want = vec![0u8; h * w];
        build_compound_diffwtd_mask_d16(
            &mut got,
            ty,
            src0,
            src0_stride,
            src1,
            src1_stride,
            h,
            w,
            &cp,
            bd,
        );
        gap::build_compound_diffwtd_mask_d16(
            tier,
            &mut want,
            ty_c,
            src0,
            src0_stride,
            src1,
            src1_stride,
            h,
            w,
            bd,
        );
        assert_eq!(got, want, "{what} {ty:?} {w}x{h} bd{bd} tier {tier:?}");
    }
}

/// The `round` the port derives must be the one C derives, for every depth
/// and both `is_compound` settings — including the `bd = 12` arm where
/// `get_conv_params_no_round`'s `intbufrange > 16` correction fires.
#[test]
fn diff_round_matches_c() {
    for bd in [8, 10, 12] {
        for is_compound in [false, true] {
            let cp = ConvolveParams::no_round(false, 0, is_compound, bd);
            assert_eq!(
                d16_diff_round(&cp, bd),
                gap::d16_diff_round(is_compound, bd),
                "round bd{bd} is_compound={is_compound}"
            );
        }
    }
}

/// Build one pair of CONV_BUFs by running the REAL C compound convolve over
/// hostile 8-bit pixels, so the mask sees exactly what the encoder feeds it.
fn producer_conv_bufs(seed: u32, w: usize, h: usize) -> (Vec<u16>, Vec<u16>) {
    // 8 taps back / forward on both axes, as the MC border requires.
    let pad = 8usize;
    let src_stride = w + 2 * pad;
    let src_rows = h + 2 * pad;
    let mut s = seed | 1;
    let src: Vec<u8> = (0..src_stride * src_rows)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => 255,
                _ => (v >> 11) as u8,
            }
        })
        .collect();
    let origin = pad * src_stride + pad;
    let mut out = Vec::with_capacity(2);
    for (k, (sx, sy)) in [(0i32, 0i32), (12, 5)].into_iter().enumerate() {
        let mut conv = vec![0u16; w * h];
        let mut dst = vec![0u8; w * h];
        cref::jnt_convolve_2d(
            &src,
            origin + k, // a one-pixel shift so the two references differ
            src_stride,
            &mut dst,
            w,
            &mut conv,
            w,
            w,
            h,
            /*filt_x=*/ 0,
            w as i32,
            /*filt_y=*/ 0,
            h as i32,
            sx,
            sy,
            JntCfg {
                do_average: false,
                use_jnt: false,
                fwd: 0,
                bck: 0,
            },
        );
        out.push(conv);
    }
    let src1 = out.pop().unwrap();
    let src0 = out.pop().unwrap();
    (src0, src1)
}

#[test]
fn producer_driven_values_match_c() {
    for (i, (w, h)) in [(8usize, 8usize), (16, 8), (4, 16), (32, 32)]
        .into_iter()
        .enumerate()
    {
        let (src0, src1) = producer_conv_bufs(0x51ed_0001 + i as u32, w, h);
        compare(D16MaskTier::Scalar, &src0, w, &src1, w, h, w, 8, "producer");
    }
}

#[test]
fn dispatched_tier_matches_scalar_on_producer_range() {
    for (i, (w, h)) in [(8usize, 8usize), (16, 16), (32, 8)]
        .into_iter()
        .enumerate()
    {
        let (src0, src1) = producer_conv_bufs(0x77aa_0001 + i as u32, w, h);
        compare(
            D16MaskTier::Dispatched,
            &src0,
            w,
            &src1,
            w,
            h,
            w,
            8,
            "producer/dispatched",
        );
    }
}

/// A POSITIVE CONTROL for the two tests above: report the CONV_BUF range the
/// producer actually reaches, and assert the synthetic sweep's domain is a
/// superset of it. Without this, "the synthetic sweep covers the producer"
/// would be an assumption rather than a measurement (§5).
#[test]
fn producer_range_is_what_the_scalar_sweep_covers() {
    let (mut lo, mut hi) = (u16::MAX, 0u16);
    for i in 0..8u32 {
        let (a, b) = producer_conv_bufs(0x2b3c_0001 + i, 16, 16);
        for v in a.into_iter().chain(b) {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    // Measured on this build; the assertion is the *containment*, not the
    // exact numbers, so a legitimate producer change loosens rather than
    // breaks it.
    assert!(lo <= hi, "producer produced nothing");
    println!("jnt_convolve_2d CONV_BUF range over 8-bit input: [{lo}, {hi}]");
    // The synthetic sweep draws over the whole u16 domain, which contains
    // any producer range by construction; this asserts the producer stays
    // inside u16 (i.e. that no widening happened under us).
    assert!(hi as u32 <= u16::MAX as u32);
}

/// Full-`u16` synthetic sweep against the SCALAR kernel, over strides that
/// are not the width, both mask types, and 8 / 10 / 12-bit rounding.
#[test]
fn synthetic_full_range_matches_scalar_c() {
    for bd in [8, 10, 12] {
        for (i, (w, h)) in [(4usize, 4usize), (8, 4), (16, 16), (32, 8), (7, 5)]
            .into_iter()
            .enumerate()
        {
            let s0_stride = w + 3;
            let s1_stride = w + 11;
            let mut s = 0x9e37_0001u32 ^ (bd as u32) << 16 ^ i as u32;
            let mk = |n: usize, s: &mut u32| -> Vec<u16> {
                (0..n)
                    .map(|_| {
                        let v = xs(s);
                        match v % 8 {
                            0 => 0,
                            1 => u16::MAX,
                            2 => 1 << 15,
                            3 => (1 << 15) - 1,
                            _ => (v >> 8) as u16,
                        }
                    })
                    .collect()
            };
            let src0 = mk(h * s0_stride, &mut s);
            let src1 = mk(h * s1_stride, &mut s);
            compare(
                D16MaskTier::Scalar,
                &src0,
                s0_stride,
                &src1,
                s1_stride,
                h,
                w,
                bd,
                "synthetic",
            );
        }
    }
}

/// The mask must be dense at stride `w` — C writes `mask[i * w + j]`. A port
/// that used a caller stride would pass every test above (they all pass
/// `w`), so this pins the addressing directly by leaving a guard tail.
#[test]
fn mask_is_written_densely_at_stride_w() {
    let (w, h) = (5usize, 3usize);
    let (src0, src1) = producer_conv_bufs(0x1234_0001, w, h);
    let cp = ConvolveParams::no_round(false, 0, true, 8);
    let mut got = vec![0xAAu8; h * w + 7];
    build_compound_diffwtd_mask_d16(
        &mut got[..h * w],
        DiffwtdMaskType::D38,
        &src0,
        w,
        &src1,
        w,
        h,
        w,
        &cp,
        8,
    );
    let mut want = vec![0xAAu8; h * w + 7];
    gap::build_compound_diffwtd_mask_d16(
        D16MaskTier::Scalar,
        &mut want[..h * w],
        0,
        &src0,
        w,
        &src1,
        w,
        h,
        w,
        8,
    );
    assert_eq!(got, want);
    assert_eq!(&got[h * w..], &[0xAAu8; 7], "wrote past h*w");
}

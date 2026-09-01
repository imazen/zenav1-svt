//! Differential parity for SVT's 8-bit + 2-bit -> 10-bit pack — evidence
//! tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (all `nm -g`-visible as `T`): `svt_aom_pack_block`
//! (inter_prediction.c:26) and `svt_enc_msb_pack2_d`
//! (C_DEFAULT/pack_unpack_c.c:18). `svt_aom_pack2d_src` (pic_operators.c:341)
//! is driven indirectly and completely — it is the whole body of the first.
//!
//! # Both C arms, named rather than assumed (§5)
//!
//! `svt_aom_pack2d_src` runs an RTCD SIMD kernel when
//! `width % 4 == 0 && height % 2 == 0` and the scalar one otherwise. The port
//! has ONE implementation, so a cell that only ever hit one arm would leave
//! the other unproven. [`both_c_arms_are_reached_and_agree`] asserts which arm
//! each extent selects (a positive control on the predicate, not an
//! assumption), then compares the SIMD-selecting extents against the scalar C
//! kernel directly — so a SIMD/scalar divergence in C would fail here rather
//! than hide.

use svtav1_cref::interpred_gap::{self as gap, PackEntry};
use svtav1_dsp::port_pack::{pack_block, pack2d_takes_simd_arm};

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// Hostile bytes: the extremes plus a spread, on both planes.
fn bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0x00,
                1 => 0xFF,
                2 => 0xC0,
                3 => 0x3F,
                4 => 0x40,
                _ => (v >> 13) as u8,
            }
        })
        .collect()
}

/// Every extent worth covering: both dispatch arms, both stride regimes, and
/// the widths the SIMD kernels special-case (4/8/16/24/32/48/64/80).
const EXTENTS: &[(usize, usize)] = &[
    (4, 2),
    (4, 4),
    (8, 2),
    (8, 8),
    (16, 4),
    (16, 16),
    (20, 24),
    (24, 6),
    (32, 8),
    (48, 4),
    (64, 2),
    (72, 4),
    (80, 2),
    // Scalar-arm selectors: odd width, odd height, and both.
    (5, 4),
    (7, 3),
    (4, 3),
    (13, 5),
    (3, 1),
    (1, 1),
];

fn one_cell(entry: PackEntry, w: usize, h: usize, extra_stride: usize, seed: u32) {
    let in8_stride = w + extra_stride;
    let inn_stride = w + 2 * extra_stride + 1;
    let out_stride = w + 3 * extra_stride;
    let in8 = bytes(h * in8_stride, seed);
    let inn = bytes(h * inn_stride, seed ^ 0x5bd1_e995);
    let mut got = vec![0u16; h * out_stride];
    let mut want = vec![0u16; h * out_stride];
    pack_block(
        &in8, in8_stride, &inn, inn_stride, &mut got, out_stride, w, h,
    );
    gap::pack_block(
        entry, &in8, in8_stride, &inn, inn_stride, &mut want, out_stride, w, h,
    );
    assert_eq!(got, want, "{entry:?} {w}x{h} extra_stride {extra_stride}");
}

#[test]
fn pack_block_matches_c_on_every_extent() {
    for (i, &(w, h)) in EXTENTS.iter().enumerate() {
        for extra in [0usize, 1, 5] {
            one_cell(PackEntry::Dispatched, w, h, extra, 0x1701_0001 + i as u32);
        }
    }
}

#[test]
fn scalar_c_kernel_matches_the_port_on_every_extent() {
    for (i, &(w, h)) in EXTENTS.iter().enumerate() {
        for extra in [0usize, 1, 5] {
            one_cell(PackEntry::Scalar, w, h, extra, 0x2802_0001 + i as u32);
        }
    }
}

/// POSITIVE CONTROL for the two tests above: prove both C arms are actually
/// reached, and that C's own SIMD and scalar arms agree where the dispatch
/// would have chosen the SIMD one.
#[test]
fn both_c_arms_are_reached_and_agree() {
    let mut simd = 0usize;
    let mut scalar = 0usize;
    for &(w, h) in EXTENTS {
        if pack2d_takes_simd_arm(w, h) {
            simd += 1;
        } else {
            scalar += 1;
        }
    }
    assert!(simd >= 8, "no SIMD-arm extents: dispatch never exercised");
    assert!(scalar >= 5, "no scalar-arm extents");

    // On a SIMD-selecting extent, C's dispatched entry and C's scalar entry
    // must produce the same bytes. If they ever did not, the port could not
    // match both, and this says which side moved.
    for (i, &(w, h)) in EXTENTS.iter().enumerate() {
        if !pack2d_takes_simd_arm(w, h) {
            continue;
        }
        let (in8_stride, inn_stride, out_stride) = (w + 3, w + 7, w + 5);
        let in8 = bytes(h * in8_stride, 0x3ff0_0001 + i as u32);
        let inn = bytes(h * inn_stride, 0x4aa0_0001 + i as u32);
        let mut a = vec![0u16; h * out_stride];
        let mut b = vec![0u16; h * out_stride];
        gap::pack_block(
            PackEntry::Dispatched,
            &in8,
            in8_stride,
            &inn,
            inn_stride,
            &mut a,
            out_stride,
            w,
            h,
        );
        gap::pack_block(
            PackEntry::Scalar,
            &in8,
            in8_stride,
            &inn,
            inn_stride,
            &mut b,
            out_stride,
            w,
            h,
        );
        assert_eq!(a, b, "C SIMD vs C scalar disagree at {w}x{h}");
    }
}

/// The pack must not write outside `width` on any row — the SIMD kernels read
/// and write in 8/16/32-lane groups, so a port that rounded the width up
/// would still match on a tightly-packed buffer. A guard tail catches it.
#[test]
fn writes_nothing_past_width_on_a_row() {
    let (w, h, out_stride) = (5usize, 3usize, 16usize);
    let in8 = bytes(h * 8, 0x9111_0001);
    let inn = bytes(h * 8, 0x9222_0001);
    let mut got = vec![0xBEEFu16; h * out_stride];
    let mut want = vec![0xBEEFu16; h * out_stride];
    pack_block(&in8, 8, &inn, 8, &mut got, out_stride, w, h);
    gap::pack_block(
        PackEntry::Scalar,
        &in8,
        8,
        &inn,
        8,
        &mut want,
        out_stride,
        w,
        h,
    );
    assert_eq!(got, want);
    for y in 0..h {
        assert_eq!(
            &got[y * out_stride + w..(y + 1) * out_stride],
            &[0xBEEFu16; 11],
            "row {y} tail clobbered"
        );
    }
}

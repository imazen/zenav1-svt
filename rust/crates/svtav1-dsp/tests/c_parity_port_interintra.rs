//! Differential parity for inter-intra compound blending — evidence tier 1
//! (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven: `svt_aom_combine_interintra`,
//! `svt_aom_combine_interintra_highbd` (both `nm -g`-visible), each preceded by
//! the real `init_ii_masks` and `svt_av1_init_wedge_masks`. That makes the
//! `static` `build_smooth_interintra_mask` and `get_ii_mask` tier-1 by
//! composition: every byte the blend reads out of the II mask table is C's own,
//! and the port's independently-built table has to agree pixel for pixel.
//!
//! The C blend itself is the RTCD-dispatched `svt_aom_blend_a64_mask` /
//! `svt_aom_highbd_blend_a64_mask`, so on this host the port's scalar
//! `blend_a64_mask` is compared against a SIMD kernel.
//!
//! `enable_interintra_compound` is on for every preset <= 8 in this port's
//! sequence-header derivation (speed_config.rs:221), so this is mainstream
//! coverage, not a corner.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_interintra::{
    IiMasks, InterIntraMode, combine_interintra, combine_interintra_highbd,
};
use svtav1_dsp::port_wedge_masks::{MAX_WEDGE_TYPES, WedgeMasks, is_interintra_wedge_used};

const BLOCK_W: [usize; 22] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; 22] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// Inter-intra is allowed for 8x8..32x32; masks exist down to 4x4 for chroma.
const II_BSIZES: [usize; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

fn u8s(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => 255,
                _ => (v >> 9) as u8,
            }
        })
        .collect()
}

fn u16s(n: usize, seed: u32, bd: u32) -> Vec<u16> {
    let max = (1u32 << bd) - 1;
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => max as u16,
                _ => ((v >> 5) % (max + 1)) as u16,
            }
        })
        .collect()
}

/// The smooth (non-wedge) arm, over every inter-intra block size and mode.
#[test]
fn combine_interintra_smooth_matches_c() {
    let ii = IiMasks::new();
    let wedge = WedgeMasks::new();
    let mut cells = 0usize;
    for &bsize in &II_BSIZES {
        for &plane_bsize in &II_BSIZES {
            // C indexes ii_masks by plane_bsize; both must be in range.
            for (m, mode) in InterIntraMode::ALL.iter().enumerate() {
                let bw = BLOCK_W[plane_bsize];
                let bh = BLOCK_H[plane_bsize];
                let cs = bw + 3;
                let inter = u8s(cs * bh, 0x1234 ^ bsize as u32 ^ (m as u32) << 8);
                let intra = u8s(cs * bh, 0x5678 ^ plane_bsize as u32 ^ (m as u32) << 8);
                let mut r = vec![0u8; cs * bh];
                let mut c = vec![0u8; cs * bh];
                combine_interintra(
                    &ii,
                    &wedge,
                    *mode,
                    false,
                    0,
                    0,
                    bsize,
                    plane_bsize,
                    &mut r,
                    cs,
                    &inter,
                    cs,
                    &intra,
                    cs,
                );
                cref::combine_interintra(
                    m as i32,
                    cref::IiWedge {
                        use_wedge: false,
                        index: 0,
                        sign: 0,
                    },
                    bsize as i32,
                    plane_bsize as i32,
                    &mut c,
                    cs,
                    &inter,
                    cs,
                    &intra,
                    cs,
                );
                assert_eq!(
                    r, c,
                    "combine_interintra smooth bsize {bsize} plane {plane_bsize} mode {m}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 400, "anti-vacuity: only {cells} cells ran");
}

/// The wedge arm, including the block sizes where
/// `svt_aom_is_interintra_wedge_used` is FALSE and C writes NOTHING — the
/// destination must come back untouched on both sides, which is the arm a
/// "fall through to smooth" port gets wrong.
#[test]
fn combine_interintra_wedge_matches_c() {
    let ii = IiMasks::new();
    let wedge = WedgeMasks::new();
    let mut cells = 0usize;
    let mut skipped = 0usize;
    let mut blended = 0usize;
    for &bsize in &II_BSIZES {
        for idx in [0usize, 1, 7, 15] {
            for sign in 0..2usize {
                let plane_bsize = bsize;
                let bw = BLOCK_W[plane_bsize];
                let bh = BLOCK_H[plane_bsize];
                let cs = bw + 2;
                let inter = u8s(cs * bh, 0x9ABC ^ bsize as u32 ^ idx as u32);
                let intra = u8s(cs * bh, 0xDEF0 ^ sign as u32 ^ idx as u32);
                // Prefill with a recognisable pattern so "wrote nothing" is
                // distinguishable from "wrote zeros".
                let seed_fill = u8s(cs * bh, 0x1111 ^ bsize as u32);
                let mut r = seed_fill.clone();
                let mut c = seed_fill.clone();
                combine_interintra(
                    &ii,
                    &wedge,
                    InterIntraMode::DcPred,
                    true,
                    idx,
                    sign,
                    bsize,
                    plane_bsize,
                    &mut r,
                    cs,
                    &inter,
                    cs,
                    &intra,
                    cs,
                );
                cref::combine_interintra(
                    0,
                    cref::IiWedge {
                        use_wedge: true,
                        index: idx as i32,
                        sign: sign as i32,
                    },
                    bsize as i32,
                    plane_bsize as i32,
                    &mut c,
                    cs,
                    &inter,
                    cs,
                    &intra,
                    cs,
                );
                assert_eq!(
                    r, c,
                    "combine_interintra wedge bsize {bsize} idx {idx} sign {sign}"
                );
                if is_interintra_wedge_used(bsize) {
                    blended += 1;
                    assert_ne!(r, seed_fill, "the wedge blend must have written something");
                } else {
                    skipped += 1;
                    assert_eq!(r, seed_fill, "the no-wedge arm must write nothing");
                }
                cells += 1;
                assert!(idx < MAX_WEDGE_TYPES);
            }
        }
    }
    assert!(cells >= 80, "anti-vacuity: only {cells} cells ran");
    assert!(
        blended > 20 && skipped > 10,
        "both arms must run: {blended}/{skipped}"
    );
}

#[test]
fn combine_interintra_highbd_matches_c() {
    let ii = IiMasks::new();
    let wedge = WedgeMasks::new();
    let mut cells = 0usize;
    for bd in [8u32, 10, 12] {
        for &bsize in &II_BSIZES {
            for (m, mode) in InterIntraMode::ALL.iter().enumerate() {
                for use_wedge in [false, true] {
                    let plane_bsize = bsize;
                    let bw = BLOCK_W[plane_bsize];
                    let bh = BLOCK_H[plane_bsize];
                    let cs = bw + 1;
                    let inter = u16s(cs * bh, 0x2222 ^ bsize as u32 ^ bd, bd);
                    let intra = u16s(cs * bh, 0x3333 ^ m as u32 ^ bd, bd);
                    let fill = u16s(cs * bh, 0x4444 ^ bsize as u32, bd);
                    let mut r = fill.clone();
                    let mut c = fill.clone();
                    combine_interintra_highbd(
                        &ii,
                        &wedge,
                        *mode,
                        use_wedge,
                        3,
                        1,
                        bsize,
                        plane_bsize,
                        &mut r,
                        cs,
                        &inter,
                        cs,
                        &intra,
                        cs,
                    );
                    cref::combine_interintra_highbd(
                        m as i32,
                        cref::IiWedge {
                            use_wedge,
                            index: 3,
                            sign: 1,
                        },
                        bsize as i32,
                        plane_bsize as i32,
                        &mut c,
                        cs,
                        &inter,
                        cs,
                        &intra,
                        cs,
                        bd as i32,
                    );
                    assert_eq!(
                        r, c,
                        "combine_interintra_highbd bd{bd} bsize {bsize} mode {m} wedge {use_wedge}"
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 240, "anti-vacuity: only {cells} cells ran");
}

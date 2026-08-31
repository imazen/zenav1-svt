//! Differential parity for the masked-compound blend — evidence tier 1
//! (`WORKING-ON-THIS.md` §4).
//!
//! Symbol driven: `svt_aom_build_masked_compound_no_round` (`nm -g`-visible),
//! which dispatches through the RTCD-filled
//! `svt_aom_lowbd_blend_a64_d16_mask` / `svt_aom_highbd_blend_a64_d16_mask` —
//! so the port's scalar blends are compared against C's dispatched kernels.
//!
//! The `static` `av1_get_compound_type_mask` is gated indirectly: the WEDGE
//! arm makes C read its own initialised wedge tables, and the DIFFWTD arm
//! makes it read the caller's `seg_mask`. Both are driven.
//!
//! The `subw`/`subh` inference is exercised by calling each block size at BOTH
//! its full (luma) dimensions and its 4:2:0 chroma dimensions.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::ConvolveParams;
use svtav1_dsp::port_masked_blend::{
    InterInterCompoundData, build_masked_compound_no_round, build_masked_compound_no_round_hbd,
};
use svtav1_dsp::port_masked_compound::CompoundType;
use svtav1_dsp::port_wedge_masks::{WedgeMasks, is_interintra_wedge_used};

const BLOCK_W: [usize; 22] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; 22] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// CONV_BUF values in the range the compound kernels actually produce: the
/// round_offset for 8-bit compound is 3072, and the payload sits above it.
fn conv_buf(n: usize, seed: u32) -> Vec<u16> {
    let mut s = seed | 1;
    (0..n).map(|_| (xs(&mut s) % 40000) as u16).collect()
}

fn mask_plane(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n).map(|_| (xs(&mut s) % 65) as u8).collect()
}

/// The masked compound block sizes — the same nine wedges are allowed on, plus
/// their 4:2:0 chroma halves.
const BSIZES: [usize; 9] = [3, 4, 5, 6, 7, 8, 9, 18, 19];

#[test]
fn build_masked_compound_no_round_matches_c() {
    let wedge = WedgeMasks::new();
    let mut cells = 0usize;
    let mut wedge_cells = 0usize;
    let mut diffwtd_cells = 0usize;
    let mut sub_cells = 0usize;
    for &bsize in &BSIZES {
        assert!(is_interintra_wedge_used(bsize));
        let (bw, bh) = (BLOCK_W[bsize], BLOCK_H[bsize]);
        // Luma, then the 4:2:0 chroma halves, which is what sets subw/subh.
        for (w, h) in [(bw, bh), (bw / 2, bh / 2), (bw / 2, bh), (bw, bh / 2)] {
            if w < 4 || h < 4 {
                continue;
            }
            for comp in [
                InterInterCompoundData {
                    compound_type: CompoundType::DiffWtd,
                    wedge_index: 0,
                    wedge_sign: 0,
                },
                InterInterCompoundData {
                    compound_type: CompoundType::Wedge,
                    wedge_index: 3,
                    wedge_sign: 0,
                },
                InterInterCompoundData {
                    compound_type: CompoundType::Wedge,
                    wedge_index: 11,
                    wedge_sign: 1,
                },
            ] {
                let s0s = w + 3;
                let s1s = w + 1;
                let dst_stride = w + 2;
                let src0 = conv_buf(s0s * h, 0x1111 ^ bsize as u32 ^ w as u32);
                let src1 = conv_buf(s1s * h, 0x2222 ^ bsize as u32 ^ h as u32);
                let seg = mask_plane(bw * bh, 0x3333 ^ bsize as u32);

                let cp = ConvolveParams::no_round(false, 0, true, 8);
                let mut r = vec![0u8; dst_stride * h];
                let mut c = vec![0u8; dst_stride * h];
                build_masked_compound_no_round(
                    &mut r, dst_stride, &src0, s0s, &src1, s1s, &comp, &seg, &wedge, bsize, h, w,
                    &cp,
                );
                let mut cseg = seg.clone();
                cref::build_masked_compound_no_round(
                    &mut c,
                    dst_stride,
                    &src0,
                    s0s,
                    &src1,
                    s1s,
                    cref::RefCompoundData {
                        compound_type: comp.compound_type as i32,
                        wedge_index: comp.wedge_index as i32,
                        wedge_sign: comp.wedge_sign as i32,
                        mask_type: 0,
                    },
                    &mut cseg,
                    bsize as i32,
                    h,
                    w,
                    8,
                    true,
                    false,
                );
                assert_eq!(
                    r, c,
                    "masked blend bsize {bsize} {w}x{h} {:?}",
                    comp.compound_type
                );
                match comp.compound_type {
                    CompoundType::Wedge => wedge_cells += 1,
                    _ => diffwtd_cells += 1,
                }
                if (w, h) != (bw, bh) {
                    sub_cells += 1;
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 80, "anti-vacuity: only {cells} cells ran");
    assert!(
        wedge_cells > 20 && diffwtd_cells > 10,
        "both mask sources must run"
    );
    assert!(
        sub_cells > 20,
        "the subsampled arms ran only {sub_cells} times"
    );
}

#[test]
fn build_masked_compound_no_round_hbd_matches_c() {
    let wedge = WedgeMasks::new();
    let mut cells = 0usize;
    // MEASURED 2026-08-31, the same oracle limit c_parity_port_inter_predictor
    // records for svt_aom_convolveHbd: at bd 12 C's RTCD-DISPATCHED
    // svt_aom_highbd_blend_a64_d16_mask saturates at 255 rather than 4095,
    // while its own `_c` kernel saturates at 4095 (d16_blend_matches_pure_c
    // sweeps bd 12 against `_c` and is green). SVT-AV1 encodes 8/10-bit only,
    // so its highbd SIMD kernels are not a valid reference at 12. bd 12
    // coverage lives in the `_c`-driven cell; this dispatched one stops at 10.
    for bd in [8i32, 10] {
        for &bsize in &BSIZES {
            let (bw, bh) = (BLOCK_W[bsize], BLOCK_H[bsize]);
            for (w, h) in [(bw, bh), (bw / 2, bh / 2)] {
                if w < 4 || h < 4 {
                    continue;
                }
                for comp in [
                    InterInterCompoundData {
                        compound_type: CompoundType::DiffWtd,
                        wedge_index: 0,
                        wedge_sign: 0,
                    },
                    InterInterCompoundData {
                        compound_type: CompoundType::Wedge,
                        wedge_index: 7,
                        wedge_sign: 1,
                    },
                ] {
                    let s0s = w + 2;
                    let s1s = w + 4;
                    let dst_stride = w + 1;
                    let src0 = conv_buf(s0s * h, 0x4444 ^ bsize as u32 ^ bd as u32);
                    let src1 = conv_buf(s1s * h, 0x5555 ^ bsize as u32 ^ bd as u32);
                    let seg = mask_plane(bw * bh, 0x6666 ^ bsize as u32);
                    let cp = ConvolveParams::no_round(false, 0, true, bd);

                    let mut r = vec![0u16; dst_stride * h];
                    build_masked_compound_no_round_hbd(
                        &mut r, dst_stride, &src0, s0s, &src1, s1s, &comp, &seg, &wedge, bsize, h,
                        w, &cp, bd,
                    );

                    let mut c_bytes = vec![0u8; dst_stride * h * 2];
                    let mut cseg = seg.clone();
                    cref::build_masked_compound_no_round(
                        &mut c_bytes,
                        dst_stride,
                        &src0,
                        s0s,
                        &src1,
                        s1s,
                        cref::RefCompoundData {
                            compound_type: comp.compound_type as i32,
                            wedge_index: comp.wedge_index as i32,
                            wedge_sign: comp.wedge_sign as i32,
                            mask_type: 0,
                        },
                        &mut cseg,
                        bsize as i32,
                        h,
                        w,
                        bd,
                        true,
                        true,
                    );
                    let c: Vec<u16> = c_bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|p| u16::from_le_bytes(*p))
                        .collect();
                    assert_eq!(r, c, "masked blend hbd bd{bd} bsize {bsize} {w}x{h}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 72, "anti-vacuity: only {cells} cells ran");
}

/// Isolation cell: the port's blend against the PURE-C kernel, called directly
/// rather than through RTCD. If this passes and the dispatched cells above do
/// not, the difference is C's own SIMD tier, not the port (the shape
/// `docs/SUSPECTED-C-BUGS.md` #9 records).
#[test]
fn d16_blend_matches_pure_c() {
    let mut cells = 0usize;
    for bd in [8i32, 10, 12] {
        for (w, h) in [(4usize, 4usize), (8, 8), (16, 8), (8, 16), (32, 32)] {
            for subw in [false, true] {
                for subh in [false, true] {
                    let s0s = w + 3;
                    let s1s = w + 1;
                    let ds = w + 2;
                    let ms = if subw { 2 * w } else { w } + 4;
                    let mrows = if subh { 2 * h } else { h };
                    let src0 = conv_buf(s0s * h, 0x7777 ^ w as u32 ^ bd as u32);
                    let src1 = conv_buf(s1s * h, 0x8888 ^ h as u32 ^ bd as u32);
                    let mask = mask_plane(ms * mrows, 0x9999 ^ w as u32);
                    let cp = ConvolveParams::no_round(false, 0, true, bd);

                    if bd == 8 {
                        let mut r = vec![0u8; ds * h];
                        let mut c = vec![0u8; ds * h];
                        svtav1_dsp::port_masked_blend::lowbd_blend_a64_d16_mask(
                            &mut r, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, &cp,
                        );
                        cref::lowbd_blend_a64_d16_mask_c(
                            &mut c, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, bd,
                        );
                        assert_eq!(r, c, "lowbd d16 blend {w}x{h} sub({subw},{subh})");
                    }
                    let mut r = vec![0u16; ds * h];
                    let mut c = vec![0u16; ds * h];
                    svtav1_dsp::port_masked_blend::highbd_blend_a64_d16_mask(
                        &mut r, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, &cp, bd,
                    );
                    cref::highbd_blend_a64_d16_mask_c(
                        &mut c, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, bd,
                    );
                    assert_eq!(r, c, "highbd d16 blend bd{bd} {w}x{h} sub({subw},{subh})");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 60, "anti-vacuity: only {cells} cells ran");
}

/// ATTRIBUTION cell: C's RTCD-dispatched lowbd d16 blend against C's own `_c`
/// kernel, on the exact inputs the driver cells use. This compares C with C —
/// it says nothing about the port, and exists only to attribute a driver
/// mismatch.
#[test]
fn c_rtcd_blend_vs_c_scalar_blend() {
    let mut differing = 0usize;
    let mut cells = 0usize;
    for (w, h) in [(4usize, 4usize), (8, 8), (16, 8), (8, 16), (32, 32)] {
        for subw in [false, true] {
            for subh in [false, true] {
                let s0s = w + 3;
                let s1s = w + 1;
                let ds = w + 2;
                let ms = if subw { 2 * w } else { w } + 4;
                let mrows = if subh { 2 * h } else { h };
                let src0 = conv_buf(s0s * h, 0xAAAA ^ w as u32);
                let src1 = conv_buf(s1s * h, 0xBBBB ^ h as u32);
                let mask = mask_plane(ms * mrows, 0xCCCC ^ w as u32);
                let mut a = vec![0u8; ds * h];
                let mut b = vec![0u8; ds * h];
                cref::lowbd_blend_a64_d16_mask_c(
                    &mut a, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, 8,
                );
                cref::lowbd_blend_a64_d16_mask_rtcd(
                    &mut b, ds, &src0, s0s, &src1, s1s, &mask, ms, w, h, subw, subh, 8,
                );
                if a != b {
                    differing += 1;
                }
                cells += 1;
            }
        }
    }
    println!("C rtcd vs C scalar d16 blend: {differing} of {cells} cells differ");
    assert_eq!(
        differing, 0,
        "C's dispatched d16 blend disagrees with its own _c kernel on {differing} of {cells} cells"
    );
}

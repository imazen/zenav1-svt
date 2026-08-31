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

/// The magnitude a CONV_BUF entry can carry at `bd`, exclusive.
///
/// `svt_av1_jnt_convolve_2d_c` (inter_prediction.c:557-565) builds its vertical
/// sum around `offset_bits = bd + 2 * FILTER_BITS - round_0`, asserts
/// `0 <= sum < 1 << (offset_bits + 2)`, and stores
/// `ROUND_POWER_OF_TWO(sum, round_1)` — so a stored entry is below
/// `1 << (bd + 2 * FILTER_BITS - round_0 + 2 - round_1)`, i.e. `1 << (bd + 6)`
/// at the compound rounding (`round_0 = 3`, `round_1 = 7`). At bd 8 that is
/// 16384; at bd >= 10 the bound reaches the width of the `uint16_t` the buffer
/// is made of, so it saturates there.
/// `compound_conv_buf_stays_inside_the_blend_domain` re-derives this by
/// driving C's own convolve rather than trusting the arithmetic above.
///
/// **This bound is load-bearing for every DISPATCHED cell.** C's x86
/// SSE4.1/AVX2 lowbd d16 blends multiply through `_mm_madd_epi16`
/// (`blend_sse4.h:188`), which reads a CONV_BUF entry as a SIGNED int16, so
/// they leave their own `_c` twin at exactly 32768 — MEASURED, and recorded as
/// `docs/SUSPECTED-C-BUGS.md` #19. aarch64's NEON kernel is unsigned end to end
/// (`vmull_u16`/`vmlal_u16`/`vqsubq_u16`, blend_a64_mask_neon.c:208) and agrees
/// with `_c` over the whole u16 domain. A generator above 32767 is therefore
/// out of contract, and reads as "C disagrees with itself" on x86 while
/// staying green on aarch64.
fn conv_domain(bd: i32) -> u32 {
    (1u32 << (bd + 6)).min(1 << 16)
}

/// CONV_BUF values uniform over `[0, limit)`.
fn conv_buf(n: usize, seed: u32, limit: u32) -> Vec<u16> {
    let mut s = seed | 1;
    (0..n).map(|_| (xs(&mut s) % limit) as u16).collect()
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
                let dom = conv_domain(8);
                let src0 = conv_buf(s0s * h, 0x1111 ^ bsize as u32 ^ w as u32, dom);
                let src1 = conv_buf(s1s * h, 0x2222 ^ bsize as u32 ^ h as u32, dom);
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
    // MEASURED 2026-08-31 and RE-MEASURED cross-ISA the same day: at bd 12 the
    // RTCD-dispatched svt_aom_highbd_blend_a64_d16_mask saturates at 255 rather
    // than 4095 ON aarch64 ONLY. Root: svt_aom_highbd_blend_a64_d16_mask_neon
    // (highbd_blend_a64_mask_neon.c:453-459) branches `bd == 10 ? 10-bit :
    // 8-bit`, so every other depth — 12 included — takes the 8-BIT kernel.
    // x86's AVX2/SSE4.1 highbd kernel is faithful at bd 12 (308/308 cells,
    // full-u16 magnitudes). The earlier note here said this was a property of
    // "the dispatched kernel"; it is a property of the NEON one, and the
    // correction is recorded in docs/SUSPECTED-C-BUGS.md #20.
    // SVT-AV1 encodes 8/10-bit only, so bd 12 is outside the shipping envelope
    // on both ISAs; bd 12 coverage lives in the `_c`-driven cell, and this
    // dispatched one stops at 10 so the sweep means the same thing everywhere.
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
                    let dom = conv_domain(bd);
                    let src0 = conv_buf(s0s * h, 0x4444 ^ bsize as u32 ^ bd as u32, dom);
                    let src1 = conv_buf(s1s * h, 0x5555 ^ bsize as u32 ^ bd as u32, dom);
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
/// `docs/SUSPECTED-C-BUGS.md` #9 records, and #19 is the instance this file
/// hit). It is also the file's WIDEST cell: `_c` and the port both read a
/// CONV_BUF entry unsigned, so this one sweeps the full `u16` domain and every
/// bit depth, where the dispatched cells are bounded by what a CONV_BUF can
/// actually hold.
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
                    // Not dispatched: both sides read a CONV_BUF entry as
                    // unsigned, so this cell sweeps the FULL u16 domain rather
                    // than the encoder-reachable one the dispatched cells use.
                    let src0 = conv_buf(s0s * h, 0x7777 ^ w as u32 ^ bd as u32, 1 << 16);
                    let src1 = conv_buf(s1s * h, 0x8888 ^ h as u32 ^ bd as u32, 1 << 16);
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
///
/// It runs at [`conv_domain`] magnitudes because that is the domain the
/// encoder can actually put in a CONV_BUF; above it the two C kernels are
/// genuinely allowed to differ on x86 (see `conv_domain`'s note), and
/// [`c_lowbd_d16_blend_domain_covers_the_conv_buf_contract`] measures where
/// the agreement ends rather than asserting it.
#[test]
fn c_rtcd_blend_vs_c_scalar_blend() {
    let mut differing = 0usize;
    let mut cells = 0usize;
    for (bw, bh) in BLOCK_W.iter().zip(BLOCK_H.iter()).map(|(&w, &h)| (w, h)) {
        for (w, h) in [(bw, bh), (bw / 2, bh / 2), (bw / 2, bh), (bw, bh / 2)] {
            if w < 4 || h < 4 {
                continue;
            }
            for (subw, subh) in [(false, false), (false, true), (true, false), (true, true)] {
                let s0s = w + 3;
                let s1s = w + 1;
                let ds = w + 2;
                let ms = if subw { 2 * w } else { w } + 4;
                let mrows = if subh { 2 * h } else { h };
                let dom = conv_domain(8);
                let src0 = conv_buf(s0s * h, 0xAAAA ^ w as u32, dom);
                let src1 = conv_buf(s1s * h, 0xBBBB ^ h as u32, dom);
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
    assert!(cells >= 300, "anti-vacuity: only {cells} shapes ran");
    assert_eq!(
        differing, 0,
        "C's dispatched d16 blend disagrees with its own _c kernel on {differing} of {cells} cells"
    );
}

/// ATTRIBUTION cell for the HIGHBD blend, and the file's per-ISA COVERAGE
/// measurement: which bit depths this host's dispatched kernel is faithful on.
///
/// MEASURED 2026-08-31 over 308 shapes at full-`u16` magnitudes:
///
/// | bd | aarch64 | x86-64 |
/// |----|---------|--------|
/// | 8  | 308/308 | 308/308 |
/// | 10 | 308/308 | 308/308 |
/// | 12 | **0/308** — takes the 8-bit kernel | 308/308 |
///
/// So the assertion is depth-shaped, not host-shaped: 8 and 10 (SVT-AV1's whole
/// shipping envelope, enc_settings.c:460) must be faithful everywhere, and at
/// 12 the dispatched kernel must be EITHER faithful OR exactly the `_c` result
/// computed at bd 8 — the NEON fall-through of `docs/SUSPECTED-C-BUGS.md` #20.
/// A third behaviour at 12, or any regression at 8/10, fails here.
#[test]
fn c_highbd_d16_blend_vs_c_scalar_blend() {
    let mut cells = 0usize;
    let mut bd12_faithful = 0usize;
    let mut bd12_is_the_8bit_kernel = 0usize;
    for bd in [8i32, 10, 12] {
        for (bw, bh) in BLOCK_W.iter().zip(BLOCK_H.iter()).map(|(&w, &h)| (w, h)) {
            for (w, h) in [(bw, bh), (bw / 2, bh / 2)] {
                if w < 4 || h < 4 {
                    continue;
                }
                for (subw, subh) in [(false, false), (true, true)] {
                    let ms = if subw { 2 * w } else { w } + 4;
                    let mrows = if subh { 2 * h } else { h };
                    let ds = w + 2;
                    let src0 = conv_buf(w * h, 0xD16D ^ w as u32 ^ (bd as u32) << 9, 1 << 16);
                    let src1 = conv_buf(w * h, 0xB1E4 ^ h as u32 ^ (bd as u32) << 9, 1 << 16);
                    let mask = mask_plane(ms * mrows, 0xEEEE ^ w as u32);
                    let mut sc = vec![0u16; ds * h];
                    let mut rt = vec![0u16; ds * h];
                    cref::highbd_blend_a64_d16_mask_c(
                        &mut sc, ds, &src0, w, &src1, w, &mask, ms, w, h, subw, subh, bd,
                    );
                    cref::highbd_blend_a64_d16_mask_rtcd(
                        &mut rt, ds, &src0, w, &src1, w, &mask, ms, w, h, subw, subh, bd,
                    );
                    if bd == 12 {
                        if rt == sc {
                            bd12_faithful += 1;
                        } else {
                            let mut as8 = vec![0u16; ds * h];
                            cref::highbd_blend_a64_d16_mask_c(
                                &mut as8, ds, &src0, w, &src1, w, &mask, ms, w, h, subw, subh, 8,
                            );
                            assert_eq!(
                                rt, as8,
                                "bd 12 dispatched highbd blend {w}x{h} sub({subw},{subh}) is \
                                 neither its own _c kernel nor the bd-8 one"
                            );
                            bd12_is_the_8bit_kernel += 1;
                        }
                    } else {
                        assert_eq!(
                            rt, sc,
                            "dispatched highbd blend bd{bd} {w}x{h} sub({subw},{subh})"
                        );
                    }
                    cells += 1;
                }
            }
        }
    }
    println!(
        "highbd d16 blend at bd 12: {bd12_faithful} faithful, \
         {bd12_is_the_8bit_kernel} take the 8-bit kernel"
    );
    assert!(cells >= 200, "anti-vacuity: only {cells} cells ran");
    assert!(
        bd12_faithful + bd12_is_the_8bit_kernel > 60,
        "the bd 12 arm ran only {} times",
        bd12_faithful + bd12_is_the_8bit_kernel
    );
}

/// PREMISE cell: the CONV_BUF magnitudes the dispatched cells are driven at are
/// the ones C's own compound convolve produces — measured, not assumed.
///
/// [`conv_domain`] derives its bound from `svt_av1_jnt_convolve_2d_c`'s assert.
/// This drives that very function over every interp filter x subpel phase and
/// asserts every stored entry lands inside the bound, so the generator can
/// never quietly drift outside the domain the SIMD kernels are written for.
#[test]
fn compound_conv_buf_stays_inside_the_blend_domain() {
    const PAD: usize = 8;
    let bound = conv_domain(8);
    let mut hi = 0u16;
    let mut lo = u16::MAX;
    let mut cells = 0usize;
    for filt in 0..4i32 {
        for (w, h) in [(4usize, 4usize), (8, 8), (16, 8), (16, 16), (64, 64)] {
            for sx in [0i32, 1, 4, 8, 15] {
                for sy in [0i32, 1, 7, 15] {
                    let stride = w + 2 * PAD;
                    let rows = h + 2 * PAD;
                    // Extremal 8-bit source: a 0/255 checkerboard drives the
                    // filter taps to their sign extremes, which is where the
                    // stored value is largest and smallest.
                    let mut src = vec![0u8; stride * rows];
                    let mut s =
                        (0x1234_5678u32) ^ (filt as u32) << 8 ^ (w as u32) ^ (sx as u32) << 3;
                    for (i, px) in src.iter_mut().enumerate() {
                        *px = if xs(&mut s) & 1 == 0 {
                            0
                        } else if i % 3 == 0 {
                            255
                        } else {
                            (xs(&mut s) >> 5) as u8
                        };
                    }
                    let origin = PAD * stride + PAD;
                    let mut cb = vec![0u16; w * h];
                    let mut dst = vec![0u8; w * h];
                    cref::jnt_convolve_2d(
                        &src,
                        origin,
                        stride,
                        &mut dst,
                        w,
                        &mut cb,
                        w,
                        w,
                        h,
                        filt,
                        w as i32,
                        filt,
                        h as i32,
                        sx,
                        sy,
                        cref::JntCfg {
                            do_average: false,
                            use_jnt: false,
                            fwd: 0,
                            bck: 0,
                        },
                    );
                    for &v in &cb {
                        hi = hi.max(v);
                        lo = lo.min(v);
                    }
                    cells += 1;
                }
            }
        }
    }
    println!("C compound CONV_BUF range over {cells} cells: [{lo}, {hi}], domain bound {bound}");
    assert!(cells >= 400, "anti-vacuity: only {cells} cells ran");
    assert!(hi > 0, "anti-vacuity: the convolve produced nothing");
    assert!(
        u32::from(hi) < bound,
        "C's compound convolve produced {hi}, at or above the {bound} the \
         dispatched-blend cells are generated inside — conv_domain is wrong"
    );
}

/// DOMAIN cell: how far C's dispatched lowbd d16 blend agrees with its own `_c`
/// kernel, MEASURED per host rather than asserted.
///
/// The assertion is the contract, not the ISA: the agreement must cover every
/// value a CONV_BUF can hold ([`conv_domain`]). The measured limit above that
/// is host-dependent and is only printed —
///
/// * aarch64 (NEON, unsigned throughout): no divergence anywhere in u16.
/// * x86-64 (SSE4.1/AVX2, `_mm_madd_epi16` = signed int16): first divergence at
///   exactly 32768, twice the contract bound.
///
/// So this fails the day either kernel's usable domain shrinks below what the
/// encoder can feed it, and it keeps recording the number instead of leaving
/// the next reader to re-derive it.
#[test]
fn c_lowbd_d16_blend_domain_covers_the_conv_buf_contract() {
    let bound = conv_domain(8);
    let mut cells = 0usize;
    for (w, h, subw, subh) in [
        (4usize, 4usize, false, false),
        (8, 8, false, false),
        (8, 8, true, true),
        (16, 8, false, true),
    ] {
        let ms = if subw { 2 * w } else { w } + 4;
        let mrows = if subh { 2 * h } else { h };
        let ds = w + 2;
        let mask: Vec<u8> = (0..ms * mrows).map(|i| (i % 65) as u8).collect();
        let mut first: Option<u32> = None;
        for v in 0..=u32::from(u16::MAX) {
            let src0 = vec![v as u16; w * h];
            let src1 = vec![(v / 2) as u16; w * h];
            let mut a = vec![0u8; ds * h];
            let mut b = vec![0u8; ds * h];
            cref::lowbd_blend_a64_d16_mask_c(
                &mut a, ds, &src0, w, &src1, w, &mask, ms, w, h, subw, subh, 8,
            );
            cref::lowbd_blend_a64_d16_mask_rtcd(
                &mut b, ds, &src0, w, &src1, w, &mask, ms, w, h, subw, subh, 8,
            );
            if a != b {
                first = Some(v);
                break;
            }
        }
        match first {
            None => {
                println!("  {w}x{h} sub({subw},{subh}): dispatched == _c over the whole u16 domain")
            }
            Some(v) => println!("  {w}x{h} sub({subw},{subh}): first divergence at {v}"),
        }
        assert!(
            first.is_none_or(|v| v >= bound),
            "C's dispatched lowbd d16 blend leaves its _c kernel at {first:?}, inside the \
             {bound} a CONV_BUF entry can hold — the dispatched cells in this file are no \
             longer driven on a domain both kernels agree on"
        );
        cells += 1;
    }
    assert_eq!(cells, 4, "anti-vacuity: only {cells} cells ran");
}

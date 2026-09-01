//! Differential parity for the 16-bit padding / plane-copy / widening
//! kernels of `Codec/pic_operators.c` vs the real exported C symbols —
//! evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! The 8-bit padding twins (`svt_aom_generate_padding`,
//! `pad_input_picture`) landed earlier with their own differential; these
//! are the 10-bit-pipeline forms, which had none.
//!
//! Padding is compared over the WHOLE allocation, not the active area, so a
//! port that padded the right region but wrote the wrong number of columns
//! into the border fails here. Both of C's easy-to-miss asymmetries are in
//! the comparison: `generate_padding16_bit` copies `src_stride` samples
//! vertically (carrying the trailing stride bytes) while
//! `pad_input_picture_16bit` copies only `width + pad_right`.

use svtav1_cref::pic_operators as cref_po;
use svtav1_dsp::pic_operators as po;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Frame-ish shapes, including odd and non-multiple-of-8 dimensions.
const SHAPES: &[(usize, usize)] = &[
    (1, 1),
    (4, 4),
    (16, 9),
    (33, 17),
    (64, 64),
    (67, 35),
    (128, 72),
];

#[test]
fn generate_padding16_bit_matches_c() {
    let mut rng = Rng(0x9E37_79B9);
    for &(w, h) in SHAPES {
        for &(pad_w, pad_h) in &[(1usize, 1usize), (8, 8), (16, 4), (32, 32)] {
            // Allocation: padding_width columns of left border, the active
            // rows, then whatever the stride adds, and padding_height rows
            // of border above and below.
            let stride = w + 2 * pad_w + 7;
            let origin = pad_h * stride + pad_w;
            let total = origin + (h + pad_h) * stride + stride;

            let mut mine: Vec<u16> = (0..total).map(|_| (rng.next() % 1024) as u16).collect();
            let mut theirs = mine.clone();

            po::generate_padding_16bit(&mut mine, origin, stride, w, h, pad_w, pad_h);
            cref_po::generate_padding16_bit(&mut theirs, origin, stride, w, h, pad_w, pad_h);

            assert_eq!(
                mine, theirs,
                "generate_padding16 {w}x{h} pad {pad_w}/{pad_h} stride {stride}"
            );
        }
    }
}

#[test]
fn pad_input_picture_16bit_matches_c() {
    let mut rng = Rng(0x1BAD_C0DE);
    for &(w, h) in SHAPES {
        for &(pr, pb) in &[(0usize, 0usize), (1, 0), (0, 1), (7, 3), (16, 16)] {
            let stride = w + pr + 5;
            let total = (h + pb) * stride + stride;
            let mut mine: Vec<u16> = (0..total).map(|_| (rng.next() % 4096) as u16).collect();
            let mut theirs = mine.clone();

            po::pad_input_picture_16bit(&mut mine, stride, w, h, pr, pb);
            cref_po::pad_input_picture_16bit(&mut theirs, stride, w, h, pr, pb);

            assert_eq!(
                mine, theirs,
                "pad_input_picture16 {w}x{h} pad {pr}/{pb} stride {stride}"
            );
        }
    }
}

#[test]
fn convert_8bit_to_16bit_matches_c() {
    let mut rng = Rng(0x0C0F_FEE0);
    for &(w, h) in SHAPES {
        let src_stride = w + 3;
        let dst_stride = w + 11;
        let src: Vec<u8> = (0..src_stride * h).map(|_| rng.next() as u8).collect();
        let mut mine = vec![0xAAAAu16; dst_stride * h + dst_stride];
        let mut theirs = mine.clone();

        po::convert_8bit_to_16bit(&src, src_stride, &mut mine, dst_stride, w, h);
        cref_po::convert_8bit_to_16bit(&src, src_stride, &mut theirs, dst_stride, w, h);

        assert_eq!(mine, theirs, "convert8to16 {w}x{h}");
    }
}

#[test]
fn yv12_copy_plane_matches_c_on_both_bit_depths_and_all_three_planes() {
    let mut rng = Rng(0xDEAD_BEE5);
    for &(w, h) in SHAPES {
        let src_stride = w + 9;
        let dst_stride = w + 2;
        for plane in 0..3usize {
            let src8: Vec<u8> = (0..src_stride * h).map(|_| rng.next() as u8).collect();
            let mut mine8 = vec![0x5Au8; dst_stride * h + dst_stride];
            let mut theirs8 = mine8.clone();
            po::yv12_copy_plane(&src8, src_stride, &mut mine8, dst_stride, w, h);
            cref_po::yv12_copy_plane_8(plane, &src8, src_stride, &mut theirs8, dst_stride, w, h);
            assert_eq!(mine8, theirs8, "yv12 copy 8bit plane {plane} {w}x{h}");

            let src16: Vec<u16> = (0..src_stride * h).map(|_| rng.next() as u16).collect();
            let mut mine16 = vec![0x5A5Au16; dst_stride * h + dst_stride];
            let mut theirs16 = mine16.clone();
            po::yv12_copy_plane(&src16, src_stride, &mut mine16, dst_stride, w, h);
            cref_po::yv12_copy_plane_16(plane, &src16, src_stride, &mut theirs16, dst_stride, w, h);
            assert_eq!(mine16, theirs16, "yv12 copy 16bit plane {plane} {w}x{h}");
        }
    }
}

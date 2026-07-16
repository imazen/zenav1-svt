//! Differential parity: sc-detection leaf primitives vs the C reference
//! (svt_av1_count_colors_with_threshold / find_dominant_value /
//! dilate_block, pic_analysis_process.c), plus behavior tests of the
//! ported AA-aware detector on constructed planes.
//!
//! The detector itself (`svt_aom_is_screen_content_antialiasing_aware`)
//! is static in C and reads a PCS, so its port is validated two ways:
//! primitive-level FFI parity here, and end-to-end via the encoder
//! identity harness once sc_class5 consumers land (#71 items 5-6).

use svtav1_cref as cref;
use svtav1_encoder::sc_detect;

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
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Fill a rows x cols block (stride >= cols) drawing from a palette of
/// `ncolors` random values — exercises both sides of every threshold.
fn fill_paletted(rng: &mut Rng, buf: &mut [u8], stride: usize, rows: usize, cols: usize, ncolors: usize) {
    let palette: Vec<u8> = (0..ncolors).map(|_| rng.byte()).collect();
    for r in 0..rows {
        for c in 0..cols {
            buf[r * stride + c] = palette[rng.below(ncolors as u64) as usize];
        }
    }
}

#[test]
fn count_colors_matches_c() {
    let mut rng = Rng(0x5c_de7ec7_0001);
    // (rows, cols, stride) incl. non-tight strides.
    let shapes = [(8usize, 8usize, 8usize), (16, 16, 16), (8, 8, 23), (16, 16, 31)];
    for &(rows, cols, stride) in &shapes {
        let mut buf = vec![0u8; rows * stride];
        for ncolors in [1usize, 2, 3, 4, 5, 6, 8, 16, 39, 40, 41, 64, 200] {
            for thresh in [1i32, 4, 6, 8, 40] {
                for _ in 0..20 {
                    fill_paletted(&mut rng, &mut buf, stride, rows, cols, ncolors);
                    let (ok_r, n_r) =
                        sc_detect::count_colors_with_threshold(&buf, stride, rows, cols, thresh);
                    let (ok_c, n_c) =
                        cref::count_colors_with_threshold(&buf, stride, rows, cols, thresh);
                    assert_eq!(
                        (ok_r, n_r),
                        (ok_c, n_c),
                        "count_colors {rows}x{cols}s{stride} ncolors={ncolors} thresh={thresh}"
                    );
                }
            }
        }
    }
}

#[test]
fn dominant_value_matches_c() {
    let mut rng = Rng(0xd0_317a17_0002);
    let shapes = [(8usize, 8usize, 8usize), (16, 16, 16), (16, 16, 29)];
    for &(rows, cols, stride) in &shapes {
        let mut buf = vec![0u8; rows * stride];
        for ncolors in [1usize, 2, 3, 4, 8, 40, 256] {
            for _ in 0..50 {
                fill_paletted(&mut rng, &mut buf, stride, rows, cols, ncolors);
                let r = sc_detect::find_dominant_value(&buf, stride, rows, cols);
                let c = cref::find_dominant_value(&buf, stride, rows, cols);
                // Tie semantics (first scan-order value to REACH the max
                // count wins, strict `>`) must match exactly.
                assert_eq!(r, c, "dominant {rows}x{cols}s{stride} ncolors={ncolors}");
            }
        }
    }
}

#[test]
fn dilate_block_matches_c() {
    let mut rng = Rng(0xd11a7e_0003);
    let shapes = [(8usize, 8usize), (16, 16)];
    for &(rows, cols) in &shapes {
        // C call sites use src at picture stride, dilated at tight blk_w
        // stride; fuzz both tight and loose strides on both sides.
        for &(src_stride, dst_stride) in &[(cols, cols), (cols + 11, cols), (cols + 3, cols + 7)] {
            let mut src = vec![0u8; rows * src_stride];
            let mut d_r = vec![0u8; rows * dst_stride];
            let mut d_c = vec![0u8; rows * dst_stride];
            for ncolors in [2usize, 3, 5, 8, 12, 40] {
                for _ in 0..40 {
                    fill_paletted(&mut rng, &mut src, src_stride, rows, cols, ncolors);
                    d_r.fill(0xAA);
                    d_c.fill(0xAA);
                    sc_detect::dilate_block(&src, src_stride, &mut d_r, dst_stride, rows, cols);
                    cref::dilate_block(&src, src_stride, &mut d_c, dst_stride, rows, cols);
                    assert_eq!(
                        d_r, d_c,
                        "dilate {rows}x{cols} src_s={src_stride} dst_s={dst_stride} ncolors={ncolors}"
                    );
                }
            }
        }
    }
}

/// The variance primitive has no directly-linkable C symbol wrapper (the C
/// path goes through the mefn_ptr vf table), but its formula is fixed:
/// Σ(x-128)² - (Σ(x-128))²/N, rounded-shifted by log2(N). Pin it with
/// hand-computed cases so any future refactor that breaks the constant-128
/// reference or the truncating division is caught.
#[test]
fn variance_formula_pinned() {
    // All-128 block: var 0.
    let flat = [128u8; 256];
    assert_eq!(sc_detect::sby_perpixel_variance(&flat, 16, 16, 16), 0);
    // All-zero 8x8: sum=-8192... per-block: diff=-128 each, sse=64*16384=1048576,
    // sum=-8192, var = 1048576 - 8192*8192/64 = 0; rounded >>6 = 0.
    let zeros = [0u8; 64];
    assert_eq!(sc_detect::sby_perpixel_variance(&zeros, 8, 8, 8), 0);
    // Half 0 / half 255 8x8 (checker rows): diffs -128/+127.
    // sse = 32*16384 + 32*16129 = 524288+516128 = 1040416; sum = 32*(-128+127) = -32;
    // var = 1040416 - (1024/64=16) = 1040400; (1040400+32)>>6 = 16256 (trunc 16256.75).
    let mut checker = [0u8; 64];
    for r in 0..8 {
        for c in 0..8 {
            checker[r * 8 + c] = if r % 2 == 0 { 0 } else { 255 };
        }
    }
    assert_eq!(sc_detect::sby_perpixel_variance(&checker, 8, 8, 8), 16256);
}

/// Detector-level behavior on constructed planes: a flat photo-like plane
/// must classify all-false; a synthetic "screen" plane (2-color text-like
/// blocks with high variance everywhere, all four quadrants) must raise
/// sc_class5 in both full and checkerboard scan modes.
#[test]
fn detector_classes_on_constructed_planes() {
    let (w, h) = (128usize, 128usize);
    // 1) Smooth gradient -> photo/solid blocks only -> all classes false.
    let mut grad = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            grad[r * w + c] = ((r + c) / 2) as u8;
        }
    }
    for fast in [false, true] {
        let cls = sc_detect::is_screen_content_antialiasing_aware(&grad, w, w, h, fast);
        assert_eq!(cls, sc_detect::ScClasses::default(), "gradient fast={fast}");
    }
    // 2) Two-value checkerboard at 4px period: every 8x8/16x16 block has
    // exactly 2 colors and huge variance -> palette+intrabc everywhere ->
    // every class true (pass=4 quadrants).
    let mut screen = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            screen[r * w + c] = if ((r / 4) + (c / 4)) % 2 == 0 { 16 } else { 240 };
        }
    }
    for fast in [false, true] {
        let cls = sc_detect::is_screen_content_antialiasing_aware(&screen, w, w, h, fast);
        assert!(
            cls.sc_class0 && cls.sc_class1 && cls.sc_class2 && cls.sc_class3 && cls.sc_class4 && cls.sc_class5,
            "screen plane fast={fast}: {cls:?}"
        );
    }
}

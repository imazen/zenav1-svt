//! Differential parity: sc-detection leaf primitives vs the C reference
//! (svt_av1_count_colors_with_threshold / find_dominant_value /
//! dilate_block, pic_analysis_process.c), plus behavior tests of the
//! ported AA-aware detector on constructed planes.
//!
//! CORRECTED 2026-08-31: this header used to say the detector itself
//! "is static in C and reads a PCS, so its port is validated two ways:
//! primitive-level FFI parity here, and end-to-end via the encoder identity
//! harness". The first half was wrong. `nm Bin/Release/libSvtAv1Enc.a` reports
//! `T _svt_aom_is_screen_content_antialiasing_aware` — an EXPORTED symbol,
//! reachable at tier 1 through a PCS facade the way
//! `pad_and_decimate_filtered_pic` already is. Both detectors are now driven
//! whole (`is_screen_content_antialiasing_aware_matches_c`,
//! `is_screen_content_matches_c`); the primitive-level tests below stay useful
//! for localizing a failure but are no longer the strongest evidence.

use svtav1_cref as cref;
use svtav1_cref::preanalysis as cpre;
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
fn fill_paletted(
    rng: &mut Rng,
    buf: &mut [u8],
    stride: usize,
    rows: usize,
    cols: usize,
    ncolors: usize,
) {
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
    let shapes = [
        (8usize, 8usize, 8usize),
        (16, 16, 16),
        (8, 8, 23),
        (16, 16, 31),
    ];
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
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        dilate_block_cases();
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}

fn dilate_block_cases() {
    // For an 8-wide SIMD load, nonexistent high lanes must not become
    // dominant-zero neighbours of column 7. Only column 4 should change.
    let src = [0, 0, 0, 0, 17, 17, 17, 17];
    let mut actual = [0xAA; 8];
    let mut expected = [0xAA; 8];
    sc_detect::dilate_block(&src, 8, &mut actual, 8, 1, 8);
    cref::dilate_block(&src, 8, &mut expected, 8, 1, 8);
    assert_eq!(expected, [0, 0, 0, 0, 0, 17, 17, 17]);
    assert_eq!(actual, expected);

    let mut rng = Rng(0xd11a7e_0003);
    let shapes = [(8usize, 8usize), (16, 16), (3, 8), (1, 16), (5, 5), (17, 8)];
    for &(rows, cols) in &shapes {
        // C call sites use src at picture stride, dilated at tight blk_w
        // stride; fuzz both tight and loose strides on both sides.
        for &(src_stride, dst_stride) in &[
            (cols, cols),
            (cols + 11, cols),
            (cols + 3, cols + 7),
            // Overlapping destination rows retain the scalar scatter order.
            (cols, cols - 1),
        ] {
            let mut src = vec![0u8; rows * src_stride];
            let mut d_r = vec![0u8; rows * dst_stride.max(cols)];
            let mut d_c = vec![0u8; rows * dst_stride.max(cols)];
            for ncolors in [1usize, 2, 3, 5, 8, 12, 40] {
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
            screen[r * w + c] = if ((r / 4) + (c / 4)) % 2 == 0 {
                16
            } else {
                240
            };
        }
    }
    for fast in [false, true] {
        let cls = sc_detect::is_screen_content_antialiasing_aware(&screen, w, w, h, fast);
        assert!(
            cls.sc_class0
                && cls.sc_class1
                && cls.sc_class2
                && cls.sc_class3
                && cls.sc_class4
                && cls.sc_class5,
            "screen plane fast={fast}: {cls:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tier-1 differentials against the exported detectors
// ---------------------------------------------------------------------------
//
// CORRECTION to this file's header: it said the AA-aware detector "is static
// in C and reads a PCS, so its port is validated two ways: primitive-level FFI
// parity here, and end-to-end via the encoder identity harness". The first
// half is wrong — `nm Bin/Release/libSvtAv1Enc.a` reports
// `T _svt_aom_is_screen_content_antialiasing_aware`, an exported symbol. It is
// reachable at tier 1 through a PCS facade the same way
// `pad_and_decimate_filtered_pic` already is, and the tests below do that. The
// primitive-level tests above remain useful for localizing a failure; they are
// no longer the strongest evidence for the detector itself.
//
// The shim runs BOTH `svt_aom_setup_common_rtcd_internal` and `init_fn_ptr()`:
// the variance is reached through `svt_aom_mefn_ptr[bs].vf`, a plain global
// that `init_fn_ptr` fills with an RTCD pointer that is NULL until the setup
// runs. One init without the other is a null two levels down.

/// A plane whose palettizable FRACTION is tunable, so the class thresholds
/// get straddled instead of saturated.
///
/// The background is deterministic noise (far more than 40 colours in any
/// 16x16 or 8x8 window, so no block there palettizes). The first
/// `n_palette_blocks` cells of the 16x16 raster grid are overwritten with a
/// `ncolors`-colour pattern at contrast `d` around 128. Because the cells are
/// 16-aligned, both the 16x16 and the 8x8 pass see a fully-paletted block, and
/// the per-pixel variance of a two-colour +/-d checkerboard is exactly `d*d`
/// (the reference variance is taken against a constant-128 buffer and the sums
/// cancel), which is how the four variance thresholds in play — 0 and 16 for
/// the `--scm 2` detector, 5 and 50 for the AA-aware one — are each crossed
/// from both sides.
fn mixed_plane(
    w: usize,
    h: usize,
    n_palette_blocks: usize,
    ncolors: usize,
    d: u8,
    seed: u64,
) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let mut plane: Vec<u8> = (0..w * h).map(|_| rng.byte()).collect();
    let cols = w / 16;
    let palette: Vec<u8> = (0..ncolors)
        .map(|i| {
            let step = if ncolors > 1 {
                (2 * u32::from(d) * i as u32) / (ncolors as u32 - 1)
            } else {
                0
            };
            (128u32 + step - u32::from(d)).clamp(0, 255) as u8
        })
        .collect();
    for blk in 0..n_palette_blocks.min(cols * (h / 16)) {
        let (by, bx) = (blk / cols * 16, blk % cols * 16);
        for r in 0..16 {
            for c in 0..16 {
                plane[(by + r) * w + bx + c] = palette[(r + c) % ncolors];
            }
        }
    }
    plane
}

/// Edge-replicate `border` extra rows and columns onto a `w` x `h` plane,
/// the way the encoder's picture border does.
///
/// This is not cosmetic. `svt_aom_is_screen_content`'s 8x8 pass measures a
/// 16x16 window (see the port's note), so it reads to row `h + 7` and column
/// `w + 7`. Handing the oracle a tightly-sized buffer would have it read
/// whatever follows the allocation, and the differential would agree or
/// disagree by luck.
fn with_border(plane: &[u8], w: usize, h: usize, border: usize) -> (Vec<u8>, usize) {
    let stride = w + border;
    let mut out = vec![0u8; stride * (h + border)];
    for r in 0..h + border {
        let sr = r.min(h - 1);
        for c in 0..stride {
            let sc = c.min(w - 1);
            out[r * stride + c] = plane[sr * w + sc];
        }
    }
    (out, stride)
}

/// The plane suite: `(name, stride, width, height, plane_with_border)`. Named
/// so a failure says which one, and swept over the palettizable fraction so no
/// threshold is reached only from one side.
fn detector_planes() -> Vec<(String, usize, usize, usize, Vec<u8>)> {
    let (w, h) = (128usize, 96usize);
    let blocks_16 = (w / 16) * (h / 16); // 48
    let mut out = Vec::new();

    // Smooth gradient: photo-like.
    let mut grad = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            grad[r * w + c] = ((r + c) / 2) as u8;
        }
    }
    out.push(("gradient".to_string(), w, h, grad));

    // Two-value checkerboard: saturates every class.
    let mut screen = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..w {
            screen[r * w + c] = if ((r / 4) + (c / 4)) % 2 == 0 {
                16
            } else {
                240
            };
        }
    }
    out.push(("checker4".to_string(), w, h, screen));

    // Flat: one colour per block, so `is_valid_palette_nb_colors` rejects
    // every block (`nb_colors <= 1`) while `count_colors_with_threshold`
    // would have accepted it — the one place the two primitives disagree.
    out.push(("flat".to_string(), w, h, vec![128u8; w * h]));

    // The fraction sweep. `n` walks the 16x16 grid one block at a time near
    // the low thresholds and in coarser steps above them, `ncolors` picks the
    // simple (<=4) vs complex (5..40, dilation) path of the AA detector, and
    // `d` straddles all four variance thresholds.
    for &n in &[1usize, 2, 3, 4, 5, 6, 7, 9, 12, 16, 24, 36, blocks_16] {
        for &(ncolors, d) in &[(2usize, 1u8), (2, 5), (2, 20), (3, 8), (12, 20), (30, 40)] {
            out.push((
                format!("mixed n={n} ncolors={ncolors} d={d}"),
                w,
                h,
                mixed_plane(
                    w,
                    h,
                    n,
                    ncolors,
                    d,
                    0x9E37_79B9 ^ (n as u64) << 8 ^ d as u64,
                ),
            ));
        }
    }

    // A second family on a FLAT background instead of noise. A flat block has
    // one colour, so `is_valid_palette_nb_colors` rejects it (`nb_colors <= 1`)
    // and it contributes to neither count — which decouples `counts_1` from
    // `counts_2` in a way the noise family cannot, because a low-contrast
    // patch surrounded by flat has a small 16x16-window variance while still
    // palettizing. That is what moves `sc_class4`'s two conjuncts
    // independently and makes its 18-vs-20 constants separable.
    for &n in &[1usize, 2, 3, 4, 5, 6, 8, 10, 14, 20, 30, 44] {
        for &d in &[1u8, 2, 3, 6, 20] {
            let cols = w / 16;
            let mut plane = vec![128u8; w * h];
            for blk in 0..n.min(cols * (h / 16)) {
                let (by, bx) = (blk / cols * 16, blk % cols * 16);
                for r in 0..16 {
                    for c in 0..16 {
                        plane[(by + r) * w + bx + c] =
                            if (r + c) % 2 == 0 { 128 - d } else { 128 + d };
                    }
                }
            }
            out.push((format!("flatmix n={n} d={d}"), w, h, plane));
        }
    }

    // The class thresholds compare `counts * blk_area * K` against the frame
    // AREA, and `counts` is an integer — so at any ONE frame size, K and K+1
    // usually round to the same verdict and a mutation of either is invisible.
    // (Measured: at 128x96 every one of the eight class constants could be
    // moved by one without a single cell changing.) Sweeping the HEIGHT moves
    // the area, and with it each threshold's integer boundary, so some frame
    // in the sweep sits between K and K+1 for each of them.
    for hm in [2usize, 3, 4, 5, 6, 7, 8] {
        let hh = hm * 16;
        for n in 1..=(w / 16) * hm {
            for &(ncolors, d) in &[(2usize, 20u8), (5, 20)] {
                out.push((
                    format!("area w=128 h={hh} n={n} ncolors={ncolors}"),
                    w,
                    hh,
                    mixed_plane(w, hh, n, ncolors, d, 0xA5A5 ^ (n as u64) << 4 ^ hh as u64),
                ));
            }
        }
    }

    // Two INDEPENDENT block populations, because `counts_2` is otherwise
    // pinned to `counts_1`.
    //
    // Every family above uses one kind of palettizable block, and any visible
    // two-colour pattern clears the 16x16 pass's `var_thresh == 0` — so
    // `counts_2 == counts_1` there and the constants that read `counts_2`
    // (sc_class1's 12, sc_class2's 30, sc_class3's 50) cannot be moved by one
    // without moving `counts_1`'s constants too.
    //
    // The separation comes from the reference buffer: C measures variance
    // against a CONSTANT 128 plane, so a 200/201 checkerboard has a raw 16x16
    // variance of 64 and `ROUND_POWER_OF_TWO(64, 8)` is 0 — it palettizes and
    // does NOT clear the variance test. `total - n_high` of those plus
    // `n_high` full-contrast blocks give `counts_1 = total` and
    // `counts_2 = n_high`, swept independently.
    //
    // The heights are not all multiples of 16. `counts` is an integer, so
    // whether constant K and K+1 differ depends on the AREA: some boundaries
    // (sc_class2's 30, sc_class3's 50) have no integer between them at
    // 128x{48,80} and do at 128x{100,120}.
    for hh in [48usize, 80, 100, 120, 200] {
        let cols = w / 16;
        let grid = cols * (hh / 16);
        for total in 0..=grid {
            let mut highs = vec![0usize, 1, 2, 3, 4, total / 2, total];
            highs.retain(|n| *n <= total);
            highs.sort_unstable();
            highs.dedup();
            for n_high in highs {
                let mut plane: Vec<u8> = {
                    let mut rng = Rng(0xC0FF_EE01 ^ hh as u64);
                    (0..w * hh).map(|_| rng.byte()).collect()
                };
                for blk in 0..total {
                    let (lo, hi) = if blk < n_high {
                        (16u8, 240u8)
                    } else {
                        (200, 201)
                    };
                    let (by, bx) = (blk / cols * 16, blk % cols * 16);
                    for r in 0..16 {
                        for c in 0..16 {
                            plane[(by + r) * w + bx + c] = if (r + c) % 2 == 0 { lo } else { hi };
                        }
                    }
                }
                out.push((
                    format!("mix2 h={hh} total={total} high={n_high}"),
                    w,
                    hh,
                    plane,
                ));
            }
        }
    }

    // Palettizable cells on the 8x8 grid instead of the 16x16 one.
    //
    // Every family above places them 16-aligned, so `counts_1` on the 8x8 pass
    // is always a multiple of four — and sc_class4's constants (18 and 20)
    // compare it against the area, where the boundary between K and K+1 falls
    // between consecutive integers that a multiple of four steps over. With
    // 8x8-granular cells the count takes every value, and a 16x16 block
    // holding one paletted 8x8 plus noise does not palettize, so this family
    // drives the 8x8 pass with the 16x16 pass held at zero. `k_high` vs `k_low`
    // splits `counts_2` from `counts_1` there the same way `mix2` does above.
    for hh in [48usize, 80, 112] {
        let cols8 = w / 8;
        let grid8 = cols8 * (hh / 8);
        for k in 0..=grid8 {
            if k > 40 && k % 7 != 0 {
                continue; // keep the suite bounded away from the boundaries
            }
            let mut highs = vec![0usize, 1, 2, 3, 4, 6, k / 2, k];
            highs.retain(|n| *n <= k);
            highs.sort_unstable();
            highs.dedup();
            for k_high in highs {
                let mut plane: Vec<u8> = {
                    let mut rng = Rng(0x5EED_1234 ^ hh as u64);
                    (0..w * hh).map(|_| rng.byte()).collect()
                };
                for blk in 0..k {
                    let (lo, hi) = if blk < k_high {
                        (16u8, 240u8)
                    } else {
                        (200, 201)
                    };
                    let (by, bx) = (blk / cols8 * 8, blk % cols8 * 8);
                    for r in 0..8 {
                        for c in 0..8 {
                            plane[(by + r) * w + bx + c] = if (r + c) % 2 == 0 { lo } else { hi };
                        }
                    }
                }
                out.push((format!("eight h={hh} k={k} high={k_high}"), w, hh, plane));
            }
        }
    }

    // Blocks with a CONTROLLED colour count before and after dilation, and a
    // controlled variance, for the AA-aware detector's six colour/variance
    // thresholds (simple 4, initial 40, final 6, final8 8, var 5, var8 50).
    //
    // Those are invisible to every family above, which uses two-colour blocks
    // at either full contrast or near-zero — nothing near 4/5, 6/7, 8/9, 40/41,
    // or a per-pixel variance near 5 or 50.
    //
    // The generator: a dominant background, `clumps` 3x3 patches each of a
    // distinct value, and `sprinkle` isolated single pixels each of a distinct
    // value. `svt_av1_dilate_block` extends the dominant value one pixel in all
    // eight directions, which erases an isolated pixel and leaves a 3x3 patch's
    // centre — so the RAW count is `1 + clumps + sprinkle` and the DILATED
    // count is `1 + clumps`. Sweeping the two independently walks the raw count
    // across 4/5 and 40/41 and the dilated count across 6/7 and 8/9.
    // `contrast` sets the per-pixel variance, which straddles 5 and 50.
    for &clumps in &[0usize, 3, 4, 5, 6, 7, 8, 9, 11] {
        for &sprinkle in &[0usize, 1, 2, 3, 20, 29, 30, 31, 32, 34, 35, 36] {
            for &contrast in &[1u8, 2, 3, 4, 6, 8, 20] {
                let hh = 64usize;
                let cols = w / 16;
                let mut plane = vec![128u8; w * hh];
                for by in (0..hh).step_by(16) {
                    for bx in (0..w).step_by(16) {
                        // Alternate the dominant value so neighbouring blocks
                        // differ and the 16x16 windows are not all identical.
                        let dom = if ((by / 16) + (bx / 16)) % 2 == 0 {
                            128
                        } else {
                            120
                        };
                        for r in 0..16 {
                            for c in 0..16 {
                                plane[(by + r) * w + bx + c] = dom;
                            }
                        }
                        let mut val = 1u32;
                        for i in 0..clumps {
                            let (cy, cx) = (1 + (i / 4) * 4, 1 + (i % 4) * 4);
                            let v = (u32::from(dom) + val * u32::from(contrast)).min(255) as u8;
                            val += 1;
                            for r in 0..3 {
                                for c in 0..3 {
                                    plane[(by + cy + r) * w + bx + cx + c] = v;
                                }
                            }
                        }
                        for i in 0..sprinkle {
                            let (sy, sx) = (14 - i / 14, 1 + i % 14);
                            let v = (u32::from(dom) + val * u32::from(contrast)).min(255) as u8;
                            val += 1;
                            plane[(by + sy) * w + bx + sx] = v;
                        }
                    }
                }
                let _ = cols;
                out.push((
                    format!("aa clumps={clumps} sprinkle={sprinkle} k={contrast}"),
                    w,
                    hh,
                    plane,
                ));
            }
        }
    }

    // The same idea on the 8x8 grid, for the AA detector's 8x8 pass — its
    // `complex_final_color_thresh_8` of 8, its `var_thresh_8` of 50, and the
    // per-region counts that sc_class4 and sc_class5 read.
    //
    // The variance sweep is over the FILL COUNT, not just the contrast:
    // per-pixel variance of `m` pixels at `128 + a` among 64 is
    // `round(p(1-p) a^2)` with `p = m/64`, which takes many values between 40
    // and 60 as `m` walks — where sweeping `a` alone jumps 49 -> 64 and steps
    // straight over 50 and 51.
    for &a in &[7u8, 8, 12, 15, 16, 20] {
        for m in (6..44).step_by(2) {
            for &clumps8 in &[0usize, 3, 6, 7, 8] {
                let hh = 64usize;
                let mut plane = vec![128u8; w * hh];
                for by in (0..hh).step_by(8) {
                    for bx in (0..w).step_by(8) {
                        let mut placed = 0usize;
                        for r in 0..8 {
                            for c in 0..8 {
                                let v = if placed < m {
                                    placed += 1;
                                    128u32 + u32::from(a)
                                } else {
                                    128
                                };
                                plane[(by + r) * w + bx + c] = v.min(255) as u8;
                            }
                        }
                        // Extra distinct values, one pixel each, to push the
                        // colour count across the 8/9 dilated threshold.
                        for i in 0..clumps8 {
                            plane[(by + 7) * w + bx + i.min(7)] =
                                (200u32 + i as u32).min(255) as u8;
                        }
                    }
                }
                out.push((format!("aa8 a={a} m={m} extra={clumps8}"), w, hh, plane));
            }
        }
    }

    // 8x8 palettizable blocks distributed ROUND-ROBIN over the four quadrants
    // rather than in raster order.
    //
    // sc_class4 and sc_class5 are gated on `pass >= 3`, the number of quadrants
    // whose own palette AND intrabc counts clear their thresholds — and a
    // raster fill reaches the third quadrant only past half the frame, where
    // the totals are already far above sc_class4's and sc_class5's own
    // constants. Round-robin puts blocks in all four quadrants from k = 4
    // onward, so the region constants (10, 25) and the total constants
    // (5, 10, 23) are crossed with the counts still small and each in its own
    // cell.
    // The per-quadrant counts are also made UNEQUAL. With all four equal the
    // total is always `4k`, and sc_class5's total constants (10, 23) sit
    // arithmetically adjacent to the per-region ones (10, 25) — four quadrants
    // each just over `region_area/640` sum to just over `area/640` — so a
    // multiple-of-four total can never land between them. The `-1` and `0`
    // patterns break that.
    for hh in [64usize, 88, 96, 120, 152] {
        let cols8 = w / 8;
        let rows8 = hh / 8;
        let per_q = (cols8 / 2) * (rows8 / 2);
        for k in 0..=per_q.min(24) {
            for pattern in [[1usize, 1, 1, 1], [1, 1, 1, 0], [1, 1, 0, 0], [1, 1, 1, 1]] {
                let drop = usize::from(pattern == [1, 1, 1, 1]);
                for &k_high in &[k, k / 2, k.saturating_sub(1), 0] {
                    let mut plane: Vec<u8> = {
                        let mut rng = Rng(0x3141_5926 ^ hh as u64);
                        (0..w * hh).map(|_| rng.byte()).collect()
                    };
                    for q in 0..4usize {
                        let (qy, qx) = ((q / 2) * (rows8 / 2), (q % 2) * (cols8 / 2));
                        let kq = if pattern[q] == 0 {
                            0
                        } else {
                            k.saturating_sub(if q == 3 { drop } else { 0 })
                        };
                        for blk in 0..kq {
                            let (lo, hi) = if blk < k_high {
                                (16u8, 240u8)
                            } else {
                                (200, 201)
                            };
                            let by = (qy + blk / (cols8 / 2)) * 8;
                            let bx = (qx + blk % (cols8 / 2)) * 8;
                            for r in 0..8 {
                                for c in 0..8 {
                                    plane[(by + r) * w + bx + c] =
                                        if (r + c) % 2 == 0 { lo } else { hi };
                                }
                            }
                        }
                    }
                    out.push((
                        format!("eightq h={hh} k={k} high={k_high} pat={pattern:?}"),
                        w,
                        hh,
                        plane,
                    ));
                }
            }
        }
    }

    // A mix of palettizable blocks and blocks with an exact colour count
    // straddling `complex_initial_color_thresh` (40).
    //
    // That threshold decides the PHOTO class, which enters sc_class0 and
    // sc_class1 only as `- count_photo / 16`. A frame where every block is a
    // photo candidate therefore has `count_palette == 0` and stays below the
    // threshold whichever way the constant goes; the flip is only visible when
    // the palette count is already near its own boundary. So: `n_pal`
    // two-colour blocks plus a remainder holding exactly `ncolors` distinct
    // values, one pixel each on a dominant field.
    for &ncolors in &[38usize, 39, 40, 41, 42, 43] {
        for n_pal in 0..24usize {
            let hh = 64usize;
            let cols = w / 16;
            let mut plane = vec![128u8; w * hh];
            for blk in 0..cols * (hh / 16) {
                let (by, bx) = (blk / cols * 16, blk % cols * 16);
                if blk < n_pal {
                    for r in 0..16 {
                        for c in 0..16 {
                            plane[(by + r) * w + bx + c] = if (r + c) % 2 == 0 { 16 } else { 240 };
                        }
                    }
                } else {
                    for r in 0..16 {
                        for c in 0..16 {
                            plane[(by + r) * w + bx + c] = 60;
                        }
                    }
                    // `ncolors - 1` further values, isolated single pixels.
                    for i in 0..ncolors - 1 {
                        let (sy, sx) = (1 + i / 14, 1 + i % 14);
                        plane[(by + sy) * w + bx + sx] = (61 + i) as u8;
                    }
                }
            }
            out.push((format!("photomix n={ncolors} pal={n_pal}"), w, hh, plane));
        }
    }

    // One frame size DERIVED to separate sc_class5's `count_intrabc_8 * 64 * 23`
    // from a 24, which no size above can do.
    //
    // sc_class5 needs `pass >= 3`, and `pass` needs each counted quadrant's
    // `region_intrabc * 64 * 25 > area / 4` — so three quadrants already put
    // `count_intrabc_8` above `area / 2133`, while the class constant only asks
    // for `area / 1472`. The gap between the 23 and 24 forms is
    // `area * (1/4416 - 1/4608)`, about `area / 106000` wide, so no integer
    // count lands inside it until the frame has ~106k pixels. 256x416 is
    // 106,496: with the fourth quadrant empty, `k = 56` blocks per quadrant and
    // `k_high = 24` of them at full contrast, `count_intrabc_8` is 72 and
    // `72 * 64 * 23 = 105,984 <= 106,496 < 110,592 = 72 * 64 * 24`.
    {
        let (w3, h3) = (256usize, 416usize);
        let (cols8, rows8) = (w3 / 8, h3 / 8);
        for k in [54usize, 56, 58] {
            for k_high in [22usize, 23, 24, 25, 26] {
                let mut plane: Vec<u8> = {
                    let mut rng = Rng(0x7777_3333);
                    (0..w3 * h3).map(|_| rng.byte()).collect()
                };
                for q in 0..3usize {
                    let (qy, qx) = ((q / 2) * (rows8 / 2), (q % 2) * (cols8 / 2));
                    for blk in 0..k {
                        let (lo, hi) = if blk < k_high {
                            (16u8, 240u8)
                        } else {
                            (200, 201)
                        };
                        let by = (qy + blk / (cols8 / 2)) * 8;
                        let bx = (qx + blk % (cols8 / 2)) * 8;
                        for r in 0..8 {
                            for c in 0..8 {
                                plane[(by + r) * w3 + bx + c] =
                                    if (r + c) % 2 == 0 { lo } else { hi };
                            }
                        }
                    }
                }
                out.push((format!("c5gap k={k} high={k_high}"), w3, h3, plane));
            }
        }
    }

    // A non-square frame, to exercise the skipped partial edge blocks.
    let (w2, h2) = (72usize, 40usize);
    out.push((
        "edge72x40".to_string(),
        w2,
        h2,
        mixed_plane(w2, h2, 4, 2, 20, 0x1357),
    ));

    // Every plane gets the 16-pixel border both detectors are entitled to
    // read into (8 is the minimum; 16 leaves headroom and matches the
    // encoder's own alignment).
    out.into_iter()
        .map(|(name, w, h, plane)| {
            let (padded, stride) = with_border(&plane, w, h, 16);
            (name, stride, w, h, padded)
        })
        .collect()
}

/// Panics unless EVERY class bit was observed both true and false over the
/// suite.
///
/// This is the anti-vacuity gate, and it is per-BIT rather than per-tuple on
/// purpose: a suite can produce several distinct tuples while one bit stays
/// constant, and a constant bit's threshold is untested no matter how many
/// cells ran. Verified by mutation — changing any one of the detector's
/// threshold constants fails these tests.
fn assert_every_bit_moved(verdicts: &[Vec<bool>], what: &str) {
    assert!(!verdicts.is_empty(), "{what}: no cells ran at all");
    let n = verdicts[0].len();
    for bit in 0..n {
        let t = verdicts.iter().filter(|v| v[bit]).count();
        let f = verdicts.len() - t;
        assert!(
            t > 0 && f > 0,
            "{what}: sc_class{bit} was {} in all {} cells, so its threshold is untested",
            if t > 0 { "true" } else { "false" },
            verdicts.len()
        );
    }
}

/// The mismatched call `svt_aom_is_screen_content`'s 8x8 pass actually makes:
/// the 16x16 variance kernel with the 8x8 normaliser. Driven against the real
/// C symbol so the claim in `sc_detect::is_screen_content`'s note is measured,
/// not read off the source.
#[test]
fn sby_perpixel_variance_mixed_block_sizes_matches_c() {
    let mut rng = Rng(0xFEED_5EED);
    let stride = 48usize;
    let mut differed = 0usize;
    for ncolors in [2usize, 3, 9, 64] {
        for d in [1u8, 5, 20, 80] {
            let mut buf = vec![0u8; stride * 20];
            // Only the top-left 8x8 is quiet; the rest of the 16x16 window is
            // noisy, which is exactly the situation the C defect misreads.
            for v in buf.iter_mut() {
                *v = rng.byte();
            }
            for r in 0..8 {
                for c in 0..8 {
                    buf[r * stride + c] =
                        (128u32 + ((r + c) % ncolors) as u32 * u32::from(d)).min(255) as u8;
                }
            }
            let mixed = cpre::sby_perpixel_variance_mixed(
                &buf,
                stride,
                cpre::ScBlockSize::Blk16x16,
                cpre::ScBlockSize::Blk8x8,
            );
            assert_eq!(
                sc_detect::sby_perpixel_variance_normalized(&buf, stride, 16, 8),
                mixed,
                "mixed variance ncolors={ncolors} d={d}"
            );
            // The whole point: it is NOT the well-formed 8x8 measurement.
            let matched = cpre::sby_perpixel_variance(&buf, stride, cpre::ScBlockSize::Blk8x8);
            if mixed != matched {
                differed += 1;
            }
        }
    }
    assert!(
        differed > 0,
        "the mixed and matched calls never differed, so this proves nothing"
    );
}

#[test]
fn sby_perpixel_variance_matches_c() {
    let mut rng = Rng(0x0BAD_C0DE);
    let stride = 40usize;
    let mut cells = 0usize;
    let mut nonzero = 0usize;
    for bs in [cpre::ScBlockSize::Blk8x8, cpre::ScBlockSize::Blk16x16] {
        let side = bs.side();
        for ncolors in [1usize, 2, 5, 17, 200] {
            let mut buf = vec![0u8; stride * (side + 2)];
            fill_paletted(&mut rng, &mut buf, stride, side, side, ncolors);
            let c = cpre::sby_perpixel_variance(&buf, stride, bs);
            let r = sc_detect::sby_perpixel_variance(&buf, stride, side, side);
            assert_eq!(r, c, "variance {side}x{side} ncolors {ncolors}");
            if c != 0 {
                nonzero += 1;
            }
            cells += 1;
        }
        // Two saturating cases: the constant-128 plane the C reference
        // subtracts against (variance exactly 0), and a full-swing block.
        let flat = vec![128u8; stride * (side + 2)];
        assert_eq!(
            sc_detect::sby_perpixel_variance(&flat, stride, side, side),
            cpre::sby_perpixel_variance(&flat, stride, bs),
            "variance flat-128 {side}x{side}"
        );
        let mut swing = vec![0u8; stride * (side + 2)];
        for r in 0..side {
            for c in 0..side {
                swing[r * stride + c] = if (r + c) % 2 == 0 { 0 } else { 255 };
            }
        }
        assert_eq!(
            sc_detect::sby_perpixel_variance(&swing, stride, side, side),
            cpre::sby_perpixel_variance(&swing, stride, bs),
            "variance full-swing {side}x{side}"
        );
        cells += 2;
    }
    assert_eq!(cells, 2 * 7);
    assert!(nonzero > 0, "every variance probe returned 0");
}

#[test]
fn is_screen_content_antialiasing_aware_matches_c() {
    let mut cells = 0usize;
    let mut verdicts: Vec<Vec<bool>> = Vec::new();
    for (name, stride, w, h, plane) in detector_planes() {
        for fast in [false, true] {
            let c = cpre::is_screen_content_antialiasing_aware(&plane, stride, w, h, fast);
            let r = sc_detect::is_screen_content_antialiasing_aware(&plane, stride, w, h, fast);
            let got = vec![
                r.sc_class0,
                r.sc_class1,
                r.sc_class2,
                r.sc_class3,
                r.sc_class4,
                r.sc_class5,
            ];
            let want = vec![
                c.sc_class0,
                c.sc_class1,
                c.sc_class2,
                c.sc_class3,
                c.sc_class4,
                c.sc_class5,
            ];
            assert_eq!(got, want, "AA detector on {name} fast={fast}");
            verdicts.push(want);
            cells += 1;
        }
    }
    assert_eq!(cells, 2 * detector_planes().len());
    assert!(cells > 400, "suite shrank to {cells} cells");
    assert_every_bit_moved(&verdicts, "AA detector");
}

#[test]
fn is_screen_content_matches_c() {
    let mut cells = 0usize;
    let mut verdicts: Vec<Vec<bool>> = Vec::new();
    for (name, stride, w, h, plane) in detector_planes() {
        let c = cpre::is_screen_content(&plane, stride, w, h);
        let r = sc_detect::is_screen_content(&plane, stride, w, h);
        // C never assigns sc_class5 in this detector; the port leaves it
        // false, and the shim reads it out of a zeroed PCS.
        assert!(
            !c.sc_class5 && !r.sc_class5,
            "scm2 wrote sc_class5 on {name}"
        );
        let got = vec![
            r.sc_class0,
            r.sc_class1,
            r.sc_class2,
            r.sc_class3,
            r.sc_class4,
        ];
        let want = vec![
            c.sc_class0,
            c.sc_class1,
            c.sc_class2,
            c.sc_class3,
            c.sc_class4,
        ];
        assert_eq!(got, want, "scm2 detector on {name}");
        verdicts.push(want);
        cells += 1;
    }
    assert_eq!(cells, detector_planes().len());
    assert!(cells > 200, "suite shrank to {cells} cells");
    assert_every_bit_moved(&verdicts, "scm2 detector");
}

#[test]
fn is_valid_palette_nb_colors_rejects_single_color() {
    // The behaviour that separates it from `count_colors_with_threshold`:
    // a one-colour block is INVALID here and within-threshold there.
    let flat = vec![7u8; 16 * 16];
    assert!(!sc_detect::is_valid_palette_nb_colors(&flat, 16, 16, 16, 4));
    assert_eq!(
        sc_detect::count_colors_with_threshold(&flat, 16, 16, 16, 4),
        (true, 1)
    );

    let mut two = flat.clone();
    two[5 * 16 + 5] = 9;
    assert!(sc_detect::is_valid_palette_nb_colors(&two, 16, 16, 16, 4));

    // Over the threshold: five distinct values with a threshold of 4.
    let mut five = flat.clone();
    for (i, v) in [11u8, 12, 13, 14].iter().enumerate() {
        five[i] = *v;
    }
    assert!(!sc_detect::is_valid_palette_nb_colors(&five, 16, 16, 16, 4));
    assert!(sc_detect::is_valid_palette_nb_colors(&five, 16, 16, 16, 5));
}

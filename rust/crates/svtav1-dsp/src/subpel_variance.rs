//! Bilinear sub-pixel variance — C's `AomVarianceFnPtr::svf`.
//!
//! Ports `SUBPIX_VAR(W, H)` and its two helper passes from
//! `Source/Lib/C_DEFAULT/variance.c`:
//!
//! * `aom_var_filter_block2d_bil_first_pass_c` (`:29`)
//! * `aom_var_filter_block2d_bil_second_pass_c` (`:55`)
//! * `SUBPIX_VAR(W, H)` (`:192`), instantiated by `VARIANCES(W, H)` (`:205`)
//!   for the 22 block sizes at `:208-229`.
//!
//! This is the error metric the PRUNED sub-pixel tree actually minimises
//! (`mcomp.c:156 svt_estimated_pref_error` -> `vfp->svf`), so every fractional
//! MV the mid-preset encoder codes is decided by these three functions.
//!
//! The C macro emits one function per (W, H) with W and H as compile-time
//! constants; the loops are otherwise textually identical, so this port takes
//! them as runtime arguments. The only size-dependent quantities are the two
//! scratch buffers and the final `W * H` divisor, all of which are carried
//! through unchanged. `crates/svtav1-dsp/tests/c_parity_subpel_variance.rs`
//! drives all 22 exported `svt_aom_sub_pixel_variance{W}x{H}_c` symbols, so
//! the parameterisation is checked against every instantiation rather than
//! assumed.
//!
//! `xoffset` / `yoffset` are the q3 sub-pel phases (`mv & 7`, i.e. `0..=7`);
//! `BIL_SUBPEL_SHIFTS` is 8 and `bilinear_filters_2t` (`Codec/filter.h:39`)
//! has exactly those eight rows.

use alloc::vec;

/// C `FILTER_BITS` (`filter.h`).
const FILTER_BITS: i32 = 7;

/// C `bilinear_filters_2t[BIL_SUBPEL_SHIFTS][2]` (`Codec/filter.h:39-48`).
pub const BILINEAR_FILTERS_2T: [[u8; 2]; 8] = [
    [128, 0],
    [112, 16],
    [96, 32],
    [80, 48],
    [64, 64],
    [48, 80],
    [32, 96],
    [16, 112],
];

/// C `ROUND_POWER_OF_TWO(value, n)` for the non-negative sums here.
#[inline]
fn round_power_of_two(value: i32, n: i32) -> i32 {
    (value + (1 << (n - 1))) >> n
}

/// C `aom_var_filter_block2d_bil_first_pass_c` (`variance.c:29-43`).
///
/// Horizontal 2-tap pass: `pixel_step` is 1 at every call site in this file,
/// so the second tap is the next pixel in the row. Output is `u16` to keep
/// the precision the second pass consumes.
///
/// `src_pixels_per_line` is the input stride; the C pointer walks
/// `output_width` samples and then skips `src_pixels_per_line - output_width`,
/// i.e. one input row per output row.
pub fn var_filter_block2d_bil_first_pass(
    a: &[u8],
    a_base: usize,
    b: &mut [u16],
    src_pixels_per_line: usize,
    pixel_step: usize,
    output_height: usize,
    output_width: usize,
    filter: &[u8; 2],
) {
    let (f0, f1) = (i32::from(filter[0]), i32::from(filter[1]));
    for i in 0..output_height {
        let row = a_base + i * src_pixels_per_line;
        for j in 0..output_width {
            let v = i32::from(a[row + j]) * f0 + i32::from(a[row + j + pixel_step]) * f1;
            b[i * output_width + j] = round_power_of_two(v, FILTER_BITS) as u16;
        }
    }
}

/// C `aom_var_filter_block2d_bil_second_pass_c` (`variance.c:55-68`).
///
/// Vertical 2-tap pass over the `u16` intermediate: `pixel_step` is the
/// intermediate stride (`W`), so the second tap is the sample one row below.
/// Output is 8-bit; C stores into a `uint8_t` and the rounded 2-tap sum of two
/// values `<= 255` with taps summing to 128 cannot exceed 255, so no clamp is
/// present in C and none is added here.
pub fn var_filter_block2d_bil_second_pass(
    a: &[u16],
    b: &mut [u8],
    src_pixels_per_line: usize,
    pixel_step: usize,
    output_height: usize,
    output_width: usize,
    filter: &[u8; 2],
) {
    let (f0, f1) = (i32::from(filter[0]), i32::from(filter[1]));
    for i in 0..output_height {
        let row = i * src_pixels_per_line;
        for j in 0..output_width {
            let v = i32::from(a[row + j]) * f0 + i32::from(a[row + j + pixel_step]) * f1;
            b[i * output_width + j] = round_power_of_two(v, FILTER_BITS) as u8;
        }
    }
}

/// C `svt_aom_sub_pixel_variance{W}x{H}_c` (`SUBPIX_VAR`, `variance.c:192-203`).
///
/// Returns `(variance, sse)`; C returns the variance and writes `sse` through
/// its out-parameter.
///
/// `a_base` is the index of the block's (0, 0) inside `a`. The first pass
/// reads `H + 1` rows and `W + 1` columns of `a`, which is why C's callers
/// hand it a reference plane with a guard band rather than a tight block.
/// Widest block `VARIANCES(W, H)` instantiates (`variance.c:208-229`).
const MAX_SUBPEL_W: usize = 128;

/// C `svt_aom_sub_pixel_variance{W}x{H}_c` (`SUBPIX_VAR`, `variance.c:192-203`).
///
/// Returns `(variance, sse)`; C returns the variance and writes `sse` through
/// its out-parameter.
///
/// `a_base` is the index of the block's (0, 0) inside `a`. The first pass
/// reads `H + 1` rows and `W + 1` columns of `a`, which is why C's callers
/// hand it a reference plane with a guard band rather than a tight block.
///
/// # Streamed, not materialised
///
/// C allocates the full `(H + 1) x W` `uint16_t` intermediate and the full
/// `H x W` `uint8_t` second-pass output, and so did this port — two heap
/// allocations per call, on a function the sub-pel tree calls once per
/// candidate. The row dependency is only one row deep (`fdata3[i]` and
/// `fdata3[i+1]`), so the whole thing streams with TWO first-pass rows and one
/// second-pass row live, all on the stack. The arithmetic is untouched: same
/// order, same `i32` intermediates, same `ROUND_POWER_OF_TWO`, same truncating
/// `(sum * sum) / n` taken BEFORE the subtraction.
///
/// `w > MAX_SUBPEL_W` cannot happen for any size C instantiates, but rather
/// than panic on one it falls back to the materialised path.
#[allow(clippy::too_many_arguments)]
pub fn sub_pixel_variance(
    a: &[u8],
    a_base: usize,
    a_stride: usize,
    xoffset: usize,
    yoffset: usize,
    b: &[u8],
    b_base: usize,
    b_stride: usize,
    w: usize,
    h: usize,
) -> (u32, u32) {
    if w > MAX_SUBPEL_W {
        return sub_pixel_variance_materialised(
            a, a_base, a_stride, xoffset, yoffset, b, b_base, b_stride, w, h,
        );
    }

    let fx = &BILINEAR_FILTERS_2T[xoffset];
    let fy = &BILINEAR_FILTERS_2T[yoffset];
    let (fx0, fx1) = (i32::from(fx[0]), i32::from(fx[1]));
    let (fy0, fy1) = (i32::from(fy[0]), i32::from(fy[1]));

    let mut prev = [0u16; MAX_SUBPEL_W];
    let mut cur = [0u16; MAX_SUBPEL_W];
    let mut temp = [0u8; MAX_SUBPEL_W];

    // First-pass row 0. `pixel_step` is 1 at every call site, so the second
    // tap is the next sample in the row.
    for j in 0..w {
        let v = i32::from(a[a_base + j]) * fx0 + i32::from(a[a_base + j + 1]) * fx1;
        prev[j] = round_power_of_two(v, FILTER_BITS) as u16;
    }

    let mut sum: i64 = 0;
    let mut sse: u64 = 0;
    for i in 0..h {
        let row = a_base + (i + 1) * a_stride;
        for j in 0..w {
            let v = i32::from(a[row + j]) * fx0 + i32::from(a[row + j + 1]) * fx1;
            cur[j] = round_power_of_two(v, FILTER_BITS) as u16;
        }
        for j in 0..w {
            let v = i32::from(prev[j]) * fy0 + i32::from(cur[j]) * fy1;
            temp[j] = round_power_of_two(v, FILTER_BITS) as u8;
        }
        let bo = b_base + i * b_stride;
        for j in 0..w {
            let diff = i32::from(temp[j]) - i32::from(b[bo + j]);
            sum += i64::from(diff);
            sse += (diff * diff) as u64;
        }
        core::mem::swap(&mut prev, &mut cur);
    }

    let sse = sse as u32;
    let n = (w * h) as i64;
    (sse.wrapping_sub(((sum * sum) / n) as u32), sse)
}

/// The materialised form C writes, kept for `w > MAX_SUBPEL_W` and as the
/// oracle [`sub_pixel_variance`]'s streaming form is pinned against
/// (`streaming_matches_materialised`).
#[allow(clippy::too_many_arguments)]
pub fn sub_pixel_variance_materialised(
    a: &[u8],
    a_base: usize,
    a_stride: usize,
    xoffset: usize,
    yoffset: usize,
    b: &[u8],
    b_base: usize,
    b_stride: usize,
    w: usize,
    h: usize,
) -> (u32, u32) {
    let mut fdata3 = vec![0u16; (h + 1) * w];
    let mut temp2 = vec![0u8; h * w];

    var_filter_block2d_bil_first_pass(
        a,
        a_base,
        &mut fdata3,
        a_stride,
        1,
        h + 1,
        w,
        &BILINEAR_FILTERS_2T[xoffset],
    );
    var_filter_block2d_bil_second_pass(
        &fdata3,
        &mut temp2,
        w,
        w,
        h,
        w,
        &BILINEAR_FILTERS_2T[yoffset],
    );

    // C: `return svt_aom_variance{W}x{H}_c(temp2, W, b, b_stride, sse);`
    variance_diff_sse(&temp2, 0, w, b, b_base, b_stride, w, h)
}

/// C `variance_c` + `VAR(W, H)` (`variance.c:141-190`), returning both the
/// variance and the sse the macro writes out.
///
/// [`crate::variance::variance_diff`] is the same computation but discards
/// `sse`; the sub-pel search needs it (it is `*sse1`, which MD later reads),
/// so this file keeps a variant that returns the pair. The arithmetic is
/// identical: `sum` accumulated in `int`, `sse` in `uint32_t`, the division
/// `((int64_t)sum * sum) / (W * H)` truncating and performed BEFORE the
/// subtraction.
#[allow(clippy::too_many_arguments)]
pub fn variance_diff_sse(
    a: &[u8],
    a_base: usize,
    a_stride: usize,
    b: &[u8],
    b_base: usize,
    b_stride: usize,
    w: usize,
    h: usize,
) -> (u32, u32) {
    // The accumulation is `crate::me_sad::block_sum_sse` (SIMD, exact — see
    // that module's range argument); the reduction below is C's, unchanged:
    // the division truncates and happens BEFORE the subtraction.
    let (sum, sse) =
        crate::me_sad::block_sum_sse(&a[a_base..], a_stride, &b[b_base..], b_stride, w, h);
    let sum = i64::from(sum);
    let n = (w * h) as i64;
    (sse.wrapping_sub(((sum * sum) / n) as u32), sse)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The streaming form must equal the materialised one C writes, at every
    /// instantiated size and every phase pair. This is the pin for the
    /// two-heap-allocations-per-call removal: the arithmetic is unchanged, so
    /// the only thing that could differ is the row plumbing.
    #[test]
    fn streaming_matches_materialised() {
        // A deterministic plane with a guard band: the first pass reads
        // `h + 1` rows and `w + 1` columns.
        let stride = 160usize;
        let mut st = 0x2545_F491u32;
        let mut next = || {
            st ^= st << 13;
            st ^= st >> 17;
            st ^= st << 5;
            (st >> 19) as u8
        };
        let a: alloc::vec::Vec<u8> = (0..stride * 160).map(|_| next()).collect();
        let bb: alloc::vec::Vec<u8> = (0..stride * 160).map(|_| next()).collect();
        const SIZES: &[(usize, usize)] = &[
            (128, 128),
            (128, 64),
            (64, 128),
            (64, 64),
            (64, 32),
            (32, 64),
            (32, 32),
            (32, 16),
            (16, 32),
            (16, 16),
            (16, 8),
            (8, 16),
            (8, 8),
            (8, 4),
            (4, 8),
            (4, 4),
            (4, 16),
            (16, 4),
            (8, 32),
            (32, 8),
            (16, 64),
            (64, 16),
        ];
        for &(w, h) in SIZES {
            for x in 0..8 {
                for y in 0..8 {
                    let got = sub_pixel_variance(&a, 3, stride, x, y, &bb, 7, stride, w, h);
                    let want =
                        sub_pixel_variance_materialised(&a, 3, stride, x, y, &bb, 7, stride, w, h);
                    assert_eq!(got, want, "{w}x{h} phase ({x},{y})");
                }
            }
        }
    }

    /// Hand-derived from C: at `xoffset == 0 && yoffset == 0` both filters are
    /// `{128, 0}`, so both passes are the identity and `svf` degenerates to
    /// `vf` on the unshifted block.
    #[test]
    fn zero_phase_is_plain_variance() {
        let mut a = [0u8; 6 * 5];
        for (i, v) in a.iter_mut().enumerate() {
            *v = (i * 7) as u8;
        }
        let b = [4u8; 16];
        let (var, sse) = sub_pixel_variance(&a, 0, 6, 0, 0, &b, 0, 4, 4, 4);
        let (evar, esse) = variance_diff_sse(&a, 0, 6, &b, 0, 4, 4, 4);
        assert_eq!((var, sse), (evar, esse));
    }

    /// The half-pel filter `{64, 64}` must average the two taps with
    /// round-half-up, and the horizontal pass must run BEFORE the vertical.
    /// Hand-derived: a 2x2 read window of a ramp.
    #[test]
    fn half_pel_averages_both_axes() {
        // 3x3 input, block 2x2, phases (4, 4) -> average of a 2x2 neighbourhood.
        let a: [u8; 9] = [0, 10, 20, 30, 40, 50, 60, 70, 80];
        let b = [0u8; 4];
        // first pass (H+1 = 3 rows, W = 2 cols), filter {64,64}:
        //   row0: (0*64+10*64+64)>>7 = 5 ; (10*64+20*64+64)>>7 = 15
        //   row1: 35 ; 45
        //   row2: 65 ; 75
        // second pass, filter {64,64}: row0: (5+35)/2 = 20 ; (15+45)/2 = 30
        //                              row1: (35+65)/2 = 50 ; (45+75)/2 = 60
        // vs b = 0: sum = 160, sse = 400+900+2500+3600 = 7400
        // var = 7400 - (160*160)/4 = 7400 - 6400 = 1000
        let (var, sse) = sub_pixel_variance(&a, 0, 3, 4, 4, &b, 0, 2, 2, 2);
        assert_eq!(sse, 7400);
        assert_eq!(var, 1000);
    }
}

// 2D square DCT-DCT drivers, vectorized across 8 columns/rows. Mirror the
// scalar `inv_txfm2d_core` / `fwd_txfm2d_core` control flow EXACTLY (same
// per-pass clamp/shift order, same stage cos-bits) — only the per-element work
// is done 8-at-a-time. One pass is contiguous across 8 lanes (no transpose);
// the other uses an 8×8 tile transpose (pure data movement). See module docs.
//
// Included into `mod v3`.

use crate::inv_txfm::inv_txfm_ranges;

/// Generate `inv_dct_<N>` and `fwd_dct_<N>` for a square size `N` (multiple of
/// 8) given its 1D inverse/forward kernels.
macro_rules! dct_square_driver {
    ($inv_fn:ident, $fwd_fn:ident, $n:literal, $idct:ident, $fdct:ident) => {
        /// Inverse square DCT-DCT (`N=$n`), no flips, `bd <= 10`. Byte-exact
        /// with `inv_txfm2d_core(.., row_1d=0, col_1d=0)`.
        #[rite]
        pub(super) fn $inv_fn(
            t: Desktop64,
            input: &[i32],
            input_stride: usize,
            output: &mut [i32],
            out_stride: usize,
            bd: u8,
        ) {
            const N: usize = $n;
            const G: usize = N / 8;
            let (row_range, col_range) = inv_txfm_ranges(bd);
            let sh = inv_txfm_shift(N, N);
            let rsh0 = -(sh[0] as i32); // right-shift (>=0) after row pass
            let rsh1 = -(sh[1] as i32); // right-shift (>=0) after col pass
            let rnd = splat(t, 1 << (COS_BIT - 1));
            let shc = _mm_cvtsi32_si128(COS_BIT as i32);
            let row_lo = splat(t, -(1 << (row_range - 1)));
            let row_hi = splat(t, (1 << (row_range - 1)) - 1);
            let col_lo = splat(t, -(1 << (col_range - 1)));
            let col_hi = splat(t, (1 << (col_range - 1)) - 1);
            let int_max = (1i32 << (7 + bd)) - 1 + (914i32 << (bd - 7));
            let wl_lo = splat(t, -int_max - 1);
            let wl_hi = splat(t, int_max);

            let mut buf = [0i32; N * N];

            // ROW PASS — process 8 rows at a time (transpose on load & store).
            for rg in 0..G {
                let rowbase = rg * 8;
                let mut pos = [_mm256_setzero_si256(); N];
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for l in 0..8 {
                        tile[l] = load8(t, input, (rowbase + l) * input_stride + s * 8);
                    }
                    let tt = transpose8(t, &tile);
                    for j in 0..8 {
                        pos[s * 8 + j] = clampv(t, tt[j], row_lo, row_hi);
                    }
                }
                let mut rowout = [_mm256_setzero_si256(); N];
                $idct(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi);
                for i in 0..N {
                    rowout[i] = round_shift_v(t, rowout[i], rsh0);
                }
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for j in 0..8 {
                        tile[j] = rowout[s * 8 + j];
                    }
                    let tt = transpose8(t, &tile);
                    for l in 0..8 {
                        store8(t, &mut buf, (rowbase + l) * N + s * 8, tt[l]);
                    }
                }
            }

            // COLUMN PASS — 8 columns at a time, contiguous (no transpose).
            for cg in 0..G {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); N];
                for r in 0..N {
                    colin[r] = clampv(t, load8(t, &buf, r * N + colbase), col_lo, col_hi);
                }
                let mut colout = [_mm256_setzero_si256(); N];
                $idct(t, &colin, &mut colout, rnd, shc, col_lo, col_hi);
                for r in 0..N {
                    let v = round_shift_v(t, colout[r], rsh1);
                    let v = wraplow(t, v, wl_lo, wl_hi);
                    store8(t, output, r * out_stride + colbase, v);
                }
            }
        }

        /// Forward square DCT-DCT (`N=$n`), no flips. Byte-exact with
        /// `fwd_txfm2d_core(.., col_1d=0, row_1d=0)` (output packed at stride N).
        #[rite]
        pub(super) fn $fwd_fn(
            t: Desktop64,
            input: &[i32],
            output: &mut [i32],
            input_stride: usize,
        ) {
            const N: usize = $n;
            const G: usize = N / 8;
            let shs = fwd_txfm_shift(N, N);
            let pre_col = -(shs[0] as i32); // round_shift_array arg (pre col)
            let post_col = -(shs[1] as i32); // post col
            let post_row = -(shs[2] as i32); // post row
            let txw = N.trailing_zeros() as usize - 2;
            let cos_bit_col = FWD_COS_BIT_COL[txw][txw];
            let cos_bit_row = FWD_COS_BIT_ROW[txw][txw];

            let mut buf = [0i32; N * N];

            // COLUMN PASS first — 8 columns at a time, contiguous.
            for cg in 0..G {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); N];
                for r in 0..N {
                    colin[r] =
                        round_shift_v(t, load8(t, input, r * input_stride + colbase), pre_col);
                }
                let mut colout = [_mm256_setzero_si256(); N];
                $fdct(t, &colin, &mut colout, cos_bit_col);
                for r in 0..N {
                    let v = round_shift_v(t, colout[r], post_col);
                    store8(t, &mut buf, r * N + colbase, v);
                }
            }

            // ROW PASS — 8 rows at a time (transpose on load & store).
            for rg in 0..G {
                let rowbase = rg * 8;
                let mut pos = [_mm256_setzero_si256(); N];
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for l in 0..8 {
                        tile[l] = load8(t, &buf, (rowbase + l) * N + s * 8);
                    }
                    let tt = transpose8(t, &tile);
                    for j in 0..8 {
                        pos[s * 8 + j] = tt[j];
                    }
                }
                let mut rowout = [_mm256_setzero_si256(); N];
                $fdct(t, &pos, &mut rowout, cos_bit_row);
                for i in 0..N {
                    rowout[i] = round_shift_v(t, rowout[i], post_row);
                }
                for s in 0..G {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for j in 0..8 {
                        tile[j] = rowout[s * 8 + j];
                    }
                    let tt = transpose8(t, &tile);
                    for l in 0..8 {
                        store8(t, output, (rowbase + l) * N + s * 8, tt[l]);
                    }
                }
            }
        }
    };
}

dct_square_driver!(inv_dct_16, fwd_dct_16, 16, idct16_x8, fdct16_x8);

/// Inverse square DCT-DCT dispatcher. Returns true if `n` has a SIMD kernel.
#[rite]
pub(super) fn inv_dct_square(
    t: Desktop64,
    input: &[i32],
    input_stride: usize,
    output: &mut [i32],
    out_stride: usize,
    n: usize,
    bd: u8,
) -> bool {
    match n {
        16 => {
            inv_dct_16(t, input, input_stride, output, out_stride, bd);
            true
        }
        _ => false,
    }
}

/// Forward square DCT-DCT dispatcher. Returns true if `n` has a SIMD kernel.
#[rite]
pub(super) fn fwd_dct_square(
    t: Desktop64,
    input: &[i32],
    output: &mut [i32],
    input_stride: usize,
    n: usize,
) -> bool {
    match n {
        16 => {
            fwd_dct_16(t, input, output, input_stride);
            true
        }
        _ => false,
    }
}

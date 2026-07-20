// 2D NON-SQUARE (rectangular) transform drivers, vectorized across 8
// columns/rows. Mirror the scalar `fwd_txfm2d_core` / `inv_txfm2d_core` control
// flow EXACTLY for `w != h`: the column pass runs a size-`h` 1D transform down
// each of the `w` columns, the row pass runs a size-`w` 1D transform along each
// of the `h` rows, and the `NewSqrt2` / `NewInvSqrt2` scale is applied on the
// row output (fwd) / row input (inv) **iff** `|log2(w) - log2(h)| == 1` (2:1
// rectangles) — 4:1 rectangles get NO extra scale, exactly like C.
//
// The 1D kernels are the SAME proven `fdct*_x8` / `idct*_x8` used by the square
// path; only the driver differs (asymmetric pass sizes + the rect scale). See
// the `rect_scale` primitive for the byte-exact i64 scale. Included into `mod
// v3`. Additive — no scalar path is modified.

// -- forward: DCT-DCT rectangular --------------------------------------------

/// Generate a forward rectangular DCT-DCT driver for `W x H` (both multiples of
/// 8). `colk` is the size-`H` 1D forward kernel (column pass), `rowk` the
/// size-`W` kernel (row pass). Byte-exact with `fwd_txfm2d_core(.., col_1d=0,
/// row_1d=0)` for `w=W, h=H` and no flips.
macro_rules! fwd_rect_driver {
    ($fn:ident, $w:literal, $h:literal, $colk:ident, $rowk:ident) => {
        #[rite]
        pub(super) fn $fn(t: Desktop64, input: &[i32], output: &mut [i32], input_stride: usize) {
            const W: usize = $w;
            const H: usize = $h;
            const WG: usize = W / 8; // groups of 8 columns
            let shs = fwd_txfm_shift(W, H);
            let pre_col = -(shs[0] as i32);
            let post_col = -(shs[1] as i32);
            let post_row = -(shs[2] as i32);
            let txw = W.trailing_zeros() as usize - 2;
            let txh = H.trailing_zeros() as usize - 2;
            let cos_bit_col = FWD_COS_BIT_COL[txw][txh];
            let cos_bit_row = FWD_COS_BIT_ROW[txw][txh];
            let rect_ratio1 = (txw as i32 - txh as i32).abs() == 1;

            let mut buf = [0i32; W * H];

            // COLUMN PASS — `W` columns (WG groups of 8), size-`H` transform,
            // contiguous down each column (no transpose).
            for cg in 0..WG {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    colin[r] =
                        round_shift_v(t, load8(t, input, r * input_stride + colbase), pre_col);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                $colk(t, &colin, &mut colout, cos_bit_col);
                for r in 0..H {
                    store8(t, &mut buf, r * W + colbase, round_shift_v(t, colout[r], post_col));
                }
            }

            // ROW PASS — `H` rows (H/8 groups of 8), size-`W` transform (load &
            // store via 8×8 tile transpose). Rect scale on the row output.
            for rg in 0..(H / 8) {
                let rowbase = rg * 8;
                let mut pos = [_mm256_setzero_si256(); W];
                for s in 0..WG {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for l in 0..8 {
                        tile[l] = load8(t, &buf, (rowbase + l) * W + s * 8);
                    }
                    let tt = transpose8(t, &tile);
                    for j in 0..8 {
                        pos[s * 8 + j] = tt[j];
                    }
                }
                let mut rowout = [_mm256_setzero_si256(); W];
                $rowk(t, &pos, &mut rowout, cos_bit_row);
                for i in 0..W {
                    let mut v = round_shift_v(t, rowout[i], post_row);
                    if rect_ratio1 {
                        v = rect_scale(t, v, NEW_SQRT2);
                    }
                    rowout[i] = v;
                }
                for s in 0..WG {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for j in 0..8 {
                        tile[j] = rowout[s * 8 + j];
                    }
                    let tt = transpose8(t, &tile);
                    for l in 0..8 {
                        store8(t, output, (rowbase + l) * W + s * 8, tt[l]);
                    }
                }
            }
        }
    };
}

// -- inverse: DCT-DCT rectangular --------------------------------------------

/// Generate an inverse rectangular DCT-DCT driver for `W x H`. `rowk` is the
/// size-`W` inverse kernel (row pass), `colk` the size-`H` kernel (column
/// pass). Byte-exact with `inv_txfm2d_core(.., row_1d=0, col_1d=0)`, `bd <= 10`.
/// The rect scale (`NewInvSqrt2`) is applied on the row INPUT before the
/// row-range clamp, matching C.
macro_rules! inv_rect_driver {
    ($fn:ident, $w:literal, $h:literal, $rowk:ident, $colk:ident) => {
        #[rite]
        pub(super) fn $fn(
            t: Desktop64,
            input: &[i32],
            input_stride: usize,
            output: &mut [i32],
            out_stride: usize,
            bd: u8,
        ) {
            const W: usize = $w;
            const H: usize = $h;
            const WG: usize = W / 8;
            let (row_range, col_range) = inv_txfm_ranges(bd);
            let sh = inv_txfm_shift(W, H);
            let rsh0 = -(sh[0] as i32);
            let rsh1 = -(sh[1] as i32);
            let rnd = splat(t, 1 << (COS_BIT - 1));
            let shc = _mm_cvtsi32_si128(COS_BIT as i32);
            let row_lo = splat(t, -(1 << (row_range - 1)));
            let row_hi = splat(t, (1 << (row_range - 1)) - 1);
            let col_lo = splat(t, -(1 << (col_range - 1)));
            let col_hi = splat(t, (1 << (col_range - 1)) - 1);
            let int_max = (1i32 << (7 + bd)) - 1 + (914i32 << (bd - 7));
            let wl_lo = splat(t, -int_max - 1);
            let wl_hi = splat(t, int_max);
            let txw = W.trailing_zeros() as i32 - 2;
            let txh = H.trailing_zeros() as i32 - 2;
            let rect_ratio1 = (txw - txh).abs() == 1;

            let mut buf = [0i32; W * H];

            // ROW PASS — `H` rows (H/8 groups of 8), size-`W` transform. Rect
            // scale on the row input, THEN the row-range clamp.
            for rg in 0..(H / 8) {
                let rowbase = rg * 8;
                let mut pos = [_mm256_setzero_si256(); W];
                for s in 0..WG {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for l in 0..8 {
                        tile[l] = load8(t, input, (rowbase + l) * input_stride + s * 8);
                    }
                    let tt = transpose8(t, &tile);
                    for j in 0..8 {
                        let mut v = tt[j];
                        if rect_ratio1 {
                            v = rect_scale(t, v, NEW_INV_SQRT2);
                        }
                        pos[s * 8 + j] = clampv(t, v, row_lo, row_hi);
                    }
                }
                let mut rowout = [_mm256_setzero_si256(); W];
                $rowk(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi);
                for i in 0..W {
                    rowout[i] = round_shift_v(t, rowout[i], rsh0);
                }
                for s in 0..WG {
                    let mut tile = [_mm256_setzero_si256(); 8];
                    for j in 0..8 {
                        tile[j] = rowout[s * 8 + j];
                    }
                    let tt = transpose8(t, &tile);
                    for l in 0..8 {
                        store8(t, &mut buf, (rowbase + l) * W + s * 8, tt[l]);
                    }
                }
            }

            // COLUMN PASS — `W` columns (WG groups of 8), size-`H` transform,
            // contiguous (no transpose).
            for cg in 0..WG {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    colin[r] = clampv(t, load8(t, &buf, r * W + colbase), col_lo, col_hi);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                $colk(t, &colin, &mut colout, rnd, shc, col_lo, col_hi);
                for r in 0..H {
                    let v = round_shift_v(t, colout[r], rsh1);
                    let v = wraplow(t, v, wl_lo, wl_hi);
                    store8(t, output, r * out_stride + colbase, v);
                }
            }
        }
    };
}

// All ten non-square DCT-DCT sizes with both dims a multiple of 8 (the 4-dim
// rects — 4x8/8x4/4x16/16x4 — stay scalar, narrower than a lane group).
fwd_rect_driver!(fwd_dct_8x16, 8, 16, fdct16_x8, fdct8_x8);
fwd_rect_driver!(fwd_dct_16x8, 16, 8, fdct8_x8, fdct16_x8);
fwd_rect_driver!(fwd_dct_16x32, 16, 32, fdct32_x8, fdct16_x8);
fwd_rect_driver!(fwd_dct_32x16, 32, 16, fdct16_x8, fdct32_x8);
fwd_rect_driver!(fwd_dct_32x64, 32, 64, fdct64_x8, fdct32_x8);
fwd_rect_driver!(fwd_dct_64x32, 64, 32, fdct32_x8, fdct64_x8);
fwd_rect_driver!(fwd_dct_8x32, 8, 32, fdct32_x8, fdct8_x8);
fwd_rect_driver!(fwd_dct_32x8, 32, 8, fdct8_x8, fdct32_x8);
fwd_rect_driver!(fwd_dct_16x64, 16, 64, fdct64_x8, fdct16_x8);
fwd_rect_driver!(fwd_dct_64x16, 64, 16, fdct16_x8, fdct64_x8);

inv_rect_driver!(inv_dct_8x16, 8, 16, idct8_x8, idct16_x8);
inv_rect_driver!(inv_dct_16x8, 16, 8, idct16_x8, idct8_x8);
inv_rect_driver!(inv_dct_16x32, 16, 32, idct16_x8, idct32_x8);
inv_rect_driver!(inv_dct_32x16, 32, 16, idct32_x8, idct16_x8);
inv_rect_driver!(inv_dct_32x64, 32, 64, idct32_x8, idct64_x8);
inv_rect_driver!(inv_dct_64x32, 64, 32, idct64_x8, idct32_x8);
inv_rect_driver!(inv_dct_8x32, 8, 32, idct8_x8, idct32_x8);
inv_rect_driver!(inv_dct_32x8, 32, 8, idct32_x8, idct8_x8);
inv_rect_driver!(inv_dct_16x64, 16, 64, idct16_x8, idct64_x8);
inv_rect_driver!(inv_dct_64x16, 64, 16, idct64_x8, idct16_x8);

/// Forward rectangular DCT-DCT dispatcher. Returns true if `(w,h)` has a SIMD
/// kernel (both dims a multiple of 8, `w != h`).
#[rite]
pub(super) fn fwd_dct_rect(
    t: Desktop64,
    input: &[i32],
    output: &mut [i32],
    input_stride: usize,
    w: usize,
    h: usize,
) -> bool {
    match (w, h) {
        (8, 16) => fwd_dct_8x16(t, input, output, input_stride),
        (16, 8) => fwd_dct_16x8(t, input, output, input_stride),
        (16, 32) => fwd_dct_16x32(t, input, output, input_stride),
        (32, 16) => fwd_dct_32x16(t, input, output, input_stride),
        (32, 64) => fwd_dct_32x64(t, input, output, input_stride),
        (64, 32) => fwd_dct_64x32(t, input, output, input_stride),
        (8, 32) => fwd_dct_8x32(t, input, output, input_stride),
        (32, 8) => fwd_dct_32x8(t, input, output, input_stride),
        (16, 64) => fwd_dct_16x64(t, input, output, input_stride),
        (64, 16) => fwd_dct_64x16(t, input, output, input_stride),
        _ => return false,
    }
    true
}

/// Inverse rectangular DCT-DCT dispatcher. Returns true if `(w,h)` has a SIMD
/// kernel.
#[rite]
pub(super) fn inv_dct_rect(
    t: Desktop64,
    input: &[i32],
    input_stride: usize,
    output: &mut [i32],
    out_stride: usize,
    w: usize,
    h: usize,
    bd: u8,
) -> bool {
    match (w, h) {
        (8, 16) => inv_dct_8x16(t, input, input_stride, output, out_stride, bd),
        (16, 8) => inv_dct_16x8(t, input, input_stride, output, out_stride, bd),
        (16, 32) => inv_dct_16x32(t, input, input_stride, output, out_stride, bd),
        (32, 16) => inv_dct_32x16(t, input, input_stride, output, out_stride, bd),
        (32, 64) => inv_dct_32x64(t, input, input_stride, output, out_stride, bd),
        (64, 32) => inv_dct_64x32(t, input, input_stride, output, out_stride, bd),
        (8, 32) => inv_dct_8x32(t, input, input_stride, output, out_stride, bd),
        (32, 8) => inv_dct_32x8(t, input, input_stride, output, out_stride, bd),
        (16, 64) => inv_dct_16x64(t, input, input_stride, output, out_stride, bd),
        (64, 16) => inv_dct_64x16(t, input, input_stride, output, out_stride, bd),
        _ => return false,
    }
    true
}

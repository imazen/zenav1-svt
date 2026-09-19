// AVX-512 2D square forward DCT-DCT drivers, vectorized across 16
// columns/rows. Mirrors the `fwd` half of `dct_square_driver!` in
// `txfm_simd_drivers.rs` EXACTLY (same per-pass shift order, same stage
// cos-bits) — only the per-element width doubles. Included into `mod v4`.
//
// The row pass still transposes 16x16 tiles, built from four 8x8 `ymm`
// transposes (the proven `transpose8` sequence applied per quadrant) —
// `transpose16` is pure data movement, so bit-exactness is preserved.

// Per-thread transform staging buffer — same contract as `mod v3`'s:
// every driver writes each element in its first pass before the second
// pass reads it.
#[cfg(feature = "std")]
std::thread_local! {
    static TXFM_STAGE_V4: core::cell::RefCell<[i32; 4096]> =
        const { core::cell::RefCell::new([0; 4096]) };
}

/// Run `f` with a staging buffer of at least `n` i32 (stale contents).
#[inline]
fn stage_buf<R>(n: usize, f: impl FnOnce(&mut [i32]) -> R) -> R {
    debug_assert!(n <= 4096);
    #[cfg(feature = "std")]
    {
        TXFM_STAGE_V4.with(|c| f(&mut c.borrow_mut()[..n]))
    }
    #[cfg(not(feature = "std"))]
    {
        let mut buf = alloc::vec![0i32; n];
        f(&mut buf)
    }
}

/// Generate `fwd_dct_<N>` for a square size `N` (multiple of 16) given its
/// 1D forward kernel.
macro_rules! fwd_dct_square_driver_v4 {
    ($fwd_fn:ident, $n:literal, $fdct:ident) => {
        /// Forward square DCT-DCT (`N=$n`), no flips. Byte-exact with
        /// `fwd_txfm2d_core(.., col_1d=0, row_1d=0)` (output packed at
        /// stride N).
        #[rite]
        pub(super) fn $fwd_fn(
            t: X64V4Token,
            input: &[i16],
            output: &mut [i32],
            input_stride: usize,
        ) {
            const N: usize = $n;
            const G: usize = N / 16;
            let shs = fwd_txfm_shift(N, N);
            let pre_col = -(shs[0] as i32); // round_shift_array arg (pre col)
            let post_col = -(shs[1] as i32); // post col
            let post_row = -(shs[2] as i32); // post row
            let txw = N.trailing_zeros() as usize - 2;
            let cos_bit_col = FWD_COS_BIT_COL[txw][txw];
            let cos_bit_row = FWD_COS_BIT_ROW[txw][txw];

            stage_buf(N * N, |buf| {
                // COLUMN PASS — 16 columns at a time, contiguous.
                for cg in 0..G {
                    let colbase = cg * 16;
                    let mut colin = [_mm512_setzero_si512(); N];
                    for r in 0..N {
                        colin[r] = round_shift_v(
                            t,
                            load16w(t, input, r * input_stride + colbase),
                            pre_col,
                        );
                    }
                    let mut colout = [_mm512_setzero_si512(); N];
                    $fdct(t, &colin, &mut colout, cos_bit_col);
                    for r in 0..N {
                        let v = round_shift_v(t, colout[r], post_col);
                        store16(t, buf, r * N + colbase, v);
                    }
                }

                // ROW PASS — 16 rows at a time (transpose on load & store).
                for rg in 0..G {
                    let rowbase = rg * 16;
                    let mut pos = [_mm512_setzero_si512(); N];
                    for s in 0..G {
                        let mut tile = [_mm512_setzero_si512(); 16];
                        for l in 0..16 {
                            tile[l] = load16(t, buf, (rowbase + l) * N + s * 16);
                        }
                        let tt = transpose16(t, &tile);
                        for j in 0..16 {
                            pos[s * 16 + j] = tt[j];
                        }
                    }
                    let mut rowout = [_mm512_setzero_si512(); N];
                    $fdct(t, &pos, &mut rowout, cos_bit_row);
                    for i in 0..N {
                        rowout[i] = round_shift_v(t, rowout[i], post_row);
                    }
                    for s in 0..G {
                        let mut tile = [_mm512_setzero_si512(); 16];
                        for j in 0..16 {
                            tile[j] = rowout[s * 16 + j];
                        }
                        let tt = transpose16(t, &tile);
                        for l in 0..16 {
                            store16(t, output, (rowbase + l) * N + s * 16, tt[l]);
                        }
                    }
                }
            })
        }
    };
}

fwd_dct_square_driver_v4!(fwd_dct_16, 16, fdct16_x16);
fwd_dct_square_driver_v4!(fwd_dct_32, 32, fdct32_x16);
fwd_dct_square_driver_v4!(fwd_dct_64, 64, fdct64_x16);

/// Forward square DCT-DCT dispatcher at 16 lanes. Returns false for `n`
/// without a 16-wide kernel (`n == 8` — the `v4` caller delegates that to
/// `v3`, which is always available when `v4` is).
#[rite]
pub(super) fn fwd_dct_square(
    t: X64V4Token,
    input: &[i16],
    output: &mut [i32],
    input_stride: usize,
    n: usize,
) -> bool {
    match n {
        16 => {
            fwd_dct_16(t, input, output, input_stride);
            true
        }
        32 => {
            fwd_dct_32(t, input, output, input_stride);
            true
        }
        64 => {
            fwd_dct_64(t, input, output, input_stride);
            true
        }
        _ => false,
    }
}

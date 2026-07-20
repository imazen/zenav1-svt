// 2D "extended" transform drivers — the FLIPADST, IDENTITY, and mixed V_/H_
// tx types the DCT / ADST paths leave scalar. Included into `mod v3`. Additive:
// no scalar path and no existing SIMD driver is modified.
//
// Two structural additions over the DCT/ADST rect drivers:
//
//   * The block edge FLIP (FLIPADST): `ud_flip` reverses the column-input row
//     read (forward) / column-output row store (inverse); `lr_flip` reverses
//     the column-output write to `buf` (forward) / column-input gather from
//     `buf` (inverse). These are pure index/lane reversals — no arithmetic
//     change — so the result is byte-identical to `fwd/inv_txfm2d_core` with the
//     matching flags. FLIPADST reuses the SAME proven `fadst*/iadst*` kernels
//     (`get_*_txfm_func` maps FLIPADST → the ADST kernel; the flip is external).
//     `ud_flip == (col_1d == FLIPADST)`, `lr_flip == (row_1d == FLIPADST)`
//     (`get_flip_cfg`, txfm_dispatch.rs).
//
//   * The IDENTITY 1D kernels — a per-size NewSqrt2 scale (`fidentity*` /
//     `iidentity*`), vectorized as a drop-in for the DCT/ADST `_x8` kernels so
//     the same driver machinery composes them. IDTX (identity on both axes)
//     reuses the EXISTING square / rect DCT drivers with the identity kernels
//     (the 2D core is 1D-type-agnostic — same shifts, same clamps); the mixed
//     V_/H_ types (identity on one axis, DCT/ADST/FLIPADST on the other) go
//     through the flip-capable `*_ext_*` drivers below.

// ---------------------------------------------------------------------------
// Primitive: reverse the 8 i32 lanes (the FLIPADST left-right block mirror).
// ---------------------------------------------------------------------------

/// `out.lane(i) = in.lane(7 - i)` — a single `vpermd`, pure data movement →
/// bit-exact. Used for the `lr_flip` horizontal mirror of a block's column
/// outputs (forward) / inputs (inverse).
#[rite]
pub(super) fn reverse8(_t: Desktop64, v: __m256i) -> __m256i {
    // set_epi32(e7,..,e0) puts e7 in lane 7; so this is idx[i] = 7 - i.
    let idx = _mm256_set_epi32(0, 1, 2, 3, 4, 5, 6, 7);
    _mm256_permutevar8x32_epi32(v, idx)
}

// ---------------------------------------------------------------------------
// Identity 1D kernels — drop-in for the `fdct*_x8` / `idct*_x8` kernels.
//
// Exact per-size scale from `fidentity*` (fwd_txfm.rs) / `iidentity*`
// (inv_txfm.rs), identical forward and inverse:
//   size  8: v * 2                         → `slli_epi32::<1>`
//   size 16: round_shift(v * 2*NewSqrt2,12) → `rect_scale(v, 2*NEW_SQRT2)`
//   size 32: v * 4                         → `slli_epi32::<2>`
// `v * 2` / `v * 4` are exact bit shifts for every valid coefficient (the
// scalar uses `input[i] * {2,4}`; `<<n` == `*2^n` in two's complement, no
// overflow in range). The 16-scale reuses the proven i64 `rect_scale`.
// cos_bit / rnd / sh / lo / hi are ignored, exactly like the scalar identity.
// ---------------------------------------------------------------------------

#[rite]
pub(super) fn fidentity8_x8(_t: Desktop64, inp: &[__m256i; 8], out: &mut [__m256i; 8], _cos_bit: i8) {
    for i in 0..8 {
        out[i] = _mm256_slli_epi32::<1>(inp[i]);
    }
}
#[rite]
pub(super) fn fidentity16_x8(
    t: Desktop64,
    inp: &[__m256i; 16],
    out: &mut [__m256i; 16],
    _cos_bit: i8,
) {
    for i in 0..16 {
        out[i] = rect_scale(t, inp[i], 2 * NEW_SQRT2);
    }
}
#[rite]
pub(super) fn fidentity32_x8(
    _t: Desktop64,
    inp: &[__m256i; 32],
    out: &mut [__m256i; 32],
    _cos_bit: i8,
) {
    for i in 0..32 {
        out[i] = _mm256_slli_epi32::<2>(inp[i]);
    }
}

#[rite]
pub(super) fn iidentity8_x8(
    _t: Desktop64,
    inp: &[__m256i; 8],
    out: &mut [__m256i; 8],
    _rnd: __m256i,
    _sh: __m128i,
    _lo: __m256i,
    _hi: __m256i,
) {
    for i in 0..8 {
        out[i] = _mm256_slli_epi32::<1>(inp[i]);
    }
}
#[rite]
pub(super) fn iidentity16_x8(
    t: Desktop64,
    inp: &[__m256i; 16],
    out: &mut [__m256i; 16],
    _rnd: __m256i,
    _sh: __m128i,
    _lo: __m256i,
    _hi: __m256i,
) {
    for i in 0..16 {
        out[i] = rect_scale(t, inp[i], 2 * NEW_SQRT2);
    }
}
#[rite]
pub(super) fn iidentity32_x8(
    _t: Desktop64,
    inp: &[__m256i; 32],
    out: &mut [__m256i; 32],
    _rnd: __m256i,
    _sh: __m128i,
    _lo: __m256i,
    _hi: __m256i,
) {
    for i in 0..32 {
        out[i] = _mm256_slli_epi32::<2>(inp[i]);
    }
}

// ---------------------------------------------------------------------------
// IDTX (identity on BOTH axes) — reuse the DCT square / rect drivers with the
// identity kernels. The 2D transform core is 1D-type-agnostic (per-size shifts,
// per-pass clamps, and the 2:1 rect NewSqrt2 scale are identical for IDTX and
// DCT_DCT), so plugging the identity kernels reproduces C's IDTX byte-exactly.
// ---------------------------------------------------------------------------

dct_square_driver!(inv_idtx_8, fwd_idtx_8, 8, iidentity8_x8, fidentity8_x8);
dct_square_driver!(inv_idtx_16, fwd_idtx_16, 16, iidentity16_x8, fidentity16_x8);
dct_square_driver!(inv_idtx_32, fwd_idtx_32, 32, iidentity32_x8, fidentity32_x8);

fwd_rect_driver!(fwd_idtx_8x16, 8, 16, fidentity16_x8, fidentity8_x8);
fwd_rect_driver!(fwd_idtx_16x8, 16, 8, fidentity8_x8, fidentity16_x8);
fwd_rect_driver!(fwd_idtx_16x32, 16, 32, fidentity32_x8, fidentity16_x8);
fwd_rect_driver!(fwd_idtx_32x16, 32, 16, fidentity16_x8, fidentity32_x8);
fwd_rect_driver!(fwd_idtx_8x32, 8, 32, fidentity32_x8, fidentity8_x8);
fwd_rect_driver!(fwd_idtx_32x8, 32, 8, fidentity8_x8, fidentity32_x8);

inv_rect_driver!(inv_idtx_8x16, 8, 16, iidentity8_x8, iidentity16_x8);
inv_rect_driver!(inv_idtx_16x8, 16, 8, iidentity16_x8, iidentity8_x8);
inv_rect_driver!(inv_idtx_16x32, 16, 32, iidentity16_x8, iidentity32_x8);
inv_rect_driver!(inv_idtx_32x16, 32, 16, iidentity32_x8, iidentity16_x8);
inv_rect_driver!(inv_idtx_8x32, 8, 32, iidentity8_x8, iidentity32_x8);
inv_rect_driver!(inv_idtx_32x8, 32, 8, iidentity32_x8, iidentity8_x8);

// ---------------------------------------------------------------------------
// Flip-capable EXT drivers (FLIPADST + mixed V_/H_ identity types) for the
// sizes where all of DCT / ADST / IDENTITY 1D kernels exist (both dims in
// {8, 16}): 8x8, 16x16, 8x16, 16x8. The kernel per axis is chosen at runtime
// from `col_1d` / `row_1d` (0=DCT, 1|2=ADST[=FLIPADST], 3=IDENTITY); `ud`/`lr`
// carry the FLIPADST block edge flip. Byte-exact with `fwd/inv_txfm2d_core`.
// ---------------------------------------------------------------------------

/// Forward flip-capable driver for `W x H`. `$c*` are the size-`H` column
/// kernels, `$r*` the size-`W` row kernels (DCT / ADST / IDENTITY).
macro_rules! fwd_ext_driver {
    ($fn:ident, $w:literal, $h:literal,
     $cdct:ident, $cadst:ident, $cid:ident,
     $rdct:ident, $radst:ident, $rid:ident) => {
        #[rite]
        #[allow(clippy::too_many_arguments)]
        pub(super) fn $fn(
            t: Desktop64,
            input: &[i32],
            output: &mut [i32],
            input_stride: usize,
            col_1d: u8,
            row_1d: u8,
            ud: bool,
            lr: bool,
        ) {
            const W: usize = $w;
            const H: usize = $h;
            const WG: usize = W / 8;
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

            // COLUMN PASS (size H) — `ud` reverses the input row read, `lr`
            // reverse-mirrors the output write to `buf`.
            for cg in 0..WG {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    let src_row = if ud { H - 1 - r } else { r };
                    colin[r] =
                        round_shift_v(t, load8(t, input, src_row * input_stride + colbase), pre_col);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                match col_1d {
                    0 => $cdct(t, &colin, &mut colout, cos_bit_col),
                    1 | 2 => $cadst(t, &colin, &mut colout, cos_bit_col),
                    _ => $cid(t, &colin, &mut colout, cos_bit_col),
                }
                for r in 0..H {
                    let v = round_shift_v(t, colout[r], post_col);
                    if lr {
                        store8(t, &mut buf, r * W + (W - colbase - 8), reverse8(t, v));
                    } else {
                        store8(t, &mut buf, r * W + colbase, v);
                    }
                }
            }

            // ROW PASS (size W) — no flip; rect NewSqrt2 scale on 2:1 rects.
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
                match row_1d {
                    0 => $rdct(t, &pos, &mut rowout, cos_bit_row),
                    1 | 2 => $radst(t, &pos, &mut rowout, cos_bit_row),
                    _ => $rid(t, &pos, &mut rowout, cos_bit_row),
                }
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

/// Inverse flip-capable driver for `W x H`. `$r*` are the size-`W` row kernels,
/// `$c*` the size-`H` column kernels (DCT / ADST / IDENTITY). `bd <= 10`.
macro_rules! inv_ext_driver {
    ($fn:ident, $w:literal, $h:literal,
     $rdct:ident, $radst:ident, $rid:ident,
     $cdct:ident, $cadst:ident, $cid:ident) => {
        #[rite]
        #[allow(clippy::too_many_arguments)]
        pub(super) fn $fn(
            t: Desktop64,
            input: &[i32],
            input_stride: usize,
            output: &mut [i32],
            out_stride: usize,
            col_1d: u8,
            row_1d: u8,
            ud: bool,
            lr: bool,
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

            // ROW PASS (size W) — no flip; rect scale on input, then row clamp.
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
                match row_1d {
                    0 => $rdct(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                    1 | 2 => $radst(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                    _ => $rid(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                }
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

            // COLUMN PASS (size H) — `lr` reverse-mirrors the `buf` gather,
            // `ud` reverses the output row store.
            for cg in 0..WG {
                let colbase = cg * 8;
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    let v = if lr {
                        reverse8(t, load8(t, &buf, r * W + (W - colbase - 8)))
                    } else {
                        load8(t, &buf, r * W + colbase)
                    };
                    colin[r] = clampv(t, v, col_lo, col_hi);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                match col_1d {
                    0 => $cdct(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                    1 | 2 => $cadst(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                    _ => $cid(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                }
                for r in 0..H {
                    let src = if ud { colout[H - 1 - r] } else { colout[r] };
                    let v = round_shift_v(t, src, rsh1);
                    let v = wraplow(t, v, wl_lo, wl_hi);
                    store8(t, output, r * out_stride + colbase, v);
                }
            }
        }
    };
}

// col kernels = size-h; row kernels = size-w.
fwd_ext_driver!(fwd_ext_8x8, 8, 8,
    fdct8_x8, fadst8_x8, fidentity8_x8,
    fdct8_x8, fadst8_x8, fidentity8_x8);
fwd_ext_driver!(fwd_ext_16x16, 16, 16,
    fdct16_x8, fadst16_x8, fidentity16_x8,
    fdct16_x8, fadst16_x8, fidentity16_x8);
fwd_ext_driver!(fwd_ext_8x16, 8, 16,
    fdct16_x8, fadst16_x8, fidentity16_x8,
    fdct8_x8, fadst8_x8, fidentity8_x8);
fwd_ext_driver!(fwd_ext_16x8, 16, 8,
    fdct8_x8, fadst8_x8, fidentity8_x8,
    fdct16_x8, fadst16_x8, fidentity16_x8);

// inverse: row kernels = size-w; col kernels = size-h.
inv_ext_driver!(inv_ext_8x8, 8, 8,
    idct8_x8, iadst8_x8, iidentity8_x8,
    idct8_x8, iadst8_x8, iidentity8_x8);
inv_ext_driver!(inv_ext_16x16, 16, 16,
    idct16_x8, iadst16_x8, iidentity16_x8,
    idct16_x8, iadst16_x8, iidentity16_x8);
inv_ext_driver!(inv_ext_8x16, 8, 16,
    idct8_x8, iadst8_x8, iidentity8_x8,
    idct16_x8, iadst16_x8, iidentity16_x8);
inv_ext_driver!(inv_ext_16x8, 16, 8,
    idct16_x8, iadst16_x8, iidentity16_x8,
    idct8_x8, iadst8_x8, iidentity8_x8);

/// Forward EXT dispatcher: FLIPADST / IDENTITY / mixed V_/H_ types.
/// `col_1d`/`row_1d` ∈ {0=DCT,1=ADST,2=FLIPADST,3=IDENTITY} with at least one
/// >= 2; `ud`/`lr` are the FLIPADST edge flips. Returns true if handled.
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn fwd_ext(
    t: Desktop64,
    input: &[i32],
    output: &mut [i32],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
) -> bool {
    if col_1d == 3 && row_1d == 3 {
        match (w, h) {
            (8, 8) => fwd_idtx_8(t, input, output, input_stride),
            (16, 16) => fwd_idtx_16(t, input, output, input_stride),
            (32, 32) => fwd_idtx_32(t, input, output, input_stride),
            (8, 16) => fwd_idtx_8x16(t, input, output, input_stride),
            (16, 8) => fwd_idtx_16x8(t, input, output, input_stride),
            (16, 32) => fwd_idtx_16x32(t, input, output, input_stride),
            (32, 16) => fwd_idtx_32x16(t, input, output, input_stride),
            (8, 32) => fwd_idtx_8x32(t, input, output, input_stride),
            (32, 8) => fwd_idtx_32x8(t, input, output, input_stride),
            _ => return false,
        }
        return true;
    }
    match (w, h) {
        (8, 8) => fwd_ext_8x8(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (16, 16) => fwd_ext_16x16(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (8, 16) => fwd_ext_8x16(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (16, 8) => fwd_ext_16x8(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        _ => return false,
    }
    true
}

/// Inverse EXT dispatcher. Same contract as [`fwd_ext`], `bd <= 10`.
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn inv_ext(
    t: Desktop64,
    input: &[i32],
    input_stride: usize,
    output: &mut [i32],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    ud: bool,
    lr: bool,
    bd: u8,
) -> bool {
    if col_1d == 3 && row_1d == 3 {
        match (w, h) {
            (8, 8) => inv_idtx_8(t, input, input_stride, output, out_stride, bd),
            (16, 16) => inv_idtx_16(t, input, input_stride, output, out_stride, bd),
            (32, 32) => inv_idtx_32(t, input, input_stride, output, out_stride, bd),
            (8, 16) => inv_idtx_8x16(t, input, input_stride, output, out_stride, bd),
            (16, 8) => inv_idtx_16x8(t, input, input_stride, output, out_stride, bd),
            (16, 32) => inv_idtx_16x32(t, input, input_stride, output, out_stride, bd),
            (32, 16) => inv_idtx_32x16(t, input, input_stride, output, out_stride, bd),
            (8, 32) => inv_idtx_8x32(t, input, input_stride, output, out_stride, bd),
            (32, 8) => inv_idtx_32x8(t, input, input_stride, output, out_stride, bd),
            _ => return false,
        }
        return true;
    }
    match (w, h) {
        (8, 8) => inv_ext_8x8(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        (16, 16) => {
            inv_ext_16x16(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd)
        }
        (8, 16) => inv_ext_8x16(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        (16, 8) => inv_ext_16x8(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        _ => return false,
    }
    true
}

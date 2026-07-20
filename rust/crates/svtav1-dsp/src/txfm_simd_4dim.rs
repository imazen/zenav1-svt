// 2D transform drivers for the 4-dim sizes (4x4, 4x8, 8x4, 4x16, 16x4) — the
// sizes a prior agent left scalar because a 4-wide axis is narrower than an
// 8-lane group. Included into `mod v3`. Additive.
//
// Structure: same column-then-row (fwd) / row-then-column (inv) composition as
// the DCT/ADST/ext drivers, but a pass along the 4-wide axis can't fill 8 lanes
// and the row pass reads `buf` column-wise (strided). Both are handled with an
// array-based gather/scatter (build an `[i32; 8]`, `load8`/`store8`), which is
// byte-exact by construction regardless of stride or lane count; the contiguous
// wide passes (8x4/16x4 column pass) take a direct `load8`/`store8` fast path.
// All 16 tx types are legal at these sizes (max dim <= 16 -> full ext_tx set),
// so the drivers pick the per-axis kernel (DCT/ADST[=FLIPADST]/IDENTITY) at
// runtime from col_1d/row_1d, with ud/lr carrying the FLIPADST edge flip.

// ---------------------------------------------------------------------------
// 4-point 1D kernels — the `_x8` form (8 independent columns/rows in the 8
// lanes). Transcribed op-for-op from the scalar 4-point kernels; the sinpi
// ADST (fadst4/iadst4) matches C's int32 arithmetic exactly via
// `_mm256_mullo_epi32` (the scalar's i64 is identical for the conformant range,
// as the `c_parity_txfm` differential proves against real C).
// ---------------------------------------------------------------------------

/// 4-point forward DCT (svt_av1_fdct4_new / fwd_txfm.rs::fdct4).
#[rite]
pub(super) fn fdct4_x8(t: Desktop64, inp: &[__m256i; 4], out: &mut [__m256i; 4], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let rnd = splat(t, 1 << (cos_bit as u32 - 1));
    let sh = _mm_cvtsi32_si128(cos_bit as i32);
    // stage 1
    let bf0 = [
        add!(inp[0], inp[3]),
        add!(inp[1], inp[2]),
        sub!(inp[1], inp[2]),
        sub!(inp[0], inp[3]),
    ];
    // stage 2
    out[0] = hbtf(t, c!(t, cospi, 32), bf0[0], c!(t, cospi, 32), bf0[1], rnd, sh);
    out[1] = hbtf(t, c!(t, cospi, 48), bf0[2], c!(t, cospi, 16), bf0[3], rnd, sh);
    out[2] = hbtf(t, cn!(t, cospi, 32), bf0[1], c!(t, cospi, 32), bf0[0], rnd, sh);
    out[3] = hbtf(t, c!(t, cospi, 48), bf0[3], cn!(t, cospi, 16), bf0[2], rnd, sh);
}

/// 4-point inverse DCT (svt_av1_idct4_new / inv_txfm.rs::idct4). cos_bit is the
/// fixed inverse 12 (baked into `rnd`/`sh`); clamps to `[lo, hi]` per stage 3.
#[rite]
pub(super) fn idct4_x8(
    t: Desktop64,
    inp: &[__m256i; 4],
    out: &mut [__m256i; 4],
    rnd: __m256i,
    sh: __m128i,
    lo: __m256i,
    hi: __m256i,
) {
    let cospi = &COSPI;
    let cl = |v| clampv(t, v, lo, hi);
    // stage 1: permutation
    let bf0 = [inp[0], inp[2], inp[1], inp[3]];
    // stage 2
    let step = [
        hbtf(t, c!(t, cospi, 32), bf0[0], c!(t, cospi, 32), bf0[1], rnd, sh),
        hbtf(t, c!(t, cospi, 32), bf0[0], cn!(t, cospi, 32), bf0[1], rnd, sh),
        hbtf(t, c!(t, cospi, 48), bf0[2], cn!(t, cospi, 16), bf0[3], rnd, sh),
        hbtf(t, c!(t, cospi, 16), bf0[2], c!(t, cospi, 48), bf0[3], rnd, sh),
    ];
    // stage 3: combine
    out[0] = cl(add!(step[0], step[3]));
    out[1] = cl(add!(step[1], step[2]));
    out[2] = cl(sub!(step[1], step[2]));
    out[3] = cl(sub!(step[0], step[3]));
}

/// 4-point forward ADST (svt_av1_fadst4_new / fwd_txfm.rs::fadst4). The sinpi
/// butterfly, i32 (matches C; the scalar's i64 is identical in range). The
/// scalar's all-zero early-out is a pure optimization — the full arithmetic
/// yields 0 for 0 input — so it is omitted (byte-exact).
#[rite]
pub(super) fn fadst4_x8(t: Desktop64, inp: &[__m256i; 4], out: &mut [__m256i; 4], cos_bit: i8) {
    let sinpi = sinpi_arr(cos_bit);
    let x0 = inp[0];
    let x1 = inp[1];
    let x2 = inp[2];
    let x3 = inp[3];
    // stage 1
    let s0 = _mm256_mullo_epi32(splat(t, sinpi[1]), x0);
    let s1 = _mm256_mullo_epi32(splat(t, sinpi[4]), x0);
    let s2 = _mm256_mullo_epi32(splat(t, sinpi[2]), x1);
    let s3 = _mm256_mullo_epi32(splat(t, sinpi[1]), x1);
    let s4 = _mm256_mullo_epi32(splat(t, sinpi[3]), x2);
    let s5 = _mm256_mullo_epi32(splat(t, sinpi[4]), x3);
    let s6 = _mm256_mullo_epi32(splat(t, sinpi[2]), x3);
    // stage 2
    let s7 = sub!(add!(x0, x1), x3);
    // stage 3
    let a0 = add!(s0, s2);
    let a1 = _mm256_mullo_epi32(splat(t, sinpi[3]), s7);
    let a2 = sub!(s1, s3);
    let a3 = s4;
    // stage 4
    let a0 = add!(a0, s5);
    let a2 = add!(a2, s6);
    // stage 5
    let o0 = add!(a0, a3);
    let o1 = a1;
    let o2 = sub!(a2, a3);
    let o3 = sub!(a2, a0);
    // stage 6
    let o3 = add!(o3, a3);
    let b = cos_bit as i32;
    out[0] = round_shift_v(t, o0, b);
    out[1] = round_shift_v(t, o1, b);
    out[2] = round_shift_v(t, o2, b);
    out[3] = round_shift_v(t, o3, b);
}

/// 4-point inverse ADST (svt_av1_iadst4_new / inv_txfm.rs::iadst4). sinpi table
/// fixed at the inverse cos_bit (SINPI); round-shift by cos_bit via `rnd`/`sh`
/// (= (v + rnd) >> sh). Matches C's int32 arithmetic. lo/hi unused (no clamp).
#[rite]
pub(super) fn iadst4_x8(
    t: Desktop64,
    inp: &[__m256i; 4],
    out: &mut [__m256i; 4],
    rnd: __m256i,
    sh: __m128i,
    _lo: __m256i,
    _hi: __m256i,
) {
    let sinpi = &SINPI;
    let rs = |v| _mm256_sra_epi32(_mm256_add_epi32(v, rnd), sh);
    // stage 1
    let s0 = _mm256_mullo_epi32(splat(t, sinpi[1]), inp[0]);
    let s1 = _mm256_mullo_epi32(splat(t, sinpi[2]), inp[0]);
    let s2 = _mm256_mullo_epi32(splat(t, sinpi[3]), inp[1]);
    let s3 = _mm256_mullo_epi32(splat(t, sinpi[4]), inp[2]);
    let s4 = _mm256_mullo_epi32(splat(t, sinpi[1]), inp[2]);
    let s5 = _mm256_mullo_epi32(splat(t, sinpi[2]), inp[3]);
    let s6 = _mm256_mullo_epi32(splat(t, sinpi[4]), inp[3]);
    // stage 2
    let s7 = add!(sub!(inp[0], inp[2]), inp[3]);
    // stage 3
    let a0 = add!(s0, s3);
    let a1 = sub!(s1, s4);
    let a3 = s2;
    let a2 = _mm256_mullo_epi32(splat(t, sinpi[3]), s7);
    // stage 4
    let a0 = add!(a0, s5);
    let a1 = sub!(a1, s6);
    // stage 5
    let x0 = add!(a0, a3);
    let x1 = add!(a1, a3);
    let x2 = a2;
    let x3 = add!(a0, a1);
    // stage 6
    let x3 = sub!(x3, a3);
    out[0] = rs(x0);
    out[1] = rs(x1);
    out[2] = rs(x2);
    out[3] = rs(x3);
}

/// 4-point forward identity — round_shift(v * NewSqrt2, 12) (fidentity4).
#[rite]
pub(super) fn fidentity4_x8(t: Desktop64, inp: &[__m256i; 4], out: &mut [__m256i; 4], _cos_bit: i8) {
    for i in 0..4 {
        out[i] = rect_scale(t, inp[i], NEW_SQRT2);
    }
}

/// 4-point inverse identity — round_shift(v * NewSqrt2, 12) (iidentity4).
#[rite]
pub(super) fn iidentity4_x8(
    t: Desktop64,
    inp: &[__m256i; 4],
    out: &mut [__m256i; 4],
    _rnd: __m256i,
    _sh: __m128i,
    _lo: __m256i,
    _hi: __m256i,
) {
    for i in 0..4 {
        out[i] = rect_scale(t, inp[i], NEW_SQRT2);
    }
}

// ---------------------------------------------------------------------------
// 4-dim 2D drivers. `$c*` are the size-H column kernels, `$r*` the size-W row
// kernels; runtime col_1d/row_1d select DCT(0)/ADST(1|2)/IDENTITY(3); ud/lr
// carry the FLIPADST block edge flip.
// ---------------------------------------------------------------------------

macro_rules! fwd_4dim_driver {
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

            // COLUMN PASS — W columns (groups of <=8), size-H kernel. `ud`
            // reverses the input row read; `lr` mirrors the buf column write.
            let wg = W.div_ceil(8);
            for cg in 0..wg {
                let colbase = cg * 8;
                let cnt = (W - colbase).min(8);
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    let src_row = if ud { H - 1 - r } else { r };
                    let base = src_row * input_stride + colbase;
                    let v = if cnt == 8 {
                        load8(t, input, base)
                    } else {
                        let mut tmp = [0i32; 8];
                        for l in 0..cnt {
                            tmp[l] = input[base + l];
                        }
                        load8(t, &tmp, 0)
                    };
                    colin[r] = round_shift_v(t, v, pre_col);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                match col_1d {
                    0 => $cdct(t, &colin, &mut colout, cos_bit_col),
                    1 | 2 => $cadst(t, &colin, &mut colout, cos_bit_col),
                    _ => $cid(t, &colin, &mut colout, cos_bit_col),
                }
                for r in 0..H {
                    let v = round_shift_v(t, colout[r], post_col);
                    if cnt == 8 {
                        if lr {
                            store8(t, &mut buf, r * W + (W - colbase - 8), reverse8(t, v));
                        } else {
                            store8(t, &mut buf, r * W + colbase, v);
                        }
                    } else {
                        let mut tmp = [0i32; 8];
                        store8(t, &mut tmp, 0, v);
                        for l in 0..cnt {
                            let dst_c = if lr { W - 1 - (colbase + l) } else { colbase + l };
                            buf[r * W + dst_c] = tmp[l];
                        }
                    }
                }
            }

            // ROW PASS — H rows (groups of <=8), size-W kernel, no flip. The
            // buf gather + output scatter are column-strided → array-based.
            let hg = H.div_ceil(8);
            for rg in 0..hg {
                let rowbase = rg * 8;
                let cnt = (H - rowbase).min(8);
                let mut pos = [_mm256_setzero_si256(); W];
                for c in 0..W {
                    let mut tmp = [0i32; 8];
                    for l in 0..cnt {
                        tmp[l] = buf[(rowbase + l) * W + c];
                    }
                    pos[c] = load8(t, &tmp, 0);
                }
                let mut rowout = [_mm256_setzero_si256(); W];
                match row_1d {
                    0 => $rdct(t, &pos, &mut rowout, cos_bit_row),
                    1 | 2 => $radst(t, &pos, &mut rowout, cos_bit_row),
                    _ => $rid(t, &pos, &mut rowout, cos_bit_row),
                }
                for c in 0..W {
                    let mut v = round_shift_v(t, rowout[c], post_row);
                    if rect_ratio1 {
                        v = rect_scale(t, v, NEW_SQRT2);
                    }
                    let mut tmp = [0i32; 8];
                    store8(t, &mut tmp, 0, v);
                    for l in 0..cnt {
                        output[(rowbase + l) * W + c] = tmp[l];
                    }
                }
            }
        }
    };
}

macro_rules! inv_4dim_driver {
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
            let (row_range, col_range) = inv_txfm_ranges(bd);
            let sh_ls = inv_txfm_shift(W, H);
            let rsh0 = -(sh_ls[0] as i32);
            let rsh1 = -(sh_ls[1] as i32);
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

            // ROW PASS — H rows (groups of <=8), size-W kernel. Rect scale on
            // input, then row clamp. No flip. Column-strided → array-based.
            let hg = H.div_ceil(8);
            for rg in 0..hg {
                let rowbase = rg * 8;
                let cnt = (H - rowbase).min(8);
                let mut pos = [_mm256_setzero_si256(); W];
                for c in 0..W {
                    let mut tmp = [0i32; 8];
                    for l in 0..cnt {
                        tmp[l] = input[(rowbase + l) * input_stride + c];
                    }
                    let mut v = load8(t, &tmp, 0);
                    if rect_ratio1 {
                        v = rect_scale(t, v, NEW_INV_SQRT2);
                    }
                    pos[c] = clampv(t, v, row_lo, row_hi);
                }
                let mut rowout = [_mm256_setzero_si256(); W];
                match row_1d {
                    0 => $rdct(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                    1 | 2 => $radst(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                    _ => $rid(t, &pos, &mut rowout, rnd, shc, row_lo, row_hi),
                }
                for c in 0..W {
                    let v = round_shift_v(t, rowout[c], rsh0);
                    let mut tmp = [0i32; 8];
                    store8(t, &mut tmp, 0, v);
                    for l in 0..cnt {
                        buf[(rowbase + l) * W + c] = tmp[l];
                    }
                }
            }

            // COLUMN PASS — W columns (groups of <=8), size-H kernel. `lr`
            // mirrors the buf gather; `ud` reverses the output row store.
            let wg = W.div_ceil(8);
            for cg in 0..wg {
                let colbase = cg * 8;
                let cnt = (W - colbase).min(8);
                let mut colin = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    let v = if cnt == 8 && !lr {
                        load8(t, &buf, r * W + colbase)
                    } else if cnt == 8 && lr {
                        reverse8(t, load8(t, &buf, r * W + (W - colbase - 8)))
                    } else {
                        let mut tmp = [0i32; 8];
                        for l in 0..cnt {
                            let src_c = if lr { W - 1 - (colbase + l) } else { colbase + l };
                            tmp[l] = buf[r * W + src_c];
                        }
                        load8(t, &tmp, 0)
                    };
                    colin[r] = clampv(t, v, col_lo, col_hi);
                }
                let mut colout = [_mm256_setzero_si256(); H];
                match col_1d {
                    0 => $cdct(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                    1 | 2 => $cadst(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                    _ => $cid(t, &colin, &mut colout, rnd, shc, col_lo, col_hi),
                }
                let mut done = [_mm256_setzero_si256(); H];
                for r in 0..H {
                    let src = if ud { colout[H - 1 - r] } else { colout[r] };
                    let v = round_shift_v(t, src, rsh1);
                    done[r] = wraplow(t, v, wl_lo, wl_hi);
                }
                for r in 0..H {
                    if cnt == 8 {
                        store8(t, output, r * out_stride + colbase, done[r]);
                    } else {
                        let mut tmp = [0i32; 8];
                        store8(t, &mut tmp, 0, done[r]);
                        for l in 0..cnt {
                            output[r * out_stride + colbase + l] = tmp[l];
                        }
                    }
                }
            }
        }
    };
}

// col kernels = size-h; row kernels = size-w.
fwd_4dim_driver!(fwd_4dim_4x4, 4, 4,
    fdct4_x8, fadst4_x8, fidentity4_x8,
    fdct4_x8, fadst4_x8, fidentity4_x8);
fwd_4dim_driver!(fwd_4dim_4x8, 4, 8,
    fdct8_x8, fadst8_x8, fidentity8_x8,
    fdct4_x8, fadst4_x8, fidentity4_x8);
fwd_4dim_driver!(fwd_4dim_8x4, 8, 4,
    fdct4_x8, fadst4_x8, fidentity4_x8,
    fdct8_x8, fadst8_x8, fidentity8_x8);
fwd_4dim_driver!(fwd_4dim_4x16, 4, 16,
    fdct16_x8, fadst16_x8, fidentity16_x8,
    fdct4_x8, fadst4_x8, fidentity4_x8);
fwd_4dim_driver!(fwd_4dim_16x4, 16, 4,
    fdct4_x8, fadst4_x8, fidentity4_x8,
    fdct16_x8, fadst16_x8, fidentity16_x8);

// inverse: row kernels = size-w; col kernels = size-h.
inv_4dim_driver!(inv_4dim_4x4, 4, 4,
    idct4_x8, iadst4_x8, iidentity4_x8,
    idct4_x8, iadst4_x8, iidentity4_x8);
inv_4dim_driver!(inv_4dim_4x8, 4, 8,
    idct4_x8, iadst4_x8, iidentity4_x8,
    idct8_x8, iadst8_x8, iidentity8_x8);
inv_4dim_driver!(inv_4dim_8x4, 8, 4,
    idct8_x8, iadst8_x8, iidentity8_x8,
    idct4_x8, iadst4_x8, iidentity4_x8);
inv_4dim_driver!(inv_4dim_4x16, 4, 16,
    idct4_x8, iadst4_x8, iidentity4_x8,
    idct16_x8, iadst16_x8, iidentity16_x8);
inv_4dim_driver!(inv_4dim_16x4, 16, 4,
    idct16_x8, iadst16_x8, iidentity16_x8,
    idct4_x8, iadst4_x8, iidentity4_x8);

/// Forward 4-dim dispatcher. Any (col_1d, row_1d) ∈ {0..3}² (all 16 tx types
/// are legal at these sizes). Returns true if `(w, h)` is a 4-dim size.
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn fwd_4dim(
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
    match (w, h) {
        (4, 4) => fwd_4dim_4x4(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (4, 8) => fwd_4dim_4x8(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (8, 4) => fwd_4dim_8x4(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (4, 16) => fwd_4dim_4x16(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        (16, 4) => fwd_4dim_16x4(t, input, output, input_stride, col_1d, row_1d, ud, lr),
        _ => return false,
    }
    true
}

/// Inverse 4-dim dispatcher. Same contract as [`fwd_4dim`], `bd <= 10`.
#[rite]
#[allow(clippy::too_many_arguments)]
pub(super) fn inv_4dim(
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
    match (w, h) {
        (4, 4) => inv_4dim_4x4(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        (4, 8) => inv_4dim_4x8(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        (8, 4) => inv_4dim_8x4(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd),
        (4, 16) => {
            inv_4dim_4x16(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd)
        }
        (16, 4) => {
            inv_4dim_16x4(t, input, input_stride, output, out_stride, col_1d, row_1d, ud, lr, bd)
        }
        _ => return false,
    }
    true
}

// 2D ADST-containing transform drivers (ADST_DCT / DCT_ADST / ADST_ADST), for
// the square 8x8 / 16x16 and the non-square 8x16 / 16x8 sizes — the only sizes
// where AV1 allows an ADST 1D transform (both dims <= 16). FLIPADST variants
// (which flip the block edges) and the 4-dim ADST sizes stay scalar.
//
// These reuse the SAME `fwd_rect_driver!` / `inv_rect_driver!` macros as the
// rectangular DCT path (a square is just `w == h`, `rect_ratio1 == false`); only
// the per-pass 1D kernel changes: `col_1d` picks the size-`h` column kernel,
// `row_1d` picks the size-`w` row kernel (DCT: fdct/idct, ADST: fadst/iadst).
// Byte-exact with `fwd_txfm2d_core` / `inv_txfm2d_core` for the matching
// (col_1d, row_1d). Included into `mod v3`. Additive.

// -- forward: colk = size-h kernel, rowk = size-w kernel ---------------------
fwd_rect_driver!(fwd_adst_dct_8x8, 8, 8, fadst8_x8, fdct8_x8);
fwd_rect_driver!(fwd_dct_adst_8x8, 8, 8, fdct8_x8, fadst8_x8);
fwd_rect_driver!(fwd_adst_adst_8x8, 8, 8, fadst8_x8, fadst8_x8);
fwd_rect_driver!(fwd_adst_dct_16x16, 16, 16, fadst16_x8, fdct16_x8);
fwd_rect_driver!(fwd_dct_adst_16x16, 16, 16, fdct16_x8, fadst16_x8);
fwd_rect_driver!(fwd_adst_adst_16x16, 16, 16, fadst16_x8, fadst16_x8);
fwd_rect_driver!(fwd_adst_dct_8x16, 8, 16, fadst16_x8, fdct8_x8);
fwd_rect_driver!(fwd_dct_adst_8x16, 8, 16, fdct16_x8, fadst8_x8);
fwd_rect_driver!(fwd_adst_adst_8x16, 8, 16, fadst16_x8, fadst8_x8);
fwd_rect_driver!(fwd_adst_dct_16x8, 16, 8, fadst8_x8, fdct16_x8);
fwd_rect_driver!(fwd_dct_adst_16x8, 16, 8, fdct8_x8, fadst16_x8);
fwd_rect_driver!(fwd_adst_adst_16x8, 16, 8, fadst8_x8, fadst16_x8);

// -- inverse: rowk = size-w kernel, colk = size-h kernel ---------------------
inv_rect_driver!(inv_adst_dct_8x8, 8, 8, idct8_x8, iadst8_x8);
inv_rect_driver!(inv_dct_adst_8x8, 8, 8, iadst8_x8, idct8_x8);
inv_rect_driver!(inv_adst_adst_8x8, 8, 8, iadst8_x8, iadst8_x8);
inv_rect_driver!(inv_adst_dct_16x16, 16, 16, idct16_x8, iadst16_x8);
inv_rect_driver!(inv_dct_adst_16x16, 16, 16, iadst16_x8, idct16_x8);
inv_rect_driver!(inv_adst_adst_16x16, 16, 16, iadst16_x8, iadst16_x8);
inv_rect_driver!(inv_adst_dct_8x16, 8, 16, idct8_x8, iadst16_x8);
inv_rect_driver!(inv_dct_adst_8x16, 8, 16, iadst8_x8, idct16_x8);
inv_rect_driver!(inv_adst_adst_8x16, 8, 16, iadst8_x8, iadst16_x8);
inv_rect_driver!(inv_adst_dct_16x8, 16, 8, idct16_x8, iadst8_x8);
inv_rect_driver!(inv_dct_adst_16x8, 16, 8, iadst16_x8, idct8_x8);
inv_rect_driver!(inv_adst_adst_16x8, 16, 8, iadst16_x8, iadst8_x8);

/// Forward ADST-containing dispatcher. `col_1d`/`row_1d` ∈ {0=DCT, 1=ADST},
/// with at least one ADST; `(w,h)` both in {8,16}. Returns true if handled.
#[rite]
pub(super) fn fwd_adst(
    t: Desktop64,
    input: &[i32],
    output: &mut [i32],
    input_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
) -> bool {
    match (w, h, col_1d, row_1d) {
        (8, 8, 1, 0) => fwd_adst_dct_8x8(t, input, output, input_stride),
        (8, 8, 0, 1) => fwd_dct_adst_8x8(t, input, output, input_stride),
        (8, 8, 1, 1) => fwd_adst_adst_8x8(t, input, output, input_stride),
        (16, 16, 1, 0) => fwd_adst_dct_16x16(t, input, output, input_stride),
        (16, 16, 0, 1) => fwd_dct_adst_16x16(t, input, output, input_stride),
        (16, 16, 1, 1) => fwd_adst_adst_16x16(t, input, output, input_stride),
        (8, 16, 1, 0) => fwd_adst_dct_8x16(t, input, output, input_stride),
        (8, 16, 0, 1) => fwd_dct_adst_8x16(t, input, output, input_stride),
        (8, 16, 1, 1) => fwd_adst_adst_8x16(t, input, output, input_stride),
        (16, 8, 1, 0) => fwd_adst_dct_16x8(t, input, output, input_stride),
        (16, 8, 0, 1) => fwd_dct_adst_16x8(t, input, output, input_stride),
        (16, 8, 1, 1) => fwd_adst_adst_16x8(t, input, output, input_stride),
        _ => return false,
    }
    true
}

/// Inverse ADST-containing dispatcher. Same contract as [`fwd_adst`], `bd <= 10`.
#[rite]
pub(super) fn inv_adst(
    t: Desktop64,
    input: &[i32],
    input_stride: usize,
    output: &mut [i32],
    out_stride: usize,
    w: usize,
    h: usize,
    col_1d: u8,
    row_1d: u8,
    bd: u8,
) -> bool {
    match (w, h, col_1d, row_1d) {
        (8, 8, 1, 0) => inv_adst_dct_8x8(t, input, input_stride, output, out_stride, bd),
        (8, 8, 0, 1) => inv_dct_adst_8x8(t, input, input_stride, output, out_stride, bd),
        (8, 8, 1, 1) => inv_adst_adst_8x8(t, input, input_stride, output, out_stride, bd),
        (16, 16, 1, 0) => inv_adst_dct_16x16(t, input, input_stride, output, out_stride, bd),
        (16, 16, 0, 1) => inv_dct_adst_16x16(t, input, input_stride, output, out_stride, bd),
        (16, 16, 1, 1) => inv_adst_adst_16x16(t, input, input_stride, output, out_stride, bd),
        (8, 16, 1, 0) => inv_adst_dct_8x16(t, input, input_stride, output, out_stride, bd),
        (8, 16, 0, 1) => inv_dct_adst_8x16(t, input, input_stride, output, out_stride, bd),
        (8, 16, 1, 1) => inv_adst_adst_8x16(t, input, input_stride, output, out_stride, bd),
        (16, 8, 1, 0) => inv_adst_dct_16x8(t, input, input_stride, output, out_stride, bd),
        (16, 8, 0, 1) => inv_dct_adst_16x8(t, input, input_stride, output, out_stride, bd),
        (16, 8, 1, 1) => inv_adst_adst_16x8(t, input, input_stride, output, out_stride, bd),
        _ => return false,
    }
    true
}

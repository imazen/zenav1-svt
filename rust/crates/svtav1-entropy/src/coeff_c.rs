//! C-exact coefficient writer — port of `av1_write_coeffs_txb_1d`
//! (`entropy_coding.c:448`) and its context helpers.
//!
//! Every table, context derivation, and symbol write mirrors SVT-AV1 v4.1.0.
//! Helper-level parity is verified against the linked C library in
//! `tests/c_parity.rs`; whole-stream conformance via
//! `tools/decode_conformance.sh` (aomdec).
//!
//! Uses C `TxSize` indices (0..19) and C `FRAME_CONTEXT` CDF layout
//! throughout (values, structural 0, adaptation counter at `[nsymbs]`).

use crate::cdf::AomCdfProb;
use crate::default_cdfs as d;
use crate::scan_tables;
use crate::writer::AomWriter;

// ---- C TxSize enum values ----
pub const TX_4X4: usize = 0;
pub const TX_8X8: usize = 1;
pub const TX_16X16: usize = 2;
pub const TX_32X32: usize = 3;
pub const TX_64X64: usize = 4;
pub const TX_4X8: usize = 5;
pub const TX_8X4: usize = 6;
pub const TX_8X16: usize = 7;
pub const TX_16X8: usize = 8;
pub const TX_16X32: usize = 9;
pub const TX_32X16: usize = 10;
pub const TX_32X64: usize = 11;
pub const TX_64X32: usize = 12;
pub const TX_4X16: usize = 13;
pub const TX_16X4: usize = 14;
pub const TX_8X32: usize = 15;
pub const TX_32X8: usize = 16;
pub const TX_16X64: usize = 17;
pub const TX_64X16: usize = 18;
/// C `TX_SIZES_ALL`.
pub const TX_SIZES_ALL: usize = 19;

// ---- Transform dimension tables (common_utils.c) ----
pub const TX_SIZE_WIDE: [usize; TX_SIZES_ALL] = [
    4, 8, 16, 32, 64, 4, 8, 8, 16, 16, 32, 32, 64, 4, 16, 8, 32, 16, 64,
];
pub const TX_SIZE_HIGH: [usize; TX_SIZES_ALL] = [
    4, 8, 16, 32, 64, 8, 4, 16, 8, 32, 16, 64, 32, 16, 4, 32, 8, 64, 16,
];
pub const TX_SIZE_WIDE_LOG2: [usize; TX_SIZES_ALL] =
    [2, 3, 4, 5, 6, 2, 3, 3, 4, 4, 5, 5, 6, 2, 4, 3, 5, 4, 6];
pub const TX_SIZE_HIGH_LOG2: [usize; TX_SIZES_ALL] =
    [2, 3, 4, 5, 6, 3, 2, 4, 3, 5, 4, 6, 5, 4, 2, 5, 3, 6, 4];

/// C `txsize_sqr_map`.
pub const TXSIZE_SQR_MAP: [usize; TX_SIZES_ALL] = [
    TX_4X4, TX_8X8, TX_16X16, TX_32X32, TX_64X64, TX_4X4, TX_4X4, TX_8X8, TX_8X8, TX_16X16,
    TX_16X16, TX_32X32, TX_32X32, TX_4X4, TX_4X4, TX_8X8, TX_8X8, TX_16X16, TX_16X16,
];
/// C `txsize_sqr_up_map`.
pub const TXSIZE_SQR_UP_MAP: [usize; TX_SIZES_ALL] = [
    TX_4X4, TX_8X8, TX_16X16, TX_32X32, TX_64X64, TX_8X8, TX_8X8, TX_16X16, TX_16X16, TX_32X32,
    TX_32X32, TX_64X64, TX_64X64, TX_16X16, TX_16X16, TX_32X32, TX_32X32, TX_64X64, TX_64X64,
];
/// C `txsize_log2_minus4` (inv_transforms.h:341).
pub const TXSIZE_LOG2_MINUS4: [usize; TX_SIZES_ALL] =
    [0, 2, 4, 6, 6, 1, 1, 3, 3, 5, 5, 6, 6, 2, 2, 4, 4, 5, 5];

/// C `av1_get_adjusted_tx_size` — 64-dimension transforms cap to 32 for
/// coefficient coding.
#[inline]
pub const fn adjusted_tx_size(tx_size: usize) -> usize {
    match tx_size {
        TX_64X64 | TX_64X32 | TX_32X64 => TX_32X32,
        TX_64X16 => TX_32X16,
        TX_16X64 => TX_16X32,
        other => other,
    }
}

#[inline]
pub const fn txb_bwl(tx_size: usize) -> usize {
    TX_SIZE_WIDE_LOG2[adjusted_tx_size(tx_size)]
}
#[inline]
pub const fn txb_wide(tx_size: usize) -> usize {
    TX_SIZE_WIDE[adjusted_tx_size(tx_size)]
}
#[inline]
pub const fn txb_high(tx_size: usize) -> usize {
    TX_SIZE_HIGH[adjusted_tx_size(tx_size)]
}

/// Map transform dimensions in pixels to the C `TxSize` index.
pub fn tx_size_from_dims(w: usize, h: usize) -> usize {
    match (w, h) {
        (4, 4) => TX_4X4,
        (8, 8) => TX_8X8,
        (16, 16) => TX_16X16,
        (32, 32) => TX_32X32,
        (64, 64) => TX_64X64,
        (4, 8) => TX_4X8,
        (8, 4) => TX_8X4,
        (8, 16) => TX_8X16,
        (16, 8) => TX_16X8,
        (16, 32) => TX_16X32,
        (32, 16) => TX_32X16,
        (32, 64) => TX_32X64,
        (64, 32) => TX_64X32,
        (4, 16) => TX_4X16,
        (16, 4) => TX_16X4,
        (8, 32) => TX_8X32,
        (32, 8) => TX_32X8,
        (16, 64) => TX_16X64,
        (64, 16) => TX_64X16,
        _ => panic!("no TxSize for {w}x{h}"),
    }
}

/// C `get_txsize_entropy_ctx`.
#[inline]
pub const fn txsize_entropy_ctx(tx_size: usize) -> usize {
    (TXSIZE_SQR_MAP[tx_size] + TXSIZE_SQR_UP_MAP[tx_size] + 1) >> 1
}

// ---- TX types and classes ----
pub const TX_TYPES: usize = 16;
pub const DCT_DCT: usize = 0;

/// C `TxClass` values.
pub const TX_CLASS_2D: usize = 0;
pub const TX_CLASS_HORIZ: usize = 1;
pub const TX_CLASS_VERT: usize = 2;

/// C `tx_type_to_class` (cabac_context_model.c:15).
pub const TX_TYPE_TO_CLASS: [usize; TX_TYPES] = [
    TX_CLASS_2D,    // DCT_DCT
    TX_CLASS_2D,    // ADST_DCT
    TX_CLASS_2D,    // DCT_ADST
    TX_CLASS_2D,    // ADST_ADST
    TX_CLASS_2D,    // FLIPADST_DCT
    TX_CLASS_2D,    // DCT_FLIPADST
    TX_CLASS_2D,    // FLIPADST_FLIPADST
    TX_CLASS_2D,    // ADST_FLIPADST
    TX_CLASS_2D,    // FLIPADST_ADST
    TX_CLASS_2D,    // IDTX
    TX_CLASS_VERT,  // V_DCT
    TX_CLASS_HORIZ, // H_DCT
    TX_CLASS_VERT,  // V_ADST
    TX_CLASS_HORIZ, // H_ADST
    TX_CLASS_VERT,  // V_FLIPADST
    TX_CLASS_HORIZ, // H_FLIPADST
];

// ---- Extended TX set derivation (common_utils.h/.c) ----
pub const EXT_TX_SET_DCTONLY: usize = 0;
pub const EXT_TX_SET_DCT_IDTX: usize = 1;
pub const EXT_TX_SET_DTT4_IDTX: usize = 2;
pub const EXT_TX_SET_DTT4_IDTX_1DDCT: usize = 3;
pub const EXT_TX_SET_DTT9_IDTX_1DDCT: usize = 4;
pub const EXT_TX_SET_ALL16: usize = 5;
pub const EXT_TX_SET_TYPES: usize = 6;

/// C `av1_num_ext_tx_set`.
pub const AV1_NUM_EXT_TX_SET: [usize; EXT_TX_SET_TYPES] = [1, 2, 5, 7, 12, 16];

/// C `ext_tx_set_index[is_inter][set_type]`.
pub const EXT_TX_SET_INDEX: [[i32; EXT_TX_SET_TYPES]; 2] =
    [[0, -1, 2, 1, -1, -1], [0, 3, -1, -1, 2, 1]];

/// C `av1_ext_tx_ind[set_type][tx_type]` (cabac_context_model.c:34).
#[rustfmt::skip]
pub const AV1_EXT_TX_IND: [[usize; TX_TYPES]; EXT_TX_SET_TYPES] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 3, 4, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 5, 6, 4, 0, 0, 0, 0, 0, 0, 2, 3, 0, 0, 0, 0],
    [3, 4, 5, 8, 6, 7, 9, 10, 11, 0, 1, 2, 0, 0, 0, 0],
    [7, 8, 9, 12, 10, 11, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6],
];

/// C `get_ext_tx_set_type`.
#[inline]
pub fn ext_tx_set_type(tx_size: usize, is_inter: bool, use_reduced_set: bool) -> usize {
    let tx_size_sqr_up = TXSIZE_SQR_UP_MAP[tx_size];
    if tx_size_sqr_up > TX_32X32 {
        return EXT_TX_SET_DCTONLY;
    }
    if tx_size_sqr_up == TX_32X32 {
        return if is_inter {
            EXT_TX_SET_DCT_IDTX
        } else {
            EXT_TX_SET_DCTONLY
        };
    }
    if use_reduced_set {
        return if is_inter {
            EXT_TX_SET_DCT_IDTX
        } else {
            EXT_TX_SET_DTT4_IDTX
        };
    }
    let tx_size_sqr = TXSIZE_SQR_MAP[tx_size];
    if is_inter {
        if tx_size_sqr == TX_16X16 {
            EXT_TX_SET_DTT9_IDTX_1DDCT
        } else {
            EXT_TX_SET_ALL16
        }
    } else if tx_size_sqr == TX_16X16 {
        EXT_TX_SET_DTT4_IDTX
    } else {
        EXT_TX_SET_DTT4_IDTX_1DDCT
    }
}

/// C `get_ext_tx_types`.
#[inline]
pub fn ext_tx_types(tx_size: usize, is_inter: bool, use_reduced_set: bool) -> usize {
    AV1_NUM_EXT_TX_SET[ext_tx_set_type(tx_size, is_inter, use_reduced_set)]
}

/// C `get_ext_tx_set`.
#[inline]
pub fn ext_tx_set(tx_size: usize, is_inter: bool, use_reduced_set: bool) -> i32 {
    EXT_TX_SET_INDEX[usize::from(is_inter)][ext_tx_set_type(tx_size, is_inter, use_reduced_set)]
}

// ---- Level map (definitions.h TX_PAD_*) ----
pub const TX_PAD_HOR_LOG2: usize = 2;
pub const TX_PAD_HOR: usize = 4;
pub const TX_PAD_TOP: usize = 2;
pub const TX_PAD_BOTTOM: usize = 2;
pub const TX_PAD_END: usize = 16;
pub const MAX_TX_SIZE: usize = 64;
/// C `TX_PAD_2D` — full padded level buffer size.
pub const TX_PAD_2D: usize =
    (MAX_TX_SIZE + TX_PAD_HOR) * (MAX_TX_SIZE + TX_PAD_TOP + TX_PAD_BOTTOM) + TX_PAD_END;

/// Per-call level-map scratch length, sized to the **coeff-coding txb cap of
/// 32x32** rather than the MAX_TX_SIZE(64)-shaped [`TX_PAD_2D`]. Coefficient
/// coding always operates on the adjusted (≤32) txb dims (`adjusted_tx_size`
/// folds every 64-dim transform to its 32-dim map), so no reader of the level
/// map ever indexes past what a 32x32 txb reaches: the deepest access is the
/// `TX_CLASS_VERT` branch of `get_nz_mag`, `base + 4*stride`, i.e. up to
/// `TX_PAD_TOP + 32 + 4` padded rows of a `32 + TX_PAD_HOR`-wide stride plus
/// `TX_PAD_END`. This equals the [`txb_init_levels`] `used` bound at
/// width=height=32, so a scratch of this length (~1456 bytes vs 4640) holds
/// every txb the encoder can code with a ~3x smaller one-time zero, and
/// `used.min(len)` never truncates below a real read. Callers that previously
/// stack-allocated (or heap-allocated) a full `TX_PAD_2D` per txb use this.
pub const LEVELS_SCRATCH_LEN: usize =
    (TX_PAD_TOP + 32 + TX_PAD_BOTTOM + 4) * (32 + TX_PAD_HOR) + TX_PAD_END;

/// Offset of the (0,0) level inside the padded buffer (C `set_levels`).
#[inline]
pub const fn levels_origin(width: usize) -> usize {
    TX_PAD_TOP * (width + TX_PAD_HOR)
}

/// C `svt_av1_txb_init_levels_c`: zero the padded map and fill
/// `levels[row * (width+4) + col] = min(|coeff|, 127)` at the origin offset.
///
/// The value fill is SIMD-dispatched (see [`crate::coeff_simd::fill_levels`]) —
/// byte-identical to the scalar map, proven against the exported real-C kernel
/// under every dispatch tier in `tests/c_parity.rs`.
pub fn txb_init_levels(coeff: &[i32], width: usize, height: usize, levels_buf: &mut [u8]) {
    // Zero only the padded extent this (width, height) txb actually uses, not the
    // whole MAX_TX_SIZE-shaped buffer (TX_PAD_2D = 4640 bytes). C keeps a
    // persistent `md_levels_buf` whose pad is zeroed once (md_process.c:235) and
    // re-fills only the body per txb; the port re-zeros per call, so at least
    // bound the re-zero to the used prefix. The context derivation
    // (`get_nz_map_contexts` -> `nz_map_ctx`/`get_nz_mag`/`br_ctx`) reads the map
    // at each coefficient's padded position plus neighbour offsets reaching at
    // most 4 rows below the bottom-right coefficient (the TX_CLASS_VERT branch of
    // `nz_mag` reads `base + 4*stride`); with the top-aligned origin (TX_PAD_TOP
    // rows) the furthest byte any reader touches is strictly below
    // `(TX_PAD_TOP + height + 3) * stride + width`, and `fill_levels` writes only
    // the body columns inside that. `used` clears that bound with >= 2*width rows
    // of margin plus TX_PAD_END, capped at the buffer length. Byte-identical:
    // every byte read and every byte written lies in `[0, used)`; bytes in
    // `[used, len)` are never accessed for a txb of this size (for a 4x4 that is
    // ~112 bytes zeroed instead of 4640).
    let stride = width + TX_PAD_HOR;
    let used =
        ((TX_PAD_TOP + height + TX_PAD_BOTTOM + 4) * stride + TX_PAD_END).min(levels_buf.len());
    for b in levels_buf[..used].iter_mut() {
        *b = 0;
    }
    crate::coeff_simd::fill_levels(coeff, width, height, levels_buf);
}

/// C `get_padded_idx`.
#[inline]
pub const fn padded_idx(idx: usize, bwl: usize) -> usize {
    idx + ((idx >> bwl) << TX_PAD_HOR_LOG2)
}

#[inline]
fn clip_max3(v: u8) -> u32 {
    (v as u32).min(3)
}

/// C `get_nz_mag` — `levels` points at the padded position of the coefficient.
#[inline]
fn nz_mag(levels: &[u8], base: usize, bwl: usize, tx_class: usize) -> u32 {
    let mut mag = clip_max3(levels[base + 1]);
    mag += clip_max3(levels[base + (1 << bwl) + TX_PAD_HOR]);
    match tx_class {
        TX_CLASS_2D => {
            mag += clip_max3(levels[base + (1 << bwl) + TX_PAD_HOR + 1]);
            mag += clip_max3(levels[base + 2]);
            mag += clip_max3(levels[base + (2 << bwl) + (2 << TX_PAD_HOR_LOG2)]);
        }
        TX_CLASS_VERT => {
            mag += clip_max3(levels[base + (2 << bwl) + (2 << TX_PAD_HOR_LOG2)]);
            mag += clip_max3(levels[base + (3 << bwl) + (3 << TX_PAD_HOR_LOG2)]);
            mag += clip_max3(levels[base + (4 << bwl) + (4 << TX_PAD_HOR_LOG2)]);
        }
        _ => {
            mag += clip_max3(levels[base + 2]);
            mag += clip_max3(levels[base + 3]);
            mag += clip_max3(levels[base + 4]);
        }
    }
    mag
}

/// C `nz_map_ctx_offset_1d`: 1D-class contexts live after the 26 2D
/// contexts (`SIG_COEF_CONTEXTS_2D`).
#[inline]
pub(crate) const fn nz_map_ctx_offset_1d(pos: usize) -> usize {
    match pos {
        0 => 26,
        1 => 31,
        _ => 36,
    }
}

/// The generating algorithm for C `eb_av1_nz_map_ctx_offset[tx_size][idx]`
/// (documented in coefficients.h:176-186); verified against the exported C
/// data in tests/c_parity.rs.
#[inline]
pub const fn nz_map_ctx_offset_2d(tx_size: usize, coeff_idx: usize) -> usize {
    if coeff_idx == 0 {
        // The (0,0) entry is 0 in the C tables (the from-stats path
        // special-cases (tx_class | coeff_idx) == 0 before the lookup).
        return 0;
    }
    // Shape branches use the ORIGINAL transform dimensions; only the
    // row/col extraction uses the adjusted (32-capped) layout.
    let adj = adjusted_tx_size(tx_size);
    let bwl = TX_SIZE_WIDE_LOG2[adj];
    let width = TX_SIZE_WIDE[tx_size];
    let height = TX_SIZE_HIGH[tx_size];
    let row = coeff_idx >> bwl;
    let col = coeff_idx - (row << bwl);
    if width < height {
        if row < 2 {
            return 11;
        }
    } else if width > height && col < 2 {
        return 16;
    }
    if row + col < 2 {
        return 1;
    }
    if row + col < 4 {
        return 6;
    }
    21
}

/// C `get_nz_map_ctx_from_stats`.
#[inline]
fn nz_map_ctx_from_stats(
    stats: u32,
    coeff_idx: usize,
    bwl: usize,
    tx_size: usize,
    tx_class: usize,
) -> usize {
    if (tx_class | coeff_idx) == 0 {
        return 0;
    }
    let ctx = (((stats + 1) >> 1) as usize).min(4);
    match tx_class {
        TX_CLASS_2D => ctx + nz_map_ctx_offset_2d(tx_size, coeff_idx),
        TX_CLASS_HORIZ => {
            let row = coeff_idx >> bwl;
            let col = coeff_idx - (row << bwl);
            ctx + nz_map_ctx_offset_1d(col)
        }
        TX_CLASS_VERT => {
            let row = coeff_idx >> bwl;
            ctx + nz_map_ctx_offset_1d(row)
        }
        _ => 0,
    }
}

/// C `get_nz_map_ctx` (encode_txb_ref_c.c:17).
#[inline]
pub(crate) fn nz_map_ctx(
    levels: &[u8],
    origin: usize,
    coeff_idx: usize,
    bwl: usize,
    height: usize,
    scan_idx: usize,
    is_eob: bool,
    tx_size: usize,
    tx_class: usize,
) -> usize {
    if is_eob {
        if scan_idx == 0 {
            return 0;
        }
        if scan_idx <= (height << bwl) / 8 {
            return 1;
        }
        if scan_idx <= (height << bwl) / 4 {
            return 2;
        }
        return 3;
    }
    let stats = nz_mag(levels, origin + padded_idx(coeff_idx, bwl), bwl, tx_class);
    nz_map_ctx_from_stats(stats, coeff_idx, bwl, tx_size, tx_class)
}

/// C `get_lower_levels_ctx_general` (coefficients.h:195 + the
/// `get_lower_levels_ctx_eob` is_last branch, coefficients.h:55): the
/// per-coefficient base-level context the RDOQ trellis
/// (`svt_av1_optimize_b`) prices with. `levels_buf` is the full padded
/// buffer from [`txb_init_levels`]; `ci` is the packed raster position.
#[inline]
pub fn lower_levels_ctx_general(
    levels_buf: &[u8],
    ci: usize,
    bwl: usize,
    height: usize,
    scan_idx: usize,
    is_last: bool,
    tx_size: usize,
    tx_class: usize,
) -> usize {
    nz_map_ctx(
        levels_buf,
        levels_origin(1 << bwl),
        ci,
        bwl,
        height,
        scan_idx,
        is_last,
        tx_size,
        tx_class,
    )
}

/// C `get_br_ctx_eob` (coefficients.h:68) — the coeff_br context for the
/// last (eob) coefficient, which never reads neighbor levels.
#[inline]
pub fn br_ctx_eob(c: usize, bwl: usize, tx_class: usize) -> usize {
    let row = c >> bwl;
    let col = c - (row << bwl);
    if c == 0 {
        return 0;
    }
    if (tx_class == TX_CLASS_2D && row < 2 && col < 2)
        || (tx_class == TX_CLASS_HORIZ && col == 0)
        || (tx_class == TX_CLASS_VERT && row == 0)
    {
        return 7;
    }
    14
}

/// C `svt_av1_get_nz_map_contexts` — the coefficient nz-map / base-level
/// context for every scanned position, mirroring the production RTCD dispatch:
/// on x86 the AVX2 arm reproduces `svt_av1_get_nz_map_contexts_sse2` (the RTCD
/// default — a **raster** fill of the whole padded block with contiguous
/// 16-byte neighbour loads, then the scan-last stamp); elsewhere the scan-order
/// scalar `_c` loop runs, exactly as C's `SET_ONLY_C`/`SET_NEON` fallbacks.
///
/// Both arms are byte-identical at every `scan[0..eob]` position — the only
/// positions any caller reads (pd0 `loop_cost_eob_pd0` and the leaf-funnel
/// coeff cost both index `coeff_contexts` exclusively at `0 == scan[0]` and
/// `scan[c], c < eob`), the same invariant production C relies on: its SIMD
/// kernels leave raster values at non-scan positions. Proven bit-identical to
/// BOTH exported real-C kernels (`_c` and `_sse2`) under every archmage
/// dispatch tier in `tests/c_parity.rs::nz_map_contexts_simd_matches_c`.
///
/// `eob == 1` short-circuits to the DC write both C kernels agree on
/// (`_sse2`'s `coeff_contexts[0] = 0` early-out; `_c`'s `scan_idx == 0`
/// is_eob arm — `scan[0]` is always the DC position 0).
pub fn get_nz_map_contexts(
    levels_buf: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    if eob == 0 {
        // `_c`'s loop body never runs; write nothing.
        return;
    }
    if eob == 1 {
        coeff_contexts[scan[0] as usize] = 0;
        return;
    }
    crate::coeff_simd::nz_map_contexts(levels_buf, scan, eob, tx_size, tx_class, coeff_contexts);
}

/// C `svt_av1_get_nz_map_contexts_c` (encode_txb_ref_c.c:35) — the scan-order
/// scalar loop, verbatim. The non-x86 dispatch arm of [`get_nz_map_contexts`]
/// and the tier-forced reference in `tests/c_parity.rs`.
pub(crate) fn nz_map_contexts_scan_order(
    levels_buf: &[u8],
    scan: &[u16],
    eob: usize,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    let bwl = txb_bwl(tx_size);
    let height = txb_high(tx_size);
    let origin = levels_origin(txb_wide(tx_size));
    for i in 0..eob {
        let pos = scan[i] as usize;
        coeff_contexts[pos] = nz_map_ctx(
            levels_buf,
            origin,
            pos,
            bwl,
            height,
            i,
            i == eob - 1,
            tx_size,
            tx_class,
        ) as i8;
    }
}

/// C `get_br_ctx` (coefficients.h:82) — `c` is the raster position.
#[inline]
pub fn br_ctx(levels_buf: &[u8], c: usize, bwl: usize, tx_class: usize) -> usize {
    let row = c >> bwl;
    let col = c - (row << bwl);
    let stride = (1 << bwl) + TX_PAD_HOR;
    // C indexes `levels` from the set_levels origin.
    let pos = levels_origin(1 << bwl) + row * stride + col;
    let mut mag = levels_buf[pos + 1] as u32;
    mag += levels_buf[pos + stride] as u32;
    match tx_class {
        TX_CLASS_2D => {
            mag += levels_buf[pos + stride + 1] as u32;
            mag = ((mag + 1) >> 1).min(6);
            if c == 0 {
                return mag as usize;
            }
            if row < 2 && col < 2 {
                return mag as usize + 7;
            }
        }
        TX_CLASS_HORIZ => {
            mag += levels_buf[pos + 2] as u32;
            mag = ((mag + 1) >> 1).min(6);
            if c == 0 {
                return mag as usize;
            }
            if col == 0 {
                return mag as usize + 7;
            }
        }
        TX_CLASS_VERT => {
            mag += levels_buf[pos + (stride << 1)] as u32;
            mag = ((mag + 1) >> 1).min(6);
            if c == 0 {
                return mag as usize;
            }
            if row == 0 {
                return mag as usize + 7;
            }
        }
        _ => {}
    }
    mag as usize + 14
}

// ---- EOB position coding (entropy_coding.c:32-44, entropy_coding.h:99) ----
#[rustfmt::skip]
pub const EOB_TO_POS_SMALL: [u8; 33] = [
    0, 1, 2,
    3, 3,
    4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
];
#[rustfmt::skip]
pub const EOB_TO_POS_LARGE: [u8; 17] = [
    6, 7, 8, 8, 9, 9, 9, 9, 10, 10, 10, 10, 10, 10, 10, 10, 11,
];
pub const EOB_GROUP_START: [i16; 12] = [0, 1, 2, 3, 5, 9, 17, 33, 65, 129, 257, 513];
pub const EOB_OFFSET_BITS: [i16; 12] = [0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

/// C `get_eob_pos_token`: returns (token, extra).
#[inline]
pub fn eob_pos_token(eob: i32) -> (usize, i32) {
    let t = if eob < 33 {
        EOB_TO_POS_SMALL[eob as usize] as usize
    } else {
        let e = (((eob - 1) >> 5) as usize).min(16);
        EOB_TO_POS_LARGE[e] as usize
    };
    (t, eob - i32::from(EOB_GROUP_START[t]))
}

// ---- Coefficient-context constants ----
pub const NUM_BASE_LEVELS: i32 = 2;
pub const COEFF_BASE_RANGE: i32 = 12;
pub const BR_CDF_SIZE: usize = 4;
pub const COEFF_CONTEXT_BITS: u32 = 6;
pub const COEFF_CONTEXT_MASK: i32 = (1 << COEFF_CONTEXT_BITS) - 1;
pub const PLANE_TYPES: usize = 2;

/// The coefficient-coding slice of the C `FRAME_CONTEXT`, in C layout.
#[derive(Clone)]
pub struct CoeffFc {
    pub txb_skip_cdf: [[AomCdfProb; 3]; 13 * 5],
    pub eob_extra_cdf: [[AomCdfProb; 3]; 9 * 2 * 5],
    pub dc_sign_cdf: [[AomCdfProb; 3]; 3 * 2],
    pub eob_flag_cdf16: [[AomCdfProb; 6]; 2 * 2],
    pub eob_flag_cdf32: [[AomCdfProb; 7]; 2 * 2],
    pub eob_flag_cdf64: [[AomCdfProb; 8]; 2 * 2],
    pub eob_flag_cdf128: [[AomCdfProb; 9]; 2 * 2],
    pub eob_flag_cdf256: [[AomCdfProb; 10]; 2 * 2],
    pub eob_flag_cdf512: [[AomCdfProb; 11]; 2 * 2],
    pub eob_flag_cdf1024: [[AomCdfProb; 12]; 2 * 2],
    pub coeff_base_eob_cdf: [[AomCdfProb; 4]; 4 * 2 * 5],
    pub coeff_base_cdf: [[AomCdfProb; 5]; 42 * 2 * 5],
    pub coeff_br_cdf: [[AomCdfProb; 5]; 21 * 2 * 4],
    pub intra_ext_tx_cdf: [[AomCdfProb; 17]; 13 * 4 * 3],
    /// C `FRAME_CONTEXT.inter_ext_tx_cdf[EXT_TX_SETS_INTER(4)]`
    /// `[EXT_TX_SIZES(4)][CDF_SIZE(TX_TYPES)]` — the tx-type rows for
    /// INTER-classified blocks (on this allintra port: IntraBC only,
    /// `av1_write_tx_type`'s `is_inter` arm, entropy_coding.c:333-337).
    pub inter_ext_tx_cdf: [[AomCdfProb; 17]; 4 * 4],
}

impl CoeffFc {
    /// C `svt_av1_default_coef_probs` + the intra_ext_tx slice of
    /// `svt_aom_init_mode_probs`, for the bucket of `base_qindex`.
    pub fn default_for_qindex(base_qindex: u8) -> alloc::boxed::Box<Self> {
        let q = d::coef_q_ctx(base_qindex);
        let mut fc = alloc::boxed::Box::new(Self {
            txb_skip_cdf: [[0; 3]; 65],
            eob_extra_cdf: [[0; 3]; 90],
            dc_sign_cdf: [[0; 3]; 6],
            eob_flag_cdf16: [[0; 6]; 4],
            eob_flag_cdf32: [[0; 7]; 4],
            eob_flag_cdf64: [[0; 8]; 4],
            eob_flag_cdf128: [[0; 9]; 4],
            eob_flag_cdf256: [[0; 10]; 4],
            eob_flag_cdf512: [[0; 11]; 4],
            eob_flag_cdf1024: [[0; 12]; 4],
            coeff_base_eob_cdf: [[0; 4]; 40],
            coeff_base_cdf: [[0; 5]; 420],
            coeff_br_cdf: [[0; 5]; 168],
            intra_ext_tx_cdf: [[0; 17]; 156],
            inter_ext_tx_cdf: [[0; 17]; 16],
        });
        fc.txb_skip_cdf
            .copy_from_slice(d::TXB_SKIP_CDF[q].as_flattened());
        fc.eob_extra_cdf
            .copy_from_slice(d::EOB_EXTRA_CDF[q].as_flattened().as_flattened());
        fc.dc_sign_cdf
            .copy_from_slice(d::DC_SIGN_CDF[q].as_flattened());
        fc.eob_flag_cdf16
            .copy_from_slice(d::EOB_FLAG_CDF16[q].as_flattened());
        fc.eob_flag_cdf32
            .copy_from_slice(d::EOB_FLAG_CDF32[q].as_flattened());
        fc.eob_flag_cdf64
            .copy_from_slice(d::EOB_FLAG_CDF64[q].as_flattened());
        fc.eob_flag_cdf128
            .copy_from_slice(d::EOB_FLAG_CDF128[q].as_flattened());
        fc.eob_flag_cdf256
            .copy_from_slice(d::EOB_FLAG_CDF256[q].as_flattened());
        fc.eob_flag_cdf512
            .copy_from_slice(d::EOB_FLAG_CDF512[q].as_flattened());
        fc.eob_flag_cdf1024
            .copy_from_slice(d::EOB_FLAG_CDF1024[q].as_flattened());
        fc.coeff_base_eob_cdf
            .copy_from_slice(d::COEFF_BASE_EOB_CDF[q].as_flattened().as_flattened());
        fc.coeff_base_cdf
            .copy_from_slice(d::COEFF_BASE_CDF[q].as_flattened().as_flattened());
        fc.coeff_br_cdf
            .copy_from_slice(d::COEFF_BR_CDF[q].as_flattened().as_flattened());
        fc.intra_ext_tx_cdf
            .copy_from_slice(d::INTRA_EXT_TX_CDF.as_flattened().as_flattened());
        fc.inter_ext_tx_cdf
            .copy_from_slice(d::INTER_EXT_TX_CDF.as_flattened());
        fc
    }

    // C-layout accessors: [tx_size-entropy-ctx][plane][ctx] flattened row-major.
    #[inline]
    fn txb_skip(&mut self, txs_ctx: usize, ctx: usize) -> &mut [AomCdfProb; 3] {
        &mut self.txb_skip_cdf[txs_ctx * 13 + ctx]
    }
    #[inline]
    fn eob_extra(&mut self, txs_ctx: usize, plane: usize, ctx: usize) -> &mut [AomCdfProb; 3] {
        &mut self.eob_extra_cdf[(txs_ctx * 2 + plane) * 9 + ctx]
    }
    #[inline]
    fn dc_sign(&mut self, plane: usize, ctx: usize) -> &mut [AomCdfProb; 3] {
        &mut self.dc_sign_cdf[plane * 3 + ctx]
    }
    #[inline]
    fn coeff_base_eob(&mut self, txs_ctx: usize, plane: usize, ctx: usize) -> &mut [AomCdfProb; 4] {
        &mut self.coeff_base_eob_cdf[(txs_ctx * 2 + plane) * 4 + ctx]
    }
    #[inline]
    fn coeff_base(&mut self, txs_ctx: usize, plane: usize, ctx: usize) -> &mut [AomCdfProb; 5] {
        &mut self.coeff_base_cdf[(txs_ctx * 2 + plane) * 42 + ctx]
    }
    #[inline]
    fn coeff_br(&mut self, txs_ctx: usize, plane: usize, ctx: usize) -> &mut [AomCdfProb; 5] {
        &mut self.coeff_br_cdf[(txs_ctx * 2 + plane) * 21 + ctx]
    }
    #[inline]
    fn intra_ext_tx(
        &mut self,
        eset: usize,
        sq_tx: usize,
        intra_dir: usize,
    ) -> &mut [AomCdfProb; 17] {
        &mut self.intra_ext_tx_cdf[(eset * 4 + sq_tx) * 13 + intra_dir]
    }
    #[inline]
    fn inter_ext_tx(&mut self, eset: usize, sq_tx: usize) -> &mut [AomCdfProb; 17] {
        &mut self.inter_ext_tx_cdf[eset * 4 + sq_tx]
    }

    /// In-place weighted per-entry average of `self` (left, ×`wt_left`) with a
    /// top-right neighbor's coeff context (×`wt_tr`) — the coeff-CDF half of
    /// `avg_cdf_symbols` (`enc_dec_process.c:2711-2723`, plus the
    /// `intra_ext_tx_cdf` slices `:2798-2799`). Every entry becomes
    /// `(left*wt_left + tr*wt_tr + (wt_left+wt_tr)/2) / (wt_left+wt_tr)`. This is
    /// the table the per-SB RDOQ rate estimation reads
    /// (`coeff_base_eob_cdf` → `base_eob_cost`), so it must track C's neighbor
    /// averaging on frames wider than two super-blocks.
    pub fn avg_cdf_with(&mut self, tr: &CoeffFc, wt_left: i32, wt_tr: i32) {
        use crate::cdf::avg_cdf_entries as avg;
        avg(self.txb_skip_cdf.as_flattened_mut(), tr.txb_skip_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.eob_extra_cdf.as_flattened_mut(), tr.eob_extra_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.dc_sign_cdf.as_flattened_mut(), tr.dc_sign_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf16.as_flattened_mut(), tr.eob_flag_cdf16.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf32.as_flattened_mut(), tr.eob_flag_cdf32.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf64.as_flattened_mut(), tr.eob_flag_cdf64.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf128.as_flattened_mut(), tr.eob_flag_cdf128.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf256.as_flattened_mut(), tr.eob_flag_cdf256.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf512.as_flattened_mut(), tr.eob_flag_cdf512.as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf1024.as_flattened_mut(), tr.eob_flag_cdf1024.as_flattened(), wt_left, wt_tr);
        avg(self.coeff_base_eob_cdf.as_flattened_mut(), tr.coeff_base_eob_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.coeff_base_cdf.as_flattened_mut(), tr.coeff_base_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.coeff_br_cdf.as_flattened_mut(), tr.coeff_br_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.intra_ext_tx_cdf.as_flattened_mut(), tr.intra_ext_tx_cdf.as_flattened(), wt_left, wt_tr);
        // C AVG_CDF_STRIDE over inter_ext_tx_cdf sets 1..3 (enc_dec_process.c
        // :2675-2677); set 0 + the beyond-nsymbs tails are identical zeros on
        // both sides, so the full-slice average is value-equivalent (the same
        // established convention as intra_ext_tx_cdf above).
        avg(self.inter_ext_tx_cdf.as_flattened_mut(), tr.inter_ext_tx_cdf.as_flattened(), wt_left, wt_tr);
    }
}

/// C `write_golomb` (entropy_coding.c:267).
fn write_golomb(w: &mut AomWriter, level: i32) {
    let x = level + 1;
    let length = (32 - (x as u32).leading_zeros()) as i32; // ilog2(x) + 1
    debug_assert!(length > 0);
    for _ in 0..length - 1 {
        w.write_bit(false);
    }
    for i in (0..length).rev() {
        w.write_bit((x >> i) & 1 != 0);
    }
}

/// C `av1_write_tx_type` (entropy_coding.c:410) — intra path.
#[allow(clippy::too_many_arguments)]
pub fn write_tx_type_intra(
    fc: &mut CoeffFc,
    w: &mut AomWriter,
    intra_dir: usize,
    tx_type: usize,
    tx_size: usize,
    base_q_idx: u8,
    reduced_tx_set: bool,
) {
    if ext_tx_types(tx_size, false, reduced_tx_set) > 1 && base_q_idx > 0 {
        let square_tx_size = TXSIZE_SQR_MAP[tx_size];
        let set_type = ext_tx_set_type(tx_size, false, reduced_tx_set);
        let eset = ext_tx_set(tx_size, false, reduced_tx_set);
        debug_assert!(eset > 0);
        let cdf = fc.intra_ext_tx(eset as usize, square_tx_size, intra_dir);
        w.write_symbol(
            AV1_EXT_TX_IND[set_type][tx_type],
            cdf,
            AV1_NUM_EXT_TX_SET[set_type],
        );
    }
}

/// C `av1_write_tx_type` (entropy_coding.c:333-337) — the `is_inter` arm.
/// On this allintra port only IntraBC blocks are inter-classified
/// (`is_inter_block` = `use_intrabc || ref_frame[0] > INTRA_FRAME`), so
/// this codes the tx type of an IntraBC block's luma txbs over
/// `inter_ext_tx_cdf[eset][square_tx_size]` (no intra-dir dimension).
pub fn write_tx_type_inter(
    fc: &mut CoeffFc,
    w: &mut AomWriter,
    tx_type: usize,
    tx_size: usize,
    base_q_idx: u8,
    reduced_tx_set: bool,
) {
    if ext_tx_types(tx_size, true, reduced_tx_set) > 1 && base_q_idx > 0 {
        let square_tx_size = TXSIZE_SQR_MAP[tx_size];
        let set_type = ext_tx_set_type(tx_size, true, reduced_tx_set);
        let eset = ext_tx_set(tx_size, true, reduced_tx_set);
        debug_assert!(eset > 0);
        let cdf = fc.inter_ext_tx(eset as usize, square_tx_size);
        w.write_symbol(
            AV1_EXT_TX_IND[set_type][tx_type],
            cdf,
            AV1_NUM_EXT_TX_SET[set_type],
        );
    }
}

/// Exact port of `av1_write_coeffs_txb_1d` (entropy_coding.c:448).
///
/// `coeffs` is the raster-order coefficient block, `width x height` of the
/// adjusted transform size, tightly packed with stride == width (matching
/// how the C caller lays out `coeff_buffer_ptr` reads for this path).
/// Returns `cul_level` (with the DC sign folded in) for neighbor updates.
/// `is_inter` routes the luma tx-type symbol through the inter CDF rows
/// (`av1_write_tx_type`'s `is_inter_block` split) — true only for IntraBC
/// blocks on this port.
#[allow(clippy::too_many_arguments)]
pub fn write_coeffs_txb_1d(
    fc: &mut CoeffFc,
    w: &mut AomWriter,
    tx_size: usize,
    tx_type: usize,
    plane_type: usize,
    txb_skip_ctx: usize,
    dc_sign_ctx: usize,
    coeffs: &[i32],
    eob: i32,
    intra_dir: usize,
    base_q_idx: u8,
    reduced_tx_set: bool,
    is_inter: bool,
) -> i32 {
    let txs_ctx = txsize_entropy_ctx(tx_size);
    let scan = scan_tables::scan(
        tx_size,
        scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
    );
    let bwl = txb_bwl(tx_size);
    let width = txb_wide(tx_size);
    let height = txb_high(tx_size);
    let tx_class = TX_TYPE_TO_CLASS[tx_type];

    debug_assert!(txs_ctx < 5);
    let cdf = fc.txb_skip(txs_ctx, txb_skip_ctx);
    w.write_symbol(usize::from(eob == 0), cdf, 2);
    if eob == 0 {
        return 0;
    }

    let mut levels_buf = [0u8; LEVELS_SCRATCH_LEN];
    txb_init_levels(coeffs, width, height, &mut levels_buf);

    if plane_type == 0 {
        if is_inter {
            write_tx_type_inter(fc, w, tx_type, tx_size, base_q_idx, reduced_tx_set);
        } else {
            write_tx_type_intra(
                fc,
                w,
                intra_dir,
                tx_type,
                tx_size,
                base_q_idx,
                reduced_tx_set,
            );
        }
    }

    let (eob_pt, eob_extra) = eob_pos_token(eob);
    let eob_multi_size = TXSIZE_LOG2_MINUS4[tx_size];
    let eob_multi_ctx = usize::from(tx_class != TX_CLASS_2D);
    match eob_multi_size {
        0 => {
            let cdf = &mut fc.eob_flag_cdf16[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 5);
        }
        1 => {
            let cdf = &mut fc.eob_flag_cdf32[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 6);
        }
        2 => {
            let cdf = &mut fc.eob_flag_cdf64[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 7);
        }
        3 => {
            let cdf = &mut fc.eob_flag_cdf128[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 8);
        }
        4 => {
            let cdf = &mut fc.eob_flag_cdf256[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 9);
        }
        5 => {
            let cdf = &mut fc.eob_flag_cdf512[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 10);
        }
        _ => {
            let cdf = &mut fc.eob_flag_cdf1024[plane_type * 2 + eob_multi_ctx];
            w.write_symbol(eob_pt - 1, cdf, 11);
        }
    }

    let eob_offset_bits = i32::from(EOB_OFFSET_BITS[eob_pt]);
    if eob_offset_bits > 0 {
        let eob_ctx = eob_pt - 3;
        let mut eob_shift = eob_offset_bits - 1;
        let bit = (eob_extra & (1 << eob_shift)) != 0;
        let cdf = fc.eob_extra(txs_ctx, plane_type, eob_ctx);
        w.write_symbol(usize::from(bit), cdf, 2);
        for i in 1..eob_offset_bits {
            eob_shift = eob_offset_bits - 1 - i;
            w.write_bit((eob_extra & (1 << eob_shift)) != 0);
        }
    }

    let mut coeff_contexts = [0i8; 32 * 32];
    get_nz_map_contexts(
        &levels_buf,
        scan,
        eob as usize,
        tx_size,
        tx_class,
        &mut coeff_contexts,
    );

    for c in (0..eob as usize).rev() {
        let pos = scan[c] as usize;
        let v = coeffs[pos];
        let coeff_ctx = coeff_contexts[pos] as usize;
        let level = v.abs();

        if c == eob as usize - 1 {
            let cdf = fc.coeff_base_eob(txs_ctx, plane_type, coeff_ctx);
            w.write_symbol((level.min(3) - 1) as usize, cdf, 3);
        } else {
            let cdf = fc.coeff_base(txs_ctx, plane_type, coeff_ctx);
            w.write_symbol(level.min(3) as usize, cdf, 4);
        }
        if level > NUM_BASE_LEVELS {
            let base_range = level - 1 - NUM_BASE_LEVELS;
            let ctx = br_ctx(&levels_buf, pos, bwl, tx_class);
            let mut idx = 0i32;
            while idx < COEFF_BASE_RANGE {
                let k = (base_range - idx).min(BR_CDF_SIZE as i32 - 1);
                let cdf = fc.coeff_br(txs_ctx.min(TX_32X32), plane_type, ctx);
                w.write_symbol(k as usize, cdf, BR_CDF_SIZE);
                if k < BR_CDF_SIZE as i32 - 1 {
                    break;
                }
                idx += BR_CDF_SIZE as i32 - 1;
            }
        }
    }

    // Signs and golomb residuals, forward scan order, DC sign first.
    let mut cul_level: i32 = 0;
    for c in 0..eob as usize {
        let pos = scan[c] as usize;
        let v = coeffs[pos];
        let level = v.abs();
        cul_level += level;
        let sign = usize::from(v < 0);
        if level != 0 {
            if c == 0 {
                let cdf = fc.dc_sign(plane_type, dc_sign_ctx);
                w.write_symbol(sign, cdf, 2);
            } else {
                w.write_bit(sign != 0);
            }
            if level > COEFF_BASE_RANGE + NUM_BASE_LEVELS {
                write_golomb(w, level - COEFF_BASE_RANGE - 1 - NUM_BASE_LEVELS);
            }
        }
    }

    cul_level = cul_level.min(COEFF_CONTEXT_MASK);
    // C `set_dc_sign`: fold the DC sign into the neighbor byte.
    if coeffs[0] < 0 {
        cul_level |= 1 << COEFF_CONTEXT_BITS;
    } else if coeffs[0] > 0 {
        cul_level |= 2 << COEFF_CONTEXT_BITS;
    }
    cul_level
}

/// C `svt_aom_get_txb_ctx` (entropy_coding.c:~275) with explicit neighbor
/// byte slices: each entry is one 4x4 unit's `(dc_sign << 6) | min(cul, 63)`,
/// or `0xFF` (`INVALID_NEIGHBOR_DATA`) when unavailable.
///
/// `plane_bsize_eq_txsize` selects the luma fast path (txb_skip_ctx = 0);
/// `is_chroma_larger` chooses the +10 vs +7 chroma offset.
pub fn get_txb_ctx(
    plane: usize,
    above: &[u8],
    left: &[u8],
    plane_bsize_eq_txsize: bool,
    is_chroma_larger: bool,
) -> (usize, usize) {
    static SIGNS: [i32; 3] = [0, -1, 1];
    const INVALID: u8 = 0xFF;

    let mut dc_sign: i32 = 0;
    if !above.is_empty() && above[0] != INVALID {
        for &b in above {
            let sign = b >> COEFF_CONTEXT_BITS;
            debug_assert!(sign <= 2);
            dc_sign += SIGNS[sign as usize];
        }
    }
    if !left.is_empty() && left[0] != INVALID {
        for &b in left {
            let sign = b >> COEFF_CONTEXT_BITS;
            debug_assert!(sign <= 2);
            dc_sign += SIGNS[sign as usize];
        }
    }
    let dc_sign_ctx = if dc_sign > 0 {
        2
    } else if dc_sign < 0 {
        1
    } else {
        0
    };

    let txb_skip_ctx = if plane == 0 {
        if plane_bsize_eq_txsize {
            0
        } else {
            static SKIP_CONTEXTS: [[u8; 5]; 5] = [
                [1, 2, 2, 2, 3],
                [1, 4, 4, 4, 5],
                [1, 4, 4, 4, 5],
                [1, 4, 4, 4, 5],
                [1, 4, 4, 4, 6],
            ];
            let mut top: i32 = 0;
            let mut l: i32 = 0;
            if !above.is_empty() && above[0] != INVALID {
                for &b in above {
                    top |= i32::from(b);
                }
            }
            top &= COEFF_CONTEXT_MASK;
            if !left.is_empty() && left[0] != INVALID {
                for &b in left {
                    l |= i32::from(b);
                }
            }
            l &= COEFF_CONTEXT_MASK;
            let max = (top | l).min(4);
            let min = top.min(l).min(4);
            SKIP_CONTEXTS[min as usize][max as usize] as usize
        }
    } else {
        let mut ctx_base_top = 0i32;
        let mut ctx_base_left = 0i32;
        if !above.is_empty() && above[0] != INVALID {
            for &b in above {
                ctx_base_top += i32::from(b != 0);
            }
        }
        if !left.is_empty() && left[0] != INVALID {
            for &b in left {
                ctx_base_left += i32::from(b != 0);
            }
        }
        let ctx_base = usize::from(ctx_base_left != 0) + usize::from(ctx_base_top != 0);
        let ctx_offset = if is_chroma_larger { 10 } else { 7 };
        ctx_base + ctx_offset
    };

    (txb_skip_ctx, dc_sign_ctx)
}

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

use crate::entropy::cdf::AomCdfProb;
use crate::entropy::default_cdfs as d;
use crate::entropy::scan_tables;
use crate::entropy::writer::AomWriter;

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

/// The 19 legal `(w, h)` shapes as a 5x5 table indexed by
/// `(log2(w) - 2) * 5 + (log2(h) - 2)`; `INVALID_TX_DIMS` marks the six
/// combinations AV1 has no transform for (aspect ratio beyond 4:1).
///
/// Row order is w = 4, 8, 16, 32, 64; column order is h = 4, 8, 16, 32, 64.
/// `tx_size_from_dims_match` below is the readable form this replaces and is
/// the oracle `tx_size_table_matches_the_match_form` checks it against.
const INVALID_TX_DIMS: u8 = u8::MAX;
#[rustfmt::skip]
const TX_SIZE_BY_DIMS: [u8; 25] = [
    //          h=4                h=8                 h=16                 h=32                 h=64
    /* w= 4 */ TX_4X4   as u8, TX_4X8   as u8, TX_4X16  as u8, INVALID_TX_DIMS,   INVALID_TX_DIMS,
    /* w= 8 */ TX_8X4   as u8, TX_8X8   as u8, TX_8X16  as u8, TX_8X32  as u8, INVALID_TX_DIMS,
    /* w=16 */ TX_16X4  as u8, TX_16X8  as u8, TX_16X16 as u8, TX_16X32 as u8, TX_16X64 as u8,
    /* w=32 */ INVALID_TX_DIMS,   TX_32X8  as u8, TX_32X16 as u8, TX_32X32 as u8, TX_32X64 as u8,
    /* w=64 */ INVALID_TX_DIMS,   INVALID_TX_DIMS,   TX_64X16 as u8, TX_64X32 as u8, TX_64X64 as u8,
];

/// Map transform dimensions in pixels to the C `TxSize` index.
///
/// A table index, not a 19-arm `match`: this is called once per `tx_unit_inner`
/// (4,238,186 times on a 512x512 photo at preset 2) and the branch chain was
/// 24 Ir a call that LLVM would not inline away — 100.5 M Ir, 0.22 % of the
/// frame, for an operation that is two shifts and a load. The `match` form is
/// kept below as the table's test oracle.
#[inline]
pub fn tx_size_from_dims(w: usize, h: usize) -> usize {
    let (iw, ih) = (w.trailing_zeros(), h.trailing_zeros());
    // Rejects a non-power-of-two, anything outside 4..=64, and the six illegal
    // aspect ratios, with the same message the `match` form panicked with.
    if w.is_power_of_two() && h.is_power_of_two() && (2..=6).contains(&iw) && (2..=6).contains(&ih)
    {
        let v = TX_SIZE_BY_DIMS[(iw as usize - 2) * 5 + (ih as usize - 2)];
        if v != INVALID_TX_DIMS {
            return v as usize;
        }
    }
    panic!("no TxSize for {w}x{h}")
}

/// The readable form of [`tx_size_from_dims`], kept as the table's oracle.
#[cfg(test)]
pub(crate) fn tx_size_from_dims_match(w: usize, h: usize) -> Option<usize> {
    Some(match (w, h) {
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
        _ => return None,
    })
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

/// Largest coefficient-coding area a transform block can have, in samples.
///
/// [`adjusted_tx_size`] caps every 64-dimension transform at 32 for
/// COEFFICIENT coding (C `av1_get_adjusted_tx_size`), so
/// `txb_wide(tx) * txb_high(tx) <= 32 * 32` for every `tx_size` the encoder
/// can reach. That bound is what lets the per-txb `coeff_contexts` scratch be
/// a fixed stack array instead of a per-call `Vec`.
pub const MAX_TXB_COEFF_AREA: usize = 32 * 32;

/// Offset of the (0,0) level inside the padded buffer (C `set_levels`).
#[inline]
pub const fn levels_origin(width: usize) -> usize {
    TX_PAD_TOP * (width + TX_PAD_HOR)
}

/// C `svt_av1_txb_init_levels_c`: zero the padded map and fill
/// `levels[row * (width+4) + col] = min(|coeff|, 127)` at the origin offset.
///
/// The value fill is SIMD-dispatched (see [`crate::entropy::coeff_simd::fill_levels`]) —
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
    let used = levels_used_len(width, height, levels_buf.len());
    for b in levels_buf[..used].iter_mut() {
        *b = 0;
    }
    crate::entropy::coeff_simd::fill_levels(coeff, width, height, levels_buf);
}

/// The prefix [`txb_init_levels`] zeroes for a `(width, height)` txb — every
/// byte any reader of the map touches for that shape lies inside it (see that
/// function's comment for the derivation).
///
/// Exposed because the call sites that SKIP `txb_init_levels` (`eob <= 1`,
/// where there is no body to fill) still READ the map, and with the shared
/// per-thread [`TxbScratch`] they must reproduce the all-zero buffer a
/// per-call stack array used to hand them. Zeroing this prefix is the exact
/// reproduction; zeroing the whole `LEVELS_SCRATCH_LEN` was what made the old
/// stack array cost `LEVELS_SCRATCH_LEN` (1,456) bytes of `memset` per call.
#[inline]
pub fn levels_used_len(width: usize, height: usize, buf_len: usize) -> usize {
    let stride = width + TX_PAD_HOR;
    ((TX_PAD_TOP + height + TX_PAD_BOTTOM + 4) * stride + TX_PAD_END).min(buf_len)
}

/// Per-thread scratch for the padded coefficient-LEVEL map and the nz-map
/// context array.
///
/// FOUR call sites carried an identical `let mut levels_buf = [0u8;
/// LEVELS_SCRATCH_LEN]` — `cost_coeffs_txb`, `cost_coeffs_txb_pd0`,
/// `optimize_b_tc` and `write_coeffs_txb_1d` — and two of them a
/// `[0i8; MAX_TXB_COEFF_AREA]` beside it. That is 1,456 + 1,024 bytes of stack
/// zeroed on EVERY call, and `txb_init_levels` immediately re-zeroes the only
/// part that matters: the `used` prefix, as little as ~112 bytes for a 4x4.
///
/// C does not pay this. It keeps ONE persistent `md_levels_buf` whose pad is
/// zeroed once at `md_process.c:235` and refills only the body per txb; the
/// comment on `txb_init_levels` has said so since it was written. This is that
/// shape.
///
/// Nothing re-zeroes the buffer on entry, so a site that skips
/// `txb_init_levels` MUST call [`levels_used_len`] and zero that prefix
/// itself.
pub struct TxbScratch {
    /// The padded level map — `txb_init_levels`' buffer.
    pub levels: [u8; LEVELS_SCRATCH_LEN],
    /// `get_nz_map_contexts`' output, `width * height` of which is used.
    pub ctx: [i8; MAX_TXB_COEFF_AREA],
}

impl TxbScratch {
    const fn new() -> Self {
        TxbScratch {
            levels: [0u8; LEVELS_SCRATCH_LEN],
            ctx: [0i8; MAX_TXB_COEFF_AREA],
        }
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static TXB_SCRATCH: core::cell::Cell<Option<alloc::boxed::Box<TxbScratch>>> =
        const { core::cell::Cell::new(None) };
}

/// Run `f` with the per-thread [`TxbScratch`].
///
/// `Cell::take` leaves `None` behind, so a RE-ENTRANT call builds its own
/// buffer rather than aliasing this one or panicking. Nothing in the encoder
/// nests these calls today; this makes a future one correct instead of a
/// hazard.
///
/// POSITIVE CONTROL. Each site re-zeroes only the `used` prefix, so a read
/// outside that prefix would silently see the PREVIOUS txb's map where a fresh
/// stack array gave it a zero. The buffer is therefore POISONED (0xAA / -86)
/// before every hand-out, so such a read changes coded bits instead of hiding.
///
/// **`cargo nextest` does not witness that read** — measured, not assumed: with
/// the `eob <= 1` prefix-zeroing deliberately removed AND the poison on, the
/// whole 2,509-test suite still passes, because the debug suite never reaches
/// an `eob == 1` txb whose DC level exceeds `NUM_BASE_LEVELS`. The evidence for
/// the `used` bound is a RELEASE build with the poison made unconditional:
/// teeth `regression_spotcheck` 96/100 (four cells with real size differences)
/// without the zeroing, and 100/100 + `identity_full_8bit` **1100/1100** with
/// it. See `benchmarks/levelscratch_ab_2026-09-03.meta`. The `debug_assertions`
/// poison below is kept because it costs nothing and a future test may reach
/// the case — it is NOT the control.
#[inline]
pub fn with_txb_scratch<R>(f: impl FnOnce(&mut TxbScratch) -> R) -> R {
    #[cfg(feature = "std")]
    {
        let mut b = TXB_SCRATCH
            .with(|c| c.take())
            .unwrap_or_else(|| alloc::boxed::Box::new(TxbScratch::new()));
        #[cfg(debug_assertions)]
        {
            b.levels.fill(0xAA);
            b.ctx.fill(-86);
        }
        let r = f(&mut b);
        TXB_SCRATCH.with(|c| c.set(Some(b)));
        r
    }
    #[cfg(not(feature = "std"))]
    {
        let mut b = TxbScratch::new();
        f(&mut b)
    }
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
///
/// `TC` is the transform class as a CONST, so the three-way branch below folds
/// away exactly as C's does: C's callers reach this through
/// `UPDATE_COEFF_EOB_CASE(TX_CLASS_2D)` and friends (full_loop.c), macros that
/// expand the whole trellis once per class with `tx_class` a literal. See
/// [`nz_mag`] for the runtime-dispatched wrapper.
#[inline(always)]
fn nz_mag_tc<const TC: usize>(levels: &[u8], base: usize, bwl: usize) -> u32 {
    let mut mag = clip_max3(levels[base + 1]);
    mag += clip_max3(levels[base + (1 << bwl) + TX_PAD_HOR]);
    match TC {
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
///
/// This is the DEFINITION, not the hot path. The run-time lookup goes through
/// [`crate::entropy::coeff_simd::nz_offset_2d`], a compile-time table built by
/// calling this very function — exactly as C reads a static table rather than
/// re-deriving. Keep this the single source of truth; the table is pinned to
/// it by `coeff_simd::nz_offset_tests`.
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

/// C `get_nz_map_ctx_from_stats`, with the transform class as a CONST.
#[inline(always)]
fn nz_map_ctx_from_stats_tc<const TC: usize>(
    stats: u32,
    coeff_idx: usize,
    bwl: usize,
    tx_size: usize,
) -> usize {
    if (TC | coeff_idx) == 0 {
        return 0;
    }
    let ctx = (((stats + 1) >> 1) as usize).min(4);
    match TC {
        TX_CLASS_2D => ctx + crate::entropy::coeff_simd::nz_offset_2d(tx_size, coeff_idx),
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

/// C `get_nz_map_ctx` (encode_txb_ref_c.c:17), with the transform class as a
/// CONST — the shape C gets for free from its per-class macro expansion.
#[inline(always)]
pub(crate) fn nz_map_ctx_tc<const TC: usize>(
    levels: &[u8],
    origin: usize,
    coeff_idx: usize,
    bwl: usize,
    height: usize,
    scan_idx: usize,
    is_eob: bool,
    tx_size: usize,
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
    let stats = nz_mag_tc::<TC>(levels, origin + padded_idx(coeff_idx, bwl), bwl);
    nz_map_ctx_from_stats_tc::<TC>(stats, coeff_idx, bwl, tx_size)
}

/// The value the `_` fall-through arms of [`nz_map_ctx_tc`]'s inner matches
/// take. No [`TX_TYPE_TO_CLASS`] entry produces it (pinned by
/// `tx_class_tests::tx_type_to_class_is_ternary`), but instantiating it keeps
/// [`nz_map_ctx`] byte-identical to the pre-monomorphisation code for EVERY
/// input rather than only the reachable ones.
const TX_CLASS_UNREACHABLE: usize = 3;

/// Runtime-`tx_class` wrapper over [`nz_map_ctx_tc`], for the callers that do
/// not know the class at compile time (the scan-order reference loop and the
/// parity tests).
#[inline(always)]
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
    match tx_class {
        TX_CLASS_2D => nz_map_ctx_tc::<TX_CLASS_2D>(
            levels, origin, coeff_idx, bwl, height, scan_idx, is_eob, tx_size,
        ),
        TX_CLASS_HORIZ => nz_map_ctx_tc::<TX_CLASS_HORIZ>(
            levels, origin, coeff_idx, bwl, height, scan_idx, is_eob, tx_size,
        ),
        TX_CLASS_VERT => nz_map_ctx_tc::<TX_CLASS_VERT>(
            levels, origin, coeff_idx, bwl, height, scan_idx, is_eob, tx_size,
        ),
        _ => nz_map_ctx_tc::<TX_CLASS_UNREACHABLE>(
            levels, origin, coeff_idx, bwl, height, scan_idx, is_eob, tx_size,
        ),
    }
}

/// C `get_lower_levels_ctx_general` (coefficients.h:195 + the
/// `get_lower_levels_ctx_eob` is_last branch, coefficients.h:55): the
/// per-coefficient base-level context the RDOQ trellis
/// (`svt_av1_optimize_b`) prices with. `levels_buf` is the full padded
/// buffer from [`txb_init_levels`]; `ci` is the packed raster position.
#[inline(always)]
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

/// [`lower_levels_ctx_general`] with the transform class as a CONST.
#[inline(always)]
pub fn lower_levels_ctx_general_tc<const TC: usize>(
    levels_buf: &[u8],
    ci: usize,
    bwl: usize,
    height: usize,
    scan_idx: usize,
    is_last: bool,
    tx_size: usize,
) -> usize {
    nz_map_ctx_tc::<TC>(
        levels_buf,
        levels_origin(1 << bwl),
        ci,
        bwl,
        height,
        scan_idx,
        is_last,
        tx_size,
    )
}

/// C `get_br_ctx_eob` (coefficients.h:68) — the coeff_br context for the
/// last (eob) coefficient, which never reads neighbor levels.
#[inline(always)]
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

/// [`br_ctx_eob`] with the transform class as a CONST.
#[inline(always)]
pub fn br_ctx_eob_tc<const TC: usize>(c: usize, bwl: usize) -> usize {
    let row = c >> bwl;
    let col = c - (row << bwl);
    if c == 0 {
        return 0;
    }
    if (TC == TX_CLASS_2D && row < 2 && col < 2)
        || (TC == TX_CLASS_HORIZ && col == 0)
        || (TC == TX_CLASS_VERT && row == 0)
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
    crate::entropy::coeff_simd::nz_map_contexts(
        levels_buf,
        scan,
        eob,
        tx_size,
        tx_class,
        coeff_contexts,
    );
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
///
/// Runtime-`tx_class` wrapper over [`br_ctx_tc`]; see [`nz_map_ctx`] for why
/// the `_` arm is instantiated rather than asserted away.
#[inline(always)]
pub fn br_ctx(levels_buf: &[u8], c: usize, bwl: usize, tx_class: usize) -> usize {
    match tx_class {
        TX_CLASS_2D => br_ctx_tc::<TX_CLASS_2D>(levels_buf, c, bwl),
        TX_CLASS_HORIZ => br_ctx_tc::<TX_CLASS_HORIZ>(levels_buf, c, bwl),
        TX_CLASS_VERT => br_ctx_tc::<TX_CLASS_VERT>(levels_buf, c, bwl),
        _ => br_ctx_tc::<TX_CLASS_UNREACHABLE>(levels_buf, c, bwl),
    }
}

/// [`br_ctx`] with the transform class as a CONST.
#[inline(always)]
pub fn br_ctx_tc<const TC: usize>(levels_buf: &[u8], c: usize, bwl: usize) -> usize {
    let row = c >> bwl;
    let col = c - (row << bwl);
    let stride = (1 << bwl) + TX_PAD_HOR;
    // C indexes `levels` from the set_levels origin.
    let pos = levels_origin(1 << bwl) + row * stride + col;
    let mut mag = levels_buf[pos + 1] as u32;
    mag += levels_buf[pos + stride] as u32;
    match TC {
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
    /// SHIPPED-C QUIRK, CDF-UPDATE half (issue #16). `false` = bitstream
    /// semantics (every real writer). `true` = this context is one of the
    /// MD-side per-SB rate contexts C evolves in its ENCODE pass, where an
    /// IntraBC block's luma tx type is adapted by
    /// `av1_transform_type_rate_estimation` (rd_cost.c:107) at
    /// `allow_update_cdf = 1` — called from `svt_av1_cost_coeffs_txb`, whose
    /// `is_inter = is_inter_mode(mode)` (rd_cost.c) ignores `use_intrabc`. So
    /// C updates `intra_ext_tx_cdf[intra_eset][sqr][DC_PRED]` with the INTRA
    /// set's symbol and leaves the inter row untouched, while its writer
    /// (`av1_write_tx_type`, `use_intrabc || is_inter_mode`) codes the inter
    /// row. The MD rate tables C rebuilds per SB therefore see a DC row that
    /// IntraBC blocks adapted. Only the encoder's chain simulation sets this.
    pub md_side_ibc_txt_update: bool,
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
            md_side_ibc_txt_update: false,
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
        use crate::entropy::cdf::avg_cdf_entries as avg;
        avg(
            self.txb_skip_cdf.as_flattened_mut(),
            tr.txb_skip_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_extra_cdf.as_flattened_mut(),
            tr.eob_extra_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.dc_sign_cdf.as_flattened_mut(),
            tr.dc_sign_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf16.as_flattened_mut(),
            tr.eob_flag_cdf16.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf32.as_flattened_mut(),
            tr.eob_flag_cdf32.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf64.as_flattened_mut(),
            tr.eob_flag_cdf64.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf128.as_flattened_mut(),
            tr.eob_flag_cdf128.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf256.as_flattened_mut(),
            tr.eob_flag_cdf256.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf512.as_flattened_mut(),
            tr.eob_flag_cdf512.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.eob_flag_cdf1024.as_flattened_mut(),
            tr.eob_flag_cdf1024.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.coeff_base_eob_cdf.as_flattened_mut(),
            tr.coeff_base_eob_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.coeff_base_cdf.as_flattened_mut(),
            tr.coeff_base_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.coeff_br_cdf.as_flattened_mut(),
            tr.coeff_br_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(
            self.intra_ext_tx_cdf.as_flattened_mut(),
            tr.intra_ext_tx_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        // C AVG_CDF_STRIDE over inter_ext_tx_cdf sets 1..3 (enc_dec_process.c
        // :2675-2677); set 0 + the beyond-nsymbs tails are identical zeros on
        // both sides, so the full-slice average is value-equivalent (the same
        // established convention as intra_ext_tx_cdf above).
        avg(
            self.inter_ext_tx_cdf.as_flattened_mut(),
            tr.inter_ext_tx_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
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

/// C `av1_transform_type_rate_estimation` (rd_cost.c:107) at
/// `allow_update_cdf = 1` for an IntraBC block's luma txb — the MD-side CDF
/// update C's encode pass performs (`coding_loop.c:1539` ->
/// `svt_aom_txb_estimate_coeff_bits` -> `svt_av1_cost_coeffs_txb`, whose
/// `is_inter = is_inter_mode(mode)` is FALSE for IntraBC). It takes the
/// INTRA arm: the intra ext-tx set for `tx_size` (DCT-only at 32x32+, so no
/// update there, unlike the writer's inter 12-/2-type sets), row
/// `intra_ext_tx_cdf[intra_eset][square][DC_PRED]` (an IntraBC block's
/// `mode` is DC_PRED, `filter_intra_mode` off), symbol
/// `av1_ext_tx_ind[intra_set][tx_type]` — which is that table's filler 0 for
/// a tx type outside the intra set (e.g. an inter-set flip type), so such a
/// txb adapts the DCT_DCT symbol — and the intra set's symbol count. No
/// `base_q_idx > 0` gate (that is the writer's; QP 0 is refused upstream).
/// Nothing is written: only the CDF moves. See `CoeffFc::md_side_ibc_txt_update`.
pub fn md_update_tx_type_ibc_quirk(
    fc: &mut CoeffFc,
    tx_type: usize,
    tx_size: usize,
    reduced_tx_set: bool,
) {
    if ext_tx_types(tx_size, false, reduced_tx_set) > 1 {
        let square_tx_size = TXSIZE_SQR_MAP[tx_size];
        let set_type = ext_tx_set_type(tx_size, false, reduced_tx_set);
        let eset = ext_tx_set(tx_size, false, reduced_tx_set);
        if eset > 0 {
            let cdf = fc.intra_ext_tx(eset as usize, square_tx_size, 0 /* DC_PRED */);
            crate::entropy::cdf::update_cdf(
                cdf,
                AV1_EXT_TX_IND[set_type][tx_type],
                AV1_NUM_EXT_TX_SET[set_type],
            );
        }
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
    with_txb_scratch(|sc| {
        write_coeffs_txb_1d_inner(
            fc,
            w,
            tx_size,
            tx_type,
            plane_type,
            txb_skip_ctx,
            dc_sign_ctx,
            coeffs,
            eob,
            intra_dir,
            base_q_idx,
            reduced_tx_set,
            is_inter,
            &mut sc.levels,
        )
    })
}

/// [`write_coeffs_txb_1d`]'s body, with the level map supplied by the caller.
#[allow(clippy::too_many_arguments)]
fn write_coeffs_txb_1d_inner(
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
    levels_buf: &mut [u8; LEVELS_SCRATCH_LEN],
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

    // `txb_init_levels` zeroes the whole `used` prefix itself, so the scratch
    // arrives dirty and leaves correct — this site needs no extra clear.
    txb_init_levels(coeffs, width, height, levels_buf);

    if plane_type == 0 {
        if is_inter {
            if fc.md_side_ibc_txt_update {
                // MD-side context (issue #16): C's encode pass adapts the
                // INTRA DC row for this IntraBC txb, not the inter row the
                // bitstream writer codes. The symbol itself is not written
                // — the chain simulation's arithmetic state is discarded.
                md_update_tx_type_ibc_quirk(fc, tx_type, tx_size, reduced_tx_set);
            } else {
                write_tx_type_inter(fc, w, tx_type, tx_size, base_q_idx, reduced_tx_set);
            }
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
        &levels_buf[..],
        scan,
        eob as usize,
        tx_size,
        tx_class,
        &mut coeff_contexts,
    );

    // The base-range escape, shared by the peeled `c == eob - 1` iteration and
    // the loop below (C writes it out twice; one macro keeps it in one place
    // and compiles to the same thing).
    let br_txs_ctx = txs_ctx.min(TX_32X32);
    macro_rules! write_br {
        ($level:expr, $pos:expr) => {
            if $level > NUM_BASE_LEVELS {
                let base_range = $level - 1 - NUM_BASE_LEVELS;
                let ctx = br_ctx(&levels_buf[..], $pos, bwl, tx_class);
                let mut idx = 0i32;
                while idx < COEFF_BASE_RANGE {
                    let k = (base_range - idx).min(BR_CDF_SIZE as i32 - 1);
                    let cdf = fc.coeff_br(br_txs_ctx, plane_type, ctx);
                    w.write_symbol(k as usize, cdf, BR_CDF_SIZE);
                    if k < BR_CDF_SIZE as i32 - 1 {
                        break;
                    }
                    idx += BR_CDF_SIZE as i32 - 1;
                }
            }
        };
    }

    // PEELED first iteration, `c == eob - 1`, exactly as C peels it
    // (entropy_coding.c:477-497). The port ran one loop that re-tested
    // `c == eob - 1` on every coefficient to pick between `coeff_base_eob_cdf`
    // (3 symbols) and `coeff_base_cdf` (4) — a loop-invariant predicate paid
    // per coefficient. `eob >= 1` holds here (`eob == 0` returned above).
    // Byte-inert: same symbols, same order.
    {
        let c = eob as usize - 1;
        let pos = scan[c] as usize;
        let v = coeffs[pos];
        let coeff_ctx = coeff_contexts[pos] as usize;
        let level = v.abs();
        let cdf = fc.coeff_base_eob(txs_ctx, plane_type, coeff_ctx);
        w.write_symbol((level.min(3) - 1) as usize, cdf, 3);
        write_br!(level, pos);
    }
    for c in (0..eob as usize - 1).rev() {
        let pos = scan[c] as usize;
        let v = coeffs[pos];
        let coeff_ctx = coeff_contexts[pos] as usize;
        let level = v.abs();
        let cdf = fc.coeff_base(txs_ctx, plane_type, coeff_ctx);
        w.write_symbol(level.min(3) as usize, cdf, 4);
        write_br!(level, pos);
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

#[cfg(test)]
mod tx_size_table_tests {
    use super::*;

    /// The 5x5 `TX_SIZE_BY_DIMS` table must agree with the 19-arm `match` it
    /// replaced on EVERY power-of-two pair in 4..=64, accepting exactly the
    /// same 19 shapes and rejecting exactly the same six.
    #[test]
    fn tx_size_table_matches_the_match_form() {
        let dims = [4usize, 8, 16, 32, 64];
        let mut accepted = 0;
        for &w in &dims {
            for &h in &dims {
                match tx_size_from_dims_match(w, h) {
                    Some(want) => {
                        assert_eq!(tx_size_from_dims(w, h), want, "tx {w}x{h}");
                        accepted += 1;
                    }
                    None => {
                        assert!(
                            std::panic::catch_unwind(|| tx_size_from_dims(w, h)).is_err(),
                            "tx {w}x{h} must be rejected"
                        );
                    }
                }
            }
        }
        assert_eq!(accepted, 19, "the table must accept exactly 19 shapes");
    }

    /// Non-power-of-two and out-of-range dimensions still panic rather than
    /// indexing the table (the `match` form had no arm for them either).
    #[test]
    fn tx_size_table_rejects_illegal_dims() {
        for (w, h) in [
            (0usize, 4usize),
            (4, 0),
            (2, 4),
            (4, 2),
            (128, 64),
            (64, 128),
            (12, 4),
        ] {
            assert!(
                std::panic::catch_unwind(|| tx_size_from_dims(w, h)).is_err(),
                "tx {w}x{h} must be rejected"
            );
        }
    }
}

#[cfg(test)]
mod tx_class_tests {
    use super::*;

    /// The RDOQ trellis and the entropy writers dispatch on `tx_class` with a
    /// three-arm `match` that instantiates a const-generic body per class
    /// (C does the same with `UPDATE_COEFF_EOB_CASE`, full_loop.c). That
    /// dispatch is exhaustive only because `tx_type_to_class` is ternary — if
    /// a fourth class ever appeared, the trellis's `_ => TX_CLASS_2D` arm
    /// would silently mis-price it. Pin the table.
    #[test]
    fn tx_type_to_class_is_ternary() {
        for (tx_type, &class) in TX_TYPE_TO_CLASS.iter().enumerate() {
            assert!(
                class <= TX_CLASS_VERT,
                "tx_type {tx_type} maps to class {class}, outside 0..=2"
            );
        }
        assert!(TX_CLASS_UNREACHABLE > TX_CLASS_VERT);
    }

    /// The const-generic context helpers must agree with the runtime-dispatch
    /// wrappers at every class — including the unreachable fall-through, which
    /// is what makes the wrappers byte-identical to the pre-monomorphisation
    /// code for every input and not merely the reachable ones.
    #[test]
    fn tc_helpers_match_their_runtime_wrappers() {
        // A padded levels buffer with a deterministic non-trivial fill.
        let mut levels = vec![0u8; LEVELS_SCRATCH_LEN];
        for (i, v) in levels.iter_mut().enumerate() {
            *v = ((i * 37 + 11) % 9) as u8;
        }
        for &tx_size in &[0usize, 1, 2, 3, 4, 5, 6, 7, 8] {
            let bwl = txb_bwl(tx_size);
            let height = txb_high(tx_size);
            let origin = levels_origin(txb_wide(tx_size));
            let n = txb_wide(tx_size) * height;
            for ci in 0..n {
                for scan_idx in [0usize, 1, n / 3, n.saturating_sub(1)] {
                    for is_eob in [false, true] {
                        for tx_class in 0..=TX_CLASS_UNREACHABLE {
                            let want = nz_map_ctx(
                                &levels, origin, ci, bwl, height, scan_idx, is_eob, tx_size,
                                tx_class,
                            );
                            let got = match tx_class {
                                TX_CLASS_2D => nz_map_ctx_tc::<TX_CLASS_2D>(
                                    &levels, origin, ci, bwl, height, scan_idx, is_eob, tx_size,
                                ),
                                TX_CLASS_HORIZ => nz_map_ctx_tc::<TX_CLASS_HORIZ>(
                                    &levels, origin, ci, bwl, height, scan_idx, is_eob, tx_size,
                                ),
                                TX_CLASS_VERT => nz_map_ctx_tc::<TX_CLASS_VERT>(
                                    &levels, origin, ci, bwl, height, scan_idx, is_eob, tx_size,
                                ),
                                _ => nz_map_ctx_tc::<TX_CLASS_UNREACHABLE>(
                                    &levels, origin, ci, bwl, height, scan_idx, is_eob, tx_size,
                                ),
                            };
                            assert_eq!(
                                want, got,
                                "nz_map_ctx tx_size={tx_size} ci={ci} si={scan_idx} \
                                 eob={is_eob} class={tx_class}"
                            );
                        }
                    }
                }
                for tx_class in 0..=TX_CLASS_UNREACHABLE {
                    let want_br = br_ctx(&levels, ci, bwl, tx_class);
                    let got_br = match tx_class {
                        TX_CLASS_2D => br_ctx_tc::<TX_CLASS_2D>(&levels, ci, bwl),
                        TX_CLASS_HORIZ => br_ctx_tc::<TX_CLASS_HORIZ>(&levels, ci, bwl),
                        TX_CLASS_VERT => br_ctx_tc::<TX_CLASS_VERT>(&levels, ci, bwl),
                        _ => br_ctx_tc::<TX_CLASS_UNREACHABLE>(&levels, ci, bwl),
                    };
                    assert_eq!(want_br, got_br, "br_ctx ci={ci} class={tx_class}");

                    if tx_class <= TX_CLASS_VERT {
                        let want_eob = br_ctx_eob(ci, bwl, tx_class);
                        let got_eob = match tx_class {
                            TX_CLASS_2D => br_ctx_eob_tc::<TX_CLASS_2D>(ci, bwl),
                            TX_CLASS_HORIZ => br_ctx_eob_tc::<TX_CLASS_HORIZ>(ci, bwl),
                            _ => br_ctx_eob_tc::<TX_CLASS_VERT>(ci, bwl),
                        };
                        assert_eq!(want_eob, got_eob, "br_ctx_eob ci={ci} class={tx_class}");
                    }
                }
            }
        }
    }
}

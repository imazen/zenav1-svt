//! The wedge mask tables and their accessors.
//!
//! Ported from `Source/Lib/Codec/inter_prediction.c` (SVT-AV1 v4.2.0):
//! `shift_copy` (:2042), `aom_convolve_copy_c` (:2027),
//! `init_wedge_primary_masks` (:2066), `get_wedge_mask_inplace` (:2162),
//! `init_wedge_masks` (:2177), `svt_av1_init_wedge_masks` (:2206),
//! `svt_aom_is_interintra_wedge_used` (:2015),
//! `svt_aom_get_wedge_bits_lookup` (:2019),
//! `svt_aom_get_contiguous_soft_mask` (:2023) and
//! `svt_aom_get_wedge_params_bits` (:2053).
//!
//! # The two `#if` arms, checked rather than assumed
//!
//! `USE_PRECOMPUTED_WEDGE_MASK` and `USE_PRECOMPUTED_WEDGE_SIGN` are both
//! **1** (inter_prediction.c:1514-1515). So:
//!
//! * `init_wedge_primary_masks` compiles the `shift_copy`-of-precomputed-tables
//!   arm (:2071-2087). Its `#else` (:2087-2102) — `sqrt` / `tanh` / `rint` on
//!   `double` — is DEAD, and porting it would both diverge and import
//!   transcendental cross-ISA risk for nothing. It is not ported, deliberately.
//! * `init_wedge_signs` (:2124) is entirely inside `#if
//!   !USE_PRECOMPUTED_WEDGE_SIGN`, so it never compiles; `wedge_signflip_lookup`
//!   is the literal table below. Not ported, for the same reason.
//!
//! # The indexing that silently corrupts every mask if you get it wrong
//!
//! `get_wedge_mask_inplace` returns a pointer *into* the 64x64 oblique
//! prototype at `MASK_PRIMARY_STRIDE * (MASK_PRIMARY_SIZE / 2 - hoff) +
//! MASK_PRIMARY_SIZE / 2 - woff`, and it picks the plane with
//! `neg ^ wsignflip` — an XOR of the requested sign with the per-(bsize,
//! index) signflip bit, NOT the requested sign alone. An off-by-one in either
//! offset, or dropping the XOR, produces a plausible-looking mask that is
//! wrong for every block.

use alloc::vec;
use alloc::vec::Vec;

/// `MAX_WEDGE_TYPES` (definitions.h:1276) — `1 << 4`.
pub const MAX_WEDGE_TYPES: usize = 16;
/// `MAX_WEDGE_SIZE_LOG2` (definitions.h:1277).
pub const MAX_WEDGE_SIZE_LOG2: usize = 5;
/// `MAX_WEDGE_SIZE` (definitions.h:1278) — 32.
pub const MAX_WEDGE_SIZE: usize = 1 << MAX_WEDGE_SIZE_LOG2;
/// `MASK_PRIMARY_SIZE` (definitions.h:1282) — `MAX_WEDGE_SIZE << 1` = 64.
pub const MASK_PRIMARY_SIZE: usize = MAX_WEDGE_SIZE << 1;
/// `MASK_PRIMARY_STRIDE` (definitions.h:1283).
pub const MASK_PRIMARY_STRIDE: usize = MASK_PRIMARY_SIZE;
/// `WEDGE_WEIGHT_BITS` (definitions.h:1281).
pub const WEDGE_WEIGHT_BITS: usize = 6;
/// `BLOCK_SIZES_ALL`.
pub const BLOCK_SIZES_ALL: usize = 22;

/// `WedgeDirectionType` (inter_prediction.h:70).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WedgeDirection {
    /// `WEDGE_HORIZONTAL`
    Horizontal = 0,
    /// `WEDGE_VERTICAL`
    Vertical = 1,
    /// `WEDGE_OBLIQUE27`
    Oblique27 = 2,
    /// `WEDGE_OBLIQUE63`
    Oblique63 = 3,
    /// `WEDGE_OBLIQUE117`
    Oblique117 = 4,
    /// `WEDGE_OBLIQUE153`
    Oblique153 = 5,
}
const WEDGE_DIRECTIONS: usize = 6;

/// `WedgeCodeType` (inter_prediction.h:88) — `{direction, x_offset, y_offset}`.
#[derive(Debug, Clone, Copy)]
pub struct WedgeCode {
    /// Which oblique/axis prototype this code shifts.
    pub direction: WedgeDirection,
    /// Horizontal offset in eighths of the block width.
    pub x_offset: i32,
    /// Vertical offset in eighths of the block height.
    pub y_offset: i32,
}

const fn wc(direction: WedgeDirection, x_offset: i32, y_offset: i32) -> WedgeCode {
    WedgeCode {
        direction,
        x_offset,
        y_offset,
    }
}

use WedgeDirection::{Horizontal, Oblique27, Oblique63, Oblique117, Oblique153, Vertical};

/// `wedge_codebook_16_hgtw` (inter_prediction.c:1932) — height greater than width.
pub const WEDGE_CODEBOOK_16_HGTW: [WedgeCode; MAX_WEDGE_TYPES] = [
    wc(Oblique27, 4, 4),
    wc(Oblique63, 4, 4),
    wc(Oblique117, 4, 4),
    wc(Oblique153, 4, 4),
    wc(Horizontal, 4, 2),
    wc(Horizontal, 4, 4),
    wc(Horizontal, 4, 6),
    wc(Vertical, 4, 4),
    wc(Oblique27, 4, 2),
    wc(Oblique27, 4, 6),
    wc(Oblique153, 4, 2),
    wc(Oblique153, 4, 6),
    wc(Oblique63, 2, 4),
    wc(Oblique63, 6, 4),
    wc(Oblique117, 2, 4),
    wc(Oblique117, 6, 4),
];

/// `wedge_codebook_16_hltw` (inter_prediction.c:1951) — height less than width.
pub const WEDGE_CODEBOOK_16_HLTW: [WedgeCode; MAX_WEDGE_TYPES] = [
    wc(Oblique27, 4, 4),
    wc(Oblique63, 4, 4),
    wc(Oblique117, 4, 4),
    wc(Oblique153, 4, 4),
    wc(Vertical, 2, 4),
    wc(Vertical, 4, 4),
    wc(Vertical, 6, 4),
    wc(Horizontal, 4, 4),
    wc(Oblique27, 4, 2),
    wc(Oblique27, 4, 6),
    wc(Oblique153, 4, 2),
    wc(Oblique153, 4, 6),
    wc(Oblique63, 2, 4),
    wc(Oblique63, 6, 4),
    wc(Oblique117, 2, 4),
    wc(Oblique117, 6, 4),
];

/// `wedge_codebook_16_heqw` (inter_prediction.c:1970) — square blocks.
pub const WEDGE_CODEBOOK_16_HEQW: [WedgeCode; MAX_WEDGE_TYPES] = [
    wc(Oblique27, 4, 4),
    wc(Oblique63, 4, 4),
    wc(Oblique117, 4, 4),
    wc(Oblique153, 4, 4),
    wc(Horizontal, 4, 2),
    wc(Horizontal, 4, 6),
    wc(Vertical, 2, 4),
    wc(Vertical, 6, 4),
    wc(Oblique27, 4, 2),
    wc(Oblique27, 4, 6),
    wc(Oblique153, 4, 2),
    wc(Oblique153, 4, 6),
    wc(Oblique63, 2, 4),
    wc(Oblique63, 6, 4),
    wc(Oblique117, 2, 4),
    wc(Oblique117, 6, 4),
];

/// `wedge_params_lookup[bsize].bits` (inter_prediction.c:1989). Nonzero for the
/// nine block sizes AV1 allows wedges on: 8x8, 8x16, 16x8, 16x16, 16x32, 32x16,
/// 32x32, 8x32, 32x8.
pub const WEDGE_BITS_LOOKUP: [i32; BLOCK_SIZES_ALL] = [
    0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0,
];

/// Which codebook `wedge_params_lookup[bsize]` points at, as an index into
/// `[HEQW, HGTW, HLTW]`. `None` where `bits == 0`.
const WEDGE_CODEBOOK_SELECT: [Option<u8>; BLOCK_SIZES_ALL] = [
    None,
    None,
    None,
    Some(0), // 8x8   heqw
    Some(1), // 8x16  hgtw
    Some(2), // 16x8  hltw
    Some(0), // 16x16 heqw
    Some(1), // 16x32 hgtw
    Some(2), // 32x16 hltw
    Some(0), // 32x32 heqw
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    Some(1), // 8x32  hgtw
    Some(2), // 32x8  hltw
    None,
    None,
];

/// `wedge_signflip_lookup` (inter_prediction.c:1534), transcribed verbatim.
/// `USE_PRECOMPUTED_WEDGE_SIGN` is 1, so this table IS the answer —
/// `init_wedge_signs`, which would derive it, never compiles.
pub const WEDGE_SIGNFLIP_LOOKUP: [[u8; MAX_WEDGE_TYPES]; BLOCK_SIZES_ALL] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0, 1, 1, 1, 0, 1],
    [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 0, 1, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

/// `wedge_primary_oblique_odd` (inter_prediction.c:1517).
const WEDGE_PRIMARY_OBLIQUE_ODD: [u8; MASK_PRIMARY_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 6,
    18, 37, 53, 60, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];

/// `wedge_primary_oblique_even` (inter_prediction.c:1522).
const WEDGE_PRIMARY_OBLIQUE_EVEN: [u8; MASK_PRIMARY_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 4, 11,
    27, 46, 58, 62, 63, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];

/// `wedge_primary_vertical` (inter_prediction.c:1527).
const WEDGE_PRIMARY_VERTICAL: [u8; MASK_PRIMARY_SIZE] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 7,
    21, 43, 57, 62, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
    64, 64, 64, 64, 64, 64, 64, 64, 64,
];

/// `block_size_wide` / `block_size_high` for the 22 sizes, in the order
/// `BlockSize` uses.
const BLOCK_W: [usize; BLOCK_SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; BLOCK_SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// `shift_copy` (inter_prediction.c:2042) — copy `src` into `dst` shifted right
/// by `shift` (or left when negative), replicating the edge sample into the gap.
pub fn shift_copy(src: &[u8], dst: &mut [u8], shift: i32, width: usize) {
    if shift >= 0 {
        let shift = shift as usize;
        dst[shift..width].copy_from_slice(&src[..width - shift]);
        dst[..shift].fill(src[0]);
    } else {
        let shift = (-shift) as usize;
        dst[..width - shift].copy_from_slice(&src[shift..width]);
        dst[width - shift..width].fill(src[width - 1]);
    }
}

/// The built wedge mask tables. C keeps these in file-scope arrays initialised
/// once by `svt_av1_init_wedge_masks`; here they are an owned value so nothing
/// is global mutable state.
pub struct WedgeMasks {
    /// `wedge_mask_obl[negative][direction]`, each `MASK_PRIMARY_SIZE` square.
    obl: Vec<u8>,
    /// `wedge_mask_buf` — the packed per-bsize masks.
    buf: Vec<u8>,
    /// Byte offset into `buf` of `wedge_params_lookup[bsize].masks[sign][idx]`.
    offsets: [[[u32; MAX_WEDGE_TYPES]; 2]; BLOCK_SIZES_ALL],
}

#[inline]
fn obl_index(neg: usize, dir: WedgeDirection) -> usize {
    (neg * WEDGE_DIRECTIONS + dir as usize) * MASK_PRIMARY_SIZE * MASK_PRIMARY_SIZE
}

impl Default for WedgeMasks {
    fn default() -> Self {
        Self::new()
    }
}

impl WedgeMasks {
    /// `svt_av1_init_wedge_masks` (inter_prediction.c:2206) —
    /// `init_wedge_primary_masks` then `init_wedge_masks`.
    /// `init_wedge_signs` is skipped because `USE_PRECOMPUTED_WEDGE_SIGN` is 1.
    pub fn new() -> Self {
        let mut this = Self {
            obl: vec![0u8; 2 * WEDGE_DIRECTIONS * MASK_PRIMARY_SIZE * MASK_PRIMARY_SIZE],
            buf: Vec::new(),
            offsets: [[[u32::MAX; MAX_WEDGE_TYPES]; 2]; BLOCK_SIZES_ALL],
        };
        this.init_wedge_primary_masks();
        this.init_wedge_masks();
        this
    }

    /// `init_wedge_primary_masks` (inter_prediction.c:2066), the
    /// `USE_PRECOMPUTED_WEDGE_MASK == 1` arm.
    fn init_wedge_primary_masks(&mut self) {
        let w = MASK_PRIMARY_SIZE;
        let h = MASK_PRIMARY_SIZE;
        let stride = MASK_PRIMARY_STRIDE;

        // The shift walks DOWN by one every two rows, starting at h/4.
        let mut shift = (h / 4) as i32;
        let o63 = obl_index(0, Oblique63);
        let overt = obl_index(0, Vertical);
        let mut i = 0;
        while i < h {
            let base = o63 + i * stride;
            let (evensrc, oddsrc) = (&WEDGE_PRIMARY_OBLIQUE_EVEN, &WEDGE_PRIMARY_OBLIQUE_ODD);
            shift_copy(
                evensrc,
                &mut self.obl[base..base + MASK_PRIMARY_SIZE],
                shift,
                MASK_PRIMARY_SIZE,
            );
            shift -= 1;
            let base_odd = o63 + (i + 1) * stride;
            shift_copy(
                oddsrc,
                &mut self.obl[base_odd..base_odd + MASK_PRIMARY_SIZE],
                shift,
                MASK_PRIMARY_SIZE,
            );
            let v0 = overt + i * stride;
            self.obl[v0..v0 + MASK_PRIMARY_SIZE].copy_from_slice(&WEDGE_PRIMARY_VERTICAL);
            let v1 = overt + (i + 1) * stride;
            self.obl[v1..v1 + MASK_PRIMARY_SIZE].copy_from_slice(&WEDGE_PRIMARY_VERTICAL);
            i += 2;
        }

        // Derive the other five directions and both complements from the two
        // prototypes. The transposes and the `w - 1 - j` mirrors are the part
        // an off-by-one silently corrupts.
        let max = (1 << WEDGE_WEIGHT_BITS) as u8;
        for i in 0..h {
            for j in 0..w {
                let msk = self.obl[obl_index(0, Oblique63) + i * stride + j];
                self.obl[obl_index(0, Oblique27) + j * stride + i] = msk;
                self.obl[obl_index(0, Oblique117) + i * stride + (w - 1 - j)] = max - msk;
                self.obl[obl_index(0, Oblique153) + (w - 1 - j) * stride + i] = max - msk;
                self.obl[obl_index(1, Oblique63) + i * stride + j] = max - msk;
                self.obl[obl_index(1, Oblique27) + j * stride + i] = max - msk;
                self.obl[obl_index(1, Oblique117) + i * stride + (w - 1 - j)] = msk;
                self.obl[obl_index(1, Oblique153) + (w - 1 - j) * stride + i] = msk;

                let mskx = self.obl[obl_index(0, Vertical) + i * stride + j];
                self.obl[obl_index(0, Horizontal) + j * stride + i] = mskx;
                self.obl[obl_index(1, Vertical) + i * stride + j] = max - mskx;
                self.obl[obl_index(1, Horizontal) + j * stride + i] = max - mskx;
            }
        }
    }

    /// `get_wedge_mask_inplace` (inter_prediction.c:2162) -> the offset into
    /// [`Self::obl`] of the mask's top-left sample.
    ///
    /// The plane is `neg ^ wsignflip`, not `neg`.
    fn wedge_mask_inplace_offset(&self, wedge_index: usize, neg: usize, bsize: usize) -> usize {
        let bw = BLOCK_W[bsize];
        let bh = BLOCK_H[bsize];
        let cb = codebook_for(bsize).expect("wedge mask requested for a bsize with bits == 0");
        let a = cb[wedge_index];
        let wsignflip = WEDGE_SIGNFLIP_LOOKUP[bsize][wedge_index] as usize;
        let woff = ((a.x_offset * bw as i32) >> 3) as usize;
        let hoff = ((a.y_offset * bh as i32) >> 3) as usize;
        obl_index(neg ^ wsignflip, a.direction)
            + MASK_PRIMARY_STRIDE * (MASK_PRIMARY_SIZE / 2 - hoff)
            + MASK_PRIMARY_SIZE / 2
            - woff
    }

    /// `init_wedge_masks` (inter_prediction.c:2177) — pack each `bw x bh` mask
    /// (both signs) contiguously via `aom_convolve_copy_c`.
    fn init_wedge_masks(&mut self) {
        for bsize in 0..BLOCK_SIZES_ALL {
            if WEDGE_BITS_LOOKUP[bsize] == 0 {
                continue;
            }
            let bw = BLOCK_W[bsize];
            let bh = BLOCK_H[bsize];
            let wtypes = 1usize << WEDGE_BITS_LOOKUP[bsize];
            for w in 0..wtypes {
                for sign in 0..2usize {
                    let src = self.wedge_mask_inplace_offset(w, sign, bsize);
                    let dst_off = self.buf.len();
                    for r in 0..bh {
                        let s = src + r * MASK_PRIMARY_STRIDE;
                        self.buf.extend_from_slice(&self.obl[s..s + bw]);
                    }
                    self.offsets[bsize][sign][w] = dst_off as u32;
                }
            }
        }
    }

    /// `svt_aom_get_contiguous_soft_mask` (inter_prediction.c:2023) —
    /// `wedge_params_lookup[bsize].masks[wedge_sign][wedge_index]`, a `bw x bh`
    /// block with stride `bw`.
    pub fn contiguous_soft_mask(
        &self,
        wedge_index: usize,
        wedge_sign: usize,
        bsize: usize,
    ) -> &[u8] {
        let off = self.offsets[bsize][wedge_sign][wedge_index];
        assert_ne!(off, u32::MAX, "no wedge mask for bsize {bsize}");
        let n = BLOCK_W[bsize] * BLOCK_H[bsize];
        &self.buf[off as usize..off as usize + n]
    }
}

/// The codebook `wedge_params_lookup[bsize]` points at.
pub fn codebook_for(bsize: usize) -> Option<&'static [WedgeCode; MAX_WEDGE_TYPES]> {
    match WEDGE_CODEBOOK_SELECT[bsize] {
        Some(0) => Some(&WEDGE_CODEBOOK_16_HEQW),
        Some(1) => Some(&WEDGE_CODEBOOK_16_HGTW),
        Some(2) => Some(&WEDGE_CODEBOOK_16_HLTW),
        _ => None,
    }
}

/// `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015) — gates whether
/// `wedge_interintra` syntax is written for a bsize, so it is bitstream-visible.
pub fn is_interintra_wedge_used(bsize: usize) -> bool {
    WEDGE_BITS_LOOKUP[bsize] > 0
}

/// `svt_aom_get_wedge_bits_lookup` (inter_prediction.c:2019).
pub fn get_wedge_bits_lookup(bsize: usize) -> i32 {
    WEDGE_BITS_LOOKUP[bsize]
}

/// `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053) — the same value
/// by a second name; both exist upstream and both are called.
pub fn get_wedge_params_bits(bsize: usize) -> i32 {
    WEDGE_BITS_LOOKUP[bsize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly nine block sizes carry wedges, and each carries 4 bits.
    #[test]
    fn wedge_bits_shape() {
        let used: Vec<usize> = (0..BLOCK_SIZES_ALL)
            .filter(|&b| is_interintra_wedge_used(b))
            .collect();
        assert_eq!(used, vec![3, 4, 5, 6, 7, 8, 9, 18, 19]);
        for b in used {
            assert_eq!(get_wedge_bits_lookup(b), 4);
            assert_eq!(get_wedge_params_bits(b), 4);
            assert!(codebook_for(b).is_some());
        }
    }

    /// `shift_copy` replicates the edge sample into the vacated span, in both
    /// directions.
    #[test]
    fn shift_copy_replicates_edges() {
        let src = [9u8, 1, 2, 3, 4, 5, 6, 7];
        let mut dst = [0u8; 8];
        shift_copy(&src, &mut dst, 3, 8);
        assert_eq!(dst, [9, 9, 9, 9, 1, 2, 3, 4]);
        shift_copy(&src, &mut dst, -2, 8);
        assert_eq!(dst, [2, 3, 4, 5, 6, 7, 7, 7]);
        shift_copy(&src, &mut dst, 0, 8);
        assert_eq!(dst, src);
    }

    /// Every mask value is a valid A64 alpha, and the two signs complement.
    #[test]
    fn masks_are_complementary_alphas() {
        let m = WedgeMasks::new();
        for bsize in 0..BLOCK_SIZES_ALL {
            if !is_interintra_wedge_used(bsize) {
                continue;
            }
            for idx in 0..MAX_WEDGE_TYPES {
                let a = m.contiguous_soft_mask(idx, 0, bsize);
                let b = m.contiguous_soft_mask(idx, 1, bsize);
                assert_eq!(a.len(), BLOCK_W[bsize] * BLOCK_H[bsize]);
                for (x, y) in a.iter().zip(b.iter()) {
                    assert!(
                        *x <= 64 && *y <= 64,
                        "alpha out of range at bsize {bsize} idx {idx}"
                    );
                    assert_eq!(*x as u16 + *y as u16, 64, "signs must complement");
                }
            }
        }
    }
}

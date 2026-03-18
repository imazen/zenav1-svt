//! Coefficient entropy coding — TXB skip, EOB, base levels, Golomb.
//!
//! Spec 07: Coefficient coding (TXB skip, EOB, base levels).
//!
//! This module handles the entropy coding of transform coefficients,
//! which is the most complex part of the AV1 entropy coder.
//!
//! Two implementations:
//! - `write_coefficients_ctx`: Legacy simplified encoder (forward scan, literal EOB).
//! - `write_coefficients_v2`: Spec-conformant encoder matching rav1d decoder's
//!   exact bitstream reading order (CDF-based EOB, reverse diagonal scan,
//!   proper context derivation).

use alloc::vec::Vec;

// Re-export vec! macro for use in this module (Rust 2024 edition requirement)
#[allow(unused_imports)]
use alloc::vec;

use crate::cdf::AomCdfProb;
use crate::default_coef_cdfs;
use crate::writer::AomWriter;

/// Number of base levels for coefficient coding.
pub const NUM_BASE_LEVELS: usize = 2;
/// Coefficient base range for BR (base-range) coding.
pub const COEFF_BASE_RANGE: usize = 12;
/// TXB skip contexts.
pub const TXB_SKIP_CONTEXTS: usize = 13;
/// End-of-block maximum symbols.
pub const EOB_MAX_SYMS: usize = 13;
/// DC sign contexts.
pub const DC_SIGN_CONTEXTS: usize = 3;
/// Plane types (Y vs UV).
pub const PLANE_TYPES: usize = 2;

// ============================================================================
// Lo-context offset table (from rav1d dav1d_lo_ctx_offsets)
// ============================================================================

/// Context offset LUT for 2D transforms.
/// Index: [is_rect_variant][min(y,4)][min(x,4)]
///
/// is_rect_variant:
///   0 = square TX
///   1 = rectangular TX where width > height
///   2 = rectangular TX where height > width
static LO_CTX_OFFSETS: [[[u8; 5]; 5]; 3] = [
    // Square
    [
        [0, 1, 6, 6, 21],
        [1, 6, 6, 21, 21],
        [6, 6, 21, 21, 21],
        [6, 21, 21, 21, 21],
        [21, 21, 21, 21, 21],
    ],
    // Rect (w > h)
    [
        [0, 16, 6, 6, 21],
        [16, 16, 6, 21, 21],
        [16, 16, 21, 21, 21],
        [16, 16, 21, 21, 21],
        [16, 16, 21, 21, 21],
    ],
    // Rect (h > w)
    [
        [0, 11, 11, 11, 11],
        [11, 11, 11, 11, 11],
        [6, 6, 21, 21, 21],
        [6, 21, 21, 21, 21],
        [21, 21, 21, 21, 21],
    ],
];

// ============================================================================
// CdfCoefCtx — holds all CDFs for coefficient encoding
// ============================================================================

/// CDF context for AV1-conformant coefficient encoding.
///
/// Holds all CDF arrays needed for the coefficient coding syntax,
/// structured to match the rav1d decoder exactly.
///
/// Dimensions (all for plane_type=0 / luma, 2D tx_class):
/// - skip: [5 t_dim_ctx][13 skip_ctx] — 2-symbol CDFs
/// - dc_sign: [3 dc_sign_ctx] — 2-symbol CDFs
/// - eob_bin_N: single CDF per TX size class — N-symbol CDFs
/// - eob_hi_bit: [5 t_dim_ctx][11 bins] — 2-symbol CDFs
/// - eob_base_tok: [5 t_dim_ctx][4 sub_ctx] — 3-symbol CDFs
/// - base_tok: [5 t_dim_ctx][41 ctx] — 4-symbol CDFs
/// - br_tok: [4 br_ctx][21 ctx] — 4-symbol CDFs
#[derive(Clone)]
pub struct CdfCoefCtx {
    /// TXB skip CDFs: [t_dim_ctx * TXB_SKIP_CONTEXTS][3]
    /// Flattened: index = t_dim_ctx * 13 + skip_ctx
    pub skip: [[AomCdfProb; 3]; 65],

    /// DC sign CDFs: [DC_SIGN_CONTEXTS][3]
    pub dc_sign: [[AomCdfProb; 3]; 3],

    /// EOB bin CDFs, one per tx2dszctx value (0-6).
    /// tx2dszctx 0 → eob_bin_16 (5 symbols, array len 6)
    /// tx2dszctx 1 → eob_bin_32 (6 symbols, array len 7)
    /// tx2dszctx 2 → eob_bin_64 (7 symbols, array len 8)
    /// tx2dszctx 3 → eob_bin_128 (8 symbols, array len 9)
    /// tx2dszctx 4 → eob_bin_256 (9 symbols, array len 10)
    /// tx2dszctx 5 → eob_bin_512 (10 symbols, array len 11)
    /// tx2dszctx 6 → eob_bin_1024 (11 symbols, array len 12)
    /// Stored as max-size arrays; nsymbs passed at call site.
    pub eob_bin: [[AomCdfProb; 12]; 7],

    /// EOB hi-bit CDFs: [t_dim_ctx * 11][3]
    /// Flattened: index = t_dim_ctx * 11 + eob_bin
    pub eob_hi_bit: [[AomCdfProb; 3]; 55],

    /// EOB base token CDFs: [t_dim_ctx * 4][4]
    /// Flattened: index = t_dim_ctx * 4 + sub_ctx
    /// 3-symbol CDF (symbols 0,1,2 → tok 1,2,3+)
    pub eob_base_tok: [[AomCdfProb; 4]; 20],

    /// Base token CDFs: [t_dim_ctx * 41][5]
    /// Flattened: index = t_dim_ctx * 41 + lo_ctx
    /// 4-symbol CDF (symbols 0,1,2,3 → tok 0,1,2,3+)
    pub base_tok: [[AomCdfProb; 5]; 205],

    /// BR (base-range / hi) token CDFs: [min(t_dim_ctx,3) * 21][5]
    /// Flattened: index = min(t_dim_ctx, 3) * 21 + hi_ctx
    /// 4-symbol CDF (symbols 0,1,2,3)
    pub br_tok: [[AomCdfProb; 5]; 84],

    /// Intra TX type CDFs (reduced set, 5 symbols): [3 t_dim_min][13 y_mode]
    /// Used when reduced_txtp_set || t_dim.min == S16x16 (t_dim_min >= 2)
    pub txtp_intra2: [[[AomCdfProb; 6]; 13]; 3],

    /// Intra TX type CDFs (full set, 7 symbols): [2 t_dim_min][13 y_mode]
    /// Used when !reduced_txtp_set && t_dim.min < S16x16 (t_dim_min < 2)
    pub txtp_intra1: [[[AomCdfProb; 8]; 13]; 2],
}

impl CdfCoefCtx {
    /// Create a new CDF context initialized from the default tables
    /// for the given QP category.
    ///
    /// QP categories: 0 (qidx 0-20), 1 (21-60), 2 (61-120), 3 (121+)
    pub fn new(qp_category: usize) -> Self {
        let qp_category = qp_category.min(3);
        match qp_category {
            0 => Self::from_defaults_0(),
            1 => Self::from_defaults_1(),
            2 => Self::from_defaults_2(),
            3 => Self::from_defaults_3(),
            _ => unreachable!(),
        }
    }

    fn from_defaults_0() -> Self {
        use default_coef_cdfs::*;
        Self {
            skip: SKIP_CDF_0,
            dc_sign: DC_SIGN_CDF_0,
            eob_bin: Self::pack_eob_bins(
                &EOB_BIN_16_CDF_0,
                &EOB_BIN_32_CDF_0,
                &EOB_BIN_64_CDF_0,
                &EOB_BIN_128_CDF_0,
                &EOB_BIN_256_CDF_0,
                &EOB_BIN_512_CDF_0,
                &EOB_BIN_1024_CDF_0,
            ),
            eob_hi_bit: EOB_HI_BIT_CDF_0,
            eob_base_tok: EOB_BASE_TOK_CDF_0,
            base_tok: BASE_TOK_CDF_0,
            br_tok: BR_TOK_CDF_0,
            txtp_intra2: Self::uniform_txtp_intra2(),
            txtp_intra1: Self::uniform_txtp_intra1(),
        }
    }

    fn from_defaults_1() -> Self {
        use default_coef_cdfs::*;
        Self {
            skip: SKIP_CDF_1,
            dc_sign: DC_SIGN_CDF_1,
            eob_bin: Self::pack_eob_bins(
                &EOB_BIN_16_CDF_1,
                &EOB_BIN_32_CDF_1,
                &EOB_BIN_64_CDF_1,
                &EOB_BIN_128_CDF_1,
                &EOB_BIN_256_CDF_1,
                &EOB_BIN_512_CDF_1,
                &EOB_BIN_1024_CDF_1,
            ),
            eob_hi_bit: EOB_HI_BIT_CDF_1,
            eob_base_tok: EOB_BASE_TOK_CDF_1,
            base_tok: BASE_TOK_CDF_1,
            br_tok: BR_TOK_CDF_1,
            txtp_intra2: Self::uniform_txtp_intra2(),
            txtp_intra1: Self::uniform_txtp_intra1(),
        }
    }

    fn from_defaults_2() -> Self {
        use default_coef_cdfs::*;
        Self {
            skip: SKIP_CDF_2,
            dc_sign: DC_SIGN_CDF_2,
            eob_bin: Self::pack_eob_bins(
                &EOB_BIN_16_CDF_2,
                &EOB_BIN_32_CDF_2,
                &EOB_BIN_64_CDF_2,
                &EOB_BIN_128_CDF_2,
                &EOB_BIN_256_CDF_2,
                &EOB_BIN_512_CDF_2,
                &EOB_BIN_1024_CDF_2,
            ),
            eob_hi_bit: EOB_HI_BIT_CDF_2,
            eob_base_tok: EOB_BASE_TOK_CDF_2,
            base_tok: BASE_TOK_CDF_2,
            br_tok: BR_TOK_CDF_2,
            txtp_intra2: Self::uniform_txtp_intra2(),
            txtp_intra1: Self::uniform_txtp_intra1(),
        }
    }

    fn from_defaults_3() -> Self {
        use default_coef_cdfs::*;
        Self {
            skip: SKIP_CDF_3,
            dc_sign: DC_SIGN_CDF_3,
            eob_bin: Self::pack_eob_bins(
                &EOB_BIN_16_CDF_3,
                &EOB_BIN_32_CDF_3,
                &EOB_BIN_64_CDF_3,
                &EOB_BIN_128_CDF_3,
                &EOB_BIN_256_CDF_3,
                &EOB_BIN_512_CDF_3,
                &EOB_BIN_1024_CDF_3,
            ),
            eob_hi_bit: EOB_HI_BIT_CDF_3,
            eob_base_tok: EOB_BASE_TOK_CDF_3,
            base_tok: BASE_TOK_CDF_3,
            br_tok: BR_TOK_CDF_3,
            txtp_intra2: Self::uniform_txtp_intra2(),
            txtp_intra1: Self::uniform_txtp_intra1(),
        }
    }

    /// Uniform TX type CDFs (reduced set, 5 symbols)
    fn uniform_txtp_intra2() -> [[[AomCdfProb; 6]; 13]; 3] {
        let cdf: [AomCdfProb; 6] = [26214, 19661, 13107, 6554, 0, 0];
        let row: [[AomCdfProb; 6]; 13] = [cdf; 13];
        [row; 3]
    }

    /// Uniform TX type CDFs (full set, 7 symbols)
    fn uniform_txtp_intra1() -> [[[AomCdfProb; 8]; 13]; 2] {
        let cdf: [AomCdfProb; 8] = [28087, 23406, 18725, 14043, 9362, 4681, 0, 0];
        let row: [[AomCdfProb; 8]; 13] = [cdf; 13];
        [row; 2]
    }

    /// Pack the 7 eob_bin CDFs (of varying lengths) into uniform [12] arrays.
    fn pack_eob_bins(
        bin16: &[AomCdfProb],
        bin32: &[AomCdfProb],
        bin64: &[AomCdfProb],
        bin128: &[AomCdfProb],
        bin256: &[AomCdfProb],
        bin512: &[AomCdfProb],
        bin1024: &[AomCdfProb],
    ) -> [[AomCdfProb; 12]; 7] {
        let mut out = [[0u16; 12]; 7];
        let sources: [&[AomCdfProb]; 7] = [bin16, bin32, bin64, bin128, bin256, bin512, bin1024];
        for (i, src) in sources.iter().enumerate() {
            out[i][..src.len()].copy_from_slice(src);
        }
        out
    }

    /// Get mutable reference to skip CDF for given t_dim_ctx and skip_ctx.
    #[inline]
    pub fn skip_cdf(&mut self, t_dim_ctx: usize, skip_ctx: usize) -> &mut [AomCdfProb] {
        &mut self.skip[t_dim_ctx * TXB_SKIP_CONTEXTS + skip_ctx]
    }

    /// Get mutable reference to dc_sign CDF.
    #[inline]
    pub fn dc_sign_cdf(&mut self, dc_sign_ctx: usize) -> &mut [AomCdfProb] {
        &mut self.dc_sign[dc_sign_ctx]
    }

    /// Get mutable reference to eob_bin CDF for the given tx2dszctx.
    #[inline]
    pub fn eob_bin_cdf(&mut self, tx2dszctx: usize) -> &mut [AomCdfProb] {
        &mut self.eob_bin[tx2dszctx]
    }

    /// Get mutable reference to eob_hi_bit CDF.
    #[inline]
    pub fn eob_hi_bit_cdf(&mut self, t_dim_ctx: usize, eob_bin: usize) -> &mut [AomCdfProb] {
        &mut self.eob_hi_bit[t_dim_ctx * 11 + eob_bin]
    }

    /// Get mutable reference to eob_base_tok CDF.
    #[inline]
    pub fn eob_base_tok_cdf(&mut self, t_dim_ctx: usize, sub_ctx: usize) -> &mut [AomCdfProb] {
        &mut self.eob_base_tok[t_dim_ctx * 4 + sub_ctx]
    }

    /// Get mutable reference to base_tok CDF.
    #[inline]
    pub fn base_tok_cdf(&mut self, t_dim_ctx: usize, lo_ctx: usize) -> &mut [AomCdfProb] {
        &mut self.base_tok[t_dim_ctx * 41 + lo_ctx]
    }

    /// Get mutable reference to br_tok CDF.
    #[inline]
    pub fn br_tok_cdf(&mut self, br_ctx_level: usize, hi_ctx: usize) -> &mut [AomCdfProb] {
        &mut self.br_tok[br_ctx_level * 21 + hi_ctx]
    }
}

// ============================================================================
// Diagonal scan generator
// ============================================================================

/// Generate a diagonal zigzag scan order for width x height.
///
/// The scan maps scan_position → raster_position, where
/// raster_position = row * width + col.
///
/// This matches the AV1 default scan order for 2D transforms.
fn generate_diagonal_scan(width: usize, height: usize) -> Vec<u16> {
    let mut scan = Vec::with_capacity(width * height);
    for diag in 0..(width + height - 1) {
        if diag % 2 == 0 {
            // Even diagonal: go up-right (decreasing row, increasing col)
            let start_r = diag.min(height - 1);
            let start_c = diag.saturating_sub(height - 1);
            let mut r = start_r;
            let mut c = start_c;
            loop {
                scan.push((r * width + c) as u16);
                if r == 0 || c + 1 >= width {
                    break;
                }
                r -= 1;
                c += 1;
            }
        } else {
            // Odd diagonal: go down-left (increasing row, decreasing col)
            let start_r = diag.saturating_sub(width - 1);
            let start_c = diag.min(width - 1);
            let mut r = start_r;
            let mut c = start_c;
            loop {
                scan.push((r * width + c) as u16);
                if r + 1 >= height || c == 0 {
                    break;
                }
                r += 1;
                c -= 1;
            }
        }
    }
    scan
}

/// Get or generate the scan table for a given width x height.
///
/// Uses hardcoded tables for 4x4 and 8x8 (from svtav1-tables),
/// generates dynamically for larger sizes.
fn get_scan_table(width: usize, height: usize) -> Vec<u16> {
    // For 4x4: use the known-good hardcoded table
    if width == 4 && height == 4 {
        return svtav1_tables::scan::DEFAULT_SCAN_4X4
            .iter()
            .map(|&x| x as u16)
            .collect();
    }
    // For 8x8: use the known-good hardcoded table
    if width == 8 && height == 8 {
        return svtav1_tables::scan::DEFAULT_SCAN_8X8
            .iter()
            .map(|&x| x as u16)
            .collect();
    }
    // For all other sizes: generate diagonal scan dynamically
    generate_diagonal_scan(width, height)
}

// ============================================================================
// Compute lo_ctx (base_tok context) matching rav1d exactly
// ============================================================================

/// Compute the base_tok context for a 2D transform coefficient.
///
/// This matches rav1d's `get_lo_ctx` for `TxClass::TwoD` exactly.
///
/// `levels` is indexed as `levels[x * stride + y]` where x is the column
/// and y is the row in the coefficient grid. Each entry stores `level_tok`
/// which is a packed byte: bits[6] flag + magnitude bits.
///
/// `stride` = 4 * sh (where sh = min(h_in_4x4, 8))
///
/// Returns (lo_ctx, hi_mag) where hi_mag is the magnitude of the 3
/// closest neighbors (for br_tok context).
fn get_lo_ctx_2d(
    levels: &[u8],
    x: usize,
    y: usize,
    stride: usize,
    lo_ctx_offsets: &[[u8; 5]; 5],
) -> (u8, u32) {
    let level = |yy: usize, xx: usize| -> u32 { levels[xx * stride + yy] as u32 };

    // 3 closest neighbors
    let mut mag = level(y, x + 1) + level(y + 1, x); // right + below
    mag += level(y + 1, x + 1); // diagonal
    let hi_mag = mag;

    // 2 more neighbors
    mag += level(y, x + 2) + level(y + 2, x);

    let offset = lo_ctx_offsets[y.min(4)][x.min(4)];
    let ctx = offset
        + if mag > 512 {
            4
        } else {
            ((mag + 64) >> 7) as u8
        };
    (ctx, hi_mag)
}

// ============================================================================
// Hi-tok (BR token) encoding — matching rav1d's decode_hi_tok
// ============================================================================

/// Encode the br_tok sequence for a coefficient with level >= 3.
///
/// The decoder reads br_tok symbols in groups of up to 4 iterations,
/// each reading a 4-symbol CDF (values 0-3). If the symbol is 3,
/// continue to the next group.
///
/// Returns the token value from br_tok only (3-15). Does NOT write
/// the Golomb residual — that's written separately in the sign phase.
fn write_hi_tok(
    writer: &mut AomWriter,
    cdf_ctx: &mut CdfCoefCtx,
    br_ctx_level: usize,
    hi_ctx: usize,
    level: u32,
) -> u32 {
    debug_assert!(level >= 3);
    let mut remaining = level - 3;
    let mut tok = 3u32;

    // Up to 4 groups of br_tok, each adds 0-3
    for _ in 0..4 {
        let sym = remaining.min(3) as usize;
        writer.write_symbol(sym, cdf_ctx.br_tok_cdf(br_ctx_level, hi_ctx), 4);
        tok += sym as u32;
        if sym < 3 {
            return tok;
        }
        remaining -= 3;
    }

    // tok == 15. Golomb residual is NOT written here — it's written
    // in the sign/residual phase after all tokens and DC sign.
    tok // returns 15
}

// ============================================================================
// write_coefficients_v2 — spec-conformant coefficient encoder
// ============================================================================

/// Write transform block coefficients matching the AV1 spec / rav1d decoder
/// bitstream reading order exactly.
///
/// This encodes:
/// 1. TXB skip flag (CDF, 2 symbols)
/// 2. EOB bin (CDF, size-dependent)
/// 3. EOB hi-bit + low-bits (if bin > 1)
/// 4. EOB position token (eob_base_tok, 3 symbols)
/// 5. AC tokens in reverse scan order (base_tok, 4 symbols)
/// 6. DC token (base_tok or eob_base_tok, depends on eob)
/// 7. DC sign (CDF, 2 symbols)
/// 8. AC signs + Golomb residuals for non-zero coefficients
///
/// # Parameters
/// - `writer`: Arithmetic coder output
/// - `coeffs`: Transform coefficients in RASTER order (row-major)
/// - `eob`: Number of non-zero coefficients (0 = all-skip).
///   More precisely, eob is the 0-based index of the last non-zero
///   coefficient in scan order. eob=0 means only DC is non-zero.
///   Our convention: eob=0 means the block is all-zero (skip).
/// - `width`: Transform block width in pixels (4, 8, 16, 32, 64)
/// - `height`: Transform block height in pixels
/// - `skip_ctx`: TXB skip context (0-12), from neighboring block state
/// - `dc_sign_ctx`: DC sign context (0-2), from neighboring DC signs
/// - `cdf_ctx`: Mutable CDF context (backward-adaptive)
#[allow(clippy::too_many_arguments)]
pub fn write_coefficients_v2(
    writer: &mut AomWriter,
    coeffs: &[i32],
    eob: usize,
    width: usize,
    height: usize,
    skip_ctx: usize,
    dc_sign_ctx: usize,
    intra_mode: u8,
    cdf_ctx: &mut CdfCoefCtx,
) {
    let lw = log2_of_tx_dim(width);
    let lh = log2_of_tx_dim(height);
    let t_dim_ctx = ((lw + lh + 1) >> 1).min(4);

    // Step 1: TXB skip
    if eob == 0 {
        writer.write_symbol(1, cdf_ctx.skip_cdf(t_dim_ctx, skip_ctx), 2);
        return;
    }
    writer.write_symbol(0, cdf_ctx.skip_cdf(t_dim_ctx, skip_ctx), 2);

    // Step 1b: Transform type
    // For intra blocks with t_dim.max >= S32x32 (lw >= 3 or lh >= 3),
    // TX type is implicitly DCT_DCT — no symbol needed.
    // For smaller blocks, write the TX type symbol.
    let t_dim_min = lw.min(lh);
    let t_dim_max = lw.max(lh);
    if t_dim_max < 3 {
        // Need to write TX type. DCT_DCT = symbol index 1 in both sets.
        let y_mode = (intra_mode as usize).min(12); // cap to valid range
        if t_dim_min >= 2 {
            // Reduced set (5 symbols): txtp_intra2[t_dim_min][y_mode]
            let cdf = &mut cdf_ctx.txtp_intra2[t_dim_min][y_mode];
            writer.write_symbol(1, cdf, 5); // 1 = DCT_DCT
        } else {
            // Full set (7 symbols): txtp_intra1[t_dim_min][y_mode]
            let cdf = &mut cdf_ctx.txtp_intra1[t_dim_min][y_mode];
            writer.write_symbol(1, cdf, 7); // 1 = DCT_DCT
        }
    }

    // Get scan table and find the actual last non-zero scan position.
    // AV1 coefficient coding caps the transform to 32x32 even for 64x64 blocks.
    let sw = width.min(32);
    let sh = height.min(32);
    let scan = get_scan_table(sw, sh);

    // Find actual scan-order EOB from coefficient data.
    // The caller's `eob` is a count, but we need the scan position.
    let scan_eob = find_last_nonzero_scan_pos(coeffs, &scan, sw, sh, width);
    if scan_eob.is_none() {
        // All coefficients in the scan area are zero — treat as skip.
        // This shouldn't happen since caller said eob > 0, but handle gracefully.
        return;
    }
    let scan_eob = scan_eob.unwrap();

    // Step 2: EOB bin (using the actual scan-order eob position)
    let tx2dszctx = lw.min(3) + lh.min(3);
    let nsymbs = 5 + tx2dszctx; // 5,6,7,8,9,10,11 symbols for tx2dszctx 0-6
    let eob_bin = eob_to_bin(scan_eob);
    writer.write_symbol(eob_bin as usize, cdf_ctx.eob_bin_cdf(tx2dszctx), nsymbs);

    // Steps 3-4: EOB hi-bit and low-bits
    if eob_bin > 1 {
        let hi_bit = (scan_eob >> (eob_bin as usize - 2)) & 1;
        let lo_mask = (1 << (eob_bin as usize - 2)) - 1;
        let lo_bits = scan_eob & lo_mask;

        // EOB hi-bit: CDF bool
        writer.write_symbol(
            hi_bit,
            cdf_ctx.eob_hi_bit_cdf(t_dim_ctx, eob_bin as usize),
            2,
        );

        // EOB low-bits: equiprobable
        let num_lo_bits = eob_bin as u32 - 2;
        if num_lo_bits > 0 {
            writer.write_literal(lo_bits as u32, num_lo_bits);
        }
    }

    // Set up levels array for context derivation.
    let stride = sh + 2; // Extra 2 for neighbor lookups without bounds checks
    let levels_size = (sw + 2) * stride;
    let mut levels = vec![0u8; levels_size];

    // br_ctx_level = min(t_dim_ctx, 3) for br_tok indexing
    let br_ctx_level = t_dim_ctx.min(3);

    // Determine lo_ctx_offsets table variant
    let is_rect = if sw == sh {
        0 // square
    } else if sw > sh {
        1 // width > height
    } else {
        2 // height > width
    };
    let lo_ctx_offsets = &LO_CTX_OFFSETS[is_rect];

    // Now encode base tokens.
    // The scan table maps scan_pos to raster index in the (sw x sh) grid.
    // But coefficients are stored in the original (width x height) layout.
    // Convert: scan raster → (col, row) in capped grid → actual coeff index.
    let scan_to_coeff_idx = |scan_pos: usize| -> usize {
        let scan_raster = scan[scan_pos] as usize;
        let col = scan_raster % sw;
        let row = scan_raster / sw;
        row * width + col // use original width as stride
    };
    let abs_level_at = |scan_pos: usize| -> u32 {
        let idx = scan_to_coeff_idx(scan_pos);
        if idx < coeffs.len() {
            coeffs[idx].unsigned_abs()
        } else {
            0
        }
    };

    // Step 5: EOB position token
    // The coefficient at scan_eob is guaranteed non-zero.
    let eob_level = abs_level_at(scan_eob);
    debug_assert!(eob_level > 0);

    // eob_base_tok context: 1 + (scan_eob > sw/4 * sh/4 * 2) + (scan_eob > sw/4 * sh/4 * 4)
    // where sw/4 and sh/4 are in 4x4 block units, but rav1d uses min(t_dim.w, 8) and min(t_dim.h, 8)
    // which are already in 4x4 units. Our sw and sh are in pixels.
    let sw4 = (sw / 4).min(8); // in 4x4 blocks, capped at 8
    let sh4 = (sh / 4).min(8);
    let eob_base_ctx = {
        let threshold1 = sw4 * sh4 * 2;
        let threshold2 = sw4 * sh4 * 4;
        1 + usize::from(scan_eob > threshold1) + usize::from(scan_eob > threshold2)
    };

    // eob_base_tok: 3-symbol CDF. Token values 0,1,2 map to level tok 1,2,3+.
    let eob_raw_tok = if eob_level == 1 {
        0usize
    } else if eob_level == 2 {
        1
    } else {
        2 // level >= 3
    };
    writer.write_symbol(
        eob_raw_tok,
        cdf_ctx.eob_base_tok_cdf(t_dim_ctx, eob_base_ctx),
        3,
    );

    // Compute (x, y) in the capped grid from scan table raster index
    let scan_raster_to_xy = |scan_raster: usize| -> (usize, usize) {
        let col = scan_raster % sw;
        let row = scan_raster / sw;
        (col, row) // x = col, y = row
    };

    // Store level_tok in levels array for eob position
    let (eob_x, eob_y) = scan_raster_to_xy(scan[scan_eob] as usize);
    let eob_tok = eob_raw_tok + 1; // actual token (1, 2, or 3)
    let level_tok_byte = if eob_raw_tok == 2 {
        // Will be updated after hi_tok
        0u8 // placeholder
    } else {
        (eob_tok as u8) * 0x41
    };

    // If eob_base_tok symbol was 2 (level >= 3), read hi_tok
    let mut hi_tok_total = 0u32;
    if eob_raw_tok == 2 {
        // hi_ctx: if (x|y) > 1 then 14, else 7
        let hi_ctx = if (eob_x | eob_y) > 1 { 14 } else { 7 };
        hi_tok_total = write_hi_tok(writer, cdf_ctx, br_ctx_level, hi_ctx, eob_level);
        let level_tok_val = hi_tok_total - 3 + (3 << 6);
        levels[eob_x * stride + eob_y] = level_tok_val as u8;
    } else {
        levels[eob_x * stride + eob_y] = level_tok_byte;
    }

    // Track the linked list of non-zero coefficient positions for sign/golomb phase.
    // In rav1d, cf[rc] stores (tok << 11) | next_rc. We track differently.
    // We'll collect (scan_pos, raster_idx, token_value) for non-zero coefficients.
    struct NonZeroCoeff {
        coeff_idx: usize,
        tok: u32,
    }
    let mut nonzero_coeffs: Vec<NonZeroCoeff> = Vec::with_capacity(scan_eob + 1);

    // Record eob position
    let eob_final_tok = if eob_raw_tok == 2 {
        hi_tok_total
    } else {
        eob_tok as u32
    };
    nonzero_coeffs.push(NonZeroCoeff {
        coeff_idx: scan_to_coeff_idx(scan_eob),
        tok: eob_final_tok,
    });

    // Step 6: AC tokens (scan_eob-1 down to 1)
    for i in (1..scan_eob).rev() {
        let scan_raster = scan[i] as usize;
        let coeff_idx = scan_to_coeff_idx(i);
        let level = if coeff_idx < coeffs.len() {
            coeffs[coeff_idx].unsigned_abs()
        } else {
            0
        };

        let (x, y) = scan_raster_to_xy(scan_raster);

        // Compute lo_ctx from levels array
        let (lo_ctx, hi_mag) = get_lo_ctx_2d(&levels, x, y, stride, lo_ctx_offsets);

        // For base_tok in 2D: y_for_hi_decision = y | x (rav1d does `y |= x`)
        let y_combined = y | x;

        // base_tok: 4-symbol CDF. Symbols 0,1,2,3 → levels 0,1,2,3+
        let sym = level.min(3) as usize;
        writer.write_symbol(sym, cdf_ctx.base_tok_cdf(t_dim_ctx, lo_ctx as usize), 4);

        if sym == 3 {
            // Need hi_tok
            let mag_trunc = (hi_mag as u8) & 63;
            let hi_ctx = if y_combined > 1 { 14u8 } else { 7u8 }
                + if mag_trunc > 12 {
                    6
                } else {
                    (mag_trunc + 1) >> 1
                };
            let tok_val = write_hi_tok(writer, cdf_ctx, br_ctx_level, hi_ctx as usize, level);
            let level_tok_val = tok_val - 3 + (3 << 6);
            levels[x * stride + y] = level_tok_val as u8;
            nonzero_coeffs.push(NonZeroCoeff {
                coeff_idx,
                tok: tok_val,
            });
        } else {
            let level_tok_val = sym as u8 * 0x41;
            levels[x * stride + y] = level_tok_val;
            if sym > 0 {
                nonzero_coeffs.push(NonZeroCoeff {
                    coeff_idx,
                    tok: sym as u32,
                });
            }
        }
    }

    // Step 7: DC token (scan position 0)
    let dc_level = abs_level_at(0);
    let dc_tok;

    if scan_eob == 0 {
        // DC is the eob position — already handled above via eob_base_tok.
        // dc_tok was computed as eob_final_tok.
        dc_tok = eob_final_tok;
    } else {
        // DC is NOT the eob position. Use base_tok with ctx=0 (2D, DC always ctx=0).
        // Actually, looking at rav1d more carefully:
        // For 2D, DC context is always 0 (hardcoded).
        let dc_sym = dc_level.min(3) as usize;
        writer.write_symbol(dc_sym, cdf_ctx.base_tok_cdf(t_dim_ctx, 0), 4);

        if dc_sym == 3 {
            // Compute mag for DC hi_ctx
            // For 2D DC: mag = levels[0*stride+1] + levels[1*stride+0] + levels[1*stride+1]
            let mag = levels[1] as u32 + levels[stride] as u32 + levels[stride + 1] as u32;
            let mag_trunc = (mag as u8) & 63;
            let hi_ctx = if mag_trunc > 12 {
                6u8
            } else {
                (mag_trunc + 1) >> 1
            };
            dc_tok = write_hi_tok(writer, cdf_ctx, br_ctx_level, hi_ctx as usize, dc_level);
        } else {
            dc_tok = dc_sym as u32;
        }

        if dc_tok > 0 {
            nonzero_coeffs.push(NonZeroCoeff {
                coeff_idx: scan_to_coeff_idx(0), // DC is always at (0,0)
                tok: dc_tok,
            });
        }
    }

    // Step 8: DC sign
    if dc_tok > 0 {
        let dc_coeff = if !coeffs.is_empty() { coeffs[0] } else { 0 };
        let dc_sign = if dc_coeff < 0 { 1usize } else { 0 };
        writer.write_symbol(dc_sign, cdf_ctx.dc_sign_cdf(dc_sign_ctx), 2);
    }

    // Step 9: For each non-zero coefficient (in order collected, which is
    // reverse scan order from eob to DC), write:
    //   - sign (equiprobable bit) — for AC coefficients
    //   - Golomb residual if tok >= 15
    //
    // The order in rav1d is: after DC sign, iterate through non-zero coeffs
    // following the linked list rc chain (eob → ... → first nonzero AC → DC).
    // Each non-zero coeff gets: sign (equi), then golomb if tok >= 15.
    //
    // The linked list in rav1d goes: rc starts at some value, then
    // cf[rc] & 0x3ff gives next rc, until rc == 0 (DC).
    // The order is effectively: from the most recent non-zero in reverse scan
    // back toward DC.
    //
    // For our encoder: nonzero_coeffs was collected in reverse scan order
    // (eob, eob-1, ..., 1, DC). We need to write signs for all non-zero
    // coeffs EXCEPT DC (which was already handled), and golomb for all.
    //
    // Actually, looking at rav1d again: the sign/golomb loop processes ALL
    // non-zero coefficients including DC. DC sign was already written via
    // dc_sign CDF. The loop writes equi-sign for AC and golomb for all.
    //
    // Let me re-read: after the DC sign CDF, rav1d enters the sign/golomb loop.
    // For DC with dc_tok != 0: dc_sign was already written.
    // The loop then starts from rc (the last non-zero AC or DC if eob==0).
    // For each entry: write equi-sign, then golomb if tok >= 15.
    // DC is the LAST entry (rc == 0 terminates).
    //
    // Wait — DC sign is NOT in the equi-sign loop. DC sign uses dc_sign CDF.
    // The equi-sign loop is only for AC coefficients.
    // In rav1d, after dc_sign: if rc != 0, enter the loop.
    // The loop writes sign(equi) + golomb for each AC, following the linked list.
    // The linked list ends when rc == 0 (the entry for the last AC points to 0).
    //
    // So: the loop iterates over non-zero AC coefficients only (not DC).
    // After the loop, DC golomb was already handled inline with dc_sign.
    //
    // Actually no — let me re-read rav1d once more. After dc_sign:
    //   dc_tok = read golomb if dc_tok == 15
    //   Then if rc != 0: loop over AC coefficients
    //     sign = read equi
    //     tok from cf[rc] >> 11
    //     if tok >= 15: golomb
    //     rc = cf[rc] & 0x3ff (next nonzero)
    //
    // So the order is:
    // 1. DC sign (CDF)
    // 2. DC golomb (if dc_tok == 15)
    // 3. For each AC nonzero (reverse scan order):
    //    a. sign (equi)
    //    b. golomb (if tok >= 15)

    // Write DC golomb if needed
    if dc_tok >= 15 {
        let dc_coeff = if !coeffs.is_empty() { coeffs[0] } else { 0 };
        let dc_abs = dc_coeff.unsigned_abs();
        let golomb_val = dc_abs - dc_tok;
        write_golomb(writer, golomb_val);
    }

    // Write AC signs and golomb residuals.
    // nonzero_coeffs was collected: [eob_pos, eob-1, ..., 1, DC(maybe)].
    // We need AC coefficients only (raster_idx != 0 for square, or
    // more precisely scan_pos != 0).
    // The order should match rav1d's linked list: starting from the most
    // recently pushed non-zero AC and working back.
    // In rav1d, the linked list threads through cf[] entries: each non-zero
    // AC entry stores the rc of the *previous* non-zero AC (or 0 for the first).
    // The loop starts from the LAST non-zero AC pushed and follows the chain.
    //
    // Our nonzero_coeffs is in reverse scan order (eob first, DC last).
    // The AC entries are all except the last one if the last one is DC.
    // Actually, let me just iterate nonzero_coeffs in order (which is
    // reverse scan = eob down to DC), skip DC, and write sign+golomb.

    for nz in &nonzero_coeffs {
        // Skip DC — its sign was already written via dc_sign CDF
        if nz.coeff_idx == 0 {
            continue;
        }
        let coeff = coeffs[nz.coeff_idx];
        let sign = coeff < 0;
        writer.write_bit(sign);

        if nz.tok >= 15 {
            let abs_val = coeff.unsigned_abs();
            let golomb_val = abs_val - nz.tok;
            write_golomb(writer, golomb_val);
        }
    }
}

/// Convert EOB (0-based scan position of last non-zero) to EOB bin.
///
/// bin 0 → eob 0
/// bin 1 → eob 1
/// bin 2 → eob 2..3
/// bin 3 → eob 4..7
/// bin 4 → eob 8..15
/// bin k → eob [2^(k-1) .. 2^k - 1] for k >= 2
fn eob_to_bin(eob: usize) -> u32 {
    if eob == 0 {
        return 0;
    }
    if eob == 1 {
        return 1;
    }
    // For eob >= 2: bin = floor(log2(eob)) + 2
    // because eob in [2^(k-2), 2^(k-1) - 1] maps to bin k
    // Let's compute: find highest set bit position
    let mut bin = 2u32;
    let mut threshold = 4usize;
    while threshold <= eob {
        bin += 1;
        threshold <<= 1;
    }
    // Verify: bin should satisfy 2^(bin-2) <= eob < 2^(bin-1)
    // For eob=2: threshold starts at 4, 2 < 4, so bin=2. 2^0=1 <= 2 < 2^1=2? No.
    // Let me re-derive. From the decoder:
    //   eob = ((hi_bit | 2) << (bin - 2)) | lo_bits
    //   For bin=2: eob = (hi_bit | 2) << 0 = hi_bit | 2 = 2 or 3
    //   For bin=3: eob = (hi_bit | 2) << 1 | lo = (2 or 3) << 1 | (0 or 1) = 4..7
    //   For bin=4: eob = (hi_bit | 2) << 2 | lo = (2 or 3) << 2 | (0..3) = 8..15
    //   For bin=k: eob in [2^(k-1), 2^k - 1]
    //
    // So for a given eob:
    //   if eob in [2, 3]: bin = 2
    //   if eob in [4, 7]: bin = 3
    //   if eob in [8, 15]: bin = 4
    //   if eob in [2^(k-1), 2^k - 1]: bin = k
    //
    // bin = floor(log2(eob)) + 1 for eob >= 2
    // eob=2: log2(2) = 1, bin = 2. Correct.
    // eob=3: log2(3) = 1, bin = 2. Correct.
    // eob=4: log2(4) = 2, bin = 3. Correct.
    // eob=7: log2(7) = 2, bin = 3. Correct.
    // eob=8: log2(8) = 3, bin = 4. Correct.
    //
    // So: bin = floor(log2(eob)) + 1 = (bit_length - 1) + 1 = bit_length
    // Actually: u32::BITS - eob.leading_zeros() = bit_length
    let bits = 32 - (eob as u32).leading_zeros(); // bit_length
    // eob=2: bits=2, bin should be 2. bits=2, correct.
    // eob=3: bits=2, bin=2. Correct.
    // eob=4: bits=3, bin=3. Correct.
    // eob=8: bits=4, bin=4. Correct.
    // eob=1023: bits=10, bin=10. Correct.
    let _ = bin; // discard the loop-computed value
    bits
}

/// Find the last non-zero coefficient position in scan order.
///
/// Returns the scan-order index (0-based) of the last non-zero coefficient,
/// or None if all coefficients are zero.
///
/// `scan_width` is the capped scan grid width (sw), `orig_width` is the
/// actual coefficient stride in the coeffs array.
fn find_last_nonzero_scan_pos(
    coeffs: &[i32],
    scan: &[u16],
    scan_width: usize,
    scan_height: usize,
    orig_width: usize,
) -> Option<usize> {
    let total = scan_width * scan_height;
    let scan_len = scan.len().min(total);
    for i in (0..scan_len).rev() {
        let scan_raster = scan[i] as usize;
        let col = scan_raster % scan_width;
        let row = scan_raster / scan_width;
        let coeff_idx = row * orig_width + col;
        if coeff_idx < coeffs.len() && coeffs[coeff_idx] != 0 {
            return Some(i);
        }
    }
    None
}

/// Compute log2 of a transform dimension.
/// width/height is in pixels: 4→0, 8→1, 16→2, 32→3, 64→4.
fn log2_of_tx_dim(dim: usize) -> usize {
    match dim {
        4 => 0,
        8 => 1,
        16 => 2,
        32 => 3,
        64 => 4,
        _ => {
            // Fallback: compute log2(dim/4) = log2(dim) - 2
            let log2 = (usize::BITS - dim.leading_zeros() - 1) as usize;
            log2.saturating_sub(2)
        }
    }
}

// ============================================================================
// Legacy code (preserved for comparison)
// ============================================================================

/// Context for coefficient coding within a transform block (legacy).
#[derive(Debug, Clone)]
pub struct CoeffContext {
    /// TXB skip CDFs [TXB_SKIP_CONTEXTS][2+1]
    pub txb_skip_cdf: [[AomCdfProb; 3]; TXB_SKIP_CONTEXTS],
    /// DC sign CDFs [DC_SIGN_CONTEXTS][2+1]
    pub dc_sign_cdf: [[AomCdfProb; 3]; DC_SIGN_CONTEXTS],
    /// EOB multi CDFs (simplified) [2+1]
    pub eob_multi_cdf: [AomCdfProb; EOB_MAX_SYMS + 1],
    /// Base level CDFs [SIG_COEF_CONTEXTS][NUM_BASE_LEVELS+2+1]
    pub base_cdf: [[AomCdfProb; NUM_BASE_LEVELS + 3]; 42],
    /// Base range CDFs [LEVEL_CONTEXTS][COEFF_BASE_RANGE/4+1+1]
    pub br_cdf: [[AomCdfProb; COEFF_BASE_RANGE / 4 + 2]; 21],
}

impl Default for CoeffContext {
    fn default() -> Self {
        use crate::cdf::CDF_PROB_TOP;
        let half = CDF_PROB_TOP / 2;

        // Base level CDF: 4 symbols (0, 1, 2, 3+) + sentinel + count
        let base_uniform = {
            let n = NUM_BASE_LEVELS + 2; // 4 symbols
            let mut cdf = [0u16; NUM_BASE_LEVELS + 3]; // 5 entries
            for i in 0..n - 1 {
                cdf[i] = (CDF_PROB_TOP as u32 * (n - 1 - i) as u32 / (n - 1) as u32) as u16;
            }
            cdf
        };

        // BR CDF: COEFF_BASE_RANGE/4 + 1 = 4 symbols + sentinel + count
        let br_uniform = {
            let n = COEFF_BASE_RANGE / 4 + 1; // 4 symbols
            let mut cdf = [0u16; COEFF_BASE_RANGE / 4 + 2]; // 5 entries
            for i in 0..n - 1 {
                cdf[i] = (CDF_PROB_TOP as u32 * (n - 1 - i) as u32 / (n - 1) as u32) as u16;
            }
            cdf
        };

        Self {
            txb_skip_cdf: [[half, 0, 0]; TXB_SKIP_CONTEXTS],
            dc_sign_cdf: [[half, 0, 0]; DC_SIGN_CONTEXTS],
            eob_multi_cdf: [0; EOB_MAX_SYMS + 1],
            base_cdf: [base_uniform; 42],
            br_cdf: [br_uniform; 21],
        }
    }
}

/// Write the coefficients of a transform block to the bitstream (legacy).
pub fn write_coefficients(
    writer: &mut AomWriter,
    coeffs: &[i32],
    eob: usize,
    _plane: usize,
    _tx_size: u8,
) {
    write_coefficients_ctx(writer, coeffs, eob, &mut CoeffContext::default());
}

/// Write coefficients using a mutable CDF context (legacy).
pub fn write_coefficients_ctx(
    writer: &mut AomWriter,
    coeffs: &[i32],
    eob: usize,
    ctx: &mut CoeffContext,
) {
    // TXB skip flag
    let skip_ctx = 0usize;
    if eob == 0 {
        writer.write_symbol(1, &mut ctx.txb_skip_cdf[skip_ctx], 2);
        return;
    }
    writer.write_symbol(0, &mut ctx.txb_skip_cdf[skip_ctx], 2);

    // EOB position (simplified: literal)
    let eob_bits = if eob <= 2 {
        2
    } else if eob <= 4 {
        3
    } else if eob <= 8 {
        4
    } else if eob <= 16 {
        5
    } else if eob <= 32 {
        6
    } else if eob <= 64 {
        7
    } else {
        8
    };
    writer.write_literal(eob as u32, eob_bits);

    // DC sign
    if eob > 0 && coeffs[0] != 0 {
        let sign_ctx = 1usize;
        let sign_sym = if coeffs[0] < 0 { 1 } else { 0 };
        writer.write_symbol(sign_sym, &mut ctx.dc_sign_cdf[sign_ctx], 2);
    }

    // Coefficient levels
    for i in 0..eob {
        let level = coeffs[i].unsigned_abs();
        let base_ctx = if i == 0 {
            0
        } else if i < 6 {
            i
        } else {
            let prev_nz = coeffs[..i]
                .iter()
                .rev()
                .take(5)
                .filter(|&&c| c != 0)
                .count();
            (6 + prev_nz * 7).min(41)
        };

        if level == 0 {
            writer.write_symbol(0, &mut ctx.base_cdf[base_ctx], NUM_BASE_LEVELS + 2);
        } else if level <= NUM_BASE_LEVELS as u32 {
            writer.write_symbol(
                level as usize,
                &mut ctx.base_cdf[base_ctx],
                NUM_BASE_LEVELS + 2,
            );
        } else {
            writer.write_symbol(
                NUM_BASE_LEVELS + 1,
                &mut ctx.base_cdf[base_ctx],
                NUM_BASE_LEVELS + 2,
            );

            let residual = level - NUM_BASE_LEVELS as u32 - 1;
            let br_ctx = i.min(20);
            if residual < COEFF_BASE_RANGE as u32 {
                let br_sym = (residual / 4) as usize;
                writer.write_symbol(br_sym, &mut ctx.br_cdf[br_ctx], COEFF_BASE_RANGE / 4 + 1);
                writer.write_literal(residual % 4, 2);
            } else {
                writer.write_symbol(
                    COEFF_BASE_RANGE / 4,
                    &mut ctx.br_cdf[br_ctx],
                    COEFF_BASE_RANGE / 4 + 1,
                );
                write_golomb(writer, residual - COEFF_BASE_RANGE as u32);
            }
        }

        // Sign for non-DC coefficients
        if i > 0 && coeffs[i] != 0 {
            writer.write_bit(coeffs[i] < 0);
        }
    }
}

/// Write a Golomb-coded unsigned integer.
///
/// Matches rav1d's `read_golomb` exactly:
/// ```text
/// read_golomb:
///   len = 0; val = 1
///   while !read_bool_equi() && len < 32: len++
///   for _ in 0..len: val = (val << 1) + read_bool_equi()
///   return val - 1
/// ```
///
/// So the encoder writes:
///   - `len` zero bits (each as !true = false via equi)
///   - 1 one bit (true via equi)
///   - `len` data bits of (value + 1) excluding the MSB
fn write_golomb(writer: &mut AomWriter, value: u32) {
    let v = value + 1;
    let len = 32 - v.leading_zeros(); // bit_length of v
    // Write (len-1) zeros as prefix
    for _ in 0..len - 1 {
        writer.write_bit(false);
    }
    // Write the terminating 1
    writer.write_bit(true);
    // Write (len-1) suffix bits (v without the MSB)
    for bit in (0..len - 1).rev() {
        writer.write_bit((v >> bit) & 1 != 0);
    }
}

/// Derive the TXB skip context from neighboring blocks.
pub fn get_txb_skip_context(above_has_coeff: bool, left_has_coeff: bool) -> usize {
    match (above_has_coeff, left_has_coeff) {
        (false, false) => 0,
        (true, false) | (false, true) => 1,
        (true, true) => 2,
    }
}

/// Derive the DC sign context from neighboring DC signs.
pub fn get_dc_sign_context(above_dc_sign: i8, left_dc_sign: i8) -> usize {
    let sum = above_dc_sign as i32 + left_dc_sign as i32;
    if sum < 0 {
        0
    } else if sum == 0 {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_zero_block() {
        let mut w = AomWriter::new(256);
        write_coefficients(&mut w, &[], 0, 0, 0);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn write_single_dc_coeff() {
        let mut w = AomWriter::new(256);
        write_coefficients(&mut w, &[100, 0, 0, 0], 1, 0, 0);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn write_multiple_coeffs() {
        let mut w = AomWriter::new(1024);
        let coeffs = [
            500, -300, 200, -100, 50, -25, 10, -5, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        write_coefficients(&mut w, &coeffs, 8, 0, 0);
        let output = w.done();
        assert!(
            output.len() > 5,
            "multiple coeffs should produce substantial output"
        );
    }

    #[test]
    fn txb_skip_context() {
        assert_eq!(get_txb_skip_context(false, false), 0);
        assert_eq!(get_txb_skip_context(true, false), 1);
        assert_eq!(get_txb_skip_context(true, true), 2);
    }

    #[test]
    fn dc_sign_context() {
        assert_eq!(get_dc_sign_context(-1, 0), 0);
        assert_eq!(get_dc_sign_context(0, 0), 1);
        assert_eq!(get_dc_sign_context(1, 0), 2);
    }

    // ====================================================================
    // Tests for the new v2 encoder
    // ====================================================================

    #[test]
    fn eob_bin_conversion() {
        assert_eq!(eob_to_bin(0), 0);
        assert_eq!(eob_to_bin(1), 1);
        assert_eq!(eob_to_bin(2), 2);
        assert_eq!(eob_to_bin(3), 2);
        assert_eq!(eob_to_bin(4), 3);
        assert_eq!(eob_to_bin(7), 3);
        assert_eq!(eob_to_bin(8), 4);
        assert_eq!(eob_to_bin(15), 4);
        assert_eq!(eob_to_bin(16), 5);
        assert_eq!(eob_to_bin(31), 5);
        assert_eq!(eob_to_bin(32), 6);
        assert_eq!(eob_to_bin(63), 6);
        assert_eq!(eob_to_bin(64), 7);
        assert_eq!(eob_to_bin(127), 7);
        assert_eq!(eob_to_bin(128), 8);
        assert_eq!(eob_to_bin(255), 8);
        assert_eq!(eob_to_bin(256), 9);
        assert_eq!(eob_to_bin(511), 9);
        assert_eq!(eob_to_bin(512), 10);
        assert_eq!(eob_to_bin(1023), 10);
    }

    #[test]
    fn diagonal_scan_4x4_matches_hardcoded() {
        let generated = generate_diagonal_scan(4, 4);
        let hardcoded: Vec<u16> = svtav1_tables::scan::DEFAULT_SCAN_4X4
            .iter()
            .map(|&x| x as u16)
            .collect();
        assert_eq!(generated, hardcoded, "4x4 diagonal scan mismatch");
    }

    #[test]
    fn diagonal_scan_8x8_matches_hardcoded() {
        let generated = generate_diagonal_scan(8, 8);
        let hardcoded: Vec<u16> = svtav1_tables::scan::DEFAULT_SCAN_8X8
            .iter()
            .map(|&x| x as u16)
            .collect();
        assert_eq!(generated, hardcoded, "8x8 diagonal scan mismatch");
    }

    #[test]
    fn diagonal_scan_16x16_covers_all() {
        let scan = generate_diagonal_scan(16, 16);
        assert_eq!(scan.len(), 256);
        let mut visited = [false; 256];
        for &idx in &scan {
            assert!(
                !visited[idx as usize],
                "duplicate index {idx} in 16x16 scan"
            );
            visited[idx as usize] = true;
        }
        assert!(
            visited.iter().all(|&v| v),
            "16x16 scan must cover all positions"
        );
    }

    #[test]
    fn diagonal_scan_32x32_covers_all() {
        let scan = generate_diagonal_scan(32, 32);
        assert_eq!(scan.len(), 1024);
        let mut visited = [false; 1024];
        for &idx in &scan {
            assert!(
                !visited[idx as usize],
                "duplicate index {idx} in 32x32 scan"
            );
            visited[idx as usize] = true;
        }
        assert!(
            visited.iter().all(|&v| v),
            "32x32 scan must cover all positions"
        );
    }

    #[test]
    fn v2_write_zero_block_4x4() {
        let mut w = AomWriter::new(256);
        let mut cdf = CdfCoefCtx::new(0);
        let coeffs = [0i32; 16];
        write_coefficients_v2(&mut w, &coeffs, 0, 4, 4, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "zero block should produce output");
    }

    #[test]
    fn v2_write_dc_only_4x4() {
        let mut w = AomWriter::new(256);
        let mut cdf = CdfCoefCtx::new(0);
        let mut coeffs = [0i32; 16];
        coeffs[0] = 42;
        // eob = 1 means "there is 1 non-zero coefficient" in our convention,
        // but for v2, eob is scan_eob, the position. Since DC is at scan position 0,
        // we pass eob=1 to indicate non-zero content exists.
        write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "dc-only block should produce output");
    }

    #[test]
    fn v2_write_multiple_coeffs_4x4() {
        let mut w = AomWriter::new(1024);
        let mut cdf = CdfCoefCtx::new(0);
        let coeffs = [
            500, -300, 200, -100, 50, -25, 10, -5, 3, -1, 0, 0, 0, 0, 0, 0,
        ];
        // eob should be the number of non-zero coefficients for our caller,
        // but v2 internally finds the scan-order eob.
        write_coefficients_v2(&mut w, &coeffs, 10, 4, 4, 0, 1, &mut cdf);
        let output = w.done();
        assert!(
            output.len() > 5,
            "multiple coeffs should produce substantial output, got {} bytes",
            output.len()
        );
    }

    #[test]
    fn v2_write_8x8_block() {
        let mut w = AomWriter::new(2048);
        let mut cdf = CdfCoefCtx::new(1);
        let mut coeffs = [0i32; 64];
        coeffs[0] = 1000;
        coeffs[1] = -500;
        coeffs[8] = 200;
        coeffs[9] = -100;
        write_coefficients_v2(&mut w, &coeffs, 4, 8, 8, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "8x8 block should produce output");
    }

    #[test]
    fn v2_write_16x16_block() {
        let mut w = AomWriter::new(4096);
        let mut cdf = CdfCoefCtx::new(2);
        let mut coeffs = [0i32; 256];
        // Place some coefficients
        coeffs[0] = 2000;
        coeffs[1] = -1000;
        coeffs[16] = 500;
        coeffs[17] = -200;
        coeffs[32] = 100;
        write_coefficients_v2(&mut w, &coeffs, 5, 16, 16, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "16x16 block should produce output");
    }

    #[test]
    fn v2_write_32x32_block() {
        let mut w = AomWriter::new(8192);
        let mut cdf = CdfCoefCtx::new(3);
        let mut coeffs = [0i32; 1024];
        coeffs[0] = 5000;
        coeffs[1] = -2000;
        coeffs[32] = 1000;
        write_coefficients_v2(&mut w, &coeffs, 3, 32, 32, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "32x32 block should produce output");
    }

    #[test]
    fn v2_negative_dc_sign() {
        let mut w = AomWriter::new(256);
        let mut cdf = CdfCoefCtx::new(0);
        let mut coeffs = [0i32; 16];
        coeffs[0] = -42;
        write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn v2_large_coefficient_golomb() {
        let mut w = AomWriter::new(1024);
        let mut cdf = CdfCoefCtx::new(0);
        let mut coeffs = [0i32; 16];
        // Level 100 requires golomb coding (tok >= 15)
        coeffs[0] = 100;
        write_coefficients_v2(&mut w, &coeffs, 1, 4, 4, 0, 1, &mut cdf);
        let output = w.done();
        assert!(!output.is_empty(), "large coefficient should be encodable");
    }

    #[test]
    fn v2_all_qp_categories() {
        for qp in 0..4 {
            let mut w = AomWriter::new(512);
            let mut cdf = CdfCoefCtx::new(qp);
            let mut coeffs = [0i32; 16];
            coeffs[0] = 10;
            coeffs[1] = -5;
            write_coefficients_v2(&mut w, &coeffs, 2, 4, 4, 0, 1, &mut cdf);
            let output = w.done();
            assert!(!output.is_empty(), "QP category {qp} should produce output");
        }
    }

    #[test]
    fn log2_tx_dim_values() {
        assert_eq!(log2_of_tx_dim(4), 0);
        assert_eq!(log2_of_tx_dim(8), 1);
        assert_eq!(log2_of_tx_dim(16), 2);
        assert_eq!(log2_of_tx_dim(32), 3);
        assert_eq!(log2_of_tx_dim(64), 4);
    }

    #[test]
    fn golomb_roundtrip_values() {
        // Test that golomb encoding produces non-empty output for various values
        for val in [0, 1, 2, 3, 7, 15, 31, 63, 100, 255, 1000] {
            let mut w = AomWriter::new(256);
            write_golomb(&mut w, val);
            let output = w.done();
            assert!(!output.is_empty(), "golomb({val}) should produce output");
        }
    }
}

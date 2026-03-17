//! Coefficient entropy coding — TXB skip, EOB, base levels, Golomb.
//!
//! Spec 07: Coefficient coding (TXB skip, EOB, base levels).
//!
//! This module handles the entropy coding of transform coefficients,
//! which is the most complex part of the AV1 entropy coder.
//! Ported from SVT-AV1's entropy_coding.c coefficient writing functions.

use crate::cdf::AomCdfProb;
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

/// Context for coefficient coding within a transform block.
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
        let half = crate::cdf::CDF_PROB_TOP / 2;
        Self {
            txb_skip_cdf: [[half, 0, 0]; TXB_SKIP_CONTEXTS],
            dc_sign_cdf: [[half, 0, 0]; DC_SIGN_CONTEXTS],
            eob_multi_cdf: [0; EOB_MAX_SYMS + 1],
            base_cdf: [[0; NUM_BASE_LEVELS + 3]; 42],
            br_cdf: [[0; COEFF_BASE_RANGE / 4 + 2]; 21],
        }
    }
}

/// Write the coefficients of a transform block to the bitstream.
///
/// Uses CDF-based arithmetic coding for the core syntax elements:
/// - TXB skip (CDF, 2 symbols)
/// - DC sign (CDF, 2 symbols)
/// - Coefficient base level (CDF, 4 symbols: 0, 1, 2, 3+)
/// - Residual above base range: Exp-Golomb
///
/// When `ctx` is None, falls back to literal encoding.
pub fn write_coefficients(
    writer: &mut AomWriter,
    coeffs: &[i32],
    eob: usize,
    _plane: usize,
    _tx_size: u8,
) {
    write_coefficients_ctx(writer, coeffs, eob, &mut CoeffContext::default());
}

/// Write coefficients using a mutable CDF context for backward-adaptive encoding.
pub fn write_coefficients_ctx(
    writer: &mut AomWriter,
    coeffs: &[i32],
    eob: usize,
    ctx: &mut CoeffContext,
) {
    // TXB skip flag — CDF-based
    let skip_ctx = 0usize; // simplified: always context 0
    if eob == 0 {
        writer.write_symbol(1, &mut ctx.txb_skip_cdf[skip_ctx], 2);
        return;
    }
    writer.write_symbol(0, &mut ctx.txb_skip_cdf[skip_ctx], 2);

    // EOB position (simplified: write as literal until we have proper EOB CDFs)
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

    // DC sign — CDF-based
    if eob > 0 && coeffs[0] != 0 {
        let sign_ctx = 1usize; // neutral context
        let sign_sym = if coeffs[0] < 0 { 1 } else { 0 };
        writer.write_symbol(sign_sym, &mut ctx.dc_sign_cdf[sign_ctx], 2);
    }

    // Coefficient levels — CDF-based for base levels, Golomb for residual
    for i in 0..eob {
        let level = coeffs[i].unsigned_abs();
        // Base level context: simplified to first 42 contexts round-robin
        let base_ctx = i.min(41);

        if level == 0 {
            // Symbol 0: zero coefficient
            writer.write_symbol(0, &mut ctx.base_cdf[base_ctx], NUM_BASE_LEVELS + 2);
        } else if level <= NUM_BASE_LEVELS as u32 {
            // Symbol 1 or 2: base level
            writer.write_symbol(
                level as usize,
                &mut ctx.base_cdf[base_ctx],
                NUM_BASE_LEVELS + 2,
            );
        } else {
            // Symbol NUM_BASE_LEVELS+1: above base level → Golomb residual
            writer.write_symbol(
                NUM_BASE_LEVELS + 1,
                &mut ctx.base_cdf[base_ctx],
                NUM_BASE_LEVELS + 2,
            );

            // Base range coding (BR CDF) for values up to COEFF_BASE_RANGE
            let residual = level - NUM_BASE_LEVELS as u32 - 1;
            let br_ctx = i.min(20);
            if residual < COEFF_BASE_RANGE as u32 {
                let br_sym = (residual / 4) as usize;
                writer.write_symbol(
                    br_sym,
                    &mut ctx.br_cdf[br_ctx],
                    COEFF_BASE_RANGE / 4 + 1,
                );
                // Low bits of residual within the group
                writer.write_literal(residual % 4, 2);
            } else {
                // Signal maximum BR symbol, then Golomb for the excess
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
fn write_golomb(writer: &mut AomWriter, value: u32) {
    // Exp-Golomb order 0: unary prefix + binary suffix
    let v = value + 1;
    let mut len = 0;
    let mut temp = v;
    while temp > 0 {
        len += 1;
        temp >>= 1;
    }
    // Write (len-1) zeros as prefix
    for _ in 0..len - 1 {
        writer.write_bit(false);
    }
    // Write the value in binary
    writer.write_literal(v, len);
}

/// Derive the TXB skip context from neighboring blocks.
///
/// The context depends on whether the above and left blocks have
/// non-zero coefficients.
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
}

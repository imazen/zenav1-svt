//! AV1 entropy coding context models (FRAME_CONTEXT).
//!
//! Spec 07: FrameContext with all CDF tables.
//!
//! Contains all CDF tables needed for a single tile/frame.
//! Ported from `cabac_context_model.c/h`.

use crate::cdf::{AomCdfProb, CDF_PROB_TOP};

// =============================================================================
// Context sizes from the AV1 spec
// =============================================================================

pub const PARTITION_CONTEXTS: usize = 20;
pub const INTRA_MODES: usize = 13;
pub const UV_INTRA_MODES: usize = 14;
pub const KF_MODE_CONTEXTS: usize = 5;
pub const COMP_INTER_CONTEXTS: usize = 5;
pub const INTER_MODE_CONTEXTS: usize = 8;
pub const NEWMV_MODE_CONTEXTS: usize = 6;
pub const GLOBALMV_MODE_CONTEXTS: usize = 2;
pub const REFMV_MODE_CONTEXTS: usize = 6;
pub const DRL_MODE_CONTEXTS: usize = 3;
pub const INTRA_INTER_CONTEXTS: usize = 4;
pub const SKIP_CONTEXTS: usize = 3;
pub const SKIP_MODE_CONTEXTS: usize = 3;
pub const TX_SIZE_CONTEXTS: usize = 3;
pub const DELTA_Q_PROBS: usize = 3;
pub const REF_CONTEXTS: usize = 3;
pub const INTERP_FILTER_CONTEXTS: usize = 16;
pub const SWITCHABLE_FILTERS: usize = 3;
pub const BLOCK_SIZE_GROUPS: usize = 4;
pub const TX_TYPES: usize = 16;
pub const EXT_TX_SIZES: usize = 4;
pub const EOB_COEF_CONTEXTS: usize = 9;
pub const SIG_COEF_CONTEXTS: usize = 42;
pub const LEVEL_CONTEXTS: usize = 21;
pub const NUM_BASE_LEVELS: usize = 2;
pub const BR_CDF_SIZE: usize = 4;
pub const COEFF_BASE_RANGE: usize = 12;
pub const DC_SIGN_CONTEXTS: usize = 3;
pub const PLANE_TYPES: usize = 2;
pub const TXB_SKIP_CONTEXTS: usize = 13;
pub const EOB_MAX_SYMS: usize = 13;

// =============================================================================
// Frame context — all CDF tables for a frame/tile
// =============================================================================

/// AV1 frame context containing all CDF probability tables.
///
/// This is the Rust equivalent of FRAME_CONTEXT in the C code.
/// Each field is a multi-dimensional CDF array used for entropy coding
/// different syntax elements.
#[derive(Clone)]
pub struct FrameContext {
    // --- Block-level syntax ---
    /// Partition type CDFs [PARTITION_CONTEXTS][4+1]
    pub partition_cdf: [[AomCdfProb; 5]; PARTITION_CONTEXTS],

    /// Skip flag CDFs [SKIP_CONTEXTS][2+1]
    pub skip_cdf: [[AomCdfProb; 3]; SKIP_CONTEXTS],

    /// Skip mode CDFs [SKIP_MODE_CONTEXTS][2+1]
    pub skip_mode_cdf: [[AomCdfProb; 3]; SKIP_MODE_CONTEXTS],

    /// Intra/inter flag CDFs [INTRA_INTER_CONTEXTS][2+1]
    pub intra_inter_cdf: [[AomCdfProb; 3]; INTRA_INTER_CONTEXTS],

    // --- Intra prediction ---
    /// Y-mode CDFs for keyframes [KF_MODE_CONTEXTS][KF_MODE_CONTEXTS][INTRA_MODES+1]
    pub kf_y_mode_cdf: [[[AomCdfProb; INTRA_MODES + 1]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS],

    /// Y-mode CDFs for inter frames [BLOCK_SIZE_GROUPS][INTRA_MODES+1]
    pub y_mode_cdf: [[AomCdfProb; INTRA_MODES + 1]; BLOCK_SIZE_GROUPS],

    /// UV-mode CDFs [2][INTRA_MODES][UV_INTRA_MODES+1] (CFL and non-CFL)
    pub uv_mode_cdf: [[[AomCdfProb; UV_INTRA_MODES + 1]; INTRA_MODES]; 2],

    // --- Inter prediction ---
    /// Inter compound mode CDFs [INTER_MODE_CONTEXTS][4+1]
    pub inter_compound_mode_cdf: [[AomCdfProb; 5]; INTER_MODE_CONTEXTS],

    /// New MV flag CDFs [NEWMV_MODE_CONTEXTS][2+1]
    pub newmv_cdf: [[AomCdfProb; 3]; NEWMV_MODE_CONTEXTS],

    /// Global MV flag CDFs [GLOBALMV_MODE_CONTEXTS][2+1]
    pub globalmv_cdf: [[AomCdfProb; 3]; GLOBALMV_MODE_CONTEXTS],

    /// Ref MV flag CDFs [REFMV_MODE_CONTEXTS][2+1]
    pub refmv_cdf: [[AomCdfProb; 3]; REFMV_MODE_CONTEXTS],

    /// DRL index CDFs [DRL_MODE_CONTEXTS][2+1]
    pub drl_cdf: [[AomCdfProb; 3]; DRL_MODE_CONTEXTS],

    // --- Transform ---
    /// TX size CDFs [TX_SIZE_CONTEXTS][3+1]
    pub tx_size_cdf: [[AomCdfProb; 4]; TX_SIZE_CONTEXTS],

    /// TXB skip CDFs [TXB_SKIP_CONTEXTS][2+1]
    pub txb_skip_cdf: [[AomCdfProb; 3]; TXB_SKIP_CONTEXTS],

    /// DC sign CDFs [PLANE_TYPES][DC_SIGN_CONTEXTS][2+1]
    pub dc_sign_cdf: [[[AomCdfProb; 3]; DC_SIGN_CONTEXTS]; PLANE_TYPES],

    /// End-of-block CDFs [PLANE_TYPES][2][EOB_MAX_SYMS+1]
    pub eob_flag_cdf: [[[AomCdfProb; EOB_MAX_SYMS + 1]; 2]; PLANE_TYPES],

    // --- Interpolation filter ---
    /// Interp filter CDFs [INTERP_FILTER_CONTEXTS][SWITCHABLE_FILTERS+1]
    pub interp_filter_cdf: [[AomCdfProb; SWITCHABLE_FILTERS + 1]; INTERP_FILTER_CONTEXTS],

    // --- Reference frames ---
    /// Single ref CDFs [REF_CONTEXTS][6][2+1]
    pub single_ref_cdf: [[[AomCdfProb; 3]; 6]; REF_CONTEXTS],

    /// Compound ref CDFs [REF_CONTEXTS][3][2+1]
    pub comp_ref_cdf: [[[AomCdfProb; 3]; 3]; REF_CONTEXTS],

    /// Comp inter CDFs [COMP_INTER_CONTEXTS][2+1]
    pub comp_inter_cdf: [[AomCdfProb; 3]; COMP_INTER_CONTEXTS],

    // --- Delta Q ---
    /// Delta Q CDFs [DELTA_Q_PROBS+1+1]
    pub delta_q_cdf: [AomCdfProb; DELTA_Q_PROBS + 2],
}

impl FrameContext {
    /// Initialize a frame context with default (uniform) CDFs.
    pub fn new_default() -> Self {
        Self {
            partition_cdf: [[
                CDF_PROB_TOP / 4 * 3,
                CDF_PROB_TOP / 4 * 2,
                CDF_PROB_TOP / 4,
                0,
                0,
            ]; PARTITION_CONTEXTS],
            skip_cdf: [[CDF_PROB_TOP / 2, 0, 0]; SKIP_CONTEXTS],
            skip_mode_cdf: [[CDF_PROB_TOP / 2, 0, 0]; SKIP_MODE_CONTEXTS],
            intra_inter_cdf: [[CDF_PROB_TOP / 2, 0, 0]; INTRA_INTER_CONTEXTS],
            kf_y_mode_cdf: [[[0; INTRA_MODES + 1]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS],
            y_mode_cdf: [[0; INTRA_MODES + 1]; BLOCK_SIZE_GROUPS],
            uv_mode_cdf: [[[0; UV_INTRA_MODES + 1]; INTRA_MODES]; 2],
            inter_compound_mode_cdf: [[
                CDF_PROB_TOP / 4 * 3,
                CDF_PROB_TOP / 4 * 2,
                CDF_PROB_TOP / 4,
                0,
                0,
            ]; INTER_MODE_CONTEXTS],
            newmv_cdf: [[CDF_PROB_TOP / 2, 0, 0]; NEWMV_MODE_CONTEXTS],
            globalmv_cdf: [[CDF_PROB_TOP / 2, 0, 0]; GLOBALMV_MODE_CONTEXTS],
            refmv_cdf: [[CDF_PROB_TOP / 2, 0, 0]; REFMV_MODE_CONTEXTS],
            drl_cdf: [[CDF_PROB_TOP / 2, 0, 0]; DRL_MODE_CONTEXTS],
            tx_size_cdf: [[CDF_PROB_TOP / 3 * 2, CDF_PROB_TOP / 3, 0, 0]; TX_SIZE_CONTEXTS],
            txb_skip_cdf: [[CDF_PROB_TOP / 2, 0, 0]; TXB_SKIP_CONTEXTS],
            dc_sign_cdf: [[[CDF_PROB_TOP / 2, 0, 0]; DC_SIGN_CONTEXTS]; PLANE_TYPES],
            eob_flag_cdf: [[[0; EOB_MAX_SYMS + 1]; 2]; PLANE_TYPES],
            interp_filter_cdf: [[CDF_PROB_TOP / 3 * 2, CDF_PROB_TOP / 3, 0, 0];
                INTERP_FILTER_CONTEXTS],
            single_ref_cdf: [[[CDF_PROB_TOP / 2, 0, 0]; 6]; REF_CONTEXTS],
            comp_ref_cdf: [[[CDF_PROB_TOP / 2, 0, 0]; 3]; REF_CONTEXTS],
            comp_inter_cdf: [[CDF_PROB_TOP / 2, 0, 0]; COMP_INTER_CONTEXTS],
            delta_q_cdf: [
                CDF_PROB_TOP / 4 * 3,
                CDF_PROB_TOP / 4 * 2,
                CDF_PROB_TOP / 4,
                0,
                0,
            ],
        }
    }
}

// =============================================================================
// Syntax element encoding functions
// =============================================================================

use crate::writer::AomWriter;

/// Encode a partition type.
pub fn write_partition(w: &mut AomWriter, ctx: usize, partition: u8, _nsymbs: usize) {
    debug_assert!(ctx < PARTITION_CONTEXTS);
    // For now just encode as literal — real impl uses CDF from FrameContext
    w.write_literal(partition as u32, 2);
}

/// Encode a skip flag.
pub fn write_skip(w: &mut AomWriter, skip: bool) {
    w.write_bit(skip);
}

/// Encode an intra/inter flag.
pub fn write_intra_inter(w: &mut AomWriter, is_inter: bool) {
    w.write_bit(is_inter);
}

/// Encode a prediction mode (literal for now, CDF-based in full impl).
pub fn write_intra_mode(w: &mut AomWriter, mode: u8) {
    w.write_literal(mode as u32, 4);
}

/// Encode a motion vector component.
pub fn write_mv_component(w: &mut AomWriter, comp: i16) {
    let sign = comp < 0;
    let mag = comp.unsigned_abs();
    w.write_bit(sign);
    // Simple magnitude coding — real impl uses MV class + fractional bits
    w.write_literal(mag as u32, 14);
}

/// Encode a transform type.
pub fn write_tx_type(w: &mut AomWriter, tx_type: u8) {
    w.write_literal(tx_type as u32, 4);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_context_default() {
        let fc = FrameContext::new_default();
        // Skip CDF should be initialized
        assert!(fc.skip_cdf[0][0] > 0);
        // Partition CDF should have 4 symbols
        assert!(fc.partition_cdf[0][0] > fc.partition_cdf[0][1]);
    }

    #[test]
    fn frame_context_clone() {
        let fc1 = FrameContext::new_default();
        let fc2 = fc1.clone();
        assert_eq!(fc1.skip_cdf[0][0], fc2.skip_cdf[0][0]);
    }

    #[test]
    fn write_skip_flag() {
        let mut w = AomWriter::new(256);
        write_skip(&mut w, true);
        write_skip(&mut w, false);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn write_mv_both_signs() {
        let mut w = AomWriter::new(256);
        write_mv_component(&mut w, 42);
        write_mv_component(&mut w, -42);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn write_intra_mode_range() {
        let mut w = AomWriter::new(256);
        for mode in 0..13 {
            write_intra_mode(&mut w, mode);
        }
        let output = w.done();
        assert!(!output.is_empty());
    }
}

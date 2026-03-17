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
    /// Partition type CDFs [PARTITION_CONTEXTS][EXT_PARTITION_TYPES+1]
    /// Small blocks (ctx 0-3): 4 types. Medium (4-15): 10 types. Large (16-19): 8 types.
    pub partition_cdf: [[AomCdfProb; 11]; PARTITION_CONTEXTS],

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

// =============================================================================
// AV1 spec default CDF tables (Section 9.3)
// Ported from SVT-AV1 cabac_context_model.c
// =============================================================================

/// Default partition CDFs from spec. Zero-padded to 11 for uniform array size.
/// Contexts 0-3: 4 types, 4-15: 10 types, 16-19: 8 types.
#[rustfmt::skip]
static DEFAULT_PARTITION_CDF: [[AomCdfProb; 11]; PARTITION_CONTEXTS] = [
    [19132, 25510, 30392, 0, 0, 0, 0, 0, 0, 0, 0],
    [13928, 19855, 28540, 0, 0, 0, 0, 0, 0, 0, 0],
    [12522, 23679, 28629, 0, 0, 0, 0, 0, 0, 0, 0],
    [ 9896, 18783, 25853, 0, 0, 0, 0, 0, 0, 0, 0],
    [15597, 20929, 24571, 26706, 27664, 28821, 29601, 30571, 31902, 0, 0],
    [ 7925, 11043, 16785, 22470, 23971, 25043, 26651, 28701, 29834, 0, 0],
    [ 5414, 13269, 15111, 20488, 22360, 24500, 25537, 26336, 32117, 0, 0],
    [ 2662,  6362,  8614, 20860, 23053, 24778, 26436, 27829, 31171, 0, 0],
    [18462, 20920, 23124, 27647, 28227, 29049, 29519, 30178, 31544, 0, 0],
    [ 7689,  9060, 12056, 24992, 25660, 26182, 26951, 28041, 29052, 0, 0],
    [ 6015,  9009, 10062, 24544, 25409, 26545, 27071, 27526, 32047, 0, 0],
    [ 1394,  2208,  2796, 28614, 29061, 29466, 29840, 30185, 31899, 0, 0],
    [20137, 21547, 23078, 29566, 29837, 30261, 30524, 30892, 31724, 0, 0],
    [ 6732,  7490,  9497, 27944, 28250, 28515, 28969, 29630, 30104, 0, 0],
    [ 5945,  7663,  8348, 28683, 29117, 29749, 30064, 30298, 32238, 0, 0],
    [  870,  1212,  1487, 31198, 31394, 31574, 31743, 31881, 32332, 0, 0],
    [27899, 28219, 28529, 32484, 32539, 32619, 32639, 0, 0, 0, 0],
    [ 6607,  6990,  8268, 32060, 32219, 32338, 32371, 0, 0, 0, 0],
    [ 5429,  6676,  7122, 32027, 32227, 32531, 32582, 0, 0, 0, 0],
    [  711,   966,  1172, 32448, 32538, 32617, 32664, 0, 0, 0, 0],
];

/// Default skip CDFs.
static DEFAULT_SKIP_CDF: [[AomCdfProb; 3]; SKIP_CONTEXTS] = [
    [31671, 0, 0],
    [16515, 0, 0],
    [4576, 0, 0],
];

/// Default intra/inter CDFs.
static DEFAULT_INTRA_INTER_CDF: [[AomCdfProb; 3]; INTRA_INTER_CONTEXTS] = [
    [806, 0, 0],
    [16662, 0, 0],
    [20186, 0, 0],
    [26538, 0, 0],
];

/// Default Y-mode CDFs for inter frames.
#[rustfmt::skip]
static DEFAULT_Y_MODE_CDF: [[AomCdfProb; INTRA_MODES + 1]; BLOCK_SIZE_GROUPS] = [
    [22801, 23489, 24293, 24756, 25601, 26123, 26606, 27418, 27945, 29228, 29685, 30349, 0, 0],
    [18673, 19845, 22631, 23318, 23950, 24649, 25527, 27364, 28152, 29701, 29984, 30852, 0, 0],
    [19770, 20979, 23396, 23939, 24241, 24654, 25136, 27073, 27830, 29360, 29730, 30659, 0, 0],
    [20155, 21301, 22838, 23178, 23261, 23533, 23703, 24804, 25352, 26575, 27016, 28049, 0, 0],
];

/// Default keyframe Y-mode CDFs [above_mode][left_mode][13 modes + sentinel].
#[rustfmt::skip]
static DEFAULT_KF_Y_MODE_CDF: [[[AomCdfProb; INTRA_MODES + 1]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS] = [
    [[15588, 17027, 19338, 20218, 20682, 21110, 21825, 23244, 24189, 28165, 29093, 30466, 0, 0],
     [12016, 18066, 19516, 20303, 20719, 21444, 21888, 23032, 24434, 28658, 30172, 31409, 0, 0],
     [10052, 10771, 22296, 22788, 23055, 23239, 24133, 25620, 26160, 29336, 29929, 31567, 0, 0],
     [14091, 15406, 16442, 18808, 19136, 19546, 19998, 22096, 24746, 29585, 30958, 32462, 0, 0],
     [12122, 13265, 15603, 16501, 18609, 20033, 22391, 25583, 26437, 30261, 31073, 32475, 0, 0]],
    [[10023, 19585, 20848, 21440, 21832, 22760, 23089, 24023, 25381, 29014, 30482, 31436, 0, 0],
     [ 5983, 24099, 24560, 24886, 25066, 25795, 25913, 26423, 27610, 29905, 31276, 31794, 0, 0],
     [ 7444, 12781, 20177, 20728, 21077, 21607, 22170, 23405, 24469, 27915, 29090, 30492, 0, 0],
     [ 8537, 14689, 15432, 17087, 17408, 18172, 18408, 19825, 24649, 29153, 31096, 32210, 0, 0],
     [ 7543, 14231, 15496, 16195, 17905, 20717, 21984, 24516, 26001, 29675, 30981, 31994, 0, 0]],
    [[12613, 13591, 21383, 22004, 22312, 22577, 23401, 25055, 25729, 29538, 30305, 32077, 0, 0],
     [ 9687, 13470, 18506, 19230, 19604, 20147, 20695, 22062, 23219, 27743, 29211, 30907, 0, 0],
     [ 6183,  6505, 26024, 26252, 26366, 26434, 27082, 28354, 28555, 30467, 30794, 32086, 0, 0],
     [10718, 11734, 14954, 17224, 17565, 17924, 18561, 21523, 23878, 28975, 30287, 32252, 0, 0],
     [ 9194,  9858, 16501, 17263, 18424, 19171, 21563, 25961, 26561, 30072, 30737, 32463, 0, 0]],
    [[12602, 14399, 15488, 18381, 18778, 19315, 19724, 21419, 25060, 29696, 30917, 32409, 0, 0],
     [ 8203, 13821, 14524, 17105, 17439, 18131, 18404, 19468, 25225, 29485, 31158, 32342, 0, 0],
     [ 8451,  9731, 15004, 17643, 18012, 18425, 19070, 21538, 24605, 29118, 30078, 32018, 0, 0],
     [ 7714,  9048,  9516, 16667, 16817, 16994, 17153, 18767, 26743, 30389, 31536, 32528, 0, 0],
     [ 8843, 10280, 11496, 15317, 16652, 17943, 19108, 22718, 25769, 29953, 30983, 32485, 0, 0]],
    [[12578, 13671, 15979, 16834, 19075, 20913, 22989, 25449, 26219, 30214, 31150, 32477, 0, 0],
     [ 9563, 13626, 15080, 15892, 17756, 20863, 22207, 24236, 25380, 29653, 31143, 32277, 0, 0],
     [ 8356,  8901, 17616, 18256, 19350, 20106, 22598, 25947, 26466, 29900, 30523, 32261, 0, 0],
     [10835, 11815, 13124, 16042, 17018, 18039, 18947, 22753, 24615, 29489, 30883, 32482, 0, 0],
     [ 7618,  8288,  9859, 10509, 15386, 18657, 22903, 28776, 29180, 31355, 31802, 32593, 0, 0]],
];

/// Default single reference CDFs.
#[rustfmt::skip]
static DEFAULT_SINGLE_REF_CDF: [[[AomCdfProb; 3]; 6]; REF_CONTEXTS] = [
    [[4897, 0, 0], [1555, 0, 0], [4236, 0, 0], [8650, 0, 0], [904, 0, 0], [1444, 0, 0]],
    [[16973, 0, 0], [16751, 0, 0], [19647, 0, 0], [24773, 0, 0], [11014, 0, 0], [15087, 0, 0]],
    [[29744, 0, 0], [30279, 0, 0], [31194, 0, 0], [31895, 0, 0], [26875, 0, 0], [30304, 0, 0]],
];

/// Default comp ref CDFs.
#[rustfmt::skip]
static DEFAULT_COMP_REF_CDF: [[[AomCdfProb; 3]; 3]; REF_CONTEXTS] = [
    [[4946, 0, 0], [9468, 0, 0], [1503, 0, 0]],
    [[19891, 0, 0], [22441, 0, 0], [15160, 0, 0]],
    [[30731, 0, 0], [31059, 0, 0], [27544, 0, 0]],
];

/// Default comp inter CDFs.
static DEFAULT_COMP_INTER_CDF: [[AomCdfProb; 3]; COMP_INTER_CONTEXTS] = [
    [26828, 0, 0],
    [24035, 0, 0],
    [12031, 0, 0],
    [10640, 0, 0],
    [2901, 0, 0],
];

impl FrameContext {
    /// Initialize a frame context with AV1 spec default CDFs (Section 9.3).
    ///
    /// These are the statistically-derived default probability tables from the
    /// AV1 specification, providing much better compression than uniform CDFs.
    pub fn new_default() -> Self {
        Self {
            partition_cdf: DEFAULT_PARTITION_CDF,
            skip_cdf: DEFAULT_SKIP_CDF,
            skip_mode_cdf: [[CDF_PROB_TOP / 2, 0, 0]; SKIP_MODE_CONTEXTS],
            intra_inter_cdf: DEFAULT_INTRA_INTER_CDF,
            kf_y_mode_cdf: DEFAULT_KF_Y_MODE_CDF,
            y_mode_cdf: DEFAULT_Y_MODE_CDF,
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
            single_ref_cdf: DEFAULT_SINGLE_REF_CDF,
            comp_ref_cdf: DEFAULT_COMP_REF_CDF,
            comp_inter_cdf: DEFAULT_COMP_INTER_CDF,
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
        // Skip CDF should be initialized with spec defaults
        assert!(fc.skip_cdf[0][0] > 0);
        assert_eq!(fc.skip_cdf[0][0], 31671); // Spec default
        // Partition CDF from spec — cumulative, monotonically increasing
        assert_eq!(fc.partition_cdf[0][0], 19132);
        assert!(fc.partition_cdf[0][0] < fc.partition_cdf[0][1]);
        // KF Y-mode CDF should have proper values
        assert_eq!(fc.kf_y_mode_cdf[0][0][0], 15588);
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

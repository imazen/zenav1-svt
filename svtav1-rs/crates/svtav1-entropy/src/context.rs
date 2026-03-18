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
/// Number of directional intra modes (V_PRED through D67_PRED).
pub const DIRECTIONAL_MODES: usize = 8;
/// Number of angle delta symbols (delta -3 to +3 = 7 values).
pub const ANGLE_DELTA_SYMS: usize = 7;

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

    /// Angle delta CDFs [DIRECTIONAL_MODES][ANGLE_DELTA_SYMS+1]
    /// For directional modes (V_PRED..D67_PRED), encodes angle offset -3..+3.
    pub angle_delta_cdf: [[AomCdfProb; ANGLE_DELTA_SYMS + 1]; DIRECTIONAL_MODES],

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
// Values stored in ICDF format: CDF_PROB_TOP - cumulative_probability
// =============================================================================

/// Default partition CDFs from spec. Zero-padded to 11 for uniform array size.
/// Contexts 0-3: 4 types, 4-15: 10 types, 16-19: 8 types.
#[rustfmt::skip]
static DEFAULT_PARTITION_CDF: [[AomCdfProb; 11]; PARTITION_CONTEXTS] = [
    [13636, 7258, 2376, 0, 0, 0, 0, 0, 0, 0, 0],
    [18840, 12913, 4228, 0, 0, 0, 0, 0, 0, 0, 0],
    [20246, 9089, 4139, 0, 0, 0, 0, 0, 0, 0, 0],
    [ 22872, 13985, 6915, 0, 0, 0, 0, 0, 0, 0, 0],
    [17171, 11839, 8197, 6062, 5104, 3947, 3167, 2197, 866, 0, 0],
    [ 24843, 21725, 15983, 10298, 8797, 7725, 6117, 4067, 2934, 0, 0],
    [ 27354, 19499, 17657, 12280, 10408, 8268, 7231, 6432, 651, 0, 0],
    [ 30106,  26406,  24154, 11908, 9715, 7990, 6332, 4939, 1597, 0, 0],
    [14306, 11848, 9644, 5121, 4541, 3719, 3249, 2590, 1224, 0, 0],
    [ 25079,  23708, 20712, 7776, 7108, 6586, 5817, 4727, 3716, 0, 0],
    [ 26753,  23759, 22706, 8224, 7359, 6223, 5697, 5242, 721, 0, 0],
    [ 31374,  30560,  29972, 4154, 3707, 3302, 2928, 2583, 869, 0, 0],
    [12631, 11221, 9690, 3202, 2931, 2507, 2244, 1876, 1044, 0, 0],
    [ 26036,  25278,  23271, 4824, 4518, 4253, 3799, 3138, 2664, 0, 0],
    [ 26823,  25105,  24420, 4085, 3651, 3019, 2704, 2470, 530, 0, 0],
    [  31898,  31556,  31281, 1570, 1374, 1194, 1025, 887, 436, 0, 0],
    [4869, 4549, 4239, 284, 229, 149, 129, 0, 0, 0, 0],
    [ 26161,  25778,  24500, 708, 549, 430, 397, 0, 0, 0, 0],
    [ 27339,  26092,  25646, 741, 541, 237, 186, 0, 0, 0, 0],
    [  32057,   31802,  31596, 320, 230, 151, 104, 0, 0, 0, 0],
];

/// Default skip CDFs.
static DEFAULT_SKIP_CDF: [[AomCdfProb; 3]; SKIP_CONTEXTS] = [
    [1097, 0, 0],
    [16253, 0, 0],
    [28192, 0, 0],
];

/// Default intra/inter CDFs.
static DEFAULT_INTRA_INTER_CDF: [[AomCdfProb; 3]; INTRA_INTER_CONTEXTS] = [
    [31962, 0, 0],
    [16106, 0, 0],
    [12582, 0, 0],
    [6230, 0, 0],
];

/// Default Y-mode CDFs for inter frames.
#[rustfmt::skip]
static DEFAULT_Y_MODE_CDF: [[AomCdfProb; INTRA_MODES + 1]; BLOCK_SIZE_GROUPS] = [
    [9967, 9279, 8475, 8012, 7167, 6645, 6162, 5350, 4823, 3540, 3083, 2419, 0, 0],
    [14095, 12923, 10137, 9450, 8818, 8119, 7241, 5404, 4616, 3067, 2784, 1916, 0, 0],
    [12998, 11789, 9372, 8829, 8527, 8114, 7632, 5695, 4938, 3408, 3038, 2109, 0, 0],
    [12613, 11467, 9930, 9590, 9507, 9235, 9065, 7964, 7416, 6193, 5752, 4719, 0, 0],
];

/// Default keyframe Y-mode CDFs [above_mode][left_mode][13 modes + sentinel].
#[rustfmt::skip]
static DEFAULT_KF_Y_MODE_CDF: [[[AomCdfProb; INTRA_MODES + 1]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS] = [
    [[17180, 15741, 13430, 12550, 12086, 11658, 10943, 9524, 8579, 4603, 3675, 2302, 0, 0],
     [20752, 14702, 13252, 12465, 12049, 11324, 10880, 9736, 8334, 4110, 2596, 1359, 0, 0],
     [22716, 21997, 10472, 9980, 9713, 9529, 8635, 7148, 6608, 3432, 2839, 1201, 0, 0],
     [18677, 17362, 16326, 13960, 13632, 13222, 12770, 10672, 8022, 3183, 1810, 306, 0, 0],
     [20646, 19503, 17165, 16267, 14159, 12735, 10377, 7185, 6331, 2507, 1695, 293, 0, 0]],
    [[22745, 13183, 11920, 11328, 10936, 10008, 9679, 8745, 7387, 3754, 2286, 1332, 0, 0],
     [ 26785, 8669, 8208, 7882, 7702, 6973, 6855, 6345, 5158, 2863, 1492, 974, 0, 0],
     [ 25324, 19987, 12591, 12040, 11691, 11161, 10598, 9363, 8299, 4853, 3678, 2276, 0, 0],
     [ 24231, 18079, 17336, 15681, 15360, 14596, 14360, 12943, 8119, 3615, 1672, 558, 0, 0],
     [ 25225, 18537, 17272, 16573, 14863, 12051, 10784, 8252, 6767, 3093, 1787, 774, 0, 0]],
    [[20155, 19177, 11385, 10764, 10456, 10191, 9367, 7713, 7039, 3230, 2463, 691, 0, 0],
     [ 23081, 19298, 14262, 13538, 13164, 12621, 12073, 10706, 9549, 5025, 3557, 1861, 0, 0],
     [ 26585,  26263, 6744, 6516, 6402, 6334, 5686, 4414, 4213, 2301, 1974, 682, 0, 0],
     [22050, 21034, 17814, 15544, 15203, 14844, 14207, 11245, 8890, 3793, 2481, 516, 0, 0],
     [ 23574,  22910, 16267, 15505, 14344, 13597, 11205, 6807, 6207, 2696, 2031, 305, 0, 0]],
    [[20166, 18369, 17280, 14387, 13990, 13453, 13044, 11349, 7708, 3072, 1851, 359, 0, 0],
     [ 24565, 18947, 18244, 15663, 15329, 14637, 14364, 13300, 7543, 3283, 1610, 426, 0, 0],
     [ 24317,  23037, 17764, 15125, 14756, 14343, 13698, 11230, 8163, 3650, 2690, 750, 0, 0],
     [ 25054,  23720,  23252, 16101, 15951, 15774, 15615, 14001, 6025, 2379, 1232, 240, 0, 0],
     [ 23925, 22488, 21272, 17451, 16116, 14825, 13660, 10050, 6999, 2815, 1785, 283, 0, 0]],
    [[20190, 19097, 16789, 15934, 13693, 11855, 9779, 7319, 6549, 2554, 1618, 291, 0, 0],
     [ 23205, 19142, 17688, 16876, 15012, 11905, 10561, 8532, 7388, 3115, 1625, 491, 0, 0],
     [ 24412,  23867, 15152, 14512, 13418, 12662, 10170, 6821, 6302, 2868, 2245, 507, 0, 0],
     [21933, 20953, 19644, 16726, 15750, 14729, 13821, 10015, 8153, 3279, 1885, 286, 0, 0],
     [ 25150,  24480,  22909, 22259, 17382, 14111, 9865, 3992, 3588, 1413, 966, 175, 0, 0]],
];

/// Default single reference CDFs.
#[rustfmt::skip]
static DEFAULT_SINGLE_REF_CDF: [[[AomCdfProb; 3]; 6]; REF_CONTEXTS] = [
    [[27871, 0, 0], [31213, 0, 0], [28532, 0, 0], [24118, 0, 0], [31864, 0, 0], [31324, 0, 0]],
    [[15795, 0, 0], [16017, 0, 0], [13121, 0, 0], [7995, 0, 0], [21754, 0, 0], [17681, 0, 0]],
    [[3024, 0, 0], [2489, 0, 0], [1574, 0, 0], [873, 0, 0], [5893, 0, 0], [2464, 0, 0]],
];

/// Default comp ref CDFs.
#[rustfmt::skip]
static DEFAULT_COMP_REF_CDF: [[[AomCdfProb; 3]; 3]; REF_CONTEXTS] = [
    [[27822, 0, 0], [23300, 0, 0], [31265, 0, 0]],
    [[12877, 0, 0], [10327, 0, 0], [17608, 0, 0]],
    [[2037, 0, 0], [1709, 0, 0], [5224, 0, 0]],
];

/// Default comp inter CDFs.
static DEFAULT_COMP_INTER_CDF: [[AomCdfProb; 3]; COMP_INTER_CONTEXTS] = [
    [5940, 0, 0],
    [8733, 0, 0],
    [20737, 0, 0],
    [22128, 0, 0],
    [29867, 0, 0],
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
            // Uniform ICDF for 7 angle delta symbols: 6/6*T, 5/6*T, 4/6*T, 3/6*T, 2/6*T, 1/6*T, 0, 0
            angle_delta_cdf: {
                let n = ANGLE_DELTA_SYMS; // 7
                let mut cdf = [0u16; ANGLE_DELTA_SYMS + 1];
                let mut i = 0;
                while i < n - 1 {
                    cdf[i] = (CDF_PROB_TOP as u32 * (n - 1 - i) as u32 / (n - 1) as u32) as u16;
                    i += 1;
                }
                // cdf[n-1] = 0 (last ICDF), cdf[n] = 0 (count)
                [cdf; DIRECTIONAL_MODES]
            },
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

/// Derive the skip context from above and left neighbors.
/// AV1 spec Section 5.11.11: ctx = above_skip + left_skip.
pub fn get_skip_context(above_skip: bool, left_skip: bool) -> usize {
    above_skip as usize + left_skip as usize
}

/// Derive the intra/inter context from above and left neighbors.
/// AV1 spec Section 5.11.7: context depends on whether neighbors are intra.
pub fn get_intra_inter_context(above_intra: bool, left_intra: bool) -> usize {
    match (above_intra, left_intra) {
        (true, true) => 0,   // Both intra → likely intra
        (true, false) => 1,  // Mixed
        (false, true) => 2,  // Mixed
        (false, false) => 3, // Both inter → likely inter
    }
}

/// Map intra prediction mode to a simplified context for kf_y_mode CDF.
/// AV1 spec Section 5.11.4: maps 13 modes to 5 context groups.
pub fn intra_mode_context(mode: u8) -> usize {
    match mode {
        0 => 0,      // DC_PRED
        1 | 2 => 1,  // V_PRED, H_PRED
        3..=6 => 2,  // Smooth modes
        7 => 3,      // Paeth
        _ => 4,      // Directional modes
    }
}

/// Map block size to block_size_group for y_mode CDF selection.
/// AV1 spec: 4 groups based on block dimensions.
pub fn block_size_group(width: usize, height: usize) -> usize {
    let n = width.min(height);
    match n {
        0..=4 => 0,
        5..=8 => 1,
        9..=16 => 2,
        _ => 3,
    }
}

/// Derive the partition context from block size.
///
/// AV1 spec Section 5.9.3: partition context depends on block size.
/// 8x8 → ctx 0-3 (4 types), 16-32 → ctx 4-15 (10 types), 64+ → ctx 16-19 (8 types).
/// Sub-context (0-3) from above/left neighbor block sizes.
pub fn get_partition_context(width: usize, above_same_size: bool, left_same_size: bool) -> (usize, usize) {
    let bsl = match width {
        w if w <= 8 => 0,
        w if w <= 16 => 1,
        w if w <= 32 => 2,
        _ => 3,
    };

    let sub = match (above_same_size, left_same_size) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };

    let ctx = bsl * 4 + sub;

    // Number of symbols: determined by context range, matching the CDF sentinel positions.
    // Contexts 0-3: 4 symbols (sentinel at cdf[3])
    // Contexts 4-15: 10 symbols (sentinel at cdf[9])
    // Contexts 16-19: 8 symbols (sentinel at cdf[7])
    let nsymbs = match ctx {
        0..=3 => 4,
        4..=15 => 10,
        _ => 8,
    };

    (ctx.min(PARTITION_CONTEXTS - 1), nsymbs)
}

/// Encode a partition type using CDF from the frame context.
///
/// `ctx` is the partition context (0-19), `partition` is the partition
/// type index, `nsymbs` is the number of valid symbols for this context.
///
/// Uses write_symbol (with CDF update) to match the decoder's behavior.
/// The AV1 decoder always updates partition CDFs after each symbol.
/// Each context has its own CDF array, so varying symbol counts across
/// contexts don't interfere — updates apply to the specific context's CDF.
pub fn write_partition(w: &mut AomWriter, fc: &mut FrameContext, ctx: usize, partition: u8, nsymbs: usize) {
    debug_assert!(ctx < PARTITION_CONTEXTS);
    let symbs = nsymbs.min(10);
    let sym = (partition as usize).min(symbs - 1);
    w.write_symbol(sym, &mut fc.partition_cdf[ctx], symbs);
}

/// Encode a skip flag using CDF.
pub fn write_skip(w: &mut AomWriter, fc: &mut FrameContext, ctx: usize, skip: bool) {
    let sym = if skip { 1 } else { 0 };
    w.write_symbol(sym, &mut fc.skip_cdf[ctx.min(SKIP_CONTEXTS - 1)], 2);
}

/// Encode an intra/inter flag using CDF.
pub fn write_intra_inter(w: &mut AomWriter, fc: &mut FrameContext, ctx: usize, is_inter: bool) {
    let sym = if is_inter { 1 } else { 0 };
    w.write_symbol(
        sym,
        &mut fc.intra_inter_cdf[ctx.min(INTRA_INTER_CONTEXTS - 1)],
        2,
    );
}

/// Encode an intra prediction mode using CDF.
///
/// For keyframes, uses kf_y_mode_cdf indexed by above and left mode context.
/// For inter frames, uses y_mode_cdf indexed by block size group.
pub fn write_intra_mode_kf(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    above_mode: usize,
    left_mode: usize,
    mode: u8,
) {
    let above = above_mode.min(KF_MODE_CONTEXTS - 1);
    let left = left_mode.min(KF_MODE_CONTEXTS - 1);
    w.write_symbol(
        mode as usize,
        &mut fc.kf_y_mode_cdf[above][left],
        INTRA_MODES,
    );
}

/// Returns true if the given intra mode is directional (V_PRED..D67_PRED).
pub fn is_directional_mode(mode: u8) -> bool {
    (1..=8).contains(&mode)
}

/// Encode the angle delta for a directional intra mode.
///
/// AV1 spec Section 5.11.42: angle_delta is signaled for directional modes
/// (V_PRED through D67_PRED) when the block is at least 8x8.
/// The delta ranges from -3 to +3 (7 symbols, symbol 3 = delta 0).
pub fn write_angle_delta(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    mode: u8,
    angle_delta: i8,
) {
    debug_assert!(is_directional_mode(mode), "angle_delta only for directional modes");
    let mode_idx = (mode as usize - 1).min(DIRECTIONAL_MODES - 1);
    // Map delta -3..+3 to symbol 0..6
    let sym = (angle_delta + 3).clamp(0, 6) as usize;
    w.write_symbol(sym, &mut fc.angle_delta_cdf[mode_idx], ANGLE_DELTA_SYMS);
}

/// Encode an intra prediction mode for inter frames.
pub fn write_intra_mode_inter(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    bsize_group: usize,
    mode: u8,
) {
    let group = bsize_group.min(BLOCK_SIZE_GROUPS - 1);
    w.write_symbol(mode as usize, &mut fc.y_mode_cdf[group], INTRA_MODES);
}

/// Legacy literal-based intra mode encoding (backward compat).
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
        assert_eq!(fc.skip_cdf[0][0], 1097); // Spec default
        // Partition CDF from spec — cumulative, monotonically increasing
        assert_eq!(fc.partition_cdf[0][0], 13636);
        assert!(fc.partition_cdf[0][0] > fc.partition_cdf[0][1]);
        // KF Y-mode CDF should have proper values
        assert_eq!(fc.kf_y_mode_cdf[0][0][0], 17180);
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
        let mut fc = FrameContext::new_default();
        write_skip(&mut w, &mut fc, 0, true);
        write_skip(&mut w, &mut fc, 1, false);
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

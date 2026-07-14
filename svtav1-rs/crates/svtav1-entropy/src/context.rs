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
/// C MAX_TX_CATS (definitions.h): rows of tx_size_cdf, one per
/// largest-TX square-size category (8, 16, 32, 64).
pub const MAX_TX_CATS: usize = 4;
/// C MAX_TX_DEPTH: a block codes at most this many split levels, so the
/// tx_depth symbol has at most MAX_TX_DEPTH+1 values.
pub const MAX_TX_DEPTH: usize = 2;
pub const DELTA_Q_PROBS: usize = 3;
pub const REF_CONTEXTS: usize = 3;
pub const INTERP_FILTER_CONTEXTS: usize = 16;
pub const SWITCHABLE_FILTERS: usize = 3;
pub const BLOCK_SIZE_GROUPS: usize = 4;
/// Number of entries in the C `BlockSize` enum / spec BLOCK_SIZES_ALL
/// (definitions.h:923-946): the 16 square/2:1 sizes + the 6 4:1 rects.
pub const BLOCK_SIZES_ALL: usize = 22;
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

    /// use_filter_intra flag CDFs [BLOCK_SIZES_ALL][2+1] — C
    /// FRAME_CONTEXT.filter_intra_cdfs, coded per eligible intra block
    /// when the SH signals enable_filter_intra (entropy_coding.c:5099-5104;
    /// decoder: read_filter_intra_mode_info).
    pub filter_intra_cdfs: [[AomCdfProb; 3]; BLOCK_SIZES_ALL],

    /// FRAME_CONTEXT.filter_intra_mode_cdf — the CDF5 filter_intra_mode
    /// symbol coded right after use_filter_intra == 1
    /// (entropy_coding.c:5105-5110; decoder read_filter_intra_mode_info).
    /// Default AOM_CDF5(8949, 12776, 17211, 29558):
    /// ICDF [23819, 19992, 15557, 3210] — the trace fingerprint of C's
    /// FILTER_DC leaves at gradient-64 q40 p6.
    pub filter_intra_mode_cdf: [AomCdfProb; 6],

    /// Per-RU `wiener_restore` flag CDF — C FRAME_CONTEXT.wiener_restore_cdf
    /// (default AOM_CDF2(11570), cabac_context_model.c:629), coded by
    /// loop_restoration_write_sb_coeffs when the frame restoration type is
    /// RESTORE_WIENER (entropy_coding.c:4186; decoder
    /// loop_restoration_read_sb_coeffs, libaom decodeframe.c:1702).
    pub wiener_restore_cdf: [AomCdfProb; 3],

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
    /// TX size (depth) CDFs [MAX_TX_CATS][TX_SIZE_CONTEXTS][MAX_TX_DEPTH+1+1]
    /// — C FRAME_CONTEXT.tx_size_cdf, coded by write_selected_tx_size
    /// (entropy_coding.c:4678) with row = bsize_to_tx_size_cat(bsize) and
    /// column = get_tx_size_context(xd).
    pub tx_size_cdf: [[[AomCdfProb; 4]; TX_SIZE_CONTEXTS]; MAX_TX_CATS],

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

// =============================================================================
// Syntax rate estimation (C md_rate_estimation.{h,c})
// =============================================================================

/// C `av1_prob_cost[128]` (md_rate_estimation.h:131):
/// `round(-log2(i/256) * (1 << AV1_PROB_COST_SHIFT))` for i = 128..255,
/// i.e. symbol costs in 1/512-bit units (AV1_PROB_COST_SHIFT = 9).
#[rustfmt::skip]
pub const AV1_PROB_COST: [u16; 128] = [
    512, 506, 501, 495, 489, 484, 478, 473, 467, 462, 456, 451, 446, 441, 435, 430,
    425, 420, 415, 410, 405, 400, 395, 390, 385, 380, 375, 371, 366, 361, 356, 352,
    347, 343, 338, 333, 329, 324, 320, 316, 311, 307, 302, 298, 294, 289, 285, 281,
    277, 273, 268, 264, 260, 256, 252, 248, 244, 240, 236, 232, 228, 224, 220, 216,
    212, 209, 205, 201, 197, 194, 190, 186, 182, 179, 175, 171, 168, 164, 161, 157,
    153, 150, 146, 143, 139, 136, 132, 129, 125, 122, 119, 115, 112, 109, 105, 102,
     99,  95,  92,  89,  86,  82,  79,  76,  73,  70,  66,  63,  60,  57,  54,  51,
     48,  45,  42,  38,  35,  32,  29,  26,  23,  20,  18,  15,  12,   9,   6,   3,
];

/// C `av1_cost_symbol(p15)` (md_rate_estimation.c:33-43): the cost in
/// 1/512-bit units of coding a symbol whose probability is `p15 / 32768`.
/// `shift` normalizes p15 into [2^14, 2^15); `prob = get_prob(p15 <<
/// shift, 32768)` maps it to [128, 255] (get_prob rounds `num * 256 /
/// den` and clips to [1, 255]).
pub fn av1_cost_symbol(p15: u32) -> u32 {
    let p15 = p15.clamp(1, CDF_PROB_TOP as u32 - 1);
    let shift = 14 - (31 - p15.leading_zeros());
    let prob = (((p15 << shift) * 256 + (1 << 14)) >> 15).clamp(1, 255);
    debug_assert!(prob >= 128);
    AV1_PROB_COST[(prob - 128) as usize] as u32 + shift * 512
}

/// Cost (1/512-bit units) of coding partition symbol `sym` at a square
/// node of `width`, with neighbor sub-context `sub_ctx` (0..3), from the
/// DEFAULT partition CDFs — the same table the frame context starts from.
///
/// This is C `svt_aom_get_syntax_rate_from_cdf` (md_rate_estimation.c:48)
/// applied to `partition_cdf[bsl*4 + sub_ctx]`: symbol probability
/// `p15 = CDF(sym) - CDF(sym-1)` recovered from the stored inverse CDFs
/// (`icdf[sym-1] - icdf[sym]`, `icdf[-1] = 32768`), floored at
/// EC_MIN_PROB = 4 like the C table builder, then `av1_cost_symbol`.
pub fn partition_symbol_cost(width: usize, sub_ctx: usize, sym: usize) -> u32 {
    debug_assert!((8..=128).contains(&width) && width.is_power_of_two());
    debug_assert!(sub_ctx < 4);
    let bsl = width.ilog2() as usize - 3;
    let row = &DEFAULT_PARTITION_CDF[(bsl * 4 + sub_ctx).min(PARTITION_CONTEXTS - 1)];
    let prev = if sym == 0 {
        CDF_PROB_TOP as u32
    } else {
        row[sym - 1] as u32
    };
    let p15 = prev.saturating_sub(row[sym] as u32).max(4); // EC_MIN_PROB
    av1_cost_symbol(p15)
}

/// Default skip CDFs.
static DEFAULT_SKIP_CDF: [[AomCdfProb; 3]; SKIP_CONTEXTS] =
    [[1097, 0, 0], [16253, 0, 0], [28192, 0, 0]];

/// Default intra/inter CDFs.
static DEFAULT_INTRA_INTER_CDF: [[AomCdfProb; 3]; INTRA_INTER_CONTEXTS] =
    [[31962, 0, 0], [16106, 0, 0], [12582, 0, 0], [6230, 0, 0]];

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
            // Real AV1 defaults (generated from the C reference, layout
            // [cfl_allowed][y_mode][UV_INTRA_MODES+1]): the decoder inits
            // uv_mode_cdf with these (libaom entropymode.c
            // default_uv_mode_cdf), so an all-zero table desyncs the stream
            // on the first uv_mode symbol.
            uv_mode_cdf: crate::default_cdfs::UV_MODE_CDF,
            // Real AV1 defaults extracted from the C reference — the decoder
            // initializes angle_delta_cdf with these, so a uniform table
            // desyncs the stream on the first directional mode.
            angle_delta_cdf: crate::default_cdfs::ANGLE_DELTA_CDF,
            // Real AV1 defaults (generated from the C reference and
            // drift-tested vs FcTable::FilterIntra in tests/c_parity.rs) —
            // the decoder initializes filter_intra_cdfs with these; wrong
            // values desync the stream on the first use_filter_intra flag.
            filter_intra_cdfs: crate::default_cdfs::FILTER_INTRA_CDF,
            filter_intra_mode_cdf: crate::default_cdfs::FILTER_INTRA_MODE_CDF,
            // AOM_CDF2(11570) in ICDF storage (32768 - 11570 = 21198) —
            // matches the C trace fingerprint `BOOL f=21198` on the
            // wiener_restore flag.
            wiener_restore_cdf: [CDF_PROB_TOP - 11570, 0, 0],
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
            // Real AV1 defaults (generated from the C reference and
            // drift-tested vs FcTable::TxSize) — the decoder initializes
            // tx_size_cdf with these; wrong values desync the stream on
            // the first tx_depth symbol.
            tx_size_cdf: crate::default_cdfs::TX_SIZE_CDF,
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

/// Map intra prediction mode to the kf_y_mode CDF context.
///
/// This is the actual `intra_mode_context[]` table shared by libaom and
/// rav1d: {0, 1, 2, 3, 4, 4, 4, 4, 3, 0, 1, 2, 0}. The decoder derives
/// kf_y_cdf[above_ctx][left_ctx] with it — any deviation desyncs the
/// stream as soon as non-DC modes appear.
pub fn intra_mode_context(mode: u8) -> usize {
    // AV1 modes: 0=DC, 1=V, 2=H, 3=D45, 4=D135, 5=D113, 6=D157, 7=D203, 8=D67,
    //            9=SMOOTH, 10=SMOOTH_V, 11=SMOOTH_H, 12=PAETH
    const INTRA_MODE_CONTEXT: [usize; 13] = [0, 1, 2, 3, 4, 4, 4, 4, 3, 0, 1, 2, 0];
    INTRA_MODE_CONTEXT.get(mode as usize).copied().unwrap_or(0)
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

/// Derive the partition context from block size and neighbor information.
///
/// AV1 spec Section 5.11.3: partition context = bsl * 4 + sub_ctx.
/// `has_above` and `has_left` indicate whether above/left neighbors exist.
/// For the simplified case where all SBs use the same partition, neighbors
/// are never smaller, so sub_ctx encodes neighbor availability.
pub fn get_partition_context(width: usize, has_above: bool, has_left: bool) -> (usize, usize) {
    let bsl = match width {
        w if w <= 8 => 0,
        w if w <= 16 => 1,
        w if w <= 32 => 2,
        _ => 3,
    };

    let sub = match (has_above, has_left) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };

    let ctx = bsl * 4 + sub;

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
pub fn write_partition(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    ctx: usize,
    partition: u8,
    nsymbs: usize,
) {
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

/// Length of the sub-TX chain from the block's largest TX down to TX_4X4.
///
/// C walks `eb_sub_tx_size_map` starting at `blocksize_to_txsize[bsize]`
/// (bsize_to_tx_size_cat / bsize_to_max_depth, inter_prediction.h:322-344).
/// For every bsize <= 64x64 the largest TX has exactly the block's own
/// dimensions, and the sub map halves the larger dimension (both when
/// square) until 4x4 — so the chain length is `log2(max(w, h)) - 2`.
/// (Chain spot-checks vs the C tables live in the tests below.)
fn tx_chain_len(width: usize, height: usize) -> usize {
    debug_assert!(width <= 64 && height <= 64, "128 blocks cap at TX_64X64");
    let max_dim = width.max(height);
    debug_assert!(max_dim >= 4 && max_dim.is_power_of_two());
    max_dim.ilog2() as usize - 2
}

/// C `bsize_to_tx_size_cat(bsize)` (inter_prediction.h:322): the
/// tx_size_cdf ROW for a block. Only valid for bsize > BLOCK_4X4.
pub fn tx_size_cat(width: usize, height: usize) -> usize {
    let cat = tx_chain_len(width, height) - 1;
    debug_assert!(cat < MAX_TX_CATS);
    cat
}

/// C `bsize_to_max_depth(bsize)` (inter_prediction.h:335): the maximum
/// codable tx_depth; the symbol alphabet is `max_depth + 1` values.
pub fn tx_max_depth(width: usize, height: usize) -> usize {
    tx_chain_len(width, height).min(MAX_TX_DEPTH)
}

/// Encode the per-block tx_depth symbol (TX_MODE_SELECT intra blocks).
///
/// C `write_selected_tx_size` (entropy_coding.c:4678-4696):
/// `aom_write_symbol(w, depth, ec_ctx->tx_size_cdf[tx_size_cat][tx_size_ctx],
/// max_depths + 1)`. Only called when `block_signals_txsize(bsize)`
/// (`bsize > BLOCK_4X4`, entropy_coding.c:4466) — the caller gates that.
pub fn write_tx_depth(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    width: usize,
    height: usize,
    ctx: usize,
    depth: usize,
) {
    let cat = tx_size_cat(width, height);
    let nsyms = tx_max_depth(width, height) + 1;
    debug_assert!(depth < nsyms);
    debug_assert!(ctx < TX_SIZE_CONTEXTS);
    w.write_symbol(depth, &mut fc.tx_size_cdf[cat][ctx], nsyms);
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

/// Encode a chroma (UV) intra prediction mode.
///
/// Mirrors libaom's decoder exactly (av1/decoder/decodemv.c:140
/// `read_intra_mode_uv`):
///
/// ```c
/// aom_read_symbol(r, ec_ctx->uv_mode_cdf[cfl_allowed][y_mode],
///                 UV_INTRA_MODES - !cfl_allowed, ...)
/// ```
///
/// i.e. the CDF is selected by `[cfl_allowed][y_mode]` and the alphabet is
/// 14 symbols when CFL is allowed, else 13. CFL is allowed for luma blocks
/// with width <= 32 && height <= 32 (libaom av1/common/blockd.h
/// `is_cfl_allowed`, non-lossless path) — the caller derives that from the
/// LUMA block dimensions. `UV_DC_PRED` is symbol 0 in both alphabets.
pub fn write_uv_mode(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    cfl_allowed: bool,
    y_mode: u8,
    uv_mode: u8,
) {
    let nsymbs = if cfl_allowed {
        UV_INTRA_MODES
    } else {
        UV_INTRA_MODES - 1
    };
    debug_assert!((uv_mode as usize) < nsymbs);
    let y = (y_mode as usize).min(INTRA_MODES - 1);
    w.write_symbol(
        uv_mode as usize,
        &mut fc.uv_mode_cdf[usize::from(cfl_allowed)][y],
        nsymbs,
    );
}

/// C `BlockSize` / spec BLOCK_SIZES_ALL index for a (width, height) pair —
/// the enum order of definitions.h:923-946 (squares/2:1 rects first, the
/// six 4:1 rects appended). This is the row index for per-bsize CDF tables
/// such as `filter_intra_cdfs[BlockSize]`.
pub fn block_size_index(width: usize, height: usize) -> usize {
    match (width, height) {
        (4, 4) => 0,     // BLOCK_4X4
        (4, 8) => 1,     // BLOCK_4X8
        (8, 4) => 2,     // BLOCK_8X4
        (8, 8) => 3,     // BLOCK_8X8
        (8, 16) => 4,    // BLOCK_8X16
        (16, 8) => 5,    // BLOCK_16X8
        (16, 16) => 6,   // BLOCK_16X16
        (16, 32) => 7,   // BLOCK_16X32
        (32, 16) => 8,   // BLOCK_32X16
        (32, 32) => 9,   // BLOCK_32X32
        (32, 64) => 10,  // BLOCK_32X64
        (64, 32) => 11,  // BLOCK_64X32
        (64, 64) => 12,  // BLOCK_64X64
        (64, 128) => 13, // BLOCK_64X128
        (128, 64) => 14, // BLOCK_128X64
        (128, 128) => 15, // BLOCK_128X128
        (4, 16) => 16,   // BLOCK_4X16
        (16, 4) => 17,   // BLOCK_16X4
        (8, 32) => 18,   // BLOCK_8X32
        (32, 8) => 19,   // BLOCK_32X8
        (16, 64) => 20,  // BLOCK_16X64
        (64, 16) => 21,  // BLOCK_64X16
        _ => panic!("no BlockSize for {width}x{height}"),
    }
}

/// Encode the `use_filter_intra` flag for an eligible intra block.
///
/// C: `aom_write_symbol(ec_writer, filter_intra_mode != FILTER_INTRA_MODES,
/// frame_context->filter_intra_cdfs[bsize], 2)` in the key-frame block walk
/// (entropy_coding.c:5099-5104; same shape on the inter-frame intra path,
/// :5231-5236), written right after the palette syntax and before
/// code_tx_size. The decoder mirror is read_filter_intra_mode_info.
///
/// Eligibility is the caller's job — C `svt_aom_filter_intra_allowed`
/// (mode_decision.c:102-108): SH filter_intra level != 0, `mode == DC_PRED`,
/// `palette_size == 0`, and `block_size_wide/high[bsize] <= 32`
/// (`svt_aom_filter_intra_allowed_bsize`). `bsize_idx` is the
/// [`block_size_index`] of the LUMA block. When `used`, C follows with a
/// CDF5 filter_intra_mode symbol — [`write_filter_intra_mode`].
pub fn write_use_filter_intra(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    bsize_idx: usize,
    used: bool,
) {
    debug_assert!(bsize_idx < BLOCK_SIZES_ALL);
    w.write_symbol(usize::from(used), &mut fc.filter_intra_cdfs[bsize_idx], 2);
}

/// Encode the filter_intra_mode symbol (0..4: FILTER_DC / V / H / D157 /
/// PAETH) following `use_filter_intra == 1`.
///
/// C: `aom_write_symbol(ec_writer, filter_intra_mode,
/// frame_context->filter_intra_mode_cdf, FILTER_INTRA_MODES)`
/// (entropy_coding.c:5105-5110; decoder read_filter_intra_mode_info).
pub fn write_filter_intra_mode(w: &mut AomWriter, fc: &mut FrameContext, fi_mode: u8) {
    debug_assert!(fi_mode < 5);
    w.write_symbol(fi_mode as usize, &mut fc.filter_intra_mode_cdf, 5);
}

/// Encode the angle delta for a directional intra mode.
///
/// AV1 spec Section 5.11.42: angle_delta is signaled for directional modes
/// (V_PRED through D67_PRED) when the block is at least 8x8.
/// The delta ranges from -3 to +3 (7 symbols, symbol 3 = delta 0).
pub fn write_angle_delta(w: &mut AomWriter, fc: &mut FrameContext, mode: u8, angle_delta: i8) {
    debug_assert!(
        is_directional_mode(mode),
        "angle_delta only for directional modes"
    );
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
        // filter_intra_cdfs from the generated C defaults (BLOCK_8X8 row
        // = 24902, BLOCK_32X32 row = 10425 — default_cdfs.rs, drift-tested
        // vs FcTable::FilterIntra in tests/c_parity.rs).
        assert_eq!(fc.filter_intra_cdfs[3][0], 24902);
        assert_eq!(fc.filter_intra_cdfs[9][0], 10425);
    }

    /// block_size_index must reproduce the C BlockSize enum order
    /// (definitions.h:923-946) — spot-check every entry via the
    /// C `block_size_wide/high` semantics.
    #[test]
    fn block_size_index_matches_c_enum_order() {
        const DIMS: [(usize, usize); 22] = [
            (4, 4),
            (4, 8),
            (8, 4),
            (8, 8),
            (8, 16),
            (16, 8),
            (16, 16),
            (16, 32),
            (32, 16),
            (32, 32),
            (32, 64),
            (64, 32),
            (64, 64),
            (64, 128),
            (128, 64),
            (128, 128),
            (4, 16),
            (16, 4),
            (8, 32),
            (32, 8),
            (16, 64),
            (64, 16),
        ];
        for (i, &(w, h)) in DIMS.iter().enumerate() {
            assert_eq!(block_size_index(w, h), i, "{w}x{h}");
        }
    }

    /// use_filter_intra = 0 through the default BLOCK_8X8 CDF must code
    /// the same arithmetic as any nsyms=2 bool with f = icdf[0] (the C
    /// aom_write_symbol nsyms==2 specialization) and must adapt the CDF.
    #[test]
    fn write_use_filter_intra_smoke() {
        let mut fc = FrameContext::new_default();
        let mut w = AomWriter::new(64);
        let before = fc.filter_intra_cdfs[3];
        write_use_filter_intra(&mut w, &mut fc, 3, false);
        assert_ne!(
            fc.filter_intra_cdfs[3], before,
            "CDF must adapt after coding (decoder updates too)"
        );
        let _ = w.done();
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

    /// Pin tx_size_cat / tx_max_depth against hand-walked C values:
    /// bsize_to_tx_size_cat / bsize_to_max_depth chains through
    /// eb_sub_tx_size_map from blocksize_to_txsize[bsize]
    /// (TX_64X64→TX_32X32→TX_16X16→TX_8X8→TX_4X4; rect TXs halve the
    /// larger dim, e.g. TX_32X64→TX_32X32, TX_4X16→TX_4X8→TX_4X4).
    #[test]
    fn tx_size_cat_and_depth_match_c_tables() {
        // (w, h, cat, max_depth) — every signaling bsize <= 64x64.
        const CASES: [(usize, usize, usize, usize); 18] = [
            (4, 8, 0, 1),
            (8, 4, 0, 1),
            (8, 8, 0, 1),
            (8, 16, 1, 2),
            (16, 8, 1, 2),
            (4, 16, 1, 2),
            (16, 4, 1, 2),
            (16, 16, 1, 2),
            (16, 32, 2, 2),
            (32, 16, 2, 2),
            (8, 32, 2, 2),
            (32, 8, 2, 2),
            (32, 32, 2, 2),
            (32, 64, 3, 2),
            (64, 32, 3, 2),
            (16, 64, 3, 2),
            (64, 16, 3, 2),
            (64, 64, 3, 2),
        ];
        for (w, h, cat, maxd) in CASES {
            assert_eq!(tx_size_cat(w, h), cat, "cat {w}x{h}");
            assert_eq!(tx_max_depth(w, h), maxd, "max_depth {w}x{h}");
        }
    }

    /// The 64x64 depth-0 tx_depth symbol must come from tx_size_cdf[3][0]
    /// with the C default icdf [26986, 21293] (op 4 of the C uniform-p13
    /// identity trace: `W CDF nsyms=3 s=0 icdf=[26986,21293,0]`).
    #[test]
    fn tx_depth_64x64_uses_cat3_defaults() {
        let fc = FrameContext::new_default();
        assert_eq!(tx_size_cat(64, 64), 3);
        assert_eq!(tx_max_depth(64, 64) + 1, 3);
        assert_eq!(&fc.tx_size_cdf[3][0][..2], &[26986, 21293]);
    }
}

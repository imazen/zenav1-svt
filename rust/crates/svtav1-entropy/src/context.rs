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

// --- CfL (chroma-from-luma) constants (C definitions.h) ---
/// `CFL_ALPHABET_SIZE_LOG2` — alpha magnitude index is 4 bits.
pub const CFL_ALPHABET_SIZE_LOG2: usize = 4;
/// `CFL_ALPHABET_SIZE` — 16 alpha magnitudes per plane.
pub const CFL_ALPHABET_SIZE: usize = 1 << CFL_ALPHABET_SIZE_LOG2;
/// `CFL_JOINT_SIGNS` — CFL_SIGNS*CFL_SIGNS - 1 = 8 (zero,zero invalid).
pub const CFL_JOINT_SIGNS: usize = 8;
/// `CFL_ALPHA_CONTEXTS` — CFL_JOINT_SIGNS + 1 - CFL_SIGNS = 6.
pub const CFL_ALPHA_CONTEXTS: usize = 6;
/// `UV_CFL_PRED` — the chroma mode index that signals CfL (after the 13
/// non-CFL uv modes DC..PAETH).
pub const UV_CFL_PRED: u8 = 13;

/// C `CFL_SIGN_U(js)` = `((js + 1) * 11) >> 5` (== (js+1)/3 for js 0..8).
#[inline]
pub fn cfl_sign_u(js: usize) -> usize {
    ((js + 1) * 11) >> 5
}
/// C `CFL_SIGN_V(js)` = `(js + 1) - CFL_SIGNS * CFL_SIGN_U(js)`.
#[inline]
pub fn cfl_sign_v(js: usize) -> usize {
    (js + 1) - 3 * cfl_sign_u(js)
}
/// C `CFL_CONTEXT_U(js)` = `js + 1 - CFL_SIGNS`.
#[inline]
pub fn cfl_context_u(js: usize) -> usize {
    js + 1 - 3
}
/// C `CFL_CONTEXT_V(js)` = `CFL_SIGN_V(js)*CFL_SIGNS + CFL_SIGN_U(js) - CFL_SIGNS`.
#[inline]
pub fn cfl_context_v(js: usize) -> usize {
    cfl_sign_v(js) * 3 + cfl_sign_u(js) - 3
}

/// C `default_cfl_sign_cdf` (cabac_context_model.c:335,
/// `AOM_CDF8(1418,2123,13340,18405,26972,28343,32294)`) in ICDF storage.
#[rustfmt::skip]
pub static CFL_SIGN_CDF_DEFAULT: [AomCdfProb; CFL_JOINT_SIGNS + 1] =
    [31350, 30645, 19428, 14363, 5796, 4425, 474, 0, 0];

/// C `default_cfl_alpha_cdf` (cabac_context_model.c:339, 6 `AOM_CDF16`
/// rows) in ICDF storage.
#[rustfmt::skip]
pub static CFL_ALPHA_CDF_DEFAULT: [[AomCdfProb; CFL_ALPHABET_SIZE + 1]; CFL_ALPHA_CONTEXTS] = [
    [25131, 12049, 1367, 287, 111, 80, 76, 72, 68, 64, 60, 56, 52, 48, 44, 0, 0],
    [18403, 9165, 4633, 1600, 601, 373, 281, 195, 148, 121, 100, 96, 92, 88, 84, 0, 0],
    [21236, 10388, 4323, 1408, 419, 245, 184, 119, 95, 91, 87, 83, 79, 75, 71, 0, 0],
    [5778, 1366, 486, 197, 76, 72, 68, 64, 60, 56, 52, 48, 44, 40, 36, 0, 0],
    [15520, 6710, 3864, 2160, 1463, 891, 642, 447, 374, 304, 252, 208, 192, 175, 146, 0, 0],
    [18030, 11090, 6989, 4867, 3744, 2466, 1788, 925, 624, 355, 248, 174, 146, 112, 108, 0, 0],
];

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

    /// palette_y_mode flag CDFs [palette bsize ctx 0..6][neighbor ctx 0..2]
    /// (C FRAME_CONTEXT.palette_y_mode_cdf; coded for every DC_PRED intra
    /// block where svt_aom_allow_palette holds).
    pub palette_y_mode_cdf: [[[AomCdfProb; 3]; 3]; 7],

    /// palette_uv_mode flag CDFs [y-palette-used ctx 0..1] (C
    /// FRAME_CONTEXT.palette_uv_mode_cdf; coded for every UV_DC_PRED
    /// chroma-ref block where svt_aom_allow_palette holds).
    pub palette_uv_mode_cdf: [[AomCdfProb; 3]; 2],

    /// palette_y_size CDFs [palette bsize ctx 0..6], 7 symbols (n-2).
    pub palette_y_size_cdf: [[AomCdfProb; 8]; 7],

    /// palette_y_color_index CDFs [n-2][color ctx 0..4]; n symbols per
    /// row, count slot at index n (tails structurally zero).
    pub palette_y_color_index_cdf: [[[AomCdfProb; 9]; 5]; 7],

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

    /// CfL joint-sign CDF — C FRAME_CONTEXT.cfl_sign_cdf
    /// (default_cfl_sign_cdf, cabac_context_model.c:335), CDF over the 8
    /// joint (Cb,Cr) sign codes. Coded first by write_cfl_alphas when
    /// uv_mode == UV_CFL_PRED.
    pub cfl_sign_cdf: [AomCdfProb; CFL_JOINT_SIGNS + 1],

    /// CfL alpha-magnitude CDFs [CFL_ALPHA_CONTEXTS][CFL_ALPHABET_SIZE+1] —
    /// C FRAME_CONTEXT.cfl_alpha_cdf (default_cfl_alpha_cdf,
    /// cabac_context_model.c:339). Coded per nonzero-sign plane by
    /// write_cfl_alphas, context selected by CFL_CONTEXT_U/V(joint_sign).
    pub cfl_alpha_cdf: [[AomCdfProb; CFL_ALPHABET_SIZE + 1]; CFL_ALPHA_CONTEXTS],

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

    // --- IntraBC (intra block copy, screen content) ---
    /// `use_intrabc` flag CDF — C FRAME_CONTEXT.intrabc_cdf
    /// (`default_intrabc_cdf` = AOM_CDF2(30531), cabac_context_model.c:
    /// 610-612, installed :792). Coded by `write_intrabc_info`
    /// (entropy_coding.c:4405-4416) for every block on a frame where
    /// `svt_aom_allow_intrabc` holds; adapted per committed block
    /// (md_rate_estimation.c:854-855).
    pub intrabc_cdf: [AomCdfProb; 3],

    /// DV (displacement vector) entropy context — C FRAME_CONTEXT.ndvc.
    /// Seeded from the EXACT same `default_nmv_context` as `nmvc`
    /// (cabac_context_model.c:795), but adapted independently: `ndvc` only
    /// ever codes IntraBC DVs (`svt_av1_encode_dv`, entropy_coding.c:4381,
    /// with literal `MV_SUBPEL_NONE` — sign/class/integer bits only).
    pub ndvc: crate::mv_coding::NmvContext,
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

/// Binary SPLIT-vs-{H,V} "alike" cost at a one-false boundary node, read
/// from the DEFAULT (frame-initial) partition CDF row `[bsl*4 + 0]` — the
/// LPD0 (PD0_LVL_5/6) analogue of [`partition_symbol_cost`] for
/// `PARTITION_SPLIT`. `bottom_edge` (`!has_rows`) gathers the vert-alike
/// cdf; else (right edge, `!has_cols`) the horz-alike (the CROSS-named
/// gather trap, cabac_context_model.h:378/393). Mirrors what the M6 tables
/// build from an adapting `partition_cdf` row, but at ctx 0 / default CDF —
/// which is what LPD0's `svt_aom_partition_rate_cost` prices against.
pub fn partition_alike_split_symbol_cost(width: usize, bottom_edge: bool, is_128: bool) -> u32 {
    debug_assert!((8..=128).contains(&width) && width.is_power_of_two());
    let bsl = width.ilog2() as usize - 3;
    let row = &DEFAULT_PARTITION_CDF[(bsl * 4).min(PARTITION_CONTEXTS - 1)];
    partition_alike_split_cost(row, bottom_edge, is_128)
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
            palette_y_mode_cdf: crate::default_cdfs::PALETTE_Y_MODE_CDF,
            palette_uv_mode_cdf: crate::default_cdfs::PALETTE_UV_MODE_CDF,
            palette_y_size_cdf: crate::default_cdfs::PALETTE_Y_SIZE_CDF,
            palette_y_color_index_cdf: crate::default_cdfs::PALETTE_Y_COLOR_INDEX_CDF,
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
            cfl_sign_cdf: CFL_SIGN_CDF_DEFAULT,
            cfl_alpha_cdf: CFL_ALPHA_CDF_DEFAULT,
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
            // AV1 default_delta_q_cdf = AOM_CDF4(28160, 32120, 32677)
            // (cabac_context_model.c:637) in ICDF form: 32768 - cum.
            delta_q_cdf: [4608, 648, 91, 0, 0],
            // C default_intrabc_cdf = AOM_CDF2(30531) (cabac_context_model.c:
            // 610-612); the generated table is drift-tested vs FcTable::IntraBc
            // in tests/c_parity.rs.
            intrabc_cdf: crate::default_cdfs::INTRABC_CDF,
            // C seeds ndvc from default_nmv_context — the SAME table as nmvc
            // (cabac_context_model.c:795); NmvContext::default() is that
            // table (drift-tested vs FcTable::Nmvc in tests/c_parity_mv.rs).
            ndvc: crate::mv_coding::NmvContext::default(),
        }
    }

    /// In-place weighted per-entry average of `self` (left, ×`wt_left`) with a
    /// top-right neighbor context (×`wt_tr`) — the FRAME_CONTEXT half of
    /// `avg_cdf_symbols` (`enc_dec_process.c:2710-2805`). Used to seed a
    /// super-block's rate-estimation context from its left×3 + top-right×1
    /// neighbors when both are available (`pic_based_rate_est == false`, the
    /// only mode C ships). Every CDF array is averaged element-wise (see
    /// [`crate::cdf::avg_cdf_entries`]); inter/MV/segmentation fields that never
    /// evolve in an intra frame hold equal defaults on both neighbors, so
    /// averaging them is a no-op there and this stays exact for still frames.
    pub fn avg_cdf_with(&mut self, tr: &FrameContext, wt_left: i32, wt_tr: i32) {
        use crate::cdf::avg_cdf_entries as avg;
        // 1D
        avg(&mut self.filter_intra_mode_cdf, &tr.filter_intra_mode_cdf, wt_left, wt_tr);
        avg(&mut self.cfl_sign_cdf, &tr.cfl_sign_cdf, wt_left, wt_tr);
        // IntraBC: C averages intrabc_cdf + the whole ndvc alongside nmvc
        // (enc_dec_process.c:2638-2640, avg_nmv :2567-2579 — every CDF field).
        avg(&mut self.intrabc_cdf, &tr.intrabc_cdf, wt_left, wt_tr);
        avg(&mut self.ndvc.joints_cdf, &tr.ndvc.joints_cdf, wt_left, wt_tr);
        for i in 0..2 {
            let (l, r) = (&mut self.ndvc.comps[i], &tr.ndvc.comps[i]);
            avg(&mut l.classes_cdf, &r.classes_cdf, wt_left, wt_tr);
            avg(l.class0_fp_cdf.as_flattened_mut(), r.class0_fp_cdf.as_flattened(), wt_left, wt_tr);
            avg(&mut l.fp_cdf, &r.fp_cdf, wt_left, wt_tr);
            avg(&mut l.sign_cdf, &r.sign_cdf, wt_left, wt_tr);
            avg(&mut l.class0_hp_cdf, &r.class0_hp_cdf, wt_left, wt_tr);
            avg(&mut l.hp_cdf, &r.hp_cdf, wt_left, wt_tr);
            avg(&mut l.class0_cdf, &r.class0_cdf, wt_left, wt_tr);
            avg(l.bits_cdf.as_flattened_mut(), r.bits_cdf.as_flattened(), wt_left, wt_tr);
        }
        avg(self.cfl_alpha_cdf.as_flattened_mut(), tr.cfl_alpha_cdf.as_flattened(), wt_left, wt_tr);
        avg(&mut self.wiener_restore_cdf, &tr.wiener_restore_cdf, wt_left, wt_tr);
        avg(&mut self.delta_q_cdf, &tr.delta_q_cdf, wt_left, wt_tr);
        // 2D
        avg(self.partition_cdf.as_flattened_mut(), tr.partition_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.skip_cdf.as_flattened_mut(), tr.skip_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.skip_mode_cdf.as_flattened_mut(), tr.skip_mode_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.intra_inter_cdf.as_flattened_mut(), tr.intra_inter_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.y_mode_cdf.as_flattened_mut(), tr.y_mode_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.angle_delta_cdf.as_flattened_mut(), tr.angle_delta_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.filter_intra_cdfs.as_flattened_mut(), tr.filter_intra_cdfs.as_flattened(), wt_left, wt_tr);
        avg(self.inter_compound_mode_cdf.as_flattened_mut(), tr.inter_compound_mode_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.newmv_cdf.as_flattened_mut(), tr.newmv_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.globalmv_cdf.as_flattened_mut(), tr.globalmv_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.refmv_cdf.as_flattened_mut(), tr.refmv_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.drl_cdf.as_flattened_mut(), tr.drl_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.txb_skip_cdf.as_flattened_mut(), tr.txb_skip_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.interp_filter_cdf.as_flattened_mut(), tr.interp_filter_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.comp_inter_cdf.as_flattened_mut(), tr.comp_inter_cdf.as_flattened(), wt_left, wt_tr);
        // palette CDFs (C enc_dec_process.c:2595-2609; the color-index
        // rows use AVG_CDF_STRIDE with nsymbs=j+2, which full-row
        // averaging reproduces because the per-row tails are zero on
        // both inputs and each row's count slot is at the same index).
        avg(self.palette_uv_mode_cdf.as_flattened_mut(), tr.palette_uv_mode_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.palette_y_mode_cdf.as_flattened_mut().as_flattened_mut(), tr.palette_y_mode_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.palette_y_size_cdf.as_flattened_mut(), tr.palette_y_size_cdf.as_flattened(), wt_left, wt_tr);
        avg(self.palette_y_color_index_cdf.as_flattened_mut().as_flattened_mut(), tr.palette_y_color_index_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        // 3D
        avg(self.kf_y_mode_cdf.as_flattened_mut().as_flattened_mut(), tr.kf_y_mode_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.uv_mode_cdf.as_flattened_mut().as_flattened_mut(), tr.uv_mode_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.tx_size_cdf.as_flattened_mut().as_flattened_mut(), tr.tx_size_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.dc_sign_cdf.as_flattened_mut().as_flattened_mut(), tr.dc_sign_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.eob_flag_cdf.as_flattened_mut().as_flattened_mut(), tr.eob_flag_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.single_ref_cdf.as_flattened_mut().as_flattened_mut(), tr.single_ref_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
        avg(self.comp_ref_cdf.as_flattened_mut().as_flattened_mut(), tr.comp_ref_cdf.as_flattened().as_flattened(), wt_left, wt_tr);
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

// ---- Frame-edge partition coding (spec 5.11.4 / C encode_partition_av1) ----
//
// Partition symbol indices (C `PartitionType`, definitions.h:932-946).
const PARTITION_HORZ: usize = 1;
const PARTITION_SPLIT: usize = 3;
const PARTITION_HORZ_A: usize = 4;
const PARTITION_HORZ_B: usize = 5;
const PARTITION_VERT_A: usize = 6;
const PARTITION_VERT_B: usize = 7;
const PARTITION_HORZ_4: usize = 8;
const PARTITION_VERT: usize = 2;
const PARTITION_VERT_4: usize = 9;

/// C `cdf_element_prob` (cabac_context_model.h:373-376): the Q15 probability
/// of `element`, recovered from the stored INVERSE CDF as
/// `(element > 0 ? cdf[element - 1] : CDF_PROB_TOP) - cdf[element]`.
///
/// Deliberately has NO `EC_MIN_PROB` floor — unlike [`partition_symbol_cost`],
/// which is the rate-table path and does floor at 4. The gathers below must
/// reproduce C's arithmetic bit-for-bit, so this stays unfloored. `wrapping_sub`
/// mirrors C's `uint16_t` arithmetic (a well-formed CDF never underflows).
fn cdf_element_prob(cdf: &[AomCdfProb], element: usize) -> AomCdfProb {
    let prev = if element > 0 { cdf[element - 1] } else { CDF_PROB_TOP };
    prev.wrapping_sub(cdf[element])
}

/// C `partition_gather_horz_alike` (cabac_context_model.h:378-391). Used at the
/// RIGHT frame edge (`has_rows && !has_cols`), where the only codable outcomes
/// are a vertical split or a full SPLIT: the 10-symbol partition CDF collapses
/// to a 2-symbol "is it SPLIT" CDF.
fn partition_gather_horz_alike(out: &mut [AomCdfProb; 3], inp: &[AomCdfProb], is_128: bool) {
    let mut v = CDF_PROB_TOP;
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_HORZ));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_SPLIT));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_HORZ_A));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_HORZ_B));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_VERT_A));
    if !is_128 {
        v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_HORZ_4));
    }
    out[0] = CDF_PROB_TOP.wrapping_sub(v); // AOM_ICDF(x) == CDF_PROB_TOP - x
    out[1] = 0; // AOM_ICDF(CDF_PROB_TOP) == 0
    out[2] = 0; // CDF adaptation counter
}

/// C `partition_gather_vert_alike` (cabac_context_model.h:393-406). Used at the
/// BOTTOM frame edge (`!has_rows && has_cols`) — horizontal split or SPLIT.
/// Note the deliberate asymmetry vs [`partition_gather_horz_alike`]: both
/// include HORZ_A and VERT_A, but this one takes VERT/VERT_B/VERT_4 where the
/// other takes HORZ/HORZ_B/HORZ_4. Transcribed exactly from C.
fn partition_gather_vert_alike(out: &mut [AomCdfProb; 3], inp: &[AomCdfProb], is_128: bool) {
    let mut v = CDF_PROB_TOP;
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_VERT));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_SPLIT));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_HORZ_A));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_VERT_A));
    v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_VERT_B));
    if !is_128 {
        v = v.wrapping_sub(cdf_element_prob(inp, PARTITION_VERT_4));
    }
    out[0] = CDF_PROB_TOP.wrapping_sub(v);
    out[1] = 0;
    out[2] = 0;
}

/// Code one partition symbol with frame-edge awareness — C
/// `encode_partition_av1` (entropy_coding.c:932-981) == spec 5.11.4.
///
/// `has_rows` / `has_cols` are `(blk_y + hbs) < aligned_h` /
/// `(blk_x + hbs) < aligned_w` with `hbs` = HALF the block width in pixels
/// (C :941-943). Three cases:
/// - both false (block's whole lower-right quadrant is off-frame): the
///   partition is FORCED to SPLIT and **no symbol is coded at all**.
/// - both true (the interior case): the ordinary full-alphabet symbol.
/// - exactly one false: a BINARY `partition == SPLIT` symbol coded against a
///   gathered 2-symbol CDF.
///
/// IMPORTANT — the binary arm does NOT adapt the frame context. C builds the
/// gathered CDF on the STACK and lets `aom_write_symbol` adapt that throwaway
/// copy, leaving `frame_context->partition_cdf` untouched; the decoder does the
/// same, so the two stay in sync. [`AomWriter::write_cdf`] encodes without
/// updating, which is exactly that behaviour.
///
/// On a 64-aligned frame every block has `has_rows == has_cols == true`, so
/// this routes to [`write_partition`] and is bit-identical to the pre-edge port.
#[allow(clippy::too_many_arguments)]
pub fn write_partition_edge(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    ctx: usize,
    partition: u8,
    nsymbs: usize,
    is_128: bool,
    has_rows: bool,
    has_cols: bool,
) {
    if !has_rows && !has_cols {
        debug_assert_eq!(
            partition as usize, PARTITION_SPLIT,
            "off-frame quadrant forces PARTITION_SPLIT (C encode_partition_av1:963)"
        );
        return;
    }
    if has_rows && has_cols {
        write_partition(w, fc, ctx, partition, nsymbs);
        return;
    }
    debug_assert!(ctx < PARTITION_CONTEXTS);
    let mut cdf = [0 as AomCdfProb; 3];
    if !has_rows {
        partition_gather_vert_alike(&mut cdf, &fc.partition_cdf[ctx], is_128);
    } else {
        partition_gather_horz_alike(&mut cdf, &fc.partition_cdf[ctx], is_128);
    }
    w.write_cdf(usize::from(partition as usize == PARTITION_SPLIT), &cdf, 2);
}

/// Cost (1/512 bit) of coding PARTITION_SPLIT at a one-false boundary node,
/// using the BINARY split-vs-{H,V} alphabet — C `svt_aom_partition_rate_cost`
/// boundary branch (rd_cost.c:1846-1863): the SPLIT rate is
/// `partition_{vert,horz}_alike_fac_bits[ctx][p == PARTITION_SPLIT]`, i.e. the
/// cost of symbol 1 in the gathered 2-symbol CDF, NOT the full-alphabet
/// `partition_fac_bits[ctx][SPLIT]`. `bottom_edge` (`!has_rows`) selects the
/// vert_alike gather; `!bottom_edge` (`!has_cols`, right edge) the horz_alike —
/// the gather is CROSS-named vs the option (see [`write_partition_edge`]).
/// `partition_cdf_row` is `fc.partition_cdf[ctx]` at `ctx = bsl*PARTITION_PLOFFSET`
/// (left = above = 0 in PD0). For the gathered 2-symbol icdf `[x, 0]`,
/// `P(sym1=SPLIT) = x`, so the cost is `av1_cost_symbol(x.max(EC_MIN_PROB))`.
pub fn partition_alike_split_cost(
    partition_cdf_row: &[AomCdfProb],
    bottom_edge: bool,
    is_128: bool,
) -> u32 {
    let mut cdf = [0 as AomCdfProb; 3];
    if bottom_edge {
        partition_gather_vert_alike(&mut cdf, partition_cdf_row, is_128);
    } else {
        partition_gather_horz_alike(&mut cdf, partition_cdf_row, is_128);
    }
    av1_cost_symbol((cdf[0] as u32).max(4 /* EC_MIN_PROB */))
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

/// C `svt_aom_allow_palette` (entropy_coding.c:4211): screen-content tools
/// on, both dims <= 64, and `bsize >= BLOCK_8X8` — which in enum order
/// EXCLUDES only 4x4/4x8/8x4 (the extended 4:1 rects sort after 128x128
/// and are palette-eligible).
pub fn allow_palette(allow_screen_content_tools: bool, width: usize, height: usize) -> bool {
    allow_screen_content_tools
        && width <= 64
        && height <= 64
        && block_size_index(width, height) >= 3
}

/// palette bsize ctx (C `svt_aom_get_palette_bsize_ctx`):
/// `num_pels_log2[bsize] - num_pels_log2[BLOCK_8X8]`, 0..=6.
pub fn palette_bsize_ctx(width: usize, height: usize) -> usize {
    (width * height).trailing_zeros() as usize - 6
}

/// C `PALETTE_MIN_SIZE` (definitions.h:403) — mirrors
/// `svtav1_encoder::palette::PALETTE_MIN_SIZE`; duplicated here (rather than
/// imported) because svtav1-entropy cannot depend on svtav1-encoder — the
/// dependency edge runs the other way.
const PALETTE_MIN_SIZE: usize = 2;

/// C `PALETTE_SIZES` (definitions.h:1171): 7 codable sizes (2..=8 colors).
const PALETTE_SIZES: usize = 7;

/// C `av1_ceil_log2` (cabac_context_model.h:366-371). Duplicated from
/// `svtav1_encoder::palette::ceil_log2` (same cross-crate-dependency reason
/// as [`PALETTE_MIN_SIZE`] above) — `0` if `n < 2`, else
/// `floor(log2(n - 1)) + 1`.
fn ceil_log2_pal(n: i32) -> i32 {
    if n < 2 {
        return 0;
    }
    let m = (n - 1) as u32;
    31 - m.leading_zeros() as i32 + 1
}

/// C `delta_encode_palette_colors` (entropy_coding.c:4256-4288) — the WRITER
/// twin of `svtav1_encoder::palette::delta_encode_steps` (identical step
/// sequence: that fn returns bit widths for the RD cost estimate, this one
/// emits the bits). Transcribed directly rather than shared because
/// svtav1-entropy cannot depend on svtav1-encoder; keep both in sync by
/// hand if either changes (cross-referenced in both directions).
// EXERCISED end-to-end since #71 palette injection landed (leaf_funnel.rs
// sets `palette: Some(..)`, pipeline.rs writes it) — no longer dead. It runs
// on the EPICA screen-content cell; that cell does not byte-match C yet
// (palette over-picking, #71), so this is exercised-but-not-yet-byte-verified.
// The step sequence is additionally locked by the self-consistency unit test
// `write_delta_encoded_colors_hand_consistency` below.
fn write_delta_encoded_colors(w: &mut AomWriter, colors: &[u16], bit_depth: u32, min_val: i32) {
    let num = colors.len();
    if num == 0 {
        return;
    }
    debug_assert!((colors[0] as u32) < (1 << bit_depth));
    w.write_literal(colors[0] as u32, bit_depth);
    if num == 1 {
        return;
    }
    let min_bits = bit_depth as i32 - 3;
    let mut max_delta = 0i32;
    // PALETTE_MAX_SIZE - 1 slots ever used (num <= 8), matching C's fixed
    // `deltas[PALETTE_MAX_SIZE]` scratch.
    let mut deltas = [0i32; svtav1_types::prediction::PALETTE_MAX_SIZE];
    for i in 1..num {
        debug_assert!((colors[i] as u32) < (1 << bit_depth));
        let delta = colors[i] as i32 - colors[i - 1] as i32;
        debug_assert!(delta >= min_val, "colors must be ascending with gaps >= min_val");
        deltas[i - 1] = delta;
        max_delta = max_delta.max(delta);
    }
    let mut bits = ceil_log2_pal(max_delta + 1 - min_val).max(min_bits);
    debug_assert!(bits <= bit_depth as i32);
    let mut range = (1i32 << bit_depth) - colors[0] as i32 - min_val;
    w.write_literal((bits - min_bits) as u32, 2);
    for i in 0..num - 1 {
        w.write_literal((deltas[i] - min_val) as u32, bits as u32);
        range -= deltas[i];
        bits = bits.min(ceil_log2_pal(range));
    }
}

/// C `write_palette_colors_y` (entropy_coding.c:4324-4341) minus the
/// cache-building: the caller already ran C `svt_get_palette_cache_y` +
/// `svt_av1_index_color_cache` (see `svtav1_encoder`'s `palette_cache` /
/// `palette::index_color_cache`, the FFI-verified twin this reuses from the
/// caller side, since svtav1-entropy cannot depend on svtav1-encoder).
/// `n` is the total palette size; `cache_found` is one flag per consulted
/// neighbor color-cache entry; `out_of_cache` is the not-in-cache colors
/// (ascending). Writes one raw bit per `cache_found` entry, stopping early
/// once every color is accounted for — exactly C's `i < n_cache &&
/// n_in_cache < n` loop guard — then delta-encodes `out_of_cache` (bd8:
/// `bit_depth = 8`, `min_val = 1`, matching every C call site for the luma
/// palette).
// Reached via the `Some` arm of `write_palette_mode_info`, which a winning
// palette leaf now takes (#71 injection landed) — exercised on the EPICA
// screen-content cell, which does not byte-match C yet (over-picking, #71), so
// exercised-but-not-yet-byte-verified.
fn write_palette_colors_y(w: &mut AomWriter, n: usize, cache_found: &[bool], out_of_cache: &[u16]) {
    let mut n_in_cache = 0usize;
    for &found in cache_found {
        if n_in_cache >= n {
            break;
        }
        w.write_bit(found);
        n_in_cache += usize::from(found);
    }
    debug_assert_eq!(n_in_cache + out_of_cache.len(), n);
    write_delta_encoded_colors(w, out_of_cache, 8, 1);
}

/// Code the palette mode-info syntax for an intra block (C
/// `write_palette_mode_info`, entropy_coding.c:4355-4379; call only when
/// [`allow_palette`] holds). The y flag is coded for DC_PRED luma; when it
/// is 1 (`palette` is `Some`), the size symbol + colors follow immediately
/// (`write_palette_colors_y`, entropy_coding.c:4324-4341). The uv flag is
/// coded for UV_DC_PRED on chroma-ref blocks — always symbol 0 (chroma
/// palette is dead in this port: `palette_size[1]` is hard-0 at injection,
/// see `docs/palette-port-map.md`) — with context `(this block's own y
/// flag)`, matching C's `(blk_ptr->palette_size[0] > 0)`
/// (entropy_coding.c:4376).
///
/// `neighbor_ctx` is the CALLER-derived `svt_aom_get_palette_mode_ctx`
/// (above/left neighbor palette-used count, 0..=2) for the Y FLAG only —
/// the uv ctx is derived internally from this call's own y flag, per C.
///
/// `palette`, when `Some`, is `(colors, cache_found, out_of_cache_colors)`:
/// - `colors`: the deduped ascending palette (2..=8 entries); `colors.len()`
///   drives the size symbol.
/// - `cache_found` / `out_of_cache_colors`: the caller's
///   `svt_av1_index_color_cache` split (see [`write_palette_colors_y`]).
///
/// This is the generalization of the former `write_no_palette_flags`: the
/// `None` arm below is EXACTLY that function's old behavior (bit-for-bit).
// The `None` arm is byte-verified: `tools/identity_matrix.sh` (54/54) and
// `tools/real_image_matrix.sh` (byte-identical on photo content) both stay
// green — non-screen content never takes the `Some` arm. The `Some` arm (y
// size symbol + colors) NOW runs against a real encode: #71 injection landed,
// so a winning palette leaf carries `Some(palette)` (leaf_funnel.rs) and this
// codes it on the EPICA screen-content cell. That cell does not byte-match C
// yet (palette over-picking, #71), so the `Some` arm is exercised-but-not-yet-
// byte-verified; the same-crate smoke test
// `write_palette_mode_info_some_vs_none_arm` locks its shape meanwhile.
pub fn write_palette_mode_info(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    width: usize,
    height: usize,
    y_mode: u8,
    uv_mode: u8,
    is_chroma_ref: bool,
    neighbor_ctx: usize,
    palette: Option<(&[u16], &[bool], &[u16])>,
) {
    let bctx = palette_bsize_ctx(width, height);
    let mut y_used = false;
    if y_mode == 0 {
        debug_assert!(neighbor_ctx < 3);
        y_used = palette.is_some();
        w.write_symbol(usize::from(y_used), &mut fc.palette_y_mode_cdf[bctx][neighbor_ctx], 2);
        if let Some((colors, cache_found, out_of_cache)) = palette {
            let n = colors.len();
            debug_assert!((PALETTE_MIN_SIZE..=svtav1_types::prediction::PALETTE_MAX_SIZE).contains(&n));
            w.write_symbol(n - PALETTE_MIN_SIZE, &mut fc.palette_y_size_cdf[bctx], PALETTE_SIZES);
            write_palette_colors_y(w, n, cache_found, out_of_cache);
        }
    }
    if uv_mode == 0 && is_chroma_ref {
        // C: palette_uv_mode_ctx = (blk_ptr->palette_size[0] > 0) — THIS
        // block's own y flag, not a neighbor count.
        let uv_ctx = usize::from(y_used);
        w.write_symbol(0, &mut fc.palette_uv_mode_cdf[uv_ctx], 2);
    }
}

/// C `get_unsigned_bits` (entropy_coding.c:4290-4292): `0` for `n == 0`,
/// else `get_msb(n) + 1` (position of the highest set bit, 0-indexed, plus
/// one) — `32 - n.leading_zeros()` for `n > 0` on a 32-bit host.
fn get_unsigned_bits(n: u32) -> u32 {
    if n == 0 { 0 } else { 32 - n.leading_zeros() }
}

/// C `write_uniform` (entropy_coding.c:4294-4306): codes `v` in `0..n` with
/// as-equal-as-possible-width literals (the truncated-binary code) — used
/// for the palette map's uncontextualized `(0, 0)` pixel.
pub fn write_uniform(w: &mut AomWriter, n: u32, v: u32) {
    let l = get_unsigned_bits(n);
    if l == 0 {
        return;
    }
    let m = (1u32 << l) - n;
    if v < m {
        w.write_literal(v, l - 1);
    } else {
        w.write_literal(m + ((v - m) >> 1), l - 1);
        w.write_literal((v - m) & 1, 1);
    }
}

/// C `svt_aom_palette_color_index_context_lookup` hash table
/// (palette.c:608): every reachable hash (2, 5, 6, 7, 8) maps to `9 -
/// hash`; kept only as a cross-check comment (see the `debug_assert` this
/// mirrors in `svtav1_encoder::palette::palette_color_index_context`).
const INVALID_COLOR_IDX_PAL: u8 = u8::MAX;

/// C `av1_fast_palette_color_index_context` + `_on_edge`
/// (palette.c:612-743): the rank-remapped color index and entropy context
/// for one palette-map pixel `(i, j)` (row, col) during the wavefront pass
/// — never called for `(0, 0)` (coded via [`write_uniform`] instead).
/// Returns `(ctx, color_new_idx)`.
///
/// DUPLICATE of `svtav1_encoder::palette::palette_color_index_context`
/// (chunk 2, already FFI/hand-vector-verified there) — re-transcribed here
/// because svtav1-entropy cannot depend on svtav1-encoder. MUST match that
/// copy's semantics exactly, including the edge path hardcoding ctx 0 (hash
/// 2 -> lookup[2] == 0). Consolidate into one crate once justified.
// PORT-NOTE(unverified): same evidence tier as the encoder-crate twin —
// hand-derived vectors (this crate's `palette_map_pixel_ctx_*_hand_vectors`
// tests reuse the SAME vectors as
// `svtav1-encoder/tests/c_parity_palette.rs`), not FFI or an identity
// cell (both static C fns, no exported symbol). Upgrade path: a
// ref_shims.c wrapper, or an EPICA/identity cell once #71 chunk 3/4
// injection exercises this end-to-end.
fn palette_map_pixel_ctx(color_map: &[u8], stride: usize, i: usize, j: usize) -> (usize, u8) {
    debug_assert!(i > 0 || j > 0);
    let has_above = i >= 1;
    let has_left = j >= 1;
    debug_assert!(has_above || has_left);

    if has_above != has_left {
        // Edge case: exactly one neighbor (top row or left column).
        let neighbor = if has_above {
            color_map[(i - 1) * stride + j]
        } else {
            color_map[i * stride + (j - 1)]
        };
        let current = color_map[i * stride + j];
        let idx = if neighbor > current {
            current + 1
        } else if neighbor == current {
            0
        } else {
            current
        };
        // color_score=2, hash_multiplier=1 => hash=2 => lookup[2]=0.
        return (0usize, idx);
    }

    // Interior case: three neighbors (left, top, top-left).
    let mut color_neighbors = [
        color_map[i * stride + (j - 1)],
        color_map[(i - 1) * stride + j],
        color_map[(i - 1) * stride + (j - 1)],
    ];
    let mut scores = [2u8, 2u8, 1u8];
    let mut num_invalid_colors = 0u8;
    if color_neighbors[0] == color_neighbors[1] {
        scores[0] += scores[1];
        color_neighbors[1] = INVALID_COLOR_IDX_PAL;
        num_invalid_colors += 1;
        if color_neighbors[0] == color_neighbors[2] {
            scores[0] += scores[2];
            num_invalid_colors += 1;
        }
    } else if color_neighbors[0] == color_neighbors[2] {
        scores[0] += scores[2];
        num_invalid_colors += 1;
    } else if color_neighbors[1] == color_neighbors[2] {
        scores[1] += scores[2];
        num_invalid_colors += 1;
    }
    let num_valid_colors = 3 - num_invalid_colors;

    if num_valid_colors > 1 {
        if color_neighbors[1] == INVALID_COLOR_IDX_PAL {
            scores[1] = scores[2];
            color_neighbors[1] = color_neighbors[2];
        }
        if scores[0] < scores[1] || (scores[0] == scores[1] && color_neighbors[0] > color_neighbors[1]) {
            scores.swap(0, 1);
            color_neighbors.swap(0, 1);
        }
        if num_valid_colors > 2 {
            if scores[0] < scores[2] {
                scores.swap(0, 2);
                color_neighbors.swap(0, 2);
            }
            if scores[1] < scores[2] {
                scores.swap(1, 2);
                color_neighbors.swap(1, 2);
            }
        }
    }

    let current = color_map[i * stride + j];
    let mut color_new_idx = current;
    for idx in 0..num_valid_colors as usize {
        if color_neighbors[idx] > current {
            color_new_idx += 1;
        } else if color_neighbors[idx] == current {
            color_new_idx = idx as u8;
            break;
        }
    }

    const HASH_MULTIPLIERS: [u8; 3] = [1, 2, 2];
    let mut hash = 0u8;
    for idx in 0..num_valid_colors as usize {
        hash += scores[idx] * HASH_MULTIPLIERS[idx];
    }
    debug_assert!(hash > 0 && hash <= 8);
    let ctx = 9 - hash as i32;
    debug_assert!(ctx >= 0 && (ctx as usize) < 5, "PALETTE_COLOR_INDEX_CONTEXTS == 5");
    (ctx as usize, color_new_idx)
}

/// Code the palette color-index map for one plane (C `pack_map_tokens`,
/// entropy_coding.c:4343-4353, fed by the SAME anti-diagonal wavefront
/// order as `svt_av1_tokenize_color_map` / `cost_and_tokenize_map`
/// (palette.c:748-782) — the search-side twin of that traversal is
/// `svtav1_encoder::palette::color_map_wavefront`, duplicated here for the
/// same cross-crate reason as the rest of this block).
///
/// `map` is the FULL nominal-size (block_w x block_h) color-index raster at
/// `stride`; `rows`/`cols` are the WITHIN-BOUNDS dims C derives from
/// `svt_aom_get_block_dimensions` (only differ from the nominal dims at
/// non-64-aligned right/bottom picture edges); `n` is the palette size
/// (2..=8).
///
/// PORT-NOTE(unverified): ONE gap remains. This function runs from
/// `write_palette_mode_info`'s `Some` arm, which #71 injection now reaches —
/// it codes the map on the EPICA screen-content cell (that cell does not
/// byte-match C yet, over-picking #71, so exercised-but-not-yet-byte-verified;
/// the `write_palette_mode_info_some_vs_none_arm` smoke test locks its shape).
/// The remaining gap is the within-bounds CLIP: full-SB frames pass `rows`/
/// `cols` equal to the nominal block dims, so the clip (rows/cols < nominal)
/// only fires on a partial-SB frame — #95 chunk 2. Verify the clip once a
/// non-64-aligned cell exercises an edge-clipped palette block.
pub fn write_palette_map_tokens(
    w: &mut AomWriter,
    fc: &mut FrameContext,
    map: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    n: usize,
) {
    debug_assert!((PALETTE_MIN_SIZE..=svtav1_types::prediction::PALETTE_MAX_SIZE).contains(&n));
    debug_assert!(rows >= 1 && cols >= 1);
    // The first color index (uncontextualized).
    write_uniform(w, n as u32, map[0] as u32);
    let n_idx = n - PALETTE_MIN_SIZE;
    for k in 1..(rows + cols - 1) {
        let j_hi = k.min(cols - 1);
        let j_lo = k.saturating_sub(rows - 1);
        for j in (j_lo..=j_hi).rev() {
            let i = k - j;
            let (ctx, color_new_idx) = palette_map_pixel_ctx(map, stride, i, j);
            debug_assert!((color_new_idx as usize) < n);
            w.write_symbol(color_new_idx as usize, &mut fc.palette_y_color_index_cdf[n_idx][ctx], n);
        }
    }
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

/// Encode the CfL alpha syntax following a `UV_CFL_PRED` chroma mode.
///
/// C `write_cfl_alphas` (entropy_coding.c:1159): code the joint sign, then
/// the Cb magnitude (if its sign is nonzero) and the Cr magnitude (if its
/// sign is nonzero). `idx` is `(CFL_IDX_U << 4) | CFL_IDX_V`, `joint_sign`
/// the 0..7 joint-sign code. Decoder mirror: read_cfl_alphas.
pub fn write_cfl_alphas(w: &mut AomWriter, fc: &mut FrameContext, idx: u8, joint_sign: u8) {
    let js = joint_sign as usize;
    debug_assert!(js < CFL_JOINT_SIGNS);
    w.write_symbol(js, &mut fc.cfl_sign_cdf, CFL_JOINT_SIGNS);
    if cfl_sign_u(js) != 0 {
        let cdf_u = &mut fc.cfl_alpha_cdf[cfl_context_u(js)];
        w.write_symbol((idx >> CFL_ALPHABET_SIZE_LOG2) as usize, cdf_u, CFL_ALPHABET_SIZE);
    }
    if cfl_sign_v(js) != 0 {
        let cdf_v = &mut fc.cfl_alpha_cdf[cfl_context_v(js)];
        w.write_symbol(
            (idx & (CFL_ALPHABET_SIZE as u8 - 1)) as usize,
            cdf_v,
            CFL_ALPHABET_SIZE,
        );
    }
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

    // =========================================================================
    // Palette PACK writers (task #71 chunk 5). `palette_map_pixel_ctx` is a
    // duplicate of `svtav1_encoder::palette::palette_color_index_context`
    // (see that fn's doc + docs/palette-port-map.md) — reusing the SAME
    // hand-derived vectors from `svtav1-encoder/tests/c_parity_palette.rs`
    // here is the cross-check that the duplication stayed faithful.
    // =========================================================================

    #[test]
    fn ceil_log2_pal_matches_c_definition() {
        assert_eq!(ceil_log2_pal(0), 0);
        assert_eq!(ceil_log2_pal(1), 0);
        assert_eq!(ceil_log2_pal(2), 1);
        assert_eq!(ceil_log2_pal(3), 2);
        assert_eq!(ceil_log2_pal(256), 8);
        assert_eq!(ceil_log2_pal(257), 9);
    }

    #[test]
    fn get_unsigned_bits_matches_c_definition() {
        // get_msb(n)+1 for n>0, i.e. floor(log2(n))+1; 0 for n==0.
        assert_eq!(get_unsigned_bits(0), 0);
        assert_eq!(get_unsigned_bits(1), 1);
        assert_eq!(get_unsigned_bits(2), 2);
        assert_eq!(get_unsigned_bits(3), 2);
        assert_eq!(get_unsigned_bits(4), 3);
        assert_eq!(get_unsigned_bits(8), 4);
    }

    /// C `write_uniform` (entropy_coding.c:4294-4306) hand-computed: n=6
    /// (palette size), l=get_unsigned_bits(6)=3, m=(1<<3)-6=2.
    #[test]
    fn write_uniform_hand_vectors() {
        // v=0 < m=2 -> literal(0, l-1=2): 2 bits "00".
        let mut w = AomWriter::new(64);
        write_uniform(&mut w, 6, 0);
        let out0 = w.done().to_vec();
        // v=1 < m=2 -> literal(1, 2): 2 bits "01".
        let mut w = AomWriter::new(64);
        write_uniform(&mut w, 6, 1);
        let out1 = w.done().to_vec();
        assert_ne!(out0, out1, "distinct v < m must produce distinct bits");
        // v=5 >= m=2 -> literal(m+((v-m)>>1), 2) then literal((v-m)&1, 1)
        // = literal(2+1,2)=literal(3,2) then literal(1,1) — 3 bits total,
        // vs 2 bits for v<m: just check it doesn't panic and emits output.
        let mut w = AomWriter::new(64);
        write_uniform(&mut w, 6, 5);
        assert!(!w.done().is_empty());
        // n=0 (never a real palette size, but write_uniform must no-op
        // rather than panic — l=0 early return).
        let mut w = AomWriter::new(64);
        write_uniform(&mut w, 0, 0);
        let _ = w.done();
    }

    /// C `delta_encode_palette_colors` (entropy_coding.c:4256-4288)
    /// self-consistency: must not panic across the size range and must
    /// depend on every input color (changing one color changes the bits).
    #[test]
    fn write_delta_encoded_colors_hand_consistency() {
        let mut w = AomWriter::new(64);
        write_delta_encoded_colors(&mut w, &[10u16, 12, 20, 21], 8, 1);
        let out_a = w.done().to_vec();
        let mut w = AomWriter::new(64);
        write_delta_encoded_colors(&mut w, &[10u16, 12, 20, 22], 8, 1);
        let out_b = w.done().to_vec();
        assert_ne!(out_a, out_b, "changing a color must change the coded bits");
        // Single-color and empty inputs must not panic (num<=1 early return).
        let mut w = AomWriter::new(64);
        write_delta_encoded_colors(&mut w, &[10u16], 8, 1);
        let _ = w.done();
        let mut w = AomWriter::new(64);
        write_delta_encoded_colors(&mut w, &[], 8, 1);
        let _ = w.done();
    }

    /// Edge case (exactly one neighbor), all three sub-branches, both
    /// orientations — identical vectors to
    /// `palette_color_index_context_edge_hand_vectors` in
    /// `svtav1-encoder/tests/c_parity_palette.rs` (dropped the trailing
    /// `palette_size` arg, which this fn's caller asserts instead).
    #[test]
    fn palette_map_pixel_ctx_edge_hand_vectors() {
        let map = [10u8, 3, 10, 10];
        assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 1), (0, 4));
        assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 2), (0, 10));
        assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 3), (0, 0));

        let map = [7u8, 2, 7, 7];
        assert_eq!(palette_map_pixel_ctx(&map, 1, 1, 0), (0, 3));
        assert_eq!(palette_map_pixel_ctx(&map, 1, 2, 0), (0, 7));
        assert_eq!(palette_map_pixel_ctx(&map, 1, 3, 0), (0, 0));
    }

    /// Interior, all three neighbors distinct — identical vectors to
    /// `palette_color_index_context_interior_all_distinct_hand_vectors`.
    #[test]
    fn palette_map_pixel_ctx_interior_all_distinct_hand_vectors() {
        let stride = 2usize;
        let mk = |current: u8| alloc::vec![9, 3, 5, current];
        assert_eq!(palette_map_pixel_ctx(&mk(3), stride, 1, 1), (1, 0));
        assert_eq!(palette_map_pixel_ctx(&mk(7), stride, 1, 1), (1, 8));
        assert_eq!(palette_map_pixel_ctx(&mk(1), stride, 1, 1), (1, 4));
    }

    /// Interior, left==top only — identical vectors to
    /// `palette_color_index_context_interior_left_eq_top_hand_vectors`.
    #[test]
    fn palette_map_pixel_ctx_interior_left_eq_top_hand_vectors() {
        let stride = 2usize;
        let mk = |current: u8| alloc::vec![9, 4, 4, current];
        assert_eq!(palette_map_pixel_ctx(&mk(4), stride, 1, 1), (3, 0));
        assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (3, 1));
        assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (3, 4));
    }

    /// Interior, left==topleft only — identical vectors to
    /// `palette_color_index_context_interior_left_eq_topleft_hand_vectors`.
    #[test]
    fn palette_map_pixel_ctx_interior_left_eq_topleft_hand_vectors() {
        let stride = 2usize;
        let mk = |current: u8| alloc::vec![6, 2, 6, current];
        assert_eq!(palette_map_pixel_ctx(&mk(6), stride, 1, 1), (2, 0));
        assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (2, 1));
        assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (2, 9));
    }

    /// Interior, top==topleft only — identical vectors to
    /// `palette_color_index_context_interior_top_eq_topleft_hand_vectors`.
    #[test]
    fn palette_map_pixel_ctx_interior_top_eq_topleft_hand_vectors() {
        let stride = 2usize;
        let mk = |current: u8| alloc::vec![7, 7, 1, current];
        assert_eq!(palette_map_pixel_ctx(&mk(7), stride, 1, 1), (2, 0));
        assert_eq!(palette_map_pixel_ctx(&mk(1), stride, 1, 1), (2, 1));
        assert_eq!(palette_map_pixel_ctx(&mk(0), stride, 1, 1), (2, 2));
    }

    /// Interior, all three neighbors equal — identical vectors to
    /// `palette_color_index_context_interior_all_equal_hand_vectors`.
    #[test]
    fn palette_map_pixel_ctx_interior_all_equal_hand_vectors() {
        let stride = 2usize;
        let mk = |current: u8| alloc::vec![5, 5, 5, current];
        assert_eq!(palette_map_pixel_ctx(&mk(5), stride, 1, 1), (4, 0));
        assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (4, 3));
        assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (4, 9));
    }

    /// `write_palette_mode_info` / `write_palette_map_tokens` end-to-end
    /// smoke test: a synthetic 4-color 2x2 map must round-trip through the
    /// writer without panicking, and the `Some` arm must code MORE symbols
    /// (hence produce different bytes) than the `None` (no-palette) arm on
    /// the same block — a cheap shape lock. #71 injection now exercises this
    /// end-to-end on the EPICA cell, which is not yet byte-matched (over-
    /// picking, #71); this test guards the writer shape independent of that.
    #[test]
    fn write_palette_mode_info_some_vs_none_arm() {
        let mut fc = FrameContext::new_default();
        let mut w = AomWriter::new(256);
        write_palette_mode_info(&mut w, &mut fc, 8, 8, 0, 0, true, 0, None);
        let none_bytes = w.done().to_vec();

        let mut fc2 = FrameContext::new_default();
        let mut w2 = AomWriter::new(256);
        let colors: [u16; 2] = [10, 20];
        let cache_found: [bool; 0] = [];
        let out_of_cache: [u16; 2] = [10, 20];
        write_palette_mode_info(
            &mut w2,
            &mut fc2,
            8,
            8,
            0,
            0,
            true,
            0,
            Some((&colors, &cache_found, &out_of_cache)),
        );
        // Map: 2x2, 2 colors, anti-diagonal pixels (1,0) and (0,1) each
        // code one context'd symbol on top of the (0,0) write_uniform.
        let map = [0u8, 1, 1, 0];
        write_palette_map_tokens(&mut w2, &mut fc2, &map, 2, 2, 2, 2);
        let some_bytes = w2.done().to_vec();

        assert_ne!(none_bytes, some_bytes, "palette Some arm must code additional symbols");
    }

    /// Hand-derived vector for the frame-edge partition CDF gathers.
    ///
    /// `partition_gather_{horz,vert}_alike` are `static INLINE` in a C header
    /// (cabac_context_model.h:378-406), so they are NOT reachable through FFI —
    /// per the evidence hierarchy they get a hand-computed vector traced from
    /// the C source, plus the end-to-end partial-SB identity gate.
    ///
    /// Synthetic 10-symbol inverse CDF with per-symbol Q15 probabilities
    /// P = [3000, 4000, 2000, 5000, 1000, 2000, 3000, 4000, 6000, 2768]
    /// (sum 32768), stored as `icdf[i] = 32768 - cumulative(i+1)`.
    #[test]
    fn partition_edge_cdf_gathers_match_c_formula() {
        let icdf: [AomCdfProb; 11] = [
            29768, 25768, 23768, 18768, 17768, 15768, 12768, 8768, 2768, 0, 0,
        ];

        // cdf_element_prob recovers the per-symbol probabilities exactly.
        let mut probs = [0u16; 10];
        for (e, slot) in probs.iter_mut().enumerate() {
            *slot = cdf_element_prob(&icdf, e);
        }
        assert_eq!(probs, [3000, 4000, 2000, 5000, 1000, 2000, 3000, 4000, 6000, 2768]);

        // horz_alike: subtract P(HORZ=1) + P(SPLIT=3) + P(HORZ_A=4)
        //           + P(HORZ_B=5) + P(VERT_A=6) + P(HORZ_4=8)
        //           = 4000+5000+1000+2000+3000+6000 = 21000
        // v = 32768 - 21000 = 11768; out[0] = AOM_ICDF(v) = 32768 - 11768 = 21000
        let mut out = [0 as AomCdfProb; 3];
        partition_gather_horz_alike(&mut out, &icdf, false);
        assert_eq!(out, [21000, 0, 0]);

        // vert_alike: P(VERT=2) + P(SPLIT=3) + P(HORZ_A=4)
        //           + P(VERT_A=6) + P(VERT_B=7) + P(VERT_4=9)
        //           = 2000+5000+1000+3000+4000+2768 = 17768
        // v = 32768 - 17768 = 15000; out[0] = 32768 - 15000 = 17768
        partition_gather_vert_alike(&mut out, &icdf, false);
        assert_eq!(out, [17768, 0, 0]);

        // is_128 drops the *_4 term from each (C `bsize != BLOCK_128X128`).
        partition_gather_horz_alike(&mut out, &icdf, true);
        assert_eq!(out, [15000, 0, 0]);
        partition_gather_vert_alike(&mut out, &icdf, true);
        assert_eq!(out, [15000, 0, 0]);
    }

    /// The both-false case codes NOTHING (forced SPLIT), and the interior case
    /// is bit-identical to the plain `write_partition` path — the property that
    /// keeps 64-aligned frames byte-unchanged.
    #[test]
    fn write_partition_edge_interior_matches_plain_and_offframe_codes_nothing() {
        // Interior (has_rows && has_cols) == plain write_partition.
        let mut fc_a = FrameContext::new_default();
        let mut w_a = AomWriter::new(256);
        write_partition(&mut w_a, &mut fc_a, 5, 3, 10);
        let plain = w_a.done().to_vec();

        let mut fc_b = FrameContext::new_default();
        let mut w_b = AomWriter::new(256);
        write_partition_edge(&mut w_b, &mut fc_b, 5, 3, 10, false, true, true);
        let edge_interior = w_b.done().to_vec();
        assert_eq!(plain, edge_interior, "interior edge-write must match write_partition");
        assert_eq!(
            fc_a.partition_cdf[5], fc_b.partition_cdf[5],
            "interior edge-write must adapt the CDF identically"
        );

        // Both-false: forced SPLIT, no symbol, and the CDF is NOT adapted.
        let mut fc_c = FrameContext::new_default();
        let before = fc_c.partition_cdf[5];
        let mut w_c = AomWriter::new(256);
        write_partition_edge(&mut w_c, &mut fc_c, 5, 3, 10, false, false, false);
        let nothing = w_c.done().to_vec();
        let mut w_empty = AomWriter::new(256);
        let empty = w_empty.done().to_vec();
        assert_eq!(nothing, empty, "off-frame quadrant must code no partition symbol");
        assert_eq!(before, fc_c.partition_cdf[5], "forced-SPLIT must not adapt the CDF");

        // Binary arm: must NOT adapt the persistent CDF (C gathers on the stack).
        let mut fc_d = FrameContext::new_default();
        let before_d = fc_d.partition_cdf[5];
        let mut w_d = AomWriter::new(256);
        write_partition_edge(&mut w_d, &mut fc_d, 5, 3, 10, false, true, false);
        assert_eq!(
            before_d, fc_d.partition_cdf[5],
            "gathered binary arm must leave the frame-context CDF untouched"
        );
    }
}

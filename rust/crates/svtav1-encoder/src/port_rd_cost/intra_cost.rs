//! The MDS0 rate of an intra candidate — `svt_aom_intra_fast_cost`
//! (rd_cost.c:526) and the chroma half it calls,
//! `svt_aom_get_intra_uv_fast_rate` (:476).
//!
//! # Two sub-rates are INPUTS, and why
//!
//! C's luma-palette arm calls four more functions (`svt_get_palette_cache_y`,
//! `svt_aom_write_uniform_cost`, `svt_av1_palette_color_cost_y`,
//! `svt_av1_cost_color_map`) whose counterparts already exist in this port,
//! spread across `palette.rs`, `pipeline.rs` and `entropy::context`. A second
//! copy here would be a silently diverging one, so [`IntraCandidate`] takes
//! the assembled `palette_mode_cost` and the two palette CONTEXTS as inputs —
//! the same treatment `port_entropy_inter::framesize` gives its sub-writers.
//! Everything else in both functions is ported here.

use svtav1_types::block::BlockSize;
use svtav1_types::motion::Mv;

use crate::entropy::context::{
    ANGLE_DELTA_SYMS, BLOCK_SIZE_GROUPS, BLOCK_SIZES_ALL, DIRECTIONAL_MODES, INTRA_INTER_CONTEXTS,
    INTRA_MODES, KF_MODE_CONTEXTS, SKIP_MODE_CONTEXTS, UV_INTRA_MODES,
};
use crate::port_entropy_inter::refframe::is_comp_ref_allowed;
use crate::port_md::pme::MvCostTable;
use crate::port_rd_cost::{MV_COST_WEIGHT_SUB, rdcost};

/// C `CFL_ALLOWED_TYPES` (definitions.h:1142).
pub const CFL_ALLOWED_TYPES: usize = 2;
/// C `CFL_PRED_PLANES` (definitions.h:1138).
pub const CFL_PRED_PLANES: usize = 2;
/// C `CFL_JOINT_SIGNS` (definitions.h:1145): `CFL_SIGNS * CFL_SIGNS - 1`.
pub const CFL_JOINT_SIGNS: usize = 8;
/// C `CFL_ALPHABET_SIZE` (definitions.h:1134).
pub const CFL_ALPHABET_SIZE: usize = 16;
/// C `PALATTE_BSIZE_CTXS` (cabac_context_model.h:260) — C's spelling.
pub const PALETTE_BSIZE_CTXS: usize = 7;
/// C `PALETTE_Y_MODE_CONTEXTS` (cabac_context_model.h:246).
pub const PALETTE_Y_MODE_CONTEXTS: usize = 3;
/// C `PALETTE_UV_MODE_CONTEXTS` (cabac_context_model.h:252).
pub const PALETTE_UV_MODE_CONTEXTS: usize = 2;
/// C `MAX_ANGLE_DELTA` (definitions.h:1327).
pub const MAX_ANGLE_DELTA: i32 = 3;
/// C `UV_CFL_PRED` (definitions.h:1246) — the last `UvPredictionMode`.
pub const UV_CFL_PRED: u8 = 13;
/// C `UV_DC_PRED`.
pub const UV_DC_PRED: u8 = 0;
/// C `DC_PRED`.
const DC_PRED: u8 = 0;
/// C `V_PRED` — the first directional mode, and the base of the
/// `angle_delta_fac_bits` row index.
const V_PRED: u8 = 1;

/// C `g_uv2y` (common_utils.c:14-30) restricted to the 14 real UV modes:
/// each `UvPredictionMode` maps to the luma mode whose directionality it
/// shares. `UV_CFL_PRED` maps to `DC_PRED`, which is why a CFL block is NOT
/// directional and pays no angle delta.
pub const UV_TO_Y_MODE: [u8; UV_INTRA_MODES] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0];

/// C `av1_is_directional_mode` (common_utils.h:126): `V_PRED..=D67_PRED`.
#[inline]
pub fn is_directional_mode(mode: u8) -> bool {
    (V_PRED..=8).contains(&mode)
}

// ---------------------------------------------------------------------------
// Rate tables
// ---------------------------------------------------------------------------

/// The `MdRateEstimationContext` rows the intra fast cost reads
/// (md_rate_estimation.h:93-109), in C's shapes.
#[derive(Debug, Clone)]
pub struct IntraFacBits {
    /// C `y_mode_fac_bits[KF_MODE_CONTEXTS][KF_MODE_CONTEXTS][INTRA_MODES]` —
    /// the KEY-frame luma mode, indexed by the two neighbour contexts.
    pub y_mode: [[[i32; INTRA_MODES]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS],
    /// C `mb_mode_fac_bits[BlockSize_GROUPS][INTRA_MODES]` — the
    /// INTER-frame luma mode.
    pub mb_mode: [[i32; INTRA_MODES]; BLOCK_SIZE_GROUPS],
    /// C `intra_uv_mode_fac_bits[CFL_ALLOWED_TYPES][INTRA_MODES][UV_INTRA_MODES]`.
    pub intra_uv_mode: [[[i32; UV_INTRA_MODES]; INTRA_MODES]; CFL_ALLOWED_TYPES],
    /// C `angle_delta_fac_bits[DIRECTIONAL_MODES][2 * MAX_ANGLE_DELTA + 1]`.
    pub angle_delta: [[i32; ANGLE_DELTA_SYMS]; DIRECTIONAL_MODES],
    /// C `cfl_alpha_fac_bits[CFL_JOINT_SIGNS][CFL_PRED_PLANES][CFL_ALPHABET_SIZE]`.
    pub cfl_alpha: [[[i32; CFL_ALPHABET_SIZE]; CFL_PRED_PLANES]; CFL_JOINT_SIGNS],
    /// C `filter_intra_fac_bits[BLOCK_SIZES_ALL][2]`.
    pub filter_intra: [[i32; 2]; BLOCK_SIZES_ALL],
    /// C `filter_intra_mode_fac_bits[FILTER_INTRA_MODES]`.
    pub filter_intra_mode: [i32; 5],
    /// C `palette_ymode_fac_bits[PALATTE_BSIZE_CTXS][PALETTE_Y_MODE_CONTEXTS][2]`.
    pub palette_ymode: [[[i32; 2]; PALETTE_Y_MODE_CONTEXTS]; PALETTE_BSIZE_CTXS],
    /// C `palette_uv_mode_fac_bits[PALETTE_UV_MODE_CONTEXTS][2]`.
    pub palette_uv_mode: [[i32; 2]; PALETTE_UV_MODE_CONTEXTS],
    /// C `intra_inter_fac_bits[INTRA_INTER_CONTEXTS][2]`.
    pub intra_inter: [[i32; 2]; INTRA_INTER_CONTEXTS],
    /// C `skip_mode_fac_bits[SKIP_CONTEXTS][2]`.
    pub skip_mode: [[i32; 2]; SKIP_MODE_CONTEXTS],
    /// C `intrabc_fac_bits[2]`.
    pub intrabc: [i32; 2],
}

impl Default for IntraFacBits {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl IntraFacBits {
    /// All-zero tables — the shape a caller fills from a frame context.
    pub fn zeroed() -> Self {
        Self {
            y_mode: [[[0; INTRA_MODES]; KF_MODE_CONTEXTS]; KF_MODE_CONTEXTS],
            mb_mode: [[0; INTRA_MODES]; BLOCK_SIZE_GROUPS],
            intra_uv_mode: [[[0; UV_INTRA_MODES]; INTRA_MODES]; CFL_ALLOWED_TYPES],
            angle_delta: [[0; ANGLE_DELTA_SYMS]; DIRECTIONAL_MODES],
            cfl_alpha: [[[0; CFL_ALPHABET_SIZE]; CFL_PRED_PLANES]; CFL_JOINT_SIGNS],
            filter_intra: [[0; 2]; BLOCK_SIZES_ALL],
            filter_intra_mode: [0; 5],
            palette_ymode: [[[0; 2]; PALETTE_Y_MODE_CONTEXTS]; PALETTE_BSIZE_CTXS],
            palette_uv_mode: [[0; 2]; PALETTE_UV_MODE_CONTEXTS],
            intra_inter: [[0; 2]; INTRA_INTER_CONTEXTS],
            skip_mode: [[0; 2]; SKIP_MODE_CONTEXTS],
            intrabc: [0; 2],
        }
    }
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// The frame- and block-level context the intra fast cost reads.
#[derive(Debug, Clone, Copy)]
pub struct IntraBlock {
    /// C `ctx->blk_geom->bsize`.
    pub bsize: BlockSize,
    /// C `ctx->blk_geom->bwidth` / `bheight`, in pixels.
    pub bwidth: u16,
    pub bheight: u16,
    /// C `pcs->slice_type == I_SLICE`.
    pub is_key_slice: bool,
    /// C `svt_aom_allow_intrabc(&frm_hdr, slice_type)`.
    pub allow_intrabc: bool,
    /// C `frm_hdr.allow_screen_content_tools`.
    pub allow_screen_content_tools: bool,
    /// C `frm_hdr.skip_mode_params.skip_mode_flag`.
    pub skip_mode_flag: bool,
    /// C `scs->seq_header.filter_intra_level`.
    pub filter_intra_level: u8,
    /// C `ctx->has_uv` — whether this block codes chroma at all.
    pub has_uv: bool,
    /// C `ctx->intra_luma_top_ctx` / `intra_luma_left_ctx` (key frames only).
    pub intra_luma_top_ctx: usize,
    pub intra_luma_left_ctx: usize,
    /// C `ctx->is_inter_ctx`.
    pub is_inter_ctx: usize,
    /// C `ctx->skip_mode_ctx`.
    pub skip_mode_ctx: usize,
    /// C `svt_aom_get_palette_bsize_ctx(bsize)` — see the module doc.
    pub palette_bsize_ctx: usize,
    /// C `svt_aom_get_palette_mode_ctx(xd)` — see the module doc.
    pub palette_mode_ctx: usize,
    /// C `ctx->blk_org_y >> MI_SIZE_LOG2` / `blk_org_x >> MI_SIZE_LOG2`.
    pub mi_row: i32,
    pub mi_col: i32,
}

/// C `cand->palette_size[0]` / `[1]`, present only when `palette_info` is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaletteSizes {
    pub y: u8,
    pub uv: u8,
}

/// The candidate fields the intra fast cost reads.
#[derive(Debug, Clone, Copy)]
pub struct IntraCandidate {
    /// C `block_mi.mode` (a luma `PredictionMode` discriminant).
    pub mode: u8,
    /// C `block_mi.uv_mode` (a `UvPredictionMode` discriminant).
    pub uv_mode: u8,
    /// C `block_mi.angle_delta[PLANE_TYPE_Y]` / `[PLANE_TYPE_UV]`, each in
    /// `-MAX_ANGLE_DELTA..=MAX_ANGLE_DELTA`.
    pub angle_delta_y: i32,
    pub angle_delta_uv: i32,
    /// C `block_mi.cfl_alpha_signs` / `cfl_alpha_idx`.
    pub cfl_alpha_signs: u8,
    pub cfl_alpha_idx: u8,
    /// C `block_mi.filter_intra_mode`; `FILTER_INTRA_MODES` (5) means "off".
    pub filter_intra_mode: u8,
    /// C `cand->palette_info` and the two sizes behind it, as one value.
    ///
    /// `None` is C's NULL `palette_info` — and that pointer is checked
    /// SEPARATELY from the sizes at every use (`cand->palette_info &&
    /// (cand->palette_size[0] > 0)`), so a null pointer with a non-zero size
    /// reads as "no palette". Folding the two into an `Option` makes that
    /// unrepresentable instead of merely documented.
    pub palette: Option<PaletteSizes>,
    /// C's assembled `palette_mode_cost` (rd_cost.c:583-598) — the ysize
    /// symbol, the uniform first-index cost, the color cost and the color-map
    /// cost. Read only when `palette_size_y > 0`. See the module doc.
    pub palette_mode_cost: u64,
    /// C `block_mi.use_intrabc`.
    pub use_intrabc: bool,
    /// C `block_mi.mv[0]` and `cand->pred_mv[0]` — the IntraBC block vector
    /// and its predictor.
    pub mv: Mv,
    pub pred_mv: Mv,
}

/// What C leaves in `cand_bf->fast_luma_rate` / `fast_chroma_rate`, plus the
/// cost it returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntraFastCost {
    pub cost: u64,
    pub fast_luma_rate: u64,
    pub fast_chroma_rate: u64,
}

// ---------------------------------------------------------------------------
// svt_aom_get_intra_uv_fast_rate (rd_cost.c:476)
// ---------------------------------------------------------------------------

/// C `svt_aom_get_intra_uv_fast_rate` (rd_cost.c:476, EXPORTED).
///
/// `use_accurate_cfl` is C's flag: at MDS0 the CFL alphas are not known yet,
/// so a `UV_CFL_PRED` candidate is PRICED AS `UV_DC_PRED` and re-priced later.
/// That substitution also decides whether the palette-off arm below is
/// reached, so it is not merely a rate approximation.
///
/// The subsampling is hardwired to 4:2:0 here exactly as in C ("Subsampling
/// assumes YUV 420 content", :491) — which is also the only format C's
/// `verify_settings` accepts.
pub fn get_intra_uv_fast_rate(
    block: &IntraBlock,
    cand: &IntraCandidate,
    use_accurate_cfl: bool,
    t: &IntraFacBits,
) -> u64 {
    debug_assert!(block.has_uv);
    let is_cfl_allowed = usize::from(block.bwidth <= 32 && block.bheight <= 32);
    let chroma_mode = if cand.uv_mode == UV_CFL_PRED && !use_accurate_cfl {
        UV_DC_PRED
    } else {
        cand.uv_mode
    };

    let mut chroma_rate =
        t.intra_uv_mode[is_cfl_allowed][cand.mode as usize][chroma_mode as usize] as u64;

    // Angular offset: only for bsize >= BLOCK_8X8 in ENUM order (which keeps
    // the 4:1 rects, indices 16..21, eligible) and only for a directional
    // LUMA-equivalent mode, so CFL (which maps to DC) never pays it.
    if block.bsize.as_index() >= BlockSize::Block8x8.as_index()
        && is_directional_mode(UV_TO_Y_MODE[chroma_mode as usize])
    {
        let row = (chroma_mode - V_PRED) as usize;
        let col = (MAX_ANGLE_DELTA + cand.angle_delta_uv) as usize;
        chroma_rate += t.angle_delta[row][col] as u64;
    }

    if chroma_mode == UV_CFL_PRED {
        let signs = cand.cfl_alpha_signs as usize;
        // C `CFL_IDX_U(idx)` = `idx >> 4`, `CFL_IDX_V(idx)` = `idx & 15`.
        let idx_u = (cand.cfl_alpha_idx >> 4) as usize;
        let idx_v = (cand.cfl_alpha_idx & 0x0f) as usize;
        chroma_rate += t.cfl_alpha[signs][0][idx_u] as u64 + t.cfl_alpha[signs][1][idx_v] as u64;
    }

    // Chroma palette is not searched, so only the "off" symbol is priced —
    // and only on a chroma-reference block that could have coded one.
    if chroma_mode == UV_DC_PRED
        && crate::entropy::context::allow_palette(
            block.allow_screen_content_tools,
            block.bwidth as usize,
            block.bheight as usize,
        )
        && crate::intrabc::is_chroma_reference(
            block.mi_row,
            block.mi_col,
            i32::from(block.bwidth) >> 2,
            i32::from(block.bheight) >> 2,
            1,
            1,
        )
    {
        let use_palette_y = usize::from(cand.palette.is_some_and(|p| p.y > 0));
        let use_palette_uv = usize::from(cand.palette.is_some_and(|p| p.uv > 0));
        chroma_rate += t.palette_uv_mode[use_palette_y][use_palette_uv] as u64;
    }

    chroma_rate
}

// ---------------------------------------------------------------------------
// svt_aom_intra_fast_cost (rd_cost.c:526)
// ---------------------------------------------------------------------------

/// C `svt_aom_filter_intra_allowed` (mode_decision.c:107, EXPORTED).
#[inline]
pub fn filter_intra_allowed(
    filter_intra_level: u8,
    bsize: BlockSize,
    palette_size: u8,
    mode: u8,
) -> bool {
    use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};
    let i = bsize.as_index();
    filter_intra_level != 0
        && mode == DC_PRED
        && palette_size == 0
        && BLOCK_SIZE_WIDE[i] <= 32
        && BLOCK_SIZE_HIGH[i] <= 32
}

/// C `svt_aom_intra_fast_cost` (rd_cost.c:526, EXPORTED).
///
/// Two arms. The IntraBC arm prices ONLY the block vector plus the
/// `use_intrabc` flag, leaves `fast_chroma_rate` at zero, and returns — an
/// IntraBC candidate pays no mode, no angle, no palette and no chroma rate at
/// MDS0. `dv` is `md_rate_est_ctx->dv_cost` / `dv_joint_cost`, at
/// `MV_COST_WEIGHT_SUB`.
///
/// The intra arm's `is_inter_rate` and `intra_mode_bits_num` are the two
/// terms an I-slice does NOT pay; `intra_luma_mode_bits_num` is the one only
/// an I-slice pays. C reads all three unconditionally from the same tables
/// and zeroes them by slice type, which is why they are separate terms here
/// rather than one branch.
pub fn intra_fast_cost(
    block: &IntraBlock,
    cand: &IntraCandidate,
    lambda: u64,
    luma_distortion: u64,
    dv: Option<&MvCostTable>,
    t: &IntraFacBits,
) -> IntraFastCost {
    if block.allow_intrabc && cand.use_intrabc {
        let mv_rate = match dv {
            Some(tables) => {
                // C `svt_av1_mv_bit_cost` (rd_cost.c:70): the table lookup
                // scaled by the weight and rounded at RDDIV_BITS.
                u64::try_from(crate::port_md::drl::mv_bit_cost(
                    cand.mv,
                    cand.pred_mv,
                    tables,
                    MV_COST_WEIGHT_SUB,
                ))
                .unwrap_or(0)
            }
            // C always has the dv tables built when IntraBC is allowed; a
            // caller with none is asking for the table-free rate, which is 0.
            None => 0,
        };
        let rate = mv_rate + t.intrabc[usize::from(cand.use_intrabc)] as u64;
        return IntraFastCost {
            cost: rdcost(lambda, rate, luma_distortion),
            fast_luma_rate: rate,
            fast_chroma_rate: 0,
        };
    }

    let bi = block.bsize.as_index();
    let group = crate::port_rd_cost::inter_cost::SIZE_GROUP_LOOKUP[bi] as usize;

    // Non-key frames code the luma mode from `mb_mode_fac_bits`; key frames
    // code it from the neighbour-conditioned `y_mode_fac_bits`.
    let intra_mode_bits = if block.is_key_slice {
        0
    } else {
        t.mb_mode[group][cand.mode as usize] as u64
    };
    let mut intra_luma_mode_bits = if block.is_key_slice {
        t.y_mode[block.intra_luma_top_ctx][block.intra_luma_left_ctx][cand.mode as usize] as u64
    } else {
        0
    };

    let skip_mode_rate =
        if !block.is_key_slice && block.skip_mode_flag && is_comp_ref_allowed(block.bsize) {
            t.skip_mode[block.skip_mode_ctx][0] as u64
        } else {
            0
        };

    let intra_luma_ang_mode_bits = if block.bsize.as_index() >= BlockSize::Block8x8.as_index()
        && is_directional_mode(cand.mode)
    {
        let row = (cand.mode - V_PRED) as usize;
        let col = (MAX_ANGLE_DELTA + cand.angle_delta_y) as usize;
        t.angle_delta[row][col] as u64
    } else {
        0
    };

    if crate::entropy::context::allow_palette(
        block.allow_screen_content_tools,
        block.bwidth as usize,
        block.bheight as usize,
    ) && cand.mode == DC_PRED
    {
        let use_palette = usize::from(cand.palette.is_some_and(|p| p.y > 0));
        intra_luma_mode_bits +=
            t.palette_ymode[block.palette_bsize_ctx][block.palette_mode_ctx][use_palette] as u64;
        if use_palette == 1 {
            intra_luma_mode_bits += cand.palette_mode_cost;
        }
    }

    let intra_filter_mode_bits = if filter_intra_allowed(
        block.filter_intra_level,
        block.bsize,
        // C `cand->palette_info ? cand->palette_size[0] : 0`.
        cand.palette.map_or(0, |p| p.y),
        cand.mode,
    ) {
        let on = cand.filter_intra_mode != FILTER_INTRA_MODES;
        let mut bits = t.filter_intra[bi][usize::from(on)] as u64;
        if on {
            bits += t.filter_intra_mode[cand.filter_intra_mode as usize] as u64;
        }
        bits
    } else {
        0
    };

    let chroma_rate = if block.has_uv {
        get_intra_uv_fast_rate(block, cand, false, t)
    } else {
        0
    };

    let is_inter_rate = if block.is_key_slice {
        0
    } else {
        t.intra_inter[block.is_inter_ctx][0] as u64
    };

    // C accumulates into a `uint32_t luma_rate`.
    let mut luma_rate = (intra_mode_bits
        + skip_mode_rate
        + intra_luma_mode_bits
        + intra_luma_ang_mode_bits
        + is_inter_rate
        + intra_filter_mode_bits) as u32;
    if block.allow_intrabc {
        // Reached only with `use_intrabc == 0` (the IntraBC arm returned
        // above), so this is always the "IntraBC off" symbol.
        luma_rate = luma_rate.wrapping_add(t.intrabc[0] as u32);
    }

    let rate = u64::from(luma_rate) + chroma_rate;
    IntraFastCost {
        cost: rdcost(lambda, rate, luma_distortion),
        fast_luma_rate: u64::from(luma_rate),
        fast_chroma_rate: chroma_rate,
    }
}

/// C `FILTER_INTRA_MODES` (definitions.h:1322) — the "no filter intra"
/// sentinel stored in `filter_intra_mode`.
pub const FILTER_INTRA_MODES: u8 = 5;

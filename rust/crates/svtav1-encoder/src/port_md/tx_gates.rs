//! The transform-search GATING of `Source/Lib/Codec/product_coding_loop.c`:
//! how deep the TX-size search goes, how many TX-type groups it tries, when
//! it collapses to DCT_DCT alone, and what a TX type costs to signal.
//!
//! | this module | C |
//! |---|---|
//! | [`get_end_tx_depth`] | `:4100-4112` |
//! | [`get_start_end_tx_depth`] | `:6698-6737` |
//! | [`get_tx_type_group`] | `:4287-4307` |
//! | [`search_dct_dct_only`] | `:4523-4551` |
//! | [`txt_rate_source`] | `av1_txt_rate_est` `:4553-4576` |
//!
//! # What the intra funnel cannot express
//!
//! [`crate::leaf_funnel::txt`] and [`crate::leaf_funnel::tx_geom`] carry the
//! reachable slice of these for the all-intra funnel, and both say so:
//! `tx_geom::end_tx_depth` hardcodes the INTRA depth caps, and `txt`'s
//! comment records that it reuses the intra group counts because "at every
//! IBC preset the C inter group counts EQUAL the intra ones". Off an
//! I-slice neither holds:
//!
//! * `get_tx_type_group` picks between FOUR fields — intra/inter x
//!   `< 16x16`/`>= 16x16` (`:4294-4298`). The inter pair is a separate
//!   config (`set_txt_controls`) and only coincides with the intra pair at
//!   the IBC presets.
//! * `get_start_end_tx_depth` clamps with `inter_class_max_depth_sq/nsq`
//!   for an inter mode and `intra_class_..` otherwise (`:6730-6732`), and
//!   its two EARLY arms — `!mds_do_txs` pinning the depth to the
//!   candidate's own `tx_depth`, and the `bypass_tx_th` shortcut — have no
//!   intra-funnel counterpart at all.
//! * `search_dct_dct_only` short-circuits on `use_tx_shortcuts_mds3` and on
//!   the same `bypass_tx_th` test (`:4530-4537`), both of which read MDS1
//!   state the intra funnel does not keep.
//! * `av1_txt_rate_est` reads `inter_tx_type_fac_bits` for an inter mode
//!   and `intra_tx_type_fac_bits[..][intra_dir]` otherwise (`:4564-4573`).
//!
//! # Evidence
//!
//! **Tier 4 throughout** — all five are `static` (or `INLINE`) in C with no
//! exported symbol (`docs/WORKING-ON-THIS.md` §4). The `get_ext_tx_set` /
//! `get_ext_tx_types` these call are the port's already-gated
//! [`crate::entropy::coeff_c`] versions rather than a second transcription.
//!
//! # Reachability
//!
//! Nothing calls this yet — the public entry point still refuses inter
//! frames (`docs/WORKING-ON-THIS.md` §7).

use crate::entropy::coeff_c as cc;

/// C `TxtControls` (md_process.h:140-170), the fields the gates read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxtControls {
    pub enabled: bool,
    /// `txt_group_inter_lt_16x16`
    pub group_inter_lt_16x16: i32,
    /// `txt_group_inter_gt_eq_16x16`
    pub group_inter_gt_eq_16x16: i32,
    /// `txt_group_intra_lt_16x16`
    pub group_intra_lt_16x16: i32,
    /// `txt_group_intra_gt_eq_16x16`
    pub group_intra_gt_eq_16x16: i32,
}

/// C `TxsControls` (md_process.h:597-617), the fields the gates read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TxsControls {
    pub enabled: bool,
    /// `intra_class_max_depth_sq`
    pub intra_class_max_depth_sq: u8,
    /// `intra_class_max_depth_nsq`
    pub intra_class_max_depth_nsq: u8,
    /// `inter_class_max_depth_sq`
    pub inter_class_max_depth_sq: u8,
    /// `inter_class_max_depth_nsq`
    pub inter_class_max_depth_nsq: u8,
    /// `depth1_txt_group_offset`
    pub depth1_txt_group_offset: i32,
    /// `depth2_txt_group_offset`
    pub depth2_txt_group_offset: i32,
}

/// C `get_end_tx_depth` (`:4100-4112`), keyed on block DIMENSIONS rather
/// than the `BlockSize` enum.
///
/// C lists fifteen block sizes that reach depth 2 and one (`BLOCK_8X8`)
/// that reaches depth 1; everything else is 0. Written as dimensions the
/// rule is: **every block whose smaller side is at least 8, plus the two
/// 4:1 shapes 16x4 and 4x16, gets 2 — except that 128-wide or 128-high
/// blocks get 0.** The comment C leaves at `:4110` names the zero set
/// (8x4, 4x8, 4x4, 128x128, 128x64, 64x128) and this reproduces it
/// exhaustively rather than by rule, so a future block size cannot slip
/// into the wrong arm silently.
#[must_use]
pub fn get_end_tx_depth(width: usize, height: usize) -> u8 {
    match (width, height) {
        (64, 64) | (32, 32) | (16, 16) => 2,
        (64, 32) | (32, 64) | (16, 32) | (32, 16) | (16, 8) | (8, 16) => 2,
        (64, 16) | (16, 64) | (32, 8) | (8, 32) | (16, 4) | (4, 16) => 2,
        (8, 8) => 1,
        // 8x4, 4x8, 4x4, 128x128, 128x64, 64x128.
        _ => 0,
    }
}

/// The MD-stage state `get_start_end_tx_depth` and `search_dct_dct_only`
/// both consult.
#[derive(Debug, Clone, Copy)]
pub struct TxShortcutState {
    /// C `ctx->perform_mds1`.
    pub perform_mds1: bool,
    /// True when `ctx->md_stage == MD_STAGE_3`.
    pub is_mds3: bool,
    /// C `ctx->use_tx_shortcuts_mds3`.
    pub use_tx_shortcuts_mds3: bool,
    /// C `ctx->tx_shortcut_ctrls.bypass_tx_th` — 0 is off.
    pub bypass_tx_th: u32,
    /// C `cand_bf->block_has_coeff`.
    pub block_has_coeff: bool,
    /// C `cand_bf->luma_fast_dist`.
    pub luma_fast_dist: u64,
    /// C `ctx->qp_index`.
    pub qp_index: u32,
}

impl TxShortcutState {
    /// The distortion test C spells out twice, identically, at `:6722-6723`
    /// and `:4534-4535`: the candidate coded nothing and its fast distortion
    /// is small relative to `area * qp_index`.
    ///
    /// C computes the right-hand side in `uint32_t` (`(uint32_t)(bheight *
    /// bwidth * qp_index)`) and the left in `uint64_t`: `luma_fast_dist` is
    /// `uint64_t`, so `luma_fast_dist * bypass_tx_th` promotes the u32
    /// `bypass_tx_th` to u64 — there is NO 32-bit truncation of the
    /// product. The comparison is `u64 < u64` (the u32 RHS promotes).
    /// At the widest block (128x128) and the highest qindex (255) the
    /// normaliser is 4.2e6, well inside 32 bits.
    #[must_use]
    fn bypass_tx_applies(&self, width: usize, height: usize) -> bool {
        self.bypass_tx_th != 0
            && !self.block_has_coeff
            && self.luma_fast_dist * u64::from(self.bypass_tx_th)
                < u64::from((height * width) as u32) * u64::from(self.qp_index)
    }
}

/// C `get_start_end_tx_depth` (`:6698-6737`).
///
/// Returns `(start_tx_depth, end_tx_depth)`.
///
/// `shape_is_square` is C's `ctx->shape == PART_N`, which selects the `_sq`
/// caps over the `_nsq` ones; `cand_tx_depth` is the candidate's own
/// `block_mi.tx_depth`, which becomes BOTH bounds when the MD stage is not
/// doing a TX-size search.
///
/// The `mimic_only_tx_4x4` arm at `:6734` pins an 8x8 square to depth 1 —
/// note it runs LAST, after the class clamp, so it can raise the end depth
/// back above a cap of 0.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn get_start_end_tx_depth(
    txs: &TxsControls,
    mds_do_txs: bool,
    cand_tx_depth: u8,
    cand_is_inter: bool,
    shape_is_square: bool,
    block: (usize, usize),
    // `origin` is `(blk_org_x, blk_org_y)`; `aligned` is the frame's
    // ALIGNED dimensions (C `pcs->ppcs->aligned_width` / `_height`).
    origin: (u32, u32),
    aligned: (u32, u32),
    state: &TxShortcutState,
    mimic_only_tx_4x4: bool,
    sq_size: usize,
) -> (u8, u8) {
    let (width, height) = block;
    let (mut start, mut end) = if !txs.enabled {
        (0, 0)
    } else if !mds_do_txs {
        (cand_tx_depth, cand_tx_depth)
    } else {
        // A block that overhangs the aligned frame is pinned to depth 0
        // (`:6711-6717`).
        let inside = origin.0 + width as u32 <= aligned.0 && origin.1 + height as u32 <= aligned.1;
        (
            0,
            if inside {
                get_end_tx_depth(width, height)
            } else {
                0
            },
        )
    };

    if state.perform_mds1 && state.is_mds3 && state.bypass_tx_applies(width, height) {
        start = 0;
        end = 0;
    }

    let cap = match (cand_is_inter, shape_is_square) {
        (false, true) => txs.intra_class_max_depth_sq,
        (false, false) => txs.intra_class_max_depth_nsq,
        (true, true) => txs.inter_class_max_depth_sq,
        (true, false) => txs.inter_class_max_depth_nsq,
    };
    end = end.min(cap);

    if mimic_only_tx_4x4 && sq_size == 8 {
        start = 1;
        end = 1;
    }
    (start, end)
}

/// C `get_tx_type_group` (`:4287-4307`).
///
/// `tx_size` is the C `TxSize` index at the depth under test — i.e.
/// `tx_depth_to_tx_size[tx_depth][bsize]`, which the caller already has.
///
/// The four-way intra/inter x size split is the whole point: reusing the
/// intra pair for an inter candidate is correct ONLY where the two configs
/// happen to coincide.
#[must_use]
pub fn get_tx_type_group(
    txt: &TxtControls,
    txs: &TxsControls,
    tx_size: usize,
    tx_depth: u8,
    only_dct_dct: bool,
    is_intra_mode: bool,
) -> i32 {
    let mut group = 1i32;
    if !only_dct_dct {
        let small = cc::TX_SIZE_WIDE[tx_size] < 16 || cc::TX_SIZE_HIGH[tx_size] < 16;
        group = match (is_intra_mode, small) {
            (true, true) => txt.group_intra_lt_16x16,
            (true, false) => txt.group_intra_gt_eq_16x16,
            (false, true) => txt.group_inter_lt_16x16,
            (false, false) => txt.group_inter_gt_eq_16x16,
        };
    }
    // The depth offsets apply even when `only_dct_dct` forced the group to
    // 1 — `MAX(1 - offset, 1)` is 1, so it is inert there, but it is not
    // guarded in C and is not guarded here.
    match tx_depth {
        1 => group = (group - txs.depth1_txt_group_offset).max(1),
        2 => group = (group - txs.depth2_txt_group_offset).max(1),
        _ => {}
    }
    group
}

/// C `search_dct_dct_only` (`:4523-4551`).
///
/// True when the TX-type search collapses to DCT_DCT alone, for any of five
/// independent reasons: the stage is not doing a type search; MDS3 shortcuts
/// are armed; the bypass-TX distortion test passes; the transform is larger
/// than 32 in either dimension; or the extended-TX set for this size and
/// mode class holds a single type.
///
/// The last clause is C's belt-and-braces `get_ext_tx_types(..) == 1 ||
/// get_ext_tx_set(..) == 0`; both are kept because the comment at `:4544`
/// says the second is the one that means "no tx_type is signalled".
#[must_use]
pub fn search_dct_dct_only(
    mds_do_txt: bool,
    state: &TxShortcutState,
    block: (usize, usize),
    tx_size: usize,
    is_inter: bool,
    reduced_tx_set: bool,
) -> bool {
    if !mds_do_txt {
        return true;
    }
    if state.is_mds3 && state.use_tx_shortcuts_mds3 {
        return true;
    }
    if state.is_mds3 && state.perform_mds1 && state.bypass_tx_applies(block.0, block.1) {
        return true;
    }
    cc::TX_SIZE_HIGH[tx_size] > 32
        || cc::TX_SIZE_WIDE[tx_size] > 32
        || cc::ext_tx_types(tx_size, is_inter, reduced_tx_set) == 1
        || cc::ext_tx_set(tx_size, is_inter, reduced_tx_set) == 0
}

/// Where C `av1_txt_rate_est` (`:4553-4576`) reads the TX-type signalling
/// cost from.
///
/// C returns the rate directly by indexing `ctx->md_rate_est_ctx`. Handing
/// the whole rate-estimation context to a pure gate would couple this module
/// to a large mutable struct for one table read, so the DECISION — which
/// table, and at which indices — is returned instead and the caller does the
/// lookup it already owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxtRateSource {
    /// No tx_type is signalled at this size; the rate is 0.
    Free,
    /// `inter_tx_type_fac_bits[set][square_tx_size][tx_type]`.
    Inter {
        set: usize,
        square_tx_size: usize,
        tx_type: usize,
    },
    /// `intra_tx_type_fac_bits[set][square_tx_size][intra_dir][tx_type]`.
    ///
    /// `intra_dir` is the FILTER-INTRA-mapped direction when the candidate
    /// uses filter intra (`fimode_to_intradir[filter_intra_mode]`), and the
    /// prediction mode otherwise (`:4567-4569`).
    Intra {
        set: usize,
        square_tx_size: usize,
        intra_dir: usize,
        tx_type: usize,
    },
}

/// C `av1_txt_rate_est` (`:4553-4576`).
///
/// Two independent "no cost" exits: the size admits a single TX type
/// (`get_ext_tx_types() <= 1`), or the set index is 0 (`get_ext_tx_set() ==
/// 0`). C tests them in that order and returns 0 from both.
#[must_use]
pub fn txt_rate_source(
    tx_size: usize,
    tx_type: usize,
    is_inter: bool,
    intra_dir: usize,
    reduced_tx_set: bool,
) -> TxtRateSource {
    if cc::ext_tx_types(tx_size, is_inter, reduced_tx_set) <= 1 {
        return TxtRateSource::Free;
    }
    let square_tx_size = cc::TXSIZE_SQR_MAP[tx_size];
    debug_assert!(
        square_tx_size < 4,
        "C asserts square_tx_size < EXT_TX_SIZES (:4557)"
    );
    let set = cc::ext_tx_set(tx_size, is_inter, reduced_tx_set);
    if set == 0 {
        return TxtRateSource::Free;
    }
    let set = set as usize;
    if is_inter {
        TxtRateSource::Inter {
            set,
            square_tx_size,
            tx_type,
        }
    } else {
        TxtRateSource::Intra {
            set,
            square_tx_size,
            intra_dir,
            tx_type,
        }
    }
}

#[cfg(test)]
mod tests;

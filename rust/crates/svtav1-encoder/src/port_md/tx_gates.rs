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
    /// bwidth * qp_index)`) and the left in the `uint32_t` promotion of
    /// `luma_fast_dist * bypass_tx_th`. At the widest block (128x128) and
    /// the highest qindex (255) the normaliser is 4.2e6, well inside 32
    /// bits; the product on the left is NOT — `luma_fast_dist` is a
    /// `uint64_t` SSE — so C truncates it. That truncation is reproduced
    /// here with an explicit `as u32`, because dropping it would make the
    /// port take the shortcut in cases C does not.
    #[must_use]
    fn bypass_tx_applies(&self, width: usize, height: usize) -> bool {
        self.bypass_tx_th != 0
            && !self.block_has_coeff
            && ((self.luma_fast_dist * u64::from(self.bypass_tx_th)) as u32)
                < ((height * width) as u32).wrapping_mul(self.qp_index)
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
mod tests {
    use super::*;

    /// Tier 4: every expectation is hand-derived from the C source line in
    /// its comment. All five functions are `static`/`INLINE` in C.
    fn txt() -> TxtControls {
        TxtControls {
            enabled: true,
            group_inter_lt_16x16: 2,
            group_inter_gt_eq_16x16: 3,
            group_intra_lt_16x16: 5,
            group_intra_gt_eq_16x16: 4,
        }
    }

    fn txs() -> TxsControls {
        TxsControls {
            enabled: true,
            intra_class_max_depth_sq: 2,
            intra_class_max_depth_nsq: 1,
            inter_class_max_depth_sq: 1,
            inter_class_max_depth_nsq: 0,
            depth1_txt_group_offset: 3,
            depth2_txt_group_offset: 4,
        }
    }

    fn state() -> TxShortcutState {
        TxShortcutState {
            perform_mds1: false,
            is_mds3: false,
            use_tx_shortcuts_mds3: false,
            bypass_tx_th: 0,
            block_has_coeff: true,
            luma_fast_dist: 0,
            qp_index: 100,
        }
    }

    /// `:4102-4110`, exhaustively over the block sizes C names.
    #[test]
    fn end_tx_depth_matches_the_c_block_size_list() {
        for (w, h) in [
            (64, 64),
            (32, 32),
            (16, 16),
            (64, 32),
            (32, 64),
            (16, 32),
            (32, 16),
            (16, 8),
            (8, 16),
            (64, 16),
            (16, 64),
            (32, 8),
            (8, 32),
            (16, 4),
            (4, 16),
        ] {
            assert_eq!(get_end_tx_depth(w, h), 2, "{w}x{h}");
        }
        assert_eq!(get_end_tx_depth(8, 8), 1);
        for (w, h) in [(8, 4), (4, 8), (4, 4), (128, 128), (128, 64), (64, 128)] {
            assert_eq!(get_end_tx_depth(w, h), 0, "{w}x{h}");
        }
    }

    /// `:4294-4298`: four fields, not two. An intra-only table would return
    /// the intra value for an inter candidate.
    #[test]
    fn the_type_group_splits_four_ways() {
        let (t, s) = (txt(), txs());
        // TX_16X16 is index 2 (wide 16, high 16) -> "not small".
        let big = cc::tx_size_from_dims(16, 16);
        // TX_8X8 is index 1 -> small.
        let small = cc::tx_size_from_dims(8, 8);
        assert_eq!(get_tx_type_group(&t, &s, big, 0, false, true), 4);
        assert_eq!(get_tx_type_group(&t, &s, small, 0, false, true), 5);
        assert_eq!(get_tx_type_group(&t, &s, big, 0, false, false), 3);
        assert_eq!(get_tx_type_group(&t, &s, small, 0, false, false), 2);
    }

    /// `:4290-4292`: "small" is `wide < 16 || high < 16`, so a 32x8 rect is
    /// small even though one side is large.
    #[test]
    fn a_rectangular_transform_is_small_if_either_side_is() {
        let (t, s) = (txt(), txs());
        let rect = cc::tx_size_from_dims(32, 8);
        assert_eq!(get_tx_type_group(&t, &s, rect, 0, false, true), 5);
    }

    /// `:4301-4305`: the depth offsets subtract and floor at 1, and they
    /// apply even to the forced group of 1.
    #[test]
    fn depth_offsets_subtract_and_floor_at_one() {
        let (t, s) = (txt(), txs());
        let big = cc::tx_size_from_dims(16, 16);
        assert_eq!(get_tx_type_group(&t, &s, big, 1, false, true), 1, "4 - 3");
        assert_eq!(
            get_tx_type_group(&t, &s, big, 2, false, true),
            1,
            "4 - 4 -> 1"
        );
        assert_eq!(get_tx_type_group(&t, &s, big, 0, true, true), 1, "forced");
        assert_eq!(
            get_tx_type_group(&t, &s, big, 1, true, true),
            1,
            "forced + offset"
        );
        // A negative offset RAISES the group — nothing in C forbids it.
        let mut s2 = s;
        s2.depth1_txt_group_offset = -2;
        assert_eq!(get_tx_type_group(&t, &s2, big, 1, false, true), 6);
    }

    /// `:6705-6708`: the two early arms. `!mds_do_txs` pins BOTH bounds to
    /// the candidate's own depth, which the intra funnel never does.
    #[test]
    fn the_early_arms_pin_both_bounds() {
        let s = state();
        let mut t = txs();
        t.enabled = false;
        assert_eq!(
            get_start_end_tx_depth(
                &t,
                true,
                2,
                false,
                true,
                (16, 16),
                (0, 0),
                (64, 64),
                &s,
                false,
                16
            ),
            (0, 0)
        );
        let t = txs();
        assert_eq!(
            get_start_end_tx_depth(
                &t,
                false,
                2,
                false,
                true,
                (16, 16),
                (0, 0),
                (64, 64),
                &s,
                false,
                16
            ),
            (2, 2),
            "a disabled TXS search keeps the candidate's own depth"
        );
    }

    /// `:6712-6717`: a block overhanging the aligned frame gets depth 0.
    #[test]
    fn an_overhanging_block_is_pinned_to_depth_zero() {
        let (t, s) = (txs(), state());
        let inside = get_start_end_tx_depth(
            &t,
            true,
            0,
            false,
            true,
            (16, 16),
            (48, 48),
            (64, 64),
            &s,
            false,
            16,
        );
        assert_eq!(inside, (0, 2));
        let over = get_start_end_tx_depth(
            &t,
            true,
            0,
            false,
            true,
            (16, 16),
            (56, 48),
            (64, 64),
            &s,
            false,
            16,
        );
        assert_eq!(over, (0, 0));
    }

    /// `:6730-6732`: the class caps differ by mode class AND by shape, and
    /// the inter caps are the ones an intra-only port cannot reach.
    #[test]
    fn the_depth_cap_is_keyed_on_mode_class_and_shape() {
        let (t, s) = (txs(), state());
        let d = |inter, square| {
            get_start_end_tx_depth(
                &t,
                true,
                0,
                inter,
                square,
                (16, 16),
                (0, 0),
                (64, 64),
                &s,
                false,
                16,
            )
            .1
        };
        assert_eq!(d(false, true), 2, "intra sq");
        assert_eq!(d(false, false), 1, "intra nsq");
        assert_eq!(d(true, true), 1, "inter sq");
        assert_eq!(d(true, false), 0, "inter nsq");
    }

    /// `:6720-6726` and `:6734-6736`: the bypass shortcut zeroes both
    /// bounds, and the lossless pin then RAISES them back to 1 because it
    /// runs after everything else.
    #[test]
    fn the_bypass_shortcut_and_the_lossless_pin_compose_in_c_order() {
        let t = txs();
        let s = TxShortcutState {
            perform_mds1: true,
            is_mds3: true,
            bypass_tx_th: 10,
            block_has_coeff: false,
            luma_fast_dist: 1,
            qp_index: 100,
            ..state()
        };
        // 8x8 area 64 * qp 100 = 6400; 1 * 10 = 10 < 6400 -> shortcut.
        assert_eq!(
            get_start_end_tx_depth(
                &t,
                true,
                0,
                false,
                true,
                (8, 8),
                (0, 0),
                (64, 64),
                &s,
                false,
                8
            ),
            (0, 0)
        );
        assert_eq!(
            get_start_end_tx_depth(
                &t,
                true,
                0,
                false,
                true,
                (8, 8),
                (0, 0),
                (64, 64),
                &s,
                true,
                8
            ),
            (1, 1),
            "mimic_only_tx_4x4 runs last and overrides the zeroed bounds"
        );
        // A candidate that DID keep coefficients never takes the shortcut.
        let coded = TxShortcutState {
            block_has_coeff: true,
            ..s
        };
        assert_eq!(
            get_start_end_tx_depth(
                &t,
                true,
                0,
                false,
                true,
                (8, 8),
                (0, 0),
                (64, 64),
                &coded,
                false,
                8
            ),
            (0, 1)
        );
    }

    /// `:4525-4548`: five independent reasons, each checked alone.
    #[test]
    fn dct_only_has_five_independent_causes() {
        let tx16 = cc::tx_size_from_dims(16, 16);
        let s = state();
        assert!(
            search_dct_dct_only(false, &s, (16, 16), tx16, false, false),
            "txt off"
        );
        let shortcut = TxShortcutState {
            is_mds3: true,
            use_tx_shortcuts_mds3: true,
            ..s
        };
        assert!(search_dct_dct_only(
            true,
            &shortcut,
            (16, 16),
            tx16,
            false,
            false
        ));
        let bypass = TxShortcutState {
            is_mds3: true,
            perform_mds1: true,
            bypass_tx_th: 10,
            block_has_coeff: false,
            luma_fast_dist: 1,
            ..s
        };
        assert!(search_dct_dct_only(
            true,
            &bypass,
            (16, 16),
            tx16,
            false,
            false
        ));
        // 64x64 is larger than 32 in both dimensions.
        let tx64 = cc::tx_size_from_dims(64, 64);
        assert!(search_dct_dct_only(true, &s, (64, 64), tx64, false, false));
        // The baseline 16x16 intra case is NOT dct-only — the positive
        // control that keeps the four above from passing vacuously.
        assert!(!search_dct_dct_only(true, &s, (16, 16), tx16, false, false));
    }

    /// `:4530` and `:4532` both require MD_STAGE_3; outside it neither
    /// shortcut fires even when its own flag is set.
    #[test]
    fn the_shortcut_arms_are_mds3_only() {
        let tx16 = cc::tx_size_from_dims(16, 16);
        let armed_but_early = TxShortcutState {
            is_mds3: false,
            use_tx_shortcuts_mds3: true,
            perform_mds1: true,
            bypass_tx_th: 10,
            block_has_coeff: false,
            luma_fast_dist: 1,
            ..state()
        };
        assert!(!search_dct_dct_only(
            true,
            &armed_but_early,
            (16, 16),
            tx16,
            false,
            false
        ));
    }

    /// `:4555-4573`: which table, and the two free exits.
    #[test]
    fn txt_rate_source_picks_the_table_by_mode_class() {
        let tx16 = cc::tx_size_from_dims(16, 16);
        assert_eq!(
            txt_rate_source(tx16, 3, false, 6, false),
            TxtRateSource::Intra {
                set: cc::ext_tx_set(tx16, false, false) as usize,
                square_tx_size: cc::TXSIZE_SQR_MAP[tx16],
                intra_dir: 6,
                tx_type: 3,
            }
        );
        assert_eq!(
            txt_rate_source(tx16, 3, true, 6, false),
            TxtRateSource::Inter {
                set: cc::ext_tx_set(tx16, true, false) as usize,
                square_tx_size: cc::TXSIZE_SQR_MAP[tx16],
                tx_type: 3,
            }
        );
        // 64x64 admits DCT_DCT only -> no signalling cost.
        let tx64 = cc::tx_size_from_dims(64, 64);
        assert_eq!(
            txt_rate_source(tx64, 0, false, 0, false),
            TxtRateSource::Free
        );
        assert_eq!(
            txt_rate_source(tx64, 0, true, 0, false),
            TxtRateSource::Free
        );
    }

    /// The reduced-TX-set frame header collapses more sizes to free, and it
    /// does so differently for intra and inter.
    #[test]
    fn the_reduced_tx_set_changes_which_sizes_are_free() {
        let mut differed = 0usize;
        // Only the real AV1 TX shapes: square, 2:1 and 4:1. `tx_size_from_dims`
        // panics on 4x32 / 32x4, which do not exist as transforms.
        const SHAPES: &[(usize, usize)] = &[
            (4, 4),
            (8, 8),
            (16, 16),
            (32, 32),
            (4, 8),
            (8, 4),
            (8, 16),
            (16, 8),
            (16, 32),
            (32, 16),
            (4, 16),
            (16, 4),
            (8, 32),
            (32, 8),
        ];
        {
            for &(w, h) in SHAPES {
                let tx = cc::tx_size_from_dims(w, h);
                for is_inter in [false, true] {
                    let full = txt_rate_source(tx, 1, is_inter, 0, false);
                    let reduced = txt_rate_source(tx, 1, is_inter, 0, true);
                    if full != reduced {
                        differed += 1;
                    }
                }
            }
        }
        assert!(
            differed > 0,
            "the reduced-set flag must reach the source decision somewhere"
        );
    }
}

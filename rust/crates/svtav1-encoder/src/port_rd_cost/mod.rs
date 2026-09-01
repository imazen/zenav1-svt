//! `Source/Lib/Codec/rd_cost.c` — the MD cost layer, ported wholesale.
//!
//! # What this file is for
//!
//! `rd_cost.c` is where a mode-decision candidate is turned into a number.
//! Every candidate the inter path injects is ranked by
//! [`inter_cost::inter_fast_cost`] at MDS0 and by [`full_cost::full_cost`] at
//! MDS3; without them an inter encoder has candidates and no way to choose
//! between them.
//!
//! # Coverage — 19 of 34 rd_cost.c functions, and what the other 15 are
//!
//! The file defines 41 functions; 7 already had counterparts before this
//! module (see the table's second half). Of the 34 the inventory listed as
//! missing:
//!
//! | C function | line | here |
//! |---|---|---|
//! | `svt_aom_get_switchable_rate` | 849 | [`inter_cost::get_switchable_rate`] |
//! | `get_compound_mode_rate` | 783 | [`inter_cost::get_compound_mode_rate`] |
//! | `av1_inter_fast_cost_light` | 870 | [`inter_cost::inter_fast_cost_light`] |
//! | `svt_aom_inter_fast_cost` | 1005 | [`inter_cost::inter_fast_cost`] |
//! | `svt_aom_get_intra_uv_fast_rate` | 476 | [`intra_cost::get_intra_uv_fast_rate`] |
//! | `svt_aom_intra_fast_cost` | 526 | [`intra_cost::intra_fast_cost`] |
//! | `svt_aom_full_cost` | 1349 | [`full_cost::full_cost`] |
//! | `svt_aom_full_cost_pd0` | 1330 | [`full_cost::full_cost_pd0`] |
//! | `svt_aom_txb_estimate_coeff_bits` | 1233 | [`full_cost::txb_estimate_coeff_bits`] |
//! | `svt_aom_txb_estimate_coeff_bits_pd0` | 1206 | [`full_cost::txb_estimate_coeff_bits_pd0`] |
//! | `svt_aom_coding_loop_context_generation` | 1475 | [`full_cost::coding_loop_context_generation`] |
//! | `is_any_masked_compound_used` (inter_prediction.h:303) | — | [`inter_cost::is_any_masked_compound_used`] |
//! | `is_interinter_compound_used` (inter_prediction.h:288) | — | [`inter_cost::is_interinter_compound_used`] |
//! | `svt_aom_is_interintra_wedge_used` (inter_prediction.c:2015) | — | [`inter_cost::is_interintra_wedge_used`] |
//! | `svt_aom_is_masked_compound_type` (inter_prediction.c:34) | — | [`inter_cost::is_masked_compound_type`] |
//! | `av1_is_interp_needed_md` (rd_cost.h:71) | — | [`inter_cost::is_interp_needed_md`] |
//! | `RDCOST` (rd_cost.h:36) | — | [`rdcost`] |
//! | `av1_cost_literal` (md_rate_estimation.h:31) | — | [`cost_literal`] |
//! | `block_signals_txsize` | 1496 | [`full_cost::block_signals_txsize`] |
//!
//! **NOT ported here, named individually rather than left implicit.** Each of
//! these was checked against the port before being dropped from the queue:
//!
//! | C function | line | why not |
//! |---|---|---|
//! | `av1_cost_coeffs_txb_loop_cost_eob` | 255 | inlined into `leaf_funnel::coeff_rate::cost_coeffs_txb`, which ports `svt_av1_cost_coeffs_txb` whole |
//! | `av1_cost_coeffs_txb_loop_cost_one_eob` | 224 | same — the `eob == 1` arm of that function |
//! | `av1_cost_skip_txb` | 213 | `leaf_funnel::coeff_rate::cost_skip_txb` |
//! | `get_eob_cost` | 198 | `crate::quant::eob_cost` |
//! | `get_golomb_cost` | 84 | `crate::quant::golomb_cost` |
//! | `svt_av1_txb_init_levels_c` | 93 | `crate::entropy::coeff_c::txb_init_levels` |
//! | `svt_av1_get_mv_joint` | 47 | `crate::intrabc::mv_joint_index` |
//! | `cost_tx_size_vartx` | 1591 | `crate::vartx` (the whole var-tx walk, writer + cost) |
//! | `get_sqr_tx_size` | 1548 | `crate::vartx::sqr_tx_size_of_dim` |
//! | `txfm_partition_context` | 1568 | `crate::vartx::txfm_partition_context` |
//! | `txfm_partition_update` | 1531 | `crate::vartx::TxfmCtx::update` |
//! | `get_vartx_max_txsize` | 1500 | `crate::vartx::drive_walk`, which computes the depth-0 unit dims inline |
//! | `max_block_wide` / `max_block_high` | 1509 / 1520 | same — `drive_walk`'s `max_units_*`, including C's asymmetry (only `high` is frame-clipped) |
//! | `set_txfm_ctx` / `set_txfm_ctxs` | 1651 / 1658 | `pipeline.rs`'s block-span txfm stamp |
//! | `tx_size_to_depth` / `get_tx_size_context` / `cost_selected_tx_size` / `svt_aom_tx_size_bits` / `svt_aom_get_tx_size_bits` | 1671..1782 | the intra arm is `leaf_funnel::mds3`'s `rates.tx_size[cat][ctx][depth]` lookup with `tx_geom::tx_size_cat`; the inter arm is `vartx::tx_size_bits_vartx` |
//! | `svt_aom_partition_rate_cost` | 1822 | `depth_refine::PartRates::bits_edge` — all three arms |
//! | `av1_transform_type_rate_estimation` | 107 | the RATE half is `leaf_funnel::rate_tables::MdRates::txt_rate`; the `allow_update_cdf` half belongs to the pack path, not to MD |
//! | `update_eob_context` | 157 | same: a CDF-adaptation helper of the pack path |
//!
//! So the honest count for THIS module is 19 written here, 19 already present
//! elsewhere, and 2 (`av1_transform_type_rate_estimation`, `update_eob_context`)
//! covered only on their rate half — their CDF-update half is unported.
//!
//! # Reachability
//!
//! Nothing here has a caller yet: `pipeline.rs`'s public entry still refuses
//! inter frames, and the wiring belongs to the chunk that owns that file. Per
//! `docs/WORKING-ON-THIS.md` §7 a faithful translation with no caller stays
//! translated and states its reachability rather than carrying
//! `#[allow(dead_code)]`.
//!
//! # Evidence — per function, not per module
//!
//! **Tier 1** (`tests/c_parity_rd_cost.rs` drives the REAL exported symbol
//! through `svtav1-cref`'s `rd_cost` shim; `nm -g Bin/Release/libSvtAv1Enc.a`
//! prints `T` for each, checked rather than inferred from the `svt_aom_`
//! prefix — `rd_cost.c` has both an unprefixed export, `get_eob_cost`, and
//! prefixed `static`s):
//!
//! * `svt_aom_get_switchable_rate`
//! * `svt_aom_inter_fast_cost` — single-ref and compound, at
//!   `approx_inter_rate` 0, 1 and 2
//! * `svt_aom_intra_fast_cost` — both arms, IntraBC and intra
//! * `svt_aom_get_intra_uv_fast_rate`
//! * `svt_aom_full_cost` and `svt_aom_full_cost_pd0`
//!
//! and, THROUGH those, the two `static`s this module also ports:
//! `get_compound_mode_rate` (reached unconditionally by every compound
//! candidate) and `av1_inter_fast_cost_light` (reached via
//! `approx_inter_rate`). They are driven as part of the exported caller
//! rather than re-transcribed into a second shim, which is what §4 asks for.
//!
//! **Not differentially tested here, and why:**
//!
//! * [`full_cost::txb_estimate_coeff_bits`] / `_pd0` — a dispatcher whose
//!   entire arithmetic is `leaf_funnel::coeff_rate::{cost_coeffs_txb,
//!   cost_skip_txb}`, already covered. What is NEW here is the per-plane
//!   routing and the luma-only `mds_subres_step` shift, which is **tier 4**:
//!   the C entry point takes a `EbPictureBufferDesc*` whose per-plane
//!   `txb_origin_index` arithmetic would have to be reproduced in the shim to
//!   call it, and a shim that reproduces the routing under test proves
//!   nothing.
//! * [`full_cost::coding_loop_context_generation`] — every context it derives
//!   is computed by a function this port already gates at tier 1
//!   (`svt_aom_get_kf_y_mode_ctx`, `svt_av1_get_intra_inter_context`,
//!   `av1_get_skip_mode_context`, `av1_get_skip_context`,
//!   `svt_aom_collect_neighbors_ref_counts_new`); this function is the three
//!   GATES around them, and it takes them as closures precisely so it can be
//!   read as those gates alone. **Tier 4.**
//! * The tx-size terms of `svt_aom_full_cost` — C recomputes them from
//!   `svt_aom_get_tx_size_bits` instead of taking them as arguments, so the
//!   differential runs at `tx_mode != TX_MODE_SELECT` where both are zero on
//!   both sides. `crate::vartx` gates that walk separately.
//! * The `use_palette == 1` arm of `svt_aom_intra_fast_cost` — its sub-cost
//!   is an INPUT here (see [`intra_cost`]'s module doc), so driving it would
//!   compare C against a number C produced. **Tier 4.**
//!
//! # Two C-vs-port defects the differential caught, recorded rather than
//! quietly fixed
//!
//! * `SWITCHABLE` is `SWITCHABLE_FILTERS + 1` = **4**, not `3`. A first draft
//!   of [`inter_cost::get_switchable_rate`] used the count instead of the
//!   sentinel, which zeroed every interpolation-filter rate on a real
//!   switchable frame — a silent under-price of every inter candidate that a
//!   hand-read of the C would not have surfaced.
//! * `svt_aom_allow_intrabc` (entropy_coding.c:4401) is a conjunction of
//!   THREE things — `slice_type == I_SLICE && allow_screen_content_tools &&
//!   frm_hdr->allow_intrabc`. The middle term is easy to drop.

pub mod full_cost;
pub mod inter_cost;
pub mod intra_cost;

/// C `AV1_PROB_COST_SHIFT` (md_rate_estimation.h:30).
pub const AV1_PROB_COST_SHIFT: u32 = 9;
/// C `RDDIV_BITS` (rd_cost.h:34).
pub const RDDIV_BITS: u32 = 7;
/// C `MV_COST_WEIGHT` (md_rate_estimation.h:23).
pub const MV_COST_WEIGHT: i32 = 108;
/// C `MV_COST_WEIGHT_SUB` (md_rate_estimation.h:24).
pub const MV_COST_WEIGHT_SUB: i32 = 120;

/// C `RDCOST(RM, R, D)` (rd_cost.h:36).
///
/// `ROUND_POWER_OF_TWO(R * RM, 9) + (D << 7)`. C evaluates the product in
/// `int64_t`; every caller in this file passes a non-negative rate and
/// distortion, so the port uses `u64` and the arithmetic is identical over
/// the reachable domain.
#[inline]
pub fn rdcost(lambda: u64, rate: u64, dist: u64) -> u64 {
    ((rate * lambda + (1 << (AV1_PROB_COST_SHIFT - 1))) >> AV1_PROB_COST_SHIFT)
        + (dist << RDDIV_BITS)
}

/// C `av1_cost_literal(n)` (md_rate_estimation.h:31): `n << 9`.
#[inline]
pub const fn cost_literal(n: u32) -> u32 {
    n << AV1_PROB_COST_SHIFT
}

//! IntraBC (intra block copy) encoder vertical — allintra KEY screen-content
//! path (task: IBC bulk-port, 2026-07-17). C reference: `Source/Lib/Codec/
//! {enc_mode_config,mode_decision,av1me,adaptive_mv_pred,inter_prediction,
//! entropy_coding,rd_cost,md_rate_estimation,mcomp,hash_motion}.{c,h}`.
//!
//! **UNWIRED.** This file is NOT declared in `lib.rs` (`pub mod intrabc;` is
//! absent) and is therefore not part of the crate's compiled surface —
//! provably inert. Wiring is a separate task: add `pub mod intrabc;` to
//! `lib.rs`, thread `IbcCtrls`/`intra_bc_search`/`build_intra_bc_candidate`
//! into `mode_decision.rs`'s candidate-injection path (mirroring
//! `inject_palette_candidates` in `palette.rs`'s wiring, once that lands),
//! flip `sc_detect.rs::ScDerivation::allow_intrabc` from its hardcoded
//! `false` to the real `IbcCtrls::for_level(intrabc_level).enabled`, and
//! feed `write_intrabc_info` into the PACK block-mode-info writer ahead of
//! the y-mode symbol (`entropy_coding.c:5022`, already the position the FH
//! `allow_intrabc` bit / LF-CDEF-LR skips in `svtav1-entropy/src/obu.rs`
//! were landed dormant for). This module was compile-checked standalone by
//! temporarily adding `pub mod intrabc;` to `lib.rs`, running `cargo build
//! -p svtav1-encoder`, then reverting `lib.rs` — see the porting session's
//! final report for the exact command and result.
//!
//! House style: pure functions over caller-provided pixel slices / scalars,
//! no `PictureControlSet`/`ModeDecisionContext` dependency (see
//! `docs/palette-port-map.md` + `src/palette.rs`/`src/sc_detect.rs` for the
//! established shape this follows). Every function cites its C file:line.
//! Spots not yet FFI/differential-verified carry `PORT-NOTE(unverified)`
//! per `CLAUDE.md`'s BULK-PORT MODE convention — grep this file for the
//! complete debt list, and see the CLAUDE.md PORT-NOTE index entry for
//! `intrabc.rs` for the summary.
//!
//! # Scope: translated vs. documented-only
//!
//! **Translated (pure logic, unit-tested where cheap):**
//! - Per-level search controls (`IbcCtrls::for_level`, §1).
//! - QP-based mesh-range scaling (§1).
//! - DV validity + chroma-reference predicate (§2).
//! - Ref-DV composition + the spec default-DV fallback rule (§3).
//! - The full-pixel diamond + exhaustive mesh search stack, parameterized
//!   over caller-supplied pixel slices and cost tables (§4).
//! - The hash-bucket **selection** algorithm, given an already-fetched
//!   bucket (§4b) — the hash **table** itself (CRC computation, bucket
//!   storage) is NOT translated, see §4b's doc.
//! - DV rate-cost tables + the search-time and RD-time MV bit-cost
//!   formulas (§5).
//! - Injection gating (palette-hint coupling, NSQ/B4 parent gating) and
//!   the candidate struct shape (§6).
//! - The bitstream writer path, reusing `svtav1_entropy::mv_coding`'s
//!   already-verified `NmvContext`/`encode_mv_diff` (§7).
//!
//! **Documented only (cited, not transcribed — see the doc comment at each
//! stub):**
//! - The hash **table**: CRC-based block hashing (`svt_av1_get_block_hash_
//!   value`) and the `HashTable`/`Vector` bucket storage + `generate_ibc_
//!   data`'s whole-picture, multi-block-size precompute (`hash_motion.{c,h}`,
//!   `md_config_process.c:585-...`). This is a frame-wide stateful
//!   subsystem, not a per-block pure function — out of scope for a
//!   "caller-provided recon slice" search skeleton. Skipping hash search
//!   entirely is always LEGAL (it is a pure speed optimization over the
//!   full-pixel search below; every DV it would have found, the diamond/
//!   mesh search can still find, just slower), so the port is correct with
//!   the hash step stubbed to "no candidate" — see §4b.
//! - The ref-mv-stack derivation of `nearestmv`/`nearmv`
//!   (`svt_av1_find_best_ref_mvs_from_stack`, `adaptive_mv_pred.c:2030`) —
//!   the general inter-prediction DRL search, a separate large subsystem.
//!   [`resolve_dv_ref`] takes its OUTPUT as an input.
//! - RD integration: how a candidate's fast/full cost is assembled
//!   (`svt_aom_intra_fast_cost`'s `use_intrabc` arm), prediction
//!   compensation (recon-domain block copy), and tx-path reuse — see the
//!   "RD integration" doc section near the bottom of this file.

use alloc::vec::Vec;
use svtav1_entropy::mv_coding::{MvSubpelPrecision, NmvContext, encode_mv_diff};
use svtav1_entropy::writer::AomWriter;
use svtav1_types::interp::InterpFilter;
use svtav1_types::motion::{FullMvLimits, Mv};
use svtav1_types::prediction::{MotionMode, PredictionMode, UvPredictionMode};
use svtav1_types::transform::TxType;

// =============================================================================
// §0. Small shared math helpers (duplicated locally per house style — see
// `palette.rs`'s `divide_and_round`/`lcg_next` for the same pattern).
// =============================================================================

/// C `ROUND_POWER_OF_TWO(value, n)` (definitions.h:478): `(value + (1<<n>>1))
/// >> n`. Only called with `n >= 1` in this module (matches every call site
/// below).
#[inline]
fn round_power_of_two(value: i32, n: u32) -> i32 {
    (value + ((1i32 << n) >> 1)) >> n
}

/// C `ROUND_POWER_OF_TWO_64` (definitions.h:485): same shape, `i64` domain.
#[inline]
fn round_power_of_two_64(value: i64, n: u32) -> i64 {
    (value + ((1i64 << n) >> 1)) >> n
}

/// C `DIVIDE_AND_ROUND(x, y)` (utility.h:96): round-half-up for non-negative
/// `x`/`y` (the only domain `svt_aom_get_qp_based_th_scaling_factors`'s
/// caller uses it in).
#[inline]
fn divide_and_round(x: i64, y: i64) -> i64 {
    (x + (y >> 1)) / y
}

// =============================================================================
// §1. IbcCtrls — per-level search controls (`set_intrabc_level`,
// enc_mode_config.c:1652-1836) + the allintra level derivation
// (`svt_aom_sig_deriv_multi_processes_allintra`, enc_mode_config.c:2337-2360)
// + QP-based mesh-range scaling (`svt_aom_get_qp_based_th_scaling_factors`,
// enc_mode_config.c:25-49, and its intrabc call site, md_config_process.c:
// 956-966).
// =============================================================================

/// C `MeshPattern` (pcs.h:125-128): one exhaustive-search ring's `(range,
/// interval)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshPattern {
    pub range: i32,
    pub interval: i32,
}

/// C `MAX_MESH_STEP` (pcs.h:122).
pub const MAX_MESH_STEP: usize = 4;

/// C `IntrabcCtrls` (pcs.h:624-640): the full per-level intra-BC search
/// configuration. `for_level` transcribes `set_intrabc_level`
/// (enc_mode_config.c:1657-1836) for the allintra-reachable level set
/// `{0, 3, 4, 5, 6, 7}` (levels 1/2 exist in C but are reachable only via
/// the MR tier / a non-allintra config not exposed by this port's allintra
/// derivation — `sc_detect.rs::derive_allintra_sc` never produces them).
#[derive(Debug, Clone, Copy, Default)]
pub struct IbcCtrls {
    pub enabled: bool,
    /// Skip the DV search entirely when the co-sited palette search (task
    /// #71) produced zero candidates for this block — see
    /// [`eval_intrabc_after_palette`].
    pub palette_hint: bool,
    /// In NSQ shapes, skip the DV search if the parent square block did not
    /// select `use_intrabc` — see [`parent_gate_allows_intrabc`].
    pub nsq_parent_gating: bool,
    /// For 4x4 square blocks, skip the DV search if the parent 8x8 square
    /// did not select `use_intrabc`.
    pub b4_parent_gating: bool,
    /// Max block size (must be square, `width == height`) eligible for hash
    /// search; `0` disables hash search outright (never the case at any
    /// allintra-reachable level: the table below is always 8 or 64).
    pub max_block_size_hash: u8,
    pub max_cand_per_bucket: u16,
    /// SAD threshold (search-domain units) to trigger the exhaustive mesh
    /// refinement after the diamond search — see [`full_pixel_search`]'s
    /// doc for the `(uint64_t)~0` "always mesh" encoding this field carries
    /// at levels 6/7.
    pub exhaustive_mesh_thresh: u64,
    /// Skip the mesh refinement if the diamond search's full-pel MV barely
    /// moved from the starting point (`<=` this many full-pel units).
    /// `-1` (the level-1/2 value) can never trigger the skip since the
    /// compared magnitude is `>= 0` — i.e. `-1` means "mesh always runs
    /// once `var` clears the SAD threshold", distinct from `0` which skips
    /// whenever the diamond search left the MV exactly at the start.
    pub mesh_search_mv_diff_threshold: i32,
    pub mesh_patterns: [MeshPattern; MAX_MESH_STEP],
    /// Scale `mesh_patterns[*].range` by the frame's QP via
    /// [`scale_mesh_patterns_by_qp`] — gated a SECOND time by the
    /// SCS-level `qp_based_th_scaling_ctrls.intra_bc_mesh_qp_scaling` flag,
    /// see that function's doc.
    pub mesh_qp_scaling: bool,
    /// `0` = search LEFT and ABOVE (both `IntrabcMotionDirection`s), `1` =
    /// search ABOVE only. C field is a raw `search_dir` int compared as
    /// `search_dir ? IBC_MOTION_LEFT : IBC_MOTION_DIRECTIONS` — i.e. this
    /// is confusingly named on the C side too: `search_dir=1` DISABLES the
    /// LEFT direction, it does not select it. See [`IntrabcMotionDirection`].
    pub search_dir: u8,
}

impl IbcCtrls {
    /// C `set_intrabc_level` (enc_mode_config.c:1657-1836), levels 0 and
    /// 3..=7 (`MAX_INTRABC_LEVEL = 7`). Levels 1/2 are transcribed too
    /// (reachable in C at the MR tier / non-allintra configs) for
    /// completeness, but PORT-NOTE below.
    ///
    /// PORT-NOTE(unverified): at levels 6 and 7, C's switch arms do NOT
    /// assign `mesh_search_mv_diff_threshold`, `mesh_patterns`, or
    /// `mesh_qp_scaling` — those fields keep whatever `pcs->intrabc_ctrls`
    /// held before this call. Because `PictureParentControlSet` objects are
    /// drawn from a reuse pool (not proven fresh-zeroed per call from this
    /// translation alone), a prior frame that ran a DIFFERENT intrabc_level
    /// with nonzero mesh state could theoretically leave stale values live
    /// at levels 6/7 in a multi-frame encode. This port's `for_level`
    /// returns a FRESH, zero-defaulted struct for the unassigned fields at
    /// levels 6/7 (matching a first-use/zero-initialized pool slot, which
    /// is what any single-KEY-frame still encode — this port's current
    /// scope — always sees). At those levels `exhaustive_mesh_thresh =
    /// u64::MAX` makes [`full_pixel_search`] always WANT to run the mesh
    /// step regardless of `mesh_search_mv_diff_threshold`'s zeroed value
    /// (see that function's doc), but with `mesh_patterns[0]` zeroed the
    /// mesh step's own range/interval validity check bails immediately —
    /// i.e. the zero-default is self-consistently a no-op mesh step, not a
    /// silent behavior change. Verify: before this port supports
    /// multi-frame (non-KEY) encodes, confirm C's PPCS pool actually
    /// zero-inits on first use (or thread real cross-frame persistence).
    pub fn for_level(level: u8) -> Self {
        match level {
            0 => IbcCtrls::default(),
            1 => IbcCtrls {
                enabled: true,
                palette_hint: false,
                nsq_parent_gating: false,
                b4_parent_gating: false,
                max_block_size_hash: 64,
                max_cand_per_bucket: 256,
                exhaustive_mesh_thresh: 1 << 20,
                mesh_search_mv_diff_threshold: -1,
                mesh_patterns: [
                    MeshPattern { range: 256, interval: 1 },
                    MeshPattern { range: 256, interval: 1 },
                    MeshPattern::default(),
                    MeshPattern::default(),
                ],
                mesh_qp_scaling: false,
                search_dir: 0,
            },
            2 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: false,
                b4_parent_gating: false,
                max_block_size_hash: 64,
                max_cand_per_bucket: 256,
                exhaustive_mesh_thresh: 1 << 20,
                mesh_search_mv_diff_threshold: -1,
                mesh_patterns: [
                    MeshPattern { range: 256, interval: 8 },
                    MeshPattern { range: 64, interval: 1 },
                    MeshPattern::default(),
                    MeshPattern::default(),
                ],
                mesh_qp_scaling: false,
                search_dir: 0,
            },
            3 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: true,
                b4_parent_gating: false,
                max_block_size_hash: 64,
                max_cand_per_bucket: 256,
                exhaustive_mesh_thresh: 1 << 20,
                mesh_search_mv_diff_threshold: 0,
                mesh_patterns: [
                    MeshPattern { range: 256, interval: 8 },
                    MeshPattern { range: 64, interval: 1 },
                    MeshPattern::default(),
                    MeshPattern::default(),
                ],
                mesh_qp_scaling: true,
                search_dir: 0,
            },
            4 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: true,
                b4_parent_gating: false,
                max_block_size_hash: 64,
                max_cand_per_bucket: 64,
                exhaustive_mesh_thresh: 1 << 24,
                mesh_search_mv_diff_threshold: 0,
                mesh_patterns: [
                    MeshPattern { range: 256, interval: 8 },
                    MeshPattern { range: 32, interval: 1 },
                    MeshPattern::default(),
                    MeshPattern::default(),
                ],
                mesh_qp_scaling: true,
                search_dir: 0,
            },
            5 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: true,
                b4_parent_gating: false,
                max_block_size_hash: 8,
                max_cand_per_bucket: 64,
                exhaustive_mesh_thresh: 1 << 24,
                mesh_search_mv_diff_threshold: 0,
                mesh_patterns: [
                    MeshPattern { range: 256, interval: 8 },
                    MeshPattern { range: 32, interval: 1 },
                    MeshPattern::default(),
                    MeshPattern::default(),
                ],
                mesh_qp_scaling: true,
                search_dir: 0,
            },
            6 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: true,
                b4_parent_gating: false,
                max_block_size_hash: 8,
                max_cand_per_bucket: 32,
                exhaustive_mesh_thresh: u64::MAX,
                // Unassigned in C at this level -- see the fn doc PORT-NOTE.
                mesh_search_mv_diff_threshold: 0,
                mesh_patterns: [MeshPattern::default(); MAX_MESH_STEP],
                mesh_qp_scaling: false,
                search_dir: 0,
            },
            7 => IbcCtrls {
                enabled: true,
                palette_hint: true,
                nsq_parent_gating: true,
                b4_parent_gating: false,
                max_block_size_hash: 8,
                max_cand_per_bucket: 32,
                exhaustive_mesh_thresh: u64::MAX,
                // Unassigned in C at this level -- see the fn doc PORT-NOTE.
                mesh_search_mv_diff_threshold: 0,
                mesh_patterns: [MeshPattern::default(); MAX_MESH_STEP],
                mesh_qp_scaling: false,
                search_dir: 1,
            },
            _ => IbcCtrls::default(),
        }
    }
}

/// C `svt_aom_sig_deriv_multi_processes_allintra`'s intra-BC level table
/// (enc_mode_config.c:2337-2360), sc_class5-gated. `preset` is the allintra
/// enc_mode (M0..=M4 reachable; M5+ and !sc_class5 both yield level 0 =
/// disabled). MR tier (level 1) is not reachable from this port's `u8`
/// preset surface (mirrors `sc_detect.rs::derive_allintra_sc`'s identical
/// `palette_level` table shape and its same MR-unreachable note).
pub fn allintra_intrabc_level(preset: u8, sc_class5: bool, enable_intrabc: bool) -> u8 {
    if !enable_intrabc || !sc_class5 {
        return 0;
    }
    match preset {
        0 => 3,
        1 => 4,
        2 => 5,
        3 => 6,
        4 => 7,
        _ => 0,
    }
}

/// C `svt_aom_get_qp_based_th_scaling_factors` (enc_mode_config.c:25-49).
/// Returns `(q_weight, q_weight_denom)`; `enable` is the SCS-level
/// `qp_based_th_scaling_ctrls.intra_bc_mesh_qp_scaling` flag (see
/// [`scale_mesh_patterns_by_qp`]'s doc for where it comes from and why it
/// is DISTINCT from `IbcCtrls::mesh_qp_scaling`).
pub fn qp_based_th_scaling_factors(enable: bool, qp: u32) -> (u32, u32) {
    if !enable {
        return (1, 1);
    }
    const MAX_QP_VALUE: u32 = 63;
    let mut q_weight = qp.max(10);
    let mut q_weight_denom = MAX_QP_VALUE;
    if qp >= 46 {
        let ex = -(f64::from(qp.max(40)) - 35.0) / 10.0;
        let mut q_weight_int = libm_exp(ex);
        q_weight_int = 1.05 - q_weight_int;
        q_weight_int *= 10000.0;
        q_weight = q_weight_int as u32;
        q_weight_denom = 10000;
    }
    (q_weight, q_weight_denom)
}

/// `exp(x)` via a `no_std`-safe minimax rational approximation is
/// overkill for one call/frame; `libm`-style `exp` is provided by `std` in
/// this crate's `std` feature (matches every other call site in this
/// crate's non-hot-path code). PORT-NOTE(unverified): if `intrabc.rs` ships
/// in a `no_std`-only build, swap this for the crate's existing `no_std`
/// float-exp helper (grep `fn exp(` — none exists yet as of this port;
/// C's `exp()` is libm's, IEEE-754 double precision).
#[cfg(feature = "std")]
fn libm_exp(x: f64) -> f64 {
    x.exp()
}
#[cfg(not(feature = "std"))]
fn libm_exp(_x: f64) -> f64 {
    unimplemented!("PORT-NOTE(unverified): no_std exp() not wired -- see doc comment")
}

/// C's intrabc-search QP-scaling call site (`md_config_process.c:950-967`,
/// inside `if (frm_hdr->allow_intrabc) { ... if
/// (intraBC_ctrls->mesh_qp_scaling) { ... } }`). Scales every
/// `mesh_patterns[*].range` by `(q_weight, q_weight_denom)`, rounding
/// half-up (`DIVIDE_AND_ROUND`). Mutates in place; a no-op when
/// `ctrls.mesh_qp_scaling` is false OR the scs-level `scs_mesh_qp_scaling`
/// flag is false.
///
/// TWO gates are ANDed here, both named "mesh qp scaling" in C but
/// distinct fields on distinct structs: `IbcCtrls::mesh_qp_scaling`
/// (per-level, set by [`IbcCtrls::for_level`]) and the SCS-wide
/// `qp_based_th_scaling_ctrls.intra_bc_mesh_qp_scaling` (set ONCE per
/// encode by `set_qp_based_th_scaling_ctrls_all_intra`,
/// `Source/Lib/Globals/enc_handle.c:3837-3877` — note: Globals, not Codec).
/// For the allintra envelope this port targets, that SCS-wide flag is `0`
/// only at the (unreachable-here) MR tier and `1` at every M0..=M6+ bucket
/// — i.e. `scs_mesh_qp_scaling` is effectively always `true` for every
/// preset this module's `IbcCtrls::for_level` can produce with
/// `mesh_qp_scaling=true` (levels 3/4/5). Pass the real derivation once
/// `enc_handle.c`'s allintra QP-scaling table is ported; `true` is the
/// correct value for this port's M0-M4 scope today.
pub fn scale_mesh_patterns_by_qp(ctrls: &mut IbcCtrls, scs_mesh_qp_scaling: bool, qp: u32) {
    if !ctrls.mesh_qp_scaling {
        return;
    }
    let (q_weight, q_weight_denom) = qp_based_th_scaling_factors(scs_mesh_qp_scaling, qp);
    for p in &mut ctrls.mesh_patterns {
        p.range = divide_and_round(
            i64::from(p.range) * i64::from(q_weight),
            i64::from(q_weight_denom),
        ) as i32;
    }
}

// =============================================================================
// §2. DV validity (pure, unit-testable): `is_chroma_reference`
// (common_utils.h:315-319) and `svt_aom_is_dv_valid` (adaptive_mv_pred.c:
// 1908-1975).
// =============================================================================

/// C `is_chroma_reference` (common_utils.h:315-319). `bw`/`bh` are MI-unit
/// block dims (`mi_size_wide`/`mi_size_high`, NOT pixels).
#[inline]
pub fn is_chroma_reference(mi_row: i32, mi_col: i32, bw_mi: i32, bh_mi: i32, ss_x: i32, ss_y: i32) -> bool {
    ((mi_row & 1) != 0 || (bh_mi & 1) == 0 || ss_y == 0) && ((mi_col & 1) != 0 || (bw_mi & 1) == 0 || ss_x == 0)
}

/// C `INTRABC_DELAY_PIXELS` / `INTRABC_DELAY_SB64` (inter_prediction.h:
/// 35-36): the spec's 256-pixel decode-order safety margin, in 64px-SB
/// units.
pub const INTRABC_DELAY_PIXELS: i32 = 256;
pub const INTRABC_DELAY_SB64: i32 = INTRABC_DELAY_PIXELS / 64;

/// Tile bounds in MI units (`TileInfo`'s subset this module reads).
#[derive(Debug, Clone, Copy)]
pub struct TileMiBounds {
    pub mi_col_start: i32,
    pub mi_col_end: i32,
    pub mi_row_start: i32,
    pub mi_row_end: i32,
}

/// C `svt_aom_is_dv_valid` (adaptive_mv_pred.c:1908-1975) — spec 5.11.35's
/// DV legality constraints: full-pel only, current-tile containment,
/// sub-8x8 chroma-reference edge margin, the 256px/`INTRABC_DELAY_SB64`
/// decode-order wavefront delay, and the SB64/SB128 "already-coded" +
/// south-west wavefront constraints. `dv` is eighth-pel (matches C's
/// `Mv dv` param); `bw`/`bh` are PIXEL block dims (`block_size_wide`/
/// `block_size_high` — NOT the MI-unit dims [`is_chroma_reference`] takes);
/// `bw_mi`/`bh_mi` are the MI-unit dims for the internal
/// `is_chroma_reference` call (C hardcodes `ss_x=1, ss_y=1`: 4:2:0 only,
/// matching this port's scope). `sb_size log2` is `seq_header.sb_size_log2`
/// (4 for a 64px SB, 5 for 128px — MI-unit log2, see
/// `Source/Lib/Globals/enc_handle.c:4100-4110`) and `sb_size_px` is the SB
/// size in pixels (64 or 128) — C re-derives both `max_mib_size = 1 <<
/// mib_size_log2` (MI units) and compares `sb_size == 64` directly against
/// the PIXEL SB size, so both representations are needed verbatim.
#[allow(clippy::too_many_arguments)]
pub fn is_dv_valid(
    dv: Mv,
    mi_row: i32,
    mi_col: i32,
    bw: i32,
    bh: i32,
    bw_mi: i32,
    bh_mi: i32,
    tile: TileMiBounds,
    sb_size_log2_mi: u32,
    sb_size_px: i32,
) -> bool {
    const SCALE_PX_TO_MV: i32 = 8;
    // C: `(dv.y & (scale_px_to_mv - 1)) || (dv.x & (scale_px_to_mv - 1))`.
    if (dv.y & 7) != 0 || (dv.x & 7) != 0 {
        return false;
    }
    let dv_x = i32::from(dv.x);
    let dv_y = i32::from(dv.y);

    let src_top_edge = mi_row * 4 * SCALE_PX_TO_MV + dv_y;
    let tile_top_edge = tile.mi_row_start * 4 * SCALE_PX_TO_MV;
    if src_top_edge < tile_top_edge {
        return false;
    }
    let src_left_edge = mi_col * 4 * SCALE_PX_TO_MV + dv_x;
    let tile_left_edge = tile.mi_col_start * 4 * SCALE_PX_TO_MV;
    if src_left_edge < tile_left_edge {
        return false;
    }
    let src_bottom_edge = (mi_row * 4 + bh) * SCALE_PX_TO_MV + dv_y;
    let tile_bottom_edge = tile.mi_row_end * 4 * SCALE_PX_TO_MV;
    if src_bottom_edge > tile_bottom_edge {
        return false;
    }
    let src_right_edge = (mi_col * 4 + bw) * SCALE_PX_TO_MV + dv_x;
    let tile_right_edge = tile.mi_col_end * 4 * SCALE_PX_TO_MV;
    if src_right_edge > tile_right_edge {
        return false;
    }

    // Sub-8x8 chroma-reference edge margin (planes 1/2, ss_x=ss_y=1 hardcoded
    // in C -- see fn doc). Both planes share the same `is_chroma_reference`
    // verdict since neither the mi-parity nor bw/bh depend on plane index.
    if is_chroma_reference(mi_row, mi_col, bw_mi, bh_mi, 1, 1) {
        if bw < 8 && src_left_edge < tile_left_edge + 4 * SCALE_PX_TO_MV {
            return false;
        }
        if bh < 8 && src_top_edge < tile_top_edge + 4 * SCALE_PX_TO_MV {
            return false;
        }
    }

    // Already-coded-SB + HW-decoder wavefront constraints.
    let max_mib_size = 1i32 << sb_size_log2_mi;
    let active_sb_row = mi_row >> sb_size_log2_mi;
    let active_sb64_col = (mi_col * 4) >> 6;
    let sb_size = max_mib_size * 4; // == sb_size_px, kept as a separate derivation to mirror C exactly
    debug_assert_eq!(sb_size, sb_size_px);
    let src_sb_row = ((src_bottom_edge >> 3) - 1).div_euclid(sb_size);
    let src_sb64_col = ((src_right_edge >> 3) - 1) >> 6;
    let total_sb64_per_row = ((tile.mi_col_end - tile.mi_col_start - 1) >> 4) + 1;
    let active_sb64 = active_sb_row * total_sb64_per_row + active_sb64_col;
    let src_sb64 = src_sb_row * total_sb64_per_row + src_sb64_col;
    if src_sb64 >= active_sb64 - INTRABC_DELAY_SB64 {
        return false;
    }

    let gradient = 1 + INTRABC_DELAY_SB64 + i32::from(sb_size_px > 64);
    let wf_offset = gradient * (active_sb_row - src_sb_row);
    if src_sb_row > active_sb_row || src_sb64_col >= active_sb64_col - INTRABC_DELAY_SB64 + wf_offset {
        return false;
    }

    if sb_size_px == 64 {
        if src_sb64_col > active_sb64_col + (active_sb_row - src_sb_row) {
            return false;
        }
    } else {
        let src_sb128_col = ((src_right_edge >> 3) - 1) >> 7;
        let active_sb128_col = (mi_col * 4) >> 7;
        if src_sb128_col > active_sb128_col + (active_sb_row - src_sb_row) {
            return false;
        }
    }

    true
}

// =============================================================================
// §3. ref_dv derivation: `svt_aom_find_ref_dv` (inter_prediction.c:2390-
// 2400, the spec default-DV rule) + the `dv_ref` composition rule inlined
// in `intra_bc_search` (mode_decision.c:3020-3026).
// =============================================================================

/// C `svt_aom_find_ref_dv` (inter_prediction.c:2390-2400): the spec default
/// DV when both ref-mv-stack candidates are unavailable/zero. `mib_size` is
/// `seq_header.sb_mi_size` (16 or 32 — MI units); `mi_col` is read by the
/// C signature but unused (`(void)mi_col`) — the spec's default DV is
/// horizontal-only when there's room above, vertical-only otherwise, never
/// column-dependent.
pub fn find_ref_dv(tile: TileMiBounds, mib_size: i32, mi_row: i32) -> Mv {
    let (mut y, mut x) = if mi_row - mib_size < tile.mi_row_start {
        (0, -4 * mib_size - INTRABC_DELAY_PIXELS)
    } else {
        (-4 * mib_size, 0)
    };
    y *= 8;
    x *= 8;
    Mv { x: x as i16, y: y as i16 }
}

/// The `dv_ref` composition inlined at the top of `intra_bc_search`
/// (mode_decision.c:3016-3026):
/// ```c
/// if (nearestmv.as_int == INVALID_MV) nearestmv.as_int = 0;
/// if (nearmv.as_int == INVALID_MV) nearmv.as_int = 0;
/// Mv dv_ref = nearestmv.as_int == 0 ? nearmv : nearestmv;
/// if (dv_ref.as_int == 0) svt_aom_find_ref_dv(&dv_ref, tile, sb_mi_size, mi_row, mi_col);
/// ```
/// `nearestmv`/`nearmv` are `svt_av1_find_best_ref_mvs_from_stack`'s
/// top-2 ref-mv-stack candidates for `ref_frame=INTRA_FRAME`
/// (`adaptive_mv_pred.c:2030`, **NOT translated** — see the module doc's
/// "documented only" list). Pass `Mv::INVALID` for either input when the
/// stack is empty or unavailable — a single-KEY-frame still encode's first
/// blocks always start this way (empty ref_mv_stack), so `find_ref_dv`'s
/// deterministic spec fallback is what actually fires for the interesting
/// (currently reachable) case.
pub fn resolve_dv_ref(nearestmv: Mv, nearmv: Mv, tile: TileMiBounds, mib_size: i32, mi_row: i32) -> Mv {
    let nearestmv = if nearestmv == Mv::INVALID { Mv::ZERO } else { nearestmv };
    let nearmv = if nearmv == Mv::INVALID { Mv::ZERO } else { nearmv };
    let dv_ref = if nearestmv == Mv::ZERO { nearmv } else { nearestmv };
    if dv_ref == Mv::ZERO {
        find_ref_dv(tile, mib_size, mi_row)
    } else {
        dv_ref
    }
}

// =============================================================================
// §4. Mesh/diamond search skeleton — operates on a caller-provided pixel
// slice (the SAME plane serves as both "source" and "search area": the DV
// search matches SOURCE pixels against SOURCE pixels at a candidate offset
// within the same picture, `x->plane[0].src = x->xdplane[0].pre[0] =
// pcs->ppcs->enhanced_pic`, mode_decision.c:3030-3038 -- this is a
// heuristic C itself uses for speed; only the FINAL prediction/compensation
// step (documented, not translated -- see the bottom of this file) must
// read the decoder-visible RECONSTRUCTED samples). No PictureControlSet /
// ModeDecisionContext dependency: every function below takes plain
// slices + scalars.
// =============================================================================

/// C `MAX_MVSEARCH_STEPS` (av1me.h:24).
pub const MAX_MVSEARCH_STEPS: i32 = 11;
/// C `MAX_FULL_PEL_VAL` (av1me.h:27): `(1 << (MAX_MVSEARCH_STEPS-1)) - 1`.
pub const MAX_FULL_PEL_VAL: i32 = (1 << (MAX_MVSEARCH_STEPS - 1)) - 1;
/// C `MAX_FIRST_STEP` (av1me.h:29): `1 << (MAX_MVSEARCH_STEPS-1)`.
pub const MAX_FIRST_STEP: i32 = 1 << (MAX_MVSEARCH_STEPS - 1);
/// C `AOM_INTERP_EXTEND` (definitions.h:77).
pub const AOM_INTERP_EXTEND: i32 = 4;
/// C `RD_EPB_SHIFT` (restoration.h:342).
pub const RD_EPB_SHIFT: u32 = 6;
/// C `MV_LOW` / `MV_UPP` (cabac_context_model.h:198-199): eighth-pel domain
/// clamp bounds, `±(1 << MV_IN_USE_BITS)` with `MV_IN_USE_BITS = 14`.
pub const MV_LOW: i32 = -(1 << 14);
pub const MV_UPP: i32 = 1 << 14;

/// C `svt_av1_set_mv_search_range` (av1me.c:98-123). `mv_limits` is
/// FULL-PEL (mutated in place, narrowed by intersection); `mv` is
/// EIGHTH-PEL (matches C's `const Mv* mv` — always `&dv_ref` at this
/// module's call sites).
pub fn set_mv_search_range(mv_limits: &mut FullMvLimits, mv: Mv) {
    let x = i32::from(mv.x);
    let y = i32::from(mv.y);
    let mut col_min = (x >> 3) - MAX_FULL_PEL_VAL + i32::from((x & 7) != 0);
    let mut row_min = (y >> 3) - MAX_FULL_PEL_VAL + i32::from((y & 7) != 0);
    let mut col_max = (x >> 3) + MAX_FULL_PEL_VAL;
    let mut row_max = (y >> 3) + MAX_FULL_PEL_VAL;
    col_min = col_min.max((MV_LOW >> 3) + 1);
    row_min = row_min.max((MV_LOW >> 3) + 1);
    col_max = col_max.min((MV_UPP >> 3) - 1);
    row_max = row_max.min((MV_UPP >> 3) - 1);
    if mv_limits.col_min < col_min {
        mv_limits.col_min = col_min;
    }
    if mv_limits.col_max > col_max {
        mv_limits.col_max = col_max;
    }
    if mv_limits.row_min < row_min {
        mv_limits.row_min = row_min;
    }
    if mv_limits.row_max > row_max {
        mv_limits.row_max = row_max;
    }
}

/// C `is_mv_in` (mcomp.h:126-132). Full-pel `(x, y)` against full-pel
/// `mv_limits`.
#[inline]
fn is_mv_in(mv_limits: FullMvLimits, x: i32, y: i32) -> bool {
    x >= mv_limits.col_min && x <= mv_limits.col_max && y >= mv_limits.row_min && y <= mv_limits.row_max
}

/// C `mv_check_bounds` (mode_decision.c:2964-2967). `dv` is EIGHTH-PEL
/// (checked via `dv >> 3` against full-pel `mv_limits`); returns `true`
/// when OUT of bounds, matching C's polarity exactly (`if
/// (!mv_check_bounds(...) && is_dv_valid(...))` at the call site).
#[inline]
pub fn mv_check_bounds(mv_limits: FullMvLimits, dv: Mv) -> bool {
    let y8 = i32::from(dv.y) >> 3;
    let x8 = i32::from(dv.x) >> 3;
    y8 < mv_limits.row_min || y8 > mv_limits.row_max || x8 < mv_limits.col_min || x8 > mv_limits.col_max
}

/// C `svt_av1_get_mv_joint` (rd_cost.c:45-51) as a 0..=3 table index
/// (0=ZERO, 1=HNZVZ i.e. x-only, 2=HZVNZ i.e. y-only, 3=HNZVNZ i.e. both).
#[inline]
fn mv_joint_index(x: i32, y: i32) -> usize {
    match (y == 0, x == 0) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    }
}

/// C `svt_av1_get_mv_class` (`md_rate_estimation.c`, re-exported verified
/// equivalent already in `svtav1-entropy`) -- reused directly, see
/// `svtav1_entropy::mv_coding::get_mv_class`.
pub use svtav1_entropy::mv_coding::get_mv_class;

/// C `MV_MAX` / `MV_VALS` (cabac_context_model.h:193-195). `MV_MAX_BITS =
/// MV_CLASSES + CLASS0_BITS + 2 = 14`; note this is numerically the SAME
/// as `MV_IN_USE_BITS` above but a logically distinct C constant (kept
/// separate here to mirror the two distinct C macros).
pub const MV_MAX: i32 = (1 << 14) - 1; // 16383
pub const MV_VALS: usize = (MV_MAX as usize) * 2 + 1; // 32767

/// One MV component's per-value bit-cost table — C `int32_t
/// mvcost[MV_VALS]` accessed through the `&mvcost[MV_MAX]` offset pointer
/// (`build_nmv_component_cost_table`, md_rate_estimation.c:387-444).
/// `cost(v)` mirrors that pointer's `[v]` indexing for `v` in
/// `[-MV_MAX, MV_MAX]`.
///
/// PORT-NOTE(unverified): C's actual lookup call sites (`mv_cost`/
/// `svt_mv_cost`) clamp the raw MV component to `[MV_LOW, MV_UPP] =
/// [-16384, 16384]` via `CLIP3` BEFORE indexing -- one ULP wider at each
/// end than the table's populated `[-MV_MAX, MV_MAX] = [-16383, 16383]`
/// range (`MV_MAX = 16383 != (MV_UPP>>0 relationship);` MV_MAX_BITS and
/// MV_IN_USE_BITS are both 14 but MV_MAX = 2^14-1 while MV_UPP = 2^14).
/// Reading `mvcost[MV_MAX + (-16384)] == mvcost[-1]` or `mvcost[MV_MAX +
/// 16384] == mvcost[MV_VALS]` in C reads one element before/past the
/// `MV_VALS`-sized array -- realistically unreachable (a DV component
/// would need to differ from its ref by >2048 pixels) but technically
/// present in the reference. [`MvComponentCost::cost`] clamps to the
/// table's actual populated range `[-MV_MAX, MV_MAX]` (one ULP narrower
/// than C's literal `CLIP3` bound) so this port never panics/reads
/// adjacent memory; confirm at wiring time that DV coordinates on any
/// supported frame size stay within `[-MV_MAX, MV_MAX]` eighth-pel (they
/// do for every sane frame/tile size -- MV_MAX/8 = ~2048px of headroom).
#[derive(Debug, Clone)]
pub struct MvComponentCost {
    table: Vec<i32>,
}

impl MvComponentCost {
    #[inline]
    pub fn cost(&self, v: i32) -> i32 {
        let v = v.clamp(-MV_MAX, MV_MAX);
        self.table[(MV_MAX + v) as usize]
    }
}

/// C `build_nmv_component_cost_table` (md_rate_estimation.c:387-444).
pub fn build_nmv_component_cost_table(
    comp: &svtav1_entropy::mv_coding::NmvComponent,
    precision: MvSubpelPrecision,
) -> MvComponentCost {
    use svtav1_entropy::mv_coding::{CLASS0_BITS, CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_OFFSET_BITS};

    let mut sign_cost = [0i32; 2];
    crate::quant::syntax_rate_from_cdf(&mut sign_cost, &comp.sign_cdf);
    let mut class_cost = [0i32; MV_CLASSES];
    crate::quant::syntax_rate_from_cdf(&mut class_cost, &comp.classes_cdf);
    let mut class0_cost = [0i32; CLASS0_SIZE];
    crate::quant::syntax_rate_from_cdf(&mut class0_cost, &comp.class0_cdf);
    let mut bits_cost = [[0i32; 2]; MV_OFFSET_BITS];
    for (i, row) in bits_cost.iter_mut().enumerate() {
        crate::quant::syntax_rate_from_cdf(row, &comp.bits_cdf[i]);
    }
    let mut class0_fp_cost = [[0i32; MV_FP_SIZE]; CLASS0_SIZE];
    for (i, row) in class0_fp_cost.iter_mut().enumerate() {
        crate::quant::syntax_rate_from_cdf(row, &comp.class0_fp_cdf[i]);
    }
    let mut fp_cost = [0i32; MV_FP_SIZE];
    crate::quant::syntax_rate_from_cdf(&mut fp_cost, &comp.fp_cdf);
    let mut class0_hp_cost = [0i32; 2];
    let mut hp_cost = [0i32; 2];
    if (precision as i32) > (MvSubpelPrecision::Low as i32) {
        crate::quant::syntax_rate_from_cdf(&mut class0_hp_cost, &comp.class0_hp_cdf);
        crate::quant::syntax_rate_from_cdf(&mut hp_cost, &comp.hp_cdf);
    }

    let mut table = alloc::vec![0i32; MV_VALS];
    table[MV_MAX as usize] = 0; // mvcost[0] = 0
    for v in 1..=MV_MAX {
        let z = v - 1;
        let (c, o) = get_mv_class(z);
        let c = c as usize;
        let mut cost = class_cost[c];
        let d = o >> 3;
        let f = (o >> 1) & 3;
        let e = o & 1;
        if c == 0 {
            cost += class0_cost[d as usize];
        } else {
            let b = c + CLASS0_BITS - 1;
            for i in 0..b {
                cost += bits_cost[i][((d >> i) & 1) as usize];
            }
        }
        if (precision as i32) > (MvSubpelPrecision::None as i32) {
            if c == 0 {
                cost += class0_fp_cost[d as usize][f as usize];
            } else {
                cost += fp_cost[f as usize];
            }
            if (precision as i32) > (MvSubpelPrecision::Low as i32) {
                if c == 0 {
                    cost += class0_hp_cost[e as usize];
                } else {
                    cost += hp_cost[e as usize];
                }
            }
        }
        table[(MV_MAX + v) as usize] = cost + sign_cost[0];
        table[(MV_MAX - v) as usize] = cost + sign_cost[1];
    }
    MvComponentCost { table }
}

/// Full DV/MV rate-cost tables — C `svt_av1_build_nmv_cost_table`
/// (md_rate_estimation.c:446-451). `comps[0]` = vertical/row/Y,
/// `comps[1]` = horizontal/col/X (matches `NmvContext`'s field order).
#[derive(Debug, Clone)]
pub struct MvCostTables {
    pub joint_cost: [i32; svtav1_entropy::mv_coding::MV_JOINTS],
    pub comp_cost: [MvComponentCost; 2],
}

pub fn build_nmv_cost_table(ctx: &NmvContext, precision: MvSubpelPrecision) -> MvCostTables {
    let mut joint_cost = [0i32; svtav1_entropy::mv_coding::MV_JOINTS];
    crate::quant::syntax_rate_from_cdf(&mut joint_cost, &ctx.joints_cdf);
    MvCostTables {
        joint_cost,
        comp_cost: [
            build_nmv_component_cost_table(&ctx.comps[0], precision),
            build_nmv_component_cost_table(&ctx.comps[1], precision),
        ],
    }
}

/// C `mv_cost` (rd_cost.c:53-58) / the textually-identical `svt_mv_cost`
/// (mcomp.h:138-141). `diff` is eighth-pel (`(mv - ref)` at RD call sites,
/// or a raw full-pel-domain diff at search call sites -- both share this
/// one lookup shape since [`MvComponentCost::cost`] just indexes by value).
#[inline]
pub fn mv_table_cost(diff_x: i32, diff_y: i32, tables: &MvCostTables) -> i32 {
    tables.joint_cost[mv_joint_index(diff_x, diff_y)]
        + tables.comp_cost[0].cost(diff_y)
        + tables.comp_cost[1].cost(diff_x)
}

/// C `PIXEL_TRANSFORM_ERROR_SCALE` (av1me.c:124).
const PIXEL_TRANSFORM_ERROR_SCALE: u32 = 4;

/// C `svt_aom_mv_err_cost` (av1me.c:141-149): the search's "precise" MV
/// rate-distortion cost, in SSD-comparable units (used only inside
/// [`get_mvpred_var`], NOT inside the diamond/mesh SAD-domain search --
/// see [`mvsad_err_cost`] for that one). `mv`/`ref_mv` eighth-pel.
pub fn mv_err_cost(mv: Mv, ref_mv: Mv, tables: &MvCostTables, error_per_bit: i32) -> i32 {
    let diff_x = i32::from(mv.x) - i32::from(ref_mv.x);
    let diff_y = i32::from(mv.y) - i32::from(ref_mv.y);
    let cost = i64::from(mv_table_cost(diff_x, diff_y, tables)) * i64::from(error_per_bit);
    round_power_of_two_64(
        cost,
        7 /* RDDIV_BITS */ + 9 /* AV1_PROB_COST_SHIFT */ - RD_EPB_SHIFT + PIXEL_TRANSFORM_ERROR_SCALE,
    ) as i32
}

/// C `svt_aom_mv_err_cost_light` (av1me.c:126-132): the `approx_inter_rate`
/// fast-path cost, independent of any cost table.
pub fn mv_err_cost_light(mv: Mv, ref_mv: Mv) -> i32 {
    const FACTOR: i32 = 50;
    let absdx = (i32::from(mv.x) - i32::from(ref_mv.x)).abs();
    let absdy = (i32::from(mv.y) - i32::from(ref_mv.y)).abs();
    1296 + FACTOR * (absdx + absdy)
}

/// C `mvsad_err_cost` (av1me.c:150-157, `static`): the diamond/mesh
/// search's SAD-domain MV cost. `mv`/`ref_mv` here are FULL-PEL (matches
/// C's call sites inside `diamond_search_sad_c`/`exhaustive_mesh_search`,
/// which pass full-pel candidates and a full-pel `fcenter_mv`); internally
/// C multiplies both components by 8 before reusing the eighth-pel
/// [`mv_table_cost`] lookup.
///
/// PORT-NOTE(unverified): `mvsad_err_cost`/`svt_av1_refining_search_sad`/
/// `full_pixel_diamond`/`exhaustive_mesh_search` are all C `static`
/// functions with no exported symbol -- see this file's `PORT-NOTE(un-
/// verified)` index summary in the module doc for the upgrade path
/// (a `ref_shims.c` wrapper, matching this project's established pattern
/// for hard-to-reach C internals, e.g. `palette.rs`'s six `static`-only
/// functions).
pub fn mvsad_err_cost(mv_x: i32, mv_y: i32, ref_mv_x: i32, ref_mv_y: i32, sad_per_bit: i32, approx_inter_rate: bool, tables: &MvCostTables) -> i32 {
    if approx_inter_rate {
        return mvsad_err_cost_light(mv_x, mv_y, ref_mv_x, ref_mv_y);
    }
    let diff_x = (mv_x - ref_mv_x) * 8;
    let diff_y = (mv_y - ref_mv_y) * 8;
    let cost = mv_table_cost(diff_x, diff_y, tables);
    // C: `ROUND_POWER_OF_TWO((unsigned)cost * sad_per_bit, AV1_PROB_COST_SHIFT)`.
    // `cost`/`sad_per_bit` are always non-negative in practice (rate-cost
    // tables and per-bit scalars are never negative), so plain i32
    // `wrapping_mul` reproduces the C `unsigned` multiply bit-for-bit
    // without needing an actual unsigned type here.
    round_power_of_two(cost.wrapping_mul(sad_per_bit), 9 /* AV1_PROB_COST_SHIFT */)
}

/// C `mvsad_err_cost_light` (av1me.c:134-139, `static`).
fn mvsad_err_cost_light(mv_x: i32, mv_y: i32, ref_mv_x: i32, ref_mv_y: i32) -> i32 {
    const FACTOR: i32 = 50;
    let absdx = (mv_x - ref_mv_x).unsigned_abs() as i32 * 8;
    let absdy = (mv_y - ref_mv_y).unsigned_abs() as i32 * 8;
    1296 + FACTOR * (absdx + absdy)
}

// Addressing convention for every function below: `pic`/`stride` is the
// picture's WHOLE luma plane starting at its true pixel (0,0) (or at
// least a region whose row 0 IS the picture's row 0 -- e.g. a
// left/top-padded plane, per `pad_to_multiple_of_8`-style conventions
// elsewhere in this crate); `block_origin` is the CURRENT block's
// absolute `(x, y)` pixel position. Every candidate position stays a
// RELATIVE offset from `block_origin` throughout the search (matching
// C's `Mv`/`MvLimits` semantics exactly -- `mv_limits` are derived
// relative to the block, see §4's per-direction functions), and is only
// converted to an ABSOLUTE picture address at the point of a pixel read,
// via [`window`]. This sidesteps C's raw-pointer-into-a-shared-buffer
// addressing (which can legally go negative relative to a per-block
// sub-pointer -- reading rows/columns above/left of the current block in
// the SAME larger buffer) without needing negative slice indices: as
// long as `mv_limits` keep every reachable relative position within the
// tile/frame (which [`direction_mv_limits`]/[`set_mv_search_range`]
// guarantee), the resulting ABSOLUTE position is always `>= 0`.

/// Absolute-position pixel window starting at `(block_origin.0 + rel_x,
/// block_origin.1 + rel_y)` within `pic`. See this section's addressing
/// convention note above for why `rel_x`/`rel_y` (which may themselves be
/// negative, matching C's `Mv` components) always resolve to a
/// non-negative absolute position in practice.
#[inline]
fn window(pic: &[u8], stride: usize, block_origin: (i32, i32), rel_x: i32, rel_y: i32) -> &[u8] {
    let abs_x = block_origin.0 + rel_x;
    let abs_y = block_origin.1 + rel_y;
    debug_assert!(
        abs_x >= 0 && abs_y >= 0,
        "PORT-NOTE(unverified): search produced an out-of-picture position \
         ({abs_x}, {abs_y}) -- indicates mv_limits weren't properly tile/frame- \
         bounded before calling into §4's search primitives."
    );
    &pic[abs_y as usize * stride + abs_x as usize..]
}

/// C `svt_av1_get_mvpred_var` (av1me.c:196-209): SAD between the block and
/// the candidate `best_mv` location, plus (if `use_mvcost`) the precise
/// [`mv_err_cost`]/[`mv_err_cost_light`] rate term. `best_mv_x`/`best_mv_y`
/// are FULL-PEL offsets from `block_origin` (C's `best_mv`); `center_mv`
/// (the ref for cost purposes) is EIGHTH-PEL, matching C's `Mv mv =
/// {best_mv->x*8, best_mv->y*8}` conversion done internally.
#[allow(clippy::too_many_arguments)]
pub fn get_mvpred_var(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    best_mv_x: i32,
    best_mv_y: i32,
    center_mv: Mv,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
    use_mvcost: bool,
) -> i32 {
    let what = window(pic, stride, block_origin, 0, 0);
    let cand = window(pic, stride, block_origin, best_mv_x, best_mv_y);
    let sad = svtav1_dsp::sad::sad(what, stride, cand, stride, bw, bh) as i32;
    let mv = Mv {
        x: (best_mv_x * 8) as i16,
        y: (best_mv_y * 8) as i16,
    };
    let rate = if !use_mvcost {
        0
    } else if approx_inter_rate {
        mv_err_cost_light(mv, center_mv)
    } else {
        mv_err_cost(mv, center_mv, tables, error_per_bit)
    };
    sad + rate
}

/// C `SearchSite` / `SearchSiteConfig` (av1me.h) + `svt_av1_init3smotion_
/// compensation` (av1me.c:159-184): the 3-step-search 8-direction-per-step
/// offset table, `MAX_FIRST_STEP` down to `1` by halving (11 steps x 8 +
/// 1 origin = 89 sites total for the default `MAX_FIRST_STEP = 1024`).
/// `offset` is unused by this port (addresses are always recomputed from
/// `(mv_x, mv_y)` via [`window`] rather than accumulated incrementally --
/// see this section's header note) but kept on the struct for a faithful
/// 1:1 field mirror of C's `SearchSite`.
#[derive(Debug, Clone, Copy)]
pub struct SearchSite {
    pub mv_x: i32,
    pub mv_y: i32,
    /// C `ss->offset` (`mv_y * stride + mv_x`) -- NOT used by this port's
    /// address computation (see struct doc); kept for field parity.
    pub offset: isize,
}

#[derive(Debug, Clone)]
pub struct SearchSiteConfig {
    pub sites: Vec<SearchSite>,
    pub searches_per_step: usize,
}

pub fn init_search_sites(stride: usize) -> SearchSiteConfig {
    let mut sites = Vec::with_capacity(89);
    sites.push(SearchSite { mv_x: 0, mv_y: 0, offset: 0 });
    let mut len = MAX_FIRST_STEP;
    while len > 0 {
        let ss_mvs: [(i32, i32); 8] = [
            (0, -len),
            (0, len),
            (-len, 0),
            (len, 0),
            (-len, -len),
            (len, -len),
            (-len, len),
            (len, len),
        ];
        for (mv_x, mv_y) in ss_mvs {
            sites.push(SearchSite {
                mv_x,
                mv_y,
                offset: mv_y as isize * stride as isize + mv_x as isize,
            });
        }
        len /= 2;
    }
    SearchSiteConfig { sites, searches_per_step: 8 }
}

/// C `svt_av1_diamond_search_sad_c` (av1me.c:291-420, EXPORTED symbol --
/// strongest evidence tier of this section).
///
/// C's signature takes TWO logically distinct MVs: `ref_mv` (mutated in
/// place by `clamp_mv` into the search's full-pel STARTING point, then
/// copied into `best_mv`) and `center_mv` (eighth-pel, used ONLY to derive
/// `fcenter_mv = {center_mv->x>>3, center_mv->y>>3}` for
/// [`mvsad_err_cost`]'s rate term -- never used as a search seed). At this
/// module's one call site (`full_pixel_diamond`, IBC only) both resolve to
/// the SAME underlying value: `ref_mv` is passed as `mvp_full = dv_ref >>
/// 3` and `center_mv` is passed as `dv_ref` itself, so `fcenter_mv ==
/// ref_mv` exactly. This port folds them into the single parameter
/// `center_eighth_pel` (= `dv_ref`) and derives BOTH the full-pel seed
/// (via `>>3` + clamp, mirroring `clamp_mv(ref_mv, ...)`) and the cost
/// reference (`fcenter_mv`) from it -- faithful to this vertical's actual
/// data flow, not a general-purpose `diamond_search_sad_c` binding (a
/// general port would need the two kept separate; document this if ever
/// reused outside IBC).
///
/// C accumulates `best_address` incrementally (`best_address +=
/// ss[best_site].offset`) to avoid recomputing a pointer from scratch each
/// improvement; this port recomputes the absolute address from `(best_x,
/// best_y)` via [`window`] on every pixel read instead -- provably
/// equivalent (both resolve to the SAME absolute position after the same
/// sequence of site moves) and simpler than threading a second piece of
/// mutable state that must stay in lock-step with `(best_x, best_y)`.
///
/// The `#if defined(NEW_DIAMOND_SEARCH)` refinement loop (av1me.c:391-408)
/// is DEAD CODE in the reference build (`NEW_DIAMOND_SEARCH` is never
/// `#define`d anywhere in `Source/`) and is NOT translated.
#[allow(clippy::too_many_arguments)]
pub fn diamond_search_sad(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    center_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    search_param: i32,
    sad_per_bit: i32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32, i32) {
    // Returns (best_x, best_y, best_sad, num00).
    let fcenter_x = i32::from(center_eighth_pel.x) >> 3;
    let fcenter_y = i32::from(center_eighth_pel.y) >> 3;
    let mut best_x = fcenter_x.clamp(mv_limits.col_min, mv_limits.col_max);
    let mut best_y = fcenter_y.clamp(mv_limits.row_min, mv_limits.row_max);
    let (start_x, start_y) = (best_x, best_y);
    let mut num00 = 0i32;

    let ss = &cfg.sites[(search_param as usize) * cfg.searches_per_step..];
    let tot_steps = (cfg.sites.len() / cfg.searches_per_step) as i32 - search_param;

    let what = window(pic, stride, block_origin, 0, 0);
    let mut bestsad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, best_x, best_y), stride, bw, bh) as i32
        + mvsad_err_cost(best_x, best_y, fcenter_x, fcenter_y, sad_per_bit, approx_inter_rate, tables);

    let mut i = 1usize;
    let mut best_site = 0usize;
    let mut last_site = 0usize;

    for _step in 0..tot_steps {
        let all_in = (best_y + ss[i].mv_y) > mv_limits.row_min
            && (best_y + ss[i + 1].mv_y) < mv_limits.row_max
            && (best_x + ss[i + 2].mv_x) > mv_limits.col_min
            && (best_x + ss[i + 3].mv_x) < mv_limits.col_max;

        if all_in {
            for _ in (0..cfg.searches_per_step).step_by(4) {
                for t in 0..4 {
                    let this_x = best_x + ss[i + t].mv_x;
                    let this_y = best_y + ss[i + t].mv_y;
                    let mut sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, this_x, this_y), stride, bw, bh) as i32;
                    if sad < bestsad {
                        sad += mvsad_err_cost(this_x, this_y, fcenter_x, fcenter_y, sad_per_bit, approx_inter_rate, tables);
                        if sad < bestsad {
                            bestsad = sad;
                            best_site = i + t;
                        }
                    }
                }
                i += 4;
            }
        } else {
            for _ in 0..cfg.searches_per_step {
                let this_x = best_x + ss[i].mv_x;
                let this_y = best_y + ss[i].mv_y;
                if is_mv_in(mv_limits, this_x, this_y) {
                    let mut sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, this_x, this_y), stride, bw, bh) as i32;
                    if sad < bestsad {
                        sad += mvsad_err_cost(this_x, this_y, fcenter_x, fcenter_y, sad_per_bit, approx_inter_rate, tables);
                        if sad < bestsad {
                            bestsad = sad;
                            best_site = i;
                        }
                    }
                }
                i += 1;
            }
        }
        if best_site != last_site {
            best_y += ss[best_site].mv_y;
            best_x += ss[best_site].mv_x;
            last_site = best_site;
        } else if (best_x, best_y) == (start_x, start_y) {
            num00 += 1;
        }
    }

    (best_x, best_y, bestsad, num00)
}

/// C `svt_av1_refining_search_sad` (av1me.c:420-460ish, `static`): 1-away
/// 4-neighbor diamond refinement, up to `search_range` iterations. C names
/// its own cost-scale parameter `error_per_bit`, but every call site in
/// this vertical (`full_pixel_diamond`) actually passes `sadpb` (the
/// SAD-domain per-bit value) into it, and the parameter flows straight
/// into [`mvsad_err_cost`]'s `sad_per_bit` argument -- so this port names
/// it `sad_per_bit` directly rather than carrying the C misnomer forward.
#[allow(clippy::too_many_arguments)]
pub fn refining_search_sad(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    start_x: i32,
    start_y: i32,
    center_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    search_range: i32,
    sad_per_bit: i32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    const NEIGHBORS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
    let fcenter_x = i32::from(center_eighth_pel.x) >> 3;
    let fcenter_y = i32::from(center_eighth_pel.y) >> 3;
    let mut x = start_x;
    let mut y = start_y;
    let what = window(pic, stride, block_origin, 0, 0);
    let mut best_sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, x, y), stride, bw, bh) as i32
        + mvsad_err_cost(x, y, fcenter_x, fcenter_y, sad_per_bit, approx_inter_rate, tables);

    for _ in 0..search_range {
        let mut best_site: Option<usize> = None;
        for (j, (dx, dy)) in NEIGHBORS.iter().enumerate() {
            let nx = x + dx;
            let ny = y + dy;
            if is_mv_in(mv_limits, nx, ny) {
                let mut sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, nx, ny), stride, bw, bh) as i32;
                if sad < best_sad {
                    sad += mvsad_err_cost(nx, ny, fcenter_x, fcenter_y, sad_per_bit, approx_inter_rate, tables);
                    if sad < best_sad {
                        best_sad = sad;
                        best_site = Some(j);
                    }
                }
            }
        }
        match best_site {
            None => break,
            Some(j) => {
                let (dx, dy) = NEIGHBORS[j];
                x += dx;
                y += dy;
            }
        }
    }
    (x, y, best_sad)
}

/// C `full_pixel_diamond` (av1me.c:489-556, `static`): the diamond search
/// entry. Runs [`diamond_search_sad`] at `step_param`, then re-runs it at
/// deepening `step_param + n` levels (each restarting from the SAME
/// `center_eighth_pel`-derived seed -- C reuses the same `mvp_full`
/// pointer across every `svt_av1_diamond_search_sad_c` call in this
/// function, and `clamp_mv` is idempotent after its first application, so
/// every level searches outward from one fixed origin, keeping whichever
/// level's result scores best) while `num00` (consecutive "didn't move"
/// steps) permits skipping ahead, then -- unless a shallow level already
/// exhausted `further_steps` or a deep level's own `num00` used up the
/// remaining budget -- a final 1-away [`refining_search_sad`] pass seeded
/// from the best point found so far. Returns `(best_x, best_y, best_cost)`
/// full-pel, where `best_cost` is [`get_mvpred_var`]'s precise
/// (`use_mvcost=true`) value at the winning point (C recomputes this via
/// `svt_av1_get_mvpred_var` after EVERY `diamond_search_sad_c`/
/// `refining_search_sad` call, never trusting the raw SAD return directly
/// for the `bestsme` comparison -- mirrored exactly below).
#[allow(clippy::too_many_arguments)]
pub fn full_pixel_diamond(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    mv_limits: FullMvLimits,
    step_param: i32,
    sadpb: i32,
    further_steps: i32,
    do_refine_in: bool,
    center_eighth_pel: Mv,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    let (mut best_x, mut best_y, _sad0, mut n) = diamond_search_sad(
        pic,
        stride,
        block_origin,
        bw,
        bh,
        cfg,
        center_eighth_pel,
        mv_limits,
        step_param,
        sadpb,
        tables,
        approx_inter_rate,
    );
    let mut bestsme = get_mvpred_var(
        pic, stride, block_origin, bw, bh, best_x, best_y, center_eighth_pel, tables, error_per_bit,
        approx_inter_rate, true,
    );

    let mut do_refine = do_refine_in;
    if n > further_steps {
        do_refine = false;
    }

    while n < further_steps {
        n += 1;
        let (cand_x, cand_y, _sad, num00) = diamond_search_sad(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            cfg,
            center_eighth_pel,
            mv_limits,
            step_param + n,
            sadpb,
            tables,
            approx_inter_rate,
        );
        if num00 > further_steps - n {
            do_refine = false;
        }
        let thissme = get_mvpred_var(
            pic, stride, block_origin, bw, bh, cand_x, cand_y, center_eighth_pel, tables, error_per_bit,
            approx_inter_rate, true,
        );
        if thissme < bestsme {
            bestsme = thissme;
            best_x = cand_x;
            best_y = cand_y;
        }
    }

    if do_refine {
        const SEARCH_RANGE: i32 = 8;
        let (rx, ry, _rsad) = refining_search_sad(
            pic,
            stride,
            block_origin,
            bw,
            bh,
            best_x,
            best_y,
            center_eighth_pel,
            mv_limits,
            SEARCH_RANGE,
            sadpb,
            tables,
            approx_inter_rate,
        );
        let thissme = get_mvpred_var(
            pic, stride, block_origin, bw, bh, rx, ry, center_eighth_pel, tables, error_per_bit, approx_inter_rate,
            true,
        );
        if thissme < bestsme {
            bestsme = thissme;
            best_x = rx;
            best_y = ry;
        }
    }

    (best_x, best_y, bestsme)
}

/// C `exhaustive_mesh_search` (av1me.c:212-290, `static`): a full raster
/// scan of the `[-range, range]` window around `center_full_pel` (clamped
/// into `mv_limits`), stepping by `step` rows and (`step>1 ? step : 4`)
/// columns. `ref_mv_full` is the full-pel cost reference (matches C's
/// `ref_mv` param -- always the caller's full-pel `ref_mv_fp` in this
/// vertical, itself `ref_mv_eighth_pel >> 3`, see
/// [`intrabc_full_pixel_exhaustive`]).
///
/// C's `x->second_best_mv` write in this function is never READ anywhere
/// in the IBC call chain (`full_pixel_search` -> `intrabc_full_pixel_
/// exhaustive` -> here); it exists for OTHER callers of this shared ME
/// primitive (regular inter ME). Not carried by this port.
///
/// PORT-NOTE(unverified): when `step == 1` and the tail of a row has fewer
/// than 4 remaining columns (`c + 3 > end_col`), C's own scalar fallback
/// loop is `for (i = 0; i < end_col - c; ++i)` -- note the STRICT `<`
/// against `end_col - c`, NOT `end_col - c + 1`. This means column
/// `end_col` itself is skipped by the tail branch whenever it is reached
/// via that branch (e.g. exactly 1 column remaining: `end_col - c == 0`,
/// zero iterations, `end_col` never visited). Reproduced bug-for-bug
/// below (`n = (end_col - c).max(0)`, not `+1`) rather than "fixed" --
/// see `CLAUDE.md`'s "translate exactly" mandate. Only affects the last
/// 0-3 columns of a mesh-search row when the window width isn't a
/// multiple of 4; `range`/`interval` come from [`IbcCtrls::mesh_patterns`]
/// (always multiples of 8 or the terminal 1-wide refinement pass, so this
/// mistranscription-shaped gap is rarely if ever exercised, but is kept
/// faithful regardless).
#[allow(clippy::too_many_arguments)]
pub fn exhaustive_mesh_search(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    ref_mv_full: (i32, i32),
    range: i32,
    step: i32,
    sad_per_bit: i32,
    center_full_pel: (i32, i32),
    mv_limits: FullMvLimits,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> ((i32, i32), i32) {
    debug_assert!(step >= 1);
    let col_step = if step > 1 { step } else { 4 };
    let what = window(pic, stride, block_origin, 0, 0);

    let fcx = center_full_pel.0.clamp(mv_limits.col_min, mv_limits.col_max);
    let fcy = center_full_pel.1.clamp(mv_limits.row_min, mv_limits.row_max);
    let mut best_mv = (fcx, fcy);
    let mut best_sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, fcx, fcy), stride, bw, bh) as i32
        + mvsad_err_cost(fcx, fcy, ref_mv_full.0, ref_mv_full.1, sad_per_bit, approx_inter_rate, tables);

    let start_row = (-range).max(mv_limits.row_min - fcy);
    let start_col = (-range).max(mv_limits.col_min - fcx);
    let end_row = range.min(mv_limits.row_max - fcy);
    let end_col = range.min(mv_limits.col_max - fcx);

    let mut r = start_row;
    while r <= end_row {
        let mut c = start_col;
        while c <= end_col {
            let n = if step > 1 {
                1
            } else if c + 3 <= end_col {
                4
            } else {
                (end_col - c).max(0)
            };
            for i in 0..n {
                let mx = fcx + c + i;
                let my = fcy + r;
                let sad = svtav1_dsp::sad::sad(what, stride, window(pic, stride, block_origin, mx, my), stride, bw, bh) as i32;
                if sad < best_sad {
                    let sad2 = sad + mvsad_err_cost(mx, my, ref_mv_full.0, ref_mv_full.1, sad_per_bit, approx_inter_rate, tables);
                    if sad2 < best_sad {
                        best_sad = sad2;
                        best_mv = (mx, my);
                    }
                }
            }
            c += col_step;
        }
        r += step;
    }
    (best_mv, best_sad)
}

/// C `MIN_RANGE` / `MAX_RANGE` / `MIN_INTERVAL` (av1me.c:558-560).
const MIN_RANGE: i32 = 7;
const MAX_RANGE: i32 = 256;
const MIN_INTERVAL: i32 = 1;

/// C `intrabc_full_pixel_exhaustive` (av1me.c:566-625, `static`): runs
/// [`exhaustive_mesh_search`] over `ctrls.mesh_patterns`, adapting the
/// first ring's range/interval to the center MV's magnitude, then
/// progressively refining through the remaining configured rings (a
/// `range == 0` entry -- the zero-default tail of [`IbcCtrls::mesh_
/// patterns`] at levels 6/7, or any level's unused trailing slots --
/// terminates the refinement early; an `interval == 1` ring is always the
/// last one run). Returns `None` when the validated first ring is
/// malformed (`range`/`interval` outside `[MIN_RANGE,MAX_RANGE]`/
/// `[MIN_INTERVAL,range]`) -- C's `INT_MAX` "not found" sentinel, e.g.
/// self-consistently a no-op whenever `mesh_patterns[0]` is left
/// zero-defaulted (levels 6/7, see [`IbcCtrls::for_level`]'s PORT-NOTE).
/// `center_full_pel` is `x->best_mv` at the call site (i.e. the diamond
/// search's winner, full-pel).
#[allow(clippy::too_many_arguments)]
pub fn intrabc_full_pixel_exhaustive(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    ctrls: &IbcCtrls,
    center_full_pel: (i32, i32),
    sad_per_bit: i32,
    ref_mv_eighth_pel: Mv,
    mv_limits: FullMvLimits,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
) -> Option<((i32, i32), i32)> {
    let ref_mv_full = (i32::from(ref_mv_eighth_pel.x) >> 3, i32::from(ref_mv_eighth_pel.y) >> 3);

    let mut range = ctrls.mesh_patterns[0].range;
    let interval0 = ctrls.mesh_patterns[0].interval;
    if !(MIN_RANGE..=MAX_RANGE).contains(&range) || !(MIN_INTERVAL..=range).contains(&interval0) {
        return None;
    }
    let base_interval_div = range / interval0;

    let mv_mag = center_full_pel.0.abs().max(center_full_pel.1.abs());
    range = range.max((5 * mv_mag) / 4).min(MAX_RANGE);
    let interval = interval0.max(range / base_interval_div);

    let (mut search_mv, mut best_cost) = exhaustive_mesh_search(
        pic, stride, block_origin, bw, bh, ref_mv_full, range, interval, sad_per_bit, center_full_pel, mv_limits,
        tables, approx_inter_rate,
    );

    // C's own `interval` local is never re-read after this gate (each
    // refinement ring below uses `pattern.interval` directly, and the
    // gate itself is checked ONCE, not per-iteration) -- matched exactly,
    // no reassignment inside the loop.
    if interval > MIN_INTERVAL && range > MIN_RANGE {
        for pattern in &ctrls.mesh_patterns[1..MAX_MESH_STEP] {
            if pattern.range == 0 {
                break;
            }
            let (mv2, cost2) = exhaustive_mesh_search(
                pic, stride, block_origin, bw, bh, ref_mv_full, pattern.range, pattern.interval, sad_per_bit,
                search_mv, mv_limits, tables, approx_inter_rate,
            );
            search_mv = mv2;
            best_cost = cost2;
            if pattern.interval == 1 {
                break;
            }
        }
    }

    if best_cost < i32::MAX {
        best_cost = get_mvpred_var(
            pic, stride, block_origin, bw, bh, search_mv.0, search_mv.1, ref_mv_eighth_pel, tables, error_per_bit,
            approx_inter_rate, true,
        );
    }
    Some((search_mv, best_cost))
}

/// C `svt_av1_full_pixel_search` (av1me.c:1115-1155, EXPORTED symbol): the
/// diamond-then-optional-mesh entry point. `mvp_full` (the diamond seed)
/// is `ref_mv_eighth_pel >> 3` in every call site of this vertical --
/// see [`diamond_search_sad`]'s doc for why this port folds "seed" and
/// "cost center" into the one `ref_mv_eighth_pel` parameter.
///
/// The `(uint64_t)~0` "always mesh" encoding ([`IbcCtrls::for_level`]
/// levels 6/7): C narrows `intrabc_ctrls->exhaustive_mesh_thresh` (u64)
/// to a plain `int` via `(int)pcs->ppcs->intrabc_ctrls.exhaustive_mesh_
/// thresh` BEFORE the bsize-scaled right-shift -- `(int)(uint64_t)~0`
/// truncates to the low 32 bits, reinterpreted as `-1` (two's complement,
/// implementation-defined by the C standard but universal in practice on
/// every mainstream target, matching Rust's `as i32` narrowing cast
/// exactly). `-1 >> n == -1` for any `n` (arithmetic shift, sign-extends),
/// so `var > -1` is true for every realistic non-negative SAD/variance
/// `var`, i.e. mesh search always fires -- exactly the comment's intent
/// ("set to INF to always allow mesh search"), achieved through this
/// specific integer-truncation trick rather than a dedicated bool flag.
#[allow(clippy::too_many_arguments)]
pub fn full_pixel_search(
    pic: &[u8],
    stride: usize,
    block_origin: (i32, i32),
    bw: usize,
    bh: usize,
    cfg: &SearchSiteConfig,
    mv_limits: FullMvLimits,
    ref_mv_eighth_pel: Mv,
    sad_per_bit: i32,
    error_per_bit: i32,
    ctrls: &IbcCtrls,
    mi_size_wide_log2: u32,
    mi_size_high_log2: u32,
    tables: &MvCostTables,
    approx_inter_rate: bool,
) -> (i32, i32, i32) {
    const STEP_PARAM: i32 = 0;
    let (mut best_x, mut best_y, mut var) = full_pixel_diamond(
        pic,
        stride,
        block_origin,
        bw,
        bh,
        cfg,
        mv_limits,
        STEP_PARAM,
        sad_per_bit,
        MAX_MVSEARCH_STEPS - 1 - STEP_PARAM,
        true,
        ref_mv_eighth_pel,
        tables,
        error_per_bit,
        approx_inter_rate,
    );

    // `10 - (mi_size_wide_log2 + mi_size_high_log2)` is C `int` arithmetic
    // (uint8_t operands promote to signed `int`) and could in principle go
    // negative for a hypothetical bsize with mi-log2 sum > 10 -- shifting
    // by a negative amount would be C UB. Every real BLOCK_SIZES_ALL entry
    // has `mi_size_wide_log2 + mi_size_high_log2 <= 10` (the max, 5+5, is
    // BLOCK_128X128 itself), so the shift amount is always in `0..=10` in
    // practice; assert that invariant rather than silently wrapping a u32
    // subtraction.
    debug_assert!(mi_size_wide_log2 + mi_size_high_log2 <= 10);
    let mut exhaustive_mesh_thresh = ctrls.exhaustive_mesh_thresh as i32; // see fn doc
    exhaustive_mesh_thresh >>= 10 - (mi_size_wide_log2 + mi_size_high_log2);

    let mut run_mesh_search = var > exhaustive_mesh_thresh;
    let mvp_full_x = i32::from(ref_mv_eighth_pel.x) >> 3;
    let mvp_full_y = i32::from(ref_mv_eighth_pel.y) >> 3;
    let full_pel_mv_diff = (mvp_full_x - best_x).abs().max((mvp_full_y - best_y).abs());
    if full_pel_mv_diff <= ctrls.mesh_search_mv_diff_threshold {
        run_mesh_search = false;
    }

    if run_mesh_search {
        if let Some(((ex_x, ex_y), var_ex)) = intrabc_full_pixel_exhaustive(
            pic, stride, block_origin, bw, bh, ctrls, (best_x, best_y), sad_per_bit, ref_mv_eighth_pel, mv_limits,
            tables, error_per_bit, approx_inter_rate,
        ) {
            if var_ex < var {
                best_x = ex_x;
                best_y = ex_y;
                var = var_ex;
            }
        }
    }

    (best_x, best_y, var)
}

// =============================================================================
// §4b. Hash search -- the SELECTION algorithm only. The hash TABLE itself
// (CRC-based block hashing + bucket storage) is NOT translated -- see the
// module doc's "documented only" list. `hash_search_eligible` +
// `hash_search_best_in_bucket` translate the reachable, pure parts of
// `svt_av1_intrabc_hash_search` (av1me.c:1056-1114); the caller is
// responsible for producing the bucket (via an unported `HashTable`
// equivalent) and the block's own `(hash_value1, hash_value2)` (via an
// unported `svt_av1_get_block_hash_value`, hash_motion.c:309+). Passing an
// empty/`None` bucket is always LEGAL and just means every DV this port
// finds comes from [`full_pixel_search`] instead -- correct, only slower.
// =============================================================================

/// C `BlockHash` (hash_motion.h:26-30): one hash-table bucket entry.
/// `x`/`y` are the ABSOLUTE picture-pixel origin of the hashed block
/// (`int16_t` in C; widened to `i32` here to avoid repeated casts in the
/// arithmetic below).
#[derive(Debug, Clone, Copy)]
pub struct BlockHashEntry {
    pub x: i32,
    pub y: i32,
    pub hash_value2: u32,
}

/// C's inline eligibility gate at the top of `svt_av1_intrabc_hash_search`
/// (av1me.c:1063-1065): hash search only applies to SQUARE blocks no
/// larger than `max_block_size_hash`.
#[inline]
pub fn hash_search_eligible(bw: i32, bh: i32, max_block_size_hash: u8) -> bool {
    bw == bh && bw <= i32::from(max_block_size_hash)
}

/// C `svt_av1_intrabc_hash_search`'s bucket-scan loop (av1me.c:1080-1113),
/// given an already-fetched `bucket` (every entry sharing the probed
/// block's `hash_value1` -- `svt_av1_hash_table_count` +
/// `svt_av1_hash_get_first_iterator`, NOT translated). `hash_value2` is
/// the probed block's own second-stage hash (`svt_av1_get_block_hash_
/// value`, NOT translated). `(x_pos, y_pos)` is the block's absolute
/// pixel origin. Returns the best `(dv, cost)` found, `None` if nothing in
/// the bucket validates (including C's own `count <= 1` "self-only
/// bucket" early return -- `intra` is always `1` at this vertical's one
/// call site, so that C ternary always resolves to `1`).
#[allow(clippy::too_many_arguments)]
pub fn hash_search_best_in_bucket(
    bucket: &[BlockHashEntry],
    hash_value2: u32,
    x_pos: i32,
    y_pos: i32,
    mi_row: i32,
    mi_col: i32,
    bw: i32,
    bh: i32,
    bw_mi: i32,
    bh_mi: i32,
    tile: TileMiBounds,
    sb_size_log2_mi: u32,
    sb_size_px: i32,
    mv_limits: FullMvLimits,
    pic: &[u8],
    stride: usize,
    ref_mv_eighth_pel: Mv,
    tables: &MvCostTables,
    error_per_bit: i32,
    approx_inter_rate: bool,
) -> Option<(Mv, i32)> {
    if bucket.len() <= 1 {
        return None;
    }
    let block_origin = (x_pos, y_pos);
    let mut best: Option<(Mv, i32)> = None;
    let mut best_cost = i32::MAX;
    for entry in bucket {
        if entry.hash_value2 != hash_value2 {
            continue;
        }
        let dv = Mv {
            x: (8 * (entry.x - x_pos)) as i16,
            y: (8 * (entry.y - y_pos)) as i16,
        };
        if !is_dv_valid(dv, mi_row, mi_col, bw, bh, bw_mi, bh_mi, tile, sb_size_log2_mi, sb_size_px) {
            continue;
        }
        let hash_x = entry.x - x_pos;
        let hash_y = entry.y - y_pos;
        if !is_mv_in(mv_limits, hash_x, hash_y) {
            continue;
        }
        let cost = get_mvpred_var(
            pic, stride, block_origin, bw as usize, bh as usize, hash_x, hash_y, ref_mv_eighth_pel, tables,
            error_per_bit, approx_inter_rate, true,
        );
        if cost < best_cost {
            best_cost = cost;
            best = Some((dv, cost));
        }
    }
    best
}

// =============================================================================
// Top-level per-block orchestration: `intra_bc_search`
// (mode_decision.c:2976-3126).
// =============================================================================

/// C `IntrabcMotionDirection` (definitions.h:2182-2187).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrabcMotionDirection {
    Above,
    Left,
}

/// C's whole-frame initial `x->mv_limits` (mode_decision.c:3001-3005) --
/// the baseline every direction's `assert_release` subset check
/// (mode_decision.c:3050-3053) validates against. NOT used to narrow the
/// per-direction bound numerically (the C `switch(dir)` fully OVERWRITES
/// all four `MvLimits` fields with fresh values, it does not intersect
/// them with this whole-frame bound); kept here only because the
/// `assert_release` checks are a real (if soft/logging-only) C invariant.
/// `mi_width`/`mi_height` are MI-unit block dims (`mi_size_wide`/
/// `mi_size_high`), NOT the pixel dims [`direction_mv_limits`] takes.
pub fn frame_mv_limits(mi_row: i32, mi_col: i32, mi_width: i32, mi_height: i32, mi_rows: i32, mi_cols: i32) -> FullMvLimits {
    FullMvLimits {
        row_min: -(((mi_row + mi_height) * 4) + AOM_INTERP_EXTEND),
        col_min: -(((mi_col + mi_width) * 4) + AOM_INTERP_EXTEND),
        row_max: (mi_rows - mi_row) * 4 + AOM_INTERP_EXTEND,
        col_max: (mi_cols - mi_col) * 4 + AOM_INTERP_EXTEND,
    }
}

/// C's per-direction `switch (dir)` arms (mode_decision.c:3041-3062).
/// `bw`/`bh` are PIXEL block dims; `sb_mi_size`/`sb_size_log2_mi` are
/// `seq_header.sb_mi_size`/`seq_header.sb_size_log2` (16/4 for a 64px SB,
/// 32/5 for 128px, `Source/Lib/Globals/enc_handle.c:4100-4110`).
pub fn direction_mv_limits(
    dir: IntrabcMotionDirection,
    tile: TileMiBounds,
    mi_row: i32,
    mi_col: i32,
    bw: i32,
    bh: i32,
    sb_mi_size: i32,
    sb_size_log2_mi: u32,
) -> FullMvLimits {
    let sb_row = mi_row >> sb_size_log2_mi;
    let sb_col = mi_col >> sb_size_log2_mi;
    match dir {
        IntrabcMotionDirection::Above => FullMvLimits {
            col_min: (tile.mi_col_start - mi_col) * 4,
            col_max: (tile.mi_col_end - mi_col) * 4 - bw,
            row_min: (tile.mi_row_start - mi_row) * 4,
            row_max: (sb_row * sb_mi_size - mi_row) * 4 - bh,
        },
        IntrabcMotionDirection::Left => {
            let bottom_coded_mi_edge = ((sb_row + 1) * sb_mi_size).min(tile.mi_row_end);
            FullMvLimits {
                col_min: (tile.mi_col_start - mi_col) * 4,
                col_max: (sb_col * sb_mi_size - mi_col) * 4 - bw,
                row_min: (tile.mi_row_start - mi_row) * 4,
                row_max: (bottom_coded_mi_edge - mi_row) * 4 - bh,
            }
        }
    }
}

/// C `intra_bc_search` (mode_decision.c:2976-3126), minus the
/// `IntraBcContext`/`PictureControlSet` plumbing (lambda/qindex-derived
/// `sadperbit16`/`errorperbit`, the `enhanced_pic` buffer setup) -- those
/// are caller responsibilities, passed in already resolved. `pic` is the
/// picture's WHOLE luma plane (source pixels, used as BOTH "source" and
/// "reference" -- see §4's header doc; absolute-coordinate convention,
/// see §4's `window` note); `hash_buckets[0]`/`[1]` are the caller-fetched
/// hash buckets for the ABOVE/LEFT directions respectively (`None` when
/// hash search is unwired or the block is hash-ineligible; see §4b).
/// Returns up to 2 DV candidates (eighth-pel), matching C's `Mv
/// dv_cand[2]` + `num_dv_cand`.
///
/// `mi_size_wide_log2`/`mi_size_high_log2` for [`full_pixel_search`]'s
/// threshold scaling are derived from `bw`/`bh` directly
/// (`trailing_zeros() - 2`, exact for the power-of-two block dims AV1
/// always uses) rather than threading two more parameters.
#[allow(clippy::too_many_arguments)]
pub fn intra_bc_search(
    pic: &[u8],
    stride: usize,
    bw: i32,
    bh: i32,
    bw_mi: i32,
    bh_mi: i32,
    mi_row: i32,
    mi_col: i32,
    mi_rows: i32,
    mi_cols: i32,
    sb_mi_size: i32,
    sb_size_log2_mi: u32,
    sb_size_px: i32,
    tile: TileMiBounds,
    dv_ref: Mv,
    cfg: &SearchSiteConfig,
    ctrls: &IbcCtrls,
    sad_per_bit: i32,
    error_per_bit: i32,
    approx_inter_rate: bool,
    tables: &MvCostTables,
    hash_buckets: [Option<&[BlockHashEntry]>; 2],
    hash_value2: u32,
) -> Vec<Mv> {
    let whole_frame = frame_mv_limits(mi_row, mi_col, bw_mi, bh_mi, mi_rows, mi_cols);
    let directions: &[IntrabcMotionDirection] = if ctrls.search_dir != 0 {
        &[IntrabcMotionDirection::Above]
    } else {
        &[IntrabcMotionDirection::Above, IntrabcMotionDirection::Left]
    };

    let mi_size_wide_log2 = (bw as u32).trailing_zeros() - 2;
    let mi_size_high_log2 = (bh as u32).trailing_zeros() - 2;

    let mut dv_cand = Vec::with_capacity(2);
    let x_pos = mi_col * 4;
    let y_pos = mi_row * 4;
    let block_origin = (x_pos, y_pos);
    let hash_eligible = hash_search_eligible(bw, bh, ctrls.max_block_size_hash);

    for (dir_idx, &dir) in directions.iter().enumerate() {
        let mut mv_limits = direction_mv_limits(dir, tile, mi_row, mi_col, bw, bh, sb_mi_size, sb_size_log2_mi);
        // C `assert_release`: the direction bound must be a subset of the
        // whole-frame bound (soft/logging-only in C, a hard debug_assert
        // here).
        debug_assert!(mv_limits.col_min >= whole_frame.col_min);
        debug_assert!(mv_limits.col_max <= whole_frame.col_max);
        debug_assert!(mv_limits.row_min >= whole_frame.row_min);
        debug_assert!(mv_limits.row_max <= whole_frame.row_max);

        set_mv_search_range(&mut mv_limits, dv_ref);
        if mv_limits.col_max < mv_limits.col_min || mv_limits.row_max < mv_limits.row_min {
            continue;
        }

        let hash_result = if hash_eligible {
            hash_buckets[dir_idx].and_then(|bucket| {
                hash_search_best_in_bucket(
                    bucket,
                    hash_value2,
                    x_pos,
                    y_pos,
                    mi_row,
                    mi_col,
                    bw,
                    bh,
                    bw_mi,
                    bh_mi,
                    tile,
                    sb_size_log2_mi,
                    sb_size_px,
                    mv_limits,
                    pic,
                    stride,
                    dv_ref,
                    tables,
                    error_per_bit,
                    approx_inter_rate,
                )
            })
        } else {
            None
        };

        if let Some((dv, _cost)) = hash_result {
            dv_cand.push(dv);
        } else {
            let (bx, by, _cost) = full_pixel_search(
                pic,
                stride,
                block_origin,
                bw as usize,
                bh as usize,
                cfg,
                mv_limits,
                dv_ref,
                sad_per_bit,
                error_per_bit,
                ctrls,
                mi_size_wide_log2,
                mi_size_high_log2,
                tables,
                approx_inter_rate,
            );
            let dv = Mv { x: (bx * 8) as i16, y: (by * 8) as i16 };
            if !mv_check_bounds(mv_limits, dv)
                && is_dv_valid(dv, mi_row, mi_col, bw, bh, bw_mi, bh_mi, tile, sb_size_log2_mi, sb_size_px)
            {
                dv_cand.push(dv);
            }
        }
    }
    dv_cand
}

// =============================================================================
// §5. DV cost (RD-time) -- `svt_av1_mv_bit_cost` / `svt_av1_mv_bit_cost_
// light` (rd_cost.c:59-78), the rate term `svt_aom_intra_fast_cost`'s
// `use_intrabc` arm charges every IBC candidate. Reuses [`mv_table_cost`]
// (§4), same as the search-time [`mv_err_cost`] -- these two C functions
// share `mv_cost`/`svt_mv_cost`'s lookup, differing only in the final
// scale (a caller-supplied `weight` + 7-bit shift here, vs. `error_per_bit`
// + a 14-bit shift for the search's precise cost).
// =============================================================================

/// C `MV_COST_WEIGHT_SUB` (md_rate_estimation.h:24): the weight
/// `svt_aom_intra_fast_cost` passes to [`mv_bit_cost`] for the DV rate.
pub const MV_COST_WEIGHT_SUB: i32 = 120;

/// C `svt_av1_mv_bit_cost` (rd_cost.c:70-78, EXPORTED symbol) -- the
/// RD-time DV rate estimate. `mv`/`ref_mv` eighth-pel.
pub fn mv_bit_cost(mv: Mv, ref_mv: Mv, tables: &MvCostTables, weight: i32) -> i32 {
    let diff_x = i32::from(mv.x) - i32::from(ref_mv.x);
    let diff_y = i32::from(mv.y) - i32::from(ref_mv.y);
    round_power_of_two(mv_table_cost(diff_x, diff_y, tables) * weight, 7 /* RDDIV_BITS */)
}

/// C `svt_av1_mv_bit_cost_light` (rd_cost.c:59-65) -- textually identical
/// to [`mv_err_cost_light`] (same eighth-pel `1296 + 50*(|dx|+|dy|)`
/// formula; C duplicates the body under a second name for this call
/// site rather than sharing it).
#[inline]
pub fn mv_bit_cost_light(mv: Mv, ref_mv: Mv) -> i32 {
    mv_err_cost_light(mv, ref_mv)
}

// =============================================================================
// §6. Injection-gate helpers + the candidate shape --
// `generate_md_stage_0_cand`'s intrabc block (mode_decision.c:3587-3620)
// and `inject_intra_bc_candidates` (mode_decision.c:3127-3163).
// =============================================================================

/// C `svt_aom_allow_intrabc` (entropy_coding.c:4396-4398): the frame-level
/// gate consumed both by the writer ([`write_intrabc_info`]'s caller) and
/// (via `ctx->md_allow_intrabc = pcs->ppcs->frm_hdr.allow_intrabc`,
/// enc_mode_config.c:7888/8005/8121 -- a plain field copy, not a function,
/// so not translated as one here) MD's own candidate-injection gate.
#[inline]
pub fn allow_intrabc_frame(is_i_slice: bool, allow_screen_content_tools: bool, allow_intrabc: bool) -> bool {
    is_i_slice && allow_screen_content_tools && allow_intrabc
}

/// C's `eval_intrabc` local (mode_decision.c:3587, 3591-3594): when
/// `palette_hint` is set, the DV search is skipped unless the co-sited
/// palette search (task #71) injected at least one candidate.
/// `palette_ran` = whether `svt_av1_allow_palette` gated the palette
/// search ON for this block at all (`eval_intrabc` starts `true` and is
/// only narrowed inside the `if (svt_aom_allow_palette(...))` block, so a
/// block where palette never runs always evaluates IBC).
#[inline]
pub fn eval_intrabc_after_palette(palette_ran: bool, palette_candidates_injected: u32) -> bool {
    !palette_ran || palette_candidates_injected > 0
}

/// C's NSQ/B4 parent-gating predicate (mode_decision.c:3601-3614).
/// `is_part_n` = `ctx->shape == PART_N`; `sq_size` the block's square
/// size in pixels. For the `PART_N`/4x4 branch, `parent_n0 = (tested,
/// used_intrabc)` describes the parent 8x8 square's `PART_N` winner
/// (`pc_tree->parent->block_data[PART_N][0]`); for the NSQ branch,
/// `sibling_n0` describes THIS node's own untested-shape `PART_N`
/// candidate (`pc_tree->block_data[PART_N][0]`) tried earlier at the same
/// position. Both default to `(false, false)` when unavailable, matching
/// C's `tested_blk` gate (an untested slot never fires the veto).
pub fn parent_gate_allows_intrabc(
    is_part_n: bool,
    sq_size: i32,
    b4_parent_gating: bool,
    nsq_parent_gating: bool,
    parent_n0: (bool, bool),
    sibling_n0: (bool, bool),
) -> bool {
    if is_part_n {
        if b4_parent_gating && sq_size == 4 && parent_n0.0 && !parent_n0.1 {
            return false;
        }
    } else if nsq_parent_gating && sibling_n0.0 && !sibling_n0.1 {
        return false;
    }
    true
}

/// The full IBC injection gate (mode_decision.c:3587-3620): the
/// palette-hint coupling ANDed with the parent gating. Call AFTER
/// confirming `ctx->md_allow_intrabc` (== `frm_hdr.allow_intrabc`, a
/// plain field, see [`allow_intrabc_frame`]'s doc) and BEFORE running
/// [`intra_bc_search`].
pub fn do_intra_bc_gate(
    ctrls: &IbcCtrls,
    palette_ran: bool,
    palette_candidates_injected: u32,
    is_part_n: bool,
    sq_size: i32,
    parent_n0: (bool, bool),
    sibling_n0: (bool, bool),
) -> bool {
    if ctrls.palette_hint && !eval_intrabc_after_palette(palette_ran, palette_candidates_injected) {
        return false;
    }
    parent_gate_allows_intrabc(is_part_n, sq_size, ctrls.b4_parent_gating, ctrls.nsq_parent_gating, parent_n0, sibling_n0)
}

/// C `ModeDecisionCandidate` fields `inject_intra_bc_candidates`
/// (mode_decision.c:3127-3163) sets for one IBC candidate. Field values
/// mirror C exactly except `filter_intra_mode`, which this port
/// represents as a plain `u8` sentinel `5` (== C's `FILTER_INTRA_MODES`)
/// rather than `Option<FilterIntraMode>`, matching this crate's
/// established `LeafWinner`-style convention (`partition.rs`).
#[derive(Debug, Clone, Copy)]
pub struct IbcCandidate {
    pub mode: PredictionMode,
    pub uv_mode: UvPredictionMode,
    pub angle_delta_y: i8,
    pub angle_delta_uv: i8,
    pub cfl_alpha_signs: u8,
    pub cfl_alpha_idx: u8,
    pub tx_type_y: TxType,
    pub tx_type_uv: TxType,
    /// `FILTER_INTRA_MODES` sentinel (5) -- filter-intra not used.
    pub filter_intra_mode: u8,
    pub motion_mode: MotionMode,
    pub is_interintra_used: bool,
    pub skip_mode_allowed: bool,
    pub interp_filter: InterpFilter,
    /// `block_mi.mv[INTRA_FRAME]` -- the winning DV, eighth-pel.
    pub dv: Mv,
    /// `pred_mv[0]` -- `ctx->ref_mv_stack[INTRA_FRAME][0].this_mv`, the
    /// SAME `dv_ref` [`resolve_dv_ref`] produced (stamped onto the ref-mv
    /// stack at `mode_decision.c:3026` before the search runs).
    pub pred_dv: Mv,
    pub drl_index: u8,
}

/// Build one IBC candidate for a found DV (`inject_intra_bc_candidates`'s
/// per-`dv_cand[]` loop body, mode_decision.c:3140-3162). `palette_info =
/// NULL`/`ref_frame = {INTRA_FRAME, NONE_FRAME}` are not modeled as
/// fields here (they are constant/contextual, not per-candidate state a
/// caller varies) -- document them at the wiring site instead.
pub fn build_intra_bc_candidate(dv: Mv, pred_dv: Mv) -> IbcCandidate {
    IbcCandidate {
        mode: PredictionMode::DcPred,
        uv_mode: UvPredictionMode::UvDcPred,
        angle_delta_y: 0,
        angle_delta_uv: 0,
        cfl_alpha_signs: 0,
        cfl_alpha_idx: 0,
        tx_type_y: TxType::DctDct,
        tx_type_uv: TxType::DctDct,
        filter_intra_mode: 5,
        motion_mode: MotionMode::SimpleTranslation,
        is_interintra_used: false,
        skip_mode_allowed: false,
        interp_filter: InterpFilter::Bilinear,
        dv,
        pred_dv,
        drl_index: 0,
    }
}

// =============================================================================
// §7. Writer helpers + CDF consts -- `write_intrabc_info`
// (entropy_coding.c:4405-4416) + `svt_av1_encode_dv` (entropy_coding.c:
// 4381-4396, already a thin wrapper over `svtav1_entropy::mv_coding::
// encode_mv_diff` -- see below) + `default_intrabc_cdf` (cabac_context_
// model.c:610-612).
//
// `svtav1_entropy::mv_coding` already carries a verified [`NmvContext`]
// default (`NmvContext::default()`, `tests/c_parity_mv.rs`), and C's own
// `ndvc` (the DV-specific entropy context) is seeded from the EXACT SAME
// table (`fc->ndvc = default_nmv_context;`, cabac_context_model.c:795 --
// identical to `fc->nmvc`'s seed one line above) -- so no separate DV
// NmvContext transcription is needed; `NmvContext::default()` IS the
// correct `ndvc` default too. Only `intrabc_cdf` (a plain binary flag,
// not carried by `mv_coding`) needs transcribing here.
//
// PORT-NOTE(unverified): at wiring time, move [`INTRABC_DEFAULT_CDF`]
// into `svtav1-entropy/src/default_cdfs.rs` (ideally via that file's
// `gen_default_cdfs` generator, matching every other default-CDF table's
// provenance) and add an `intrabc_cdf: [AomCdfProb; 3]` + `ndvc:
// NmvContext` field pair to `FrameContext` (`context.rs`) alongside the
// other per-frame CDF state, rather than leaving them here.
// =============================================================================

/// C `default_intrabc_cdf` (cabac_context_model.c:610-612): `AOM_CDF2(30531)`.
pub const INTRABC_DEFAULT_CDF: [u16; 3] = [svtav1_entropy::cdf::aom_icdf(30531), 0, 0];

/// C `write_intrabc_info` (entropy_coding.c:4405-4416) + `svt_av1_encode_
/// dv` (entropy_coding.c:4381-4396, inlined here as a call to
/// [`encode_mv_diff`] with `MvSubpelPrecision::None` -- a DV never carries
/// fractional-pel bits, spec 5.11.35). `dv_ref` is `blk_ptr->predmv[0]`
/// (== [`IbcCandidate::pred_dv`]); `dv` is `mbmi->block_mi.mv[INTRA_FRAME]`
/// (== [`IbcCandidate::dv`]).
pub fn write_intrabc_info(w: &mut AomWriter, intrabc_cdf: &mut [u16], ndvc: &mut NmvContext, use_intrabc: bool, dv: Mv, dv_ref: Mv) {
    w.write_symbol(usize::from(use_intrabc), intrabc_cdf, 2);
    if use_intrabc {
        let diff_row = i32::from(dv.y) - i32::from(dv_ref.y);
        let diff_col = i32::from(dv.x) - i32::from(dv_ref.x);
        encode_mv_diff(w, ndvc, diff_row, diff_col, MvSubpelPrecision::None);
    }
}

// =============================================================================
// RD integration -- DOCUMENTED, NOT WIRED (task scope: cite the call
// chain, do not transcribe generic RD/prediction machinery this vertical
// doesn't own).
//
// 1. FAST COST (`svt_aom_intra_fast_cost`, rd_cost.c:522-543): when
//    `svt_aom_allow_intrabc(frm_hdr, slice_type) && cand->use_intrabc`,
//    the ENTIRE normal intra fast-cost body (luma/chroma mode bits,
//    angle-delta, palette, filter-intra) is bypassed. Instead:
//      rate = mv_bit_cost(dv, pred_dv, dv_cost_tables, MV_COST_WEIGHT_SUB)
//           + intrabc_fac_bits[1]     // md_rate_estimation.c:254,
//                                     // svt_aom_get_syntax_rate_from_cdf(
//                                     //   intrabc_fac_bits, fc->intrabc_cdf)
//                                     // -- this port's INTRABC_DEFAULT_CDF
//                                     //   run through crate::quant::
//                                     //   syntax_rate_from_cdf gives the
//                                     //   same [i32; 2] shape.
//      cand_bf->fast_luma_rate = rate; cand_bf->fast_chroma_rate = 0;
//      return RDCOST(lambda, rate, luma_distortion);
//    (`RDCOST(rm, r, d) = ROUND_POWER_OF_TWO(r*rm, AV1_PROB_COST_SHIFT) +
//    (d << RDDIV_BITS)`, rd_cost.h:36-38 -- this port's [`mv_bit_cost`] +
//    [`intrabc_fac_bits`]-shaped lookup supply the two `rate` terms; the
//    RDCOST composition itself is generic RD-cost machinery this vertical
//    does not own, so it is not re-transcribed here.) `dv_cost_tables`
//    here is `ctx->md_rate_est_ctx->dv_cost`/`dv_joint_cost`, i.e.
//    [`build_nmv_cost_table`] run over the frame's live `ndvc` (NOT the
//    default) with `MvSubpelPrecision::None` -- see §4's [`build_nmv_cost_
//    table`] doc. When `!cand->use_intrabc` but `allow_intrabc_frame` is
//    still true, the ordinary intra path ADDITIONALLY charges `luma_rate
//    += intrabc_fac_bits[0]` (rd_cost.c:630-631) -- i.e. EVERY intra
//    candidate on an IBC-enabled frame pays the `intrabc_cdf` flag's cost
//    one way or the other, never for free.
// 2. FULL COST (rd_cost.c:1417ish): reuses the fast-luma-rate computed
//    above plus the real coefficient rate -- no palette-style
//    re-derivation at MDS3.
// 3. PREDICTION (compensation): IBC candidates route through the INTER
//    prediction dispatch (`product_prediction_fun_table[is_inter_mode(
//    mode) || use_intrabc]`, product_coding_loop.c:1270/6862 and 10+
//    other `is_inter =(...||use_intrabc)` sites) with `ref_frame[0] ==
//    INTRA_FRAME` selecting the CURRENT picture's own reconstructed
//    buffer as the motion-compensation reference (spec 7.11.3: IBC is
//    "prediction from the current frame"). The actual compensation
//    (`svt_inter_predictor`, inter_prediction.c:1386-1440) special-cases
//    `is_intrabc`: given [`is_dv_valid`] already guarantees a DV has zero
//    sub-pel bits (see §2), the `is_intrabc && subpel_x==0 && subpel_y==0`
//    condition always holds, so compensation ALWAYS falls through to the
//    `svt_aom_convolve[0][0][is_compound]` dispatch slot -- the
//    zero-tap "no filtering" convolve variant, i.e. a plain RECON-DOMAIN
//    block copy. This is the ONE place in the whole vertical that MUST
//    read decoder-visible reconstructed samples rather than source pixels
//    -- contrast with §4's search, which deliberately reads SOURCE pixels
//    for both "self" and "candidate" as a speed heuristic (never
//    reconstructed pixels), matching C's own `x->plane[0].src =
//    x->xdplane[0].pre[0] = pcs->ppcs->enhanced_pic` wiring exactly.
// 4. TRANSFORM/QUANTIZATION: ordinary. Grepping every tx/quant C file
//    (`transforms.c`, `av1_quantize.c`/`inv_transforms.c`,
//    `encodetxb`-equivalents) for `use_intrabc` finds exactly ONE hit,
//    `full_loop.c:2219`'s `is_inter = (is_inter_mode(mode) ||
//    use_intrabc)` -- feeding the SAME inter-vs-intra tx-set-size
//    classification every inter block gets, not an IBC-specific branch.
//    Whether MDS3 therefore re-searches tx type against the (larger)
//    inter tx set for an IBC block instead of keeping the injection-time
//    fixed `DCT_DCT` (see [`build_intra_bc_candidate`]) is a further
//    wiring-time question this citation does not resolve.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn tile_full_frame(mi_cols: i32, mi_rows: i32) -> TileMiBounds {
        TileMiBounds { mi_col_start: 0, mi_col_end: mi_cols, mi_row_start: 0, mi_row_end: mi_rows }
    }

    #[test]
    fn ibc_ctrls_level_table_shape() {
        assert!(!IbcCtrls::for_level(0).enabled);
        let l3 = IbcCtrls::for_level(3);
        assert!(l3.enabled && l3.palette_hint && l3.nsq_parent_gating && l3.mesh_qp_scaling);
        assert_eq!(l3.max_block_size_hash, 64);
        let l7 = IbcCtrls::for_level(7);
        assert_eq!(l7.max_block_size_hash, 8);
        assert_eq!(l7.search_dir, 1);
        assert_eq!(l7.exhaustive_mesh_thresh, u64::MAX);
        assert!(!l7.mesh_qp_scaling); // unassigned at this level, see for_level's doc
    }

    #[test]
    fn allintra_intrabc_level_table() {
        assert_eq!(allintra_intrabc_level(0, true, true), 3);
        assert_eq!(allintra_intrabc_level(4, true, true), 7);
        assert_eq!(allintra_intrabc_level(5, true, true), 0);
        assert_eq!(allintra_intrabc_level(0, false, true), 0);
        assert_eq!(allintra_intrabc_level(0, true, false), 0);
    }

    #[test]
    fn is_chroma_reference_matches_c_parity_rule() {
        // Even-dimensioned luma (bw_mi=bh_mi=2, an 8x8 block): the
        // `!(bh&1)` / `!(bw&1)` disjuncts are unconditionally true, so
        // EVERY mi position is a chroma reference -- mi parity never
        // matters once a block is >=8x8 in both dims under 4:2:0.
        assert!(is_chroma_reference(0, 0, 2, 2, 1, 1));
        assert!(is_chroma_reference(1, 1, 2, 2, 1, 1));
        assert!(is_chroma_reference(0, 1, 2, 2, 1, 1));

        // Odd bw_mi (BLOCK_4X8, bw_mi=1, bh_mi=2): the column clause now
        // genuinely depends on mi_col parity (the row clause stays always
        // true since bh_mi=2 is even) -- only odd mi_col is a reference,
        // matching AV1's "last sub-8-wide column carries chroma" rule.
        assert!(!is_chroma_reference(0, 0, 1, 2, 1, 1));
        assert!(is_chroma_reference(0, 1, 1, 2, 1, 1));

        // Both dims odd (BLOCK_4X4): a reference only when BOTH mi_row and
        // mi_col are odd.
        assert!(!is_chroma_reference(0, 0, 1, 1, 1, 1));
        assert!(!is_chroma_reference(1, 0, 1, 1, 1, 1));
        assert!(!is_chroma_reference(0, 1, 1, 1, 1, 1));
        assert!(is_chroma_reference(1, 1, 1, 1, 1, 1));
    }

    #[test]
    fn find_ref_dv_two_branches() {
        let tile = tile_full_frame(32, 32);
        // Room above (mi_row - mib_size >= tile_row_start): vertical-only.
        let dv_below = find_ref_dv(tile, 16, 20);
        assert_eq!(dv_below, Mv { x: 0, y: -16 * 4 * 8 });
        // No room above: horizontal-only, delayed by INTRABC_DELAY_PIXELS.
        let dv_top = find_ref_dv(tile, 16, 0);
        assert_eq!(dv_top, Mv { x: (-16 * 4 - INTRABC_DELAY_PIXELS) * 8, y: 0 });
    }

    #[test]
    fn resolve_dv_ref_coerces_invalid_and_falls_back() {
        let tile = tile_full_frame(32, 32);
        // Both invalid -> falls all the way to find_ref_dv.
        let resolved = resolve_dv_ref(Mv::INVALID, Mv::INVALID, tile, 16, 20);
        assert_eq!(resolved, find_ref_dv(tile, 16, 20));
        // nearestmv nonzero -> used directly.
        let nz = Mv { x: 40, y: -8 };
        assert_eq!(resolve_dv_ref(nz, Mv::ZERO, tile, 16, 20), nz);
        // nearestmv zero, nearmv nonzero -> nearmv used.
        assert_eq!(resolve_dv_ref(Mv::ZERO, nz, tile, 16, 20), nz);
    }

    #[test]
    fn is_dv_valid_rejects_self_reference() {
        // DV = (0,0): "copy from here" is never legal -- the "already
        // coded" wavefront constraint always rejects it (src_sb64 ==
        // active_sb64, never >= active_sb64 - INTRABC_DELAY_SB64 apart).
        let tile = tile_full_frame(64, 64);
        assert!(!is_dv_valid(Mv::ZERO, 20, 20, 8, 8, 2, 2, tile, 4, 64));
    }

    #[test]
    fn is_dv_valid_rejects_out_of_tile() {
        let tile = tile_full_frame(64, 64);
        // DV pointing above the tile's top edge.
        let dv = Mv { x: 0, y: -1000 * 8 };
        assert!(!is_dv_valid(dv, 20, 20, 8, 8, 2, 2, tile, 4, 64));
    }

    #[test]
    fn is_dv_valid_rejects_subpel() {
        let tile = tile_full_frame(64, 64);
        let dv = Mv { x: 5, y: 0 }; // not a multiple of 8
        assert!(!is_dv_valid(dv, 20, 20, 8, 8, 2, 2, tile, 4, 64));
    }

    #[test]
    fn mv_joint_index_table() {
        assert_eq!(mv_joint_index(0, 0), 0);
        assert_eq!(mv_joint_index(5, 0), 1);
        assert_eq!(mv_joint_index(0, 5), 2);
        assert_eq!(mv_joint_index(5, 5), 3);
    }

    #[test]
    fn round_power_of_two_matches_c_macro() {
        assert_eq!(round_power_of_two(10, 2), 3); // (10+2)>>2 = 3
        assert_eq!(round_power_of_two(0, 4), 0);
        assert_eq!(round_power_of_two_64(1000, 9), (1000 + 256) / 512);
    }

    #[test]
    fn divide_and_round_half_up() {
        assert_eq!(divide_and_round(10, 4), 3); // (10+2)/4
        assert_eq!(divide_and_round(0, 5), 0);
    }

    #[test]
    fn qp_scaling_disabled_is_identity() {
        assert_eq!(qp_based_th_scaling_factors(false, 40), (1, 1));
        let mut ctrls = IbcCtrls::for_level(3);
        let before = ctrls.mesh_patterns;
        scale_mesh_patterns_by_qp(&mut ctrls, false, 40);
        assert_eq!(ctrls.mesh_patterns, before);
    }

    #[test]
    fn qp_scaling_low_qp_uses_linear_branch() {
        // qp < 46: q_weight = max(10, qp), denom = 63 (MAX_QP_VALUE).
        assert_eq!(qp_based_th_scaling_factors(true, 20), (20, 63));
        assert_eq!(qp_based_th_scaling_factors(true, 5), (10, 63));
    }

    #[test]
    fn hash_search_eligible_gate() {
        assert!(hash_search_eligible(8, 8, 64));
        assert!(!hash_search_eligible(8, 16, 64)); // not square
        assert!(!hash_search_eligible(16, 16, 8)); // too big for level 5-7
        assert!(hash_search_eligible(8, 8, 8));
    }

    #[test]
    fn eval_intrabc_after_palette_gate() {
        assert!(eval_intrabc_after_palette(false, 0)); // palette never ran
        assert!(!eval_intrabc_after_palette(true, 0)); // ran, produced nothing
        assert!(eval_intrabc_after_palette(true, 3));
    }

    #[test]
    fn parent_gate_b4_and_nsq() {
        // PART_N, sq_size=4, gating on, parent tested but didn't use IBC.
        assert!(!parent_gate_allows_intrabc(true, 4, true, false, (true, false), (false, false)));
        // Same but parent DID use IBC -> allowed.
        assert!(parent_gate_allows_intrabc(true, 4, true, false, (true, true), (false, false)));
        // sq_size != 4 -> b4 gate never fires regardless of parent state.
        assert!(parent_gate_allows_intrabc(true, 8, true, false, (true, false), (false, false)));
        // NSQ branch mirrors the same shape via sibling_n0.
        assert!(!parent_gate_allows_intrabc(false, 8, false, true, (false, false), (true, false)));
    }

    #[test]
    fn allow_intrabc_frame_requires_all_three() {
        assert!(allow_intrabc_frame(true, true, true));
        assert!(!allow_intrabc_frame(false, true, true));
        assert!(!allow_intrabc_frame(true, false, true));
        assert!(!allow_intrabc_frame(true, true, false));
    }

    #[test]
    fn intrabc_default_cdf_value() {
        // AOM_CDF2(30531) -> icdf = 32768 - 30531 = 2237.
        assert_eq!(INTRABC_DEFAULT_CDF, [2237, 0, 0]);
    }

    #[test]
    fn mv_cost_table_zero_is_zero() {
        let ctx = NmvContext::default();
        let tables = build_nmv_cost_table(&ctx, MvSubpelPrecision::None);
        assert_eq!(tables.comp_cost[0].cost(0), 0);
        assert_eq!(tables.comp_cost[1].cost(0), 0);
        // Cost table lookup clamps symmetrically past MV_MAX (see
        // MvComponentCost's doc PORT-NOTE) rather than panicking.
        assert_eq!(tables.comp_cost[0].cost(MV_MAX + 10), tables.comp_cost[0].cost(MV_MAX));
        assert_eq!(tables.comp_cost[0].cost(-MV_MAX - 10), tables.comp_cost[0].cost(-MV_MAX));
    }

    #[test]
    fn init_search_sites_shape() {
        let cfg = init_search_sites(256);
        // 1 origin + MAX_MVSEARCH_STEPS(11) levels * 8 sites = 89.
        assert_eq!(cfg.sites.len(), 89);
        assert_eq!(cfg.searches_per_step, 8);
        assert_eq!(cfg.sites[0].mv_x, 0);
        assert_eq!(cfg.sites[0].mv_y, 0);
        // First real step is +/- MAX_FIRST_STEP.
        assert_eq!(cfg.sites[1].mv_y, -MAX_FIRST_STEP);
    }

    #[test]
    fn set_mv_search_range_narrows_only() {
        let mut limits = FullMvLimits { col_min: -1000, col_max: 1000, row_min: -1000, row_max: 1000 };
        let mv = Mv { x: 0, y: 0 };
        set_mv_search_range(&mut limits, mv);
        assert_eq!(limits.col_min, -MAX_FULL_PEL_VAL);
        assert_eq!(limits.col_max, MAX_FULL_PEL_VAL);
        // Narrower input bound stays narrower (intersection, not overwrite).
        let mut tight = FullMvLimits { col_min: -5, col_max: 5, row_min: -5, row_max: 5 };
        set_mv_search_range(&mut tight, mv);
        assert_eq!(tight.col_min, -5);
        assert_eq!(tight.col_max, 5);
    }
}

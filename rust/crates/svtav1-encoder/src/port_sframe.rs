//! S-frames (AV1 `S_FRAME` / switch frames) — a port of the ten functions in
//! `Codec/pd_process.c` that decide where one goes, what quantizer it uses,
//! how it rewrites the reference structure, and how it prunes what later
//! frames may predict from.
//!
//! An S-frame is an inter frame a decoder can *tune in on*: it refreshes all
//! eight DPB slots and codes `error_resilient_mode`, so a receiver joining
//! mid-stream can start decoding there without a key frame's bitrate. That
//! makes it the adaptive-bitrate switch point in a live ladder.
//!
//! | Rust | C (`Codec/pd_process.c`) |
//! |---|---|
//! | [`dist_to_s`] | `get_dist_to_s` (1494) — static |
//! | [`sframe_qp`] | `get_sframe_qp` (1509) — static |
//! | [`sframe_qp_offset`] | `get_sframe_qp_offset` (1525) — static |
//! | [`setup_sframe_qp`] | `setup_sframe_qp` (1541) — static |
//! | [`position_offset`] | `sframe_position_offset` (1563) — static |
//! | [`set_sframe_type`] | `set_sframe_type` (1571) — static |
//! | [`decide_sframe_mg`] | `decide_sframe_mg` (1689) — static |
//! | [`set_sframe_rps`] | `set_sframe_rps` (1726) — static |
//! | [`prune_sframe_refs`] | `prune_sframe_refs` (1003) — static |
//! | [`update_sframe_ref_order_hint`] | `update_sframe_ref_order_hint` (4521) — static |
//!
//! # Reachability, measured
//!
//! Every entry point is gated on the application asking for S-frames:
//! `set_sframe_type` runs only when `enc_ctx->sf_cfg.sframe_dist > 0 ||
//! static_config.sframe_posi.sframe_posis` (`pd_process.c:2272`),
//! `decide_sframe_mg` only under `IS_SFRAME_FLEXIBLE_INSERT` (`:2264`),
//! `set_sframe_rps` only when the frame type is already `S_FRAME` (`:3487`),
//! and `prune_sframe_refs` only when `ctx->sframe_poc > 0 && mfmv_enabled`
//! (`:1004`). With no S-frame configured every one of them is a no-op, which
//! is why leaving them out was byte-inert — and why they must be here before
//! the encoder can offer the feature at all
//! (`docs/WORKING-ON-THIS.md` §7: dead-looking C stays translated).
//!
//! # The `-1` sentinel is part of the contract
//!
//! `get_dist_to_s` returns `-1` for "no S-frame at or after this picture" and
//! writes `-1` into `dist_to_next_s` for the same reason, and its callers then
//! do signed arithmetic against those values (`dist_to_s > 0 && dist_to_s <
//! next_mg_size`, and an unconditional `dist_to_s = dist_to_next_s`
//! reassignment). Wrapping it in `Option` would move the sentinel out of the
//! arithmetic and back again at every use, so it is kept as [`NO_SFRAME`] and
//! named.
//!
//! # Evidence
//!
//! Tier 4 — every one of these is `static` with no exported symbol
//! (`nm -g Bin/Release/libSvtAv1Enc.a` has no entry for any of them).
//! `tests/port_sframe_traced.rs` carries the derivations.

use crate::port_picstruct::{
    Av1RpsNode, BWDREF_FRAME, EncCtxPicParams, PicDecisionCtx, PicParams, PredStructure, REF_FRAMES,
};

/// C's "no S-frame at or after this picture" sentinel, returned by
/// [`dist_to_s`] and compared arithmetically by its callers.
pub const NO_SFRAME: i32 = -1;

/// C `EbSFrameMode` (`API/EbSvtAv1Enc.h:163-172`). There is no zero variant;
/// C spells "off" as `sframe_dist == 0` with no position list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SFrameMode {
    /// C `SFRAME_STRICT_BASE = 1` — only a base-layer inter frame becomes one.
    StrictBase = 1,
    /// C `SFRAME_NEAREST_BASE = 2` — the next base-layer inter frame does.
    NearestBase = 2,
    /// C `SFRAME_FLEXIBLE_BASE = 3` — reshape the mini-GOP so the wanted
    /// picture becomes a base-layer frame, then make that one an S-frame.
    FlexibleBase = 3,
    /// C `SFRAME_DEC_POSI_BASE = 4` — as `FlexibleBase`, but positions are in
    /// DECODE order, so the frame before the insert point is promoted and the
    /// NEXT base-layer frame becomes the S-frame.
    DecPosiBase = 4,
}

impl SFrameMode {
    /// C `IS_SFRAME_FLEXIBLE_INSERT` — the two modes that reshape mini-GOPs.
    #[must_use]
    pub fn is_flexible_insert(self) -> bool {
        matches!(self, Self::FlexibleBase | Self::DecPosiBase)
    }
}

/// C `SvtAv1SFramePositions` (`API/EbSvtAv1Enc.h:201-207`) — the
/// application's explicit S-frame schedule.
///
/// C carries `sframe_num` alongside three raw pointers; the slices carry their
/// own lengths here, and `positions.len()` is `sframe_num`. The `qps` and
/// `qp_offsets` arrays are indexed by the SAME index as `positions`, so C
/// reads out of bounds if the application supplies a shorter one; the lookups
/// below use `get` and fall back to C's not-found value instead. That differs
/// from C only where C's behaviour is undefined.
#[derive(Debug, Clone, Copy, Default)]
pub struct SFramePositions<'a> {
    /// C `sframe_posis` — the picture numbers, ascending.
    pub positions: Option<&'a [u64]>,
    /// C `sframe_qps` — a per-position absolute QP, or 0 for "unset".
    pub qps: Option<&'a [u8]>,
    /// C `sframe_qp_offsets` — a per-position QP delta, or 0 for "unset".
    pub qp_offsets: Option<&'a [i8]>,
}

/// The S-frame configuration, gathered from the two places C keeps it:
/// `enc_ctx->sf_cfg` and `scs->static_config`.
#[derive(Debug, Clone, Copy)]
pub struct SFrameConfig<'a> {
    /// C `enc_ctx->sf_cfg.sframe_mode`.
    pub mode: SFrameMode,
    /// C `enc_ctx->sf_cfg.sframe_dist` — 0 means "only the explicit list".
    pub dist: i32,
    /// C `scs->static_config.sframe_qp` — 0 means "use the list".
    pub qp: u8,
    /// C `scs->static_config.sframe_qp_offset` — 0 means "use the list".
    pub qp_offset: i8,
    /// C `scs->static_config.sframe_posi`.
    pub positions: SFramePositions<'a>,
    /// C `scs->static_config.hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `scs->static_config.pred_structure`.
    pub pred_structure: PredStructure,
    /// C `scs->static_config.intra_period_length`.
    pub intra_period_length: i32,
    /// C `scs->static_config.min_qp_allowed`.
    pub min_qp_allowed: u8,
    /// C `scs->static_config.max_qp_allowed`.
    pub max_qp_allowed: u8,
    /// C `scs->mfmv_enabled` — gates [`prune_sframe_refs`].
    pub mfmv_enabled: bool,
    /// C `scs->seq_header.order_hint_info.order_hint_bits`.
    pub order_hint_bits: u32,
}

/// C `get_dist_to_s` (`pd_process.c:1494`) — static.
///
/// Distance from `picture_num` to the first scheduled S-frame at or after it,
/// and — only when `picture_num` IS a scheduled position — the distance from
/// there to the one after. Both are [`NO_SFRAME`] when there is none.
///
/// The asymmetry is C's and is load-bearing: `dist_to_next_s` stays
/// [`NO_SFRAME`] for a picture that merely precedes a scheduled S-frame, so
/// `set_sframe_type`'s `dist_to_s = dist_to_next_s` handoff only fires on the
/// S-frame itself.
#[must_use]
pub fn dist_to_s(posi: &SFramePositions<'_>, picture_num: u64) -> (i32, i32) {
    let mut to_next = NO_SFRAME;
    let Some(positions) = posi.positions else {
        return (NO_SFRAME, to_next);
    };
    for (i, &p) in positions.iter().enumerate() {
        if p >= picture_num {
            if p == picture_num {
                to_next = match positions.get(i + 1) {
                    Some(&next) => i32::try_from(next - picture_num).unwrap_or(i32::MAX),
                    None => NO_SFRAME,
                };
            }
            return (i32::try_from(p - picture_num).unwrap_or(i32::MAX), to_next);
        }
    }
    (NO_SFRAME, to_next) // all s-frame spots are expired
}

/// C `get_sframe_qp` (`pd_process.c:1509`) — static.
///
/// With no position list C returns `qps[0]` unconditionally, which is how a
/// single `--sframe-qp` applies to every S-frame; with one, only an exact
/// position match returns a value.
#[must_use]
pub fn sframe_qp(posi: &SFramePositions<'_>, picture_num: u64) -> u8 {
    let Some(qps) = posi.qps else { return 0 };
    let Some(positions) = posi.positions else {
        return qps.first().copied().unwrap_or(0);
    };
    positions
        .iter()
        .position(|&p| p == picture_num)
        .and_then(|i| qps.get(i).copied())
        .unwrap_or(0)
}

/// C `get_sframe_qp_offset` (`pd_process.c:1525`) — static. Same shape as
/// [`sframe_qp`], for the signed per-position delta.
#[must_use]
pub fn sframe_qp_offset(posi: &SFramePositions<'_>, picture_num: u64) -> i8 {
    let Some(offsets) = posi.qp_offsets else {
        return 0;
    };
    let Some(positions) = posi.positions else {
        return offsets.first().copied().unwrap_or(0);
    };
    positions
        .iter()
        .position(|&p| p == picture_num)
        .and_then(|i| offsets.get(i).copied())
        .unwrap_or(0)
}

/// C `setup_sframe_qp` (`pd_process.c:1541`) — static.
///
/// Two traps transcribed rather than tidied:
///
/// * the picture number the schedule is looked up by is the DECODE order under
///   [`SFrameMode::DecPosiBase`] and the display order otherwise;
/// * C clips through `int8_t`, so a configured QP above 127 wraps negative
///   before the clip. `CLIP3(min, max, (int8_t)sframe_qp)` on a `sframe_qp` of
///   200 clips `-56` up to `min_qp_allowed`, not down to `max_qp_allowed`.
///   Reproduced with an explicit `as i8`.
pub fn setup_sframe_qp(pic: &mut PicParams, cfg: &SFrameConfig<'_>) {
    let pic_num = if cfg.mode == SFrameMode::DecPosiBase {
        pic.decode_order
    } else {
        pic.picture_number
    };
    let qp = if cfg.qp > 0 {
        cfg.qp
    } else {
        sframe_qp(&cfg.positions, pic_num)
    };
    if qp > 0 {
        let clipped = (qp as i8).clamp(cfg.min_qp_allowed as i8, cfg.max_qp_allowed as i8);
        pic.picture_qp = clipped as u8;
        pic.qp_on_the_fly = true;
    }
    let offset = if cfg.qp_offset != 0 {
        cfg.qp_offset
    } else {
        sframe_qp_offset(&cfg.positions, pic_num)
    };
    if offset != 0 {
        pic.sframe_qp_offset = offset;
    }
}

/// C `sframe_position_offset` (`pd_process.c:1563`) — static.
///
/// 1 only for decode-order positions in random access, where the decode order
/// differs from the display order; low delay needs no adjustment because the
/// two coincide.
#[must_use]
pub fn position_offset(cfg: &SFrameConfig<'_>) -> i32 {
    i32::from(
        cfg.mode == SFrameMode::DecPosiBase && cfg.pred_structure == PredStructure::RandomAccess,
    )
}

/// Lower `ctx.sframe_hier_lvls` to the deepest level whose mini-GOP fits
/// inside `dist` pictures — C's identical three-line loop, which appears at
/// `pd_process.c:1632`, `:1660` and `:1712`.
///
/// C asserts the result stays in `0..=hierarchical_levels`; it does by
/// construction, because the loop only ever lowers and starts from that value.
fn downgrade_hier_lvls(ctx: &mut PicDecisionCtx, dist: i32) {
    for lvl in 0..ctx.sframe_hier_lvls {
        if dist < (1 << (lvl + 1)) {
            ctx.sframe_hier_lvls = lvl;
            break;
        }
    }
}

/// C `set_sframe_type` (`pd_process.c:1571`) — static.
///
/// Decides whether THIS inter frame becomes an S-frame, and — in the two
/// flexible modes — shrinks the NEXT mini-GOP so the scheduled S-frame lands
/// on a base-layer picture.
///
/// Called only for non-I slices, and only when
/// `sframe_dist > 0 || sframe_posi.sframe_posis` (`pd_process.c:2272`).
pub fn set_sframe_type(pic: &mut PicParams, cfg: &SFrameConfig<'_>, ctx: &mut PicDecisionCtx) {
    let is_arf = pic.temporal_layer_index == 0;
    let frames_since_key = pic.picture_number - ctx.key_poc;

    match cfg.mode {
        SFrameMode::StrictBase => {
            if is_arf && cfg.dist > 0 && frames_since_key.is_multiple_of(cfg.dist as u64) {
                pic.is_switch_frame = true;
            }
        }
        SFrameMode::NearestBase => {
            if cfg.pred_structure == PredStructure::RandomAccess {
                // Pictures reach picture decision in DECODE order, so when the
                // scheduled position falls anywhere inside this mini-GOP, this
                // base-layer frame is the next S-frame.
                if is_arf
                    && cfg.dist > 0
                    && frames_since_key % (cfg.dist as u64) < u64::from(ctx.mg_size)
                {
                    pic.is_switch_frame = true;
                }
            } else {
                if cfg.dist > 0 && frames_since_key.is_multiple_of(cfg.dist as u64) {
                    ctx.sframe_due = true;
                }
                if ctx.sframe_due && is_arf {
                    pic.is_switch_frame = true;
                    ctx.sframe_due = false;
                }
            }
        }
        SFrameMode::FlexibleBase | SFrameMode::DecPosiBase => {
            if is_arf {
                set_sframe_type_flexible(pic, cfg, ctx, frames_since_key);
                ctx.sframe_last_arf = frames_since_key;
            }
        }
    }

    if pic.is_switch_frame {
        setup_sframe_qp(pic, cfg);
    }
    pic.sframe_ref_pruned = false;
}

/// The `SFRAME_FLEXIBLE_BASE` / `SFRAME_DEC_POSI_BASE` arm of
/// [`set_sframe_type`] (`pd_process.c:1600-1670`), split out because it is the
/// only arm with real control flow.
fn set_sframe_type_flexible(
    pic: &mut PicParams,
    cfg: &SFrameConfig<'_>,
    ctx: &mut PicDecisionCtx,
    frames_since_key: u64,
) {
    let sframe_offset = position_offset(cfg);

    // A decode-order insert defers by one base-layer frame, decided last time.
    if ctx.next_arf_is_s {
        pic.is_switch_frame = true;
        ctx.next_arf_is_s = false;
    }

    let mut next_mg_size = 1i32 << ctx.sframe_hier_lvls;

    if cfg.positions.positions.is_some() {
        // With an explicit schedule the encoder looks at the distance to the
        // next TWO S-frames: the first decides this picture, the second sizes
        // the mini-GOP that follows it.
        let lookup = pic.picture_number.wrapping_add(sframe_offset as u64);
        let (mut d, d_next) = dist_to_s(&cfg.positions, lookup);
        if d == 0 {
            if sframe_offset != 0 {
                ctx.next_arf_is_s = true;
            } else {
                pic.is_switch_frame = true;
            }
            // A fresh S-frame restarts the pyramid at full depth.
            ctx.sframe_hier_lvls = i32::from(cfg.hierarchical_levels);
            next_mg_size = 1i32 << ctx.sframe_hier_lvls;
            d = d_next;
        }
        if d > 0 && d < next_mg_size {
            downgrade_hier_lvls(ctx, d);
        }
    } else {
        if cfg.dist > 0
            && frames_since_key
                .wrapping_add(sframe_offset as u64)
                .is_multiple_of(cfg.dist as u64)
        {
            if sframe_offset != 0 {
                ctx.next_arf_is_s = true;
            } else {
                pic.is_switch_frame = true;
            }
            ctx.sframe_hier_lvls = i32::from(cfg.hierarchical_levels);
            next_mg_size = 1i32 << ctx.sframe_hier_lvls;
        }
        // Only shrink the next mini-GOP if it will not run past the next key
        // frame — a key frame restarts the structure anyway.
        let before_next_key = cfg.mode != SFrameMode::DecPosiBase
            || cfg.intra_period_length <= 0
            || frames_since_key + next_mg_size as u64 <= cfg.intra_period_length as u64;
        if before_next_key && cfg.dist > 0 {
            let gap_arf = (frames_since_key
                .wrapping_add(sframe_offset as u64)
                .wrapping_add(next_mg_size as u64))
                % (cfg.dist as u64);
            if gap_arf != 0 && gap_arf < next_mg_size as u64 {
                let arf_dist = next_mg_size - gap_arf as i32;
                downgrade_hier_lvls(ctx, arf_dist);
            }
        }
    }
}

/// C `decide_sframe_mg` (`pd_process.c:1689`) — static.
///
/// Runs on a key frame, under `IS_SFRAME_FLEXIBLE_INSERT` only: the key frame
/// restarts the pyramid at full depth, and the first mini-GOP after it is then
/// shrunk if an S-frame falls inside it.
pub fn decide_sframe_mg(pic: &PicParams, cfg: &SFrameConfig<'_>, ctx: &mut PicDecisionCtx) {
    let sframe_offset = position_offset(cfg);
    ctx.next_arf_is_s = false;
    ctx.sframe_hier_lvls = i32::from(cfg.hierarchical_levels);

    let next_mg_size = 1i32 << ctx.sframe_hier_lvls;
    let mut sframe_dist = cfg.dist;
    if cfg.positions.positions.is_some() {
        let lookup = pic.picture_number.wrapping_add(sframe_offset as u64);
        let (d, d_next) = dist_to_s(&cfg.positions, lookup);
        if d > 0 {
            sframe_dist = d;
        } else if d == 0 && d_next > 0 {
            sframe_dist = d_next;
        } else {
            return;
        }
    }
    if sframe_dist < next_mg_size {
        downgrade_hier_lvls(ctx, sframe_dist);
    }
}

/// C `set_sframe_rps` (`pd_process.c:1726`) — static.
///
/// What makes an S-frame a tune-in point: `error_resilient_mode` is coded, all
/// eight DPB slots are refreshed, both layer toggles restart, and
/// `ctx.sframe_poc` is bookmarked so [`prune_sframe_refs`] can stop later
/// frames predicting across the switch.
pub fn set_sframe_rps(
    pic: &mut PicParams,
    ctx: &mut PicDecisionCtx,
    enc_ctx: &mut EncCtxPicParams,
) {
    pic.error_resilient_mode = true;
    pic.rps.refresh_frame_mask = 0xFF;
    ctx.lay0_toggle = 0;
    ctx.lay1_toggle = 0;
    ctx.sframe_poc = pic.picture_number;
    enc_ctx.elapsed_non_cra_count = 0;
}

/// C `prune_sframe_refs` (`pd_process.c:1003`) — static.
///
/// Removes from the mode-decision candidate set every reference type whose
/// list-0 half points AT the S-frame, for pictures that precede it. Motion
/// field estimation would otherwise project motion vectors across the switch
/// point, which a receiver that tuned in there cannot reconstruct.
///
/// Gated on `ctx.sframe_poc > 0 && mfmv_enabled`, so it is a no-op with no
/// S-frame pending. Returns whether anything was pruned (C sets
/// `ppcs->sframe_ref_pruned`).
///
/// # Two C defects in one expression
///
/// C's test is
/// `(rf[0] < BWDREF_FRAME && ppcs->ref_order_hint[rf[0]] == sframe_poc) ||
///  (rf[1] < BWDREF_FRAME && ppcs->ref_order_hint[rf[1]] == sframe_poc)`.
/// Both halves are wrong, in different ways (`docs/SUSPECTED-C-BUGS.md`):
///
/// 1. **`rf[1]` is `NONE_FRAME` = -1 for every SINGLE reference**
///    (`inter_prediction.h:518`), which passes `< BWDREF_FRAME` and makes C
///    evaluate `ref_order_hint[-1]` — a read one `uint32_t` before a
///    7-element array. Every call with a single-reference candidate does it,
///    which is every call. The value read is whatever `PictureParentControlSet`
///    holds there, so C's pruning decision is not even deterministic. This
///    port cannot reproduce an out-of-bounds read and does not try: `rf < 0`
///    is treated as no-match, the only defined behaviour available.
/// 2. **The index is `rf` itself**, while every other reader of
///    `ref_order_hint` uses `ref_frame - 1` (see
///    `crate::port_picstruct::set_ref_frame_sign_bias`, which documents that
///    index space). `rf[0]` is 1..=4 for a list-0 reference, so the read stays
///    in bounds and is deterministic — it is simply the neighbouring
///    reference's hint. That one IS reproduced, literally, because it is what
///    the oracle does.
pub fn prune_sframe_refs(
    pic: &PicParams,
    cfg: &SFrameConfig<'_>,
    ctx: &PicDecisionCtx,
    ref_frame_arr: &mut [i8],
    tot_ref_frames: &mut u8,
) -> bool {
    if !(ctx.sframe_poc > 0 && pic.picture_number < ctx.sframe_poc && cfg.mfmv_enabled) {
        return false;
    }
    let sframe_poc = (ctx.sframe_poc % (1u64 << cfg.order_hint_bits)) as u32;
    let mut pruned = false;
    let mut i = 0usize;
    while i < usize::from(*tot_ref_frames) {
        let rf = crate::inter_mvp::av1_set_ref_frame(ref_frame_arr[i]);
        // `0..BWDREF_FRAME` is C's `rf < BWDREF_FRAME` with the negative half
        // — C's out-of-bounds read — excluded. See the doc comment.
        let hits =
            |r: i8| (0..BWDREF_FRAME).contains(&r) && pic.ref_order_hint[r as usize] == sframe_poc;
        if hits(rf[0]) || hits(rf[1]) {
            *tot_ref_frames -= 1;
            for j in i..usize::from(*tot_ref_frames) {
                ref_frame_arr[j] = ref_frame_arr[j + 1];
            }
            pruned = true;
            // `i` deliberately does not advance: the entry that shifted down
            // into this slot has not been examined yet.
            continue;
        }
        i += 1;
    }
    pruned
}

/// C `update_sframe_ref_order_hint` (`pd_process.c:4521`) — static.
///
/// Publishes the shadow DPB's per-slot order hints into the picture's
/// `dpb_order_hint[]` — the array an S-frame's `ref_order_hint[]` header
/// syntax is written from — and then folds this frame's own hint into every
/// slot its refresh mask touches.
///
/// Trap: in LOW DELAY the published hints are made RELATIVE to the last key
/// frame (`hint - key_poc`), while random access publishes them absolute.
/// Getting that backwards writes a valid-looking header whose hints are off by
/// the whole GOP.
pub fn update_sframe_ref_order_hint(
    pic: &mut PicParams,
    cfg: &SFrameConfig<'_>,
    ctx: &mut PicDecisionCtx,
) {
    if cfg.pred_structure == PredStructure::LowDelay {
        for i in 0..REF_FRAMES {
            pic.dpb_order_hint[i] =
                (u64::from(ctx.ref_order_hint[i]).wrapping_sub(ctx.key_poc)) as u32;
        }
    } else {
        pic.dpb_order_hint = ctx.ref_order_hint;
    }
    if pic.rps.refresh_frame_mask != 0 {
        let cur = (pic.picture_number % (1u64 << cfg.order_hint_bits)) as u32;
        for i in 0..REF_FRAMES {
            if (pic.rps.refresh_frame_mask >> i) & 1 == 1 {
                ctx.ref_order_hint[i] = cur;
            }
        }
    }
}

/// Not used by this module; re-exported so a reader can see that the RPS type
/// an S-frame rewrites is the same one the prediction structure produces.
pub type SFrameRps = Av1RpsNode;

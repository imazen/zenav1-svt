//! Application-driven long-term reference management — a port of the
//! CLEAR / STORE / USE machinery in `Codec/pd_process.c:1162-1478`.
//!
//! The application can ask the encoder to pin a coded picture into a DPB slot
//! under an opaque `pic_id` (**STORE**), release it (**CLEAR**), and force a
//! later frame to predict from it alone (**USE**). That turns an ordinary
//! inter frame into a long-term anchor, which is how a conferencing or
//! screen-share sender recovers from loss without sending a key frame.
//!
//! | Rust | C (`Codec/pd_process.c`) |
//! |---|---|
//! | [`reset_state`] | `ref_mgmt_reset_state` (1162) — static |
//! | [`stored_mask`] | `ref_mgmt_stored_mask` (1172) — static |
//! | [`find_slot`] | `ref_mgmt_find_slot` (1191) — static |
//! | [`apply_clear`] | `apply_ref_clear` (1204) — static |
//! | [`exclusive_write_slots_mask_ld_cbr`] | `exclusive_write_slots_mask_ld_cbr` (1225) — static |
//! | [`storeable_slots_mask`] | `svt_aom_ref_mgmt_storeable_slots_mask` (1259) — **EXPORTED** |
//! | [`apply_store`] | `apply_ref_store` (1289) — static |
//! | [`apply_use`] | `apply_ref_use` (1336) — static |
//! | [`apply_events`] | `apply_ref_mgmt_events` (1378) — static |
//!
//! # Why this is not dead code even with no events queued
//!
//! [`apply_events`]' **phase 3 always runs**. It masks every currently-STOREd
//! slot out of `refresh_frame_mask`, so once the application holds one anchor,
//! every subsequent frame's refresh mask — a written frame-header field —
//! differs from what the prediction-structure branch chose. With no STORE held
//! the derived mask is 0 and the whole function is byte-inert, which is why
//! omitting it was invisible until now.
//!
//! # Diagnostics are values, not prints
//!
//! C reports every rejected event with `SVT_ERROR`/`SVT_WARN` and then
//! silently no-ops. Printing is not a behaviour a test can assert, so the port
//! returns the same information as [`RefMgmtReport`] and leaves the reporting
//! to the caller. The no-op semantics, and the field scrubbing that goes with
//! them, are reproduced exactly — including that a failed STORE clears
//! `store_id` so downstream packetization does not stamp the "stored" flag on
//! the output.
//!
//! # Evidence
//!
//! Tier 1 for [`storeable_slots_mask`] and, through it,
//! [`exclusive_write_slots_mask_ld_cbr`] — the exported symbol calls the
//! static one, so a differential on the wrapper drives both
//! (`tests/c_parity_picstruct_ref_mgmt.rs`). Tier 4 for the rest, which are
//! `static` and reachable only through `av1_generate_rps_info`.

use core::num::NonZeroU32;

use crate::port_picstruct::{
    INTER_REFS_PER_FRAME, LAY1_OFF, LAY2_OFF, PicDecisionCtx, PicParams, PredStructure, REF_FRAMES,
    SeqPicParams,
};

/// C `pcs->ref_mgmt` — the events the application queued on one picture.
///
/// C uses `0` as the "no id" sentinel in a `uint32_t`; that is spelled here as
/// `None`, which also makes the sentinel unrepresentable as a real id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RefMgmtEvents {
    /// C `ref_mgmt.store_id` — pin this picture under this id.
    pub store_id: Option<NonZeroU32>,
    /// C `ref_mgmt.clear_id` — release the slot holding this id.
    pub clear_id: Option<NonZeroU32>,
    /// C `ref_mgmt.use_id` — predict this picture only from this id.
    pub use_id: Option<NonZeroU32>,
}

impl RefMgmtEvents {
    /// C `store_id != 0 || clear_id != 0 || use_id != 0`.
    #[must_use]
    pub fn any(self) -> bool {
        self.store_id.is_some() || self.clear_id.is_some() || self.use_id.is_some()
    }
}

/// One diagnostic C would have printed. Never affects the bitstream on its own
/// — the accompanying no-op does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMgmtDiag {
    /// C: "ignoring events on AV1 overlay frame".
    EventsOnOverlay,
    /// C: "ignoring events on non-base frame".
    EventsOnNonBase {
        /// The picture's temporal layer.
        temporal_layer: u8,
    },
    /// C: "duplicate pic_id across STORE/CLEAR/USE on same frame".
    DuplicateIdAcrossEvents,
    /// C: "CLEAR pic_id not found in DPB".
    ClearIdNotFound(NonZeroU32),
    /// C: "STORE pic_id already STOREd".
    StoreIdAlreadyHeld(NonZeroU32),
    /// C: "already at max_managed_refs cap".
    StoreCapReached {
        /// Slots currently holding an id.
        held: u8,
        /// `scs->static_config.max_managed_refs`.
        cap: u8,
    },
    /// C: "safe slot pool is full".
    StorePoolFull {
        /// The mask [`storeable_slots_mask`] returned.
        storeable_mask: u8,
    },
    /// C: "USE pic_id not found in DPB".
    UseIdNotFound(NonZeroU32),
    /// C: "refresh_frame_mask collapsed to 0" — the frame is valid AV1 but its
    /// reconstruction never enters the DPB.
    RefreshMaskCollapsed {
        /// What the prediction-structure branch had chosen.
        wanted: u8,
        /// The STOREd slots that masked it away.
        preserved: u8,
    },
}

/// What [`apply_events`] did, in the order C would have reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefMgmtReport {
    /// The slot a successful STORE claimed.
    pub new_store_slot: Option<u8>,
    /// Whether a USE actually redirected the references.
    pub use_applied: bool,
    /// Every diagnostic C would have printed, in C's order.
    pub diagnostics: heapless_diags::Diags,
}

/// A fixed-capacity diagnostic list, so [`apply_events`] allocates nothing.
///
/// Eight is the ceiling C can reach in one call: the three event-validation
/// messages are mutually exclusive with the per-phase ones, and each phase
/// reports at most one.
pub mod heapless_diags {
    use super::RefMgmtDiag;

    /// At most this many diagnostics can be produced by one call.
    pub const MAX: usize = 8;

    /// A push-only, fixed-capacity list of [`RefMgmtDiag`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Diags {
        items: [Option<RefMgmtDiag>; MAX],
        len: usize,
    }

    impl Diags {
        pub(super) fn push(&mut self, d: RefMgmtDiag) {
            if self.len < MAX {
                self.items[self.len] = Some(d);
                self.len += 1;
            }
        }
        /// The diagnostics recorded, in order.
        pub fn as_slice(&self) -> impl Iterator<Item = RefMgmtDiag> + '_ {
            self.items[..self.len].iter().filter_map(|d| *d)
        }
        /// How many were recorded.
        #[must_use]
        pub fn len(&self) -> usize {
            self.len
        }
        /// Whether none were recorded.
        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
        /// Whether a particular diagnostic was recorded.
        #[must_use]
        pub fn contains(&self, d: RefMgmtDiag) -> bool {
            self.items[..self.len].contains(&Some(d))
        }
    }
}

/// C `ref_mgmt_reset_state` (`pd_process.c:1162`) — static.
///
/// Called from `set_key_frame_rps`: a key frame refreshes all eight slots, so
/// every held anchor is destroyed and its id must stop resolving.
pub fn reset_state(ctx: &mut PicDecisionCtx) {
    ctx.pic_id_per_dpb_slot = [None; REF_FRAMES];
}

/// C `ref_mgmt_stored_mask` (`pd_process.c:1172`) — static.
///
/// Bit `i` is set iff slot `i` holds an id. C derives this on demand rather
/// than keeping a second mask in sync, and so does this.
#[must_use]
pub fn stored_mask(ctx: &PicDecisionCtx) -> u8 {
    let mut m = 0u8;
    for (i, slot) in ctx.pic_id_per_dpb_slot.iter().enumerate() {
        if slot.is_some() {
            m |= 1u8 << i;
        }
    }
    m
}

/// C `ref_mgmt_find_slot` (`pd_process.c:1191`) — static.
///
/// C returns `REF_FRAMES` on a miss and special-cases the `pic_id == 0`
/// sentinel; both collapse into `Option` here, since [`NonZeroU32`] cannot be
/// the sentinel.
#[must_use]
pub fn find_slot(ctx: &PicDecisionCtx, pic_id: NonZeroU32) -> Option<u8> {
    ctx.pic_id_per_dpb_slot
        .iter()
        .position(|s| *s == Some(pic_id))
        .map(|i| u8::try_from(i).expect("REF_FRAMES is 8"))
}

/// C `exclusive_write_slots_mask_ld_cbr` (`pd_process.c:1225`) — static.
///
/// The slots some active low-delay CBR branch refreshes as the SOLE bit of
/// `refresh_frame_mask`. Locking one of those would collapse that branch's
/// mask to zero in [`apply_events`]' phase 3, so they are excluded from the
/// STORE pool.
///
/// C's `default:` arm for an out-of-range `ld_reduce_ref_buffs` is
/// `assert(0)`, which under `NDEBUG` — the Release build the oracle uses —
/// adds no bits. That is what this reproduces.
#[must_use]
pub fn exclusive_write_slots_mask_ld_cbr(seq: &SeqPicParams) -> u8 {
    let mut mask = 0u8;
    let hier = seq.hierarchical_levels;
    let ld_reduce = seq.mrp_ctrls.ld_reduce_ref_buffs;

    // Layer 0 writes a single `1 << lay0_toggle` (toggle in 0..=2) only at
    // ld_reduce 0; the `| 0xf0` / `| 0xfc` backup bits make it non-exclusive
    // otherwise.
    if ld_reduce == 0 {
        mask |= 0x07;
    }
    if hier >= 1 {
        mask |= match ld_reduce {
            0 => (1u8 << LAY1_OFF) | (1u8 << (LAY1_OFF + 1)),
            1 => 1u8 << LAY1_OFF,
            2 => 1u8 << 1,
            _ => 0,
        };
    }
    // Layer 2's refresh is force-zeroed at ld_reduce > 0, so it is exclusive
    // only at 0.
    if hier >= 2 && ld_reduce == 0 {
        mask |= 1u8 << LAY2_OFF;
    }
    mask
}

/// C `svt_aom_ref_mgmt_storeable_slots_mask` (`pd_process.c:1259`) —
/// **EXPORTED**, tier 1.
///
/// Which DPB slots a STORE may claim.
///
/// * Flat RTC: the top four. Slots 0..3 are the sliding window the layer-0
///   toggle rotates through, and freezing one would silently drop a live
///   reference out of that window; slots 4..7 are the per-frame `| 0xf0`
///   backup and are never read as references.
/// * Low delay with a pyramid: everything the LD-CBR branches do not write
///   exclusively (see [`exclusive_write_slots_mask_ld_cbr`]).
/// * Everything else: all eight. Long-term references are rejected for those
///   configurations upstream in `enc_settings.c`, so the value is unreachable
///   rather than permissive.
#[must_use]
pub fn storeable_slots_mask(seq: &SeqPicParams) -> u8 {
    if seq.rtc && seq.hierarchical_levels == 0 {
        return 0xF0;
    }
    if seq.pred_structure == PredStructure::LowDelay && seq.hierarchical_levels >= 1 {
        return !exclusive_write_slots_mask_ld_cbr(seq);
    }
    0xFF
}

/// C `apply_ref_clear` (`pd_process.c:1204`) — static.
///
/// Releases the slot holding `clear_id`. An unknown id is a diagnostic and a
/// no-op, exactly as in C.
fn apply_clear(pic: &PicParams, ctx: &mut PicDecisionCtx, report: &mut RefMgmtReport) {
    let Some(pid) = pic.ref_mgmt.clear_id else {
        return;
    };
    match find_slot(ctx, pid) {
        Some(slot) => ctx.pic_id_per_dpb_slot[slot as usize] = None,
        None => report.diagnostics.push(RefMgmtDiag::ClearIdNotFound(pid)),
    }
}

/// C `apply_ref_store` (`pd_process.c:1289`) — static.
///
/// Claims the LOWEST free storeable slot and force-sets its bit in
/// `refresh_frame_mask`, so this frame's reconstruction lands there whatever
/// the prediction-structure branch chose. Three failure modes, each a
/// diagnostic and a no-op: a duplicate id, the simultaneous-hold cap, and an
/// exhausted pool.
fn apply_store(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    report: &mut RefMgmtReport,
) -> Option<u8> {
    let pid = pic.ref_mgmt.store_id?;
    if find_slot(ctx, pid).is_some() {
        report
            .diagnostics
            .push(RefMgmtDiag::StoreIdAlreadyHeld(pid));
        return None;
    }
    let held = stored_mask(ctx).count_ones();
    let cap = u32::from(seq.max_managed_refs);
    if held >= cap {
        report.diagnostics.push(RefMgmtDiag::StoreCapReached {
            held: u8::try_from(held).expect("at most 8 slots"),
            cap: seq.max_managed_refs,
        });
        return None;
    }
    let storeable = storeable_slots_mask(seq);
    let free = storeable & !stored_mask(ctx);
    if free == 0 {
        report.diagnostics.push(RefMgmtDiag::StorePoolFull {
            storeable_mask: storeable,
        });
        return None;
    }
    let slot = u8::try_from(free.trailing_zeros()).expect("free is nonzero and 8 bits");
    pic.rps.refresh_frame_mask |= 1u8 << slot;
    ctx.pic_id_per_dpb_slot[slot as usize] = Some(pid);
    Some(slot)
}

/// C `apply_ref_use` (`pd_process.c:1336`) — static.
///
/// Points ALL seven AV1 reference positions at the anchor's slot and clamps
/// the list counts to `(1, 0)`. Both halves matter: the splatter means a mode
/// decision that picks LAST2..ALT still resolves to the anchor, and the clamp
/// stops it building compound candidates from references that no longer exist.
fn apply_use(pic: &mut PicParams, ctx: &PicDecisionCtx, report: &mut RefMgmtReport) -> bool {
    let Some(pid) = pic.ref_mgmt.use_id else {
        return false;
    };
    let Some(slot) = find_slot(ctx, pid) else {
        report.diagnostics.push(RefMgmtDiag::UseIdNotFound(pid));
        return false;
    };
    let poc = ctx.dpb[slot as usize].picture_number;
    pic.rps.ref_dpb_index = [slot; INTER_REFS_PER_FRAME];
    pic.rps.ref_poc_array = [poc; INTER_REFS_PER_FRAME];
    pic.ref_list0_count = 1;
    pic.ref_list1_count = 0;
    true
}

/// C `apply_ref_mgmt_events` (`pd_process.c:1378`) — static.
///
/// The dispatcher, run at the end of every `av1_generate_rps_info` — including
/// the key-frame early return. Four phases in C's order:
///
/// 1. **CLEAR** first, so a same-frame STORE can reuse the slot it freed.
/// 2. **STORE**, which claims a slot and force-refreshes it.
/// 3. **The refresh guard**, which runs whether or not there were events: every
///    slot still holding an id is masked out of `refresh_frame_mask`, except
///    the one this frame just STOREd. With nothing held this is a no-op.
/// 4. **USE**, which redirects the references and then rewrites the refresh
///    mask to every non-held slot — a recovery point, after which future
///    frames can only reference anchors or this frame.
///
/// Three gates reject the whole event set before phase 1: an overlay frame
/// (whose refresh mask is force-zeroed downstream anyway), a non-base frame
/// (which cannot be a standalone anchor), and a `pic_id` reused across two
/// events of the same picture.
pub fn apply_events(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
) -> RefMgmtReport {
    let mut report = RefMgmtReport::default();

    if pic.ref_mgmt.any() {
        let ok = if pic.is_overlay {
            report.diagnostics.push(RefMgmtDiag::EventsOnOverlay);
            false
        } else if pic.temporal_layer_index != 0 {
            report.diagnostics.push(RefMgmtDiag::EventsOnNonBase {
                temporal_layer: pic.temporal_layer_index,
            });
            false
        } else {
            let e = pic.ref_mgmt;
            let clash = |a: Option<NonZeroU32>, b: Option<NonZeroU32>| a.is_some() && a == b;
            if clash(e.store_id, e.clear_id)
                || clash(e.store_id, e.use_id)
                || clash(e.clear_id, e.use_id)
            {
                report
                    .diagnostics
                    .push(RefMgmtDiag::DuplicateIdAcrossEvents);
                false
            } else {
                true
            }
        };
        if !ok {
            pic.ref_mgmt = RefMgmtEvents::default();
        }
    }

    // Phase 1 — CLEAR. C scrubs `clear_id` unconditionally afterwards so a
    // downstream consumer never sees a phantom CLEAR behind the warning.
    if pic.ref_mgmt.clear_id.is_some() {
        apply_clear(pic, ctx, &mut report);
        pic.ref_mgmt.clear_id = None;
    }

    // Phase 2 — STORE. A failed STORE scrubs `store_id` so packetization does
    // NOT stamp the "reference stored" flag on the output; that flag is the
    // application's ground truth for its own anchor bookkeeping.
    if pic.ref_mgmt.store_id.is_some() {
        report.new_store_slot = apply_store(pic, seq, ctx, &mut report);
        if report.new_store_slot.is_none() {
            pic.ref_mgmt.store_id = None;
        }
    }

    // Phase 3 — the refresh guard. Always runs.
    {
        let stored = stored_mask(ctx);
        let preserve = match report.new_store_slot {
            Some(slot) => stored & !(1u8 << slot),
            None => stored,
        };
        let wanted = pic.rps.refresh_frame_mask;
        pic.rps.refresh_frame_mask &= !preserve;
        if wanted != 0 && pic.rps.refresh_frame_mask == 0 && !pic.is_overlay {
            report.diagnostics.push(RefMgmtDiag::RefreshMaskCollapsed {
                wanted,
                preserved: preserve,
            });
        }
    }

    // Phase 4 — USE, and the recovery-point refresh that follows a successful
    // one.
    if pic.ref_mgmt.use_id.is_some() && apply_use(pic, ctx, &mut report) {
        report.use_applied = true;
        // C writes `0xFFu & ~ref_mgmt_stored_mask(ctx)`; the `0xFF` mask is
        // redundant on a u8 and is dropped.
        let mut refresh = !stored_mask(ctx);
        if let Some(slot) = report.new_store_slot {
            refresh |= 1u8 << slot;
        }
        pic.rps.refresh_frame_mask = refresh;
    }

    report
}

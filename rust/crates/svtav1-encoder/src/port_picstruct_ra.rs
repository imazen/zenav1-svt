//! The random-access **hierarchical** reference-structure branches of
//! `av1_generate_rps_info` (`Codec/pd_process.c:2270-3482`) — one per
//! `hierarchical_levels` in 1..=5.
//!
//! These are the pyramid GOPs: a base layer, `hierarchical_levels` layers of
//! bidirectionally-predicted frames above it, and a fixed assignment of the
//! eight DPB slots to the layers. Every value this module produces is a
//! written frame-header field (`ref_frame_idx[]`, `refresh_frame_flags`,
//! `show_frame`, `show_existing_frame`) or a direct input to one, so a wrong
//! entry is a wrong bitstream — not a quality regression.
//!
//! Until 2026-08-31 these five branches were refused by
//! [`crate::port_picstruct::generate_rps_info`] rather than guessed. This
//! module translates them.
//!
//! # Shape of the translation
//!
//! C writes each case as seven consecutive `ref_dpb_index[...] = ...`
//! assignments over symbolic slot names (`base2_idx`, `lay1_1_idx`, …), about
//! 1,200 lines of them. That is a **table**, and it is transcribed here as
//! one: [`slot_table`] returns the seven [`Slot`]s in C's own assignment order
//! — `[LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT]` — and [`SlotIdx::resolve`]
//! turns them into DPB indices. Three faithful simplifications, each
//! value-preserving:
//!
//! * C's `ref_dpb_index[GOLD] = ref_dpb_index[LAST]` (and the `ALT = BWD`
//!   pairs) are written as the concrete slot the mirrored entry holds in that
//!   same case. Nothing else can be observed: the mirror is always assigned
//!   two lines above it, in the same arm.
//! * The per-branch `lay1_toggle` adjustment guard — `pic_idx == 0` at HL2,
//!   `pic_idx < 3` at HL3, `< 7` at HL4, `< 15` at HL5, and absent at HL1 —
//!   is the single expression `pic_idx < (1 << (hier - 1)) - 1`, which yields
//!   0, 1, 3, 7, 15 for HL1..HL5. Written once, in [`toggles_for_picture`].
//! * The `(temporal_layer, pic_idx)` pairs C does not enumerate end in
//!   `SVT_LOG("Error in MG indexing …")` and then fall through with a **stale**
//!   `ref_dpb_index`, producing an RPS from the previous picture. This port
//!   returns [`RpsError::MiniGopIndex`] instead, per
//!   `docs/WORKING-ON-THIS.md` §6 — a wrong reference structure that decodes
//!   is exactly the failure that rule exists to prevent.
//!
//! # Evidence
//!
//! **Tier 2** — `tests/c_parity_picstruct_ra_rps.rs` reads
//! `refresh_frame_flags`, `ref_frame_idx[]`, `show_frame` and
//! `frame_to_show_map_idx` straight out of ten REAL C-encoder bitstreams
//! (`hierarchical_levels` 1..=5 at presets 8 and 4, regenerate with
//! `tools/gen_ra_rps_captures.sh`, inspect with `tools/ra_rps_oracle.py`).
//! Every `pic_idx` of every table is exercised; of the 1,092 reference columns
//! compared, 865 carry the table's own value and 227 carry `prune_refs`'s.
//! Tier 1 is not reachable: `av1_generate_rps_info` is `static` with no
//! exported symbol.
//!
//! **Tier 4** — `tests/port_picstruct_ra_traced.rs` covers what the bitstream
//! cannot witness: the `is_ref` gating of each layer's refresh mask, the HL1
//! overlay's skipped toggle, the HL2 low-delay long-term base, and the
//! low-delay toggle adjustment.

use crate::port_picstruct::{
    ALT, ALT2, BWD, GOLD, INTER_REFS_PER_FRAME, LAST, LAST2, LAST3, LAY1_OFF, LAY2_OFF, LAY3_OFF,
    LAY4_OFF, PicDecisionCtx, PicParams, PredStructure, RpsError, SeqPicParams, circ_dec, circ_inc,
    prune_refs, set_frame_display_params, set_ref_list_counts, update_ref_poc_array,
};

/// C's long-term base slot (`long_base_idx`, `pd_process.c:2396`) — DPB slot 7,
/// refreshed every `LONG_BASE_PIC` pictures on the HL2 low-delay arm.
const LONG_BASE_IDX: u8 = 7;
/// C `long_base_pic` (`pd_process.c:2397`).
const LONG_BASE_PIC: u64 = 128;

/// A DPB slot named the way C's RPS tables name it.
///
/// The numeric suffixes are C's: `base2` is the **newest** layer-0 picture in
/// the DPB and `base0` the oldest; `lay1_1` is the newest layer-1 picture and
/// `lay1_0` the older of the two. Layers 2, 3 and 4 hold exactly one picture
/// each, at fixed slots [`LAY2_OFF`], [`LAY3_OFF`] and [`LAY4_OFF`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// C `base0_idx` — the oldest layer-0 picture in the DPB.
    Base0,
    /// C `base1_idx` — the middle layer-0 picture.
    Base1,
    /// C `base2_idx` — the newest layer-0 picture.
    Base2,
    /// C `lay1_0_idx` — the older of the two layer-1 pictures.
    Lay1Prev,
    /// C `lay1_1_idx` — the newest layer-1 picture.
    Lay1Cur,
    /// C `lay2_idx` = [`LAY2_OFF`].
    Lay2,
    /// C `lay3_idx` = [`LAY3_OFF`].
    Lay3,
    /// C `lay4_idx` = [`LAY4_OFF`].
    Lay4,
    /// C `long_base_idx` — slot 7, the HL2 low-delay long-term base reference.
    LongBase,
}

/// The DPB slot each [`Slot`] name resolves to for one picture.
///
/// Built by [`toggles_for_picture`], which is where the layer-0 and layer-1
/// toggles — the whole reason this derivation is stateful — are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotIdx {
    /// C `base0_idx`.
    pub base0: u8,
    /// C `base1_idx`.
    pub base1: u8,
    /// C `base2_idx`.
    pub base2: u8,
    /// C `lay1_0_idx`.
    pub lay1_prev: u8,
    /// C `lay1_1_idx`.
    pub lay1_cur: u8,
}

impl SlotIdx {
    /// The DPB index for one symbolic slot.
    #[must_use]
    pub fn resolve(self, slot: Slot) -> u8 {
        match slot {
            Slot::Base0 => self.base0,
            Slot::Base1 => self.base1,
            Slot::Base2 => self.base2,
            Slot::Lay1Prev => self.lay1_prev,
            Slot::Lay1Cur => self.lay1_cur,
            Slot::Lay2 => LAY2_OFF,
            Slot::Lay3 => LAY3_OFF,
            Slot::Lay4 => LAY4_OFF,
            Slot::LongBase => LONG_BASE_IDX,
        }
    }

    /// Resolve a whole `[LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT]` row.
    #[must_use]
    pub fn resolve_row(self, row: [Slot; INTER_REFS_PER_FRAME]) -> [u8; INTER_REFS_PER_FRAME] {
        row.map(|s| self.resolve(s))
    }
}

/// C's per-branch toggle preamble (`pd_process.c:2271-2288` and the identical
/// blocks at `:2369`, `:2511`, `:2713`, `:3014`).
///
/// The toggles in [`PicDecisionCtx`] are maintained in **decode** order, which
/// for random access puts the base picture first and the layer-1 picture
/// second. A picture whose own prediction structure is `LOW_DELAY` inside a
/// random-access sequence (an incomplete mini-GOP at a GOP boundary) is
/// decoded in display order instead, so C compensates by advancing the local
/// copies: layer 0 always, layer 1 only for the first half of the mini-GOP.
///
/// "First half" is `pic_idx == 0` at HL2, `< 3` at HL3, `< 7` at HL4 and
/// `< 15` at HL5 — i.e. `(1 << (hier - 1)) - 1` — and HL1 has no layer-1
/// adjustment at all, which the same expression gives (threshold 0, never
/// satisfied).
#[must_use]
pub fn toggles_for_picture(pic: &PicParams, ctx: &PicDecisionCtx, pic_idx: u32) -> SlotIdx {
    let mut lay0_toggle = ctx.lay0_toggle;
    let mut lay1_toggle = ctx.lay1_toggle;

    if pic.pred_struct_type != PredStructure::RandomAccess && pic.temporal_layer_index != 0 {
        lay0_toggle = circ_inc(lay0_toggle, 0, 2);
        let first_half = (1u32 << (pic.hierarchical_levels - 1)) - 1;
        if pic_idx < first_half {
            lay1_toggle = 1 - lay1_toggle;
        }
    }

    let base2 = lay0_toggle;
    let base1 = circ_dec(base2, 0, 2);
    let base0 = circ_dec(base1, 0, 2);
    let lay1_cur = LAY1_OFF + lay1_toggle;
    let lay1_prev = circ_dec(lay1_cur, LAY1_OFF, LAY1_OFF + 1);

    SlotIdx {
        base0,
        base1,
        base2,
        lay1_prev,
        lay1_cur,
    }
}

/// The reference row for one `(hierarchical_levels, temporal_layer, pic_idx)`,
/// in C's assignment order `[LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT]`.
///
/// `None` is C's `SVT_LOG("Error in MG indexing …")` fall-through.
///
/// `referencing_scheme` and `more_5l_refs` are the two `MrpCtrls` fields that
/// change a row rather than merely capping the list lengths.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn slot_table(
    hier: u8,
    temporal_layer: u8,
    pic_idx: u32,
    referencing_scheme: u8,
    more_5l_refs: bool,
    is_overlay: bool,
) -> Option<[Slot; INTER_REFS_PER_FRAME]> {
    use Slot::{Base0, Base1, Base2, Lay1Cur, Lay1Prev, Lay2, Lay3, Lay4};

    // Every top layer's overlay frame references only the newest base picture.
    let overlay_row = [Base2; INTER_REFS_PER_FRAME];

    Some(match (hier, temporal_layer) {
        // -------------------------------------------------------------- HL1
        // pd_process.c:2296-2344
        (1, 0) => [Base2, Base0, Base2, Base2, Base2, Base1, Base2],
        (1, 1) if is_overlay => overlay_row,
        (1, 1) => {
            // LAST2 / ALT2 are the only entries the referencing scheme moves.
            let last2 = if referencing_scheme == 0 {
                Base0
            } else {
                Lay1Cur
            };
            let alt2 = if referencing_scheme == 0 {
                Base2
            } else {
                Lay1Prev
            };
            [Base1, last2, Base0, Base1, Base2, alt2, Base2]
        }

        // -------------------------------------------------------------- HL2
        // pd_process.c:2397-2478. Layer 0's LAST3 is the long-term base only
        // in low delay; the caller passes `hier == 2` with the sequence's own
        // pred structure folded into `long_base_last3`.
        (2, 0) => [Base2, Base0, Base2, Base2, Base2, Base1, Base2],
        (2, 1) => [Base1, Lay1Cur, Base0, Base1, Base2, Base2, Base2],
        (2, 2) if is_overlay => overlay_row,
        (2, 2) => match pic_idx {
            0 => [Base1, Lay1Prev, Base0, Base1, Lay1Cur, Base2, Lay1Cur],
            2 => [Lay1Cur, Base1, Lay2, Lay1Cur, Base2, Base2, Base2],
            _ => return None,
        },

        // -------------------------------------------------------------- HL3
        // pd_process.c:2555-2676
        (3, 0) => [Base2, Base0, Base2, Base2, Base2, Base1, Base2],
        (3, 1) => [Base1, Lay1Cur, Base0, Base1, Base2, Base2, Base2],
        (3, 2) => match pic_idx {
            1 => [Base1, Lay2, Lay1Prev, Base0, Lay1Cur, Base2, Lay1Cur],
            5 => [Lay1Cur, Lay2, Base1, Lay1Prev, Base2, Base2, Base2],
            _ => return None,
        },
        (3, 3) if is_overlay => overlay_row,
        (3, 3) => match pic_idx {
            0 => [Base1, Lay1Prev, Lay3, Base1, Lay2, Lay1Cur, Base2],
            2 => [Lay2, Base1, Lay3, Lay2, Lay1Cur, Base2, Lay1Cur],
            4 => [Lay1Cur, Base1, Lay3, Lay1Cur, Lay2, Base2, Lay2],
            6 => [Lay2, Lay1Cur, Lay3, Lay2, Base2, Base2, Base2],
            _ => return None,
        },

        // -------------------------------------------------------------- HL4
        // pd_process.c:2758-2975
        (4, 0) => {
            let last3 = if more_5l_refs { Lay1Cur } else { Base2 };
            let alt = if more_5l_refs { Lay1Prev } else { Base2 };
            [Base2, Base0, last3, Base2, Base2, Base1, alt]
        }
        (4, 1) => [Base1, Lay1Cur, Base0, Base1, Base2, Lay2, Base2],
        (4, 2) => match pic_idx {
            3 => {
                let alt = if more_5l_refs { Lay3 } else { Lay1Cur };
                [Base1, Lay2, Lay1Prev, Base0, Lay1Cur, Base2, alt]
            }
            11 => {
                let alt = if more_5l_refs { Lay1Prev } else { Base2 };
                [Lay1Cur, Lay2, Base1, Lay3, Base2, Lay4, alt]
            }
            _ => return None,
        },
        (4, 3) => match pic_idx {
            1 => [Base1, Lay3, Lay1Prev, Base0, Lay2, Lay1Cur, Base2],
            5 => {
                let alt = if more_5l_refs { Lay4 } else { Lay1Cur };
                [Lay2, Lay3, Base1, Lay1Prev, Lay1Cur, Base2, alt]
            }
            9 => [Lay1Cur, Lay3, Base1, Lay1Prev, Lay2, Base2, Lay2],
            13 => [Lay2, Lay3, Lay1Cur, Base1, Base2, Lay4, Base2],
            _ => return None,
        },
        (4, 4) if is_overlay => overlay_row,
        (4, 4) => match pic_idx {
            0 => [Base1, Lay1Prev, Lay4, Base0, Lay3, Lay2, Lay1Cur],
            2 => [Lay3, Base1, Lay4, Lay1Prev, Lay2, Lay1Cur, Base2],
            4 => [Lay2, Base1, Lay4, Lay1Prev, Lay3, Lay1Cur, Base2],
            6 => {
                let alt = if more_5l_refs { Lay1Prev } else { Lay1Cur };
                [Lay3, Lay2, Lay4, Base1, Lay1Cur, Base2, alt]
            }
            8 => [Lay1Cur, Base1, Lay4, Lay1Prev, Lay3, Lay2, Base2],
            10 => [Lay3, Lay1Cur, Lay4, Base1, Lay2, Base2, Lay2],
            12 => [Lay2, Lay1Cur, Lay4, Base1, Lay3, Base2, Lay3],
            14 => [Lay3, Lay2, Lay4, Lay1Cur, Base2, Base1, Base2],
            _ => return None,
        },

        // -------------------------------------------------------------- HL5
        // pd_process.c:3049-3425
        (5, 0) => [Base2, Base1, Base0, Base2, Base2, Lay1Cur, Base2],
        (5, 1) => [Base1, Lay1Cur, Base0, Lay1Prev, Base2, Lay2, Lay3],
        (5, 2) => match pic_idx {
            7 => [Base1, Lay2, Lay1Prev, Base1, Lay1Cur, Base2, Lay3],
            23 => [Lay1Cur, Lay2, Base1, Lay1Cur, Base2, Lay4, Lay1Prev],
            _ => return None,
        },
        (5, 3) => match pic_idx {
            3 => [Base1, Lay3, Lay1Prev, Base0, Lay2, Lay1Cur, Base2],
            11 => [Lay2, Lay3, Base1, Lay1Prev, Lay1Cur, Base2, Lay1Cur],
            19 => [Lay1Cur, Lay3, Base1, Lay1Cur, Lay2, Base2, Lay2],
            27 => [Lay2, Lay3, Lay1Cur, Base1, Base2, Base0, Base2],
            _ => return None,
        },
        (5, 4) => match pic_idx {
            1 => [Base1, Lay4, Lay1Prev, Base2, Lay3, Lay2, Lay1Cur],
            5 => [Lay3, Lay4, Base1, Lay1Prev, Lay2, Lay1Cur, Base2],
            9 => [Lay2, Lay4, Base1, Lay1Prev, Lay3, Lay1Cur, Base2],
            13 => [Lay3, Lay4, Lay2, Base1, Lay1Cur, Base2, Lay1Cur],
            17 => [Lay1Cur, Lay4, Base1, Lay1Prev, Lay3, Lay2, Base2],
            21 => [Lay3, Lay4, Lay1Cur, Base1, Lay2, Base2, Lay2],
            25 => [Lay2, Lay4, Lay1Cur, Base1, Lay3, Base2, Lay3],
            29 => [Lay3, Lay4, Lay2, Lay1Cur, Base2, Base1, Base0],
            _ => return None,
        },
        (5, 5) if is_overlay => overlay_row,
        (5, 5) => match pic_idx {
            0 => [Base1, Lay1Prev, Lay1Cur, Base2, Lay4, Lay3, Lay2],
            2 => [Lay4, Base1, Lay1Prev, Base2, Lay3, Lay2, Lay1Cur],
            4 => [Lay3, Base1, Lay1Prev, Lay3, Lay4, Lay2, Lay1Cur],
            6 => [Lay4, Lay3, Base1, Lay4, Lay2, Lay1Cur, Base2],
            8 => [Lay2, Base1, Lay1Prev, Lay2, Lay4, Lay3, Lay1Cur],
            10 => [Lay4, Lay2, Base1, Lay1Prev, Lay3, Lay1Cur, Base2],
            12 => [Lay3, Lay2, Base1, Lay1Prev, Lay4, Lay1Cur, Base2],
            14 => [Lay4, Lay3, Lay2, Lay1Prev, Lay1Cur, Base2, Base1],
            16 => [Lay1Cur, Base1, Lay1Prev, Base2, Lay4, Lay3, Lay2],
            18 => [Lay4, Lay1Cur, Base1, Lay1Prev, Lay3, Lay2, Base2],
            20 => [Lay3, Lay1Cur, Base1, Lay1Prev, Lay4, Lay2, Base2],
            22 => [Lay4, Lay3, Lay1Cur, Base1, Lay2, Base2, Base0],
            24 => [Lay2, Lay1Cur, Base1, Lay1Prev, Lay4, Lay3, Base2],
            26 => [Lay4, Lay2, Lay1Cur, Base1, Lay3, Base2, Base0],
            28 => [Lay3, Lay2, Lay1Cur, Base1, Lay4, Base2, Base0],
            30 => [Lay4, Lay3, Lay2, Base1, Base2, Lay1Cur, Base0],
            _ => return None,
        },

        _ => return None,
    })
}

/// C's `show_existing_frame` slot for a top-layer picture of a complete
/// random-access mini-GOP (`pd_process.c:2352`, `:2494`, `:2694`, `:2957`,
/// `:3402`).
///
/// `None` is C's `SVT_LOG("Error in GOP indexing …")` fall-through.
#[must_use]
pub fn show_existing_slot(hier: u8, pic_idx: u32) -> Option<Slot> {
    use Slot::{Base2, Lay1Cur, Lay2, Lay3, Lay4};
    Some(match (hier, pic_idx) {
        (1, 0) => Base2,

        (2, 0) => Lay1Cur,
        (2, 2) => Base2,

        (3, 0 | 4) => Lay2,
        (3, 2) => Lay1Cur,
        (3, 6) => Base2,

        (4, 0 | 4 | 8 | 12) => Lay3,
        (4, 2 | 10) => Lay2,
        (4, 6) => Lay1Cur,
        (4, 14) => Base2,

        (5, 0 | 4 | 8 | 12 | 16 | 20 | 24 | 28) => Lay4,
        (5, 2 | 10 | 18 | 26) => Lay3,
        (5, 6 | 22) => Lay2,
        (5, 14) => Lay1Cur,
        (5, 30) => Base2,

        _ => return None,
    })
}

/// C `av1_generate_rps_info`'s `hierarchical_levels` 1..=5 branches
/// (`pd_process.c:2270-3482`) — static, tier 4.
///
/// Fills `ref_dpb_index[7]`, `ref_poc_array[7]`, `refresh_frame_mask`,
/// `show_frame`, `has_show_existing` and `show_existing_frame`, and advances
/// the layer toggles in `ctx`.
///
/// # Errors
///
/// [`RpsError::MiniGopIndex`] where C would log `Error in MG indexing` and
/// continue with a stale reference structure;
/// [`RpsError::UnsupportedBranch`] for `hierarchical_levels` outside 1..=5,
/// where C prints `Unsupported MG structure!` and calls `exit(0)`.
pub fn rps_random_access_hier(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsError> {
    let hier = pic.hierarchical_levels;
    let tl = pic.temporal_layer_index;
    if !(1..=5).contains(&hier) {
        return Err(RpsError::UnsupportedBranch {
            hierarchical_levels: hier,
            temporal_layer: tl,
        });
    }

    let idx = toggles_for_picture(pic, ctx, pic_idx);
    let mrp = &seq.mrp_ctrls;

    let mut row = slot_table(
        hier,
        tl,
        pic_idx,
        mrp.referencing_scheme,
        mrp.more_5l_refs != 0,
        pic.is_overlay,
    )
    .ok_or(RpsError::MiniGopIndex {
        hierarchical_levels: hier,
        temporal_layer: tl,
        pic_idx,
    })?;

    // HL2 layer 0, low delay only: LAST3 is the long-term base in slot 7
    // (`pd_process.c:2403-2407`). In random access it stays == LAST.
    if hier == 2 && tl == 0 && seq.pred_structure == PredStructure::LowDelay {
        row[LAST3] = Slot::LongBase;
    }

    pic.rps.ref_dpb_index = idx.resolve_row(row);

    // The refresh mask and the toggle advance, per layer. Note that C advances
    // `ctx->lay0_toggle` / `ctx->lay1_toggle` from the CONTEXT value, not from
    // the locally adjusted copy in `idx` — the adjustment only ever applies to
    // a non-base picture, and only layers 0 and 1 advance a toggle.
    pic.rps.refresh_frame_mask = if tl == 0 {
        ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
        1u8 << ctx.lay0_toggle
    } else if tl == 1 {
        if hier == 1 && pic.is_overlay {
            // HL1's layer-1 overlay arm (`pd_process.c:2325-2334`) is the ONE
            // layer-1 path that does not advance the toggle; it zeroes the
            // mask directly.
            0
        } else {
            ctx.lay1_toggle = 1 - ctx.lay1_toggle;
            let mask = 1u8 << (LAY1_OFF + ctx.lay1_toggle);
            // HL1's layer 1 is the TOP layer, so it is the one layer-1 arm
            // gated on `is_ref` (`pd_process.c:2341`); HL2..HL5 refresh
            // unconditionally.
            if hier == 1 && !pic.is_ref { 0 } else { mask }
        }
    } else if hier == 5 && tl == 5 {
        // The one top layer C zeroes unconditionally (`pd_process.c:3424`)
        // rather than through `is_ref`.
        0
    } else {
        let slot = match tl {
            2 => LAY2_OFF,
            3 => LAY3_OFF,
            _ => LAY4_OFF,
        };
        // Every other top layer is `is_ref`-gated; the layers below it are not.
        if tl == hier && !pic.is_ref {
            0
        } else {
            1u8 << slot
        }
    };

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);

    // HL2 low delay only (`pd_process.c:2484-2489`): keep a long-term base
    // reference alive by refreshing slot 7 every 128 pictures.
    if hier == 2
        && seq.pred_structure == PredStructure::LowDelay
        && pic.picture_number - ctx.last_long_base_pic >= LONG_BASE_PIC
        && tl == 0
    {
        pic.rps.refresh_frame_mask |= 1u8 << LONG_BASE_IDX;
        ctx.last_long_base_pic = pic.picture_number;
    }

    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );

    if !set_frame_display_params(pic, ctx, mg_idx) {
        if tl < hier {
            pic.show_frame = false;
            pic.has_show_existing = false;
        } else {
            pic.show_frame = true;
            pic.has_show_existing = true;
            let slot = show_existing_slot(hier, pic_idx).ok_or(RpsError::MiniGopIndex {
                hierarchical_levels: hier,
                temporal_layer: tl,
                pic_idx,
            })?;
            pic.show_existing_frame = idx.resolve(slot);
        }
    }

    Ok(())
}

/// The reference-list index order C assigns in, kept next to the table so the
/// two can be read together.
///
/// `slot_table`'s rows are `[LAST, LAST2, LAST3, GOLD, BWD, ALT2, ALT]`; this
/// asserts that is the constants' own numeric order, which is what makes a
/// positional array literal equivalent to C's seven named assignments.
const _: () = {
    assert!(LAST == 0 && LAST2 == 1 && LAST3 == 2 && GOLD == 3);
    assert!(BWD == 4 && ALT2 == 5 && ALT == 6);
};

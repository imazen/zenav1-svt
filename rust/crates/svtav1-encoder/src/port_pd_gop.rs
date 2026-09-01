//! The remaining translatable picture-decision surface of
//! `Codec/pd_process.c`: the GF-group grouping RC consumes, the frame-skip
//! predicate, and the whole-picture MRP distortion detector.
//!
//! | Rust | C (`Codec/pd_process.c`) |
//! |---|---|
//! | [`is_pic_skipped`] | `svt_aom_is_pic_skipped` (996) — **EXPORTED** |
//! | [`mrp_detector_hme_level0`] | `mrp_detector_hme_level0` (492) — EXPORTED |
//! | [`store_gf_group`] | `store_gf_group` (4378) — EXPORTED |
//! | [`pred_struct_for_picture`] | the per-picture half of `get_pred_struct_for_all_frames` (942) — static |
//!
//! # What is COUNTED OUT of this file's queue, and why
//!
//! These are pipeline plumbing that this port replaces by design rather than
//! translates, named individually so the omission is a decision and not a gap:
//!
//! * `update_rc_param_queue` (4547) — a mutex-guarded circular pool of shared
//!   `RateControlParam` objects; the port has no such pool.
//! * `check_window_availability` (4637) — walks the picture-decision reorder
//!   queue's ring buffer and publishes raw `PictureParentControlSet*` window
//!   pointers.
//! * `print_pre_ass_buffer` (4421) — behind `#if LAD_MG_PRINT`, `SVT_LOG` only.
//! * `svt_aom_picture_decision_kernel` / `_iter` / `_ctor` / `_dctor`,
//!   `process_pics`, `send_picture_out`,
//!   `release_prev_picture_from_reorder_queue`, `assign_and_release_pa_refs`,
//!   `search_ref_in_ref_queue_pa`, `low_delay_store_tf_pictures`,
//!   `process_first_pass`, `initialize_overlay_frame`,
//!   `perform_simple_picture_analysis_for_overlay` — the thread kernel and its
//!   object-pool / live-count bookkeeping.
//!
//! # Evidence
//!
//! Tier 1 for [`is_pic_skipped`] via
//! `crates/svtav1-cref/shims/refmgmt_shims.c`. Tier 4 for the rest — both
//! `mrp_detector_hme_level0` and `store_gf_group` are exported but take
//! whole `PictureParentControlSet` graphs (a picture-pointer array and a
//! downsampled reference buffer), so a shim would have to build the object
//! graph the port deliberately does not have.

use alloc::vec::Vec;

use crate::port_picstruct::{DsPlane, FULL_SAD_SEARCH, PredStructure, SliceType, early_hme_b64};

/// C `svt_aom_is_pic_skipped` (`pd_process.c:996`) — **EXPORTED**.
///
/// A non-reference picture in the statistics-generation pass that is not the
/// first of its mini-GOP contributes nothing, so it is dropped before encode.
/// All three conditions must hold; in particular a REFERENCE picture is never
/// skipped no matter what the pass is.
#[must_use]
pub fn is_pic_skipped(
    is_ref: bool,
    rc_stat_gen_pass_mode: bool,
    first_frame_in_minigop: bool,
) -> bool {
    !is_ref && rc_stat_gen_pass_mode && !first_frame_in_minigop
}

/// C `mrp_detector_hme_level0` (`pd_process.c:492`) — EXPORTED.
///
/// Total 16x-downsampled HME distortion of one reference against the source,
/// summed over every 64x64 base block. The multi-reference pruner divides two
/// of these to decide whether a candidate reference is similar enough to
/// another to be dropped, so it is a whole-picture scalar, not a per-block
/// one.
///
/// Coordinate trap, transcribed: the block origins are in FULL-resolution
/// pixels and every use of them is shifted right by 2 — `>> 2`, not `>> 4`,
/// even though the plane is a SIXTEENTH downsample. That is because the
/// sixteenth plane is quarter-resolution in each dimension, so a 64x64 luma
/// block is 16x16 there and `x / 4` is its column.
///
/// `b64_size` is C's `scs->b64_size` for the grid count, while the origin
/// stride is a hardcoded 64 — the two are the same in every shipping
/// configuration, and C's asymmetry is kept rather than unified.
#[must_use]
pub fn mrp_detector_hme_level0(
    src_sixteenth: &DsPlane<'_>,
    ref_sixteenth: &DsPlane<'_>,
    aligned_width: u32,
    aligned_height: u32,
    b64_size: u32,
) -> u64 {
    let pic_width_in_b64 = aligned_width.div_ceil(b64_size);
    let pic_height_in_b64 = aligned_height.div_ceil(b64_size);
    let mut tot_dist = 0u64;

    for y_b64_idx in 0..pic_height_in_b64 {
        for x_b64_idx in 0..pic_width_in_b64 {
            let b64_origin_x = x_b64_idx * 64;
            let b64_origin_y = y_b64_idx * 64;
            let buffer_index = ((b64_origin_y >> 2) as usize) * src_sixteenth.stride
                + ((b64_origin_x >> 2) as usize);
            let src = &src_sixteenth.data[src_sixteenth.origin + buffer_index..];
            let (sad, _mv) = early_hme_b64(
                src,
                src_sixteenth.stride,
                FULL_SAD_SEARCH,
                (b64_origin_x as i16) >> 2,
                (b64_origin_y as i16) >> 2,
                16,
                16,
                8,
                8,
                ref_sixteenth,
            );
            tot_dist += sad;
        }
    }
    tot_dist
}

/// The per-picture inputs [`store_gf_group`] classifies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GfPic {
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `pcs->slice_type`.
    pub slice_type: SliceType,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `svt_aom_is_delayed_intra(pcs)`.
    pub is_delayed_intra: bool,
    /// C `svt_aom_is_incomp_mg_frame(pcs)`.
    pub is_incomp_mg_frame: bool,
    /// C `pcs->idr_flag`.
    pub idr_flag: bool,
    /// C `pcs->end_of_sequence_flag`.
    pub end_of_sequence_flag: bool,
}

/// What [`store_gf_group`] writes onto one picture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GfAssignment {
    /// C `pcs->gf_interval`.
    pub gf_interval: i32,
    /// C `pcs->gf_update_due`.
    pub gf_update_due: bool,
    /// C `pcs->gf_group[]`, as indices into the mini-GOP array with
    /// [`GfGroupMember::Centre`] standing for the centre picture itself.
    pub gf_group: Vec<GfGroupMember>,
}

/// One entry of a picture's `gf_group[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GfGroupMember {
    /// The picture the group is being built FOR — C stores its own pointer.
    Centre,
    /// `ctx->mg_pictures_array[i]`.
    MiniGop(usize),
}

/// C `store_gf_group` (`pd_process.c:4378`) — EXPORTED.
///
/// Builds the golden-frame group the rate control walks: which pictures belong
/// to it, how long it is, and which of them owe a GF update. It runs only for
/// a picture that STARTS a group — an I slice, a non-delayed base-layer
/// picture, or an incomplete mini-GOP frame — and does nothing otherwise,
/// which is why it returns `None` there rather than an empty assignment.
///
/// Three shapes, and the third is the one that is easy to get wrong:
///
/// 1. a DELAYED intra puts ITSELF first and the whole mini-GOP after it, so
///    `gf_interval` is `1 + mg_size`;
/// 2. otherwise the group is the mini-GOP alone — except that an incomplete
///    mini-GOP whose LAST picture is an IDR drops that picture, because the
///    IDR starts the next group;
/// 3. an I slice at the end of the sequence collapses to just itself.
///
/// The per-member pass then re-runs the same start-of-group predicate on every
/// member — but note it tests `!pcs->is_delayed_intra` (the CENTRE picture's
/// flag) against `gf_group[i]->temporal_layer_index`, mixing the two pictures.
/// That is C's, and it is transcribed rather than tidied.
///
/// The tail rewrites a LATER incomplete-mini-GOP member's own group to the
/// mini-GOP minus its first entry — this is the "P pictures after an I" case
/// C's comment names, and it is what lets RC see a group for a picture that
/// was itself not a group start.
#[must_use]
pub fn store_gf_group(centre: &GfPic, mg: &[GfPic]) -> Option<Vec<(usize, GfAssignment)>> {
    let starts_group = |p: &GfPic, delayed_intra_of_centre: bool| {
        p.slice_type == SliceType::I
            || (!delayed_intra_of_centre && p.temporal_layer_index == 0)
            || p.is_incomp_mg_frame
    };
    if !starts_group(centre, centre.is_delayed_intra) {
        return None;
    }

    let mut group: Vec<GfGroupMember> = Vec::new();
    if centre.is_delayed_intra {
        group.push(GfGroupMember::Centre);
        group.extend((0..mg.len()).map(GfGroupMember::MiniGop));
    } else {
        let mut mg_size = mg.len();
        if centre.is_incomp_mg_frame && mg_size > 0 && mg[mg_size - 1].idr_flag {
            mg_size -= 1;
        }
        group.extend((0..mg_size).map(GfGroupMember::MiniGop));
    }
    if centre.slice_type == SliceType::I && centre.end_of_sequence_flag {
        group.clear();
        group.push(GfGroupMember::Centre);
    }

    let gf_interval = i32::try_from(group.len()).unwrap_or(i32::MAX);
    let member = |m: GfGroupMember| match m {
        GfGroupMember::Centre => centre,
        GfGroupMember::MiniGop(i) => &mg[i],
    };

    let mut out: Vec<(usize, GfAssignment)> = Vec::new();
    for (pos, &m) in group.iter().enumerate() {
        let p = member(m);
        let mut assign = GfAssignment {
            gf_interval: 0,
            // Note the mixed subject: C's predicate here reads the CENTRE
            // picture's `is_delayed_intra` and the MEMBER's temporal layer.
            gf_update_due: starts_group(p, centre.is_delayed_intra),
            gf_group: Vec::new(),
        };
        // The "P pictures that come after an I" rewrite.
        if centre.slice_type == SliceType::I
            && p.is_incomp_mg_frame
            && centre.picture_number < p.picture_number
        {
            assign.gf_interval = gf_interval - 1;
            assign.gf_group = (1..=(gf_interval as usize - 1).min(mg.len().saturating_sub(1)))
                .map(GfGroupMember::MiniGop)
                .collect();
            assign.gf_update_due = false;
        }
        out.push((pos, assign));
    }

    // The centre picture's own assignment is the group itself.
    if let Some(slot) = out
        .iter_mut()
        .find(|(pos, _)| matches!(group[*pos], GfGroupMember::Centre))
    {
        slot.1.gf_interval = gf_interval;
        slot.1.gf_group.clone_from(&group);
    }
    Some(out)
}

/// The per-picture half of C `get_pred_struct_for_all_frames`
/// (`pd_process.c:942`) — static.
///
/// Returns `(pred_structure, hierarchical_levels)` for one picture of one
/// mini-GOP. The buffer walk around it is plumbing; THIS is the decision, and
/// the trap is that an IDR takes the SEQUENCE's configured depth while every
/// other picture takes the depth this mini-GOP was assigned — a key frame
/// restarts the pyramid at full depth even inside a shortened GOP.
#[must_use]
pub fn pred_struct_for_picture(
    seq_pred_structure: PredStructure,
    seq_hierarchical_levels: u8,
    mini_gop_hierarchical_levels: u8,
    idr_flag: bool,
) -> (PredStructure, u8) {
    let hier = if idr_flag {
        seq_hierarchical_levels
    } else {
        mini_gop_hierarchical_levels
    };
    (seq_pred_structure, hier)
}

/// The startup-mini-GOP and startup-GOP latches
/// `get_pred_struct_for_all_frames` maintains (`pd_process.c:975-988`).
///
/// `enable_startup_mg` is armed by any IDR or CRA and cleared by the very next
/// picture; `is_startup_gop` is set only by the IDR at picture 0 and cleared
/// by any later IDR or CRA, so it marks the FIRST GOP of the stream and no
/// other. Both are carried per picture, so this returns the new state
/// alongside the value the picture records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartupState {
    /// C `ctx->enable_startup_mg`.
    pub enable_startup_mg: bool,
    /// C `ctx->is_startup_gop`.
    pub is_startup_gop: bool,
}

/// Advance [`StartupState`] for one picture. `startup_mg_size == 0` disables
/// the first latch entirely, exactly as C's guard does.
#[must_use]
pub fn advance_startup_state(
    state: StartupState,
    startup_mg_size: u8,
    idr_flag: bool,
    cra_flag: bool,
    picture_number: u64,
) -> StartupState {
    let mut next = state;
    if startup_mg_size != 0 {
        if idr_flag || cra_flag {
            next.enable_startup_mg = true;
        } else if state.enable_startup_mg {
            next.enable_startup_mg = false;
        }
    }
    if idr_flag && picture_number == 0 {
        next.is_startup_gop = true;
    } else if idr_flag || cra_flag {
        next.is_startup_gop = false;
    }
    next
}

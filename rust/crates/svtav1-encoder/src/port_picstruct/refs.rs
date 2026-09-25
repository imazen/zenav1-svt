use super::*;

/// C `svt_aom_is_pic_used_as_ref` (`pd_process.c:1770-1803`) — EXPORTED.
///
/// Whether a picture enters the DPB at all. `referencing_scheme` 0 forbids
/// top-layer refs, 1 allows all, 2 allows some by position. Sub-top-layer
/// pictures are always refs; overlays never are.
///
/// C's `default:` arm asserts and then falls through to `return true`; with
/// `NDEBUG` (the Release build the oracle uses) that is a plain `true`, which
/// is what this reproduces.
#[must_use]
pub fn is_pic_used_as_ref(
    hierarchical_levels: u32,
    temporal_layer: u32,
    picture_index: u32,
    referencing_scheme: u32,
    is_overlay: bool,
) -> bool {
    if is_overlay {
        return false;
    }
    // Frames below top layer are always used as ref
    if temporal_layer < hierarchical_levels {
        return true;
    }
    match hierarchical_levels {
        0 => true,
        1 => referencing_scheme != 0,
        // hierarchical_levels 2 and 3 share an arm in C.
        2 | 3 => {
            if referencing_scheme == 0 {
                false
            } else if referencing_scheme == 1 {
                true
            } else {
                picture_index == 0
            }
        }
        4 => {
            if referencing_scheme == 0 {
                false
            } else if referencing_scheme == 1 {
                true
            } else {
                picture_index == 0 || picture_index == 8
            }
        }
        5 => false,
        _ => true,
    }
}

/// C `svt_aom_is_incomp_mg_frame` (`pd_process.c:4986-4989`) — EXPORTED.
///
/// True for a picture whose own prediction structure is low delay inside a
/// random-access sequence: the incomplete mini-GOP at a GOP boundary. Read by
/// [`set_ref_list_counts`] (forces list 1 empty) and by the `reference_mode`
/// derivation (forces `SINGLE_REFERENCE`).
/// C `pcs->is_highest_layer` as `picture_decision_kernel` assigns it
/// (`pd_process.c:5559-5561`):
///
/// ```c
/// // For flat, set is_highest_layer to false to avoid using aggressive settings for all pictures
/// pcs->is_highest_layer = (pcs->temporal_layer_index == pcs->hierarchical_levels) &&
///     pcs->hierarchical_levels != 0;
/// ```
///
/// The second clause is load-bearing and has been dropped by more than one
/// re-derivation in this port: on a FLAT GOP (`hierarchical_levels == 0`)
/// every picture is temporal layer 0 == `hierarchical_levels`, and C still
/// says **false**, so `is_not_last_layer` (`!is_highest_layer`, the form the
/// `enc_mode_config.c` ladders read) is TRUE for every picture of the inter
/// campaign's low-delay grid. A `temporal_layer_index != hierarchical_levels`
/// paraphrase gets the flat GOP backwards.
///
/// The overlay arm (`initialize_overlay_frame`, `pd_process.c:3581`, forces
/// `true`) is not modelled: overlays are `enable_overlays`-gated and this port
/// never sets it.
///
/// Readers: `set_md_stage_counts`'s NIC picture type
/// (`product_coding_loop.c:1398`, `I_SLICE ? 0 : !is_highest_layer ? 1 : 2`),
/// the DLF / LR / MD-config `is_not_last_layer` ladders.
///
/// Evidence: tier 4 on the assignment itself (inline in a `static` kernel,
/// no exported symbol); the consumers that read it through
/// `svt_aom_sig_deriv_mode_decision_config_default` / `svt_aom_set_nics`
/// are tier-1 gated on their own inputs.
#[must_use]
#[inline]
pub fn is_highest_layer(temporal_layer_index: u8, hierarchical_levels: u8) -> bool {
    temporal_layer_index == hierarchical_levels && hierarchical_levels != 0
}

#[must_use]
pub fn is_incomp_mg_frame(pic: &PicParams, seq: &SeqPicParams) -> bool {
    pic.pred_struct_type == PredStructure::LowDelay
        && seq.pred_structure == PredStructure::RandomAccess
}

/// C `frame_is_leaf` (`enc_mode_config.h:111-116`): `update_type ==
/// LF_UPDATE`. NOT a paraphrase of `is_highest_layer` — on a flat GOP C
/// forces `is_highest_layer` false for every picture (`pd_process.c:5560`)
/// while every non-key frame is still `LF_UPDATE`, so `!frame_is_leaf` and
/// `!is_highest_layer` DISAGREE there. `set_cand_reduction_ctrls`'s regular
/// arm reads this one (`enc_mode_config.c:4100`); `:8912` reads the other.
#[must_use]
pub fn frame_is_leaf(update_type: FrameUpdateType) -> bool {
    update_type == FrameUpdateType::Lf
}

/// C `frame_is_kf_gf_arf` / `frame_is_boosted` (`enc_mode_config.h:100-110`).
#[must_use]
pub fn frame_is_boosted(pic: &PicParams) -> bool {
    pic.is_intra_only
        || pic.update_type == FrameUpdateType::Arf
        || pic.update_type == FrameUpdateType::Gf
}

/// C `update_count_try` (`pd_process.c:4507-4517`) — EXPORTED.
///
/// Caps the *try* counts (what MD enumerates) against the MRP per-layer
/// limits. These, not `ref_list{0,1}_count`, decide how many references the
/// candidate loop walks.
pub fn update_count_try(pic: &mut PicParams, seq: &SeqPicParams) {
    let mrp = &seq.mrp_ctrls;
    if frame_is_boosted(pic) {
        pic.ref_list0_count_try = pic.ref_list0_count.min(mrp.base_ref_list0_count);
        pic.ref_list1_count_try = pic.ref_list1_count.min(mrp.base_ref_list1_count);
    } else {
        pic.ref_list0_count_try = pic.ref_list0_count.min(mrp.non_base_ref_list0_count);
        pic.ref_list1_count_try = pic.ref_list1_count.min(mrp.non_base_ref_list1_count);
    }
}

/// C `svt_aom_get_gm_needed_resolutions` (`pd_process.c:990-994`) — EXPORTED.
///
/// Maps the global-motion downsample level to which pyramid levels must exist.
/// Returns `(full, quarter, sixteenth)`. C's `GM_FULL`/`GM_DOWN`/`GM_DOWN16`/
/// `GM_ADAPT_0`/`GM_ADAPT_1` are 0/1/2/3/4.
#[must_use]
pub fn get_gm_needed_resolutions(ds_lvl: u8) -> (bool, bool, bool) {
    const GM_FULL: u8 = 0;
    const GM_DOWN: u8 = 1;
    const GM_DOWN16: u8 = 2;
    const GM_ADAPT_0: u8 = 3;
    const GM_ADAPT_1: u8 = 4;
    let need_full = ds_lvl == GM_FULL || ds_lvl == GM_ADAPT_0;
    let need_quart = ds_lvl == GM_DOWN || ds_lvl == GM_ADAPT_0 || ds_lvl == GM_ADAPT_1;
    let need_sixteen = ds_lvl == GM_DOWN16 || ds_lvl == GM_ADAPT_1;
    (need_full, need_quart, need_sixteen)
}

/// C `svt_av1_setup_skip_mode_allowed` (`pd_process.c:102-166`) — EXPORTED.
///
/// Derives `skip_mode_allowed` and the two skip-mode reference indices from
/// `ref_order_hint[]` against `cur_order_hint`. `skip_mode_present` is a frame
/// header bit, so a wrong answer here flips a bit on every inter frame.
///
/// Index trap: C's `ref_frame_idx_{0,1}` are `LAST_FRAME + min/max(i, j)`
/// where `i`/`j` index `ref_order_hint[0..7]` — i.e. they are
/// `MvReferenceFrame` values, not list positions.
pub fn setup_skip_mode_allowed(pic: &mut PicParams, seq: &SeqPicParams) {
    let ohi = seq.order_hint_info;
    pic.skip_mode.skip_mode_allowed = 0;
    pic.skip_mode.ref_frame_idx_0 = INVALID_IDX;
    pic.skip_mode.ref_frame_idx_1 = INVALID_IDX;

    if !ohi.enable_order_hint
        || pic.slice_type == SliceType::I
        || pic.reference_mode == ReferenceMode::Single
    {
        return;
    }

    let cur_order_hint = pic.cur_order_hint as i32;
    // C seeds the forward slot with -1 and the backward slot with INT_MAX.
    let mut ref_order_hints: [i32; 2] = [-1, i32::MAX];
    let mut ref_idx: [i32; 2] = [INVALID_IDX, INVALID_IDX];

    for i in 0..INTER_REFS_PER_FRAME {
        let ref_hint = pic.ref_order_hint[i] as i32;
        let d = get_relative_dist(ohi, ref_hint, cur_order_hint);
        if d < 0 {
            if ref_order_hints[0] == -1 || get_relative_dist(ohi, ref_hint, ref_order_hints[0]) > 0
            {
                ref_order_hints[0] = ref_hint;
                ref_idx[0] = i as i32;
            }
        } else if d > 0
            && (ref_order_hints[1] == i32::MAX
                || get_relative_dist(ohi, ref_hint, ref_order_hints[1]) < 0)
        {
            ref_order_hints[1] = ref_hint;
            ref_idx[1] = i as i32;
        }
    }

    if ref_idx[0] != INVALID_IDX && ref_idx[1] != INVALID_IDX {
        // Bi-directional prediction.
        pic.skip_mode.skip_mode_allowed = 1;
        pic.skip_mode.ref_frame_idx_0 = i32::from(LAST_FRAME) + ref_idx[0].min(ref_idx[1]);
        pic.skip_mode.ref_frame_idx_1 = i32::from(LAST_FRAME) + ref_idx[0].max(ref_idx[1]);
    } else if ref_idx[0] != INVALID_IDX && ref_idx[1] == INVALID_IDX {
        // Forward prediction only: find the second-nearest forward reference.
        ref_order_hints[1] = -1;
        for i in 0..INTER_REFS_PER_FRAME {
            let ref_hint = pic.ref_order_hint[i] as i32;
            if (ref_order_hints[0] != -1
                && get_relative_dist(ohi, ref_hint, ref_order_hints[0]) < 0)
                && (ref_order_hints[1] == -1
                    || get_relative_dist(ohi, ref_hint, ref_order_hints[1]) > 0)
            {
                ref_order_hints[1] = ref_hint;
                ref_idx[1] = i as i32;
            }
        }
        if ref_order_hints[1] != -1 {
            pic.skip_mode.skip_mode_allowed = 1;
            pic.skip_mode.ref_frame_idx_0 = i32::from(LAST_FRAME) + ref_idx[0].min(ref_idx[1]);
            pic.skip_mode.ref_frame_idx_1 = i32::from(LAST_FRAME) + ref_idx[0].max(ref_idx[1]);
        }
    }
}

// ---------------------------------------------------------------------------
// Static C helpers (tier 4: hand-derived vectors traced against the C source)
// ---------------------------------------------------------------------------

/// C `prune_refs` (`pd_process.c:1100-1131`) — static.
///
/// Collapses unused list slots onto LAST (list 0) and BWD (list 1) in BOTH the
/// slot array and the POC array. Without it the header's unused `ref_frame_idx`
/// entries point at stale DPB slots and the frame header diverges even when the
/// used references are right.
///
/// Order matters: `ALT` is folded onto `BWD` *before* `ALT2` is, and `BWD` may
/// itself already have been folded onto `LAST`, so the C sequence is
/// transcribed literally rather than "simplified".
pub fn prune_refs(rps: &mut Av1RpsNode, ref_list0_count: u32, ref_list1_count: u32) {
    if ref_list0_count < 4 {
        rps.ref_dpb_index[GOLD] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[GOLD] = rps.ref_poc_array[LAST];
    }
    if ref_list0_count < 3 {
        rps.ref_dpb_index[LAST3] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[LAST3] = rps.ref_poc_array[LAST];
    }
    if ref_list0_count < 2 {
        rps.ref_dpb_index[LAST2] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[LAST2] = rps.ref_poc_array[LAST];
    }
    if ref_list1_count < 1 {
        rps.ref_dpb_index[BWD] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[BWD] = rps.ref_poc_array[LAST];
    }
    if ref_list1_count < 3 {
        rps.ref_dpb_index[ALT] = rps.ref_dpb_index[BWD];
        rps.ref_poc_array[ALT] = rps.ref_poc_array[BWD];
    }
    if ref_list1_count < 2 {
        rps.ref_dpb_index[ALT2] = rps.ref_dpb_index[BWD];
        rps.ref_poc_array[ALT2] = rps.ref_poc_array[BWD];
    }
}

/// C `update_ref_poc_array` (`pd_process.c:1901-1910`) — static.
///
/// Reads the seven reference POCs out of the shadow DPB. Source of
/// `pcs->ref_order_hint[]` (a written frame-header field) and of every MVP
/// temporal-distance scale.
pub fn update_ref_poc_array(rps: &mut Av1RpsNode, dpb: &[DpbEntry; REF_FRAMES]) {
    for i in 0..INTER_REFS_PER_FRAME {
        rps.ref_poc_array[i] = dpb[rps.ref_dpb_index[i] as usize].picture_number;
    }
}

/// C `update_dpb` (`pd_process.c:5179-5191`) — static.
///
/// Applies `refresh_frame_mask` to the shadow DPB. Skipping it makes every
/// frame after the first inter frame read wrong reference POCs.
pub fn update_dpb(pic: &PicParams, ctx: &mut PicDecisionCtx) {
    if pic.rps.refresh_frame_mask != 0 {
        for i in 0..REF_FRAMES {
            if (pic.rps.refresh_frame_mask >> i) & 1 == 1 {
                ctx.dpb[i].picture_number = pic.picture_number;
                ctx.dpb[i].decode_order = pic.decode_order;
                ctx.dpb[i].temporal_layer_index = pic.temporal_layer_index;
            }
        }
    }
}

/// C `set_key_frame_rps` (`pd_process.c:1480-1490`) — static.
///
/// Resets the toggles and the managed-ref slot state at each key frame.
/// Without it the toggle sequence that picks DPB slots desynchronises from C
/// after the first GOP.
///
/// The C body also calls `ref_mgmt_reset_state`, which clears
/// `ctx->pic_id_per_dpb_slot`: a key frame refreshes all eight DPB slots, so
/// every long-term anchor the application was holding is destroyed and its id
/// must stop resolving.
pub fn set_key_frame_rps(pic: &mut PicParams, ctx: &mut PicDecisionCtx) {
    ctx.lay0_toggle = 0;
    ctx.lay1_toggle = 0;
    crate::port_ref_mgmt::reset_state(ctx);
    pic.show_frame = true;
    pic.has_show_existing = false;
}

/// C `set_ref_list_counts` (`pd_process.c:1804-1900`) — static.
///
/// Derives `ref_list0_count` / `ref_list1_count` by de-duplicating
/// `ref_poc_array` and capping against [`MrpCtrls`]. Feeds [`prune_refs`],
/// [`update_count_try`] and [`set_all_ref_frame_type`], so the number of
/// signalled references and the whole MD candidate set are wrong without it.
///
/// Two index traps transcribed literally:
/// * the list-1 inner loop starts at `LAST2` when `i == BWD` and at `LAST`
///   otherwise — BWD and LAST are *allowed* to match (they do in base layer);
/// * the `j <= GOLD && j + 1 > ref_list0_count` guard skips list-0 entries
///   that this frame will not actually signal.
pub fn set_ref_list_counts(pic: &mut PicParams, seq: &SeqPicParams, ctx: &PicDecisionCtx) {
    if pic.slice_type == SliceType::I {
        pic.ref_list0_count = 0;
        pic.ref_list1_count = 0;
        return;
    }

    let mrp = &seq.mrp_ctrls;
    let is_base = frame_is_boosted(pic);
    let poc = &pic.rps.ref_poc_array;

    // list 0
    let mut list0_count: u8 = 1;
    let mut breakout = false;
    for i in LAST2..=GOLD {
        if breakout {
            break;
        }
        for j in LAST..i {
            if poc[i] == poc[j] {
                breakout = true;
                break;
            }
        }
        if !breakout {
            list0_count += 1;
        }
    }
    pic.ref_list0_count = list0_count.min(if is_base {
        mrp.base_ref_list0_count
    } else {
        mrp.non_base_ref_list0_count
    });

    if is_incomp_mg_frame(pic, seq) || pic.is_overlay {
        pic.ref_list1_count = 0;
        return;
    }

    // list 1
    let mut list1_count: u8 = 0;
    breakout = false;
    for i in BWD..=ALT {
        if breakout {
            break;
        }
        let jstart = if i == BWD { LAST2 } else { LAST };
        for j in jstart..i {
            if j <= GOLD && (j as u8) + 1 > pic.ref_list0_count {
                continue;
            }
            // S-frame mini-GOP: list 0 gets pruned in set_all_ref_frame_type,
            // so the duplicate check must not also empty list 1.
            if seq.pred_structure == PredStructure::RandomAccess
                && pic.picture_number < ctx.sframe_poc
                && j <= GOLD
                && poc[j] == ctx.sframe_poc
            {
                continue;
            }
            if poc[i] == poc[j] {
                breakout = true;
                break;
            }
        }
        if !breakout {
            list1_count += 1;
        }
    }
    pic.ref_list1_count = list1_count.min(if is_base {
        mrp.base_ref_list1_count
    } else {
        mrp.non_base_ref_list1_count
    });
}

/// C `svt_get_ref_frame_type` (`mode_decision.c:265`) via `to_ref_frame`.
///
/// List 0 is {LAST, LAST2, LAST3, GOLDEN}, list 1 is {BWDREF, ALTREF2, ALTREF}.
#[must_use]
pub fn get_ref_frame_type(list: u8, ref_idx: u8) -> i8 {
    const TO_REF_FRAME: [[i8; 4]; 2] = [
        [LAST_FRAME, LAST2_FRAME, LAST3_FRAME, GOLDEN_FRAME],
        [BWDREF_FRAME, ALTREF2_FRAME, ALTREF_FRAME, 0],
    ];
    TO_REF_FRAME[list as usize][ref_idx as usize]
}

/// C `set_all_ref_frame_type` (`pd_process.c:1044-1099`) — static.
///
/// Builds the exact ordered set of single and compound reference candidates MD
/// walks: single list-0, single list-1, every list0 x list1 bi-directional
/// compound, then (B slices only) the uni-directional compounds. A different
/// set is a different RD winner on essentially every block.
///
/// The S-frame pruning tail (`prune_sframe_refs`) is NOT applied here — it is
/// gated on `ctx->sframe_poc > 0 && scs->mfmv_enabled`, and S-frames are
/// outside this port's envelope. [`set_all_ref_frame_type`] therefore matches C
/// exactly whenever no S-frame is pending, which is every configuration the
/// port encodes today.
pub fn set_all_ref_frame_type(pic: &PicParams) -> ([i8; MODE_CTX_REF_FRAMES], u8) {
    let mut arr = [0i8; MODE_CTX_REF_FRAMES];
    let mut tot: usize = 0;

    // single ref - list 0
    for ref_idx0 in 0..pic.ref_list0_count_try {
        arr[tot] = get_ref_frame_type(0, ref_idx0);
        tot += 1;
    }
    // single ref - list 1
    for ref_idx1 in 0..pic.ref_list1_count_try {
        arr[tot] = get_ref_frame_type(1, ref_idx1);
        tot += 1;
    }
    // compound bi-dir
    for ref_idx0 in 0..pic.ref_list0_count_try {
        for ref_idx1 in 0..pic.ref_list1_count_try {
            let rf = [
                get_ref_frame_type(0, ref_idx0),
                get_ref_frame_type(1, ref_idx1),
            ];
            arr[tot] = av1_ref_frame_type(rf);
            tot += 1;
        }
    }
    if pic.slice_type == SliceType::B {
        // compound uni-dir
        if pic.ref_list0_count_try > 1 {
            arr[tot] = av1_ref_frame_type([LAST_FRAME, LAST2_FRAME]);
            tot += 1;
            if pic.ref_list0_count_try > 2 {
                arr[tot] = av1_ref_frame_type([LAST_FRAME, LAST3_FRAME]);
                tot += 1;
                if pic.ref_list0_count_try > 3 {
                    arr[tot] = av1_ref_frame_type([LAST_FRAME, GOLDEN_FRAME]);
                    tot += 1;
                }
            }
        }
        if pic.ref_list1_count_try > 2 {
            arr[tot] = av1_ref_frame_type([BWDREF_FRAME, ALTREF_FRAME]);
            tot += 1;
        }
    }
    (arr, tot as u8)
}

/// C `set_frame_display_params` (`pd_process.c:1132-1161`) — static.
///
/// Returns false exactly where C does (a B frame of a complete random-access
/// mini-GOP), which tells the caller to derive show/show-existing from the
/// picture index instead.
pub fn set_frame_display_params(
    pic: &mut PicParams,
    ctx: &PicDecisionCtx,
    mini_gop_index: usize,
) -> bool {
    if pic.pred_struct_type == PredStructure::LowDelay || pic.is_overlay {
        pic.show_frame = true;
        pic.has_show_existing = false;
    } else if pic.slice_type == SliceType::I {
        // Key frames are handled before this; the remaining I cases are a
        // mini-GOP broken by a scene change / intra refresh (shown) vs a
        // complete one (hidden, emitted later as show_existing).
        if ctx.mini_gop_length[mini_gop_index] < pic.pred_struct_entry_count {
            pic.show_frame = true;
            pic.has_show_existing = false;
        } else {
            pic.show_frame = false;
            pic.has_show_existing = false;
        }
    } else {
        return false;
    }
    true
}

/// C `set_ref_frame_sign_bias` (`pd_process.c:4894-4909`) — static.
///
/// Fills `ref_frame_sign_bias[8]` via `get_relative_dist`. Consumed by the MVP
/// stack, compound-mode allowance and MFMV projection; a wrong sign bias
/// silently mis-signs every temporal MV candidate.
///
/// Index trap: `ref_frame_sign_bias` is indexed by `MvReferenceFrame`
/// (`LAST_FRAME`=1..`ALTREF_FRAME`=7) while `ref_order_hint` is indexed by
/// `ref_frame - 1`. Slot 0 (`INTRA_FRAME`) always stays 0.
pub fn set_ref_frame_sign_bias(pic: &mut PicParams, seq: &SeqPicParams) {
    pic.ref_frame_sign_bias = [0; REF_FRAMES];
    if seq.order_hint_info.enable_order_hint {
        for ref_frame in LAST_FRAME..=ALTREF_FRAME {
            let hint = pic.ref_order_hint[(ref_frame - 1) as usize] as i32;
            pic.ref_frame_sign_bias[ref_frame as usize] = i32::from(
                get_relative_dist(seq.order_hint_info, hint, pic.cur_order_hint as i32) > 0,
            );
        }
    }
}

/// C `set_layer_depth` (`pd_process.c:4576-4583`) — static.
pub fn set_layer_depth(pic: &mut PicParams) {
    pic.layer_depth = if pic.is_key_frame {
        0
    } else {
        pic.temporal_layer_index + 1
    };
}

/// C `set_frame_update_type` (`pd_process.c:4591-4611`) — static.
///
/// The video-mode qindex derivation (`svt_av1_frame_type_qdelta`) keys directly
/// off `update_type`, so a wrong value here is a wrong `base_q_idx` on every
/// frame.
///
/// Trap: the flat (`hierarchical_levels == 0`) arm uses
/// `frame_offset % MAX(4, 1 << hierarchical_levels)`, which is
/// `frame_offset % 4` — the `1 << 0 == 1` term never wins. Every 4th frame from
/// the last IDR is a `GF_UPDATE`, odd offsets are `LF_UPDATE`, and the
/// remaining even offsets are `INTNL_ARF_UPDATE`.
pub fn set_frame_update_type(pic: &mut PicParams) {
    pic.update_type = if pic.is_key_frame {
        FrameUpdateType::Kf
    } else if pic.hierarchical_levels > 0 {
        if pic.temporal_layer_index == 0 {
            FrameUpdateType::Arf
        } else if pic.temporal_layer_index == pic.hierarchical_levels {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    } else {
        let m = u64::from(4u32.max(1u32 << pic.hierarchical_levels));
        if pic.frame_offset.is_multiple_of(m) {
            FrameUpdateType::Gf
        } else if pic.frame_offset & 1 == 1 {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    };
}

/// C `set_gf_group_param` (`pd_process.c:4612-4615`) — static.
///
/// Trivial, but it fixes the ORDER: update type first, then layer depth. Both
/// feed the video qindex derivation.
pub fn set_gf_group_param(pic: &mut PicParams) {
    set_frame_update_type(pic);
    set_layer_depth(pic);
}

// ---------------------------------------------------------------------------
// av1_generate_rps_info (pd_process.c:1911-3506) — static, tier 4
// ---------------------------------------------------------------------------

/// A prediction-structure branch of `av1_generate_rps_info` this port has not
/// translated yet.
///
/// Returned instead of a guess, per `docs/WORKING-ON-THIS.md` §6: a
/// plausible-but-wrong reference structure is indistinguishable from a correct
/// one at the integration seam and would produce a decodable stream that
/// predicts from the wrong pictures. Refusing is the correct behaviour until
/// the arm is transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpsBranchUnsupported {
    /// `pcs->hierarchical_levels` of the refused picture.
    pub hierarchical_levels: u8,
    /// `pcs->temporal_layer_index` of the refused picture.
    pub temporal_layer: u8,
}

/// Why [`generate_rps_info`] declined to produce a reference structure.
///
/// Both variants are refusals in the sense of `docs/WORKING-ON-THIS.md` §6 —
/// the alternative is a stream that decodes while predicting from the wrong
/// pictures, which is indistinguishable from a correct one at the integration
/// seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpsError {
    /// A prediction-structure branch this port has not translated, or one C
    /// itself rejects (`Unsupported MG structure!`, `pd_process.c:3484`).
    UnsupportedBranch {
        /// `pcs->hierarchical_levels` of the refused picture.
        hierarchical_levels: u8,
        /// `pcs->temporal_layer_index` of the refused picture.
        temporal_layer: u8,
    },
    /// A `(temporal_layer, pic_idx)` pair the branch's table does not cover.
    ///
    /// C logs `Error in MG indexing - HL%d, temporal layer %d` here and then
    /// **falls through with the previous picture's `ref_dpb_index`**, so it
    /// emits an RPS built from stale slots. This port refuses instead.
    MiniGopIndex {
        /// `pcs->hierarchical_levels` of the refused picture.
        hierarchical_levels: u8,
        /// `pcs->temporal_layer_index` of the refused picture.
        temporal_layer: u8,
        /// `pic_idx` — the position inside the mini-GOP.
        pic_idx: u32,
    },
}

impl From<RpsBranchUnsupported> for RpsError {
    fn from(e: RpsBranchUnsupported) -> Self {
        Self::UnsupportedBranch {
            hierarchical_levels: e.hierarchical_levels,
            temporal_layer: e.temporal_layer,
        }
    }
}

impl core::fmt::Display for RpsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::UnsupportedBranch {
                hierarchical_levels,
                temporal_layer,
            } => write!(
                f,
                "unsupported RPS branch: hierarchical_levels {hierarchical_levels}, temporal layer {temporal_layer}"
            ),
            Self::MiniGopIndex {
                hierarchical_levels,
                temporal_layer,
                pic_idx,
            } => write!(
                f,
                "mini-GOP index {pic_idx} is not a coded position at hierarchical_levels {hierarchical_levels}, temporal layer {temporal_layer}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RpsError {}

/// C `av1_generate_rps_info` (`pd_process.c:1911-3506`) — static, tier 4.
///
/// THE reference-structure derivation: fills `ref_dpb_index[7]`,
/// `ref_poc_array[7]`, `refresh_frame_mask` and `is_ref`. Without it a port
/// has no DPB slot mapping and no refresh mask, so `ref_frame_idx[]` and
/// `refresh_frame_flags` in every inter frame header are invented.
///
/// **Coverage — all 9 of C's top-level branches are translated.**
/// `pd_process.c:1954-3483` splits on: RTC flat
/// (`rtc && hierarchical_levels == 0`), low-delay CQP/CRF, low-delay CBR
/// (flat plus hierarchical levels 1 and 2 — C's `else` after the `hier == 1`
/// arm carries only a debug `assert(hier == 2)` and its
/// `switch (temporal_layer)` serves every other level, flat included —
/// "LD CBR only supports flat/1L/2L"), flat RA
/// (`hierarchical_levels == 0`), and one branch per RA hierarchical level
/// 1..=5 — the last five delegated to [`crate::port_picstruct_ra`],
/// including the `pred_struct_ptr->pred_type != RANDOM_ACCESS` cut-short
/// arms inside them. A `LOW_DELAY` sequence under VBR falls through C's
/// rate-control tests into the hierarchical branches exactly as it does
/// here. The residual error surface is therefore the same shapes C only
/// logs an error for: LD-CBR temporal layers outside 0..=2 and mini-GOP
/// positions outside the ported tables.
///
/// The S-frame paths (`set_sframe_type`, `set_sframe_rps`,
/// `decide_sframe_mg`) and the app-driven reference-management events
/// (`apply_ref_mgmt_events`, which can mask `refresh_frame_mask` bits held
/// by a STORE) are translated too — threaded through
/// [`generate_rps_info_sframe`]'s optional hooks so the no-S-frame,
/// no-queued-event configuration this port encodes reduces to C's no-ops.
///
/// # Errors
///
/// Returns [`RpsError`] for a shape C itself logs as an error (LD-CBR
/// outside hierarchical levels 1-2, or a mini-GOP position outside the
/// ported tables).
pub fn generate_rps_info(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsError> {
    generate_rps_info_sframe(pic, seq, ctx, pic_idx, mg_idx, None)
}

/// The S-frame hooks `av1_generate_rps_info` calls, bundled so they can be
/// threaded through as one optional argument.
///
/// C reaches them through `enc_ctx->sf_cfg` and `scs->static_config`, both of
/// which are always present; `None` here is the configuration where
/// `sframe_dist == 0` and no position list was given, which is what makes
/// every hook a no-op (`pd_process.c:2272`, `:2264`, `:3487`).
pub struct SFrameHooks<'cfg, 'ctx> {
    /// The application's S-frame configuration.
    pub cfg: &'cfg crate::port_sframe::SFrameConfig<'cfg>,
    /// C `enc_ctx` — [`crate::port_sframe::set_sframe_rps`] resets
    /// `elapsed_non_cra_count` on it.
    pub enc_ctx: &'ctx mut EncCtxPicParams,
}

use super::*;

/// [`generate_rps_info`] with C's three S-frame hooks wired in.
///
/// They are interleaved INSIDE the function rather than wrapped around it,
/// because the order is load-bearing: `set_sframe_rps` forces
/// `refresh_frame_mask` to `0xFF` and must run BEFORE
/// [`crate::port_ref_mgmt::apply_events`], whose phase 3 then masks the
/// long-term anchors back out of it.
///
/// # Errors
///
/// As [`generate_rps_info`].
pub fn generate_rps_info_sframe(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
    mut sframe: Option<SFrameHooks<'_, '_>>,
) -> Result<(), RpsError> {
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;

    pic.is_ref = if seq.allintra {
        false
    } else {
        is_pic_used_as_ref(
            u32::from(hier),
            u32::from(temporal_layer),
            pic_idx,
            u32::from(seq.mrp_ctrls.referencing_scheme),
            pic.is_overlay,
        )
    };

    // Set frame type
    if pic.slice_type == SliceType::I {
        pic.rps.refresh_frame_mask = 0xFF;
        if pic.is_key_frame {
            set_key_frame_rps(pic, ctx);
            set_ref_list_counts(pic, seq, ctx);
            // C `pd_process.c:2264-2266`: only the two flexible-insert modes
            // reshape the first mini-GOP after a key frame.
            if let Some(h) = sframe.as_ref()
                && h.cfg.mode.is_flexible_insert()
            {
                crate::port_sframe::decide_sframe_mg(pic, h.cfg, ctx);
            }
            // C's key-frame early return still runs the ref-management
            // dispatcher (`pd_process.c:1265-1268`): the key frame refreshes
            // all eight slots, and if the application STOREd it, that must be
            // recorded now.
            crate::port_ref_mgmt::apply_events(pic, seq, ctx);
            return Ok(());
        }
    } else if let Some(h) = sframe.as_ref() {
        // C `pd_process.c:2271-2275`: a non-I slice is a candidate for the
        // switch, but only when the application asked for S-frames at all.
        if h.cfg.dist > 0 || h.cfg.positions.positions.is_some() {
            crate::port_sframe::set_sframe_type(pic, h.cfg, ctx);
        }
    }

    if seq.rtc && hier == 0 {
        rps_rtc_flat(pic, seq, ctx, mg_idx);
    } else if seq.pred_structure == PredStructure::LowDelay
        && seq.rate_control_mode == RcMode::CqpOrCrf
    {
        rps_low_delay_cqp(pic, seq, ctx, pic_idx, mg_idx);
    } else if seq.pred_structure == PredStructure::LowDelay && seq.rate_control_mode == RcMode::Cbr
    {
        rps_low_delay_cbr(pic, seq, ctx, pic_idx, mg_idx)?;
    } else if hier == 0 {
        rps_random_access_flat(pic, seq, ctx, mg_idx);
    } else {
        crate::port_picstruct_ra::rps_random_access_hier(pic, seq, ctx, pic_idx, mg_idx)?;
    }

    // C's tail (`pd_process.c:3487-3502`): the S-frame RPS, then the
    // ref-management events, then the overlay reset. The order matters —
    // `set_sframe_rps` forces the refresh mask to 0xFF and phase 3 of the
    // dispatcher masks the held anchors back out of it. Phase 3 runs
    // unconditionally, so the dispatcher is NOT skippable even when the
    // application queued nothing; it is a no-op only while no slot is held.
    if pic.is_switch_frame
        && let Some(h) = sframe.as_mut()
    {
        crate::port_sframe::set_sframe_rps(pic, ctx, h.enc_ctx);
    }
    crate::port_ref_mgmt::apply_events(pic, seq, ctx);
    if pic.is_overlay {
        pic.rps.refresh_frame_mask = 0;
    }
    Ok(())
}

/// C `av1_generate_rps_info`'s `scs->static_config.rtc && hierarchical_levels == 0`
/// branch (`pd_process.c:1954-1986`).
///
/// Up to `flat_max_refs` consecutive previous frames as list-0 references;
/// list 1 mirrors LAST. The refresh mask deliberately also sets the bits of
/// the slots this configuration never uses (`0xf0` plus every bit at or above
/// `max_refs`) so old pictures are dropped and their buffers freed.
pub(super) fn rps_rtc_flat(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    mg_idx: usize,
) {
    let max_refs = seq.mrp_ctrls.flat_max_refs;
    let pic0_idx = ctx.lay0_toggle; // newest pic
    let pic1_idx = circ_dec(pic0_idx, 0, max_refs - 1);
    let pic2_idx = circ_dec(pic1_idx, 0, max_refs - 1);
    let pic3_idx = circ_dec(pic2_idx, 0, max_refs - 1);

    pic.rps.ref_dpb_index[LAST] = pic0_idx;
    pic.rps.ref_dpb_index[LAST2] = pic1_idx;
    pic.rps.ref_dpb_index[LAST3] = pic2_idx;
    pic.rps.ref_dpb_index[GOLD] = pic3_idx;
    pic.rps.ref_dpb_index[BWD] = pic.rps.ref_dpb_index[LAST];
    pic.rps.ref_dpb_index[ALT2] = pic.rps.ref_dpb_index[LAST];
    pic.rps.ref_dpb_index[ALT] = pic.rps.ref_dpb_index[LAST];

    // Layer0 toggle 0->1->2->3
    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, max_refs - 1);
    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
    let mut i = 3i32;
    while i >= i32::from(max_refs) {
        pic.rps.refresh_frame_mask |= 1u8 << i;
        i -= 1;
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
}

/// C `av1_generate_rps_info`'s low-delay CQP/CRF branch
/// (`pd_process.c:1987-2064`) — the campaign's first cell.
///
/// The structure is the previous 3 non-base frames + the previous 3 base
/// frames + one long-term reference in slot 7, refreshed every 128 pictures.
///
/// Trap: `lay1_pic_idx` is `(1 << (hierarchical_levels - 1)) - 1` and is
/// special-cased to 0 at `hierarchical_levels == 0` — the shift would be
/// `1 << -1` otherwise. It selects whether a non-base picture past the layer-1
/// picture takes the layer-1 picture as LAST instead of the previous base.
pub(super) fn rps_low_delay_cqp(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) {
    let mrp = &seq.mrp_ctrls;
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;

    let base2_idx = ctx.lay0_toggle; // newest L0 in the DPB
    let base1_idx = circ_dec(base2_idx, 0, 2); // middle L0
    let base0_idx = circ_dec(base1_idx, 0, 2); // oldest L0

    let lay1_offset = if mrp.ld_reduce_ref_buffs == 0 {
        LAY1_OFF
    } else {
        1
    };
    let lay1_2_idx = if mrp.ld_reduce_ref_buffs == 2 {
        1
    } else {
        lay1_offset + ctx.lay1_toggle
    };
    let lay1_1_idx = circ_dec(lay1_2_idx, lay1_offset, lay1_offset + 2);
    let lay1_0_idx = circ_dec(lay1_1_idx, lay1_offset, lay1_offset + 2);
    const LONG_BASE_IDX: u8 = 7;
    const LONG_BASE_PIC: u64 = 128;

    let is_base = temporal_layer == 0;
    let ref_list1_count = if is_base {
        mrp.base_ref_list1_count
    } else {
        mrp.non_base_ref_list1_count
    };

    let lay1_pic_idx: u32 = if hier == 0 {
        0
    } else {
        (1u32 << (hier - 1)) - 1
    };
    // When list1 is unused, pictures after the layer-1 picture take the
    // layer-1 picture as LAST instead of the previous base.
    pic.rps.ref_dpb_index[LAST] = if pic_idx > lay1_pic_idx && !is_base && ref_list1_count == 0 {
        lay1_2_idx
    } else {
        base2_idx
    };
    pic.rps.ref_dpb_index[LAST2] = lay1_1_idx;
    pic.rps.ref_dpb_index[LAST3] = LONG_BASE_IDX;
    pic.rps.ref_dpb_index[GOLD] = base0_idx;
    pic.rps.ref_dpb_index[BWD] = lay1_2_idx;
    pic.rps.ref_dpb_index[ALT2] = lay1_0_idx;
    pic.rps.ref_dpb_index[ALT] = base1_idx;

    if temporal_layer == 0 {
        if mrp.ld_reduce_ref_buffs == 2 {
            // Only 2 DPB entries used; refresh the rest to free ref buffers.
            pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
        } else if mrp.ld_reduce_ref_buffs == 1 {
            pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
        } else {
            // Layer0 toggle 0->1->2
            ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
            pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
        }
    } else if pic.is_ref {
        if mrp.ld_reduce_ref_buffs == 2 {
            pic.rps.refresh_frame_mask = 1u8 << 1;
        } else {
            // Layer1 toggle 0->1->2
            ctx.lay1_toggle = circ_inc(ctx.lay1_toggle, 0, 2);
            pic.rps.refresh_frame_mask = 1u8 << (lay1_offset + ctx.lay1_toggle);
        }
    } else {
        pic.rps.refresh_frame_mask = 0;
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    // Keep the long-term base reference in the base layer.
    if pic.picture_number - ctx.last_long_base_pic >= LONG_BASE_PIC && pic.temporal_layer_index == 0
    {
        pic.rps.refresh_frame_mask |= 1u8 << LONG_BASE_IDX;
        ctx.last_long_base_pic = pic.picture_number;
    }
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
}

/// C `av1_generate_rps_info`'s low-delay CBR branch (`pd_process.c:2065-2237`).
///
/// LD CBR supports only `hierarchical_levels` 1 and 2 (C asserts it); anything
/// else is refused here rather than falling into the wrong table.
///
/// # Errors
///
/// Returns [`RpsBranchUnsupported`] for a hierarchical level or temporal layer
/// C itself logs as unexpected.
pub(super) fn rps_low_delay_cbr(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsBranchUnsupported> {
    let mrp = &seq.mrp_ctrls;
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;
    let lay0_toggle = ctx.lay0_toggle;
    let lay1_toggle = ctx.lay1_toggle;

    let base2_idx = lay0_toggle;
    let base1_idx = circ_dec(base2_idx, 0, 2);
    let base0_idx = circ_dec(base1_idx, 0, 2);

    // Index trap: at ld_reduce_ref_buffs == 2 C writes `!lay0_toggle`, the
    // LOGICAL negation of the toggle (0 -> 1, anything else -> 0), NOT a
    // bitwise complement and not `LAY1_OFF + something`.
    let lay1_1_idx = if mrp.ld_reduce_ref_buffs == 2 {
        u8::from(lay0_toggle == 0)
    } else if mrp.ld_reduce_ref_buffs == 1 {
        LAY1_OFF
    } else {
        LAY1_OFF + lay1_toggle
    };
    let lay1_0_idx = circ_dec(lay1_1_idx, LAY1_OFF, LAY1_OFF + 1);
    let lay2_idx = LAY2_OFF;
    const LONG_BASE_IDX: u8 = 7;
    const LONG_BASE_PIC: u64 = 128;

    let idx = &mut pic.rps.ref_dpb_index;
    if hier == 1 {
        match temporal_layer {
            0 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = base1_idx;
                idx[LAST3] = LONG_BASE_IDX;
                idx[GOLD] = base0_idx;
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
                } else {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
                }
            }
            1 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = if mrp.referencing_scheme == 0 {
                    base1_idx
                } else {
                    lay1_1_idx
                };
                idx[LAST3] = base1_idx;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                pic.rps.refresh_frame_mask = 0;
                if pic.is_ref {
                    if mrp.ld_reduce_ref_buffs == 2 {
                        pic.rps.refresh_frame_mask = (1u8 << u8::from(ctx.lay0_toggle == 0)) | 0xfc;
                    } else if mrp.ld_reduce_ref_buffs == 1 {
                        pic.rps.refresh_frame_mask = (1u8 << LAY1_OFF) | 0xf0;
                    } else {
                        // Layer1 toggle 0->1
                        ctx.lay1_toggle = 1 - ctx.lay1_toggle;
                        pic.rps.refresh_frame_mask = 1u8 << (LAY1_OFF + ctx.lay1_toggle);
                    }
                }
            }
            _ => {
                return Err(RpsBranchUnsupported {
                    hierarchical_levels: hier,
                    temporal_layer,
                });
            }
        }
    } else {
        // C's `else` after `hierarchical_levels == 1` carries only
        // `assert(hierarchical_levels == 2)` — a DEBUG check — and the comment
        // "LD CBR only supports flat/1L/2L". In a release build a flat
        // (hier 0) stream falls through to this `switch (temporal_layer)`,
        // whose case 0 is the arm every flat-LD picture takes. Treating
        // `hier != 1` uniformly IS the faithful behaviour; the Err below is
        // for the shapes C itself only logs an error for (temporal_layer
        // outside 0..=2, or an HL2 mini-GOP position it cannot index).
        match temporal_layer {
            0 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = base0_idx;
                idx[LAST3] = LONG_BASE_IDX;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
                } else {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
                }
            }
            1 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = lay1_1_idx;
                idx[LAST3] = base1_idx;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << u8::from(ctx.lay0_toggle == 0)) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    pic.rps.refresh_frame_mask = (1u8 << LAY1_OFF) | 0xf0;
                } else {
                    ctx.lay1_toggle = 1 - ctx.lay1_toggle;
                    pic.rps.refresh_frame_mask = 1u8 << (LAY1_OFF + ctx.lay1_toggle);
                }
            }
            2 => {
                if pic_idx == 0 {
                    idx[LAST] = base2_idx;
                    idx[LAST2] = lay1_1_idx;
                    idx[LAST3] = base1_idx;
                } else if pic_idx == 2 {
                    idx[LAST] = lay1_1_idx;
                    idx[LAST2] = base2_idx;
                    idx[LAST3] = lay1_0_idx;
                } else {
                    // C logs "Error in MG indexing - LD CBR HL2" and leaves the
                    // indices at whatever they were. Refuse instead.
                    return Err(RpsBranchUnsupported {
                        hierarchical_levels: hier,
                        temporal_layer,
                    });
                }
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                pic.rps.refresh_frame_mask = if pic.is_ref { 1u8 << lay2_idx } else { 0 };
                // Redundant in C, kept to avoid a hang on a bad setting.
                if mrp.ld_reduce_ref_buffs != 0 {
                    pic.rps.refresh_frame_mask = 0;
                }
            }
            _ => {
                return Err(RpsBranchUnsupported {
                    hierarchical_levels: hier,
                    temporal_layer,
                });
            }
        }
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    if seq.pred_structure == PredStructure::LowDelay
        && pic.picture_number - ctx.last_long_base_pic >= LONG_BASE_PIC
        && pic.temporal_layer_index == 0
    {
        pic.rps.refresh_frame_mask |= 1u8 << LONG_BASE_IDX;
        ctx.last_long_base_pic = pic.picture_number;
    }
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
    Ok(())
}

/// C `av1_generate_rps_info`'s `hierarchical_levels == 0` branch
/// (`pd_process.c:2238-2269`) — random access, flat.
///
/// Walks all 8 DPB slots: list 0 takes the 1st/3rd/5th/8th newest base
/// pictures and list 1 the 2nd/4th/6th, matching the `{1,3,5,7}` /
/// `{2,4,6,0}` GOP tables in the C comment.
///
/// Note the ORDER: the toggle advances AFTER `prune_refs`, unlike the
/// low-delay branches where it advances before `update_ref_poc_array`. A port
/// that hoists the toggle to the top of the branch reads the wrong slot for
/// LAST on every frame.
pub(super) fn rps_random_access_flat(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    mg_idx: usize,
) {
    let base0_idx = ctx.lay0_toggle;
    let base1_idx = circ_dec(base0_idx, 0, 7);
    let base2_idx = circ_dec(base1_idx, 0, 7);
    let base3_idx = circ_dec(base2_idx, 0, 7);
    let base4_idx = circ_dec(base3_idx, 0, 7);
    let base5_idx = circ_dec(base4_idx, 0, 7);
    let base7_idx = circ_dec(base5_idx, 0, 7);

    pic.rps.ref_dpb_index[LAST] = base0_idx;
    pic.rps.ref_dpb_index[LAST2] = base2_idx;
    pic.rps.ref_dpb_index[LAST3] = base4_idx;
    pic.rps.ref_dpb_index[GOLD] = base7_idx;
    pic.rps.ref_dpb_index[BWD] = base1_idx;
    pic.rps.ref_dpb_index[ALT2] = base3_idx;
    pic.rps.ref_dpb_index[ALT] = base5_idx;

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );

    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 7);
    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;

    // Flat mode outputs every frame; C calls set_frame_display_params and then
    // unconditionally overrides both fields.
    set_frame_display_params(pic, ctx, mg_idx);
    pic.show_frame = true;
    pic.has_show_existing = false;
}

// ---------------------------------------------------------------------------
// init_pic_settings + the per-picture call sequence
// ---------------------------------------------------------------------------

use super::*;

/// C `svt_aom_set_tpl_group` (`initial_rc_process.c:204-306`) — EXPORTED.
///
/// Fills [`TplControls`] `enable` / `reduced_tpl_group` / `synth_blk_size` and
/// the `r0_adjust_factor`. Returns `(controls, synth_blk_size)`.
///
/// `pic` is `None` for C's `pcs == NULL` probe call, which returns only the
/// synthesizer block size and writes nothing back.
///
/// **The `slice_type` trap.** C writes `pcs->slice_type ? A : B`. `SliceType`
/// is `B_SLICE = 0, I_SLICE = 1`, so the TRUE arm is the **I slice** and the
/// FALSE arm is the inter slice — the opposite of what "slice_type ?" reads
/// like. Every conditional below preserves that orientation explicitly.
///
/// # Panics
///
/// Panics on an unknown `tpl_group_level` (C asserts there).
#[must_use]
pub fn set_tpl_group(
    pic: Option<&TplPicParams>,
    tpl_group_level: u8,
    source_width: u32,
    source_height: u32,
) -> (TplControls, u8) {
    let mut t = TplControls::default();
    let is_i = |p: &TplPicParams| p.slice_type == SliceType::I;
    let small = |p: &TplPicParams| p.input_resolution <= INPUT_SIZE_480P_RANGE;

    match tpl_group_level {
        0 => t.enable = 0,
        1 => {
            t.enable = 1;
            t.reduced_tpl_group = -1;
            t.synth_blk_size = 16;
        }
        2 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) if is_i(p) => -1,
                Some(p) => {
                    if p.hierarchical_levels == 5 {
                        4
                    } else {
                        3
                    }
                }
            };
            t.synth_blk_size = 16;
        }
        3 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) => {
                    if p.hierarchical_levels == 5 {
                        4
                    } else {
                        3
                    }
                }
            };
            t.synth_blk_size = 16;
        }
        4 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) => match p.hierarchical_levels {
                    5 => {
                        if is_i(p) {
                            2
                        } else if small(p) {
                            3
                        } else {
                            1
                        }
                    }
                    // C's hierarchical_levels == 4 arm really does yield 2
                    // for BOTH the I-slice and the <= 480p inter case; the two
                    // branches are kept apart because the C source spells them
                    // out separately and they diverge at every other level.
                    #[allow(clippy::if_same_then_else)]
                    4 => {
                        if is_i(p) {
                            2
                        } else if small(p) {
                            2
                        } else {
                            1
                        }
                    }
                    _ => {
                        if is_i(p) {
                            3
                        } else if small(p) {
                            2
                        } else {
                            0
                        }
                    }
                },
            };
            t.synth_blk_size = if source_width.min(source_height) >= 720 {
                32
            } else {
                16
            };
        }
        _ => panic!("svt_aom_set_tpl_group: unknown tpl_group_level {tpl_group_level}"),
    }

    let Some(p) = pic else {
        return (t, t.synth_blk_size);
    };

    if i32::from(p.hierarchical_levels) <= i32::from(t.reduced_tpl_group) {
        t.reduced_tpl_group = -1;
    }

    if t.reduced_tpl_group >= 0 {
        // The r0 compensation for TPL not using every available frame.
        t.r0_adjust_factor = match i32::from(p.hierarchical_levels) - i32::from(t.reduced_tpl_group)
        {
            1 => {
                if p.hierarchical_levels <= 2 {
                    0.4
                } else if p.hierarchical_levels <= 3 {
                    0.8
                } else {
                    1.6
                }
            }
            2 => {
                if p.hierarchical_levels <= 2 {
                    0.6
                } else if p.hierarchical_levels <= 3 {
                    1.2
                } else {
                    2.4
                }
            }
            3 => {
                if p.hierarchical_levels <= 3 {
                    1.4
                } else {
                    2.8
                }
            }
            4 => 4.0,
            5 => 6.0,
            // C's `case 0: default:` share an arm, so a NEGATIVE difference
            // lands here too.
            _ => 0.0,
        };
        if p.tpl_lad_mg == 0 {
            t.r0_adjust_factor *= 1.25;
        }
    } else {
        t.r0_adjust_factor = 0.0;
        if p.tpl_lad_mg == 0 {
            t.r0_adjust_factor = if is_i(p) {
                0.0
            } else if p.hierarchical_levels <= 2 {
                0.4
            } else if p.hierarchical_levels <= 3 {
                0.8
            } else {
                1.6
            };
        }
    }
    if p.rate_control_mode == RcMode::Vbr {
        t.r0_adjust_factor *= 1.25;
        t.r0_adjust_factor = t.r0_adjust_factor.min(3.0);
    }
    (t, t.synth_blk_size)
}

/// C `get_tpl_params_level` (`initial_rc_process.c:307-318`) — static.
#[must_use]
pub fn get_tpl_params_level(enc_mode: i8) -> u8 {
    const ENC_M2: i8 = 2;
    const ENC_M7: i8 = 7;
    if enc_mode <= ENC_M2 {
        1
    } else if enc_mode <= ENC_M7 {
        4
    } else {
        5
    }
}

/// C `set_tpl_params` (`initial_rc_process.c:319-405`) — static.
///
/// Sets what TPL computes and therefore what qindex each SB gets. Note that
/// this MUTATES an existing [`TplControls`] (C writes through
/// `&pcs->tpl_ctrls`) rather than starting from zero, so `enable`,
/// `reduced_tpl_group`, `r0_adjust_factor` and `synth_blk_size` — all set by
/// [`set_tpl_group`] — survive.
///
/// # Panics
///
/// Panics on an unknown `tpl_level` (C asserts there).
pub fn set_tpl_params(t: &mut TplControls, tpl_level: u8, input_resolution: u8) {
    let small_shape = if input_resolution <= INPUT_SIZE_480P_RANGE {
        N2_SHAPE
    } else {
        N4_SHAPE
    };
    match tpl_level {
        0 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 0;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = DEFAULT_SHAPE;
            t.use_sad_in_src_search = 0;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 0;
        }
        1 => {
            t.compute_rate = 1;
            t.enable_tpl_qps = 1;
            t.disable_intra_pred_nref = 0;
            t.intra_mode_end = PAETH_PRED;
            t.pf_shape = DEFAULT_SHAPE;
            t.use_sad_in_src_search = 0;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 0;
        }
        2 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = PAETH_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 0;
        }
        3 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 4;
        }
        4 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 4;
        }
        5 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 1;
            t.subsample_tx = 2;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 4;
        }
        _ => panic!("set_tpl_params: unknown tpl_level {tpl_level}"),
    }
}

/// C `is_frame_already_exists` (`initial_rc_process.c:161-170`) — static.
///
/// De-duplication inside the TPL group build. Omitting it double-counts a
/// picture in the propagation.
#[must_use]
pub fn is_frame_already_exists(tpl_group_pocs: &[u64], end_index: usize, pic_num: u64) -> bool {
    tpl_group_pocs[..end_index].contains(&pic_num)
}

/// C `validate_pic_for_tpl` (`initial_rc_process.c:171-189`) — EXPORTED.
///
/// Admission test for a picture into the TPL group. Returns whether the
/// picture is valid; the caller increments `used_tpl_frame_num` when it is.
///
/// Trap: the `reduced_tpl_group` test is `temporal_layer_index <=
/// reduced_tpl_group`, and it applies only when `reduced_tpl_group >= 0`. A
/// value of 0 means "base layer only", NOT "no reduction" — that is -1.
#[must_use]
pub fn validate_pic_for_tpl(
    tpl_group_pocs: &[u64],
    tpl_group_layers: &[u8],
    pic_index: usize,
    reduced_tpl_group: i8,
    is_pic_skipped: bool,
) -> bool {
    if is_frame_already_exists(tpl_group_pocs, pic_index, tpl_group_pocs[pic_index])
        || is_pic_skipped
    {
        return false;
    }
    if reduced_tpl_group >= 0 {
        i32::from(tpl_group_layers[pic_index]) <= i32::from(reduced_tpl_group)
    } else {
        true
    }
}

/// One member of the extended lookahead group, as
/// `store_extended_group` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtGroupPic {
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `pcs->slice_type`.
    pub slice_type: SliceType,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->ext_mg_id`.
    pub ext_mg_id: i64,
    /// C `svt_aom_is_delayed_intra(pcs)`, precomputed by the caller.
    pub is_delayed_intra: bool,
    /// C `svt_aom_is_pic_skipped(pcs)`, precomputed by the caller.
    pub is_skipped: bool,
}

/// The TPL group `store_extended_group` produces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TplGroup {
    /// Indices into the extended group, in order — C's `pcs->tpl_group[]`.
    pub members: alloc::vec::Vec<usize>,
    /// C `pcs->tpl_valid_pic[]`, indexed like the EXTENDED group (not like
    /// `members`) — C writes `tpl_valid_pic[i]` where `i` is the ext-group
    /// index, which is the same as the member index only while nothing is
    /// skipped.
    pub valid: alloc::vec::Vec<u8>,
    /// C `pcs->used_tpl_frame_num`.
    pub used_tpl_frame_num: u32,
}

/// C `store_extended_group`'s GROUP-SELECTION half
/// (`initial_rc_process.c:439-497`) — EXPORTED symbol, ported at tier 4.
///
/// Selects TPL group MEMBERSHIP: different membership is a different `r0` and
/// a different qindex on every SB.
///
/// The first half of the C function (walking `ctx->lad_queue` to build
/// `pcs->ext_group`) is NOT ported — it is queue plumbing over a circular
/// buffer of in-flight PCS objects, which this port replaces by design. The
/// caller passes the extended group directly.
///
/// Traps, all transcribed literally:
/// * `tpl_valid_pic[0]` is forced to 1 BEFORE the loop, so picture 0 is
///   admitted even if `validate_pic_for_tpl` would reject it — but
///   `used_tpl_frame_num` is NOT incremented for it unless validation passes.
/// * `limited_tpl_group_size` counts `1 + (tpl_lad_mg + 1) * mg_size` for an I
///   slice and `(tpl_lad_mg + 1) * mg_size` otherwise, then clamps to the
///   extended group size.
/// * A non-delayed intra at `i != 0` is ADDED and then closes the GOP
///   (`is_gop_end = 1`); a DELAYED intra at `i != 0` breaks immediately
///   WITHOUT being added. The two arms look symmetric and are not.
/// * After `is_gop_end`, only pictures with the SAME `ext_mg_id` as that intra
///   continue; the first one with a different id breaks.
#[must_use]
pub fn store_extended_group(
    ext_group: &[ExtGroupPic],
    slice_type: SliceType,
    hierarchical_levels: u8,
    tpl_lad_mg: u32,
    reduced_tpl_group: i8,
) -> TplGroup {
    let mut g = TplGroup {
        members: alloc::vec::Vec::new(),
        valid: alloc::vec![0u8; ext_group.len()],
        used_tpl_frame_num: 0,
    };
    if ext_group.is_empty() {
        return g;
    }
    g.valid[0] = 1;

    let mg_size = 1u32 << hierarchical_levels;
    let limited = if slice_type == SliceType::I {
        (1 + (tpl_lad_mg + 1) * mg_size).min(ext_group.len() as u32)
    } else {
        ((tpl_lad_mg + 1) * mg_size).min(ext_group.len() as u32)
    } as usize;

    let mut is_gop_end = false;
    let mut last_intra_mg_id: i64 = 0;
    // Group POCs/layers accumulated so far, for the de-duplication test.
    let mut pocs: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let mut layers: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    let admit = |g: &mut TplGroup,
                 pocs: &mut alloc::vec::Vec<u64>,
                 layers: &mut alloc::vec::Vec<u8>,
                 i: usize| {
        let cur = ext_group[i];
        g.members.push(i);
        // C's validate_pic_for_tpl indexes pcs->tpl_group[pic_index] with the
        // EXT-group index, and by construction the picture just appended sits
        // at that index while nothing has been skipped.
        pocs.push(cur.picture_number);
        layers.push(cur.temporal_layer_index);
        let at = pocs.len() - 1;
        if validate_pic_for_tpl(pocs, layers, at, reduced_tpl_group, cur.is_skipped) {
            g.valid[i] = 1;
            g.used_tpl_frame_num += 1;
        }
    };

    // The two `admit` arms after `is_gop_end` are identical in body and
    // different in guard; C spells them out separately and the guards are the
    // whole point, so they are not merged.
    #[allow(clippy::if_same_then_else)]
    for i in 0..limited {
        let cur = ext_group[i];
        if cur.slice_type == SliceType::I {
            if cur.is_delayed_intra {
                if i == 0 {
                    admit(&mut g, &mut pocs, &mut layers, i);
                } else {
                    break;
                }
            } else if i == 0 {
                admit(&mut g, &mut pocs, &mut layers, i);
            } else {
                admit(&mut g, &mut pocs, &mut layers, i);
                last_intra_mg_id = cur.ext_mg_id;
                is_gop_end = true;
            }
        } else if !is_gop_end {
            admit(&mut g, &mut pocs, &mut layers, i);
        } else if cur.ext_mg_id == last_intra_mg_id {
            admit(&mut g, &mut pocs, &mut layers, i);
        } else {
            break;
        }
    }
    g
}

// ---------------------------------------------------------------------------
// Reference binding + primary_ref_frame — Codec/pic_manager_process.c
// ---------------------------------------------------------------------------

/// C `PRIMARY_REF_NONE` (`definitions.h:1470`).
pub const PRIMARY_REF_NONE: u8 = 7;
/// C `REFRESH_FRAME_CONTEXT_BACKWARD`.
pub const REFRESH_FRAME_CONTEXT_BACKWARD: u8 = 1;

/// C `get_list_idx` (`inter_prediction.h:531-535`) via `ref_type_to_list_idx`.
#[must_use]
pub fn get_list_idx(ref_type: u8) -> u8 {
    const REF_TYPE_TO_LIST_IDX: [u8; 8] = [0, 0, 0, 0, 0, 1, 1, 1];
    REF_TYPE_TO_LIST_IDX[ref_type as usize]
}

/// C `get_ref_frame_idx` (`inter_prediction.h:537-541`) via `ref_type_to_ref_idx`.
#[must_use]
pub fn get_ref_frame_idx(ref_type: u8) -> u8 {
    const REF_TYPE_TO_REF_IDX: [u8; 8] = [0, 0, 1, 2, 3, 0, 1, 2];
    REF_TYPE_TO_REF_IDX[ref_type as usize]
}

/// One entry of C's `enc_ctx->ref_pic_list` — a reference the picture manager
/// can bind to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefQueueEntry {
    /// C `ref_pic_entry->picture_number`.
    pub picture_number: u64,
    /// C `ref_pic_entry->is_valid`.
    pub is_valid: bool,
    /// C `ref_pic_entry->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `ref_obj->base_q_idx`.
    pub base_q_idx: u8,
    /// C `ref_obj->slice_type`.
    pub slice_type: SliceType,
    /// C `ref_obj->r0`.
    pub r0: f64,
}

/// C `search_ref_in_ref_queue` (`pic_manager_process.c:178-188`) — EXPORTED.
///
/// Resolves a reference POC to its reference-queue entry, skipping invalid
/// slots. Returns the index, or `None`.
///
/// Trap: the C loop assigns `ref_pic_entry` on every iteration and returns
/// only on a match, so a caller that reads the variable after a miss sees the
/// LAST scanned entry — but the function itself returns NULL, which is what
/// this reproduces.
#[must_use]
pub fn search_ref_in_ref_queue(ref_pic_list: &[RefQueueEntry], ref_poc: u64) -> Option<usize> {
    ref_pic_list
        .iter()
        .position(|e| e.is_valid && e.picture_number == ref_poc)
}

/// What the picture manager binds onto a child PCS for one picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefBinding {
    /// C `child_pcs->ref_base_q_idx[list][idx]`.
    pub ref_base_q_idx: [[u8; 4]; 2],
    /// C `child_pcs->ref_slice_type[list][idx]`.
    pub ref_slice_type: [[SliceType; 4]; 2],
    /// C `child_pcs->ref_pic_r0[list][idx]`.
    pub ref_pic_r0: [[f64; 4]; 2],
    /// C `child_pcs->ppcs->frm_hdr.primary_ref_frame` — a written header field.
    pub primary_ref_frame: u8,
    /// C `child_pcs->ppcs->refresh_frame_context`.
    pub refresh_frame_context: u8,
    /// The queue index each reference resolved to, indexed by
    /// [`LAST`]..=[`ALT`]; `None` where the reference was not in the queue.
    pub resolved: [Option<usize>; INTER_REFS_PER_FRAME],
}

impl Default for RefBinding {
    fn default() -> Self {
        Self {
            ref_base_q_idx: [[0; 4]; 2],
            ref_slice_type: [[SliceType::B; 4]; 2],
            ref_pic_r0: [[0.0; 4]; 2],
            primary_ref_frame: PRIMARY_REF_NONE,
            refresh_frame_context: REFRESH_FRAME_CONTEXT_BACKWARD,
            resolved: [None; INTER_REFS_PER_FRAME],
        }
    }
}

/// C `svt_aom_picture_manager_kernel_iter`'s EB_PIC_INPUT reference-binding
/// block (`pic_manager_process.c:798-874`) — this is inter campaign chunk
/// C1b's deliverable.
///
/// Derives `frm_hdr.primary_ref_frame` and `refresh_frame_context`, and binds
/// `ref_base_q_idx` / `ref_slice_type` / `ref_pic_r0` per reference. CDF
/// continuation off the wrong primary ref desynchronises the entropy decoder,
/// so this is a hard bitstream field, not a heuristic.
///
/// **EVIDENCE TIER 4, and the reason is worth stating.**
/// `svt_aom_picture_manager_kernel_iter` IS an exported symbol (`nm -g` on
/// `Bin/Release/libSvtAv1Enc.a` finds it) but is NOT callable in isolation:
/// the first thing it does is `EB_GET_FULL_OBJECT` on a fifo, so a shim would
/// block rather than return. "A symbol exists" and "tier 1 is reachable" are
/// different facts here. The block is therefore gated by hand-derived vectors
/// traced against the C source, and a byte-identity gate on the inter frame
/// header (tier 2) is the upgrade path.
///
/// **The primary-ref rule, stated precisely because it is easy to get wrong:**
/// walk LAST..ALT in order and keep the reference with the LARGEST
/// `temporal_layer_index` that is still `<= this picture's` temporal layer.
/// Ties keep the FIRST such reference (the comparison is strict `<`), so LAST
/// wins over a later reference at the same layer. The result is stored as a
/// `REF_FRAME_MINUS1` (0..6), NOT as an `MvReferenceFrame` — C asserts
/// `ref_index == (int)ref` on exactly that point.
///
/// Not ported, and named rather than dropped: the `ref_global_motion[]` copy
/// inside the same guard (it needs the reference object's warp params, which
/// live in the global-motion module) and the `ref_pic_ptr_array` /
/// live-count bookkeeping, which is buffer plumbing this port replaces.
///
/// # Panics
///
/// Panics if a B-slice reference POC is absent from the queue. C asserts and
/// raises `EB_ENC_PM_ERROR10` there; a missing reference means the DPB model
/// and the queue have already diverged, and continuing would bind a wrong
/// picture.
#[must_use]
pub fn bind_refs_and_primary_ref_frame(
    pic: &PicParams,
    ref_pic_list: &[RefQueueEntry],
    frame_end_cdf_update_mode: bool,
    is_s_frame: bool,
) -> RefBinding {
    let mut b = RefBinding::default();
    let mut ref_index: i8 = 0;

    if pic.slice_type == SliceType::B {
        let mut max_temporal_index: i8 = -1;
        for r in LAST..=ALT {
            // The overlay frame hardcodes its own POC as the reference.
            let ref_poc = if pic.is_overlay {
                pic.picture_number
            } else {
                pic.rps.ref_poc_array[r]
            };
            let ref_type = (r as u8) + 1;
            let list_idx = get_list_idx(ref_type) as usize;
            let ref_idx = get_ref_frame_idx(ref_type) as usize;

            let found = search_ref_in_ref_queue(ref_pic_list, ref_poc).unwrap_or_else(|| {
                panic!(
                    "picture manager: reference POC {ref_poc} for ref {r} of picture \
                     {} is not in the reference queue (C asserts + EB_ENC_PM_ERROR10)",
                    pic.picture_number
                )
            });
            b.resolved[r] = Some(found);
            let e = ref_pic_list[found];

            if frame_end_cdf_update_mode
                && max_temporal_index < e.temporal_layer_index as i8
                && e.temporal_layer_index <= pic.temporal_layer_index
            {
                max_temporal_index = e.temporal_layer_index as i8;
                // Stored as REF_FRAME_MINUS1, not as an MvReferenceFrame.
                ref_index = get_ref_frame_type(list_idx as u8, ref_idx as u8) - LAST_FRAME;
                debug_assert_eq!(ref_index as usize, r);
                // C also copies ref_global_motion[] here; see the doc comment.
            }

            b.ref_base_q_idx[list_idx][ref_idx] = e.base_q_idx;
            b.ref_slice_type[list_idx][ref_idx] = e.slice_type;
            b.ref_pic_r0[list_idx][ref_idx] = e.r0;
        }
    }

    if frame_end_cdf_update_mode {
        b.primary_ref_frame = if pic.slice_type != SliceType::I && !is_s_frame {
            ref_index as u8
        } else {
            PRIMARY_REF_NONE
        };
    } else {
        b.primary_ref_frame = PRIMARY_REF_NONE;
    }
    // C sets REFRESH_FRAME_CONTEXT_BACKWARD in BOTH arms; the comment there
    // says it is never disabled so the feature can be on in higher layers
    // while off in low ones.
    b.refresh_frame_context = REFRESH_FRAME_CONTEXT_BACKWARD;
    b
}

// ---------------------------------------------------------------------------
// send_picture_out's reference-count adjustment + temporal-filter params
// ---------------------------------------------------------------------------

/// C `INVALID_LUMA` (`definitions.h:90`).
///
/// It is **256**, not -1 and not 0. `avg_luma` is a `uint64_t` holding an
/// 8-bit mean, so 256 is the out-of-range sentinel; a port that guessed -1
/// would treat a genuinely invalid reference as valid and compare against
/// garbage. Read from the header, not inferred from the name.
pub const INVALID_LUMA: u64 = 256;

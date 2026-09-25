use super::*;

/// C `init_pic_settings` (`pd_process.c:4910-4965`) — static, tier 4.
///
/// The single per-picture inter-settings funnel: `reference_mode`,
/// `mi_cols`/`mi_rows`, `ref_order_hint[]`/`cur_order_hint`, sign bias, the
/// try counts, the skip-mode params and `ref_frame_type_arr`. Every one is
/// either a written header field or an MD gate.
///
/// **Two C calls in this body are deliberately NOT made here** because they
/// belong to other modules and other lanes own them:
/// * `copy_tf_params(scs, pcs, ctx)` — the temporal-filter control mapping.
///   Measured: in `LOW_DELAY` `tf_level` is forced to 0 before any preset
///   logic (`enc_handle.c:3339-3343`), so this is a no-op for the campaign's
///   first cell; it is live in random access.
/// * `svt_aom_sig_deriv_multi_processes_{allintra,rtc,default}` — the
///   per-preset feature-level derivation (a different file group entirely).
///
/// Everything else in the C body is reproduced, in C's order.
///
/// Index trap: `ref_order_hint[i]` is `ref_poc_array[i] % (1 << order_hint_bits)`
/// — the array is indexed by `REF_FRAME_MINUS1` (0..6), while
/// [`set_ref_frame_sign_bias`] indexes the SAME data by `MvReferenceFrame`
/// (1..7). Getting the two off by one silently mis-signs every temporal MV.
///
/// `pa_luma` resolves a reference's PA `avg_luma` — C's
/// `ref_pa_pic_ptr_array[list][ref]->avg_luma`, which `send_picture_out`'s
/// `get_similar_ref_brightness` reads after `assign_and_release_pa_refs`.
/// The caller passes its own poc/slot→PA resolution so the same
/// in-window/DPB origins are used for ME refs and this lookup. Return
/// [`INVALID_LUMA`] for a reference that cannot be resolved — C's check is
/// `!= INVALID_LUMA`, so an unresolvable reference disables the test.
pub fn init_pic_settings(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pa_luma: &dyn Fn(u64, usize) -> u64,
) {
    pic.allow_comp_inter_inter = pic.slice_type != SliceType::I;
    pic.reference_mode = if pic.slice_type == SliceType::I {
        ReferenceMode::IntraSentinel
    } else if is_incomp_mg_frame(pic, seq) {
        ReferenceMode::Single
    } else {
        ReferenceMode::Select
    };

    // mi_cols/mi_rows come from the ALIGNED dimensions, not the display ones.
    pic.mi_cols = pic.aligned_width >> MI_SIZE_LOG2;
    pic.mi_rows = pic.aligned_height >> MI_SIZE_LOG2;

    // Initialize the order hints.
    let bits = seq.order_hint_info.order_hint_bits;
    let modulus = 1u64 << bits;
    for i in 0..INTER_REFS_PER_FRAME {
        pic.ref_order_hint[i] = (pic.rps.ref_poc_array[i] % modulus) as u32;
    }
    pic.cur_order_hint = (pic.picture_number % modulus) as u32;

    set_ref_frame_sign_bias(pic, seq);

    // C `copy_tf_params` (`pd_process.c:4931`), placed where C calls it —
    // inside `init_pic_settings`, right after the order hints and sign bias.
    // A `Disabled` choice maps to `tf_ctrls.enabled = 0`; every other choice
    // copies the whole `tf_params_per_type` entry, including the case where
    // the table itself is disabled (`tf_level == 0` under LOW_DELAY).
    pic.tf_ctrls = match copy_tf_params(
        seq.pred_structure,
        pic.slice_type,
        pic.is_key_frame,
        pic.temporal_layer_index,
        pic.hierarchical_levels,
        pic.is_overlay,
        seq.enable_tf_key,
        pic.is_delayed_intra,
    ) {
        TfParamsChoice::DelayedIntra => seq.tf_params_per_type[0],
        TfParamsChoice::Base => seq.tf_params_per_type[1],
        TfParamsChoice::L1 => seq.tf_params_per_type[2],
        TfParamsChoice::Disabled => TfCtrls::default(),
    };
    // sig_deriv_multi_processes: see the doc comment.

    update_count_try(pic, seq);

    if ctx.transition_detected == 1 && pic.temporal_layer_index == 0 {
        pic.transition_present = 1;
        ctx.transition_detected = 0;
    }

    if ctx.list0_only && pic.slice_type == SliceType::B && pic.temporal_layer_index == 0 {
        pic.ref_list1_count_try = 0;
    }
    debug_assert!(pic.ref_list0_count_try <= pic.ref_list0_count);
    debug_assert!(pic.ref_list1_count_try <= pic.ref_list1_count);

    // Skip mode syntax, spec 5.9.22.
    setup_skip_mode_allowed(pic, seq);
    pic.skip_mode.skip_mode_flag = pic.skip_mode.skip_mode_allowed;

    let (arr, tot) = set_all_ref_frame_type(pic);
    pic.ref_frame_type_arr = arr;
    pic.tot_ref_frame_types = tot;

    // C `send_picture_out` (`pd_process.c:5132`) runs the reference-count
    // prunes at the END of pd_process, after `set_all_ref_frame_type` and
    // after `assign_and_release_pa_refs` has resolved this picture's PA
    // reference objects. `get_similar_ref_brightness` (`:4251`) reads the
    // FIRST PA reference of each list — `ref_pa_pic_ptr_array[list][0]`,
    // the object for `ref_poc_array[LAST]` / `ref_poc_array[BWD]` — so
    // `pa_luma` resolves the same (poc, dpb_slot) pair the caller's own
    // `ref_pa_pic_ptr_array` resolution uses.
    pic.similar_brightness_refs = get_similar_ref_brightness(
        pic.slice_type,
        pic.hierarchical_levels,
        pic.ref_list1_count_try,
        pa_luma(
            pic.rps.ref_poc_array[LAST],
            usize::from(pic.rps.ref_dpb_index[LAST]),
        ),
        pa_luma(
            pic.rps.ref_poc_array[BWD],
            usize::from(pic.rps.ref_dpb_index[BWD]),
        ),
        pic.avg_luma,
    );
    // `hme_dist` is `None`: the RTC early-HME prune needs
    // `mrp_detector_hme_level0` on downsampled refs, and `seq.rtc` is never
    // set here anyway.
    send_picture_out_ref_counts(pic, seq, None, pic.similar_brightness_refs);

    #[cfg(feature = "std")]
    if crate::dbgenv::rpsdbg() {
        eprintln!(
            "RPSDBG poc={} l0c={} l1c={} l0t={} l1t={} sim={} sln={} l0only={} avg={} tl={} hl={} dpb={:?} pocs={:?}",
            pic.picture_number,
            pic.ref_list0_count,
            pic.ref_list1_count,
            pic.ref_list0_count_try,
            pic.ref_list1_count_try,
            u8::from(pic.similar_brightness_refs),
            seq.mrp_ctrls.safe_limit_nref,
            u8::from(ctx.list0_only),
            pic.avg_luma,
            pic.temporal_layer_index,
            pic.hierarchical_levels,
            pic.rps.ref_dpb_index,
            pic.rps.ref_poc_array,
        );
    }
}

/// C `MI_SIZE_LOG2`.
pub const MI_SIZE_LOG2: u32 = 2;

/// The per-picture call sequence from `svt_aom_picture_decision_kernel_iter`
/// (`pd_process.c:5672-5692`), transcribed.
///
/// **The order is load-bearing and is NOT what a reading of the file suggests.**
/// Measured in the C source at the call site rather than inferred:
///
/// ```text
/// frm_hdr.frame_type = ...     (5674-5679)
/// set_gf_group_param(pcs)      (5680)   <-- BEFORE the RPS, not after
/// av1_generate_rps_info(...)   (5681)
/// update_dpb(pcs, ctx)         (5688)
/// init_pic_settings(...)       (5691)
/// ```
///
/// `set_gf_group_param` running FIRST is what makes `frame_is_boosted` — and
/// therefore the base-vs-non-base MRP caps inside
/// [`set_ref_list_counts`], which `av1_generate_rps_info` calls — read THIS
/// picture's `update_type` rather than a stale one. A port that ran the RPS
/// first would silently cap both lists with the wrong row of [`MrpCtrls`].
///
/// # Errors
///
/// Propagates [`RpsError`] from [`generate_rps_info`].
pub fn picture_decision_per_picture(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
    pa_luma: &dyn Fn(u64, usize) -> u64,
) -> Result<(), RpsError> {
    set_gf_group_param(pic);
    generate_rps_info(pic, seq, ctx, pic_idx, mg_idx)?;
    // Snapshot BEFORE `update_dpb` applies this picture's own refresh mask —
    // see [`PicParams::ref_queue_dpb`].
    pic.ref_queue_dpb = ctx.dpb;
    update_dpb(pic, ctx);
    init_pic_settings(pic, seq, ctx, pa_luma);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mini-GOP structure (pd_process.c:759-988, 4720-4893) + utility.c's table
// ---------------------------------------------------------------------------

/// C `MINI_GOP_MAX_COUNT` (`utility.h:168`).
pub const MINI_GOP_MAX_COUNT: usize = 31;
/// C `MIN_HIERARCHICAL_LEVEL` (`utility.h:171`).
pub const MIN_HIERARCHICAL_LEVEL: u8 = 1;
/// C `MAX_HIERARCHICAL_LEVEL` (`API/EbSvtAv1Enc.h:34`).
pub const MAX_HIERARCHICAL_LEVEL: u8 = 6;
/// C `HIERARCHICAL_LEVELS_AUTO` (`API/EbSvtAv1Enc.h:33`) — the `(uint32_t)~0`
/// sentinel `cfg.hierarchical_levels` defaults to, resolved inside the
/// library against pred_structure / rate_control_mode / enc_mode /
/// input_resolution (`Globals/enc_handle.c:4556-4567`). The port's parameter
/// is `u8`, so the sentinel is `u8::MAX`; see
/// [`crate::pipeline::EncodePipeline::resolve_hierarchical_levels_auto`].
pub const HIERARCHICAL_LEVELS_AUTO: u8 = u8::MAX;
/// C `mini_gop_offset` (`utility.h:172`), indexed by
/// `hierarchical_levels - MIN_HIERARCHICAL_LEVEL`.
pub const MINI_GOP_OFFSET: [u8; (MAX_HIERARCHICAL_LEVEL - MIN_HIERARCHICAL_LEVEL) as usize] =
    [1, 3, 7, 15, 31];

/// C `MinigopIndex` (`utility.h:183-214`) — the entries the activity array is
/// addressed by name.
pub const L6_INDEX: usize = 0;
/// See [`L6_INDEX`].
pub const L5_0_INDEX: usize = 1;
/// See [`L6_INDEX`].
pub const L4_0_INDEX: usize = 2;
/// See [`L6_INDEX`].
pub const L3_0_INDEX: usize = 3;
/// See [`L6_INDEX`].
pub const L2_0_INDEX: usize = 4;
/// See [`L6_INDEX`].
pub const L3_2_INDEX: usize = 10;
/// See [`L6_INDEX`].
pub const L2_2_INDEX: usize = 7;
/// See [`L6_INDEX`].
pub const L2_4_INDEX: usize = 11;
/// See [`L6_INDEX`].
pub const L2_6_INDEX: usize = 14;
/// See [`L6_INDEX`].
pub const L5_1_INDEX: usize = 16;
/// See [`L6_INDEX`].
pub const L4_2_INDEX: usize = 17;
/// See [`L6_INDEX`].
pub const L3_4_INDEX: usize = 18;
/// See [`L6_INDEX`].
pub const L2_8_INDEX: usize = 19;
/// See [`L6_INDEX`].
pub const L2_10_INDEX: usize = 22;
/// See [`L6_INDEX`].
pub const L3_6_INDEX: usize = 25;
/// See [`L6_INDEX`].
pub const L2_12_INDEX: usize = 26;
/// See [`L6_INDEX`].
pub const L2_14_INDEX: usize = 29;

/// C `MiniGopStats` (`utility.h:174-179`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniGopStats {
    /// C `hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `start_index` into the pre-assignment buffer.
    pub start_index: u8,
    /// C `end_index` (inclusive).
    pub end_index: u8,
    /// C `length` (== `end_index - start_index + 1`).
    pub length: u8,
}

/// C `mini_gop_stats_array` (`utility.c:129-161`) — the 31 candidate mini-GOP
/// shapes a 32-picture pre-assignment buffer can be cut into.
pub(super) const MINI_GOP_STATS_ARRAY: [MiniGopStats; MINI_GOP_MAX_COUNT] = {
    const fn s(h: u8, a: u8, b: u8, l: u8) -> MiniGopStats {
        MiniGopStats {
            hierarchical_levels: h,
            start_index: a,
            end_index: b,
            length: l,
        }
    }
    [
        s(5, 0, 31, 32),
        s(4, 0, 15, 16),
        s(3, 0, 7, 8),
        s(2, 0, 3, 4),
        s(1, 0, 1, 2),
        s(1, 2, 3, 2),
        s(2, 4, 7, 4),
        s(1, 4, 5, 2),
        s(1, 6, 7, 2),
        s(3, 8, 15, 8),
        s(2, 8, 11, 4),
        s(1, 8, 9, 2),
        s(1, 10, 11, 2),
        s(2, 12, 15, 4),
        s(1, 12, 13, 2),
        s(1, 14, 15, 2),
        s(4, 16, 31, 16),
        s(3, 16, 23, 8),
        s(2, 16, 19, 4),
        s(1, 16, 17, 2),
        s(1, 18, 19, 2),
        s(2, 20, 23, 4),
        s(1, 20, 21, 2),
        s(1, 22, 23, 2),
        s(3, 24, 31, 8),
        s(2, 24, 27, 4),
        s(1, 24, 25, 2),
        s(1, 26, 27, 2),
        s(2, 28, 31, 4),
        s(1, 28, 29, 2),
        s(1, 30, 31, 2),
    ]
};

/// C `svt_aom_get_mini_gop_stats` (`utility.c:168-170`) — EXPORTED.
///
/// # Panics
///
/// Panics for `mini_gop_index >= MINI_GOP_MAX_COUNT`; C indexes the array
/// unchecked, so an out-of-range index is caller misuse in both.
#[must_use]
pub fn get_mini_gop_stats(mini_gop_index: usize) -> MiniGopStats {
    MINI_GOP_STATS_ARRAY[mini_gop_index]
}

/// C `g_prediction_structure_config_array[hier].entry_array[pos].temporal_layer_index`
/// (`pred_structure.c:77-462`, selected by `hierarchical_levels` at
/// `pred_structure.c:464-470`).
///
/// The temporal-layer column of C's six prediction-structure tables. The SAME
/// tables serve low delay and random access — for LOW_DELAY only this column
/// is read (the `decode_order` column reorders the random-access mini-GOP,
/// which the low-delay pipeline never does), and `pred_struct_position`
/// advances linearly through it.
pub const PRED_STRUCT_TEMPORAL_LAYER: [&[u8]; 6] = [
    // flat_pred_struct
    &[0],
    // two_level_hierarchical_pred_struct
    &[0, 1],
    // three_level_hierarchical_pred_struct
    &[0, 2, 1, 2],
    // four_level_hierarchical_pred_struct
    &[0, 3, 2, 3, 1, 3, 2, 3],
    // five_level_hierarchical_pred_struct
    &[0, 4, 3, 4, 2, 4, 3, 4, 1, 4, 3, 4, 2, 4, 3, 4],
    // six_level_hierarchical_pred_struct
    &[
        0, 5, 4, 5, 3, 5, 4, 5, 2, 5, 4, 5, 3, 5, 4, 5, 1, 5, 4, 5, 3, 5, 4, 5, 2, 5, 4, 5, 3, 5,
        4, 5,
    ],
];

/// C `g_prediction_structure_config_array[hier].entry_array[pos].decode_order`
/// (`pred_structure.c:77-462`), the second column of the same six tables.
///
/// For `RANDOM_ACCESS` this is the coded position of each picture inside its
/// mini-GOP: `pcs->decode_order = decode_base_number +
/// entry[pred_struct_index]->decode_order` (`pd_process.c:5637-5641`), applied
/// only while `mini_gop_length == pred_struct_entry_count` — an incomplete MG
/// keeps `decode_order == picture_number` instead. For `LOW_DELAY` the Ctor
/// overwrites every entry with its own index (`pred_structure.c:695`), so the
/// identity permutation here is already the correct low-delay value.
///
/// Verified against the tier-2 oracle on real C streams
/// (`tests/data/picstruct_ra/ra_hl*.obu`): the coded `order_hint` sequence of
/// an HL3 stream is 8,4,2,1,3,6,5,7 — exactly the sort of
/// `entry[i].decode_order` below.
pub const PRED_STRUCT_DECODE_ORDER: [&[u8]; 6] = [
    // flat_pred_struct
    &[0],
    // two_level_hierarchical_pred_struct
    &[0, 1],
    // three_level_hierarchical_pred_struct
    &[0, 2, 1, 3],
    // four_level_hierarchical_pred_struct
    &[0, 3, 2, 4, 1, 6, 5, 7],
    // five_level_hierarchical_pred_struct
    &[0, 4, 3, 5, 2, 7, 6, 8, 1, 11, 10, 12, 9, 14, 13, 15],
    // six_level_hierarchical_pred_struct
    &[
        0, 5, 4, 6, 3, 8, 7, 9, 2, 12, 11, 13, 10, 15, 14, 16, 1, 20, 19, 21, 18, 23, 22, 24, 17,
        27, 26, 28, 25, 30, 29, 31,
    ],
];

/// The `EncodeContext` fields the mini-GOP and pred-struct derivation reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EncCtxPicParams {
    /// C `enc_ctx->pre_assignment_buffer_count`.
    pub pre_assignment_buffer_count: u32,
    /// C `enc_ctx->pre_assignment_buffer_intra_count`.
    pub pre_assignment_buffer_intra_count: u32,
    /// C `enc_ctx->pre_assignment_buffer_idr_count`.
    pub pre_assignment_buffer_idr_count: u32,
    /// C `enc_ctx->previous_mini_gop_hierarchical_levels`.
    pub previous_mini_gop_hierarchical_levels: u32,
    /// C `enc_ctx->mini_gop_cnt_per_gop`.
    pub mini_gop_cnt_per_gop: u32,
    /// C `enc_ctx->pred_struct_position`.
    pub pred_struct_position: u32,
    /// C `enc_ctx->last_idr_picture`.
    pub last_idr_picture: u64,
    /// C `enc_ctx->elapsed_non_cra_count`.
    pub elapsed_non_cra_count: u32,
    /// C `enc_ctx->decode_base_number` — the decode-order base for the
    /// CURRENT mini-GOP; `+= mini_gop_length` after each MG
    /// (`pd_process.c:5660-5662`). Only random access reads it — the
    /// low-delay `decode_order = picture_number` path never does.
    pub decode_base_number: u64,
    /// C `enc_ctx->picture_number_alt` — the monotonic decode-order counter
    /// for pictures the RA permutation does NOT cover (cut-short mini-GOPs,
    /// IDR-carrying windows): `decode_order = picture_number_alt` there
    /// (`pd_process.c:5633-5644`). Incremented once per picture in the
    /// window's second pass whether or not the RA arm took it.
    pub picture_number_alt: u64,
}

/// The mini-GOP window state `set_mini_gop_structure` fills.
///
/// C keeps these as parallel arrays on `PictureDecisionContext`; kept here as
/// one struct so the whole window map moves together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniGopMap {
    /// C `ctx->mini_gop_activity_array[MINI_GOP_MAX_COUNT]`.
    pub activity: [bool; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_start_index[]`.
    pub start_index: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_end_index[]`.
    pub end_index: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_length[]`.
    pub length: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_hierarchical_levels[]`.
    pub hierarchical_levels: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_intra_count[]`.
    pub intra_count: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_idr_count[]`.
    pub idr_count: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->total_number_of_mini_gops`.
    pub total_number_of_mini_gops: usize,
    /// C `ctx->enable_startup_mg`.
    pub enable_startup_mg: bool,
    /// C `ctx->is_startup_gop`.
    pub is_startup_gop: bool,
    /// C `ctx->sframe_hier_lvls`.
    pub sframe_hier_lvls: i32,
    /// C `ctx->list0_only`.
    pub list0_only: bool,
}

impl Default for MiniGopMap {
    fn default() -> Self {
        Self {
            activity: [false; MINI_GOP_MAX_COUNT],
            start_index: [0; MINI_GOP_MAX_COUNT],
            end_index: [0; MINI_GOP_MAX_COUNT],
            length: [0; MINI_GOP_MAX_COUNT],
            hierarchical_levels: [0; MINI_GOP_MAX_COUNT],
            intra_count: [0; MINI_GOP_MAX_COUNT],
            idr_count: [0; MINI_GOP_MAX_COUNT],
            total_number_of_mini_gops: 0,
            enable_startup_mg: false,
            is_startup_gop: false,
            // C's picture_decision_context_ctor sets 0 here
            // (pd_process.c:249) -- but that value is DEAD: the kernel
            // overwrites it with the configured hierarchy at picture_number 0
            // (pd_process.c:5407-5409) before set_mini_gop_structure ever
            // reads it. Use `MiniGopMap::for_sequence` so the live value is
            // the one in play; the ctor value is reproduced here only so a
            // reader who greps the C ctor finds the same number.
            sframe_hier_lvls: 0,
            list0_only: false,
        }
    }
}

impl MiniGopMap {
    /// The state as of `picture_number == 0`, where the kernel initialises
    /// `sframe_hier_lvls` from the configured hierarchy
    /// (`pd_process.c:5407-5409`).
    ///
    /// Using [`Default`] instead leaves `sframe_hier_lvls` at C's *ctor* value
    /// of 0, which makes [`set_mini_gop_structure`]'s S-frame override fire on
    /// every non-zero configured hierarchy and collapse the mini-GOP to level
    /// 0. That is a real trap: the ctor line is the one a grep finds first and
    /// it is not the value the code runs with.
    #[must_use]
    pub fn for_sequence(config_hierarchical_levels: u8) -> Self {
        Self {
            sframe_hier_lvls: i32::from(config_hierarchical_levels),
            ..Self::default()
        }
    }
}

/// C `initialize_mini_gop_activity_array` (`pd_process.c:759-848`) — static.
///
/// Chooses which mini-GOP shapes are permitted for the current pre-assignment
/// buffer count. `activity[i] == true` means "this shape is still a candidate
/// and must be subdivided"; the nested cascade clears the flag on the LARGEST
/// shape that fits, then recurses into the remainder.
///
/// Trap: every arm is `count >= N && !(count == N && idr_flag)`. The IDR guard
/// means a buffer holding EXACTLY N pictures whose first is an IDR does NOT
/// take the N-picture shape — an off-by-one that only shows up at a GOP
/// boundary, which is the case a short test cell hits first.
///
/// Returns `true` when the caller must run the dynamic-GOP 6L-vs-5L split
/// (`eval_sub_mini_gop`), which is NOT ported: it needs `early_hme` /
/// `calc_mini_gop_activity`, a different chunk. Measured: `scs->enable_dg` is
/// 1 for single-pass CQP/CRF `RANDOM_ACCESS` below 4K
/// (`enc_handle.c:4294-4300`), so this is on by default there — the caller
/// must not treat a `true` return as an exotic case.
pub fn initialize_mini_gop_activity_array(
    map: &mut MiniGopMap,
    enc: &EncCtxPicParams,
    idr_flag: bool,
    enable_dg: bool,
    list0_only_base: bool,
) -> bool {
    for gopindex in 0..MINI_GOP_MAX_COUNT {
        map.activity[gopindex] =
            get_mini_gop_stats(gopindex).hierarchical_levels > MIN_HIERARCHICAL_LEVEL;
    }

    let n = enc.pre_assignment_buffer_count;
    // `fits(k)` is C's `count >= k && !(count == k && idr_flag)`.
    let fits = |count: u32, k: u32| count >= k && !(count == k && idr_flag);

    if fits(n, 32) {
        map.activity[L6_INDEX] = false;
    } else if fits(n, 16) {
        map.activity[L5_0_INDEX] = false;
        if fits(n - 16, 8) {
            map.activity[L4_2_INDEX] = false;
            if fits(n - 16 - 8, 4) {
                map.activity[L3_6_INDEX] = false;
                if fits(n - 16 - 8 - 4, 2) {
                    map.activity[L2_14_INDEX] = false;
                }
            } else if fits(n - 16 - 8, 2) {
                map.activity[L2_12_INDEX] = false;
            }
        } else if fits(n - 16, 4) {
            map.activity[L3_4_INDEX] = false;
            if fits(n - 16 - 4, 2) {
                map.activity[L2_10_INDEX] = false;
            }
        } else if fits(n - 16, 2) {
            map.activity[L2_8_INDEX] = false;
        }
    } else if fits(n, 8) {
        map.activity[L4_0_INDEX] = false;
        if fits(n - 8, 4) {
            map.activity[L3_2_INDEX] = false;
            if fits(n - 8 - 4, 2) {
                map.activity[L2_6_INDEX] = false;
            }
        } else if fits(n - 8, 2) {
            map.activity[L2_4_INDEX] = false;
        }
    } else if fits(n, 4) {
        map.activity[L3_0_INDEX] = false;
        if fits(n - 4, 2) {
            map.activity[L2_2_INDEX] = false;
        }
    } else if fits(n, 2) {
        map.activity[L2_0_INDEX] = false;
    }

    map.list0_only = list0_only_base;

    // 6L vs 5L: C calls eval_sub_mini_gop here.
    enable_dg && !map.activity[L6_INDEX]
}

/// C `generate_picture_window_split` (`pd_process.c:857-891`) — static.
///
/// Turns the activity array into the concrete list of mini-GOPs. The loop's
/// STRIDE is the subtle part: an ACTIVE (still-to-subdivide) shape advances by
/// 1 so its children are visited, while an INACTIVE (chosen) shape skips its
/// whole subtree via `mini_gop_offset[levels - MIN_HIERARCHICAL_LEVEL]`.
pub fn generate_picture_window_split(map: &mut MiniGopMap, enc: &EncCtxPicParams) {
    map.total_number_of_mini_gops = 0;
    let mut gopindex = 0usize;
    while gopindex < MINI_GOP_MAX_COUNT {
        let stats = get_mini_gop_stats(gopindex);
        if u32::from(stats.end_index) < enc.pre_assignment_buffer_count && !map.activity[gopindex] {
            let t = map.total_number_of_mini_gops;
            map.start_index[t] = u32::from(stats.start_index);
            map.end_index[t] = u32::from(stats.end_index);
            map.length[t] = u32::from(stats.length);
            map.hierarchical_levels[t] = u32::from(stats.hierarchical_levels);
            map.intra_count[t] = 0;
            map.idr_count[t] = 0;
            map.total_number_of_mini_gops += 1;
        }
        gopindex += if map.activity[gopindex] {
            1
        } else {
            usize::from(
                MINI_GOP_OFFSET[(stats.hierarchical_levels - MIN_HIERARCHICAL_LEVEL) as usize],
            )
        };
    }
    if map.total_number_of_mini_gops != 0 {
        let last = map.total_number_of_mini_gops - 1;
        map.intra_count[last] = enc.pre_assignment_buffer_intra_count;
        map.idr_count[last] = enc.pre_assignment_buffer_idr_count;
    }
}

/// C `handle_incomplete_picture_window_map` (`pd_process.c:892-927`) — static.
///
/// Fixes up the last, short mini-GOP at a GOP boundary — exactly the
/// end-of-sequence case a 2- or 5-frame test cell hits first.
pub fn handle_incomplete_picture_window_map(
    hierarchical_level: u32,
    map: &mut MiniGopMap,
    enc: &EncCtxPicParams,
) {
    if map.total_number_of_mini_gops == 0 {
        let hier = hierarchical_level.min(u32::from(MIN_HIERARCHICAL_LEVEL));
        let t = map.total_number_of_mini_gops;
        map.start_index[t] = 0;
        map.end_index[t] = enc.pre_assignment_buffer_count - 1;
        map.length[t] = enc.pre_assignment_buffer_count - map.start_index[t];
        map.hierarchical_levels[t] = hier;
        map.total_number_of_mini_gops += 1;
    } else if map.end_index[map.total_number_of_mini_gops - 1] < enc.pre_assignment_buffer_count - 1
    {
        let t = map.total_number_of_mini_gops;
        map.start_index[t] = map.end_index[t - 1] + 1;
        map.end_index[t] = enc.pre_assignment_buffer_count - 1;
        map.length[t] = enc.pre_assignment_buffer_count - map.start_index[t];
        map.hierarchical_levels[t] = u32::from(MIN_HIERARCHICAL_LEVEL);
        // C zeroes the PREVIOUS entry's counts here, then writes the buffer
        // totals into the NEW last entry two lines later.
        map.intra_count[t - 1] = 0;
        map.idr_count[t - 1] = 0;
        map.total_number_of_mini_gops += 1;
    }
    let last = map.total_number_of_mini_gops - 1;
    map.intra_count[last] = enc.pre_assignment_buffer_intra_count;
    map.idr_count[last] = enc.pre_assignment_buffer_idr_count;
}

/// C `set_mini_gop_structure` (`pd_process.c:4720-4768`) — static.
///
/// Sets the single default mini-GOP covering the whole pre-assignment buffer,
/// then subdivides it (activity array -> window split -> incomplete fixup)
/// only when the buffer holds more than one picture, or holds no intra picture
/// in random access. In low delay `pre_assignment_buffer_count` is 1, so the
/// subdivision never runs and the default MG stands — that is why this
/// function "degenerates" in the campaign's first cell rather than being
/// irrelevant to it.
///
/// Returns `true` when the caller must run the un-ported dynamic-GOP split;
/// see [`initialize_mini_gop_activity_array`].
#[allow(clippy::too_many_arguments)]
pub fn set_mini_gop_structure(
    map: &mut MiniGopMap,
    enc: &mut EncCtxPicParams,
    seq: &SeqPicParams,
    pic: &PicParams,
    config_hierarchical_levels: u32,
    startup_mg_size: u32,
    idr_flag: bool,
    enable_dg: bool,
    list0_only_base: bool,
) -> bool {
    let mut next_mg_hierarchical_levels = config_hierarchical_levels;
    // S-frame mini-GOP size override.
    if map.sframe_hier_lvls != config_hierarchical_levels as i32 {
        next_mg_hierarchical_levels = map.sframe_hier_lvls as u32;
    }
    if map.enable_startup_mg {
        next_mg_hierarchical_levels = startup_mg_size;
    }
    // RTC (implies LOW_DELAY + CBR) supports on-the-fly hierarchy changes.
    if seq.pred_structure == PredStructure::LowDelay
        && seq.rtc
        && seq.rate_control_mode == RcMode::Cbr
    {
        next_mg_hierarchical_levels = u32::from(pic.hierarchical_levels);
    }

    map.start_index[0] = 0;
    map.end_index[0] = enc.pre_assignment_buffer_count - 1;
    map.length[0] = enc.pre_assignment_buffer_count;
    map.hierarchical_levels[0] = next_mg_hierarchical_levels;
    map.intra_count[0] = enc.pre_assignment_buffer_intra_count;
    map.idr_count[0] = enc.pre_assignment_buffer_idr_count;
    map.total_number_of_mini_gops = 1;

    enc.previous_mini_gop_hierarchical_levels = if pic.picture_number == 0 {
        next_mg_hierarchical_levels
    } else {
        enc.previous_mini_gop_hierarchical_levels
    };
    enc.mini_gop_cnt_per_gop = if enc.pre_assignment_buffer_idr_count != 0 {
        0
    } else {
        enc.mini_gop_cnt_per_gop + 1
    };

    let mut needs_dg = false;
    if enc.pre_assignment_buffer_count > 1
        || (enc.pre_assignment_buffer_intra_count == 0
            && seq.pred_structure == PredStructure::RandomAccess)
    {
        needs_dg =
            initialize_mini_gop_activity_array(map, enc, idr_flag, enable_dg, list0_only_base);
        generate_picture_window_split(map, enc);
        handle_incomplete_picture_window_map(next_mg_hierarchical_levels, map, enc);
    }
    needs_dg
}

/// One picture's slice of `get_pred_struct_for_all_frames`
/// (`pd_process.c:942-988`) — static.
///
/// Sets `pred_structure`, `hierarchical_levels` and `is_startup_gop`.
/// Everything downstream (RPS branch selection, layer depth, TF params) keys
/// off these.
///
/// Trap: an IDR takes the SEQUENCE's `hierarchical_levels`, every other
/// picture takes the MINI-GOP's. A port that used one value for the whole
/// buffer picks the wrong RPS branch on the frame after every key frame.
///
/// C also assigns `pcs->pred_struct_ptr` from the prediction-structure group;
/// that table lives in `pred_structure.c` and is not this module's.
pub fn get_pred_struct_for_frame(
    pic: &mut PicParams,
    map: &mut MiniGopMap,
    mini_gop_index: usize,
    seq_pred_structure: PredStructure,
    config_hierarchical_levels: u8,
    startup_mg_size: u32,
    idr_flag: bool,
    cra_flag: bool,
) {
    pic.pred_struct_type = seq_pred_structure;
    pic.hierarchical_levels = if idr_flag {
        config_hierarchical_levels
    } else {
        map.hierarchical_levels[mini_gop_index] as u8
    };

    if startup_mg_size != 0 {
        if idr_flag || cra_flag {
            map.enable_startup_mg = true;
        } else if map.enable_startup_mg {
            map.enable_startup_mg = false;
        }
    }
    if idr_flag && pic.picture_number == 0 {
        map.is_startup_gop = true;
    } else if idr_flag || cra_flag {
        map.is_startup_gop = false;
    }
}

/// C `is_pic_cutting_short_ra_mg` (`pd_process.c:928-941`) — EXPORTED.
///
/// Detects a random-access mini-GOP cut short by an intra picture, which
/// switches the picture to the LOW_DELAY prediction structure mid-stream.
#[must_use]
pub fn is_pic_cutting_short_ra_mg(
    map: &MiniGopMap,
    pic: &PicParams,
    mg_idx: usize,
    idr_flag: bool,
    cra_flag: bool,
) -> bool {
    (map.length[mg_idx] < pic.pred_struct_entry_count || map.idr_count[mg_idx] > 0)
        && pic.pred_struct_type == PredStructure::RandomAccess
        && !idr_flag
        && !cra_flag
}

/// C `svt_aom_is_delayed_intra` (`pd_process.c:3620-3635`) — EXPORTED.
///
/// Whether an intra picture is held back to join the next mini-GOP. Selects
/// `tf_params_per_type[0]` in `copy_tf_params` and gates the delayed-intra
/// handling in the picture-decision sequence.
#[must_use]
pub fn is_delayed_intra(
    idr_flag: bool,
    cra_flag: bool,
    pred_structure: PredStructure,
    intra_period_length: i32,
    end_of_sequence_flag: bool,
    pre_assignment_buffer_count: u32,
    pred_struct_entry_count: u32,
) -> bool {
    if (idr_flag || cra_flag) && pred_structure == PredStructure::RandomAccess {
        if intra_period_length == 0 || end_of_sequence_flag {
            false
        } else {
            idr_flag || (cra_flag && pre_assignment_buffer_count < pred_struct_entry_count)
        }
    } else {
        false
    }
}

/// C `search_this_pic` (`pd_process.c:3606-3619`) — EXPORTED.
///
/// Locates a picture by POC in a picture buffer; returns -1 when absent. This
/// is the lookup `derive_tf_window_params` uses to assemble its window.
#[must_use]
pub fn search_this_pic(buf: &[u64], input_pic: u64) -> i32 {
    for (i, &poc) in buf.iter().enumerate() {
        if poc == input_pic {
            return i as i32;
        }
    }
    -1
}

/// C `avail_past_pictures` (`pd_process.c:3592-3605`) — static.
///
/// Counts how many pictures in the buffer precede `input_pic`, which caps the
/// temporal-filter window size at the start of a sequence.
#[must_use]
pub fn avail_past_pictures(buf: &[u64], input_pic: u64) -> i32 {
    buf.iter().filter(|&&poc| poc < input_pic).count() as i32
}

/// C `perform_sc_detection`'s INHERITANCE half (`pd_process.c:4769-4813`) —
/// static.
///
/// Inter frames do NOT run screen-content detection: they inherit
/// `sc_class0..5` and `is_luma_dominant_input` from the last I picture. An I
/// picture runs detection (single-threaded mode only) and then publishes its
/// classes into the context.
///
/// Only the inheritance and publication are ported here; the detection calls
/// themselves (`svt_aom_is_screen_content`,
/// `svt_aom_is_screen_content_antialiasing_aware`,
/// `svt_aom_is_input_luma_dominant`) live in the screen-content module. The
/// caller passes the freshly detected classes for an I picture.
///
/// This matters because without it a port re-detects per frame and flips
/// palette / IntraBC / SC-tuned thresholds mid-GOP.
pub fn perform_sc_detection(
    is_i_slice: bool,
    detected: ScClasses,
    last_i: &mut ScClasses,
) -> ScClasses {
    if is_i_slice {
        *last_i = detected;
        detected
    } else {
        *last_i
    }
}

/// The screen-content classification a picture carries
/// (`pcs->sc_class0..5` + `pcs->is_luma_dominant_input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScClasses {
    /// C `pcs->sc_class0..5`.
    pub class: [u8; 6],
    /// C `pcs->is_luma_dominant_input`.
    pub is_luma_dominant_input: bool,
}

/// C `store_mg_picture_arrays` (`pd_process.c:4966-4985`) — static.
///
/// Sorts the mini-GOP into DECODE order and keeps a display-order copy. It
/// literally determines the order frames are coded into the bitstream in
/// random access.
///
/// Trap: C's inner swap writes `mg_pics[i] = ctx->mg_pictures_array[j]` — the
/// SAME array `mg_pics` aliases, so it is an ordinary selection sort and not,
/// as the two spellings suggest, a copy from a second array. Reproduced as a
/// stable-by-construction selection sort on `decode_order`.
///
/// `decode_orders[k]` is the `decode_order` of the k-th picture **in display
/// order** (C's incoming `ctx->mg_pictures_array`). Returns
/// `(decode_order_permutation, display_order_permutation)` as index lists into
/// that same input.
#[must_use]
pub fn store_mg_picture_arrays(
    decode_orders: &[u64],
) -> (alloc::vec::Vec<usize>, alloc::vec::Vec<usize>) {
    let n = decode_orders.len();
    let display: alloc::vec::Vec<usize> = (0..n).collect();
    let mut decode: alloc::vec::Vec<usize> = (0..n).collect();
    for i in 0..n.saturating_sub(1) {
        for j in (i + 1)..n {
            if decode_orders[decode[j]] < decode_orders[decode[i]] {
                decode.swap(i, j);
            }
        }
    }
    (decode, display)
}

/// C `get_pic_idx_in_mg` (`pd_process.c:4872-4893`) — static.
///
/// Produces the RPS branch selector (`pic_idx_in_mg`) and, in low delay, also
/// writes `pcs->frame_offset` — the `set_frame_update_type` selector. Two
/// distinct downstream decisions ride on this one call.
///
/// Trap: in low delay `pic_idx_in_mg` is `(pred_struct_position - 1) %
/// entry_count`, with a special case for position 0 — NOT the position
/// itself. `frame_offset` is the distance to the last IDR, which is a
/// different quantity from `pic_idx_in_mg` and is written even when the
/// S-frame branch overrides the index.
///
/// The `IS_SFRAME_FLEXIBLE_INSERT` override is not ported (S-frames are
/// outside the port's envelope) and is named here rather than dropped.
pub fn get_pic_idx_in_mg(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    enc: &EncCtxPicParams,
    map: &MiniGopMap,
    pic_idx: u32,
    mini_gop_index: usize,
) -> u32 {
    match seq.pred_structure {
        PredStructure::RandomAccess => pic_idx - map.start_index[mini_gop_index],
        PredStructure::LowDelay => {
            let mg_pos = u64::from(enc.pred_struct_position);
            let idx = if mg_pos == 0 {
                0
            } else {
                ((mg_pos - 1) % u64::from(pic.pred_struct_entry_count)) as u32
            };
            pic.frame_offset = pic.picture_number - enc.last_idr_picture;
            idx
        }
        PredStructure::AllIntra => 0,
    }
}

/// C `update_pred_struct_and_pic_type` (`pd_process.c:4814-4871`) — static.
///
/// Walks `enc_ctx->pred_struct_position` and picks the slice type. A wrong
/// position means the wrong prediction-structure entry, i.e. the wrong
/// temporal layer for the frame.
///
/// Returns the derived [`SliceType`]. Sets `pic.pred_struct_type` to
/// `LOW_DELAY` and `map.cut_short_ra_mg` when the mini-GOP is cut short (C
/// re-fetches the LOW_DELAY prediction structure at that point).
///
/// Trap: the position rules are an if/else CHAIN with a specific priority —
/// mini-GOP switch, then IDR, then CRA-with-short-MG, then
/// "directly after a CRA" (`elapsed_non_cra_count == 0`, which sets
/// `init_pic_index + 1`, not `init_pic_index`), and only then the ordinary
/// increment. Reordering any two arms changes the position on a real frame.
#[allow(clippy::too_many_arguments)]
pub fn update_pred_struct_and_pic_type(
    pic: &mut PicParams,
    enc: &mut EncCtxPicParams,
    map: &mut MiniGopMap,
    ctx: &mut PicDecisionCtx,
    mini_gop_index: usize,
    pre_assignment_buffer_first_pass_flag: bool,
    idr_flag: bool,
    cra_flag: bool,
    init_pred_struct_position_flag: bool,
    init_pic_index: u32,
) -> SliceType {
    let picture_type;
    if is_pic_cutting_short_ra_mg(map, pic, mini_gop_index, idr_flag, cra_flag) {
        // Correct the pred index before switching structures.
        if pre_assignment_buffer_first_pass_flag {
            enc.pred_struct_position -= init_pic_index;
        }
        pic.pred_struct_type = PredStructure::LowDelay;
        picture_type = SliceType::B;
        ctx.cut_short_ra_mg = 1;
    } else {
        picture_type = if idr_flag || cra_flag {
            SliceType::I
        } else {
            SliceType::B
        };
    }

    if init_pred_struct_position_flag {
        enc.pred_struct_position = init_pic_index;
    }

    // The first two arms assign the same value in C too; they are kept
    // separate (rather than or-ed) because their GUARDS differ and the chain's
    // priority is the load-bearing part.
    #[allow(clippy::if_same_then_else)]
    if idr_flag {
        enc.pred_struct_position = init_pic_index;
    } else if cra_flag && map.length[mini_gop_index] < pic.pred_struct_entry_count {
        enc.pred_struct_position = init_pic_index;
    } else if enc.elapsed_non_cra_count == 0 {
        // Directly after a CRA: skip the entry that would violate it.
        enc.pred_struct_position = init_pic_index + 1;
    } else {
        enc.pred_struct_position += 1;
    }

    if idr_flag {
        enc.last_idr_picture = pic.picture_number;
    }

    if enc.pred_struct_position == pic.pred_struct_entry_count {
        enc.pred_struct_position -= pic.pred_struct_entry_count;
    }
    picture_type
}

// ---------------------------------------------------------------------------
// TPL group selection — Codec/initial_rc_process.c:161-526
// ---------------------------------------------------------------------------
//
// Measured reachability (`enc_handle.c:3657-3668`): `get_tpl` returns 0 for
// LOW_DELAY, for allintra and for `aq_mode == 0`, so everything in this
// section is DEAD for the campaign's first cell and LIVE for the default
// random-access video config. TPL sets `r0` and the per-SB qindex offsets in
// random access, so no RA frame is byte-identical without it — the port keeps
// it translated per `docs/WORKING-ON-THIS.md` §7 rather than dropping it
// because the first cell does not reach it.

/// C `TplControls` (`pcs.h:459-486`) — the subset `svt_aom_set_tpl_group` and
/// `set_tpl_params` write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TplControls {
    /// C `enable` — 0: TPL off.
    pub enable: u8,
    /// C `compute_rate`.
    pub compute_rate: u8,
    /// C `enable_tpl_qps`.
    pub enable_tpl_qps: u8,
    /// C `disable_intra_pred_nref`.
    pub disable_intra_pred_nref: u8,
    /// C `intra_mode_end` (`PredictionMode`; `DC_PRED` = 0, `PAETH_PRED` = 12).
    pub intra_mode_end: u8,
    /// C `pf_shape` (`TxCoeffShape`; `DEFAULT_SHAPE` 0, `N2_SHAPE` 1, `N4_SHAPE` 2).
    pub pf_shape: u8,
    /// C `use_sad_in_src_search`.
    pub use_sad_in_src_search: u8,
    /// C `reduced_tpl_group` — the temporal-layer cutoff, -1 for "all".
    pub reduced_tpl_group: i8,
    /// C `r0_adjust_factor`.
    pub r0_adjust_factor: f64,
    /// C `dispenser_search_level`.
    pub dispenser_search_level: u8,
    /// C `subsample_tx`.
    pub subsample_tx: u8,
    /// C `synth_blk_size`.
    pub synth_blk_size: u8,
    /// C `subpel_depth` (`SUBPEL_FORCE_STOP`; `EIGHTH_PEL` 0, `QUARTER_PEL` 1,
    /// `HALF_PEL` 2, `FULL_PEL` 3).
    pub subpel_depth: u8,
    /// C `subpel_diag_refinement`.
    pub subpel_diag_refinement: u8,
}

impl Default for TplControls {
    /// C initialises the struct with `TplControls tpl_ctrls_struct = {0}`,
    /// so every unwritten field is zero — including `reduced_tpl_group`, whose
    /// zero means "temporal layer 0 only", NOT "all frames" (-1). The level-0
    /// arm writes only `enable`, so a level-0 `TplControls` really does carry
    /// `reduced_tpl_group == 0`.
    fn default() -> Self {
        Self {
            enable: 0,
            compute_rate: 0,
            enable_tpl_qps: 0,
            disable_intra_pred_nref: 0,
            intra_mode_end: 0,
            pf_shape: 0,
            use_sad_in_src_search: 0,
            reduced_tpl_group: 0,
            r0_adjust_factor: 0.0,
            dispenser_search_level: 0,
            subsample_tx: 0,
            synth_blk_size: 0,
            subpel_depth: 0,
            subpel_diag_refinement: 0,
        }
    }
}

/// C `DC_PRED`.
pub const DC_PRED: u8 = 0;
/// C `PAETH_PRED`.
pub const PAETH_PRED: u8 = 12;
/// C `DEFAULT_SHAPE` / `N2_SHAPE` / `N4_SHAPE` (`definitions.h:2062-2064`).
pub const DEFAULT_SHAPE: u8 = 0;
/// See [`DEFAULT_SHAPE`].
pub const N2_SHAPE: u8 = 1;
/// See [`DEFAULT_SHAPE`].
pub const N4_SHAPE: u8 = 2;
/// C `SUBPEL_FORCE_STOP` (`definitions.h:868`).
pub const EIGHTH_PEL: u8 = 0;
/// See [`EIGHTH_PEL`].
pub const QUARTER_PEL: u8 = 1;
/// See [`EIGHTH_PEL`].
pub const HALF_PEL: u8 = 2;
/// See [`EIGHTH_PEL`].
pub const FULL_PEL: u8 = 3;
/// C `INPUT_SIZE_480p_RANGE` (`definitions.h:1826`).
pub const INPUT_SIZE_480P_RANGE: u8 = 2;

/// The sequence/picture inputs `svt_aom_set_tpl_group` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TplPicParams {
    /// C `pcs->slice_type` — used as a C truthiness test (`I_SLICE` == 1 is
    /// TRUE, `B_SLICE` == 0 is FALSE), which reads backwards from its name.
    pub slice_type: SliceType,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->scs->input_resolution` (`ResolutionRange`).
    pub input_resolution: u8,
    /// C `pcs->scs->tpl_lad_mg`.
    pub tpl_lad_mg: u8,
    /// C `pcs->scs->static_config.rate_control_mode`.
    pub rate_control_mode: RcMode,
}

/// C `svt_aom_get_tpl_group_level` (`initial_rc_process.c:190-202`) — EXPORTED.
///
/// Maps `(scs->tpl, enc_mode)` to the TPL group level.
#[must_use]
pub fn get_tpl_group_level(tpl: u8, enc_mode: i8) -> u8 {
    const ENC_M5: i8 = 5;
    const ENC_M8: i8 = 8;
    if tpl == 0 {
        0
    } else if enc_mode <= ENC_M5 {
        1
    } else if enc_mode <= ENC_M8 {
        3
    } else {
        4
    }
}

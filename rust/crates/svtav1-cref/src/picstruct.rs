//! FFI bindings for the picture-decision reference-structure oracle
//! (`Codec/pd_process.c`).
//!
//! Backed by `shims/picstruct_shims.c`, which drives the REAL exported C
//! symbols `svt_aom_is_pic_used_as_ref`, `svt_aom_get_gm_needed_resolutions`,
//! `svt_aom_is_incomp_mg_frame`, `update_count_try` and
//! `svt_av1_setup_skip_mode_allowed` — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with a concurrent lane.

unsafe extern "C" {
    fn ref_is_pic_used_as_ref(
        hierarchical_levels: u32,
        temporal_layer: u32,
        picture_index: u32,
        referencing_scheme: u32,
        is_overlay: i32,
    ) -> i32;

    fn ref_get_gm_needed_resolutions(
        ds_lvl: u8,
        full: *mut i32,
        quart: *mut i32,
        sixteen: *mut i32,
    );

    fn ref_is_incomp_mg_frame(pic_pred_type: u8, seq_pred_structure: u8) -> i32;

    #[allow(clippy::too_many_arguments)]
    fn ref_update_count_try(
        frame_type: u8,
        update_type: u8,
        list0_count: u8,
        list1_count: u8,
        base_l0: u8,
        base_l1: u8,
        nonbase_l0: u8,
        nonbase_l1: u8,
        out_l0_try: *mut u8,
        out_l1_try: *mut u8,
    );

    #[allow(clippy::too_many_arguments)]
    fn ref_setup_skip_mode_allowed(
        enable_order_hint: i32,
        order_hint_bits: u8,
        slice_type: u8,
        reference_mode: u8,
        ref_order_hint: *const u32,
        cur_order_hint: u32,
        out_allowed: *mut i32,
        out_idx0: *mut i32,
        out_idx1: *mut i32,
    );
}

/// C `svt_aom_is_pic_used_as_ref` (`pd_process.c:1770-1803`).
#[must_use]
pub fn is_pic_used_as_ref(
    hierarchical_levels: u32,
    temporal_layer: u32,
    picture_index: u32,
    referencing_scheme: u32,
    is_overlay: bool,
) -> bool {
    unsafe {
        ref_is_pic_used_as_ref(
            hierarchical_levels,
            temporal_layer,
            picture_index,
            referencing_scheme,
            i32::from(is_overlay),
        ) != 0
    }
}

/// C `svt_aom_get_gm_needed_resolutions` (`pd_process.c:990-994`).
///
/// Returns `(full, quarter, sixteenth)`.
#[must_use]
pub fn get_gm_needed_resolutions(ds_lvl: u8) -> (bool, bool, bool) {
    let (mut f, mut q, mut s) = (0i32, 0i32, 0i32);
    unsafe { ref_get_gm_needed_resolutions(ds_lvl, &mut f, &mut q, &mut s) };
    (f != 0, q != 0, s != 0)
}

/// C `svt_aom_is_incomp_mg_frame` (`pd_process.c:4986-4989`).
///
/// `pic_pred_type` / `seq_pred_structure` are `PredStructure`
/// (`ALL_INTRA`=0, `LOW_DELAY`=1, `RANDOM_ACCESS`=2).
#[must_use]
pub fn is_incomp_mg_frame(pic_pred_type: u8, seq_pred_structure: u8) -> bool {
    unsafe { ref_is_incomp_mg_frame(pic_pred_type, seq_pred_structure) != 0 }
}

/// C `update_count_try` (`pd_process.c:4507-4517`).
///
/// Returns `(ref_list0_count_try, ref_list1_count_try)`. `frame_type` is
/// `FrameType` (`KEY_FRAME`=0, `INTER_FRAME`=1, `INTRA_ONLY_FRAME`=2,
/// `S_FRAME`=3) and `update_type` is `SvtAv1FrameUpdateType`.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn update_count_try(
    frame_type: u8,
    update_type: u8,
    list0_count: u8,
    list1_count: u8,
    base_l0: u8,
    base_l1: u8,
    nonbase_l0: u8,
    nonbase_l1: u8,
) -> (u8, u8) {
    let (mut l0, mut l1) = (0u8, 0u8);
    unsafe {
        ref_update_count_try(
            frame_type,
            update_type,
            list0_count,
            list1_count,
            base_l0,
            base_l1,
            nonbase_l0,
            nonbase_l1,
            &mut l0,
            &mut l1,
        );
    }
    (l0, l1)
}

/// C `svt_av1_setup_skip_mode_allowed` (`pd_process.c:102-166`).
///
/// Returns `(skip_mode_allowed, ref_frame_idx_0, ref_frame_idx_1)`.
#[must_use]
pub fn setup_skip_mode_allowed(
    enable_order_hint: bool,
    order_hint_bits: u8,
    slice_type: u8,
    reference_mode: u8,
    ref_order_hint: &[u32; 7],
    cur_order_hint: u32,
) -> (i32, i32, i32) {
    let (mut a, mut i0, mut i1) = (0i32, 0i32, 0i32);
    unsafe {
        ref_setup_skip_mode_allowed(
            i32::from(enable_order_hint),
            order_hint_bits,
            slice_type,
            reference_mode,
            ref_order_hint.as_ptr(),
            cur_order_hint,
            &mut a,
            &mut i0,
            &mut i1,
        );
    }
    (a, i0, i1)
}

unsafe extern "C" {
    fn ref_get_mini_gop_stats(
        index: u32,
        hier: *mut u8,
        start: *mut u8,
        end: *mut u8,
        len: *mut u8,
    );

    fn ref_is_pic_cutting_short_ra_mg(
        mg_len: u32,
        mg_idr_count: u32,
        entry_count: u32,
        pic_pred_type: u8,
        idr_flag: i32,
        cra_flag: i32,
    ) -> i32;

    #[allow(clippy::too_many_arguments)]
    fn ref_is_delayed_intra(
        idr_flag: i32,
        cra_flag: i32,
        pred_structure: u8,
        intra_period_length: i32,
        end_of_sequence_flag: i32,
        pre_assignment_buffer_count: u32,
        pred_struct_entry_count: u32,
    ) -> i32;

    fn ref_search_this_pic(pocs: *const u64, buf_size: u32, input_pic: u64) -> i32;
}

/// C `svt_aom_get_mini_gop_stats` (`utility.c:168-170`).
///
/// Returns `(hierarchical_levels, start_index, end_index, length)`.
#[must_use]
pub fn get_mini_gop_stats(index: u32) -> (u8, u8, u8, u8) {
    let (mut h, mut s, mut e, mut l) = (0u8, 0u8, 0u8, 0u8);
    unsafe { ref_get_mini_gop_stats(index, &mut h, &mut s, &mut e, &mut l) };
    (h, s, e, l)
}

/// C `is_pic_cutting_short_ra_mg` (`pd_process.c:928-941`).
#[must_use]
pub fn is_pic_cutting_short_ra_mg(
    mg_len: u32,
    mg_idr_count: u32,
    entry_count: u32,
    pic_pred_type: u8,
    idr_flag: bool,
    cra_flag: bool,
) -> bool {
    unsafe {
        ref_is_pic_cutting_short_ra_mg(
            mg_len,
            mg_idr_count,
            entry_count,
            pic_pred_type,
            i32::from(idr_flag),
            i32::from(cra_flag),
        ) != 0
    }
}

/// C `svt_aom_is_delayed_intra` (`pd_process.c:3620-3635`).
#[must_use]
pub fn is_delayed_intra(
    idr_flag: bool,
    cra_flag: bool,
    pred_structure: u8,
    intra_period_length: i32,
    end_of_sequence_flag: bool,
    pre_assignment_buffer_count: u32,
    pred_struct_entry_count: u32,
) -> bool {
    unsafe {
        ref_is_delayed_intra(
            i32::from(idr_flag),
            i32::from(cra_flag),
            pred_structure,
            intra_period_length,
            i32::from(end_of_sequence_flag),
            pre_assignment_buffer_count,
            pred_struct_entry_count,
        ) != 0
    }
}

/// C `search_this_pic` (`pd_process.c:3606-3619`).
#[must_use]
pub fn search_this_pic(pocs: &[u64], input_pic: u64) -> i32 {
    unsafe { ref_search_this_pic(pocs.as_ptr(), pocs.len() as u32, input_pic) }
}

unsafe extern "C" {
    fn ref_get_tpl_group_level(tpl: u8, enc_mode: i8) -> u8;

    #[allow(clippy::too_many_arguments)]
    fn ref_set_tpl_group(
        pcs_present: i32,
        slice_type: u8,
        hierarchical_levels: u8,
        input_resolution: u8,
        tpl_lad_mg: u8,
        rate_control_mode: u8,
        tpl_group_level: u8,
        source_width: u32,
        source_height: u32,
        out_enable: *mut u8,
        out_reduced: *mut i8,
        out_synth: *mut u8,
        out_r0_adjust: *mut f64,
    ) -> u8;
}

/// C `svt_aom_get_tpl_group_level` (`initial_rc_process.c:190-202`).
#[must_use]
pub fn get_tpl_group_level(tpl: u8, enc_mode: i8) -> u8 {
    unsafe { ref_get_tpl_group_level(tpl, enc_mode) }
}

/// The observable outputs of `svt_aom_set_tpl_group`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TplGroupOut {
    /// `tpl_ctrls.enable`.
    pub enable: u8,
    /// `tpl_ctrls.reduced_tpl_group`.
    pub reduced_tpl_group: i8,
    /// `tpl_ctrls.synth_blk_size`.
    pub synth_blk_size: u8,
    /// `tpl_ctrls.r0_adjust_factor`.
    pub r0_adjust_factor: f64,
    /// The function's return value (also the synthesizer block size).
    pub returned: u8,
}

/// C `svt_aom_set_tpl_group` (`initial_rc_process.c:204-306`).
///
/// `pcs_present == false` drives C's `pcs == NULL` probe path.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn set_tpl_group(
    pcs_present: bool,
    slice_type: u8,
    hierarchical_levels: u8,
    input_resolution: u8,
    tpl_lad_mg: u8,
    rate_control_mode: u8,
    tpl_group_level: u8,
    source_width: u32,
    source_height: u32,
) -> TplGroupOut {
    let (mut e, mut red, mut synth, mut r0) = (0u8, 0i8, 0u8, 0f64);
    let returned = unsafe {
        ref_set_tpl_group(
            i32::from(pcs_present),
            slice_type,
            hierarchical_levels,
            input_resolution,
            tpl_lad_mg,
            rate_control_mode,
            tpl_group_level,
            source_width,
            source_height,
            &mut e,
            &mut red,
            &mut synth,
            &mut r0,
        )
    };
    TplGroupOut {
        enable: e,
        reduced_tpl_group: red,
        synth_blk_size: synth,
        r0_adjust_factor: r0,
        returned,
    }
}

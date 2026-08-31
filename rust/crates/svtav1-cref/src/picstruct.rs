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

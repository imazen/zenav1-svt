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

unsafe extern "C" {
    fn ref_search_ref_in_ref_queue(
        pocs: *const u64,
        valid: *const i32,
        n: u32,
        ref_poc: u64,
    ) -> i32;

    fn ref_get_similar_ref_brightness(
        slice_type: u8,
        hierarchical_levels: u8,
        ref_list1_count_try: u8,
        ref0_avg_luma: u64,
        ref1_avg_luma: u64,
        cur_avg_luma: u64,
    ) -> i32;
}

/// C `search_ref_in_ref_queue` (`pic_manager_process.c:178-188`).
///
/// Returns the matched index, or `None`.
#[must_use]
pub fn search_ref_in_ref_queue(pocs: &[u64], valid: &[i32], ref_poc: u64) -> Option<usize> {
    assert_eq!(pocs.len(), valid.len());
    let r = unsafe {
        ref_search_ref_in_ref_queue(pocs.as_ptr(), valid.as_ptr(), pocs.len() as u32, ref_poc)
    };
    if r < 0 { None } else { Some(r as usize) }
}

/// C `get_similar_ref_brightness` (`pd_process.c:4251-4267`).
///
/// `avg_luma` is a `uint64_t` in C and `INVALID_LUMA` is compared against it
/// after a cast to `int`, so the sentinel is passed here as its unsigned
/// bit pattern.
#[must_use]
pub fn get_similar_ref_brightness(
    slice_type: u8,
    hierarchical_levels: u8,
    ref_list1_count_try: u8,
    ref0_avg_luma: u64,
    ref1_avg_luma: u64,
    cur_avg_luma: u64,
) -> bool {
    unsafe {
        ref_get_similar_ref_brightness(
            slice_type,
            hierarchical_levels,
            ref_list1_count_try,
            ref0_avg_luma,
            ref1_avg_luma,
            cur_avg_luma,
        ) != 0
    }
}

// ---------------------------------------------------------------------------
// The three `static` pd_process.c functions promoted by build.rs
// ---------------------------------------------------------------------------
//
// `set_ref_list_counts`, `set_all_ref_frame_type` and
// `scene_transition_detector` are `static` in C, so `nm -g` on
// libSvtAv1Enc.a does not find them. build.rs promotes them with
// `llvm-objcopy --globalize-symbol` on a PRIVATE COPY of the CMake object
// file and links that object alongside the archive; see
// `link_globalized_pd_statics` there. When the object or objcopy is missing
// on the host, build.rs emits a `cargo:warning` and the `picstruct_statics`
// cfg stays off, so everything below disappears rather than half-linking.

/// Whether build.rs was able to promote the `static` pd_process.c functions on
/// this host, i.e. whether [`set_ref_list_counts`] and
/// [`set_all_ref_frame_type`] exist.
///
/// The SKIP DECISION BELONGS TO THE CALLER, not to a test body: set
/// `SVT_CREF_REQUIRE_PICSTRUCT_STATICS=1` to turn an unavailable oracle into a
/// loud test failure instead of a narrower test suite.
pub const PICSTRUCT_STATICS_AVAILABLE: bool = cfg!(picstruct_statics);

#[cfg(picstruct_statics)]
unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_set_ref_list_counts(
        slice_type: u8,
        frame_type: u8,
        update_type: u8,
        is_overlay: u8,
        pic_pred_type: u8,
        seq_pred_structure: u8,
        ref_poc_array: *const u64,
        base_l0: u8,
        base_l1: u8,
        nonbase_l0: u8,
        nonbase_l1: u8,
        picture_number: u64,
        sframe_poc: u64,
        out_l0: *mut u8,
        out_l1: *mut u8,
    );

    fn ref_set_all_ref_frame_type(slice_type: u8, l0_try: u8, l1_try: u8, out_arr: *mut i8) -> u8;
}

/// C `set_ref_list_counts` (`pd_process.c:1804-1900`), reached through the
/// promoted symbol. Returns `(ref_list0_count, ref_list1_count)`, or `None`
/// when the promotion was not possible on this host (see
/// [`PICSTRUCT_STATICS_AVAILABLE`]).
#[must_use]
#[allow(clippy::too_many_arguments, unused_variables)]
pub fn set_ref_list_counts(
    slice_type: u8,
    frame_type: u8,
    update_type: u8,
    is_overlay: bool,
    pic_pred_type: u8,
    seq_pred_structure: u8,
    ref_poc_array: &[u64; 7],
    base_l0: u8,
    base_l1: u8,
    nonbase_l0: u8,
    nonbase_l1: u8,
    picture_number: u64,
    sframe_poc: u64,
) -> Option<(u8, u8)> {
    #[cfg(picstruct_statics)]
    {
        let (mut l0, mut l1) = (0u8, 0u8);
        unsafe {
            ref_set_ref_list_counts(
                slice_type,
                frame_type,
                update_type,
                u8::from(is_overlay),
                pic_pred_type,
                seq_pred_structure,
                ref_poc_array.as_ptr(),
                base_l0,
                base_l1,
                nonbase_l0,
                nonbase_l1,
                picture_number,
                sframe_poc,
                &mut l0,
                &mut l1,
            );
        }
        Some((l0, l1))
    }
    #[cfg(not(picstruct_statics))]
    {
        None
    }
}

/// C `set_all_ref_frame_type` (`pd_process.c:1044-1099`), reached through the
/// promoted symbol. Returns the ordered candidate set, or `None` when the
/// promotion was not possible on this host.
///
/// `ctx->sframe_poc` is left 0 so the `prune_sframe_refs` tail is a no-op,
/// matching the port's envelope.
#[must_use]
#[allow(unused_variables)]
pub fn set_all_ref_frame_type(slice_type: u8, l0_try: u8, l1_try: u8) -> Option<Vec<i8>> {
    #[cfg(picstruct_statics)]
    {
        let mut arr = [0i8; 32];
        let tot =
            unsafe { ref_set_all_ref_frame_type(slice_type, l0_try, l1_try, arr.as_mut_ptr()) };
        Some(arr[..tot as usize].to_vec())
    }
    #[cfg(not(picstruct_statics))]
    {
        None
    }
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_dg_detector_hme_level0(
        src_data: *mut u8,
        src_origin: u32,
        src_stride: u32,
        src_w: u16,
        src_h: u16,
        src_border: u16,
        ref_data: *mut u8,
        ref_origin: u32,
        ref_stride: u32,
        ref_w: u16,
        ref_h: u16,
        ref_border: u16,
        input_resolution: u8,
        aligned_width: u32,
        aligned_height: u32,
        b64_size: u16,
        seg_idx: u32,
        seg_cols: u32,
        seg_rows: u32,
        out_tot_dist: *mut u64,
        out_tot_cplx: *mut u32,
        out_tot_active: *mut u32,
        out_sum_in_vectors: *mut i32,
        out_seg_completed: *mut u16,
    );
}

/// One padded downsampled plane, as the dynamic-GOP detector reads it.
///
/// `origin` is the index of pixel (0, 0) inside `data`; the search reads
/// negative offsets from there, so `data` must extend `border` pixels in every
/// direction.
#[derive(Debug)]
pub struct DgPlane<'a> {
    /// The whole padded allocation.
    pub data: &'a mut [u8],
    /// Index of pixel (0, 0).
    pub origin: u32,
    /// Row stride.
    pub stride: u32,
    /// Un-padded width.
    pub width: u16,
    /// Un-padded height.
    pub height: u16,
    /// Padding on each side.
    pub border: u16,
}

/// C `DGDetectorMetrics` after one `dg_detector_hme_level0` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DgMetrics {
    /// C `tot_dist`.
    pub tot_dist: u64,
    /// C `tot_cplx`.
    pub tot_cplx: u32,
    /// C `tot_active`.
    pub tot_active: u32,
    /// C `sum_in_vectors`.
    pub sum_in_vectors: i32,
    /// C `seg_completed`.
    pub seg_completed: u16,
}

/// C `dg_detector_hme_level0` (`pd_process.c:532-629`).
///
/// Builds the whole `pa_ref_pic_wrapper` / `EbPaReferenceObject` /
/// `DGDetectorSeg` chain the callee walks, with a REAL mutex and semaphore
/// (it takes the one and posts the other), runs one segment, and returns the
/// accumulated metrics.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn dg_detector_hme_level0(
    src: &mut DgPlane<'_>,
    reference: &mut DgPlane<'_>,
    input_resolution: u8,
    aligned_width: u32,
    aligned_height: u32,
    b64_size: u16,
    seg_idx: u32,
    seg_cols: u32,
    seg_rows: u32,
) -> DgMetrics {
    let mut m = DgMetrics::default();
    unsafe {
        ref_dg_detector_hme_level0(
            src.data.as_mut_ptr(),
            src.origin,
            src.stride,
            src.width,
            src.height,
            src.border,
            reference.data.as_mut_ptr(),
            reference.origin,
            reference.stride,
            reference.width,
            reference.height,
            reference.border,
            input_resolution,
            aligned_width,
            aligned_height,
            b64_size,
            seg_idx,
            seg_cols,
            seg_rows,
            &mut m.tot_dist,
            &mut m.tot_cplx,
            &mut m.tot_active,
            &mut m.sum_in_vectors,
            &mut m.seg_completed,
        );
    }
    m
}

unsafe extern "C" {
    fn ref_tf_max_ref_per_struct(hierarchical_levels: u32, ty: u8, direction: i32) -> u8;
}

/// C `svt_aom_tf_max_ref_per_struct` (`enc_handle.c:2506-2519`).
#[must_use]
pub fn tf_max_ref_per_struct(hierarchical_levels: u32, ty: u8, direction: bool) -> u8 {
    unsafe { ref_tf_max_ref_per_struct(hierarchical_levels, ty, i32::from(direction)) }
}

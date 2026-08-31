//! FFI bindings for the MODE-DECISION oracle (lane `wp-modedecision`).
//!
//! Backed by `shims/mode_decision_shims.c`, which drives the REAL exported
//! C symbols listed in that file's header — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane
//! never shares an editable file with the concurrent inter lanes.

unsafe extern "C" {
    fn ref_md_get_ref_frame_type(list: i32, ref_idx: i32) -> i32;
    fn ref_md_get_max_drl_index(refmv_cnt: i32, mode: i32) -> i32;
    fn ref_md_is_interintra_allowed(
        enable_inter_intra: i32,
        bsize: i32,
        mode: i32,
        rf0: i32,
        rf1: i32,
    ) -> i32;
    fn ref_md_get_wedge_params_bits(bsize: i32) -> i32;
    fn ref_md_get_me_block_offset(
        org_x: i32,
        org_y: i32,
        bsize: i32,
        enable_me_8x8: i32,
        enable_me_16x16: i32,
    ) -> i32;
    fn ref_md_is_valid_unipred_ref(
        pruning_enabled: i32,
        do_ref_flat: *const u8,
        closest_refs: *const u8,
        inter_cand_group: i32,
        list_idx: i32,
        ref_idx: i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_md_is_me_data_present(
        me_block_offset: i32,
        me_cand_offset: i32,
        total_me_candidate_index: *const u8,
        n_blocks: i32,
        cands: *const i32,
        n_cands: i32,
        list_idx: i32,
        ref_idx: i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_md_obmc_motion_mode_allowed(
        trans_face_off: i32,
        obmc_enabled: i32,
        obmc_max_blk_size: i32,
        situation: i32,
        is_motion_mode_switchable: i32,
        force_integer_mv: i32,
        gm_wmtype: *const i32,
        overlappable_neighbors: i32,
        bsize: i32,
        rf0: i32,
        rf1: i32,
        mode: i32,
    ) -> i32;
}

/// C `TOT_INTER_GROUP` (md_process.h:78).
pub const TOT_INTER_GROUP: usize = 11;
/// C `MAX_NUM_OF_REF_PIC_LIST` (definitions.h:2048).
pub const MAX_NUM_OF_REF_PIC_LIST: usize = 2;
/// C `REF_LIST_MAX_DEPTH` (definitions.h).
pub const REF_LIST_MAX_DEPTH: usize = 4;

/// C `svt_get_ref_frame_type` (mode_decision.c:265, EXPORTED).
pub fn get_ref_frame_type(list: u8, ref_idx: u8) -> i32 {
    unsafe { ref_md_get_ref_frame_type(i32::from(list), i32::from(ref_idx)) }
}

/// C `svt_aom_get_max_drl_index` (mode_decision.c:269, EXPORTED).
pub fn get_max_drl_index(refmv_cnt: u8, mode: u8) -> u8 {
    unsafe { ref_md_get_max_drl_index(i32::from(refmv_cnt), i32::from(mode)) as u8 }
}

/// C `svt_is_interintra_allowed` (mode_decision.c:96, EXPORTED).
pub fn is_interintra_allowed(enable: u8, bsize: u8, mode: u8, rf: [i8; 2]) -> i32 {
    unsafe {
        ref_md_is_interintra_allowed(
            i32::from(enable),
            i32::from(bsize),
            i32::from(mode),
            i32::from(rf[0]),
            i32::from(rf[1]),
        )
    }
}

/// C `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053, EXPORTED).
pub fn get_wedge_params_bits(bsize: u8) -> i32 {
    unsafe { ref_md_get_wedge_params_bits(i32::from(bsize)) }
}

/// C `svt_aom_get_me_block_offset` (mode_decision.c:117, EXPORTED).
pub fn get_me_block_offset(
    org_x: u32,
    org_y: u32,
    bsize: u8,
    enable_me_8x8: u8,
    enable_me_16x16: u8,
) -> u32 {
    unsafe {
        ref_md_get_me_block_offset(
            org_x as i32,
            org_y as i32,
            i32::from(bsize),
            i32::from(enable_me_8x8),
            i32::from(enable_me_16x16),
        ) as u32
    }
}

/// C `svt_aom_is_valid_unipred_ref` (mode_decision.c:762, EXPORTED).
///
/// `do_ref` is C's `ctx->ref_filtering_res[group][list][ref].do_ref` in
/// C's index order.
pub fn is_valid_unipred_ref(
    pruning_enabled: bool,
    do_ref: &[u8; TOT_INTER_GROUP * MAX_NUM_OF_REF_PIC_LIST * REF_LIST_MAX_DEPTH],
    closest_refs: &[u8; TOT_INTER_GROUP],
    inter_cand_group: u8,
    list_idx: u8,
    ref_idx: u8,
) -> bool {
    unsafe {
        ref_md_is_valid_unipred_ref(
            i32::from(pruning_enabled),
            do_ref.as_ptr(),
            closest_refs.as_ptr(),
            i32::from(inter_cand_group),
            i32::from(list_idx),
            i32::from(ref_idx),
        ) != 0
    }
}

/// One `MeCandidate` (me_sb_results.h:29) as the shim packs it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefMeCandidate {
    pub direction: u8,
    pub ref_idx_l0: u8,
    pub ref_idx_l1: u8,
    pub ref0_list: u8,
    pub ref1_list: u8,
}

/// C `svt_aom_is_me_data_present` (mode_decision.c:179, EXPORTED).
pub fn is_me_data_present(
    me_block_offset: u32,
    me_cand_offset: u32,
    totals: &[u8],
    cands: &[RefMeCandidate],
    list_idx: u8,
    ref_idx: u8,
) -> u8 {
    let flat: Vec<i32> = cands
        .iter()
        .flat_map(|c| {
            [
                i32::from(c.direction),
                i32::from(c.ref_idx_l0),
                i32::from(c.ref_idx_l1),
                i32::from(c.ref0_list),
                i32::from(c.ref1_list),
            ]
        })
        .collect();
    unsafe {
        ref_md_is_me_data_present(
            me_block_offset as i32,
            me_cand_offset as i32,
            totals.as_ptr(),
            totals.len() as i32,
            flat.as_ptr(),
            cands.len() as i32,
            i32::from(list_idx),
            i32::from(ref_idx),
        ) as u8
    }
}

/// Inputs to [`obmc_motion_mode_allowed`], mirroring the C context fields
/// the predicate reads.
#[derive(Clone, Copy, Debug)]
pub struct ObmcAllowedInput {
    pub trans_face_off: u8,
    pub obmc_enabled: u8,
    pub obmc_max_blk_size: u8,
    pub situation: u8,
    pub is_motion_mode_switchable: u8,
    pub force_integer_mv: u8,
    /// `pcs->ppcs->global_motion[rf].wmtype`, `TOTAL_REFS_PER_FRAME` entries.
    pub gm_wmtype: [i32; 8],
    pub overlappable_neighbors: u32,
    pub bsize: u8,
    pub rf0: i8,
    pub rf1: i8,
    pub mode: u8,
}

/// C `svt_aom_obmc_motion_mode_allowed` (mode_decision.c:214, EXPORTED).
/// Returns C's `MotionMode` as an `i32`.
pub fn obmc_motion_mode_allowed(i: &ObmcAllowedInput) -> i32 {
    unsafe {
        ref_md_obmc_motion_mode_allowed(
            i32::from(i.trans_face_off),
            i32::from(i.obmc_enabled),
            i32::from(i.obmc_max_blk_size),
            i32::from(i.situation),
            i32::from(i.is_motion_mode_switchable),
            i32::from(i.force_integer_mv),
            i.gm_wmtype.as_ptr(),
            i.overlappable_neighbors as i32,
            i32::from(i.bsize),
            i32::from(i.rf0),
            i32::from(i.rf1),
            i32::from(i.mode),
        )
    }
}

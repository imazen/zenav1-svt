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

// ---------------------------------------------------------------------------
// PME SAD kernel + the MD motion-search cost model.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_md_fp_mv_err_cost(
        mv_x: i32,
        mv_y: i32,
        ref_x: i32,
        ref_y: i32,
        mv_cost_type: i32,
        error_per_bit: i32,
        mvj: *const i32,
        mvc0: *const i32,
        mvc1: *const i32,
        use_tables: i32,
    ) -> i32;
    fn ref_md_get_sad_per_bit(qidx: i32, is_hbd: i32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_md_pme_sad_loop_kernel(
        ref_x: i32,
        ref_y: i32,
        mv_cost_type: i32,
        error_per_bit: i32,
        mvj: *const i32,
        mvc0: *const i32,
        mvc1: *const i32,
        use_tables: i32,
        src: *const u8,
        src_stride: i32,
        refbuf: *const u8,
        ref_stride: i32,
        block_height: i32,
        block_width: i32,
        best_cost: *mut u32,
        best_mvx: *mut i16,
        best_mvy: *mut i16,
        search_position_start_x: i32,
        search_position_start_y: i32,
        search_area_width: i32,
        search_area_height: i32,
        search_step: i32,
        mvx: i32,
        mvy: i32,
    );
}

/// C's `mvjcost` + `mvcost[2]` triple. The two component tables are
/// `MV_VALS`-long and the C pointer is offset by `MV_MAX`, so the slices
/// here are the WHOLE tables and the shim re-applies the offset.
pub struct MvCostTablesRef<'a> {
    pub joint: &'a [i32; 4],
    /// Indexed `MV_MAX + value` (2 * MV_MAX + 1 entries).
    pub comp0: &'a [i32],
    pub comp1: &'a [i32],
}

/// C `svt_aom_fp_mv_err_cost` (mcomp.c:775, EXPORTED). `tables` is `None`
/// for C's NULL-`mvcost` arm.
pub fn fp_mv_err_cost(
    mv: (i16, i16),
    ref_mv: (i16, i16),
    mv_cost_type: i32,
    error_per_bit: i32,
    tables: Option<&MvCostTablesRef<'_>>,
) -> i32 {
    // C's `mvcost[i]` points at &nmv_costs[i][MV_MAX].
    const MV_MAX: usize = (1 << 14) - 1;
    let (mvj, c0, c1, used) = match tables {
        Some(t) => (
            t.joint.as_ptr(),
            t.comp0[MV_MAX..].as_ptr(),
            t.comp1[MV_MAX..].as_ptr(),
            1,
        ),
        None => (std::ptr::null(), std::ptr::null(), std::ptr::null(), 0),
    };
    unsafe {
        ref_md_fp_mv_err_cost(
            i32::from(mv.0),
            i32::from(mv.1),
            i32::from(ref_mv.0),
            i32::from(ref_mv.1),
            mv_cost_type,
            error_per_bit,
            mvj,
            c0,
            c1,
            used,
        )
    }
}

/// C `svt_aom_get_sad_per_bit` (mode_decision.c:2048, EXPORTED). The shim
/// calls `svt_av1_init_me_luts` first, which is idempotent.
pub fn get_sad_per_bit(qidx: i32, is_hbd: bool) -> i32 {
    unsafe { ref_md_get_sad_per_bit(qidx, i32::from(is_hbd)) }
}

/// The kernel's three out-params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmeBest {
    pub cost: u32,
    pub mvx: i16,
    pub mvy: i16,
}

/// C `svt_pme_sad_loop_kernel_c` (product_coding_loop.c:1775, EXPORTED).
#[allow(clippy::too_many_arguments)]
pub fn pme_sad_loop_kernel(
    ref_mv: (i16, i16),
    mv_cost_type: i32,
    error_per_bit: i32,
    tables: Option<&MvCostTablesRef<'_>>,
    src: &[u8],
    src_stride: usize,
    ref_buf: &[u8],
    ref_offset: usize,
    ref_stride: usize,
    block_height: usize,
    block_width: usize,
    best: &mut PmeBest,
    search_position_start_x: i16,
    search_position_start_y: i16,
    search_area_width: i16,
    search_area_height: i16,
    search_step: i16,
    mvx: i16,
    mvy: i16,
) {
    const MV_MAX: usize = (1 << 14) - 1;
    let (mvj, c0, c1, used) = match tables {
        Some(t) => (
            t.joint.as_ptr(),
            t.comp0[MV_MAX..].as_ptr(),
            t.comp1[MV_MAX..].as_ptr(),
            1,
        ),
        None => (std::ptr::null(), std::ptr::null(), std::ptr::null(), 0),
    };
    unsafe {
        ref_md_pme_sad_loop_kernel(
            i32::from(ref_mv.0),
            i32::from(ref_mv.1),
            mv_cost_type,
            error_per_bit,
            mvj,
            c0,
            c1,
            used,
            src.as_ptr(),
            src_stride as i32,
            ref_buf.as_ptr().add(ref_offset),
            ref_stride as i32,
            block_height as i32,
            block_width as i32,
            &mut best.cost,
            &mut best.mvx,
            &mut best.mvy,
            i32::from(search_position_start_x),
            i32::from(search_position_start_y),
            i32::from(search_area_width),
            i32::from(search_area_height),
            i32::from(search_step),
            i32::from(mvx),
            i32::from(mvy),
        );
    }
}

// ---------------------------------------------------------------------------
// Per-stage candidate counts.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_md_set_nics(
        s1: i32,
        s2: i32,
        s3: i32,
        pic_type: i32,
        qp: i32,
        nic_max_qp_based_th_scaling: i32,
        mds1: *mut u32,
        mds2: *mut u32,
        mds3: *mut u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_md_set_md_stage_counts(
        s1: i32,
        s2: i32,
        s3: i32,
        md_staging_mode: i32,
        is_i_slice: i32,
        is_highest_layer: i32,
        qp: i32,
        nic_max_qp_based_th_scaling: i32,
        mds1: *mut u32,
        mds2: *mut u32,
        mds3: *mut u32,
        bypass1: *mut i32,
        bypass2: *mut i32,
    );
}

/// C `CAND_CLASS_TOTAL` (definitions.h:793).
pub const CAND_CLASS_TOTAL: usize = 5;

/// The three per-class stage counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MdStageCounts {
    pub mds1: [u32; CAND_CLASS_TOTAL],
    pub mds2: [u32; CAND_CLASS_TOTAL],
    pub mds3: [u32; CAND_CLASS_TOTAL],
}

/// C `svt_aom_set_nics` (product_coding_loop.c:1358, EXPORTED).
pub fn set_nics(
    scaling: (u8, u8, u8),
    pic_type: u8,
    qp: u32,
    nic_max_qp_based_th_scaling: bool,
) -> MdStageCounts {
    let mut out = MdStageCounts::default();
    unsafe {
        ref_md_set_nics(
            i32::from(scaling.0),
            i32::from(scaling.1),
            i32::from(scaling.2),
            i32::from(pic_type),
            qp as i32,
            i32::from(nic_max_qp_based_th_scaling),
            out.mds1.as_mut_ptr(),
            out.mds2.as_mut_ptr(),
            out.mds3.as_mut_ptr(),
        );
    }
    out
}

/// C `set_md_stage_counts` (product_coding_loop.c:1394, EXPORTED — the
/// name carries no `svt_aom_` prefix; `nm -g` is what establishes that).
#[allow(clippy::too_many_arguments)]
pub fn set_md_stage_counts(
    scaling: (u8, u8, u8),
    md_staging_mode: u8,
    is_i_slice: bool,
    is_highest_layer: bool,
    qp: u32,
    nic_max_qp_based_th_scaling: bool,
) -> (MdStageCounts, bool, bool) {
    let mut out = MdStageCounts::default();
    let mut b1 = 0i32;
    let mut b2 = 0i32;
    unsafe {
        ref_md_set_md_stage_counts(
            i32::from(scaling.0),
            i32::from(scaling.1),
            i32::from(scaling.2),
            i32::from(md_staging_mode),
            i32::from(is_i_slice),
            i32::from(is_highest_layer),
            qp as i32,
            i32::from(nic_max_qp_based_th_scaling),
            out.mds1.as_mut_ptr(),
            out.mds2.as_mut_ptr(),
            out.mds3.as_mut_ptr(),
            &mut b1,
            &mut b2,
        );
    }
    (out, b1 != 0, b2 != 0)
}

// ---------------------------------------------------------------------------
// DRL selection.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_md_choose_best_av1_mv_pred(
        shut_fast_rate: i32,
        approx_inter_rate: i32,
        stack: *const i32,
        ref_mv_count: i32,
        ref_frame: i32,
        mode: i32,
        mv0_as_int: u32,
        mv1_as_int: u32,
        nmv_vec_cost: *const i32,
        nmv_costs0: *const i32,
        nmv_costs1: *const i32,
        drl_fac_bits: *const i32,
        best_drl_index_io: *mut i32,
        best_pred_mv_io: *mut u32,
    );
}

/// C `MAX_REF_MV_STACK_SIZE`.
pub const MAX_REF_MV_STACK_SIZE: usize = 8;
/// C `DRL_MODE_CONTEXTS` (definitions.h:1343).
pub const DRL_MODE_CONTEXTS: usize = 3;
/// C `MV_JOINTS`.
pub const MV_JOINTS: usize = 4;
/// C `MV_VALS` (cabac_context_model.h:195).
pub const MV_VALS: usize = ((1 << 14) - 1) * 2 + 1;

/// C `svt_aom_choose_best_av1_mv_pred` (mode_decision.c:527, EXPORTED).
///
/// `best_drl_index` / `best_pred_mv` are IN/OUT: C leaves them untouched
/// on the `shut_fast_rate` early return.
#[allow(clippy::too_many_arguments)]
pub fn choose_best_av1_mv_pred(
    shut_fast_rate: bool,
    approx_inter_rate: u8,
    stack: &[(u32, u32, i32); MAX_REF_MV_STACK_SIZE],
    ref_mv_count: u8,
    ref_frame: i32,
    mode: u8,
    mv0_as_int: u32,
    mv1_as_int: u32,
    nmv_vec_cost: &[i32; MV_JOINTS],
    nmv_costs0: &[i32],
    nmv_costs1: &[i32],
    drl_fac_bits: &[[i32; 2]; DRL_MODE_CONTEXTS],
    best_drl_index: &mut u8,
    best_pred_mv: &mut [u32; 2],
) {
    let flat: Vec<i32> = stack
        .iter()
        .flat_map(|&(a, b, w)| [a as i32, b as i32, w])
        .collect();
    let fac: Vec<i32> = drl_fac_bits.iter().flat_map(|r| [r[0], r[1]]).collect();
    let mut drl = i32::from(*best_drl_index);
    unsafe {
        ref_md_choose_best_av1_mv_pred(
            i32::from(shut_fast_rate),
            i32::from(approx_inter_rate),
            flat.as_ptr(),
            i32::from(ref_mv_count),
            ref_frame,
            i32::from(mode),
            mv0_as_int,
            mv1_as_int,
            nmv_vec_cost.as_ptr(),
            nmv_costs0.as_ptr(),
            nmv_costs1.as_ptr(),
            fac.as_ptr(),
            &mut drl,
            best_pred_mv.as_mut_ptr(),
        );
    }
    *best_drl_index = drl as u8;
}

// ---------------------------------------------------------------------------
// High-bit-depth tune-SSIM distortion.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_md_similarity(
        sum_s: u32,
        sum_r: u32,
        sum_sq_s: u32,
        sum_sq_r: u32,
        sum_sxr: u32,
        count: i32,
        bd: u32,
    ) -> f64;
    fn ref_md_ssim_4x4_hbd(s: *const u16, sp: u32, r: *const u16, rp: u32) -> f64;
    fn ref_md_ssim_8x8_hbd(s: *const u16, sp: u32, r: *const u16, rp: u32) -> f64;
    #[allow(clippy::too_many_arguments)]
    fn ref_md_spatial_full_distortion_ssim(
        input: *const u16,
        input_offset: u32,
        input_stride: u32,
        recon: *const u16,
        recon_offset: i32,
        recon_stride: u32,
        area_width: u32,
        area_height: u32,
        hbd: i32,
        ac_bias: f64,
    ) -> u64;
}

/// C `svt_aom_similarity` (enc_dec_process.c:645, EXPORTED).
pub fn similarity(
    sum_s: u32,
    sum_r: u32,
    sum_sq_s: u32,
    sum_sq_r: u32,
    sum_sxr: u32,
    count: i32,
    bd: u32,
) -> f64 {
    unsafe { ref_md_similarity(sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr, count, bd) }
}

/// C `svt_ssim_4x4_hbd_c` (mode_decision.c:4220, EXPORTED).
pub fn ssim_4x4_hbd(s: &[u16], sp: usize, r: &[u16], rp: usize) -> f64 {
    unsafe { ref_md_ssim_4x4_hbd(s.as_ptr(), sp as u32, r.as_ptr(), rp as u32) }
}

/// C `svt_ssim_8x8_hbd_c` (mode_decision.c:4245, EXPORTED).
pub fn ssim_8x8_hbd(s: &[u16], sp: usize, r: &[u16], rp: usize) -> f64 {
    unsafe { ref_md_ssim_8x8_hbd(s.as_ptr(), sp as u32, r.as_ptr(), rp as u32) }
}

/// C `svt_spatial_full_distortion_ssim_kernel` (mode_decision.c:4372,
/// EXPORTED), driven on the `hbd` arm.
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_ssim_hbd(
    input: &[u16],
    input_offset: usize,
    input_stride: usize,
    recon: &[u16],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
    ac_bias: f64,
) -> u64 {
    unsafe {
        ref_md_spatial_full_distortion_ssim(
            input.as_ptr(),
            input_offset as u32,
            input_stride as u32,
            recon.as_ptr(),
            recon_offset as i32,
            recon_stride as u32,
            area_width as u32,
            area_height as u32,
            1,
            ac_bias,
        )
    }
}

// ---------------------------------------------------------------------------
// Reference-frame signalling rate.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_md_estimate_ref_frame_type_bits(
        above: *const i32,
        left: *const i32,
        ref_frame_type: i32,
        is_compound: i32,
        comp_ref_type: *const i32,
        uni_comp_ref: *const i32,
        comp_ref: *const i32,
        comp_bwd_ref: *const i32,
        single_ref: *const i32,
        mode_ctx_out: *mut i32,
        ref_counts_out: *mut u8,
    ) -> u64;
}

/// A neighbour as the shim packs it: `(ref0, ref1, use_intrabc)`.
pub type RefNeighbor = (i8, i8, bool);

/// The six `MdRateEstimationContext` tables, flattened in C's index
/// order.
pub struct RefRateTables {
    pub comp_ref_type: Vec<i32>,
    pub uni_comp_ref: Vec<i32>,
    pub comp_ref: Vec<i32>,
    pub comp_bwd_ref: Vec<i32>,
    pub single_ref: Vec<i32>,
}

/// C `estimate_ref_frame_type_bits` (rd_cost.c:643, EXPORTED), plus the
/// neighbour counts and reference-mode context the same driver collects.
pub fn estimate_ref_frame_type_bits(
    above: Option<RefNeighbor>,
    left: Option<RefNeighbor>,
    ref_frame_type: i32,
    is_compound: bool,
    t: &RefRateTables,
) -> (u64, i32, [u8; 8]) {
    let pack = |n: Option<RefNeighbor>| -> Option<[i32; 3]> {
        n.map(|(a, b, ibc)| [i32::from(a), i32::from(b), i32::from(ibc)])
    };
    let a = pack(above);
    let l = pack(left);
    let mut mode_ctx = 0i32;
    let mut counts = [0u8; 8];
    let bits = unsafe {
        ref_md_estimate_ref_frame_type_bits(
            a.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            l.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            ref_frame_type,
            i32::from(is_compound),
            t.comp_ref_type.as_ptr(),
            t.uni_comp_ref.as_ptr(),
            t.comp_ref.as_ptr(),
            t.comp_bwd_ref.as_ptr(),
            t.single_ref.as_ptr(),
            &mut mode_ctx,
            counts.as_mut_ptr(),
        )
    };
    (bits, mode_ctx, counts)
}

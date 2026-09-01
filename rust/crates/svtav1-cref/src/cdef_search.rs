//! CDEF search signal-derivation oracle — `shims/cdef_shims.c`.
//!
//! **Evidence tier 1.** `set_cdef_search_controls` (`enc_mode_config.c:891`) is
//! file-`static` and both `cdef_search_level` ladders are inline in their
//! callers, so none can be called directly — but the EXPORTED
//! `svt_aom_sig_deriv_multi_processes_{default,allintra}` run all three and
//! leave the result in `pcs->cdef_level` + `pcs->cdef_search_ctrls`, which the
//! shim reads back. So this differential drives the real C ladders and the
//! real C controls table.
//!
//! Unlike the DLF twin, the LEVEL scalar IS stored by C (`pcs->cdef_level`),
//! so the ladder and the table are observable separately rather than only as
//! a composite.

unsafe extern "C" {
    fn ref_cdef_search_ctrls_default(input: *const i32, out: *mut i64);
    fn ref_cdef_search_ctrls_allintra(input: *const i32, out: *mut i64);
    fn ref_cdef_search_ctrls_in_slots() -> i32;
    fn ref_cdef_search_ctrls_out_slots() -> i32;
    fn ref_cdef_search_ctrls_arr_len() -> i32;
}

/// Input slot indices, mirroring the shim's `CDS_I_*`.
///
/// The layout is the multi-processes shim's `MP_I_*` (the whole
/// `svt_aom_sig_deriv_multi_processes_*` body runs, so every field it
/// dereferences must be populated) plus `scs->enable_hbd_mode_decision` (read
/// only by the allintra arm) and the two inputs of `frame_is_boosted` /
/// `frame_is_leaf`.
pub mod cdef_in {
    /// `pcs->enc_mode`
    pub const ENC_MODE: usize = 0;
    /// `pcs->slice_type == I_SLICE`
    pub const IS_ISLICE: usize = 1;
    /// `pcs->temporal_layer_index` — the LADDER's `is_base`.
    pub const TEMPORAL_LAYER: usize = 2;
    /// `pcs->input_resolution`
    pub const INPUT_RES: usize = 3;
    /// `scs->static_config.fast_decode`
    pub const FAST_DECODE: usize = 4;
    /// `pcs->sc_class5`
    pub const SC_CLASS5: usize = 5;
    /// `pcs->is_highest_layer`
    pub const IS_HIGHEST_LAYER: usize = 6;
    /// `pcs->tf_ctrls.hme_me_level`
    pub const TF_HME_LEVEL: usize = 7;
    /// `scs->static_config.enable_intrabc`
    pub const ENABLE_INTRABC: usize = 8;
    /// `scs->seq_header.cdef_level`
    pub const SEQ_CDEF_LEVEL: usize = 9;
    /// `scs->static_config.cdef_level` (`DEFAULT` = -1)
    pub const CFG_CDEF_LEVEL: usize = 10;
    /// `scs->seq_header.enable_restoration`
    pub const SEQ_ENABLE_RESTORATION: usize = 11;
    /// `scs->max_initial_input_luma_width`
    pub const INIT_LUMA_W: usize = 12;
    /// `scs->max_initial_input_luma_height`
    pub const INIT_LUMA_H: usize = 13;
    /// `scs->encoder_bit_depth`
    pub const ENCODER_BIT_DEPTH: usize = 14;
    /// `scs->static_config.hbd_mds` (`DEFAULT` = -1) — the default arm's.
    pub const CFG_HBD_MDS: usize = 15;
    /// `scs->enable_hbd_mode_decision` (`DEFAULT` = -1) — the allintra arm's.
    pub const HBD_MODE_DECISION: usize = 16;
    /// `pcs->frm_hdr.frame_type` — `KEY_FRAME` = 0. Half of
    /// `frame_is_boosted`.
    pub const FRAME_TYPE: usize = 17;
    /// `pcs->update_type` — `SVT_AV1_KF_UPDATE` = 0, `LF_UPDATE` = 1,
    /// `GF_UPDATE` = 2, `ARF_UPDATE` = 3. Drives `frame_is_leaf` and the
    /// other half of `frame_is_boosted`.
    pub const UPDATE_TYPE: usize = 18;
    /// Slot count; cross-checked against the shim.
    pub const COUNT: usize = 19;
}

/// C `TOTAL_STRENGTHS` — the length of each candidate array in the output.
pub const ARR: usize = 64;

/// Output slot indices, mirroring the shim's `CDS_O_*`.
pub mod cdef_out {
    /// `pcs->cdef_level` — the ladder's answer, before the table.
    pub const LEVEL: usize = 0;
    /// `cdef_search_ctrls.enabled`
    pub const ENABLED: usize = 1;
    /// `first_pass_fs_num`
    pub const FIRST_NUM: usize = 2;
    /// `default_second_pass_fs_num`
    pub const SECOND_NUM: usize = 3;
    /// `use_reference_cdef_fs`
    pub const USE_REF_FS: usize = 4;
    /// `subsampling_factor`
    pub const SUBSAMPLING: usize = 5;
    /// `search_best_ref_fs`
    pub const BEST_REF_FS: usize = 6;
    /// `skip_th`
    pub const SKIP_TH: usize = 7;
    /// `uv_from_y`
    pub const UV_FROM_Y: usize = 8;
    /// `use_qp_strength`
    pub const USE_QP_STRENGTH: usize = 9;
    /// `frm_hdr.allow_intrabc` — the ladder's own level-0 guard.
    pub const ALLOW_INTRABC: usize = 10;
    /// `pcs->palette_level`
    pub const PALETTE_LEVEL: usize = 11;
    /// `default_first_pass_fs[0..64]`
    pub const FIRST_FS: usize = 16;
    /// `default_second_pass_fs[0..64]`
    pub const SECOND_FS: usize = FIRST_FS + super::ARR;
    /// `default_first_pass_fs_uv[0..64]`
    pub const FIRST_FS_UV: usize = SECOND_FS + super::ARR;
    /// `default_second_pass_fs_uv[0..64]`
    pub const SECOND_FS_UV: usize = FIRST_FS_UV + super::ARR;
    /// Slot count; cross-checked against the shim.
    pub const COUNT: usize = SECOND_FS_UV + super::ARR;
}

fn check_slots() {
    assert_eq!(
        unsafe { ref_cdef_search_ctrls_in_slots() } as usize,
        cdef_in::COUNT,
        "C shim cdef input slot count drifted"
    );
    assert_eq!(
        unsafe { ref_cdef_search_ctrls_out_slots() } as usize,
        cdef_out::COUNT,
        "C shim cdef output slot count drifted"
    );
    assert_eq!(
        unsafe { ref_cdef_search_ctrls_arr_len() } as usize,
        ARR,
        "C TOTAL_STRENGTHS drifted"
    );
}

/// Run the VIDEO arm — `svt_aom_sig_deriv_multi_processes_default`
/// (`enc_mode_config.c:1973`) — and return `pcs->cdef_level` plus
/// `pcs->cdef_search_ctrls`.
///
/// # Panics
/// If the C shim's slot counts disagree with [`cdef_in::COUNT`] /
/// [`cdef_out::COUNT`].
#[must_use]
pub fn cdef_search_ctrls_default(input: &[i32; cdef_in::COUNT]) -> [i64; cdef_out::COUNT] {
    check_slots();
    let mut out = [0i64; cdef_out::COUNT];
    unsafe { ref_cdef_search_ctrls_default(input.as_ptr(), out.as_mut_ptr()) };
    out
}

/// Run the STILL/AVIF arm — `svt_aom_sig_deriv_multi_processes_allintra`
/// (`enc_mode_config.c:2337`) — and return `pcs->cdef_level` plus
/// `pcs->cdef_search_ctrls`.
///
/// # Panics
/// If the C shim's slot counts disagree with [`cdef_in::COUNT`] /
/// [`cdef_out::COUNT`].
#[must_use]
pub fn cdef_search_ctrls_allintra(input: &[i32; cdef_in::COUNT]) -> [i64; cdef_out::COUNT] {
    check_slots();
    let mut out = [0i64; cdef_out::COUNT];
    unsafe { ref_cdef_search_ctrls_allintra(input.as_ptr(), out.as_mut_ptr()) };
    out
}

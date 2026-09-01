//! FFI bindings for the long-term reference-management oracle
//! (`Codec/pd_process.c:1162-1478`).
//!
//! Backed by `shims/refmgmt_shims.c`, which drives the REAL exported C symbols
//! `svt_aom_ref_mgmt_storeable_slots_mask` and `svt_aom_is_pic_skipped` —
//! evidence tier 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! The first of those calls the file-static `exclusive_write_slots_mask_ld_cbr`
//! internally, so a differential on it covers that helper too.

unsafe extern "C" {
    fn refmgmt_storeable_slots_mask(
        rtc: i32,
        hierarchical_levels: u8,
        pred_structure: u8,
        ld_reduce_ref_buffs: u8,
    ) -> u8;

    fn refmgmt_is_pic_skipped(
        is_ref: i32,
        rc_stat_gen_pass_mode: u8,
        first_frame_in_minigop: u8,
    ) -> i32;
}

/// C `svt_aom_ref_mgmt_storeable_slots_mask` (`pd_process.c:1259`) —
/// the DPB slots a long-term STORE may claim.
///
/// `pred_structure` is `PredStructure`: 0 all-intra, 1 low delay, 2 random
/// access.
#[must_use]
pub fn storeable_slots_mask(
    rtc: bool,
    hierarchical_levels: u8,
    pred_structure: u8,
    ld_reduce_ref_buffs: u8,
) -> u8 {
    unsafe {
        refmgmt_storeable_slots_mask(
            i32::from(rtc),
            hierarchical_levels,
            pred_structure,
            ld_reduce_ref_buffs,
        )
    }
}

/// C `svt_aom_is_pic_skipped` (`pd_process.c:996`).
#[must_use]
pub fn is_pic_skipped(is_ref: bool, rc_stat_gen_pass_mode: u8, first_frame_in_minigop: u8) -> bool {
    unsafe {
        refmgmt_is_pic_skipped(
            i32::from(is_ref),
            rc_stat_gen_pass_mode,
            first_frame_in_minigop,
        ) != 0
    }
}

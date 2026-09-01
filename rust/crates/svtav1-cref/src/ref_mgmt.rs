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

// ---------------------------------------------------------------------------
// pcs.c geometry + sizing (`shims/refmgmt_shims.c`)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn pcsgeom_max_allocated_me_refs(l0: u8, l1: u8, max_ref: *mut u8, max_cand: *mut u8);
    fn pcsgeom_out_buffer_size(w: u32, h: u32) -> u32;
    fn pcsgeom_b64_geom_init(
        b64_size: u8,
        width: u16,
        height: u16,
        cap: u32,
        org_x: *mut u16,
        org_y: *mut u16,
        w: *mut u8,
        h: *mut u8,
        complete: *mut u8,
    ) -> u32;
    fn pcsgeom_sb_geom_init(
        sb_size: u16,
        width: u16,
        height: u16,
        cap: u32,
        org_x: *mut u16,
        org_y: *mut u16,
        w: *mut u8,
        h: *mut u8,
    ) -> u32;
}

/// C `svt_aom_get_max_allocated_me_refs` (`pcs.c:88`).
#[must_use]
pub fn max_allocated_me_refs(l0: u8, l1: u8) -> (u8, u8) {
    let (mut r, mut c) = (0u8, 0u8);
    unsafe { pcsgeom_max_allocated_me_refs(l0, l1, &mut r, &mut c) };
    (r, c)
}

/// C `svt_aom_get_out_buffer_size` (`pcs.c:374`).
#[must_use]
pub fn out_buffer_size(w: u32, h: u32) -> u32 {
    unsafe { pcsgeom_out_buffer_size(w, h) }
}

/// One row of C's `B64Geom` array: `(org_x, org_y, width, height, is_complete)`.
pub type B64GeomRow = (u16, u16, u8, u8, bool);

/// C `b64_geom_init` (`pcs.c:1491`) — the whole 64x64 base-block grid.
#[must_use]
pub fn b64_geom_init(b64_size: u8, width: u16, height: u16) -> Vec<B64GeomRow> {
    let cap = 4096usize;
    let (mut x, mut y) = (vec![0u16; cap], vec![0u16; cap]);
    let (mut w, mut h, mut c) = (vec![0u8; cap], vec![0u8; cap], vec![0u8; cap]);
    let n = unsafe {
        pcsgeom_b64_geom_init(
            b64_size,
            width,
            height,
            cap as u32,
            x.as_mut_ptr(),
            y.as_mut_ptr(),
            w.as_mut_ptr(),
            h.as_mut_ptr(),
            c.as_mut_ptr(),
        )
    } as usize;
    (0..n)
        .map(|i| (x[i], y[i], w[i], h[i], c[i] != 0))
        .collect()
}

/// One row of C's `SbGeom` array: `(org_x, org_y, width, height)`.
pub type SbGeomRow = (u16, u16, u8, u8);

/// C `sb_geom_init` (`pcs.c:1535`) — the whole superblock grid.
#[must_use]
pub fn sb_geom_init(sb_size: u16, width: u16, height: u16) -> Vec<SbGeomRow> {
    let cap = 4096usize;
    let (mut x, mut y) = (vec![0u16; cap], vec![0u16; cap]);
    let (mut w, mut h) = (vec![0u8; cap], vec![0u8; cap]);
    let n = unsafe {
        pcsgeom_sb_geom_init(
            sb_size,
            width,
            height,
            cap as u32,
            x.as_mut_ptr(),
            y.as_mut_ptr(),
            w.as_mut_ptr(),
            h.as_mut_ptr(),
        )
    } as usize;
    (0..n).map(|i| (x[i], y[i], w[i], h[i])).collect()
}

//! FFI bindings for the RD-COST oracle (`Source/Lib/Codec/rd_cost.c`).
//!
//! Backed by `shims/rd_cost_shims.c`, which drives the REAL exported C
//! symbols listed in that file's header comment — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with a concurrent lane.
//!
//! # The rate tables cross as a FLAT ARRAY, not as a struct blob
//!
//! `MdRateEstimationContext` is ~500 KB of `int32_t` tables and the cost
//! functions read seventeen of them. Copying the whole struct across would
//! make both sides depend on C's field OFFSETS — the same ABI coupling that
//! `-DNDEBUG` layout skew is made of. Instead each side walks the SAME
//! ORDERED LIST of tables and packs them end to end; the shim exposes its own
//! element count ([`inter_table_len`] / [`intra_table_len`]) so a mismatch is
//! an assertion, not a silent misread.

/// Number of `i32`s describing one neighbour to the shim — the same encoding
/// `entropy_inter` uses, because these cost functions call the same contexts.
pub const NB_FIELDS: usize = 10;
/// Number of `i32`s in [`inter_fast_cost`]'s scalar description.
pub const IFC_FIELDS: usize = 47;
/// Number of `i32`s in the intra description.
pub const INTRA_FIELDS: usize = 30;
/// Number of `i32`s in [`full_cost`]'s description.
pub const FULL_FIELDS: usize = 11;
/// C `MV_VALS` (cabac_context_model.h:195).
pub const MV_VALS: usize = 32767;

unsafe extern "C" {
    fn ref_rd_nb_fields() -> i32;
    fn ref_rd_inter_fast_cost_fields() -> i32;
    fn ref_rd_intra_fields() -> i32;
    fn ref_rd_full_fields() -> i32;
    fn ref_rd_inter_table_len() -> i32;
    fn ref_rd_intra_table_len() -> i32;
    fn ref_rd_mv_vals() -> i32;
    fn ref_rd_switchable_rate(
        interp_filter: i32,
        rf0: i32,
        rf1: i32,
        interp_filters: i32,
        above: *const i32,
        left: *const i32,
        up_avail: i32,
        left_avail: i32,
        enable_dual_filter: i32,
        tbl: *const i32,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_rd_inter_fast_cost(
        i: *const i32,
        above: *const i32,
        left: *const i32,
        tables: *const i32,
        nmv_vec_cost: *const i32,
        nmv_costs2: *const i32,
        ref_order_hint7: *const i32,
        gm_wmtype8: *const i32,
        stack_weights: *const i32,
        ref_frames_num_bits: u64,
        lambda: u64,
        luma_distortion: u64,
        out: *mut u32,
    ) -> u64;
    fn ref_rd_intra_uv_fast_rate(
        i: *const i32,
        above: *const i32,
        left: *const i32,
        tables: *const i32,
        dv_joint: *const i32,
        dv_cost2: *const i32,
    ) -> u64;
    fn ref_rd_intra_fast_cost(
        i: *const i32,
        above: *const i32,
        left: *const i32,
        tables: *const i32,
        dv_joint: *const i32,
        dv_cost2: *const i32,
        lambda: u64,
        luma_distortion: u64,
        out: *mut u32,
    ) -> u64;
    fn ref_rd_full_cost(
        i: *const i32,
        d: *const u64,
        skip_fac_bits: *const i32,
        skip_mode_fac_bits: *const i32,
        y_coeff_bits: u64,
        cb_coeff_bits: u64,
        cr_coeff_bits: u64,
        lambda: u64,
        out: *mut u64,
    );
    fn ref_rd_full_cost_pd0(
        y_coeff_bits: u64,
        y_distortion: u64,
        skip_fac_bits_00: i32,
        partition_fac_bits_0_none: i32,
        lambda: u64,
    ) -> u64;
}

/// The shim's own view of the layout constants, for the test to assert
/// against its own.
pub fn layout() -> (usize, usize, usize, usize, usize, usize, usize) {
    unsafe {
        (
            ref_rd_nb_fields() as usize,
            ref_rd_inter_fast_cost_fields() as usize,
            ref_rd_intra_fields() as usize,
            ref_rd_full_fields() as usize,
            ref_rd_inter_table_len() as usize,
            ref_rd_intra_table_len() as usize,
            ref_rd_mv_vals() as usize,
        )
    }
}

/// C `svt_aom_get_switchable_rate` (rd_cost.c:849).
#[allow(clippy::too_many_arguments)]
pub fn switchable_rate(
    interp_filter: i32,
    rf: [i32; 2],
    interp_filters: u32,
    above: &[i32; NB_FIELDS],
    left: &[i32; NB_FIELDS],
    up_avail: bool,
    left_avail: bool,
    enable_dual_filter: bool,
    tbl: &[i32],
) -> i32 {
    unsafe {
        ref_rd_switchable_rate(
            interp_filter,
            rf[0],
            rf[1],
            interp_filters as i32,
            above.as_ptr(),
            left.as_ptr(),
            i32::from(up_avail),
            i32::from(left_avail),
            i32::from(enable_dual_filter),
            tbl.as_ptr(),
        )
    }
}

/// C `svt_aom_inter_fast_cost` (rd_cost.c:1005). Returns
/// `(cost, fast_luma_rate, fast_chroma_rate)`.
#[allow(clippy::too_many_arguments)]
pub fn inter_fast_cost(
    fields: &[i32; IFC_FIELDS],
    above: &[i32; NB_FIELDS],
    left: &[i32; NB_FIELDS],
    tables: &[i32],
    nmv_vec_cost: &[i32; 4],
    nmv_costs: &[i32],
    ref_order_hint: &[i32; 7],
    gm_wmtype: &[i32; 8],
    stack_weights: &[i32; 8],
    ref_frames_num_bits: u64,
    lambda: u64,
    luma_distortion: u64,
) -> (u64, u32, u32) {
    assert_eq!(nmv_costs.len(), 2 * MV_VALS);
    let mut out = [0u32; 2];
    let cost = unsafe {
        ref_rd_inter_fast_cost(
            fields.as_ptr(),
            above.as_ptr(),
            left.as_ptr(),
            tables.as_ptr(),
            nmv_vec_cost.as_ptr(),
            nmv_costs.as_ptr(),
            ref_order_hint.as_ptr(),
            gm_wmtype.as_ptr(),
            stack_weights.as_ptr(),
            ref_frames_num_bits,
            lambda,
            luma_distortion,
            out.as_mut_ptr(),
        )
    };
    (cost, out[0], out[1])
}

/// C `svt_aom_get_intra_uv_fast_rate` (rd_cost.c:476).
pub fn intra_uv_fast_rate(
    fields: &[i32; INTRA_FIELDS],
    above: &[i32; NB_FIELDS],
    left: &[i32; NB_FIELDS],
    tables: &[i32],
    dv_joint: &[i32; 4],
    dv_cost: &[i32],
) -> u64 {
    assert_eq!(dv_cost.len(), 2 * MV_VALS);
    unsafe {
        ref_rd_intra_uv_fast_rate(
            fields.as_ptr(),
            above.as_ptr(),
            left.as_ptr(),
            tables.as_ptr(),
            dv_joint.as_ptr(),
            dv_cost.as_ptr(),
        )
    }
}

/// C `svt_aom_intra_fast_cost` (rd_cost.c:526). Returns
/// `(cost, fast_luma_rate, fast_chroma_rate)`.
#[allow(clippy::too_many_arguments)]
pub fn intra_fast_cost(
    fields: &[i32; INTRA_FIELDS],
    above: &[i32; NB_FIELDS],
    left: &[i32; NB_FIELDS],
    tables: &[i32],
    dv_joint: &[i32; 4],
    dv_cost: &[i32],
    lambda: u64,
    luma_distortion: u64,
) -> (u64, u32, u32) {
    assert_eq!(dv_cost.len(), 2 * MV_VALS);
    let mut out = [0u32; 2];
    let cost = unsafe {
        ref_rd_intra_fast_cost(
            fields.as_ptr(),
            above.as_ptr(),
            left.as_ptr(),
            tables.as_ptr(),
            dv_joint.as_ptr(),
            dv_cost.as_ptr(),
            lambda,
            luma_distortion,
            out.as_mut_ptr(),
        )
    };
    (cost, out[0], out[1])
}

/// C `svt_aom_full_cost` (rd_cost.c:1349). Returns
/// `[cost, total_rate, full_dist, full_cost_ssim, forced_coeff_skip, skip_mode]`.
#[allow(clippy::too_many_arguments)]
pub fn full_cost(
    fields: &[i32; FULL_FIELDS],
    dist: &[u64; 12],
    skip_fac_bits: &[i32; 6],
    skip_mode_fac_bits: &[i32; 6],
    y_coeff_bits: u64,
    cb_coeff_bits: u64,
    cr_coeff_bits: u64,
    lambda: u64,
) -> [u64; 6] {
    let mut out = [0u64; 6];
    unsafe {
        ref_rd_full_cost(
            fields.as_ptr(),
            dist.as_ptr(),
            skip_fac_bits.as_ptr(),
            skip_mode_fac_bits.as_ptr(),
            y_coeff_bits,
            cb_coeff_bits,
            cr_coeff_bits,
            lambda,
            out.as_mut_ptr(),
        );
    }
    out
}

/// C `svt_aom_full_cost_pd0` (rd_cost.c:1330).
pub fn full_cost_pd0(
    y_coeff_bits: u64,
    y_distortion: u64,
    skip_fac_bits_00: i32,
    partition_fac_bits_0_none: i32,
    lambda: u64,
) -> u64 {
    unsafe {
        ref_rd_full_cost_pd0(
            y_coeff_bits,
            y_distortion,
            skip_fac_bits_00,
            partition_fac_bits_0_none,
            lambda,
        )
    }
}

// ---------------------------------------------------------------------------
// full_loop.c — the MD-side oracle (shims/full_loop_md_shims.c)
// ---------------------------------------------------------------------------

/// Number of `i32`s in [`do_md_recon`]'s description.
pub const RECON_FIELDS: usize = 13;

unsafe extern "C" {
    fn ref_fl_recon_fields() -> i32;
    fn ref_fl_do_md_recon(i: *const i32) -> i32;
}

/// The shim's own field count, for the test to assert against.
pub fn recon_fields() -> usize {
    unsafe { ref_fl_recon_fields() as usize }
}

/// C `svt_aom_do_md_recon` (full_loop.c:2739).
pub fn do_md_recon(fields: &[i32; RECON_FIELDS]) -> bool {
    unsafe { ref_fl_do_md_recon(fields.as_ptr()) != 0 }
}

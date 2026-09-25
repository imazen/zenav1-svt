// ---- IntraBC (IBC chunks 2-3, docs/ibc-port-map.md) ----

/// `MV_VALS` (cabac_context_model.h): 2 * MV_MAX + 1 with MV_MAX = 2^14 - 1.
pub const MV_VALS: usize = 2 * ((1 << 14) - 1) + 1;
/// `MV_JOINTS`.
pub const MV_JOINTS: usize = 4;
/// Flat u16 length of the C `NmvContext` (asserted against sizeof at runtime).
pub const NMV_FLAT_LEN: usize = 143;

unsafe extern "C" {
    pub(super) fn ref_is_dv_valid(
        dv_x: i16,
        dv_y: i16,
        mi_row: i32,
        mi_col: i32,
        bsize: i32,
        mib_size_log2: i32,
        tile_row_start: i32,
        tile_row_end: i32,
        tile_col_start: i32,
        tile_col_end: i32,
    ) -> i32;
    pub(super) fn ref_find_ref_dv(
        tile_row_start: i32,
        mib_size: i32,
        mi_row: i32,
        mi_col: i32,
    ) -> u32;
    pub(super) fn ref_qp_based_th_scaling_factors(
        enable: i32,
        qp: u32,
        q_weight: *mut u32,
        q_weight_denom: *mut u32,
    );
    pub(super) fn ref_nmv_context_flat_len() -> usize;
    pub(super) fn ref_estimate_mv_rate(
        approx_inter_rate: i32,
        allow_intrabc: i32,
        allow_high_precision_mv: i32,
        nmvc_flat: *const u16,
        ndvc_flat: *const u16,
        nmv_joint: *mut i32,
        nmv_costs: *mut i32,
        dv_joint: *mut i32,
        dv_costs: *mut i32,
    );
    pub(super) fn ref_mv_bit_cost(
        mv_x: i16,
        mv_y: i16,
        ref_x: i16,
        ref_y: i16,
        mvjcost: *const i32,
        mvcost0_full: *const i32,
        mvcost1_full: *const i32,
        weight: i32,
    ) -> i32;
    pub(super) fn ref_mv_bit_cost_light(mv_x: i16, mv_y: i16, ref_x: i16, ref_y: i16) -> i32;
    pub(super) fn ref_mv_err_cost(
        mv_x: i16,
        mv_y: i16,
        ref_x: i16,
        ref_y: i16,
        mvjcost: *const i32,
        mvcost0_full: *const i32,
        mvcost1_full: *const i32,
        error_per_bit: i32,
    ) -> i32;
    pub(super) fn ref_mv_err_cost_light(mv_x: i16, mv_y: i16, ref_x: i16, ref_y: i16) -> i32;
    pub(super) fn ref_estimate_syntax_rate_intrabc(
        intrabc_cdf3: *const u16,
        allow_intrabc: i32,
        fac_out2: *mut i32,
    );
}

/// Reference `svt_aom_is_dv_valid` (adaptive_mv_pred.c:1908). `dv` is
/// `(x, y)` eighth-pel; `tile` is `(row_start, row_end, col_start, col_end)`
/// in MI units.
#[allow(clippy::too_many_arguments)]
pub fn is_dv_valid(
    dv: (i16, i16),
    mi_row: i32,
    mi_col: i32,
    bsize: i32,
    mib_size_log2: i32,
    tile: (i32, i32, i32, i32),
) -> bool {
    unsafe {
        ref_is_dv_valid(
            dv.0,
            dv.1,
            mi_row,
            mi_col,
            bsize,
            mib_size_log2,
            tile.0,
            tile.1,
            tile.2,
            tile.3,
        ) != 0
    }
}

/// Reference `svt_aom_find_ref_dv` (inter_prediction.c:2390) → `(x, y)`.
pub fn find_ref_dv(tile_row_start: i32, mib_size: i32, mi_row: i32, mi_col: i32) -> (i16, i16) {
    let packed = unsafe { ref_find_ref_dv(tile_row_start, mib_size, mi_row, mi_col) };
    // C Mv union: x = low half, y = high half (little-endian in-memory order).
    ((packed & 0xFFFF) as i16, (packed >> 16) as i16)
}

/// Reference `svt_aom_get_qp_based_th_scaling_factors` (enc_mode_config.c:25).
pub fn qp_based_th_scaling_factors(enable: bool, qp: u32) -> (u32, u32) {
    let (mut w, mut d) = (0u32, 0u32);
    unsafe { ref_qp_based_th_scaling_factors(i32::from(enable), qp, &mut w, &mut d) };
    (w, d)
}

/// The C `svt_aom_estimate_mv_rate` outputs (md_rate_estimation.c:458).
pub struct MvRateTables {
    /// `nmv_vec_cost[MV_JOINTS]`.
    pub nmv_joint: [i32; MV_JOINTS],
    /// The SELECTED (hp or non-hp) component stacks, full `[2][MV_VALS]`.
    pub nmv_costs: Vec<i32>,
    /// `dv_joint_cost[MV_JOINTS]`.
    pub dv_joint: [i32; MV_JOINTS],
    /// `dv_cost`, full `[2][MV_VALS]`.
    pub dv_costs: Vec<i32>,
}

/// Reference `svt_aom_estimate_mv_rate`. `nmvc_flat`/`ndvc_flat` are the
/// 143-u16 `NmvContext` serializations (c_parity_mv.rs field order == C
/// struct layout); `None` keeps the C default context. The dv outputs are
/// seeded with `dv_sentinel` so the `!allow_intrabc` / `approx_inter_rate`
/// "left unfilled" arms are observable.
pub fn estimate_mv_rate(
    approx_inter_rate: bool,
    allow_intrabc: bool,
    allow_high_precision_mv: bool,
    nmvc_flat: Option<&[u16]>,
    ndvc_flat: Option<&[u16]>,
    dv_sentinel: i32,
) -> MvRateTables {
    assert_eq!(unsafe { ref_nmv_context_flat_len() }, NMV_FLAT_LEN);
    for f in [nmvc_flat, ndvc_flat].into_iter().flatten() {
        assert_eq!(f.len(), NMV_FLAT_LEN);
    }
    let mut out = MvRateTables {
        nmv_joint: [0; MV_JOINTS],
        nmv_costs: vec![0i32; 2 * MV_VALS],
        dv_joint: [dv_sentinel; MV_JOINTS],
        dv_costs: vec![dv_sentinel; 2 * MV_VALS],
    };
    unsafe {
        ref_estimate_mv_rate(
            i32::from(approx_inter_rate),
            i32::from(allow_intrabc),
            i32::from(allow_high_precision_mv),
            nmvc_flat.map_or(core::ptr::null(), |f| f.as_ptr()),
            ndvc_flat.map_or(core::ptr::null(), |f| f.as_ptr()),
            out.nmv_joint.as_mut_ptr(),
            out.nmv_costs.as_mut_ptr(),
            out.dv_joint.as_mut_ptr(),
            out.dv_costs.as_mut_ptr(),
        );
    }
    out
}

/// Reference `svt_av1_mv_bit_cost` (rd_cost.c:70). `mvcost0/1` are FULL
/// `MV_VALS` tables (the shim applies the `+MV_MAX` mid-table offset).
pub fn mv_bit_cost(
    mv: (i16, i16),
    ref_mv: (i16, i16),
    mvjcost: &[i32; MV_JOINTS],
    mvcost0: &[i32],
    mvcost1: &[i32],
    weight: i32,
) -> i32 {
    assert_eq!(mvcost0.len(), MV_VALS);
    assert_eq!(mvcost1.len(), MV_VALS);
    unsafe {
        ref_mv_bit_cost(
            mv.0,
            mv.1,
            ref_mv.0,
            ref_mv.1,
            mvjcost.as_ptr(),
            mvcost0.as_ptr(),
            mvcost1.as_ptr(),
            weight,
        )
    }
}

/// Reference `svt_av1_mv_bit_cost_light` (rd_cost.c:59).
pub fn mv_bit_cost_light(mv: (i16, i16), ref_mv: (i16, i16)) -> i32 {
    unsafe { ref_mv_bit_cost_light(mv.0, mv.1, ref_mv.0, ref_mv.1) }
}

/// Reference `svt_aom_mv_err_cost` (av1me.c:141).
pub fn mv_err_cost(
    mv: (i16, i16),
    ref_mv: (i16, i16),
    mvjcost: &[i32; MV_JOINTS],
    mvcost0: &[i32],
    mvcost1: &[i32],
    error_per_bit: i32,
) -> i32 {
    assert_eq!(mvcost0.len(), MV_VALS);
    assert_eq!(mvcost1.len(), MV_VALS);
    unsafe {
        ref_mv_err_cost(
            mv.0,
            mv.1,
            ref_mv.0,
            ref_mv.1,
            mvjcost.as_ptr(),
            mvcost0.as_ptr(),
            mvcost1.as_ptr(),
            error_per_bit,
        )
    }
}

/// Reference `svt_aom_mv_err_cost_light` (av1me.c:126).
pub fn mv_err_cost_light(mv: (i16, i16), ref_mv: (i16, i16)) -> i32 {
    unsafe { ref_mv_err_cost_light(mv.0, mv.1, ref_mv.0, ref_mv.1) }
}

/// Reference `svt_aom_estimate_syntax_rate`'s intrabc_fac_bits slice
/// (md_rate_estimation.c:253-255). `intrabc_cdf3 = None` keeps the C
/// default CDF. Returns the fac bits, pre-seeded with `sentinel` so the
/// `!allow_intrabc` "left untouched" arm is observable.
pub fn estimate_syntax_rate_intrabc(
    intrabc_cdf3: Option<&[u16; 3]>,
    allow_intrabc: bool,
    sentinel: i32,
) -> [i32; 2] {
    let mut fac = [sentinel; 2];
    unsafe {
        ref_estimate_syntax_rate_intrabc(
            intrabc_cdf3.map_or(core::ptr::null(), |c| c.as_ptr()),
            i32::from(allow_intrabc),
            fac.as_mut_ptr(),
        );
    }
    fac
}

// ---------------------------------------------------------------------------
// IntraBC hash table (IBC chunk 4)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_crc32c(buf: *const u8, len: usize) -> u32;
    pub(super) fn ref_generate_block_2x2_hash(
        pic: *const u8,
        stride: i32,
        w: i32,
        h: i32,
        dst: *mut u32,
    );
    pub(super) fn ref_generate_block_hash(
        w: i32,
        h: i32,
        block_size: i32,
        src: *const u32,
        dst: *mut u32,
    );
    pub(super) fn ref_hash_table_create() -> *mut core::ffi::c_void;
    pub(super) fn ref_hash_table_add(
        table: *mut core::ffi::c_void,
        pic_hash: *const u32,
        pic_width: i32,
        pic_height: i32,
        block_size: i32,
        max_cand_per_bucket: u16,
    );
    pub(super) fn ref_hash_table_count(table: *mut core::ffi::c_void, hash_value1: u32) -> i32;
    pub(super) fn ref_hash_table_read_bucket(
        table: *mut core::ffi::c_void,
        hash_value1: u32,
        xs: *mut i16,
        ys: *mut i16,
        hv2s: *mut u32,
        cap: i32,
    ) -> i32;
    pub(super) fn ref_hash_table_destroy(table: *mut core::ffi::c_void);
    pub(super) fn ref_get_block_hash_value(
        src: *const u8,
        stride: i32,
        block_size: i32,
        hash_value1: *mut u32,
        hash_value2: *mut u32,
    );
}

/// Reference CRC-32C (`svt_av1_get_crc32c_value_c`, hash.c:55).
pub fn crc32c(buf: &[u8]) -> u32 {
    unsafe { ref_crc32c(buf.as_ptr(), buf.len()) }
}

/// Reference `svt_av1_generate_block_2x2_hash_value` (hash_motion.c:153),
/// bd8. Returns the full `w*h` array (last row/col positions untouched by
/// C — pre-seeded 0 on both sides for comparison of the valid region).
pub fn generate_block_2x2_hash(pic: &[u8], stride: usize, w: usize, h: usize) -> Vec<u32> {
    assert!(pic.len() >= stride * h);
    let mut dst = vec![0u32; w * h];
    unsafe {
        ref_generate_block_2x2_hash(
            pic.as_ptr(),
            stride as i32,
            w as i32,
            h as i32,
            dst.as_mut_ptr(),
        );
    }
    dst
}

/// Reference `svt_av1_generate_block_hash_value` (hash_motion.c:192).
pub fn generate_block_hash(w: usize, h: usize, block_size: usize, src: &[u32]) -> Vec<u32> {
    assert!(src.len() >= w * h);
    let mut dst = vec![0u32; w * h];
    unsafe {
        ref_generate_block_hash(
            w as i32,
            h as i32,
            block_size as i32,
            src.as_ptr(),
            dst.as_mut_ptr(),
        );
    }
    dst
}

/// Owned handle to a C-side `HashTable` (create + per-size add + bucket
/// readback), for chunk-4 order differentials and as the C-side table
/// input to the chunk-5 `svt_av1_intrabc_hash_search` differential.
pub struct CHashTable {
    pub(super) ptr: *mut core::ffi::c_void,
}

impl CHashTable {
    pub fn new() -> Self {
        Self {
            ptr: unsafe { ref_hash_table_create() },
        }
    }

    /// `svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data`.
    pub fn add(
        &mut self,
        pic_hash: &[u32],
        pic_width: usize,
        pic_height: usize,
        block_size: usize,
        max_cand_per_bucket: u16,
    ) {
        assert!(pic_hash.len() >= pic_width * pic_height);
        unsafe {
            ref_hash_table_add(
                self.ptr,
                pic_hash.as_ptr(),
                pic_width as i32,
                pic_height as i32,
                block_size as i32,
                max_cand_per_bucket,
            );
        }
    }

    /// `svt_av1_hash_table_count`.
    pub fn count(&self, hash_value1: u32) -> usize {
        let c = unsafe { ref_hash_table_count(self.ptr, hash_value1) };
        usize::try_from(c.max(0)).unwrap()
    }

    /// Bucket entries `(x, y, hash_value2)` in ITERATION order.
    pub fn bucket(&self, hash_value1: u32) -> Vec<(i16, i16, u32)> {
        let count = unsafe { ref_hash_table_count(self.ptr, hash_value1) };
        if count <= 0 {
            return Vec::new();
        }
        let mut xs = vec![0i16; count as usize];
        let mut ys = vec![0i16; count as usize];
        let mut hv2s = vec![0u32; count as usize];
        let got = unsafe {
            ref_hash_table_read_bucket(
                self.ptr,
                hash_value1,
                xs.as_mut_ptr(),
                ys.as_mut_ptr(),
                hv2s.as_mut_ptr(),
                count,
            )
        };
        assert_eq!(got, count);
        (0..count as usize)
            .map(|i| (xs[i], ys[i], hv2s[i]))
            .collect()
    }

    pub(crate) fn raw(&self) -> *mut core::ffi::c_void {
        self.ptr
    }
}

impl Default for CHashTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CHashTable {
    fn drop(&mut self) {
        unsafe { ref_hash_table_destroy(self.ptr) };
    }
}

/// Reference `svt_av1_get_block_hash_value` (hash_motion.c:309), bd8.
pub fn get_block_hash_value(src: &[u8], stride: usize, block_size: usize) -> (u32, u32) {
    assert!(src.len() >= stride * (block_size - 1) + block_size);
    let (mut hv1, mut hv2) = (0u32, 0u32);
    unsafe {
        ref_get_block_hash_value(
            src.as_ptr(),
            stride as i32,
            block_size as i32,
            &mut hv1,
            &mut hv2,
        );
    }
    (hv1, hv2)
}

// ---------------------------------------------------------------------------
// IntraBC DV search (IBC chunk 5)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_mefn_sdf(
        bsize: i32,
        src: *const u8,
        src_stride: i32,
        r: *const u8,
        ref_stride: i32,
    ) -> u32;
    pub(super) fn ref_mefn_vf(
        bsize: i32,
        src: *const u8,
        src_stride: i32,
        r: *const u8,
        ref_stride: i32,
    ) -> u32;
    pub(super) fn ref_diamond_search(
        pic: *const u8,
        stride: i32,
        x_pos: i32,
        y_pos: i32,
        bsize: i32,
        center_x_ep: i32,
        center_y_ep: i32,
        search_param: i32,
        sad_per_bit: i32,
        col_min: i32,
        col_max: i32,
        row_min: i32,
        row_max: i32,
        dv_joint: *const i32,
        dv_cost0: *const i32,
        dv_cost1: *const i32,
        errorperbit: i32,
        approx_inter_rate: i32,
        out_x: *mut i32,
        out_y: *mut i32,
        out_num00: *mut i32,
    ) -> i32;
    pub(super) fn ref_full_pixel_search(
        pic: *const u8,
        stride: i32,
        x_pos: i32,
        y_pos: i32,
        bsize: i32,
        ref_mv_x_ep: i32,
        ref_mv_y_ep: i32,
        sad_per_bit: i32,
        col_min: i32,
        col_max: i32,
        row_min: i32,
        row_max: i32,
        exhaustive_mesh_thresh: u64,
        mesh_search_mv_diff_threshold: i32,
        mesh_patterns8: *const i32,
        dv_joint: *const i32,
        dv_cost0: *const i32,
        dv_cost1: *const i32,
        errorperbit: i32,
        approx_inter_rate: i32,
        out_x: *mut i32,
        out_y: *mut i32,
    );
    pub(super) fn ref_intrabc_hash_search(
        pic: *const u8,
        stride: i32,
        x_pos: i32,
        y_pos: i32,
        bsize: i32,
        ref_mv_x_ep: i32,
        ref_mv_y_ep: i32,
        hash_table: *mut core::ffi::c_void,
        max_block_size_hash: i32,
        sb_size_log2: i32,
        tile_row_start: i32,
        tile_row_end: i32,
        tile_col_start: i32,
        tile_col_end: i32,
        col_min: i32,
        col_max: i32,
        row_min: i32,
        row_max: i32,
        dv_joint: *const i32,
        dv_cost0: *const i32,
        dv_cost1: *const i32,
        errorperbit: i32,
        approx_inter_rate: i32,
        out_x: *mut i32,
        out_y: *mut i32,
    ) -> i32;
    pub(super) fn ref_intra_bc_search_driver(
        pic: *const u8,
        stride: i32,
        bsize: i32,
        bw: i32,
        bh: i32,
        mi_row: i32,
        mi_col: i32,
        mi_rows: i32,
        mi_cols: i32,
        sb_mi_size: i32,
        sb_size_log2: i32,
        tile_row_start: i32,
        tile_row_end: i32,
        tile_col_start: i32,
        tile_col_end: i32,
        dv_ref_x: i32,
        dv_ref_y: i32,
        search_dir: i32,
        max_block_size_hash: i32,
        exhaustive_mesh_thresh: u64,
        mesh_search_mv_diff_threshold: i32,
        mesh_patterns8: *const i32,
        hash_table_or_null: *mut core::ffi::c_void,
        sadperbit16: i32,
        errorperbit: i32,
        dv_joint: *const i32,
        dv_cost0: *const i32,
        dv_cost1: *const i32,
        approx_inter_rate: i32,
        out_dv: *mut i16,
    ) -> i32;
}

/// The exact per-bsize SAD kernel the C search binds (`svt_aom_mefn_ptr[bsize].sdf`).
pub fn mefn_sdf(bsize: usize, src: &[u8], src_stride: usize, r: &[u8], ref_stride: usize) -> u32 {
    unsafe {
        ref_mefn_sdf(
            bsize as i32,
            src.as_ptr(),
            src_stride as i32,
            r.as_ptr(),
            ref_stride as i32,
        )
    }
}

/// The exact per-bsize VARIANCE kernel (`svt_aom_mefn_ptr[bsize].vf`) — the
/// cross-stage DV search metric.
pub fn mefn_vf(bsize: usize, src: &[u8], src_stride: usize, r: &[u8], ref_stride: usize) -> u32 {
    unsafe {
        ref_mefn_vf(
            bsize as i32,
            src.as_ptr(),
            src_stride as i32,
            r.as_ptr(),
            ref_stride as i32,
        )
    }
}

/// Cost tables + context scalars shared by the search oracles. `dv_joint`
/// is `[i32; 4]` (MV_JOINTS); `dv_cost0`/`dv_cost1` are full un-centered
/// `MV_VALS` arrays (the shim re-centers them at `+MV_MAX`).
pub struct SearchCosts<'a> {
    pub dv_joint: &'a [i32],
    pub dv_cost0: &'a [i32],
    pub dv_cost1: &'a [i32],
    pub errorperbit: i32,
    pub approx_inter_rate: bool,
}

/// Reference `svt_av1_diamond_search_sad_c` (av1me.c:291) in the folded
/// IBC form (seed = center >> 3). Returns (best_x, best_y, bestsad, num00).
#[allow(clippy::too_many_arguments)]
pub fn diamond_search(
    pic: &[u8],
    stride: usize,
    block: (i32, i32),
    bsize: usize,
    center_ep: (i32, i32),
    search_param: i32,
    sad_per_bit: i32,
    limits: (i32, i32, i32, i32),
    costs: &SearchCosts,
) -> (i32, i32, i32, i32) {
    assert_eq!(costs.dv_joint.len(), 4);
    assert_eq!(costs.dv_cost0.len(), MV_VALS);
    assert_eq!(costs.dv_cost1.len(), MV_VALS);
    let (mut ox, mut oy, mut n00) = (0i32, 0i32, 0i32);
    let sad = unsafe {
        ref_diamond_search(
            pic.as_ptr(),
            stride as i32,
            block.0,
            block.1,
            bsize as i32,
            center_ep.0,
            center_ep.1,
            search_param,
            sad_per_bit,
            limits.0,
            limits.1,
            limits.2,
            limits.3,
            costs.dv_joint.as_ptr(),
            costs.dv_cost0.as_ptr(),
            costs.dv_cost1.as_ptr(),
            costs.errorperbit,
            i32::from(costs.approx_inter_rate),
            &mut ox,
            &mut oy,
            &mut n00,
        )
    };
    (ox, oy, sad, n00)
}

/// Reference `svt_av1_full_pixel_search` (av1me.c:1115): diamond +
/// optional mesh. Returns the winning full-pel mv (`x->best_mv`).
#[allow(clippy::too_many_arguments)]
pub fn full_pixel_search(
    pic: &[u8],
    stride: usize,
    block: (i32, i32),
    bsize: usize,
    ref_mv_ep: (i32, i32),
    sad_per_bit: i32,
    limits: (i32, i32, i32, i32),
    exhaustive_mesh_thresh: u64,
    mesh_search_mv_diff_threshold: i32,
    mesh_patterns: &[(i32, i32); 4],
    costs: &SearchCosts,
) -> (i32, i32) {
    let flat: Vec<i32> = mesh_patterns.iter().flat_map(|&(r, i)| [r, i]).collect();
    let (mut ox, mut oy) = (0i32, 0i32);
    unsafe {
        ref_full_pixel_search(
            pic.as_ptr(),
            stride as i32,
            block.0,
            block.1,
            bsize as i32,
            ref_mv_ep.0,
            ref_mv_ep.1,
            sad_per_bit,
            limits.0,
            limits.1,
            limits.2,
            limits.3,
            exhaustive_mesh_thresh,
            mesh_search_mv_diff_threshold,
            flat.as_ptr(),
            costs.dv_joint.as_ptr(),
            costs.dv_cost0.as_ptr(),
            costs.dv_cost1.as_ptr(),
            costs.errorperbit,
            i32::from(costs.approx_inter_rate),
            &mut ox,
            &mut oy,
        );
    }
    (ox, oy)
}

/// Reference `svt_av1_intrabc_hash_search` (av1me.c:1056). Returns
/// `Some((mv_x, mv_y, cost))` on a hash hit (full-pel mv), `None` on miss.
#[allow(clippy::too_many_arguments)]
pub fn intrabc_hash_search(
    pic: &[u8],
    stride: usize,
    block: (i32, i32),
    bsize: usize,
    ref_mv_ep: (i32, i32),
    table: &CHashTable,
    max_block_size_hash: u8,
    sb_size_log2: i32,
    tile: (i32, i32, i32, i32),
    limits: (i32, i32, i32, i32),
    costs: &SearchCosts,
) -> Option<(i32, i32, i32)> {
    let (mut ox, mut oy) = (0i32, 0i32);
    let cost = unsafe {
        ref_intrabc_hash_search(
            pic.as_ptr(),
            stride as i32,
            block.0,
            block.1,
            bsize as i32,
            ref_mv_ep.0,
            ref_mv_ep.1,
            table.raw(),
            i32::from(max_block_size_hash),
            sb_size_log2,
            tile.0,
            tile.1,
            tile.2,
            tile.3,
            limits.0,
            limits.1,
            limits.2,
            limits.3,
            costs.dv_joint.as_ptr(),
            costs.dv_cost0.as_ptr(),
            costs.dv_cost1.as_ptr(),
            costs.errorperbit,
            i32::from(costs.approx_inter_rate),
            &mut ox,
            &mut oy,
        )
    };
    if cost < i32::MAX {
        Some((ox, oy, cost))
    } else {
        None
    }
}

/// The intra_bc_search driver (mode_decision.c:2976-3125) transcribed over
/// the real exported search fns. Returns the eighth-pel DV candidates.
#[allow(clippy::too_many_arguments)]
pub fn intra_bc_search_driver(
    pic: &[u8],
    stride: usize,
    bsize: usize,
    b_dims: (i32, i32),
    mi_pos: (i32, i32),
    mi_dims: (i32, i32),
    sb: (i32, i32),
    tile: (i32, i32, i32, i32),
    dv_ref_ep: (i32, i32),
    search_dir: u8,
    max_block_size_hash: u8,
    exhaustive_mesh_thresh: u64,
    mesh_search_mv_diff_threshold: i32,
    mesh_patterns: &[(i32, i32); 4],
    table: Option<&CHashTable>,
    sadperbit16: i32,
    costs: &SearchCosts,
) -> Vec<(i16, i16)> {
    let flat: Vec<i32> = mesh_patterns.iter().flat_map(|&(r, i)| [r, i]).collect();
    let mut out_dv = [0i16; 4];
    let n = unsafe {
        ref_intra_bc_search_driver(
            pic.as_ptr(),
            stride as i32,
            bsize as i32,
            b_dims.0,
            b_dims.1,
            mi_pos.0,
            mi_pos.1,
            mi_dims.0,
            mi_dims.1,
            sb.0,
            sb.1,
            tile.0,
            tile.1,
            tile.2,
            tile.3,
            dv_ref_ep.0,
            dv_ref_ep.1,
            i32::from(search_dir),
            i32::from(max_block_size_hash),
            exhaustive_mesh_thresh,
            mesh_search_mv_diff_threshold,
            flat.as_ptr(),
            table.map_or(core::ptr::null_mut(), |t| t.raw()),
            sadperbit16,
            costs.errorperbit,
            costs.dv_joint.as_ptr(),
            costs.dv_cost0.as_ptr(),
            costs.dv_cost1.as_ptr(),
            i32::from(costs.approx_inter_rate),
            out_dv.as_mut_ptr(),
        )
    };
    (0..n as usize)
        .map(|i| (out_dv[i * 2], out_dv[i * 2 + 1]))
        .collect()
}

// ---------------------------------------------------------------------------
// IntraBC MVP stack (IBC chunk 6)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_setup_ref_mv_list_intra(
        cells: *const i32,
        grid_rows: i32,
        grid_cols: i32,
        mi_row: i32,
        mi_col: i32,
        bsize_cur: i32,
        mi_rows: i32,
        mi_cols: i32,
        tile_row_start: i32,
        tile_row_end: i32,
        tile_col_start: i32,
        tile_col_end: i32,
        sb_size_is_128: i32,
        stack_out: *mut i32,
        mode_ctx_out: *mut i32,
        nearest_out: *mut u32,
        near_out: *mut u32,
    ) -> i32;
}

/// One packed mode-info grid cell for [`setup_ref_mv_list_intra`]:
/// `(bsize, mode, use_intrabc, ref_frame0, ref_frame1, mv0_as_int, partition)`.
pub type MvpCell = (u8, u8, bool, i8, i8, u32, u8);

/// The result of the C `setup_ref_mv_list` (INTRA_FRAME) +
/// `svt_av1_find_best_ref_mvs_from_stack` chain.
pub struct MvpResult {
    pub count: u8,
    /// All 8 raw stack slots `(this_mv_as_int, weight)` — including the
    /// gm-filled slots beyond `count`.
    pub stack: [(u32, i32); 8],
    pub mode_context: i16,
    pub nearest: u32,
    pub near: u32,
}

/// Reference `setup_ref_mv_list` (adaptive_mv_pred.c:651, EXPORTED) for
/// `INTRA_FRAME` on a packed KEY-frame grid, + the from-stack read.
#[allow(clippy::too_many_arguments)]
pub fn setup_ref_mv_list_intra(
    cells: &[MvpCell],
    grid_rows: usize,
    grid_cols: usize,
    mi_pos: (i32, i32),
    bsize_cur: usize,
    mi_dims: (i32, i32),
    tile: (i32, i32, i32, i32),
    sb_size_is_128: bool,
) -> MvpResult {
    assert_eq!(cells.len(), grid_rows * grid_cols);
    let packed: Vec<i32> = cells
        .iter()
        .flat_map(|&(bsize, mode, ibc, r0, r1, mv, part)| {
            [
                i32::from(bsize),
                i32::from(mode),
                i32::from(ibc),
                i32::from(r0),
                i32::from(r1),
                mv as i32,
                i32::from(part),
            ]
        })
        .collect();
    let mut stack_out = [0i32; 16];
    let mut mode_ctx = 0i32;
    let (mut nearest, mut near) = (0u32, 0u32);
    let count = unsafe {
        ref_setup_ref_mv_list_intra(
            packed.as_ptr(),
            grid_rows as i32,
            grid_cols as i32,
            mi_pos.0,
            mi_pos.1,
            bsize_cur as i32,
            mi_dims.0,
            mi_dims.1,
            tile.0,
            tile.1,
            tile.2,
            tile.3,
            i32::from(sb_size_is_128),
            stack_out.as_mut_ptr(),
            &mut mode_ctx,
            &mut nearest,
            &mut near,
        )
    };
    let mut stack = [(0u32, 0i32); 8];
    for (i, slot) in stack.iter_mut().enumerate() {
        *slot = (stack_out[i * 2] as u32, stack_out[i * 2 + 1]);
    }
    MvpResult {
        count: count as u8,
        stack,
        mode_context: mode_ctx as i16,
        nearest,
        near,
    }
}

//! FFI bindings for the INTER MVP oracle (inter campaign chunk C2,
//! `rust/docs/INTER-ENCODE-PLAN.md` §2).
//!
//! Backed by `shims/inter_mvp_shims.c`, which drives the REAL exported C
//! symbols `setup_ref_mv_list`, `svt_aom_gm_get_motion_vector_enc`,
//! `svt_aom_compute_inter_mode_ctx_light`, `svt_aom_get_av1_mv_pred_drl`
//! and `svt_av1_find_best_ref_mvs_from_stack` — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane
//! never shares an editable file with the concurrent chunk-C3 lane.

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_setup_ref_mv_list_inter(
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
        ref_frame: i32,
        gm_wmtype: *const i32,
        gm_wmmat: *const i32,
        sign_bias: *const i32,
        allow_high_precision_mv: i32,
        force_integer_mv: i32,
        use_ref_frame_mvs: i32,
        enable_order_hint: i32,
        order_hint_bits: i32,
        cur_order_hint: i32,
        ref_order_hint: *const i32,
        tpl_cells: *const i32,
        tpl_n: i32,
        mi_stride_full: i32,
        sb64_sq_no4xn_geom: i32,
        symmetric_refs: i32,
        stack_out: *mut i32,
        mode_ctx_out: *mut i32,
        nearest_out: *mut u32,
        near_out: *mut u32,
        mv_ref0_out: *mut u32,
    ) -> i32;

    fn ref_gm_get_motion_vector_enc(
        wmtype: i32,
        wmmat: *const i32,
        allow_hp: i32,
        bsize: i32,
        mi_col: i32,
        mi_row: i32,
        is_integer: i32,
    ) -> u32;

    fn ref_compute_inter_mode_ctx_light(
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
        ref_frame: i32,
    ) -> i32;

    fn ref_get_av1_mv_pred_drl(
        stack_in: *const i32,
        refmv_count: i32,
        ref_frame: i32,
        is_compound: i32,
        mode: i32,
        drl_index: i32,
        io: *mut u32,
    );
}

/// One packed mode-info grid cell for the inter oracle:
/// `(bsize, mode, use_intrabc, ref_frame0, ref_frame1, mv0_as_int,
/// mv1_as_int, partition)`.
pub type InterMvpCell = (u8, u8, bool, i8, i8, u32, u32, u8);

/// One packed temporal-MV-field cell: `(mfmv0_as_int, ref_frame_offset)`.
pub type TplCell = (u32, u8);

/// Frame-level knobs the inter MVP path reads (mirrors
/// `svtav1_encoder::inter_mvp::InterMvpEnv`).
#[derive(Debug, Clone)]
pub struct InterMvpEnvC {
    /// `wmtype` per ref frame (0=IDENTITY, 1=TRANSLATION, 2=ROTZOOM, 3=AFFINE).
    pub gm_wmtype: [i32; 8],
    /// `wmmat[6]` per ref frame, row-major `[ref][coeff]`.
    pub gm_wmmat: [[i32; 6]; 8],
    pub ref_frame_sign_bias: [i32; 8],
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub use_ref_frame_mvs: bool,
    pub enable_order_hint: bool,
    pub order_hint_bits: i32,
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 8],
    pub mi_stride_full: i32,
    pub sb64_sq_no4xn_geom: bool,
    pub symmetric_refs: bool,
}

/// The result of the C `setup_ref_mv_list` (general ref frame) +
/// `svt_av1_find_best_ref_mvs_from_stack` chain.
pub struct InterMvpResult {
    pub count: u8,
    /// All 8 raw stack slots `(this_mv_as_int, comp_mv_as_int, weight)`.
    pub stack: [(u32, u32, i32); 8],
    pub mode_context: i16,
    pub nearest: u32,
    pub near: u32,
    /// C's `Mv mv_ref0[64]` scratch after the MFMV walk.
    pub mv_ref0: [u32; 64],
}

/// Reference `setup_ref_mv_list` (adaptive_mv_pred.c:651, EXPORTED) for an
/// arbitrary `ref_frame` type on a packed inter mode-info grid, with the
/// temporal-MVP block live when `env.use_ref_frame_mvs`.
#[allow(clippy::too_many_arguments)]
pub fn setup_ref_mv_list_inter(
    cells: &[InterMvpCell],
    grid_rows: usize,
    grid_cols: usize,
    mi_pos: (i32, i32),
    bsize_cur: usize,
    mi_dims: (i32, i32),
    tile: (i32, i32, i32, i32),
    sb_size_is_128: bool,
    ref_frame: i8,
    tpl: &[TplCell],
    env: &InterMvpEnvC,
) -> InterMvpResult {
    assert_eq!(cells.len(), grid_rows * grid_cols);
    let packed: Vec<i32> = cells
        .iter()
        .flat_map(|&(bsize, mode, ibc, r0, r1, mv0, mv1, part)| {
            [
                i32::from(bsize),
                i32::from(mode),
                i32::from(ibc),
                i32::from(r0),
                i32::from(r1),
                mv0 as i32,
                mv1 as i32,
                i32::from(part),
            ]
        })
        .collect();
    let tpl_packed: Vec<i32> = tpl
        .iter()
        .flat_map(|&(mfmv0, off)| [mfmv0 as i32, i32::from(off)])
        .collect();
    let gm_wmmat_flat: Vec<i32> = env.gm_wmmat.iter().flat_map(|r| *r).collect();

    let mut stack_out = [0i32; 24];
    let mut mode_ctx = 0i32;
    let (mut nearest, mut near) = (0u32, 0u32);
    let mut mv_ref0 = [0u32; 64];
    let count = unsafe {
        ref_setup_ref_mv_list_inter(
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
            i32::from(ref_frame),
            env.gm_wmtype.as_ptr(),
            gm_wmmat_flat.as_ptr(),
            env.ref_frame_sign_bias.as_ptr(),
            i32::from(env.allow_high_precision_mv),
            i32::from(env.force_integer_mv),
            i32::from(env.use_ref_frame_mvs),
            i32::from(env.enable_order_hint),
            env.order_hint_bits,
            env.cur_order_hint,
            env.ref_order_hint.as_ptr(),
            tpl_packed.as_ptr(),
            tpl.len() as i32,
            env.mi_stride_full,
            i32::from(env.sb64_sq_no4xn_geom),
            i32::from(env.symmetric_refs),
            stack_out.as_mut_ptr(),
            &mut mode_ctx,
            &mut nearest,
            &mut near,
            mv_ref0.as_mut_ptr(),
        )
    };
    let mut stack = [(0u32, 0u32, 0i32); 8];
    for (i, slot) in stack.iter_mut().enumerate() {
        *slot = (
            stack_out[i * 3] as u32,
            stack_out[i * 3 + 1] as u32,
            stack_out[i * 3 + 2],
        );
    }
    InterMvpResult {
        count: count as u8,
        stack,
        mode_context: mode_ctx as i16,
        nearest,
        near,
        mv_ref0,
    }
}

/// Reference `svt_aom_gm_get_motion_vector_enc` (adaptive_mv_pred.c:983,
/// EXPORTED). Returns the MV packed as `as_int`.
pub fn gm_get_motion_vector_enc(
    wmtype: i32,
    wmmat: &[i32; 6],
    allow_hp: bool,
    bsize: usize,
    mi_col: i32,
    mi_row: i32,
    is_integer: bool,
) -> u32 {
    unsafe {
        ref_gm_get_motion_vector_enc(
            wmtype,
            wmmat.as_ptr(),
            i32::from(allow_hp),
            bsize as i32,
            mi_col,
            mi_row,
            i32::from(is_integer),
        )
    }
}

/// Reference `svt_aom_compute_inter_mode_ctx_light` (adaptive_mv_pred.c:1138,
/// EXPORTED). Returns `ctx->inter_mode_ctx[ref_frame]`.
#[allow(clippy::too_many_arguments)]
pub fn compute_inter_mode_ctx_light(
    cells: &[InterMvpCell],
    grid_rows: usize,
    grid_cols: usize,
    mi_pos: (i32, i32),
    bsize_cur: usize,
    mi_dims: (i32, i32),
    tile: (i32, i32, i32, i32),
    sb_size_is_128: bool,
    ref_frame: i8,
) -> i16 {
    assert_eq!(cells.len(), grid_rows * grid_cols);
    let packed: Vec<i32> = cells
        .iter()
        .flat_map(|&(bsize, mode, ibc, r0, r1, mv0, mv1, part)| {
            [
                i32::from(bsize),
                i32::from(mode),
                i32::from(ibc),
                i32::from(r0),
                i32::from(r1),
                mv0 as i32,
                mv1 as i32,
                i32::from(part),
            ]
        })
        .collect();
    let out = unsafe {
        ref_compute_inter_mode_ctx_light(
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
            i32::from(ref_frame),
        )
    };
    out as i16
}

/// Reference `svt_aom_get_av1_mv_pred_drl` (adaptive_mv_pred.c:1407,
/// EXPORTED).
///
/// `io` is `[nearest0, nearest1, near0, near1, ref_mv0, ref_mv1]` as
/// `as_int`, in AND out — C leaves some of them untouched on some
/// branches, so the caller's incoming values are load-bearing.
pub fn get_av1_mv_pred_drl(
    stack: &[(u32, u32, i32); 8],
    refmv_count: u8,
    ref_frame: i8,
    is_compound: bool,
    mode: u8,
    drl_index: u8,
    io: &mut [u32; 6],
) {
    let mut packed = [0i32; 24];
    for (i, &(this, comp, w)) in stack.iter().enumerate() {
        packed[i * 3] = this as i32;
        packed[i * 3 + 1] = comp as i32;
        packed[i * 3 + 2] = w;
    }
    unsafe {
        ref_get_av1_mv_pred_drl(
            packed.as_ptr(),
            i32::from(refmv_count),
            i32::from(ref_frame),
            i32::from(is_compound),
            i32::from(mode),
            i32::from(drl_index),
            io.as_mut_ptr(),
        );
    }
}

unsafe extern "C" {
    fn ref_mode_context_analyzer(mode_context: i32, rf0: i32, rf1: i32) -> i32;
    fn ref_count_overlappable_neighbors(
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
    ) -> i32;
}

/// Reference `svt_aom_mode_context_analyzer` (inter_prediction.c:2565,
/// EXPORTED).
pub fn mode_context_analyzer(mode_context: i16, rf: [i8; 2]) -> i16 {
    unsafe {
        ref_mode_context_analyzer(i32::from(mode_context), i32::from(rf[0]), i32::from(rf[1]))
            as i16
    }
}

/// Reference `svt_av1_count_overlappable_neighbors` (adaptive_mv_pred.c:1893,
/// EXPORTED). Returns `blk_ptr->overlappable_neighbors`.
#[allow(clippy::too_many_arguments)]
pub fn count_overlappable_neighbors(
    cells: &[InterMvpCell],
    grid_rows: usize,
    grid_cols: usize,
    mi_pos: (i32, i32),
    bsize_cur: usize,
    mi_dims: (i32, i32),
    tile: (i32, i32, i32, i32),
) -> u32 {
    assert_eq!(cells.len(), grid_rows * grid_cols);
    let packed: Vec<i32> = cells
        .iter()
        .flat_map(|&(bsize, mode, ibc, r0, r1, mv0, mv1, part)| {
            [
                i32::from(bsize),
                i32::from(mode),
                i32::from(ibc),
                i32::from(r0),
                i32::from(r1),
                mv0 as i32,
                mv1 as i32,
                i32::from(part),
            ]
        })
        .collect();
    let out = unsafe {
        ref_count_overlappable_neighbors(
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
        )
    };
    out as u32
}

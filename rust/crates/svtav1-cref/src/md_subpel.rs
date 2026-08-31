//! FFI bindings for `shims/md_subpel_shims.c` — the mcomp.c sub-pixel search
//! oracle and the `svf` / `vf` variance kernels it minimises.
//!
//! Evidence tier 1 (`rust/docs/WORKING-ON-THIS.md` §4): every function here
//! drives real C code out of `libSvtAv1Enc.a`.

use core::ffi::c_int;

/// C `MV_MAX` (`cabac_context_model.h:194`) = `(1 << 14) - 1`.
pub const MV_MAX: i32 = 16383;
/// C `MV_VALS` (`cabac_context_model.h:195`) = `(MV_MAX << 1) + 1`.
pub const MV_VALS: usize = 32767;

unsafe extern "C" {
    fn ref_sub_pixel_variance(
        w: c_int,
        h: c_int,
        a: *const u8,
        a_stride: c_int,
        xoffset: c_int,
        yoffset: c_int,
        b: *const u8,
        b_stride: c_int,
        sse: *mut u32,
    ) -> u32;
    fn ref_subpel_variance_vf(
        w: c_int,
        h: c_int,
        a: *const u8,
        a_stride: c_int,
        b: *const u8,
        b_stride: c_int,
        sse: *mut u32,
    ) -> u32;
    fn ref_fp_mv_err_cost(
        mv_x: c_int,
        mv_y: c_int,
        ref_mv_x: c_int,
        ref_mv_y: c_int,
        mv_cost_type: c_int,
        mvjcost: *const c_int,
        mvcost_row: *const c_int,
        mvcost_col: *const c_int,
        error_per_bit: c_int,
    ) -> c_int;
    fn ref_md_subpel_tree(a: *const RefSubpelArgs) -> u32;
}

/// C `svt_aom_sub_pixel_variance{w}x{h}_c`: `(variance, sse)`.
///
/// `a_base` is the index of the block's (0, 0) inside `a`; the kernel reads
/// `h + 1` rows and `w + 1` columns from there.
#[allow(clippy::too_many_arguments)]
pub fn sub_pixel_variance(
    w: usize,
    h: usize,
    a: &[u8],
    a_base: usize,
    a_stride: usize,
    xoffset: i32,
    yoffset: i32,
    b: &[u8],
    b_base: usize,
    b_stride: usize,
) -> (u32, u32) {
    let mut sse = 0u32;
    let r = unsafe {
        ref_sub_pixel_variance(
            w as c_int,
            h as c_int,
            a.as_ptr().add(a_base),
            a_stride as c_int,
            xoffset,
            yoffset,
            b.as_ptr().add(b_base),
            b_stride as c_int,
            &mut sse,
        )
    };
    assert_ne!(r, u32::MAX, "the C shim has no sub_pixel_variance{w}x{h}");
    (r, sse)
}

/// C `svt_aom_variance{w}x{h}_c`: `(variance, sse)`.
#[allow(clippy::too_many_arguments)]
pub fn variance_vf(
    w: usize,
    h: usize,
    a: &[u8],
    a_base: usize,
    a_stride: usize,
    b: &[u8],
    b_base: usize,
    b_stride: usize,
) -> (u32, u32) {
    let mut sse = 0u32;
    let r = unsafe {
        ref_subpel_variance_vf(
            w as c_int,
            h as c_int,
            a.as_ptr().add(a_base),
            a_stride as c_int,
            b.as_ptr().add(b_base),
            b_stride as c_int,
            &mut sse,
        )
    };
    assert_ne!(r, u32::MAX, "the C shim has no variance{w}x{h}");
    (r, sse)
}

/// C `svt_aom_fp_mv_err_cost` (mcomp.c:775) — the whole `MV_COST_TYPE`
/// dispatch. `mvcost` is `None` to drive the `if (mvcost)` NULL arm.
#[allow(clippy::too_many_arguments)]
pub fn fp_mv_err_cost(
    mv: (i32, i32),
    ref_mv: (i32, i32),
    mv_cost_type: i32,
    mvjcost: &[i32; 4],
    mvcost: Option<(&[i32; MV_VALS], &[i32; MV_VALS])>,
    error_per_bit: i32,
) -> i32 {
    let (row, col) = match mvcost {
        Some((r, c)) => (r.as_ptr(), c.as_ptr()),
        None => (core::ptr::null(), core::ptr::null()),
    };
    unsafe {
        ref_fp_mv_err_cost(
            mv.0,
            mv.1,
            ref_mv.0,
            ref_mv.1,
            mv_cost_type,
            mvjcost.as_ptr(),
            row,
            col,
            error_per_bit,
        )
    }
}

/// Mirrors `RefSubpelArgs` in `shims/md_subpel_shims.c` field-for-field.
#[repr(C)]
struct RefSubpelArgs {
    pruned: c_int,
    use_rtcd: c_int,
    use_ctx: c_int,
    src: *const u8,
    src_stride: c_int,
    ref_alloc: *const u8,
    ref_base: c_int,
    ref_stride: c_int,
    w: c_int,
    h: c_int,
    bsize: c_int,
    allow_hp: c_int,
    forced_stop: c_int,
    iters_per_step: c_int,
    pred_variance_th: c_int,
    abs_th_mult: c_int,
    round_dev_th: c_int,
    skip_diag_refinement: c_int,
    search_stage: c_int,
    list_idx: c_int,
    ref_idx: c_int,
    subpel_search_type: c_int,
    bias_fp: c_int,
    col_min: c_int,
    col_max: c_int,
    row_min: c_int,
    row_max: c_int,
    ref_mv_x: c_int,
    ref_mv_y: c_int,
    mv_cost_type: c_int,
    mvjcost: *const c_int,
    mvcost_row: *const c_int,
    mvcost_col: *const c_int,
    error_per_bit: c_int,
    early_exit_th: c_int,
    pd_pass: c_int,
    mvp_th: c_int,
    hp_mv_th: c_int,
    best_fp_mvp_dist: u32,
    best_fp_mvp_x: c_int,
    best_fp_mvp_y: c_int,
    start_mv_x: c_int,
    start_mv_y: c_int,
    best_mv_x: *mut c_int,
    best_mv_y: *mut c_int,
    distortion: *mut c_int,
    sse1: *mut u32,
    fp_me_dist_out: *mut u32,
}

/// Everything the two entry points take, in port-side spelling.
#[derive(Clone, Copy, Debug)]
pub struct SubpelParams {
    /// `true` -> `svt_av1_find_best_sub_pixel_tree_pruned`, else the unpruned tree.
    pub pruned: bool,
    /// Install `svt_aom_mefn_ptr[bsize]` (this host's SIMD tier) instead of the
    /// `_c` kernels. A positive control, not the oracle the port targets.
    pub use_rtcd: bool,
    /// Allocate a `ModeDecisionContext` and pass it as `ictx`.
    pub use_ctx: bool,
    pub w: usize,
    pub h: usize,
    pub bsize: i32,
    pub src_stride: usize,
    pub ref_base: i64,
    pub ref_stride: usize,
    pub allow_hp: bool,
    pub forced_stop: i32,
    pub iters_per_step: i32,
    pub pred_variance_th: i32,
    pub abs_th_mult: u8,
    pub round_dev_th: i32,
    pub skip_diag_refinement: u8,
    pub search_stage: i32,
    pub list_idx: usize,
    pub ref_idx: usize,
    pub subpel_search_type: i32,
    pub bias_fp: i32,
    pub col_min: i32,
    pub col_max: i32,
    pub row_min: i32,
    pub row_max: i32,
    pub ref_mv: (i32, i32),
    pub mv_cost_type: i32,
    pub error_per_bit: i32,
    pub early_exit_th: i32,
    pub pd_pass: i32,
    pub mvp_th: i32,
    pub hp_mv_th: i32,
    pub best_fp_mvp_dist: u32,
    pub best_fp_mvp: (i32, i32),
    pub start_mv: (i32, i32),
}

/// What C returns and writes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubpelResult {
    pub besterr: u32,
    pub best_mv: (i32, i32),
    pub distortion: i32,
    pub sse1: u32,
    /// `ctx->fp_me_dist[list][ref]` after the call (0 when `use_ctx` is false).
    pub fp_me_dist: u32,
}

/// Drive `svt_av1_find_best_sub_pixel_tree{,_pruned}` on the C side.
pub fn subpel_tree(
    p: &SubpelParams,
    src: &[u8],
    ref_alloc: &[u8],
    mvjcost: &[i32; 4],
    mvcost: Option<(&[i32; MV_VALS], &[i32; MV_VALS])>,
) -> SubpelResult {
    let (row, col) = match mvcost {
        Some((r, c)) => (r.as_ptr(), c.as_ptr()),
        None => (core::ptr::null(), core::ptr::null()),
    };
    let mut best_mv_x: c_int = 0;
    let mut best_mv_y: c_int = 0;
    let mut distortion: c_int = 0;
    let mut sse1: u32 = 0;
    let mut fp_me_dist: u32 = 0;
    let args = RefSubpelArgs {
        pruned: c_int::from(p.pruned),
        use_rtcd: c_int::from(p.use_rtcd),
        use_ctx: c_int::from(p.use_ctx),
        src: src.as_ptr(),
        src_stride: p.src_stride as c_int,
        ref_alloc: ref_alloc.as_ptr(),
        ref_base: p.ref_base as c_int,
        ref_stride: p.ref_stride as c_int,
        w: p.w as c_int,
        h: p.h as c_int,
        bsize: p.bsize,
        allow_hp: c_int::from(p.allow_hp),
        forced_stop: p.forced_stop,
        iters_per_step: p.iters_per_step,
        pred_variance_th: p.pred_variance_th,
        abs_th_mult: c_int::from(p.abs_th_mult),
        round_dev_th: p.round_dev_th,
        skip_diag_refinement: c_int::from(p.skip_diag_refinement),
        search_stage: p.search_stage,
        list_idx: p.list_idx as c_int,
        ref_idx: p.ref_idx as c_int,
        subpel_search_type: p.subpel_search_type,
        bias_fp: p.bias_fp,
        col_min: p.col_min,
        col_max: p.col_max,
        row_min: p.row_min,
        row_max: p.row_max,
        ref_mv_x: p.ref_mv.0,
        ref_mv_y: p.ref_mv.1,
        mv_cost_type: p.mv_cost_type,
        mvjcost: mvjcost.as_ptr(),
        mvcost_row: row,
        mvcost_col: col,
        error_per_bit: p.error_per_bit,
        early_exit_th: p.early_exit_th,
        pd_pass: p.pd_pass,
        mvp_th: p.mvp_th,
        hp_mv_th: p.hp_mv_th,
        best_fp_mvp_dist: p.best_fp_mvp_dist,
        best_fp_mvp_x: p.best_fp_mvp.0,
        best_fp_mvp_y: p.best_fp_mvp.1,
        start_mv_x: p.start_mv.0,
        start_mv_y: p.start_mv.1,
        best_mv_x: &mut best_mv_x,
        best_mv_y: &mut best_mv_y,
        distortion: &mut distortion,
        sse1: &mut sse1,
        fp_me_dist_out: &mut fp_me_dist,
    };
    let besterr = unsafe { ref_md_subpel_tree(&args) };
    assert_ne!(
        besterr,
        u32::MAX,
        "the C shim could not build a {}x{} search (no vf/svf for that size?)",
        p.w,
        p.h
    );
    SubpelResult {
        besterr,
        best_mv: (best_mv_x, best_mv_y),
        distortion,
        sse1,
        fp_me_dist,
    }
}

// Screen-content detection primitives (pic_analysis_process.c) — the leaf
// functions of the AA-aware detector (#71). All exported `T` symbols.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn svt_av1_count_colors_with_threshold(
        src: *const u8,
        stride: i32,
        rows: i32,
        cols: i32,
        num_colors_threshold: i32,
        num_colors: *mut i32,
    ) -> bool;
    pub(super) fn svt_av1_find_dominant_value(
        src: *const u8,
        stride: i32,
        rows: i32,
        cols: i32,
    ) -> u8;
    pub(super) fn svt_av1_dilate_block(
        src: *const u8,
        src_stride: i32,
        dilated: *mut u8,
        dilated_stride: i32,
        rows: i32,
        cols: i32,
    );
}

/// Reference `svt_av1_count_colors_with_threshold` (pic_analysis_process.c:911).
pub fn count_colors_with_threshold(
    src: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    threshold: i32,
) -> (bool, i32) {
    assert!(src.len() >= (rows - 1) * stride + cols);
    let mut n: i32 = 0;
    let ok = unsafe {
        svt_av1_count_colors_with_threshold(
            src.as_ptr(),
            stride as i32,
            rows as i32,
            cols as i32,
            threshold,
            &mut n,
        )
    };
    (ok, n)
}

/// Reference `svt_av1_find_dominant_value` (pic_analysis_process.c:986).
pub fn find_dominant_value(src: &[u8], stride: usize, rows: usize, cols: usize) -> u8 {
    assert!(src.len() >= (rows - 1) * stride + cols);
    unsafe { svt_av1_find_dominant_value(src.as_ptr(), stride as i32, rows as i32, cols as i32) }
}

/// Reference `svt_av1_dilate_block` (pic_analysis_process.c:1024).
pub fn dilate_block(
    src: &[u8],
    src_stride: usize,
    dilated: &mut [u8],
    dilated_stride: usize,
    rows: usize,
    cols: usize,
) {
    assert!(src.len() >= (rows - 1) * src_stride + cols);
    assert!(dilated.len() >= (rows - 1) * dilated_stride + cols);
    unsafe {
        svt_av1_dilate_block(
            src.as_ptr(),
            src_stride as i32,
            dilated.as_mut_ptr(),
            dilated_stride as i32,
            rows as i32,
            cols as i32,
        )
    }
}

// ---------------------------------------------------------------------------
// Palette pipeline primitives (#71 chunk 1): svt_av1_count_colors
// (pic_analysis_process.c:892), svt_av1_index_color_cache /
// svt_av1_k_means_dim1_c / svt_av1_calc_indices_dim1_c (palette.c). All
// exported `T` symbols.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn svt_av1_count_colors(
        src: *const u8,
        stride: i32,
        rows: i32,
        cols: i32,
        val_count: *mut i32,
    ) -> i32;
    pub(super) fn svt_av1_index_color_cache(
        color_cache: *const u16,
        n_cache: i32,
        colors: *const u16,
        n_colors: i32,
        cache_color_found: *mut u8,
        out_cache_colors: *mut i32,
    ) -> i32;
    pub(super) fn svt_av1_k_means_dim1_c(
        data: *const i32,
        centroids: *mut i32,
        indices: *mut u8,
        n: i32,
        k: i32,
        max_itr: i32,
    );
    pub(super) fn svt_av1_calc_indices_dim1_c(
        data: *const i32,
        centroids: *const i32,
        indices: *mut u8,
        n: i32,
        k: i32,
    );
}

/// Reference `svt_av1_count_colors` (pic_analysis_process.c:892). Writes
/// the 256-bin histogram into `val_count` and returns the distinct-color
/// count.
pub fn count_colors(
    src: &[u8],
    stride: usize,
    rows: usize,
    cols: usize,
    val_count: &mut [i32; 256],
) -> i32 {
    assert!(src.len() >= (rows - 1) * stride + cols);
    unsafe {
        svt_av1_count_colors(
            src.as_ptr(),
            stride as i32,
            rows as i32,
            cols as i32,
            val_count.as_mut_ptr(),
        )
    }
}

/// Reference `svt_av1_index_color_cache` (palette.c:111-141).
/// `out_cache_colors` is `int*` in C (arithmetic convenience for the
/// downstream delta-encode cost, not a wider color domain) — kept as
/// `i32` here so the FFI boundary matches the real signature exactly;
/// callers narrow to `u16` when comparing against the Rust port.
pub fn index_color_cache(
    color_cache: &[u16],
    colors: &[u16],
    cache_color_found: &mut [u8],
    out_cache_colors: &mut [i32],
) -> i32 {
    assert!(cache_color_found.len() >= color_cache.len());
    assert!(out_cache_colors.len() >= colors.len());
    unsafe {
        svt_av1_index_color_cache(
            color_cache.as_ptr(),
            color_cache.len() as i32,
            colors.as_ptr(),
            colors.len() as i32,
            cache_color_found.as_mut_ptr(),
            out_cache_colors.as_mut_ptr(),
        )
    }
}

/// Reference `svt_av1_k_means_dim1_c` (k_means_template.h, `dim=1`
/// instantiation via palette.c:55-56). `centroids`/`indices` are mutated
/// in place exactly as the C function does.
pub fn k_means_dim1(
    data: &[i32],
    centroids: &mut [i32],
    indices: &mut [u8],
    k: usize,
    max_itr: i32,
) {
    let n = data.len();
    assert!(indices.len() >= n);
    assert!(centroids.len() >= k);
    unsafe {
        svt_av1_k_means_dim1_c(
            data.as_ptr(),
            centroids.as_mut_ptr(),
            indices.as_mut_ptr(),
            n as i32,
            k as i32,
            max_itr,
        )
    }
}

/// Reference `svt_av1_calc_indices_dim1_c` (k_means_template.h, `dim=1`
/// instantiation via palette.c:55-56).
pub fn calc_indices_dim1(data: &[i32], centroids: &[i32], indices: &mut [u8], k: usize) {
    let n = data.len();
    assert!(indices.len() >= n);
    assert!(centroids.len() >= k);
    unsafe {
        svt_av1_calc_indices_dim1_c(
            data.as_ptr(),
            centroids.as_ptr(),
            indices.as_mut_ptr(),
            n as i32,
            k as i32,
        )
    }
}

// ---- High-bit-depth intra predictors (intra_prediction.c sized macro) ----

unsafe extern "C" {
    pub(super) fn ref_highbd_intra_pred(
        mode: i32,
        dst: *mut u16,
        stride: isize,
        above: *const u16,
        left: *const u16,
        top_left: u16,
        w: i32,
        h: i32,
        bd: i32,
    );
}

/// Which reference high-bit-depth intra predictor to run (the sized
/// `svt_aom_highbd_*_predictor_WxH_c` family, intra_prediction.c:1602).
///
/// C splits DC into four distinct wrappers; the port folds them into a single
/// `hbd::predict_dc_hbd` with a `(has_above, has_left)` flag pair, so these
/// four variants map onto that pair (documented per variant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HbdIntraPred {
    /// `highbd_dc_predictor` — DC of above+left (`has_above && has_left`).
    Dc = 0,
    /// `highbd_dc_top_predictor` — DC of above only (`has_above && !has_left`).
    DcTop = 1,
    /// `highbd_dc_left_predictor` — DC of left only (`!has_above && has_left`).
    DcLeft = 2,
    /// `highbd_dc_128_predictor` — `128 << (bd - 8)` (`!has_above && !has_left`).
    Dc128 = 3,
    /// `highbd_v_predictor` — replicate the above row down every column.
    V = 4,
    /// `highbd_h_predictor` — replicate the left column across every row.
    H = 5,
    /// `highbd_paeth_predictor` — nearest of {left, above, top-left}.
    Paeth = 6,
    /// `highbd_smooth_predictor` — bilinear H+V smooth blend.
    Smooth = 7,
    /// `highbd_smooth_v_predictor` — vertical smooth blend.
    SmoothV = 8,
    /// `highbd_smooth_h_predictor` — horizontal smooth blend.
    SmoothH = 9,
}

/// Run the reference C high-bit-depth intra predictor into a fresh
/// `dst_stride`-strided block of `height` rows, returned as a
/// `height * dst_stride` buffer (only the first `width` columns of each row are
/// written, matching both C and the port's `hbd::predict_*_hbd` output layout).
///
/// `above` holds the `width` samples of the row above the block, `left` holds
/// the `height` samples of the column to its left, and `top_left` is the corner
/// sample (`above[-1]` in C, read only by [`HbdIntraPred::Paeth`]). `bd` is the
/// bit depth in {8, 10, 12}.
#[allow(clippy::too_many_arguments)]
pub fn highbd_intra_pred(
    mode: HbdIntraPred,
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    top_left: u16,
    width: usize,
    height: usize,
    bd: i32,
) -> Vec<u16> {
    assert!(above.len() >= width, "above must hold width samples");
    assert!(left.len() >= height, "left must hold height samples");
    assert!(dst_stride >= width, "dst_stride must be >= width");
    let mut dst = vec![0u16; height * dst_stride];
    unsafe {
        ref_highbd_intra_pred(
            mode as i32,
            dst.as_mut_ptr(),
            dst_stride as isize,
            above.as_ptr(),
            left.as_ptr(),
            top_left,
            width as i32,
            height as i32,
            bd,
        );
    }
    dst
}

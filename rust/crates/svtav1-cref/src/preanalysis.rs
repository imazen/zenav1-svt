//! FFI bindings for the pre-analysis oracle (temporal filtering / noise model /
//! source statistics group).
//!
//! Backed by `shims/preanalysis_shims.c`, which calls the REAL exported C
//! symbols `svt_aom_downsample_2d_c`, `calculate_histogram`,
//! `svt_aom_generate_padding`, `pad_input_picture` and
//! `svt_aom_is_input_luma_dominant` — evidence tier 1
//! (`docs/WORKING-ON-THIS.md` §4).
//!
//! Kept in its own module (and its own C translation unit) so this lane never
//! shares an editable file with the concurrent inter-campaign lanes.

unsafe extern "C" {
    fn ref_pre_downsample_2d(
        input_samples: *mut u8,
        input_stride: u32,
        w: u32,
        h: u32,
        decim_samples: *mut u8,
        decim_stride: u32,
        decim_step: u32,
    );
    fn ref_pre_calculate_histogram(
        input_samples: *mut u8,
        w: u32,
        h: u32,
        stride: u32,
        decim_step: u8,
        histogram: *mut u32,
        sum: *mut u64,
    );
    fn ref_pre_generate_padding(
        buf: *mut u8,
        origin: u32,
        src_stride: u32,
        w: u32,
        h: u32,
        padding_width: u32,
        padding_height: u32,
    );
    fn ref_pre_pad_input_picture(
        src: *mut u8,
        src_stride: u32,
        w: u32,
        h: u32,
        pad_right: u32,
        pad_bottom: u32,
    );
    fn ref_pre_is_input_luma_dominant(
        color_format: u32,
        width: u32,
        height: u32,
        u_buffer: *mut u8,
        u_stride: u32,
        v_buffer: *mut u8,
        v_stride: u32,
    ) -> i32;
}

/// `svt_aom_downsample_2d_c` — 2x2 0-phase averaging decimator.
///
/// `input` starts at C's `input_samples`; `decim` at `decim_samples`.
pub fn downsample_2d(
    input: &[u8],
    input_stride: u32,
    width: u32,
    height: u32,
    decim: &mut [u8],
    decim_stride: u32,
    decim_step: u32,
) {
    let mut src = input.to_vec();
    unsafe {
        ref_pre_downsample_2d(
            src.as_mut_ptr(),
            input_stride,
            width,
            height,
            decim.as_mut_ptr(),
            decim_stride,
            decim_step,
        );
    }
}

/// `calculate_histogram` — n-bin histogram on a `decim_step` lattice.
///
/// `sum` accumulates (C never zeroes it).
pub fn calculate_histogram(
    input: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    decim_step: u8,
    histogram: &mut [u32; 256],
    sum: &mut u64,
) {
    let mut src = input.to_vec();
    unsafe {
        ref_pre_calculate_histogram(
            src.as_mut_ptr(),
            width,
            height,
            stride,
            decim_step,
            histogram.as_mut_ptr(),
            sum,
        );
    }
}

/// `svt_aom_generate_padding` — border replicate around an active area.
///
/// `buf` is the whole allocation; `origin` is the offset of C's `src_pic`.
pub fn generate_padding(
    buf: &mut [u8],
    origin: u32,
    src_stride: u32,
    width: u32,
    height: u32,
    padding_width: u32,
    padding_height: u32,
) {
    unsafe {
        ref_pre_generate_padding(
            buf.as_mut_ptr(),
            origin,
            src_stride,
            width,
            height,
            padding_width,
            padding_height,
        );
    }
}

/// `pad_input_picture` — right-then-bottom pad to a min-block multiple.
pub fn pad_input_picture(
    buf: &mut [u8],
    src_stride: u32,
    width: u32,
    height: u32,
    pad_right: u32,
    pad_bottom: u32,
) {
    unsafe {
        ref_pre_pad_input_picture(
            buf.as_mut_ptr(),
            src_stride,
            width,
            height,
            pad_right,
            pad_bottom,
        );
    }
}

/// `svt_aom_is_input_luma_dominant` — the near-neutral-chroma frame detector.
///
/// `color_format` uses C's `EbColorFormat` numbering (`EB_YUV400 = 0`,
/// `EB_YUV420 = 1`, `EB_YUV422 = 2`, `EB_YUV444 = 3`).
pub fn is_input_luma_dominant(
    color_format: u32,
    width: u32,
    height: u32,
    u_plane: &[u8],
    u_stride: u32,
    v_plane: &[u8],
    v_stride: u32,
) -> bool {
    let mut u = u_plane.to_vec();
    let mut v = v_plane.to_vec();
    let r = unsafe {
        ref_pre_is_input_luma_dominant(
            color_format,
            width,
            height,
            u.as_mut_ptr(),
            u_stride,
            v.as_mut_ptr(),
            v_stride,
        )
    };
    r != 0
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_pre_downsample_filtering_input_picture(
        enable_hme: i32,
        tf_enable_hme: i32,
        enable_hme_l0: i32,
        tf_enable_hme_l0: i32,
        enable_hme_l1: i32,
        tf_enable_hme_l1: i32,
        in_buf: *mut u8,
        in_origin: u32,
        in_stride: u32,
        in_w: u32,
        in_h: u32,
        q_buf: *mut u8,
        q_origin: u32,
        q_stride: u32,
        q_w: u32,
        q_h: u32,
        q_border: u32,
        s_buf: *mut u8,
        s_origin: u32,
        s_stride: u32,
        s_w: u32,
        s_h: u32,
        s_border: u32,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_pre_pad_input_pictures(
        min_blk_only: i32,
        bit_depth: u32,
        color_format: u32,
        subsampling_x: u32,
        subsampling_y: u32,
        pad_right: u32,
        pad_bottom: u32,
        y_buf: *mut u8,
        y_origin: u32,
        y_stride: u32,
        width: u32,
        height: u32,
        border: u32,
        u_buf: *mut u8,
        u_origin: u32,
        u_stride: u32,
        v_buf: *mut u8,
        v_origin: u32,
        v_stride: u32,
    );
}

/// Geometry of one plane as `EbPictureBufferDesc` describes it.
#[derive(Debug, Clone, Copy)]
pub struct PlaneGeom {
    pub origin: u32,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
    pub border: u32,
}

/// `svt_aom_downsample_filtering_input_picture` — fills the 1/4 and 1/16
/// luma planes from the padded input and pads each to its own border.
#[allow(clippy::too_many_arguments)]
pub fn downsample_filtering_input_picture(
    enables: [bool; 6],
    input: &[u8],
    input_geom: PlaneGeom,
    quarter: &mut [u8],
    quarter_geom: PlaneGeom,
    sixteenth: &mut [u8],
    sixteenth_geom: PlaneGeom,
) {
    let mut src = input.to_vec();
    unsafe {
        ref_pre_downsample_filtering_input_picture(
            i32::from(enables[0]),
            i32::from(enables[1]),
            i32::from(enables[2]),
            i32::from(enables[3]),
            i32::from(enables[4]),
            i32::from(enables[5]),
            src.as_mut_ptr(),
            input_geom.origin,
            input_geom.stride,
            input_geom.width,
            input_geom.height,
            quarter.as_mut_ptr(),
            quarter_geom.origin,
            quarter_geom.stride,
            quarter_geom.width,
            quarter_geom.height,
            quarter_geom.border,
            sixteenth.as_mut_ptr(),
            sixteenth_geom.origin,
            sixteenth_geom.stride,
            sixteenth_geom.width,
            sixteenth_geom.height,
            sixteenth_geom.border,
        );
    }
}

/// `svt_aom_pad_input_pictures` (`min_blk_only == false`) or just
/// `svt_aom_pad_picture_to_multiple_of_min_blk_size_dimensions`
/// (`min_blk_only == true`).
#[allow(clippy::too_many_arguments)]
pub fn pad_input_pictures(
    min_blk_only: bool,
    bit_depth: u32,
    color_format: u32,
    subsampling_x: u32,
    subsampling_y: u32,
    pad_right: u32,
    pad_bottom: u32,
    y: &mut [u8],
    y_geom: PlaneGeom,
    u: &mut [u8],
    u_geom: PlaneGeom,
    v: &mut [u8],
    v_geom: PlaneGeom,
) {
    unsafe {
        ref_pre_pad_input_pictures(
            i32::from(min_blk_only),
            bit_depth,
            color_format,
            subsampling_x,
            subsampling_y,
            pad_right,
            pad_bottom,
            y.as_mut_ptr(),
            y_geom.origin,
            y_geom.stride,
            y_geom.width,
            y_geom.height,
            y_geom.border,
            u.as_mut_ptr(),
            u_geom.origin,
            u_geom.stride,
            v.as_mut_ptr(),
            v_geom.origin,
            v_geom.stride,
        );
    }
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_pre_gathering_picture_statistics(
        calc_hist: i32,
        calculate_variance: i32,
        regions_w: u32,
        regions_h: u32,
        scene_change_detection: i32,
        sixteenth: *mut u8,
        s_origin: u32,
        s_stride: u32,
        s_w: u32,
        s_h: u32,
        out_histogram: *mut u32,
        out_avg_intensity: *mut u64,
        out_avg_luma: *mut u64,
        out_pic_avg_variance: *mut u16,
    ) -> i32;
}

/// What `svt_aom_gathering_picture_statistics` wrote into the PCS.
pub struct GatheredStatistics {
    /// `[region_w][region_h][bin]`, flattened row-major.
    pub histogram: Vec<u32>,
    /// `[region_w][region_h]`, flattened row-major.
    pub average_intensity_per_region: Vec<u64>,
    pub avg_luma: u64,
    pub pic_avg_variance: u16,
}

/// `svt_aom_gathering_picture_statistics`.
///
/// Returns `None` when `calculate_variance` is set: that arm walks
/// `pcs->b64_geom` and divides by `pcs->b64_total_count`, which a facade PCS
/// cannot supply. `calculate_variance == 0` is the value the video-mode
/// configuration uses (enc_handle.c:4361-4366).
#[allow(clippy::too_many_arguments)]
pub fn gathering_picture_statistics(
    calc_hist: bool,
    calculate_variance: bool,
    regions_w: u32,
    regions_h: u32,
    scene_change_detection: bool,
    sixteenth: &[u8],
    s_origin: u32,
    s_stride: u32,
    s_w: u32,
    s_h: u32,
) -> Option<GatheredStatistics> {
    let mut src = sixteenth.to_vec();
    let mut histogram = vec![0u32; 4 * 4 * 256];
    let mut average_intensity_per_region = vec![0u64; 4 * 4];
    let mut avg_luma = 0u64;
    let mut pic_avg_variance = 0u16;
    let ok = unsafe {
        ref_pre_gathering_picture_statistics(
            i32::from(calc_hist),
            i32::from(calculate_variance),
            regions_w,
            regions_h,
            i32::from(scene_change_detection),
            src.as_mut_ptr(),
            s_origin,
            s_stride,
            s_w,
            s_h,
            histogram.as_mut_ptr(),
            average_intensity_per_region.as_mut_ptr(),
            &mut avg_luma,
            &mut pic_avg_variance,
        )
    };
    (ok != 0).then_some(GatheredStatistics {
        histogram,
        average_intensity_per_region,
        avg_luma,
        pic_avg_variance,
    })
}

// ---------------------------------------------------------------------------
// Screen-content detection
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_pre_sby_perpixel_variance(src: *const u8, stride: i32, fn_bs: i32, norm_bs: i32) -> u32;
    fn ref_pre_is_screen_content_aa(
        fast_detection: i32,
        y_buf: *mut u8,
        y_origin: u32,
        y_stride: u32,
        width: u32,
        height: u32,
        out: *mut i32,
    );
    fn ref_pre_is_screen_content(
        y_buf: *mut u8,
        y_origin: u32,
        y_stride: u32,
        width: u32,
        height: u32,
        out: *mut i32,
    );
}

/// `BlockSize` values the screen-content detectors use. C's `BlockSize` enum
/// is a plain `int` at the ABI, and only these two reach the detectors, so an
/// enum here is both clearer than a bare integer and impossible to misuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScBlockSize {
    /// `BLOCK_8X8`.
    Blk8x8,
    /// `BLOCK_16X16`.
    Blk16x16,
}

impl ScBlockSize {
    /// The `BlockSize` discriminant, from `block_structures.h`'s enum order.
    fn as_c(self) -> i32 {
        match self {
            // BLOCK_4X4=0, 4X8=1, 8X4=2, 8X8=3, 8X16=4, 16X8=5, 16X16=6.
            Self::Blk8x8 => 3,
            Self::Blk16x16 => 6,
        }
    }

    /// Side length in pixels.
    pub fn side(self) -> usize {
        match self {
            Self::Blk8x8 => 8,
            Self::Blk16x16 => 16,
        }
    }
}

/// `svt_av1_get_sby_perpixel_variance` (pic_analysis_process.c:945) with
/// matched block sizes — the well-formed call.
///
/// Dereferences `svt_aom_mefn_ptr[bs].vf`, so the shim runs BOTH the RTCD
/// setup and `init_fn_ptr()` before the call.
pub fn sby_perpixel_variance(src: &[u8], stride: usize, bs: ScBlockSize) -> u32 {
    sby_perpixel_variance_mixed(src, stride, bs, bs)
}

/// The same C function with the variance kernel and the normalising block
/// size chosen INDEPENDENTLY.
///
/// `svt_aom_is_screen_content` needs this: it binds `fn_ptr` to
/// `&svt_aom_mefn_ptr[BLOCK_16X16]` once (pic_analysis_process.c:1367) and
/// never rebinds it, so its 8x8 pass is `fn_bs = Blk16x16`,
/// `norm_bs = Blk8x8`. `src` must therefore be large enough for a
/// `fn_bs`-sized read, not a `norm_bs`-sized one.
pub fn sby_perpixel_variance_mixed(
    src: &[u8],
    stride: usize,
    fn_bs: ScBlockSize,
    norm_bs: ScBlockSize,
) -> u32 {
    let side = fn_bs.side();
    assert!(
        src.len() >= (side - 1) * stride + side,
        "source too small for the {side}x{side} variance kernel at stride {stride}"
    );
    unsafe {
        ref_pre_sby_perpixel_variance(src.as_ptr(), stride as i32, fn_bs.as_c(), norm_bs.as_c())
    }
}

/// The six `pcs->sc_class{0..5}` bits the detectors write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScClassBits {
    pub sc_class0: bool,
    pub sc_class1: bool,
    pub sc_class2: bool,
    pub sc_class3: bool,
    pub sc_class4: bool,
    pub sc_class5: bool,
}

impl ScClassBits {
    fn from_raw(raw: [i32; 6]) -> Self {
        Self {
            sc_class0: raw[0] != 0,
            sc_class1: raw[1] != 0,
            sc_class2: raw[2] != 0,
            sc_class3: raw[3] != 0,
            sc_class4: raw[4] != 0,
            sc_class5: raw[5] != 0,
        }
    }
}

fn check_plane(y: &[u8], y_stride: usize, width: usize, height: usize) {
    assert!(width <= y_stride, "width {width} exceeds stride {y_stride}");
    assert!(
        y.len() >= (height - 1) * y_stride + width,
        "plane too small for {width}x{height} at stride {y_stride}"
    );
}

/// `svt_aom_is_screen_content_antialiasing_aware` (pic_analysis_process.c:1208)
/// — the LIVE detector (`screen_content_mode == 3`).
pub fn is_screen_content_antialiasing_aware(
    y: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
    fast_detection: bool,
) -> ScClassBits {
    check_plane(y, y_stride, width, height);
    let mut buf = y.to_vec();
    let mut raw = [0i32; 6];
    unsafe {
        ref_pre_is_screen_content_aa(
            i32::from(fast_detection),
            buf.as_mut_ptr(),
            0,
            y_stride as u32,
            width as u32,
            height as u32,
            raw.as_mut_ptr(),
        );
    }
    ScClassBits::from_raw(raw)
}

/// `svt_aom_is_screen_content` (pic_analysis_process.c:1355) — the
/// `screen_content_mode == 2` detector, which v4.2.0 never selects (see the
/// port's reachability note). Never writes `sc_class5`, so it stays `false`.
///
/// `y` must carry EIGHT extra rows and columns beyond `width` x `height`.
/// C's 8x8 pass calls the 16x16 variance kernel (its `fn_ptr` is bound once to
/// `BLOCK_16X16` and never rebound), so at the last 8x8 block it reads to row
/// `height + 7` and column `width + 7` — into the picture border the encoder
/// always supplies. Without those rows the oracle reads whatever follows the
/// buffer and the differential becomes nondeterministic, so this is checked
/// rather than hoped for.
pub fn is_screen_content(y: &[u8], y_stride: usize, width: usize, height: usize) -> ScClassBits {
    check_plane(y, y_stride, width, height);
    assert!(
        width + 8 <= y_stride && y.len() >= (height + 7) * y_stride + width + 8,
        "the scm-2 detector reads 8 rows/cols past {width}x{height}; supply the border"
    );
    let mut buf = y.to_vec();
    let mut raw = [0i32; 6];
    unsafe {
        ref_pre_is_screen_content(
            buf.as_mut_ptr(),
            0,
            y_stride as u32,
            width as u32,
            height as u32,
            raw.as_mut_ptr(),
        );
    }
    ScClassBits::from_raw(raw)
}

// ---------------------------------------------------------------------------
// src_ops_process.c per-block variance measures
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sops_get_perpixel_variance(buf: *const u8, stride: u32, block_size: i32) -> u32;
    fn ref_sops_get_mean_and_perpixel_variance(
        buf: *const u8,
        stride: u32,
        block_size: i32,
        perpixel_var: *mut u32,
        mean: *mut u32,
    );
    fn ref_sops_get_perceptual_perpixel_variance(
        buf: *const u8,
        stride: u32,
        block_size: i32,
    ) -> u32;
}

/// `svt_aom_get_perpixel_variance` (src_ops_process.c:2129).
///
/// `block_size` is a `BlockSize` discriminant; the caller supplies it, since
/// these take any block size rather than the two the screen-content detectors
/// use.
pub fn sops_get_perpixel_variance(buf: &[u8], stride: usize, block_size: i32, rows: usize) -> u32 {
    assert!(buf.len() >= (rows - 1) * stride + 1);
    unsafe { ref_sops_get_perpixel_variance(buf.as_ptr(), stride as u32, block_size) }
}

/// `svt_aom_get_mean_and_perpixel_variance`. Returns `(perpixel_var, mean)`.
pub fn sops_get_mean_and_perpixel_variance(
    buf: &[u8],
    stride: usize,
    block_size: i32,
    rows: usize,
) -> (u32, u32) {
    assert!(buf.len() >= (rows - 1) * stride + 1);
    let (mut var, mut mean) = (0u32, 0u32);
    unsafe {
        ref_sops_get_mean_and_perpixel_variance(
            buf.as_ptr(),
            stride as u32,
            block_size,
            &mut var,
            &mut mean,
        );
    }
    (var, mean)
}

/// `svt_aom_get_perceptual_perpixel_variance`.
pub fn sops_get_perceptual_perpixel_variance(
    buf: &[u8],
    stride: usize,
    block_size: i32,
    rows: usize,
) -> u32 {
    assert!(buf.len() >= (rows - 1) * stride + 1);
    unsafe { ref_sops_get_perceptual_perpixel_variance(buf.as_ptr(), stride as u32, block_size) }
}

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

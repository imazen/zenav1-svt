use super::*;

/// C `RestorationTileLimits` (restoration.h:259).
#[derive(Clone, Copy, Debug)]
pub struct TileLimits {
    pub h_start: i32,
    pub h_end: i32,
    pub v_start: i32,
    pub v_end: i32,
}

/// C `Av1PixelRect` (restoration.h:193).
#[derive(Clone, Copy, Debug)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// C `RestorationStripeBoundaries` (restoration.h:217) at either bit depth.
/// Buffer column `i` corresponds to plane column `i - RESTORATION_EXTRA_HORZ`;
/// row `RESTORATION_CTX_VERT * frame_stripe + j` holds the j-th saved line
/// of that stripe's boundary.
///
/// C keeps ONE `uint8_t*` buffer and scales every byte offset by
/// `<< use_highbd` (restoration.c:249-400, :1492-1597); in PIXEL units the
/// two depths are the same walk, which is what the type parameter expresses.
/// `stride` is in pixels.
#[derive(Clone, Debug, Default)]
pub struct StripeBoundariesT<T> {
    pub above: alloc::vec::Vec<T>,
    pub below: alloc::vec::Vec<T>,
    pub stride: usize,
}

/// The 8-bit boundaries (unchanged name for every existing caller).
pub type StripeBoundaries = StripeBoundariesT<u8>;

/// C `get_stripe_boundary_info` (restoration.c:216).
pub(super) fn get_stripe_boundary_info(
    limits: &TileLimits,
    tile_rect: &PixelRect,
    ss_y: i32,
) -> (bool, bool) {
    let mut copy_above = true;
    let mut copy_below = true;

    let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;

    let first_stripe_in_tile = limits.v_start == tile_rect.top;
    let this_stripe_height = full_stripe_height
        - if first_stripe_in_tile {
            runit_offset
        } else {
            0
        };
    let last_stripe_in_tile = limits.v_start + this_stripe_height >= tile_rect.bottom;

    if first_stripe_in_tile {
        copy_above = false;
    }
    if last_stripe_in_tile {
        copy_below = false;
    }
    (copy_above, copy_below)
}

/// Line save/restore scratch — C `RestorationLineBuffers` (restoration.h:206),
/// boundary rows only (the cdef/lr column buffers are unused in the
/// single-tile path). C sizes the rows in BYTES (`tmp_save_above[..][RESTORATION_LINEBUFFER_WIDTH]`,
/// 400 = `2 * RESTORATION_PROC_UNIT_SIZE * 2 + ...` at highbd), so 400 pixels
/// per row covers both depths.
pub(super) struct LineBuffers<T> {
    pub(super) above: [[T; 400]; RESTORATION_BORDER as usize],
    pub(super) below: [[T; 400]; RESTORATION_BORDER as usize],
}

impl<T: Copy + Default> LineBuffers<T> {
    pub(super) fn new() -> Self {
        LineBuffers {
            above: [[T::default(); 400]; RESTORATION_BORDER as usize],
            below: [[T::default(); 400]; RESTORATION_BORDER as usize],
        }
    }
}

/// C `setup_processing_stripe_boundary` (restoration.c:249), opt=0, at either
/// bit depth (C's `use_highbd` only rescales the byte counts; every offset here
/// is in pixels, so one body serves both).
#[allow(clippy::too_many_arguments)]
pub(super) fn setup_processing_stripe_boundary<T: Copy>(
    limits: &TileLimits,
    rsb: &StripeBoundariesT<T>,
    rsb_row: i32,
    h: i32,
    data: &mut [T],
    data_origin: usize,
    data_stride: usize,
    rlbs: &mut LineBuffers<T>,
    copy_above: bool,
    copy_below: bool,
) {
    let buf_stride = rsb.stride as i32;
    let buf_x0_off = limits.h_start;
    let line_width = (limits.h_end - limits.h_start) + 2 * RESTORATION_EXTRA_HORZ;
    let line_size = line_width as usize;

    let data_x0 = limits.h_start - RESTORATION_EXTRA_HORZ;

    if copy_above {
        let data_tl = data_origin as isize
            + data_x0 as isize
            + limits.v_start as isize * data_stride as isize;
        for i in -RESTORATION_BORDER..0 {
            let buf_row = rsb_row + (i + RESTORATION_CTX_VERT).max(0);
            let buf_off = (buf_x0_off + buf_row * buf_stride) as usize;
            let dst = (data_tl + i as isize * data_stride as isize) as usize;
            rlbs.above[(i + RESTORATION_BORDER) as usize][..line_size]
                .copy_from_slice(&data[dst..dst + line_size]);
            data[dst..dst + line_size].copy_from_slice(&rsb.above[buf_off..buf_off + line_size]);
        }
    }
    if copy_below {
        let stripe_end = limits.v_start + h;
        let data_bl =
            data_origin as isize + data_x0 as isize + stripe_end as isize * data_stride as isize;
        for i in 0..RESTORATION_BORDER {
            let buf_row = rsb_row + i.min(RESTORATION_CTX_VERT - 1);
            let buf_off = (buf_x0_off + buf_row * buf_stride) as usize;
            let dst = (data_bl + i as isize * data_stride as isize) as usize;
            rlbs.below[i as usize][..line_size].copy_from_slice(&data[dst..dst + line_size]);
            data[dst..dst + line_size].copy_from_slice(&rsb.below[buf_off..buf_off + line_size]);
        }
    }
}

/// C `restore_processing_stripe_boundary` (restoration.c:347), opt=0, at
/// either bit depth (same pixel-unit walk as the setup above).
#[allow(clippy::too_many_arguments)]
pub(super) fn restore_processing_stripe_boundary<T: Copy>(
    limits: &TileLimits,
    rlbs: &LineBuffers<T>,
    h: i32,
    data: &mut [T],
    data_origin: usize,
    data_stride: usize,
    copy_above: bool,
    copy_below: bool,
) {
    let line_width = (limits.h_end - limits.h_start) + 2 * RESTORATION_EXTRA_HORZ;
    let line_size = line_width as usize;
    let data_x0 = limits.h_start - RESTORATION_EXTRA_HORZ;

    if copy_above {
        let data_tl = data_origin as isize
            + data_x0 as isize
            + limits.v_start as isize * data_stride as isize;
        for i in -RESTORATION_BORDER..0 {
            let dst = (data_tl + i as isize * data_stride as isize) as usize;
            data[dst..dst + line_size]
                .copy_from_slice(&rlbs.above[(i + RESTORATION_BORDER) as usize][..line_size]);
        }
    }
    if copy_below {
        let stripe_bottom = limits.v_start + h;
        let data_bl =
            data_origin as isize + data_x0 as isize + stripe_bottom as isize * data_stride as isize;
        for i in 0..RESTORATION_BORDER {
            if stripe_bottom + i >= limits.v_end + RESTORATION_BORDER {
                break;
            }
            let dst = (data_bl + i as isize * data_stride as isize) as usize;
            data[dst..dst + line_size].copy_from_slice(&rlbs.below[i as usize][..line_size]);
        }
    }
}

/// C `wiener_filter_stripe` (restoration.c:399): proc-unit column loop with
/// the 16-px width round-up.
#[allow(clippy::too_many_arguments)]
pub(super) fn wiener_filter_stripe(
    wiener: &WienerInfo,
    stripe_width: i32,
    stripe_height: i32,
    procunit_width: i32,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
) {
    let mut j = 0i32;
    while j < stripe_width {
        let w = procunit_width.min((stripe_width - j + 15) & !15);
        wiener_convolve_add_src(
            src,
            src_origin + j as usize,
            src_stride,
            dst,
            dst_origin + j as usize,
            dst_stride,
            &wiener.hfilter,
            &wiener.vfilter,
            w as usize,
            stripe_height as usize,
        );
        j += procunit_width;
    }
}

/// The one per-depth kernel the unit filter needs: C's
/// `wiener_filter_stripe` / `wiener_filter_stripe_highbd` split
/// (restoration.c:399 / :987). Private — the public surface is the two typed
/// entry points below.
pub(super) trait WienerStripePixel: Copy + Default {
    #[allow(clippy::too_many_arguments)]
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    );

    /// `sgrproj_filter_stripe` / `sgrproj_filter_stripe_highbd`
    /// (restoration.c:964 / :1010) — the second entry of C's
    /// `stripe_filters[]` table, selected by
    /// `2 * highbd + (unit_rtype == RESTORE_SGRPROJ)`.
    #[allow(clippy::too_many_arguments)]
    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[Self],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [Self],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    );
}

impl WienerStripePixel for u8 {
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bd: i32,
    ) {
        wiener_filter_stripe(
            wiener,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }

    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u8],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u8],
        dst_origin: usize,
        dst_stride: usize,
        _bd: i32,
    ) {
        crate::port_sgr::sgrproj_filter_stripe(
            ep,
            xqd,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
        );
    }
}

impl WienerStripePixel for u16 {
    fn filter_stripe(
        wiener: &WienerInfo,
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    ) {
        wiener_filter_stripe_hbd(
            wiener,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
            bd,
        );
    }

    fn filter_stripe_sgr(
        ep: usize,
        xqd: &[i32; 2],
        stripe_width: i32,
        stripe_height: i32,
        procunit_width: i32,
        src: &[u16],
        src_origin: usize,
        src_stride: usize,
        dst: &mut [u16],
        dst_origin: usize,
        dst_stride: usize,
        bd: i32,
    ) {
        crate::port_sgr::sgrproj_filter_stripe_highbd(
            ep,
            xqd,
            stripe_width,
            stripe_height,
            procunit_width,
            src,
            src_origin,
            src_stride,
            dst,
            dst_origin,
            dst_stride,
            bd,
        );
    }
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040), 8-bit.
/// Dispatches C's `filter_idx = 2 * highbd + (unit_rtype == RESTORE_SGRPROJ)`;
/// the SGR arm is live on the VIDEO path at presets 0..3 (it is `sg_filter_lvl
/// = 0`, hence unreachable, on the all-intra one).
///
/// `data`/`dst` are padded planes; `*_origin` indexes plane (0,0). `data` is
/// temporarily modified around stripe boundaries when `need_boundaries` is
/// set (decoder-exact application); the search path passes false
/// (`use_boundaries_in_rest_search = 0`, enc_handle.c:4483).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundaries,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [u8],
    data_origin: usize,
    stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
) {
    filter_unit_impl(
        need_boundaries,
        limits,
        rui,
        rsb,
        tile_rect,
        tile_stripe0,
        ss_x,
        ss_y,
        data,
        data_origin,
        stride,
        dst,
        dst_origin,
        dst_stride,
        8,
    );
}

/// C `svt_av1_loop_restoration_filter_unit` (restoration.c:1040) at
/// `highbd = 1` with BOTH `need_boundaries` arms — the decoder-exact APPLY
/// twin of [`loop_restoration_filter_unit`] for the 10-bit recon (issue #13).
///
/// The stripe split, the boundary save/substitute/restore and the RESTORE_NONE
/// copy are the same pixel-unit walk as at 8 bits (C only rescales byte
/// counts by `use_highbd`); the convolve is `wiener_filter_stripe_highbd`.
/// [`loop_restoration_filter_unit_search_hbd`] remains the boundary-less
/// search arm and is untouched.
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_hbd(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundariesT<u16>,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    filter_unit_impl(
        need_boundaries,
        limits,
        rui,
        rsb,
        tile_rect,
        tile_stripe0,
        ss_x,
        ss_y,
        data,
        data_origin,
        stride,
        dst,
        dst_origin,
        dst_stride,
        bd,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn filter_unit_impl<T: WienerStripePixel>(
    need_boundaries: bool,
    limits: &TileLimits,
    rui: &RestUnitParams,
    rsb: &StripeBoundariesT<T>,
    tile_rect: &PixelRect,
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    data: &mut [T],
    data_origin: usize,
    stride: usize,
    dst: &mut [T],
    dst_origin: usize,
    dst_stride: usize,
    bd: i32,
) {
    let unit_h = limits.v_end - limits.v_start;
    let unit_w = limits.h_end - limits.h_start;
    let data_tl = data_origin + limits.v_start as usize * stride + limits.h_start as usize;
    let dst_tl = dst_origin + limits.v_start as usize * dst_stride + limits.h_start as usize;

    if rui.rtype == RESTORE_NONE {
        for i in 0..unit_h as usize {
            let s = data_tl + i * stride;
            let d = dst_tl + i * dst_stride;
            let (a, b) = (s..s + unit_w as usize, d..d + unit_w as usize);
            dst[b].copy_from_slice(&data[a]);
        }
        return;
    }
    // C `filter_idx = 2 * highbd + (unit_rtype == RESTORE_SGRPROJ)` — the
    // pixel type carries the `highbd` half, this flag the other.
    debug_assert!(rui.rtype == RESTORE_WIENER || rui.rtype == RESTORE_SGRPROJ);
    let is_sgr = rui.rtype == RESTORE_SGRPROJ;

    let procunit_width = RESTORATION_PROC_UNIT_SIZE >> ss_x;
    let mut rlbs = LineBuffers::<T>::new();

    let mut remaining = *limits;
    let mut i = 0i32;
    while i < unit_h {
        remaining.v_start = limits.v_start + i;
        let (copy_above, copy_below) = get_stripe_boundary_info(&remaining, tile_rect, ss_y);

        let full_stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
        let runit_offset = RESTORATION_UNIT_OFFSET >> ss_y;

        let tile_stripe = (remaining.v_start - tile_rect.top + runit_offset) / full_stripe_height;
        let frame_stripe = tile_stripe0 + tile_stripe;
        let rsb_row = RESTORATION_CTX_VERT * frame_stripe;

        let nominal_stripe_height =
            full_stripe_height - if tile_stripe == 0 { runit_offset } else { 0 };
        let h = nominal_stripe_height.min(remaining.v_end - remaining.v_start);

        if need_boundaries {
            setup_processing_stripe_boundary(
                &remaining,
                rsb,
                rsb_row,
                h,
                data,
                data_origin,
                stride,
                &mut rlbs,
                copy_above,
                copy_below,
            );
        }
        if is_sgr {
            T::filter_stripe_sgr(
                rui.sgr_ep,
                &rui.sgr_xqd,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        } else {
            T::filter_stripe(
                &rui.wiener,
                unit_w,
                h,
                procunit_width,
                data,
                data_tl + i as usize * stride,
                stride,
                dst,
                dst_tl + i as usize * dst_stride,
                dst_stride,
                bd,
            );
        }
        if need_boundaries {
            restore_processing_stripe_boundary(
                &remaining,
                &rlbs,
                h,
                data,
                data_origin,
                stride,
                copy_above,
                copy_below,
            );
        }

        i += h;
    }
}

/// C `count_units_in_tile` (restoration.c:71).
pub fn count_units_in_tile(unit_size: i32, tile_size: i32) -> i32 {
    ((tile_size + (unit_size >> 1)) / unit_size).max(1)
}

/// Iterate restoration units exactly like C `foreach_rest_unit_in_tile`
/// (restoration.c:1227): unit extents with the 150% edge extension and the
/// RESTORATION_UNIT_OFFSET upward shift. Calls `f(limits, unit_idx)`.
pub fn foreach_rest_unit_in_tile(
    tile_rect: &PixelRect,
    hunits_per_tile: i32,
    unit_size: i32,
    ss_y: i32,
    mut f: impl FnMut(&TileLimits, i32),
) {
    let tile_w = tile_rect.right - tile_rect.left;
    let tile_h = tile_rect.bottom - tile_rect.top;
    let ext_size = unit_size * 3 / 2;

    let mut y0 = 0i32;
    let mut i = 0i32;
    while y0 < tile_h {
        let remaining_h = tile_h - y0;
        let h = if remaining_h < ext_size {
            remaining_h
        } else {
            unit_size
        };

        let mut limits = TileLimits {
            h_start: 0,
            h_end: 0,
            v_start: tile_rect.top + y0,
            v_end: tile_rect.top + y0 + h,
        };
        let voffset = RESTORATION_UNIT_OFFSET >> ss_y;
        limits.v_start = tile_rect.top.max(limits.v_start - voffset);
        if limits.v_end < tile_rect.bottom {
            limits.v_end -= voffset;
        }

        let mut x0 = 0i32;
        let mut j = 0i32;
        while x0 < tile_w {
            let remaining_w = tile_w - x0;
            let w = if remaining_w < ext_size {
                remaining_w
            } else {
                unit_size
            };
            limits.h_start = tile_rect.left + x0;
            limits.h_end = tile_rect.left + x0 + w;

            f(&limits, i * hunits_per_tile + j);

            x0 += w;
            j += 1;
        }
        y0 += h;
        i += 1;
    }
}

/// C `extend_lines` (restoration.c:1492); the `use_highbitdepth` arm is the
/// same fill in `uint16_t` units, so one generic body.
pub(super) fn extend_lines<T: Copy>(
    buf: &mut [T],
    start: usize,
    width: usize,
    height: usize,
    stride: usize,
    extend: usize,
) {
    for i in 0..height {
        let row = start + i * stride;
        let left = buf[row];
        let right = buf[row + width - 1];
        buf[row - extend..row].fill(left);
        buf[row + width..row + width + extend].fill(right);
    }
}

/// C `svt_aom_save_deblock_boundary_lines` (restoration.c:1507), no superres,
/// either bit depth (`use_highbd` there only rescales byte counts).
#[allow(clippy::too_many_arguments)]
pub(super) fn save_deblock_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let bdry_buf = if is_above {
        &mut boundaries.above
    } else {
        &mut boundaries.below
    };
    let bdry_stride = boundaries.stride;
    // bdry_start = buf + RESTORATION_EXTRA_HORZ
    let bdry_rows = RESTORATION_EXTRA_HORZ as usize
        + RESTORATION_CTX_VERT as usize * stripe as usize * bdry_stride;

    let lines_to_save = RESTORATION_CTX_VERT.min(src_height - row);
    debug_assert!(lines_to_save == 1 || lines_to_save == 2);

    let upscaled_width = src_width as usize;
    for i in 0..lines_to_save as usize {
        let s = src_origin + (row as usize + i) * src_stride;
        let d = bdry_rows + i * bdry_stride;
        bdry_buf[d..d + upscaled_width].copy_from_slice(&src[s..s + upscaled_width]);
    }
    if lines_to_save == 1 {
        let (a, b) = (bdry_rows, bdry_rows + bdry_stride);
        bdry_buf.copy_within(a..a + upscaled_width, b);
    }
    extend_lines(
        bdry_buf,
        bdry_rows,
        upscaled_width,
        RESTORATION_CTX_VERT as usize,
        bdry_stride,
        RESTORATION_EXTRA_HORZ as usize,
    );
}

/// C `svt_aom_save_cdef_boundary_lines` (restoration.c:1561), no superres,
/// either bit depth.
#[allow(clippy::too_many_arguments)]
pub(super) fn save_cdef_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    row: i32,
    stripe: i32,
    is_above: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let bdry_buf = if is_above {
        &mut boundaries.above
    } else {
        &mut boundaries.below
    };
    let bdry_stride = boundaries.stride;
    let bdry_rows = RESTORATION_EXTRA_HORZ as usize
        + RESTORATION_CTX_VERT as usize * stripe as usize * bdry_stride;
    let upscaled_width = src_width as usize;
    let s = src_origin + row as usize * src_stride;
    for i in 0..RESTORATION_CTX_VERT as usize {
        let d = bdry_rows + i * bdry_stride;
        bdry_buf[d..d + upscaled_width].copy_from_slice(&src[s..s + upscaled_width]);
    }
    extend_lines(
        bdry_buf,
        bdry_rows,
        upscaled_width,
        RESTORATION_CTX_VERT as usize,
        bdry_stride,
        RESTORATION_EXTRA_HORZ as usize,
    );
}

/// C `svt_aom_save_tile_row_boundary_lines` (restoration.c:1591): one tile
/// row spanning the whole frame. `after_cdef=false` saves deblocked context,
/// `true` saves CDEF context where deblocked context was NOT saved.
///
/// Generic over the pixel type: `u8` is the existing 8-bit path (every caller
/// infers it), `u16` is the highbd twin the 10-bit apply needs (issue #13) —
/// C's `use_highbd` flag only rescales byte counts inside the helpers.
#[allow(clippy::too_many_arguments)]
pub fn save_tile_row_boundary_lines<T: Copy>(
    src: &[T],
    src_origin: usize,
    src_stride: usize,
    src_width: i32,
    src_height: i32,
    ss_y: i32,
    after_cdef: bool,
    boundaries: &mut StripeBoundariesT<T>,
) {
    let stripe_height = RESTORATION_PROC_UNIT_SIZE >> ss_y;
    let stripe_off = RESTORATION_UNIT_OFFSET >> ss_y;
    // whole_frame_rect on this plane
    let tile_rect = PixelRect {
        left: 0,
        top: 0,
        right: src_width,
        bottom: src_height,
    };
    let plane_height = src_height;

    let mut tile_stripe = 0i32;
    loop {
        let rel_y0 = (tile_stripe * stripe_height - stripe_off).max(0);
        let y0 = tile_rect.top + rel_y0;
        if y0 >= tile_rect.bottom {
            break;
        }
        let rel_y1 = (tile_stripe + 1) * stripe_height - stripe_off;
        let y1 = (tile_rect.top + rel_y1).min(tile_rect.bottom);

        let frame_stripe = tile_stripe;
        let use_deblock_above = frame_stripe > 0;
        let use_deblock_below = y1 < plane_height;

        if !after_cdef {
            if use_deblock_above {
                save_deblock_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    src_height,
                    y0 - RESTORATION_CTX_VERT,
                    frame_stripe,
                    true,
                    boundaries,
                );
            }
            if use_deblock_below {
                save_deblock_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    src_height,
                    y1,
                    frame_stripe,
                    false,
                    boundaries,
                );
            }
        } else {
            if !use_deblock_above {
                save_cdef_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    y0,
                    frame_stripe,
                    true,
                    boundaries,
                );
            }
            if !use_deblock_below {
                save_cdef_boundary_lines(
                    src,
                    src_origin,
                    src_stride,
                    src_width,
                    y1 - 1,
                    frame_stripe,
                    false,
                    boundaries,
                );
            }
        }
        tile_stripe += 1;
    }
}

/// Stripe-boundary buffer allocation, C `svt_av1_alloc_restoration_buffers`
/// (restoration.c:1685): rows for `ceil((8 + mi_rows*4) / 64)` stripes at a
/// 32-aligned `plane_w + 8` stride.
pub fn alloc_stripe_boundaries(frame_width: i32, frame_height: i32, ss_x: i32) -> StripeBoundaries {
    alloc_stripe_boundaries_t::<u8>(frame_width, frame_height, ss_x)
}

/// [`alloc_stripe_boundaries`] at any pixel type (`u16` for the 10-bit apply).
/// C allocates `stripe_boundary_size << use_highbd` BYTES (restoration.c:1685-
/// 1700) — the same number of pixels at either depth.
pub fn alloc_stripe_boundaries_t<T: Copy + Default>(
    frame_width: i32,
    frame_height: i32,
    ss_x: i32,
) -> StripeBoundariesT<T> {
    let ext_h = RESTORATION_UNIT_OFFSET + frame_height;
    let num_stripes = (ext_h + 63) / 64;
    let plane_w = ((frame_width + ss_x) >> ss_x) + 2 * RESTORATION_EXTRA_HORZ;
    // ALIGN_POWER_OF_TWO(plane_w, 5)
    let stride = ((plane_w + 31) & !31) as usize;
    let size = num_stripes as usize * stride * RESTORATION_CTX_VERT as usize;
    StripeBoundariesT {
        above: alloc::vec![T::default(); size],
        below: alloc::vec![T::default(); size],
        stride,
    }
}

/// Region SSE (C `svt_aom_get_sse` semantics as used by
/// `sse_restoration_unit`, svt_psnr.c:189).
#[allow(clippy::too_many_arguments)]
pub fn sse_region(
    a: &[u8],
    a_origin: usize,
    a_stride: usize,
    b: &[u8],
    b_origin: usize,
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    // Same sum of squared diffs as the scalar nest — `variance::sse` reads
    // `row * stride + col` over `height` rows, identical reach.
    crate::variance::sse(
        &a[a_origin..],
        a_stride,
        &b[b_origin..],
        b_stride,
        width,
        height,
    ) as i64
}

// ===========================================================================
// HIGHBD arm — the `is_16bit` (10-bit) loop-restoration SEARCH.
//
// C keeps a parallel highbd implementation of every kernel the Wiener search
// touches, selected by `cm->use_highbitdepth`:
//   sse_restoration_unit -> svt_aom_highbd_get_{y,u,v}_sse_part
//                           (restoration_pick.c:43-51, svt_psnr.c:93)
//   search_wiener_seg    -> svt_av1_compute_stats_highbd
//                           (restoration_pick.c:1332, :692)
//   try_restoration_unit -> svt_av1_loop_restoration_filter_unit(.., highbd=1,
//                           bit_depth) -> wiener_filter_stripe_highbd
//                           -> svt_av1_highbd_wiener_convolve_add_src
//                           (restoration.c, convolve.c:200)
//   svt_extend_frame     -> extend_frame_highbd (restoration.c:152)
// Every one of them is ported below and FFI-pinned in
// tests/c_parity_wiener_hbd.rs. The bd8 kernels above are untouched.
// ===========================================================================

/// C `find_average_highbd` (restoration_pick.h:33) — u16 twin of
/// [`find_average`], returning the u16 mean (C truncates the u64 quotient).
#[allow(clippy::too_many_arguments)]
pub fn find_average_hbd(
    src: &[u16],
    origin: usize,
    stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
) -> u16 {
    let mut sum: u64 = 0;
    for i in v_start..v_end {
        for j in h_start..h_end {
            let idx = origin as isize + i as isize * stride as isize + j as isize;
            sum += src[idx as usize] as u64;
        }
    }
    (sum / ((v_end - v_start) as u64 * (h_end - h_start) as u64)) as u16
}

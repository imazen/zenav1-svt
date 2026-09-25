// ---- Loop restoration (Wiener) ----

unsafe extern "C" {
    pub(super) fn ref_wiener_convolve_add_src(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        filter_x: *const i16,
        filter_y: *const i16,
        w: i32,
        h: i32,
    );
    pub(super) fn ref_compute_stats(
        wiener_win: i32,
        dgd: *const u8,
        src: *const u8,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        dgd_stride: i32,
        src_stride: i32,
        m: *mut i64,
        h: *mut i64,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_loop_restoration_filter_unit(
        need_boundaries: u8,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        rtype: i32,
        vfilter: *const i16,
        hfilter: *const i16,
        bdry_above: *const u8,
        bdry_below: *const u8,
        bdry_stride: i32,
        tile_left: i32,
        tile_top: i32,
        tile_right: i32,
        tile_bottom: i32,
        tile_stripe0: i32,
        ss_x: i32,
        ss_y: i32,
        data: *mut u8,
        stride: i32,
        dst: *mut u8,
        dst_stride: i32,
    );
    pub(super) fn ref_extend_frame(
        data: *mut u8,
        width: i32,
        height: i32,
        stride: i32,
        border_horz: i32,
        border_vert: i32,
    );
    pub(super) fn ref_write_refsubexpfin_bytes(
        n: u16,
        k: u16,
        r: u16,
        v: u16,
        out: *mut u8,
        cap: u32,
    ) -> u32;
    pub(super) fn ref_count_refsubexpfin(n: u16, k: u16, r: u16, v: u16) -> i32;
    pub(super) fn ref_write_sgrproj_filter_bytes(
        ep: i32,
        xqd: *const i32,
        ref_xqd: *const i32,
        out: *mut u8,
        cap: u32,
    ) -> u32;
    pub(super) fn ref_default_sgrproj_xqd(out2: *mut i32);

    // ---- highbd arm (the is_16bit / 10-bit pipeline) ----
    pub(super) fn ref_highbd_wiener_convolve_add_src(
        src: *const u16,
        src_stride: i32,
        dst: *mut u16,
        dst_stride: i32,
        filter_x: *const i16,
        filter_y: *const i16,
        w: i32,
        h: i32,
        bd: i32,
    );
    pub(super) fn ref_compute_stats_highbd(
        wiener_win: i32,
        dgd: *const u16,
        src: *const u16,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        dgd_stride: i32,
        src_stride: i32,
        m: *mut i64,
        h: *mut i64,
        bit_depth: i32,
    );
    pub(super) fn ref_highbd_get_sse(
        a: *const u16,
        a_stride: i32,
        b: *const u16,
        b_stride: i32,
        width: i32,
        height: i32,
    ) -> i64;
    pub(super) fn ref_extend_frame_highbd(
        data: *mut u16,
        width: i32,
        height: i32,
        stride: i32,
        border_horz: i32,
        border_vert: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_loop_restoration_filter_unit_highbd(
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        rtype: i32,
        vfilter: *const i16,
        hfilter: *const i16,
        tile_left: i32,
        tile_top: i32,
        tile_right: i32,
        tile_bottom: i32,
        tile_stripe0: i32,
        ss_x: i32,
        ss_y: i32,
        bit_depth: i32,
        data: *mut u16,
        stride: i32,
        dst: *mut u16,
        dst_stride: i32,
    );
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ref_loop_restoration_filter_unit_highbd_bnd(
        need_boundaries: u8,
        h_start: i32,
        h_end: i32,
        v_start: i32,
        v_end: i32,
        rtype: i32,
        vfilter: *const i16,
        hfilter: *const i16,
        bdry_above: *const u16,
        bdry_below: *const u16,
        bdry_stride: i32,
        tile_left: i32,
        tile_top: i32,
        tile_right: i32,
        tile_bottom: i32,
        tile_stripe0: i32,
        ss_x: i32,
        ss_y: i32,
        bit_depth: i32,
        data: *mut u16,
        stride: i32,
        dst: *mut u16,
        dst_stride: i32,
    );
}

/// Reference `svt_av1_highbd_wiener_convolve_add_src_c` (convolve.c:200).
/// Same margin contract as the 8-bit [`wiener_convolve_add_src`].
#[allow(clippy::too_many_arguments)]
pub fn highbd_wiener_convolve_add_src(
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
    hfilter: &[i16; 8],
    vfilter: &[i16; 8],
    w: usize,
    h: usize,
    bd: i32,
) {
    assert!(src_origin >= 3 * src_stride + 3);
    assert!(src_origin + (h + 2) * src_stride + w + 4 <= src.len());
    assert!(dst_origin + (h - 1) * dst_stride + w <= dst.len());
    unsafe {
        ref_highbd_wiener_convolve_add_src(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr().add(dst_origin),
            dst_stride as i32,
            hfilter.as_ptr(),
            vfilter.as_ptr(),
            w as i32,
            h as i32,
            bd,
        );
    }
}

/// Reference `svt_av1_compute_stats_highbd_c` (restoration_pick.c:692).
#[allow(clippy::too_many_arguments)]
pub fn compute_stats_highbd(
    wiener_win: usize,
    dgd: &[u16],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u16],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
    bit_depth: i32,
) {
    unsafe {
        ref_compute_stats_highbd(
            wiener_win as i32,
            dgd.as_ptr().add(dgd_origin),
            src.as_ptr().add(src_origin),
            h_start,
            h_end,
            v_start,
            v_end,
            dgd_stride as i32,
            src_stride as i32,
            m.as_mut_ptr(),
            h.as_mut_ptr(),
            bit_depth,
        );
    }
}

/// Reference `svt_aom_highbd_get_sse` (svt_psnr.c) — the kernel behind
/// `svt_aom_highbd_get_{y,u,v}_sse_part`, i.e. the LR search's
/// `sse_restoration_unit` at `highbd = 1`.
#[allow(clippy::too_many_arguments)]
pub fn highbd_get_sse(
    a: &[u16],
    a_origin: usize,
    a_stride: usize,
    b: &[u16],
    b_origin: usize,
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    unsafe {
        ref_highbd_get_sse(
            a.as_ptr().add(a_origin),
            a_stride as i32,
            b.as_ptr().add(b_origin),
            b_stride as i32,
            width as i32,
            height as i32,
        )
    }
}

/// Reference `svt_extend_frame` at `highbd = 1` (restoration.c).
pub fn extend_frame_highbd(
    data: &mut [u16],
    origin: usize,
    width: usize,
    height: usize,
    stride: usize,
    border_horz: usize,
    border_vert: usize,
) {
    unsafe {
        ref_extend_frame_highbd(
            data.as_mut_ptr().add(origin),
            width as i32,
            height as i32,
            stride as i32,
            border_horz as i32,
            border_vert as i32,
        );
    }
}

/// Reference `svt_av1_loop_restoration_filter_unit` at `highbd = 1`,
/// `need_boundaries = 0` — the SEARCH path
/// (`use_boundaries_in_rest_search = 0`).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_highbd(
    limits: (i32, i32, i32, i32),
    rtype: i32,
    vfilter: &[i16; 8],
    hfilter: &[i16; 8],
    tile_rect: (i32, i32, i32, i32),
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    bit_depth: i32,
    data: &mut [u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
) {
    unsafe {
        ref_loop_restoration_filter_unit_highbd(
            limits.0,
            limits.1,
            limits.2,
            limits.3,
            rtype,
            vfilter.as_ptr(),
            hfilter.as_ptr(),
            tile_rect.0,
            tile_rect.1,
            tile_rect.2,
            tile_rect.3,
            tile_stripe0,
            ss_x,
            ss_y,
            bit_depth,
            data.as_mut_ptr().add(data_origin),
            stride as i32,
            dst.as_mut_ptr().add(dst_origin),
            dst_stride as i32,
        );
    }
}

/// Reference `svt_av1_loop_restoration_filter_unit` at `highbd = 1` with the
/// stripe-boundary machinery live — the decoder-exact APPLY arm (issue #13).
/// `bdry_above`/`bdry_below` are the `u16` boundary buffers at `bdry_stride`
/// pixels (C's `RestorationStripeBoundaries` at `use_highbd`).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit_highbd_bnd(
    need_boundaries: bool,
    limits: (i32, i32, i32, i32),
    rtype: u8,
    vfilter: &[i16; 8],
    hfilter: &[i16; 8],
    bdry_above: &[u16],
    bdry_below: &[u16],
    bdry_stride: usize,
    tile_rect: (i32, i32, i32, i32),
    tile_stripe0: i32,
    ss_x: i32,
    ss_y: i32,
    bit_depth: i32,
    data: &mut [u16],
    data_origin: usize,
    stride: usize,
    dst: &mut [u16],
    dst_origin: usize,
    dst_stride: usize,
) {
    unsafe {
        ref_loop_restoration_filter_unit_highbd_bnd(
            need_boundaries as u8,
            limits.0,
            limits.1,
            limits.2,
            limits.3,
            rtype as i32,
            vfilter.as_ptr(),
            hfilter.as_ptr(),
            bdry_above.as_ptr(),
            bdry_below.as_ptr(),
            bdry_stride as i32,
            tile_rect.0,
            tile_rect.1,
            tile_rect.2,
            tile_rect.3,
            tile_stripe0,
            ss_x,
            ss_y,
            bit_depth,
            data.as_mut_ptr().add(data_origin),
            stride as i32,
            dst.as_mut_ptr().add(dst_origin),
            dst_stride as i32,
        );
    }
}

/// Reference `svt_av1_wiener_convolve_add_src_c`. `src_origin`/`dst_origin`
/// index the block's top-left inside padded planes; the caller guarantees
/// 3/3/3/4 (top/left/bottom/right) in-bounds margins around the block.
#[allow(clippy::too_many_arguments)]
pub fn wiener_convolve_add_src(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_origin: usize,
    dst_stride: usize,
    filter_x: &[i16; 8],
    filter_y: &[i16; 8],
    w: usize,
    h: usize,
) {
    assert!(src_origin >= 3 * src_stride + 3);
    assert!(src.len() >= src_origin + (h + 2) * src_stride + w + 4);
    assert!(dst.len() >= dst_origin + (h - 1) * dst_stride + w);
    unsafe {
        ref_wiener_convolve_add_src(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr().add(dst_origin),
            dst_stride as i32,
            filter_x.as_ptr(),
            filter_y.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

/// Reference `svt_av1_compute_stats_c`. Origins index plane (0,0).
#[allow(clippy::too_many_arguments)]
pub fn compute_stats(
    wiener_win: usize,
    dgd: &[u8],
    dgd_origin: usize,
    dgd_stride: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    h_start: i32,
    h_end: i32,
    v_start: i32,
    v_end: i32,
    m: &mut [i64],
    h: &mut [i64],
) {
    let win2 = wiener_win * wiener_win;
    assert!(m.len() >= win2 && h.len() >= win2 * win2);
    unsafe {
        ref_compute_stats(
            wiener_win as i32,
            dgd.as_ptr().add(dgd_origin),
            src.as_ptr().add(src_origin),
            h_start,
            h_end,
            v_start,
            v_end,
            dgd_stride as i32,
            src_stride as i32,
            m.as_mut_ptr(),
            h.as_mut_ptr(),
        );
    }
}

/// Reference `svt_av1_loop_restoration_filter_unit` (8-bit, wiener/none).
/// `data`/`dst` are padded planes with `origin` indexing plane (0,0); the
/// boundary buffers use the C layout (column i == plane column i-4).
#[allow(clippy::too_many_arguments)]
pub fn loop_restoration_filter_unit(
    need_boundaries: bool,
    limits: (i32, i32, i32, i32),
    rtype: u8,
    vfilter: &[i16; 8],
    hfilter: &[i16; 8],
    bdry_above: &[u8],
    bdry_below: &[u8],
    bdry_stride: usize,
    tile_rect: (i32, i32, i32, i32),
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
    let (h_start, h_end, v_start, v_end) = limits;
    let (left, top, right, bottom) = tile_rect;
    unsafe {
        ref_loop_restoration_filter_unit(
            need_boundaries as u8,
            h_start,
            h_end,
            v_start,
            v_end,
            rtype as i32,
            vfilter.as_ptr(),
            hfilter.as_ptr(),
            bdry_above.as_ptr(),
            bdry_below.as_ptr(),
            bdry_stride as i32,
            left,
            top,
            right,
            bottom,
            tile_stripe0,
            ss_x,
            ss_y,
            data.as_mut_ptr().add(data_origin),
            stride as i32,
            dst.as_mut_ptr().add(dst_origin),
            dst_stride as i32,
        );
    }
}

/// Reference `svt_extend_frame` (8-bit): `origin` indexes crop (0,0).
pub fn extend_frame(
    data: &mut [u8],
    origin: usize,
    width: usize,
    height: usize,
    stride: usize,
    border_horz: usize,
    border_vert: usize,
) {
    assert!(origin >= border_vert * stride + border_horz);
    unsafe {
        ref_extend_frame(
            data.as_mut_ptr().add(origin),
            width as i32,
            height as i32,
            stride as i32,
            border_horz as i32,
            border_vert as i32,
        );
    }
}

/// Reference `svt_aom_write_primitive_refsubexpfin` through a fresh od_ec
/// coder: returns the finalized byte stream for (n, k, ref, v).
pub fn write_refsubexpfin_bytes(n: u16, k: u16, r: u16, v: u16) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    let n_bytes = unsafe { ref_write_refsubexpfin_bytes(n, k, r, v, out.as_mut_ptr(), 64) };
    out.truncate(n_bytes as usize);
    out
}

/// Reference `svt_aom_count_primitive_refsubexpfin`.
pub fn count_refsubexpfin(n: u16, k: u16, r: u16, v: u16) -> i32 {
    unsafe { ref_count_refsubexpfin(n, k, r, v) }
}

/// Reference bytes for one SGR restoration unit's filter payload — C's
/// `write_sgrproj_filter` body composed from its own exported primitives (see
/// the shim's comment for the evidence tier).
#[must_use]
pub fn write_sgrproj_filter_bytes(ep: i32, xqd: [i32; 2], ref_xqd: [i32; 2]) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    let n = unsafe {
        ref_write_sgrproj_filter_bytes(ep, xqd.as_ptr(), ref_xqd.as_ptr(), out.as_mut_ptr(), 64)
    };
    out.truncate(n as usize);
    out
}

/// Reference `set_default_sgrproj` (restoration.h:243).
#[must_use]
pub fn default_sgrproj_xqd() -> [i32; 2] {
    let mut out = [0i32; 2];
    unsafe { ref_default_sgrproj_xqd(out.as_mut_ptr()) };
    out
}

// ---------------------------------------------------------------------------

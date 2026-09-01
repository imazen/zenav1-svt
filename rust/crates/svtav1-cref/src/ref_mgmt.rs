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

// ---------------------------------------------------------------------------
// resize.c superres decision (`shims/refmgmt_shims.c`)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn superres_frame_update_type(is_key_frame: i32, hier: u8, temporal_layer: u8) -> i32;
    fn superres_denom_idx(scale_denom: u8) -> u8;
}

/// C `svt_aom_get_frame_update_type` (`resize.c:1246`) — the
/// `SvtAv1FrameUpdateType` discriminant.
#[must_use]
pub fn frame_update_type(is_key_frame: bool, hier: u8, temporal_layer: u8) -> i32 {
    unsafe { superres_frame_update_type(i32::from(is_key_frame), hier, temporal_layer) }
}

/// C `svt_aom_get_denom_idx` (`resize.c:1425`).
#[must_use]
pub fn denom_idx(scale_denom: u8) -> u8 {
    unsafe { superres_denom_idx(scale_denom) }
}

// ---------------------------------------------------------------------------
// resize.c two-dimensional plane resize (`shims/refmgmt_shims.c`)
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn resize2d_plane(
        input: *const u8,
        height: i32,
        width: i32,
        in_stride: i32,
        output: *mut u8,
        height2: i32,
        width2: i32,
        out_stride: i32,
    ) -> i32;
    fn resize2d_highbd_plane(
        input: *const u16,
        height: i32,
        width: i32,
        in_stride: i32,
        output: *mut u16,
        height2: i32,
        width2: i32,
        out_stride: i32,
        bd: i32,
    ) -> i32;
}

/// C `svt_av1_resize_plane_c` (`resize.c:422`) — the 8-bit 2-D plane resize.
#[allow(clippy::too_many_arguments)]
pub fn resize_plane(
    input: &[u8],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u8],
    height2: usize,
    width2: usize,
    out_stride: usize,
) {
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height2 - 1) * out_stride + width2);
    let rc = unsafe {
        resize2d_plane(
            input.as_ptr(),
            height as i32,
            width as i32,
            in_stride as i32,
            output.as_mut_ptr(),
            height2 as i32,
            width2 as i32,
            out_stride as i32,
        )
    };
    assert_eq!(rc, 0, "svt_av1_resize_plane_c failed (rc {rc})");
}

/// C `svt_av1_highbd_resize_plane_c` (`resize.c:725`) — the 10-bit variant.
#[allow(clippy::too_many_arguments)]
pub fn highbd_resize_plane(
    input: &[u16],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u16],
    height2: usize,
    width2: usize,
    out_stride: usize,
    bd: i32,
) {
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height2 - 1) * out_stride + width2);
    let rc = unsafe {
        resize2d_highbd_plane(
            input.as_ptr(),
            height as i32,
            width as i32,
            in_stride as i32,
            output.as_mut_ptr(),
            height2 as i32,
            width2 as i32,
            out_stride as i32,
            bd,
        )
    };
    assert_eq!(rc, 0, "svt_av1_highbd_resize_plane_c failed (rc {rc})");
}

/// One plane handed to [`resize_frame`]: `(src, src_stride, dst, dst_stride)`.
pub struct CrefFramePlane<'a> {
    /// The source plane.
    pub src: &'a [u8],
    /// Its stride.
    pub src_stride: usize,
    /// The destination plane.
    pub dst: &'a mut [u8],
    /// Its stride.
    pub dst_stride: usize,
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn resize2d_frame(
        sy: *mut u8,
        su: *mut u8,
        sv: *mut u8,
        sys: u16,
        sus: u16,
        svs: u16,
        src_w: u16,
        src_h: u16,
        dy: *mut u8,
        du: *mut u8,
        dv: *mut u8,
        dys: u16,
        dus: u16,
        dvs: u16,
        dst_w: u16,
        dst_h: u16,
        num_planes: i32,
        ss_x: u32,
        ss_y: u32,
    ) -> i32;
}

/// C `svt_aom_resize_frame` (`resize.c:881`) at `bd = 8`, three planes.
///
/// The shim builds two synthetic `EbPictureBufferDesc`s with `border = 0`, so
/// the buffer pointers are pixel (0, 0) and C's border arithmetic — which only
/// runs on the `bd > 8 && !is_packed` path — never applies.
#[allow(clippy::too_many_arguments)]
pub fn resize_frame(
    planes: &mut [CrefFramePlane<'_>; 3],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    num_planes: i32,
    ss_x: u32,
    ss_y: u32,
) {
    let (s0, s1, s2) = (
        planes[0].src_stride,
        planes[1].src_stride,
        planes[2].src_stride,
    );
    let (d0, d1, d2) = (
        planes[0].dst_stride,
        planes[1].dst_stride,
        planes[2].dst_stride,
    );
    // C takes non-const `uint8_t*`, but never writes through the source.
    let sy = planes[0].src.as_ptr().cast_mut();
    let su = planes[1].src.as_ptr().cast_mut();
    let sv = planes[2].src.as_ptr().cast_mut();
    let [p0, p1, p2] = planes;
    let rc = unsafe {
        resize2d_frame(
            sy,
            su,
            sv,
            s0 as u16,
            s1 as u16,
            s2 as u16,
            src_w as u16,
            src_h as u16,
            p0.dst.as_mut_ptr(),
            p1.dst.as_mut_ptr(),
            p2.dst.as_mut_ptr(),
            d0 as u16,
            d1 as u16,
            d2 as u16,
            dst_w as u16,
            dst_h as u16,
            num_planes,
            ss_x,
            ss_y,
        )
    };
    assert_eq!(rc, 0, "svt_aom_resize_frame failed (rc {rc})");
}

// ---- 2D transform wrappers ----

unsafe extern "C" {
    pub(super) fn ref_fwd_txfm2d(
        n: i32,
        input: *mut i16,
        output: *mut i32,
        stride: u32,
        tx_type: i32,
    );
    pub(super) fn ref_inv_txfm2d_add(
        n: i32,
        input: *const i32,
        output_r: *const u16,
        stride_r: i32,
        output_w: *mut u16,
        stride_w: i32,
        tx_type: i32,
    );
    pub(super) fn ref_fwd_txfm2d_rect(
        w: i32,
        h: i32,
        input: *mut i16,
        output: *mut i32,
        stride: u32,
        tx_type: i32,
    );
    pub(super) fn ref_inv_txfm2d_add_rect(
        w: i32,
        h: i32,
        input: *const i32,
        output_r: *const u16,
        stride_r: i32,
        output_w: *mut u16,
        stride_w: i32,
        tx_type: i32,
    );
}

/// Reference 2D forward transform (square `n`, 8-bit).
pub fn fwd_txfm2d(n: usize, input: &[i16], tx_type: usize) -> Vec<i32> {
    assert!(input.len() >= n * n);
    let mut out = vec![0i32; n * n];
    let mut inp = input.to_vec();
    unsafe {
        ref_fwd_txfm2d(
            n as i32,
            inp.as_mut_ptr(),
            out.as_mut_ptr(),
            n as u32,
            tx_type as i32,
        )
    };
    out
}

/// Reference 2D inverse transform + add onto `base` (square `n`, 8-bit).
/// Returns the reconstructed pixels.
pub fn inv_txfm2d_add(n: usize, coeffs: &[i32], base: &[u16], tx_type: usize) -> Vec<u16> {
    assert!(coeffs.len() >= n * n && base.len() >= n * n);
    let mut out = vec![0u16; n * n];
    unsafe {
        ref_inv_txfm2d_add(
            n as i32,
            coeffs.as_ptr(),
            base.as_ptr(),
            n as i32,
            out.as_mut_ptr(),
            n as i32,
            tx_type as i32,
        )
    };
    out
}

// ---- Walsh-Hadamard transform (AV1 lossless / qindex 0) ----

unsafe extern "C" {
    pub(super) fn ref_fwht4x4(input: *mut i16, output: *mut i32, stride: u32);
    pub(super) fn ref_highbd_iwht4x4_16_add(
        input: *const i32,
        dest_r: *const u16,
        stride_r: i32,
        dest_w: *mut u16,
        stride_w: i32,
        bd: i32,
    );
    pub(super) fn ref_highbd_iwht4x4_1_add(
        input: *const i32,
        dest_r: *const u16,
        stride_r: i32,
        dest_w: *mut u16,
        stride_w: i32,
        bd: i32,
    );
}

/// Reference forward 4x4 Walsh-Hadamard (`svt_av1_fwht4x4_c`,
/// transforms.c:3879). `input` is a 4x4 residual block at row stride `stride`.
/// Returns the 16 coefficients packed at stride 4.
pub fn fwht4x4(input: &[i16], stride: usize) -> Vec<i32> {
    assert!(input.len() >= 3 * stride + 4);
    let mut inp = input.to_vec();
    let mut out = vec![0i32; 16];
    unsafe { ref_fwht4x4(inp.as_mut_ptr(), out.as_mut_ptr(), stride as u32) };
    out
}

pub(super) fn iwht4x4_add_common(
    f: unsafe extern "C" fn(*const i32, *const u16, i32, *mut u16, i32, i32),
    coeffs: &[i32],
    base: &[u16],
    stride_r: usize,
    stride_w: usize,
    bd: u8,
) -> Vec<u16> {
    assert!(base.len() >= 3 * stride_r + 4 && stride_w >= 4);
    // The C kernels write only the 4x4 window; pre-fill so a caller can tell
    // written cells from untouched ones.
    let mut out = vec![0u16; 3 * stride_w + 4];
    unsafe {
        f(
            coeffs.as_ptr(),
            base.as_ptr(),
            stride_r as i32,
            out.as_mut_ptr(),
            stride_w as i32,
            i32::from(bd),
        )
    };
    out
}

/// Reference `svt_av1_highbd_iwht4x4_16_add_c` (inv_transforms.c:2782):
/// inverse WHT of `coeffs` (16 values at stride 4) added onto `base`.
/// Returns the destination buffer (`3 * stride_w + 4` samples).
pub fn highbd_iwht4x4_16_add(
    coeffs: &[i32],
    base: &[u16],
    stride_r: usize,
    stride_w: usize,
    bd: u8,
) -> Vec<u16> {
    assert!(coeffs.len() >= 16);
    iwht4x4_add_common(
        ref_highbd_iwht4x4_16_add,
        coeffs,
        base,
        stride_r,
        stride_w,
        bd,
    )
}

/// Reference `svt_av1_highbd_iwht4x4_1_add_c` (inv_transforms.c:2843), the
/// `eob <= 1` arm. Reads only `coeffs[0]`.
pub fn highbd_iwht4x4_1_add(
    coeffs: &[i32],
    base: &[u16],
    stride_r: usize,
    stride_w: usize,
    bd: u8,
) -> Vec<u16> {
    assert!(!coeffs.is_empty());
    iwht4x4_add_common(
        ref_highbd_iwht4x4_1_add,
        coeffs,
        base,
        stride_r,
        stride_w,
        bd,
    )
}

/// Reference 2D forward transform (rectangular `w` x `h`, 8-bit).
/// `input` is packed row-major with stride `w`.
pub fn fwd_txfm2d_rect(w: usize, h: usize, input: &[i16], tx_type: usize) -> Vec<i32> {
    assert!(input.len() >= w * h);
    let mut out = vec![0i32; w * h];
    let mut inp = input.to_vec();
    unsafe {
        ref_fwd_txfm2d_rect(
            w as i32,
            h as i32,
            inp.as_mut_ptr(),
            out.as_mut_ptr(),
            w as u32,
            tx_type as i32,
        )
    };
    out
}

/// Reference 2D inverse transform + add onto `base` (rectangular `w` x `h`,
/// 8-bit). For 64-dim sizes the C function reads `coeffs` packed at stride
/// min(w, 32) with min(h, 32) rows. Returns the reconstructed pixels.
pub fn inv_txfm2d_add_rect(
    w: usize,
    h: usize,
    coeffs: &[i32],
    base: &[u16],
    tx_type: usize,
) -> Vec<u16> {
    assert!(base.len() >= w * h);
    assert!(coeffs.len() >= w.min(32) * h.min(32));
    let mut out = vec![0u16; w * h];
    unsafe {
        ref_inv_txfm2d_add_rect(
            w as i32,
            h as i32,
            coeffs.as_ptr(),
            base.as_ptr(),
            w as i32,
            out.as_mut_ptr(),
            w as i32,
            tx_type as i32,
        )
    };
    out
}

// ---- Deblocking loop filter kernels + thresholds ----

unsafe extern "C" {
    pub(super) fn ref_lpf(
        kind: i32,
        buf: *mut u8,
        off: i32,
        pitch: i32,
        blimit: u8,
        limit: u8,
        thresh: u8,
    );
    pub(super) fn ref_lf_limits(sharpness: i32, lim_out: *mut u8, mblim_out: *mut u8);
    pub(super) fn ref_lpf_hbd(
        kind: i32,
        buf: *mut u16,
        off: i32,
        pitch: i32,
        blimit: u8,
        limit: u8,
        thresh: u8,
        bd: i32,
    );
}

/// Which reference loop-filter kernel to run (`svt_aom_lpf_*_c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LpfKind {
    H4 = 0,
    V4 = 1,
    H6 = 2,
    V6 = 3,
    H8 = 4,
    V8 = 5,
    H14 = 6,
    V14 = 7,
}

impl LpfKind {
    /// (tap reach each side of the edge, is_vertical): the kernel touches
    /// `reach` samples on each side along the filter axis, over 4 lines.
    pub fn geometry(self) -> (usize, bool) {
        match self {
            LpfKind::H4 => (2, false),
            LpfKind::V4 => (2, true),
            LpfKind::H6 => (3, false),
            LpfKind::V6 => (3, true),
            LpfKind::H8 => (4, false),
            LpfKind::V8 => (4, true),
            LpfKind::H14 => (7, false),
            LpfKind::V14 => (7, true),
        }
    }
}

/// Run the reference C loop-filter kernel in place. `off` indexes q0 of the
/// first filtered line; bounds are asserted against the kernel's reach.
pub fn lpf(kind: LpfKind, buf: &mut [u8], off: usize, pitch: usize, mblim: u8, lim: u8, hev: u8) {
    let (reach, vertical) = kind.geometry();
    let (axis_step, line_step) = if vertical { (1, pitch) } else { (pitch, 1) };
    // First line lowest tap / last line highest tap must be in bounds.
    assert!(off >= reach * axis_step);
    assert!(off + 3 * line_step + (reach - 1) * axis_step < buf.len());
    unsafe {
        ref_lpf(
            kind as i32,
            buf.as_mut_ptr(),
            off as i32,
            pitch as i32,
            mblim,
            lim,
            hev,
        )
    };
}

/// Run the reference C HIGH-BIT-DEPTH loop-filter kernel in place on a u16
/// plane (`svt_aom_highbd_lpf_*_c`), `bd` in {10, 12}. Same geometry/bounds
/// contract as [`lpf`].
#[allow(clippy::too_many_arguments)]
pub fn lpf_hbd(
    kind: LpfKind,
    buf: &mut [u16],
    off: usize,
    pitch: usize,
    mblim: u8,
    lim: u8,
    hev: u8,
    bd: i32,
) {
    let (reach, vertical) = kind.geometry();
    let (axis_step, line_step) = if vertical { (1, pitch) } else { (pitch, 1) };
    assert!(off >= reach * axis_step);
    assert!(off + 3 * line_step + (reach - 1) * axis_step < buf.len());
    unsafe {
        ref_lpf_hbd(
            kind as i32,
            buf.as_mut_ptr(),
            off as i32,
            pitch as i32,
            mblim,
            lim,
            hev,
            bd,
        )
    };
}

// ---- High-bit-depth distortion / variance / SAD kernels ----

unsafe extern "C" {
    pub(super) fn ref_full_distortion_kernel16(
        input: *const u16,
        in_off: u32,
        in_stride: u32,
        pred: *const u16,
        pred_off: i32,
        pred_stride: u32,
        w: u32,
        h: u32,
    ) -> u64;
    pub(super) fn ref_variance_highbd(
        a: *const u16,
        a_stride: i32,
        b: *const u16,
        b_stride: i32,
        w: i32,
        h: i32,
        sse_out: *mut u32,
    ) -> u32;
    pub(super) fn ref_sad_16b_kernel(
        src: *const u16,
        src_stride: u32,
        r: *const u16,
        ref_stride: u32,
        height: u32,
        width: u32,
    ) -> u32;
}

/// Reference `svt_full_distortion_kernel16_bits_c`: SSE between two u16 planes
/// over a `w x h` window. Offsets are u16-element indices (not bytes).
#[allow(clippy::too_many_arguments)]
pub fn full_distortion_kernel16(
    input: &[u16],
    in_off: usize,
    in_stride: usize,
    pred: &[u16],
    pred_off: usize,
    pred_stride: usize,
    w: usize,
    h: usize,
) -> u64 {
    unsafe {
        ref_full_distortion_kernel16(
            input.as_ptr(),
            in_off as u32,
            in_stride as u32,
            pred.as_ptr(),
            pred_off as i32,
            pred_stride as u32,
            w as u32,
            h as u32,
        )
    }
}

/// Reference `svt_aom_variance_highbd_c`: returns `(sse, variance)` for two
/// u16 planes over `w x h`.
pub fn variance_highbd(
    a: &[u16],
    a_stride: usize,
    b: &[u16],
    b_stride: usize,
    w: usize,
    h: usize,
) -> (u32, u32) {
    let mut sse = 0u32;
    let var = unsafe {
        ref_variance_highbd(
            a.as_ptr(),
            a_stride as i32,
            b.as_ptr(),
            b_stride as i32,
            w as i32,
            h as i32,
            &mut sse,
        )
    };
    (sse, var)
}

/// Reference `svt_aom_sad_16b_kernel_c`. The C order is `(.., height, width)`;
/// this wrapper takes `(width, height)` to match the port's house convention
/// and swaps internally.
pub fn sad_16b_kernel(
    src: &[u16],
    src_stride: usize,
    r: &[u16],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    unsafe {
        ref_sad_16b_kernel(
            src.as_ptr(),
            src_stride as u32,
            r.as_ptr(),
            ref_stride as u32,
            height as u32,
            width as u32,
        )
    }
}

/// Reference `svt_aom_update_sharpness` limits: `(lim, mblim)` arrays
/// indexed by filter level 0..=63.
pub fn lf_limits(sharpness: u8) -> ([u8; 64], [u8; 64]) {
    let mut lim = [0u8; 64];
    let mut mblim = [0u8; 64];
    unsafe { ref_lf_limits(sharpness as i32, lim.as_mut_ptr(), mblim.as_mut_ptr()) };
    (lim, mblim)
}

// ---- CDEF reference kernels ----

unsafe extern "C" {
    pub(super) fn ref_cdef_find_dir(
        img: *const u16,
        stride: i32,
        var: *mut i32,
        coeff_shift: i32,
    ) -> u8;
    pub(super) fn ref_cdef_find_dir_8bit(
        img: *const u8,
        stride: i32,
        var: *mut i32,
        coeff_shift: i32,
    ) -> u8;
    pub(super) fn ref_cdef_filter_block_8(
        dst: *mut u8,
        dstride: i32,
        input: *const u16,
        pri_strength: i32,
        sec_strength: i32,
        dir: i32,
        pri_damping: i32,
        sec_damping: i32,
        bsize: i32,
        coeff_shift: i32,
        subsampling_factor: u8,
    );
    pub(super) fn ref_cdef_filter_block_8bit(
        dst: *mut u8,
        dstride: i32,
        input: *const u8,
        pri_strength: i32,
        sec_strength: i32,
        dir: i32,
        damping: i32,
        bsize: i32,
        coeff_shift: i32,
        subsampling_factor: u8,
    );
    pub(super) fn ref_cdef_filter_block_16(
        dst: *mut u16,
        dstride: i32,
        input: *const u16,
        pri_strength: i32,
        sec_strength: i32,
        dir: i32,
        pri_damping: i32,
        sec_damping: i32,
        bsize: i32,
        coeff_shift: i32,
        subsampling_factor: u8,
    );
    pub(super) fn ref_compute_cdef_dist_16bit(
        plane: *const u16,
        dstride: i32,
        packed: *const u16,
        byx: *const u8,
        cdef_count: i32,
        bsize: i32,
        coeff_shift: i32,
        subsampling_factor: u8,
    ) -> u64;
    pub(super) fn ref_compute_cdef_dist_8bit(
        plane: *const u8,
        dstride: i32,
        packed: *const u8,
        byx: *const u8,
        cdef_count: i32,
        bsize: i32,
        coeff_shift: i32,
        subsampling_factor: u8,
    ) -> u64;
}

/// Reference `svt_aom_cdef_find_dir_c`: 8x8 direction search over 16-bit
/// pixels. Returns `(dir, var)`.
pub fn cdef_find_dir(img: &[u16], stride: usize, coeff_shift: i32) -> (u8, i32) {
    assert!(img.len() >= 7 * stride + 8);
    let mut var = 0i32;
    let dir = unsafe { ref_cdef_find_dir(img.as_ptr(), stride as i32, &mut var, coeff_shift) };
    (dir, var)
}

/// Reference `svt_aom_cdef_find_dir_8bit_c`.
pub fn cdef_find_dir_8bit(img: &[u8], stride: usize, coeff_shift: i32) -> (u8, i32) {
    assert!(img.len() >= 7 * stride + 8);
    let mut var = 0i32;
    let dir = unsafe { ref_cdef_find_dir_8bit(img.as_ptr(), stride as i32, &mut var, coeff_shift) };
    (dir, var)
}

/// Reference `svt_cdef_filter_block_c` (dst8 arm). `inb`/`ioff` locate the
/// block origin inside a `CDEF_BSTRIDE`(=144)-strided padded buffer; the
/// asserts keep every possible tap (`|off| <= 2*144+2`) in bounds.
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_8(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: u8,
) {
    const TAP_REACH: usize = 2 * 144 + 2;
    assert!(ioff >= TAP_REACH);
    assert!(ioff + 7 * 144 + 7 + TAP_REACH < inb.len());
    assert!(doff + 7 * dstride + 8 <= dst.len());
    assert!((0..=7).contains(&dir));
    unsafe {
        ref_cdef_filter_block_8(
            dst.as_mut_ptr().add(doff),
            dstride as i32,
            inb.as_ptr().add(ioff),
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
    }
}

/// Reference `svt_cdef_filter_block_8bit_c` (interior, no sentinel).
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_8bit(
    dst: &mut [u8],
    doff: usize,
    dstride: usize,
    inb: &[u8],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: u8,
) {
    const TAP_REACH: usize = 2 * 144 + 2;
    assert!(ioff >= TAP_REACH);
    assert!(ioff + 7 * 144 + 7 + TAP_REACH < inb.len());
    assert!(doff + 7 * dstride + 8 <= dst.len());
    assert!((0..=7).contains(&dir));
    unsafe {
        ref_cdef_filter_block_8bit(
            dst.as_mut_ptr().add(doff),
            dstride as i32,
            inb.as_ptr().add(ioff),
            pri_strength,
            sec_strength,
            dir,
            damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
    }
}

/// Reference `svt_cdef_filter_block_c` **dst16 arm** — the store path the
/// `is_16bit` (10-bit) pipeline takes (`svt_cdef_filter_fb` passes
/// `dst8 = NULL, dst16 = tmp_dst` at `is_16bit`, cdef_process.c:527-528).
#[allow(clippy::too_many_arguments)]
pub fn cdef_filter_block_16(
    dst: &mut [u16],
    doff: usize,
    dstride: usize,
    inb: &[u16],
    ioff: usize,
    pri_strength: i32,
    sec_strength: i32,
    dir: i32,
    pri_damping: i32,
    sec_damping: i32,
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: u8,
) {
    const TAP_REACH: usize = 2 * 144 + 2;
    assert!(ioff >= TAP_REACH);
    assert!(ioff + 7 * 144 + 7 + TAP_REACH < inb.len());
    assert!(doff + 7 * dstride + 8 <= dst.len());
    assert!((0..=7).contains(&dir));
    unsafe {
        ref_cdef_filter_block_16(
            dst.as_mut_ptr().add(doff),
            dstride as i32,
            inb.as_ptr().add(ioff),
            pri_strength,
            sec_strength,
            dir,
            pri_damping,
            sec_damping,
            bsize,
            coeff_shift,
            subsampling_factor,
        );
    }
}

/// Reference `svt_aom_compute_cdef_dist_16bit_c` (enc_cdef.c:77).
///
/// C's parameter names are inverted relative to its only call site: `plane`
/// here is C's `dst` (the SOURCE picture, already offset to the filter
/// block's top-left, at the picture stride `dstride`) and `packed` is C's
/// `src` (the tmp_dst buffer of packed filtered blocks). `byx` is the dlist
/// as flat `(by, bx)` pairs.
#[allow(clippy::too_many_arguments)]
pub fn compute_cdef_dist_16bit(
    plane: &[u16],
    plane_off: usize,
    dstride: usize,
    packed: &[u16],
    dlist: &[(u8, u8)],
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: u8,
) -> u64 {
    let byx: Vec<u8> = dlist.iter().flat_map(|&(by, bx)| [by, bx]).collect();
    assert!(plane_off <= plane.len());
    unsafe {
        ref_compute_cdef_dist_16bit(
            plane.as_ptr().add(plane_off),
            dstride as i32,
            packed.as_ptr(),
            byx.as_ptr(),
            dlist.len() as i32,
            bsize,
            coeff_shift,
            subsampling_factor,
        )
    }
}

/// Reference `svt_aom_compute_cdef_dist_8bit_c` (enc_cdef.c:114). Same
/// inverted naming as [`compute_cdef_dist_16bit`].
#[allow(clippy::too_many_arguments)]
pub fn compute_cdef_dist_8bit(
    plane: &[u8],
    plane_off: usize,
    dstride: usize,
    packed: &[u8],
    dlist: &[(u8, u8)],
    bsize: i32,
    coeff_shift: i32,
    subsampling_factor: u8,
) -> u64 {
    let byx: Vec<u8> = dlist.iter().flat_map(|&(by, bx)| [by, bx]).collect();
    assert!(plane_off <= plane.len());
    unsafe {
        ref_compute_cdef_dist_8bit(
            plane.as_ptr().add(plane_off),
            dstride as i32,
            packed.as_ptr(),
            byx.as_ptr(),
            dlist.len() as i32,
            bsize,
            coeff_shift,
            subsampling_factor,
        )
    }
}

// ---- CDEF strength picker (all three branches, C float/double semantics) ----

unsafe extern "C" {
    pub(super) fn ref_pick_cdef_from_qp(
        base_q_idx: i32,
        bit_depth: i32,
        is_screen_content: i32,
        is_intra: i32,
        pred_y_strength: *mut i32,
        pred_uv_strength: *mut i32,
    );
    pub(super) fn ref_pick_cdef_from_qp_intra(
        base_q_idx: i32,
        bit_depth: i32,
        pred_y_strength: *mut i32,
        pred_uv_strength: *mut i32,
    );
}

/// Reference `svt_pick_cdef_from_qp` (enc_cdef.c:823) at the given bit depth
/// (8/10/12 — the `EbBitDepth` enum value), selecting the branch the way C
/// does: `is_screen_content` wins over everything, else `is_intra` picks the
/// intra vs inter fit. Returns the packed `(y_strength, uv_strength)` pair,
/// evaluated with C's expression semantics (the screen arm is `double` +
/// truncating cast; the other two are `float` + `roundf`) against the
/// library's real `svt_aom_ac_quant_qtx`.
pub fn pick_cdef_from_qp(
    base_q_idx: u8,
    bit_depth: u8,
    is_screen_content: bool,
    is_intra: bool,
) -> (i32, i32) {
    let (mut y, mut uv) = (0i32, 0i32);
    unsafe {
        ref_pick_cdef_from_qp(
            base_q_idx as i32,
            bit_depth as i32,
            i32::from(is_screen_content),
            i32::from(is_intra),
            &mut y,
            &mut uv,
        )
    };
    (y, uv)
}

/// Reference `svt_pick_cdef_from_qp` intra branch at the given bit depth
/// (8/10/12 — the `EbBitDepth` enum value): returns the packed
/// `(y_strength, uv_strength)` pair for a qindex, evaluated with C float
/// semantics against the library's `svt_aom_ac_quant_qtx`.
pub fn pick_cdef_from_qp_intra(base_q_idx: u8, bit_depth: u8) -> (i32, i32) {
    let (mut y, mut uv) = (0i32, 0i32);
    unsafe { ref_pick_cdef_from_qp_intra(base_q_idx as i32, bit_depth as i32, &mut y, &mut uv) };
    (y, uv)
}

/// Reference `svt_pick_cdef_from_qp` SCREEN-CONTENT branch (enc_cdef.c:837-844)
/// at the given bit depth. C reaches this arm when
/// `allintra ? ppcs->sc_class5 : ppcs->sc_class1` is set (enc_cdef.c:913-916),
/// i.e. on `sc_class5` frames for the all-intra still path.
pub fn pick_cdef_from_qp_screen(base_q_idx: u8, bit_depth: u8) -> (i32, i32) {
    pick_cdef_from_qp(base_q_idx, bit_depth, true, true)
}

/// bd8 convenience wrapper (kept for existing callers).
pub fn pick_cdef_from_qp_intra_8bit(base_q_idx: u8) -> (i32, i32) {
    pick_cdef_from_qp_intra(base_q_idx, 8)
}

// ---- RD multiplier base ----

unsafe extern "C" {
    pub(super) fn ref_compute_rd_mult_based_on_qindex(
        bit_depth: i32,
        update_type: i32,
        qindex: i32,
    ) -> i32;
}

/// Reference `svt_aom_compute_rd_mult_based_on_qindex` (rc_process.c:365):
/// the per-(bit_depth, update_type, qindex) rdmult base that every post-MD
/// RD search's lambda is built from. `update_type` 0 == `SVT_AV1_KF_UPDATE`.
pub fn compute_rd_mult_based_on_qindex(bit_depth: u8, update_type: i32, qindex: u8) -> i32 {
    unsafe { ref_compute_rd_mult_based_on_qindex(bit_depth as i32, update_type, qindex as i32) }
}

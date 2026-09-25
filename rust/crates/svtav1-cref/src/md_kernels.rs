// MD fast-loop kernels (M6 leaf funnel): filter-intra predictor, aom
// Hadamard/SATD. All global `T` symbols in the non-LTO archive.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn svt_av1_filter_intra_predictor_c(
        dst: *mut u8,
        stride: isize,
        tx_size: u32,
        above: *const u8,
        left: *const u8,
        mode: i32,
    );
    pub(super) fn svt_aom_hadamard_8x8_c(src_diff: *const i16, src_stride: isize, coeff: *mut i32);
    pub(super) fn svt_aom_hadamard_16x16_c(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
    pub(super) fn svt_aom_hadamard_32x32_c(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
    pub(super) fn svt_aom_satd_c(coeff: *const i32, length: i32) -> i32;
    pub(super) fn svt_aom_highbd_hadamard_8x8_c(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
}

// The AVX2 Hadamard kernels exist only in an x86_64 build of the C library.
// On aarch64 the C encoder ships NEON kernels instead, so these symbols are
// absent and an ungated declaration makes EVERY c_parity test fail to LINK on
// ARM — which is how it stood before 2026-07-28, i.e. the C-parity gates could
// not run on ARM at all.
#[cfg(target_arch = "x86_64")]
unsafe extern "C" {
    pub(super) fn svt_aom_hadamard_16x16_avx2(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
    pub(super) fn svt_aom_hadamard_32x32_avx2(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
    pub(super) fn svt_aom_highbd_hadamard_8x8_avx2(
        src_diff: *const i16,
        src_stride: isize,
        coeff: *mut i32,
    );
}

/// Reference `svt_av1_filter_intra_predictor_c`.
///
/// `above_with_corner[0]` is the top-left corner sample; the block's above
/// row starts at `above_with_corner[1]` (C reads `&above[-1]` through
/// `above[bw-1]`). `left` holds `bh` samples. `c_tx_size` is the C TxSize
/// index of the (square, <=32x32) transform.
pub fn filter_intra_predictor(
    dst: &mut [u8],
    stride: usize,
    c_tx_size: usize,
    above_with_corner: &[u8],
    left: &[u8],
    mode: u8,
) {
    unsafe {
        svt_av1_filter_intra_predictor_c(
            dst.as_mut_ptr(),
            stride as isize,
            c_tx_size as u32,
            above_with_corner.as_ptr().add(1),
            left.as_ptr(),
            mode as i32,
        );
    }
}

/// Reference `svt_aom_hadamard_{8x8,16x16,32x32}_c` (dim = 8, 16 or 32).
pub fn hadamard(dim: usize, src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    assert!(coeff.len() >= dim * dim);
    unsafe {
        match dim {
            8 => svt_aom_hadamard_8x8_c(src_diff.as_ptr(), src_stride as isize, coeff.as_mut_ptr()),
            16 => {
                svt_aom_hadamard_16x16_c(src_diff.as_ptr(), src_stride as isize, coeff.as_mut_ptr())
            }
            32 => {
                svt_aom_hadamard_32x32_c(src_diff.as_ptr(), src_stride as isize, coeff.as_mut_ptr())
            }
            _ => panic!("unsupported hadamard dim {dim}"),
        }
    }
}

/// Reference `svt_aom_satd_c`.
pub fn satd(coeff: &[i32]) -> i32 {
    unsafe { svt_aom_satd_c(coeff.as_ptr(), coeff.len() as i32) }
}

/// Reference `svt_aom_highbd_hadamard_8x8_c`: int16-truncating first pass,
/// int32 second pass. Exists in every oracle (pre-`1e3da1d7` it lived in
/// picture_operators_c.c); Ghost Robot moved it to
/// highbd_picture_operators_c.c and made it the inner kernel of the new
/// `svt_aom_highbd_hadamard_{16x16,32x32}_c`, which no other oracle ships —
/// so it is the compositional oracle those are pinned against.
pub fn highbd_hadamard_8x8_c(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    assert!(coeff.len() >= 64);
    unsafe {
        svt_aom_highbd_hadamard_8x8_c(src_diff.as_ptr(), src_stride as isize, coeff.as_mut_ptr());
    }
}

/// `svt_aom_highbd_hadamard_8x8_avx2` — the kernel Ghost Robot's
/// `hadamard_path` binds on x86 for TX_8X8 after `1e3da1d7`
/// (`SET_AVX2(svt_aom_highbd_hadamard_8x8, _c, _avx2)`, common_dsp_rtcd.c):
/// both passes in 32-bit lanes, nothing truncates.
#[cfg(target_arch = "x86_64")]
pub fn highbd_hadamard_8x8_avx2(src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    assert!(coeff.len() >= 64);
    unsafe {
        svt_aom_highbd_hadamard_8x8_avx2(src_diff.as_ptr(), src_stride as isize, coeff.as_mut_ptr());
    }
}

/// `svt_aom_hadamard_{16x16,32x32}_avx2` — the kernel the ENCODER actually
/// runs (`SET_AVX2(svt_aom_hadamard_32x32, _c, _avx2)`, common_dsp_rtcd.c
/// :1047-1048) on any AVX2 host. It is NOT equivalent to the `_c` reference
/// above once the residual exceeds the 8-bit range: `svt_aom_hadamard_32x32
/// _avx2` buffers its four 16x16 sub-transforms in an `int16_t temp_coeff
/// [32*32]` (pic_operators_intrin_avx2.c:1721, `is_final = 0`) and only then
/// sign-extends to 32-bit, whereas `svt_aom_hadamard_32x32_c` carries those
/// sub-results in `int32_t`. At 8-bit the 16x16 stage spans [-32640, 32640]
/// and fits int16, so the two agree; at 10-bit it reaches ~+/-130560 and the
/// AVX2 kernel WRAPS. The bd10 MD fast loop feeds exactly such residuals, so
/// bit-exactness with the real encoder requires matching `_avx2` (task #94).
#[cfg(target_arch = "x86_64")]
pub fn hadamard_avx2(dim: usize, src_diff: &[i16], src_stride: usize, coeff: &mut [i32]) {
    assert!(coeff.len() >= dim * dim);
    unsafe {
        match dim {
            16 => svt_aom_hadamard_16x16_avx2(
                src_diff.as_ptr(),
                src_stride as isize,
                coeff.as_mut_ptr(),
            ),
            32 => svt_aom_hadamard_32x32_avx2(
                src_diff.as_ptr(),
                src_stride as isize,
                coeff.as_mut_ptr(),
            ),
            _ => panic!("unsupported avx2 hadamard dim {dim}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Intra edge filter / upsample + upsample-capable dr prediction kernels
// (M5 leaf funnel: SH enable_intra_edge_filter=1 directional prediction).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_filter_intra_edge(p: *mut u8, sz: i32, strength: i32);
    pub(super) fn svt_av1_upsample_intra_edge_c(p: *mut u8, sz: i32);
    pub(super) fn svt_aom_intra_edge_filter_strength(
        bs0: i32,
        bs1: i32,
        delta: i32,
        type_: i32,
    ) -> i32;
    pub(super) fn svt_aom_use_intra_edge_upsample(
        bs0: i32,
        bs1: i32,
        delta: i32,
        type_: i32,
    ) -> i32;
    pub(super) fn svt_av1_dr_prediction_z1_c(
        dst: *mut u8,
        stride: isize,
        bw: i32,
        bh: i32,
        above: *const u8,
        left: *const u8,
        upsample_above: i32,
        dx: i32,
        dy: i32,
    );
    pub(super) fn svt_av1_dr_prediction_z2_c(
        dst: *mut u8,
        stride: isize,
        bw: i32,
        bh: i32,
        above: *const u8,
        left: *const u8,
        upsample_above: i32,
        upsample_left: i32,
        dx: i32,
        dy: i32,
    );
    pub(super) fn svt_av1_dr_prediction_z3_c(
        dst: *mut u8,
        stride: isize,
        bw: i32,
        bh: i32,
        above: *const u8,
        left: *const u8,
        upsample_left: i32,
        dx: i32,
        dy: i32,
    );
}

/// Reference `svt_av1_filter_intra_edge_c` on `p[start..start+sz]`.
pub fn filter_intra_edge(p: &mut [u8], start: usize, sz: usize, strength: i32) {
    unsafe { ref_filter_intra_edge(p.as_mut_ptr().add(start), sz as i32, strength) }
}

// ---------------------------------------------------------------------------

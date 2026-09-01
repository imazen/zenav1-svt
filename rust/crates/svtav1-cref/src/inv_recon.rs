//! Bindings for the inverse-transform RECONSTRUCTION entries of
//! `Codec/inv_transforms.c` — see `shims/inv_recon_shims.c` for why these go
//! through a shim (RTCD setup is mandatory: the `svt_av1_inv_txfm_add` and
//! `svt_av1_inv_txfm2d_add_*` symbols are `.bss` function pointers in this
//! build, NULL until `svt_aom_setup_common_rtcd_internal` runs).

unsafe extern "C" {
    fn ref_inv_transform_recon8bit(
        coeff: *const i32,
        pred: *const u8,
        pred_stride: u32,
        recon: *mut u8,
        recon_stride: u32,
        txsize: i32,
        tx_type: i32,
        eob: u32,
        lossless: u8,
        alias_in_place: i32,
    );
    fn ref_inv_txfm_add_c(
        coeff: *const i32,
        pred: *const u8,
        pred_stride: u32,
        recon: *mut u8,
        recon_stride: u32,
        txsize: i32,
        tx_type: i32,
        eob: u32,
        lossless: u8,
    );
    fn ref_inv_transform_recon(
        coeff: *const i32,
        pred: *const u16,
        pred_stride: u32,
        recon: *mut u16,
        recon_stride: u32,
        txsize: i32,
        bit_depth: u32,
        tx_type: i32,
        eob: u32,
        lossless: u8,
        alias_in_place: i32,
    );
}

/// `svt_aom_inv_transform_recon8bit` (inv_transforms.c:3138) with distinct
/// read/write buffers — the shape C forces `eob = av1_get_max_eob` in.
/// Returns the `w * h` reconstruction.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon8bit(
    coeff: &[i32],
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    txsize: usize,
    tx_type: usize,
    eob: u32,
    lossless: bool,
) -> Vec<u8> {
    let mut recon = vec![0u8; w * h];
    unsafe {
        ref_inv_transform_recon8bit(
            coeff.as_ptr(),
            pred.as_ptr(),
            pred_stride as u32,
            recon.as_mut_ptr(),
            w as u32,
            txsize as i32,
            tx_type as i32,
            eob,
            u8::from(lossless),
            0,
        )
    };
    recon
}

/// `svt_aom_inv_transform_recon8bit` with ONE buffer for both pointers — the
/// aliasing shape TPL uses, and the only one in which C keeps the caller's
/// `eob`. `pred` seeds the buffer; the reconstruction replaces it.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon8bit_in_place(
    coeff: &[i32],
    pred: &[u8],
    w: usize,
    h: usize,
    txsize: usize,
    tx_type: usize,
    eob: u32,
    lossless: bool,
) -> Vec<u8> {
    let mut recon = pred[..w * h].to_vec();
    unsafe {
        ref_inv_transform_recon8bit(
            coeff.as_ptr(),
            core::ptr::null(),
            w as u32,
            recon.as_mut_ptr(),
            w as u32,
            txsize as i32,
            tx_type as i32,
            eob,
            u8::from(lossless),
            1,
        )
    };
    recon
}

/// `svt_av1_inv_txfm_add_c` (inv_transforms.c:3266) — the pinned SCALAR
/// route, for attributing a divergence between the port and the
/// RTCD-dispatched entry above.
#[allow(clippy::too_many_arguments)]
pub fn inv_txfm_add_c(
    coeff: &[i32],
    pred: &[u8],
    pred_stride: usize,
    w: usize,
    h: usize,
    txsize: usize,
    tx_type: usize,
    eob: u32,
    lossless: bool,
) -> Vec<u8> {
    let mut recon = vec![0u8; w * h];
    unsafe {
        ref_inv_txfm_add_c(
            coeff.as_ptr(),
            pred.as_ptr(),
            pred_stride as u32,
            recon.as_mut_ptr(),
            w as u32,
            txsize as i32,
            tx_type as i32,
            eob,
            u8::from(lossless),
        )
    };
    recon
}

/// `svt_aom_inv_transform_recon` (inv_transforms.c:3237) — u16 pixels at any
/// bit depth, distinct read/write buffers.
#[allow(clippy::too_many_arguments)]
pub fn inv_transform_recon(
    coeff: &[i32],
    pred: &[u16],
    pred_stride: usize,
    w: usize,
    h: usize,
    txsize: usize,
    bit_depth: u32,
    tx_type: usize,
    eob: u32,
    lossless: bool,
) -> Vec<u16> {
    let mut recon = vec![0u16; w * h];
    unsafe {
        ref_inv_transform_recon(
            coeff.as_ptr(),
            pred.as_ptr(),
            pred_stride as u32,
            recon.as_mut_ptr(),
            w as u32,
            txsize as i32,
            bit_depth,
            tx_type as i32,
            eob,
            u8::from(lossless),
            0,
        )
    };
    recon
}

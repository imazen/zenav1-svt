// Quantizers (full_loop.c) — `svt_av1_quantize_fp_facade` / `svt_aom_quantize_b`
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub(super) fn ref_quantize_fp(
        coeff: *const i32,
        n_coeffs: isize,
        zbin: *const i16,
        round_fp: *const i16,
        quant_fp: *const i16,
        quant_shift: *const i16,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        dequant: *const i16,
        scan: *const i16,
        iscan: *const i16,
        log_scale: i32,
        dispatch: i32,
    ) -> u16;
    pub(super) fn ref_spatial_full_distortion_ssim(
        input: *const u8,
        input_offset: u32,
        input_stride: u32,
        recon: *const u8,
        recon_offset: i32,
        recon_stride: u32,
        area_width: u32,
        area_height: u32,
        ac_bias: f64,
    ) -> u64;
    pub(super) fn ref_generate_noise_table(
        width: u32,
        height: u32,
        noise_strength: u32,
        noise_strength_chroma: i32,
        noise_chroma_from_luma: i32,
        noise_size: i32,
        color_range_provided: i32,
        color_range: i32,
        avif: i32,
        out: *mut i32,
    ) -> i32;
    pub(super) fn ref_quantize_b_qm(
        coeff: *const i32,
        n_coeffs: isize,
        zbin: *const i16,
        round: *const i16,
        quant: *const i16,
        quant_shift: *const i16,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        dequant: *const i16,
        scan: *const i16,
        iscan: *const i16,
        qm: *const u8,
        iqm: *const u8,
        log_scale: i32,
    ) -> u16;
    pub(super) fn ref_quantize_fp_qm(
        coeff: *const i32,
        n_coeffs: isize,
        zbin: *const i16,
        round_fp: *const i16,
        quant_fp: *const i16,
        quant_shift: *const i16,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        dequant: *const i16,
        scan: *const i16,
        iscan: *const i16,
        qm: *const u8,
        iqm: *const u8,
        log_scale: i32,
    ) -> u16;
    pub(super) fn ref_quantize_b(
        coeff: *const i32,
        n_coeffs: isize,
        zbin: *const i16,
        round: *const i16,
        quant: *const i16,
        quant_shift: *const i16,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        dequant: *const i16,
        scan: *const i16,
        iscan: *const i16,
        log_scale: i32,
        dispatch: i32,
    ) -> u16;
}

/// One qindex row of the C `Quants`/`Dequants` tables in the exact SHAPE the
/// quantize kernels require: `DECLARE_ALIGNED(16, int16_t, y_quant[..][8])`
/// (pcs.h:78, commented "8: SIMD width"), filled `[DC, AC, AC, AC, AC, AC, AC,
/// AC]` by `svt_av1_build_quantizer` (md_config_process.c:151 copies `[1]` into
/// `[2..8]`).
///
/// The 8 lanes are NOT padding. The scalar `_c` kernels only read `[0]`/`[1]`,
/// but the SIMD ones `_mm_loadu_si128` the whole 8-lane row and
/// `init_one_qp`/`update_qp` (av1_quantize_avx2.c:41/:69) broadcast the HIGH
/// 64 bits — lanes `[4..8]` — as the AC quantizer for every coefficient past
/// the first 16. A 2-lane row therefore reads 6 lanes of adjacent memory and
/// silently mis-quantizes (or faults). Use [`QuantRow::new`].
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, align(16))]
pub struct QuantRow {
    pub zbin: [i16; 8],
    pub round: [i16; 8],
    pub quant: [i16; 8],
    pub quant_shift: [i16; 8],
    pub round_fp: [i16; 8],
    pub quant_fp: [i16; 8],
    pub dequant: [i16; 8],
}

unsafe extern "C" {
    pub(super) fn ref_build_quantizer_rows(bit_depth: i32, sharpness: i32, out: *mut i16);
}

/// All 256 luma rows from real C `svt_av1_build_quantizer`, with sequence
/// base qindex 31 and zero delta-q. Includes C's replicated AC lanes.
pub fn quantizer_rows(bit_depth: u8, sharpness: i8) -> Vec<QuantRow> {
    assert!(matches!(bit_depth, 8 | 10));
    assert_eq!(core::mem::size_of::<QuantRow>(), 7 * 8 * 2);
    let mut rows = vec![QuantRow::default(); 256];
    // SAFETY: C writes exactly 256 rows of seven eight-lane i16 arrays.
    // QuantRow has that C layout, with no inter-row padding (asserted above).
    unsafe {
        ref_build_quantizer_rows(
            i32::from(bit_depth),
            i32::from(sharpness),
            rows.as_mut_ptr().cast::<i16>(),
        );
    }
    rows
}

impl QuantRow {
    /// Build a row from its (DC, AC) pair, replicating AC across lanes 1..8
    /// exactly like `svt_av1_build_quantizer`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        zbin: [i16; 2],
        round: [i16; 2],
        quant: [i16; 2],
        quant_shift: [i16; 2],
        round_fp: [i16; 2],
        quant_fp: [i16; 2],
        dequant: [i16; 2],
    ) -> Self {
        let lanes = |v: [i16; 2]| -> [i16; 8] { [v[0], v[1], v[1], v[1], v[1], v[1], v[1], v[1]] };
        Self {
            zbin: lanes(zbin),
            round: lanes(round),
            quant: lanes(quant),
            quant_shift: lanes(quant_shift),
            round_fp: lanes(round_fp),
            quant_fp: lanes(quant_fp),
            dequant: lanes(dequant),
        }
    }
}

/// `iscan[scan[i]] = i` — the inverse scan the SIMD kernels index by.
pub(super) fn build_iscan(scan: &[u16]) -> Vec<i16> {
    let mut iscan = vec![0i16; scan.len()];
    for (i, &rc) in scan.iter().enumerate() {
        iscan[rc as usize] = i as i16;
    }
    iscan
}

/// Drives `svt_av1_quantize_fp_facade`'s non-QM branch. `dispatch = true`
/// calls the RTCD pointer (what a real encode runs); `false` calls the scalar
/// `_c` reference. Returns eob.
pub fn quantize_fp(
    coeff: &[i32],
    row: &QuantRow,
    scan: &[u16],
    log_scale: i32,
    dispatch: bool,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    assert_eq!(coeff.len(), scan.len());
    assert!(qcoeff.len() >= coeff.len() && dqcoeff.len() >= coeff.len());
    let iscan = build_iscan(scan);
    let scan_i16: Vec<i16> = scan.iter().map(|&v| v as i16).collect();
    unsafe {
        ref_quantize_fp(
            coeff.as_ptr(),
            coeff.len() as isize,
            row.zbin.as_ptr(),
            row.round_fp.as_ptr(),
            row.quant_fp.as_ptr(),
            row.quant_shift.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            row.dequant.as_ptr(),
            scan_i16.as_ptr(),
            iscan.as_ptr(),
            log_scale,
            i32::from(dispatch),
        )
    }
}

/// Drives `svt_aom_quantize_b` (QM off). Returns eob.
pub fn quantize_b(
    coeff: &[i32],
    row: &QuantRow,
    scan: &[u16],
    log_scale: i32,
    dispatch: bool,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    assert_eq!(coeff.len(), scan.len());
    assert!(qcoeff.len() >= coeff.len() && dqcoeff.len() >= coeff.len());
    let iscan = build_iscan(scan);
    let scan_i16: Vec<i16> = scan.iter().map(|&v| v as i16).collect();
    unsafe {
        ref_quantize_b(
            coeff.as_ptr(),
            coeff.len() as isize,
            row.zbin.as_ptr(),
            row.round.as_ptr(),
            row.quant.as_ptr(),
            row.quant_shift.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            row.dequant.as_ptr(),
            scan_i16.as_ptr(),
            iscan.as_ptr(),
            log_scale,
            i32::from(dispatch),
        )
    }
}

/// Drives the exported `svt_spatial_full_distortion_ssim_kernel` (tune-SSIM
/// MD distortion, mode_decision.c:4430), 8-bit path.
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_ssim(
    input: &[u8],
    input_offset: usize,
    input_stride: usize,
    recon: &[u8],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
    ac_bias: f64,
) -> u64 {
    assert!(input.len() >= input_offset + (area_height - 1) * input_stride + area_width);
    assert!(recon.len() >= recon_offset + (area_height - 1) * recon_stride + area_width);
    unsafe {
        ref_spatial_full_distortion_ssim(
            input.as_ptr(),
            input_offset as u32,
            input_stride as u32,
            recon.as_ptr(),
            recon_offset as i32,
            recon_stride as u32,
            area_width as u32,
            area_height as u32,
            ac_bias,
        )
    }
}

/// Drives the exported `svt_av1_generate_noise_table` (photon-noise film
/// grain, noise_generation.c) and returns the flattened AomFilmGrain as
/// 159 i32s (see the shim comment for the layout).
#[allow(clippy::too_many_arguments)]
pub fn generate_noise_table(
    width: u32,
    height: u32,
    noise_strength: u32,
    noise_strength_chroma: i32,
    noise_chroma_from_luma: i32,
    noise_size: i32,
    color_range_provided: bool,
    full_range: bool,
    avif: bool,
) -> Option<Vec<i32>> {
    let mut out = vec![0i32; 159];
    let n = unsafe {
        ref_generate_noise_table(
            width,
            height,
            noise_strength,
            noise_strength_chroma,
            noise_chroma_from_luma,
            noise_size,
            i32::from(color_range_provided),
            i32::from(full_range),
            i32::from(avif),
            out.as_mut_ptr(),
        )
    };
    (n == 159).then_some(out)
}

/// Drives `svt_aom_quantize_b_c` with non-NULL qm/iqm (the QM branch).
#[allow(clippy::too_many_arguments)]
pub fn quantize_b_qm(
    coeff: &[i32],
    row: &QuantRow,
    scan: &[u16],
    log_scale: i32,
    qm: &[u8],
    iqm: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    assert_eq!(coeff.len(), scan.len());
    assert!(qm.len() >= coeff.len() && iqm.len() >= coeff.len());
    assert!(qcoeff.len() >= coeff.len() && dqcoeff.len() >= coeff.len());
    let iscan = build_iscan(scan);
    let scan_i16: Vec<i16> = scan.iter().map(|&v| v as i16).collect();
    unsafe {
        ref_quantize_b_qm(
            coeff.as_ptr(),
            coeff.len() as isize,
            row.zbin.as_ptr(),
            row.round.as_ptr(),
            row.quant.as_ptr(),
            row.quant_shift.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            row.dequant.as_ptr(),
            scan_i16.as_ptr(),
            iscan.as_ptr(),
            qm.as_ptr(),
            iqm.as_ptr(),
            log_scale,
        )
    }
}

/// Drives `svt_av1_quantize_fp_qm_c` (the fp QM branch). Returns eob.
#[allow(clippy::too_many_arguments)]
pub fn quantize_fp_qm(
    coeff: &[i32],
    row: &QuantRow,
    scan: &[u16],
    log_scale: i32,
    qm: &[u8],
    iqm: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    assert_eq!(coeff.len(), scan.len());
    assert!(qm.len() >= coeff.len() && iqm.len() >= coeff.len());
    assert!(qcoeff.len() >= coeff.len() && dqcoeff.len() >= coeff.len());
    let iscan = build_iscan(scan);
    let scan_i16: Vec<i16> = scan.iter().map(|&v| v as i16).collect();
    unsafe {
        ref_quantize_fp_qm(
            coeff.as_ptr(),
            coeff.len() as isize,
            row.zbin.as_ptr(),
            row.round_fp.as_ptr(),
            row.quant_fp.as_ptr(),
            row.quant_shift.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            row.dequant.as_ptr(),
            scan_i16.as_ptr(),
            iscan.as_ptr(),
            qm.as_ptr(),
            iqm.as_ptr(),
            log_scale,
        )
    }
}

// ---------------------------------------------------------------------------

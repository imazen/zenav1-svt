//! Test-only FFI harness over the in-tree C SVT-AV1 static library.
//!
//! This crate exists solely for differential parity testing: every Rust
//! module ports a C module, and the tests here drive the *actual* C
//! implementation on identical inputs to assert bit-for-bit equality.
//!
//! This is the single sanctioned `unsafe` exception in the workspace — it is
//! `publish = false`, used only as a dev-dependency, and never part of a
//! shipped artifact.

use std::ffi::c_void;

unsafe extern "C" {
    fn ref_od_ec_enc_sizeof() -> usize;
    fn ref_od_ec_enc_alignof() -> usize;
    fn ref_od_ec_enc_init(enc: *mut c_void, size: u32);
    fn ref_od_ec_enc_reset(enc: *mut c_void);
    fn ref_od_ec_enc_clear(enc: *mut c_void);
    fn ref_od_ec_encode_cdf_q15(enc: *mut c_void, s: i32, icdf: *const u16, nsyms: i32);
    fn ref_od_ec_encode_bool_q15(enc: *mut c_void, val: i32, f: u32);
    fn ref_od_ec_enc_done(enc: *mut c_void, nbytes: *mut u32) -> *const u8;
    fn ref_od_ec_enc_error(enc: *const c_void) -> i32;
    fn ref_od_ec_enc_tell(enc: *const c_void) -> u32;
    fn ref_update_cdf(cdf: *mut u16, val: i8, nsymbs: i32);
    fn ref_write_symbol(enc: *mut c_void, symb: i32, cdf: *mut u16, nsymbs: i32);
}

/// The reference C range encoder (`OdEcEnc`), heap-allocated as an opaque blob.
pub struct RefEcEnc {
    /// Backing storage for the C struct; `u64` alignment covers the struct's
    /// requirement (verified against `_Alignof(OdEcEnc)` at construction).
    blob: Box<[u64]>,
    /// `done()` may only be called once before a reset.
    finished: bool,
}

impl RefEcEnc {
    /// Create and initialize a reference encoder with `size` bytes of initial
    /// buffer storage (the C side reallocs as needed).
    pub fn new(size: u32) -> Self {
        let bytes = unsafe { ref_od_ec_enc_sizeof() };
        let align = unsafe { ref_od_ec_enc_alignof() };
        assert!(
            align <= 8,
            "OdEcEnc alignment {align} exceeds u64 alignment"
        );
        let words = bytes.div_ceil(8);
        let blob = vec![0u64; words].into_boxed_slice();
        let mut this = Self {
            blob,
            finished: false,
        };
        unsafe { ref_od_ec_enc_init(this.ptr(), size) };
        this
    }

    fn ptr(&mut self) -> *mut c_void {
        self.blob.as_mut_ptr() as *mut c_void
    }

    fn cptr(&self) -> *const c_void {
        self.blob.as_ptr() as *const c_void
    }

    /// Encode symbol `s` with the given ICDF table (C layout: values then a
    /// structural 0 at `icdf[nsyms-1]`; slice must hold at least `nsyms`).
    pub fn encode_cdf_q15(&mut self, s: usize, icdf: &[u16], nsyms: usize) {
        assert!(!self.finished);
        assert!(s < nsyms && icdf.len() >= nsyms);
        unsafe { ref_od_ec_encode_cdf_q15(self.ptr(), s as i32, icdf.as_ptr(), nsyms as i32) };
    }

    /// Encode a boolean with probability `f` (Q15) that the value is one.
    pub fn encode_bool_q15(&mut self, val: bool, f: u32) {
        assert!(!self.finished);
        unsafe { ref_od_ec_encode_bool_q15(self.ptr(), i32::from(val), f) };
    }

    /// The real write path: encode symbol then adapt the CDF in place
    /// (C layout: slice must hold `nsymbs + 1` entries, counter at `[nsymbs]`).
    pub fn write_symbol(&mut self, symb: usize, cdf: &mut [u16], nsymbs: usize) {
        assert!(!self.finished);
        assert!(symb < nsymbs && cdf.len() >= nsymbs + 1);
        unsafe { ref_write_symbol(self.ptr(), symb as i32, cdf.as_mut_ptr(), nsymbs as i32) };
    }

    /// Bits "used" so far (reference `svt_od_ec_enc_tell`).
    pub fn tell(&self) -> u32 {
        unsafe { ref_od_ec_enc_tell(self.cptr()) }
    }

    /// Finalize and copy out the encoded bytes.
    pub fn done(&mut self) -> Vec<u8> {
        assert!(!self.finished, "done() called twice without reset");
        self.finished = true;
        let mut nbytes = 0u32;
        let p = unsafe { ref_od_ec_enc_done(self.ptr(), &mut nbytes) };
        assert!(!p.is_null(), "C encoder reported an error in done()");
        assert_eq!(unsafe { ref_od_ec_enc_error(self.cptr()) }, 0);
        unsafe { std::slice::from_raw_parts(p, nbytes as usize) }.to_vec()
    }

    /// Reset for reuse after `done()`.
    pub fn reset(&mut self) {
        unsafe { ref_od_ec_enc_reset(self.ptr()) };
        self.finished = false;
    }
}

impl Drop for RefEcEnc {
    fn drop(&mut self) {
        unsafe { ref_od_ec_enc_clear(self.ptr()) };
    }
}

/// Reference CDF adaptation (`update_cdf` from `cabac_context_model.h`).
/// C layout: `cdf[nsymbs]` is the adaptation counter, so the slice must hold
/// at least `nsymbs + 1` entries.
pub fn update_cdf(cdf: &mut [u16], val: usize, nsymbs: usize) {
    assert!(cdf.len() >= nsymbs + 1 && val < nsymbs);
    unsafe { ref_update_cdf(cdf.as_mut_ptr(), val as i8, nsymbs as i32) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_encode_and_done() {
        let mut enc = RefEcEnc::new(1024);
        // 2-symbol uniform ICDF, C layout: [16384, 0(structural), 0(counter)]
        let icdf = [16384u16, 0, 0];
        for i in 0..64 {
            enc.encode_cdf_q15(i & 1, &icdf, 2);
        }
        let bytes = enc.done();
        assert!(!bytes.is_empty(), "64 coin flips must produce output bytes");
        // ~1 bit/symbol -> ~8 bytes plus termination
        assert!(
            bytes.len() >= 8 && bytes.len() <= 12,
            "got {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn smoke_update_cdf_counter_position() {
        // C layout: counter at cdf[nsymbs] (index 4 for nsymbs=4).
        let mut cdf = [24576u16, 16384, 8192, 0, 0];
        update_cdf(&mut cdf, 2, 4);
        assert_eq!(cdf, [24832, 16896, 7936, 0, 1]);
    }

    #[test]
    fn smoke_write_symbol_adapts() {
        let mut enc = RefEcEnc::new(256);
        let mut cdf = [16384u16, 0, 0];
        enc.write_symbol(0, &mut cdf, 2);
        assert_eq!(cdf[2], 1, "counter must advance");
        assert_ne!(cdf[0], 16384, "probability must adapt");
        let _ = enc.done();
    }
}

// ---- Default CDF table extraction (FRAME_CONTEXT) ----

macro_rules! fc_tables {
    ($(($variant:ident, $sizeof_fn:ident, $copy_fn:ident)),* $(,)?) => {
        unsafe extern "C" {
            fn ref_fc_init(base_qindex: i32);
            $(fn $sizeof_fn() -> usize;
              fn $copy_fn(dst: *mut u16);)*
        }

        /// Tables extractable from the C `FRAME_CONTEXT` after default init.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum FcTable {
            $($variant,)*
        }

        /// Copy one table out of the C context as a flat `u16` vector.
        /// Call [`fc_init`] first.
        pub fn fc_table(t: FcTable) -> Vec<u16> {
            match t {
                $(FcTable::$variant => {
                    let bytes = unsafe { $sizeof_fn() };
                    assert!(bytes % 2 == 0);
                    let mut v = vec![0u16; bytes / 2];
                    unsafe { $copy_fn(v.as_mut_ptr()) };
                    v
                })*
            }
        }
    };
}

/// Initialize the C `FRAME_CONTEXT` with the reference defaults for
/// `base_qindex` (`svt_av1_default_coef_probs` + `svt_aom_init_mode_probs`).
pub fn fc_init(base_qindex: i32) {
    unsafe { ref_fc_init(base_qindex) };
}

fc_tables! {
    (TxbSkip, ref_fc_sizeof_txb_skip_cdf, ref_fc_copy_txb_skip_cdf),
    (EobExtra, ref_fc_sizeof_eob_extra_cdf, ref_fc_copy_eob_extra_cdf),
    (DcSign, ref_fc_sizeof_dc_sign_cdf, ref_fc_copy_dc_sign_cdf),
    (EobFlag16, ref_fc_sizeof_eob_flag_cdf16, ref_fc_copy_eob_flag_cdf16),
    (EobFlag32, ref_fc_sizeof_eob_flag_cdf32, ref_fc_copy_eob_flag_cdf32),
    (EobFlag64, ref_fc_sizeof_eob_flag_cdf64, ref_fc_copy_eob_flag_cdf64),
    (EobFlag128, ref_fc_sizeof_eob_flag_cdf128, ref_fc_copy_eob_flag_cdf128),
    (EobFlag256, ref_fc_sizeof_eob_flag_cdf256, ref_fc_copy_eob_flag_cdf256),
    (EobFlag512, ref_fc_sizeof_eob_flag_cdf512, ref_fc_copy_eob_flag_cdf512),
    (EobFlag1024, ref_fc_sizeof_eob_flag_cdf1024, ref_fc_copy_eob_flag_cdf1024),
    (CoeffBaseEob, ref_fc_sizeof_coeff_base_eob_cdf, ref_fc_copy_coeff_base_eob_cdf),
    (CoeffBase, ref_fc_sizeof_coeff_base_cdf, ref_fc_copy_coeff_base_cdf),
    (CoeffBr, ref_fc_sizeof_coeff_br_cdf, ref_fc_copy_coeff_br_cdf),
    (Partition, ref_fc_sizeof_partition_cdf, ref_fc_copy_partition_cdf),
    (Skip, ref_fc_sizeof_skip_cdfs, ref_fc_copy_skip_cdfs),
    (KfY, ref_fc_sizeof_kf_y_cdf, ref_fc_copy_kf_y_cdf),
    (AngleDelta, ref_fc_sizeof_angle_delta_cdf, ref_fc_copy_angle_delta_cdf),
    (IntraExtTx, ref_fc_sizeof_intra_ext_tx_cdf, ref_fc_copy_intra_ext_tx_cdf),
    (TxSize, ref_fc_sizeof_tx_size_cdf, ref_fc_copy_tx_size_cdf),
    (UvMode, ref_fc_sizeof_uv_mode_cdf, ref_fc_copy_uv_mode_cdf),
    (FilterIntra, ref_fc_sizeof_filter_intra_cdfs, ref_fc_copy_filter_intra_cdfs),
    (FilterIntraMode, ref_fc_sizeof_filter_intra_mode_cdf, ref_fc_copy_filter_intra_mode_cdf),
    (DeltaQ, ref_fc_sizeof_delta_q_cdf, ref_fc_copy_delta_q_cdf),
    (IntraBc, ref_fc_sizeof_intrabc_cdf, ref_fc_copy_intrabc_cdf),
    (YMode, ref_fc_sizeof_y_mode_cdf, ref_fc_copy_y_mode_cdf),
    (Nmvc, ref_fc_sizeof_nmvc, ref_fc_copy_nmvc),
}

// ---- MV entropy encode oracle (AUDIT 2026-07-14) ----

unsafe extern "C" {
    fn ref_get_mv_class(z: i32, offset: *mut i32) -> i32;
    fn ref_encode_mv_seq(
        mv_y: *const i32,
        mv_x: *const i32,
        ref_y: *const i32,
        ref_x: *const i32,
        n: i32,
        precision: i32,
        out: *mut u8,
        cap: u32,
    ) -> u32;
}

// ---- Temporal-filter noise estimator oracle (AUDIT 2026-07-14) ----

unsafe extern "C" {
    fn ref_estimate_noise_fp16(src: *const u8, width: u16, height: u16, y_stride: u16) -> i32;
}

/// Reference `svt_estimate_noise_fp16_c` (temporal_filtering.c): FP16
/// noise-level estimate of a luma plane (Sobel edge rejection + Laplacian),
/// or `-65536` (-1 in fp16) if too few smooth pixels.
pub fn estimate_noise_fp16(src: &[u8], width: usize, height: usize, y_stride: usize) -> i32 {
    assert!(src.len() >= (height - 1) * y_stride + width);
    unsafe { ref_estimate_noise_fp16(src.as_ptr(), width as u16, height as u16, y_stride as u16) }
}

/// Reference `svt_av1_get_mv_class(z)`: returns `(class, offset)`.
pub fn get_mv_class(z: i32) -> (i32, i32) {
    let mut offset = 0i32;
    let c = unsafe { ref_get_mv_class(z, &mut offset) };
    (c, offset)
}

/// Reference MV-difference entropy encode of a whole sequence through one
/// adapting `NmvContext` (default CDFs), in C encode order (vertical/Y first).
/// Faithful transcription of `svt_av1_encode_mv` + `encode_mv_component`
/// driving the real `svt_av1_get_mv_class` + `aom_write_symbol`. `precision`
/// is the `MvSubpelPrecision` int (-1 none, 0 low, 1 high). Returns the
/// finalized od_ec byte stream.
pub fn encode_mv_seq(
    mvs: &[(i16, i16)],
    refs: &[(i16, i16)],
    precision: i32,
) -> Vec<u8> {
    assert_eq!(mvs.len(), refs.len());
    let n = mvs.len();
    let mv_y: Vec<i32> = mvs.iter().map(|m| m.1 as i32).collect();
    let mv_x: Vec<i32> = mvs.iter().map(|m| m.0 as i32).collect();
    let ref_y: Vec<i32> = refs.iter().map(|m| m.1 as i32).collect();
    let ref_x: Vec<i32> = refs.iter().map(|m| m.0 as i32).collect();
    let mut out = vec![0u8; 4096];
    let nbytes = unsafe {
        ref_encode_mv_seq(
            mv_y.as_ptr(),
            mv_x.as_ptr(),
            ref_y.as_ptr(),
            ref_x.as_ptr(),
            n as i32,
            precision,
            out.as_mut_ptr(),
            out.len() as u32,
        )
    };
    assert!(nbytes as usize <= out.len(), "MV seq exceeded oracle buffer");
    out.truncate(nbytes as usize);
    out
}

// ---- Scan orders + coefficient-context helpers ----

unsafe extern "C" {
    fn ref_scan_len(tx_size: i32) -> i32;
    fn ref_scan_copy(tx_size: i32, scan_class: i32, scan_out: *mut i16, len: i32);
    fn ref_tx_type_to_scan_index(tx_type: i32) -> i32;
    fn ref_get_br_ctx(levels: *const u8, c: i32, bwl: i32, tx_class: i32) -> i32;
    fn ref_get_eob_pos_token(eob: i32, extra: *mut i32) -> i32;
    fn ref_nz_map_ctx_offset(tx_size: i32, coeff_idx: i32) -> i32;
    fn ref_txb_init_levels(coeff: *const i32, width: i32, height: i32, levels: *mut u8);
    fn ref_get_nz_map_contexts(
        levels: *const u8,
        scan: *const i16,
        eob: u16,
        tx_size: i32,
        tx_class: i32,
        coeff_contexts: *mut i8,
    );
    fn ref_get_txsize_entropy_ctx(tx_size: i32) -> i32;
    fn ref_get_txb_bwl(tx_size: i32) -> i32;
    fn ref_get_txb_wide(tx_size: i32) -> i32;
    fn ref_get_txb_high(tx_size: i32) -> i32;
}

/// Number of coefficients scanned for `tx_size` (adjusted dimensions).
pub fn scan_len(tx_size: usize) -> usize {
    unsafe { ref_scan_len(tx_size as i32) as usize }
}

/// Copy the reference scan order for (tx_size, scan_class 0..3).
pub fn scan(tx_size: usize, scan_class: usize) -> Vec<i16> {
    let len = scan_len(tx_size);
    let mut v = vec![0i16; len];
    unsafe {
        ref_scan_copy(
            tx_size as i32,
            scan_class as i32,
            v.as_mut_ptr(),
            len as i32,
        )
    };
    v
}

pub fn tx_type_to_scan_index(tx_type: usize) -> usize {
    unsafe { ref_tx_type_to_scan_index(tx_type as i32) as usize }
}

pub fn get_br_ctx(levels: &[u8], c: usize, bwl: usize, tx_class: usize) -> i32 {
    unsafe { ref_get_br_ctx(levels.as_ptr(), c as i32, bwl as i32, tx_class as i32) }
}

pub fn get_eob_pos_token(eob: i32) -> (i32, i32) {
    let mut extra = 0i32;
    let t = unsafe { ref_get_eob_pos_token(eob, &mut extra) };
    (t, extra)
}

pub fn nz_map_ctx_offset(tx_size: usize, coeff_idx: usize) -> i32 {
    unsafe { ref_nz_map_ctx_offset(tx_size as i32, coeff_idx as i32) }
}

pub fn txb_init_levels(coeff: &[i32], width: usize, height: usize, levels: &mut [u8]) {
    unsafe {
        ref_txb_init_levels(
            coeff.as_ptr(),
            width as i32,
            height as i32,
            levels.as_mut_ptr(),
        )
    };
}

pub fn get_nz_map_contexts(
    levels: &[u8],
    scan: &[i16],
    eob: u16,
    tx_size: usize,
    tx_class: usize,
    coeff_contexts: &mut [i8],
) {
    unsafe {
        ref_get_nz_map_contexts(
            levels.as_ptr(),
            scan.as_ptr(),
            eob,
            tx_size as i32,
            tx_class as i32,
            coeff_contexts.as_mut_ptr(),
        )
    };
}

pub fn txsize_entropy_ctx(tx_size: usize) -> usize {
    unsafe { ref_get_txsize_entropy_ctx(tx_size as i32) as usize }
}
pub fn txb_bwl(tx_size: usize) -> usize {
    unsafe { ref_get_txb_bwl(tx_size as i32) as usize }
}
pub fn txb_wide(tx_size: usize) -> usize {
    unsafe { ref_get_txb_wide(tx_size as i32) as usize }
}
pub fn txb_high(tx_size: usize) -> usize {
    unsafe { ref_get_txb_high(tx_size as i32) as usize }
}

// ---- AV1 quantizer step tables ----

unsafe extern "C" {
    fn ref_dc_quant_qtx(qindex: i32) -> i16;
    fn ref_ac_quant_qtx(qindex: i32) -> i16;
    fn ref_dc_quant_qtx_bd(qindex: i32, bd: i32) -> i16;
    fn ref_ac_quant_qtx_bd(qindex: i32, bd: i32) -> i16;
}

/// Reference `svt_aom_dc_quant_qtx(qindex, 0, 8-bit)`.
pub fn dc_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_dc_quant_qtx(qindex) }
}

/// Reference `svt_aom_ac_quant_qtx(qindex, 0, 8-bit)`.
pub fn ac_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_ac_quant_qtx(qindex) }
}

/// Reference `svt_aom_dc_quant_qtx(qindex, 0, bd)` for `bd` in {8, 10, 12}
/// (the `EbBitDepth` values). Backs the bd10 qlookup-table FFI check.
pub fn dc_quant_qtx_bd(qindex: i32, bd: i32) -> i16 {
    unsafe { ref_dc_quant_qtx_bd(qindex, bd) }
}

/// Reference `svt_aom_ac_quant_qtx(qindex, 0, bd)` for `bd` in {8, 10, 12}.
pub fn ac_quant_qtx_bd(qindex: i32, bd: i32) -> i16 {
    unsafe { ref_ac_quant_qtx_bd(qindex, bd) }
}

// ---- variance-boost helper wrappers (rc_aq.c, exported in both modes) ----

unsafe extern "C" {
    fn svt_av1_convert_qindex_to_q_fp8(qindex: i32, bit_depth: i32) -> i32;
    fn svt_av1_compute_qdelta_fp(qstart_fp8: i32, qtarget_fp8: i32, bit_depth: i32) -> i32;
}

/// C `svt_av1_convert_qindex_to_q_fp8`. `bit_depth` is the EbBitDepth enum
/// value (8/10/12).
pub fn convert_qindex_to_q_fp8(qindex: i32, bit_depth: i32) -> i32 {
    unsafe { svt_av1_convert_qindex_to_q_fp8(qindex, bit_depth) }
}

/// C `svt_av1_compute_qdelta_fp`.
pub fn compute_qdelta_fp(qstart_fp8: i32, qtarget_fp8: i32, bit_depth: i32) -> i32 {
    unsafe { svt_av1_compute_qdelta_fp(qstart_fp8, qtarget_fp8, bit_depth) }
}

// ---- AC-bias wrappers (ac_bias.c, exported; feature code in both modes) ----

unsafe extern "C" {
    fn ref_psy_distortion(
        input: *const u8,
        input_stride: u32,
        recon: *const u8,
        recon_stride: u32,
        width: u32,
        height: u32,
    ) -> u64;
    fn svt_psy_adjust_rate_light(
        coeff: *const i32,
        coeff_bits: u64,
        width: u32,
        height: u32,
        ac_bias: f64,
    ) -> u64;
    fn get_effective_ac_bias(ac_bias: f64, is_islice: bool, temporal_layer_index: u8) -> f64;
}

/// C `svt_psy_distortion` (8-bit).
pub fn psy_distortion(input: &[u8], input_stride: u32, recon: &[u8], recon_stride: u32, width: u32, height: u32) -> u64 {
    unsafe { ref_psy_distortion(input.as_ptr(), input_stride, recon.as_ptr(), recon_stride, width, height) }
}

/// C `svt_psy_adjust_rate_light`.
pub fn psy_adjust_rate_light(coeff: &[i32], coeff_bits: u64, width: u32, height: u32, ac_bias: f64) -> u64 {
    unsafe { svt_psy_adjust_rate_light(coeff.as_ptr(), coeff_bits, width, height, ac_bias) }
}

/// C `get_effective_ac_bias`.
pub fn effective_ac_bias(ac_bias: f64, is_islice: bool, temporal_layer_index: u8) -> f64 {
    unsafe { get_effective_ac_bias(ac_bias, is_islice, temporal_layer_index) }
}

// ---- picture-analysis sub-sampled mean producers (pic_analysis_process.c) ----

unsafe extern "C" {
    fn svt_compute_sub_mean_8x8_c(input_samples: *const u8, input_stride: u16) -> u64;
    fn svt_aom_compute_sub_mean_squared_values_c(
        input_samples: *const u8,
        input_stride: u32,
        input_area_width: u32,
        input_area_height: u32,
    ) -> u64;
}

/// C `svt_compute_sub_mean_8x8_c` (fp8 sub-sampled 8x8 mean).
pub fn sub_mean_8x8(block: &[u8], stride: u16) -> u64 {
    unsafe { svt_compute_sub_mean_8x8_c(block.as_ptr(), stride) }
}

/// C `svt_aom_compute_sub_mean_squared_values_c` (fp16 sub-sampled mean of squares).
pub fn sub_mean_squared_8x8(block: &[u8], stride: u32) -> u64 {
    unsafe { svt_aom_compute_sub_mean_squared_values_c(block.as_ptr(), stride, 8, 8) }
}

unsafe extern "C" {
    fn ref_noise_normalization(
        dequant_dc: i16,
        dequant_ac: i16,
        coeff: *const i32,
        qcoeff: *mut i32,
        dqcoeff: *mut i32,
        eob: *mut u16,
        tx_size: i32,
        tx_type: i32,
        strength: u8,
    );
}

/// Fork `svt_av1_perform_noise_normalization` via a minimal-struct shim
/// (no QM). Buffers are packed rasters like the quantizer's.
#[allow(clippy::too_many_arguments)]
pub fn noise_normalization(
    dequant: [i16; 2],
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    tx_size: i32,
    tx_type: i32,
    strength: u8,
) {
    unsafe {
        ref_noise_normalization(
            dequant[0],
            dequant[1],
            coeff.as_ptr(),
            qcoeff.as_mut_ptr(),
            dqcoeff.as_mut_ptr(),
            eob,
            tx_size,
            tx_type,
            strength,
        )
    }
}

unsafe extern "C" {
    fn ref_spatial_facade(
        input: *const u8,
        input_stride: u32,
        recon: *const u8,
        recon_stride: u32,
        width: u32,
        height: u32,
        mode: u8,
        uv_mode: u8,
        is_chroma: u8,
        is_interintra: u8,
        comp_type: u8,
        temporal_layer_index: u8,
        ac_bias: f64,
        tx_bias: u8,
    ) -> u64;
}

/// Fork mds0 distortion facade (`svt_spatial_full_distortion_kernel_facade`)
/// driven with a synthetic BlockModeInfo. 8-bit, offsets 0.
#[allow(clippy::too_many_arguments)]
pub fn spatial_facade(
    input: &[u8],
    input_stride: u32,
    recon: &[u8],
    recon_stride: u32,
    width: u32,
    height: u32,
    mode: u8,
    uv_mode: u8,
    is_chroma: bool,
    is_interintra: bool,
    comp_type: u8,
    temporal_layer_index: u8,
    ac_bias: f64,
    tx_bias: u8,
) -> u64 {
    unsafe {
        ref_spatial_facade(
            input.as_ptr(),
            input_stride,
            recon.as_ptr(),
            recon_stride,
            width,
            height,
            mode,
            uv_mode,
            u8::from(is_chroma),
            u8::from(is_interintra),
            comp_type,
            temporal_layer_index,
            ac_bias,
            tx_bias,
        )
    }
}

// ---- 2D transform wrappers ----

unsafe extern "C" {
    fn ref_fwd_txfm2d(n: i32, input: *mut i16, output: *mut i32, stride: u32, tx_type: i32);
    fn ref_inv_txfm2d_add(
        n: i32,
        input: *const i32,
        output_r: *const u16,
        stride_r: i32,
        output_w: *mut u16,
        stride_w: i32,
        tx_type: i32,
    );
    fn ref_fwd_txfm2d_rect(
        w: i32,
        h: i32,
        input: *mut i16,
        output: *mut i32,
        stride: u32,
        tx_type: i32,
    );
    fn ref_inv_txfm2d_add_rect(
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
    fn ref_lpf(kind: i32, buf: *mut u8, off: i32, pitch: i32, blimit: u8, limit: u8, thresh: u8);
    fn ref_lf_limits(sharpness: i32, lim_out: *mut u8, mblim_out: *mut u8);
    fn ref_lpf_hbd(
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
    fn ref_cdef_find_dir(img: *const u16, stride: i32, var: *mut i32, coeff_shift: i32) -> u8;
    fn ref_cdef_find_dir_8bit(img: *const u8, stride: i32, var: *mut i32, coeff_shift: i32) -> u8;
    fn ref_cdef_filter_block_8(
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
    fn ref_cdef_filter_block_8bit(
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

// ---- CDEF strength picker (intra branch, C float semantics) ----

unsafe extern "C" {
    fn ref_pick_cdef_from_qp_intra_8bit(
        base_q_idx: i32,
        pred_y_strength: *mut i32,
        pred_uv_strength: *mut i32,
    );
}

/// Reference `svt_pick_cdef_from_qp` intra branch at 8-bit: returns the
/// packed `(y_strength, uv_strength)` pair for a qindex, evaluated with C
/// float semantics against the library's `svt_aom_ac_quant_qtx`.
pub fn pick_cdef_from_qp_intra_8bit(base_q_idx: u8) -> (i32, i32) {
    let (mut y, mut uv) = (0i32, 0i32);
    unsafe { ref_pick_cdef_from_qp_intra_8bit(base_q_idx as i32, &mut y, &mut uv) };
    (y, uv)
}

// ---- Loop restoration (Wiener) ----

unsafe extern "C" {
    fn ref_wiener_convolve_add_src(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        filter_x: *const i16,
        filter_y: *const i16,
        w: i32,
        h: i32,
    );
    fn ref_compute_stats(
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
    fn ref_loop_restoration_filter_unit(
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
    fn ref_extend_frame(
        data: *mut u8,
        width: i32,
        height: i32,
        stride: i32,
        border_horz: i32,
        border_vert: i32,
    );
    fn ref_write_refsubexpfin_bytes(n: u16, k: u16, r: u16, v: u16, out: *mut u8, cap: u32) -> u32;
    fn ref_count_refsubexpfin(n: u16, k: u16, r: u16, v: u16) -> i32;
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

// ---------------------------------------------------------------------------
// MD fast-loop kernels (M6 leaf funnel): filter-intra predictor, aom
// Hadamard/SATD. All global `T` symbols in the non-LTO archive.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn svt_av1_filter_intra_predictor_c(
        dst: *mut u8,
        stride: isize,
        tx_size: u32,
        above: *const u8,
        left: *const u8,
        mode: i32,
    );
    fn svt_aom_hadamard_8x8_c(src_diff: *const i16, src_stride: isize, coeff: *mut i32);
    fn svt_aom_hadamard_16x16_c(src_diff: *const i16, src_stride: isize, coeff: *mut i32);
    fn svt_aom_hadamard_32x32_c(src_diff: *const i16, src_stride: isize, coeff: *mut i32);
    fn svt_aom_satd_c(coeff: *const i32, length: i32) -> i32;
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

// ---------------------------------------------------------------------------
// Intra edge filter / upsample + upsample-capable dr prediction kernels
// (M5 leaf funnel: SH enable_intra_edge_filter=1 directional prediction).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_filter_intra_edge(p: *mut u8, sz: i32, strength: i32);
    fn svt_av1_upsample_intra_edge_c(p: *mut u8, sz: i32);
    fn svt_aom_intra_edge_filter_strength(bs0: i32, bs1: i32, delta: i32, type_: i32) -> i32;
    fn svt_aom_use_intra_edge_upsample(bs0: i32, bs1: i32, delta: i32, type_: i32) -> i32;
    fn svt_av1_dr_prediction_z1_c(
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
    fn svt_av1_dr_prediction_z2_c(
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
    fn svt_av1_dr_prediction_z3_c(
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
// AUDIT 2026-07-14: inter / motion DSP oracles (sad, variance, convolve8).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_sad(w: i32, h: i32, src: *const u8, ss: i32, r: *const u8, rs: i32) -> u32;
    fn ref_variance(
        w: i32,
        h: i32,
        a: *const u8,
        as_: i32,
        b: *const u8,
        bs: i32,
        sse: *mut u32,
    ) -> u32;
    fn ref_convolve8_horiz(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        taps: *const i16,
        w: i32,
        h: i32,
    );
    fn ref_convolve8_vert(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        taps: *const i16,
        w: i32,
        h: i32,
    );
}

/// Reference `svt_aom_sad{w}x{h}_c`: sum of abs differences over the block.
/// `src_origin`/`ref_origin` index the block top-left inside their buffers.
#[allow(clippy::too_many_arguments)]
pub fn sad(
    w: usize,
    h: usize,
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    r: &[u8],
    ref_origin: usize,
    ref_stride: usize,
) -> u32 {
    assert!(src_origin + (h - 1) * src_stride + w <= src.len());
    assert!(ref_origin + (h - 1) * ref_stride + w <= r.len());
    let out = unsafe {
        ref_sad(
            w as i32,
            h as i32,
            src.as_ptr().add(src_origin),
            src_stride as i32,
            r.as_ptr().add(ref_origin),
            ref_stride as i32,
        )
    };
    assert_ne!(out, 0xFFFF_FFFF, "cref: unsupported SAD size {w}x{h}");
    out
}

/// Reference `svt_aom_variance{w}x{h}_c`: returns `(variance, sse)` where
/// `sse = sum((a-b)^2)` and `variance = sse - sum(a-b)^2 / (w*h)` (two blocks).
#[allow(clippy::too_many_arguments)]
pub fn variance(
    w: usize,
    h: usize,
    a: &[u8],
    a_origin: usize,
    a_stride: usize,
    b: &[u8],
    b_origin: usize,
    b_stride: usize,
) -> (u32, u32) {
    assert!(a_origin + (h - 1) * a_stride + w <= a.len());
    assert!(b_origin + (h - 1) * b_stride + w <= b.len());
    let mut sse = 0u32;
    let var = unsafe {
        ref_variance(
            w as i32,
            h as i32,
            a.as_ptr().add(a_origin),
            a_stride as i32,
            b.as_ptr().add(b_origin),
            b_stride as i32,
            &mut sse,
        )
    };
    assert_ne!(sse, 0xFFFF_FFFF, "cref: unsupported variance size {w}x{h}");
    (var, sse)
}

/// Reference `svt_aom_convolve8_horiz_c` with `x_step_q4=16` and the given
/// 8 taps. `src_origin` points at the C-convention origin (the kernel reads
/// `src[origin-3 ..= origin+w+3]` per row — 3 left / 4 right of the window).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_horiz(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    taps: &[i16; 8],
    w: usize,
    h: usize,
) {
    assert!(src_origin >= 3);
    assert!(src_origin + (h - 1) * src_stride + w + 4 <= src.len());
    assert!((h - 1) * dst_stride + w <= dst.len());
    unsafe {
        ref_convolve8_horiz(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            taps.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

/// Reference `svt_aom_convolve8_vert_c` with `y_step_q4=16` and the given
/// 8 taps. `src_origin` points at the C-convention origin (the kernel reads
/// rows `origin-3 ..= origin+h+3` — 3 above / 4 below the window).
#[allow(clippy::too_many_arguments)]
pub fn convolve8_vert(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    taps: &[i16; 8],
    w: usize,
    h: usize,
) {
    assert!(src_origin >= 3 * src_stride);
    assert!(src_origin + (h + 3) * src_stride + w <= src.len());
    assert!((h - 1) * dst_stride + w <= dst.len());
    unsafe {
        ref_convolve8_vert(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            taps.as_ptr(),
            w as i32,
            h as i32,
        );
    }
}

unsafe extern "C" {
    fn ref_obmc_mask(length: i32, out: *mut u8);
    fn ref_obmc_blend_above(
        dst: *mut u8,
        dst_stride: i32,
        above: *const u8,
        above_stride: i32,
        w: i32,
        overlap: i32,
    );
    fn ref_obmc_blend_left(
        dst: *mut u8,
        dst_stride: i32,
        left: *const u8,
        left_stride: i32,
        overlap: i32,
        h: i32,
    );
}

/// Reference `svt_av1_get_obmc_mask(length)` (length in {1,2,4,8,16,32}).
pub fn obmc_mask(length: usize) -> Vec<u8> {
    let mut out = vec![0u8; length];
    unsafe { ref_obmc_mask(length as i32, out.as_mut_ptr()) };
    out
}

/// Reference OBMC "above" blend: `svt_aom_blend_a64_vmask_c(dst, dst, above,
/// obmc_mask(overlap))` over the top `overlap` rows × `w` cols — the exact
/// `build_obmc_inter_pred_above` reconstruction blend (dst = current pred).
pub fn obmc_blend_above(
    dst: &mut [u8],
    dst_stride: usize,
    above: &[u8],
    above_stride: usize,
    w: usize,
    overlap: usize,
) {
    assert!((overlap - 1) * dst_stride + w <= dst.len());
    assert!((overlap - 1) * above_stride + w <= above.len());
    unsafe {
        ref_obmc_blend_above(
            dst.as_mut_ptr(),
            dst_stride as i32,
            above.as_ptr(),
            above_stride as i32,
            w as i32,
            overlap as i32,
        );
    }
}

/// Reference OBMC "left" blend: `svt_aom_blend_a64_hmask_c(dst, dst, left,
/// obmc_mask(overlap))` over `h` rows × the left `overlap` cols — the exact
/// `build_obmc_inter_pred_left` reconstruction blend (dst = current pred).
pub fn obmc_blend_left(
    dst: &mut [u8],
    dst_stride: usize,
    left: &[u8],
    left_stride: usize,
    overlap: usize,
    h: usize,
) {
    assert!((h - 1) * dst_stride + overlap <= dst.len());
    assert!((h - 1) * left_stride + overlap <= left.len());
    unsafe {
        ref_obmc_blend_left(
            dst.as_mut_ptr(),
            dst_stride as i32,
            left.as_ptr(),
            left_stride as i32,
            overlap as i32,
            h as i32,
        );
    }
}

// ---------------------------------------------------------------------------
// AUDIT 2026-07-14: oracles for the dormant inter/scaling stubs
// (warp.rs / scale.rs / superres.rs are NOT ports of these — see the
// c_parity_{warp,scale,superres} suites which pin the divergence).
// ---------------------------------------------------------------------------

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn ref_warp_affine(
        mat: *const i32,
        r: *const u8,
        width: i32,
        height: i32,
        stride: i32,
        pred: *mut u8,
        p_col: i32,
        p_row: i32,
        p_width: i32,
        p_height: i32,
        p_stride: i32,
        alpha: i16,
        beta: i16,
        gamma: i16,
        delta: i16,
    );
    #[allow(clippy::too_many_arguments)]
    fn ref_convolve_2d_scale(
        src: *const u8,
        src_stride: i32,
        dst: *mut u8,
        dst_stride: i32,
        w: i32,
        h: i32,
        subpel_x_qn: i32,
        x_step_qn: i32,
        subpel_y_qn: i32,
        y_step_qn: i32,
    );
    fn ref_superres_filter_normative(phase: i32, out8: *mut i16);
    fn ref_superres_upscale_row(input: *const u8, in_width: i32, output: *mut u8, out_width: i32);
}

/// Reference `svt_av1_warp_affine_c` (non-compound, 8-bit). `mat` is the 6-entry
/// Q16 affine model; `pred` is `p_height` rows × `p_stride`. See warped_motion.c.
#[allow(clippy::too_many_arguments)]
pub fn warp_affine(
    mat: &[i32; 6],
    r: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    pred: &mut [u8],
    p_col: i32,
    p_row: i32,
    p_width: usize,
    p_height: usize,
    p_stride: usize,
    shear: (i16, i16, i16, i16),
) {
    assert!(r.len() >= height * stride);
    assert!(pred.len() >= (p_height - 1) * p_stride + p_width);
    unsafe {
        ref_warp_affine(
            mat.as_ptr(),
            r.as_ptr(),
            width as i32,
            height as i32,
            stride as i32,
            pred.as_mut_ptr(),
            p_col,
            p_row,
            p_width as i32,
            p_height as i32,
            p_stride as i32,
            shear.0,
            shear.1,
            shear.2,
            shear.3,
        );
    }
}

/// Reference `svt_av1_convolve_2d_scale_c` (non-compound 8-bit, EIGHTTAP_REGULAR
/// both axes). Phases are in the `SCALE_SUBPEL_BITS = 10` domain. `src` must be
/// pre-offset so the kernel's fo_horiz/fo_vert (3) taps stay in bounds.
#[allow(clippy::too_many_arguments)]
pub fn convolve_2d_scale(
    src: &[u8],
    src_origin: usize,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    w: usize,
    h: usize,
    subpel_x_qn: i32,
    x_step_qn: i32,
    subpel_y_qn: i32,
    y_step_qn: i32,
) {
    unsafe {
        ref_convolve_2d_scale(
            src.as_ptr().add(src_origin),
            src_stride as i32,
            dst.as_mut_ptr(),
            dst_stride as i32,
            w as i32,
            h as i32,
            subpel_x_qn,
            x_step_qn,
            subpel_y_qn,
            y_step_qn,
        );
    }
}

/// Copy one 8-tap phase (0..=63) of the C normative resize filter
/// `svt_av1_resize_filter_normative`.
pub fn superres_filter_normative(phase: usize) -> [i16; 8] {
    let mut out = [0i16; 8];
    unsafe { ref_superres_filter_normative(phase as i32, out.as_mut_ptr()) };
    out
}

/// Reference one-row normative horizontal upscale (`upscale_normative_rect` ->
/// `av1_convolve_horiz_rs_c`). `input` indexes the first pixel of a row that has
/// >= 5 border bytes on each side (the rect replicates/restores them in place).
pub fn superres_upscale_row(
    input: &mut [u8],
    input_origin: usize,
    in_width: usize,
    output: &mut [u8],
    out_width: usize,
) {
    assert!(input_origin >= 5 && input_origin + in_width + 5 <= input.len());
    assert!(output.len() >= out_width);
    unsafe {
        ref_superres_upscale_row(
            input.as_ptr().add(input_origin),
            in_width as i32,
            output.as_mut_ptr(),
            out_width as i32,
        );
    }
}

/// Reference `svt_av1_upsample_intra_edge_c` with the block edge at
/// `p[origin..origin+sz]` (writes `p[origin-2..]`).
pub fn upsample_intra_edge(p: &mut [u8], origin: usize, sz: usize) {
    unsafe { svt_av1_upsample_intra_edge_c(p.as_mut_ptr().add(origin), sz as i32) }
}

/// Reference `svt_aom_intra_edge_filter_strength`.
pub fn intra_edge_filter_strength(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> i32 {
    unsafe { svt_aom_intra_edge_filter_strength(bs0, bs1, delta, filt_type) }
}

/// Reference `svt_aom_use_intra_edge_upsample`.
pub fn use_intra_edge_upsample(bs0: i32, bs1: i32, delta: i32, filt_type: i32) -> bool {
    unsafe { svt_aom_use_intra_edge_upsample(bs0, bs1, delta, filt_type) != 0 }
}

/// Reference dr predictor over origin-based edged buffers
/// (`above[origin+i]` = C `above_row[i]`), dispatching to the C
/// z1/z2/z3 kernels exactly like C `svt_aom_dr_predictor`.
#[allow(clippy::too_many_arguments)]
pub fn dr_predictor_edged(
    dst: &mut [u8],
    stride: usize,
    above: &[u8],
    left: &[u8],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    bw: usize,
    bh: usize,
    angle: i32,
) {
    // C eb_dr_intra_derivative lookups (get_dx/get_dy, intra_prediction.c).
    const DR: [u16; 90] = [
        0, 0, 0, 1023, 0, 0, 547, 0, 0, 372, 0, 0, 0, 0, 273, 0, 0, 215, 0, 0, 178, 0, 0, 151, 0,
        0, 132, 0, 0, 116, 0, 0, 102, 0, 0, 0, 90, 0, 0, 80, 0, 0, 71, 0, 0, 64, 0, 0, 57, 0, 0,
        51, 0, 0, 45, 0, 0, 0, 40, 0, 0, 35, 0, 0, 31, 0, 0, 27, 0, 0, 23, 0, 0, 19, 0, 0, 15, 0,
        0, 0, 0, 11, 0, 0, 7, 0, 0, 3, 0, 0,
    ];
    let dx = if angle > 0 && angle < 90 {
        DR[angle as usize] as i32
    } else if angle > 90 && angle < 180 {
        DR[(180 - angle) as usize] as i32
    } else {
        1
    };
    let dy = if angle > 90 && angle < 180 {
        DR[(angle - 90) as usize] as i32
    } else if angle > 180 && angle < 270 {
        DR[(270 - angle) as usize] as i32
    } else {
        1
    };
    unsafe {
        let a = above.as_ptr().add(origin);
        let l = left.as_ptr().add(origin);
        if angle > 0 && angle < 90 {
            svt_av1_dr_prediction_z1_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_above as i32,
                dx,
                dy,
            );
        } else if angle > 90 && angle < 180 {
            svt_av1_dr_prediction_z2_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_above as i32,
                upsample_left as i32,
                dx,
                dy,
            );
        } else if angle > 180 && angle < 270 {
            svt_av1_dr_prediction_z3_c(
                dst.as_mut_ptr(),
                stride as isize,
                bw as i32,
                bh as i32,
                a,
                l,
                upsample_left as i32,
                dx,
                dy,
            );
        } else {
            panic!("dr_predictor_edged: exact 90/180 handled by V/H paths");
        }
    }
}

// ---------------------------------------------------------------------------
// Quantizers (full_loop.c) — `svt_av1_quantize_fp_facade` / `svt_aom_quantize_b`
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn ref_quantize_fp(
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
    fn ref_spatial_full_distortion_ssim(
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
    fn ref_generate_noise_table(
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
    fn ref_quantize_b_qm(
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
    fn ref_quantize_fp_qm(
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
    fn ref_quantize_b(
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
fn build_iscan(scan: &[u16]) -> Vec<i16> {
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
// Screen-content detection primitives (pic_analysis_process.c) — the leaf
// functions of the AA-aware detector (#71). All exported `T` symbols.
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn svt_av1_count_colors_with_threshold(
        src: *const u8,
        stride: i32,
        rows: i32,
        cols: i32,
        num_colors_threshold: i32,
        num_colors: *mut i32,
    ) -> bool;
    fn svt_av1_find_dominant_value(src: *const u8, stride: i32, rows: i32, cols: i32) -> u8;
    fn svt_av1_dilate_block(
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
    fn svt_av1_count_colors(src: *const u8, stride: i32, rows: i32, cols: i32, val_count: *mut i32) -> i32;
    fn svt_av1_index_color_cache(
        color_cache: *const u16,
        n_cache: i32,
        colors: *const u16,
        n_colors: i32,
        cache_color_found: *mut u8,
        out_cache_colors: *mut i32,
    ) -> i32;
    fn svt_av1_k_means_dim1_c(data: *const i32, centroids: *mut i32, indices: *mut u8, n: i32, k: i32, max_itr: i32);
    fn svt_av1_calc_indices_dim1_c(data: *const i32, centroids: *const i32, indices: *mut u8, n: i32, k: i32);
}

/// Reference `svt_av1_count_colors` (pic_analysis_process.c:892). Writes
/// the 256-bin histogram into `val_count` and returns the distinct-color
/// count.
pub fn count_colors(src: &[u8], stride: usize, rows: usize, cols: usize, val_count: &mut [i32; 256]) -> i32 {
    assert!(src.len() >= (rows - 1) * stride + cols);
    unsafe { svt_av1_count_colors(src.as_ptr(), stride as i32, rows as i32, cols as i32, val_count.as_mut_ptr()) }
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
pub fn k_means_dim1(data: &[i32], centroids: &mut [i32], indices: &mut [u8], k: usize, max_itr: i32) {
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
    unsafe { svt_av1_calc_indices_dim1_c(data.as_ptr(), centroids.as_ptr(), indices.as_mut_ptr(), n as i32, k as i32) }
}

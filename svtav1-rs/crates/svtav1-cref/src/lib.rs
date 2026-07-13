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
}

/// Reference `svt_aom_dc_quant_qtx(qindex, 0, 8-bit)`.
pub fn dc_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_dc_quant_qtx(qindex) }
}

/// Reference `svt_aom_ac_quant_qtx(qindex, 0, 8-bit)`.
pub fn ac_quant_qtx(qindex: i32) -> i16 {
    unsafe { ref_ac_quant_qtx(qindex) }
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
    let dir =
        unsafe { ref_cdef_find_dir_8bit(img.as_ptr(), stride as i32, &mut var, coeff_shift) };
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

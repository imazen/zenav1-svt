//! Test-only FFI harness over the in-tree C SVT-AV1 static library.
//!
//! This crate exists solely for differential parity testing: every Rust
//! module ports a C module, and the tests here drive the *actual* C
//! implementation on identical inputs to assert bit-for-bit equality.
//!
//! This is the single sanctioned `unsafe` exception in the workspace — it is
//! `publish = false`, used only as a dev-dependency, and never part of a
//! shipped artifact.

pub mod cdef_search;
pub mod dlf;
pub mod enc_dec_metrics;
pub mod entropy_block;
pub mod entropy_inter;
pub mod frame_cdf;
pub mod inter_me;
pub mod inter_mvp;
pub mod inter_pred;
pub mod interpred_gap;
pub mod inv_recon;
pub mod md_subpel;
pub mod mode_decision;
pub mod pcl;
pub mod pic_operators;
pub mod picstruct;
pub mod preanalysis;
pub mod rate_control;
pub mod rc_vbr_cbr;
pub mod rd_cost;
pub mod ref_mgmt;
pub mod sig_deriv;
pub mod temporal_filtering;
pub mod txfm_pf;

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
        assert!(symb < nsymbs && cdf.len() > nsymbs);
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
    assert!(cdf.len() > nsymbs && val < nsymbs);
    unsafe { ref_update_cdf(cdf.as_mut_ptr(), val as i8, nsymbs as i32) };
}

#[cfg(test)]
mod tests;

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
    (PaletteYMode, ref_fc_sizeof_palette_y_mode_cdf, ref_fc_copy_palette_y_mode_cdf),
    (PaletteUvMode, ref_fc_sizeof_palette_uv_mode_cdf, ref_fc_copy_palette_uv_mode_cdf),
    (PaletteYSize, ref_fc_sizeof_palette_y_size_cdf, ref_fc_copy_palette_y_size_cdf),
    (PaletteYColorIndex, ref_fc_sizeof_palette_y_color_index_cdf, ref_fc_copy_palette_y_color_index_cdf),
    (Nmvc, ref_fc_sizeof_nmvc, ref_fc_copy_nmvc),
    (Ndvc, ref_fc_sizeof_ndvc, ref_fc_copy_ndvc),
    (InterExtTx, ref_fc_sizeof_inter_ext_tx_cdf, ref_fc_copy_inter_ext_tx_cdf),
    (TxfmPartition, ref_fc_sizeof_txfm_partition_cdf, ref_fc_copy_txfm_partition_cdf),
    (SegTree, ref_fc_sizeof_seg_tree_cdf, ref_fc_copy_seg_tree_cdf),
    (SegPred, ref_fc_sizeof_seg_pred_cdf, ref_fc_copy_seg_pred_cdf),
    (SegSpatialPred, ref_fc_sizeof_spatial_pred_seg_cdf, ref_fc_copy_spatial_pred_seg_cdf),
}

pub mod film_grain;

mod wp_filters;
pub use wp_filters::*;

mod mv_entropy;
pub use mv_entropy::*;

mod intrabc;
pub use intrabc::*;

mod screen_intra;
pub use screen_intra::*;

mod quant;
pub use quant::*;

mod inter_dsp;
pub use inter_dsp::*;

mod md_kernels;
pub use md_kernels::*;

mod lr_wiener;
pub use lr_wiener::*;

mod dsp_kernels;
pub use dsp_kernels::*;

mod rc_seg_oracles;
pub use rc_seg_oracles::*;

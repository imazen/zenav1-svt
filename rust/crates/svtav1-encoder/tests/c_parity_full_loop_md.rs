//! Differential parity: the MD-side decisions of `full_loop.c`
//! (`svtav1-encoder/src/port_full_loop_md.rs`).
//!
//! **Evidence tier 1** for `svt_aom_do_md_recon` (full_loop.c:2739): the
//! oracle is the REAL exported symbol (`nm -g Bin/Release/libSvtAv1Enc.a`
//! prints `T`), driven over ALL 2^13 assignments of the thirteen booleans it
//! reads — an exhaustive sweep, not a sample, because the function is pure
//! boolean logic over a small domain and exhaustion is cheaper than choosing
//! a representative subset.
//!
//! The other three functions in that module (`shave_coeff`,
//! `ec_shave_est_zero_rate_save`, `skip_chroma_rate_est`) are C `static`
//! with no exported symbol; their tests live beside them and are labelled
//! **tier 4** there, with the reason.

use svtav1_cref::rd_cost as cref;
use svtav1_encoder::port_full_loop_md::{MdReconInputs, do_md_recon};

#[test]
fn do_md_recon_matches_c_exhaustively() {
    assert_eq!(cref::recon_fields(), cref::RECON_FIELDS);
    for mask in 0u32..(1 << cref::RECON_FIELDS) {
        let b = |k: u32| (mask >> k) & 1 == 1;
        let fields: [i32; cref::RECON_FIELDS] = core::array::from_fn(|k| i32::from(b(k as u32)));
        let c = cref::do_md_recon(&fields);
        let p = do_md_recon(&MdReconInputs {
            bypass_encdec: b(0),
            pd_pass_1: b(1),
            skip_intra: b(2),
            inter_intra_enabled: b(3),
            is_ref: b(4),
            recon_enabled: b(5),
            dlf_enabled: b(6),
            cdef_enabled: b(7),
            cdef_use_qp_strength: b(8),
            cdef_use_reference_fs: b(9),
            enable_restoration: b(10),
            compute_psnr: b(11),
            compute_ssim: b(12),
        });
        assert_eq!(c, p, "mask={mask:#x}");
    }
}

//! Default CDF tables extracted from the C reference (libSvtAv1Enc.a,
//! SVT-AV1 v4.1.0) via `svt_av1_default_coef_probs` and
//! `svt_aom_init_mode_probs`. Exact C `FRAME_CONTEXT` layout: ICDF
//! values, structural 0 at `[nsymbs-1]`, adaptation counter slot at
//! `[nsymbs]`.
//!
//! GENERATED FILES — DO NOT EDIT. Regenerate with:
//!   cargo run --release -p zenav1-svt-cref --bin gen_default_cdfs -- \
//!     crates/svtav1-encoder/src/entropy
//! The c_default_cdfs_match test asserts these stay in sync with C.

use crate::entropy::cdf::AomCdfProb;

mod coeff;
mod coeff_base;
mod mode;
pub use coeff::*;
pub use coeff_base::*;
pub use mode::*;

/// Number of coefficient-CDF quality buckets (C `TOKEN_CDF_Q_CTXS`).
pub const TOKEN_CDF_Q_CTXS: usize = 4;

/// C `get_q_ctx`: map base_qindex to the coefficient-CDF bucket.
#[inline]
pub fn coef_q_ctx(base_qindex: u8) -> usize {
    match base_qindex {
        0..=20 => 0,
        21..=60 => 1,
        61..=120 => 2,
        _ => 3,
    }
}

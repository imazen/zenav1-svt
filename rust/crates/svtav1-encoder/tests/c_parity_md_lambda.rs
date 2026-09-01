//! Differential parity: the intra tx-type helpers of `mode_decision.c`
//! (`svtav1-encoder/src/port_md_lambda.rs`).
//!
//! **Evidence tier 1**: the oracles are the REAL exported
//! `svt_aom_filter_intra_allowed_bsize` (mode_decision.c:102) and
//! `svt_aom_get_intra_uv_tx_type` (:2950) — `nm -g` prints `T` for both —
//! and both sweeps are EXHAUSTIVE over the function's whole input domain
//! rather than sampled, because that domain is small enough to enumerate.
//!
//! The uv sweep also drives the `static` `intra_mode_to_tx_type` (:2940) on
//! its `PLANE_TYPE_UV` arm, which is the only arm `get_intra_uv_tx_type`
//! reaches.
//!
//! The two lambda tuners in that module are tier 4 and say so; they need
//! `pa_me_data`'s scaling-factor arrays and the superres/TPL geometry built
//! in a shim before the arithmetic under test is reached.

use svtav1_cref::rd_cost as cref;
use svtav1_encoder::port_md_lambda::{filter_intra_allowed_bsize, get_intra_uv_tx_type};
use svtav1_types::block::BlockSize;
use svtav1_types::transform::TxSize;

#[test]
fn filter_intra_allowed_bsize_matches_c_for_every_block_size() {
    for b in BlockSize::ALL {
        let c = cref::filter_intra_allowed_bsize(b.as_index() as i32);
        let p = filter_intra_allowed_bsize(b);
        assert_eq!(c, p, "{b:?}");
    }
}

#[test]
fn intra_uv_tx_type_matches_c_over_its_whole_domain() {
    // 14 UvPredictionModes x 19 TxSizes x both reduced_tx_set values.
    for uv_mode in 0..14i32 {
        for tx in 0..TxSize::SIZES_ALL {
            for reduced in [false, true] {
                let c = cref::get_intra_uv_tx_type(uv_mode, tx as i32, reduced);
                let p = get_intra_uv_tx_type(uv_mode as u8, tx, reduced) as i32;
                assert_eq!(c, p, "uv_mode={uv_mode} tx={tx} reduced={reduced}");
            }
        }
    }
}

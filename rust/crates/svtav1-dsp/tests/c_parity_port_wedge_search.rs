//! Differential parity for the masked-compound search.
//!
//! `pick_wedge_fixed_sign` (enc_inter_prediction.c:489) is EXPORTED and is
//! gated here at evidence TIER 1 on its `use_rd_model = 0` arm: with that flag
//! clear it reads nothing off the `ModeDecisionContext` beyond the flag itself
//! and never touches the `PictureControlSet`, so a zeroed calloc'd context is
//! a complete stand-in (`shims/inter_pred_shims.c`).
//!
//! NOT GATED HERE, and named rather than implied:
//! * `pick_wedge_fixed_sign`'s `use_rd_model = 1` arm, which adds
//!   `md_rate_est_ctx->wedge_idx_fac_bits[bsize][wedge_index]` to the rate and
//!   needs a real `PictureControlSet` for `model_rd_with_curvfit`.
//! * `pick_wedge`, `pick_interinter_wedge`, `pick_interinter_seg` and
//!   `pick_interinter_mask` — all `static`, all reaching the PCS/ctx. Their
//!   control flow is TIER 4 (traced against the C source, in-module), but every
//!   quantity they compare comes from a primitive that IS tier-1 gated in this
//!   port: subtract_block, sum_squares_i16, wedge_compute_delta_squares,
//!   wedge_sign_from_residuals, wedge_sse_from_residuals,
//!   get_contiguous_soft_mask, build_compound_diffwtd_mask and
//!   model_rd_from_* / RDCOST.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_wedge_masks::WedgeMasks;
use svtav1_dsp::port_wedge_search::{SearchCtx, pick_wedge_fixed_sign};
use svtav1_types::block::BlockSize;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// Residuals spanning the full int16 range, so the in-loop clamp inside
/// `svt_av1_wedge_sse_from_residuals` is live in the search too.
fn residuals(n: usize, seed: u32) -> Vec<i16> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 7 {
                0 => i16::MIN,
                1 => i16::MAX,
                2 => 0,
                _ => (v >> 8) as i16,
            }
        })
        .collect()
}

/// The nine wedge-capable block sizes.
const BSIZES: [usize; 9] = [3, 4, 5, 6, 7, 8, 9, 18, 19];

const BLOCK_W: [usize; 22] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; 22] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

#[test]
fn pick_wedge_fixed_sign_matches_c() {
    let wedge = WedgeMasks::new();
    let ctx = SearchCtx {
        hbd: false,
        full_lambda: 0,
        // The bound arm: no rate model, so no PCS and no rate table.
        use_rate: false,
        quantizer: 0,
    };
    let mut cells = 0usize;
    let mut indices = [false; 16];
    for &bsize in &BSIZES {
        let n = BLOCK_W[bsize] * BLOCK_H[bsize];
        for sign in 0..2usize {
            for seed in [0x1234u32, 0xBEEF, 0x0F0F, 0x9E37] {
                let residual1 = residuals(n, seed ^ bsize as u32);
                let diff10 = residuals(n, seed.wrapping_mul(7) ^ bsize as u32);
                let (rd, idx) = pick_wedge_fixed_sign(
                    &wedge,
                    &ctx,
                    BlockSize::from_u8(bsize as u8).unwrap(),
                    &residual1,
                    &diff10,
                    sign,
                    &[0i32; 16],
                );
                let (c_rd, c_idx) =
                    cref::pick_wedge_fixed_sign(bsize as i32, &residual1, &diff10, sign as i32);
                assert_eq!(
                    (rd, idx),
                    (c_rd, c_idx),
                    "pick_wedge_fixed_sign bsize {bsize} sign {sign} seed {seed:x}"
                );
                assert!((0..16).contains(&idx), "no wedge index chosen");
                indices[idx as usize] = true;
                cells += 1;
            }
        }
    }
    assert!(cells >= 72, "anti-vacuity: only {cells} cells ran");
    // The search must actually range over the codebook, not pin one index.
    let distinct = indices.iter().filter(|&&b| b).count();
    assert!(
        distinct >= 4,
        "only {distinct} distinct wedge indices were ever chosen"
    );
}

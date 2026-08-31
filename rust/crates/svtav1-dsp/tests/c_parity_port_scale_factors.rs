//! Differential parity for the reference-scale factors and the compound
//! distance weights — evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven: `svt_av1_setup_scale_factors_for_frame`,
//! `svt_av1_dist_wtd_comp_weight_assign`, `svt_aom_get_relative_dist_enc`
//! (all `nm -g`-visible), plus the header inline `av1_is_scaled` through a
//! shim. `get_fixed_point_scale_factor`,
//! `fixed_point_scale_to_coarse_point_scale`, `valid_ref_frame_size` and
//! `av1_is_valid_scale` are `static` / `static INLINE` with no exported
//! symbol, so they are gated INDIRECTLY: the exported
//! `svt_av1_setup_scale_factors_for_frame` is their only caller and its four
//! outputs are exactly their composed results, which pins each one.
//!
//! `has_scale`, `scaled_x`, `scaled_y`, `unscaled_value` and
//! `revert_scale_extra_bits` are `static` with no caller that exports their
//! result, so they carry tier-4 evidence only (hand-derived vectors traced
//! against the C source) — see the unit tests in the module itself. `has_scale`
//! is additionally pinned here through `av1_is_scaled`, which decides the same
//! identity-vs-scaled question one level up.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_scale_factors::{
    DistWtdWeights, ScaleFactors, dist_wtd_comp_weight_assign, get_relative_dist_enc,
};

/// Frame sizes spanning the valid-ratio boundary (2x larger / 16x smaller) and
/// well past it in both directions, plus odd and non-power-of-two dimensions.
const SIZES: [i32; 14] = [
    16, 33, 63, 64, 65, 96, 127, 128, 256, 320, 640, 1024, 1920, 4096,
];

#[test]
fn setup_scale_factors_matches_c() {
    let mut cells = 0usize;
    let mut valid = 0usize;
    let mut invalid = 0usize;
    let mut scaled = 0usize;
    for &ow in &SIZES {
        for &oh in &SIZES {
            for &tw in &SIZES {
                for &th in &SIZES {
                    // The shim seeds x_step_q4/y_step_q4 with -1 so the
                    // untouched-on-early-return case is observable.
                    let rust = ScaleFactors::setup_for_frame_with_sentinel(ow, oh, tw, th, -1);
                    let c = cref::setup_scale_factors_for_frame(ow, oh, tw, th);
                    assert_eq!(
                        (
                            rust.x_scale_fp,
                            rust.y_scale_fp,
                            rust.x_step_q4,
                            rust.y_step_q4
                        ),
                        c,
                        "setup_scale_factors ref {ow}x{oh} cur {tw}x{th}"
                    );
                    assert_eq!(
                        rust.is_scaled(),
                        cref::av1_is_scaled(ow, oh, tw, th),
                        "av1_is_scaled ref {ow}x{oh} cur {tw}x{th}"
                    );
                    if rust.is_valid_scale() {
                        valid += 1;
                        if rust.is_scaled() {
                            scaled += 1;
                        }
                    } else {
                        invalid += 1;
                    }
                    cells += 1;
                }
            }
        }
    }
    // Anti-vacuity: every arm of the function must actually be reached.
    assert_eq!(cells, SIZES.len().pow(4));
    assert!(valid > 100, "only {valid} valid-size cells");
    assert!(invalid > 100, "only {invalid} invalid-size cells");
    assert!(scaled > 100, "only {scaled} scaled cells");
}

#[test]
fn get_relative_dist_enc_matches_c() {
    let mut cells = 0usize;
    let mut nonzero = 0usize;
    for enable in [false, true] {
        for bits in 1..=8i32 {
            for a in [0, 1, 2, 7, 15, 16, 31, 32, 63, 64, 127, 128, 200, 255] {
                for b in [0, 1, 3, 8, 16, 33, 64, 100, 129, 255] {
                    let rust = get_relative_dist_enc(enable, bits, a, b);
                    let c = cref::get_relative_dist_enc(enable, bits, a, b);
                    assert_eq!(
                        rust, c,
                        "relative_dist enable {enable} bits {bits} ({a},{b})"
                    );
                    if rust != 0 {
                        nonzero += 1;
                    }
                    cells += 1;
                }
            }
        }
    }
    assert!(
        cells > 1000 && nonzero > 100,
        "{cells} cells, {nonzero} nonzero"
    );
}

/// The full `svt_av1_dist_wtd_comp_weight_assign` decision surface.
///
/// The C early return writes only `*use_dist_wtd_comp_avg`, so the Rust port
/// is handed the same sentinel `prev` the shim seeds (-1 / -1) and the two
/// must agree on leaving them alone.
#[test]
fn dist_wtd_comp_weight_assign_matches_c() {
    let prev = DistWtdWeights {
        fwd_offset: -1,
        bck_offset: -1,
        use_dist_wtd_comp_avg: -1,
    };
    let mut cells = 0usize;
    let mut weighted = 0usize;
    let mut early = 0usize;
    let mut zero_dist = 0usize;
    for enable in [false, true] {
        for bits in [3i32, 5, 7] {
            for cur in [0i32, 4, 8, 16, 31] {
                for bck in [0i32, 2, 6, 12, 24, 30] {
                    for fwd in [0i32, 1, 5, 10, 20, 31] {
                        for compound_idx in [0i32, 1] {
                            for order_idx in [0usize, 1] {
                                for is_compound in [false, true] {
                                    let rust = dist_wtd_comp_weight_assign(
                                        enable,
                                        bits,
                                        cur,
                                        bck,
                                        fwd,
                                        compound_idx,
                                        order_idx,
                                        is_compound,
                                        prev,
                                    );
                                    let c = cref::dist_wtd_comp_weight_assign(
                                        enable,
                                        bits,
                                        cur,
                                        bck,
                                        fwd,
                                        compound_idx,
                                        order_idx as i32,
                                        is_compound,
                                    );
                                    assert_eq!(
                                        (
                                            rust.fwd_offset,
                                            rust.bck_offset,
                                            rust.use_dist_wtd_comp_avg
                                        ),
                                        c,
                                        "dist_wtd enable {enable} bits {bits} cur {cur} bck {bck} \
                                         fwd {fwd} cidx {compound_idx} oidx {order_idx} comp {is_compound}"
                                    );
                                    if rust.use_dist_wtd_comp_avg == 0 {
                                        early += 1;
                                    } else {
                                        weighted += 1;
                                        // d0 == 0 || d1 == 0 lands on table row 3.
                                        if rust.fwd_offset == 13
                                            || rust.fwd_offset == 3
                                            || rust.bck_offset == 13
                                            || rust.bck_offset == 3
                                        {
                                            zero_dist += 1;
                                        }
                                    }
                                    cells += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(cells > 2000, "only {cells} cells");
    assert!(early > 100, "the early-return arm ran only {early} times");
    assert!(weighted > 100, "the weighted arm ran only {weighted} times");
    assert!(zero_dist > 10, "the row-3 arm ran only {zero_dist} times");
}

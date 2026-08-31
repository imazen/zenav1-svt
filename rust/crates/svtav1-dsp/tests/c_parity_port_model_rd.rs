//! Differential parity for the fast RD models — evidence tier 1
//! (`WORKING-ON-THIS.md` §4) for the exported half.
//!
//! Symbols driven: `svt_av1_model_rd_from_var_lapndz` and `model_rd_from_sse`
//! (both `nm -g`-visible), plus the header inlines `svt_log2f_safe` and
//! `get_msb` through shims.
//!
//! `model_rd_norm` is `static` with no exported symbol, and is gated
//! INDIRECTLY but completely: `svt_av1_model_rd_from_var_lapndz` is its only
//! caller and passes both of its outputs straight through the two closed-form
//! expressions below, so a difference in the table interpolation shows up in
//! every nonzero-variance cell here.
//!
//! `av1_model_rd_curvfit`, `model_rd_with_curvfit` and
//! `sse_norm_curvfit_model_cat_lookup` are `static` and their only exported
//! caller (`model_rd_for_sb_with_curvfit`) takes a `PictureControlSet*` and a
//! `ModeDecisionContext*`, which a shim cannot synthesise without building most
//! of the encoder. They therefore carry TIER 4 evidence only — hand-derived
//! vectors traced against the C source, in the module's own unit tests — and
//! that is stated rather than implied.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_model_rd::{get_msb, log2f_safe, model_rd_from_sse, model_rd_from_var_lapndz};
use svtav1_types::block::BlockSize;

#[test]
fn log2f_safe_and_get_msb_match_c() {
    for x in [
        0u32,
        1,
        2,
        3,
        4,
        7,
        8,
        15,
        16,
        255,
        256,
        1023,
        1024,
        65535,
        65536,
        u32::MAX,
    ] {
        assert_eq!(log2f_safe(x), cref::log2f_safe(x), "svt_log2f_safe({x})");
        if x != 0 {
            assert_eq!(get_msb(x), cref::get_msb(x), "get_msb({x})");
        }
    }
    // Every power of two and its neighbours.
    for b in 0..32u32 {
        let v = 1u32 << b;
        assert_eq!(log2f_safe(v), cref::log2f_safe(v));
        assert_eq!(
            log2f_safe(v.wrapping_sub(1)),
            cref::log2f_safe(v.wrapping_sub(1))
        );
    }
}

/// Sweeps `var` across seven orders of magnitude, every block-size log2 the
/// encoder uses, and the qstep range a real quantizer produces — so the
/// `MAX_XSQ_Q10` clamp, the `var == 0` short circuit and the table
/// interpolation are all reached.
#[test]
fn model_rd_from_var_lapndz_matches_c() {
    let mut cells = 0usize;
    let mut clamped = 0usize;
    let mut zeroed = 0usize;
    for var in [
        0i64,
        1,
        2,
        7,
        15,
        63,
        255,
        1_023,
        4_095,
        16_383,
        65_535,
        262_143,
        1_048_575,
        16_777_215,
        268_435_455,
    ] {
        for n_log2 in 4u32..=14 {
            for qstep in [1u32, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377] {
                let got = model_rd_from_var_lapndz(var, n_log2, qstep);
                let want = cref::model_rd_from_var_lapndz(var, n_log2, qstep);
                assert_eq!(got, want, "lapndz var {var} n_log2 {n_log2} qstep {qstep}");
                if var == 0 {
                    zeroed += 1;
                } else {
                    // The clamp fires when the raw xsq exceeds MAX_XSQ_Q10.
                    let raw =
                        (((qstep as u64) * (qstep as u64)) << (n_log2 + 10)) / var.max(1) as u64;
                    if raw > 245_727 {
                        clamped += 1;
                    }
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 2000, "anti-vacuity: only {cells} cells ran");
    assert!(zeroed > 100, "the var == 0 arm ran only {zeroed} times");
    assert!(
        clamped > 100,
        "the MAX_XSQ_Q10 clamp fired only {clamped} times"
    );
}

/// Both arms of `model_rd_from_sse`: the fast approximation (which mutates
/// `quantizer` in place and branches on 120) and the Laplacian one.
#[test]
fn model_rd_from_sse_matches_c() {
    let mut cells = 0usize;
    let mut fast_rate_zero = 0usize;
    let mut fast_rate_nonzero = 0usize;
    for bsize in BlockSize::ALL {
        for bit_depth in [8i32, 10] {
            for quantizer in [4i16, 32, 128, 512, 2048, 8192, 32000] {
                for sse in [0u64, 1, 100, 10_000, 1_000_000, 100_000_000] {
                    for simple in [false, true] {
                        let got = model_rd_from_sse(bsize, quantizer, bit_depth as u8, sse, simple);
                        let want = cref::model_rd_from_sse(
                            bsize as i32,
                            quantizer as i32,
                            bit_depth,
                            sse,
                            simple,
                        );
                        assert_eq!(
                            got, want,
                            "model_rd_from_sse bsize {bsize:?} bd {bit_depth} q {quantizer} sse {sse} simple {simple}"
                        );
                        if simple {
                            if got.0 == 0 {
                                fast_rate_zero += 1;
                            } else {
                                fast_rate_nonzero += 1;
                            }
                        }
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 3000, "anti-vacuity: only {cells} cells ran");
    assert!(
        fast_rate_zero > 50 && fast_rate_nonzero > 50,
        "both sides of the `quantizer < 120` branch must run: {fast_rate_zero}/{fast_rate_nonzero}"
    );
}

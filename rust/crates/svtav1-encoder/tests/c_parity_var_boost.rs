//! C-parity for the variance-boost helpers (rc_aq.c) — the exported
//! functions exist in BOTH SVT_HDR_MODE libs, so this runs under the
//! standard differential setup regardless of which lib is linked.
//!
//! Swept at BOTH bit depths. `svt_av1_convert_qindex_to_q_fp8` and
//! `svt_av1_compute_qdelta_fp` (rc_aq.c:24-61) are the only two bit-depth
//! entry points in the whole variance-boost chain, and they change BOTH the
//! qlookup table and the shift per depth (8-bit `ac_quant_qtx(q,0,bd) << 6`,
//! 10-bit `<< 4`). The port hardcoded the 8-bit form, which skewed every boost
//! at bd10 and was one of the two roots of the fork-mode bd10 divergence — a
//! bd8-only sweep could not see it.
use svtav1_cref as cref;
use svtav1_encoder::var_boost;

/// bd12 is out of scope for this port (docs/bd10-port-map.md) — the Rust side
/// panics on it deliberately, so it is not swept here.
const DEPTHS: [u8; 2] = [8, 10];

#[test]
fn convert_qindex_to_q_fp8_matches_c_all_qindex() {
    for bd in DEPTHS {
        for q in 0..=255i32 {
            assert_eq!(
                var_boost::convert_qindex_to_q_fp8(q, bd),
                cref::convert_qindex_to_q_fp8(q, bd as i32),
                "bd{bd} qindex {q}"
            );
        }
    }
}

/// The two depths must not be accidentally equivalent — otherwise the sweep
/// above would pass with the bit-depth argument ignored, which is exactly the
/// bug it exists to catch.
#[test]
fn convert_qindex_to_q_fp8_actually_depends_on_bit_depth() {
    let differs = (0..=255i32)
        .any(|q| var_boost::convert_qindex_to_q_fp8(q, 8) != var_boost::convert_qindex_to_q_fp8(q, 10));
    assert!(differs, "bd8 and bd10 fp8 Q curves are identical — bit depth is being ignored");
}

#[test]
fn compute_qdelta_fp_matches_c_grid() {
    // Sweep start/target fp8 values derived from real qindexes plus
    // synthetic off-table points (the C scan clamps at MAXQ).
    for bd in DEPTHS {
        let mut points: Vec<i32> = (0..=255)
            .step_by(5)
            .map(|q| var_boost::convert_qindex_to_q_fp8(q, bd))
            .collect();
        points.extend([1, 100, 1000, 250_000, 1_000_000]);
        for &a in &points {
            for &b in &points {
                assert_eq!(
                    var_boost::compute_qdelta_fp(a, b, bd),
                    cref::compute_qdelta_fp(a, b, bd as i32),
                    "bd{bd} start {a} target {b}"
                );
            }
        }
    }
}

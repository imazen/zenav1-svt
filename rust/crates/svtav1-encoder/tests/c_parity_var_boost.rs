//! C-parity for the variance-boost helpers (rc_aq.c) — the exported
//! functions exist in BOTH SVT_HDR_MODE libs, so this runs under the
//! standard differential setup regardless of which lib is linked.
use svtav1_cref as cref;
use svtav1_encoder::var_boost;

#[test]
fn convert_qindex_to_q_fp8_matches_c_all_qindex() {
    for q in 0..=255i32 {
        assert_eq!(
            var_boost::convert_qindex_to_q_fp8(q),
            cref::convert_qindex_to_q_fp8(q, 8),
            "qindex {q}"
        );
    }
}

#[test]
fn compute_qdelta_fp_matches_c_grid() {
    // Sweep start/target fp8 values derived from real qindexes plus
    // synthetic off-table points (the C scan clamps at MAXQ).
    let mut points: Vec<i32> = (0..=255).step_by(5).map(var_boost::convert_qindex_to_q_fp8).collect();
    points.extend([1, 100, 1000, 250_000, 1_000_000]);
    for &a in &points {
        for &b in &points {
            assert_eq!(
                var_boost::compute_qdelta_fp(a, b),
                cref::compute_qdelta_fp(a, b, 8),
                "start {a} target {b}"
            );
        }
    }
}

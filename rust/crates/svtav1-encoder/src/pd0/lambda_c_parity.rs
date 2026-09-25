use svtav1_cref as cref;

/// `SVT_AV1_KF_UPDATE` (definitions.h) — the KEY-frame update type.
const KF_UPDATE: i32 = 0;

/// `update_lambda`'s frame-type scale for KF at each bit depth:
/// `rd_frame_type_factor[bit_depth != EB_EIGHT_BIT][KF_UPDATE]`
/// (rc_process.c:395-396) = 150 at bd8, 128 at bd10.
fn c_full_lambda_unweighted(bit_depth: u8, qindex: u8) -> u32 {
    let base = cref::compute_rd_mult_based_on_qindex(bit_depth, KF_UPDATE, qindex) as i64;
    let ftf: i64 = if bit_depth == 8 { 150 } else { 128 };
    ((base * ftf) >> 7) as u32
}

/// The CDEF search's lambda: `svt_aom_lambda_assign(.., enhanced_pic->
/// bit_depth, base_q_idx, multiply_lambda = false)` (enc_cdef.c:958).
#[test]
fn cdef_search_lambda_matches_c_at_every_qindex() {
    for q in 0..=255u16 {
        let q = q as u8;
        assert_eq!(
            super::kf_full_lambda_8bit_unweighted(q),
            c_full_lambda_unweighted(8, q),
            "bd8 CDEF lambda at qindex {q}"
        );
        assert_eq!(
            super::kf_full_lambda_bd10_unweighted(q),
            c_full_lambda_unweighted(10, q),
            "bd10 CDEF lambda at qindex {q}"
        );
    }
    // Non-vacuity: the two depths must genuinely differ (a bd10 arm that
    // silently returned the bd8 value would pass a same-value compare).
    let differ = (0..=255u16)
        .filter(|&q| {
            super::kf_full_lambda_bd10_unweighted(q as u8)
                != super::kf_full_lambda_8bit_unweighted(q as u8)
        })
        .count();
    assert!(
        differ > 200,
        "bd8/bd10 lambdas differ at only {differ} qindexes"
    );
}

/// The LR search's `x->rdmult` = `pic_full_lambda[EB_{8,10}_BIT_MD]`
/// (enc_dec_process.c:3246), i.e. `multiply_lambda = true` — which only
/// scales the 10-bit arm (`*= 16`, rc_process.c:479).
#[test]
fn lr_search_rdmult_matches_c_at_every_qindex() {
    for q in 0..=255u16 {
        let q = q as u8;
        // bd8: multiply_lambda is a no-op, so it equals the unweighted.
        assert_eq!(
            super::kf_full_lambda_8bit_unweighted(q),
            c_full_lambda_unweighted(8, q),
            "bd8 LR rdmult at qindex {q}"
        );
        assert_eq!(
            super::kf_full_lambda_bd10_pic(q),
            c_full_lambda_unweighted(10, q) * 16,
            "bd10 LR rdmult at qindex {q}"
        );
    }
}

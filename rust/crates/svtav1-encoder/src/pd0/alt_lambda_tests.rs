#[test]
fn alt_factor_changes_lambda() {
    let base = super::kf_full_lambda_8bit_ex(160, 40, false, 0);
    let alt = super::kf_full_lambda_8bit_ex(160, 40, true, 0);
    assert!(alt < base, "{alt} vs {base}");
    assert_eq!(base, super::kf_full_lambda_8bit(160, 40));
    let qd = super::kf_full_lambda_8bit_ex(160, 40, false, 9);
    assert!(qd > base);
}

use super::*;

#[test]
fn research_omits_normal_weight_but_preserves_iq_and_extended_crf() {
    for qp in 0..=63 {
        for bump in [0, 28, 784] {
            assert_eq!(frame_lambda_weight_for_preset(-1, qp, false, bump), bump);
            assert_eq!(
                frame_lambda_weight_for_preset(-1, qp, true, bump),
                crate::tune::iq_lambda_weight(qp) + bump
            );
            for preset in 0..=13 {
                assert_eq!(
                    frame_lambda_weight_for_preset(preset, qp, false, bump),
                    frame_lambda_weight(qp, false, bump)
                );
            }
        }
    }
}

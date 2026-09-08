//! Signed transport checks; these do not establish research-mode bit parity.
use svtav1_encoder::speed_config::{NativePreset, SpeedConfig};

#[test]
fn native_validator_retains_research_and_rejects_named_dead_modes() {
    for value in i8::MIN..=i8::MAX {
        assert_eq!(
            NativePreset::new(value).is_some(),
            (-1..=13).contains(&value)
        );
    }
    assert_eq!(NativePreset::RESEARCH.value(), -1);
    let cfg = SpeedConfig::from_native_preset(NativePreset::RESEARCH);
    assert_eq!(cfg.preset, -1);
    assert_eq!(cfg.max_partition_depth, 4);
    assert_eq!(SpeedConfig::from_preset(255).preset, 13);
}

#[test]
fn research_sgr_uses_both_full_refined_search_lanes() {
    use svtav1_encoder::{port_enc_mode_config::leaf, port_lr_level};
    let level = leaf::get_sg_filter_level_allintra(NativePreset::RESEARCH.value());
    let controls = port_lr_level::set_sg_filter_ctrls(level);
    assert!(controls.enabled && controls.use_chroma);
    assert_eq!(controls.start_ep, [0, 0]);
    assert_eq!(controls.end_ep, [16, 16]);
    assert_eq!(controls.ep_inc, [1, 1]);
    assert_eq!(controls.refine, [true, true]);
    assert_eq!(leaf::get_sg_filter_level_allintra(0), 0);
    assert_eq!(port_lr_level::sg_filter_level_default(-1, 1, false), 1);
    assert_eq!(port_lr_level::sg_filter_level_default(0, 1, false), 3);
    assert_eq!(port_lr_level::sg_filter_level_default(-1, 6, false), 0);
}

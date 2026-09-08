use svtav1::avif::{
    AvifEncoder, Effort, EncodingPolicy, NativePreset, SvtReference, ZenEnhancement,
};

#[test]
fn effort_is_checked_and_reports_native_buckets() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
        assert!(Effort::new(bad).is_err());
    }
    assert_eq!(Effort::new(-0.0).unwrap().value().to_bits(), 0);
    let mut previous = 9;
    for step in 0..=100 {
        let e = Effort::new(step as f32 / 100.0).unwrap();
        let plan = AvifEncoder::new()
            .with_effort(e)
            .resolve_still_policy()
            .unwrap();
        assert!(plan.native_preset.value() <= previous);
        previous = plan.native_preset.value();
        assert_eq!(plan.requested_effort, Some(e));
    }
    assert_eq!(previous, -1);
}

#[test]
fn parity_refuses_conflicting_reference_extensions_and_mono_in_real_encoder() {
    let encoder = AvifEncoder::new().with_policy(EncodingPolicy::svt_parity());
    assert_eq!(encoder.reference(), SvtReference::Mainline420);
    let y = [128u8; 64];
    let uv = [128u8; 16];
    for bad in [
        encoder.clone().with_reference(SvtReference::Hybrid3115),
        encoder
            .clone()
            .with_native_preset(NativePreset::RESEARCH)
            .with_enhancement(ZenEnhancement::AomIntraEdgeFilter),
    ] {
        assert!(bad.resolve_still_policy().is_err());
        assert!(bad.encode_yuv420(&y, &uv, &uv, 8, 8, 8).is_err());
    }
    assert!(encoder.encode_y8(&y, 8, 8, 8).is_err());
}

#[test]
fn effort_resolution_reaches_the_actual_encoder_and_legacy_setters_replace_it() {
    let y: Vec<u8> = (0..1024).map(|i| ((i * 17 + i / 32) % 256) as u8).collect();
    let uv = [128u8; 256];
    for value in [0.0, 1.0] {
        let e = Effort::new(value).unwrap();
        let automatic = AvifEncoder::new()
            .with_policy(EncodingPolicy::svt_parity())
            .with_effort(e);
        let native = AvifEncoder::new()
            .with_reference(SvtReference::Mainline420)
            .with_native_preset(e.native_preset());
        assert_eq!(
            automatic
                .encode_yuv420(&y, &uv, &uv, 32, 32, 32)
                .unwrap()
                .data,
            native.encode_yuv420(&y, &uv, &uv, 32, 32, 32).unwrap().data
        );
        assert_eq!(
            automatic
                .clone()
                .with_speed(10)
                .resolved_native_preset()
                .value(),
            9
        );
        assert_eq!(
            automatic
                .with_native_preset(NativePreset::new(4).unwrap())
                .resolved_native_preset()
                .value(),
            4
        );
    }
}

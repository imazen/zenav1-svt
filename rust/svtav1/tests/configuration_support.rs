use svtav1::avif::{AvifEncoder, ChromaSubsampling};

#[test]
fn support_preserves_native_mono_and_lossless_extensions() {
    for depth in [8, 10] {
        for speed in [1, 6, 10] {
            AvifEncoder::new()
                .with_bit_depth(depth)
                .with_speed(speed)
                .with_lossless(true)
                .validate_configuration()
                .expect("native depth and lossless remain supported");
        }
    }
}

#[test]
fn query_and_encoder_reject_unimplemented_formats() {
    let y = vec![100; 8 * 8];
    for config in [
        AvifEncoder::new().with_bit_depth(12),
        AvifEncoder::new().with_chroma_subsampling(ChromaSubsampling::Yuv444),
        AvifEncoder::new().with_quality(f32::NAN),
    ] {
        let query = config.validate_configuration().unwrap_err().to_string();
        let actual = config.encode_y8(&y, 8, 8, 8).unwrap_err().to_string();
        assert_eq!(query, actual);
    }
}

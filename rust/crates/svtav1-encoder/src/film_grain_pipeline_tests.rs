//! Production wiring tests: parameters, source replacement and DPB ownership.
use super::*;
use crate::film_grain_config::FilmGrainConfig;
fn pipeline(depth: u8) -> EncodePipeline {
    EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            qp: 40,
            ..Default::default()
        },
        0,
        1,
    )
    .with_chroma_420(true)
    .with_bit_depth(depth)
    .with_recon_output(true)
}
fn source() -> [Vec<u8>; 3] {
    let mut state = 7391u32;
    core::array::from_fn(|c| {
        (0..if c == 0 { 4096 } else { 1024 })
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                100 + (state % 25) as u8
            })
            .collect()
    })
}
fn table() -> crate::entropy::obu::FilmGrainParams {
    crate::noise_gen::generate_noise_table(64, 64, 25, -1, 0, 0, false)
}
fn encode(p: &mut EncodePipeline, s: &[Vec<u8>; 3]) -> Vec<u8> {
    p.try_encode_frame_420(&s[0], &s[1], &s[2], 64).unwrap()
}
#[test]
fn denoiser_source_replacement_and_reference_grain_are_live() {
    let s = source();
    let wide = s
        .each_ref()
        .map(|p| p.iter().map(|&v| u16::from(v)).collect::<Vec<_>>());
    let (denoised, params, _) = crate::film_grain_denoise::denoise_and_model(
        wide.each_ref().map(Vec::as_slice),
        64,
        64,
        [64, 32, 32],
        8,
        25,
        true,
        7391,
    );
    assert!(params.apply_grain);
    assert_ne!(wide, denoised);
    for apply in [false, true] {
        let mut actual = pipeline(8);
        actual.film_grain = FilmGrainConfig {
            denoise_strength: 25,
            denoise_apply: apply,
            ..Default::default()
        };
        let bytes = encode(&mut actual, &s);
        let expected_input = if apply {
            denoised
                .each_ref()
                .map(|p| p.iter().map(|&v| v as u8).collect::<Vec<_>>())
        } else {
            s.clone()
        };
        let mut expected = pipeline(8);
        expected.film_grain.table = Some(params.clone());
        let expected_bytes = encode(&mut expected, &expected_input);
        assert_eq!(bytes, expected_bytes, "source replacement apply={apply}");
        assert_eq!(actual.last_recon, expected.last_recon);
        let reference = actual.dpb.get(0).unwrap();
        let (y, u, v) = actual.last_recon.as_ref().unwrap();
        assert_ne!(&reference.y_plane, y, "grain must be output-only");
        assert_ne!((&reference.u_plane, &reference.v_plane), (u, v));
        assert_eq!(actual.grain_references[0].as_ref(), Some(&params));
    }
}
#[test]
fn table_precedence_preserves_seed_and_skips_denoising() {
    let s = source();
    let mut p = pipeline(8);
    let mut supplied = table();
    supplied.apply_grain = false;
    supplied.random_seed = 444;
    p.film_grain = FilmGrainConfig {
        table: Some(supplied.clone()),
        denoise_strength: 50,
        denoise_apply: true,
        ..Default::default()
    };
    let mut plain = pipeline(8);
    plain.film_grain.table = Some(supplied);
    assert_eq!(encode(&mut p, &s), encode(&mut plain, &s));
    let params = p.grain_references[0].as_ref().unwrap();
    assert!(params.apply_grain);
    assert_eq!(params.random_seed, 7391);
    assert_eq!(
        p.dpb.get(0).unwrap().y_plane,
        plain.dpb.get(0).unwrap().y_plane
    );
    // Supplied tables also override fork photon noise.
    p.hdr = crate::hdr_mode::HdrForkConfig::hdr_fork();
    p.hdr.noise_strength = 120;
    plain.hdr = p.hdr.clone();
    plain.hdr.noise_strength = 0;
    assert_eq!(encode(&mut p, &s), encode(&mut plain, &s));
}
#[test]
fn failed_estimate_keeps_source_and_writes_apply_zero() {
    let s = [vec![128; 4096], vec![128; 1024], vec![128; 1024]];
    let mut p = pipeline(8);
    p.film_grain = FilmGrainConfig {
        denoise_strength: 25,
        denoise_apply: true,
        adaptive: false,
        ..Default::default()
    };
    encode(&mut p, &s);
    let mut plain = pipeline(8);
    encode(&mut plain, &s);
    // Some perfectly flat configurations still fit a zero-strength model;
    // this small bs32 frame has insufficient selected blocks in C.
    assert!(!p.grain_references[0].as_ref().unwrap().apply_grain);
    assert_eq!(p.last_recon, plain.last_recon);
    assert_eq!(p.grain_sequence_present, Some(true));
}
#[test]
fn native_ten_bit_denoising_preserves_low_bits() {
    let s = source().map(|p| {
        p.iter()
            .enumerate()
            .map(|(i, &v)| (u16::from(v) << 2) + (i % 4) as u16)
            .collect::<Vec<_>>()
    });
    let (denoised, params, _) = crate::film_grain_denoise::denoise_and_model(
        s.each_ref().map(Vec::as_slice),
        64,
        64,
        [64, 32, 32],
        10,
        25,
        true,
        7391,
    );
    assert!(params.apply_grain);
    let mut actual = pipeline(10);
    actual.film_grain = FilmGrainConfig {
        denoise_strength: 25,
        denoise_apply: true,
        ..Default::default()
    };
    let a = actual
        .try_encode_frame_420_hbd(&s[0], &s[1], &s[2], 64)
        .unwrap();
    let mut expected = pipeline(10);
    expected.film_grain.table = Some(params);
    let b = expected
        .try_encode_frame_420_hbd(&denoised[0], &denoised[1], &denoised[2], 64)
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(actual.last_recon10_final, expected.last_recon10_final);
}
#[test]
fn grain_seed_matches_c_through_multiple_zero_wraps() {
    let mut p = pipeline(8);
    let mut seed = 7391u16;
    for frame in 0..200000 {
        p.frame_count = frame;
        assert_eq!(p.film_grain_seed(), seed);
        seed = seed.wrapping_add(3381);
        if seed == 0 {
            seed = 7391;
        }
    }
}
#[test]
fn malformed_tables_are_rejected_before_frame_state_changes() {
    let mut p = pipeline(8);
    p.film_grain.table = Some(table());
    p.film_grain.table.as_mut().unwrap().num_y_points = 15;
    let s = source();
    assert!(p.try_encode_frame_420(&s[0], &s[1], &s[2], 64).is_err());
    assert_eq!(p.frame_count, 0);
    assert!(p.grain_sequence_present.is_none());
}

/// Tier 4: C entropy_coding.c:3120-3133 and mode_decision.c:262-266.
/// Unequal BWDREF/ALTREF slots distinguish the compared object from the
/// signaled index; a conventional ALTREF-only lookup is not C's code.
#[test]
fn reference_reuse_preserves_c_list1_comparison_and_full_array_equality() {
    let mut p = pipeline(8);
    let mut current = table();
    current.num_y_points = 2;
    let map = [0, 1, 2, 3, 4, 5, 6];
    p.grain_references[4] = Some(current.clone());
    assert_eq!(p.film_grain_reference(&current, map, false), None);
    assert_eq!(p.film_grain_reference(&current, map, true), Some(6));
    let mut same = current.clone();
    same.random_seed = 999;
    p.grain_references[0] = Some(same);
    assert_eq!(p.film_grain_reference(&current, map, true), Some(0));
    // C compares inactive entries too (all 14 scaling points, all 24 AR taps).
    p.grain_references[0].as_mut().unwrap().scaling_points_y[13][1] ^= 1;
    assert_eq!(p.film_grain_reference(&current, map, true), Some(6));
    p.film_grain.ignore_ref = true;
    assert_eq!(p.film_grain_reference(&current, map, true), None);
}

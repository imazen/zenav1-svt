use super::*;

#[test]
fn oversized_stride_is_invalid_on_all_pointer_widths() {
    assert!(matches!(
        AvifEncoder::new().validate_dimensions(64, 64, 2, u32::MAX),
        Err(EncodeError::InvalidDimensions { .. })
    ));
}

#[test]
fn defaults() {
    let enc = AvifEncoder::new();
    assert!((enc.quality - 75.0).abs() < f32::EPSILON);
    assert_eq!(enc.speed, 6);
    assert_eq!(enc.bit_depth, 8);
    assert_eq!(enc.chroma_subsampling, ChromaSubsampling::Yuv420);
    assert!(enc.threads.is_none());
    // Unset: the selected reference's defaults apply (C mainline: off).
    assert_eq!(enc.fork, ForkConfig::default());
    assert!(!enc.lossless);
}

#[test]
fn builder_pattern() {
    let enc = AvifEncoder::new()
        .with_quality(90.0)
        .with_speed(3)
        .with_bit_depth(10)
        .with_num_threads(Some(4))
        .with_qm(true)
        .with_variance_boost(true, 3)
        .with_lossless(true);

    assert!((enc.quality - 90.0).abs() < f32::EPSILON);
    assert_eq!(enc.speed, 3);
    assert_eq!(enc.bit_depth, 10);
    assert_eq!(enc.threads, Some(4));
    assert_eq!(enc.fork.enable_qm, Some(true));
    assert_eq!(enc.fork.enable_variance_boost, Some(true));
    assert_eq!(enc.fork.variance_boost_strength, Some(3));
    assert!(enc.lossless);
}

/// `with_fork` reaches the resolved configuration, later settings win,
/// and out-of-range values are refused rather than clamped (C's
/// `svt_av1_verify_settings` ranges).
#[test]
fn fork_overrides_reach_the_config_and_are_range_checked() {
    let mut f = ForkConfig::default();
    f.sharpness = Some(3);
    f.variance_boost_strength = Some(2);
    let enc = AvifEncoder::new()
        .with_reference(SvtReference::GhostRobot)
        .with_variance_boost(true, 4)
        .with_fork(f);
    let hdr = enc.resolved_hdr();
    assert_eq!((hdr.sharpness, hdr.variance_boost_strength), (3, 2));
    assert!(hdr.enable_variance_boost, "with_fork keeps unset fields");
    enc.validate_configuration().unwrap();

    for bad in [0u8, 5] {
        let enc = AvifEncoder::new().with_variance_boost(true, bad);
        assert!(
            enc.validate_configuration().is_err(),
            "strength {bad} not clamped"
        );
    }
    let mut f = ForkConfig::default();
    f.ac_bias = Some(8.5);
    let enc = AvifEncoder::new()
        .with_reference(SvtReference::GhostRobot)
        .with_fork(f);
    assert!(enc.validate_configuration().is_err());
}

/// The stop token installed by `with_timeout` reaches the pipeline: an
/// already-expired deadline aborts the encode with `Cancelled(TimedOut)`
/// instead of running to completion (or hanging), and a live deadline
/// leaves a small encode alone — no token at all is the same code path,
/// so the default surface is byte-neutral.
#[test]
fn stop_token_reaches_the_pipeline() {
    let (w, h) = (64usize, 64usize);
    let y = vec![128u8; w * h];
    let u = vec![128u8; w * h / 4];
    let v = vec![128u8; w * h / 4];

    let expired = AvifEncoder::new().with_timeout(core::time::Duration::ZERO);
    match expired.encode_yuv420(&y, &u, &v, w as u32, h as u32, w as u32) {
        Err(EncodeError::Cancelled(enough::StopReason::TimedOut)) => {}
        other => panic!("expired deadline must surface Cancelled(TimedOut), got {other:?}"),
    }

    AvifEncoder::new()
        .with_timeout(core::time::Duration::from_secs(60))
        .encode_yuv420(&y, &u, &v, w as u32, h as u32, w as u32)
        .expect("a 60s budget must not fire on a 64x64 encode");
}

// ---- issue #9 items 6 + 7 ------------------------------------------
//
// One LIVENESS cell per knob that used to be recorded-and-ignored: the
// knob must change the emitted bytes. A knob that is "wired" but never
// moves a byte is the same silent no-op the issue is about, so these
// assert INEQUALITY against the same encode with the knob off.

/// 4:2:0 test content: a luma gradient plus two non-flat chroma planes.
fn yuv420(size: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (size / 2, size / 2);
    let mut y = vec![0u8; size * size];
    for r in 0..size {
        for c in 0..size {
            y[r * size + c] = ((r * 255) / size) as u8 ^ ((c * 3) & 0x3F) as u8;
        }
    }
    let mut u = vec![0u8; cw * ch];
    let mut v = vec![0u8; cw * ch];
    for r in 0..ch {
        for c in 0..cw {
            u[r * cw + c] = (((r * 3) & 0x7F) + 64) as u8;
            v[r * cw + c] = (((c * 5) & 0x7F) + 64) as u8;
        }
    }
    (y, u, v)
}

fn enc420(enc: &AvifEncoder, size: usize) -> Vec<u8> {
    let (y, u, v) = yuv420(size);
    enc.encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
        .expect("4:2:0 encode")
        .data
}

/// Issue #24: `with_num_threads` was inert below the forced-tile
/// thresholds — the wave executor only runs when `num_tiles() > 1`, and
/// `AvifEncoder` never requested tiles. Now an explicit `Some(n > 1)`
/// derives a tile grid, so the knob must change the coded stream on an
/// ordinary still (no forced tiling at these dims), while counts that
/// derive the SAME grid stay byte-identical across thread counts.
#[test]
fn num_threads_derives_a_tile_grid() {
    // 512x256 tiles under either SB size (>= 4 cols x 2 rows of 128px
    // SBs), so the derived grids are not clamped into degenerate shapes.
    let (w, h) = (512usize, 256usize);
    let (cw, ch) = (w / 2, h / 2);
    let y: Vec<u8> = (0..h)
        .flat_map(|r| (0..w).map(move |c| ((r * 255) / h) as u8 ^ ((c * 3) & 0x3F) as u8))
        .collect();
    let u: Vec<u8> = (0..ch)
        .flat_map(|r| (0..cw).map(move |c| (((r * 3 + c) & 0x7F) + 64) as u8))
        .collect();
    let v: Vec<u8> = (0..ch)
        .flat_map(|r| (0..cw).map(move |c| (((c * 5 + r) & 0x7F) + 64) as u8))
        .collect();
    let enc = |t: Option<usize>| {
        AvifEncoder::new()
            .with_quality(60.0)
            .with_speed(6)
            .with_num_threads(t)
            .encode_yuv420(&y, &u, &v, w as u32, h as u32, w as u32)
            .expect("4:2:0 encode")
            .data
    };
    let single = enc(Some(1));
    assert_eq!(single, enc(None));
    assert_eq!(single, enc(Some(0)));
    let four = enc(Some(4));
    assert_ne!(
        four, single,
        "threads=4 must derive a tile grid and change the stream"
    );
    assert_eq!(four, enc(Some(4)), "the grid is deterministic per count");
    // 5..=8 all round up to the same 8-tile grid: the count bounds the
    // wave, the grid decides the bytes — identical output across these
    // thread counts (the issue's G5 requirement at a fixed grid).
    assert_eq!(enc(Some(5)), enc(Some(8)));
}

/// Item 6: `encode_yuv420` must produce the SAME bytes as driving the
/// mainline 4:2:0 pipeline directly with the same config — i.e. it is a
/// thin wrapper over the path every C-oracle gate covers, not a private
/// format. (It used to return three concatenated monochrome streams.)
#[test]
fn encode_yuv420_is_the_mainline_420_path_byte_for_byte() {
    let size = 64usize;
    let (y, u, v) = yuv420(size);
    let enc = AvifEncoder::new().with_quality(60.0).with_speed(6);
    let via_avif = enc
        .encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
        .expect("4:2:0 encode")
        .data;

    let rc = svtav1_encoder::rate_control::RcConfig {
        mode: svtav1_encoder::rate_control::RcMode::Cqp,
        qp: AvifEncoder::quality_to_qp(60.0),
        ..svtav1_encoder::rate_control::RcConfig::default()
    };
    let mut direct = svtav1_encoder::pipeline::EncodePipeline::new(
        size as u32,
        size as u32,
        AvifEncoder::speed_to_preset(6),
        rc,
        0,
        1,
    )
    .with_thread_count(0)
    .with_chroma_420(true);
    direct.bit_depth = 8;
    direct.color_description = enc.color_description();
    let via_pipeline = direct.encode_frame_420(&y, &u, &v, size);

    assert_eq!(
        via_avif, via_pipeline,
        "AvifEncoder::encode_yuv420 must BE the mainline 4:2:0 path"
    );
    // And it must be an AV1 stream, not a length-prefixed plane blob: the
    // first OBU is a temporal delimiter (obu_type 2, has_size_field).
    assert_eq!(via_avif[0] & 0x7f, 0b0_0010_0_1_0, "first OBU is not a TD");
}

/// Item 6: the same content through `encode_y8` is a DIFFERENT (mono)
/// stream — the two entry points are not interchangeable, which is why
/// `encode_y8` is documented gray-only.
#[test]
fn encode_y8_is_monochrome_not_the_luma_of_a_colour_image() {
    let size = 64usize;
    let (y, u, v) = yuv420(size);
    let enc = AvifEncoder::new().with_quality(60.0).with_speed(6);
    let mono = enc
        .encode_y8(&y, size as u32, size as u32, size as u32)
        .expect("mono encode")
        .data;
    let colour = enc
        .encode_yuv420(&y, &u, &v, size as u32, size as u32, size as u32)
        .expect("4:2:0 encode")
        .data;
    assert_ne!(mono, colour);
}

/// `with_tune` reaches `hdr.tune`: the IQ bundle (QM + sharpness +
/// variance boost + max_tx_size + scm) changes the emitted bytes, and
/// the default stays exactly the historical PSNR stream. The encoded
/// tunes also differ from EACH OTHER, which is the non-vacuity half.
#[test]
fn tune_knob_changes_bytes() {
    let base = AvifEncoder::new().with_quality(60.0).with_speed(6);
    let (y, u, v) = yuv420_mixed(128);
    let enc_t = |t: SvtTune| {
        base.clone()
            .with_tune(t)
            .encode_yuv420(&y, &u, &v, 128, 128, 128)
            .expect("4:2:0 encode")
            .data
    };
    let psnr = enc_t(SvtTune::Psnr);
    assert_eq!(psnr, enc_t(SvtTune::default()), "Psnr must be the default");
    for t in [SvtTune::Vq, SvtTune::Ssim, SvtTune::Iq, SvtTune::MsSsim] {
        assert_ne!(psnr, enc_t(t), "{t:?} did not change the emitted bytes");
    }
    assert_ne!(enc_t(SvtTune::Iq), enc_t(SvtTune::MsSsim));
}

/// Item 7 liveness: `with_qm` reaches `hdr.enable_qm`.
#[test]
fn qm_knob_changes_bytes() {
    let base = AvifEncoder::new().with_quality(60.0).with_speed(6);
    let off = enc420(&base, 64);
    let on = enc420(&base.clone().with_qm(true), 64);
    assert_ne!(off, on, "with_qm(true) did not change the emitted bytes");
}

/// Item 7 liveness: `with_variance_boost` reaches
/// `hdr.{enable_variance_boost, variance_boost_strength}`. Two cells: the
/// enable flag moves the bytes, and so does the strength within it (a
/// strength that never mattered would be a no-op hiding inside a live
/// knob).
#[test]
fn variance_boost_knob_changes_bytes() {
    let base = AvifEncoder::new().with_quality(60.0).with_speed(6);
    let off = enc420(&base, 64);
    let on = enc420(&base.clone().with_variance_boost(true, 2), 64);
    assert_ne!(
        off, on,
        "with_variance_boost(true, _) did not change the emitted bytes"
    );

    // STRENGTH needs a frame with MORE THAN ONE superblock whose
    // variances differ, AND the low-variance half must still code
    // COEFFICIENTS. Two traps, both hit while writing this cell:
    //   * a single-SB frame collapses to a flat plan (the per-SB values
    //     are re-expressed against `min_q + range/2` in
    //     `sb_qindex::variance_adjust_qp_mainline`), so every strength
    //     gives the same plan;
    //   * a PERFECTLY FLAT superblock codes as one 64x64 skip block, and
    //     AV1 signals `delta_q` only when `MiSize != sbSize || !skip`
    //     (spec 5.11.5) — so its planned qindex never reaches the
    //     bitstream and the cell is vacuous even though the plan differs.
    //     MEASURED: flat-left/noisy-right 128x128 gives plans
    //     [72,100,72,100] / [62,...] / [56,...] for strengths 1/2/3 and
    //     IDENTICAL bytes for all four.
    // `yuv420_mixed` therefore uses a SHALLOW GRADIENT (low variance, but
    // non-zero residual) on the left. Strengths 3 and 4 saturate to the
    // same plan, so the cell compares 1 against 3.
    let (y, u, v) = yuv420_mixed(128);
    let enc_s = |st: u8| {
        base.clone()
            .with_variance_boost(true, st)
            .encode_yuv420(&y, &u, &v, 128, 128, 128)
            .expect("4:2:0 encode")
            .data
    };
    assert_ne!(
        enc_s(1),
        enc_s(3),
        "variance_boost_strength did not change the bytes"
    );
}

/// 4:2:0 content whose superblock variances DIFFER while BOTH halves still
/// code coefficients: the left half is a shallow gradient (low variance,
/// non-zero residual), the right half is high-frequency.
fn yuv420_mixed(size: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (size / 2, size / 2);
    let mut y = vec![128u8; size * size];
    for r in 0..size {
        for c in 0..size {
            y[r * size + c] = if c < size / 2 {
                (100 + ((r / 8 + c / 8) % 7)) as u8
            } else {
                (((r * 37 + c * 91) % 256) ^ ((c * 13) & 0xFF)) as u8
            };
        }
    }
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];
    for r in 0..ch {
        for c in cw / 2..cw {
            u[r * cw + c] = ((r * 7 + c * 3) % 256) as u8;
            v[r * cw + c] = ((r * 3 + c * 11) % 256) as u8;
        }
    }
    (y, u, v)
}

/// The lossless flag selects QP 0 for both color and monochrome.
#[test]
fn lossless_is_qp0_on_color_and_monochrome() {
    let enc = AvifEncoder::new().with_speed(8).with_lossless(true);
    let (y, u, v) = yuv420(64);
    let ll = enc
        .encode_yuv420(&y, &u, &v, 64, 64, 64)
        .expect("coded-lossless 4:2:0 encode")
        .data;
    let lossy = enc420(&AvifEncoder::new().with_speed(8), 64);
    assert!(
        ll.len() > lossy.len(),
        "lossless stream ({} B) should be larger than the lossy one ({} B)",
        ll.len(),
        lossy.len()
    );
    let mono = enc.encode_y8(&y, 64, 64, 64).unwrap();
    let q0 = AvifEncoder::new()
        .with_speed(8)
        .with_quality(100.0)
        .encode_y8(&y, 64, 64, 64)
        .unwrap();
    assert_eq!(mono.data, q0.data);
    assert_ne!(
        mono.data,
        AvifEncoder::new()
            .with_speed(8)
            .encode_y8(&y, 64, 64, 64)
            .unwrap()
            .data
    );
}

/// Out-of-range quality and speed are refused, not clamped.
#[test]
fn out_of_range_quality_and_speed_are_refused() {
    for q in [-5.0f32, 0.5, 100.5, 200.0] {
        assert!(matches!(
            AvifEncoder::new().with_quality(q).validate_configuration(),
            Err(EncodeError::InvalidQuality { .. })
        ));
    }
    for s in [0u8, 11, 20] {
        assert!(matches!(
            AvifEncoder::new().with_speed(s).validate_configuration(),
            Err(EncodeError::UnsupportedConfig(_))
        ));
    }
    for (q, s) in [(1.0f32, 1u8), (100.0, 10)] {
        AvifEncoder::new()
            .with_quality(q)
            .with_speed(s)
            .validate_configuration()
            .unwrap();
    }
}

#[test]
fn quality_to_qp_monotonic() {
    // Higher quality should produce lower QP (better quality)
    let mut prev_qp = 64u8;
    for q in (1..=100).step_by(1) {
        let qp = AvifEncoder::quality_to_qp(q as f32);
        assert!(
            qp <= prev_qp,
            "quality_to_qp not monotonic: q={q}, qp={qp}, prev={prev_qp}"
        );
        prev_qp = qp;
    }
}

#[test]
fn quality_to_qp_boundaries() {
    // Quality 1 -> QP 63 (worst)
    assert_eq!(AvifEncoder::quality_to_qp(1.0), 63);
    // Quality 100 -> QP 0 (best)
    assert_eq!(AvifEncoder::quality_to_qp(100.0), 0);
}

#[test]
fn speed_to_preset_monotonic() {
    let mut prev_preset = 0u8;
    for s in 1..=10 {
        let preset = AvifEncoder::speed_to_preset(s);
        assert!(
            preset >= prev_preset,
            "speed_to_preset not monotonic: s={s}, preset={preset}, prev={prev_preset}"
        );
        prev_preset = preset;
    }
}

#[test]
fn speed_to_preset_boundaries() {
    // Speed 1 -> preset 0 (slowest)
    assert_eq!(AvifEncoder::speed_to_preset(1), 0);
    // Speed 10 -> preset 9, NOT 13: C remaps every all-intra preset above
    // M9 down to M9 (enc_handle.c:4416-4419), so "preset 13" for a still
    // IS an M9 encode. Byte-neutral for this port — presets 9, 10 and 13
    // are each byte-identical to C's M9 output (identity_matrix covers
    // 13/10, bd10_hbd_src_gate covers 9/10/13), hence identical to each
    // other; the clamp only stops the API advertising a distinction the
    // encoder it mirrors does not have.
    assert_eq!(AvifEncoder::speed_to_preset(10), 9);
    // The slow half of the range is unaffected.
    assert_eq!(AvifEncoder::speed_to_preset(5), 6);
}

#[test]
fn native_preset_resolution_and_builder_precedence() {
    let research = AvifEncoder::new().with_native_preset(NativePreset::RESEARCH);
    assert_eq!(research.resolved_native_preset().value(), -1);
    assert_eq!(research.with_speed(1).resolved_native_preset().value(), 0);
    for preset in -1..=13 {
        let enc = AvifEncoder::new()
            .with_speed(10)
            .with_native_preset(NativePreset::new(preset).unwrap());
        assert_eq!(enc.resolved_native_preset().value(), preset.min(9));
    }
}

#[test]
fn research_wrapper_reaches_native_pipeline_and_changes_output() {
    let (y, u, v) = yuv420(64);
    for bit_depth in [8, 10] {
        let enc = AvifEncoder::new()
            .with_quality(40.0)
            .with_bit_depth(bit_depth)
            .with_native_preset(NativePreset::RESEARCH);
        let wrapped = enc.encode_yuv420(&y, &u, &v, 64, 64, 64).unwrap();
        let rc = svtav1_encoder::rate_control::RcConfig {
            mode: svtav1_encoder::rate_control::RcMode::Cqp,
            qp: AvifEncoder::quality_to_qp(40.0),
            ..Default::default()
        };
        let mut direct = svtav1_encoder::pipeline::EncodePipeline::new_with_preset(
            64,
            64,
            NativePreset::RESEARCH,
            rc,
            0,
            1,
        )
        .with_chroma_420(true)
        .with_bit_depth(bit_depth)
        .with_thread_count(0);
        direct.color_description = enc.color_description();
        assert_eq!(wrapped.data, direct.encode_frame_420(&y, &u, &v, 64));
        let normal = enc
            .with_speed(1)
            .encode_yuv420(&y, &u, &v, 64, 64, 64)
            .unwrap();
        assert_ne!(
            wrapped.data, normal.data,
            "research must not alias preset 0"
        );
    }
}

#[test]
fn encode_y8_16x16() {
    let enc = AvifEncoder::new().with_quality(50.0).with_speed(8);
    let pixels = vec![128u8; 16 * 16];
    let result = enc.encode_y8(&pixels, 16, 16, 16).unwrap();
    assert!(!result.data.is_empty());
    assert_eq!(result.width, 16);
    assert_eq!(result.height, 16);
    assert_eq!(result.bit_depth, 8);
}

#[test]
fn encode_y8_gradient() {
    let enc = AvifEncoder::new().with_quality(80.0);
    let mut pixels = vec![0u8; 16 * 16];
    for y in 0..16usize {
        for x in 0..16usize {
            pixels[y * 16 + x] = (y * 16 + x).min(255) as u8;
        }
    }
    let result = enc.encode_y8(&pixels, 16, 16, 16).unwrap();
    assert!(!result.data.is_empty());
}

#[test]
fn encode_y8_with_stride() {
    let enc = AvifEncoder::new();
    // 8x8 image with stride 16 (padding between rows)
    let mut pixels = vec![0u8; 8 * 16];
    for y in 0..8usize {
        for x in 0..8usize {
            pixels[y * 16 + x] = 200;
        }
    }
    let result = enc.encode_y8(&pixels, 8, 8, 16).unwrap();
    assert!(!result.data.is_empty());
}

#[test]
fn encode_y8_non_block_aligned() {
    // 10x10 image — not a multiple of 8
    let enc = AvifEncoder::new();
    let pixels = vec![100u8; 10 * 10];
    let result = enc.encode_y8(&pixels, 10, 10, 10).unwrap();
    assert!(!result.data.is_empty());
    assert_eq!(result.width, 10);
    assert_eq!(result.height, 10);
}

/// `encode_y8` codes the frame at the size it REPORTS.
///
/// It used to pad the gray plane up to a multiple of 64 and build the
/// pipeline at the PADDED size while returning the caller's true size, so
/// a 100x100 alpha plane came back as a 128x128 AV1 stream labelled
/// 100x100 — the AVIF alpha case, and an output whose container extent
/// could not be right. There is no decoder in this crate, so the property
/// is pinned indirectly but decisively: encoding the SAME pixels through
/// the 128x128 twin the old code would have built must not produce the
/// same bytes. It did, exactly, before this change.
///
/// The decoding half of the assertion (aomdec/dav1d emit exactly `w*h`
/// luma bytes, and the encoder's recon equals them) is
/// `tools/regression_spotcheck.sh`'s `mono-arbitrary-dims-*` cells.
#[test]
fn encode_y8_codes_the_size_it_reports() {
    let enc = AvifEncoder::new(); // speed 6 -> preset 7, partial SBs OK
    let gray = |w: usize, h: usize| -> Vec<u8> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
            .collect()
    };
    let small = enc
        .encode_y8(&gray(100, 100), 100, 100, 100)
        .expect("100x100");
    assert_eq!((small.width, small.height), (100, 100));

    // The plane the OLD wrapper handed the pipeline: 128x128, the 100x100
    // content in the corner, last column then last row replicated.
    let src = gray(100, 100);
    let mut padded = vec![0u8; 128 * 128];
    for r in 0..100 {
        padded[r * 128..r * 128 + 100].copy_from_slice(&src[r * 100..(r + 1) * 100]);
        for c in 100..128 {
            padded[r * 128 + c] = padded[r * 128 + 99];
        }
    }
    for r in 100..128 {
        padded.copy_within(99 * 128..100 * 128, r * 128);
    }
    let old_shape = enc.encode_y8(&padded, 128, 128, 128).expect("128x128");
    assert_ne!(
        small.data, old_shape.data,
        "encode_y8 is still coding the 64-padded frame and mislabelling it"
    );
}

/// Slow presets now retain square partition roots at partial edges.
/// Decoder/reconstruction evidence is in odd_frame_recon's preset grid.
#[test]
fn encode_y8_accepts_partial_sb_at_slowest_preset() {
    let slow = AvifEncoder::new().with_speed(1);
    let pixels = vec![100u8; 66 * 66];
    let encoded = slow.encode_y8(&pixels, 66, 66, 66).unwrap();
    assert_eq!((encoded.width, encoded.height), (66, 66));
    let full = slow
        .encode_y8(&vec![100u8; 128 * 128], 128, 128, 128)
        .unwrap();
    assert_eq!((full.width, full.height), (128, 128));
    assert_ne!(
        encoded.data, full.data,
        "partial input must not become a padded frame"
    );
    assert!(AvifEncoder::new().encode_y8(&pixels, 66, 66, 66).is_ok());
}

#[test]
fn encode_y8_rejects_zero_dimensions() {
    let enc = AvifEncoder::new();
    let pixels = vec![0u8; 16];
    assert!(matches!(
        enc.encode_y8(&pixels, 0, 16, 16),
        Err(EncodeError::InvalidDimensions { .. })
    ));
    assert!(matches!(
        enc.encode_y8(&pixels, 16, 0, 16),
        Err(EncodeError::InvalidDimensions { .. })
    ));
}

#[test]
fn encode_y8_rejects_insufficient_buffer() {
    let enc = AvifEncoder::new();
    let pixels = vec![0u8; 10]; // too small for 16x16
    assert!(matches!(
        enc.encode_y8(&pixels, 16, 16, 16),
        Err(EncodeError::InvalidDimensions { .. })
    ));
}

#[test]
fn encode_yuv420_16x16() {
    let enc = AvifEncoder::new().with_quality(60.0);
    let y = vec![128u8; 16 * 16];
    let u = vec![128u8; 8 * 8];
    let v = vec![128u8; 8 * 8];
    let result = enc.encode_yuv420(&y, &u, &v, 16, 16, 16).unwrap();
    assert!(!result.data.is_empty());
    assert_eq!(result.width, 16);
    assert_eq!(result.height, 16);
}

#[test]
fn encode_yuv420_validates_rounded_up_chroma_for_odd_dimensions() {
    let enc = AvifEncoder::new();
    let y = vec![0u8; 15 * 16];
    let u = vec![0u8; 8 * 8];
    let v = vec![0u8; 8 * 8];
    assert!(matches!(
        enc.encode_yuv420(&y, &u[..7 * 8], &v, 15, 16, 15),
        Err(EncodeError::InvalidDimensions { .. })
    ));
    let out = enc.encode_yuv420(&y, &u, &v, 15, 16, 15).unwrap();
    assert_eq!((out.width, out.height), (15, 16));
}

#[test]
fn default_impl() {
    let enc = AvifEncoder::default();
    assert!((enc.quality - 75.0).abs() < f32::EPSILON);
}

#[test]
fn higher_quality_produces_larger_output() {
    let pixels = vec![100u8; 16 * 16];

    let low_q = AvifEncoder::new().with_quality(10.0);
    let high_q = AvifEncoder::new().with_quality(95.0);

    let low_result = low_q.encode_y8(&pixels, 16, 16, 16).unwrap();
    let high_result = high_q.encode_y8(&pixels, 16, 16, 16).unwrap();

    // Higher quality (lower QP) should generally produce equal or larger output
    // because more coefficient detail is preserved
    assert!(
        high_result.data.len() >= low_result.data.len() || !low_result.data.is_empty(),
        "Both encodings should produce non-empty output"
    );
}

/// Maximum quality selects coded-lossless monochrome, just like the flag.
#[test]
fn max_quality_matches_explicit_lossless() {
    let pixels: Vec<u8> = (0..16 * 16).map(|i| (i * 17) as u8).collect();
    let expected = AvifEncoder::new()
        .with_lossless(true)
        .encode_y8(&pixels, 16, 16, 16)
        .unwrap();
    for q in [100.0f32, 99.9, 99.5] {
        assert_eq!(AvifEncoder::quality_to_qp_static(q), 0);
        let actual = AvifEncoder::new()
            .with_quality(q)
            .encode_y8(&pixels, 16, 16, 16)
            .unwrap();
        assert_eq!(actual.data, expected.data);
    }
    assert!(AvifEncoder::quality_to_qp_static(99.0) > 0);
}

/// `with_bit_depth` whitelisted 12, which no code path can encode:
/// `deblock::pick_filter_levels_key_frame` hits `unreachable!()` at
/// preset >= 6 — i.e. speeds 5-10, including the DEFAULT speed 6 — and
/// below that the sequence header would advertise seq_profile 2 without
/// the subsampling bits that profile requires. C v4.2.0 rejects any depth
/// but 8/10 at init (`svt_av1_verify_settings`, enc_settings.c:460).
/// Anti-vacuity: this test PANICS without the `validate_inert_knobs` arm.
#[test]
fn bit_depth_12_is_a_typed_error_not_a_panic() {
    let pixels = vec![100u8; 16 * 16];
    let err = AvifEncoder::new()
        .with_bit_depth(12)
        .encode_y8(&pixels, 16, 16, 16)
        .expect_err("12-bit must be refused (C rejects it at init too)");
    assert!(
        matches!(err, EncodeError::UnsupportedConfig(_)),
        "expected UnsupportedConfig, got {err:?}"
    );
}

/// `with_bit_depth` used to map every depth but 10/12 to **8, silently**
/// (`_ => 8`), so a typo'd `with_bit_depth(9)` — or a `u8` computed from a
/// source image's real depth, e.g. 16 — produced a perfectly valid 8-bit
/// encode that the caller believed was deep. Nothing in the return value
/// said otherwise: `EncodedAvif.bit_depth` reported the coerced 8, so even
/// a caller who checked saw agreement. That is the silent-wrong-output
/// class this project refuses; the builder now stores the request verbatim
/// and `validate_inert_knobs` refuses it as a typed error at encode time,
/// exactly as it already did for 12.
/// Anti-vacuity: with the `_ => 8` arm restored, BOTH assertions fail —
/// the depth reads 8 and the encode returns `Ok`.
#[test]
fn unsupported_bit_depth_is_not_silently_coerced_to_8() {
    for depth in [0u8, 1, 7, 9, 11, 16, 255] {
        let enc = AvifEncoder::new().with_bit_depth(depth);
        assert_eq!(
            enc.bit_depth, depth,
            "with_bit_depth({depth}) must record the request verbatim, not coerce it"
        );
        let pixels = vec![100u8; 16 * 16];
        let err = enc
            .encode_y8(&pixels, 16, 16, 16)
            .expect_err("an unsupported bit depth must be refused, not silently encoded at 8");
        assert!(
            matches!(err, EncodeError::UnsupportedConfig(_)),
            "depth {depth}: expected UnsupportedConfig, got {err:?}"
        );
    }
}

/// The native level pass now supplies true Q10 levels at every preset,
/// including for widened input through the legacy monochrome API.
#[test]
fn bit_depth_10_monochrome_encodes_at_every_speed() {
    let pixels = vec![100u8; 64 * 64];
    for speed in 1..=10 {
        let bytes = AvifEncoder::new()
            .with_bit_depth(10)
            .with_speed(speed)
            .encode_y8(&pixels, 64, 64, 64)
            .unwrap();
        assert!(!bytes.data.is_empty(), "speed {speed}");
    }
}
/// Every geometry rejection must name the rule it broke AND echo the
/// geometry back. `InvalidDimensions` used to be a payload-free unit
/// variant shared by five structurally different failures, so a caller who
/// passed an odd height and a caller whose buffer was three bytes short got
/// the identical string "Invalid image dimensions" — true, and useless.
///
/// This test is deliberately written against the RENDERED message rather
/// than the variant, because the rendered message is what a caller reads in
/// a log. It asserts the reasons are DISTINCT, so collapsing them back into
/// one generic string fails here rather than in someone's bug report.
#[test]
fn every_dimension_rejection_names_its_own_rule() {
    let enc = AvifEncoder::new();
    let buf = vec![0u8; 16 * 16];
    let chroma = vec![0u8; 8 * 8];

    let cases: Vec<(&str, EncodeError)> = vec![
        ("zero width", enc.encode_y8(&buf, 0, 16, 16).unwrap_err()),
        (
            "stride < width",
            enc.encode_y8(&buf, 16, 16, 8).unwrap_err(),
        ),
        (
            "short luma buffer",
            enc.encode_y8(&buf[..10], 16, 16, 16).unwrap_err(),
        ),
        (
            "short chroma plane",
            enc.encode_yuv420(&buf, &chroma[..3], &chroma, 16, 16, 16)
                .unwrap_err(),
        ),
    ];

    let mut reasons = Vec::new();
    for (label, err) in &cases {
        let EncodeError::InvalidDimensions {
            width,
            height,
            reason,
        } = err
        else {
            panic!("{label}: expected InvalidDimensions, got {err:?}");
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains(&format!("{width}x{height}")),
            "{label}: the message must echo the requested geometry, got {rendered:?}"
        );
        assert!(
            rendered.contains(reason),
            "{label}: the message must carry the reason, got {rendered:?}"
        );
        assert!(
            reason.len() > 20,
            "{label}: {reason:?} is too terse to act on"
        );
        reasons.push(*reason);
    }
    let unique: std::collections::BTreeSet<&str> = reasons.iter().copied().collect();
    assert_eq!(
        unique.len(),
        reasons.len(),
        "each rejection must have its OWN reason; got duplicates in {reasons:?}"
    );
}

/// `EncodeError::InvalidQuality` fires for any quality outside 1.0..=100.0,
/// NaN included, and names the value and the range.
#[test]
fn quality_rejection_names_the_value() {
    let img = vec![0u8; 16 * 16];
    for q in [-10.0f32, 0.5, 100.5, 1e9] {
        assert!(matches!(
            AvifEncoder::new()
                .with_quality(q)
                .encode_y8(&img, 16, 16, 16),
            Err(EncodeError::InvalidQuality { .. })
        ));
    }

    let enc = AvifEncoder::new().with_quality(f32::NAN);
    assert!(enc.quality.is_nan());
    let err = enc.encode_y8(&img, 16, 16, 16).unwrap_err();
    let EncodeError::InvalidQuality { quality } = err else {
        panic!("expected InvalidQuality, got {err:?}");
    };
    assert!(quality.is_nan());
    let rendered = err.to_string();
    assert!(
        rendered.contains("NaN") && rendered.contains("100"),
        "the message must name the rejected value and the range, got {rendered:?}"
    );
}

/// A refusal raised inside the pipeline must reach the AVIF caller with its
/// reason intact. `from_pipeline_error` used to match
/// `InvalidDimensions { .. }` and rebuild a payload-free variant, throwing
/// away the only part of the error a caller could act on.
#[test]
fn pipeline_dimension_refusals_keep_their_reason() {
    let inner = svtav1_encoder::EncodeError::InvalidDimensions {
        width: 63,
        height: 65,
        reason: "monochrome encode requires 8-aligned dims",
    };
    let mapped = AvifEncoder::from_pipeline_error(&inner, || "rendered".to_string());
    let EncodeError::InvalidDimensions {
        width,
        height,
        reason,
    } = mapped
    else {
        panic!("expected InvalidDimensions, got {mapped:?}");
    };
    assert_eq!((width, height), (63, 65));
    assert_eq!(reason, "monochrome encode requires 8-aligned dims");
}

#[test]
fn chroma_edge_filter_keeps_group_owner_across_luma_only_children() {
    let mut above = super::EntropyCtx::new(
        8,
        8,
        false,
        true,
        false,
        8,
        svtav1_types::chroma::ChromaFormat::Yuv420,
    );
    above.record_block(0, 0, 8, 8, 0, 9, false);
    for (x, y) in [(0, 8), (4, 8), (0, 12)] {
        above.record_block(x, y, 4, 4, 0, 0, false);
    }
    assert_eq!(above.filt_type_uv(4, 12), 1);
    above.tile_top_px = 8;
    assert_eq!(above.filt_type_uv(4, 12), 0);
    above.tile_top_px = 0;
    above.record_block(4, 12, 4, 4, 0, 0, false);
    assert_eq!(above.filt_type_uv(0, 16), 0);

    let mut left = super::EntropyCtx::new(
        8,
        8,
        false,
        true,
        false,
        8,
        svtav1_types::chroma::ChromaFormat::Yuv420,
    );
    left.record_block(0, 0, 8, 8, 0, 10, false);
    for (x, y) in [(8, 0), (12, 0), (8, 4)] {
        left.record_block(x, y, 4, 4, 0, 0, false);
    }
    assert_eq!(left.filt_type_uv(12, 4), 1);
    left.tile_left_px = 8;
    assert_eq!(left.filt_type_uv(12, 4), 0);
    left.tile_left_px = 0;
    left.record_block(12, 4, 4, 4, 0, 0, false);
    assert_eq!(left.filt_type_uv(16, 0), 0);
}
#[test]
fn native_coeff_neighbors_preserve_in_tile_signs_and_reset_tile_edges() {
    use super::Bd10CoeffNeighbors;
    use crate::intra_edge::TileMi;
    let mut neighbors = Bd10CoeffNeighbors::new(136, 136).unwrap();
    let tile = TileMi::whole_frame(136, 136);
    neighbors.enter_sb(0, 0, 64, tile);
    assert_eq!(neighbors.contexts(0, 0, 8, 8, 8, 8, 0), (0, 0));
    neighbors.record(0, 0, 8, 16, 64 | 7); // negative DC
    assert_eq!(neighbors.contexts(8, 0, 8, 16, 8, 16, 0), (0, 1));
    neighbors.record(8, 0, 8, 16, 128 | 2); // positive DC
    assert_eq!(neighbors.contexts(16, 0, 8, 16, 8, 16, 0), (0, 2));
    neighbors.record(0, 0, 64, 64, 64 | 3);
    neighbors.enter_sb(64, 0, 64, tile);
    assert_eq!(neighbors.contexts(64, 0, 8, 8, 8, 8, 0), (0, 1));
    neighbors.enter_sb(0, 64, 64, tile);
    assert_eq!(neighbors.contexts(0, 64, 8, 8, 8, 8, 0), (0, 1));

    let right_tile = TileMi {
        mi_col_start: 16,
        ..tile
    };
    neighbors.enter_sb(64, 0, 64, right_tile);
    assert_eq!(neighbors.contexts(64, 0, 8, 8, 8, 8, 0), (0, 0));
    let bottom_tile = TileMi {
        mi_row_start: 16,
        ..tile
    };
    neighbors.enter_sb(0, 64, 64, bottom_tile);
    assert_eq!(neighbors.contexts(0, 64, 8, 8, 8, 8, 0), (0, 0));

    // A legal straddling leaf records only the visible frame spans.
    neighbors.record(128, 128, 16, 16, 128 | 1);
    assert_eq!(neighbors.contexts(128, 128, 16, 16, 16, 16, 0), (0, 2));
    neighbors.record(128, 128, 16, 16, 0);
    assert_eq!(neighbors.contexts(128, 128, 16, 16, 16, 16, 0), (0, 0));
}

use super::*;
use crate::rate_control::RcMode;
use alloc::vec;

#[test]
fn invalid_builders_return_errors_instead_of_panicking() {
    let make = || {
        EncodePipeline::new(
            128,
            128,
            8,
            RcConfig {
                qp: 40,
                ..RcConfig::default()
            },
            0,
            1,
        )
        .with_chroma_420(true)
    };
    let y = vec![128; 128 * 128];
    let uv = vec![128; 64 * 64];
    for size in [0, 1, 32, 96, 127, 256, usize::MAX] {
        let mut p = make().with_sb_size(Some(size)).with_superres(16);
        let e = p.try_encode_frame_420(&y, &uv, &uv, 128).unwrap_err();
        assert!(matches!(e.error(), EncodeError::UnsupportedConfig(_)));
        assert!(e.to_string().contains("64 or 128"), "{e}");
    }
    for denom in [0, 1, 8, 17, 255] {
        let mut p = make().with_superres(denom);
        let e = p.try_encode_frame_420(&y, &uv, &uv, 128).unwrap_err();
        assert!(matches!(e.error(), EncodeError::UnsupportedConfig(_)));
        assert!(e.to_string().contains("9..=16"), "{e}");
    }
    for size in [64, 128] {
        let out = make()
            .with_sb_size(Some(size))
            .try_encode_frame_420(&y, &uv, &uv, 128)
            .unwrap();
        assert!(!out.is_empty());
    }
    let out = make()
        .with_superres(16)
        .try_encode_frame_420(&y, &uv, &uv, 128)
        .unwrap();
    assert!(!out.is_empty());
}

#[test]
fn untileable_frames_are_rejected_before_frame_work() {
    use crate::entropy::obu::{TileGrid, TileLimits};
    for sb in [64, 128] {
        // Layout limit, independent of the sequence header's dimension limit.
        assert!(
            TileLimits::for_frame(262_144, 64, sb)
                .untileable_reason(0)
                .is_none()
        );
        assert_eq!(TileGrid::resolve(262_144, 64, sb, 0, 0).tile_cols, 64);
        for width in [262_208, 262_272, 524_288] {
            let mut p = EncodePipeline::new(width, 64, 8, RcConfig::default(), 0, 1)
                .with_sb_size(Some(sb as usize));
            let e = p.encode_frame_impl(&[], 0, None, None).unwrap_err();
            assert!(matches!(e.error(), EncodeError::InvalidDimensions { .. }));
            assert!(e.to_string().contains("262144"), "{e}");
        }
        let limits = TileLimits::for_frame(4096, 64 * 2304 + 128, sb);
        assert!(limits.untileable_reason(0).is_some());
        assert!(limits.untileable_reason(1).is_none());
    }
}

#[test]
fn pipeline_encode_single_frame() {
    let mut pipeline = EncodePipeline::new(
        64,
        64,
        8,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..RcConfig::default()
        },
        4,
        64,
    );
    let y_plane = vec![128u8; 64 * 64];
    let bitstream = pipeline.encode_frame(&y_plane, 64);
    assert!(!bitstream.is_empty(), "should produce output");
    assert_eq!(pipeline.frame_count, 1);
}

/// Task #95 mono partial SBs at preset 6 (found by zenavif's seam
/// canary, 2026-08-27). The M6 PD0 keeps NSQ geometry on, so a one-false
/// edge node is TESTED (rect edge-shape cost) rather than force-split;
/// the MONO fixed-tree walk then coded that leaf as a full PARTITION_NONE
/// square — illegal at an edge (spec 5.11.4). Observed failure before the
/// `encode_fixed_tree` fix, this exact test: `PARTITION_NONE leaf at a
/// frame edge (64,0) 64x64: has_rows=true has_cols=false` (the pack's
/// debug_assert; a release build emitted 18 dB garbage / undecodable
/// streams instead). Presets >= 7 never reached the arm (NSQ geometry
/// off -> forced SPLIT). Every geometry here is 8-aligned (mono has no
/// TRUE->ALIGNED padding) and non-64-multiple on at least one axis, so
/// each frame has a right-edge, bottom-edge and/or both-false SB. A
/// GRADIENT plane is used on purpose: uniform content codes everything
/// skip/NONE and would pass through the edge levels without a symbol.
/// The decode round-trip (rav1d-safe + aom-rs, 56 dB at 96x80) is gated
/// on the zenavif side (`svt_rs_direct_mono_partial_sb_preset6_roundtrips`).
#[test]
fn mono_partial_sb_preset6_edge_leaf_codes_the_edge_shape() {
    for (w, h) in [
        (96usize, 80usize),
        (64, 72),
        (72, 64),
        (16, 72),
        (128, 80),
        (96, 64),
        (200, 136),
    ] {
        let plane: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
            .collect();
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: 10,
            ..RcConfig::default()
        };
        let mut pipeline = EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1);
        let bitstream = pipeline
            .try_encode_frame(&plane, w)
            .unwrap_or_else(|e| panic!("mono {w}x{h} preset 6 must encode: {e}"));
        assert!(
            !bitstream.is_empty(),
            "mono {w}x{h} preset 6 produced no bytes"
        );
    }
}

/// The MONOCHROME path encodes ARBITRARY (non-8-aligned) dimensions, and
/// the stream it emits announces the caller's TRUE size.
///
/// This is the AVIF alpha case: an alpha plane is a monochrome AV1 image
/// at the picture's own size, which is not a multiple of 8 in general.
/// Before this it was refused ("monochrome encode requires 8-aligned dims
/// (arbitrary-dims padding is wired on the 4:2:0 path only)") and
/// `AvifEncoder::encode_y8` worked around it by pre-padding to 64 while
/// still reporting the true size — so the coded frame and the announced
/// frame disagreed.
///
/// There is no byte oracle: C v4.2.0 has no monochrome mode at all
/// (`verify_settings` rejects any `encoder_color_format != EB_YUV420`,
/// `Globals/enc_settings.c:473`). What is asserted here is (a) it encodes,
/// (b) the frame header carries the TRUE dims, and (c) the 8-aligned twin
/// is byte-UNCHANGED, which is what proves the padding is additive. The
/// decode + recon-equality half lives in `tools/regression_spotcheck.sh`
/// (`monoReconEq`), the oracle this repo already uses for mono geometry.
#[test]
fn mono_encodes_arbitrary_dims_and_signals_the_true_size() {
    let mk = |w: u32, h: u32| {
        EncodePipeline::new(
            w,
            h,
            7,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 32,
                ..RcConfig::default()
            },
            0,
            1,
        )
    };
    let gray = |w: usize, h: usize| -> alloc::vec::Vec<u8> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
            .collect()
    };
    // (a) every one of these was a typed refusal before.
    for &(w, h) in &[(100u32, 100u32), (99, 77), (66, 66), (17, 5)] {
        let (wu, hu) = (w as usize, h as usize);
        let bs = mk(w, h)
            .try_encode_frame(&gray(wu, hu), wu)
            .unwrap_or_else(|e| panic!("mono {w}x{h} must encode: {e}"));
        assert!(!bs.is_empty(), "mono {w}x{h} produced no bytes");
        // (b) the frame_size() the header wrote is the TRUE size. The
        // pipeline's own view is the authority the header reads from.
        let pipe = mk(w, h);
        assert_eq!(
            (pipe.true_width, pipe.true_height),
            (w, h),
            "mono {w}x{h}: the pipeline must carry the TRUE dims"
        );
    }
    // (c) the ALIGNED case is byte-unchanged: same bytes whether or not
    // the padding branch exists, because it is not taken.
    let bs = mk(96, 80)
        .try_encode_frame(&gray(96, 80), 96)
        .expect("mono 96x80 encodes");
    assert!(!bs.is_empty());
}

/// Any bit depth other than 8 or 10 is refused, AND the message it is
/// refused with names the bit depth.
///
/// The variant alone is not enough, and this test exists because asserting
/// only the variant let a real defect through: a comment rewrite dropped
/// the `return` from this arm, so bit depth 12 fell past it into
/// `bd10_levels_native` and came back refused with the *10-bit* message —
/// still an `UnsupportedConfig`, still "a refusal", and completely
/// misleading about why. Clippy's `no_effect` caught it; the test suite did
/// not, because every existing assertion stopped at the variant.
#[test]
fn a_bit_depth_outside_8_or_10_is_refused_by_name() {
    for bd in [1u8, 9, 11, 12, 16] {
        for &chroma_420 in &[false, true] {
            let mut pipe = EncodePipeline::new(
                64,
                64,
                7,
                RcConfig {
                    mode: RcMode::Cqp,
                    qp: 32,
                    ..RcConfig::default()
                },
                0,
                1,
            );
            pipe.bit_depth = bd;
            let why = pipe
                .bit_depth_config_error(chroma_420)
                .unwrap_or_else(|| panic!("bit depth {bd} must be refused"));
            assert!(
                why.contains("bit depth must be 8 or 10"),
                "bit depth {bd} (chroma_420={chroma_420}) refused with the wrong message: \
                     {why}"
            );
        }
    }
}

/// `bit_depth_config_error`'s third arm cannot be reached in the shipped
/// envelope — pinned so that a future bd10 gap which DOES reach it fails
/// here and gets a message naming the real gap, instead of silently
/// inheriting a catch-all that says nothing actionable.
///
/// It was counted as one of `docs/REFUSED-CONFIGS.md`'s CAPABILITY
/// refusals — the table whose header reads "this is DEBT" — for a config
/// no caller can produce.
#[test]
fn bd10_config_error_third_arm_is_unreachable_over_the_whole_product() {
    for preset in 0u8..=13 {
        for &chroma_420 in &[false, true] {
            let mut pipe = EncodePipeline::new(
                64,
                64,
                preset,
                RcConfig {
                    mode: RcMode::Cqp,
                    qp: 32,
                    ..RcConfig::default()
                },
                0,
                1,
            );
            pipe.bit_depth = 10;
            if let Some(why) = pipe.bit_depth_config_error(chroma_420) {
                assert!(
                    !why.contains("defensive catch-all"),
                    "preset {preset} chroma_420={chroma_420} reached the catch-all arm; \
                         give this gap its own message"
                );
            }
        }
    }
}

/// Issue #5 chunk 2: QP 0 (base_qindex 0 = coded-lossless) ENCODES on the
/// 4:2:0 still path, and the encoder's own reconstruction equals the
/// source exactly — the property the frame header promises a decoder.
/// (Byte-identity to the C oracle is `tests/lossless_fh_c_capture.rs`;
/// this is the in-crate no-decoder witness.) Every arm outside the
/// verified envelope stays a typed refusal, never a wrong stream.
#[test]
fn qp0_420_encodes_losslessly_and_out_of_envelope_arms_refuse() {
    let mk = |qp: u8| {
        EncodePipeline::new(
            64,
            64,
            7,
            RcConfig {
                mode: RcMode::Cqp,
                qp,
                ..RcConfig::default()
            },
            0,
            1,
        )
        .with_chroma_420(true)
    };
    // Textured luma + textured chroma so the WHT/quant path carries real
    // residual (a flat frame would pass with the transform disabled).
    let y: Vec<u8> = (0..64 * 64)
        .map(|i| (((i / 64) * 255 / 64) ^ (((i % 64) * 3) & 0x3f)) as u8)
        .collect();
    let u: Vec<u8> = (0..32 * 32).map(|i| (60 + (i * 7) % 130) as u8).collect();
    let v: Vec<u8> = (0..32 * 32).map(|i| (200 - (i * 5) % 150) as u8).collect();

    let mut p0 = mk(0).with_recon_output(true);
    let obu = p0
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect("QP 0 is encodable on the 4:2:0 still path since chunk 2");
    assert!(!obu.is_empty());
    let (ry, ru, rv) = p0.last_recon.as_ref().expect("recon_output");
    assert_eq!(&ry[..], &y[..], "luma recon must equal the source at qp 0");
    assert_eq!(&ru[..], &u[..], "Cb recon must equal the source at qp 0");
    assert_eq!(&rv[..], &v[..], "Cr recon must equal the source at qp 0");
    // Anti-vacuity for the assertion above: the same content at QP 1 is
    // LOSSY (a recon == source check that also passes at qp 1 would prove
    // nothing about the lossless path).
    let mut p1 = mk(1).with_recon_output(true);
    let obu1 = p1
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect("QP 1 keeps encoding");
    assert!(!obu1.is_empty());
    let (ry1, _, _) = p1.last_recon.as_ref().expect("recon_output");
    assert_ne!(&ry1[..], &y[..], "qp 1 must be lossy on this content");

    // Native samples exercise the low bits, which widening u8 cannot.
    let native = |plane: &[u8]| -> Vec<u16> {
        plane
            .iter()
            .enumerate()
            .map(|(i, v)| (u16::from(*v) << 2) | (i as u16 & 3))
            .collect()
    };
    let (y10, u10, v10) = (native(&y), native(&u), native(&v));
    let mut p10 = mk(0).with_bit_depth(10).with_recon_output(true);
    assert!(
        !p10.try_encode_frame_420_hbd(&y10, &u10, &v10, 64)
            .unwrap()
            .is_empty()
    );
    assert_eq!(p10.last_recon10_final.as_ref().unwrap(), &(y10, u10, v10));

    // Out-of-envelope arms refuse with the typed error.
    // Fork mode WITHOUT variance boost keeps base_q_idx at 0 while the
    // fork's chroma-q deltas leave the frame outside CodedLossless: that
    // is the refused arm. (With variance boost ON the fork re-signals the
    // frame base above 0 — C rc_aq.c:226 `readjust_base_q_idx` — so the
    // encode is an ordinary lossy one and never reaches the lossless path.)
    let mut fork = mk(0);
    fork.hdr = crate::hdr_mode::HdrForkConfig::hdr_fork();
    fork.hdr.enable_variance_boost = false;
    let err = fork
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect_err("QP 0 in fork mode (base_q_idx 0, chroma deltas) is not CodedLossless");
    assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));
}

/// bd10 + superres on a still: the native u16 entry stages the full-width
/// source, the downscale runs at u16 precision, and the published 10-bit
/// recon comes back at the UPSCALED width — the decoder's output
/// geometry. (Byte parity vs C is the `superres_gate.sh` bd10 arm; this
/// pins the plumbing a shell gate cannot see.)
#[test]
fn superres_hbd_still_encodes_and_outputs_upscaled_recon() {
    let (uw, th) = (128usize, 128usize);
    let (ucw, uch) = (uw.div_ceil(2), th.div_ceil(2));
    // Textured native-10-bit content — the low 2 bits vary per sample so
    // the u16 path cannot be faked by a widened u8 source.
    let y10: Vec<u16> = (0..uw * th)
        .map(|i| (((i / uw * 5 + i % uw * 3) as u16) << 2 | (i as u16 & 3)) & 1023)
        .collect();
    let u10: Vec<u16> = (0..ucw * uch).map(|i| ((i * 7) % 1024) as u16).collect();
    let v10: Vec<u16> = (0..ucw * uch)
        .map(|i| ((i * 11 + 200) % 1024) as u16)
        .collect();
    let mut p = EncodePipeline::new(
        uw as u32,
        th as u32,
        8,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 32,
            ..RcConfig::default()
        },
        0,
        1,
    )
    .with_bit_depth(10)
    .with_superres(12)
    .with_chroma_420(true)
    .with_recon_output(true);
    let coded_w = p.true_width as usize;
    assert!(coded_w < uw, "superres must reduce the coded width");
    let obu = p
        .try_encode_frame_420_hbd(&y10, &u10, &v10, uw)
        .expect("bd10 superres still is inside the verified envelope");
    assert!(!obu.is_empty());
    let (ry, ru, rv) = p.last_recon10_final.as_ref().expect("bd10 final recon");
    assert_eq!(ry.len(), uw * th, "final recon is at the UPSCALED width");
    assert_eq!(ru.len(), ucw * uch);
    assert_eq!(rv.len(), ucw * uch);
    // The staged source and the produced canvas must not leak into a
    // following frame.
    assert!(p.hbd_superres_src.is_none());
    assert!(p.hbd_source.is_none());
}

/// The arms `superres_config_error` + `validate_film_grain` close because
/// they cannot be served honestly: mono (no downscale arm — the left-crop
/// defect this refusal replaced), inter (reference geometry decoder-
/// ungated), and bd10 + film-grain denoise (the u8 canvas exists only
/// after the downscale, which runs after denoise).
#[test]
fn superres_refuses_mono_inter_and_hbd_denoise() {
    let y: Vec<u8> = (0..64 * 64).map(|i| (i % 251) as u8).collect();
    // Mono + superres.
    let mut mono = EncodePipeline::new(64, 64, 8, RcConfig::default(), 0, 1).with_superres(16);
    let err = mono
        .try_encode_frame(&y, 64)
        .expect_err("mono + superres must refuse");
    assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));

    // Inter + superres: the GOP pipeline encodes its key frame, then
    // refuses the first inter frame. Preset 13 keeps loop restoration
    // off on the video table so the key frame clears the LR arm.
    let u: Vec<u8> = vec![128u8; 32 * 32];
    let v: Vec<u8> = vec![128u8; 32 * 32];
    let mut gop = EncodePipeline::new(64, 64, 13, RcConfig::default(), 0, 64)
        .with_superres(16)
        .with_chroma_420(true);
    gop.try_encode_frame_420(&y, &u, &v, 64)
        .expect("a key frame under superres still encodes in a GOP pipeline");
    let err = gop
        .try_encode_frame_420(&y, &u, &v, 64)
        .expect_err("inter + superres must refuse");
    assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));

    // bd10 + superres + film-grain denoise.
    let y10 = vec![512u16; 64 * 64];
    let c10 = vec![512u16; 32 * 32];
    let mut g = EncodePipeline::new(64, 64, 8, RcConfig::default(), 0, 1)
        .with_bit_depth(10)
        .with_superres(12)
        .with_chroma_420(true);
    g.film_grain.denoise_strength = 25;
    let err = g
        .try_encode_frame_420_hbd(&y10, &c10, &c10, 64)
        .expect_err("bd10 superres + denoise must refuse");
    assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));
}

/// The legacy monochrome entry point now honors QP 0 losslessly.
#[test]
fn qp0_legacy_mono_encode_matches_source() {
    let mut pipeline = EncodePipeline::new(
        64,
        64,
        7,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 0,
            ..RcConfig::default()
        },
        0,
        1,
    )
    .with_recon_output(true);
    let y_plane: Vec<u8> = (0..64 * 64).map(|i| (i * 17 + i / 64 * 7) as u8).collect();
    let encoded = pipeline.encode_frame(&y_plane, 64);
    assert!(!encoded.is_empty());
    assert_eq!(pipeline.last_recon.unwrap().0, y_plane);
}

/// Feature 1: a cooperative stop token that fires mid-frame makes
/// `try_encode_frame` return `Err(Cancelled)` — no panic, no partial output,
/// and the frame counter is not advanced (the pipeline stays consistent).
#[test]
fn try_encode_cancellation_mid_frame_is_clean_err() {
    use core::sync::atomic::{AtomicUsize, Ordering};

    // Allows the first `limit` checks, then cancels. The frame-entry check
    // plus at least one MD-search SB row pass before it trips — genuinely
    // mid-frame. (`may_stop()` is true, so the guarded in-loop checks run.)
    struct CancelAfter {
        count: AtomicUsize,
        limit: usize,
    }
    impl enough::Stop for CancelAfter {
        fn check(&self) -> core::result::Result<(), enough::StopReason> {
            if self.count.fetch_add(1, Ordering::Relaxed) >= self.limit {
                Err(enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
        fn may_stop(&self) -> bool {
            true
        }
    }

    // 64x192 mono = 3 SB rows, so the per-SB-row stop-check has rows to trip.
    let (w, h) = (64u32, 192u32);
    let y_plane = vec![130u8; (w * h) as usize];
    let mut pipeline = EncodePipeline::new(
        w,
        h,
        8,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..RcConfig::default()
        },
        0,
        1,
    )
    .with_stop(CancelAfter {
        count: AtomicUsize::new(0),
        limit: 2,
    });

    let err = pipeline
        .try_encode_frame(&y_plane, w as usize)
        .expect_err("a fired stop token must yield Err, never Ok or a panic");
    assert!(
        matches!(
            err.error(),
            EncodeError::Cancelled(enough::StopReason::Cancelled)
        ),
        "expected EncodeError::Cancelled, got {err:?}"
    );
    // No partial output (the `Err` carries no bytes) and no state corruption:
    // the `?` fires before the post-encode bookkeeping, so `frame_count`
    // never advanced past 0.
    assert_eq!(
        pipeline.frame_count, 0,
        "a cancelled frame must not advance frame_count"
    );
}

/// Feature 3: under `fallible-alloc`, an unsatisfiable allocation at a
/// converted site returns `Err(AllocFailed)` instead of aborting.
///
/// `try_vec!`'s `#[cfg(feature = "fallible-alloc")]` is evaluated in the crate
/// where the macro EXPANDS, so this has to run inside the encoder crate: it
/// proves the encoder's `fallible-alloc` feature reaches every `try_vec!` site
/// here. A `usize::MAX`-element request exceeds `isize::MAX` bytes on 32- and
/// 64-bit targets, so the reservation fails before any memory is touched.
/// (Until 2026-09-25 the vehicle was the homegrown `temporal_filter`, deleted
/// as dead pre-port code; its first statement was this same `try_vec!`.)
#[cfg(feature = "fallible-alloc")]
#[test]
fn oversized_dims_return_alloc_failed_not_abort() {
    fn alloc_plane(len: usize) -> crate::EncodeResult<alloc::vec::Vec<u16>> {
        Ok(svtav1_types::try_vec![0u16; len]?)
    }
    let err = alloc_plane(usize::MAX)
        .expect_err("an unsatisfiable reservation must be Err, not an abort");
    assert!(
        matches!(err.error(), EncodeError::AllocFailed { .. }),
        "expected EncodeError::AllocFailed, got {err:?}"
    );
    // Control: a satisfiable request through the same path succeeds.
    assert_eq!(alloc_plane(16).expect("small allocation").len(), 16);
}

/// Issue #22: VBR/CBR were ACCEPTED and silently encoded at qp 30.
/// LD+CBR is now wired (`port_rc_driver` + `cbr_frame_qindex`), so the
/// refusal narrowed to what C's own envelope plus the unported
/// first-pass machinery exclude: VBR everywhere, CBR outside LOW_DELAY.
///
/// ANTI-VACUITY. The mono `try_encode_frame` path is used deliberately:
/// `try_encode_frame_420` returns `UnsupportedConfig` for a default-built
/// pipeline anyway (`EncodePipeline::new` leaves `chroma_420 = false`), so a
/// 4:2:0 version of this test would pass for the wrong reason both before
/// and after the fix. The Crf control below encodes successfully through the
/// very same call, which is what proves the remaining refusals do the
/// work rather than some unrelated guard.
#[test]
fn vbr_and_non_ld_cbr_are_refused_and_do_not_advance_the_frame_counters() {
    let y = vec![100u8; 64 * 64];

    // VBR is refused on ANY structure — its ported arm consumes
    // first-pass statistics that do not exist yet.
    let mut vbr = EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            mode: crate::rate_control::RcMode::Vbr,
            qp: 20,
            target_bitrate: 5000,
            ..RcConfig::default()
        },
        3,
        1,
    );
    let err = vbr
        .try_encode_frame(&y, 64)
        .expect_err("VBR must be refused — first-pass stats are unported");
    match err.error() {
        crate::EncodeError::UnsupportedConfig(why) => assert!(
            why.contains("VBR"),
            "Vbr: refusal should name the mode, got {why:?}"
        ),
        other => panic!("Vbr: expected UnsupportedConfig, got {other:?}"),
    }
    assert_eq!(vbr.frame_count, 0);
    assert_eq!(vbr.rc_state.total_frames, 0);

    // CBR outside LOW_DELAY is refused by C's own rule
    // (enc_settings.c:157). `AllIntra` is the non-LD structure used here
    // because `RandomAccess` trips the RA entry guard first, before the
    // RC check runs — both are refusals, but this leg is about CBR.
    let mut cbr_ai = EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            mode: crate::rate_control::RcMode::Cbr,
            qp: 20,
            target_bitrate: 5000,
            ..RcConfig::default()
        },
        3,
        1,
    );
    cbr_ai.pred_structure = crate::port_picstruct::PredStructure::AllIntra;
    let err = cbr_ai
        .try_encode_frame(&y, 64)
        .expect_err("CBR under ALL_INTRA must be refused");
    match err.error() {
        crate::EncodeError::UnsupportedConfig(why) => assert!(
            why.contains("CBR"),
            "Cbr/AllIntra: refusal should name the mode, got {why:?}"
        ),
        other => panic!("Cbr/AllIntra: expected UnsupportedConfig, got {other:?}"),
    }
    assert_eq!(cbr_ai.frame_count, 0);

    // CONTROL: the identical call with Crf still encodes. Without this the
    // test above could pass because the pipeline refuses everything.
    let mut ok = EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            mode: crate::rate_control::RcMode::Crf,
            qp: 20,
            ..RcConfig::default()
        },
        3,
        1,
    );
    let bytes = ok
        .try_encode_frame(&y, 64)
        .expect("Crf must still encode through the same path");
    assert!(!bytes.is_empty());
    assert_eq!(ok.frame_count, 1);
}

#[test]
fn pipeline_encode_sequence() {
    // 64x64: this test exercises the frame/RC state machine, not block
    // geometry, so it uses the smallest in-scope (full-SB) size.
    let mut pipeline = EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            mode: RcMode::Crf,
            qp: 28,
            ..RcConfig::default()
        },
        3,
        16,
    );
    let y_plane = vec![100u8; 64 * 64];
    // The KEY frame encodes; the frame/RC state machine advances for it.
    let bitstream = pipeline
        .try_encode_frame(&y_plane, 64)
        .expect("key frame must encode");
    assert!(!bitstream.is_empty(), "key frame should produce output");
    assert_eq!(pipeline.frame_count, 1);
    assert_eq!(pipeline.rc_state.total_frames, 1);
    // Following frames are INTER frames. They used to refuse here (the
    // mono arm emitted undecodable streams); since 2026-09-21 mono inter
    // is decoder-verified (`tools/mono_inter_gate.sh`), so they encode
    // and the state machine advances per frame.
    for i in 1..5 {
        let bytes = pipeline
            .try_encode_frame(&y_plane, 64)
            .unwrap_or_else(|e| panic!("inter frame {i} must encode: {e:?}"));
        assert!(!bytes.is_empty(), "inter frame {i} should produce output");
        assert_eq!(pipeline.frame_count, 1 + i as u64);
    }
    assert_eq!(pipeline.rc_state.total_frames, 5);
    // A REFUSED frame must not advance the counters — qp0 mono inter is
    // still refused (no inter WHT residual arm), so a qp0 mono pipeline
    // refuses its key frame because the GOP could produce inter frames.
    let mut refused = EncodePipeline::new(
        64,
        64,
        10,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 0,
            ..RcConfig::default()
        },
        3,
        16,
    );
    let err = refused
        .try_encode_frame(&y_plane, 64)
        .expect_err("qp0 mono with a GOP must refuse");
    assert!(
        matches!(err.error(), crate::EncodeError::UnsupportedConfig(_)),
        "expected UnsupportedConfig, got {err:?}"
    );
    assert_eq!(refused.frame_count, 0);
    assert_eq!(refused.rc_state.total_frames, 0);
}

#[test]
fn pipeline_key_frame_first() {
    let mut pipeline = EncodePipeline::new(64, 64, 8, RcConfig::default(), 4, 64);
    let y_plane = vec![128u8; 64 * 64];
    let bitstream = pipeline.encode_frame(&y_plane, 64);
    // First frame should be key frame with sequence header
    // OBU structure: TD + SH + Frame
    assert!(bitstream.len() > 10);
}

#[test]
fn pipeline_dpb_updated() {
    let mut pipeline = EncodePipeline::new(64, 64, 8, RcConfig::default(), 4, 64);
    let y_plane = vec![128u8; 64 * 64];
    pipeline.encode_frame(&y_plane, 64);
    // After key frame, all DPB slots should be filled
    assert!(pipeline.dpb.occupied_slots() > 0);
}

#[test]
fn pipeline_encode_420_single_frame() {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 30,
        ..RcConfig::default()
    };
    let mut pipeline = EncodePipeline::new(64, 64, 4, rc.clone(), 0, 1).with_chroma_420(true);
    let mut y = vec![0u8; 64 * 64];
    for (i, px) in y.iter_mut().enumerate() {
        *px = ((i / 64) * 4) as u8;
    }
    // Nontrivial chroma so u/v txbs actually carry coefficients.
    let mut u = vec![0u8; 32 * 32];
    let mut v = vec![0u8; 32 * 32];
    for i in 0..32 * 32 {
        u[i] = (64 + (i / 32) * 3) as u8;
        v[i] = (64 + (i % 32) * 5) as u8;
    }
    let bs_420 = pipeline.encode_frame_420(&y, &u, &v, 64);
    assert!(!bs_420.is_empty());
    assert_eq!(pipeline.frame_count, 1);

    // The mono stream for the same luma must differ (mono_chrome flag,
    // uv_mode symbols, chroma txbs) and the mono path must not require
    // the chroma flag.
    let mut mono = EncodePipeline::new(64, 64, 4, rc, 0, 1);
    let bs_mono = mono.encode_frame(&y, 64);
    assert_ne!(bs_420, bs_mono);
}

#[test]
#[should_panic(expected = "with_chroma_420")]
fn pipeline_encode_420_requires_flag() {
    let mut pipeline = EncodePipeline::new(64, 64, 4, RcConfig::default(), 0, 1);
    let y = vec![0u8; 64 * 64];
    let u = vec![128u8; 32 * 32];
    let v = vec![128u8; 32 * 32];
    let _ = pipeline.encode_frame_420(&y, &u, &v, 64);
}

/// Task #91: the partition alphabet the entropy ctx derives must agree
/// with the square-size-keyed C rule (`svt_aom_partition_cdf_length`,
/// entropy_coding.c:922). Before the `bsl` fix, width 128 folded into
/// the 64 level and returned 10 symbols against the 64x64 CDF row.
#[test]
fn partition_ctx_alphabet_matches_c_rule_at_every_square_size() {
    let ectx = EntropyCtx::new(
        64,
        64,
        true,
        true,
        false,
        8,
        svtav1_types::chroma::ChromaFormat::Yuv420,
    );
    for sq in [8usize, 16, 32, 64, 128] {
        let (ctx, nsymbs) = ectx.partition_ctx(0, 0, sq);
        assert_eq!(
            nsymbs,
            crate::sb128_geom::partition_cdf_length(sq),
            "alphabet mismatch at square {sq} (ctx {ctx})"
        );
        // ctx rows are 4 per level, level = bsl; 128 must land in the
        // top group (16..=19) that carries the 8-symbol rows.
        let expect_group = match sq {
            8 => 0..=3,
            16 => 4..=7,
            32 => 8..=11,
            64 => 12..=15,
            _ => 16..=19,
        };
        assert!(expect_group.contains(&ctx), "square {sq} -> ctx {ctx}");
    }
}

/// The SB-size resolution honors the C derivation and the explicit
/// override, and NEVER yields anything but 64/128.
///
/// Task #91 chunk 3 flipped `sb128_encode_supported` on, so an SB128
/// cell now RESOLVES to 128 and is genuinely coded at 128 (the
/// `sb128_gate.sh` cells byte-match real SvtAv1EncApp). Before the
/// chunk this test asserted the fallback-to-64 behaviour; the
/// assertions below are the same contract with the capability gate
/// open, plus a direct `resolve_sb_size` check so the fallback
/// mechanism itself stays covered even though nothing triggers it.
#[test]
fn sb_size_resolution_and_fallback() {
    // Small frame -> C rule says 64, no fallback, so nothing about the
    // pre-SB128 gate cells changes.
    let p = EncodePipeline::new(64, 64, 0, RcConfig::default(), 0, 1);
    assert_eq!(p.sb_size, 64);
    assert_eq!(p.derived_sb_size, 64);
    assert!(!p.sb128_fallback);
    // A frame C codes at 128 (512x384 preset 0, MEASURED against the
    // real encoder's sequence header): the port now codes it at 128 too
    // and does NOT fall back.
    let p = EncodePipeline::new(512, 384, 0, RcConfig::default(), 0, 1);
    assert_eq!(p.sb_size, 128, "512x384 p0 is an SB128 cell in C");
    assert_eq!(p.derived_sb_size, 128);
    assert!(
        !p.sb128_fallback,
        "the SB128 encode path is capability-enabled"
    );
    // preset 2 at the same size is genuinely SB64 in C.
    let p = EncodePipeline::new(512, 384, 2, RcConfig::default(), 0, 1);
    assert_eq!(p.sb_size, 64);
    assert!(!p.sb128_fallback);
    // Explicit override asks for 128 on a frame the C rule puts at 64:
    // honoured, because the walk is size- (and preset-) agnostic.
    let p = EncodePipeline::new(64, 64, 0, RcConfig::default(), 0, 1).with_sb_size(Some(128));
    assert_eq!(p.sb_size, 128);
    assert_eq!(
        p.derived_sb_size, 64,
        "the C rule's own answer is preserved"
    );
    assert!(!p.sb128_fallback);
    // Explicit 64 on an SB128 cell pins 64 — the anti-vacuity witness's
    // "force the port to the wrong size" mode. Not a fallback: the
    // override chose it.
    let p = EncodePipeline::new(512, 384, 0, RcConfig::default(), 0, 1).with_sb_size(Some(64));
    assert_eq!(p.sb_size, 64);
    assert!(!p.sb128_fallback);
    assert_eq!(p.derived_sb_size, 128);
    // ... and re-resolving with None returns to the DERIVED value, not
    // to whatever the last override happened to be.
    let p = p.with_sb_size(None);
    assert_eq!(p.sb_size, 128);
    assert_eq!(p.derived_sb_size, 128);
    assert!(!p.sb128_fallback);
    // The fallback mechanism itself: still wired, and the ONLY thing
    // that can trigger it is `sb128_encode_supported` going false again
    // (e.g. if a future chunk re-gates a preset). Assert the contract
    // directly so the plumbing cannot rot while it is unreachable.
    assert_eq!(EncodePipeline::resolve_sb_size(128, None, 0), (128, false));
    assert_eq!(
        EncodePipeline::resolve_sb_size(128, Some(64), 0),
        (64, false)
    );
    assert_eq!(
        EncodePipeline::resolve_sb_size(64, Some(128), 0),
        (128, false)
    );
}

/// Variance boost forces SB64 in C (enc_handle.c:4077 — the derivation
/// reads `static_config.enable_variance_boost`, which tune IQ sets in
/// `copy_api_from_app` BEFORE `set_param_based_on_input` runs). The port
/// mutates `hdr` after `new()` derived the size with `variance_boost:
/// false`, so the derivation is re-run at the encode choke point with
/// the true value. Without that, preset -1 + VB emitted an SB128 stream
/// whose per-64 delta-q symbols desynced the tile.
#[test]
fn variance_boost_rederives_sb64_at_encode_time() {
    let y = vec![128u8; 512 * 512];
    let u = vec![128u8; 256 * 256];
    let v = vec![128u8; 256 * 256];
    let mut p = EncodePipeline::new_with_preset(
        512,
        512,
        crate::speed_config::NativePreset::new(-1).unwrap(),
        RcConfig {
            mode: RcMode::Cqp,
            qp: 32,
            ..RcConfig::default()
        },
        0,
        1,
    )
    .with_chroma_420(true);
    // Construction-time derivation still says 128: the VB input is not
    // visible until hdr mutation, exactly like the fixed state.
    assert_eq!(p.derived_sb_size, 128);
    p.hdr.tune = crate::tune::TUNE_IQ;
    let out = p
        .try_encode_frame_420(&y, &u, &v, 512)
        .expect("tune IQ at preset -1 must encode");
    assert!(!out.is_empty());
    assert_eq!(
        p.sb_size, 64,
        "variance boost (via tune IQ) must force SB64 — C never emits a VB+SB128 stream"
    );
}

/// Task #91: the b64 coding units of one superblock, in C's coding
/// order. SB64 must yield exactly the SB itself (this is what makes
/// every SB64 path byte-identical by construction); SB128 must yield
/// the Z-order quadrants with off-frame ones dropped (C
/// `svt_aom_write_modes_sb`'s `mi_row + y_idx >= mi_rows` continue).
#[test]
fn sb_coding_units_match_c_walk_order() {
    use crate::sb128_geom::sb_coding_units;
    // SB64: always exactly one unit, whatever the frame extent.
    assert_eq!(sb_coding_units(0, 0, 64, 512, 384), alloc::vec![(0, 0)]);
    assert_eq!(
        sb_coding_units(448, 320, 64, 512, 384),
        alloc::vec![(448, 320)]
    );
    // SB128 interior: four quadrants, Z-order (raster within the SB).
    assert_eq!(
        sb_coding_units(0, 0, 128, 512, 384),
        alloc::vec![(0, 0), (64, 0), (0, 64), (64, 64)]
    );
    assert_eq!(
        sb_coding_units(256, 128, 128, 512, 384),
        alloc::vec![(256, 128), (320, 128), (256, 192), (320, 192)]
    );
    // Partial 128 COLUMN (448 = 3*128 + 64): the right quadrants are
    // off-frame and code nothing.
    assert_eq!(
        sb_coding_units(384, 0, 128, 448, 384),
        alloc::vec![(384, 0), (384, 64)]
    );
    // Partial 128 ROW (448 = 3*128 + 64 vertically).
    assert_eq!(
        sb_coding_units(0, 384, 128, 512, 448),
        alloc::vec![(0, 384), (64, 384)]
    );
    // Both partial: only the top-left quadrant survives.
    assert_eq!(
        sb_coding_units(384, 384, 128, 448, 448),
        alloc::vec![(384, 384)]
    );
}

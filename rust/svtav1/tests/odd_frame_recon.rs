//! Partial-edge witnesses found by the animation decoder comparisons.
//! Requires aomdec (AOMDEC may name an explicit executable).
//! Pure-Rust CI explicitly sets ZENAV1_SKIP_DECODER_TESTS for its decoder leg.
use std::{fs, process::Command};
use svtav1_encoder::{
    pipeline::EncodePipeline,
    rate_control::{RcConfig, RcMode},
};

#[test]
fn pristine_and_hybrid_chroma_reference_matches_c() {
    use svtav1::avif::{AvifEncoder, NativePreset, SvtReference};
    // Before the reference-specific presort, both selections emitted the
    // hybrid's 71-byte bd8 p0 stream instead of pristine C's 72-byte stream.
    // The native10 p0 witnesses have equal lengths but different bytes.
    let raw8 = include_bytes!("fixtures/reference_chroma/diag64-8.yuv");
    let raw10: Vec<u16> = include_bytes!("fixtures/reference_chroma/diag64-10.yuv")
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    let cases: [(u8, i8, &[u8], &[u8]); 4] = [
        (
            8,
            -1,
            include_bytes!("fixtures/reference_chroma/diag64-8-p-1-mainline.obu"),
            include_bytes!("fixtures/reference_chroma/diag64-8-p-1-hybrid.obu"),
        ),
        (
            8,
            0,
            include_bytes!("fixtures/reference_chroma/diag64-8-p0-mainline.obu"),
            include_bytes!("fixtures/reference_chroma/diag64-8-p0-hybrid.obu"),
        ),
        (
            10,
            -1,
            include_bytes!("fixtures/reference_chroma/diag64-10-p-1-mainline.obu"),
            include_bytes!("fixtures/reference_chroma/diag64-10-p-1-hybrid.obu"),
        ),
        (
            10,
            0,
            include_bytes!("fixtures/reference_chroma/diag64-10-p0-mainline.obu"),
            include_bytes!("fixtures/reference_chroma/diag64-10-p0-hybrid.obu"),
        ),
    ];
    for (depth, preset, pristine, hybrid) in cases {
        if preset == 0 {
            assert_ne!(pristine, hybrid, "enabled metric witness at depth {depth}");
        }
        for (reference, golden) in [
            (SvtReference::Mainline420, pristine),
            (SvtReference::Hybrid3115, hybrid),
        ] {
            let mut p = EncodePipeline::new_with_preset(
                64,
                64,
                NativePreset::new(preset).unwrap(),
                RcConfig {
                    mode: RcMode::Cqp,
                    qp: 48,
                    ..RcConfig::default()
                },
                0,
                1,
            )
            .with_chroma_420(true)
            .with_bit_depth(depth)
            .with_recon_output(true);
            p.reference = reference;
            let (obu, expected) = if depth == 8 {
                let obu = p
                    .try_encode_frame_420(&raw8[..4096], &raw8[4096..5120], &raw8[5120..], 64)
                    .unwrap();
                (obu, crop(p.last_recon.as_ref().unwrap(), 64, 64, 64))
            } else {
                let obu = p
                    .try_encode_frame_420_hbd(
                        &raw10[..4096],
                        &raw10[4096..5120],
                        &raw10[5120..],
                        64,
                    )
                    .unwrap();
                let recon = crop(p.last_recon10_final.as_ref().unwrap(), 64, 64, 64)
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                (obu, recon)
            };
            assert_eq!(obu, golden, "{reference:?} depth{depth} preset{preset}");
            decode_eq(
                &format!("reference-{reference:?}-{depth}-{preset}"),
                &obu,
                &expected,
            );
            if depth == 8 {
                let encoder = AvifEncoder::new()
                    .with_quality(24.57)
                    .with_native_preset(NativePreset::new(preset).unwrap())
                    .with_reference(reference)
                    .with_color_space(2, 2, 2, false);
                assert_eq!(AvifEncoder::quality_to_qp_static(24.57), 48);
                assert_eq!(encoder.reference(), reference);
                assert_eq!(
                    encoder
                        .encode_yuv420(&raw8[..4096], &raw8[4096..5120], &raw8[5120..], 64, 64, 64)
                        .unwrap()
                        .data,
                    golden
                );
            }
        }
    }
    let gray = [128u8; 64 * 64];
    assert!(
        AvifEncoder::new()
            .with_reference(SvtReference::Mainline420)
            .encode_y8(&gray, 64, 64, 64)
            .is_err()
    );
    let mut incompatible = pipeline(64, 64, 0, 24.57, 8);
    incompatible.reference = SvtReference::Mainline420;
    incompatible.hdr.tx_bias = 1;
    assert!(
        incompatible
            .try_encode_frame_420(&gray, &gray[..1024], &gray[..1024], 64)
            .is_err()
    );
}

#[test]
fn zen_research_intra_edges_match_decoder() {
    use svtav1::avif::{AvifEncoder, NativePreset, SvtReference, ZenEnhancement};
    let raw8 = include_bytes!("fixtures/reference_chroma/diag64-8.yuv");
    let raw10: Vec<u16> = include_bytes!("fixtures/reference_chroma/diag64-10.yuv")
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    for depth in [8, 10] {
        let mut p = EncodePipeline::new_with_preset(
            64,
            64,
            NativePreset::RESEARCH,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 48,
                ..RcConfig::default()
            },
            0,
            1,
        )
        .with_chroma_420(true)
        .with_bit_depth(depth)
        .with_recon_output(true);
        p.reference = SvtReference::Mainline420;
        // A sequence-header flag alone changes bytes. Require an actual pixel
        // effect as well, with the same native -1 configuration and source.
        let native_recon: Vec<u8> = if depth == 8 {
            p.try_encode_frame_420(&raw8[..4096], &raw8[4096..5120], &raw8[5120..], 64)
                .unwrap();
            crop(p.last_recon.as_ref().unwrap(), 64, 64, 64)
        } else {
            p.try_encode_frame_420_hbd(&raw10[..4096], &raw10[4096..5120], &raw10[5120..], 64)
                .unwrap();
            crop(p.last_recon10_final.as_ref().unwrap(), 64, 64, 64)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        p.enhancements = p.enhancements.with(ZenEnhancement::AomIntraEdgeFilter);
        let (obu, expected) = if depth == 8 {
            let obu = p
                .try_encode_frame_420(&raw8[..4096], &raw8[4096..5120], &raw8[5120..], 64)
                .unwrap();
            (obu, crop(p.last_recon.as_ref().unwrap(), 64, 64, 64))
        } else {
            let obu = p
                .try_encode_frame_420_hbd(&raw10[..4096], &raw10[4096..5120], &raw10[5120..], 64)
                .unwrap();
            (
                obu,
                crop(p.last_recon10_final.as_ref().unwrap(), 64, 64, 64)
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
            )
        };
        let native: &[u8] = if depth == 8 {
            include_bytes!("fixtures/reference_chroma/diag64-8-p-1-mainline.obu")
        } else {
            include_bytes!("fixtures/reference_chroma/diag64-10-p-1-mainline.obu")
        };
        assert_ne!(obu, native, "experiment must change the encoded policy");
        assert_ne!(
            expected, native_recon,
            "intra-edge filtering must affect pixels at depth {depth}"
        );
        decode_eq(&format!("zen-intra-edge-{depth}"), &obu, &expected);
        if depth == 8 {
            let enc = AvifEncoder::new()
                .with_quality(24.57)
                .with_native_preset(NativePreset::RESEARCH)
                .with_reference(SvtReference::Mainline420)
                .with_color_space(2, 2, 2, false)
                .with_enhancement(ZenEnhancement::AomIntraEdgeFilter);
            assert!(
                enc.enhancements()
                    .contains(ZenEnhancement::AomIntraEdgeFilter)
            );
            assert_eq!(
                enc.encode_yuv420(&raw8[..4096], &raw8[4096..5120], &raw8[5120..], 64, 64, 64)
                    .unwrap()
                    .data,
                obu
            );
            assert!(enc.with_speed(1).validate_configuration().is_err());
        }
    }
}

#[test]
fn zen_intra_edges_at_partial_frames_and_tiles() {
    use svtav1::avif::{NativePreset, SvtReference, ZenEnhancement};
    use svtav1_encoder::entropy::obu::TileGrid;
    for enabled in [false, true] {
        for (w, h, rows, cols) in [(65usize, 67usize, 0, 0), (128, 128, 1, 0), (128, 128, 0, 1)] {
            for depth in [8, 10] {
                for qp in [0, 20, 48] {
                    let mut p = EncodePipeline::new_with_preset(
                        w as u32,
                        h as u32,
                        NativePreset::RESEARCH,
                        RcConfig {
                            mode: RcMode::Cqp,
                            qp,
                            ..Default::default()
                        },
                        0,
                        1,
                    )
                    .with_bit_depth(depth)
                    .with_chroma_420(true)
                    .with_recon_output(true)
                    .with_tile_rows_log2(rows)
                    .with_tile_cols_log2(cols);
                    p.reference = SvtReference::Mainline420;
                    if enabled {
                        p.enhancements = p.enhancements.with(ZenEnhancement::AomIntraEdgeFilter);
                    }
                    let grid = TileGrid::resolve(p.width, p.height, p.sb_size as u32, rows, cols);
                    assert_eq!(
                        (grid.tile_rows_log2, grid.tile_cols_log2),
                        (rows, cols),
                        "tile witness must not clamp away"
                    );
                    let y: Vec<u8> = (0..w * h)
                        .map(|i| {
                            ((i % w * 3 + i / w * 5 + (i % w > i / w) as usize * 64) % 256) as u8
                        })
                        .collect();
                    let c = w.div_ceil(2) * h.div_ceil(2);
                    let u: Vec<u8> = (0..c).map(|i| (80 + i * 7 % 90) as u8).collect();
                    let v: Vec<u8> = (0..c).map(|i| (90 + i * 11 % 100) as u8).collect();
                    let (obu, expected) = if depth == 8 {
                        let obu = p.try_encode_frame_420(&y, &u, &v, w).unwrap();
                        (
                            obu,
                            crop(p.last_recon.as_ref().unwrap(), p.width as usize, w, h),
                        )
                    } else {
                        let native = |s: &[u8]| {
                            s.iter()
                                .enumerate()
                                .map(|(i, &v)| (u16::from(v) << 2) + (i % 4) as u16)
                                .collect::<Vec<_>>()
                        };
                        let obu = p
                            .try_encode_frame_420_hbd(&native(&y), &native(&u), &native(&v), w)
                            .unwrap();
                        let recon = crop(
                            p.last_recon10_final.as_ref().unwrap(),
                            p.width as usize,
                            w,
                            h,
                        )
                        .into_iter()
                        .flat_map(u16::to_le_bytes)
                        .collect();
                        (obu, recon)
                    };
                    decode_eq(
                        &format!(
                            "zen-edge-enabled{enabled}-{w}x{h}-d{depth}-q{qp}-tiles{rows}-{cols}"
                        ),
                        &obu,
                        &expected,
                    );
                }
            }
        }
    }
}

#[test]
fn zen_restoration_unit_search_reconstructs_at_unit_and_tile_boundaries() {
    use svtav1::avif::{NativePreset, SvtReference, ZenEnhancement};
    let mut selected_smaller = [false; 2];
    for (depth_index, depth) in [8, 10].into_iter().enumerate() {
        for (w, h, rows, cols, sb) in [
            (192usize, 160usize, 0, 0, 64),
            (193, 161, 0, 0, 64),
            (192, 160, 1, 0, 64),
            (192, 160, 0, 1, 64),
            (256, 256, 0, 0, 128),
        ] {
            let mut p = EncodePipeline::new_with_preset(
                w as u32,
                h as u32,
                NativePreset::RESEARCH,
                RcConfig {
                    mode: RcMode::Cqp,
                    qp: 32,
                    ..Default::default()
                },
                0,
                1,
            )
            .with_chroma_420(true)
            .with_bit_depth(depth)
            .with_recon_output(true)
            .with_tile_rows_log2(rows)
            .with_tile_cols_log2(cols)
            .with_sb_size(Some(sb));
            assert_eq!(p.sb_size, sb);
            let grid = svtav1_encoder::entropy::obu::TileGrid::resolve(
                p.width,
                p.height,
                p.sb_size as u32,
                rows,
                cols,
            );
            assert_eq!((grid.tile_rows_log2, grid.tile_cols_log2), (rows, cols));
            p.reference = SvtReference::Mainline420;
            p.hdr.screen_content_mode = Some(0);
            p.enhancements = p
                .enhancements
                .with(ZenEnhancement::AomRestorationUnitSearch);
            // Correctness witness with spatially varying texture: smaller units
            // must actually win, restore pixels, and survive independent decode.
            let plane = |pw: usize, ph: usize, seed: usize| -> Vec<u8> {
                (0..pw * ph)
                    .map(|i| {
                        let (x, y) = (i % pw, i / pw);
                        let base = 60 + (x * 3 + y * 2 + seed) % 128;
                        let noise = if x > pw / 2 {
                            (x * 71 + y * 113 + x * y * 7 + seed) % 37
                        } else {
                            0
                        };
                        (base + noise) as u8
                    })
                    .collect()
            };
            let y = plane(w, h, 0);
            let u = plane(w.div_ceil(2), h.div_ceil(2), 23);
            let v = plane(w.div_ceil(2), h.div_ceil(2), 71);
            let (obu, expected) = if depth == 8 {
                let obu = p.try_encode_frame_420(&y, &u, &v, w).unwrap();
                (
                    obu,
                    crop(p.last_recon.as_ref().unwrap(), p.width as usize, w, h),
                )
            } else {
                let native = |s: &[u8]| {
                    s.iter()
                        .enumerate()
                        .map(|(i, &v)| (u16::from(v) << 2) + (i % 4) as u16)
                        .collect::<Vec<_>>()
                };
                let obu = p
                    .try_encode_frame_420_hbd(&native(&y), &native(&u), &native(&v), w)
                    .unwrap();
                (
                    obu,
                    crop(
                        p.last_recon10_final.as_ref().unwrap(),
                        p.width as usize,
                        w,
                        h,
                    )
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
                )
            };
            let size = p.last_lr_unit_size.expect("restoration search must run");
            assert!(size >= p.sb_size && [64, 128, 256].contains(&size));
            selected_smaller[depth_index] |= size < 256 && p.last_lr_stats.0 != [0; 3];
            eprintln!(
                "Zen LR: {w}x{h} depth={depth} tiles={rows},{cols} unit={size} types={:?}",
                p.last_lr_stats.0
            );
            decode_eq(
                &format!("zen-lr-{w}x{h}-d{depth}-tiles{rows}-{cols}"),
                &obu,
                &expected,
            );
        }
    }
    assert_eq!(
        selected_smaller, [true; 2],
        "must enable smaller restoration units at both native depths"
    );
}

fn pipeline(w: usize, h: usize, preset: u8, quality: f32, depth: u8) -> EncodePipeline {
    EncodePipeline::new(
        w as u32,
        h as u32,
        preset,
        RcConfig {
            mode: RcMode::Cqp,
            qp: svtav1::avif::AvifEncoder::quality_to_qp_static(quality),
            ..RcConfig::default()
        },
        0,
        1,
    )
    .with_bit_depth(depth)
    .with_chroma_420(true)
    .with_recon_output(true)
}

fn crop<T: Copy>(planes: &(Vec<T>, Vec<T>, Vec<T>), stride: usize, w: usize, h: usize) -> Vec<T> {
    let mut out = Vec::new();
    for (p, s, cols, rows) in [
        (&planes.0, stride, w, h),
        (&planes.1, stride.div_ceil(2), w.div_ceil(2), h.div_ceil(2)),
        (&planes.2, stride.div_ceil(2), w.div_ceil(2), h.div_ceil(2)),
    ] {
        for r in 0..rows {
            out.extend_from_slice(&p[r * s..r * s + cols]);
        }
    }
    out
}

fn decode_eq(name: &str, obu: &[u8], expected: &[u8]) {
    if std::env::var_os("ZENAV1_SKIP_DECODER_TESTS").is_some() {
        eprintln!("{name}: decoder comparison explicitly skipped by ZENAV1_SKIP_DECODER_TESTS");
        return;
    }
    let dir = std::env::temp_dir().join(format!("odd-recon-{}-{name}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("frame.obu"), obu).unwrap();
    let output = Command::new(std::env::var_os("AOMDEC").unwrap_or_else(|| "aomdec".into()))
        .args(["--rawvideo", "--output-bit-depth=0", "-o"])
        .arg(dir.join("decoded.yuv"))
        .arg(dir.join("frame.obu"))
        .output()
        .expect("aomdec is required");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decoded = fs::read(dir.join("decoded.yuv")).unwrap();
    if decoded != expected {
        fs::write(dir.join("expected.yuv"), expected).unwrap();
    }
    assert_eq!(decoded.len(), expected.len());
    assert!(
        decoded == expected,
        "{name}: first differing byte {:?}; artifacts {}",
        decoded.iter().zip(expected).position(|(a, b)| a != b),
        dir.display()
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cached_chroma_at_partial_right_edge() {
    // Before the clipped copy: 656 unfiltered chroma bytes differed from dav1d.
    let (w, h) = (65usize, 67usize);
    let y: Vec<u8> = (0..w * h).map(|i| (32 + i % w % 180) as u8).collect();
    let uv: Vec<u8> = (0..w.div_ceil(2) * h.div_ceil(2))
        .map(|i| (100 + i * 7 % 60) as u8)
        .collect();
    let mut p = pipeline(w, h, 1, 75.0, 8);
    let obu = p.try_encode_frame_420(&y, &uv, &uv, w).unwrap();
    let expected = crop(p.last_recon.as_ref().unwrap(), p.width as usize, w, h);
    let mut no_recon = pipeline(w, h, 1, 75.0, 8).with_recon_output(false);
    assert_eq!(obu, no_recon.try_encode_frame_420(&y, &uv, &uv, w).unwrap());
    decode_eq("cached-chroma", &obu, &expected);
}

#[test]
fn native_odd_chroma_filter_bounds() {
    // Before normative output bounds: 277 filtered bytes differed from dav1d;
    // the native unfiltered reconstruction was already exact.
    let w = 65usize;
    let stride = w + 5;
    let y: Vec<u16> = (0..stride * w)
        .map(|i| (100 + i * 7 % 750) as u16)
        .collect();
    let u: Vec<u16> = (0..w.div_ceil(2).pow(2))
        .map(|i| (480 + i % 64) as u16)
        .collect();
    let v: Vec<u16> = (0..w.div_ceil(2).pow(2))
        .map(|i| (500 + i % 32) as u16)
        .collect();
    let mut p = pipeline(w, w, 9, 40.0, 10);
    let obu = p.try_encode_frame_420_hbd(&y, &u, &v, stride).unwrap();
    let expected: Vec<u8> = crop(
        p.last_recon10_final.as_ref().unwrap(),
        p.width as usize,
        w,
        w,
    )
    .iter()
    .flat_map(|v| v.to_le_bytes())
    .collect();
    let mut no_recon = pipeline(w, w, 9, 40.0, 10).with_recon_output(false);
    assert_eq!(
        obu,
        no_recon
            .try_encode_frame_420_hbd(&y, &u, &v, stride)
            .unwrap()
    );
    decode_eq("native-chroma", &obu, &expected);
}

#[test]
fn superresolution_odd_chroma_reconstruction() {
    for w in [65usize, 66, 72] {
        for h in [65usize, 67, 72] {
            let y: Vec<u8> = (0..w * h).map(|i| (32 + (i * 7) % 180) as u8).collect();
            let uv: Vec<u8> = (0..w.div_ceil(2) * h.div_ceil(2))
                .map(|i| (100 + i * 7 % 60) as u8)
                .collect();
            for denom in [9, 12, 16] {
                for preset in [7, 9] {
                    for quality in [5.0, 40.0, 75.0, 98.0] {
                        let mut p = pipeline(w, h, preset, quality, 8).with_superres(denom);
                        let obu = p.try_encode_frame_420(&y, &uv, &uv, w).unwrap();
                        let expected = crop(p.last_recon.as_ref().unwrap(), w, w, h);
                        let mut no_recon = pipeline(w, h, preset, quality, 8)
                            .with_superres(denom)
                            .with_recon_output(false);
                        assert_eq!(obu, no_recon.try_encode_frame_420(&y, &uv, &uv, w).unwrap());
                        decode_eq(
                            &format!("superres-{w}x{h}-{denom}-p{preset}-q{quality}"),
                            &obu,
                            &expected,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn monochrome_low_presets_partial_blocks() {
    for (w, h) in [
        (65usize, 67usize),
        (66, 66),
        (64, 80),
        (80, 64),
        (72, 88),
        (96, 80),
        (80, 96),
        (127, 129),
    ] {
        for preset in 0..=5 {
            for quality in [40.0, 75.0, 98.0] {
                let stride = w + 5;
                let y: Vec<u8> = (0..stride * h)
                    .map(|i| (32 + (i * 7 + i / stride * 13) % 180) as u8)
                    .collect();
                let mut p = pipeline(w, h, preset, quality, 8).with_chroma_420(false);
                let obu = p.try_encode_frame(&y, stride).unwrap();
                let expected: Vec<u8> = p
                    .last_recon
                    .as_ref()
                    .unwrap()
                    .0
                    .chunks_exact(p.width as usize)
                    .take(h)
                    .flat_map(|row| row[..w].iter().copied())
                    .collect();
                let mut no_recon = pipeline(w, h, preset, quality, 8)
                    .with_chroma_420(false)
                    .with_recon_output(false);
                assert_eq!(obu, no_recon.try_encode_frame(&y, stride).unwrap());
                decode_eq(
                    &format!("mono-p{preset}-{w}x{h}-q{quality}"),
                    &obu,
                    &expected,
                );
            }
        }
    }
}

#[test]
fn native_monochrome_low_presets_match_decoder() {
    for (w, h, tiles) in [
        (64usize, 64usize, 0),
        (65, 67, 0),
        (64, 80, 0),
        (127, 129, 0),
        (128, 128, 1),
        (129, 131, 1),
    ] {
        for preset in 0..=8 {
            for quality in [40.0, 98.0] {
                let stride = w + 5;
                let y: Vec<u16> = (0..stride * h)
                    .map(|i| (100 + (i * 7 + i / stride * 13) % 750) as u16)
                    .collect();
                let mut p = pipeline(w, h, preset, quality, 10)
                    .with_chroma_420(false)
                    .with_tile_rows_log2(tiles)
                    .with_tile_cols_log2(tiles);
                let obu = p.try_encode_frame_hbd(&y, stride).unwrap();
                let expected: Vec<u8> = p
                    .last_recon10_final
                    .as_ref()
                    .unwrap()
                    .0
                    .chunks_exact(p.width as usize)
                    .take(h)
                    .flat_map(|row| row[..w].iter().flat_map(|v| v.to_le_bytes()))
                    .collect();
                decode_eq(
                    &format!("native-mono-p{preset}-{w}x{h}-q{quality}-tiles{tiles}"),
                    &obu,
                    &expected,
                );
                let mut no_recon = pipeline(w, h, preset, quality, 10)
                    .with_chroma_420(false)
                    .with_tile_rows_log2(tiles)
                    .with_tile_cols_log2(tiles)
                    .with_recon_output(false);
                assert_eq!(obu, no_recon.try_encode_frame_hbd(&y, stride).unwrap());
                let msb: Vec<u16> = y.iter().map(|v| v & !3).collect();
                let mut truncated = pipeline(w, h, preset, quality, 10)
                    .with_chroma_420(false)
                    .with_tile_rows_log2(tiles)
                    .with_tile_cols_log2(tiles);
                assert_ne!(
                    obu,
                    truncated.try_encode_frame_hbd(&msb, stride).unwrap(),
                    "native low bits must change coded output"
                );
            }
        }
    }
}

#[test]
fn lossless_quantization_options_match_source() {
    for (qm_enabled, variance_boost) in [(true, false), (false, true), (true, true)] {
        // C and the port both applied nonidentity matrices at QP0, even though
        // the decoder ignores them in a lossless segment. Their identical bytes
        // decoded to wrong samples at both depths (gradient64, preset 7).
        for depth in [8, 10] {
            for mono in [false, true] {
                for (w, h) in [(16usize, 16usize), (65, 67), (128, 128)] {
                    for preset in [0, 4, 9, 13] {
                        let stride = w + 3;
                        let max = (1usize << depth) - 1;
                        let y: Vec<u16> = (0..stride * h)
                            .map(|i| ((i * 37 + i / stride * 71) & max) as u16)
                            .collect();
                        let u: Vec<u16> = (0..w.div_ceil(2) * h.div_ceil(2))
                            .map(|i| ((i * 17 + 71) & max) as u16)
                            .collect();
                        let v: Vec<u16> = u.iter().map(|v| max as u16 - v).collect();
                        let make = || pipeline(w, h, preset, 100.0, depth).with_chroma_420(!mono);
                        let encode = |p: &mut EncodePipeline| {
                            if depth == 10 {
                                if mono {
                                    p.try_encode_frame_hbd(&y, stride).unwrap()
                                } else {
                                    p.try_encode_frame_420_hbd(&y, &u, &v, stride).unwrap()
                                }
                            } else {
                                let y8: Vec<u8> = y.iter().map(|v| *v as u8).collect();
                                let u8: Vec<u8> = u.iter().map(|v| *v as u8).collect();
                                let v8: Vec<u8> = v.iter().map(|v| *v as u8).collect();
                                if mono {
                                    p.try_encode_frame(&y8, stride).unwrap()
                                } else {
                                    p.try_encode_frame_420(&y8, &u8, &v8, stride).unwrap()
                                }
                            }
                        };
                        let mut qm = make();
                        qm.hdr.enable_qm = qm_enabled;
                        qm.hdr.enable_variance_boost = variance_boost;
                        let bytes = encode(&mut qm);
                        let mut source: Vec<u16> = y
                            .chunks_exact(stride)
                            .flat_map(|row| row[..w].iter().copied())
                            .collect();
                        if !mono {
                            source.extend_from_slice(&u);
                            source.extend_from_slice(&v);
                        }
                        let expected: Vec<u8> = if depth == 8 {
                            source.into_iter().map(|v| v as u8).collect()
                        } else {
                            source.into_iter().flat_map(|v| v.to_le_bytes()).collect()
                        };
                        decode_eq(
                            &format!(
                                "lossless-qm{qm_enabled}-vb{variance_boost}-{w}x{h}-p{preset}-bd{depth}-mono{mono}"
                            ),
                            &bytes,
                            &expected,
                        );
                        assert_eq!(
                            bytes,
                            encode(&mut make()),
                            "lossless quantization options must preserve identity"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn native_lossless_matches_source() {
    // Before the native WHT wiring, QP 0 refused. Bypassing that refusal
    // alone produced wrong pixels at preset 9 (gradient 64x64: 2699 B).
    for (w, h, tiles) in [(16usize, 16usize, 0), (65, 67, 0), (129, 131, 1)] {
        for preset in 0..=13 {
            for mono in [false, true] {
                for kind in 0..3 {
                    let stride = w + 5;
                    let sample = |i: usize, row_stride: usize| match kind {
                        0 => 512 + ((i + i / row_stride) % 4) as u16,
                        1 => {
                            if (i + i / row_stride).is_multiple_of(2) {
                                0
                            } else {
                                1023
                            }
                        }
                        _ => ((i * 37 + i / row_stride * 71) % 1024) as u16,
                    };
                    let y: Vec<u16> = (0..stride * h).map(|i| sample(i, stride)).collect();
                    let cw = w.div_ceil(2);
                    let u: Vec<u16> = (0..cw * h.div_ceil(2)).map(|i| sample(i + 7, cw)).collect();
                    let v: Vec<u16> = (0..u.len()).map(|i| sample(i + 13, cw)).collect();
                    let make = || {
                        pipeline(w, h, preset, 100.0, 10)
                            .with_chroma_420(!mono)
                            .with_tile_rows_log2(tiles)
                            .with_tile_cols_log2(tiles)
                    };
                    let encode = |p: &mut EncodePipeline| {
                        if mono {
                            p.try_encode_frame_hbd(&y, stride).unwrap()
                        } else {
                            p.try_encode_frame_420_hbd(&y, &u, &v, stride).unwrap()
                        }
                    };
                    let mut p = make();
                    let bytes = encode(&mut p);
                    assert_eq!(bytes, encode(&mut make().with_recon_output(false)));
                    let mut source: Vec<u16> = y
                        .chunks_exact(stride)
                        .flat_map(|row| row[..w].iter().copied())
                        .collect();
                    if !mono {
                        source.extend_from_slice(&u);
                        source.extend_from_slice(&v);
                    }
                    let expected: Vec<u8> = source.iter().flat_map(|v| v.to_le_bytes()).collect();
                    decode_eq(
                        &format!("native-lossless-{w}x{h}-p{preset}-mono{mono}-kind{kind}"),
                        &bytes,
                        &expected,
                    );
                }
            }
        }
    }
}

#[test]
fn lossless_monochrome_matches_source() {
    for (w, h, tiles) in [(16usize, 16usize, 0), (65, 67, 0), (128, 128, 1)] {
        for preset in 0..=9 {
            for kind in 0..3 {
                let stride = w + 5;
                let y: Vec<u8> = (0..stride * h)
                    .map(|i| match kind {
                        0 => 128,
                        1 => {
                            if (i + i / stride) % 2 == 0 {
                                0
                            } else {
                                255
                            }
                        }
                        _ => (i * 37 + i / stride * 71) as u8,
                    })
                    .collect();
                let mut p = pipeline(w, h, preset, 100.0, 8)
                    .with_chroma_420(false)
                    .with_tile_rows_log2(tiles)
                    .with_tile_cols_log2(tiles);
                let bytes = p.try_encode_frame(&y, stride).unwrap();
                let mut no_recon = pipeline(w, h, preset, 100.0, 8)
                    .with_chroma_420(false)
                    .with_tile_rows_log2(tiles)
                    .with_tile_cols_log2(tiles)
                    .with_recon_output(false);
                assert_eq!(bytes, no_recon.try_encode_frame(&y, stride).unwrap());
                let source: Vec<u8> = y
                    .chunks_exact(stride)
                    .flat_map(|r| r[..w].iter().copied())
                    .collect();
                let recon: Vec<u8> = p
                    .last_recon
                    .as_ref()
                    .unwrap()
                    .0
                    .chunks_exact(p.width as usize)
                    .take(h)
                    .flat_map(|r| r[..w].iter().copied())
                    .collect();
                assert_eq!(source, recon, "lossless internal reconstruction");
                decode_eq(
                    &format!("lossless-mono-{w}x{h}-p{preset}-kind{kind}"),
                    &bytes,
                    &source,
                );
            }
        }
    }
}

/// Exercise the public native-preset selector through a partial-frame encode
/// and independent decoder. The wrapper's u8 input is widened at depth 10;
/// native u16 source precision is covered by the pipeline witnesses above.
#[test]
fn research_wrapper_partial_frame_matches_decoder() {
    use svtav1::avif::{AvifEncoder, NativePreset};
    let (w, h) = (65usize, 67usize);
    let y: Vec<u8> = (0..w * h)
        .map(|i| ((i * 17 + i / w * 7) % 256) as u8)
        .collect();
    let u = vec![117u8; w.div_ceil(2) * h.div_ceil(2)];
    let v = vec![139u8; u.len()];
    for depth in [8, 10] {
        let enc = AvifEncoder::new()
            .with_quality(40.0)
            .with_bit_depth(depth)
            .with_native_preset(NativePreset::RESEARCH);
        let obu = enc
            .encode_yuv420(&y, &u, &v, w as u32, h as u32, w as u32)
            .unwrap()
            .data;
        let mut direct = EncodePipeline::new_with_preset(
            w as u32,
            h as u32,
            NativePreset::RESEARCH,
            RcConfig {
                mode: RcMode::Cqp,
                qp: AvifEncoder::quality_to_qp_static(40.0),
                ..Default::default()
            },
            0,
            1,
        )
        .with_bit_depth(depth)
        .with_chroma_420(true)
        .with_recon_output(true);
        direct.color_description = svtav1::encoder::entropy::obu::ColorDescription {
            color_primaries: 1,
            transfer_characteristics: 13,
            matrix_coefficients: 1,
            full_range: false,
        };
        assert_eq!(obu, direct.try_encode_frame_420(&y, &u, &v, w).unwrap());
        let expected = if depth == 8 {
            crop(
                direct.last_recon.as_ref().unwrap(),
                direct.width as usize,
                w,
                h,
            )
        } else {
            crop(
                direct.last_recon10_final.as_ref().unwrap(),
                direct.width as usize,
                w,
                h,
            )
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect()
        };
        decode_eq(&format!("research-wrapper-{depth}"), &obu, &expected);
    }
}

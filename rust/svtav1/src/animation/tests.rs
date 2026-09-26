use super::*;
use std::{fs, process::Command};

/// Where the public-domain video assets live, probing the same way
/// `tools/lib_corpus.sh` and `tests/tier_invariance.rs` do rather than
/// assuming one host. Returns the resolved path or panics with the list.
fn pd_video_asset(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(dir) = std::env::var("ZENAV1_VIDEO_ASSETS") {
        candidates.push(format!("{dir}/{name}"));
    }
    if let Ok(root) = std::env::var("ZENAV1_CORPUS_ROOT") {
        candidates.push(format!("{root}/video/pd-derf-720p/{name}"));
    }
    candidates.push(format!("{home}/work/zen/video/pd-derf-720p/{name}"));
    candidates.push(format!("/root/work/video/pd-derf-720p/{name}"));
    candidates
        .iter()
        .find(|c| std::path::Path::new(c).exists())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "public-domain video asset {name} not found. Probed:\n  {}\n\
                     Fetch it with `tools/fetch_r2_assets.sh video/pd-derf-720p/ <dir>` \
                     and point ZENAV1_VIDEO_ASSETS at <dir>, or set \
                     ZENAV1_SKIP_CORPUS_TESTS=1 to skip DELIBERATELY. It is not \
                     skipped by default because every other animation test in \
                     this file synthesises its frames, so this is the only real \
                     motion the animation path ever sees.",
                candidates.join("\n  ")
            )
        })
}

/// Animated AVIF over REAL motion, not a procedural gradient.
///
/// Every other test in this module builds its frames from an arithmetic
/// expression over the pixel index. Those frames are smooth, noiseless and
/// move by a fixed offset, which is exactly the content an encoder finds
/// easiest — the same blind spot the inter gates had until
/// `tools/real_video_inter_gate.sh` (`docs/IDENTITY-STATUS.md`). This one
/// muxes three consecutive frames of a public-domain 720p clip.
///
/// The load-bearing assertion is the LAST one: the decoded frames must
/// differ from each other. A sequence that round-trips to three identical
/// images would satisfy the frame count, the timing and the dimensions
/// while proving nothing about the animation path, and that is not a
/// hypothetical — the source clips are 30 fps material published at 60, so
/// consecutive frames of the undecimated original ARE byte-identical and
/// both encoders code the second one to 24 bytes.
#[test]
fn three_frame_real_video_sequence_round_trips_with_motion_intact() {
    if std::env::var_os("ZENAV1_SKIP_CORPUS_TESTS").is_some() {
        eprintln!(
            "animation: SKIPPED by ZENAV1_SKIP_CORPUS_TESTS — the real-video \
                 animation round trip did NOT run in this invocation"
        );
        return;
    }
    let (w, h) = (128usize, 128usize);
    let (cw, ch) = (w / 2, h / 2);
    let frame_len = w * h + 2 * cw * ch;
    let path = pd_video_asset("vidyo3_128x128_8f.i420");
    let bytes = fs::read(&path).unwrap();
    assert!(
        bytes.len() >= 3 * frame_len,
        "{}: {} bytes, need at least 3 frames of {frame_len}",
        path.display(),
        bytes.len()
    );
    let plane = |f: usize, off: usize, len: usize| {
        bytes[f * frame_len + off..f * frame_len + off + len].to_vec()
    };
    let ys: Vec<Vec<u8>> = (0..3).map(|f| plane(f, 0, w * h)).collect();
    let us: Vec<Vec<u8>> = (0..3).map(|f| plane(f, w * h, cw * ch)).collect();
    let vs: Vec<Vec<u8>> = (0..3).map(|f| plane(f, w * h + cw * ch, cw * ch)).collect();
    for i in 1..3 {
        assert_ne!(
            ys[i - 1],
            ys[i],
            "source frames {} and {i} are identical — this asset cannot \
                 exercise the animation path (regenerate with \
                 tools/mk_video_assets.py, which decimates duplicated frames)",
            i - 1
        );
    }

    let out = std::env::temp_dir().join(format!("svt-animation-real-{}", std::process::id()));
    fs::create_dir_all(&out).unwrap();
    let enc = AvifEncoder::new().with_speed(7);
    let frames: Vec<_> = (0..3)
        .map(|f| AnimationFrame {
            y: &ys[f],
            u: &us[f],
            v: &vs[f],
            y_stride: w,
            alpha: None,
            duration: [100, 200, 300][f],
        })
        .collect();
    let avif = enc
        .encode_animation_yuv420(
            &frames,
            w as u32,
            h as u32,
            AnimationTiming { timescale: 1000 },
        )
        .unwrap();
    fs::write(out.join("real.avif"), &avif).unwrap();

    let result = Command::new("avifdec")
        .args(["-j", "1", "--index", "all"])
        .arg(out.join("real.avif"))
        .arg(out.join("frame.png"))
        .output()
        .expect("avifdec required for animated AVIF conformance");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let info = String::from_utf8_lossy(&result.stdout);
    assert!(info.contains("3 frames"), "{info}");
    for duration in [100, 200, 300] {
        assert!(info.contains(&format!("({duration} timescales)")), "{info}");
    }

    let mut decoded: Vec<Vec<u8>> = Vec::new();
    for i in 0..3 {
        let file = fs::File::open(out.join(format!("frame-{i:010}.png"))).unwrap();
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let image = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((image.width, image.height), (w as u32, h as u32));
        pixels.truncate(image.buffer_size());
        decoded.push(pixels);
    }
    for i in 1..3 {
        assert_ne!(
            decoded[i - 1],
            decoded[i],
            "decoded frames {} and {i} are identical — the animation \
                 round-tripped real motion into a still",
            i - 1
        );
    }
}

#[test]
fn three_frame_alpha_sequence_decodes_with_exact_timing_and_alpha() {
    for (w, h, has_alpha, speed, lossless, screen) in [
        (64usize, 64usize, true, 7, false, false),
        (65, 67, true, 7, false, false),
        (64, 64, false, 7, false, false),
        (65, 67, false, 2, false, false),
        (64, 64, true, 7, true, false),
        (65, 67, true, 7, true, false),
        (65, 67, false, 7, true, false),
        // Lower-preset lossless screen content, including the fixed-tree
        // p4 IntraBC source path and an odd-sized animation with alpha.
        (64, 64, true, 2, true, true),
        (128, 128, true, 3, true, true),
        (65, 67, true, 2, true, true),
    ] {
        let out =
            std::env::temp_dir().join(format!("svt-animation-{}-{w}x{h}", std::process::id()));
        fs::create_dir_all(&out).unwrap();
        let enc = AvifEncoder::new().with_speed(speed).with_lossless(lossless);
        let colors: Vec<Vec<u8>> = (0..3)
            .map(|f| {
                (0..w * h)
                    .map(|i| {
                        if screen {
                            let (r, c) = (i / w, i % w);
                            let panel = ((r / 24) & 1) * 2 + ((c / 32 + f) & 1);
                            let text = (6..12).contains(&(r % 24)) && (c / 3 + r / 24 + f) % 5 != 0;
                            if text { 16 } else { [35, 110, 180, 235][panel] }
                        } else {
                            (32 + (i % w + f * 30) % 180) as u8
                        }
                    })
                    .collect()
            })
            .collect();
        let alphas: Vec<Vec<u8>> = (0..3)
            .map(|f| (0..w * h).map(|i| ((i + f * 13) % 256) as u8).collect())
            .collect();
        let uv: Vec<u8> = (0..w.div_ceil(2) * h.div_ceil(2))
            .map(|i| (100 + (i * 7) % 60) as u8)
            .collect();
        let frames: Vec<_> = (0..3)
            .map(|f| AnimationFrame {
                y: &colors[f],
                u: &uv,
                v: &uv,
                y_stride: w,
                alpha: has_alpha.then_some(alphas[f].as_slice()),
                duration: [100, 200, 300][f],
            })
            .collect();
        let avif = enc
            .encode_animation_yuv420(
                &frames,
                w as u32,
                h as u32,
                AnimationTiming { timescale: 1000 },
            )
            .unwrap_or_else(|e| {
                panic!("{w}x{h} speed {speed} lossless {lossless} screen {screen}: {e}")
            });
        fs::write(out.join("sequence.avif"), avif).unwrap();
        let result = Command::new("avifdec")
            .args(["-j", "1", "--index", "all"])
            .arg(out.join("sequence.avif"))
            .arg(out.join("frame.png"))
            .output()
            .expect("avifdec required for animated AVIF conformance");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let info = String::from_utf8_lossy(&result.stdout);
        assert!(info.contains("3 frames"), "{info}");
        assert!(info.contains("600 timescales"), "{info}");
        for duration in [100, 200, 300] {
            assert!(info.contains(&format!("({duration} timescales)")), "{info}");
        }
        let mut alpha_settings = enc.clone();
        alpha_settings.full_range = true;
        for (i, alpha) in alphas.iter().enumerate() {
            if !has_alpha {
                continue;
            }
            let mut pipe = alpha_settings
                .build_pipeline(w as u32, h as u32)
                .with_recon_output(true);
            pipe.try_encode_frame(alpha, w).unwrap();
            let (reference, _, _) = pipe.last_recon.unwrap();
            let file = fs::File::open(out.join(format!("frame-{i:010}.png"))).unwrap();
            let mut reader = png::Decoder::new(std::io::BufReader::new(file))
                .read_info()
                .unwrap();
            let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
            let image = reader.next_frame(&mut pixels).unwrap();
            assert_eq!(image.color_type, png::ColorType::Rgba);
            assert_eq!((image.width, image.height), (w as u32, h as u32));
            for y in 0..h {
                for x in 0..w {
                    if lossless {
                        assert_eq!(pixels[(y * w + x) * 4 + 3], alpha[y * w + x]);
                    }
                    assert_eq!(
                        pixels[(y * w + x) * 4 + 3],
                        reference[y * pipe.width as usize + x],
                        "frame {i} alpha ({x},{y})"
                    );
                }
            }
        }
        let result = Command::new("avifdec")
            .args(["-j", "1", "--index", "all"])
            .arg(out.join("sequence.avif"))
            .arg(out.join("frame.y4m"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        // ONE pipeline for all three frames, with the SAME GOP the
        // animation used. A fresh `build_pipeline` per frame would code
        // each as a standalone still, and a still is not what an
        // inter-coded sequence contains -- not even at frame 0, because
        // `intra_period > 1` selects C's video signal derivation rather
        // than its all-intra one.
        let mut pipe = enc
            .build_pipeline_gop(
                w as u32,
                h as u32,
                enc.animation_keyframes(&AnimationOptions::default())
                    .intra_period(),
            )
            .with_chroma_420(true)
            .with_image_sequence()
            .with_recon_output(true);
        for (i, source) in colors.iter().enumerate() {
            let raw = pipe.try_encode_frame_420(source, &uv, &uv, w).unwrap();
            fs::write(out.join("color.obu"), raw).unwrap();
            let mut pre = Vec::new();
            let (py, pu, pv) = pipe.last_recon_unfiltered.as_ref().unwrap();
            for (plane, stride, width, height) in [
                (py, pipe.width as usize, w, h),
                (pu, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                (pv, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
            ] {
                for row in 0..height {
                    pre.extend_from_slice(&plane[row * stride..row * stride + width]);
                }
            }
            fs::write(out.join("color-prefilter.yuv"), pre).unwrap();
            let (y, u, v) = pipe.last_recon.take().unwrap();
            let mut expected = Vec::new();
            for (plane, stride, width, height) in [
                (&y, pipe.width as usize, w, h),
                (&u, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
                (&v, pipe.width as usize / 2, w.div_ceil(2), h.div_ceil(2)),
            ] {
                for row in 0..height {
                    expected.extend_from_slice(&plane[row * stride..row * stride + width]);
                }
            }
            if lossless {
                assert_eq!(
                    expected,
                    [source.as_slice(), uv.as_slice(), uv.as_slice()].concat()
                );
            }
            let decoded = fs::read(out.join(format!("frame-{i:010}.y4m"))).unwrap();
            let start = decoded.windows(6).position(|s| s == b"FRAME\n").unwrap() + 6;
            fs::write(out.join("color-final.yuv"), &expected).unwrap();
            assert_eq!(decoded.len() - start, expected.len());
            assert!(
                decoded[start..] == expected,
                "8-bit color frame {i}, {w}x{h}, speed {speed}, first differing byte {:?}",
                decoded[start..]
                    .iter()
                    .zip(&expected)
                    .position(|(a, b)| a != b)
            );
        }
        fs::remove_dir_all(out).unwrap();
    }
}

#[test]
fn native_ten_bit_animation_matches_color_and_alpha_reconstruction() {
    for w in [64usize, 65, 72] {
        let stride = w + 5;
        let directory =
            std::env::temp_dir().join(format!("svt-animation-hbd-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for quality in [40.0, 98.0, 100.0] {
            for (speed, has_alpha) in [(9, true), (6, false)] {
                let enc = AvifEncoder::new()
                    .with_bit_depth(10)
                    .with_speed(speed)
                    .with_quality(quality);
                let colors: Vec<Vec<u16>> = (0..2)
                    .map(|f| {
                        (0..stride * w)
                            .map(|i| (100 + (i * 7 + f * 133) % 750) as u16)
                            .collect()
                    })
                    .collect();
                let alphas: Vec<Vec<u16>> = (0..2)
                    .map(|f| {
                        (0..w * w)
                            .map(|i| ((i * 3 + f * 47) % 1024) as u16)
                            .collect()
                    })
                    .collect();
                let u: Vec<u16> = (0..w.div_ceil(2).pow(2))
                    .map(|i| (480 + i % 64) as u16)
                    .collect();
                let v: Vec<u16> = (0..w.div_ceil(2).pow(2))
                    .map(|i| (500 + i % 32) as u16)
                    .collect();
                let frames: Vec<_> = (0..2)
                    .map(|i| AnimationFrame {
                        y: &colors[i],
                        u: &u,
                        v: &v,
                        y_stride: stride,
                        alpha: has_alpha.then_some(alphas[i].as_slice()),
                        duration: [17, 29][i],
                    })
                    .collect();
                let bytes = enc
                    .encode_animation_yuv420_hbd(
                        &frames,
                        w as u32,
                        w as u32,
                        AnimationTiming { timescale: 1000 },
                    )
                    .unwrap();
                let input = directory.join("input.avif");
                fs::write(&input, bytes).unwrap();
                for extension in ["png", "y4m"] {
                    let result = Command::new("avifdec")
                        .args(["-j", "1", "--index", "all"])
                        .arg(&input)
                        .arg(directory.join(format!("frame.{extension}")))
                        .output()
                        .unwrap();
                    assert!(
                        result.status.success(),
                        "{}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
                // One pipeline for both frames on the animation's own GOP:
                // frame 1 is coded against frame 0, so re-encoding it as a
                // standalone still is not the same picture. The
                // recon-is-byte-inert check keeps its own pipeline and has
                // to walk the same frames to stay in step.
                let gop = enc
                    .animation_keyframes(&AnimationOptions::default())
                    .intra_period();
                let mut pipe = enc
                    .build_pipeline_gop(w as u32, w as u32, gop)
                    .with_chroma_420(true)
                    .with_image_sequence()
                    .with_recon_output(true);
                let mut pipe_no_recon = enc
                    .build_pipeline_gop(w as u32, w as u32, gop)
                    .with_chroma_420(true)
                    .with_image_sequence();
                let fps = 1000.0 / f64::from([17u32, 29][0].min([17u32, 29][1]));
                pipe.rc_config.framerate = fps;
                pipe_no_recon.rc_config.framerate = fps;
                for i in 0..2 {
                    let raw = pipe
                        .try_encode_frame_420_hbd(&colors[i], &u, &v, stride)
                        .unwrap();
                    let without_recon = pipe_no_recon
                        .try_encode_frame_420_hbd(&colors[i], &u, &v, stride)
                        .unwrap();
                    assert_eq!(
                        raw, without_recon,
                        "reconstruction must not change coded bytes"
                    );
                    fs::write(directory.join("color.obu"), raw).unwrap();
                    let mut pre = Vec::new();
                    let (pu, pv) = pipe.last_recon10_uv.as_ref().unwrap();
                    for (plane, stride, side) in [
                        (
                            pipe.last_recon10_y.as_ref().unwrap(),
                            pipe.width as usize,
                            w,
                        ),
                        (pu, pipe.width as usize / 2, w.div_ceil(2)),
                        (pv, pipe.width as usize / 2, w.div_ceil(2)),
                    ] {
                        for row in 0..side {
                            pre.extend(
                                plane[row * stride..row * stride + side]
                                    .iter()
                                    .flat_map(|n| n.to_le_bytes()),
                            );
                        }
                    }
                    fs::write(directory.join("color-prefilter.yuv"), pre).unwrap();
                    let (y, u, v) = pipe.last_recon10_final.take().unwrap();
                    let mut expected = Vec::new();
                    for (plane, stride, side) in [
                        (&y, pipe.width as usize, w),
                        (&u, pipe.width as usize / 2, w.div_ceil(2)),
                        (&v, pipe.width as usize / 2, w.div_ceil(2)),
                    ] {
                        for row in 0..side {
                            expected.extend(
                                plane[row * stride..row * stride + side]
                                    .iter()
                                    .flat_map(|n| n.to_le_bytes()),
                            );
                        }
                    }
                    if quality == 100.0 {
                        let mut source: Vec<u8> = colors[i]
                            .chunks_exact(stride)
                            .flat_map(|row| row[..w].iter().flat_map(|v| v.to_le_bytes()))
                            .collect();
                        source.extend(frames[i].u.iter().flat_map(|v| v.to_le_bytes()));
                        source.extend(frames[i].v.iter().flat_map(|v| v.to_le_bytes()));
                        assert_eq!(expected, source, "native lossless color must equal source");
                    }
                    assert!(
                        y.iter().any(|n| n & 3 != 0),
                        "fixture must preserve native low bits"
                    );
                    let decoded = fs::read(directory.join(format!("frame-{i:010}.y4m"))).unwrap();
                    let offset = decoded.windows(6).position(|p| p == b"FRAME\n").unwrap() + 6;
                    fs::write(directory.join("color-final.yuv"), &expected).unwrap();
                    assert_eq!(decoded.len() - offset, expected.len());
                    assert!(
                        decoded[offset..] == expected,
                        "10-bit color frame {i}, side {w}, quality {quality}, first differing byte {:?}",
                        decoded[offset..]
                            .iter()
                            .zip(&expected)
                            .position(|(a, b)| a != b)
                    );

                    if !has_alpha {
                        continue;
                    }
                    let mut settings = enc.clone();
                    settings.full_range = true;
                    let mut pipe = settings
                        .build_pipeline(w as u32, w as u32)
                        .with_recon_output(true);
                    pipe.try_encode_frame_hbd(&alphas[i], w).unwrap();
                    let (alpha, _, _) = pipe.last_recon10_final.unwrap();
                    assert_eq!(alpha.len(), pipe.width as usize * pipe.height as usize);
                    let file =
                        fs::File::open(directory.join(format!("frame-{i:010}.png"))).unwrap();
                    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
                        .read_info()
                        .unwrap();
                    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                    let info = reader.next_frame(&mut pixels).unwrap();
                    assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
                    assert_eq!(info.color_type, png::ColorType::Rgba);
                    assert_eq!((info.width as usize, info.height as usize), (w, w));
                    for (pixel_index, pixel) in pixels.as_chunks::<8>().0.iter().enumerate() {
                        let sample =
                            alpha[(pixel_index / w) * pipe.width as usize + pixel_index % w];
                        if quality == 100.0 {
                            assert_eq!(
                                sample, alphas[i][pixel_index],
                                "native lossless alpha must equal source"
                            );
                        }
                        let actual = u16::from_be_bytes([pixel[6], pixel[7]]);
                        // PNG expands alpha to 16 bits. Recover the native 10-bit
                        // integer to avoid testing the reader's expansion rounding.
                        let native = ((u32::from(actual) * 1023 + 32767) / 65535) as u16;
                        assert_eq!(native, sample, "10-bit alpha frame {i}");
                    }
                }
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn monochrome_animation_decodes_with_exact_luma_and_alpha() {
    fn luma_sample(i: usize, frame: usize, depth: u8) -> u32 {
        let scale = 1u32 << (depth - 8);
        (32 + (i * 7 + frame * 30) % 180) as u32 * scale + i as u32 % scale
    }
    macro_rules! exercise {
            ($sample:ty, $depth:expr, $speed:expr, $entry:ident, $raw:ident, $sizes:expr) => {
                for (w, h) in $sizes {
                    for quality in [40.0, 98.0, 100.0] {
                        for has_alpha in [false, true] {
                            let dir = std::env::temp_dir().join(format!("svt-mono-animation-{}-{}-{w}x{h}-{}-{quality}-{has_alpha}", std::process::id(), $depth, $speed));
                            fs::create_dir_all(&dir).unwrap();
                            let stride = w + 5;
                            let max = (1u32 << $depth) - 1;
                            let colors: Vec<Vec<$sample>> = (0..2).map(|f| {
                                (0..stride*h).map(|i| luma_sample(i, f, $depth) as $sample).collect()
                            }).collect();
                            let alphas: Vec<Vec<$sample>> = (0..2).map(|f| {
                                (0..w*h).map(|i| ((i*3+f*47) as u32 & max) as $sample).collect()
                            }).collect();
                            let frames: Vec<_> = (0..2).map(|i| MonochromeAnimationFrame {
                                y: &colors[i], y_stride: stride,
                                alpha: has_alpha.then_some(alphas[i].as_slice()), duration: [17, 29][i],
                            }).collect();
                            let enc = AvifEncoder::new().with_bit_depth($depth).with_speed($speed).with_quality(quality);
                            let bytes = enc.$entry(&frames, w as u32, h as u32, AnimationTiming { timescale: 1000 }).unwrap();
                            fs::write(dir.join("input.avif"), bytes).unwrap();
                            for extension in ["y4m", "png"] {
                                let result = Command::new("avifdec").args(["-j", "1", "--index", "all"])
                                    .arg(dir.join("input.avif")).arg(dir.join(format!("frame.{extension}"))).output().unwrap();
                                assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
                            }
                            // The colour references follow the product's own GOP
                            // choice: 8-bit mono animations inter-code (561267534),
                            // and fall back to all-key when an inter frame would be
                            // coded-lossless (`encode_monochrome_animation_samples`).
                            let reference_run = |gop: u32| {
                                let mut pipe = enc.build_pipeline_gop(w as u32, h as u32, gop).with_chroma_420(false).with_image_sequence().with_recon_output(true);
                                pipe.rc_config.framerate = 1000.0 / 17.0;
                                (0..2).map(|i| {
                                    pipe.$raw(&colors[i], stride)?;
                                    Ok(if $depth == 8 {
                                        pipe.last_recon.take().unwrap().0.into_iter().map(u16::from).collect::<Vec<u16>>()
                                    } else { pipe.last_recon10_final.take().unwrap().0 })
                                }).collect::<svtav1_encoder::EncodeResult<Vec<_>>>().map(|refs| (refs, pipe.width as usize))
                            };
                            let (color_refs, color_width) = match reference_run(enc.animation_keyframes(&AnimationOptions::default()).intra_period()) {
                                Err(e) if matches!(e.error(), svtav1_encoder::EncodeError::UnsupportedConfig(why) if is_mono_inter_refusal(why)) => reference_run(1).unwrap(),
                                r => r.unwrap(),
                            };
                            let mut alpha_settings = enc.clone();
                            alpha_settings.full_range = true;
                            let mut alpha_pipe = alpha_settings.build_pipeline(w as u32, h as u32).with_image_sequence().with_recon_output(true);
                            alpha_pipe.rc_config.framerate = 1000.0 / 17.0;
                            for i in 0..2 {
                                let reference = &color_refs[i];
                                let y4m = fs::read(dir.join(format!("frame-{i:010}.y4m"))).unwrap();
                                let header_end = y4m.iter().position(|&b| b == b'\n').unwrap();
                                let header = std::str::from_utf8(&y4m[..header_end]).unwrap();
                                assert!(header.contains(if $depth == 8 { "Cmono " } else { "Cmono10 " }), "{header}");
                                assert_eq!(&y4m[header_end+1..header_end+7], b"FRAME\n");
                                let pixels = &y4m[header_end+7..];
                                assert_eq!(pixels.len(), w*h*if $depth == 8 { 1 } else { 2 });
                                for y in 0..h { for x in 0..w {
                                    let offset = y*w+x;
                                    let actual = if $depth == 8 { u16::from(pixels[offset]) }
                                        else { u16::from_le_bytes([pixels[2*offset], pixels[2*offset+1]]) };
                                    if quality == 100.0 { assert_eq!(actual, colors[i][y*stride+x] as u16, "lossless luma must equal source"); }
                                    assert_eq!(actual, reference[y*color_width+x], "luma depth={} {w}x{h} q={quality} frame={i} ({x},{y})", $depth);
                                }}
                                if has_alpha {
                                    alpha_pipe.$raw(&alphas[i], w).unwrap();
                                    let reference: Vec<u16> = if $depth == 8 {
                                        alpha_pipe.last_recon.take().unwrap().0.into_iter().map(u16::from).collect()
                                    } else { alpha_pipe.last_recon10_final.take().unwrap().0 };
                                    let mut reader = png::Decoder::new(std::io::BufReader::new(fs::File::open(dir.join(format!("frame-{i:010}.png"))).unwrap())).read_info().unwrap();
                                    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                                    let info = reader.next_frame(&mut pixels).unwrap();
                                    assert_eq!(info.color_type, png::ColorType::GrayscaleAlpha);
                                    assert_eq!(info.bit_depth, if $depth == 8 { png::BitDepth::Eight } else { png::BitDepth::Sixteen });
                                    assert_eq!((info.width, info.height), (w as u32, h as u32));
                                    for y in 0..h { for x in 0..w {
                                        let pixel = y*w+x;
                                        let actual = if $depth == 8 { u16::from(pixels[2*pixel+1]) } else {
                                            let wide = u16::from_be_bytes([pixels[4*pixel+2], pixels[4*pixel+3]]);
                                            ((u32::from(wide)*1023+32767)/65535) as u16
                                        };
                                        if quality == 100.0 { assert_eq!(actual, alphas[i][y*w+x] as u16, "lossless alpha must equal source"); }
                                        assert_eq!(actual, reference[y*alpha_pipe.width as usize+x], "alpha depth={} {w}x{h} q={quality} frame={i} ({x},{y})", $depth);
                                    }}
                                }
                            }
                            fs::remove_dir_all(dir).unwrap();
                        }
                    }
                }
            };
        }
    exercise!(
        u8,
        8,
        7,
        encode_animation_mono,
        try_encode_frame,
        [(64usize, 80usize), (65, 67)]
    );
    exercise!(
        u8,
        8,
        2,
        encode_animation_mono,
        try_encode_frame,
        [(64usize, 64usize), (64, 80), (65, 67)]
    );
    exercise!(
        u16,
        10,
        9,
        encode_animation_mono_hbd,
        try_encode_frame_hbd,
        [(64usize, 80usize), (65, 67)]
    );
    exercise!(
        u16,
        10,
        2,
        encode_animation_mono_hbd,
        try_encode_frame_hbd,
        [(64usize, 80usize), (65, 67)]
    );
}

#[test]
fn validates_whole_monochrome_animation_before_encoding() {
    let plane = vec![128; 65 * 67];
    let frames = [
        MonochromeAnimationFrame {
            y: &plane,
            y_stride: 65,
            alpha: None,
            duration: 1,
        },
        MonochromeAnimationFrame {
            y: &plane,
            y_stride: 65,
            alpha: None,
            duration: 0,
        },
    ];
    // Validate the whole submission, including an invalid second frame.
    assert!(matches!(
        AvifEncoder::new().with_speed(2).encode_animation_mono(
            &frames,
            65,
            67,
            AnimationTiming { timescale: 1000 }
        ),
        Err(EncodeError::InvalidDimensions {
            width: 65,
            height: 67,
            ..
        })
    ));
}

#[test]
fn validates_crop_before_encoding() {
    let plane = vec![128; 64 * 80];
    let frames = [AnimationFrame {
        y: &plane,
        u: &plane,
        v: &plane,
        y_stride: 64,
        alpha: None,
        duration: 1,
    }];
    for crop in [
        CropRect::new(0, 0, 0, 80),
        CropRect::new(0, 0, 64, 0),
        CropRect::new(64, 0, 1, 1),
        CropRect::new(0, 80, 1, 1),
        CropRect::new(1, 0, 64, 80),
        CropRect::new(0, 1, 64, 80),
        CropRect::new(u32::MAX, 0, 2, 2),
        CropRect::new(0, 0, u32::MAX, 1),
    ] {
        let options = AnimationOptions {
            crop: Some(crop),
            ..AnimationOptions::default()
        };
        assert!(matches!(
            AvifEncoder::new().encode_animation_yuv420_with_options(
                &frames,
                64,
                80,
                AnimationTiming { timescale: 1000 },
                &options
            ),
            Err(EncodeError::UnsupportedConfig(
                "crop rectangle must be nonempty and within the image"
            ))
        ));
    }
}

#[test]
fn validates_orientation_before_encoding() {
    let plane = vec![128; 64 * 64];
    let frames = [AnimationFrame {
        y: &plane,
        u: &plane,
        v: &plane,
        y_stride: 64,
        alpha: None,
        duration: 1,
    }];
    for (rotation, mirror) in [(4, 0), (255, 0), (0, 2), (0, 255)] {
        let options = AnimationOptions {
            rotation: Some(rotation),
            mirror: Some(mirror),
            ..AnimationOptions::default()
        };
        assert!(matches!(
            AvifEncoder::new().encode_animation_yuv420_with_options(
                &frames,
                64,
                64,
                AnimationTiming { timescale: 1000 },
                &options
            ),
            Err(EncodeError::UnsupportedConfig(
                "rotation must be 0..=3 and mirror axis must be 0..=1"
            ))
        ));
    }
}

#[test]
fn validates_pixel_aspect_ratio_before_encoding() {
    let plane = vec![128; 64 * 64];
    let frames = [AnimationFrame {
        y: &plane,
        u: &plane,
        v: &plane,
        y_stride: 64,
        alpha: None,
        duration: 1,
    }];
    for (h, v) in [(0, 0), (0, 1), (1, 0), (2, 1), (1, 2)] {
        let options = AnimationOptions {
            pixel_aspect_ratio: Some(PaspBox::new(h, v)),
            ..AnimationOptions::default()
        };
        assert!(matches!(
            AvifEncoder::new().encode_animation_yuv420_with_options(
                &frames,
                64,
                64,
                AnimationTiming { timescale: 1000 },
                &options
            ),
            Err(EncodeError::UnsupportedConfig(
                "AVIF pixel aspect ratio must be positive and 1:1"
            ))
        ));
    }
}

#[test]
fn validates_whole_animation_before_encoding() {
    let enc = AvifEncoder::new();
    assert!(
        enc.encode_animation_yuv420(&[], 64, 64, AnimationTiming { timescale: 1000 })
            .is_err()
    );
    let p = vec![128; 64 * 64];
    let frame = AnimationFrame {
        y: &p,
        u: &p,
        v: &p,
        y_stride: 64,
        alpha: None,
        duration: 0,
    };
    assert!(
        enc.encode_animation_yuv420(&[frame], 64, 64, AnimationTiming { timescale: 1000 })
            .is_err()
    );
}

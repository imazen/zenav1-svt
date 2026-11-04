//! Every input plane may carry its own stride: padding between rows is never
//! read, and the encoded stream is byte-identical to the tightly packed one.
//!
//! The padding holds garbage that would change the output if any path read it
//! (and, at 10 bits, samples out of range, which the depth check must ignore).
//! Each strided buffer is the exact minimum `(rows - 1) * stride + width`, so a
//! path that wrongly requires `rows * stride` panics or refuses here.

#[cfg(feature = "avif-container")]
use svtav1::avif::animation::{AnimationFrame, AnimationTiming, MonochromeAnimationFrame};
use svtav1::avif::{AvifEncoder, EncodeError};
use svtav1_encoder::film_grain_config::FilmGrainConfig;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::speed_config::NativePreset;
use svtav1_types::chroma::ChromaFormat;

/// 8-aligned full superblock, 8-aligned partial superblock, odd partial
/// superblock on each axis, and odd below one superblock.
const DIMS: [(usize, usize); 5] = [(64, 64), (72, 40), (65, 67), (66, 81), (33, 27)];

fn sample(x: usize, y: usize, frame: usize, plane: usize, max: u32) -> u32 {
    // Texture plus motion: enough residual and search to exercise the paths.
    let v = (x * 3 + y * 5 + (x * y) / 7 + frame * 11 + plane * 37) as u32;
    (v * max / 255 + (x ^ y) as u32 % 17) % (max + 1)
}

fn tight<T>(w: usize, h: usize, frame: usize, plane: usize, f: impl Fn(u32) -> T) -> Vec<T> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| f(sample(x, y, frame, plane, 255)))
        .collect()
}

/// `src` (tight, `w` wide) at `stride`, with padding on every row but the last.
fn strided<T: Copy>(src: &[T], w: usize, stride: usize, garbage: T) -> Vec<T> {
    let h = src.len() / w;
    let mut out = vec![garbage; (h - 1) * stride + w];
    for (row, line) in src.chunks_exact(w).enumerate() {
        out[row * stride..row * stride + w].copy_from_slice(line);
    }
    out
}

struct Frame<T> {
    w: usize,
    cw: usize,
    y: Vec<T>,
    u: Vec<T>,
    v: Vec<T>,
}

impl<T: Copy> Frame<T> {
    fn new(
        w: usize,
        h: usize,
        cw: usize,
        ch: usize,
        n: usize,
        f: impl Fn(u32) -> T + Copy,
    ) -> Self {
        Self {
            w,
            cw,
            y: tight(w, h, n, 0, f),
            u: tight(cw, ch, n, 1, f),
            v: tight(cw, ch, n, 2, f),
        }
    }

    /// Each plane padded by a different amount, so a stride mix-up cannot pass.
    fn padded(&self, garbage: T) -> [(Vec<T>, usize); 3] {
        let (ys, us, vs) = (self.w + 7, self.cw + 3, self.cw + 11);
        [
            (strided(&self.y, self.w, ys, garbage), ys),
            (strided(&self.u, self.cw, us, garbage), us),
            (strided(&self.v, self.cw, vs, garbage), vs),
        ]
    }
}

fn rc(qp: u8) -> RcConfig {
    RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    }
}

/// Encode `frames` through both forms and require identical streams and recon.
fn assert_420_identical(label: &str, build: impl Fn() -> EncodePipeline, frames: &[Frame<u8>]) {
    let mut a = build();
    let mut b = build();
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    for f in frames {
        sa.extend(
            a.try_encode_frame_420(&f.y, f.w, &f.u, f.w.div_ceil(2), &f.v, f.w.div_ceil(2))
                .unwrap(),
        );
        let [(y, ys), (u, us), (v, vs)] = f.padded(0xA5);
        sb.extend(b.try_encode_frame_420(&y, ys, &u, us, &v, vs).unwrap());
    }
    sa.extend(a.try_flush().unwrap());
    sb.extend(b.try_flush().unwrap());
    assert!(!sa.is_empty(), "{label}: empty stream");
    assert!(sa == sb, "{label}: strided stream differs from tight");
    assert!(a.last_recon == b.last_recon, "{label}: recon differs");
}

fn frames_420(w: usize, h: usize, n: usize) -> Vec<Frame<u8>> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    (0..n)
        .map(|i| Frame::new(w, h, cw, ch, i, |v| v as u8))
        .collect()
}

#[test]
fn eight_bit_420_stills_and_low_delay_inter_are_byte_identical() {
    for (w, h) in DIMS {
        let frames = frames_420(w, h, 3);
        for preset in [10u8, 6] {
            let build = || {
                EncodePipeline::new(w as u32, h as u32, preset, rc(32), 0, 2).with_chroma_420(true)
            };
            assert_420_identical(&format!("{w}x{h} p{preset}"), build, &frames);
        }
    }
}

#[test]
fn eight_bit_420_superres_is_byte_identical() {
    for (w, h) in [(65, 67), (72, 72)] {
        let frames = frames_420(w, h, 1);
        for denom in [9u8, 16] {
            let build = || {
                EncodePipeline::new(w as u32, h as u32, 9, rc(32), 0, 1)
                    .with_chroma_420(true)
                    .with_superres(denom)
            };
            assert_420_identical(&format!("{w}x{h} superres {denom}"), build, &frames);
        }
    }
}

#[test]
fn film_grain_denoise_reads_strided_sources() {
    for (w, h) in [(64, 64), (65, 67)] {
        let frames = frames_420(w, h, 1);
        for apply in [false, true] {
            let build = || {
                let mut p =
                    EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1).with_chroma_420(true);
                p.film_grain = FilmGrainConfig {
                    denoise_strength: 25,
                    denoise_apply: apply,
                    ..Default::default()
                };
                p
            };
            assert_420_identical(&format!("{w}x{h} grain apply={apply}"), build, &frames);
        }
    }
}

#[test]
fn random_access_buffers_strided_inputs() {
    for (w, h) in [(128usize, 128usize), (129, 131)] {
        let frames = frames_420(w, h, 5);
        let build = || {
            EncodePipeline::new_with_preset(
                w as u32,
                h as u32,
                NativePreset::new(6).unwrap(),
                rc(40),
                HIERARCHICAL_LEVELS_AUTO,
                64,
            )
            .with_chroma_420(true)
            .with_pred_structure(PredStructure::RandomAccess)
        };
        assert_420_identical(&format!("random access {w}x{h}"), build, &frames);
    }
}

#[test]
fn four_four_four_is_byte_identical() {
    for (w, h) in [(64usize, 64usize), (65, 67), (33, 27)] {
        let build = || {
            EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 2)
                .with_chroma_format(Some(ChromaFormat::Yuv444))
        };
        let (mut a, mut b) = (build(), build());
        let (mut sa, mut sb) = (Vec::new(), Vec::new());
        for n in 0..3 {
            let f = Frame::new(w, h, w, h, n, |v| v as u8);
            sa.extend(a.try_encode_frame_444(&f.y, w, &f.u, w, &f.v, w).unwrap());
            let [(y, ys), (u, us), (v, vs)] = f.padded(0xA5);
            sb.extend(b.try_encode_frame_444(&y, ys, &u, us, &v, vs).unwrap());
        }
        assert!(!sa.is_empty());
        assert!(sa == sb, "{w}x{h} 4:4:4");
    }
}

/// 4:2:2 has no encode arm yet: both forms take the same refusal, and geometry
/// is checked with the same rules.
#[test]
fn four_two_two_refuses_alike_and_validates_strides() {
    let (w, h) = (65usize, 67usize);
    let build = || {
        EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1)
            .with_chroma_format(Some(ChromaFormat::Yuv422))
    };
    let cw = w.div_ceil(2);
    let f = Frame::new(w, h, cw, h, 0, |v| v as u8);
    let tight = build()
        .try_encode_frame_422(&f.y, w, &f.u, w.div_ceil(2), &f.v, w.div_ceil(2))
        .unwrap_err();
    let [(y, ys), (u, us), (v, vs)] = f.padded(0xA5);
    let padded = build()
        .try_encode_frame_422(&y, ys, &u, us, &v, vs)
        .unwrap_err();
    assert_eq!(tight.error().to_string(), padded.error().to_string());
    assert!(matches!(
        tight.error(),
        svtav1::pipeline::EncodeError::UnsupportedConfig(_)
    ));
    let short = build().try_encode_frame_422(&y, ys, &u[..u.len() - 1], us, &v, vs);
    assert!(matches!(
        short.unwrap_err().error(),
        svtav1::pipeline::EncodeError::InvalidDimensions { .. }
    ));
}

fn wide(v: u32) -> u16 {
    (v << 2 | v >> 6) as u16
}

fn assert_hbd_identical(label: &str, build: impl Fn() -> EncodePipeline, frames: &[Frame<u16>]) {
    let (mut a, mut b) = (build(), build());
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    for f in frames {
        sa.extend(
            a.try_encode_frame_420_hbd(&f.y, f.w, &f.u, f.w.div_ceil(2), &f.v, f.w.div_ceil(2))
                .unwrap(),
        );
        // 0xFFFF exceeds 10 bits: only padding may hold it.
        let [(y, ys), (u, us), (v, vs)] = f.padded(0xFFFF);
        sb.extend(b.try_encode_frame_420_hbd(&y, ys, &u, us, &v, vs).unwrap());
    }
    assert!(!sa.is_empty(), "{label}: empty stream");
    assert!(sa == sb, "{label}: strided stream differs from tight");
    assert!(
        a.last_recon10_final == b.last_recon10_final,
        "{label}: recon differs"
    );
}

#[test]
fn ten_bit_420_is_byte_identical_with_out_of_range_padding() {
    for (w, h) in DIMS {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let frames: Vec<_> = (0..2).map(|n| Frame::new(w, h, cw, ch, n, wide)).collect();
        for preset in [10u8, 6] {
            let build = || {
                EncodePipeline::new(w as u32, h as u32, preset, rc(32), 0, 2)
                    .with_bit_depth(10)
                    .with_chroma_420(true)
            };
            assert_hbd_identical(&format!("{w}x{h} p{preset}"), build, &frames);
        }
    }
}

#[test]
fn ten_bit_420_superres_is_byte_identical() {
    let (w, h) = (65usize, 67usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let frames = [Frame::new(w, h, cw, ch, 0, wide)];
    for denom in [9u8, 16] {
        let build = || {
            EncodePipeline::new(w as u32, h as u32, 9, rc(32), 0, 1)
                .with_bit_depth(10)
                .with_chroma_420(true)
                .with_superres(denom)
        };
        assert_hbd_identical(&format!("superres {denom}"), build, &frames);
    }
}

#[test]
fn ten_bit_monochrome_ignores_out_of_range_padding() {
    let (w, h) = (65usize, 67usize);
    let y: Vec<u16> = tight(w, h, 0, 0, wide);
    let build = || {
        let mut p = EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1).with_bit_depth(10);
        p.chroma_420 = false;
        p
    };
    let plain = build().try_encode_frame_hbd(&y, w).unwrap();
    let padded = strided(&y, w, w + 9, 0xFFFF);
    assert!(plain == build().try_encode_frame_hbd(&padded, w + 9).unwrap());
    // An out-of-range IMAGE sample is still refused.
    let mut bad = padded;
    bad[w + 9] = 1 << 10;
    assert!(build().try_encode_frame_hbd(&bad, w + 9).is_err());
}

#[test]
fn hbd_image_samples_out_of_range_are_still_refused_on_every_plane() {
    let (w, h) = (64usize, 64usize);
    let f = Frame::new(w, h, 32, 32, 0, wide);
    let build = || {
        EncodePipeline::new(64, 64, 10, rc(32), 0, 1)
            .with_bit_depth(10)
            .with_chroma_420(true)
    };
    for plane in 0..3 {
        let mut p = f.padded(0);
        let (buf, stride) = &mut p[plane];
        buf[*stride + 1] = 1 << 10;
        let [(y, ys), (u, us), (v, vs)] = p;
        assert!(
            matches!(
                build()
                    .try_encode_frame_420_hbd(&y, ys, &u, us, &v, vs)
                    .unwrap_err()
                    .error(),
                svtav1::pipeline::EncodeError::UnsupportedConfig(_)
            ),
            "plane {plane}"
        );
    }
}

#[test]
fn bad_geometry_is_an_error_on_every_entry_point() {
    let (w, h) = (65usize, 67usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let invalid = |r: svtav1::pipeline::EncodeResult<Vec<u8>>, what: &str| {
        let e = r.expect_err(&format!("{what} must be refused"));
        assert!(
            matches!(
                e.error(),
                svtav1::pipeline::EncodeError::InvalidDimensions { .. }
            ),
            "{what}: {e}"
        );
    };
    let p420 = || EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1).with_chroma_420(true);
    let f8 = Frame::new(w, h, cw, ch, 0, |v| v as u8);
    let [(y, ys), (u, us), (v, vs)] = f8.padded(0);
    // A stride below the width, on each plane.
    invalid(
        p420().try_encode_frame_420(&y, w - 1, &u, us, &v, vs),
        "y stride",
    );
    invalid(
        p420().try_encode_frame_420(&y, ys, &u, cw - 1, &v, vs),
        "u stride",
    );
    invalid(
        p420().try_encode_frame_420(&y, ys, &u, us, &v, cw - 1),
        "v stride",
    );
    // One sample short of the minimum, on each plane.
    invalid(
        p420().try_encode_frame_420(&y[..y.len() - 1], ys, &u, us, &v, vs),
        "short y",
    );
    invalid(
        p420().try_encode_frame_420(&y, ys, &u[..u.len() - 1], us, &v, vs),
        "short u",
    );
    invalid(
        p420().try_encode_frame_420(&y, ys, &u, us, &v[..v.len() - 1], vs),
        "short v",
    );
    // A stride whose offset arithmetic overflows.
    invalid(
        p420().try_encode_frame_420(&y, usize::MAX, &u, us, &v, vs),
        "overflow",
    );
    // The tight entry point validates luma too.
    invalid(
        p420().try_encode_frame_420(&y[..w], w, &f8.u, w.div_ceil(2), &f8.v, w.div_ceil(2)),
        "short tight y",
    );

    let p10 = || {
        EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1)
            .with_bit_depth(10)
            .with_chroma_420(true)
    };
    let f10 = Frame::new(w, h, cw, ch, 0, wide);
    let [(y, ys), (u, us), (v, vs)] = f10.padded(0);
    invalid(
        p10().try_encode_frame_420_hbd(&y, ys, &u, cw - 1, &v, vs),
        "hbd u stride",
    );
    invalid(
        p10().try_encode_frame_420_hbd(&y, ys, &u, us, &v[..v.len() - 1], vs),
        "hbd short v",
    );

    let p444 = || {
        EncodePipeline::new(w as u32, h as u32, 10, rc(32), 0, 1)
            .with_chroma_format(Some(ChromaFormat::Yuv444))
    };
    let f444 = Frame::new(w, h, w, h, 0, |v| v as u8);
    let [(y, ys), (u, us), (v, vs)] = f444.padded(0);
    invalid(
        p444().try_encode_frame_444(&y, ys, &u, w - 1, &v, vs),
        "444 u stride",
    );
    invalid(
        p444().try_encode_frame_444(&y, ys, &u, us, &v[..v.len() - 1], vs),
        "444 short v",
    );

    // Random access validates before it buffers.
    let mut ra = EncodePipeline::new_with_preset(
        w as u32,
        h as u32,
        NativePreset::new(10).unwrap(),
        rc(40),
        HIERARCHICAL_LEVELS_AUTO,
        64,
    )
    .with_chroma_420(true)
    .with_pred_structure(PredStructure::RandomAccess);
    let [(y, ys), (u, us), (v, vs)] = f8.padded(0);
    invalid(
        ra.try_encode_frame_420(&y, ys, &u[..1], us, &v, vs),
        "RA short u",
    );
}

#[test]
fn facade_still_is_byte_identical_and_validates() {
    let encoder = AvifEncoder::new().with_speed(9).with_quality(60.0);
    for (w, h) in DIMS {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let f = Frame::new(w, h, cw, ch, 0, |v| v as u8);
        let (wu, hu, cwu) = (w as u32, h as u32, cw as u32);
        let plain = encoder
            .encode_yuv420(&f.y, wu, &f.u, wu.div_ceil(2), &f.v, wu.div_ceil(2), wu, hu)
            .unwrap();
        let [(y, ys), (u, us), (v, vs)] = f.padded(0xA5);
        let padded = encoder
            .encode_yuv420(&y, ys as u32, &u, us as u32, &v, vs as u32, wu, hu)
            .unwrap();
        assert!(plain.data == padded.data, "{w}x{h}");
        for r in [
            encoder.encode_yuv420(&y, wu - 1, &u, us as u32, &v, vs as u32, wu, hu),
            encoder.encode_yuv420(&y, ys as u32, &u, cwu - 1, &v, vs as u32, wu, hu),
            encoder.encode_yuv420(&y, ys as u32, &u, us as u32, &v[..1], vs as u32, wu, hu),
        ] {
            assert!(
                matches!(r, Err(EncodeError::InvalidDimensions { .. })),
                "{w}x{h}"
            );
        }
    }
}

#[cfg(feature = "avif-container")]
const TIMING: AnimationTiming = AnimationTiming { timescale: 1000 };

#[cfg(feature = "avif-container")]
macro_rules! animation_case {
    ($test:ident, $sample:ty, $depth:expr, $garbage:expr, $map:expr, $color:ident, $mono:ident) => {
        #[test]
        fn $test() {
            let encoder = AvifEncoder::new()
                .with_bit_depth($depth)
                .with_speed(9)
                .with_quality(60.0);
            for (w, h) in [(64usize, 64usize), (65, 67), (33, 27)] {
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let (wu, hu) = (w as u32, h as u32);
                let frames: Vec<Frame<$sample>> =
                    (0..3).map(|n| Frame::new(w, h, cw, ch, n, $map)).collect();
                let alphas: Vec<Vec<$sample>> = (0..3).map(|n| tight(w, h, n, 3, $map)).collect();
                let padded: Vec<_> = frames.iter().map(|f| f.padded($garbage)).collect();
                let astride = w + 13;
                let apadded: Vec<_> = alphas
                    .iter()
                    .map(|a| strided(a, w, astride, $garbage))
                    .collect();

                let plain: Vec<_> = (0..3)
                    .map(|i| AnimationFrame {
                        y: &frames[i].y,
                        u: &frames[i].u,
                        v: &frames[i].v,
                        y_stride: w,
                        u_stride: cw,
                        v_stride: cw,
                        alpha: Some(&alphas[i]),
                        alpha_stride: w,
                        duration: 10,
                    })
                    .collect();
                let with_padding: Vec<_> = (0..3)
                    .map(|i| AnimationFrame {
                        y: &padded[i][0].0,
                        u: &padded[i][1].0,
                        v: &padded[i][2].0,
                        y_stride: padded[i][0].1,
                        u_stride: padded[i][1].1,
                        v_stride: padded[i][2].1,
                        alpha: Some(&apadded[i]),
                        alpha_stride: astride,
                        duration: 10,
                    })
                    .collect();
                assert!(
                    encoder.$color(&plain, wu, hu, TIMING).unwrap()
                        == encoder.$color(&with_padding, wu, hu, TIMING).unwrap(),
                    "{w}x{h} color"
                );

                let plain: Vec<_> = (0..3)
                    .map(|i| MonochromeAnimationFrame {
                        y: &frames[i].y,
                        y_stride: w,
                        alpha: Some(&alphas[i]),
                        alpha_stride: w,
                        duration: 10,
                    })
                    .collect();
                let with_padding: Vec<_> = (0..3)
                    .map(|i| MonochromeAnimationFrame {
                        y: &padded[i][0].0,
                        y_stride: padded[i][0].1,
                        alpha: Some(&apadded[i]),
                        alpha_stride: astride,
                        duration: 10,
                    })
                    .collect();
                assert!(
                    encoder.$mono(&plain, wu, hu, TIMING).unwrap()
                        == encoder.$mono(&with_padding, wu, hu, TIMING).unwrap(),
                    "{w}x{h} mono"
                );

                // Bad geometry on the planes that used to be assumed tight.
                let good = &with_padding[0];
                let short_alpha = MonochromeAnimationFrame {
                    y: good.y,
                    y_stride: good.y_stride,
                    alpha: good.alpha.map(|a| &a[..a.len() - 1]),
                    alpha_stride: astride,
                    duration: 10,
                };
                assert!(matches!(
                    encoder.$mono(&[short_alpha], wu, hu, TIMING),
                    Err(EncodeError::InvalidDimensions { .. })
                ));
                let narrow_alpha = MonochromeAnimationFrame {
                    y: good.y,
                    y_stride: good.y_stride,
                    alpha: good.alpha,
                    alpha_stride: w - 1,
                    duration: 10,
                };
                assert!(matches!(
                    encoder.$mono(&[narrow_alpha], wu, hu, TIMING),
                    Err(EncodeError::InvalidDimensions { .. })
                ));
                let color_with = |u_len_cut: usize, u_stride: usize| AnimationFrame {
                    y: &padded[0][0].0,
                    u: &padded[0][1].0[..padded[0][1].0.len() - u_len_cut],
                    v: &padded[0][2].0,
                    y_stride: padded[0][0].1,
                    u_stride,
                    v_stride: padded[0][2].1,
                    alpha: None,
                    alpha_stride: 0,
                    duration: 10,
                };
                assert!(matches!(
                    encoder.$color(&[color_with(1, padded[0][1].1)], wu, hu, TIMING),
                    Err(EncodeError::InvalidDimensions { .. })
                ));
                assert!(matches!(
                    encoder.$color(&[color_with(0, cw - 1)], wu, hu, TIMING),
                    Err(EncodeError::InvalidDimensions { .. })
                ));
            }
        }
    };
}
#[cfg(feature = "avif-container")]
animation_case!(
    eight_bit_animation_is_byte_identical,
    u8,
    8,
    0xA5,
    |v| v as u8,
    encode_animation_yuv420,
    encode_animation_mono
);
#[cfg(feature = "avif-container")]
animation_case!(
    ten_bit_animation_is_byte_identical_with_out_of_range_padding,
    u16,
    10,
    0xFFFF,
    wide,
    encode_animation_yuv420_hbd,
    encode_animation_mono_hbd
);

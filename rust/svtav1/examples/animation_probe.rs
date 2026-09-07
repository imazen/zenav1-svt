#[cfg(feature = "avif-container")]
fn main() {
    use svtav1::avif::{
        AvifEncoder,
        animation::{
            AnimationFrame, AnimationOptions, AnimationTiming, ClliBox, MdcvBox, RepetitionCount,
        },
    };
    let w = 64;
    let mut colors: Vec<Vec<u8>> = (0..3)
        .map(|f| {
            (0..w * w)
                .map(|i| (32 + (i % w + f * 30) % 180) as u8)
                .collect()
        })
        .collect();
    let alphas: Vec<Vec<u8>> = (0..3)
        .map(|f| (0..w * w).map(|i| ((i + f * 13) % 256) as u8).collect())
        .collect();
    if std::env::var_os("AVIF_PREMULTIPLIED").is_some() {
        for (color, alpha) in colors.iter_mut().zip(&alphas) {
            for (y, &a) in color.iter_mut().zip(alpha) {
                *y = 16 + ((u16::from(*y - 16) * u16::from(a) + 127) / 255) as u8;
            }
        }
    }
    let uv = vec![128; w * w / 4];
    let frames: Vec<_> = (0..3)
        .map(|f| AnimationFrame {
            y: &colors[f],
            u: &uv,
            v: &uv,
            y_stride: w,
            alpha: if std::env::var_os("AVIF_NO_ALPHA").is_some() {
                None
            } else {
                Some(&alphas[f])
            },
            duration: [100, 200, 300][f],
        })
        .collect();
    let mut options = AnimationOptions::default();
    if let Ok(count) = std::env::var("AVIF_REPEAT") {
        options.repetition = if count == "infinite" {
            RepetitionCount::Infinite
        } else {
            RepetitionCount::Finite(count.parse().unwrap())
        };
    }
    options.premultiplied_alpha = std::env::var_os("AVIF_PREMULTIPLIED").is_some();
    if std::env::var_os("AVIF_METADATA").is_some() {
        options.icc = Some(std::fs::read(std::env::var("AVIF_ICC").expect("AVIF_ICC")).unwrap());
        options.exif = Some(b"II*\0\x08\0\0\0\0\0\0\0\0\0".to_vec());
        options.xmp = Some(
            b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><probe>animation</probe></x:xmpmeta>".to_vec(),
        );
        options.clli = Some(ClliBox::new(1000, 400));
        options.mdcv = Some(MdcvBox::new(
            [(13250, 34500), (7500, 3000), (34000, 16000)],
            (15635, 16450),
            10000000,
            50,
        ));
    }
    let bytes = AvifEncoder::new()
        .with_speed(7)
        .encode_animation_yuv420_with_options(
            &frames,
            w as u32,
            w as u32,
            AnimationTiming { timescale: 1000 },
            &options,
        )
        .unwrap();
    std::fs::write(std::env::args().nth(1).expect("output path"), bytes).unwrap();
}
#[cfg(not(feature = "avif-container"))]
fn main() {
    panic!("requires --features avif-container");
}

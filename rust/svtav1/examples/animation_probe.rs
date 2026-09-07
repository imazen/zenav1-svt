#[cfg(feature = "avif-container")]
fn main() {
    use svtav1::avif::{
        AvifEncoder,
        animation::{AnimationFrame, AnimationTiming},
    };
    let w = 64;
    let colors: Vec<Vec<u8>> = (0..3)
        .map(|f| {
            (0..w * w)
                .map(|i| (32 + (i % w + f * 30) % 180) as u8)
                .collect()
        })
        .collect();
    let alphas: Vec<Vec<u8>> = (0..3)
        .map(|f| (0..w * w).map(|i| ((i + f * 13) % 256) as u8).collect())
        .collect();
    let uv = vec![128; w * w / 4];
    let frames: Vec<_> = (0..3)
        .map(|f| AnimationFrame {
            y: &colors[f],
            u: &uv,
            v: &uv,
            y_stride: w,
            alpha: Some(&alphas[f]),
            duration: [100, 200, 300][f],
        })
        .collect();
    let bytes = AvifEncoder::new()
        .with_speed(7)
        .encode_animation_yuv420(
            &frames,
            w as u32,
            w as u32,
            AnimationTiming { timescale: 1000 },
        )
        .unwrap();
    std::fs::write(std::env::args().nth(1).expect("output path"), bytes).unwrap();
}
#[cfg(not(feature = "avif-container"))]
fn main() {
    panic!("requires --features avif-container");
}

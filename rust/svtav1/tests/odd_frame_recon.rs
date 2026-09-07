//! Partial-edge witnesses found by the animation decoder comparisons.
//! Requires aomdec (AOMDEC may name an explicit executable).
//! Pure-Rust CI explicitly sets ZENAV1_SKIP_DECODER_TESTS for its decoder leg.
use std::{fs, process::Command};
use svtav1_encoder::{
    pipeline::EncodePipeline,
    rate_control::{RcConfig, RcMode},
};

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
        (&planes.1, stride / 2, w.div_ceil(2), h.div_ceil(2)),
        (&planes.2, stride / 2, w.div_ceil(2), h.div_ceil(2)),
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

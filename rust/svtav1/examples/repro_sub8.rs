//! Repro: one failing conformance chroma cell.
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn main() {
    let sz = 48usize;
    let sb = 64usize;
    let pw = sz.div_ceil(sb) * sb;
    let ph = pw;
    let mut small = vec![0u8; sz * sz];
    for r in 0..sz {
        for c in 0..sz {
            small[r * sz + c] = ((r * 255) / sz) as u8 ^ ((c * 3) & 0x3F) as u8;
        }
    }
    let mut src = vec![128u8; pw * ph];
    for r in 0..sz {
        for c in 0..sz {
            src[r * pw + c] = small[r * sz + c];
        }
        for c in sz..pw {
            src[r * pw + c] = src[r * pw + sz - 1];
        }
    }
    for r in sz..ph {
        for c in 0..pw {
            src[r * pw + c] = src[(sz - 1) * pw + c];
        }
    }
    let rc = RcConfig { mode: RcMode::Cqp, qp: 20, ..RcConfig::default() };
    let mut p = EncodePipeline::new(pw as u32, ph as u32, 3, rc, 0, 1).with_chroma_420(true);
    let u = vec![128u8; pw * ph / 4];
    let v = vec![128u8; pw * ph / 4];
    let obu = p.encode_frame_420(&src, &u, &v, pw);
    std::fs::write("/tmp/repro48.obu", &obu).unwrap();
    // I420 for the C capture driver (identical bytes).
    let mut yuv = src.clone();
    yuv.extend_from_slice(&u);
    yuv.extend_from_slice(&v);
    std::fs::write("/tmp/repro48.yuv", &yuv).unwrap();
    eprintln!("wrote {} bytes", obu.len());
}

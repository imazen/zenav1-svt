//! Still-picture panic sweep on textured content, under `cargo nextest`.
//!
//! The byte gates cover stills against C, but as shell scripts; the nextest
//! surface ran the 10-bit re-encode and partial-superblock paths only on
//! tiny noise frames (`never_panics`). This sweep encodes a textured image
//! at a partial-SB size and an aligned one, at every preset band, a low, mid
//! and high qp, at 8 bits and at 10 bits with real low bits, and lists every
//! cell that panicked rather than stopping at the first. It checks no bytes
//! against C; the gates do that.

use std::panic::{AssertUnwindSafe, catch_unwind};

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::speed_config::NativePreset;

fn texture(w: usize, h: usize) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    let px = |x: usize, y: usize| -> u16 {
        let (x, y) = (x as u64, y as u64);
        let mut z = (x * 2654435761) ^ (y * 40503);
        z ^= z >> 13;
        let edges = if (x / 24 + y / 16) % 3 == 0 { 300 } else { 0 };
        (120 + (x * 5 + y * 3) % 400 + edges + z % 23) as u16
    };
    let y = (0..h).flat_map(|r| (0..w).map(move |c| px(c, r))).collect();
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let u = (0..cw * ch).map(|i| 400 + (i % cw) as u16 * 3).collect();
    let v = (0..cw * ch).map(|i| 600 - (i / cw) as u16 * 2).collect();
    (y, u, v)
}

fn sweep(preset: i8) {
    let mut panics = Vec::new();
    let mut refused = Vec::new();
    let mut ok = 0;
    let mut n = 0;
    for &(w, h) in &[(72usize, 88usize), (128, 128)] {
        let (y10, u10, v10) = texture(w, h);
        let to8 = |p: &[u16]| p.iter().map(|&s| (s >> 2) as u8).collect::<Vec<u8>>();
        let (y8, u8p, v8) = (to8(&y10), to8(&u10), to8(&v10));
        for bd in [8u8, 10] {
            for qp in [5u8, 32, 63] {
                n += 1;
                let r = catch_unwind(AssertUnwindSafe(|| {
                    let rc = RcConfig {
                        mode: RcMode::Cqp,
                        qp,
                        ..RcConfig::default()
                    };
                    let mut p = EncodePipeline::new_with_preset(
                        w as u32,
                        h as u32,
                        NativePreset::new(preset).expect("preset"),
                        rc,
                        0,
                        1,
                    )
                    .with_bit_depth(bd)
                    .with_chroma_420(true);
                    if bd == 10 {
                        p.try_encode_frame_420_hbd(&y10, &u10, &v10, w)
                    } else {
                        p.try_encode_frame_420(&y8, &u8p, &v8, w)
                    }
                    .map(|b| b.len())
                    .map_err(|e| e.to_string())
                }));
                match r {
                    Ok(Ok(len)) if len > 0 => ok += 1,
                    Ok(Ok(_)) => refused.push(format!("{w}x{h} bd{bd} q{qp}: empty stream")),
                    Ok(Err(e)) => refused.push(format!("{w}x{h} bd{bd} q{qp}: {e}")),
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        panics.push(format!("{w}x{h} bd{bd} q{qp}: {msg}"));
                    }
                }
            }
        }
    }
    assert!(
        panics.is_empty() && refused.is_empty() && ok == n,
        "preset {preset}: {ok} of {n} encoded; panics:\n{}\nrefusals:\n{}",
        panics.join("\n"),
        refused.join("\n")
    );
}

macro_rules! still_sweep {
    ($($name:ident => $preset:expr),* $(,)?) => {
        $( #[test] fn $name() { sweep($preset); } )*
    };
}
still_sweep! {
    still_sweep_pm1 => -1,
    still_sweep_p0 => 0,
    still_sweep_p2 => 2,
    still_sweep_p4 => 4,
    still_sweep_p6 => 6,
    still_sweep_p8 => 8,
    still_sweep_p10 => 10,
    still_sweep_p13 => 13,
}

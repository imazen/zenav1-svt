//! Video paths through the public pipeline, under `cargo nextest`.
//!
//! The byte gates verify the inter tools against C and a decoder, but they
//! are shell scripts, so the nextest surface (and `tools/coverage_gate.sh`)
//! never ran them: on 2026-09-26 the TPL dispenser, the CBR driver, warp
//! refinement and OBMC refinement read 0-3 % region coverage, and a panic
//! there would surface only in CI's slowest shard. `never_panics` covers LD
//! and RA on tiny noise frames, which reach none of them: TPL needs random
//! access with `aq_mode` 2, and warp / OBMC need real non-translational
//! motion at the low presets.
//!
//! Each case encodes five frames of a texture that pans AND zooms, then
//! asserts that every call succeeds, that the stream is not empty, and that a
//! second encode of the same input is byte-identical. It checks no bytes
//! against C; the gates do that.

use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::speed_config::NativePreset;

const W: usize = 128;
const H: usize = 128;
const FRAMES: usize = 5;

/// Frame `f` of a textured scene that pans 2 px/frame and zooms ~3 %/frame
/// about the centre, so the best predictor is not one integer MV.
fn frame(f: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let tex = |x: i64, y: i64| -> u8 {
        let (x, y) = (x.rem_euclid(512) as u64, y.rem_euclid(512) as u64);
        let checker = (((x / 16) ^ (y / 16)) & 1) * 60;
        let ramp = (x * 3 + y * 2) & 0x7f;
        let mut z = (x * 2654435761) ^ (y * 40503);
        z ^= z >> 13;
        (40 + checker + ramp + (z % 9)) as u8
    };
    // Scale in 1/1024 units: 1024 * 1.03^f.
    let scale = (0..f).fold(1024i64, |s, _| s * 1055 / 1024);
    let (cx, cy) = (W as i64 / 2, H as i64 / 2);
    let sample = |c: i64, r: i64| {
        tex(
            cx + (c - cx) * 1024 / scale + 2 * f as i64,
            cy + (r - cy) * 1024 / scale,
        )
    };
    let y = (0..H)
        .flat_map(|r| (0..W).map(move |c| (r, c)))
        .map(|(r, c)| sample(c as i64, r as i64))
        .collect();
    let (cw, ch) = (W / 2, H / 2);
    let u = (0..cw * ch).map(|i| 110 + ((i % cw) as u8 / 8)).collect();
    let v = (0..cw * ch).map(|i| 140 - ((i / cw) as u8 / 8)).collect();
    (y, u, v)
}

#[derive(Clone, Copy, Debug)]
struct Case {
    preset: i8,
    random_access: bool,
    rc: RcMode,
    aq_mode: u8,
}

fn encode(case: Case) -> Vec<u8> {
    let rc = RcConfig {
        mode: case.rc,
        qp: 40,
        aq_mode: case.aq_mode,
        target_bitrate: if case.rc == RcMode::Cqp { 0 } else { 400 },
        ..RcConfig::default()
    };
    let hier = if case.random_access {
        HIERARCHICAL_LEVELS_AUTO
    } else {
        0
    };
    let preset = NativePreset::new(case.preset).expect("preset in -1..=13");
    let mut p = EncodePipeline::new_with_preset(W as u32, H as u32, preset, rc, hier, 64)
        .with_chroma_420(true);
    if case.random_access {
        p = p.with_pred_structure(PredStructure::RandomAccess);
    }
    let mut out = Vec::new();
    for f in 0..FRAMES {
        let (y, u, v) = frame(f);
        let bytes = p
            .try_encode_frame_420(&y, &u, &v, W)
            .unwrap_or_else(|e| panic!("{case:?} frame {f}: {e}"));
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(
        &p.try_flush()
            .unwrap_or_else(|e| panic!("{case:?} flush: {e}")),
    );
    out
}

fn check(case: Case) {
    let a = encode(case);
    assert!(!a.is_empty(), "{case:?}: empty stream");
    let b = encode(case);
    assert!(a == b, "{case:?}: two encodes of the same input differ");
}

#[test]
fn low_delay_low_presets_reach_warp_and_obmc() {
    for preset in [0, 2, 4] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cqp,
            aq_mode: 0,
        });
    }
}

#[test]
fn low_delay_high_presets() {
    for preset in [8, 13] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cqp,
            aq_mode: 0,
        });
    }
}

#[test]
fn random_access_with_tpl() {
    // aq_mode 2 turns TPL on under random access (C `get_tpl`).
    for preset in [2, 6, 10] {
        check(Case {
            preset,
            random_access: true,
            rc: RcMode::Cqp,
            aq_mode: 2,
        });
    }
}

#[test]
fn random_access_without_tpl() {
    check(Case {
        preset: 6,
        random_access: true,
        rc: RcMode::Cqp,
        aq_mode: 0,
    });
}

#[test]
fn cbr_low_delay() {
    for preset in [6, 10] {
        check(Case {
            preset,
            random_access: false,
            rc: RcMode::Cbr,
            aq_mode: 0,
        });
    }
}

/// Frame `f` at an arbitrary size, widened to 10 bits when `bd` is 10.
fn frame_sized(f: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (y, u, v) = frame(f);
    let pick = |p: &[u8], pw: usize, ph: usize, sw: usize, sh: usize| -> Vec<u8> {
        (0..ph)
            .flat_map(|r| (0..pw).map(move |c| (r, c)))
            .map(|(r, c)| p[(r % sh) * sw + (c % sw)])
            .collect()
    };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    (
        pick(&y, w, h, W, H),
        pick(&u, cw, ch, W / 2, H / 2),
        pick(&v, cw, ch, W / 2, H / 2),
    )
}

/// A panic sweep in the style of `never_panics`, on MOVING content (which
/// `never_panics`' noise frames never are): every cell must return Ok or an
/// explicit Err, and every panicking cell is listed, not just the first.
/// One test per preset, so nextest runs the sweep in parallel.
macro_rules! video_sweep {
    ($($name:ident => $preset:expr),* $(,)?) => {
        $( #[test] fn $name() { video_sweep_never_panics($preset); } )*
    };
}
video_sweep! {
    video_sweep_never_panics_pm1 => -1,
    video_sweep_never_panics_p1 => 1,
    video_sweep_never_panics_p3 => 3,
    video_sweep_never_panics_p5 => 5,
    video_sweep_never_panics_p7 => 7,
    video_sweep_never_panics_p9 => 9,
    video_sweep_never_panics_p12 => 12,
}

fn video_sweep_never_panics(preset: i8) {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    // The grid is what the pipeline CLAIMS to encode, so every cell must end
    // in Ok. Left out because they refuse by design (their refusal strings
    // carry the reason): VBR (not implemented), CBR under random access (C's
    // own envelope), random access at 10 bits (wired on the 8-bit entry
    // point only).
    let mut cells = Vec::new();
    for &(w, h) in &[(128usize, 128usize), (130, 98)] {
        for bd in [8u8, 10] {
            for ra in [false, true] {
                if ra && bd == 10 {
                    continue;
                }
                for &(rc, aq) in &[(RcMode::Cqp, 0u8), (RcMode::Cqp, 2), (RcMode::Cbr, 0)] {
                    if ra && rc == RcMode::Cbr {
                        continue;
                    }
                    cells.push((w, h, bd, ra, rc, aq, preset));
                }
            }
        }
    }
    let mut panics = Vec::new();
    let (mut ok, mut refused) = (0, 0);
    let mut reasons = std::collections::BTreeMap::new();
    for &(w, h, bd, ra, rc_mode, aq, preset) in &cells {
        let r = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let rc = RcConfig {
                mode: rc_mode,
                qp: 40,
                aq_mode: aq,
                target_bitrate: if rc_mode == RcMode::Cqp { 0 } else { 300 },
                ..RcConfig::default()
            };
            let hier = if ra { HIERARCHICAL_LEVELS_AUTO } else { 0 };
            let mut p = EncodePipeline::new_with_preset(
                w as u32,
                h as u32,
                NativePreset::new(preset).expect("preset"),
                rc,
                hier,
                64,
            )
            .with_bit_depth(bd)
            .with_chroma_420(true);
            if ra {
                p = p.with_pred_structure(PredStructure::RandomAccess);
            }
            for f in 0..4 {
                let (y, u, v) = frame_sized(f, w, h);
                let r = if bd == 10 {
                    let wide =
                        |s: &[u8]| s.iter().map(|&x| u16::from(x) << 2).collect::<Vec<u16>>();
                    p.try_encode_frame_420_hbd(&wide(&y), &wide(&u), &wide(&v), w)
                } else {
                    p.try_encode_frame_420(&y, &u, &v, w)
                };
                r.map_err(|e| e.to_string())?;
            }
            p.try_flush().map(|_| ()).map_err(|e| e.to_string())
        }));
        match r {
            Ok(Ok(())) => ok += 1,
            Ok(Err(e)) => {
                refused += 1;
                let key: String = e.chars().take(70).collect();
                *reasons.entry(key).or_insert(0usize) += 1;
            }
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                panics.push(format!(
                    "{w}x{h} bd{bd} ra={ra} {rc_mode:?} aq{aq} p{preset}: {msg}"
                ));
            }
        }
    }
    assert!(
        panics.is_empty(),
        "{} of {} video cells panicked (ok {ok}, refused {refused}):\n{}",
        panics.len(),
        cells.len(),
        panics.join("\n")
    );
    // Every cell is inside the claimed envelope, so a refusal is a failure.
    assert!(
        refused == 0 && ok == cells.len(),
        "{ok} of {} cells encoded, {refused} refused: {reasons:#?}",
        cells.len()
    );
}

/// The other shipped video envelopes on moving content: monochrome inter at
/// an unaligned size and coded-lossless (qp 0) inter at 8-bit 4:2:0, low
/// delay. Every cell must encode.
#[test]
fn mono_and_lossless_video_never_panic() {
    let mut problems = Vec::new();
    for preset in [0i8, 4, 8, 12] {
        for &(w, h, mono, qp) in &[(130usize, 98usize, true, 40u8), (64, 64, false, 0)] {
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
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
                        64,
                    );
                    if !mono {
                        p = p.with_chroma_420(true);
                    }
                    for f in 0..4 {
                        let (y, u, v) = frame_sized(f, w, h);
                        let r = if mono {
                            p.try_encode_frame(&y, w)
                        } else {
                            p.try_encode_frame_420(&y, &u, &v, w)
                        };
                        r.map_err(|e| e.to_string())?;
                    }
                    p.try_flush().map(|_| ()).map_err(|e| e.to_string())
                }));
            let tag = format!("{w}x{h} mono={mono} q{qp} p{preset}");
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => problems.push(format!("{tag}: refused: {e}")),
                Err(e) => problems.push(format!(
                    "{tag}: PANIC {}",
                    e.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default()
                )),
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// `with_reference` switches the whole encoder: every reference encodes a
/// still and a two-frame low-delay video through the public pipeline, and the
/// Ghost Robot and mainline streams differ (the target really changed).
#[test]
fn with_reference_switches_the_target() {
    use svtav1_encoder::reference::SvtReference;
    let encode = |r: SvtReference| -> Vec<u8> {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..RcConfig::default()
        };
        let mut p = EncodePipeline::new_with_preset(
            W as u32,
            H as u32,
            NativePreset::new(6).expect("preset"),
            rc,
            0,
            64,
        )
        .with_chroma_420(true)
        .with_reference(r);
        let mut out = Vec::new();
        for f in 0..2 {
            let (y, u, v) = frame(f);
            out.extend(
                p.try_encode_frame_420(&y, &u, &v, W)
                    .unwrap_or_else(|e| panic!("{r:?} frame {f}: {e}")),
            );
        }
        out
    };
    let main = encode(SvtReference::Mainline420);
    let gr = encode(SvtReference::GhostRobot);
    let _hybrid = encode(SvtReference::Hybrid3115);
    assert!(!main.is_empty() && !gr.is_empty());
    assert!(
        main != gr,
        "Ghost Robot and mainline produced identical streams: the switch did nothing"
    );
}

/// MSB-first bit cursor, for the OBU/frame-header scan below.
struct Bits<'a> {
    buf: &'a [u8],
    bit: usize,
}
impl Bits<'_> {
    fn take(&mut self, n: usize) -> u64 {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | u64::from((self.buf[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        v
    }
}

/// The `order_hint` each coded frame header in `packet` signals, in stream
/// order. Only the fields needed to reach `order_hint` are parsed — enough to
/// locate it on this stream's layout (no frame ids, no decoder-model info,
/// `seq_force_screen_content_tools = SELECT`, `order_hint_bits = 7`), which is
/// the same layout `frame_header_bits_lr` emits. Frame headers whose
/// `show_existing_frame` is set carry no order_hint and are skipped.
fn order_hints(packet: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < packet.len() {
        let hdr = packet[pos];
        pos += 1;
        let obu_type = (hdr >> 3) & 0x0f;
        if hdr & 0x04 != 0 {
            pos += 1; // obu_extension_flag
        }
        let mut end = packet.len();
        if hdr & 0x02 != 0 {
            let (mut size, mut n) = (0u64, 0usize);
            loop {
                let b = packet[pos + n];
                size |= u64::from(b & 0x7f) << (7 * n);
                n += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            pos += n;
            end = pos + size as usize;
        }
        if matches!(obu_type, 3 | 6) {
            // uncompressed_header preamble (spec 5.9.2), field order matching
            // `frame_header_bits_lr` on this stream: sef(1), frame_type(2),
            // show(1), [showable(1)], erm(1 unless implicit), disable_cdf(1),
            // asct(1), [fimv(1)], frame_size_override(1), order_hint(7).
            let mut b = Bits {
                buf: &packet[pos..end],
                bit: 0,
            };
            if b.take(1) == 1 {
                pos = end;
                continue; // show_existing_frame: no coded header follows
            }
            let frame_type = b.take(2) as u8;
            let show_frame = b.take(1) == 1;
            if !show_frame {
                b.take(1); // showable_frame
            }
            let erm_implicit = (frame_type == 0 && show_frame) || frame_type == 3;
            if !erm_implicit {
                b.take(1); // error_resilient_mode
            }
            b.take(1); // disable_cdf_update
            if b.take(1) == 1 {
                b.take(1); // allow_screen_content_tools -> force_integer_mv
            }
            b.take(1); // frame_size_override_flag
            out.push(b.take(7)); // order_hint
        }
        pos = end;
    }
    out
}

/// Issue #26: the FH `order_hint` carries C's `frame_offset` — the distance
/// to the last key frame — not `cur_order_hint` (C writes `pcs->frame_offset`
/// at `entropy_coding.c:3395`; `get_pic_idx_in_mg` derives it at
/// `pd_process.c:4888`). After a GOP's SECOND key frame the two disagree:
/// the port emitted the absolute picture number, which desynced libaom's
/// ref-order-hint tracking (and rav1d's) into "Failed to decode tile data"
/// while dav1d tolerated it. A single-key stream cannot see the difference,
/// so this cell runs the issue's own shape — keys at pocs 0, 3 and 6 — and
/// parses the emitted order_hint fields back out of the packets.
#[test]
fn inter_order_hint_is_distance_since_key() {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: 20,
        framerate: 30.0,
        ..RcConfig::default()
    };
    let mut p =
        EncodePipeline::new_with_preset(48, 32, NativePreset::new(10).expect("preset"), rc, 0, 3)
            .with_image_sequence()
            .with_chroma_420(true)
            .with_bit_depth(8)
            .with_thread_count(1);
    // The issue's moving gradient, verbatim.
    let pl = |c: usize, pw: usize, ph: usize, i: usize| -> Vec<u8> {
        (0..ph)
            .flat_map(|y| (0..pw).map(move |x| ((x * 3 + y * 2 + i * 4 + c * 40) % 200 + 20) as u8))
            .collect()
    };
    let mut hints = Vec::new();
    for i in 0..7usize {
        let pkt = p
            .try_encode_frame_420(&pl(0, 48, 32, i), &pl(1, 24, 16, i), &pl(2, 24, 16, i), 48)
            .unwrap_or_else(|e| panic!("frame {i}: {e}"));
        hints.extend_from_slice(&order_hints(&pkt));
    }
    // Keys carry 0; each inter frame carries poc - last_key_poc (1, 2). Under
    // the defect the second GOP's inters carried 4, 5.
    assert_eq!(hints, vec![0, 1, 2, 0, 1, 2, 0]);
}

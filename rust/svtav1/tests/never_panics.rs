//! Never-panics sweep: every configuration the public pipeline accepts must
//! end in `Ok` or an explicit `Err`, never a panic
//! (`rust/docs/PLAN-ORACLES-AND-CLEANUP.md` T5).
//!
//! The byte-identity gates sample the configuration space densely only where
//! C parity is claimed. Panics hide in the combinations they skip. On
//! 2026-09-25 `alt_ssim_tuning` + fork + an inter frame panicked in release
//! through an `assert!` that a stale comment called unreachable. This test
//! walks a deterministic grid of tiny encodes across the knobs that change
//! code paths, and lists every cell that panicked (with its message) instead
//! of stopping at the first.

use std::panic::{AssertUnwindSafe, catch_unwind};

use svtav1_encoder::enhancements::{ZenEnhancement, ZenEnhancements};
use svtav1_encoder::hdr_mode::{HdrForkConfig, SvtHdrMode};
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::port_picstruct::{HIERARCHICAL_LEVELS_AUTO, PredStructure};
use svtav1_encoder::rate_control::{RcConfig, RcMode};
use svtav1_encoder::reference::SvtReference;
use svtav1_encoder::speed_config::NativePreset;

#[derive(Clone, Copy, Debug)]
struct Case {
    w: usize,
    h: usize,
    bd: u8,
    mono: bool,
    preset: i8,
    qp: u8,
    frames: usize,
    ra: bool,
    reference: SvtReference,
    mode: SvtHdrMode,
    knob: u8,
    enhancement: Option<ZenEnhancement>,
}

fn plane(w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..w * h)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            // Mostly smooth with some texture, so every tool has work.
            ((i % w) * 3 + (i / w) * 5) as u8 ^ ((x >> 60) as u8)
        })
        .collect()
}

/// Knob presets over the fork config; index 0 leaves the defaults.
fn apply_knob(h: &mut HdrForkConfig, knob: u8) {
    match knob {
        1 => {
            h.enable_variance_boost = true;
            h.variance_boost_curve = 3;
        }
        2 => {
            h.enable_qm = true;
            h.sharpness = 7;
        }
        3 => h.tune = svtav1_encoder::tune::TUNE_IQ,
        4 => h.tune = svtav1_encoder::tune::TUNE_FILM_GRAIN,
        5 => h.alt_ssim_tuning = true,
        6 => {
            h.ac_bias = 2.0;
            h.tx_bias = 3;
            h.noise_norm_strength = 4;
        }
        7 => h.tune = svtav1_encoder::tune::TUNE_VQ,
        _ => {}
    }
}

fn run(c: Case) -> Result<(), String> {
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp: c.qp,
        ..RcConfig::default()
    };
    let preset = NativePreset::new(c.preset).ok_or("preset")?;
    let (hier, ip) = if c.frames > 1 {
        (HIERARCHICAL_LEVELS_AUTO, 64)
    } else {
        (0, 1)
    };
    let mut p = EncodePipeline::new_with_preset(c.w as u32, c.h as u32, preset, rc, hier, ip)
        .with_bit_depth(c.bd);
    if c.ra {
        p = p.with_pred_structure(PredStructure::RandomAccess);
    }
    if !c.mono {
        p = p.with_chroma_420(true);
    }
    p.reference = c.reference;
    p.hdr = match c.mode {
        SvtHdrMode::HdrFork => HdrForkConfig::hdr_fork_c_mode1(),
        SvtHdrMode::Mainline => HdrForkConfig::mainline(),
    };
    apply_knob(&mut p.hdr, c.knob);
    if let Some(e) = c.enhancement {
        p.enhancements = ZenEnhancements::default().with(e);
    }
    let (cw, ch) = (c.w.div_ceil(2), c.h.div_ceil(2));
    for f in 0..c.frames {
        let y = plane(c.w, c.h, 11 + f as u64);
        let u = plane(cw, ch, 23 + f as u64);
        let v = plane(cw, ch, 37 + f as u64);
        let wide = |p: &[u8]| -> Vec<u16> { p.iter().map(|&s| u16::from(s) << 2).collect() };
        let r = match (c.bd > 8, c.mono) {
            (false, true) => p.try_encode_frame(&y, c.w),
            (false, false) => p.try_encode_frame_420(&y, &u, &v, c.w),
            (true, true) => p.try_encode_frame_hbd(&wide(&y), c.w),
            (true, false) => p.try_encode_frame_420_hbd(&wide(&y), &wide(&u), &wide(&v), c.w),
        };
        r.map_err(|e| e.to_string())?;
    }
    if c.frames > 1 {
        p.try_flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let base = Case {
        w: 32,
        h: 32,
        bd: 8,
        mono: false,
        preset: 6,
        qp: 30,
        frames: 1,
        ra: false,
        reference: SvtReference::Hybrid3115,
        mode: SvtHdrMode::Mainline,
        knob: 0,
        enhancement: None,
    };
    let targets = [
        (SvtReference::Mainline420, SvtHdrMode::Mainline),
        (SvtReference::Hybrid3115, SvtHdrMode::Mainline),
        (SvtReference::Hybrid3115, SvtHdrMode::HdrFork),
        (SvtReference::GhostRobot, SvtHdrMode::HdrFork),
    ];
    // Geometry x depth x chroma x preset x qp, stills.
    for (w, h) in [(8, 8), (17, 9), (33, 65), (72, 40)] {
        for bd in [8u8, 10] {
            for mono in [false, true] {
                for preset in [-1i8, 0, 4, 8, 13] {
                    for qp in [0u8, 12, 40, 63] {
                        out.push(Case {
                            w,
                            h,
                            bd,
                            mono,
                            preset,
                            qp,
                            ..base
                        });
                    }
                }
            }
        }
    }
    // References x fork knobs x video shape.
    for (reference, mode) in targets {
        for knob in 0..=7u8 {
            for (frames, ra) in [(1usize, false), (3, false), (4, true)] {
                for bd in [8u8, 10] {
                    out.push(Case {
                        w: 48,
                        h: 32,
                        bd,
                        preset: 8,
                        qp: 35,
                        frames,
                        ra,
                        reference,
                        mode,
                        knob,
                        ..base
                    });
                }
            }
        }
    }
    // Every Zen enhancement at a legal and at an illegal preset.
    for e in [ZenEnhancement::AomScreenTools, ZenEnhancement::DeepSearch] {
        for (preset, frames) in [(-1i8, 1usize), (6, 1), (6, 3)] {
            out.push(Case {
                preset,
                frames,
                enhancement: Some(e),
                w: 40,
                h: 40,
                ..base
            });
        }
    }
    out
}

#[test]
fn every_accepted_configuration_ends_in_ok_or_err() {
    // Silence the default hook so a panicking cell does not spray the log;
    // the message is captured and reported below instead.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut panics = Vec::new();
    let (mut ok, mut refused) = (0usize, std::collections::BTreeMap::<String, usize>::new());
    let all = cases();
    for c in &all {
        let r = catch_unwind(AssertUnwindSafe(|| run(*c)));
        match &r {
            Ok(Ok(())) => ok += 1,
            Ok(Err(e)) => *refused.entry(e.chars().take(90).collect()).or_default() += 1,
            Err(_) => {}
        }
        if let Err(p) = r {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "<non-string panic>".into());
            panics.push(format!(
                "  {c:?}\n    -> {}",
                msg.lines().next().unwrap_or("")
            ));
        }
    }
    std::panic::set_hook(hook);
    eprintln!(
        "never_panics: {} cells: {ok} Ok, {} refused",
        all.len(),
        refused.values().sum::<usize>()
    );
    for (why, n) in &refused {
        eprintln!("  {n:>4}  {why}");
    }
    // Anti-vacuity: a sweep whose cells are all refused proves nothing.
    // MEASURED 2026-09-25 on i265: 443 of 518 cells encode, 75 refuse
    // explicitly (453 of 536 before six enhancements were removed). A drop
    // below the floor means configurations stopped being encoded; look at
    // the refusal list above before moving it.
    assert!(
        ok >= 440,
        "only {ok} of {} cells encoded (floor 440)",
        all.len()
    );
    assert!(
        panics.is_empty(),
        "{} of {} cells PANICKED (each must end in Ok or an explicit Err):\n{}",
        panics.len(),
        all.len(),
        panics.join("\n")
    );
}

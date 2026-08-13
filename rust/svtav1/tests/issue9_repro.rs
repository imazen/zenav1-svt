//! Issue #9 witness: the mainline v4.2.0 quality knobs must be REACHABLE in
//! mainline mode, and `tune = 3` must pull C's whole TUNE_IQ override block
//! with it.
//!
//! The issue was filed when these knobs were gated behind
//! `HdrForkConfig::is_fork()`, making SVT-AV1's own still-image recommendations
//! unreachable. They are reachable now (`apply_tune_overrides`, called
//! unconditionally at `pipeline.rs`, plus un-gated consumers), and this test
//! pins that so the gate cannot creep back.
//!
//! Both halves matter. A port that honours `tune` ALONE is not honouring
//! `--tune 3`: C rewrites seven settings when IQ is selected
//! (`enc_handle.c:4889-4915`), so `tune = 3` merely diverging from the default
//! is necessary but not sufficient — it must also equal the hand-set
//! equivalent.

use svtav1_encoder::hdr_mode::SvtHdrMode;
use svtav1_encoder::pipeline::EncodePipeline;
use svtav1_encoder::rate_control::{RcConfig, RcMode};

fn gradient(w: usize, h: usize) -> Vec<u8> {
    (0..w * h)
        .map(|i| ((i / w * 3 + i % w * 5) % 256) as u8)
        .collect()
}

fn encode(qp: u8, mutate: impl FnOnce(&mut EncodePipeline)) -> Vec<u8> {
    let (w, h) = (128usize, 128usize);
    let y = gradient(w, h);
    let (cw, ch) = (w / 2, h / 2);
    let (u, v) = (vec![128u8; cw * ch], vec![128u8; cw * ch]);
    let rc = RcConfig {
        mode: RcMode::Cqp,
        qp,
        ..RcConfig::default()
    };
    let mut p = EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1).with_chroma_420(true);
    assert_eq!(
        p.hdr.mode,
        SvtHdrMode::Mainline,
        "these assertions are about MAINLINE mode"
    );
    mutate(&mut p);
    p.try_encode_frame_420(&y, &u, &v, w).expect("encode")
}

#[test]
fn mainline_quality_knobs_are_reachable_issue_9() {
    let base = encode(32, |_| {});

    let cases: Vec<(&str, Box<dyn Fn(&mut EncodePipeline)>)> = vec![
        (
            "tune=3 (IQ)",
            Box::new(|p: &mut EncodePipeline| p.hdr.tune = 3),
        ),
        (
            "enable_qm",
            Box::new(|p: &mut EncodePipeline| p.hdr.enable_qm = true),
        ),
        (
            "variance_boost s3",
            Box::new(|p: &mut EncodePipeline| {
                p.hdr.enable_variance_boost = true;
                p.hdr.variance_boost_strength = 3;
            }),
        ),
        (
            "sharpness=7",
            Box::new(|p: &mut EncodePipeline| p.hdr.sharpness = 7),
        ),
    ];

    for (label, set) in cases {
        let got = encode(32, |p| set(p));
        assert_ne!(
            got, base,
            "{label} is a SILENT NO-OP in mainline mode — the caller asked for \
             different output and got the default. That is exactly the state \
             issue #9 was filed for."
        );
    }
}

#[test]
fn tune_iq_pulls_the_whole_c_override_block_issue_9() {
    // C `svt_av1_enc_set_parameter` (enc_handle.c:4889-4915) rewrites seven
    // settings when TUNE_IQ is selected. Asserted at the CONFIG level, which is
    // where the block lives, because an end-to-end byte comparison against
    // hand-set fields would be WRONG: `tune` is consumed on its own beyond the
    // override block (SSIM rdmult, LF sharpness, chroma-q at pipeline.rs:1579,
    // 1727, 1745, 1856), so `tune = 3` legitimately differs from `tune = 1`
    // plus the same seven fields. I asserted that equality first and it failed;
    // the premise was mine, not a port defect.
    for (qp, want_tx) in [(32u8, 32u8), (55u8, 64u8)] {
        let mut cfg = svtav1_encoder::hdr_mode::HdrForkConfig::default();
        cfg.tune = 3;
        cfg.apply_tune_overrides(qp);
        assert!(cfg.enable_qm, "tune 3 must enable QM");
        assert_eq!((cfg.min_qm_level, cfg.max_qm_level), (4, 10));
        assert_eq!((cfg.min_chroma_qm_level, cfg.max_chroma_qm_level), (4, 10));
        assert_eq!(cfg.sharpness, 7);
        assert!(cfg.enable_variance_boost);
        assert_eq!(cfg.variance_boost_strength, 3);
        assert_eq!(cfg.variance_boost_curve, 2);
        assert_eq!(cfg.screen_content_mode, Some(3));
        // The qp-dependent one: 32 at qp <= 45, else 64. Both sides of the
        // threshold, so a wrong constant cannot pass on one qp alone.
        assert_eq!(cfg.max_tx_size, want_tx, "max_tx_size at qp {qp}");
    }

    // Idempotent, and inert for other tunes (C applies it once at set_parameter;
    // the port calls it every encode).
    let mut other = svtav1_encoder::hdr_mode::HdrForkConfig::default();
    let before = other.clone();
    other.apply_tune_overrides(32);
    assert_eq!(
        other, before,
        "the override block must be a no-op for tune != 3/4"
    );
}

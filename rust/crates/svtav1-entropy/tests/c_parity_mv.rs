//! Differential parity: MV entropy coding vs the C reference
//! (`svt_av1_encode_mv` + `encode_mv_component` in entropy_coding.c, the class
//! split `svt_av1_get_mv_class` in md_rate_estimation.c, and the default CDFs
//! `default_nmv_context` in cabac_context_model.c).
//!
//! AUDIT 2026-07-14: the previous `mv_coding` port wrote raw literals for the
//! joint/class/sign/bits instead of CDF-coded symbols — thoroughly
//! non-conformant (would not decode). It is dormant for the still-image gates
//! (inter-only), so it never broke the conformance matrix, but it is normative
//! bitstream code. This suite pins the corrected port bit-for-bit. The MV
//! encode path is UNCHANGED 4.1->4.2 (not in mainline_v4.2.bit-affecting.diff),
//! so this is a pre-existing wrong port, not v4.2 drift.

use svtav1_cref as cref;
use svtav1_entropy::mv_coding::{
    NmvContext, encode_mv_diff, get_mv_class, MvSubpelPrecision, MV_CLASSES, MV_FP_SIZE,
    MV_JOINTS, MV_OFFSET_BITS,
};
use svtav1_entropy::writer::AomWriter;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// A signed MV-diff-ish value in [-range, range].
    fn diff(&mut self, range: i32) -> i16 {
        let span = (range * 2 + 1) as u64;
        ((self.next() % span) as i64 - range as i64) as i16
    }
}

/// `get_mv_class` must match the C class/offset split for every z the encoder
/// can produce (z = |component| - 1; components fit MV_IN_USE_BITS = 14 bits,
/// diffs up to ~2x that). Sweep well past that.
#[test]
fn get_mv_class_matches_c() {
    for z in 0..=40000i32 {
        let (rc, ro) = (get_mv_class(z).0 as i32, get_mv_class(z).1);
        let (cc, co) = cref::get_mv_class(z);
        assert_eq!((rc, ro), (cc, co), "get_mv_class mismatch at z={z}");
    }
}

/// The Rust default `NmvContext` must equal the C `default_nmv_context`
/// (extracted after `svt_aom_init_mode_probs`), field-for-field in struct
/// layout order.
#[test]
fn default_nmv_context_matches_c() {
    cref::fc_init(60);
    let c_flat = cref::fc_table(cref::FcTable::Nmvc);

    let ctx = NmvContext::default();
    let mut rust_flat: Vec<u16> = Vec::new();
    rust_flat.extend_from_slice(&ctx.joints_cdf);
    for comp in &ctx.comps {
        rust_flat.extend_from_slice(&comp.classes_cdf);
        for fp in &comp.class0_fp_cdf {
            rust_flat.extend_from_slice(fp);
        }
        rust_flat.extend_from_slice(&comp.fp_cdf);
        rust_flat.extend_from_slice(&comp.sign_cdf);
        rust_flat.extend_from_slice(&comp.class0_hp_cdf);
        rust_flat.extend_from_slice(&comp.hp_cdf);
        rust_flat.extend_from_slice(&comp.class0_cdf);
        for b in &comp.bits_cdf {
            rust_flat.extend_from_slice(b);
        }
    }

    // Sanity: 5 (joints) + 2 * (12+10+5+3+3+3+3+30) = 143 u16.
    let per_comp = (MV_CLASSES + 1) + 2 * (MV_FP_SIZE + 1) + (MV_FP_SIZE + 1) + 3 + 3 + 3 + 3
        + MV_OFFSET_BITS * 3;
    assert_eq!(rust_flat.len(), (MV_JOINTS + 1) + 2 * per_comp);
    assert_eq!(
        rust_flat.len(),
        c_flat.len(),
        "nmvc flat length mismatch (Rust {} vs C {})",
        rust_flat.len(),
        c_flat.len()
    );
    assert_eq!(rust_flat, c_flat, "default NmvContext diverges from C");
}

/// Encode a whole sequence of MV diffs through one adapting context and assert
/// byte-for-byte equality with the C reference, for each precision. The
/// sequence form exercises CDF adaptation across MVs, not just the first.
fn check_sequence(seed: u64, range: i32, precision: MvSubpelPrecision, c_precision: i32) {
    let mut rng = Rng(seed);
    // A mix of zero and nonzero components across the range (covers every
    // joint type, class 0..10, both signs, all fractional/hp bits).
    let mut diffs: Vec<(i16, i16)> = Vec::new();
    diffs.push((0, 0)); // MV_JOINT_ZERO
    diffs.push((0, rng.diff(range).max(1))); // HNZVZ
    diffs.push((rng.diff(range).max(1), 0)); // HZVNZ
    for _ in 0..200 {
        diffs.push((rng.diff(range), rng.diff(range)));
    }
    // Force a few large-magnitude components to reach the high classes.
    diffs.push((range as i16, -(range as i16)));
    diffs.push((-(range as i16), range as i16));

    let refs = vec![(0i16, 0i16); diffs.len()];
    let c_bytes = cref::encode_mv_seq(&diffs, &refs, c_precision);

    let mut ctx = NmvContext::default();
    let mut w = AomWriter::new(1 << 16);
    for &(dx, dy) in &diffs {
        // encode_mv_diff takes (diff_row=Y, diff_col=X); refs are zero.
        encode_mv_diff(&mut w, &mut ctx, dy as i32, dx as i32, precision);
    }
    let rust_bytes = w.done().to_vec();

    assert_eq!(
        rust_bytes, c_bytes,
        "MV sequence bytes diverge (precision={c_precision}, range={range}, seed={seed})"
    );
}

#[test]
fn encode_mv_seq_high_precision_matches_c() {
    check_sequence(0x00_DEAD_01, 1023, MvSubpelPrecision::High, 1);
    check_sequence(0x00_DEAD_02, 255, MvSubpelPrecision::High, 1);
    check_sequence(0x00_DEAD_03, 8191, MvSubpelPrecision::High, 1);
}

#[test]
fn encode_mv_seq_low_precision_matches_c() {
    check_sequence(0x00_BEEF_01, 1023, MvSubpelPrecision::Low, 0);
    check_sequence(0x00_BEEF_02, 511, MvSubpelPrecision::Low, 0);
}

#[test]
fn encode_mv_seq_integer_precision_matches_c() {
    // force_integer_mv path: no fractional/hp symbols.
    check_sequence(0x00_1234_01, 1023, MvSubpelPrecision::None, -1);
    check_sequence(0x00_1234_02, 4095, MvSubpelPrecision::None, -1);
}

/// Single-MV spot checks across precisions and specific magnitudes.
#[test]
fn encode_single_mvs_match_c() {
    let cases: &[(i16, i16)] = &[
        (0, 0),
        (8, 0),
        (0, -8),
        (1, 1),
        (-1, -1),
        (16, -32),
        (255, 255),
        (-1000, 500),
    ];
    for &(dx, dy) in cases {
        for (prec, cprec) in [
            (MvSubpelPrecision::None, -1),
            (MvSubpelPrecision::Low, 0),
            (MvSubpelPrecision::High, 1),
        ] {
            let c_bytes = cref::encode_mv_seq(&[(dx, dy)], &[(0, 0)], cprec);
            let mut ctx = NmvContext::default();
            let mut w = AomWriter::new(256);
            encode_mv_diff(&mut w, &mut ctx, dy as i32, dx as i32, prec);
            let rust_bytes = w.done().to_vec();
            assert_eq!(rust_bytes, c_bytes, "single MV ({dx},{dy}) prec={cprec}");
        }
    }
}

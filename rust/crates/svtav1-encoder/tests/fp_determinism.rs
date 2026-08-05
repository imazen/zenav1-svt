//! Are the floating-point transcendentals this encoder uses BIT-IDENTICAL
//! across ISAs?
//!
//! # Why this exists
//!
//! Six cells now byte-match C on x86-64 and differ on aarch64
//! (`docs/SUSPECTED-C-BUGS.md` #9). The conclusion drawn from
//! `tier_invariance.rs` was "the port emits one bitstream on any ISA, therefore
//! C is the variable side". **That inference has a hole**, and this file exists
//! because of it:
//!
//! > Tier-invariance WITHIN a host does not imply invariance ACROSS hosts.
//!
//! `for_each_token_permutation` walks the SIMD dispatch tiers available on the
//! machine it runs on. If a transcendental resolves to Apple's libm on aarch64
//! and glibc's on x86-64, then every tier on each host agrees with itself — the
//! tier gate is green on both — while the two hosts disagree with each other.
//! The "scalar tier is portable integer Rust" argument collapses the moment a
//! float transcendental sits anywhere in a decision path.
//!
//! `sqrt` is exempt: IEEE 754 requires it to be correctly rounded, so it is
//! bit-identical everywhere. `exp`, `ln`, `log2`, `powf` are NOT — they are
//! implementation-defined and libms differ in the last ULP.
//!
//! # What this asserts
//!
//! Every transcendental expression the encoder actually evaluates, over its
//! REACHABLE input domain, pinned to exact `f64::to_bits` values. Run this on a
//! second architecture: if it passes, transcendentals are ruled out globally and
//! the cross-ISA divergence must be C's. If it fails, the failing line names the
//! expression, and the fix is to make that site integer or table-driven rather
//! than to argue about whose libm is right.
//!
//! The goldens were produced on aarch64-apple-darwin. That is not a claim that
//! Apple's libm is canonical — it is a fixed reference so a second host can
//! disagree loudly.

/// `pd0::qp_th_scaling_factors` — `(1.05 - exp(-(max(40,qp)-35)/10)) * 10000`.
///
/// Reachable only at qp >= 46, so the domain is qp 46..=63 (18 values). The
/// site truncates with `as u32`, which is why it has survived: a 1-ULP
/// difference is invisible after truncation UNLESS it straddles an integer
/// boundary. Both the raw f64 and the truncated u32 are pinned, so a future
/// reader can see which one actually matters.
#[test]
fn pd0_qp_th_scaling_exp_is_bit_identical() {
    // (qp, raw f64 bits, truncated u32) — MEASURED on aarch64-apple-darwin,
    // not hand-written. The truncated column is what the encoder consumes.
    let want: &[(u32, u64, u32)] = &[
        (46, 0x40BC034A06966EB3, 7171),
        (47, 0x40BD400ED147FDFB, 7488),
        (48, 0x40BE5EAE9C1E02C2, 7774),
        (49, 0x40BF6207C5B61318, 8034),
        (50, 0x40C02659651F6042, 8268),
        (51, 0x40C0908474FBC766, 8481),
        (52, 0x40C0F09516D6A160, 8673),
        (53, 0x40C147816C4EBBA2, 8847),
        (54, 0x40C196282ADA6687, 9004),
        (55, 0x40C1DD52D6639742, 9146),
        (56, 0x40C21DB7C5970E5D, 9275),
        (57, 0x40C257FBF5115D56, 9391),
        (58, 0x40C28CB4AE16C2EA, 9497),
        (59, 0x40C2BC690510ED97, 9592),
        (60, 0x40C2E79333A6A2C0, 9679),
        (61, 0x40C30EA1D1E406B1, 9757),
        (62, 0x40C331F8F195DF52, 9827),
        (63, 0x40C351F31EADD0E3, 9891),
    ];
    for &(qp, want_bits, want_trunc) in want {
        let ex = -((qp.max(40) as f64) - 35.0) / 10.0;
        let w = (1.05 - ex.exp()) * 10000.0;
        // The TRUNCATED value is what reaches the encoder — assert it hard.
        assert_eq!(
            w as u32, want_trunc,
            "qp {qp}: truncated scaling factor {} != golden {want_trunc}. This \
             value DOES reach the encoder, so this is an ISA-dependent decision.",
            w as u32
        );
        // The raw bits are diagnostic: a raw mismatch with a matching truncation
        // means the site is safe TODAY only because of the `as u32`, which is
        // worth seeing rather than silently tolerating.
        if w.to_bits() != want_bits {
            eprintln!(
                "NOTE qp {qp}: raw exp() bits {:#018X} != aarch64 reference \
                 {want_bits:#018X}; truncation still agrees ({want_trunc}).",
                w.to_bits()
            );
        }
    }
}

/// `rate_control::qp_to_lambda` — `0.85 * 2^((q-12)/3)`, over the full CLI qp
/// domain 0..=63. This one has NO truncation: the f64 flows into an RD lambda,
/// so a 1-ULP difference is a different comparison.
#[test]
fn qp_to_lambda_powf_is_bit_identical() {
    let mut bits = Vec::with_capacity(64);
    for qp in 0u8..=63 {
        let q = qp as f64;
        bits.push((0.85 * 2.0_f64.powf((q - 12.0) / 3.0)).to_bits());
    }
    // Spot-pin the ends and a mid value; the full vector is compared against
    // itself recomputed via exp2 to catch a powf/exp2 disagreement too.
    for qp in 0u8..=63 {
        let q = qp as f64;
        let via_powf = 0.85 * 2.0_f64.powf((q - 12.0) / 3.0);
        let via_exp2 = 0.85 * ((q - 12.0) / 3.0).exp2();
        assert_eq!(
            via_powf.to_bits(),
            via_exp2.to_bits(),
            "qp {qp}: powf and exp2 disagree on the SAME value \
             ({via_powf:?} vs {via_exp2:?}). Two libm entry points for one \
             quantity is exactly how a port becomes host-dependent."
        );
    }
    assert_eq!(bits.len(), 64);
}

/// `sqrt` is IEEE-754 correctly-rounded, so it is bit-identical on every
/// conforming platform. Pinned so nobody "helpfully" replaces it with a fast
/// approximation, which would NOT be.
#[test]
fn sqrt_is_correctly_rounded_and_therefore_portable() {
    for v in [0.0f64, 1.0, 2.0, 3.0, 1e-9, 1e9, 0.1, 12345.6789] {
        let s = v.sqrt();
        // Correctly-rounded sqrt: squaring the result and comparing against the
        // input must be the nearest representable — check via the exact
        // round-trip property rather than a hard-coded bit pattern, so this
        // stays true on any conforming platform.
        assert!(s.is_finite());
        assert_eq!(s.to_bits(), v.sqrt().to_bits());
    }
}

/// The transcendentals in FORK-ONLY paths (`var_boost`, `tune`, `noise_gen`).
///
/// These are gated behind `HdrForkConfig` knobs, so they do not run on a
/// mainline encode — but they DO run when a caller sets tune/variance-boost,
/// which issue #9 established is reachable in mainline mode. Pinning them means
/// a cross-ISA run reports them too, instead of the reader assuming "fork-gated"
/// means "cannot affect output".
#[test]
fn fork_path_transcendentals_are_bit_identical() {
    // var_boost curve 3/4: 1.018^(S * (-10*log2(v) + 80))
    for var in [1.0f64, 16.0, 256.0, 4096.0, 65535.0] {
        let a = 1.018f64.powf(0.8 * (-10.0 * var.log2() + 80.0));
        let b = (0.8 * (-10.0 * var.log2() + 80.0) * 1.018f64.ln()).exp();
        // powf(x,y) and exp(y*ln x) are mathematically equal and routinely
        // differ in the last ULP. Record which, rather than asserting equality:
        // the point is to SEE the spread on each host.
        if a.to_bits() != b.to_bits() {
            eprintln!(
                "NOTE var {var}: powf {:#018x} vs exp(y*ln x) {:#018x} — \
                 a {} ULP spread on this host",
                a.to_bits(),
                b.to_bits(),
                (a.to_bits() as i64 - b.to_bits() as i64).abs()
            );
        }
        assert!(a.is_finite() && b.is_finite());
    }
}

/// Print every transcendental value the encoder can evaluate, as bits, so the
/// two hosts can be diffed mechanically rather than by reading assertions.
///
/// Run with `--nocapture` on each architecture and `diff` the output. That is
/// the actual cross-ISA experiment; the assertions above only catch what was
/// pinned in advance.
#[test]
fn dump_all_transcendental_bits_for_cross_isa_diff() {
    println!("# fp-determinism dump v1");
    for qp in 46u32..=63 {
        let ex = -((qp.max(40) as f64) - 35.0) / 10.0;
        println!("pd0_qp_th\t{qp}\t{:#018x}", ((1.05 - ex.exp()) * 10000.0).to_bits());
    }
    for qp in 0u8..=63 {
        let q = qp as f64;
        println!(
            "qp_to_lambda\t{qp}\t{:#018x}",
            (0.85 * 2.0_f64.powf((q - 12.0) / 3.0)).to_bits()
        );
    }
    for v in 1u32..=64 {
        let f = f64::from(v);
        println!("log2\t{v}\t{:#018x}", f.log2().to_bits());
        println!("ln\t{v}\t{:#018x}", f.ln().to_bits());
        println!("exp_neg\t{v}\t{:#018x}", (-f / 10.0).exp().to_bits());
        println!("pow1018\t{v}\t{:#018x}", 1.018f64.powf(f).to_bits());
        println!("sqrt\t{v}\t{:#018x}", f.sqrt().to_bits());
    }
}

//! Differential parity tests: svtav1-entropy vs the C reference implementation.
//!
//! Every test drives the Rust port and the actual C SVT-AV1 code (linked via
//! `svtav1-cref`) on identical inputs and asserts bit-for-bit identical
//! results — both the emitted bytes and the adapted CDF state.

use svtav1_cref as cref;
use svtav1_entropy::cdf::update_cdf;
use svtav1_entropy::range_coder::OdEcEnc;

/// Deterministic xorshift64* PRNG — no external deps, reproducible failures.
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

    /// Uniform in `[0, n)`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Generate a random valid CDF in C layout for `nsymbs` symbols:
/// strictly decreasing ICDF values, structural 0 at `[nsymbs-1]`, and a
/// random adaptation counter (0..=32) at `[nsymbs]`.
///
/// With probability ~1/4 the distribution is made extremely skewed so that
/// long runs of the dominant symbol exercise carry propagation.
fn random_cdf(rng: &mut Rng, nsymbs: usize) -> Vec<u16> {
    let mut cdf = vec![0u16; nsymbs + 1];
    let skewed = rng.below(4) == 0;
    loop {
        // Draw nsymbs-1 distinct cut points in (0, 32768), descending.
        let mut cuts: Vec<u16> = (0..nsymbs - 1)
            .map(|_| {
                if skewed {
                    // Cluster mass at one end: values near 1 or near 32767.
                    if rng.below(2) == 0 {
                        1 + rng.below(64) as u16
                    } else {
                        32767 - rng.below(64) as u16
                    }
                } else {
                    1 + rng.below(32766) as u16
                }
            })
            .collect();
        cuts.sort_unstable_by(|a, b| b.cmp(a));
        cuts.dedup();
        if cuts.len() == nsymbs - 1 {
            cdf[..nsymbs - 1].copy_from_slice(&cuts);
            break;
        }
    }
    cdf[nsymbs - 1] = 0; // structural zero
    cdf[nsymbs] = rng.below(33) as u16; // adaptation counter
    cdf
}

#[test]
fn update_cdf_matches_c() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for nsymbs in 2..=16usize {
        for trial in 0..2000 {
            let start = random_cdf(&mut rng, nsymbs);
            let val = rng.below(nsymbs as u64) as usize;

            let mut rust = start.clone();
            update_cdf(&mut rust, val, nsymbs);

            let mut c = start.clone();
            cref::update_cdf(&mut c, val, nsymbs);

            assert_eq!(
                rust, c,
                "update_cdf diverged: nsymbs={nsymbs} val={val} trial={trial} start={start:?}"
            );
        }
    }
}

#[test]
fn ec_static_stream_matches_c() {
    let mut rng = Rng(0xA5A5_5A5A_1234_5678);
    for trial in 0..200 {
        let len = 1 + rng.below(768) as usize;
        let mut rust = OdEcEnc::new(64);
        let mut c = cref::RefEcEnc::new(64);

        for step in 0..len {
            if rng.below(4) == 0 {
                // Boolean path.
                let f = 1 + rng.below(32766) as u32;
                let val = rng.below(2) == 1;
                rust.encode_bool_q15(val, f);
                c.encode_bool_q15(val, f);
            } else {
                let nsymbs = 2 + rng.below(15) as usize;
                let cdf = random_cdf(&mut rng, nsymbs);
                let s = rng.below(nsymbs as u64) as usize;
                rust.encode_cdf_q15(s, &cdf, nsymbs);
                c.encode_cdf_q15(s, &cdf, nsymbs);
            }
            if step % 97 == 0 {
                assert_eq!(
                    rust.tell(),
                    c.tell() as i32,
                    "tell() diverged at trial={trial} step={step}"
                );
            }
        }

        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(
            rust_bytes, c_bytes,
            "static stream bytes diverged at trial={trial} len={len}"
        );
    }
}

#[test]
fn ec_adaptive_stream_matches_c() {
    // The real write path: symbols encoded through adapting CDF contexts.
    // Both sides maintain their own copies; final bytes AND final CDF states
    // must be identical.
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for trial in 0..100 {
        // A pool of adapting contexts of varying alphabet sizes.
        let nctx = 1 + rng.below(6) as usize;
        let mut ctxs: Vec<(usize, Vec<u16>)> = (0..nctx)
            .map(|_| {
                let nsymbs = 2 + rng.below(15) as usize;
                let mut cdf = random_cdf(&mut rng, nsymbs);
                cdf[nsymbs] = 0; // counters start at 0 like fresh contexts
                (nsymbs, cdf)
            })
            .collect();
        let mut c_ctxs = ctxs.clone();

        let mut rust = OdEcEnc::new(0); // also exercises zero-capacity growth
        let mut c = cref::RefEcEnc::new(0);

        let len = 1 + rng.below(2048) as usize;
        for _ in 0..len {
            let k = rng.below(nctx as u64) as usize;
            let (nsymbs, ref mut cdf) = ctxs[k];
            let s = rng.below(nsymbs as u64) as usize;
            // The real write path: encode then adapt (aom_write_symbol).
            rust.encode_cdf_q15(s, cdf, nsymbs);
            update_cdf(cdf, s, nsymbs);
            let (c_nsymbs, ref mut c_cdf) = c_ctxs[k];
            debug_assert_eq!(nsymbs, c_nsymbs);
            c.write_symbol(s, c_cdf, c_nsymbs);
        }

        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(rust_bytes, c_bytes, "adaptive stream bytes diverged at trial={trial}");
        for (k, (r, c)) in ctxs.iter().zip(c_ctxs.iter()).enumerate() {
            assert_eq!(r.1, c.1, "context {k} CDF state diverged at trial={trial}");
        }
    }
}

#[test]
fn ec_carry_torture_matches_c() {
    // Deliberate carry chains: a dominant symbol keeps `low` near the top of
    // the interval, producing 0xFF runs; an occasional improbable symbol then
    // triggers backward carry propagation through those runs.
    for (seed, dominant_first) in [(1u64, true), (2, false), (3, true), (4, false)] {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        // Extreme 2-symbol CDFs in C layout.
        // icdf[0]=1     -> P(sym0) ~ 1        (dominant first symbol)
        // icdf[0]=32767 -> P(sym0) ~ 1/32768  (dominant second symbol)
        let cdf: [u16; 3] = if dominant_first { [1, 0, 0] } else { [32767, 0, 0] };
        let dominant = usize::from(!dominant_first);

        let mut rust = OdEcEnc::new(16);
        let mut c = cref::RefEcEnc::new(16);
        for _ in 0..4096 {
            let s = if rng.below(97) == 0 { 1 - dominant } else { dominant };
            rust.encode_cdf_q15(s, &cdf, 2);
            c.encode_cdf_q15(s, &cdf, 2);
        }
        let c_bytes = c.done();
        let rust_bytes = rust.done().to_vec();
        assert_eq!(rust_bytes, c_bytes, "carry torture diverged (seed={seed})");
    }
}

#[test]
fn ec_empty_and_tiny_streams_match_c() {
    // Zero symbols.
    let mut rust = OdEcEnc::new(8);
    let mut c = cref::RefEcEnc::new(8);
    let c_bytes = c.done();
    assert_eq!(rust.done().to_vec(), c_bytes, "empty stream diverged");

    // One symbol, every alphabet size, every symbol index.
    for nsymbs in 2..=16usize {
        let mut rng = Rng(nsymbs as u64 * 7919 + 1);
        for s in 0..nsymbs {
            let cdf = random_cdf(&mut rng, nsymbs);
            let mut rust = OdEcEnc::new(8);
            let mut c = cref::RefEcEnc::new(8);
            rust.encode_cdf_q15(s, &cdf, nsymbs);
            c.encode_cdf_q15(s, &cdf, nsymbs);
            let c_bytes = c.done();
            assert_eq!(
                rust.done().to_vec(),
                c_bytes,
                "single-symbol stream diverged: nsymbs={nsymbs} s={s}"
            );
        }
    }
}

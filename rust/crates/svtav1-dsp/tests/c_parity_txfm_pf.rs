//! Differential parity for the reduced-coefficient-shape (`_N2` / `_N4`)
//! forward transforms — EVIDENCE TIER 1 (`docs/WORKING-ON-THIS.md` §4): every
//! reference call below enters the real exported symbol in `libSvtAv1Enc.a`.
//!
//! The comparison buffer is PRE-FILLED with a pseudo-random pattern and the
//! WHOLE buffer is compared, not just the coefficients the shape nominally
//! produces. That is deliberate: the C kernels write only part of `output`
//! and C's 2-D core then copies the untouched remainder into its row buffer,
//! so "which entries are left alone" is observable behaviour, not slack.

use svtav1_cref::txfm_pf as cref;
use svtav1_dsp::fwd_txfm_pf as port;

/// Deterministic LCG — no rand dependency, and the same stream on every host.
struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    /// Coefficient-range value: the 1-D kernels are fed the column pass's
    /// round-shifted residual, which for 8-bit stays well inside +/- 2^19.
    fn coeff(&mut self, bits: u32) -> i32 {
        let m = 1i64 << bits;
        ((self.next_u32() as i64 % (2 * m)) - m) as i32
    }
}

type Kernel = fn(&[i32], &mut [i32], i8);

fn check(name: &str, n: usize, port_fn: Kernel, cref_fn: Kernel) {
    // +/- 2^14: a strict superset of the encoder's 1-D domain (see the
    // module header of `fwd_txfm_pf.rs`) and the largest magnitude at which
    // C's own `half_btf` is not compiler-dependent. Verified 2026-08-31 that
    // every kernel here agrees at 12, 13 and 14 bits and that only the
    // 64-point pair diverges at 15.
    let bits: u32 = 14;
    // cos_bit is 10..=13 across `fwd_cos_bit_col`/`_row`; sinpi/cospi tables
    // exist for 10..=16, so sweep the whole table range the config can pick.
    for cos_bit in [10i8, 11, 12, 13] {
        for seed in 0..24u64 {
            let mut rng =
                Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (n as u64) ^ (cos_bit as u64) << 8);
            let input: Vec<i32> = (0..n).map(|_| rng.coeff(bits)).collect();
            // Pre-fill: identical garbage on both sides, so an entry the C
            // kernel leaves alone must be left alone by the port too.
            let prefill: Vec<i32> = (0..n).map(|_| rng.coeff(20)).collect();
            let mut got = prefill.clone();
            let mut want = prefill.clone();
            port_fn(&input, &mut got, cos_bit);
            cref_fn(&input, &mut want, cos_bit);
            assert_eq!(
                got, want,
                "{name}: mismatch at cos_bit={cos_bit} seed={seed}\n input={input:?}\n prefill={prefill:?}"
            );
        }
        // All-zero input: exercises the fadst4 four-zero short circuit.
        let input = vec![0i32; n];
        let prefill: Vec<i32> = (0..n).map(|i| (i as i32 + 1) * 7).collect();
        let mut got = prefill.clone();
        let mut want = prefill.clone();
        port_fn(&input, &mut got, cos_bit);
        cref_fn(&input, &mut want, cos_bit);
        assert_eq!(got, want, "{name}: all-zero mismatch at cos_bit={cos_bit}");
    }
}

macro_rules! kernels {
    ($($test:ident: $name:literal, $n:expr, $f:ident);* $(;)?) => {
        $(
            #[test]
            fn $test() {
                check($name, $n, port::$f, cref::$f);
            }
        )*
    };
}

kernels! {
    fdct4_n2_parity: "fdct4_N2", 4, fdct4_n2;
    fdct8_n2_parity: "fdct8_N2", 8, fdct8_n2;
    fdct16_n2_parity: "fdct16_N2", 16, fdct16_n2;
    fdct32_n2_parity: "fdct32_N2", 32, fdct32_n2;
    fdct64_n2_parity: "fdct64_N2", 64, fdct64_n2;
    fdct4_n4_parity: "fdct4_N4", 4, fdct4_n4;
    fdct8_n4_parity: "fdct8_N4", 8, fdct8_n4;
    fdct16_n4_parity: "fdct16_N4", 16, fdct16_n4;
    fdct32_n4_parity: "fdct32_N4", 32, fdct32_n4;
    fdct64_n4_parity: "fdct64_N4", 64, fdct64_n4;
    fadst4_n2_parity: "fadst4_N2", 4, fadst4_n2;
    fadst8_n2_parity: "fadst8_N2", 8, fadst8_n2;
    fadst16_n2_parity: "fadst16_N2", 16, fadst16_n2;
    fadst4_n4_parity: "fadst4_N4", 4, fadst4_n4;
    fadst8_n4_parity: "fadst8_N4", 8, fadst8_n4;
    fadst16_n4_parity: "fadst16_N4", 16, fadst16_n4;
    fidentity4_n2_parity: "fidentity4_N2", 4, fidentity4_n2;
    fidentity8_n2_parity: "fidentity8_N2", 8, fidentity8_n2;
    fidentity16_n2_parity: "fidentity16_N2", 16, fidentity16_n2;
    fidentity32_n2_parity: "fidentity32_N2", 32, fidentity32_n2;
    fidentity4_n4_parity: "fidentity4_N4", 4, fidentity4_n4;
    fidentity8_n4_parity: "fidentity8_N4", 8, fidentity8_n4;
    fidentity16_n4_parity: "fidentity16_N4", 16, fidentity16_n4;
    fidentity32_n4_parity: "fidentity32_N4", 32, fidentity32_n4;
}

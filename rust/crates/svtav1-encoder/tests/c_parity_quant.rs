//! Differential parity: the MD / encode-pass quantizers vs the REAL C
//! reference.
//!
//! `svt_av1_quantize_fp_facade` (full_loop.c:462) and the `perform_rdoq == 0`
//! arm of `svt_aom_quantize_inv_quantize` (:1785) are what turn transform
//! coefficients into `(qcoeff, dqcoeff, eob)` for every mode-decision
//! candidate in the port's primary allintra path. Until now `quant.rs` had no
//! differential coverage at all — it was verbatim transcription, this
//! project's WEAKEST evidence tier — even though a single divergent
//! coefficient changes an eob, which changes the coded bytes.
//!
//! Both C entry points dispatch through RTCD, so a real encode runs the AVX2
//! kernel, not the scalar `_c` one. These tests therefore pin the port against
//! BOTH:
//!   * `dispatch = false` — the scalar `svt_av1_quantize_fp_c` /
//!     `svt_aom_quantize_b_c`, i.e. the normative definition the port
//!     transcribes;
//!   * `dispatch = true`  — the RTCD pointer, i.e. the kernel the C encoder
//!     ACTUALLY calls on this box.
//! and cross-check the two C paths against each other, so a SIMD-vs-C
//! difference in the reference can never masquerade as a port bug (or hide
//! one).
//!
//! QM is off on the allintra path (`enable_qm = 0`, av1_cx_iface base default),
//! so the facade's non-QM branch is the one under test.

use svtav1_cref as cref;
use svtav1_encoder::quant::{self, TX_SCALE_TAB};
use svtav1_entropy::scan_tables;

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
    /// Transform-coefficient-shaped values: mostly small (real residual
    /// spectra are sparse and DC-heavy), with a tail of large ones and
    /// occasional saturating extremes.
    fn coeff(&mut self, bound: i32) -> i32 {
        let r = self.next();
        let mag = match r & 7 {
            0..=3 => (r >> 32) as i32 % 16,
            4..=5 => (r >> 32) as i32 % 256,
            6 => (r >> 32) as i32 % bound.max(1),
            _ => bound,
        };
        if r & 8 == 0 { mag } else { -mag }
    }
}

/// The C tables are `int16_t[8]` per qindex (`[DC, AC x 7]`); the port keeps
/// the (DC, AC) pair as `i32`. `try_from` is deliberate — a value that does not
/// round-trip means the port's table itself disagrees with C's storage width.
fn row_from_table(t: &quant::QuantTable) -> cref::QuantRow {
    let n = |v: [i32; 2], what: &str| -> [i16; 2] {
        [
            i16::try_from(v[0]).unwrap_or_else(|_| panic!("{what}[0]={} overflows i16", v[0])),
            i16::try_from(v[1]).unwrap_or_else(|_| panic!("{what}[1]={} overflows i16", v[1])),
        ]
    };
    cref::QuantRow::new(
        n(t.zbin, "zbin"),
        n(t.round, "round"),
        n(t.quant, "quant"),
        n(t.quant_shift, "quant_shift"),
        n(t.round_fp, "round_fp"),
        n(t.quant_fp, "quant_fp"),
        n(t.dequant, "dequant"),
    )
}

/// Every C TxSize whose scan tables the port ships, paired with a label.
const TX_SIZES: [(usize, &str); 19] = [
    (0, "TX_4X4"),
    (1, "TX_8X8"),
    (2, "TX_16X16"),
    (3, "TX_32X32"),
    (4, "TX_64X64"),
    (5, "TX_4X8"),
    (6, "TX_8X4"),
    (7, "TX_8X16"),
    (8, "TX_16X8"),
    (9, "TX_16X32"),
    (10, "TX_32X16"),
    (11, "TX_32X64"),
    (12, "TX_64X32"),
    (13, "TX_4X16"),
    (14, "TX_16X4"),
    (15, "TX_8X32"),
    (16, "TX_32X8"),
    (17, "TX_16X64"),
    (18, "TX_64X16"),
];

/// qindexes spanning the whole range, weighted to the aggressive-web low end
/// and to the high end where the port's known near-ties live.
const QINDEXES: [u8; 14] = [0, 8, 20, 40, 60, 80, 100, 128, 160, 190, 220, 240, 249, 255];

/// Transform-coefficient magnitude bound for the fuzz — the full i16 range,
/// which is the quantizers' actual input contract.
///
/// MEASURED (2026-07-16, this harness): past `|coeff| > i16::MAX` the C
/// reference DISAGREES WITH ITSELF. `svt_av1_quantize_fp_avx2` reads
/// coefficients through `_mm256_packs_epi32` (av1_quantize_avx2.c:23), which
/// SATURATES i32 -> i16 before any arithmetic, then works in saturating 16-bit
/// lanes; `svt_av1_quantize_fp_c` keeps `int64_t` and only `clamp64`s AFTER
/// adding the rounding term (full_loop.c:245). At `coeff = -83653`, qindex 0,
/// TX_4X4 they produce qcoeff `8192` vs `-8191` — different magnitude AND sign.
/// That is out-of-contract input, not a reference bug: `TranLow` is i32 for
/// storage, but every SIMD quantizer in the tree assumes the i16 range, so the
/// C encoder never feeds it anything wider.
///
/// The port transcribes the SCALAR semantics, so it agrees with `_c` at any
/// magnitude but can only agree with the kernel a real encode RUNS inside this
/// range. Fuzzing wider would assert a difference between two C paths and tell
/// us nothing about the port — `c_scalar_and_dispatched_agree` is what proves
/// the bound is the right one rather than a convenient one.
const COEFF_BOUND: i32 = i16::MAX as i32;

struct Case {
    coeff: Vec<i32>,
    scan: &'static [u16],
    log_scale: i32,
    qindex: u8,
    tx: &'static str,
    scan_class: usize,
}

fn cases(seed: u64, iters_per_cell: usize, bound: i32) -> Vec<Case> {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    for &(c_tx, tx) in TX_SIZES.iter() {
        let log_scale = TX_SCALE_TAB[c_tx];
        // scan_class: 0 = default (2D), 1/2 = the H/V classes that
        // H_DCT/V_DCT/H_ADST/... select.
        for scan_class in 0..3usize {
            let scan = scan_tables::scan(c_tx, scan_class);
            for &qindex in QINDEXES.iter() {
                for _ in 0..iters_per_cell {
                    let coeff: Vec<i32> = (0..scan.len()).map(|_| rng.coeff(bound)).collect();
                    out.push(Case {
                        coeff,
                        scan,
                        log_scale,
                        qindex,
                        tx,
                        scan_class,
                    });
                }
            }
        }
    }
    out
}

fn check(kind: &str, dispatch: bool, seed: u64, iters: usize, bound: i32) {
    let mut checked = 0usize;
    for c in cases(seed, iters, bound) {
        let t = quant::build_quant_table(c.qindex);
        let row = row_from_table(&t);
        let n = c.coeff.len();

        let (mut rq, mut rdq) = (vec![0i32; n], vec![0i32; n]);
        let (mut cq, mut cdq) = (vec![0i32; n], vec![0i32; n]);

        let rs_eob = match kind {
            "fp" => quant::quantize_fp(&c.coeff, c.scan, &t, c.log_scale, &mut rq, &mut rdq),
            _ => quant::quantize_b(&c.coeff, c.scan, &t, c.log_scale, &mut rq, &mut rdq),
        };
        let c_eob = match kind {
            "fp" => cref::quantize_fp(
                &c.coeff,
                &row,
                c.scan,
                c.log_scale,
                dispatch,
                &mut cq,
                &mut cdq,
            ),
            _ => cref::quantize_b(
                &c.coeff,
                &row,
                c.scan,
                c.log_scale,
                dispatch,
                &mut cq,
                &mut cdq,
            ),
        };

        let ctx = format!(
            "{kind} dispatch={dispatch} {} scan_class={} qindex={} log_scale={} n={n}",
            c.tx, c.scan_class, c.qindex, c.log_scale
        );
        assert_eq!(rs_eob, c_eob, "eob mismatch: {ctx}");
        if let Some(i) = (0..n).find(|&i| rq[i] != cq[i]) {
            panic!(
                "qcoeff[{i}] rust={} c={} (coeff={}): {ctx}",
                rq[i], cq[i], c.coeff[i]
            );
        }
        if let Some(i) = (0..n).find(|&i| rdq[i] != cdq[i]) {
            panic!(
                "dqcoeff[{i}] rust={} c={} (coeff={}): {ctx}",
                rdq[i], cdq[i], c.coeff[i]
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "no cases ran for {kind}");
    eprintln!("{kind} dispatch={dispatch}: {checked} blocks bit-exact");
}

/// The port's `quantize_fp` vs the scalar C reference it transcribes.
#[test]
fn quantize_fp_matches_c_scalar() {
    check("fp", false, 0x5eed_0001, 4, COEFF_BOUND);
}

/// The port's `quantize_fp` vs the RTCD-dispatched kernel a real C encode
/// actually runs (AVX2 on this box).
#[test]
fn quantize_fp_matches_c_dispatched() {
    check("fp", true, 0x5eed_0002, 4, COEFF_BOUND);
}

/// The port's `quantize_b` vs the scalar C reference.
#[test]
fn quantize_b_matches_c_scalar() {
    check("b", false, 0x5eed_0003, 4, COEFF_BOUND);
}

/// The port's `quantize_b` vs the RTCD-dispatched kernel.
#[test]
fn quantize_b_matches_c_dispatched() {
    check("b", true, 0x5eed_0004, 4, COEFF_BOUND);
}

/// The two C paths must agree with each other. If this ever fails, the
/// reference's SIMD and scalar kernels disagree and the port must follow the
/// DISPATCHED one (that is what the encoder calls) — this test exists so that
/// distinction is never silently conflated with a port bug.
#[test]
fn c_scalar_and_dispatched_agree() {
    for (kind, seed) in [("fp", 0x5eed_0005u64), ("b", 0x5eed_0006)] {
        for c in cases(seed, 4, COEFF_BOUND) {
            let t = quant::build_quant_table(c.qindex);
            let row = row_from_table(&t);
            let n = c.coeff.len();
            let (mut sq, mut sdq) = (vec![0i32; n], vec![0i32; n]);
            let (mut dq_, mut ddq) = (vec![0i32; n], vec![0i32; n]);
            let f = if kind == "fp" {
                cref::quantize_fp
            } else {
                cref::quantize_b
            };
            let s_eob = f(
                &c.coeff,
                &row,
                c.scan,
                c.log_scale,
                false,
                &mut sq,
                &mut sdq,
            );
            let d_eob = f(&c.coeff, &row, c.scan, c.log_scale, true, &mut dq_, &mut ddq);
            let ctx = format!(
                "{kind} {} scan_class={} qindex={} log_scale={}",
                c.tx, c.scan_class, c.qindex, c.log_scale
            );
            assert_eq!(s_eob, d_eob, "C scalar vs dispatched eob: {ctx}");
            assert_eq!(sq, dq_, "C scalar vs dispatched qcoeff: {ctx}");
            assert_eq!(sdq, ddq, "C scalar vs dispatched dqcoeff: {ctx}");
        }
    }
}

//! Differential parity for MD's transform entry `svt_aom_estimate_transform`
//! (transforms.c:3938) — EVIDENCE TIER 1 (`docs/WORKING-ON-THIS.md` §4).
//!
//! The C entry is exported but takes a `PictureControlSet*` and a
//! `ModeDecisionContext*`. It reads nothing from either except through
//! `svt_av1_is_lossless_segment` (mode_decision.c:71), so the shim builds
//! exactly that much state per call and drives the real function — which in
//! turn drives the four `static` shape dispatchers
//! (`av1_estimate_transform_{default,N2,N4,ONLY_DC}`) that would otherwise
//! only be reachable at tier 4.
//!
//! Reachability, so the coverage claim is honest rather than nominal
//! (MEASURED from the C sig-deriv, not inferred here — see
//! `docs/INTER-ENCODE-PLAN.md` and enc_mode_config.c): `ctx->pf_ctrls.pf_shape`
//! is hard-set to DEFAULT_SHAPE at all three `set_pf_controls` sites
//! (enc_mode_config.c:7893/8010/8126), and N4_SHAPE reaches MD only through
//! the local `tx_shortcut` overrides in full_loop.c and product_coding_loop.c.
//! So DEFAULT is the everyday path, N4 arms on non-base inter frames from
//! preset 3, and N2 / ONLY_DC are currently dead in MD — ported and gated
//! anyway, per `WORKING-ON-THIS.md` §7 ("dead-looking C stays translated").

use svtav1_cref::txfm_pf as cref;
use svtav1_dsp::fwd_txfm_pf as port;
use svtav1_dsp::fwd_txfm_pf::TxCoeffShape;
use svtav1_types::transform::{TxSize, TxType};

const ALL_TX_SIZES: [TxSize; 19] = [
    TxSize::Tx4x4,
    TxSize::Tx8x8,
    TxSize::Tx16x16,
    TxSize::Tx32x32,
    TxSize::Tx64x64,
    TxSize::Tx4x8,
    TxSize::Tx8x4,
    TxSize::Tx8x16,
    TxSize::Tx16x8,
    TxSize::Tx16x32,
    TxSize::Tx32x16,
    TxSize::Tx32x64,
    TxSize::Tx64x32,
    TxSize::Tx4x16,
    TxSize::Tx16x4,
    TxSize::Tx8x32,
    TxSize::Tx32x8,
    TxSize::Tx16x64,
    TxSize::Tx64x16,
];

const ALL_TX_TYPES: [TxType; 16] = [
    TxType::DctDct,
    TxType::AdstDct,
    TxType::DctAdst,
    TxType::AdstAdst,
    TxType::FlipAdstDct,
    TxType::DctFlipAdst,
    TxType::FlipAdstFlipAdst,
    TxType::AdstFlipAdst,
    TxType::FlipAdstAdst,
    TxType::Idtx,
    TxType::VDct,
    TxType::HDct,
    TxType::VAdst,
    TxType::HAdst,
    TxType::VFlipAdst,
    TxType::HFlipAdst,
];

struct Lcg(u64);
impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn residual(&mut self, bd: u32) -> i16 {
        let m = (1i32 << bd) - 1;
        ((self.next_u32() as i32 % (2 * m + 1)) - m) as i16
    }
    fn coeff(&mut self) -> i32 {
        (self.next_u32() as i32 % 2_000_001) - 1_000_000
    }
}

/// See `c_parity_txfm_pf_entry.rs`: at bd > 8 C's NEON highbd kernels index a
/// table keyed on the whole 2-D tx_type and NULL for every ADST-containing
/// one, so a 32 or 64 dimension is restricted to the types AV1's ext-tx sets
/// actually allow. `svt_aom_estimate_transform` routes through the same RTCD
/// pointers.
fn types_for(tx_size: TxSize) -> &'static [TxType] {
    const DCT_ONLY: [TxType; 1] = [TxType::DctDct];
    const DCT_IDTX: [TxType; 2] = [TxType::DctDct, TxType::Idtx];
    let (w, h) = port::tx_size_dims(tx_size);
    match w.max(h) {
        64 => &DCT_ONLY,
        32 => &DCT_IDTX,
        _ => &ALL_TX_TYPES,
    }
}

fn run_shape(shape: TxCoeffShape, shape_id: i32) {
    let mut checked = 0usize;
    let mut energy_cells = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for &tx_type in types_for(tx_size) {
            for bd in [8u32, 10] {
                for seed in 0..3u64 {
                    let mut rng = Lcg(0xE571
                        ^ seed
                        ^ ((tx_size as u64) << 8)
                        ^ ((tx_type as u64) << 16)
                        ^ ((bd as u64) << 24)
                        ^ ((shape_id as u64) << 32));
                    let stride = w + 9;
                    let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(bd)).collect();
                    let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
                    // A sentinel energy: C writes it only for the five
                    // 64-dimension sizes, so the port must leave it alone
                    // everywhere else.
                    let sentinel = 0xDEAD_BEEF_1234_5678u64;

                    let mut got = prefill.clone();
                    let mut got_energy = sentinel;
                    assert!(
                        port::estimate_transform(
                            &input,
                            stride,
                            &mut got,
                            tx_size,
                            tx_type,
                            shape,
                            &mut got_energy,
                            false,
                        ),
                        "port refused {tx_type:?}/{tx_size:?} shape{shape_id}"
                    );

                    let mut want = prefill.clone();
                    let mut want_energy = sentinel;
                    let rc = cref::estimate_transform(
                        &input,
                        stride,
                        &mut want,
                        w,
                        tx_size as usize,
                        &mut want_energy,
                        bd,
                        tx_type as usize,
                        0,
                        shape_id,
                        false,
                    );
                    assert_eq!(rc, 0, "C returned EbErrorType {rc}");
                    assert_eq!(
                        got, want,
                        "{tx_type:?}/{tx_size:?} bd{bd} seed{seed} shape{shape_id}: coeffs"
                    );
                    assert_eq!(
                        got_energy, want_energy,
                        "{tx_type:?}/{tx_size:?} bd{bd} seed{seed} shape{shape_id}: energy"
                    );
                    if want_energy != sentinel {
                        energy_cells += 1;
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 19 * 2, "only {checked} cells compared");
    assert!(
        energy_cells > 0,
        "three_quad_energy was never written — the 64-dimension handle_transform \
         path did not run"
    );
}

#[test]
fn estimate_transform_default_parity() {
    run_shape(TxCoeffShape::Default, 0);
}

#[test]
fn estimate_transform_n2_parity() {
    run_shape(TxCoeffShape::N2, 1);
}

#[test]
fn estimate_transform_n4_parity() {
    run_shape(TxCoeffShape::N4, 2);
}

#[test]
fn estimate_transform_only_dc_parity() {
    run_shape(TxCoeffShape::OnlyDc, 3);
}

/// The lossless arm: C guards on TX_4X4 ONLY (transforms.c:3949), runs
/// `svt_av1_fwht4x4` and transposes the kernel output into the caller's
/// buffer. Every larger size falls through to the ordinary dispatcher even
/// when the segment is lossless — the fix for upstream gitlab#2373, and a
/// detail a "if lossless, do the WHT" port would get wrong.
#[test]
fn estimate_transform_lossless_parity() {
    let mut wht_cells = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for seed in 0..4u64 {
            let mut rng = Lcg(0x105_5 ^ seed ^ ((tx_size as u64) << 8));
            let stride = w + 2;
            let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(8)).collect();
            let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
            let sentinel = 0x1122_3344_5566_7788u64;

            let mut got = prefill.clone();
            let mut got_energy = sentinel;
            assert!(port::estimate_transform(
                &input,
                stride,
                &mut got,
                tx_size,
                TxType::DctDct,
                TxCoeffShape::Default,
                &mut got_energy,
                true,
            ));

            let mut want = prefill.clone();
            let mut want_energy = sentinel;
            let rc = cref::estimate_transform(
                &input,
                stride,
                &mut want,
                w,
                tx_size as usize,
                &mut want_energy,
                8,
                TxType::DctDct as usize,
                0,
                0,
                true,
            );
            assert_eq!(rc, 0);
            assert_eq!(got, want, "lossless {tx_size:?} seed{seed}: coeffs");
            assert_eq!(got_energy, want_energy, "lossless {tx_size:?}: energy");
            if tx_size == TxSize::Tx4x4 {
                wht_cells += 1;
            }
        }
    }
    assert!(wht_cells > 0, "the lossless TX_4X4 WHT arm never ran");
}

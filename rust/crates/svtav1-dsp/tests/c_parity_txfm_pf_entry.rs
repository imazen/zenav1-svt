//! Differential parity for the transform ENTRY POINTS — EVIDENCE TIER 1
//! (`docs/WORKING-ON-THIS.md` §4). Everything here drives an exported symbol
//! of `libSvtAv1Enc.a`:
//!
//!   * `svt_av1_highbd_fwd_txfm` / `_n2` / `_n4` (transforms.c:4476/:4409/
//!     :4342) — including their TX_4X4 no-op hole and the TX_4X16 quirk;
//!   * `svt_av1_wht_fwd_txfm` (:4527), TPL's only transform entry;
//!   * the DEFAULT-shape 2-D entries, which the shared `div == 1` core also
//!     serves (`svt_av1_transform_two_d_*_c` / `svt_av1_fwd_txfm2d_*_c` —
//!     the `_c` implementations, not the RTCD pointers);
//!   * `svt_handle_transform{16x64,32x64,64x16,64x32,64x64}_c` and their
//!     `_N2_N4_c` twins (:3105-:3291).
//!
//! Note the two oracles in play. The DEFAULT 2-D test calls the `_c`
//! implementations directly. The highbd / wht tests go through C's RTCD
//! dispatch, i.e. whatever kernel the host actually selects (NEON here), so
//! they additionally assert that C's SIMD kernel agrees with this port.

use svtav1_cref::txfm_pf as cref;
use svtav1_dsp::fwd_txfm_pf as port;
use svtav1_dsp::fwd_txfm_pf::{HandleTransform, TxCoeffShape};
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

/// The tx_types the ENTRY POINTS may legally be called with, by AV1's
/// ext-tx set rule: a 64 dimension is DCT_DCT only (and the C wrappers
/// `assert` exactly that), a 32 dimension is `EXT_TX_SET_DCT_IDTX`, and
/// everything smaller takes all 16.
///
/// MEASURED 2026-08-31, and the reason this restriction is here rather than
/// a full 16-type sweep: `svt_av1_highbd_fwd_txfm*` routes through C's RTCD
/// table, and on this host that is the NEON kernel. At `bd > 8`
/// `svt_av1_fwd_txfm2d_16x32_neon` indexes `col_highbd_txfm32_xn_arr[tx_type]`
/// (ASM_NEON/highbd_fwd_txfm_neon.c:1851), which is NULL for every
/// ADST-containing tx_type — including DCT_ADST, whose 32-point dimension is
/// a DCT, because the table is keyed on the whole 2-D type. Calling it there
/// SEGFAULTS the C library. That is out of AV1's envelope, not a port gap:
/// no ext-tx set pairs an ADST with a 32 dimension. The full 16 x 19 sweep
/// still happens against the `_c` implementations in
/// `c_parity_txfm_pf_2d.rs`, which have no such hole.
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

/// The DEFAULT-shape 2-D entries via the shared `div == 1` core.
#[test]
fn fwd_txfm2d_default_parity_all() {
    let mut checked = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for &tx_type in &ALL_TX_TYPES {
            for bd in [8u32, 10] {
                for seed in 0..3u64 {
                    let mut rng = Lcg(seed
                        ^ ((tx_size as u64) << 8)
                        ^ ((tx_type as u64) << 16)
                        ^ ((bd as u64) << 24));
                    let stride = w + 5;
                    let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(bd)).collect();
                    let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
                    let mut got = prefill.clone();
                    if !port::fwd_txfm2d_pf(
                        &input,
                        &mut got,
                        stride,
                        tx_size,
                        tx_type,
                        TxCoeffShape::Default,
                    ) {
                        continue; // the ADST32 hole, already gated elsewhere
                    }
                    let mut want = prefill.clone();
                    cref::fwd_txfm2d_default(
                        tx_size as usize,
                        &input,
                        &mut want,
                        stride,
                        tx_type as usize,
                        bd as u8,
                    );
                    assert_eq!(got, want, "{tx_type:?}/{tx_size:?} bd{bd} seed{seed}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 19 * 6, "only {checked} cells compared");
}

fn highbd_shape(shape: TxCoeffShape, variant: i32) {
    let mut checked = 0usize;
    let mut tx4x4_cells = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for &tx_type in types_for(tx_size) {
            for bd in [8u32, 10] {
                for seed in 0..3u64 {
                    let mut rng = Lcg(0xA11CE
                        ^ seed
                        ^ ((tx_size as u64) << 8)
                        ^ ((tx_type as u64) << 16)
                        ^ ((bd as u64) << 24)
                        ^ ((variant as u64) << 32));
                    let stride = w + 3;
                    let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(bd)).collect();
                    let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
                    let mut got = prefill.clone();
                    if !port::highbd_fwd_txfm(&input, &mut got, stride, tx_type, tx_size, shape) {
                        continue; // ADST32 hole
                    }
                    let mut want = prefill.clone();
                    cref::highbd_fwd_txfm(
                        variant,
                        &input,
                        &mut want,
                        stride,
                        tx_type as usize,
                        tx_size as usize,
                        bd as i32,
                    );
                    assert_eq!(
                        got, want,
                        "variant{variant} {tx_type:?}/{tx_size:?} bd{bd} seed{seed}"
                    );
                    if tx_size == TxSize::Tx4x4 {
                        // The `//hack` hole: both sides must have left the
                        // pre-filled buffer completely alone.
                        assert_eq!(got, prefill, "TX_4X4 hole was not a no-op");
                        tx4x4_cells += 1;
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 19 * 2, "only {checked} cells compared");
    assert!(tx4x4_cells > 0, "the TX_4X4 hole was never exercised");
}

#[test]
fn highbd_fwd_txfm_default_parity() {
    highbd_shape(TxCoeffShape::Default, 0);
}

#[test]
fn highbd_fwd_txfm_n2_parity() {
    highbd_shape(TxCoeffShape::N2, 1);
}

#[test]
fn highbd_fwd_txfm_n4_parity() {
    highbd_shape(TxCoeffShape::N4, 2);
}

/// `svt_av1_wht_fwd_txfm`, over the tx sizes TPL actually asks for plus the
/// full set, and all four `TxCoeffShape` values — including `ONLY_DC_SHAPE`,
/// which falls through C's `default:` arm to the unpruned dispatcher.
#[test]
fn wht_fwd_txfm_parity() {
    let shapes = [
        (TxCoeffShape::Default, 0i32),
        (TxCoeffShape::N2, 1),
        (TxCoeffShape::N4, 2),
        (TxCoeffShape::OnlyDc, 3),
    ];
    let mut checked = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for (shape, shape_id) in shapes {
            for bd in [8u32, 10] {
                for seed in 0..2u64 {
                    let mut rng = Lcg(0x5A17
                        ^ seed
                        ^ ((tx_size as u64) << 8)
                        ^ ((shape_id as u64) << 16)
                        ^ ((bd as u64) << 24));
                    let stride = w + 1;
                    let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(bd)).collect();
                    let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
                    let mut got = prefill.clone();
                    assert!(
                        port::wht_fwd_txfm(
                            &input,
                            stride,
                            &mut got,
                            tx_size,
                            shape,
                            bd as i32,
                            bd > 8
                        ),
                        "wht refused {tx_size:?} shape{shape_id} — it is DCT_DCT only, \
                         so there is no ADST32 hole to hit"
                    );
                    let mut want = prefill.clone();
                    cref::wht_fwd_txfm(
                        &input,
                        stride,
                        &mut want,
                        tx_size as usize,
                        shape_id,
                        bd as i32,
                        bd > 8,
                    );
                    assert_eq!(got, want, "{tx_size:?} shape{shape_id} bd{bd} seed{seed}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 19 * 4 * 2 * 2, "the wht grid did not run in full");
}

/// `svt_handle_transform*`, full and `_N2_N4_c`, on pre-filled coefficient
/// blocks — both the returned energy and the (possibly repacked) buffer.
#[test]
fn handle_transform_parity() {
    let cases = [
        (HandleTransform::T16x64, 0i32, 16 * 64),
        (HandleTransform::T32x64, 1, 32 * 64),
        (HandleTransform::T64x16, 2, 64 * 16),
        (HandleTransform::T64x32, 3, 64 * 32),
        (HandleTransform::T64x64, 4, 64 * 64),
    ];
    let mut repack_seen = 0usize;
    for (which, which_id, len) in cases {
        for pf in [false, true] {
            for seed in 0..4u64 {
                let mut rng = Lcg(0xDEED ^ seed ^ ((which_id as u64) << 8) ^ ((pf as u64) << 16));
                let prefill: Vec<i32> = (0..len).map(|_| rng.coeff()).collect();
                let mut got = prefill.clone();
                let got_energy = port::handle_transform(which, pf, &mut got);
                let mut want = prefill.clone();
                let want_energy = cref::handle_transform(which_id, pf, &mut want);
                assert_eq!(
                    got_energy, want_energy,
                    "{which:?} pf={pf} seed={seed}: energy"
                );
                assert_eq!(got, want, "{which:?} pf={pf} seed={seed}: buffer");
                // The finding this test exists to pin: the _N2_N4_c variants
                // of the three 64-WIDE entries still repack rows.
                if pf && got != prefill {
                    repack_seen += 1;
                }
            }
        }
    }
    assert!(
        repack_seen > 0,
        "no _N2_N4_c variant modified its buffer — the repack claim is untested"
    );
}

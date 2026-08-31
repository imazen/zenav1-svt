//! Differential parity for the reduced-shape 2-D transforms and the transform
//! configuration surface — EVIDENCE TIER 1 (`docs/WORKING-ON-THIS.md` §4):
//! every reference call enters an exported symbol of `libSvtAv1Enc.a`.
//!
//! Covered here:
//!   * `svt_aom_transform_config` (all 16 tx_types x 19 tx_sizes),
//!   * `svt_av1_gen_fwd_stage_range` (same grid, bd 8 and 10),
//!   * `svt_aom_transform_two_d_*_N2_c` / `_N4_c` and
//!     `svt_av1_fwd_txfm2d_*_N2_c` / `_N4_c` (all 19 sizes x both shapes),
//!   * `svt_aom_fwd_txfm_type_to_func` / `svt_aom_inv_txfm_type_to_func`,
//!     by CALLING what the C table returns (a function pointer cannot be
//!     compared across the FFI boundary, its behaviour can).

use svtav1_cref::txfm_pf as cref;
use svtav1_dsp::fwd_txfm_pf as port;
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
    /// Residual value for bit depth `bd`: exactly the domain the encoder
    /// feeds the forward transform (`src - pred` on `bd`-bit samples).
    fn residual(&mut self, bd: u32) -> i16 {
        let m = (1i32 << bd) - 1;
        ((self.next_u32() as i32 % (2 * m + 1)) - m) as i16
    }
    fn coeff(&mut self) -> i32 {
        (self.next_u32() as i32 % 2_000_001) - 1_000_000
    }
}

/// `svt_aom_transform_config`: the whole 16 x 19 grid, every field.
#[test]
fn transform_config_parity_all() {
    for &tx_size in &ALL_TX_SIZES {
        for &tx_type in &ALL_TX_TYPES {
            let want = cref::transform_config(tx_type as usize, tx_size as usize);
            let got = port::transform_config(tx_type, tx_size);
            let label = format!("{tx_type:?}/{tx_size:?}");
            assert_eq!(want[0] != 0, got.ud_flip, "{label}: ud_flip");
            assert_eq!(want[1] != 0, got.lr_flip, "{label}: lr_flip");
            assert_eq!(
                [want[2] as i8, want[3] as i8, want[4] as i8],
                got.shift,
                "{label}: shift"
            );
            assert_eq!(want[5] as i8, got.cos_bit_col, "{label}: cos_bit_col");
            assert_eq!(want[6] as i8, got.cos_bit_row, "{label}: cos_bit_row");
            assert_eq!(want[7], got.txfm_type_col as i32, "{label}: txfm_type_col");
            assert_eq!(want[8], got.txfm_type_row as i32, "{label}: txfm_type_row");
            // stage_num_col/row: C reads av1_txfm_stage_num_list out of
            // bounds for TXFM_TYPE_INVALID (the 64-point ADST hole), so the
            // comparison only applies where the type is valid.
            if got.txfm_type_col != port::TxfmType::Invalid {
                assert_eq!(want[9], got.stage_num_col, "{label}: stage_num_col");
            }
            if got.txfm_type_row != port::TxfmType::Invalid {
                assert_eq!(want[10], got.stage_num_row, "{label}: stage_num_row");
            }
            for i in 0..port::MAX_TXFM_STAGE_NUM {
                assert_eq!(
                    want[11 + i],
                    got.stage_range_col[i] as i32,
                    "{label}: stage_range_col[{i}]"
                );
                assert_eq!(
                    want[11 + port::MAX_TXFM_STAGE_NUM + i],
                    got.stage_range_row[i] as i32,
                    "{label}: stage_range_row[{i}]"
                );
            }
        }
    }
}

/// `svt_av1_get_inv_txfm_cfg`: the whole 16 x 19 grid, every field.
///
/// Note the shape difference from the forward config, which is real: the
/// inverse shift table has TWO entries per size (inv_transforms.c:37), the
/// cos-bit tables are a constant `INV_COS_BIT` wherever the size pair is
/// legal, and no `set_fwd_txfm_non_scale_range` runs — the only stage range
/// written is `iadst4_range` over an ADST4 column or row.
#[test]
fn get_inv_txfm_cfg_parity_all() {
    for &tx_size in &ALL_TX_SIZES {
        for &tx_type in &ALL_TX_TYPES {
            let want = cref::get_inv_txfm_cfg(tx_type as usize, tx_size as usize);
            let got = port::get_inv_txfm_cfg(tx_type, tx_size);
            let label = format!("inv {tx_type:?}/{tx_size:?}");
            assert_eq!(want[0] != 0, got.ud_flip, "{label}: ud_flip");
            assert_eq!(want[1] != 0, got.lr_flip, "{label}: lr_flip");
            // Only two shifts exist on the inverse side.
            assert_eq!(
                [want[2] as i8, want[3] as i8],
                [got.shift[0], got.shift[1]],
                "{label}: shift"
            );
            assert_eq!(want[5] as i8, got.cos_bit_col, "{label}: cos_bit_col");
            assert_eq!(want[6] as i8, got.cos_bit_row, "{label}: cos_bit_row");
            assert_eq!(want[7], got.txfm_type_col as i32, "{label}: txfm_type_col");
            assert_eq!(want[8], got.txfm_type_row as i32, "{label}: txfm_type_row");
            if got.txfm_type_col != port::TxfmType::Invalid {
                assert_eq!(want[9], got.stage_num_col, "{label}: stage_num_col");
            }
            if got.txfm_type_row != port::TxfmType::Invalid {
                assert_eq!(want[10], got.stage_num_row, "{label}: stage_num_row");
            }
            for i in 0..port::MAX_TXFM_STAGE_NUM {
                assert_eq!(
                    want[11 + i],
                    got.stage_range_col[i] as i32,
                    "{label}: stage_range_col[{i}]"
                );
                assert_eq!(
                    want[11 + port::MAX_TXFM_STAGE_NUM + i],
                    got.stage_range_row[i] as i32,
                    "{label}: stage_range_row[{i}]"
                );
            }
        }
    }
}

/// `svt_av1_gen_fwd_stage_range` over the same grid, at both shipping depths.
#[test]
fn gen_fwd_stage_range_parity_all() {
    for &tx_size in &ALL_TX_SIZES {
        for &tx_type in &ALL_TX_TYPES {
            let cfg = port::transform_config(tx_type, tx_size);
            // Skip the ADST-64 hole: C's stage_num there is an out-of-bounds
            // read, so its loop bound is not defined behaviour to reproduce.
            if cfg.txfm_type_col == port::TxfmType::Invalid
                || cfg.txfm_type_row == port::TxfmType::Invalid
            {
                continue;
            }
            for bd in [8, 10] {
                let (wc, wr) = cref::gen_fwd_stage_range(tx_type as usize, tx_size as usize, bd);
                let (gc, gr) = port::gen_fwd_stage_range(&cfg, bd);
                assert_eq!(wc, gc, "{tx_type:?}/{tx_size:?} bd{bd}: stage_range_col");
                assert_eq!(wr, gr, "{tx_type:?}/{tx_size:?} bd{bd}: stage_range_row");
            }
        }
    }
}

/// The 2-D entries: all 19 sizes x {N2, N4} x every tx_type the C dispatch
/// can serve, on real residual input at both bit depths.
///
/// The output buffer is PRE-FILLED identically on both sides and compared in
/// full, so an entry C leaves alone must be left alone by the port too.
fn fwd_2d_shape(shape: port::TxCoeffShape, shape_id: i32) {
    let mut checked = 0usize;
    let mut refused = 0usize;
    for &tx_size in &ALL_TX_SIZES {
        let (w, h) = port::tx_size_dims(tx_size);
        for &tx_type in &ALL_TX_TYPES {
            for bd in [8u32, 10] {
                for seed in 0..3u64 {
                    let mut rng = Lcg(seed
                        ^ ((tx_size as u64) << 8)
                        ^ ((tx_type as u64) << 16)
                        ^ ((bd as u64) << 24)
                        ^ ((shape_id as u64) << 32));
                    let stride = w + 7;
                    let input: Vec<i16> = (0..stride * h).map(|_| rng.residual(bd)).collect();
                    let prefill: Vec<i32> = (0..w * h).map(|_| rng.coeff()).collect();
                    let mut got = prefill.clone();
                    let supported =
                        port::fwd_txfm2d_pf(&input, &mut got, stride, tx_size, tx_type, shape);
                    let cfg = port::transform_config(tx_type, tx_size);
                    let c_has_kernel = cfg.txfm_type_col != port::TxfmType::Invalid
                        && cfg.txfm_type_row != port::TxfmType::Invalid;
                    if !supported {
                        // The port refuses exactly where a 32-point ADST is
                        // needed. C's `fwd_txfm_type_to_func_N2/_N4` map that
                        // to the UNPRUNED `av1_fadst32_new`; the type is
                        // unreachable in a conformant stream (no ext-tx set
                        // pairs ADST with a 32 dimension) and
                        // `fwd_txfm::get_fwd_txfm_func` has the same hole.
                        assert!(
                            cfg.txfm_type_col == port::TxfmType::Adst32
                                || cfg.txfm_type_row == port::TxfmType::Adst32
                                || !c_has_kernel,
                            "{tx_type:?}/{tx_size:?}: refused for an unexpected reason \
                             (col={:?} row={:?})",
                            cfg.txfm_type_col,
                            cfg.txfm_type_row
                        );
                        refused += 1;
                        continue;
                    }
                    let mut want = prefill.clone();
                    cref::fwd_txfm2d_pf(
                        tx_size as usize,
                        shape_id,
                        &input,
                        &mut want,
                        stride,
                        tx_type as usize,
                        bd as u8,
                    );
                    assert_eq!(
                        got, want,
                        "{tx_type:?}/{tx_size:?} bd{bd} seed{seed} shape{shape_id}"
                    );
                    checked += 1;
                }
            }
        }
    }
    // Anti-vacuity: the sweep must actually have driven the C entries.
    assert!(
        checked >= 19 * 6,
        "only {checked} cells compared (refused {refused}) — the grid did not run"
    );
}

#[test]
fn fwd_txfm2d_n2_parity_all() {
    fwd_2d_shape(port::TxCoeffShape::N2, 1);
}

#[test]
fn fwd_txfm2d_n4_parity_all() {
    fwd_2d_shape(port::TxCoeffShape::N4, 2);
}

/// `svt_aom_fwd_txfm_type_to_func`: gate the port's `get_fwd_txfm_func`
/// against what C's table actually dispatches to, including the deliberate
/// ADST32 hole (C returns `av1_fadst32_new`; the port returns `None`).
#[test]
fn fwd_txfm_type_to_func_parity() {
    // (C TxfmType id, 1-D length, port tx_type_1d: 0=DCT 1=ADST 3=IDTX)
    let cases: [(i32, usize, u8); 14] = [
        (0, 4, 0),
        (1, 8, 0),
        (2, 16, 0),
        (3, 32, 0),
        (4, 64, 0),
        (5, 4, 1),
        (6, 8, 1),
        (7, 16, 1),
        (8, 32, 1),
        (9, 4, 3),
        (10, 8, 3),
        (11, 16, 3),
        (12, 32, 3),
        (13, 64, 3),
    ];
    let mut adst32_hole_seen = false;
    for (c_id, n, tx_1d) in cases {
        let mut rng = Lcg(0x5EED ^ c_id as u64);
        let input: Vec<i32> = (0..n)
            .map(|_| (rng.next_u32() as i32 % 4096) - 2048)
            .collect();
        let mut want = vec![0i32; n];
        let called = cref::call_fwd_txfm_type_to_func(c_id, &input, &mut want, 12);
        assert!(called, "C returned NULL for TxfmType {c_id}");
        match svtav1_dsp::fwd_txfm::get_fwd_txfm_func(tx_1d, n) {
            Some(f) => {
                let mut got = vec![0i32; n];
                f(&input, &mut got, 12);
                assert_eq!(got, want, "TxfmType {c_id} ({tx_1d}, {n})");
            }
            None => {
                // The only hole the port has, and it is on purpose:
                // TXFM_TYPE_ADST32 -> av1_fadst32_new. No AV1 ext-tx set
                // pairs an ADST with a 32-point dimension, so C's entry is
                // dead code. MEASURED here rather than assumed: C does
                // return a callable kernel for it.
                assert_eq!((c_id, tx_1d, n), (8, 1, 32), "unexpected port hole");
                adst32_hole_seen = true;
            }
        }
    }
    assert!(adst32_hole_seen, "the ADST32 hole was not exercised");
}

/// `svt_aom_inv_txfm_type_to_func`, same shape as above against
/// `inv_txfm::get_inv_txfm_func`.
///
/// The two sides carry `stage_range` differently and the difference is real,
/// not cosmetic: C's kernels take `(cos_bit, const int8_t* stage_range)` and
/// clamp with `stage_range[stage]` per stage, while the port's inverse
/// kernels take ONE `range` for the whole kernel and hard-code
/// `cos_bit = COS_BIT`. So the reference is driven with `cos_bit = 12` and a
/// stage_range array filled uniformly with the same `range` the port gets —
/// the only configuration in which the two are comparable at all. (Found by
/// this test: passing `12` as the port's third argument compares a cos_bit
/// against a stage range and silently mis-clamps.)
#[test]
fn inv_txfm_type_to_func_parity() {
    let cases: [(i32, usize, u8); 14] = [
        (0, 4, 0),
        (1, 8, 0),
        (2, 16, 0),
        (3, 32, 0),
        (4, 64, 0),
        (5, 4, 1),
        (6, 8, 1),
        (7, 16, 1),
        (8, 32, 1),
        (9, 4, 3),
        (10, 8, 3),
        (11, 16, 3),
        (12, 32, 3),
        (13, 64, 3),
    ];
    let mut adst32_hole_seen = false;
    for (c_id, n, tx_1d) in cases {
        for range in [12i8, 18, 22] {
            let mut rng = Lcg(0xBEEF ^ c_id as u64 ^ ((range as u64) << 40));
            let input: Vec<i32> = (0..n)
                .map(|_| (rng.next_u32() as i32 % 4096) - 2048)
                .collect();
            let mut want = vec![0i32; n];
            let called = cref::call_inv_txfm_type_to_func(c_id, &input, &mut want, 12, range);
            assert!(called, "C returned NULL for TxfmType {c_id}");
            match svtav1_dsp::inv_txfm::get_inv_txfm_func(tx_1d, n) {
                Some(f) => {
                    let mut got = vec![0i32; n];
                    f(&input, &mut got, range);
                    assert_eq!(
                        got, want,
                        "inv TxfmType {c_id} ({tx_1d}, {n}) range {range}"
                    );
                }
                None => {
                    assert_eq!((c_id, tx_1d, n), (8, 1, 32), "unexpected inv port hole");
                    adst32_hole_seen = true;
                }
            }
        }
    }
    assert!(
        adst32_hole_seen,
        "the inverse ADST32 hole was not exercised"
    );
}

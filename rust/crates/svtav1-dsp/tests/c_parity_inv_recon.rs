//! Differential parity for the inverse-transform RECONSTRUCTION entries —
//! **evidence tier 1** against the real exported C symbols
//! `svt_aom_inv_transform_recon8bit` (inv_transforms.c:3138),
//! `svt_aom_inv_transform_recon` (:3237) and the pinned scalar
//! `svt_av1_inv_txfm_add_c` (:3266).
//!
//! What this covers that the existing transform tests do not: the
//! COMPOSITION. `c_parity_txfm.rs` pins the 2-D transform and adds the
//! pixels itself; this drives C's own entry, so the `lossless` routing to the
//! Walsh-Hadamard inverse, the `eob` rule, the u8 -> u16 -> u8 staging inside
//! `svt_av1_inv_txfm_add_c`, and the packed 64-dimension coefficient layout
//! are all C's decisions rather than the test's.
//!
//! ## The shim stages every buffer — measured on a second ISA, not argued
//!
//! A first version handed C the Rust `Vec` pointers at stride `w`. It passed
//! on macOS aarch64 and SIGSEGV'd on x86-64 Linux inside
//! `svt_dav1d_inv_txfm2d_add_8x8_avx2`, and ONLY through the high-bit-depth
//! entry: the 8-bit one stages the caller's pixels into its own
//! `DECLARE_ALIGNED(32, uint16_t, tmp[MAX_TX_SQUARE])` before reaching a
//! kernel (`svt_av1_inv_txfm_add_c`, inv_transforms.c:3269), so C's SIMD
//! never sees a caller buffer there, while `svt_aom_inv_transform_recon`
//! passes the caller's pointers straight down. `shims/inv_recon_shims.c` now
//! stages coefficients, prediction and reconstruction into 64-byte-aligned
//! scratch at `MAX_TX_SIZE` stride — the shape the encoder actually hands
//! these entries (`full_loop.c:1915` passes a picture buffer and a picture
//! stride, not a packed `w * h` block). Side benefit: `pred_stride` and
//! `recon_stride` no longer equal `w`, so a stride bug in the port cannot
//! hide.
//!
//! ## The cell set is bounded by what the PRODUCER can produce, and that
//! bound is not cosmetic — it is what stops this test SEGFAULTING.
//!
//! A first version swept all 16 tx types at all 19 sizes. It crashed with
//! SIGSEGV at the fourth size, on `16x32 ADST_DCT`. Cause: unlike the
//! `svt_av1_*_c` symbols `c_parity_txfm.rs` drives — which are total over all
//! 16 types — `svt_aom_inv_transform_recon8bit` goes through the RTCD pointer
//! `svt_av1_inv_txfm_add`, which resolves to `svt_dav1d_inv_txfm_add_neon`
//! here (common_dsp_rtcd.c:1099) and to `_ssse3`/`_avx2` on x86-64
//! (:540/:542). Those kernels are tables indexed by (tx_size, tx_type) and
//! they carry entries only for the pairs an AV1 bitstream can signal; an
//! illegal pair reads a null slot and jumps through it. C is not wrong — it
//! is simply defined only on legal input, and the encoder never asks for
//! anything else.
//!
//! So the sweep is gated by `av1_ext_tx_used[get_ext_tx_set_type(...)]`
//! (common_utils.h:59, common_utils.c:197), unioned over `is_inter` at
//! `use_reduced_set = 0` — exactly the set a conformant encoder can hand this
//! entry. 155 of the 304 (size, type) pairs.
//!
//! Two C behaviours the port refuses rather than reproducing (see
//! `InvReconError`) are inside that legal set and still not driven: none, as
//! it happens — `32x32` legal types are exactly DCT_DCT/IDTX and 64-dim legal
//! types are exactly DCT_DCT, which is what C's own asserts require. The
//! typed errors therefore guard a region the bitstream cannot reach, which is
//! the right place for them.

use svtav1_cref as cref;
use svtav1_dsp::txfm_dispatch as port;
use svtav1_types::transform::{TxSize, TxType};

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
    fn residual(&mut self) -> i16 {
        (self.next() % 511) as i16 - 255
    }
    fn pixel(&mut self) -> u8 {
        (self.next() % 256) as u8
    }
}

/// `(w, h, TxSize, C TxSize id)` for all 19 sizes. The C ids are the
/// `TX_4X4..TX_64X16` enum order (definitions.h).
const SIZES: [(usize, usize, TxSize, usize); 19] = [
    (4, 4, TxSize::Tx4x4, 0),
    (8, 8, TxSize::Tx8x8, 1),
    (16, 16, TxSize::Tx16x16, 2),
    (32, 32, TxSize::Tx32x32, 3),
    (64, 64, TxSize::Tx64x64, 4),
    (4, 8, TxSize::Tx4x8, 5),
    (8, 4, TxSize::Tx8x4, 6),
    (8, 16, TxSize::Tx8x16, 7),
    (16, 8, TxSize::Tx16x8, 8),
    (16, 32, TxSize::Tx16x32, 9),
    (32, 16, TxSize::Tx32x16, 10),
    (32, 64, TxSize::Tx32x64, 11),
    (64, 32, TxSize::Tx64x32, 12),
    (4, 16, TxSize::Tx4x16, 13),
    (16, 4, TxSize::Tx16x4, 14),
    (8, 32, TxSize::Tx8x32, 15),
    (32, 8, TxSize::Tx32x8, 16),
    (16, 64, TxSize::Tx16x64, 17),
    (64, 16, TxSize::Tx64x16, 18),
];

const TYPES: [(TxType, usize); 16] = [
    (TxType::DctDct, 0),
    (TxType::AdstDct, 1),
    (TxType::DctAdst, 2),
    (TxType::AdstAdst, 3),
    (TxType::FlipAdstDct, 4),
    (TxType::DctFlipAdst, 5),
    (TxType::FlipAdstFlipAdst, 6),
    (TxType::AdstFlipAdst, 7),
    (TxType::FlipAdstAdst, 8),
    (TxType::Idtx, 9),
    (TxType::VDct, 10),
    (TxType::HDct, 11),
    (TxType::VAdst, 12),
    (TxType::HAdst, 13),
    (TxType::VFlipAdst, 14),
    (TxType::HFlipAdst, 15),
];

/// C `av1_ext_tx_used` (common_utils.c:197), rows in `TxSetType` order:
/// DCTONLY, DCT_IDTX, DTT4_IDTX, DTT4_IDTX_1DDCT, DTT9_IDTX_1DDCT, ALL16.
const EXT_TX_USED: [[u8; 16]; 6] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
];

/// C `get_ext_tx_set_type` (common_utils.h:59) at `use_reduced_set = 0`.
/// `txsize_sqr_up_map` is the square of the LARGER dimension and
/// `txsize_sqr_map` the square of the smaller one, so both are derivable from
/// (w, h) without the tables.
fn ext_tx_set_type(w: usize, h: usize, is_inter: bool) -> usize {
    let sqr_up = w.max(h);
    let sqr = w.min(h);
    if sqr_up > 32 {
        return 0; // DCTONLY
    }
    if sqr_up == 32 {
        return if is_inter { 1 } else { 0 }; // DCT_IDTX : DCTONLY
    }
    if is_inter {
        if sqr == 16 { 4 } else { 5 } // DTT9_IDTX_1DDCT : ALL16
    } else if sqr == 16 {
        2 // DTT4_IDTX
    } else {
        3 // DTT4_IDTX_1DDCT
    }
}

/// Can a conformant AV1 bitstream signal this (size, type) pair at all?
/// Unioned over `is_inter`, since the recon entry serves both.
fn ext_tx_legal(w: usize, h: usize, txt: usize) -> bool {
    EXT_TX_USED[ext_tx_set_type(w, h, false)][txt] == 1
        || EXT_TX_USED[ext_tx_set_type(w, h, true)][txt] == 1
}

/// Coefficients for a cell: C's own forward transform of a real residual,
/// truncated to the packed length the inverse entry expects. `pred` is a
/// realistic 8-bit prediction, so `pred + residual` stays in gamut the way
/// the encoder's does.
fn cell_inputs(rng: &mut Rng, w: usize, h: usize, txt: usize) -> (Vec<i32>, Vec<u8>) {
    let res16: Vec<i16> = (0..w * h).map(|_| rng.residual()).collect();
    let full = if w == h {
        cref::fwd_txfm2d(w, &res16, txt)
    } else {
        cref::fwd_txfm2d_rect(w, h, &res16, txt)
    };
    // The forward output is dense w x h; the inverse entries read the packed
    // min(w,32) x min(h,32) block, exactly as `svt_handle_transform*` leaves
    // it. Repack rather than truncate.
    let (cw, ch) = (w.min(32), h.min(32));
    let mut coeff = vec![0i32; cw * ch];
    for r in 0..ch {
        coeff[r * cw..(r + 1) * cw].copy_from_slice(&full[r * w..r * w + cw]);
    }
    let pred: Vec<u8> = (0..w * h).map(|_| rng.pixel()).collect();
    (coeff, pred)
}

#[test]
fn inv_transform_recon8bit_matches_c() {
    let mut rng = Rng(0x1EC0_2026_0831_0001);
    let mut cells = 0usize;
    let mut refused = 0usize;
    for &(w, h, ts, c_ts) in &SIZES {
        for &(t, txt) in &TYPES {
            if !ext_tx_legal(w, h, txt) {
                refused += 1;
                continue;
            }
            cells += 1;
            for _ in 0..6 {
                let (coeff, pred) = cell_inputs(&mut rng, w, h, txt);
                let eob = port::max_eob(ts);
                let want = cref::inv_recon::inv_transform_recon8bit(
                    &coeff, &pred, w, w, h, c_ts, txt, eob, false,
                );

                let mut got = vec![0u8; w * h];
                port::inv_transform_recon8bit(
                    &coeff,
                    w.min(32),
                    &pred,
                    w,
                    &mut got,
                    w,
                    ts,
                    t,
                    false,
                )
                .expect("port must reconstruct this pair");

                assert_eq!(got.len(), want.len(), "{w}x{h} {t:?}: length");
                if got != want {
                    let first = got
                        .iter()
                        .zip(want.iter())
                        .position(|(a, b)| a != b)
                        .unwrap();
                    panic!(
                        "recon8bit {w}x{h} {t:?}: first diff at {} (r{} c{}): ours={} c={}",
                        first,
                        first / w,
                        first % w,
                        got[first],
                        want[first]
                    );
                }
            }
        }
    }
    assert_eq!(
        (cells, refused),
        (155, 149),
        "legal / illegal (size, type) counts changed"
    );
}

/// The RTCD-dispatched entry and the pinned scalar `svt_av1_inv_txfm_add_c`
/// must agree with each other, so a future divergence can be attributed.
/// (`svt_av1_inv_txfm_add` resolves to `svt_dav1d_inv_txfm_add_neon` on
/// aarch64 and `_ssse3`/`_avx2` on x86-64 — different implementation
/// families, not widened copies of `_c`.)
#[test]
fn c_dispatched_and_scalar_recon_agree() {
    let mut rng = Rng(0x1EC0_2026_0831_0002);
    for &(w, h, ts, c_ts) in &SIZES {
        for &(t, txt) in &TYPES {
            if !ext_tx_legal(w, h, txt) {
                continue;
            }
            for _ in 0..4 {
                let (coeff, pred) = cell_inputs(&mut rng, w, h, txt);
                let eob = port::max_eob(ts);
                let dispatched = cref::inv_recon::inv_transform_recon8bit(
                    &coeff, &pred, w, w, h, c_ts, txt, eob, false,
                );
                let scalar =
                    cref::inv_recon::inv_txfm_add_c(&coeff, &pred, w, w, h, c_ts, txt, eob, false);
                assert_eq!(dispatched, scalar, "C SIMD vs C scalar at {w}x{h} {t:?}");
            }
        }
    }
}

/// bd10 through `svt_aom_inv_transform_recon`'s u16 pixel path.
///
/// **bd10 only, and bd12's absence is a measurement, not an oversight.**
/// C v4.2.0 ships 8- and 10-bit only — `svt_av1_verify_settings`
/// (`Globals/enc_settings.c:460`) rejects every other depth — so bd12 is
/// outside the envelope on BOTH sides. It is also not a single oracle there:
/// this entry reaches its kernels through the `svt_av1_inv_txfm2d_add_*`
/// RTCD pointers, and at bd12 the x86-64 arm
/// (`svt_dav1d_inv_txfm2d_add_4x4_avx2`) clips the reconstruction to 10 bits
/// while the aarch64 arm does not — measured 2026-08-31, `4x4 DCT_DCT`,
/// x86-64 C returned 1023 where aarch64 C and the port both return 1582.
/// `inv_txfm2d_add_hbd_scalar_matches_port` below covers bd12 against the
/// `_c` kernels, which have no such per-ISA arm.
#[test]
fn inv_transform_recon_hbd_matches_c() {
    let mut rng = Rng(0x1EC0_2026_0831_0003);
    for bd in [10u32] {
        let maxv = (1u32 << bd) - 1;
        for &(w, h, ts, c_ts) in &SIZES {
            for &(t, txt) in &TYPES {
                if !ext_tx_legal(w, h, txt) {
                    continue;
                }
                for _ in 0..3 {
                    let (coeff, pred8) = cell_inputs(&mut rng, w, h, txt);
                    let pred: Vec<u16> = pred8
                        .iter()
                        .map(|&p| ((u32::from(p) * maxv) / 255) as u16)
                        .collect();
                    let eob = port::max_eob(ts);
                    let want = cref::inv_recon::inv_transform_recon(
                        &coeff, &pred, w, w, h, c_ts, bd, txt, eob, false,
                    );
                    let mut got = vec![0u16; w * h];
                    port::inv_transform_recon(
                        &coeff,
                        w.min(32),
                        &pred,
                        w,
                        &mut got,
                        w,
                        ts,
                        t,
                        false,
                        bd as u8,
                    )
                    .expect("port must reconstruct this pair");
                    if got != want {
                        let first = got
                            .iter()
                            .zip(want.iter())
                            .position(|(a, b)| a != b)
                            .unwrap();
                        panic!(
                            "recon bd{bd} {w}x{h} {t:?}: first diff at {} (r{} c{}): ours={} c={}",
                            first,
                            first / w,
                            first % w,
                            got[first],
                            want[first]
                        );
                    }
                }
            }
        }
    }
}

/// The lossless 4x4 Walsh-Hadamard arm, both eob branches.
///
/// `eob` reaches `highbd_iwht4x4_add` (inv_transforms.c:2874) ONLY here, and
/// only when C's read and write pointers are the same buffer — otherwise
/// `svt_aom_inv_transform_recon8bit` overwrites it with
/// `av1_get_max_eob(TX_4X4) = 16` (:3151-3155). No shipping C call site
/// combines aliasing with `lossless != 0`, so `svt_av1_highbd_iwht4x4_1_add_c`
/// is unreachable in the encoder and reachable here.
#[test]
fn lossless_wht_4x4_both_eob_branches_match_c() {
    let mut rng = Rng(0x1EC0_2026_0831_0004);
    let mut dc_only_branch_taken = 0usize;
    for trial in 0..64 {
        // A DC-only coefficient block for half the trials so the eob <= 1
        // branch gets a coefficient set it can actually be right about.
        let coeff: Vec<i32> = if trial % 2 == 0 {
            let mut c = vec![0i32; 16];
            c[0] = (rng.next() % 4096) as i32 - 2048;
            c
        } else {
            (0..16).map(|_| (rng.next() % 4096) as i32 - 2048).collect()
        };
        let pred: Vec<u8> = (0..16).map(|_| rng.pixel()).collect();

        // Distinct buffers: C forces eob = 16, so the caller's value is dead.
        for eob in [0u32, 1, 5, 16] {
            let want =
                cref::inv_recon::inv_transform_recon8bit(&coeff, &pred, 4, 4, 4, 0, 0, eob, true);
            let mut got = vec![0u8; 16];
            port::inv_transform_recon8bit(
                &coeff,
                4,
                &pred,
                4,
                &mut got,
                4,
                TxSize::Tx4x4,
                TxType::DctDct,
                true,
            )
            .unwrap();
            assert_eq!(got, want, "lossless distinct-buffer, caller eob {eob}");
        }

        // Aliased buffers: the caller's eob survives, so eob <= 1 selects the
        // DC-only kernel.
        for eob in [0u32, 1, 2, 16] {
            let want = cref::inv_recon::inv_transform_recon8bit_in_place(
                &coeff, &pred, 4, 4, 0, 0, eob, true,
            );
            let mut got = pred.clone();
            port::inv_transform_recon8bit_in_place(
                &coeff,
                4,
                &mut got,
                4,
                TxSize::Tx4x4,
                TxType::DctDct,
                eob,
                true,
            )
            .unwrap();
            assert_eq!(got, want, "lossless aliased, eob {eob}");
            if eob <= 1 {
                dc_only_branch_taken += 1;
            }
        }
    }
    assert_eq!(
        dc_only_branch_taken, 128,
        "the eob <= 1 DC-only Walsh-Hadamard branch was never exercised"
    );
}

/// The port against C's PINNED SCALAR high-bit-depth kernels
/// (`svt_av1_inv_txfm2d_add_{size}_c`), at bd10 AND bd12.
///
/// Two things this adds over `inv_transform_recon_hbd_matches_c`, which
/// drives the RTCD-dispatched entry:
///   * it is the same oracle on every ISA, so it can carry bd12 — where the
///     dispatched x86-64 kernels clip to 10 bits (see above);
///   * a divergence here is attributable to the port, whereas a divergence
///     there could be either the port or C's own per-ISA SIMD.
#[test]
fn inv_txfm2d_add_hbd_scalar_matches_port() {
    let mut rng = Rng(0x1EC0_2026_0831_0005);
    let mut cells = 0usize;
    for bd in [10u32, 12] {
        let maxv = (1u32 << bd) - 1;
        for &(w, h, ts, c_ts) in &SIZES {
            for &(t, txt) in &TYPES {
                if !ext_tx_legal(w, h, txt) {
                    continue;
                }
                cells += 1;
                for _ in 0..3 {
                    let (coeff, pred8) = cell_inputs(&mut rng, w, h, txt);
                    let pred: Vec<u16> = pred8
                        .iter()
                        .map(|&p| ((u32::from(p) * maxv) / 255) as u16)
                        .collect();
                    let want = cref::inv_recon::inv_txfm2d_add_c_bd(
                        &coeff, &pred, w, w, h, c_ts, txt, bd as i32,
                    );
                    let mut got = vec![0u16; w * h];
                    port::highbd_inv_txfm_add(
                        &coeff,
                        w.min(32),
                        &pred,
                        w,
                        &mut got,
                        w,
                        ts,
                        t,
                        port::max_eob(ts),
                        false,
                        bd as u8,
                    )
                    .expect("port must reconstruct this pair");
                    if got != want {
                        let first = got
                            .iter()
                            .zip(want.iter())
                            .position(|(a, b)| a != b)
                            .unwrap();
                        panic!(
                            "scalar bd{bd} {w}x{h} {t:?}: first diff at {} (r{} c{}): ours={} c={}",
                            first,
                            first / w,
                            first % w,
                            got[first],
                            want[first]
                        );
                    }
                }
            }
        }
    }
    assert_eq!(
        cells, 310,
        "scalar hbd cell count (155 legal pairs x 2 depths)"
    );
}

/// C-vs-C control at bd10: the RTCD-dispatched entry must agree with the
/// pinned scalar kernels. Pins the ISA-dependence this file learned about, so
/// a future divergence is attributable rather than ambiguous.
#[test]
fn c_hbd_dispatched_and_scalar_agree_at_bd10() {
    let mut rng = Rng(0x1EC0_2026_0831_0006);
    let bd = 10u32;
    let maxv = (1u32 << bd) - 1;
    for &(w, h, ts, c_ts) in &SIZES {
        for &(t, txt) in &TYPES {
            if !ext_tx_legal(w, h, txt) {
                continue;
            }
            for _ in 0..2 {
                let (coeff, pred8) = cell_inputs(&mut rng, w, h, txt);
                let pred: Vec<u16> = pred8
                    .iter()
                    .map(|&p| ((u32::from(p) * maxv) / 255) as u16)
                    .collect();
                let dispatched = cref::inv_recon::inv_transform_recon(
                    &coeff,
                    &pred,
                    w,
                    w,
                    h,
                    c_ts,
                    bd,
                    txt,
                    port::max_eob(ts),
                    false,
                );
                let scalar = cref::inv_recon::inv_txfm2d_add_c_bd(
                    &coeff, &pred, w, w, h, c_ts, txt, bd as i32,
                );
                assert_eq!(
                    dispatched, scalar,
                    "C SIMD vs C scalar bd10 at {w}x{h} {t:?}"
                );
            }
        }
    }
}

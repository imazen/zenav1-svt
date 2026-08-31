//! Differential parity for `svtav1_encoder::port_md_rate_estimation` against
//! the REAL exported C symbols — **evidence tier 1**
//! (`rust/docs/WORKING-ON-THIS.md` §4).
//!
//! Three oracles, all `T` in `nm -g Bin/Release/libSvtAv1Enc.a`:
//! * `svt_aom_get_me_qindex` (md_rate_estimation.c:1084),
//! * `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053),
//! * `svt_aom_estimate_syntax_rate` (md_rate_estimation.c:74), driven for its
//!   `wedge_idx_fac_bits` rows.
//!
//! `get_interinter_wedge_bits` itself is C `static` (it prints nothing under
//! `nm -g`), but it is a pure function of the exported
//! `svt_aom_get_wedge_params_bits`, and the table it GATES is built by the
//! exported `svt_aom_estimate_syntax_rate` — so both its input and its
//! observable effect are driven against real C rather than re-transcribed.

use svtav1_cref::md_subpel as cref;
use svtav1_encoder::port_md_rate_estimation::{
    BLOCK_SIZES_ALL, get_interinter_wedge_bits, get_me_qindex, get_wedge_params_bits,
    wedge_idx_fac_bits,
};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// The port's `WEDGE_PARAMS_BITS` table is a transcription of a C `static`
/// array; the exported accessor is the oracle for every entry.
#[test]
fn wedge_params_bits_match_c_for_every_block_size() {
    let mut nonzero = 0usize;
    for b in 0..BLOCK_SIZES_ALL {
        let c = cref::get_wedge_params_bits(b);
        assert_eq!(get_wedge_params_bits(b), c, "wedge_params_bits[{b}]");
        if c != 0 {
            nonzero += 1;
        }
    }
    // ANTI-VACUITY: an all-zero table would satisfy the loop above trivially.
    assert_eq!(
        nonzero, 9,
        "expected the nine wedge-capable block sizes to report non-zero bits"
    );
}

/// `get_interinter_wedge_bits` = `wbits > 0 ? wbits + 1 : 0`, rebuilt from the
/// exported accessor.
#[test]
fn interinter_wedge_bits_match_c_derived_values() {
    for b in 0..BLOCK_SIZES_ALL {
        let wbits = cref::get_wedge_params_bits(b);
        let expect = if wbits > 0 { wbits + 1 } else { 0 };
        assert_eq!(get_interinter_wedge_bits(b), expect, "bsize {b}");
    }
}

/// The observable effect: which `wedge_idx_fac_bits` rows
/// `svt_aom_estimate_syntax_rate` fills, and with what values.
///
/// A rejected row keeps the context's zero initialiser, so this test is what
/// distinguishes a correct gate from one that is inverted, off by a size, or
/// always-true.
#[test]
fn wedge_idx_fac_bits_match_c() {
    let mut rng = Rng(0x51ab_cdef_1234_9876);
    for round in 0..8 {
        let mut cdf = [[0u16; 17]; BLOCK_SIZES_ALL];
        for (b, row) in cdf.iter_mut().enumerate() {
            // A valid inverse CDF: strictly decreasing to 0 at index 15, with
            // the symbol-count slot at 16 (C reads cdf[16] only as the counter).
            let mut prev = 32768u32;
            for (j, slot) in row.iter_mut().enumerate().take(16) {
                if j == 15 {
                    *slot = 0;
                } else {
                    let step = 1 + (rng.next() % 2000) as u32;
                    prev = prev.saturating_sub(step).max((15 - j) as u32 * 4 + 4);
                    *slot = prev as u16;
                }
            }
            row[16] = (round + b) as u16;
        }
        let c = cref::wedge_idx_fac_bits(&cdf);
        let r = wedge_idx_fac_bits(&cdf);
        for b in 0..BLOCK_SIZES_ALL {
            assert_eq!(r[b], c[b], "wedge_idx_fac_bits[{b}] round {round}");
        }
        // ANTI-VACUITY: the nine gated rows must actually be filled and the
        // other thirteen must actually be zero, or "they match" means nothing.
        let filled = (0..BLOCK_SIZES_ALL)
            .filter(|&b| c[b].iter().any(|&v| v != 0))
            .count();
        assert_eq!(
            filled, 9,
            "C filled {filled} rows, not the nine wedge-capable sizes"
        );
    }
}

/// `svt_aom_get_me_qindex` across both SB sizes and every edge geometry.
#[test]
fn get_me_qindex_matches_c() {
    let mut rng = Rng(0x0102_0304_0506_0708);
    // Frame geometries chosen so the b64 grid is 1x1, 1xN, Nx1 and NxN, and so
    // that non-multiple-of-64 dimensions exercise the ceil-divide.
    let geoms: [(u16, u16); 8] = [
        (64, 64),
        (64, 256),
        (256, 64),
        (256, 256),
        (128, 128),
        (192, 320),
        (65, 65),
        (320, 192),
    ];
    let mut cells_checked = 0usize;
    let mut distinct_divisors = [false; 5];
    for (aw, ah) in geoms {
        let w_b64 = u32::from(aw).div_ceil(64);
        let h_b64 = u32::from(ah).div_ceil(64);
        let q: Vec<u8> = (0..(w_b64 * h_b64) as usize)
            .map(|_| (rng.next() % 256) as u8)
            .collect();
        for y in 0..h_b64 {
            for x in 0..w_b64 {
                let idx = x + y * w_b64;
                // SB64: index lookup.
                let c = cref::get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, false);
                let r = get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, false);
                assert_eq!(r, c, "sb64 {aw}x{ah} idx {idx}");
                cells_checked += 1;

                // SB128: the SB covers the 2x2 b64 quad whose top-left is here.
                // C derives b64_index from org_x/org_y, so a 128-aligned origin
                // is what a real SB128 would pass; both aligned and unaligned
                // origins are driven to prove the /64 is what selects the cell.
                let c = cref::get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, true);
                let r = get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, true);
                assert_eq!(r, c, "sb128 {aw}x{ah} at b64 ({x},{y})");
                cells_checked += 1;

                let right = (x + 1) < w_b64;
                let below = (y + 1) < h_b64;
                let divisor = match (right, below) {
                    (false, false) => 1usize,
                    (true, false) | (false, true) => 2,
                    (true, true) => 4,
                };
                distinct_divisors[divisor] = true;
            }
        }
    }
    // ANTI-VACUITY: all three reachable divisors must have been exercised, and
    // 3 must never be one of them (see the port's note).
    assert!(cells_checked >= 100, "only {cells_checked} cells");
    assert!(distinct_divisors[1], "divisor 1 never reached");
    assert!(distinct_divisors[2], "divisor 2 never reached");
    assert!(distinct_divisors[4], "divisor 4 never reached");
    assert!(
        !distinct_divisors[3],
        "divisor 3 is supposed to be unreachable"
    );
}

/// Saturating values: a b64 map of all 255s and all 0s, at every geometry, so
/// the `uint16_t` accumulator and the integer division are driven at their
/// extremes.
#[test]
fn get_me_qindex_extremes_match_c() {
    for fill in [0u8, 1, 254, 255] {
        for (aw, ah) in [(256u16, 256u16), (192, 128), (64, 64)] {
            let w_b64 = u32::from(aw).div_ceil(64);
            let h_b64 = u32::from(ah).div_ceil(64);
            let q = vec![fill; (w_b64 * h_b64) as usize];
            for y in 0..h_b64 {
                for x in 0..w_b64 {
                    let idx = x + y * w_b64;
                    assert_eq!(
                        get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, true),
                        cref::get_me_qindex(&q, aw, ah, idx, x * 64, y * 64, true),
                        "fill {fill} {aw}x{ah} ({x},{y})"
                    );
                }
            }
        }
    }
}

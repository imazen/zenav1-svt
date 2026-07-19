//! CDF (Cumulative Distribution Function) tables and update logic.
//!
//! Spec 07: CDF update (cabac_context_model.h).
//!
//! Ported from `cabac_context_model.h`.

/// CDF probability type (Q15 fixed-point, 15 fractional bits).
pub type AomCdfProb = u16;

/// Number of probability bits.
pub const CDF_PROB_BITS: u32 = 15;
/// Maximum CDF value (2^15 = 32768).
pub const CDF_PROB_TOP: u16 = 1 << CDF_PROB_BITS;
/// Initial CDF top value.
pub const CDF_INIT_TOP: u16 = 32768;

/// Convert a probability to its inverse CDF representation.
/// AOM_ICDF(x) = CDF_PROB_TOP - x
#[inline]
pub const fn aom_icdf(x: u16) -> u16 {
    CDF_PROB_TOP - x
}

/// Size of a CDF array for N symbols (N+1 to include the counter).
#[inline]
pub const fn cdf_size(nsymbs: usize) -> usize {
    nsymbs + 1
}

/// Weighted per-entry average of two CDF arrays, in place into `left`.
///
/// Exact port of `avg_cdf_symbol` (`enc_dec_process.c:2668-2679`): each
/// entry becomes `(left*wt_left + tr*wt_tr + (wt_left+wt_tr)/2) / (wt_left+wt_tr)`
/// with integer arithmetic. C's `avg_cdf_symbols` invokes this over `j` in
/// `0..=nsymbs` per CDF with `stride == CDF_SIZE(nsymbs)`, i.e. it touches
/// *every* element of each averaged array — including the adaptation counter
/// at `[nsymbs]` — so a flat element-wise pass over the whole contiguous
/// array reproduces it. For the few `AVG_CDF_STRIDE` calls where the C stride
/// exceeds `CDF_SIZE(nsymbs)` (e.g. `uv_mode_cdf[0]`, the 4-way
/// `partition_cdf` contexts) the skipped tail slots are unused padding beyond
/// that context's alphabet, so averaging them here is a harmless no-op on any
/// value a rate builder reads.
///
/// The two weights C ever uses are `AVG_CDF_WEIGHT_LEFT=3` / `AVG_CDF_WEIGHT_TOP=1`.
#[inline]
pub fn avg_cdf_entries(left: &mut [AomCdfProb], tr: &[AomCdfProb], wt_left: i32, wt_tr: i32) {
    debug_assert_eq!(left.len(), tr.len());
    let denom = wt_left + wt_tr;
    let bias = denom / 2;
    for (l, r) in left.iter_mut().zip(tr.iter()) {
        // Result of averaging two values each < CDF_PROB_TOP is < CDF_PROB_TOP,
        // so it always fits AomCdfProb (matches C's `(AomCdfProb)` cast + assert).
        *l = ((i32::from(*l) * wt_left + i32::from(*r) * wt_tr + bias) / denom) as u16;
    }
}

/// Update CDF probabilities after encoding/decoding a symbol.
///
/// Exact port of `update_cdf` from `cabac_context_model.h` (C layout:
/// ICDF values at `cdf[0..nsymbs-1]` with a structural 0 at `cdf[nsymbs-1]`,
/// and the adaptation counter at `cdf[nsymbs]`, capped at 32). Verified
/// bit-for-bit against the linked C reference in `tests/c_parity.rs`.
#[inline]
pub fn update_cdf(cdf: &mut [AomCdfProb], val: usize, nsymbs: usize) {
    debug_assert!(nsymbs >= 2 && nsymbs < 17);
    debug_assert!(val < nsymbs);
    debug_assert!(cdf.len() >= nsymbs + 1);

    let count = cdf[nsymbs];
    // rate = 4 + (count > 15) + (count > 31) + (nsymbs > 3), folded into a
    // shift since count never exceeds 32 — see the C source derivation.
    let rate = u32::from(4 + (count >> 4)) + u32::from(nsymbs > 3);

    for i in 0..nsymbs - 1 {
        if i < val {
            cdf[i] += (CDF_PROB_TOP - cdf[i]) >> rate;
        } else {
            cdf[i] -= cdf[i] >> rate;
        }
    }
    cdf[nsymbs] += u16::from(count < 32);
}

/// Initialize a uniform CDF for `nsymbs` symbols.
///
/// Each symbol gets equal probability: CDF[i] = CDF_PROB_TOP * (nsymbs - 1 - i) / (nsymbs - 1)
pub fn init_uniform_cdf(cdf: &mut [AomCdfProb], nsymbs: usize) {
    debug_assert!(cdf.len() > nsymbs);
    for (i, prob) in cdf[..nsymbs - 1].iter_mut().enumerate() {
        *prob = (CDF_PROB_TOP as u32 * (nsymbs - 1 - i) as u32 / (nsymbs - 1) as u32) as u16;
    }
    // Counter starts at 0
    cdf[nsymbs] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_cdf_binary() {
        // Binary CDF for nsymbs=2: needs 3 elements [prob_0, (unused), count]
        // cdf[0] is the probability threshold between symbol 0 and symbol 1
        // In AV1's inverse CDF: higher cdf[0] means symbol 0 is MORE likely
        // When val=0: all i >= val get decreased, so cdf[0] decreases
        // (In AV1 inverse CDF, *decreasing* cdf[0] means symbol 0 gets more probable)
        let mut cdf = [CDF_PROB_TOP / 2, 0u16, 0u16];
        let initial = cdf[0];
        update_cdf(&mut cdf, 0, 2);
        // val=0: loop runs for i=0, since 0 is NOT < 0, cdf[0] gets decreased
        assert!(cdf[0] < initial, "cdf[0] should decrease for val=0");
    }

    #[test]
    fn update_cdf_converges() {
        // A 4-symbol CDF needs nsymbs + 1 = 5 elements
        let top = CDF_PROB_TOP as u32;
        let mut cdf = [
            (top * 3 / 4) as u16, // cdf[0]
            (top * 2 / 4) as u16, // cdf[1]
            (top / 4) as u16,     // cdf[2]
            0u16,                 // unused by update loop iteration
            0u16,                 // cdf[4] = counter
        ];

        // Update 100 times with val=0 — symbol 0 should become dominant
        for _ in 0..100 {
            update_cdf(&mut cdf, 0, 4);
        }

        // After many observations of val=0, all CDF values should decrease
        // (in inverse CDF, lower = more probability mass on earlier symbols)
        // cdf[0] should be quite low
        assert!(
            cdf[0] < (top / 4) as u16,
            "cdf[0] should be small after many val=0 updates"
        );
    }

    #[test]
    fn init_uniform() {
        let mut cdf = [0u16; 5]; // 4 symbols + 1 counter
        init_uniform_cdf(&mut cdf, 4);
        // Uniform: CDF_PROB_TOP * [3/3, 2/3, 1/3] = [32768, 21845, 10922]
        assert_eq!(cdf[0], 32768);
        assert!(cdf[1] > 21000 && cdf[1] < 22000);
        assert!(cdf[2] > 10000 && cdf[2] < 11000);
        assert_eq!(cdf[4], 0);
    }

    #[test]
    fn avg_cdf_entries_matches_c_formula() {
        // C `avg_cdf_symbol` (enc_dec_process.c:2668-2679) with
        // wt_left=3, wt_tr=1: entry = (left*3 + tr*1 + (3+1)/2) / (3+1)
        //                            = (left*3 + tr + 2) / 4  (integer).
        let expect = |l: u32, t: u32| ((l * 3 + t + 2) / 4) as u16;
        let cases = [(100u16, 200u16), (200, 100), (10, 20), (0, 0), (32767, 32767), (32767, 0)];
        for (l, t) in cases {
            let mut left = [l];
            avg_cdf_entries(&mut left, &[t], 3, 1);
            assert_eq!(left[0], expect(u32::from(l), u32::from(t)), "avg({l},{t}) with 3:1");
        }
        // The pass is element-wise across the whole array (including the
        // adaptation counter at the tail), exactly like C's `j <= nsymbs`
        // loop over a CDF_SIZE(nsymbs)-strided array.
        let mut left = [12631u16, 11221, 9690, 4]; // ...counter = 4
        let tr = [14306u16, 11848, 9644, 8]; //     counter = 8
        avg_cdf_entries(&mut left, &tr, 3, 1);
        assert_eq!(left[0], (12631 * 3 + 14306 + 2) / 4);
        assert_eq!(left[1], (11221 * 3 + 11848 + 2) / 4);
        assert_eq!(left[2], (9690 * 3 + 9644 + 2) / 4);
        assert_eq!(left[3], (4 * 3 + 8 + 2) / 4); // counter averaged too -> 5
    }

    #[test]
    fn avg_cdf_equal_contexts_is_identity() {
        // Averaging a context with an identical one must be a no-op — this is
        // why averaging the intra-frame-inert (inter/MV) CDF fields in
        // FrameContext::avg_cdf_with is harmless: both neighbors hold equal
        // defaults there.
        let mut a = [30000u16, 20000, 10000, 0, 31, 1, 4, 12345];
        let b = a;
        avg_cdf_entries(&mut a, &b, 3, 1);
        assert_eq!(a, b);
    }
}

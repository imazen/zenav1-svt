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

/// Update CDF probabilities after encoding/decoding a symbol.
///
/// This is the core backward-adaptive CDF update from the AV1 spec.
/// The CDF array has `nsymbs` probability entries followed by a count
/// at index `nsymbs`.
///
/// Ported from `cabac_context_model.h` lines 469-497.
#[inline]
/// Update CDF after observing symbol `val`.
///
/// Matched exactly to rav1d's update_cdf (msac.rs) for bitstream conformance.
/// CDFs are stored in ICDF format (CDF_PROB_TOP - cumulative_probability).
pub fn update_cdf(cdf: &mut [AomCdfProb], val: usize, nsymbs: usize) {
    debug_assert!(nsymbs < 17);
    debug_assert!(val < nsymbs);

    // Counter is stored at cdf[nsymbs-1], matching rav1d's cdf[n_symbols]
    // where n_symbols = nsymbs - 1 = number of CDF entries.
    let n = nsymbs - 1;
    let count = cdf[n];
    let rate = 4 + (count >> 4) + u16::from(nsymbs > 3);

    // rav1d-compatible update: for ICDF values
    for i in 0..n {
        if i < val {
            // Increase ICDF (decrease cumulative probability below val)
            cdf[i] = cdf[i].wrapping_add((CDF_PROB_TOP.wrapping_sub(cdf[i])) >> rate);
        } else {
            // Decrease ICDF (increase cumulative probability at/above val)
            cdf[i] = cdf[i].wrapping_sub(cdf[i] >> rate);
        }
    }

    // Increment count, capped at 32
    cdf[n] = count + u16::from(count < 32);
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
}

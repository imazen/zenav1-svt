//! The cheap RDOQ substitute from `Source/Lib/Codec/full_loop.c`.
//!
//! ## Coverage — 2 of 2 functions in this group
//!
//! | C function | line | here |
//! |---|---|---|
//! | `update_coeff_eob_fast` | 1006 | [`update_coeff_eob_fast`] |
//! | `svt_fast_optimize_b` | 1028 | [`fast_optimize_b`] |
//!
//! MISSING from this file: nothing in the group. The rest of full_loop.c
//! (`svt_av1_optimize_b` and the LPD1 driver around it) is not ported here.
//!
//! ## Reachability — measured, not assumed
//!
//! C takes this path at `full_loop.c:1818`, inside
//! `svt_aom_quantize_inv_quantize`, when
//! `eob_perc >= ctx->rdoq_ctrls.eob_fast_th`. `eob_fast_th` is **255**
//! (i.e. never — `eob_perc` is a percentage) at rdoq levels 1-3, and 30 / 0 at
//! levels 4 / 5. Levels 4 and 5 are assigned ONLY in the LPD1 arm
//! (`enc_mode_config.c:7451/7453`), i.e. video preset >= M9. So this is
//! required for preset >= 9 VIDEO byte-parity and is structurally unreachable
//! below that. Per `docs/WORKING-ON-THIS.md` §7 it is translated anyway and
//! its reachability written down rather than being called dead.
//!
//! ## Evidence tier — 4, and here is why it is not 1
//!
//! Both functions are C `static` with no exported symbol (`nm -g` on
//! `Bin/Release/libSvtAv1Enc.a` prints nothing for either; the positive
//! control in the same object, `svt_aom_quantize_inv_quantize`, prints `T`).
//! The only exported caller is `svt_aom_quantize_inv_quantize`, which takes a
//! `PictureControlSet*` and a `ModeDecisionContext*` and would have to run a
//! full quantize first and land `eob_perc` above the threshold before the call
//! is even made — a shell far larger than the twelve lines under test, and one
//! whose own construction would need verifying. So the tests are
//! **hand-derived vectors traced against the C source**, the weakest tier
//! (`docs/WORKING-ON-THIS.md` §4), and they say so.

/// C `ROUND_POWER_OF_TWO(value, n)` (definitions.h:478) on `int`.
#[inline]
fn round_power_of_two(value: i32, n: u32) -> i32 {
    (value + ((1i32 << n) >> 1)) >> n
}

/// C `update_coeff_eob_fast` (full_loop.c:1006-1025).
///
/// Walks the scan BACKWARD from `eob - 1` and retracts the trailing run of
/// coefficients whose magnitude falls under a widened zbin, stopping at the
/// first one that survives. Zeroes `qcoeff`/`dqcoeff` for each retracted
/// position and returns the new eob.
///
/// Three details, each of which changes the coded coefficients:
///
/// * **`zbin[rc != 0]` indexes on the BOOLEAN**, not on `rc`. `zbin[0]` is the
///   DC threshold and `zbin[1]` is used for EVERY other scan position — it is
///   not `zbin[rc]`. (This is the same shape as `percents[hier <= 4][idx]`
///   elsewhere in this tree; read the index, do not infer it.)
/// * the zbin is `dequant + ROUND_POWER_OF_TWO(dequant * 70, 7)`, i.e. 70/128
///   wider than the dequant step — computed in `int`, from the `int16_t`
///   `dequant_qtx` pair.
/// * `abs_coeff` is `int64_t` and the comparison shifts it LEFT by
///   `1 + shift`, so it is the coefficient in a finer domain than the zbin;
///   doing that shift in 32 bits would overflow for large coefficients at
///   `shift == 2`.
///
/// `scan` maps scan index -> raster position; `coeff`, `qcoeff` and `dqcoeff`
/// are indexed by raster position, exactly as in C.
pub fn update_coeff_eob_fast(
    eob: &mut u16,
    shift: i32,
    dequant: &[i16; 2],
    scan: &[u16],
    coeff: &[i32],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) {
    let mut eob_out = i32::from(*eob);
    let zbin = [
        i32::from(dequant[0]) + round_power_of_two(i32::from(dequant[0]) * 70, 7),
        i32::from(dequant[1]) + round_power_of_two(i32::from(dequant[1]) * 70, 7),
    ];
    for i in (0..i32::from(*eob)).rev() {
        let rc = scan[i as usize] as usize;
        let q = qcoeff[rc];
        let c = coeff[rc];
        // C: `const int coeff_sign = -(coeff < 0); int64_t abs_coeff =
        // (coeff ^ coeff_sign) - coeff_sign;` — the branchless |c|.
        let coeff_sign = -i32::from(c < 0);
        let abs_coeff = i64::from(c ^ coeff_sign) - i64::from(coeff_sign);
        if (abs_coeff << (1 + shift)) < i64::from(zbin[usize::from(rc != 0)]) || q == 0 {
            eob_out -= 1;
            qcoeff[rc] = 0;
            dqcoeff[rc] = 0;
        } else {
            break;
        }
    }
    *eob = eob_out as u16;
}

/// C `svt_fast_optimize_b` (full_loop.c:1028-1036).
///
/// The cheap RDOQ substitute: pick the scan for `(tx_size, tx_type)`, take the
/// transform's scale shift, and run [`update_coeff_eob_fast`] once. C passes
/// `p->dequant_qtx`, the `int16_t[2]` DC/AC dequant pair.
///
/// `tx_size` is the C `TxSize` value and `tx_type` the C `TxType` value, so
/// the scan and scale lookups are the same table indices C uses.
pub fn fast_optimize_b(
    coeff: &[i32],
    dequant: &[i16; 2],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
    eob: &mut u16,
    tx_size: usize,
    tx_type: usize,
) {
    let scan_class = crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize;
    let scan = crate::entropy::scan_tables::scan(tx_size, scan_class);
    let shift = crate::quant::TX_SCALE_TAB[tx_size];
    update_coeff_eob_fast(eob, shift, dequant, scan, coeff, qcoeff, dqcoeff);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVIDENCE TIER 4 — hand-derived from full_loop.c:1006-1025. Both
    /// functions are C `static` with no exported symbol, so no differential
    /// against the real C is reachable without building a
    /// PictureControlSet + ModeDecisionContext shell (see the module doc).
    ///
    /// The zbin: dequant 100 -> 100 + ROUND_POWER_OF_TWO(7000, 7)
    /// = 100 + ((7000 + 64) >> 7) = 100 + 55 = 155.
    /// dequant 40 -> 40 + ((2800 + 64) >> 7) = 40 + 22 = 62.
    #[test]
    fn zbin_widening_is_70_over_128() {
        assert_eq!(100 + round_power_of_two(100 * 70, 7), 155);
        assert_eq!(40 + round_power_of_two(40 * 70, 7), 62);
        assert_eq!(0 + round_power_of_two(0, 7), 0);
    }

    /// The retraction stops at the FIRST surviving coefficient, walking
    /// backward — it does not sweep the whole block. Scan is the identity so
    /// scan index == raster position and the trace is readable.
    ///
    /// shift = 0, so the test is `|c| << 1 < zbin`.
    /// dequant = (100, 40) -> zbin = (155, 62).
    /// positions 0..5, all qcoeff nonzero:
    ///   rc=4: |c|=10 -> 20 < 62  -> retract, eob 5 -> 4
    ///   rc=3: |c|=40 -> 80 > 62  -> SURVIVES, loop breaks
    /// so eob = 4 and positions 0..3 are untouched even though rc=1's
    /// magnitude is also under the AC zbin.
    #[test]
    fn retraction_stops_at_the_first_survivor() {
        let scan: Vec<u16> = (0..5u16).collect();
        let coeff = [500i32, 5, 300, -40, 10];
        let mut q = [7i32, 1, 5, -2, 1];
        let mut dq = [700i32, 40, 300, -80, 40];
        let mut eob = 5u16;
        update_coeff_eob_fast(&mut eob, 0, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 4);
        assert_eq!(q, [7, 1, 5, -2, 0]);
        assert_eq!(dq, [700, 40, 300, -80, 0]);
    }

    /// `zbin[rc != 0]` indexes on the BOOLEAN. A port that wrote `zbin[rc]`
    /// would read out of bounds for rc >= 2; one that used the DC threshold
    /// everywhere would retract differently. Here DC's threshold (155) is
    /// large enough to retract a coefficient the AC threshold (62) keeps.
    #[test]
    fn zbin_index_is_the_boolean_not_the_position() {
        // Single coefficient at rc = 0 (DC): |c| = 70, 70 << 1 = 140 < 155
        // -> retracted by the DC threshold.
        let scan = [0u16];
        let coeff = [70i32];
        let mut q = [1i32];
        let mut dq = [70i32];
        let mut eob = 1u16;
        update_coeff_eob_fast(&mut eob, 0, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 0, "DC uses zbin[0] = 155");

        // Same magnitude at rc = 1 (AC): 140 > 62 -> kept.
        let scan = [1u16];
        let coeff = [0i32, 70];
        let mut q = [0i32, 1];
        let mut dq = [0i32, 70];
        let mut eob = 1u16;
        update_coeff_eob_fast(&mut eob, 0, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 1, "AC uses zbin[1] = 62");
    }

    /// `qcoeff == 0` retracts regardless of magnitude — the `|| (qcoeff == 0)`
    /// arm.
    #[test]
    fn zero_qcoeff_retracts_whatever_the_magnitude() {
        let scan: Vec<u16> = (0..3u16).collect();
        let coeff = [1000i32, 900, 800];
        let mut q = [5i32, 4, 0];
        let mut dq = [500i32, 400, 300];
        let mut eob = 3u16;
        update_coeff_eob_fast(&mut eob, 0, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 2);
        assert_eq!(dq, [500, 400, 0]);
    }

    /// `abs_coeff << (1 + shift)` is 64-bit in C. At shift = 2 a coefficient
    /// near the 32-bit ceiling would overflow an `i32` shift; the port must
    /// widen first. Traced: 2^30 << 3 = 2^33, which is > any zbin, so the
    /// coefficient must SURVIVE. An i32 shift would wrap to 0 and retract it.
    #[test]
    fn shift_is_64_bit() {
        let scan = [0u16];
        let coeff = [1i32 << 30];
        let mut q = [1i32];
        let mut dq = [1i32];
        let mut eob = 1u16;
        update_coeff_eob_fast(&mut eob, 2, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 1);
    }

    /// Negative coefficients take the branchless-abs path
    /// (`(c ^ -(c<0)) - -(c<0)`), which must equal `|c|`.
    #[test]
    fn branchless_abs_matches_abs() {
        for c in [-2147483647i32, -1000, -1, 0, 1, 1000, 2147483647] {
            let sign = -i32::from(c < 0);
            let abs = i64::from(c ^ sign) - i64::from(sign);
            assert_eq!(abs, i64::from(c).abs(), "c = {c}");
        }
    }

    /// `fast_optimize_b` must pick the scan for `(tx_size, tx_type)` and the
    /// scale shift for `tx_size`. TX_4X4 (C TxSize 0) has scale 0; DCT_DCT
    /// (TxType 0) maps to scan class 0. With every coefficient under the AC
    /// zbin the whole tail retracts to eob 0.
    #[test]
    fn fast_optimize_b_wires_scan_and_shift() {
        assert_eq!(crate::quant::TX_SCALE_TAB[0], 0);
        let n = 16;
        let coeff = alloc::vec![5i32; n];
        let mut q = alloc::vec![1i32; n];
        let mut dq = alloc::vec![5i32; n];
        let mut eob = 16u16;
        fast_optimize_b(&coeff, &[100, 40], &mut q, &mut dq, &mut eob, 0, 0);
        assert_eq!(eob, 0);
        assert!(q.iter().all(|&v| v == 0));
        assert!(dq.iter().all(|&v| v == 0));
    }

    /// An eob of 0 is a no-op: the loop body never runs.
    #[test]
    fn zero_eob_is_a_noop() {
        let scan = [0u16, 1];
        let coeff = [5i32, 5];
        let mut q = [1i32, 1];
        let mut dq = [5i32, 5];
        let mut eob = 0u16;
        update_coeff_eob_fast(&mut eob, 0, &[100, 40], &scan, &coeff, &mut q, &mut dq);
        assert_eq!(eob, 0);
        assert_eq!(q, [1, 1]);
    }
}

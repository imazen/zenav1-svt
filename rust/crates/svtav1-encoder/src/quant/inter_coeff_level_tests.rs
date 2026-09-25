use super::*;

/// The INTER thresholds are `{5833/96, 5833/48, 16666/48}` with C's
/// INTEGER division, not the rationals: 60.76 -> 60, 121.5 -> 121,
/// 347.2 -> 347. Getting that wrong moves a band boundary by one, which
/// is a different RDOQ level for a whole frame.
#[test]
fn the_inter_thresholds_are_cs_truncated_ones() {
    assert_eq!((5833 / 96, 5833 / 48, 16666 / 48), (60, 121, 347));
    // 240p (this campaign's cells) scales them by 1.7 -> {102, 205, 589}.
    let (w, h) = (64usize, 64usize);
    let at = |d: u64| derive_inter_coeff_level(d, 1, w, h);
    assert_eq!(at(101), CoeffLvl::VLow);
    assert_eq!(at(102), CoeffLvl::Low);
    assert_eq!(at(204), CoeffLvl::Low);
    assert_eq!(at(205), CoeffLvl::Normal);
    assert_eq!(at(589), CoeffLvl::Normal);
    assert_eq!(at(590), CoeffLvl::High);
    // The qp divisor is `max(1, qp)`, so qp 0 must not divide by zero —
    // it divides by ONE, which is a different (and much larger) cmplx
    // than a naive `qp` divisor would give at qp 1.
    assert_eq!(at(0), CoeffLvl::VLow);
    assert_eq!(derive_inter_coeff_level(1000, 0, w, h), CoeffLvl::High);
    assert_eq!(derive_inter_coeff_level(1000, 5, w, h), CoeffLvl::Low);
}

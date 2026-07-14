//! Loop-restoration tile syntax — C-exact ports of the wiener coefficient
//! coding chain and the per-RU `wiener_restore` flag.
//!
//! C sources (SVT-AV1 v4.2.0-rc, entropy_coding.c):
//! - `recenter_nonneg` (:2895) / `recenter_finite_nonneg` (:2907)
//! - `svt_aom_write_primitive_quniform` (:2916) + count (:2944)
//! - `svt_aom_write_primitive_subexpfin` (:2954) + count (:3000)
//! - `svt_aom_write_primitive_refsubexpfin` (:3028) + count (:3045)
//! - `write_wiener_filter` (:4074) — taps v0(win7) v1 v2 h0(win7) h1 h2 as
//!   refsubexpfin against the running per-plane reference, which the call
//!   then updates (ref chaining; decoder mirror libaom decodeframe.c:1605
//!   `read_wiener_filter`).
//! - `loop_restoration_write_sb_coeffs` (:4150), `frame_rtype ==
//!   RESTORE_WIENER` arm: `aom_write_symbol(w, unit_rtype != RESTORE_NONE,
//!   frame_context->wiener_restore_cdf, 2)` then the filter when set.
//!
//! Tap coding constants cite restoration.h:129-151; the (n, k) pairs are
//! fixed by the spec (5.11.58 read_lr_unit).

use crate::writer::AomWriter;

/// WIENER_FILT_TAP bounds (restoration.h:129-147).
pub const WIENER_FILT_TAP0_MINV: i32 = 3 - (1 << 4) / 2; // -5
pub const WIENER_FILT_TAP0_MAXV: i32 = 3 - 1 + (1 << 4) / 2; // 10
pub const WIENER_FILT_TAP1_MINV: i32 = -7 - (1 << 5) / 2; // -23
pub const WIENER_FILT_TAP1_MAXV: i32 = -7 - 1 + (1 << 5) / 2; // 8
pub const WIENER_FILT_TAP2_MINV: i32 = 15 - (1 << 6) / 2; // -17
pub const WIENER_FILT_TAP2_MAXV: i32 = 15 - 1 + (1 << 6) / 2; // 46
/// Subexp K per tap (restoration.h:149-151).
pub const WIENER_FILT_TAP0_SUBEXP_K: u16 = 1;
pub const WIENER_FILT_TAP1_SUBEXP_K: u16 = 2;
pub const WIENER_FILT_TAP2_SUBEXP_K: u16 = 3;

/// WIENER_WIN / WIENER_WIN_CHROMA (restoration.h:116/123).
pub const WIENER_WIN: usize = 7;
pub const WIENER_WIN_CHROMA: usize = 5;

/// C `recenter_nonneg` (entropy_coding.c:2895).
#[inline]
fn recenter_nonneg(r: u16, v: u16) -> u16 {
    if v > (r << 1) {
        v
    } else if v >= r {
        (v - r) << 1
    } else {
        ((r - v) << 1) - 1
    }
}

/// C `recenter_finite_nonneg` (entropy_coding.c:2907).
#[inline]
fn recenter_finite_nonneg(n: u16, r: u16, v: u16) -> u16 {
    if (r << 1) <= n {
        recenter_nonneg(r, v)
    } else {
        recenter_nonneg(n - 1 - r, n - 1 - v)
    }
}

/// get_msb (position of highest set bit), C semantics for n >= 1.
#[inline]
fn get_msb(n: u32) -> u32 {
    31 - n.leading_zeros()
}

/// C `svt_aom_write_primitive_quniform` (entropy_coding.c:2916).
fn write_primitive_quniform(w: &mut AomWriter, n: u16, v: u16) {
    if n <= 1 {
        return;
    }
    let l = get_msb(n as u32 - 1) + 1;
    let m = (1u32 << l) as i32 - n as i32;
    if (v as i32) < m {
        w.write_literal(v as u32, l - 1);
    } else {
        w.write_literal((m + ((v as i32 - m) >> 1)) as u32, l - 1);
        w.write_bit((v as i32 - m) & 1 != 0);
    }
}

/// C `svt_aom_count_primitive_quniform` (entropy_coding.c:2944).
fn count_primitive_quniform(n: u16, v: u16) -> i32 {
    if n <= 1 {
        return 0;
    }
    let l = get_msb(n as u32 - 1) as i32 + 1;
    let m = (1i32 << l) - n as i32;
    if (v as i32) < m {
        l - 1
    } else {
        l
    }
}

/// C `svt_aom_write_primitive_subexpfin` (entropy_coding.c:2954).
fn write_primitive_subexpfin(w: &mut AomWriter, n: u16, k: u16, v: u16) {
    let mut i = 0i32;
    let mut mk = 0i32;
    loop {
        let b = if i != 0 { k as i32 + i - 1 } else { k as i32 };
        let a = 1i32 << b;
        if (n as i32) <= mk + 3 * a {
            write_primitive_quniform(w, (n as i32 - mk) as u16, (v as i32 - mk) as u16);
            break;
        } else {
            let t = v as i32 >= mk + a;
            w.write_bit(t);
            if t {
                i += 1;
                mk += a;
            } else {
                w.write_literal((v as i32 - mk) as u32, b as u32);
                break;
            }
        }
    }
}

/// C `svt_aom_count_primitive_subexpfin` (entropy_coding.c:3000).
fn count_primitive_subexpfin(n: u16, k: u16, v: u16) -> i32 {
    let mut count = 0i32;
    let mut i = 0i32;
    let mut mk = 0i32;
    loop {
        let b = if i != 0 { k as i32 + i - 1 } else { k as i32 };
        let a = 1i32 << b;
        if (n as i32) <= mk + 3 * a {
            count += count_primitive_quniform((n as i32 - mk) as u16, (v as i32 - mk) as u16);
            break;
        } else {
            let t = v as i32 >= mk + a;
            count += 1;
            if t {
                i += 1;
                mk += a;
            } else {
                count += b;
                break;
            }
        }
    }
    count
}

/// C `svt_aom_write_primitive_refsubexpfin` (entropy_coding.c:3028).
pub fn write_primitive_refsubexpfin(w: &mut AomWriter, n: u16, k: u16, r: u16, v: u16) {
    write_primitive_subexpfin(w, n, k, recenter_finite_nonneg(n, r, v));
}

/// C `svt_aom_count_primitive_refsubexpfin` (entropy_coding.c:3045).
pub fn count_primitive_refsubexpfin(n: u16, k: u16, r: u16, v: u16) -> i32 {
    count_primitive_subexpfin(n, k, recenter_finite_nonneg(n, r, v))
}

/// C `write_wiener_filter` (entropy_coding.c:4074): code the six signalable
/// taps (four when `wiener_win == WIENER_WIN_CHROMA` — TAP0 skipped) against
/// the running reference, then update the reference to this filter.
pub fn write_wiener_filter(
    w: &mut AomWriter,
    wiener_win: usize,
    vfilter: &[i16; 8],
    hfilter: &[i16; 8],
    ref_vfilter: &mut [i16; 8],
    ref_hfilter: &mut [i16; 8],
) {
    let taps = |f: &[i16; 8], rf: &[i16; 8], idx: usize, minv: i32, maxv: i32, k: u16| {
        (
            (maxv - minv + 1) as u16,
            k,
            (rf[idx] as i32 - minv) as u16,
            (f[idx] as i32 - minv) as u16,
        )
    };
    if wiener_win == WIENER_WIN {
        let (n, k, r, v) = taps(
            vfilter,
            ref_vfilter,
            0,
            WIENER_FILT_TAP0_MINV,
            WIENER_FILT_TAP0_MAXV,
            WIENER_FILT_TAP0_SUBEXP_K,
        );
        write_primitive_refsubexpfin(w, n, k, r, v);
    } else {
        debug_assert!(vfilter[0] == 0 && vfilter[WIENER_WIN - 1] == 0);
    }
    let (n, k, r, v) = taps(
        vfilter,
        ref_vfilter,
        1,
        WIENER_FILT_TAP1_MINV,
        WIENER_FILT_TAP1_MAXV,
        WIENER_FILT_TAP1_SUBEXP_K,
    );
    write_primitive_refsubexpfin(w, n, k, r, v);
    let (n, k, r, v) = taps(
        vfilter,
        ref_vfilter,
        2,
        WIENER_FILT_TAP2_MINV,
        WIENER_FILT_TAP2_MAXV,
        WIENER_FILT_TAP2_SUBEXP_K,
    );
    write_primitive_refsubexpfin(w, n, k, r, v);
    if wiener_win == WIENER_WIN {
        let (n, k, r, v) = taps(
            hfilter,
            ref_hfilter,
            0,
            WIENER_FILT_TAP0_MINV,
            WIENER_FILT_TAP0_MAXV,
            WIENER_FILT_TAP0_SUBEXP_K,
        );
        write_primitive_refsubexpfin(w, n, k, r, v);
    } else {
        debug_assert!(hfilter[0] == 0 && hfilter[WIENER_WIN - 1] == 0);
    }
    let (n, k, r, v) = taps(
        hfilter,
        ref_hfilter,
        1,
        WIENER_FILT_TAP1_MINV,
        WIENER_FILT_TAP1_MAXV,
        WIENER_FILT_TAP1_SUBEXP_K,
    );
    write_primitive_refsubexpfin(w, n, k, r, v);
    let (n, k, r, v) = taps(
        hfilter,
        ref_hfilter,
        2,
        WIENER_FILT_TAP2_MINV,
        WIENER_FILT_TAP2_MAXV,
        WIENER_FILT_TAP2_SUBEXP_K,
    );
    write_primitive_refsubexpfin(w, n, k, r, v);
    // svt_memcpy(ref_wiener_info, wiener_info, ...) — ref chaining.
    *ref_vfilter = *vfilter;
    *ref_hfilter = *hfilter;
}

/// C `count_wiener_bits` (restoration_pick.c:1005) — the search's bit count
/// for a candidate filter against the running reference (NO ref update; the
/// search updates its reference only when WIENER wins the unit RD).
pub fn count_wiener_bits(
    wiener_win: usize,
    vfilter: &[i16; 8],
    hfilter: &[i16; 8],
    ref_vfilter: &[i16; 8],
    ref_hfilter: &[i16; 8],
) -> i32 {
    let mut bits = 0i32;
    let tap = |f: &[i16; 8], rf: &[i16; 8], idx: usize, minv: i32, maxv: i32, k: u16| {
        count_primitive_refsubexpfin(
            (maxv - minv + 1) as u16,
            k,
            (rf[idx] as i32 - minv) as u16,
            (f[idx] as i32 - minv) as u16,
        )
    };
    if wiener_win == WIENER_WIN {
        bits += tap(
            vfilter,
            ref_vfilter,
            0,
            WIENER_FILT_TAP0_MINV,
            WIENER_FILT_TAP0_MAXV,
            WIENER_FILT_TAP0_SUBEXP_K,
        );
    }
    bits += tap(
        vfilter,
        ref_vfilter,
        1,
        WIENER_FILT_TAP1_MINV,
        WIENER_FILT_TAP1_MAXV,
        WIENER_FILT_TAP1_SUBEXP_K,
    );
    bits += tap(
        vfilter,
        ref_vfilter,
        2,
        WIENER_FILT_TAP2_MINV,
        WIENER_FILT_TAP2_MAXV,
        WIENER_FILT_TAP2_SUBEXP_K,
    );
    if wiener_win == WIENER_WIN {
        bits += tap(
            hfilter,
            ref_hfilter,
            0,
            WIENER_FILT_TAP0_MINV,
            WIENER_FILT_TAP0_MAXV,
            WIENER_FILT_TAP0_SUBEXP_K,
        );
    }
    bits += tap(
        hfilter,
        ref_hfilter,
        1,
        WIENER_FILT_TAP1_MINV,
        WIENER_FILT_TAP1_MAXV,
        WIENER_FILT_TAP1_SUBEXP_K,
    );
    bits += tap(
        hfilter,
        ref_hfilter,
        2,
        WIENER_FILT_TAP2_MINV,
        WIENER_FILT_TAP2_MAXV,
        WIENER_FILT_TAP2_SUBEXP_K,
    );
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// recenter_finite_nonneg round-trip sanity: distinct codes per v.
    #[test]
    fn recenter_is_injective() {
        for n in [16u16, 32, 64] {
            for r in 0..n {
                let mut seen = alloc::vec![false; n as usize];
                for v in 0..n {
                    let c = recenter_finite_nonneg(n, r, v);
                    assert!((c as usize) < n as usize);
                    assert!(!seen[c as usize]);
                    seen[c as usize] = true;
                }
            }
        }
    }

    /// The reference at its own value codes 0 -> minimal bits.
    #[test]
    fn ref_equal_value_is_cheapest() {
        for n in [16u16, 32, 64] {
            let k = 3u16;
            for r in 0..n {
                let self_bits = count_primitive_refsubexpfin(n, k, r, r);
                for v in 0..n {
                    assert!(count_primitive_refsubexpfin(n, k, r, v) >= self_bits);
                }
            }
        }
    }
}

//! The C encoder's small integer helpers, defined once.
//!
//! The port had ~40 private copies of these across the DSP and encoder crates
//! (`round_power_of_two` alone ×20, in six textual variants). A copy that
//! drifts is a silent bit-exactness hazard, so every copy now imports from
//! here under its old local name, and call sites are unchanged.
//!
//! `ROUND_POWER_OF_TWO(v, n)` is `((v) + (((1 << (n)) >> 1))) >> (n)` in C. The
//! half is computed as `(1 << n) >> 1` in UNSIGNED arithmetic for every width.
//! That equals the `1 << (n - 1)` spelling some copies used for every `n > 0`,
//! is defined at `n == 0` (where `1 << (n - 1)` panics in a debug build and
//! shifts by 31 in release), and stays positive at the top shift (`n == 31` on
//! `i32`), where the signed `(1i32 << n) >> 1` spelling other copies used
//! sign-extends to a negative half. Arithmetic is plain, not wrapping, as in the copies it
//! replaces; the two `wrapping_add` variants (`inter_mvp`, `port_sgr`) keep
//! their own definitions because their overflow behaviour differs.
//!
//! Two shift-type families, because the C shift counts arrive as `u32` in the
//! encoder and as `int` (`i32`) fields in the DSP convolution parameters:
//! [`shift_u32`] and [`shift_i32`]. The arithmetic is shared.

/// Helpers whose shift count is `u32`.
pub mod shift_u32 {
    /// `ROUND_POWER_OF_TWO` on `i32`.
    #[inline]
    pub const fn round_power_of_two_i32(value: i32, n: u32) -> i32 {
        (value + ((1u32 << n) >> 1) as i32) >> n
    }
    /// `ROUND_POWER_OF_TWO_64` on `i64`.
    #[inline]
    pub const fn round_power_of_two_i64(value: i64, n: u32) -> i64 {
        (value + ((1u64 << n) >> 1) as i64) >> n
    }
    /// `ROUND_POWER_OF_TWO` on `u32`.
    #[inline]
    pub const fn round_power_of_two_u32(value: u32, n: u32) -> u32 {
        (value + ((1u32 << n) >> 1)) >> n
    }
    /// `ROUND_POWER_OF_TWO` on `u64`.
    #[inline]
    pub const fn round_power_of_two_u64(value: u64, n: u32) -> u64 {
        (value + ((1u64 << n) >> 1)) >> n
    }
    /// `ROUND_POWER_OF_TWO_SIGNED`: rounds the magnitude, keeps the sign.
    #[inline]
    pub const fn round_power_of_two_signed_i32(value: i32, n: u32) -> i32 {
        if value < 0 {
            -round_power_of_two_i32(-value, n)
        } else {
            round_power_of_two_i32(value, n)
        }
    }
    /// `ROUND_POWER_OF_TWO_SIGNED_64`.
    #[inline]
    pub const fn round_power_of_two_signed_i64(value: i64, n: u32) -> i64 {
        if value < 0 {
            -round_power_of_two_i64(-value, n)
        } else {
            round_power_of_two_i64(value, n)
        }
    }
    /// `DIVIDE_AND_ROUND(x, y)` = `((x) + ((y) >> 1)) / (y)`.
    #[inline]
    pub const fn divide_and_round_i32(x: i32, y: i32) -> i32 {
        (x + (y >> 1)) / y
    }
    /// `DIVIDE_AND_ROUND` on `i64`.
    #[inline]
    pub const fn divide_and_round_i64(x: i64, y: i64) -> i64 {
        (x + (y >> 1)) / y
    }
    /// `DIVIDE_AND_ROUND` on `u32`.
    #[inline]
    pub const fn divide_and_round_u32(x: u32, y: u32) -> u32 {
        (x + (y >> 1)) / y
    }
    /// `DIVIDE_AND_ROUND` on `u64`.
    #[inline]
    pub const fn divide_and_round_u64(x: u64, y: u64) -> u64 {
        (x + (y >> 1)) / y
    }
    /// `CLIP3(lo, hi, v)`. Unlike `Ord::clamp` it does not panic when
    /// `lo > hi`; it returns `lo` for values below it first, as C does.
    #[inline]
    pub const fn clip3_i32(lo: i32, hi: i32, v: i32) -> i32 {
        if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        }
    }
    /// `CLIP3` on `i64`.
    #[inline]
    pub const fn clip3_i64(lo: i64, hi: i64, v: i64) -> i64 {
        if v < lo {
            lo
        } else if v > hi {
            hi
        } else {
            v
        }
    }
}

/// The same helpers with an `i32` shift count (C `int`), for the DSP code
/// whose convolution parameters carry shifts as `i32`. A negative count is a
/// caller bug and is caught in debug builds.
pub mod shift_i32 {
    use super::shift_u32 as u;
    /// `ROUND_POWER_OF_TWO` on `i32`.
    #[inline]
    pub const fn round_power_of_two_i32(value: i32, n: i32) -> i32 {
        debug_assert!(n >= 0);
        u::round_power_of_two_i32(value, n as u32)
    }
    /// `ROUND_POWER_OF_TWO_64` on `i64`.
    #[inline]
    pub const fn round_power_of_two_i64(value: i64, n: i32) -> i64 {
        debug_assert!(n >= 0);
        u::round_power_of_two_i64(value, n as u32)
    }
    /// `ROUND_POWER_OF_TWO_SIGNED`.
    #[inline]
    pub const fn round_power_of_two_signed_i32(value: i32, n: i32) -> i32 {
        debug_assert!(n >= 0);
        u::round_power_of_two_signed_i32(value, n as u32)
    }
    /// `ROUND_POWER_OF_TWO_SIGNED_64`.
    #[inline]
    pub const fn round_power_of_two_signed_i64(value: i64, n: i32) -> i64 {
        debug_assert!(n >= 0);
        u::round_power_of_two_signed_i64(value, n as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::shift_u32::*;

    #[test]
    fn matches_the_c_macros() {
        assert_eq!(round_power_of_two_i32(10, 2), 3);
        assert_eq!(round_power_of_two_i32(9, 0), 9);
        assert_eq!(round_power_of_two_i32(-10, 2), -2);
        assert_eq!(round_power_of_two_signed_i32(-10, 2), -3);
        assert_eq!(round_power_of_two_i64(1 << 40, 8), 1 << 32);
        assert_eq!(round_power_of_two_u64(7, 1), 4);
        assert_eq!(divide_and_round_i32(7, 2), 4);
        assert_eq!(divide_and_round_u64(10, 4), 3);
        assert_eq!(clip3_i32(0, 10, -5), 0);
        assert_eq!(clip3_i64(0, 10, 50), 10);
        // Every n > 0 agrees with the `1 << (n - 1)` spelling, up to the top
        // shift.
        // A negative half would give -1 here.
        assert_eq!(round_power_of_two_i32(1 << 29, 31), 0);
        for n in 1..31u32 {
            for v in [-1000i32, -1, 0, 1, 7, 12345] {
                assert_eq!(round_power_of_two_i32(v, n), (v + (1 << (n - 1))) >> n);
            }
        }
    }
}

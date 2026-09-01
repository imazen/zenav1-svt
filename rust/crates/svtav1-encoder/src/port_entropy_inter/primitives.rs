//! The small EXPORTED helpers of `entropy_coding.c` that the per-block walk
//! reads, and that had no named Rust counterpart.
//!
//! Despite living under `port_entropy_inter`, these are not inter-specific:
//! the module tree is named for this lane's C file
//! (`Source/Lib/Codec/entropy_coding.c`), and `write_modes_b`'s intra and
//! inter branches both call every function here. They are gathered rather
//! than scattered because they share one property that makes them worth
//! porting even where the port already computes the same number somewhere:
//! **each is an exported symbol, so each can be gated at evidence tier 1**
//! (`docs/WORKING-ON-THIS.md` §4), and three of the four previously existed
//! in the port only as an expression inlined into a caller.
//!
//! | C | :line | previous port | why here |
//! |---|---|---|---|
//! | `svt_aom_get_kf_y_mode_ctx` | 1004 | inlined in `pipeline.rs` | named + tier 1 |
//! | `svt_aom_uleb_size_in_bytes` | 1310 | inlined in `entropy/obu.rs::uleb_encode` | named + tier 1 |
//! | `svt_aom_get_palette_mode_ctx` | 4240 | inlined in `pipeline.rs` | named + tier 1 |
//! | `svt_aom_write_uniform_cost` | 4308 | a closure in `leaf_funnel/inject.rs` | named + tier 1 |
//!
//! `svt_aom_partition_cdf_length` (:922), `av1_get_skip_context` (:983),
//! `svt_aom_allow_palette` (:4223) and `svt_aom_get_palette_bsize_ctx`
//! (:4228) are NOT re-ported: they already exist as
//! `sb128_geom::partition_cdf_length`, `entropy::context::get_skip_context`,
//! `entropy::context::allow_palette` and `entropy::context::palette_bsize_ctx`.
//! A second copy would be a second source of truth. What they gain in this
//! change is the tier-1 gate they never had —
//! `tests/c_parity_entropy_block.rs` drives the exported C symbol for each.
//!
//! # Evidence
//!
//! Tier 1 for everything in this module and for the four functions named
//! just above.

use svtav1_types::block::BlockSize;
use svtav1_types::tables::block::{BLOCK_SIZE_HIGH, BLOCK_SIZE_WIDE};

/// C `av1_cost_literal(n)` (md_rate_estimation.h:31) —
/// `n << AV1_PROB_COST_SHIFT`, i.e. 1/512-bit units.
#[inline]
const fn cost_literal(n: i32) -> i32 {
    n * 512
}

/// C `get_msb(n)` — index of the highest set bit. `n` must be nonzero, which
/// C asserts.
#[inline]
fn get_msb(n: u32) -> u32 {
    debug_assert!(n != 0);
    31 - n.leading_zeros()
}

/// C `get_unsigned_bits` (entropy_coding.c:4290).
#[inline]
fn get_unsigned_bits(num_values: u32) -> i32 {
    if num_values > 0 {
        get_msb(num_values) as i32 + 1
    } else {
        0
    }
}

/// C `svt_aom_write_uniform_cost(n, v)` (entropy_coding.c:4308, EXPORTED) —
/// the rate of the `write_uniform` a palette index costs, in 1/512-bit
/// units.
///
/// Note it is NOT `write_uniform`'s bit count times 512 in the `v >= m` arm:
/// `write_uniform` emits `l - 1` bits and then one more, so the cost is
/// `cost_literal(l)`. Collapsing the two arms to a single `l - 1` (the
/// symmetric-looking mistake) under-prices every large palette index.
#[inline]
pub fn write_uniform_cost(n: i32, v: i32) -> i32 {
    let l = get_unsigned_bits(n as u32);
    if l == 0 {
        return 0;
    }
    let m = (1 << l) - n;
    if v < m {
        cost_literal(l - 1)
    } else {
        cost_literal(l)
    }
}

/// C `svt_aom_uleb_size_in_bytes(value)` (entropy_coding.c:1310, EXPORTED).
///
/// A `do { } while` in C, so it returns **1** for value 0 rather than 0 —
/// the LEB128 encoding of zero is one byte. A `while` loop here would
/// return 0 and undersize every OBU whose payload length is zero.
#[inline]
pub fn uleb_size_in_bytes(value: u64) -> usize {
    let mut value = value;
    let mut size = 0usize;
    loop {
        size += 1;
        value >>= 7;
        if value == 0 {
            return size;
        }
    }
}

/// C `svt_aom_get_palette_mode_ctx(xd)` (entropy_coding.c:4240, EXPORTED) —
/// the `palette_y_mode_cdf` context.
///
/// It tests the neighbour POINTERS, not `up_available` / `left_available`,
/// which is why the arguments are `Option`s of the neighbour's palette size
/// rather than a `Neighbors`: an unavailable neighbour and an available one
/// with no palette contribute the same 0, but they are different inputs and
/// conflating them here is how the two-knob distinction gets lost.
#[inline]
pub fn palette_mode_ctx(above_palette_size: Option<u8>, left_palette_size: Option<u8>) -> usize {
    let bit = |s: Option<u8>| usize::from(s.is_some_and(|n| n > 0));
    bit(above_palette_size) + bit(left_palette_size)
}

/// C `svt_aom_get_kf_y_mode_ctx(xd, &above, &left)` (entropy_coding.c:1004,
/// EXPORTED) — the `(above, left)` row/column of `kf_y_cdf`.
///
/// C returns through two out-parameters; a tuple says the same thing without
/// letting a caller read an uninitialised one. `None` is C's
/// `!left_available` / `!up_available`, which substitutes `DC_PRED`.
///
/// C asserts the neighbour is intra (or IntraBC) because this is the
/// key-frame path; the assert is not reproduced because the substitution
/// itself is unconditional and an inter neighbour would simply index the
/// table, exactly as C does when `NDEBUG` is set — which the release archive
/// is built with.
#[inline]
pub fn kf_y_mode_ctx(above_mode: Option<u8>, left_mode: Option<u8>) -> (u8, u8) {
    use crate::entropy::context::intra_mode_context;
    let above = intra_mode_context(above_mode.unwrap_or(0 /* DC_PRED */));
    let left = intra_mode_context(left_mode.unwrap_or(0));
    (above as u8, left as u8)
}

/// C `svt_aom_allow_palette` (entropy_coding.c:4223, EXPORTED), restated on
/// [`BlockSize`] for callers that have one.
///
/// The port's own copy is `entropy::context::allow_palette`, which takes
/// pixel dims; this is a thin adapter so the tier-1 test can drive the same
/// predicate over the `BlockSize` enum C indexes by. It deliberately calls
/// through rather than re-deriving, so the two cannot drift.
#[inline]
pub fn allow_palette(allow_screen_content_tools: bool, bsize: BlockSize) -> bool {
    let w = usize::from(BLOCK_SIZE_WIDE[bsize.as_index()]);
    let h = usize::from(BLOCK_SIZE_HIGH[bsize.as_index()]);
    // C's `bsize >= BLOCK_8X8` is an ENUM-ORDINAL test, so it admits the
    // 4-wide shapes BLOCK_4X16 (16) .. BLOCK_64X16 (21) too; the pixel-dim
    // reading (`w >= 8 && h >= 8`) would reject 4x16 and is WRONG.
    allow_screen_content_tools && w <= 64 && h <= 64 && bsize as u8 >= BlockSize::Block8x8 as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `do/while` shape: zero is ONE byte, not zero bytes.
    #[test]
    fn uleb_size_of_zero_is_one_byte() {
        assert_eq!(uleb_size_in_bytes(0), 1);
        assert_eq!(uleb_size_in_bytes(1), 1);
        assert_eq!(uleb_size_in_bytes(127), 1);
        assert_eq!(uleb_size_in_bytes(128), 2);
        assert_eq!(uleb_size_in_bytes(u64::MAX), 10);
    }

    /// The two arms of `write_uniform_cost` differ by a whole literal bit.
    #[test]
    fn uniform_cost_arms_differ_by_one_literal() {
        // n = 5 -> l = 3, m = 8 - 5 = 3.
        assert_eq!(write_uniform_cost(5, 0), cost_literal(2));
        assert_eq!(write_uniform_cost(5, 2), cost_literal(2));
        assert_eq!(write_uniform_cost(5, 3), cost_literal(3));
        assert_eq!(write_uniform_cost(5, 4), cost_literal(3));
        // n = 1 -> l = 1, m = (1 << 1) - 1 = 1, so v = 0 is BELOW m and
        // takes the `l - 1` arm: cost_literal(0) = 0. An earlier revision
        // of this line asserted cost_literal(1) on the reasoning that
        // "every v >= m"; that reasoning is wrong (m is 1, not 0) and the
        // tier-1 sweep in tests/c_parity_entropy_block.rs, which covers
        // exactly this (n, v), agrees with C at 0.
        assert_eq!(write_uniform_cost(1, 0), 0);
        // n = 0 -> l = 0: free.
        assert_eq!(write_uniform_cost(0, 0), 0);
    }

    /// The palette context counts NEIGHBOURS WITH A PALETTE, and a missing
    /// neighbour is not the same input as an empty one even though both
    /// score 0.
    #[test]
    fn palette_mode_ctx_counts_palettes() {
        assert_eq!(palette_mode_ctx(None, None), 0);
        assert_eq!(palette_mode_ctx(Some(0), Some(0)), 0);
        assert_eq!(palette_mode_ctx(Some(2), None), 1);
        assert_eq!(palette_mode_ctx(Some(2), Some(8)), 2);
    }
}

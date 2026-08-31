//! Two pieces of `Source/Lib/Codec/md_rate_estimation.c` that the inter path
//! needs and the port did not have.
//!
//! ## Coverage — 2 of 2 functions in this group, plus one dependency
//!
//! | C function | line | here |
//! |---|---|---|
//! | `get_interinter_wedge_bits` | 23 | [`get_interinter_wedge_bits`] |
//! | `svt_aom_get_me_qindex` | 1084 | [`get_me_qindex`] |
//! | `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053) | — | [`get_wedge_params_bits`] |
//!
//! MISSING from md_rate_estimation.c: everything else — this module does NOT
//! port `svt_aom_estimate_syntax_rate` itself, only the `wedge_idx_fac_bits`
//! rows it builds through the gate above ([`wedge_idx_fac_bits`]), and the
//! MV-rate half of the file already lives in `crate::inter_mv_code`.
//!
//! ## Evidence
//!
//! Tier 1 for both. `svt_aom_get_me_qindex` and `svt_aom_get_wedge_params_bits`
//! are exported symbols (`nm -g` prints `T`); `get_interinter_wedge_bits` is
//! `static` and prints nothing, but the table it consults is reachable through
//! the exported `svt_aom_get_wedge_params_bits`, AND the row it gates
//! (`md_rate_est_ctx->wedge_idx_fac_bits`) is produced by the exported
//! `svt_aom_estimate_syntax_rate` — so the gate's OBSERVABLE EFFECT is driven
//! end to end rather than the two-line wrapper being re-transcribed.
//! `tests/c_parity_md_rate_estimation.rs` has the differentials.

/// C `BLOCK_SIZES_ALL`.
pub const BLOCK_SIZES_ALL: usize = 22;

/// C `wedge_params_lookup[bsize].bits` (inter_prediction.c:1990-2013), the
/// only field `svt_aom_get_wedge_params_bits` (`:2053`) returns.
///
/// Non-zero for exactly the ten block sizes that have a wedge codebook:
/// 8x8, 8x16, 16x8, 16x16, 16x32, 32x16, 32x32, 8x32, 32x8 — plus nothing
/// else. Every entry is either 0 or 4.
pub const WEDGE_PARAMS_BITS: [i32; BLOCK_SIZES_ALL] = [
    0, // BLOCK_4X4
    0, // BLOCK_4X8
    0, // BLOCK_8X4
    4, // BLOCK_8X8
    4, // BLOCK_8X16
    4, // BLOCK_16X8
    4, // BLOCK_16X16
    4, // BLOCK_16X32
    4, // BLOCK_32X16
    4, // BLOCK_32X32
    0, // BLOCK_32X64
    0, // BLOCK_64X32
    0, // BLOCK_64X64
    0, // BLOCK_64X128
    0, // BLOCK_128X64
    0, // BLOCK_128X128
    0, // BLOCK_4X16
    0, // BLOCK_16X4
    4, // BLOCK_8X32
    4, // BLOCK_32X8
    0, // BLOCK_16X64
    0, // BLOCK_64X16
];

/// C `svt_aom_get_wedge_params_bits` (inter_prediction.c:2053-2055, EXPORTED).
#[inline]
pub fn get_wedge_params_bits(bsize: usize) -> i32 {
    WEDGE_PARAMS_BITS[bsize]
}

/// C `get_interinter_wedge_bits` (md_rate_estimation.c:23-26).
///
/// `wbits + 1` — the extra bit is the wedge SIGN, which is coded alongside the
/// index. Returning `wbits` would under-count every compound-wedge candidate's
/// rate; returning a non-zero value for a size with no codebook would make
/// `svt_aom_estimate_syntax_rate` fill a `wedge_idx_fac_bits` row that C leaves
/// at its zero initialiser.
#[inline]
pub fn get_interinter_wedge_bits(bsize: usize) -> i32 {
    let wbits = get_wedge_params_bits(bsize);
    if wbits > 0 { wbits + 1 } else { 0 }
}

/// The `wedge_idx_fac_bits` half of `svt_aom_estimate_syntax_rate`
/// (md_rate_estimation.c:316-320).
///
/// C's loop is
/// ```text
/// for (i = 0; i < BLOCK_SIZES_ALL; ++i)
///     if (get_interinter_wedge_bits((BlockSize)i))
///         svt_aom_get_syntax_rate_from_cdf(md->wedge_idx_fac_bits[i], fc->wedge_idx_cdf[i], NULL);
/// ```
/// so a rejected row keeps whatever the context was zero-initialised with.
/// This function reproduces that: rows the gate rejects stay all-zero.
///
/// The whole loop lives inside `if (!is_i_slice)` (`:257`), so on a key frame
/// C fills NO row; that is the caller's condition, not this function's.
pub fn wedge_idx_fac_bits(
    wedge_idx_cdf: &[[u16; 17]; BLOCK_SIZES_ALL],
) -> [[i32; 16]; BLOCK_SIZES_ALL] {
    let mut out = [[0i32; 16]; BLOCK_SIZES_ALL];
    for i in 0..BLOCK_SIZES_ALL {
        if get_interinter_wedge_bits(i) != 0 {
            crate::quant::syntax_rate_from_cdf(&mut out[i], &wedge_idx_cdf[i]);
        }
    }
    out
}

/// C `svt_aom_get_me_qindex` (md_rate_estimation.c:1084-1114, EXPORTED): the
/// per-SB ME qindex that sets MD's lambda.
///
/// At SB64 it is a straight `b64_me_qindex[sb->index]` lookup. At SB128 the SB
/// covers up to four b64 cells and C averages the ones that exist.
///
/// The divisor is NOT "the number of neighbours checked": `valid_b64_cnt`
/// starts at 1, gains 1 for a right neighbour and 1 for a below neighbour, and
/// then — only if BOTH were present, i.e. it reached exactly 3 — takes the
/// diagonal and increments to 4. So the reachable divisors are **1, 2 and 4;
/// 3 never divides anything**. An implementation that averaged three cells on
/// the frame's right or bottom edge would produce a different lambda.
///
/// `b64_me_qindex` is filled by `svt_av1_generate_b64_me_qindex_map`
/// (rc_aq.c:656, called from rc_process.c:748), which is in the rate-control
/// group and is NOT ported here — so this function currently has no producer
/// inside the port. It is translated now because it is the second qindex
/// argument to `svt_aom_mode_decision_configure_sb` (enc_dec_process.c:2926)
/// and to `svt_aom_compute_rd_mult` (rc_aq.c:767), and a wrong lambda changes
/// the RD winner on every block.
///
/// Video-only: `b64_me_qindex` has no still-picture producer either.
pub fn get_me_qindex(
    b64_me_qindex: &[u8],
    aligned_width: u16,
    aligned_height: u16,
    sb_index: u32,
    sb_org_x: u32,
    sb_org_y: u32,
    is_sb128: bool,
) -> u8 {
    if !is_sb128 {
        return b64_me_qindex[sb_index as usize];
    }

    let pic_width_in_b64 = u32::from(aligned_width).div_ceil(64);
    let pic_height_in_b64 = u32::from(aligned_height).div_ceil(64);

    let x_b64_index = sb_org_x / 64;
    let y_b64_index = sb_org_y / 64;
    let b64_index = x_b64_index + y_b64_index * pic_width_in_b64;

    let mut valid_b64_cnt: u8 = 1;
    // C accumulates in `uint16_t`: four qindex values <= 255 sum to <= 1020,
    // so it cannot wrap, but the type is kept so the division matches.
    let mut sum_me_qindex: u16 = u16::from(b64_me_qindex[b64_index as usize]);

    if (x_b64_index + 1) < pic_width_in_b64 {
        sum_me_qindex += u16::from(b64_me_qindex[(b64_index + 1) as usize]);
        valid_b64_cnt += 1;
    }
    if (y_b64_index + 1) < pic_height_in_b64 {
        sum_me_qindex += u16::from(b64_me_qindex[(b64_index + pic_width_in_b64) as usize]);
        valid_b64_cnt += 1;
    }
    if valid_b64_cnt == 3 {
        sum_me_qindex += u16::from(b64_me_qindex[(b64_index + 1 + pic_width_in_b64) as usize]);
        valid_b64_cnt += 1;
    }

    (sum_me_qindex / u16::from(valid_b64_cnt)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `+ 1` (the wedge sign bit) and the zero gate.
    #[test]
    fn wedge_bits_add_the_sign_bit() {
        assert_eq!(get_interinter_wedge_bits(3), 5, "BLOCK_8X8: 4 bits + sign");
        assert_eq!(get_interinter_wedge_bits(0), 0, "BLOCK_4X4 has no codebook");
        assert_eq!(get_interinter_wedge_bits(15), 0, "BLOCK_128X128");
        assert_eq!(get_interinter_wedge_bits(18), 5, "BLOCK_8X32");
    }

    /// The divisor 3 is unreachable — see [`get_me_qindex`]'s note.
    #[test]
    fn sb128_divisor_is_never_three() {
        // 2x2 b64 grid, all four cells present: divisor 4.
        let q = [40u8, 44, 48, 52];
        assert_eq!(
            get_me_qindex(&q, 128, 128, 0, 0, 0, true),
            (40 + 44 + 48 + 52) / 4
        );
        // Right edge (1 column of b64): only the below neighbour -> divisor 2.
        let q = [40u8, 60];
        assert_eq!(get_me_qindex(&q, 64, 128, 0, 0, 0, true), (40 + 60) / 2);
        // Bottom edge (1 row): only the right neighbour -> divisor 2.
        let q = [40u8, 60];
        assert_eq!(get_me_qindex(&q, 128, 64, 0, 0, 0, true), (40 + 60) / 2);
        // Single b64: divisor 1.
        let q = [77u8];
        assert_eq!(get_me_qindex(&q, 64, 64, 0, 0, 0, true), 77);
    }

    /// SB64 ignores the geometry entirely and indexes by sb->index.
    #[test]
    fn sb64_is_a_plain_lookup() {
        let q = [10u8, 20, 30, 40];
        assert_eq!(get_me_qindex(&q, 999, 999, 2, 12345, 6789, false), 30);
    }
}

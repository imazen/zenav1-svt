//! SVT's split 8-bit + 2-bit 10-bit representation, and the pack back to
//! `u16`.
//!
//! Ported from SVT-AV1 v4.2.0:
//! * `Source/Lib/Codec/inter_prediction.c` — `svt_aom_pack_block` (:26).
//! * `Source/Lib/Codec/pic_operators.c` — `svt_aom_pack2d_src` (:341).
//! * `Source/Lib/C_DEFAULT/pack_unpack_c.c` — `svt_enc_msb_pack2_d` (:18).
//!
//! # What the representation is
//!
//! SVT stores a 10-bit plane as TWO 8-bit planes: `in8_bit_buffer` holds the
//! eight MSBs, and `inn_bit_buffer` holds the two LSBs **in the top two bits
//! of a whole byte** — one byte per pixel, not four pixels per byte. (The
//! four-per-byte form is a different function, `svt_compressed_packmsb_c`,
//! reached through `svt_aom_compressed_pack_sb`; do not confuse the two.)
//! Packing is therefore
//! `out = (msb << 2) | (lsb_byte >> 6)`.
//!
//! # Why there is ONE implementation here and TWO in C
//!
//! `svt_aom_pack2d_src` picks `svt_pack2d_16_bit_src_mul4` (an RTCD SIMD
//! kernel) when `width % 4 == 0 && height % 2 == 0`, and the scalar
//! `svt_enc_msb_pack2_d` otherwise. That is a SPEED dispatch: both arms are
//! required to produce the same bytes, and the parity test drives C's real
//! `svt_aom_pack_block` on widths/heights that select each arm, so the single
//! Rust implementation is gated against both.
//!
//! # Argument order
//!
//! C's `svt_enc_msb_pack2_d` interleaves its arguments
//! (`in8, in8_stride, inn, out16, inn_stride, out_stride, w, h` — the output
//! pointer sits between the two input strides). This port uses
//! `svt_aom_pack2d_src`'s order instead (each buffer immediately followed by
//! its own stride), which is the same call with the pairs kept together.
//!
//! # Evidence
//!
//! TIER 1 — `svt_aom_pack_block`, `svt_aom_pack2d_src` and
//! `svt_enc_msb_pack2_d` are all exported symbols (`nm`: `T`), gated in
//! `tests/c_parity_port_pack.rs`.

/// `INTERPOLATION_OFFSET` (definitions.h:365) — the MC border, in pixels, that
/// the 10-bit light-PD1 path packs around the block on every side.
pub const INTERPOLATION_OFFSET: usize = 8;

/// `svt_enc_msb_pack2_d` (C_DEFAULT/pack_unpack_c.c:18), and — because the
/// SIMD arm reproduces it — the semantics of `svt_aom_pack2d_src`
/// (pic_operators.c:341) and `svt_aom_pack_block` (inter_prediction.c:26).
///
/// C's `& 3` after `>> 6` is a no-op on a `uint8_t` and is dropped; the shift
/// alone already yields 0..=3.
#[allow(clippy::too_many_arguments)]
pub fn pack2d_src(
    in8: &[u8],
    in8_stride: usize,
    inn: &[u8],
    inn_stride: usize,
    out16: &mut [u16],
    out_stride: usize,
    width: usize,
    height: usize,
) {
    for y in 0..height {
        let msb = &in8[y * in8_stride..][..width];
        let lsb = &inn[y * inn_stride..][..width];
        let dst = &mut out16[y * out_stride..][..width];
        for ((o, &m), &l) in dst.iter_mut().zip(msb).zip(lsb) {
            *o = (u16::from(m) << 2) | u16::from(l >> 6);
        }
    }
}

/// `svt_aom_pack_block` (inter_prediction.c:26) — a pass-through to
/// [`pack2d_src`], kept as its own name because it is the one the MC path
/// calls and the one the parity test drives.
#[allow(clippy::too_many_arguments)]
pub fn pack_block(
    in8: &[u8],
    in8_stride: usize,
    inn: &[u8],
    inn_stride: usize,
    out16: &mut [u16],
    out_stride: usize,
    width: usize,
    height: usize,
) {
    pack2d_src(
        in8, in8_stride, inn, inn_stride, out16, out_stride, width, height,
    );
}

/// Whether `svt_aom_pack2d_src` would take the SIMD (`_mul4`) arm rather than
/// the scalar one, for a given extent.
///
/// The port has one implementation, so this changes nothing about its output;
/// it exists so a test can SAY which C arm a cell drives instead of assuming.
pub fn pack2d_takes_simd_arm(width: usize, height: usize) -> bool {
    width % 4 == 0 && height % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_msb_and_the_top_two_lsb_bits() {
        // 0xFF msb with 0b11_000000 lsb is the largest 10-bit value.
        let in8 = [0xFFu8, 0x00, 0x80, 0x01];
        let inn = [0xC0u8, 0xC0, 0x00, 0x40];
        let mut out = [0u16; 4];
        pack2d_src(&in8, 4, &inn, 4, &mut out, 4, 4, 1);
        assert_eq!(out, [1023, 3, 512, 5]);
    }

    #[test]
    fn only_the_top_two_lsb_bits_survive() {
        // 0x3F has all of bits 0..5 set and none of 6..7: it must contribute
        // nothing. A port that used `& 3` on the unshifted byte would give 3.
        let mut out = [0u16; 1];
        pack2d_src(&[0x10], 1, &[0x3F], 1, &mut out, 1, 1, 1);
        assert_eq!(out, [0x10 << 2]);
    }

    #[test]
    fn strides_are_independent() {
        let in8 = [1u8, 2, 9, 9, 3, 4, 9, 9];
        let inn = [0u8, 0, 0, 0x40, 0, 0, 0, 0];
        let mut out = [0u16; 6];
        pack2d_src(&in8, 4, &inn, 4, &mut out, 3, 2, 2);
        assert_eq!(out, [4, 8, 0, 12, 16, 0]);
    }

    #[test]
    fn simd_arm_predicate_matches_c_condition() {
        assert!(pack2d_takes_simd_arm(20, 24));
        assert!(!pack2d_takes_simd_arm(21, 24));
        assert!(!pack2d_takes_simd_arm(20, 23));
    }
}

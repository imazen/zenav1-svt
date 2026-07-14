//! Overlapped Block Motion Compensation (OBMC).
//!
//! Spec 06: Overlapped block motion compensation blending.
//!
//! OBMC blends the prediction of the current block with predictions from
//! neighboring blocks to reduce blocking artifacts at boundaries. This is a
//! bit-exact port of the SVT-AV1 reconstruction-side blend
//! (`build_obmc_inter_pred_{above,left}` in enc_inter_prediction.c), which
//! calls `svt_aom_blend_a64_{v,h}mask_c` with `svt_av1_get_obmc_mask(overlap)`.
//!
//! Blend rule (`AOM_BLEND_A64`): with mask `m` (0..=64), the CURRENT block
//! prediction gets weight `m` and the neighbor gets `64 - m`:
//!   `out = (m * cur + (64 - m) * neighbor + 32) >> 6`.
//! The mask starts low at the boundary (neighbor influence peaks there) and
//! rises to 64 (fully current) away from it. Verified bit-exact against the C
//! reference in `tests/c_parity_obmc.rs`.

/// Blend the current prediction with the ABOVE neighbor over the top `overlap`
/// rows. Mirrors `svt_aom_blend_a64_vmask_c` (mask indexed by row).
pub fn obmc_blend_above(
    dst: &mut [u8],
    dst_stride: usize,
    above_pred: &[u8],
    above_stride: usize,
    width: usize,
    height: usize,
    overlap: usize,
) {
    let masks = obmc_mask(overlap);
    for r in 0..overlap.min(height) {
        let mask = masks[r] as u32;
        for c in 0..width {
            let cur = dst[r * dst_stride + c] as u32;
            let above = above_pred[r * above_stride + c] as u32;
            // AOM_BLEND_A64(mask, cur, above)
            dst[r * dst_stride + c] = ((mask * cur + (64 - mask) * above + 32) >> 6) as u8;
        }
    }
}

/// Blend the current prediction with the LEFT neighbor over the left `overlap`
/// columns. Mirrors `svt_aom_blend_a64_hmask_c` (mask indexed by column).
pub fn obmc_blend_left(
    dst: &mut [u8],
    dst_stride: usize,
    left_pred: &[u8],
    left_stride: usize,
    width: usize,
    height: usize,
    overlap: usize,
) {
    let masks = obmc_mask(overlap);
    for r in 0..height {
        for c in 0..overlap.min(width) {
            let mask = masks[c] as u32;
            let cur = dst[r * dst_stride + c] as u32;
            let left = left_pred[r * left_stride + c] as u32;
            // AOM_BLEND_A64(mask, cur, left)
            dst[r * dst_stride + c] = ((mask * cur + (64 - mask) * left + 32) >> 6) as u8;
        }
    }
}

/// OBMC blend mask for an overlap size — verbatim `svt_av1_get_obmc_mask`
/// (`obmc_mask_{1,2,4,8,16,32}` in inter_prediction.c). Weights rise from the
/// boundary (neighbor-heavy) to 64 (fully current). C `assert(0)`s on any
/// other length; only powers of two up to 32 ever occur (overlap = dim/2).
fn obmc_mask(overlap: usize) -> &'static [u8] {
    match overlap {
        1 => &[64],
        2 => &[45, 64],
        4 => &[39, 50, 59, 64],
        8 => &[36, 42, 48, 53, 57, 61, 64, 64],
        16 => &[34, 37, 40, 43, 46, 49, 52, 54, 56, 58, 60, 61, 64, 64, 64, 64],
        32 => &[
            33, 35, 36, 38, 40, 41, 43, 44, 45, 47, 48, 50, 51, 52, 53, 55, 56, 57, 58, 59, 60, 60,
            61, 62, 64, 64, 64, 64, 64, 64, 64, 64,
        ],
        _ => panic!("obmc_mask: unsupported overlap {overlap} (C supports 1/2/4/8/16/32)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obmc_blend_above_basic() {
        let mut dst = [100u8; 16]; // 4x4
        let above = [200u8; 16];
        obmc_blend_above(&mut dst, 4, &above, 4, 4, 4, 2);
        // mask[0]=45: (45*100 + 19*200 + 32)>>6 = 8332>>6 = 130 (current-dominant).
        assert_eq!(dst[0], 130, "row 0 blends toward current with mask 45");
        // mask[1]=64: fully current -> unchanged.
        assert_eq!(dst[4], 100, "row 1 mask=64 -> current");
        // Row 2+ outside the overlap band -> unchanged.
        assert_eq!(dst[8], 100, "row 2 unchanged");
    }

    #[test]
    fn obmc_blend_left_basic() {
        let mut dst = [100u8; 16];
        let left = [200u8; 16];
        obmc_blend_left(&mut dst, 4, &left, 4, 4, 4, 2);
        // col0 mask=45 -> 130; col1 mask=64 -> current (100).
        assert_eq!(dst[0], 130);
        assert_eq!(dst[1], 100);
        // Column 2+ outside the overlap band -> unchanged.
        assert_eq!(dst[2], 100);
    }

    #[test]
    fn obmc_mask_sizes() {
        assert_eq!(obmc_mask(1).len(), 1);
        assert_eq!(obmc_mask(2).len(), 2);
        assert_eq!(obmc_mask(4).len(), 4);
        assert_eq!(obmc_mask(8).len(), 8);
        assert_eq!(obmc_mask(16).len(), 16);
        assert_eq!(obmc_mask(32).len(), 32);
    }

    #[test]
    fn obmc_mask_boundary_to_current() {
        // Every table ends at 64 (fully current away from the boundary) and
        // starts below 64 (neighbor has influence at the boundary).
        for &n in &[2usize, 4, 8, 16, 32] {
            let m = obmc_mask(n);
            assert_eq!(*m.last().unwrap(), 64, "overlap {n} ends fully-current");
            assert!(m[0] < 64, "overlap {n} blends neighbor at boundary");
        }
    }
}

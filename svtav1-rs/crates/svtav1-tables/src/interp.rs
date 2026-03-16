//! Interpolation filter coefficient tables.
//!
//! Ported from `inter_prediction.c` lines 238-1180.

/// Number of sub-pixel shifts (16 phases).
pub const SUBPEL_SHIFTS: usize = 16;

/// Interpolation kernel: 8 taps per sub-pixel phase.
pub type InterpKernel = [i16; 8];

/// 8-tap regular interpolation filter (EIGHTTAP_REGULAR).
pub const SUB_PEL_FILTERS_8: [InterpKernel; SUBPEL_SHIFTS] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 2, -6, 126, 8, -2, 0, 0],
    [0, 2, -10, 122, 18, -4, 0, 0],
    [0, 2, -12, 116, 28, -8, 2, 0],
    [0, 2, -14, 110, 38, -10, 2, 0],
    [0, 2, -14, 102, 48, -12, 2, 0],
    [0, 2, -16, 94, 58, -12, 2, 0],
    [0, 2, -14, 84, 66, -12, 2, 0],
    [0, 2, -14, 76, 76, -14, 2, 0],
    [0, 2, -12, 66, 84, -14, 2, 0],
    [0, 2, -12, 58, 94, -16, 2, 0],
    [0, 2, -12, 48, 102, -14, 2, 0],
    [0, 2, -10, 38, 110, -14, 2, 0],
    [0, 2, -8, 28, 116, -12, 2, 0],
    [0, 0, -4, 18, 122, -10, 2, 0],
    [0, 0, -2, 8, 126, -6, 2, 0],
];

/// 8-tap sharp interpolation filter (MULTITAP_SHARP).
pub const SUB_PEL_FILTERS_8SHARP: [InterpKernel; SUBPEL_SHIFTS] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [-2, 2, -6, 126, 8, -2, 2, 0],
    [-2, 6, -12, 124, 16, -6, 4, -2],
    [-2, 8, -18, 120, 26, -10, 6, -2],
    [-4, 10, -22, 116, 38, -14, 6, -2],
    [-4, 10, -22, 108, 48, -18, 8, -2],
    [-4, 10, -24, 100, 60, -20, 8, -2],
    [-4, 10, -24, 90, 70, -22, 10, -2],
    [-4, 12, -24, 80, 80, -24, 12, -4],
    [-2, 10, -22, 70, 90, -24, 10, -4],
    [-2, 8, -20, 60, 100, -24, 10, -4],
    [-2, 8, -18, 48, 108, -22, 10, -4],
    [-2, 6, -14, 38, 116, -22, 10, -4],
    [-2, 6, -10, 26, 120, -18, 8, -2],
    [-2, 4, -6, 16, 124, -12, 6, -2],
    [0, 2, -2, 8, 126, -6, 2, -2],
];

/// 8-tap smooth interpolation filter (EIGHTTAP_SMOOTH).
pub const SUB_PEL_FILTERS_8SMOOTH: [InterpKernel; SUBPEL_SHIFTS] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 2, 28, 62, 34, 2, 0, 0],
    [0, 0, 26, 62, 36, 4, 0, 0],
    [0, 0, 22, 62, 40, 4, 0, 0],
    [0, 0, 20, 60, 42, 6, 0, 0],
    [0, 0, 18, 58, 44, 8, 0, 0],
    [0, 0, 16, 56, 46, 10, 0, 0],
    [0, -2, 16, 54, 48, 12, 0, 0],
    [0, -2, 14, 52, 52, 14, -2, 0],
    [0, 0, 12, 48, 54, 16, -2, 0],
    [0, 0, 10, 46, 56, 16, 0, 0],
    [0, 0, 8, 44, 58, 18, 0, 0],
    [0, 0, 6, 42, 60, 20, 0, 0],
    [0, 0, 4, 40, 62, 22, 0, 0],
    [0, 0, 4, 36, 62, 26, 0, 0],
    [0, 0, 2, 34, 62, 28, 2, 0],
];

/// Bilinear interpolation filter.
pub const BILINEAR_FILTERS: [InterpKernel; SUBPEL_SHIFTS] = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, 0, 120, 8, 0, 0, 0],
    [0, 0, 0, 112, 16, 0, 0, 0],
    [0, 0, 0, 104, 24, 0, 0, 0],
    [0, 0, 0, 96, 32, 0, 0, 0],
    [0, 0, 0, 88, 40, 0, 0, 0],
    [0, 0, 0, 80, 48, 0, 0, 0],
    [0, 0, 0, 72, 56, 0, 0, 0],
    [0, 0, 0, 64, 64, 0, 0, 0],
    [0, 0, 0, 56, 72, 0, 0, 0],
    [0, 0, 0, 48, 80, 0, 0, 0],
    [0, 0, 0, 40, 88, 0, 0, 0],
    [0, 0, 0, 32, 96, 0, 0, 0],
    [0, 0, 0, 24, 104, 0, 0, 0],
    [0, 0, 0, 16, 112, 0, 0, 0],
    [0, 0, 0, 8, 120, 0, 0, 0],
];

#[cfg(test)]
mod tests {
    use super::*;

    /// All filter taps must sum to 128 (the fixed-point normalization factor).
    #[test]
    fn filter_taps_sum_to_128() {
        for (name, filters) in [
            ("regular", &SUB_PEL_FILTERS_8),
            ("sharp", &SUB_PEL_FILTERS_8SHARP),
            ("smooth", &SUB_PEL_FILTERS_8SMOOTH),
            ("bilinear", &BILINEAR_FILTERS),
        ] {
            for (phase, kernel) in filters.iter().enumerate() {
                let sum: i16 = kernel.iter().sum();
                assert_eq!(
                    sum, 128,
                    "{name} filter phase {phase}: taps sum to {sum}, expected 128"
                );
            }
        }
    }

    /// Phase 0 must be the identity (passthrough) for all filters.
    #[test]
    fn phase_zero_is_identity() {
        for filters in [
            &SUB_PEL_FILTERS_8,
            &SUB_PEL_FILTERS_8SHARP,
            &SUB_PEL_FILTERS_8SMOOTH,
            &BILINEAR_FILTERS,
        ] {
            assert_eq!(filters[0], [0, 0, 0, 128, 0, 0, 0, 0]);
        }
    }

    /// Regular filter phase 8 must be symmetric (half-pixel).
    #[test]
    fn regular_half_pixel_symmetric() {
        let hp = SUB_PEL_FILTERS_8[8];
        // [0, 2, -14, 76, 76, -14, 2, 0] — symmetric around center
        assert_eq!(hp[3], hp[4]);
        assert_eq!(hp[2], hp[5]);
        assert_eq!(hp[1], hp[6]);
        assert_eq!(hp[0], hp[7]);
    }
}

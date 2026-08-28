//! Coefficient scan order tables.
//!
//! Ported from `coefficients.c` lines 15-1200+.
//!
//! Scan orders define the zigzag traversal of transform coefficients.
//! There are three scan types per transform size: default (diagonal),
//! row, and column.

/// 4x4 column scan order.
pub const MCOL_SCAN_4X4: [i16; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];

/// 4x4 row scan order.
pub const MROW_SCAN_4X4: [i16; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// 4x4 default (diagonal zigzag) scan order.
pub const DEFAULT_SCAN_4X4: [i16; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// 8x8 default scan order.
pub const DEFAULT_SCAN_8X8: [i16; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// 8x8 column scan order.
pub const MCOL_SCAN_8X8: [i16; 64] = [
    0, 8, 16, 24, 32, 40, 48, 56, 1, 9, 17, 25, 33, 41, 49, 57, 2, 10, 18, 26, 34, 42, 50, 58, 3,
    11, 19, 27, 35, 43, 51, 59, 4, 12, 20, 28, 36, 44, 52, 60, 5, 13, 21, 29, 37, 45, 53, 61, 6,
    14, 22, 30, 38, 46, 54, 62, 7, 15, 23, 31, 39, 47, 55, 63,
];

/// 8x8 row scan order.
pub const MROW_SCAN_8X8: [i16; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
];

/// Scan type index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ScanType {
    /// Diagonal zigzag (default for 2D transforms).
    Default = 0,
    /// Row-major (for horizontal transforms).
    Row = 1,
    /// Column-major (for vertical transforms).
    Col = 2,
}

impl ScanType {
    pub const COUNT: usize = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default 4x4 scan must visit all 16 positions exactly once.
    #[test]
    fn scan_4x4_covers_all() {
        let mut visited = [false; 16];
        for &idx in &DEFAULT_SCAN_4X4 {
            assert!(!visited[idx as usize], "duplicate index {idx}");
            visited[idx as usize] = true;
        }
        assert!(visited.iter().all(|&v| v));
    }

    /// Default 8x8 scan must visit all 64 positions exactly once.
    #[test]
    fn scan_8x8_covers_all() {
        let mut visited = [false; 64];
        for &idx in &DEFAULT_SCAN_8X8 {
            assert!(!visited[idx as usize], "duplicate index {idx}");
            visited[idx as usize] = true;
        }
        assert!(visited.iter().all(|&v| v));
    }

    /// Column scan 4x4: first column visited first.
    #[test]
    fn col_scan_4x4_order() {
        // First 4 entries should be column 0: 0, 4, 8, 12
        assert_eq!(&MCOL_SCAN_4X4[..4], &[0, 4, 8, 12]);
    }

    /// Row scan 4x4: first row visited first.
    #[test]
    fn row_scan_4x4_order() {
        assert_eq!(&MROW_SCAN_4X4[..4], &[0, 1, 2, 3]);
    }
}

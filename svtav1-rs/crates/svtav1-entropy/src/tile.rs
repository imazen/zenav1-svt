//! Tile coding — splitting frames into independently-codeable tiles.
//!
//! AV1 supports uniform and non-uniform tile grids. Each tile is
//! independently entropy-coded, enabling parallel encoding/decoding.
//! Ported from SVT-AV1's entropy_coding.c tile functions.

use alloc::vec::Vec;

/// Tile grid configuration.
#[derive(Debug, Clone)]
pub struct TileConfig {
    /// Number of tile columns.
    pub tile_cols: u8,
    /// Number of tile rows.
    pub tile_rows: u8,
    /// Whether tiles are uniformly spaced.
    pub uniform_spacing: bool,
    /// Log2 of tile columns.
    pub tile_cols_log2: u8,
    /// Log2 of tile rows.
    pub tile_rows_log2: u8,
}

impl TileConfig {
    /// Create a uniform tile grid.
    pub fn uniform(cols_log2: u8, rows_log2: u8) -> Self {
        Self {
            tile_cols: 1 << cols_log2,
            tile_rows: 1 << rows_log2,
            uniform_spacing: true,
            tile_cols_log2: cols_log2,
            tile_rows_log2: rows_log2,
        }
    }

    /// Single tile (no tiling).
    pub fn single() -> Self {
        Self::uniform(0, 0)
    }

    /// Total number of tiles.
    pub fn num_tiles(&self) -> usize {
        self.tile_cols as usize * self.tile_rows as usize
    }
}

/// A single tile's encoding result.
#[derive(Debug)]
pub struct TileData {
    /// Tile row index.
    pub tile_row: u8,
    /// Tile col index.
    pub tile_col: u8,
    /// Encoded tile data (entropy-coded).
    pub data: Vec<u8>,
}

/// Write tile group OBU data.
///
/// For single-tile frames, just writes the tile data directly.
/// For multi-tile frames, writes tile sizes followed by tile data.
pub fn write_tile_group(tiles: &[TileData]) -> Vec<u8> {
    if tiles.len() == 1 {
        // Single tile — no size prefix needed
        return tiles[0].data.clone();
    }

    let mut output = Vec::new();

    // For multi-tile, each tile (except the last) is preceded by its size
    for (i, tile) in tiles.iter().enumerate() {
        if i < tiles.len() - 1 {
            // Write tile size as 4 bytes (little-endian)
            let size = tile.data.len() as u32;
            output.extend_from_slice(&size.to_le_bytes());
        }
        output.extend_from_slice(&tile.data);
    }

    output
}

/// Compute tile boundaries for a uniform grid.
pub fn compute_tile_boundaries(
    frame_width: u32,
    frame_height: u32,
    sb_size: u32,
    config: &TileConfig,
) -> Vec<TileBounds> {
    let sb_cols = frame_width.div_ceil(sb_size);
    let sb_rows = frame_height.div_ceil(sb_size);

    let mut tiles = Vec::new();
    for tr in 0..config.tile_rows {
        for tc in 0..config.tile_cols {
            let sb_col_start = (tc as u32 * sb_cols) / config.tile_cols as u32;
            let sb_col_end = ((tc as u32 + 1) * sb_cols) / config.tile_cols as u32;
            let sb_row_start = (tr as u32 * sb_rows) / config.tile_rows as u32;
            let sb_row_end = ((tr as u32 + 1) * sb_rows) / config.tile_rows as u32;

            tiles.push(TileBounds {
                col_start: sb_col_start * sb_size,
                col_end: (sb_col_end * sb_size).min(frame_width),
                row_start: sb_row_start * sb_size,
                row_end: (sb_row_end * sb_size).min(frame_height),
            });
        }
    }
    tiles
}

/// Pixel boundaries of a tile.
#[derive(Debug, Clone)]
pub struct TileBounds {
    pub col_start: u32,
    pub col_end: u32,
    pub row_start: u32,
    pub row_end: u32,
}

impl TileBounds {
    pub fn width(&self) -> u32 {
        self.col_end - self.col_start
    }
    pub fn height(&self) -> u32 {
        self.row_end - self.row_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tile() {
        let config = TileConfig::single();
        assert_eq!(config.num_tiles(), 1);
    }

    #[test]
    fn multi_tile() {
        let config = TileConfig::uniform(1, 1);
        assert_eq!(config.num_tiles(), 4); // 2x2
    }

    #[test]
    fn tile_boundaries_single() {
        let config = TileConfig::single();
        let bounds = compute_tile_boundaries(1920, 1080, 64, &config);
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].col_start, 0);
        assert_eq!(bounds[0].col_end, 1920);
        assert_eq!(bounds[0].row_start, 0);
        assert_eq!(bounds[0].row_end, 1080);
    }

    #[test]
    fn tile_boundaries_2x2() {
        let config = TileConfig::uniform(1, 1);
        let bounds = compute_tile_boundaries(128, 128, 64, &config);
        assert_eq!(bounds.len(), 4);
        // First tile
        assert_eq!(bounds[0].col_start, 0);
        assert_eq!(bounds[0].col_end, 64);
        assert_eq!(bounds[0].row_start, 0);
        assert_eq!(bounds[0].row_end, 64);
        // Last tile
        assert_eq!(bounds[3].col_start, 64);
        assert_eq!(bounds[3].col_end, 128);
    }

    #[test]
    fn write_single_tile_group() {
        let tiles = vec![TileData {
            tile_row: 0,
            tile_col: 0,
            data: vec![1, 2, 3, 4],
        }];
        let output = write_tile_group(&tiles);
        assert_eq!(output, vec![1, 2, 3, 4]);
    }

    #[test]
    fn write_multi_tile_group() {
        let tiles = vec![
            TileData {
                tile_row: 0,
                tile_col: 0,
                data: vec![10, 20],
            },
            TileData {
                tile_row: 0,
                tile_col: 1,
                data: vec![30, 40, 50],
            },
        ];
        let output = write_tile_group(&tiles);
        // First tile: 4-byte size (2) + data (10, 20)
        // Second tile: no size prefix + data (30, 40, 50)
        assert_eq!(output.len(), 4 + 2 + 3);
        assert_eq!(&output[0..4], &2u32.to_le_bytes());
    }
}

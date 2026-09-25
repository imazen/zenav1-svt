use super::*;

#[test]
fn partial_search_compares_boundary_rectangles_and_forced_corner_split() {
    for (w, h, expected) in [
        (64, 32, PartitionType::Horz),
        (32, 64, PartitionType::Vert),
        (32, 32, PartitionType::Split),
    ] {
        let source = alloc::vec![128u8; w * h];
        let mut recon = alloc::vec![128u8; w * h];
        let mut config = PartitionSearchConfig::full();
        config.aligned_w = w;
        config.aligned_h = h;
        let result = partition_search_frame_edges(
            &source, w, &mut recon, w, 64, 100, 1000, 4, &config, 0, 0, None,
        );
        assert_eq!(result.partition_type, expected, "{w}x{h}");
        let Some(PartitionTree::Split {
            width,
            height,
            children,
            ..
        }) = result.tree
        else {
            panic!("edge needs a square parent")
        };
        assert_eq!((width, height, children.len()), (64, 64, 1));
        assert_eq!(recon, source);
    }
}

use alloc::vec;

#[test]
fn partition_search_uniform() {
    let src = vec![128u8; 16 * 16];
    let mut recon = vec![0u8; 16 * 16];
    let result = partition_search(&src, 16, &mut recon, 16, 16, 16, 30, 256, 3);
    assert_eq!(
        result.distortion, 0,
        "uniform block should have zero distortion"
    );
}

#[test]
fn partition_search_gradient() {
    let mut src = vec![0u8; 32 * 32];
    for r in 0..32 {
        for c in 0..32 {
            src[r * 32 + c] = (r * 8 + c * 4) as u8;
        }
    }
    let mut recon = vec![0u8; 32 * 32];
    let result = partition_search(&src, 32, &mut recon, 32, 32, 32, 25, 256, 3);
    assert!(result.num_blocks > 1, "gradient should trigger splitting");
}

#[test]
fn partition_respects_min_size() {
    let src = vec![100u8; 4 * 4];
    let mut recon = vec![0u8; 4 * 4];
    let result = partition_search(&src, 4, &mut recon, 4, 4, 4, 30, 256, 10);
    assert_eq!(result.num_blocks, 1, "4x4 should not split");
}

#[test]
fn partition_search_produces_recon() {
    let src: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    let mut recon = vec![0u8; 16 * 16];
    let result = partition_search(&src, 16, &mut recon, 16, 16, 16, 25, 256, 2);
    // Recon should be populated (not all zeros)
    assert!(recon.iter().any(|&v| v != 0), "recon should be non-zero");
    assert!(result.rd_cost > 0);
}

#[test]
fn partition_search_reads_frame_neighbors() {
    // Frame buffer with a gradient row above the target block: the
    // search must read it as the above neighbors and reconstruct into
    // the same buffer.
    let w = 32usize;
    let h = 48usize;
    let mut frame = vec![128u8; w * h];
    for c in 0..w {
        frame[15 * w + c] = (c * 8) as u8; // row 15 = above the block at y=16
    }
    let mut src = vec![0u8; 16 * 16];
    for r in 0..16 {
        for c in 0..16 {
            src[r * 16 + c] = (c * 8) as u8;
        }
    }
    let result = partition_search_with_config(
        &src,
        16,
        &mut frame,
        w,
        16,
        16,
        30,
        256,
        2,
        &PartitionSearchConfig::full(),
        0,
        16,
        None,
    );
    assert!(result.num_blocks >= 1);
    // The block region must have been reconstructed (roughly matching src).
    let recon_center = frame[(16 + 8) * w + 8];
    assert!(
        (recon_center as i32 - src[8 * 16 + 8] as i32).abs() < 64,
        "recon {} vs src {}",
        recon_center,
        src[8 * 16 + 8]
    );
}

#[test]
fn extract_neighbors_frame_edge() {
    // Both edges unavailable: the C decoder fills above with base-1 = 127
    // and left with base+1 = 129 (libaom reconintra.c), top-left 128.
    let frame = vec![100u8; 64 * 64];
    let nb__ = extract_neighbors(&frame, 64, 0, 0, 8, 8, 64, 64);
    let (above, left, tl, has_above, has_left) = nb__.parts();
    assert!(!has_above);
    assert!(!has_left);
    assert!(above.iter().all(|&v| v == 127), "above fill: {above:?}");
    assert!(left.iter().all(|&v| v == 129), "left fill: {left:?}");
    assert_eq!(tl, 128);
}

#[test]
fn extract_neighbors_single_edge_fill_matches_c() {
    // Above missing + left available: above[] = left_ref[0].
    // Left missing + above available: left[] = above_ref[0].
    let w = 64;
    let mut frame = vec![0u8; w * w];
    for r in 0..w {
        for c in 0..w {
            frame[r * w + c] = if c < 8 { 32 } else { 224 };
        }
    }
    // Block at (8, 0): top frame edge, left column available (value 32).
    let nb__ = extract_neighbors(&frame, w, 8, 0, 8, 8, w, w);
    let (above, left, tl, has_above, has_left) = nb__.parts();
    assert!(!has_above);
    assert!(has_left);
    assert!(
        above.iter().all(|&v| v == 32),
        "above = left_ref[0]: {above:?}"
    );
    assert!(left.iter().all(|&v| v == 32));
    assert_eq!(tl, 32, "top-left = left_ref[0] when only left exists");

    // Block at (0, 8): left frame edge, above row available (value 32).
    let nb__ = extract_neighbors(&frame, w, 0, 8, 8, 8, w, w);
    let (above, left, tl, has_above, has_left) = nb__.parts();
    assert!(has_above);
    assert!(!has_left);
    assert!(above.iter().all(|&v| v == 32));
    assert!(
        left.iter().all(|&v| v == 32),
        "left = above_ref[0]: {left:?}"
    );
    assert_eq!(tl, 32, "top-left = above_ref[0] when only above exists");
}

#[test]
fn extract_neighbors_reads_above_row() {
    let w = 128;
    let h = 128;
    let mut frame = vec![0u8; w * h];
    for r in 0..64 {
        for c in 0..w {
            frame[r * w + c] = ((r + c) % 256) as u8;
        }
    }
    let nb__ = extract_neighbors(&frame, w, 0, 64, 8, 8, w, h);
    let (above, _left, _tl, has_above, has_left) = nb__.parts();
    assert!(has_above);
    assert!(!has_left);
    for i in 0..8 {
        assert_eq!(above[i], ((63 + i) % 256) as u8);
    }
}

/// Task #86: the SAME position as `extract_neighbors_reads_above_row`
/// (abs_y=64, real non-zero data sits in row 63) but with `tile_top =
/// 64` — i.e. row 64 is THIS tile's own top row. `has_above` must be
/// false (AV1 intra prediction never crosses a tile boundary) even
/// though row 63 holds real, readable pixel data in the buffer — a
/// conforming decoder has no such row for this tile and would desync
/// if the encoder predicted from it.
#[test]
fn extract_neighbors_tiled_top_row_has_no_above() {
    let w = 128;
    let h = 128;
    let mut frame = vec![0u8; w * h];
    for r in 0..64 {
        for c in 0..w {
            frame[r * w + c] = ((r + c) % 256) as u8;
        }
    }
    let nb__ = extract_neighbors_tiled(&frame, w, 0, 64, 8, 8, 64, 0, w, h);
    let (above, left, tl, has_above, has_left) = nb__.parts();
    assert!(!has_above, "row 64 IS this tile's own top row");
    assert!(!has_left);
    // Unavailable-above fallback: left_ref[0] if left exists, else 127
    // (left is also unavailable here, abs_x=0) — matches
    // extract_neighbors_frame_edge's plain frame-edge expectation.
    assert!(above.iter().all(|&v| v == 127), "above = {above:?}");
    assert!(
        left.iter().all(|&v| v == 129),
        "left = above_ref[0].unwrap_or(129) when neither is available: {left:?}"
    );
    assert_eq!(tl, 128, "top-left = 128 when neither is available");
}

/// Same tile boundary, but abs_x > 0 so "left" IS available — the
/// unavailable-above fallback must copy left_ref[0], not a flat 127.
#[test]
fn extract_neighbors_tiled_top_row_falls_back_to_left() {
    let w = 128;
    let mut frame = vec![0u8; w * 128];
    // Row 64 (this tile's own top row), starting at col 4: give the
    // "left" column (col 3) a distinct, non-127/128/129 value so the
    // fallback is unambiguous.
    frame[64 * w + 3] = 200;
    let nb__ = extract_neighbors_tiled(&frame, w, 4, 64, 8, 8, 64, 0, w, 128);
    let (above, _left, tl, has_above, has_left) = nb__.parts();
    assert!(!has_above);
    assert!(has_left);
    assert!(
        above.iter().all(|&v| v == 200),
        "above = left_ref[0] when only left exists: {above:?}"
    );
    assert_eq!(tl, 200, "top-left = left_ref[0] when only left exists");
}

/// A block strictly BELOW a tile's top row (not the first row) still
/// sees a real above neighbor from earlier in the SAME tile — only
/// the tile's OWN top row loses availability.
#[test]
fn extract_neighbors_tiled_interior_row_has_above() {
    let w = 128;
    let mut frame = vec![0u8; w * 128];
    for c in 0..w {
        frame[71 * w + c] = 77;
    }
    let nb__ = extract_neighbors_tiled(&frame, w, 0, 72, 8, 8, 64, 0, w, 128);
    let (above, _left, _tl, has_above, _has_left) = nb__.parts();
    assert!(has_above, "row 72 is inside the tile (top row = 64)");
    assert!(above.iter().all(|&v| v == 77));
}

/// Task #96, the COLUMN mirror of
/// `extract_neighbors_tiled_top_row_has_no_above`: a block at a tile's
/// own LEFT column has no "left" neighbour even though `abs_x > 0`.
/// Reading real pixels across a tile-column boundary would desync a
/// conforming decoder, which reconstructs each tile independently.
#[test]
fn extract_neighbors_tiled_left_col_has_no_left() {
    let w = 128;
    let h = 128;
    let mut frame = vec![0u8; w * h];
    // Fill the column immediately left of the tile boundary with a
    // distinctive value; if it leaks in, the assertions below fail.
    for r in 0..h {
        frame[r * w + 63] = 200;
        frame[r * w + 64] = 55;
    }
    // Block at (64, 0) where the tile's left column IS 64.
    let nb__ = extract_neighbors_tiled(&frame, w, 64, 0, 8, 8, 0, 64, w, h);
    let (_above, left, _tl, _has_above, has_left) = nb__.parts();
    assert!(!has_left, "col 64 IS this tile's own left column");
    assert!(
        left.iter().all(|&v| v != 200),
        "the neighbouring tile's column must not leak in: {left:?}"
    );

    // One SB further right, INSIDE the same tile: left is available
    // again and reads the real recon.
    let nb__ = extract_neighbors_tiled(&frame, w, 72, 0, 8, 8, 0, 64, w, h);
    let (_a2, left2, _tl2, _ha2, has_left2) = nb__.parts();
    assert!(has_left2, "col 72 is interior to the tile (left col = 64)");
    assert!(left2.iter().all(|&v| v == 0), "left2 = {left2:?}");

    // tile_left = 0 (single tile column) is the pre-#96 behaviour:
    // abs_x > 0 alone decides, so col 64 DOES see its left neighbour.
    let nb__ = extract_neighbors_tiled(&frame, w, 64, 0, 8, 8, 0, 0, w, h);
    let (_a3, left3, _tl3, _ha3, has_left3) = nb__.parts();
    assert!(has_left3);
    assert!(left3.iter().all(|&v| v == 200));
}

#[test]
fn extract_neighbors_reads_left_column() {
    let w = 128;
    let h = 64;
    let mut frame = vec![0u8; w * h];
    for r in 0..h {
        for c in 0..64 {
            frame[r * w + c] = ((r * 2 + c) % 256) as u8;
        }
    }
    let nb__ = extract_neighbors(&frame, w, 64, 0, 8, 8, w, h);
    let (_above, left, _tl, _has_above, has_left) = nb__.parts();
    assert!(has_left);
    for i in 0..8 {
        assert_eq!(left[i], ((i * 2 + 63) % 256) as u8);
    }
}

/// Issue #15: a block straddling the ALIGNED frame edge must stop
/// reading real reference samples AT that edge and replicate the last
/// one, exactly like C's `n_left_px` / `n_top_px`
/// (`build_intra_predictors`, via `svt_av1_predict_intra_block`'s
/// `xr`/`yd`, enc_intra_prediction.c:489-492 + :540-543) and spec
/// 7.11.2's `Min(maxY, ...)` clamp.
///
/// The buffer deliberately HOLDS distinct data past the extent — that is
/// the real situation on a partial superblock, where the recon working
/// buffers keep the aligned stride but are sized to the SB-extent product
/// and a straddling block has already written its own recon there. Before
/// the fix those rows/columns were read, so the encoder predicted from
/// pixels no conforming decoder produces.
#[test]
fn extract_neighbors_clamps_to_aligned_extent() {
    // 96x88 aligned inside a 96x128 SB-extent buffer — the #15 shape.
    let (w, ext_h, aligned_h) = (96usize, 128usize, 88usize);
    let mut frame = vec![0u8; w * ext_h];
    for r in 0..ext_h {
        // Left column (x = 63) of the block at (64, 64): 10 in-frame,
        // 99 past the aligned bottom.
        frame[r * w + 63] = if r < aligned_h { 10 } else { 99 };
    }
    let nb__ = extract_neighbors_tiled(&frame, w, 64, 64, 32, 32, 0, 0, w, aligned_h);
    let (_above, left, _tl, _ha, has_left) = nb__.parts();
    assert!(has_left);
    assert_eq!(left.len(), 32);
    assert!(
        left.iter().all(|&v| v == 10),
        "rows past aligned_h must replicate the last in-frame sample, not              read the buffer: {left:?}"
    );
    // Anti-vacuity: the same call WITHOUT the clamp does read the 99s, so
    // this test discriminates the fix rather than passing either way.
    let nb__ = extract_neighbors_tiled(&frame, w, 64, 64, 32, 32, 0, 0, w, ext_h);
    let (_a2, left2, _tl2, _ha2, _hl2) = nb__.parts();
    assert!(
        left2.contains(&99),
        "unclamped extent must expose the out-of-frame data this test guards"
    );

    // Column mirror: 188x256 true -> 192x256 aligned, block at x = 160.
    let (aw, h) = (192usize, 64usize);
    let mut f2 = vec![0u8; aw * h];
    for c in 0..aw {
        f2[15 * aw + c] = if c < 188 { 20 } else { 88 };
    }
    let nb__ = extract_neighbors_tiled(&f2, aw, 160, 16, 32, 32, 0, 0, 188, h);
    let (above, _l3, _tl3, has_above, _hl3) = nb__.parts();
    assert!(has_above);
    assert!(
        above.iter().all(|&v| v == 20),
        "columns past the extent must replicate: {above:?}"
    );
}

#[test]
fn extract_neighbors_in_sb_positions_are_live() {
    // Neighbors INSIDE the current superblock must be read from the
    // buffer (the historical bug returned 128 for them).
    let w = 64;
    let mut frame = vec![0u8; w * w];
    for c in 0..w {
        frame[7 * w + c] = 200; // row 7 — above a block at y=8 inside the SB
    }
    let nb__ = extract_neighbors(&frame, w, 8, 8, 8, 8, w, w);
    let (above, _left, _tl, has_above, _has_left) = nb__.parts();
    assert!(has_above);
    assert!(
        above.iter().all(|&v| v == 200),
        "in-SB above must be live: {above:?}"
    );
}

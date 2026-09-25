use super::*;

/// Extract prediction neighbors for a block at absolute position
/// (abs_x, abs_y) directly from the reconstruction buffer.
///
/// The buffer is the live frame (or SB) reconstruction written in coding
/// order, so above/left pixels — including those inside the current
/// superblock — are always current, exactly as the decoder sees them.
///
/// Unavailable edges are filled with the C decoder's rules
/// (libaom reconintra.c build_intra_predictors):
/// - above row missing: fill with left_ref[0] if the left column exists,
///   else 127
/// - left column missing: fill with above_ref[0] if the above row exists,
///   else 129
/// - top-left: above_ref[-1] if both exist; above_ref[0] if only above;
///   left_ref[0] if only left; 128 if neither
/// - samples past the reconstructed area extend the last available sample
///
/// Filling with anything else (previously a flat 128) makes the encoder
/// predict from pixels the decoder never sees: an edge V_PRED block coded
/// against pred=128 decodes against pred=left_ref[0], corrupting the
/// reconstruction by the difference. `tile_top` extends this same rule to
/// tile-row boundaries (see [`extract_neighbors`] for the untiled form).
pub(crate) fn extract_neighbors_tiled(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    tile_top: usize,
    tile_left: usize,
    plane_w: usize,
    plane_h: usize,
) -> NeighborEdges {
    assert!(
        width <= MAX_EDGE_PX && height <= MAX_EDGE_PX,
        "intra reference edge {width}x{height} exceeds MAX_SB_SIZE {MAX_EDGE_PX}"
    );
    // Task #86: `tile_top` is this plane's tile-row origin (0 = single
    // tile row / unchanged pre-#86 behavior). AV1 intra prediction never
    // crosses a tile boundary — a block at a TILE's own top row has no
    // "above" neighbor even when it is NOT the frame's top row (the
    // frame-absolute `abs_y > 0` this used to read was correct only
    // because every block used to be in the one tile spanning the whole
    // frame). Reading real pixel data across a tile-row boundary would
    // desync a conforming decoder — it reconstructs each tile
    // independently and has no such pixels to read either.
    let has_above = abs_y > tile_top;
    // Task #96: same rule on the column axis — a block at a TILE's own
    // left column has no "left" neighbour even when it is not the frame's
    // left column. `tile_left` is 0 for a single-column frame, so this is
    // byte-identical to the previous `abs_x > 0` there.
    let has_left = abs_x > tile_left;

    // C `n_top_px` / `n_left_px` (`build_intra_predictors`, via
    // `svt_av1_predict_intra_block`'s `xr`/`yd`, enc_intra_prediction.c:489-492
    // + :540-543): the count of REAL reference samples is capped by the ALIGNED
    // frame extent, and everything past it replicates the last real sample.
    // Spec 7.11.2 states the same rule as a coordinate clamp
    // (`CurrFrame[plane][Min(maxY, ...)][...]`, maxY = MiRows*MI_SIZE-1).
    //
    // `plane_w`/`plane_h` are that extent in THIS plane's pixel domain. They
    // are NOT the recon buffer's shape: on a partial superblock the working
    // buffers keep the aligned stride but are sized to the SB-extent product,
    // so a block straddling the bottom would otherwise read the encoder's own
    // recon of rows a conforming decoder replicates instead -- an encoder/
    // decoder prediction mismatch, not merely a byte-identity gap. Identical
    // to `width`/`height` whenever the block is fully inside the extent, i.e.
    // always on a 64-aligned frame.
    let n_top_px = if has_above {
        width.min(plane_w.saturating_sub(abs_x))
    } else {
        0
    };
    let n_left_px = if has_left {
        height.min(plane_h.saturating_sub(abs_y))
    } else {
        0
    };
    // C selects the DC/edge-fill variants on `n_*_px > 0`, not on the raw
    // availability flags, and they differ for a TX unit whose origin is
    // already past the extent (reachable inside a straddling leaf).
    let has_above = n_top_px > 0;
    let has_left = n_left_px > 0;

    // C left_ref[0] / above_ref[0]: the first sample of each neighbor edge.
    let left_ref0 = if has_left {
        recon.get(abs_y * stride + abs_x - 1).copied()
    } else {
        None
    };
    let above_ref0 = if has_above {
        recon.get((abs_y - 1) * stride + abs_x).copied()
    } else {
        None
    };

    let mut above = [0u8; MAX_EDGE_PX];
    if has_above {
        let row = abs_y - 1;
        // Every availability bound is monotone in `i` (`i < n_top_px`,
        // `x < stride`, `idx < recon.len()` — each stays false once false),
        // so the real samples form a single contiguous prefix: bulk-copy it
        // and fill the tail with its last value, instead of three bounds
        // checks per pixel.
        let base = row * stride + abs_x;
        let n_real = n_top_px
            .min(stride.saturating_sub(abs_x))
            .min(recon.len().saturating_sub(base));
        above[..n_real].copy_from_slice(&recon[base..base + n_real]);
        let last = if n_real > 0 {
            recon[base + n_real - 1]
        } else {
            above_ref0.unwrap_or(127)
        };
        above[n_real..width].fill(last);
    } else {
        above[..width].fill(left_ref0.unwrap_or(127));
    }

    let mut left = [0u8; MAX_EDGE_PX];
    if has_left {
        let col = abs_x - 1;
        // Same monotone-prefix shape down the column: `i < n_left_px` and
        // `(abs_y + i) * stride + col < recon.len()` both stay false once
        // false, so the real run is `n_real` strided loads then a tail fill.
        let base = abs_y * stride + col;
        let n_real = n_left_px.min(if base < recon.len() {
            (recon.len() - 1 - base) / stride + 1
        } else {
            0
        });
        let mut last = left_ref0.unwrap_or(129);
        for (dst, idx) in left[..n_real]
            .iter_mut()
            .zip((0..n_real).map(|i| base + i * stride))
        {
            last = recon[idx];
            *dst = last;
        }
        left[n_real..height].fill(last);
    } else {
        left[..height].fill(above_ref0.unwrap_or(129));
    }

    let top_left = if has_above && has_left {
        recon
            .get((abs_y - 1) * stride + abs_x - 1)
            .copied()
            .unwrap_or(128)
    } else if has_above {
        above_ref0.unwrap_or(128)
    } else if has_left {
        left_ref0.unwrap_or(128)
    } else {
        128
    };

    NeighborEdges {
        above,
        left,
        top_left,
        has_above,
        has_left,
        w: width,
        h: height,
    }
}

/// High-bit-depth (u16) mirror of [`extract_neighbors_tiled`] for the bd10
/// u16 MD path (task #94). Identical neighbour-availability + edge-extend
/// rules; the only bit-depth dependence is the C `build_intra_predictors_high`
/// fallback fills — `base = 128 << (bd - 8)` (512 at bd10), so a missing above
/// row with no left is `base - 1` (511) and a missing left column with no
/// above is `base + 1` (513), top-left-neither is `base` (512). At bd == 8
/// this reduces to the exact 127/129/128 the u8 path uses (verified in tests),
/// so the u8 path is untouched.
///
/// ISSUE #18: `tile_top`/`tile_left` are NOT optional decoration. This used to
/// derive availability from the FRAME (`abs_y > 0` / `abs_x > 0`) with a doc
/// note claiming "`tile_top == 0` (single tile row) matches the funnel's
/// current scope" — but the funnel's scope is not the caller's choice. AV1
/// FORCES a multi-tile grid once the frame exceeds `MAX_TILE_AREA`
/// (4096*2304 px) or `MAX_TILE_WIDTH` (4096 px), so an AVIF encode that never
/// asked for a tile still got two, and the bd10 predictor then read real
/// pixels across the tile edge while a conforming decoder — which
/// reconstructs each tile independently — used the unavailable-edge fills.
/// Everything from the boundary onward drifted. Witness:
/// `svtav1/tests/issue18_repro.rs`.
pub(crate) fn extract_neighbors_hbd(
    recon: &[u16],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    bd: u8,
    tile_top: usize,
    tile_left: usize,
    plane_w: usize,
    plane_h: usize,
) -> (alloc::vec::Vec<u16>, alloc::vec::Vec<u16>, u16, bool, bool) {
    let base: u16 = 128u16 << (bd - 8);
    // Same rule as the u8 twin (`extract_neighbors_tiled`): a block on a
    // TILE's own top row / left column has no above / left neighbour even
    // when it is not the FRAME's. `tile_top`/`tile_left` are 0 for a
    // single-tile encode, where this is bit-for-bit the previous behaviour.
    let has_above = abs_y > tile_top;
    let has_left = abs_x > tile_left;
    // C `n_top_px` / `n_left_px` — see the u8 twin's comment. Same rule, same
    // reason: on a partial superblock the recon buffer holds data past the
    // ALIGNED extent that a conforming decoder replicates instead.
    let n_top_px = if has_above {
        width.min(plane_w.saturating_sub(abs_x))
    } else {
        0
    };
    let n_left_px = if has_left {
        height.min(plane_h.saturating_sub(abs_y))
    } else {
        0
    };
    let has_above = n_top_px > 0;
    let has_left = n_left_px > 0;

    let left_ref0 = if has_left {
        recon.get(abs_y * stride + abs_x - 1).copied()
    } else {
        None
    };
    let above_ref0 = if has_above {
        recon.get((abs_y - 1) * stride + abs_x).copied()
    } else {
        None
    };

    // Monotone-prefix copies, same shape as the u8 twin: every availability
    // bound stays false once false, so the real run is one bulk copy (above)
    // or one strided gather (left) plus a tail fill.
    let above: alloc::vec::Vec<u16> = if has_above {
        let row = abs_y - 1;
        let b = row * stride + abs_x;
        let n_real = n_top_px
            .min(stride.saturating_sub(abs_x))
            .min(recon.len().saturating_sub(b));
        let mut v = alloc::vec::Vec::with_capacity(width);
        v.extend_from_slice(&recon[b..b + n_real]);
        let last = if n_real > 0 {
            recon[b + n_real - 1]
        } else {
            above_ref0.unwrap_or(base - 1)
        };
        v.resize(width, last);
        v
    } else {
        alloc::vec![left_ref0.unwrap_or(base - 1); width]
    };

    let left: alloc::vec::Vec<u16> = if has_left {
        let col = abs_x - 1;
        let b = abs_y * stride + col;
        let n_real = n_left_px.min(if b < recon.len() {
            (recon.len() - 1 - b) / stride + 1
        } else {
            0
        });
        let mut v = alloc::vec::Vec::with_capacity(height);
        v.extend((0..n_real).map(|i| recon[b + i * stride]));
        let last = if n_real > 0 {
            v[n_real - 1]
        } else {
            left_ref0.unwrap_or(base + 1)
        };
        v.resize(height, last);
        v
    } else {
        alloc::vec![above_ref0.unwrap_or(base + 1); height]
    };

    let top_left = if has_above && has_left {
        recon
            .get((abs_y - 1) * stride + abs_x - 1)
            .copied()
            .unwrap_or(base)
    } else if has_above {
        above_ref0.unwrap_or(base)
    } else if has_left {
        left_ref0.unwrap_or(base)
    } else {
        base
    };

    (above, left, top_left, has_above, has_left)
}

/// Save a rectangular region of the reconstruction buffer.
pub(super) fn save_region(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec![0u8; width * height];
    for r in 0..height {
        let src = (abs_y + r) * stride + abs_x;
        out[r * width..r * width + width].copy_from_slice(&recon[src..src + width]);
    }
    out
}

/// Restore a rectangular region of the reconstruction buffer.
pub(super) fn restore_region(
    recon: &mut [u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    saved: &[u8],
) {
    for r in 0..height {
        let dst = (abs_y + r) * stride + abs_x;
        recon[dst..dst + width].copy_from_slice(&saved[r * width..r * width + width]);
    }
}

/// Recursive partition tree for spec-conformant bitstream encoding.
///
/// AV1 requires encoding partition syntax in recursive tree order:
/// write the partition type at each node, then recurse into children.
/// This tree captures the full partition structure for an SB.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // boxing `Leaf(BlockDecision)` would put an allocation
// on every leaf of the partition search; unmeasured perf change, and this tree is hot
pub enum PartitionTree {
    /// Leaf node: PARTITION_NONE — encode block directly.
    Leaf(BlockDecision),
    /// Internal node: partition type + child sub-trees.
    Split {
        partition_type: PartitionType,
        width: u16,
        height: u16,
        children: alloc::vec::Vec<PartitionTree>,
    },
}

use super::*;

/// Uses default config (all features enabled). No frame context (mid-gray neighbors).
pub fn partition_search(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    lambda: u64,
    max_depth: u32,
) -> PartitionResult {
    partition_search_with_config(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        lambda,
        max_depth,
        &PartitionSearchConfig::full(),
        0,
        0,
        None,
    )
}

/// Search a square coding unit against the frame's aligned extent. Complete
/// squares retain the existing search. At a partial square, compare SPLIT
/// against the legal single-edge rectangle when that rectangle fits. Nodes
/// with both lower halves absent must split without coding a partition bit.
/// Only complete prediction/transform blocks enter the leaf search; geometry
/// splits continue past the configured search depth when the frame requires it.
#[allow(clippy::too_many_arguments)]
pub fn partition_search_frame_edges(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    size: usize,
    qindex: u8,
    lambda: u64,
    max_depth: u32,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    ref_ctx: Option<&RefFrameCtx>,
) -> PartitionResult {
    let visible_w = size.min(config.aligned_w.saturating_sub(abs_x));
    let visible_h = size.min(config.aligned_h.saturating_sub(abs_y));
    assert!(visible_w > 0 && visible_h > 0 && size.is_power_of_two());
    if visible_w == size && visible_h == size {
        return partition_search_with_config(
            src,
            src_stride,
            recon,
            recon_stride,
            size,
            size,
            qindex,
            lambda,
            max_depth,
            config,
            abs_x,
            abs_y,
            ref_ctx,
        );
    }
    // Pipeline extents are multiples of eight, so an intersecting 8x8 square
    // is always complete. Do not invent smaller non-power-of-two transforms.
    assert!(size > 8, "partial monochrome node below aligned frame grid");
    let half = size / 2;
    let has_rows = visible_h > half;
    let has_cols = visible_w > half;
    let rates = if has_rows && has_cols {
        [0, partition_rate_256(size, PartitionType::Split)]
    } else if !has_rows && !has_cols {
        [0, 0] // AV1 5.11.4: forced SPLIT, no partition symbol
    } else {
        crate::entropy::context::partition_alike_symbol_costs(size, !has_rows).map(|r| (r + 1) >> 1)
    };
    let baseline = save_region(recon, recon_stride, abs_x, abs_y, visible_w, visible_h);
    let mut best = PartitionResult {
        partition_type: PartitionType::Split,
        rd_cost: 0,
        distortion: 0,
        rate: rates[1],
        num_blocks: 0,
        decisions: alloc::vec::Vec::new(),
        tree: None,
    };
    let mut children = alloc::vec::Vec::new();
    for quadrant in 0..4 {
        let x = (quadrant & 1) * half;
        let y = (quadrant >> 1) * half;
        if x >= visible_w || y >= visible_h {
            continue;
        }
        let child = partition_search_frame_edges(
            &src[y * src_stride + x..],
            src_stride,
            recon,
            recon_stride,
            half,
            qindex,
            lambda,
            max_depth.saturating_sub(1),
            config,
            abs_x + x,
            abs_y + y,
            ref_ctx,
        );
        best.distortion += child.distortion;
        best.rate += child.rate;
        best.num_blocks += child.num_blocks;
        best.decisions.extend(child.decisions);
        if let Some(tree) = child.tree {
            children.push(tree);
        }
    }
    best.tree = Some(PartitionTree::Split {
        partition_type: PartitionType::Split,
        width: size as u16,
        height: size as u16,
        children,
    });
    best.rd_cost = best.distortion + ((lambda * u64::from(best.rate)) >> 8);
    let rectangle = if !has_rows && has_cols && visible_w == size && visible_h == half {
        Some((
            size,
            half,
            PartitionType::Horz,
            svtav1_types::partition::PartitionType::Horz,
        ))
    } else if has_rows && !has_cols && visible_w == half && visible_h == size {
        Some((
            half,
            size,
            PartitionType::Vert,
            svtav1_types::partition::PartitionType::Vert,
        ))
    } else {
        None
    };
    if let Some((bw, bh, kind, leaf_kind)) = rectangle {
        let split_recon = save_region(recon, recon_stride, abs_x, abs_y, visible_w, visible_h);
        restore_region(
            recon,
            recon_stride,
            abs_x,
            abs_y,
            visible_w,
            visible_h,
            &baseline,
        );
        let mut rect = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            bw,
            bh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            leaf_kind,
            false,
        );
        rect.partition_type = kind;
        rect.rate += rates[0];
        rect.rd_cost = rect.distortion + ((lambda * u64::from(rect.rate)) >> 8);
        rect.tree = Some(PartitionTree::Split {
            partition_type: kind,
            width: size as u16,
            height: size as u16,
            children: rect.tree.into_iter().collect(),
        });
        if rect.rd_cost < best.rd_cost {
            best = rect;
        } else {
            restore_region(
                recon,
                recon_stride,
                abs_x,
                abs_y,
                visible_w,
                visible_h,
                &split_recon,
            );
        }
    }
    best
}

/// Encode a superblock with recursive partition search using explicit config.
///
/// Tries PARTITION_NONE at the current size, then optionally tries HORZ, VERT,
/// extended partitions, 4:1 partitions, and SPLIT, picking lowest RD cost.
/// Config gates which partition types and intra modes are evaluated.
///
/// `recon` is the full frame (or standalone block) reconstruction buffer with
/// `recon_stride`; the block lives at (abs_x, abs_y). Predictions read
/// above/left neighbors directly from this buffer — including neighbors
/// inside the current superblock — exactly as the decoder reconstructs them.
/// When `ref_ctx` is provided, inter prediction is also tried using ME.
pub fn partition_search_with_config(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qindex: u8,
    lambda: u64,
    max_depth: u32,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    ref_ctx: Option<&RefFrameCtx>,
) -> PartitionResult {
    // Base case: minimum size or max depth reached
    if width <= MIN_BLOCK_SIZE || height <= MIN_BLOCK_SIZE || max_depth == 0 {
        let mut leaf = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::None,
            false,
        );
        add_none_node_cost(&mut leaf, width, height, lambda);
        return leaf;
    }

    // Try PARTITION_NONE: encode at current size
    let mut none_result = encode_with_neighbors(
        src,
        src_stride,
        recon,
        recon_stride,
        width,
        height,
        qindex,
        config,
        abs_x,
        abs_y,
        ref_ctx,
        svtav1_types::partition::PartitionType::None,
        false,
    );
    // Every square node the tile writer visits codes a partition symbol:
    // price PARTITION_NONE with its real entropy cost and rescore with
    // THIS search's lambda — the leaf rd formula uses a fixed scale, and
    // comparing that against the lambda-scaled candidates below is what
    // used to misprice NONE (docs/IDENTITY-STATUS.md, op-0 divergence).
    add_none_node_cost(&mut none_result, width, height, lambda);

    // If block is small enough, don't bother splitting further
    if width <= 8 && height <= 8 {
        return none_result;
    }

    let mut best_result = none_result;
    // Snapshot of the winning candidate's reconstruction for this region
    // (PARTITION_NONE was just encoded into the buffer).
    let mut best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);

    // Try PARTITION_HORZ: two halves stacked vertically.
    // Children are height/2 tall — gate keeps them >= min_block_dim
    // (identical to the historical `height >= 8` at min_block_dim = 4).
    if height >= 2 * config.min_block_dim {
        let hh = height / 2;
        let mut horz_result = PartitionResult {
            partition_type: PartitionType::Horz,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Horz),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        // Top half
        let top = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Horz,
            false,
        );
        horz_result.distortion += top.distortion;
        horz_result.rate += top.rate;
        horz_result.num_blocks += top.num_blocks;
        horz_result.decisions.extend(top.decisions);

        // Bottom half — neighbors come straight from the live buffer.
        let bot = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            width,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::Horz,
            false,
        );
        horz_result.distortion += bot.distortion;
        horz_result.rate += bot.rate;
        horz_result.num_blocks += bot.num_blocks;
        horz_result.decisions.extend(bot.decisions);
        let mut horz_children = alloc::vec::Vec::new();
        if let Some(t) = top.tree {
            horz_children.push(t);
        }
        if let Some(t) = bot.tree {
            horz_children.push(t);
        }
        horz_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Horz,
            width: width as u16,
            height: height as u16,
            children: horz_children,
        });
        horz_result.rd_cost = horz_result.distortion + ((lambda * horz_result.rate as u64) >> 8);

        if horz_result.rd_cost < best_result.rd_cost {
            best_result = horz_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT: two halves side by side
    if width >= 2 * config.min_block_dim {
        let hw = width / 2;
        let mut vert_result = PartitionResult {
            partition_type: PartitionType::Vert,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Vert),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        // Left half
        let left = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Vert,
            false,
        );
        vert_result.distortion += left.distortion;
        vert_result.rate += left.rate;
        vert_result.num_blocks += left.num_blocks;
        vert_result.decisions.extend(left.decisions);

        // Right half — neighbors come straight from the live buffer.
        let right = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::Vert,
            false,
        );
        vert_result.distortion += right.distortion;
        vert_result.rate += right.rate;
        vert_result.num_blocks += right.num_blocks;
        vert_result.decisions.extend(right.decisions);
        let mut vert_children = alloc::vec::Vec::new();
        if let Some(t) = left.tree {
            vert_children.push(t);
        }
        if let Some(t) = right.tree {
            vert_children.push(t);
        }
        vert_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Vert,
            width: width as u16,
            height: height as u16,
            children: vert_children,
        });
        vert_result.rd_cost = vert_result.distortion + ((lambda * vert_result.rate as u64) >> 8);

        if vert_result.rd_cost < best_result.rd_cost {
            best_result = vert_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_4: four horizontal strips (each height/4)
    // Gated by config.enable_4to1_partitions (Spec 10: "4:1 partitions at preset <= 6")
    if height >= 4 * config.min_block_dim && config.enable_4to1_partitions {
        let qh = height / 4;
        let mut h4_result = PartitionResult {
            partition_type: PartitionType::Horz4,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Horz4),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut h4_children = alloc::vec::Vec::new();
        for strip in 0..4 {
            let y0 = strip * qh;
            let cur_h = qh.min(height - y0);
            let sub = encode_with_neighbors(
                &src[y0 * src_stride..],
                src_stride,
                recon,
                recon_stride,
                width,
                cur_h,
                qindex,
                config,
                abs_x,
                abs_y + y0,
                ref_ctx,
                svtav1_types::partition::PartitionType::Horz4,
                false,
            );
            h4_result.distortion += sub.distortion;
            h4_result.rate += sub.rate;
            h4_result.num_blocks += sub.num_blocks;
            h4_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                h4_children.push(t);
            }
        }
        h4_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Horz4,
            width: width as u16,
            height: height as u16,
            children: h4_children,
        });
        h4_result.rd_cost = h4_result.distortion + ((lambda * h4_result.rate as u64) >> 8);
        if h4_result.rd_cost < best_result.rd_cost {
            best_result = h4_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_4: four vertical strips (each width/4)
    if width >= 4 * config.min_block_dim && config.enable_4to1_partitions {
        let qw = width / 4;
        let mut v4_result = PartitionResult {
            partition_type: PartitionType::Vert4,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Vert4),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut v4_children = alloc::vec::Vec::new();
        for strip in 0..4 {
            let x0 = strip * qw;
            let cur_w = qw.min(width - x0);
            let sub = encode_with_neighbors(
                &src[x0..],
                src_stride,
                recon,
                recon_stride,
                cur_w,
                height,
                qindex,
                config,
                abs_x + x0,
                abs_y,
                ref_ctx,
                svtav1_types::partition::PartitionType::Vert4,
                false,
            );
            v4_result.distortion += sub.distortion;
            v4_result.rate += sub.rate;
            v4_result.num_blocks += sub.num_blocks;
            v4_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                v4_children.push(t);
            }
        }
        v4_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Vert4,
            width: width as u16,
            height: height as u16,
            children: v4_children,
        });
        v4_result.rd_cost = v4_result.distortion + ((lambda * v4_result.rate as u64) >> 8);
        if v4_result.rd_cost < best_result.rd_cost {
            best_result = v4_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_A: top split into 2 quarters + bottom half
    // Gated by config.enable_ext_partitions (Spec 10: "extended partitions at preset <= 8")
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut ha_result = PartitionResult {
            partition_type: PartitionType::HorzA,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::HorzA),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut ha_children = alloc::vec::Vec::new();
        // Top-left quarter
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        // Top-right quarter
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            hh,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        // Bottom half
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            width,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzA,
            false,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            ha_children.push(t);
        }
        ha_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::HorzA,
            width: width as u16,
            height: height as u16,
            children: ha_children,
        });
        ha_result.rd_cost = ha_result.distortion + ((lambda * ha_result.rate as u64) >> 8);
        if ha_result.rd_cost < best_result.rd_cost {
            best_result = ha_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_HORZ_B: top half + bottom split into 2 quarters
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut hb_result = PartitionResult {
            partition_type: PartitionType::HorzB,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::HorzB),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut hb_children = alloc::vec::Vec::new();
        // Top half
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            width,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        // Bottom-left quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            hw,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        // Bottom-right quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride + hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height - hh,
            qindex,
            config,
            abs_x + hw,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::HorzB,
            false,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            hb_children.push(t);
        }
        hb_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::HorzB,
            width: width as u16,
            height: height as u16,
            children: hb_children,
        });
        hb_result.rd_cost = hb_result.distortion + ((lambda * hb_result.rate as u64) >> 8);
        if hb_result.rd_cost < best_result.rd_cost {
            best_result = hb_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_A: left split into 2 quarters + right half
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut va_result = PartitionResult {
            partition_type: PartitionType::VertA,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::VertA),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut va_children = alloc::vec::Vec::new();
        // Top-left quarter
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            hh,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        // Bottom-left quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride..],
            src_stride,
            recon,
            recon_stride,
            hw,
            height - hh,
            qindex,
            config,
            abs_x,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        // Right half
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertA,
            false,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            va_children.push(t);
        }
        va_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::VertA,
            width: width as u16,
            height: height as u16,
            children: va_children,
        });
        va_result.rd_cost = va_result.distortion + ((lambda * va_result.rate as u64) >> 8);
        if va_result.rd_cost < best_result.rd_cost {
            best_result = va_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_VERT_B: left half + right split into 2 quarters
    if width >= 2 * config.min_block_dim
        && height >= 2 * config.min_block_dim
        && config.enable_ext_partitions
    {
        let hw = width / 2;
        let hh = height / 2;
        let mut vb_result = PartitionResult {
            partition_type: PartitionType::VertB,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::VertB),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };
        let mut vb_children = alloc::vec::Vec::new();
        // Left half
        let s = encode_with_neighbors(
            src,
            src_stride,
            recon,
            recon_stride,
            hw,
            height,
            qindex,
            config,
            abs_x,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        // Top-right quarter
        let s = encode_with_neighbors(
            &src[hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            hh,
            qindex,
            config,
            abs_x + hw,
            abs_y,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        // Bottom-right quarter
        let s = encode_with_neighbors(
            &src[hh * src_stride + hw..],
            src_stride,
            recon,
            recon_stride,
            width - hw,
            height - hh,
            qindex,
            config,
            abs_x + hw,
            abs_y + hh,
            ref_ctx,
            svtav1_types::partition::PartitionType::VertB,
            false,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.decisions.extend(s.decisions);
        if let Some(t) = s.tree {
            vb_children.push(t);
        }
        vb_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::VertB,
            width: width as u16,
            height: height as u16,
            children: vb_children,
        });
        vb_result.rd_cost = vb_result.distortion + ((lambda * vb_result.rate as u64) >> 8);
        if vb_result.rd_cost < best_result.rd_cost {
            best_result = vb_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Try PARTITION_SPLIT: encode 4 sub-blocks.
    // With the default min_block_dim (4) SPLIT is tried unconditionally,
    // exactly as before; with the 4:2:0 min-8x8 policy it is gated so
    // quadrants never drop below min_block_dim. (The `width <= 8 &&
    // height <= 8` early-return above already prevents SPLIT below 16 for
    // the square blocks the recursion produces — this gate makes the
    // policy explicit for any caller-supplied shape.)
    let allow_split = config.min_block_dim <= MIN_BLOCK_SIZE
        || (width / 2 >= config.min_block_dim && height / 2 >= config.min_block_dim);
    if allow_split {
        let hw = width / 2;
        let hh = height / 2;
        let mut split_result = PartitionResult {
            partition_type: PartitionType::Split,
            rd_cost: 0,
            distortion: 0,
            rate: partition_rate_256(width, PartitionType::Split),
            num_blocks: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
        };

        // Encode 4 quadrants, collect child trees
        let mut split_children = alloc::vec::Vec::new();
        for (qr, qc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let x0 = qc * hw;
            let y0 = qr * hh;
            let cur_w = hw.min(width - x0);
            let cur_h = hh.min(height - y0);

            let sub_src_offset = y0 * src_stride + x0;

            let sub = partition_search_with_config(
                &src[sub_src_offset..],
                src_stride,
                recon,
                recon_stride,
                cur_w,
                cur_h,
                qindex,
                lambda,
                max_depth - 1,
                config,
                abs_x + x0,
                abs_y + y0,
                ref_ctx,
            );

            split_result.distortion += sub.distortion;
            split_result.rate += sub.rate;
            split_result.num_blocks += sub.num_blocks;
            split_result.decisions.extend(sub.decisions);
            if let Some(t) = sub.tree {
                split_children.push(t);
            }
        }
        split_result.tree = Some(PartitionTree::Split {
            partition_type: PartitionType::Split,
            width: width as u16,
            height: height as u16,
            children: split_children,
        });
        split_result.rd_cost = split_result.distortion + ((lambda * split_result.rate as u64) >> 8);

        // Check if SPLIT is better than current best
        if split_result.rd_cost < best_result.rd_cost {
            best_result = split_result;
            best_snap = save_region(recon, recon_stride, abs_x, abs_y, width, height);
        }
    }

    // Leave the winning candidate's reconstruction in the buffer.
    restore_region(recon, recon_stride, abs_x, abs_y, width, height, &best_snap);
    best_result
}

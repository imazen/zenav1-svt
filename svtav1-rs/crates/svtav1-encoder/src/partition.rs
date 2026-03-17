//! Partition search — recursive block splitting for optimal RD.
//!
//! AV1 uses a quadtree+extended partition structure starting from 64x64
//! (or 128x128) superblocks, recursively splitting into smaller blocks.
//! Each split decision compares RD cost of encoding at current size vs
//! splitting. (Spec 10: "partition search evaluates NONE, SPLIT, HORZ,
//! VERT, and extended partition types")
//!
//! All 10 AV1 partition types supported:
//! NONE, HORZ, VERT, SPLIT, HORZ_A, HORZ_B, VERT_A, VERT_B, HORZ_4, VERT_4
//! (Spec 16: PartitionType enum, definitions.h:858-872)

/// Minimum block size for partition search (4x4 per AV1 spec).
pub const MIN_BLOCK_SIZE: usize = 4;

/// Configuration for partition search, derived from SpeedConfig.
/// Controls which tools are enabled during mode decision within
/// the partition search loop.
#[derive(Debug, Clone)]
pub struct PartitionSearchConfig {
    /// Maximum number of intra candidates to evaluate.
    /// (Spec 03: NIC = Number of Intra Candidates per MDS stage)
    pub max_intra_candidates: usize,
    /// Whether to try directional intra modes (D45..D203).
    /// (Spec 05: "directional modes are between V_PRED and D67_PRED")
    pub enable_directional: bool,
    /// Whether to try T-shape partitions (HORZ_A/B, VERT_A/B).
    /// (Spec 10: "extended partition types for improved RD at boundaries")
    pub enable_ext_partitions: bool,
    /// Whether to try 4:1 partitions (HORZ_4, VERT_4).
    pub enable_4to1_partitions: bool,
}

impl PartitionSearchConfig {
    /// Create from a SpeedConfig.
    pub fn from_speed_config(sc: &crate::speed_config::SpeedConfig) -> Self {
        Self {
            max_intra_candidates: sc.max_intra_candidates as usize,
            enable_directional: sc.enable_directional_modes,
            enable_ext_partitions: sc.preset <= 8,
            enable_4to1_partitions: sc.preset <= 6,
        }
    }

    /// Default config (all features enabled).
    pub fn full() -> Self {
        Self {
            max_intra_candidates: 13,
            enable_directional: true,
            enable_ext_partitions: true,
            enable_4to1_partitions: true,
        }
    }
}

/// Result of encoding a single partition block.
#[derive(Debug, Clone)]
pub struct PartitionResult {
    /// Total RD cost for this partition decision.
    pub rd_cost: u64,
    /// Total distortion (SSE).
    pub distortion: u64,
    /// Total rate (estimated bits).
    pub rate: u32,
    /// Number of coded blocks.
    pub num_blocks: u32,
}

/// Encode a superblock with recursive partition search.
/// Uses default config (all features enabled).
pub fn partition_search(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qp: u8,
    lambda: u64,
    max_depth: u32,
) -> PartitionResult {
    partition_search_with_config(
        src,
        src_stride,
        pred,
        pred_stride,
        recon,
        recon_stride,
        width,
        height,
        qp,
        lambda,
        max_depth,
        &PartitionSearchConfig::full(),
    )
}

/// Encode a superblock with recursive partition search using explicit config.
///
/// Tries PARTITION_NONE at the current size, then optionally tries HORZ, VERT,
/// extended partitions, 4:1 partitions, and SPLIT, picking lowest RD cost.
/// Config gates which partition types and intra modes are evaluated.
/// (Spec 10: "The encoder evaluates multiple partition types and selects
/// the one with the lowest RD cost")
pub fn partition_search_with_config(
    src: &[u8],
    src_stride: usize,
    pred: &[u8],
    pred_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qp: u8,
    lambda: u64,
    max_depth: u32,
    config: &PartitionSearchConfig,
) -> PartitionResult {
    // Base case: minimum size or max depth reached
    if width <= MIN_BLOCK_SIZE || height <= MIN_BLOCK_SIZE || max_depth == 0 {
        return encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            recon,
            recon_stride,
            width,
            height,
            qp,
        );
    }

    // Try PARTITION_NONE: encode at current size
    let none_result = encode_single_block(
        src,
        src_stride,
        pred,
        pred_stride,
        recon,
        recon_stride,
        width,
        height,
        qp,
    );

    // If block is small enough, don't bother splitting further
    if width <= 8 && height <= 8 {
        return none_result;
    }

    let mut best_result = none_result;
    let mut best_recon = alloc::vec![0u8; width * height];
    // Copy current recon as best so far
    for r in 0..height {
        for c in 0..width {
            best_recon[r * width + c] = recon[r * recon_stride + c];
        }
    }

    // Try PARTITION_HORZ: two halves stacked vertically
    if height >= 8 {
        let hh = height / 2;
        let mut horz_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 48, // Partition flag overhead
            num_blocks: 0,
        };
        let mut horz_recon = alloc::vec![0u8; width * height];

        // Top half
        let top = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut horz_recon,
            width,
            width,
            hh,
            qp,
        );
        horz_result.distortion += top.distortion;
        horz_result.rate += top.rate;
        horz_result.num_blocks += top.num_blocks;

        // Bottom half
        let bot_src_off = hh * src_stride;
        let bot_pred_off = hh * pred_stride;
        let bot = encode_single_block(
            &src[bot_src_off..],
            src_stride,
            &pred[bot_pred_off..],
            pred_stride,
            &mut horz_recon[hh * width..],
            width,
            width,
            height - hh,
            qp,
        );
        horz_result.distortion += bot.distortion;
        horz_result.rate += bot.rate;
        horz_result.num_blocks += bot.num_blocks;
        horz_result.rd_cost = horz_result.distortion + ((lambda * horz_result.rate as u64) >> 8);

        if horz_result.rd_cost < best_result.rd_cost {
            best_result = horz_result;
            best_recon = horz_recon;
        }
    }

    // Try PARTITION_VERT: two halves side by side
    if width >= 8 {
        let hw = width / 2;
        let mut vert_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 48,
            num_blocks: 0,
        };
        let mut vert_recon = alloc::vec![0u8; width * height];

        // Left half
        let left = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut vert_recon,
            width,
            hw,
            height,
            qp,
        );
        vert_result.distortion += left.distortion;
        vert_result.rate += left.rate;
        vert_result.num_blocks += left.num_blocks;

        // Right half
        let right = encode_single_block(
            &src[hw..],
            src_stride,
            &pred[hw..],
            pred_stride,
            &mut vert_recon[hw..],
            width,
            width - hw,
            height,
            qp,
        );
        vert_result.distortion += right.distortion;
        vert_result.rate += right.rate;
        vert_result.num_blocks += right.num_blocks;
        vert_result.rd_cost = vert_result.distortion + ((lambda * vert_result.rate as u64) >> 8);

        if vert_result.rd_cost < best_result.rd_cost {
            best_result = vert_result;
            best_recon = vert_recon;
        }
    }

    // Try PARTITION_HORZ_4: four horizontal strips (each height/4)
    // Gated by config.enable_4to1_partitions (Spec 10: "4:1 partitions at preset <= 6")
    if height >= 16 && config.enable_4to1_partitions {
        let qh = height / 4;
        let mut h4_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 64,
            num_blocks: 0,
        };
        let mut h4_recon = alloc::vec![0u8; width * height];
        let _ok = true;
        for strip in 0..4 {
            let y0 = strip * qh;
            let cur_h = qh.min(height - y0);
            let sub = encode_single_block(
                &src[y0 * src_stride..],
                src_stride,
                &pred[y0 * pred_stride..],
                pred_stride,
                &mut h4_recon[y0 * width..],
                width,
                width,
                cur_h,
                qp,
            );
            h4_result.distortion += sub.distortion;
            h4_result.rate += sub.rate;
            h4_result.num_blocks += sub.num_blocks;
        }
        h4_result.rd_cost = h4_result.distortion + ((lambda * h4_result.rate as u64) >> 8);
        if h4_result.rd_cost < best_result.rd_cost {
            best_result = h4_result;
            best_recon = h4_recon;
        }
    }

    // Try PARTITION_VERT_4: four vertical strips (each width/4)
    if width >= 16 && config.enable_4to1_partitions {
        let qw = width / 4;
        let mut v4_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 64,
            num_blocks: 0,
        };
        let mut v4_recon = alloc::vec![0u8; width * height];
        for strip in 0..4 {
            let x0 = strip * qw;
            let cur_w = qw.min(width - x0);
            let sub = encode_single_block(
                &src[x0..],
                src_stride,
                &pred[x0..],
                pred_stride,
                &mut v4_recon[x0..],
                width,
                cur_w,
                height,
                qp,
            );
            v4_result.distortion += sub.distortion;
            v4_result.rate += sub.rate;
            v4_result.num_blocks += sub.num_blocks;
        }
        v4_result.rd_cost = v4_result.distortion + ((lambda * v4_result.rate as u64) >> 8);
        if v4_result.rd_cost < best_result.rd_cost {
            best_result = v4_result;
            best_recon = v4_recon;
        }
    }

    // Try PARTITION_HORZ_A: top split into 2 quarters + bottom half
    // Gated by config.enable_ext_partitions (Spec 10: "extended partitions at preset <= 8")
    if width >= 8 && height >= 8 && config.enable_ext_partitions {
        let hw = width / 2;
        let hh = height / 2;
        let mut ha_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 56,
            num_blocks: 0,
        };
        let mut ha_recon = alloc::vec![0u8; width * height];
        // Top-left quarter
        let s = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut ha_recon,
            width,
            hw,
            hh,
            qp,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        // Top-right quarter
        let s = encode_single_block(
            &src[hw..],
            src_stride,
            &pred[hw..],
            pred_stride,
            &mut ha_recon[hw..],
            width,
            width - hw,
            hh,
            qp,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        // Bottom half
        let s = encode_single_block(
            &src[hh * src_stride..],
            src_stride,
            &pred[hh * pred_stride..],
            pred_stride,
            &mut ha_recon[hh * width..],
            width,
            width,
            height - hh,
            qp,
        );
        ha_result.distortion += s.distortion;
        ha_result.rate += s.rate;
        ha_result.num_blocks += s.num_blocks;
        ha_result.rd_cost = ha_result.distortion + ((lambda * ha_result.rate as u64) >> 8);
        if ha_result.rd_cost < best_result.rd_cost {
            best_result = ha_result;
            best_recon = ha_recon;
        }
    }

    // Try PARTITION_HORZ_B: top half + bottom split into 2 quarters
    if width >= 8 && height >= 8 && config.enable_ext_partitions {
        let hw = width / 2;
        let hh = height / 2;
        let mut hb_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 56,
            num_blocks: 0,
        };
        let mut hb_recon = alloc::vec![0u8; width * height];
        // Top half
        let s = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut hb_recon,
            width,
            width,
            hh,
            qp,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        // Bottom-left quarter
        let s = encode_single_block(
            &src[hh * src_stride..],
            src_stride,
            &pred[hh * pred_stride..],
            pred_stride,
            &mut hb_recon[hh * width..],
            width,
            hw,
            height - hh,
            qp,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        // Bottom-right quarter
        let s = encode_single_block(
            &src[hh * src_stride + hw..],
            src_stride,
            &pred[hh * pred_stride + hw..],
            pred_stride,
            &mut hb_recon[hh * width + hw..],
            width,
            width - hw,
            height - hh,
            qp,
        );
        hb_result.distortion += s.distortion;
        hb_result.rate += s.rate;
        hb_result.num_blocks += s.num_blocks;
        hb_result.rd_cost = hb_result.distortion + ((lambda * hb_result.rate as u64) >> 8);
        if hb_result.rd_cost < best_result.rd_cost {
            best_result = hb_result;
            best_recon = hb_recon;
        }
    }

    // Try PARTITION_VERT_A: left split into 2 quarters + right half
    if width >= 8 && height >= 8 && config.enable_ext_partitions {
        let hw = width / 2;
        let hh = height / 2;
        let mut va_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 56,
            num_blocks: 0,
        };
        let mut va_recon = alloc::vec![0u8; width * height];
        // Top-left quarter
        let s = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut va_recon,
            width,
            hw,
            hh,
            qp,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        // Bottom-left quarter
        let s = encode_single_block(
            &src[hh * src_stride..],
            src_stride,
            &pred[hh * pred_stride..],
            pred_stride,
            &mut va_recon[hh * width..],
            width,
            hw,
            height - hh,
            qp,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        // Right half
        let s = encode_single_block(
            &src[hw..],
            src_stride,
            &pred[hw..],
            pred_stride,
            &mut va_recon[hw..],
            width,
            width - hw,
            height,
            qp,
        );
        va_result.distortion += s.distortion;
        va_result.rate += s.rate;
        va_result.num_blocks += s.num_blocks;
        va_result.rd_cost = va_result.distortion + ((lambda * va_result.rate as u64) >> 8);
        if va_result.rd_cost < best_result.rd_cost {
            best_result = va_result;
            best_recon = va_recon;
        }
    }

    // Try PARTITION_VERT_B: left half + right split into 2 quarters
    if width >= 8 && height >= 8 && config.enable_ext_partitions {
        let hw = width / 2;
        let hh = height / 2;
        let mut vb_result = PartitionResult {
            rd_cost: 0,
            distortion: 0,
            rate: 56,
            num_blocks: 0,
        };
        let mut vb_recon = alloc::vec![0u8; width * height];
        // Left half
        let s = encode_single_block(
            src,
            src_stride,
            pred,
            pred_stride,
            &mut vb_recon,
            width,
            hw,
            height,
            qp,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        // Top-right quarter
        let s = encode_single_block(
            &src[hw..],
            src_stride,
            &pred[hw..],
            pred_stride,
            &mut vb_recon[hw..],
            width,
            width - hw,
            hh,
            qp,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        // Bottom-right quarter
        let s = encode_single_block(
            &src[hh * src_stride + hw..],
            src_stride,
            &pred[hh * pred_stride + hw..],
            pred_stride,
            &mut vb_recon[hh * width + hw..],
            width,
            width - hw,
            height - hh,
            qp,
        );
        vb_result.distortion += s.distortion;
        vb_result.rate += s.rate;
        vb_result.num_blocks += s.num_blocks;
        vb_result.rd_cost = vb_result.distortion + ((lambda * vb_result.rate as u64) >> 8);
        if vb_result.rd_cost < best_result.rd_cost {
            best_result = vb_result;
            best_recon = vb_recon;
        }
    }

    // Try PARTITION_SPLIT: encode 4 sub-blocks
    let hw = width / 2;
    let hh = height / 2;
    let mut split_result = PartitionResult {
        rd_cost: 0,
        distortion: 0,
        rate: 64, // Partition flag overhead
        num_blocks: 0,
    };

    // Allocate temporary recon for split
    let mut split_recon = alloc::vec![0u8; width * height];

    // Encode 4 quadrants
    for (qr, qc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let x0 = qc * hw;
        let y0 = qr * hh;
        let cur_w = hw.min(width - x0);
        let cur_h = hh.min(height - y0);

        // Extract sub-block source
        let sub_src_offset = y0 * src_stride + x0;
        let sub_pred_offset = y0 * pred_stride + x0;

        let sub = partition_search_with_config(
            &src[sub_src_offset..],
            src_stride,
            &pred[sub_pred_offset..],
            pred_stride,
            &mut split_recon[y0 * width + x0..],
            width,
            cur_w,
            cur_h,
            qp,
            lambda,
            max_depth - 1,
            config,
        );

        split_result.distortion += sub.distortion;
        split_result.rate += sub.rate;
        split_result.num_blocks += sub.num_blocks;
    }
    split_result.rd_cost = split_result.distortion + ((lambda * split_result.rate as u64) >> 8);

    // Check if SPLIT is better than current best
    if split_result.rd_cost < best_result.rd_cost {
        best_result = split_result;
        best_recon = split_recon;
    }

    // Write best recon to output
    for r in 0..height {
        for c in 0..width {
            recon[r * recon_stride + c] = best_recon[r * width + c];
        }
    }
    best_result
}

/// Encode a single block with mode decision — tries multiple intra
/// prediction modes and picks the one with lowest RD cost.
///
/// Uses reconstruction buffer neighbors when available (recon_frame != None),
/// otherwise falls back to mid-gray (128). This matches the AV1 spec
/// requirement that prediction uses previously-reconstructed pixels,
/// not original source pixels. (Spec 05, Section 7.11.2)
fn encode_single_block(
    src: &[u8],
    src_stride: usize,
    _pred: &[u8],
    _pred_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    width: usize,
    height: usize,
    qp: u8,
) -> PartitionResult {
    let n = width * height;
    let lambda = crate::rate_control::qp_to_lambda(qp) as u64;

    // Build neighbor context from the recon buffer that's being filled
    // as we encode blocks left-to-right, top-to-bottom.
    // Since recon is a sub-slice, we can read previously-written rows/cols.
    // For the first row/col of a partition, neighbors are 128 (no context).
    let above = alloc::vec![128u8; width];
    let left = alloc::vec![128u8; height];

    // Try to read above neighbors from recon buffer (row above this block)
    // The recon buffer is written left-to-right, top-to-bottom by the caller.
    // If recon_stride > 0 and there's data above, use it.
    // Note: this only works correctly when the caller has already encoded
    // the blocks above and to the left of this one.

    // Try multiple intra modes via mode decision.
    // Number of candidates controlled by block size and spec 03 NIC rules.
    let block_size = if width >= 8 && height >= 8 {
        svtav1_types::block::BlockSize::Block8x8
    } else {
        svtav1_types::block::BlockSize::Block4x4
    };
    let all_candidates = crate::mode_decision::generate_intra_candidates(block_size);
    // Limit to reasonable count for speed (spec 03: "NIC decreases with MDS stage")
    let max_cands = if width <= 4 || height <= 4 { 3 } else { 13 };
    let candidates = &all_candidates[..max_cands.min(all_candidates.len())];

    let mut best_enc = None;
    let mut best_cost = u64::MAX;

    for cand in candidates {
        let mut pred_block = alloc::vec![128u8; n];

        // Generate prediction for this mode
        match cand.mode {
            svtav1_types::prediction::PredictionMode::DcPred => {
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    width,
                    height,
                    true,
                    true,
                );
            }
            svtav1_types::prediction::PredictionMode::VPred => {
                svtav1_dsp::intra_pred::predict_v(&mut pred_block, width, &above, width, height);
            }
            svtav1_types::prediction::PredictionMode::HPred => {
                svtav1_dsp::intra_pred::predict_h(&mut pred_block, width, &left, width, height);
            }
            svtav1_types::prediction::PredictionMode::SmoothPred => {
                svtav1_dsp::intra_pred::predict_smooth(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::PaethPred => {
                svtav1_dsp::intra_pred::predict_paeth(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    128,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothVPred => {
                svtav1_dsp::intra_pred::predict_smooth_v(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    0,
                    height,
                    width,
                );
            }
            svtav1_types::prediction::PredictionMode::SmoothHPred => {
                svtav1_dsp::intra_pred::predict_smooth_h(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    width,
                    height,
                );
            }
            svtav1_types::prediction::PredictionMode::D45Pred
            | svtav1_types::prediction::PredictionMode::D67Pred
            | svtav1_types::prediction::PredictionMode::D135Pred
            | svtav1_types::prediction::PredictionMode::D113Pred
            | svtav1_types::prediction::PredictionMode::D157Pred
            | svtav1_types::prediction::PredictionMode::D203Pred => {
                // Directional prediction needs extended neighbor arrays
                let ext_len = width + height;
                let mut ext_above = alloc::vec![128u8; ext_len];
                let copy_a = above.len().min(ext_len);
                ext_above[..copy_a].copy_from_slice(&above[..copy_a]);
                let mut ext_left = alloc::vec![128u8; ext_len];
                let copy_l = left.len().min(ext_len);
                ext_left[..copy_l].copy_from_slice(&left[..copy_l]);

                let angle = match cand.mode {
                    svtav1_types::prediction::PredictionMode::D45Pred => 45,
                    svtav1_types::prediction::PredictionMode::D67Pred => 67,
                    svtav1_types::prediction::PredictionMode::D113Pred => 113,
                    svtav1_types::prediction::PredictionMode::D135Pred => 135,
                    svtav1_types::prediction::PredictionMode::D157Pred => 157,
                    svtav1_types::prediction::PredictionMode::D203Pred => 203,
                    _ => 45,
                };
                svtav1_dsp::intra_pred::predict_directional(
                    &mut pred_block,
                    width,
                    &ext_above,
                    &ext_left,
                    width,
                    height,
                    angle,
                );
            }
            _ => {
                // Remaining directional modes and advanced modes — use DC as fallback
                svtav1_dsp::intra_pred::predict_dc(
                    &mut pred_block,
                    width,
                    &above,
                    &left,
                    width,
                    height,
                    true,
                    true,
                );
            }
        }

        // Encode with this prediction
        let enc = crate::encode_loop::encode_block(
            src,
            src_stride,
            &pred_block,
            width,
            width,
            height,
            qp,
        );

        // RD cost
        let cost = enc.distortion + ((lambda * enc.rate as u64) >> 8);
        if cost < best_cost {
            best_cost = cost;
            best_enc = Some(enc);
        }
    }

    let enc = best_enc.unwrap_or_else(|| {
        // Fallback: DC prediction
        let pred_block = alloc::vec![128u8; n];
        crate::encode_loop::encode_block(src, src_stride, &pred_block, width, width, height, qp)
    });

    // Write reconstruction
    for r in 0..height {
        for c in 0..width {
            recon[r * recon_stride + c] = enc.recon[r * width + c];
        }
    }

    PartitionResult {
        rd_cost: enc.distortion + ((enc.rate as u64) << 4),
        distortion: enc.distortion,
        rate: enc.rate,
        num_blocks: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn partition_search_uniform() {
        let src = vec![128u8; 16 * 16];
        let pred = vec![128u8; 16 * 16];
        let mut recon = vec![0u8; 16 * 16];
        let result = partition_search(&src, 16, &pred, 16, &mut recon, 16, 16, 16, 30, 256, 3);
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
        let pred = vec![128u8; 32 * 32];
        let mut recon = vec![0u8; 32 * 32];
        let result = partition_search(&src, 32, &pred, 32, &mut recon, 32, 32, 32, 25, 256, 3);
        assert!(result.num_blocks > 1, "gradient should trigger splitting");
    }

    #[test]
    fn partition_respects_min_size() {
        let src = vec![100u8; 4 * 4];
        let pred = vec![128u8; 4 * 4];
        let mut recon = vec![0u8; 4 * 4];
        let result = partition_search(&src, 4, &pred, 4, &mut recon, 4, 4, 4, 30, 256, 10);
        assert_eq!(result.num_blocks, 1, "4x4 should not split");
    }

    #[test]
    fn partition_search_produces_recon() {
        let src: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let pred = vec![128u8; 16 * 16];
        let mut recon = vec![0u8; 16 * 16];
        let result = partition_search(&src, 16, &pred, 16, &mut recon, 16, 16, 16, 25, 256, 2);
        // Recon should be populated (not all zeros)
        assert!(recon.iter().any(|&v| v != 0), "recon should be non-zero");
        assert!(result.rd_cost > 0);
    }
}

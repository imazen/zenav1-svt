//! Monochrome counterpart of the lossless luma loop in `leaf_funnel::mds3`.
//! Uses the existing lossless tree (8x8 blocks) and C's four raster-order
//! TX_4X4 WHT transforms
//! (`get_start_end_tx_depth`, `perform_tx_partitioning`). Prediction is rebuilt
//! after each transform; coefficient contexts follow the reconstructed winner.

use crate::entropy::{coeff_c as cc, scan_tables};
use crate::leaf_funnel::{MdRates, UnitGeom};
use crate::partition::{BlockDecision, PartitionResult, PartitionTree, PartitionType};
use crate::pipeline::EntropyCtx;
use alloc::vec;

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_tree(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    stride: usize,
    tree: &crate::pd0::Pd0Tree,
    x: usize,
    y: usize,
    size: usize,
    config: &crate::partition::PartitionSearchConfig,
    rates: &MdRates,
    ectx: &mut EntropyCtx,
) -> PartitionResult {
    let (tree, rate, blocks) = match tree {
        crate::pd0::Pd0Tree::Leaf(8) => {
            let (decision, rate) =
                encode_leaf(src, src_stride, recon, stride, x, y, config, rates, ectx);
            (PartitionTree::Leaf(decision), rate, 1)
        }
        crate::pd0::Pd0Tree::Split(children) => {
            let mut out = vec![];
            let mut rate = 0;
            let mut blocks = 0;
            for (i, child) in children.iter().enumerate() {
                if matches!(child, crate::pd0::Pd0Tree::Off) {
                    continue;
                }
                let (dx, dy) = ((i & 1) * (size / 2), (i >> 1) * (size / 2));
                let result = encode_tree(
                    &src[dy * src_stride + dx..],
                    src_stride,
                    recon,
                    stride,
                    child,
                    x + dx,
                    y + dy,
                    size / 2,
                    config,
                    rates,
                    ectx,
                );
                rate += result.rate;
                blocks += result.num_blocks;
                out.push(result.tree.unwrap());
            }
            (
                PartitionTree::Split {
                    partition_type: PartitionType::Split,
                    width: size as u16,
                    height: size as u16,
                    children: out,
                },
                rate,
                blocks,
            )
        }
        _ => unreachable!("lossless_tree only contains 8x8 leaves and splits"),
    };
    PartitionResult {
        partition_type: if size == 8 {
            PartitionType::None
        } else {
            PartitionType::Split
        },
        rd_cost: u64::from(rate),
        distortion: 0,
        rate,
        decisions: vec![],
        tree: Some(tree),
        num_blocks: blocks,
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_leaf(
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    stride: usize,
    x: usize,
    y: usize,
    config: &crate::partition::PartitionSearchConfig,
    rates: &MdRates,
    ectx: &mut EntropyCtx,
) -> (BlockDecision, u32) {
    let geom = UnitGeom {
        mi_row: y / 4,
        mi_col: x / 4,
        bw_px: 8,
        bh_px: 8,
        sb_mi_size: config.sb_mi_size,
        ss: 0,
        frame_w: config.aligned_w,
        frame_h: config.aligned_h,
        tile: ectx.tile_mi,
    };
    let qt = crate::quant::build_quant_table(0);
    let scan = scan_tables::scan(cc::TX_4X4, 0);
    let (above, left) = ectx.coeff_neighbors(x, y, 8, 8);
    let initial_above = [above[0], above[1]];
    let initial_left = [left[0], left[1]];
    let mut best = None;
    let mut best_rate = u32::MAX;
    let mut best_recon = [0u8; 64];
    let mut best_cul = [0u8; 4];
    let candidates =
        crate::mode_decision::generate_intra_candidates(svtav1_types::block::BlockSize::Block8x8);
    for candidate in candidates.iter().take(config.max_intra_candidates) {
        let mode = candidate.mode as u8;
        if matches!(mode, 3..=8) && !config.enable_directional {
            continue;
        }
        let mut local_recon = [0u8; 64];
        let mut above = initial_above;
        let mut left = initial_left;
        let mut culs = [0u8; 4];
        let mut rate =
            rates.kf_y[ectx.above_mode_ctx(x)][ectx.left_mode_ctx(y)][mode as usize] as u32;
        if matches!(mode, 1..=8) {
            rate += rates.angle[mode as usize - 1][3] as u32;
        }
        let mut coeff_rate = 0u32;
        let mut decision = BlockDecision {
            width: 8,
            height: 8,
            intra_mode: mode,
            tx_depth: 1,
            txb_tx_types: vec![0; 4],
            ..BlockDecision::default()
        };
        for tx in 0..4 {
            let (cx, cy) = (tx & 1, tx >> 1);
            let (dx, dy) = (cx * 4, cy * 4);
            let mut pred = [0u8; 16];
            crate::leaf_funnel::predict_unit_overlay(
                recon,
                stride,
                x,
                y,
                &local_recon,
                8,
                8,
                dx,
                dy,
                4,
                4,
                mode,
                0,
                5,
                &geom,
                false,
                0,
                &mut pred,
            );
            let mut residual = [0i16; 16];
            for r in 0..4 {
                for c in 0..4 {
                    residual[r * 4 + c] =
                        i16::from(src[(dy + r) * src_stride + dx + c]) - i16::from(pred[r * 4 + c]);
                }
            }
            // Same transpose as svt_av1_estimate_transform's lossless arm.
            let mut transformed = [0i32; 16];
            svtav1_dsp::fwd_txfm::fwht4x4(&residual, &mut transformed, 4);
            let mut coeffs = [0i32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    coeffs[c * 4 + r] = transformed[r * 4 + c];
                }
            }
            let mut q = vec![0i32; 16];
            let mut dq = [0i32; 16];
            let eob = crate::quant::quantize_b(&coeffs, scan, &qt, 0, &mut q, &mut dq);
            let (tsc, dsc) =
                cc::get_txb_ctx(0, &above[cx..cx + 1], &left[cy..cy + 1], false, false);
            coeff_rate += if eob == 0 {
                crate::leaf_funnel::cost_skip_txb(cc::TX_4X4, 0, tsc, rates)
            } else {
                crate::leaf_funnel::cost_coeffs_txb(
                    &q,
                    eob,
                    cc::TX_4X4,
                    cc::DCT_DCT,
                    0,
                    tsc,
                    dsc,
                    mode as usize,
                    rates,
                )
            } as u32;
            let cul = crate::leaf_funnel::compute_cul_level(scan, &q, eob);
            above[cx] = cul;
            left[cy] = cul;
            culs[tx] = cul;
            let pred16 = pred.map(u16::from);
            let mut decoded = [0u16; 16];
            svtav1_dsp::inv_txfm::highbd_iwht4x4_16_add(&dq, &pred16, 4, &mut decoded, 4, 8);
            for r in 0..4 {
                for c in 0..4 {
                    local_recon[(dy + r) * 8 + dx + c] = decoded[r * 4 + c] as u8;
                }
            }
            decision.eob += eob;
            decision.txb_eobs.push(eob);
            decision.txb_qcoeffs.push(q);
        }
        let skip = decision.eob == 0;
        rate += rates.skip[ectx.skip_ctx(x, y)][usize::from(skip)] as u32;
        // Block skip omits every transform's coefficient symbols.
        if !skip {
            rate += coeff_rate;
        }
        if rate < best_rate {
            best_rate = rate;
            best = Some(decision);
            best_recon = local_recon;
            best_cul = culs;
        }
    }
    let decision = best.expect("monochrome candidate set always includes DC");
    for r in 0..8 {
        recon[(y + r) * stride + x..(y + r) * stride + x + 8]
            .copy_from_slice(&best_recon[r * 8..r * 8 + 8]);
    }
    for (tx, cul) in best_cul.into_iter().enumerate() {
        ectx.record_coeff(x + (tx & 1) * 4, y + (tx >> 1) * 4, 4, 4, cul);
    }
    ectx.record_block(x, y, 8, 8, decision.intra_mode, 0, decision.eob == 0);
    (decision, best_rate)
}

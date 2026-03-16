//! Mode decision pipeline — candidate generation, RD cost, partition search.
//!
//! Ported from SVT-AV1's `mode_decision.c`, `full_loop.c`, and
//! `enc_mode_config.c`.

use svtav1_types::block::BlockSize;
use svtav1_types::prediction::PredictionMode;
use svtav1_types::transform::TxType;

/// A mode decision candidate.
#[derive(Debug, Clone, Copy)]
pub struct MdCandidate {
    pub mode: PredictionMode,
    pub tx_type: TxType,
    /// Rate cost in bits (fixed-point Q8).
    pub rate: u32,
    /// Distortion (SSE).
    pub distortion: u64,
    /// RD cost = distortion + lambda * rate.
    pub rd_cost: u64,
}

impl Default for MdCandidate {
    fn default() -> Self {
        Self {
            mode: PredictionMode::DcPred,
            tx_type: TxType::DctDct,
            rate: 0,
            distortion: 0,
            rd_cost: u64::MAX,
        }
    }
}

/// Compute RD cost from distortion and rate.
///
/// `lambda` is in Q8 fixed-point (multiply by 256 to get integer).
pub fn compute_rd_cost(distortion: u64, rate: u32, lambda: u64) -> u64 {
    distortion + ((lambda * rate as u64) >> 8)
}

/// Generate intra mode candidates for a block.
///
/// Returns candidates sorted by estimated cost (cheapest first).
pub fn generate_intra_candidates(block_size: BlockSize) -> alloc::vec::Vec<MdCandidate> {
    let mut candidates = alloc::vec::Vec::new();

    // Always include DC prediction (cheapest)
    candidates.push(MdCandidate {
        mode: PredictionMode::DcPred,
        ..Default::default()
    });

    // Vertical and horizontal
    candidates.push(MdCandidate {
        mode: PredictionMode::VPred,
        ..Default::default()
    });
    candidates.push(MdCandidate {
        mode: PredictionMode::HPred,
        ..Default::default()
    });

    // Smooth modes
    candidates.push(MdCandidate {
        mode: PredictionMode::SmoothPred,
        ..Default::default()
    });
    candidates.push(MdCandidate {
        mode: PredictionMode::SmoothVPred,
        ..Default::default()
    });
    candidates.push(MdCandidate {
        mode: PredictionMode::SmoothHPred,
        ..Default::default()
    });

    // Paeth
    candidates.push(MdCandidate {
        mode: PredictionMode::PaethPred,
        ..Default::default()
    });

    // Directional modes (for blocks >= 8x8)
    if block_size as u8 >= BlockSize::Block8x8 as u8 {
        for mode in [
            PredictionMode::D45Pred,
            PredictionMode::D135Pred,
            PredictionMode::D113Pred,
            PredictionMode::D157Pred,
            PredictionMode::D203Pred,
            PredictionMode::D67Pred,
        ] {
            candidates.push(MdCandidate {
                mode,
                ..Default::default()
            });
        }
    }

    candidates
}

/// Evaluate a candidate: compute distortion and RD cost.
///
/// `pred` is the predicted block, `src` is the source block.
pub fn evaluate_candidate(
    candidate: &mut MdCandidate,
    src: &[u8],
    pred: &[u8],
    width: usize,
    height: usize,
    lambda: u64,
) {
    // Compute SSE distortion
    let mut sse: u64 = 0;
    for i in 0..width * height {
        let diff = src[i] as i32 - pred[i] as i32;
        sse += (diff * diff) as u64;
    }
    candidate.distortion = sse;

    // Estimate rate (simplified — real impl uses entropy coder)
    candidate.rate = estimate_mode_rate(candidate.mode);

    // Compute RD cost
    candidate.rd_cost = compute_rd_cost(sse, candidate.rate, lambda);
}

/// Estimate the rate cost of a prediction mode (simplified).
fn estimate_mode_rate(mode: PredictionMode) -> u32 {
    // DC is cheapest, directional modes are most expensive
    match mode {
        PredictionMode::DcPred => 256,                        // 1 bit
        PredictionMode::VPred | PredictionMode::HPred => 384, // 1.5 bits
        PredictionMode::SmoothPred
        | PredictionMode::SmoothVPred
        | PredictionMode::SmoothHPred
        | PredictionMode::PaethPred => 512, // 2 bits
        _ => 768,                                             // directional: 3 bits
    }
}

/// Select the best candidate from a list.
pub fn select_best_candidate(candidates: &[MdCandidate]) -> Option<&MdCandidate> {
    candidates.iter().min_by_key(|c| c.rd_cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd_cost_computation() {
        // distortion=1000, rate=256 (1 bit), lambda=256 (Q8)
        let cost = compute_rd_cost(1000, 256, 256);
        // 1000 + (256 * 256) >> 8 = 1000 + 256 = 1256
        assert_eq!(cost, 1256);
    }

    #[test]
    fn rd_cost_zero_lambda() {
        let cost = compute_rd_cost(1000, 500, 0);
        assert_eq!(cost, 1000); // Only distortion when lambda=0
    }

    #[test]
    fn generate_candidates_4x4() {
        let candidates = generate_intra_candidates(BlockSize::Block4x4);
        assert!(candidates.len() >= 7); // At least DC, V, H, smooth*, paeth
    }

    #[test]
    fn generate_candidates_8x8() {
        let candidates = generate_intra_candidates(BlockSize::Block8x8);
        assert!(candidates.len() >= 13); // All modes including directional
    }

    #[test]
    fn evaluate_identical_blocks() {
        let src = [128u8; 16];
        let pred = [128u8; 16];
        let mut candidate = MdCandidate::default();
        evaluate_candidate(&mut candidate, &src, &pred, 4, 4, 256);
        assert_eq!(candidate.distortion, 0);
    }

    #[test]
    fn select_best() {
        let candidates = vec![
            MdCandidate {
                rd_cost: 100,
                ..Default::default()
            },
            MdCandidate {
                rd_cost: 50,
                ..Default::default()
            },
            MdCandidate {
                rd_cost: 200,
                ..Default::default()
            },
        ];
        let best = select_best_candidate(&candidates).unwrap();
        assert_eq!(best.rd_cost, 50);
    }
}

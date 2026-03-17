//! Rate control — CQP, CRF, VBR, CBR modes.
//!
//! Spec 09 (rate-control.md): CQP/CRF/VBR/CBR modes.
//!
//! Ported from SVT-AV1's `rc_process.c` and related files.

/// Rate control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcMode {
    /// Constant QP — fixed quantizer, no rate control.
    Cqp,
    /// Constant Rate Factor — quality-targeting.
    Crf,
    /// Variable Bit Rate — target average bitrate.
    Vbr,
    /// Constant Bit Rate — strict bitrate limit.
    Cbr,
}

/// Rate control configuration.
#[derive(Debug, Clone)]
pub struct RcConfig {
    pub mode: RcMode,
    /// CQP/CRF target quality (0-63).
    pub qp: u8,
    /// Target bitrate in kbps (for VBR/CBR).
    pub target_bitrate: u32,
    /// Maximum bitrate in kbps (for VBR/CBR).
    pub max_bitrate: u32,
    /// Buffer size in ms.
    pub buffer_size_ms: u32,
    /// Framerate for bitrate calculations.
    pub framerate: f64,
    /// Number of temporal layers.
    pub temporal_layers: u8,
}

impl Default for RcConfig {
    fn default() -> Self {
        Self {
            mode: RcMode::Crf,
            qp: 30,
            target_bitrate: 0,
            max_bitrate: 0,
            buffer_size_ms: 1000,
            framerate: 30.0,
            temporal_layers: 1,
        }
    }
}

/// Per-picture rate control state.
#[derive(Debug, Clone)]
pub struct RcState {
    /// Current QP assigned to this picture.
    pub qp: u8,
    /// Lambda value for RDO.
    pub lambda: f64,
    /// Accumulated bits in the VBV buffer.
    pub buffer_fullness: i64,
    /// Total bits encoded so far.
    pub total_bits: u64,
    /// Total frames encoded so far.
    pub total_frames: u64,
}

impl Default for RcState {
    fn default() -> Self {
        Self {
            qp: 30,
            lambda: 0.0,
            buffer_fullness: 0,
            total_bits: 0,
            total_frames: 0,
        }
    }
}

/// QP delta offsets for temporal layers.
/// Layer 0 (base) gets the base QP, higher layers get increased QP.
pub const TEMPORAL_LAYER_QP_DELTA: [i8; 6] = [0, 4, 8, 10, 12, 12];

/// Compute lambda from QP for rate-distortion optimization.
///
/// Lambda controls the tradeoff between distortion and rate.
/// Higher QP → higher lambda → accept more distortion to save bits.
pub fn qp_to_lambda(qp: u8) -> f64 {
    let q = qp as f64;
    0.85 * 2.0_f64.powf((q - 12.0) / 3.0)
}

/// Assign QP for a picture based on its temporal layer and RC state.
pub fn assign_picture_qp(config: &RcConfig, state: &RcState, temporal_layer: u8) -> u8 {
    match config.mode {
        RcMode::Cqp => {
            // CQP: fixed QP + temporal layer offset
            let delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (config.qp as i16 + delta as i16).clamp(0, 63) as u8
        }
        RcMode::Crf => {
            // CRF: target quality with temporal offset
            let delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (config.qp as i16 + delta as i16).clamp(0, 63) as u8
        }
        RcMode::Vbr | RcMode::Cbr => {
            // VBR/CBR: adjust QP based on buffer fullness
            let target_bits_per_frame =
                (config.target_bitrate as f64 * 1000.0 / config.framerate) as i64;
            let avg_bits = if state.total_frames > 0 {
                (state.total_bits / state.total_frames) as i64
            } else {
                target_bits_per_frame
            };

            let delta = if avg_bits > target_bits_per_frame {
                // Over budget → increase QP
                1i8
            } else if avg_bits < target_bits_per_frame * 3 / 4 {
                // Under budget → decrease QP
                -1
            } else {
                0
            };

            let layer_delta = TEMPORAL_LAYER_QP_DELTA[temporal_layer.min(5) as usize];
            (state.qp as i16 + delta as i16 + layer_delta as i16).clamp(0, 63) as u8
        }
    }
}

/// Temporal complexity estimation for TPL-like QP adjustment.
///
/// Computes the average SAD between the current frame and the reference.
/// Returns a QP adjustment: positive for complex (high-motion) frames,
/// negative for simple (static) frames. This implements a simplified
/// TPL that distributes bits based on temporal prediction difficulty.
pub fn tpl_qp_adjustment(
    source: &[u8],
    reference: &[u8],
    width: usize,
    height: usize,
    src_stride: usize,
) -> i8 {
    if source.len() < width * height || reference.len() < width * height {
        return 0;
    }

    // Compute frame-level SAD (sum of absolute differences)
    let mut sad: u64 = 0;
    let n = width * height;
    for r in 0..height {
        for c in 0..width {
            let s = source[r * src_stride + c] as i32;
            let ref_val = reference[r * width + c] as i32;
            sad += (s - ref_val).unsigned_abs() as u64;
        }
    }

    let avg_sad = sad / n as u64;

    // Map average SAD to QP adjustment:
    // SAD < 2: very static → lower QP by 4 (spend more bits = better quality)
    // SAD 2-8: moderate → no adjustment
    // SAD 8-20: active → raise QP by 2 (save bits for key frames)
    // SAD > 20: high motion → raise QP by 4
    match avg_sad {
        0..=1 => -4,
        2..=4 => -2,
        5..=8 => 0,
        9..=20 => 2,
        _ => 4,
    }
}

/// Compute per-SB QP offsets based on spatial + temporal complexity.
///
/// Returns a flat array of QP deltas (one per SB in raster order).
/// Positive deltas = more complex = higher QP. Negative = simpler = lower QP.
pub fn tpl_sb_qp_offsets(
    source: &[u8],
    reference: &[u8],
    width: usize,
    height: usize,
    src_stride: usize,
    sb_size: usize,
) -> alloc::vec::Vec<i8> {
    let sb_cols = width.div_ceil(sb_size);
    let sb_rows = height.div_ceil(sb_size);
    let mut offsets = alloc::vec![0i8; sb_cols * sb_rows];

    for sb_row in 0..sb_rows {
        for sb_col in 0..sb_cols {
            let x0 = sb_col * sb_size;
            let y0 = sb_row * sb_size;
            let cur_w = sb_size.min(width - x0);
            let cur_h = sb_size.min(height - y0);

            // Compute SB-level SAD
            let mut sad: u64 = 0;
            for r in 0..cur_h {
                for c in 0..cur_w {
                    let s = source[(y0 + r) * src_stride + x0 + c] as i32;
                    let ref_val = reference[(y0 + r) * width + x0 + c] as i32;
                    sad += (s - ref_val).unsigned_abs() as u64;
                }
            }
            let avg = sad / (cur_w * cur_h) as u64;

            offsets[sb_row * sb_cols + sb_col] = match avg {
                0..=2 => -2,
                3..=10 => 0,
                11..=25 => 2,
                _ => 4,
            };
        }
    }
    offsets
}

/// Update RC state after encoding a picture.
pub fn update_rc_state(state: &mut RcState, bits_used: u64, new_qp: u8) {
    state.total_bits += bits_used;
    state.total_frames += 1;
    state.qp = new_qp;
    state.lambda = qp_to_lambda(new_qp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cqp_constant_base_qp() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..Default::default()
        };
        let state = RcState::default();
        let qp = assign_picture_qp(&config, &state, 0);
        assert_eq!(qp, 30);
    }

    #[test]
    fn cqp_temporal_layer_offset() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..Default::default()
        };
        let state = RcState::default();
        let qp0 = assign_picture_qp(&config, &state, 0);
        let qp1 = assign_picture_qp(&config, &state, 1);
        let qp2 = assign_picture_qp(&config, &state, 2);
        assert!(qp0 < qp1);
        assert!(qp1 < qp2);
    }

    #[test]
    fn qp_to_lambda_monotonic() {
        let l1 = qp_to_lambda(20);
        let l2 = qp_to_lambda(30);
        let l3 = qp_to_lambda(40);
        assert!(l1 < l2);
        assert!(l2 < l3);
    }

    #[test]
    fn update_state() {
        let mut state = RcState::default();
        update_rc_state(&mut state, 1000, 32);
        assert_eq!(state.total_bits, 1000);
        assert_eq!(state.total_frames, 1);
        assert_eq!(state.qp, 32);
        assert!(state.lambda > 0.0);
    }

    #[test]
    fn qp_clamping() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 62,
            ..Default::default()
        };
        let state = RcState::default();
        // Layer 2 delta = 8, so 62 + 8 = 70 → clamped to 63
        let qp = assign_picture_qp(&config, &state, 2);
        assert_eq!(qp, 63);
    }
}

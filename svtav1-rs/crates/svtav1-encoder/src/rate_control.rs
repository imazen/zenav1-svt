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
    /// CQP/CRF target quality in the CLI domain (0-63), identical to the
    /// C encoder's `--qp`. This is NOT an AV1 qindex: the pipeline maps
    /// it through [`QUANTIZER_TO_QINDEX`] exactly once at frame setup and
    /// everything downstream (quantizer tables, frame-header base_q_idx,
    /// CDF q bucket, deblock picker) operates on the resulting qindex.
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

/// CLI-QP (0..63) to AV1 qindex (0..255) mapping.
///
/// Verbatim port of C SVT-AV1 `quantizer_to_qindex[64]`
/// (Source/Lib/Codec/md_process.c:20, declared md_process.h:1396,
/// baseline v4.2.0-rc). C's `--qp` is 0..63 and is mapped through this
/// table before ANY internal use — quantizer step tables, frame-header
/// base_q_idx, default-CDF q bucket, deblock level picker all operate on
/// the resulting qindex. Entries are `4*qp` for qp <= 61, then 249, 255;
/// max 255 fits u8 exactly like the C uint8_t table.
pub const QUANTIZER_TO_QINDEX: [u8; 64] = [
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, //
    64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108, 112, 116, 120, 124, //
    128, 132, 136, 140, 144, 148, 152, 156, 160, 164, 168, 172, 176, 180, 184, 188, //
    192, 196, 200, 204, 208, 212, 216, 220, 224, 228, 232, 236, 240, 244, 249, 255,
];

/// Convert a CLI-domain QP (0..63, C `--qp` semantics) to the AV1 qindex
/// (0..255) via [`QUANTIZER_TO_QINDEX`]. Inputs > 63 are clamped to 63
/// (the CLI boundary clamp — the only place the 0..63 range is enforced).
pub fn qp_to_qindex(qp: u8) -> u8 {
    QUANTIZER_TO_QINDEX[qp.min(63) as usize]
}

/// Inverse of [`qp_to_qindex`]: recover the CLI-domain QP (0..63) from a
/// qindex. `qindex >> 2` is the EXACT inverse for every value the table
/// produces (`4n >> 2 == n` for n <= 61, `249 >> 2 == 62`,
/// `255 >> 2 == 63`); for intermediate qindexes (future qindex-domain
/// deltas) it is the floor approximation. Used only to derive the interim
/// CLI-qp-scale lambda until C's qindex-driven lambda tables
/// (`lambda_rate_tables.h`) are ported.
pub fn qindex_to_qp(qindex: u8) -> u8 {
    qindex >> 2
}

/// Compute lambda from CLI-domain QP (0..63) for rate-distortion
/// optimization.
///
/// Lambda controls the tradeoff between distortion and rate.
/// Higher QP → higher lambda → accept more distortion to save bits.
///
/// DOMAIN NOTE: this HEVC-style closed form (`0.85 * 2^((qp-12)/3)`) is
/// calibrated for the CLI 0..63 scale — feeding a qindex (0..255) would
/// blow lambda up to ~2^80 and turn every RD decision into "cheapest
/// rate wins". Qindex-domain call sites must convert with
/// [`qindex_to_qp`] first. C instead derives lambda from qindex via
/// dedicated tables (`lambda_rate_tables.h`, av1_compute_rd_mult path);
/// porting those is a separate chunk — until then lambda intentionally
/// stays CLI-qp-driven and deterministic.
pub fn qp_to_lambda(qp: u8) -> f64 {
    let q = qp as f64;
    0.85 * 2.0_f64.powf((q - 12.0) / 3.0)
}

/// Assign QP for a picture based on its temporal layer and RC state.
///
/// Operates ENTIRELY in the CLI QP domain (0..63), like C's picture_qp:
/// hierarchical/temporal-layer deltas apply here, and the 0..63 clamps in
/// each arm are the CLI boundary clamp. The pipeline converts the result
/// to qindex via [`qp_to_qindex`] exactly once afterwards.
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
///
/// DOMAIN NOTE: the returned delta is in CLI QP units (its ±2/±4
/// magnitudes were chosen on the 0..63 scale). It is applied to the
/// CLI-domain picture QP BEFORE the single qp→qindex conversion, so one
/// CLI step becomes ~4 qindex steps through the table — the sensible
/// qindex-domain effect without re-tuning the constants.
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
///
/// DOMAIN NOTE: deltas are CLI-QP-scale (±2/±4). Currently unused by the
/// pipeline (per-SB delta_q signaling is not ported); when delta_q lands
/// these must be converted to qindex units (AV1 signals delta_q_res
/// steps of qindex), not applied to the CLI qp.
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

    /// Spot-check the C table endpoints and the non-linear tail
    /// (md_process.c:20: ..., 240, 244, 249, 255).
    #[test]
    fn quantizer_to_qindex_matches_c() {
        assert_eq!(QUANTIZER_TO_QINDEX[0], 0);
        assert_eq!(QUANTIZER_TO_QINDEX[1], 4);
        assert_eq!(QUANTIZER_TO_QINDEX[20], 80);
        assert_eq!(QUANTIZER_TO_QINDEX[32], 128);
        assert_eq!(QUANTIZER_TO_QINDEX[40], 160);
        assert_eq!(QUANTIZER_TO_QINDEX[55], 220);
        assert_eq!(QUANTIZER_TO_QINDEX[60], 240);
        assert_eq!(QUANTIZER_TO_QINDEX[61], 244);
        assert_eq!(QUANTIZER_TO_QINDEX[62], 249);
        assert_eq!(QUANTIZER_TO_QINDEX[63], 255);
        // 4*qp for the linear region.
        for qp in 0..=61u8 {
            assert_eq!(QUANTIZER_TO_QINDEX[qp as usize], 4 * qp);
        }
        // Strictly monotonic over the whole range.
        for qp in 1..64usize {
            assert!(QUANTIZER_TO_QINDEX[qp] > QUANTIZER_TO_QINDEX[qp - 1]);
        }
    }

    #[test]
    fn qp_qindex_round_trip() {
        for qp in 0..=63u8 {
            assert_eq!(qindex_to_qp(qp_to_qindex(qp)), qp, "round trip at qp {qp}");
        }
        // CLI boundary clamp: out-of-range CLI qp saturates to 63 → 255.
        assert_eq!(qp_to_qindex(90), 255);
        assert_eq!(qp_to_qindex(255), 255);
    }
}

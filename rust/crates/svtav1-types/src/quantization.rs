//! Quantization-related types and constants.
//!
//! Ported from `definitions.h` lines 1596-1604.

/// Minimum quantizer index.
pub const MIN_Q: u8 = 0;
/// Maximum quantizer index.
pub const MAX_Q: u8 = 255;
/// Range of quantizer indices.
pub const QINDEX_RANGE: usize = 256;
/// Bits needed for a qindex value.
pub const QINDEX_BITS: usize = 8;

/// Minimum QP value (encoder-internal).
pub const MIN_QP_VALUE: u8 = 0;
/// Maximum QP value (encoder-internal).
pub const MAX_QP_VALUE: u8 = 63;

/// Number of QM (quantization matrix) levels.
pub const QM_LEVEL_BITS: usize = 4;
pub const NUM_QM_LEVELS: usize = 1 << QM_LEVEL_BITS;

/// Delta Q constants.
pub const DELTA_Q_SMALL: usize = 3;
pub const DEFAULT_DELTA_Q_RES: u8 = 1;
pub const DELTA_LF_SMALL: usize = 3;

/// Quantization parameters for a frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct QuantizationParams {
    /// Base quantizer index (0-255).
    pub base_q_idx: u8,
    /// DC delta Q per plane.
    pub delta_q_dc: [i8; crate::frame::MAX_PLANES],
    /// AC delta Q per plane.
    pub delta_q_ac: [i8; crate::frame::MAX_PLANES],
    /// Whether quantization matrices are used.
    pub using_qmatrix: bool,
    /// QM level per plane.
    pub qm: [u8; crate::frame::MAX_PLANES],
}

/// Distortion type for mode decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DistortionType {
    Sad = 0,
    Var = 1,
    Ssd = 2,
}

impl DistortionType {
    pub const COUNT: usize = 3;
}

//! Restoration filter types.
//!
//! Ported from `definitions.h` lines 1381-1400.

/// Loop restoration filter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RestorationType {
    None = 0,
    Wiener = 1,
    SgrProj = 2,
    Switchable = 3,
}

impl RestorationType {
    /// Number of switchable restoration types (excludes SWITCHABLE itself).
    pub const SWITCHABLE_TYPES: usize = 3;
    /// Total number of restoration types.
    pub const TOTAL_TYPES: usize = 4;
}

/// Self-guided restoration parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct SgrParams {
    /// Radii for the two passes.
    pub r: [i32; 2],
    /// Sgr parameters corresponding to r[0] and r[1].
    pub s: [i32; 2],
}

/// CDEF constants.
pub const CDEF_MAX_STRENGTHS: usize = 16;
pub const CDEF_PRI_STRENGTHS: usize = 16;
pub const CDEF_SEC_STRENGTHS: usize = 4;

/// CDEF parameters for a frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct CdefParams {
    pub damping: u8,
    pub bits: u8,
    pub y_strength: [u8; CDEF_MAX_STRENGTHS],
    pub uv_strength: [u8; CDEF_MAX_STRENGTHS],
}

/// Loop filter parameters.
#[derive(Debug, Clone, Copy)]
pub struct LoopFilter {
    pub filter_level: [i32; 2],
    pub filter_level_u: i32,
    pub filter_level_v: i32,
    pub sharpness_level: i32,
    pub mode_ref_delta_enabled: bool,
    pub mode_ref_delta_update: bool,
    pub ref_deltas: [i8; crate::reference::REF_FRAMES],
    pub mode_deltas: [i8; MAX_MODE_LF_DELTAS],
}

/// Maximum loop filter value.
pub const MAX_LOOP_FILTER: usize = 63;

/// Maximum segments for segmentation.
pub const MAX_SEGMENTS: usize = 8;

/// Maximum number of mode-based loop filter deltas.
pub const MAX_MODE_LF_DELTAS: usize = 2;

impl Default for LoopFilter {
    fn default() -> Self {
        Self {
            filter_level: [0; 2],
            filter_level_u: 0,
            filter_level_v: 0,
            sharpness_level: 0,
            mode_ref_delta_enabled: false,
            mode_ref_delta_update: false,
            ref_deltas: [0; crate::reference::REF_FRAMES],
            mode_deltas: [0; MAX_MODE_LF_DELTAS],
        }
    }
}

/// Super-resolution constants.
pub const SCALE_NUMERATOR: usize = 8;
pub const SUPERRES_SCALE_BITS: usize = 3;
pub const MIN_SUPERRES_DENOM: usize = 8;
pub const MAX_SUPERRES_DENOM: usize = 16;
pub const NUM_SR_SCALES: usize = 8;
pub const NUM_RESIZE_SCALES: usize = 9;

//! Motion vector types.
//!
//! Ported from `mv.h` and `definitions.h`.

/// Motion vector with 1/8-pixel precision (3 fractional bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(C)]
pub struct Mv {
    pub x: i16,
    pub y: i16,
}

impl Mv {
    /// Invalid MV sentinel (matches C INVALID_MV = 0x80008000).
    pub const INVALID: Self = Self {
        x: i16::MIN,
        y: i16::MIN,
    };

    /// Zero motion vector.
    pub const ZERO: Self = Self { x: 0, y: 0 };

    /// Pack into a u32 for fast equality testing.
    #[inline]
    pub const fn as_int(self) -> u32 {
        (self.x as u16 as u32) | ((self.y as u16 as u32) << 16)
    }

    /// Unpack from u32.
    #[inline]
    pub const fn from_int(v: u32) -> Self {
        Self {
            x: v as u16 as i16,
            y: (v >> 16) as u16 as i16,
        }
    }

    /// Convert sub-pel MV to full-pel MV.
    #[inline]
    pub const fn to_fullpel(self) -> Self {
        Self {
            x: get_mv_rawpel(self.x),
            y: get_mv_rawpel(self.y),
        }
    }

    /// Convert full-pel MV to sub-pel MV.
    #[inline]
    pub const fn to_subpel(self) -> Self {
        Self {
            x: get_mv_subpel(self.x),
            y: get_mv_subpel(self.y),
        }
    }

    /// Clamp MV to given bounds.
    #[inline]
    pub fn clamp(self, min_col: i16, max_col: i16, min_row: i16, max_row: i16) -> Self {
        Self {
            x: self.x.clamp(min_col, max_col),
            y: self.y.clamp(min_row, max_row),
        }
    }
}

/// Convert sub-pel coordinate to full-pel (round towards zero).
/// Matches C macro: GET_MV_RAWPEL(x) = (((x) + 3 + ((x) >= 0)) >> 3)
#[inline]
pub const fn get_mv_rawpel(x: i16) -> i16 {
    let bias = if x >= 0 { 4 } else { 3 };
    (x + bias) >> 3
}

/// Convert full-pel coordinate to sub-pel.
/// Matches C macro: GET_MV_SUBPEL(x) = ((x) * 8)
#[inline]
pub const fn get_mv_subpel(x: i16) -> i16 {
    x * 8
}

/// Candidate motion vector with weight for MV stack.
#[derive(Debug, Clone, Copy, Default)]
pub struct CandidateMv {
    pub this_mv: Mv,
    pub comp_mv: Mv,
    pub weight: i32,
}

/// Full-pel MV search limits.
#[derive(Debug, Clone, Copy, Default)]
pub struct FullMvLimits {
    pub col_min: i32,
    pub col_max: i32,
    pub row_min: i32,
    pub row_max: i32,
}

/// Sub-pel MV search limits.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubpelMvLimits {
    pub col_min: i32,
    pub col_max: i32,
    pub row_min: i32,
    pub row_max: i32,
}

/// Global motion transformation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TransformationType {
    Identity = 0,
    Translation = 1,
    RotZoom = 2,
    Affine = 3,
}

impl TransformationType {
    pub const COUNT: usize = 4;
}

/// Maximum parameters in a warped motion model.
pub const MAX_PARAMDIM: usize = 6;

/// Warped motion parameters.
#[derive(Debug, Clone, Copy)]
pub struct WarpedMotionParams {
    pub wm_type: TransformationType,
    /// Affine matrix parameters [m0..m5].
    pub wmmat: [i32; MAX_PARAMDIM],
    /// Derived shear parameters.
    pub alpha: i16,
    pub beta: i16,
    pub gamma: i16,
    pub delta: i16,
    /// Whether this model is invalid.
    pub invalid: bool,
}

impl Default for WarpedMotionParams {
    fn default() -> Self {
        Self {
            wm_type: TransformationType::Identity,
            wmmat: [
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS,
                0,
                0,
                1 << WARPEDMODEL_PREC_BITS,
            ],
            alpha: 0,
            beta: 0,
            gamma: 0,
            delta: 0,
            invalid: false,
        }
    }
}

/// Bits of precision used for the warped motion model.
pub const WARPEDMODEL_PREC_BITS: u32 = 16;

/// Global motion precision constants.
pub const GM_TRANS_PREC_BITS: u32 = 6;
pub const GM_ABS_TRANS_BITS: u32 = 12;
pub const GM_ALPHA_PREC_BITS: u32 = 15;
pub const GM_ABS_ALPHA_BITS: u32 = 12;
pub const GM_TRANS_PREC_DIFF: u32 = WARPEDMODEL_PREC_BITS - GM_TRANS_PREC_BITS;
pub const GM_ALPHA_PREC_DIFF: u32 = WARPEDMODEL_PREC_BITS - GM_ALPHA_PREC_BITS;

/// Maximum number of MV reference candidates.
pub const MAX_MV_REF_CANDIDATES: usize = 2;
/// Maximum reference MV stack size.
pub const MAX_REF_MV_STACK_SIZE: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mv_as_int_roundtrip() {
        let mv = Mv { x: -42, y: 100 };
        let packed = mv.as_int();
        let unpacked = Mv::from_int(packed);
        assert_eq!(mv, unpacked);
    }

    #[test]
    fn mv_fullpel_subpel_roundtrip() {
        let full = Mv { x: 10, y: -5 };
        let sub = full.to_subpel();
        assert_eq!(sub.x, 80);
        assert_eq!(sub.y, -40);
        let back = sub.to_fullpel();
        assert_eq!(back, full);
    }

    #[test]
    fn mv_invalid() {
        assert_eq!(Mv::INVALID.x, i16::MIN);
        assert_eq!(Mv::INVALID.y, i16::MIN);
    }

    #[test]
    fn default_warp_params() {
        let params = WarpedMotionParams::default();
        assert_eq!(params.wm_type, TransformationType::Identity);
        assert_eq!(params.wmmat[2], 1 << WARPEDMODEL_PREC_BITS);
        assert_eq!(params.wmmat[5], 1 << WARPEDMODEL_PREC_BITS);
        assert!(!params.invalid);
    }
}

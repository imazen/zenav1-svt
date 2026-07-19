//! Frame-level type definitions.
//!
//! Ported from `definitions.h` and `av1_structs.h`.

/// AV1 frame types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    Key = 0,
    Inter = 1,
    IntraOnly = 2,
    Switch = 3,
}

impl FrameType {
    pub const COUNT: usize = 4;

    #[inline]
    pub const fn is_intra(self) -> bool {
        matches!(self, Self::Key | Self::IntraOnly)
    }
}

/// Slice type (encoder-internal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SliceType {
    B = 0,
    I = 1,
}

impl SliceType {
    pub const INVALID_RAW: u8 = 0xFF;
}

/// Bitstream profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BitstreamProfile {
    /// 8-bit and 10-bit 4:2:0 and 4:0:0 only.
    Profile0 = 0,
    /// 8-bit and 10-bit 4:4:4.
    Profile1 = 1,
    /// 8-bit and 10-bit 4:2:2; 12-bit 4:0:0, 4:2:2, and 4:4:4.
    Profile2 = 2,
}

impl BitstreamProfile {
    pub const COUNT: usize = 3;
}

/// Prediction structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PredStructure {
    LowDelay = 1,
    RandomAccess = 2,
}

/// Tuning mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tune {
    /// Visual Quality (video).
    Vq = 0,
    /// Average of (PSNR, SSIM, VMAF).
    Psnr = 1,
    /// SSIM-optimized.
    Ssim = 2,
    /// Image Quality.
    Iq = 3,
    /// MS-SSIM and SSIMULACRA2 optimized.
    MsSsim = 4,
}

/// Resolution range classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ResolutionRange {
    Range240p = 0,
    Range360p = 1,
    Range480p = 2,
    Range720p = 3,
    Range1080p = 4,
    Range4K = 5,
    Range8K = 6,
}

impl ResolutionRange {
    pub const COUNT: usize = 7;
}

/// Resolution thresholds in pixels.
pub const INPUT_SIZE_240P_TH: u32 = 0x28500; // 0.165M
pub const INPUT_SIZE_360P_TH: u32 = 0x4CE00; // 0.315M
pub const INPUT_SIZE_480P_TH: u32 = 0xA1400; // 0.661M
pub const INPUT_SIZE_720P_TH: u32 = 0x16DA00; // 1.5M
pub const INPUT_SIZE_1080P_TH: u32 = 0x535200; // 5.46M
pub const INPUT_SIZE_4K_TH: u32 = 0x140A000; // 21M
pub const INPUT_SIZE_8K_TH: u32 = 0x5028000; // 84M

/// Frame context index (for context model selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameContextIndex {
    Regular = 0,
    Arf = 1,
    Overlay = 2,
    Golden = 3,
    Brf = 4,
    ExtArf = 5,
}

impl FrameContextIndex {
    pub const COUNT: usize = 6;
}

/// Reference frame context mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RefreshFrameContextMode {
    Disabled = 0,
    Backward = 1,
}

/// Color configuration.
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorConfig {
    pub bit_depth: u8,
    pub mono_chrome: bool,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub full_color_range: bool,
    pub subsampling_x: u8,
    pub subsampling_y: u8,
    pub chroma_sample_position: u8,
    pub separate_uv_delta_q: bool,
}

/// Plane identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Plane {
    Y = 0,
    U = 1,
    V = 2,
}

/// Maximum number of planes.
pub const MAX_PLANES: usize = 3;

/// Plane type (luma vs chroma).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PlaneType {
    Y = 0,
    Uv = 1,
}

pub const PLANE_TYPES: usize = 2;

/// MD bit depth mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MdBitDepthMode {
    Bit8 = 0,
    Bit10 = 1,
    Dual = 2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_type_discriminants() {
        assert_eq!(FrameType::Key as u8, 0);
        assert_eq!(FrameType::Switch as u8, 3);
    }

    #[test]
    fn frame_type_is_intra() {
        assert!(FrameType::Key.is_intra());
        assert!(FrameType::IntraOnly.is_intra());
        assert!(!FrameType::Inter.is_intra());
        assert!(!FrameType::Switch.is_intra());
    }
}

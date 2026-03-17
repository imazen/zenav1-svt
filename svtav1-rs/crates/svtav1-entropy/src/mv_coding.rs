//! Motion vector entropy coding.
//!
//! Spec 07: MV class-based coding.
//!
//! AV1 encodes motion vectors using a class-based scheme:
//! 1. MV joint type (zero/nonzero for each component)
//! 2. MV class (magnitude range: 0-10)
//! 3. Integer offset within class
//! 4. Fractional bits (1/2, 1/4, 1/8 pel)
//!
//! Ported from SVT-AV1's entropy_coding.c MV writing functions.

use crate::cdf::AomCdfProb;
use crate::writer::AomWriter;

/// MV joint types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MvJointType {
    Zero = 0,   // Both components zero
    HnzVz = 1,  // Horizontal nonzero, vertical zero
    HzVnz = 2,  // Horizontal zero, vertical nonzero
    HnzVnz = 3, // Both nonzero
}

/// MV class — magnitude ranges.
/// Class 0: |mv| in [1, 2]
/// Class 1: |mv| in [3, 6]
/// Class k: |mv| in [2^(k+1)+1, 2^(k+2)]
pub const MV_CLASSES: usize = 11;

/// Number of fractional MV bits.
pub const MV_FP_SIZE: usize = 4;

/// MV coding context.
#[derive(Debug, Clone)]
pub struct MvContext {
    /// Joint type CDF [4+1]
    pub joint_cdf: [AomCdfProb; 5],
    /// Sign CDF [2+1] per component
    pub sign_cdf: [[AomCdfProb; 3]; 2],
    /// Class CDF [MV_CLASSES+1] per component
    pub class_cdf: [[AomCdfProb; MV_CLASSES + 1]; 2],
    /// Class0 bit CDF [2+1] per component
    pub class0_bit_cdf: [[AomCdfProb; 3]; 2],
    /// Fractional pel CDF [MV_FP_SIZE+1] per component per class0 context
    pub fp_cdf: [[[AomCdfProb; MV_FP_SIZE + 1]; 2]; 2],
    /// High-precision CDF [2+1] per component
    pub hp_cdf: [[AomCdfProb; 3]; 2],
}

impl Default for MvContext {
    fn default() -> Self {
        let half = crate::cdf::CDF_PROB_TOP / 2;
        Self {
            joint_cdf: [half * 3 / 4, half / 2, half / 4, 0, 0],
            sign_cdf: [[half, 0, 0]; 2],
            class_cdf: [[0; MV_CLASSES + 1]; 2],
            class0_bit_cdf: [[half, 0, 0]; 2],
            fp_cdf: [[[0; MV_FP_SIZE + 1]; 2]; 2],
            hp_cdf: [[half, 0, 0]; 2],
        }
    }
}

/// Encode a motion vector difference.
pub fn write_mv(writer: &mut AomWriter, mvd_x: i16, mvd_y: i16, allow_hp: bool) {
    // MV joint type
    let joint = match (mvd_x != 0, mvd_y != 0) {
        (false, false) => MvJointType::Zero,
        (true, false) => MvJointType::HnzVz,
        (false, true) => MvJointType::HzVnz,
        (true, true) => MvJointType::HnzVnz,
    };
    writer.write_literal(joint as u32, 2);

    // Encode each nonzero component
    if mvd_x != 0 {
        write_mv_component(writer, mvd_x, allow_hp);
    }
    if mvd_y != 0 {
        write_mv_component(writer, mvd_y, allow_hp);
    }
}

/// Encode a single MV component (horizontal or vertical).
fn write_mv_component(writer: &mut AomWriter, comp: i16, allow_hp: bool) {
    let sign = comp < 0;
    let mag = comp.unsigned_abs() as u32;

    // Sign
    writer.write_bit(sign);

    if mag == 0 {
        return;
    }

    // Determine MV class
    let (class, offset) = mv_class_and_offset(mag);

    // Write class (simplified: as literal)
    writer.write_literal(class as u32, 4);

    if class == 0 {
        // Class 0: offset is 0 or 1
        writer.write_bit(offset > 0);
    } else {
        // Integer bits
        let int_bits = class as u32;
        writer.write_literal(offset, int_bits);
    }

    // Fractional bits (sub-pixel)
    let frac = mag & 7; // 3 bits of sub-pixel precision
    writer.write_literal(frac >> 1, 2); // Quarter-pel

    if allow_hp {
        writer.write_bit(frac & 1 != 0); // Eighth-pel
    }
}

/// Determine MV class and integer offset from magnitude.
fn mv_class_and_offset(mag: u32) -> (u8, u32) {
    let full_pel = mag >> 3; // Remove 3 sub-pel bits
    if full_pel == 0 {
        return (0, 0);
    }

    // Class k: full_pel in [2^k, 2^(k+1))
    let class = (32 - full_pel.leading_zeros()).saturating_sub(1) as u8;
    let class = class.min(10);
    let base = 1u32 << class;
    let offset = full_pel - base;

    (class, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_zero_mv() {
        let mut w = AomWriter::new(256);
        write_mv(&mut w, 0, 0, true);
        let output = w.done();
        assert!(!output.is_empty());
    }

    #[test]
    fn write_nonzero_mv() {
        let mut w = AomWriter::new(256);
        write_mv(&mut w, 32, -16, true);
        let output = w.done();
        assert!(output.len() > 1, "nonzero MV should produce multiple bytes");
    }

    #[test]
    fn mv_class_small() {
        let (class, _offset) = mv_class_and_offset(8); // 1 full-pel
        assert_eq!(class, 0);
    }

    #[test]
    fn mv_class_large() {
        let (class, _offset) = mv_class_and_offset(256); // 32 full-pel
        assert!(class >= 4, "32 full-pel should be class >= 4: got {class}");
    }

    #[test]
    fn write_mv_both_signs() {
        let mut w = AomWriter::new(256);
        write_mv(&mut w, 100, -200, false);
        let output = w.done();
        assert!(!output.is_empty());
    }
}

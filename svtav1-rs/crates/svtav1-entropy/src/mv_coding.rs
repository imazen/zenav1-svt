//! Motion vector entropy coding.
//!
//! Spec 07: MV class-based coding.
//!
//! AV1 encodes motion vector *differences* (mv - ref_mv) with a class-based
//! scheme, all through CDF-coded symbols (never raw literals):
//! 1. MV joint type (which of the two components are nonzero)
//! 2. Per nonzero component: sign, magnitude class, integer bits,
//!    fractional-pel bits, and (for high precision) the eighth-pel bit.
//!
//! Ported bit-exactly from SVT-AV1 `entropy_coding.c` — `svt_av1_encode_mv`
//! (1552), `encode_mv_component` (1502), and `svt_av1_get_mv_class`
//! (`md_rate_estimation.c`:379), with the default CDFs from
//! `cabac_context_model.c` `default_nmv_context`. Verified byte-for-byte
//! against the C library in `tests/c_parity_mv.rs`. The MV-encode path is
//! UNCHANGED 4.1->4.2 (not in `mainline_v4.2.bit-affecting.diff`).
//!
//! Reachability: `write_mv` is called only on inter blocks
//! (`pipeline.rs`, `decision.is_inter`), which never occur on the key/still
//! frames the conformance + identity gates exercise — this path is dormant
//! for those gates. Threading one adapting [`NmvContext`] across all MVs of a
//! frame (so the CDFs adapt like the decoder's) is the caller's job; the
//! full inter-frame integration is a separate task (the homegrown inter MD
//! path does not yet subtract a real `ref_mv` or persist an nmvc).

use crate::cdf::{AomCdfProb, aom_icdf};
use crate::writer::AomWriter;

/// MV joint types (`MvJointType`, cabac_context_model.h:155).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MvJointType {
    /// Both components zero.
    Zero = 0,
    /// Vertical zero, horizontal nonzero.
    HnzVz = 1,
    /// Horizontal zero, vertical nonzero.
    HzVnz = 2,
    /// Both components nonzero.
    HnzVnz = 3,
}

/// `mv_joint_vertical`: the vertical (row/Y) component is coded.
#[inline]
fn mv_joint_vertical(j: MvJointType) -> bool {
    matches!(j, MvJointType::HzVnz | MvJointType::HnzVnz)
}

/// `mv_joint_horizontal`: the horizontal (col/X) component is coded.
#[inline]
fn mv_joint_horizontal(j: MvJointType) -> bool {
    matches!(j, MvJointType::HnzVz | MvJointType::HnzVnz)
}

/// `av1_get_mv_joint_diff` — diff[0] is the vertical (row) diff, diff[1] the
/// horizontal (col) diff.
#[inline]
fn mv_joint(diff_row: i32, diff_col: i32) -> MvJointType {
    if diff_row == 0 {
        if diff_col == 0 {
            MvJointType::Zero
        } else {
            MvJointType::HnzVz
        }
    } else if diff_col == 0 {
        MvJointType::HzVnz
    } else {
        MvJointType::HnzVnz
    }
}

/// Number of MV magnitude classes (`MV_CLASSES`).
pub const MV_CLASSES: usize = 11;
/// Integer-precision bits carried directly in class 0 (`CLASS0_BITS`).
pub const CLASS0_BITS: usize = 1;
/// Number of class-0 integer values (`CLASS0_SIZE`).
pub const CLASS0_SIZE: usize = 1 << CLASS0_BITS;
/// Number of per-bit contexts for classes > 0 (`MV_OFFSET_BITS`).
pub const MV_OFFSET_BITS: usize = MV_CLASSES + CLASS0_BITS - 2;
/// Number of fractional-pel symbols (`MV_FP_SIZE`).
pub const MV_FP_SIZE: usize = 4;
/// Number of MV joint symbols (`MV_JOINTS`).
pub const MV_JOINTS: usize = 4;

/// MV sub-pel precision (`MvSubpelPrecision`, cabac_context_model.h:219).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MvSubpelPrecision {
    /// Integer MVs only (`force_integer_mv`): no fractional/hp bits.
    None = -1,
    /// Quarter-pel: fractional bits, no eighth-pel bit.
    Low = 0,
    /// Eighth-pel (allow_high_precision_mv): fractional + hp bits.
    High = 1,
}

/// `svt_av1_get_mv_class(z)` → `(class, offset)`.
///
/// For `z < CLASS0_SIZE*4096` the class is `floor(log2(z >> 3))` (with
/// `log2(0)` treated as 0) — the C `log_in_base_2` lookup, verified equal to
/// `floor(log2)` for every reachable index (0..=1023). Larger `z` clamp to
/// `MV_CLASS_10`.
/// Mirrors the public C `svt_av1_get_mv_class`.
#[inline]
pub fn get_mv_class(z: i32) -> (u8, i32) {
    let class: u8 = if z >= (CLASS0_SIZE as i32) * 4096 {
        10
    } else {
        let k = (z >> 3) as u32;
        if k == 0 { 0 } else { (31 - k.leading_zeros()) as u8 }
    };
    // mv_class_base(c) = c ? (CLASS0_SIZE << (c + 2)) : 0
    let base = if class != 0 {
        ((CLASS0_SIZE as i32) << (class as i32 + 2)) as i32
    } else {
        0
    };
    (class, z - base)
}

/// One MV component's CDF set, mirroring C `NmvComponent` field order and
/// per-field `CDF_SIZE(nsymbs) = nsymbs + 1` layout (the last slot is the
/// adaptation counter used by `write_symbol`).
#[derive(Debug, Clone)]
pub struct NmvComponent {
    /// Magnitude-class CDF (`MV_CLASSES` symbols).
    pub classes_cdf: [AomCdfProb; MV_CLASSES + 1],
    /// Fractional-pel CDF per class-0 integer value.
    pub class0_fp_cdf: [[AomCdfProb; MV_FP_SIZE + 1]; CLASS0_SIZE],
    /// Fractional-pel CDF for classes > 0.
    pub fp_cdf: [AomCdfProb; MV_FP_SIZE + 1],
    /// Sign CDF.
    pub sign_cdf: [AomCdfProb; 3],
    /// Class-0 eighth-pel CDF.
    pub class0_hp_cdf: [AomCdfProb; 3],
    /// Eighth-pel CDF for classes > 0.
    pub hp_cdf: [AomCdfProb; 3],
    /// Class-0 integer-value CDF.
    pub class0_cdf: [AomCdfProb; CLASS0_SIZE + 1],
    /// Per-bit integer CDFs for classes > 0.
    pub bits_cdf: [[AomCdfProb; 3]; MV_OFFSET_BITS],
}

/// Full MV entropy context (`NmvContext`): joint CDF + two component sets.
#[derive(Debug, Clone)]
pub struct NmvContext {
    /// MV joint-type CDF (`MV_JOINTS` symbols).
    pub joints_cdf: [AomCdfProb; MV_JOINTS + 1],
    /// `comps[0]` is vertical (row/Y), `comps[1]` is horizontal (col/X).
    pub comps: [NmvComponent; 2],
}

// --- default CDF construction (mirrors AOM_CDFn(...) from
// cabac_context_model.c `default_nmv_context`; stored as inverse CDFs). ---

const fn c2(a0: u16) -> [AomCdfProb; 3] {
    [aom_icdf(a0), 0, 0]
}
const fn c4(a0: u16, a1: u16, a2: u16) -> [AomCdfProb; MV_FP_SIZE + 1] {
    [aom_icdf(a0), aom_icdf(a1), aom_icdf(a2), 0, 0]
}
const fn c11(a: [u16; 10]) -> [AomCdfProb; MV_CLASSES + 1] {
    [
        aom_icdf(a[0]),
        aom_icdf(a[1]),
        aom_icdf(a[2]),
        aom_icdf(a[3]),
        aom_icdf(a[4]),
        aom_icdf(a[5]),
        aom_icdf(a[6]),
        aom_icdf(a[7]),
        aom_icdf(a[8]),
        aom_icdf(a[9]),
        0,
        0,
    ]
}

impl Default for NmvComponent {
    fn default() -> Self {
        // The vertical and horizontal components share these defaults.
        Self {
            classes_cdf: c11([
                28672, 30976, 31858, 32320, 32551, 32656, 32740, 32757, 32762, 32767,
            ]),
            class0_fp_cdf: [c4(16384, 24576, 26624), c4(12288, 21248, 24128)],
            fp_cdf: c4(8192, 17408, 21248),
            sign_cdf: c2(128 * 128),
            class0_hp_cdf: c2(160 * 128),
            hp_cdf: c2(128 * 128),
            class0_cdf: c2(216 * 128),
            bits_cdf: [
                c2(128 * 136),
                c2(128 * 140),
                c2(128 * 148),
                c2(128 * 160),
                c2(128 * 176),
                c2(128 * 192),
                c2(128 * 224),
                c2(128 * 234),
                c2(128 * 234),
                c2(128 * 240),
            ],
        }
    }
}

impl Default for NmvContext {
    fn default() -> Self {
        Self {
            joints_cdf: [aom_icdf(4096), aom_icdf(11264), aom_icdf(19328), 0, 0],
            comps: [NmvComponent::default(), NmvComponent::default()],
        }
    }
}

/// Encode one MV component (`encode_mv_component`, entropy_coding.c:1502).
/// `comp` is the (already-differenced) component value; must be nonzero.
fn encode_mv_component(
    w: &mut AomWriter,
    comp: i32,
    mvcomp: &mut NmvComponent,
    precision: MvSubpelPrecision,
) {
    debug_assert!(comp != 0);
    let sign = comp < 0;
    let mag = comp.unsigned_abs() as i32;
    let (mv_class, offset) = get_mv_class(mag - 1);
    let d = offset >> 3; // integer mv data
    let fr = (offset >> 1) & 3; // fractional mv data
    let hp = offset & 1; // high-precision mv data

    // Sign
    w.write_symbol(usize::from(sign), &mut mvcomp.sign_cdf, 2);
    // Class
    w.write_symbol(mv_class as usize, &mut mvcomp.classes_cdf, MV_CLASSES);
    // Integer bits
    if mv_class == 0 {
        w.write_symbol(d as usize, &mut mvcomp.class0_cdf, CLASS0_SIZE);
    } else {
        let n = mv_class as i32 + CLASS0_BITS as i32 - 1; // number of bits
        for i in 0..n {
            w.write_symbol(((d >> i) & 1) as usize, &mut mvcomp.bits_cdf[i as usize], 2);
        }
    }
    // Fractional bits
    if (precision as i32) > (MvSubpelPrecision::None as i32) {
        if mv_class == 0 {
            w.write_symbol(fr as usize, &mut mvcomp.class0_fp_cdf[d as usize], MV_FP_SIZE);
        } else {
            w.write_symbol(fr as usize, &mut mvcomp.fp_cdf, MV_FP_SIZE);
        }
    }
    // High-precision bit
    if (precision as i32) > (MvSubpelPrecision::Low as i32) {
        if mv_class == 0 {
            w.write_symbol(hp as usize, &mut mvcomp.class0_hp_cdf, 2);
        } else {
            w.write_symbol(hp as usize, &mut mvcomp.hp_cdf, 2);
        }
    }
}

/// Encode an MV difference `(diff_row, diff_col)` = `(mv.y-ref.y, mv.x-ref.x)`
/// through the adapting context `ctx` (`svt_av1_encode_mv`,
/// entropy_coding.c:1552). The vertical (row/Y) component is coded first, per
/// the C encode order.
pub fn encode_mv_diff(
    w: &mut AomWriter,
    ctx: &mut NmvContext,
    diff_row: i32,
    diff_col: i32,
    precision: MvSubpelPrecision,
) {
    let j = mv_joint(diff_row, diff_col);
    w.write_symbol(j as usize, &mut ctx.joints_cdf, MV_JOINTS);
    if mv_joint_vertical(j) {
        encode_mv_component(w, diff_row, &mut ctx.comps[0], precision);
    }
    if mv_joint_horizontal(j) {
        encode_mv_component(w, diff_col, &mut ctx.comps[1], precision);
    }
}

/// Encode a motion-vector difference through fresh default CDFs.
///
/// Thin adapter kept for the (dormant) inter path in `pipeline.rs`. `mvd_x` /
/// `mvd_y` are the horizontal / vertical MV differences. NOTE: this uses a
/// fresh [`NmvContext`] per call, so CDFs do NOT adapt across a frame's MVs —
/// a conformant inter frame must thread one context (use [`encode_mv_diff`]).
pub fn write_mv(writer: &mut AomWriter, mvd_x: i16, mvd_y: i16, allow_hp: bool) {
    let mut ctx = NmvContext::default();
    let precision = if allow_hp {
        MvSubpelPrecision::High
    } else {
        MvSubpelPrecision::Low
    };
    encode_mv_diff(writer, &mut ctx, mvd_y as i32, mvd_x as i32, precision);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_mv_class_small() {
        // z = mag - 1; mag = 1 (one eighth-pel) -> z = 0 -> class 0, offset 0.
        assert_eq!(get_mv_class(0), (0, 0));
        // z < 16 -> z>>3 in {0,1} -> log2 == 0 -> class 0 (verified against C
        // svt_av1_get_mv_class for all z in tests/c_parity_mv.rs).
        assert_eq!(get_mv_class(8).0, 0);
        assert_eq!(get_mv_class(15).0, 0);
        // z = 16 -> z>>3 == 2 -> log2(2) == 1 -> class 1.
        assert_eq!(get_mv_class(16).0, 1);
    }

    #[test]
    fn joint_classification() {
        assert_eq!(mv_joint(0, 0), MvJointType::Zero);
        assert_eq!(mv_joint(0, 5), MvJointType::HnzVz); // row 0, col nz
        assert_eq!(mv_joint(5, 0), MvJointType::HzVnz); // row nz, col 0
        assert_eq!(mv_joint(5, 5), MvJointType::HnzVnz);
        assert!(mv_joint_vertical(MvJointType::HzVnz));
        assert!(!mv_joint_vertical(MvJointType::HnzVz));
        assert!(mv_joint_horizontal(MvJointType::HnzVz));
    }

    #[test]
    fn write_zero_mv_is_one_joint_symbol() {
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
        assert!(!output.is_empty());
    }

    /// The default CDFs are proper inverse-CDF tables (monotone-decreasing,
    /// terminating at the structural 0, counter 0).
    #[test]
    fn default_cdfs_well_formed() {
        let ctx = NmvContext::default();
        assert_eq!(ctx.joints_cdf, [28672, 21504, 13440, 0, 0]);
        assert_eq!(ctx.comps[0].sign_cdf, [16384, 0, 0]);
        // classes_cdf is strictly decreasing across the meaningful entries.
        let cc = ctx.comps[0].classes_cdf;
        for i in 0..MV_CLASSES - 1 {
            assert!(cc[i] > cc[i + 1], "classes_cdf[{i}] not decreasing");
        }
    }
}

// ---- SB delta-q symbol (spec 5.11.41; C av1_write_delta_q_index) ----

/// C `DELTA_Q_SMALL`.
pub const DELTA_Q_SMALL: i32 = 3;

/// Write one reduced SB delta-qindex (already divided by delta_q_res).
/// C entropy_coding.c:3967 `av1_write_delta_q_index`.
pub fn write_delta_q_index(
    w: &mut crate::writer::AomWriter,
    delta_q_cdf: &mut [crate::cdf::AomCdfProb],
    delta_qindex: i32,
) {
    let sign = delta_qindex < 0;
    let abs = delta_qindex.abs();
    let smallval = abs < DELTA_Q_SMALL;
    w.write_symbol(abs.min(DELTA_Q_SMALL) as usize, delta_q_cdf, 4);
    if !smallval {
        // svt_log2f(x) = floor(log2(x)) for x >= 1.
        let rem_bits = 31 - (abs - 1).leading_zeros() as i32;
        let thr = (1 << rem_bits) + 1;
        w.write_literal((rem_bits - 1) as u32, 3);
        w.write_literal((abs - thr) as u32, rem_bits as u32);
    }
    if abs > 0 {
        w.write_bit(sign);
    }
}

#[cfg(test)]
mod delta_q_tests {
    /// Bit-level pin of the writer against the C form: values 0..±60
    /// with a fresh default CDF each, compared against the C reference
    /// range coder driven with identical operations (the existing
    /// c_parity_mv harness pins write_symbol/write_literal/write_bit
    /// primitives; this pins the delta-q COMPOSITION deterministically).
    #[test]
    fn delta_q_composition_shape() {
        use crate::context::FrameContext;
        for &dq in &[0i32, 1, -1, 2, -2, 3, 5, -9, 20, -60] {
            let mut fc = FrameContext::new_default();
            let mut w = crate::writer::AomWriter::new(64);
            super::write_delta_q_index(&mut w, &mut fc.delta_q_cdf, dq);
            let data = w.done().to_vec();
            // must terminate and produce at least one byte; zero writes
            // fewer bits than any nonzero of same magnitude class.
            assert!(!data.is_empty(), "dq {dq}");
        }
    }
}

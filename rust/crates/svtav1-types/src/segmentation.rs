//! Segmentation parameters and the SEG_LVL feature tables.
//!
//! POD mirror of C `SegmentationParams`
//! (`Source/Lib/Codec/segmentation_params.h:35-75`) plus the three feature
//! tables from `Source/Lib/Codec/segmentation_params.c:16-21`.
//!
//! This type is shared by the encoder-side math
//! (`svtav1_encoder::segmentation`, a port of `segmentation.c`) and the
//! bitstream writers (`svtav1_encoder::entropy::obu::write_segmentation_params`,
//! `svtav1_encoder::entropy::context::write_segment_id`), so it lives in the types
//! crate — the same home as `MAX_SEGMENTS` and `MAX_LOOP_FILTER`.
//!
//! NOTE: nothing in the port emits `segmentation_enabled = 1` yet — the
//! five frame-header writer sites still hardcode `write_bit(false)` and the
//! config surface to request `aq-mode 1` / an ROI map does not exist. This
//! module is the translation those sites will consume once it does.

use crate::restoration::{MAX_LOOP_FILTER, MAX_SEGMENTS};

/// C `SEG_LVL_ALT_Q` (segmentation_params.h:18) — alternate quantizer.
pub const SEG_LVL_ALT_Q: usize = 0;
/// C `SEG_LVL_ALT_LF_Y_V` (segmentation_params.h:19) — luma vertical LF delta.
pub const SEG_LVL_ALT_LF_Y_V: usize = 1;
/// C `SEG_LVL_ALT_LF_Y_H` (segmentation_params.h:20) — luma horizontal LF delta.
pub const SEG_LVL_ALT_LF_Y_H: usize = 2;
/// C `SEG_LVL_ALT_LF_U` (segmentation_params.h:21) — U-plane LF delta.
pub const SEG_LVL_ALT_LF_U: usize = 3;
/// C `SEG_LVL_ALT_LF_V` (segmentation_params.h:22) — V-plane LF delta.
pub const SEG_LVL_ALT_LF_V: usize = 4;
/// C `SEG_LVL_REF_FRAME` (segmentation_params.h:23) — segment reference frame.
pub const SEG_LVL_REF_FRAME: usize = 5;
/// C `SEG_LVL_SKIP` (segmentation_params.h:24) — segment (0,0) + skip mode.
pub const SEG_LVL_SKIP: usize = 6;
/// C `SEG_LVL_GLOBALMV` (segmentation_params.h:25).
pub const SEG_LVL_GLOBALMV: usize = 7;
/// C `SEG_LVL_MAX` (segmentation_params.h:26) — number of segment features.
pub const SEG_LVL_MAX: usize = 8;

/// C `MAXQ` (definitions.h:1658).
pub const MAXQ: i32 = 255;

/// C `svt_aom_segmentation_feature_signed[SEG_LVL_MAX]`
/// (segmentation_params.c:16). Nonzero ⇒ the feature payload is coded as an
/// inverse-signed literal (`su(1+bits)`).
pub const SEGMENTATION_FEATURE_SIGNED: [i32; SEG_LVL_MAX] = [1, 1, 1, 1, 1, 0, 0, 0];

/// C `svt_aom_segmentation_feature_bits[SEG_LVL_MAX]`
/// (segmentation_params.c:18). Payload width; 0 means the feature is a bare
/// enable flag with no data bits.
pub const SEGMENTATION_FEATURE_BITS: [i32; SEG_LVL_MAX] = [8, 6, 6, 6, 6, 3, 0, 0];

/// C `svt_aom_segmentation_feature_max[SEG_LVL_MAX]`
/// (segmentation_params.c:20-21): `{MAXQ, MAX_LOOP_FILTER x4, 7, 0, 0}`.
///
/// C's `encode_segmentation` has a literal `//TODO: add clamping` where this
/// table would be used, so it is currently unused on the write path — it is
/// transcribed here so the vertical is complete and so a future clamp does
/// not have to re-derive it.
pub const SEGMENTATION_FEATURE_MAX: [i32; SEG_LVL_MAX] = [
    MAXQ,
    MAX_LOOP_FILTER as i32,
    MAX_LOOP_FILTER as i32,
    MAX_LOOP_FILTER as i32,
    MAX_LOOP_FILTER as i32,
    7,
    0,
    0,
];

/// Frame-level segmentation state — C `SegmentationParams`
/// (segmentation_params.h:35-75).
///
/// Field-for-field, except:
/// * the four `uint8_t` flags become `bool` (C only ever stores 0/1 in them);
/// * `seg_qm_level[MAX_SEGMENTS][SEG_LVL_MAX]` is omitted — it is declared in
///   C but never read or written anywhere in the tree (verified by grep over
///   `Source/`), so carrying it would be dead weight.
///
/// `feature_data` / `feature_enabled` keep C's `int16_t` element type because
/// `find_segment_qps` deliberately relies on the narrowing store (see
/// `svtav1_encoder::segmentation::find_segment_qps`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentationParams {
    /// C `segmentation_enabled` — frame uses the segmentation tool.
    pub segmentation_enabled: bool,
    /// C `segmentation_update_map` — the map is coded in this frame.
    pub segmentation_update_map: bool,
    /// C `segmentation_temporal_update` — map coded relative to the previous
    /// frame's. SVT never sets this (see the writer's note).
    pub segmentation_temporal_update: bool,
    /// C `segmentation_update_data` — per-segment features are coded.
    pub segmentation_update_data: bool,
    /// C `feature_data[MAX_SEGMENTS][SEG_LVL_MAX]`.
    pub feature_data: [[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    /// C `feature_enabled[MAX_SEGMENTS][SEG_LVL_MAX]`.
    pub feature_enabled: [[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
    /// C `last_active_seg_id` — highest segment id with any enabled feature.
    pub last_active_seg_id: u8,
    /// C `seg_id_pre_skip` — segment id is coded before the skip flag.
    pub seg_id_pre_skip: u8,
    /// C `variance_bin_edge[MAX_SEGMENTS]` — the aq-mode-1 binning ladder.
    pub variance_bin_edge: [i16; MAX_SEGMENTS],
}

impl Default for SegmentationParams {
    /// All-zero, matching the calloc'd `FrameHeader` C starts from.
    fn default() -> Self {
        Self {
            segmentation_enabled: false,
            segmentation_update_map: false,
            segmentation_temporal_update: false,
            segmentation_update_data: false,
            feature_data: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
            feature_enabled: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
            last_active_seg_id: 0,
            seg_id_pre_skip: 0,
            variance_bin_edge: [0; MAX_SEGMENTS],
        }
    }
}

impl SegmentationParams {
    /// C `svt_aom_seg_feature_active` (deblocking_common.c:22-24).
    #[inline]
    pub fn feature_active(&self, segment_id: usize, feature_id: usize) -> bool {
        self.segmentation_enabled && self.feature_enabled[segment_id][feature_id] != 0
    }

    /// C `get_segdata` (deblocking_common.c:26-28).
    #[inline]
    pub fn seg_data(&self, segment_id: usize, feature_id: usize) -> i32 {
        i32::from(self.feature_data[segment_id][feature_id])
    }
}

/// C `seg_lvl_lf_lut[MAX_PLANES][2]` (deblocking_common.c:18-20): the
/// SEG_LVL feature that carries the loop-filter delta for
/// `[plane][dir]`, `dir` 0 = vertical edges, 1 = horizontal.
pub const SEG_LVL_LF_LUT: [[usize; 2]; 3] = [
    [SEG_LVL_ALT_LF_Y_V, SEG_LVL_ALT_LF_Y_H],
    [SEG_LVL_ALT_LF_U, SEG_LVL_ALT_LF_U],
    [SEG_LVL_ALT_LF_V, SEG_LVL_ALT_LF_V],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_tables_match_c() {
        // segmentation_params.c:16-21 verbatim.
        assert_eq!(SEGMENTATION_FEATURE_SIGNED, [1, 1, 1, 1, 1, 0, 0, 0]);
        assert_eq!(SEGMENTATION_FEATURE_BITS, [8, 6, 6, 6, 6, 3, 0, 0]);
        assert_eq!(SEGMENTATION_FEATURE_MAX, [255, 63, 63, 63, 63, 7, 0, 0]);
    }

    #[test]
    fn feature_active_requires_enabled_frame() {
        let mut seg = SegmentationParams::default();
        seg.feature_enabled[3][SEG_LVL_ALT_Q] = 1;
        seg.feature_data[3][SEG_LVL_ALT_Q] = -12;
        // C `svt_aom_seg_feature_active` ANDs with segmentation_enabled.
        assert!(!seg.feature_active(3, SEG_LVL_ALT_Q));
        seg.segmentation_enabled = true;
        assert!(seg.feature_active(3, SEG_LVL_ALT_Q));
        assert_eq!(seg.seg_data(3, SEG_LVL_ALT_Q), -12);
    }
}

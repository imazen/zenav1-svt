//! Per-block loop-filter level derivation — `Codec/deblocking_common.c`.
//!
//! Two entry points, both of which turn the frame header's loop-filter
//! state into the level a single edge is actually filtered at:
//!
//! * [`loop_filter_frame_init`] (`svt_av1_loop_filter_frame_init`,
//!   deblocking_common.c:86) precomputes the whole
//!   `[plane][segment][dir][ref_frame][mode]` table once per frame.
//! * [`filter_level_delta_lf`] (`svt_aom_get_filter_level_delta_lf`,
//!   deblocking_common.c:46) computes ONE entry on the fly, and additionally
//!   folds in the superblock's `delta_lf` — the table form has no delta_lf
//!   axis, so C keeps both.
//!
//! ## Why this is not the same as the uniform level `deblock.rs` applies
//!
//! [`crate::deblock::filter_plane`] filters a whole plane at one level. That
//! is correct for what this encoder signals, and this module says exactly
//! why rather than leaving it as an assumption:
//!
//! * `mode_ref_delta_enabled` is assigned **0 and never re-assigned**
//!   anywhere in the C encoder (`resource_coordination_process.c:389` is the
//!   only write; the eight `ref_deltas` and two `mode_deltas` beside it at
//!   `:394-401` are initialized but then multiplied by zero-gated code).
//!   With it clear, `loop_filter_frame_init` memsets every
//!   `[ref][mode]` cell of a `[plane][seg][dir]` group to one value, so ref
//!   and mode cannot change a level.
//! * `delta_lf_present` is likewise assigned 0 twice
//!   (`resource_coordination_process.c:434,439`) and **asserted** 0 at both
//!   read sites (`entropy_coding.c:3578`, `ec_process.c:84`), so the
//!   `sb_delta_lf` axis is zero.
//! * Segmentation is the ONE axis that is live: `segmentation.c:172-196`
//!   enables all four `SEG_LVL_ALT_LF_*` features and fills their data
//!   whenever the segmentation tool is on, which makes the level a function
//!   of `segment_id`.
//!
//! So the uniform-level application is faithful for `segmentation_enabled ==
//! false` and would NOT be for a segmented frame. This module is the piece
//! that has to be wired in before segmentation-with-LF-deltas can be
//! signaled, and it is written to C's full generality so that wiring is a
//! call, not a rewrite.
//!
//! Evidence: tier 1 —
//! `crates/svtav1-encoder/tests/c_parity_lf_levels.rs` drives the real
//! exported `svt_av1_loop_filter_frame_init`,
//! `svt_aom_get_filter_level_delta_lf` and `svt_aom_update_sharpness`.

use svtav1_types::restoration::MAX_SEGMENTS;
use svtav1_types::segmentation::{SEG_LVL_LF_LUT, SegmentationParams};

/// C `MAX_LOOP_FILTER` (definitions.h:1666).
pub const MAX_LOOP_FILTER: i32 = 63;
/// C `REF_FRAMES`.
pub const REF_FRAMES: usize = 8;
/// C `MAX_MODE_LF_DELTAS` — the two mode classes (ZERO_MV, MV).
pub const MAX_MODE_LF_DELTAS: usize = 2;
/// C `MAX_PLANES`.
pub const MAX_PLANES: usize = 3;
/// C `INTRA_FRAME` — reference index 0.
pub const INTRA_FRAME: usize = 0;
/// C `LAST_FRAME` — the first inter reference index.
pub const LAST_FRAME: usize = 1;

/// Which edge orientation a level applies to.
///
/// C spells this as a bare `dir` / `dir_idx` of 0 or 1 and indexes
/// `filter_level[dir]` and `seg_lvl_lf_lut[plane][dir]` with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDir {
    /// C `dir == 0`: vertical edges, `filter_level[0]`, `SEG_LVL_ALT_LF_Y_V`.
    Vert = 0,
    /// C `dir == 1`: horizontal edges, `filter_level[1]`, `SEG_LVL_ALT_LF_Y_H`.
    Horz = 1,
}

impl EdgeDir {
    /// C's `dir` index.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// C `LoopFilter` (definitions.h:1670-1687), restricted to the fields the
/// two derivations read. `combine_vert_horz_lf` and the `*_update` flags are
/// signaling-only and deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopFilterParams {
    /// C `filter_level[2]`: `[0]` vertical luma edges, `[1]` horizontal.
    pub filter_level: [i32; 2],
    /// C `filter_level_u`.
    pub filter_level_u: i32,
    /// C `filter_level_v`.
    pub filter_level_v: i32,
    /// C `sharpness_level`.
    pub sharpness_level: i32,
    /// C `mode_ref_delta_enabled`. See the module doc: the C encoder never
    /// sets this, but the derivation below honours it.
    pub mode_ref_delta_enabled: bool,
    /// C `ref_deltas[REF_FRAMES]`, indexed by `MvReferenceFrame`.
    pub ref_deltas: [i8; REF_FRAMES],
    /// C `mode_deltas[MAX_MODE_LF_DELTAS]`, indexed by
    /// `mode_lf_lut[pred_mode]`.
    pub mode_deltas: [i8; MAX_MODE_LF_DELTAS],
}

impl Default for LoopFilterParams {
    /// The all-zero state C's calloc'd `FrameHeader` starts from.
    fn default() -> Self {
        Self {
            filter_level: [0; 2],
            filter_level_u: 0,
            filter_level_v: 0,
            sharpness_level: 0,
            mode_ref_delta_enabled: false,
            ref_deltas: [0; REF_FRAMES],
            mode_deltas: [0; MAX_MODE_LF_DELTAS],
        }
    }
}

impl LoopFilterParams {
    /// C's `filt_lvl[plane]` / `filt_lvl_r[plane]` pair, selected by `dir`
    /// (`svt_av1_loop_filter_frame_init`:100-107). Only luma has a separate
    /// horizontal level; both chroma planes reuse their single one.
    #[inline]
    fn base_level(&self, plane: usize, dir: EdgeDir) -> i32 {
        match plane {
            0 => self.filter_level[dir.index()],
            1 => self.filter_level_u,
            _ => self.filter_level_v,
        }
    }
}

/// C `LoopFilterInfoN::lvl[MAX_PLANES][MAX_SEGMENTS][2][REF_FRAMES][MAX_MODE_LF_DELTAS]`
/// (definitions.h:1701).
///
/// **Not every cell is written.** With `mode_ref_delta_enabled` set, C fills
/// `[INTRA_FRAME][0]` and then `[LAST_FRAME..REF_FRAMES][0..MAX_MODE_LF_DELTAS]`
/// — `[INTRA_FRAME][1]` is skipped, because an intra block has no mode
/// delta. It also `break`s out of the plane loop entirely (not `continue`s)
/// when luma's two levels are both zero, leaving chroma untouched. Both are
/// reproduced exactly; `unwritten` is the sentinel a caller can seed to see
/// which cells C leaves alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopFilterLevels {
    lvl: [[[[[u8; MAX_MODE_LF_DELTAS]; REF_FRAMES]; 2]; MAX_SEGMENTS]; MAX_PLANES],
}

impl Default for LoopFilterLevels {
    fn default() -> Self {
        Self::filled(0)
    }
}

impl LoopFilterLevels {
    /// A table with every cell set to `v` — the pre-state
    /// `loop_filter_frame_init` writes over.
    #[must_use]
    pub fn filled(v: u8) -> Self {
        Self {
            lvl: [[[[[v; MAX_MODE_LF_DELTAS]; REF_FRAMES]; 2]; MAX_SEGMENTS]; MAX_PLANES],
        }
    }

    /// The level for one `[plane][segment][dir][ref_frame][mode]` cell.
    #[must_use]
    pub fn level(
        &self,
        plane: usize,
        segment_id: usize,
        dir: EdgeDir,
        ref_frame: usize,
        mode: usize,
    ) -> u8 {
        self.lvl[plane][segment_id][dir.index()][ref_frame][mode]
    }

    /// Row-major flattening, in C's declaration order, for differential
    /// comparison against the C struct.
    #[must_use]
    pub fn as_flat(&self) -> [u8; MAX_PLANES * MAX_SEGMENTS * 2 * REF_FRAMES * MAX_MODE_LF_DELTAS] {
        let mut out = [0u8; MAX_PLANES * MAX_SEGMENTS * 2 * REF_FRAMES * MAX_MODE_LF_DELTAS];
        let mut i = 0;
        for plane in &self.lvl {
            for seg in plane {
                for dir in seg {
                    for r in dir {
                        for &m in r {
                            out[i] = m;
                            i += 1;
                        }
                    }
                }
            }
        }
        out
    }
}

/// C's `clamp(x, 0, MAX_LOOP_FILTER)` then narrow to `uint8_t`.
#[inline]
fn clamp_level(x: i32) -> i32 {
    x.clamp(0, MAX_LOOP_FILTER)
}

/// The per-`(plane, dir, segment)` base level: the header level plus the
/// segment's LF feature data, clamped. Shared by both entry points
/// (deblocking_common.c:64-70 and :117-127 are the same three lines).
#[inline]
fn segment_adjusted_level(
    base_level: i32,
    seg: &SegmentationParams,
    plane: usize,
    dir: EdgeDir,
    segment_id: usize,
) -> i32 {
    let mut lvl_seg = clamp_level(base_level);
    let feature = SEG_LVL_LF_LUT[plane][dir.index()];
    if seg.feature_active(segment_id, feature) {
        lvl_seg = clamp_level(lvl_seg + seg.seg_data(segment_id, feature));
    }
    lvl_seg
}

/// C `svt_av1_loop_filter_frame_init` (deblocking_common.c:86-150), over
/// planes `plane_start..plane_end`.
///
/// `pre` is the table the C caller's `LoopFilterInfoN` already held; C
/// writes only the cells listed on [`LoopFilterLevels`], so anything else
/// keeps its previous value. Pass `LoopFilterLevels::default()` for C's
/// zeroed struct.
#[must_use]
pub fn loop_filter_frame_init(
    lf: &LoopFilterParams,
    seg: &SegmentationParams,
    plane_start: usize,
    plane_end: usize,
    pre: LoopFilterLevels,
) -> LoopFilterLevels {
    let mut out = pre;
    for plane in plane_start..plane_end.min(MAX_PLANES) {
        // C's guard: plane 0 with BOTH luma levels zero `break`s the loop —
        // chroma is never reached. Planes 1/2 `continue` on their own level.
        match plane {
            0 if lf.filter_level[0] == 0 && lf.filter_level[1] == 0 => break,
            1 if lf.filter_level_u == 0 => continue,
            2 if lf.filter_level_v == 0 => continue,
            _ => {}
        }

        for segment_id in 0..MAX_SEGMENTS {
            for dir in [EdgeDir::Vert, EdgeDir::Horz] {
                // NOTE: C does NOT clamp the header level here before adding
                // the segment data — it clamps only the sum. `filt_lvl` is
                // in range by construction, so `segment_adjusted_level`'s
                // leading clamp is a no-op on every reachable input; it is
                // there so an out-of-range header level cannot produce an
                // out-of-range table entry.
                let lvl_seg =
                    segment_adjusted_level(lf.base_level(plane, dir), seg, plane, dir, segment_id);
                let cell = &mut out.lvl[plane][segment_id][dir.index()];

                if !lf.mode_ref_delta_enabled {
                    // C memsets the whole [ref][mode] group.
                    for r in cell.iter_mut() {
                        r.fill(lvl_seg as u8);
                    }
                    continue;
                }

                // n_shift: 1x for levels 0..31, 2x for 32..63.
                let scale = 1 << (lvl_seg >> 5);
                cell[INTRA_FRAME][0] =
                    clamp_level(lvl_seg + i32::from(lf.ref_deltas[INTRA_FRAME]) * scale) as u8;
                // [INTRA_FRAME][1] is deliberately left alone — see the
                // LoopFilterLevels doc.
                for (ref_frame, cell_ref) in cell.iter_mut().enumerate().skip(LAST_FRAME) {
                    for (mode, out_lvl) in cell_ref.iter_mut().enumerate() {
                        let inter_lvl = lvl_seg
                            + i32::from(lf.ref_deltas[ref_frame]) * scale
                            + i32::from(lf.mode_deltas[mode]) * scale;
                        *out_lvl = clamp_level(inter_lvl) as u8;
                    }
                }
            }
        }
    }
    out
}

/// The superblock `delta_lf` state read by [`filter_level_delta_lf`].
///
/// C `delta_lf_id_lut[MAX_PLANES][2]` (deblocking_common.c:16) picks which
/// of the four values a `(plane, dir)` pair uses when `delta_lf_multi` is
/// set; otherwise every pair uses `[0]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SbDeltaLf {
    /// C `sb_delta_lf[4]`.
    pub values: [i32; 4],
    /// C `delta_lf_params.delta_lf_multi`.
    pub multi: bool,
}

/// C `mode_lf_lut[]` (deblocking_common.h:33), indexed by `PredictionMode`:
/// which of the two `mode_deltas` a prediction mode uses.
///
/// Every intra mode (0..=12) takes delta 0. Among inter modes only the two
/// global-motion ones do — `GLOBALMV` (15) and `GLOBAL_GLOBALMV` (22) — and
/// every other inter or compound mode takes delta 1. C asserts the table is
/// `MB_MODE_COUNT` long; this port fixes the length at 25 so the compiler
/// checks the same thing.
pub const MODE_LF_LUT: [usize; 25] = [
    // INTRA_MODES: DC_PRED .. PAETH_PRED, then the intra-only extras.
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    // INTER_MODES: NEARESTMV, NEARMV, GLOBALMV, NEWMV.
    1, 1, 0, 1, //
    // INTER_COMPOUND_MODES, GLOBAL_GLOBALMV == 0.
    1, 1, 1, 1, 1, 1, 0, 1,
];

/// The `mode_deltas` index for a `PredictionMode`, i.e. C's
/// `mode_lf_lut[pred_mode]`. Returns `None` for a value outside
/// `MB_MODE_COUNT` rather than reading past the table as C would.
#[must_use]
pub fn mode_lf_delta(pred_mode: usize) -> Option<usize> {
    MODE_LF_LUT.get(pred_mode).copied()
}

/// C `delta_lf_id_lut[MAX_PLANES][2]` (deblocking_common.c:16).
const DELTA_LF_ID_LUT: [[usize; 2]; MAX_PLANES] = [[0, 1], [2, 2], [3, 3]];

/// C `svt_aom_get_filter_level_delta_lf` (deblocking_common.c:46-80): the
/// level for ONE edge, including the superblock delta_lf that the
/// precomputed table has no axis for.
///
/// `ref_frame_0` is C's `MvReferenceFrame ref_frame_0`; `mode_lf_delta`
/// is C's `mode_lf_lut[pred_mode]` — the caller resolves the LUT, because
/// the mode enum lives in the entropy layer, not here. The mode delta is
/// added only for inter references, exactly as C's `ref_frame_0 >
/// INTRA_FRAME` guard does.
#[must_use]
pub fn filter_level_delta_lf(
    lf: &LoopFilterParams,
    seg: &SegmentationParams,
    dir: EdgeDir,
    plane: usize,
    sb_delta_lf: SbDeltaLf,
    segment_id: usize,
    mode_lf_delta: usize,
    ref_frame_0: usize,
) -> u8 {
    let delta_lf = if sb_delta_lf.multi {
        sb_delta_lf.values[DELTA_LF_ID_LUT[plane][dir.index()]]
    } else {
        sb_delta_lf.values[0]
    };

    let mut lvl_seg = segment_adjusted_level(
        delta_lf + lf.base_level(plane, dir),
        seg,
        plane,
        dir,
        segment_id,
    );

    if lf.mode_ref_delta_enabled {
        let scale = 1 << (lvl_seg >> 5);
        lvl_seg += i32::from(lf.ref_deltas[ref_frame_0]) * scale;
        if ref_frame_0 > INTRA_FRAME {
            lvl_seg += i32::from(lf.mode_deltas[mode_lf_delta]) * scale;
        }
        lvl_seg = clamp_level(lvl_seg);
    }
    lvl_seg as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(level: i32) -> LoopFilterParams {
        LoopFilterParams {
            filter_level: [level, level],
            filter_level_u: level,
            filter_level_v: level,
            ..LoopFilterParams::default()
        }
    }

    #[test]
    fn uniform_when_no_deltas_and_no_segments() {
        let lf = params(24);
        let seg = SegmentationParams::default();
        let t = loop_filter_frame_init(&lf, &seg, 0, MAX_PLANES, LoopFilterLevels::filled(0xFF));
        for plane in 0..MAX_PLANES {
            for s in 0..MAX_SEGMENTS {
                for dir in [EdgeDir::Vert, EdgeDir::Horz] {
                    for r in 0..REF_FRAMES {
                        for m in 0..MAX_MODE_LF_DELTAS {
                            assert_eq!(t.level(plane, s, dir, r, m), 24, "{plane} {s} {r} {m}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zero_luma_level_breaks_out_and_leaves_chroma_untouched() {
        // C `break`s, so chroma keeps whatever the struct held.
        let mut lf = params(31);
        lf.filter_level = [0, 0];
        let seg = SegmentationParams::default();
        let t = loop_filter_frame_init(&lf, &seg, 0, MAX_PLANES, LoopFilterLevels::filled(0xFF));
        assert_eq!(t.level(1, 0, EdgeDir::Vert, 0, 0), 0xFF);
        assert_eq!(t.level(2, 0, EdgeDir::Vert, 0, 0), 0xFF);
    }

    #[test]
    fn intra_frame_mode_one_is_never_written() {
        let mut lf = params(20);
        lf.mode_ref_delta_enabled = true;
        lf.ref_deltas = [1, 0, 0, 0, -1, 0, -1, -1];
        lf.mode_deltas = [0, 0];
        let seg = SegmentationParams::default();
        let t = loop_filter_frame_init(&lf, &seg, 0, 1, LoopFilterLevels::filled(0xFF));
        assert_eq!(t.level(0, 0, EdgeDir::Vert, INTRA_FRAME, 0), 21);
        assert_eq!(
            t.level(0, 0, EdgeDir::Vert, INTRA_FRAME, 1),
            0xFF,
            "C never writes [INTRA_FRAME][1]"
        );
        // ALTREF (ref 6) carries -1 * scale(1).
        assert_eq!(t.level(0, 0, EdgeDir::Vert, 6, 0), 19);
    }

    #[test]
    fn scale_doubles_above_level_31() {
        let mut lf = params(32);
        lf.mode_ref_delta_enabled = true;
        lf.ref_deltas = [1, 0, 0, 0, 0, 0, 0, 0];
        let seg = SegmentationParams::default();
        let t = loop_filter_frame_init(&lf, &seg, 0, 1, LoopFilterLevels::default());
        // 32 >> 5 == 1 -> scale 2 -> 32 + 1*2
        assert_eq!(t.level(0, 0, EdgeDir::Vert, INTRA_FRAME, 0), 34);
    }

    #[test]
    fn segment_feature_shifts_the_level() {
        let lf = params(20);
        let mut seg = SegmentationParams::default();
        seg.segmentation_enabled = true;
        seg.feature_enabled[3][SEG_LVL_LF_LUT[0][0]] = 1;
        seg.feature_data[3][SEG_LVL_LF_LUT[0][0]] = -8;
        let t = loop_filter_frame_init(&lf, &seg, 0, 1, LoopFilterLevels::default());
        assert_eq!(t.level(0, 3, EdgeDir::Vert, 0, 0), 12);
        assert_eq!(t.level(0, 2, EdgeDir::Vert, 0, 0), 20);
        // The horizontal feature is a different slot and is still off.
        assert_eq!(t.level(0, 3, EdgeDir::Horz, 0, 0), 20);
    }

    #[test]
    fn delta_lf_multi_selects_per_plane_dir_slots() {
        let lf = params(20);
        let seg = SegmentationParams::default();
        let sb = SbDeltaLf {
            values: [1, 2, 3, 4],
            multi: true,
        };
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Vert, 0, sb, 0, 0, 0),
            21
        );
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Horz, 0, sb, 0, 0, 0),
            22
        );
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Vert, 1, sb, 0, 0, 0),
            23
        );
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Horz, 2, sb, 0, 0, 0),
            24
        );
        // Without multi, every (plane, dir) reads slot 0.
        let single = SbDeltaLf {
            values: [1, 2, 3, 4],
            multi: false,
        };
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Horz, 2, single, 0, 0, 0),
            21
        );
    }

    #[test]
    fn mode_delta_applies_to_inter_references_only() {
        let mut lf = params(20);
        lf.mode_ref_delta_enabled = true;
        lf.ref_deltas = [0; REF_FRAMES];
        lf.mode_deltas = [0, 5];
        let seg = SegmentationParams::default();
        let sb = SbDeltaLf::default();
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Vert, 0, sb, 0, 1, INTRA_FRAME),
            20,
            "intra takes no mode delta"
        );
        assert_eq!(
            filter_level_delta_lf(&lf, &seg, EdgeDir::Vert, 0, sb, 0, 1, LAST_FRAME),
            25
        );
    }
}

//! Deblocking: filter-level selection, frame geometry, and decoder-exact
//! frame application.
//!
//! Level picker: C-exact port of the closed-form q-based selection
//! (`svt_av1_pick_filter_level_by_q`, Source/Lib/Codec/deblocking_filter.c:1026,
//! 8-bit KEY_FRAME specialization — see [`pick_filter_levels_key_frame`]).
//!
//! Frame application: ports the DECODER's edge walk (libaom
//! av1/common/av1_loopfilter.c set_lpf_parameters +
//! av1_filter_block_plane_vert/_horz; SVT's deblocking_filter.c carries the
//! same code) over the per-4x4 geometry recorded during the entropy walk.

use alloc::vec::Vec;
use svtav1_dsp::loop_filter as lf;

/// AV1 MAX_LOOP_FILTER.
const MAX_LOOP_FILTER: i32 = 63;

/// Frame loop-filter levels as signaled in the frame header
/// (spec 5.9.11): `[0]` = luma vertical edges, `[1]` = luma horizontal
/// edges, `[2]` = U plane, `[3]` = V plane (both edge directions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LfLevels {
    pub levels: [u8; 4],
}

impl LfLevels {
    /// True when any plane has a nonzero level (filtering happens at all).
    pub fn any(&self) -> bool {
        // Chroma filtering requires luma levels nonzero (the decoder's
        // plane loop breaks out entirely when both luma levels are 0 —
        // svt_aom_loop_filter_sb / libaom check_planes_to_loop_filter).
        self.levels[0] != 0 || self.levels[1] != 0
    }
}

/// C-exact port of `svt_av1_pick_filter_level_by_q`
/// (deblocking_filter.c:1026) for 8-bit KEY_FRAME pictures.
///
/// The C function reduces to a closed form for still/key frames:
/// - `me_based_dlf_skip` returns immediately for I_SLICE (do_y = do_uv =
///   true), so no zero-forcing applies.
/// - The min-ref-filter-level scan never runs (key frames reference
///   nothing), leaving `min_ref_filter_level[*] = MAX_LOOP_FILTER` (truthy),
///   so every level takes the `clamp(filt_guess, 0, 63)` arm.
/// - 8-bit KEY_FRAME: `filt_guess = ROUND_POWER_OF_TWO(q * 17563 - 421574,
///   18)` with `q = svt_aom_ac_quant_qtx(qindex, 0, 8-bit)` (the AC step
///   table), a linear fit of the searched level ("Keyframes: filt_guess =
///   q * 0.06699 - 1.60817" per the C comment).
/// - `filt_guess_chroma = filt_guess / 2` (C truncating integer division,
///   applied BEFORE clamping), then each level clamps to [0, 63].
///
/// Returns `[filt_guess, filt_guess, chroma, chroma]` for
/// `[level0, level1, U, V]`.
pub fn pick_filter_levels_key_frame(qindex: u8) -> LfLevels {
    let q = svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex as usize] as i32;
    // ROUND_POWER_OF_TWO(v, 18) on a possibly-negative value: C computes
    // (v + (1 << 17)) >> 18 with an arithmetic shift; Rust's i32 >> is
    // arithmetic too.
    let filt_guess = (q * 17563 - 421574 + (1 << 17)) >> 18;
    let filt_guess_chroma = filt_guess / 2;
    let y = filt_guess.clamp(0, MAX_LOOP_FILTER) as u8;
    let uv = filt_guess_chroma.clamp(0, MAX_LOOP_FILTER) as u8;
    LfLevels {
        levels: [y, y, uv, uv],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-computed values of the C closed form (AC step table x the
    /// KEY_FRAME fit): q_step(30) = 34 -> (597142 - 421574 + 131072) >> 18
    /// = 1; q_step(50) = 54 -> 657900 >> 18 = 2; q_step(63) = 70 ->
    /// 938908 >> 18 = 3. Chroma = y / 2 (truncated before clamp).
    #[test]
    fn key_frame_levels_match_c_formula() {
        assert_eq!(pick_filter_levels_key_frame(30).levels, [1, 1, 0, 0]);
        assert_eq!(pick_filter_levels_key_frame(50).levels, [2, 2, 1, 1]);
        assert_eq!(pick_filter_levels_key_frame(63).levels, [3, 3, 1, 1]);
        // qindex 0: q_step = 4 -> 70252 - 421574 = negative -> clamps to 0.
        assert_eq!(pick_filter_levels_key_frame(0).levels, [0, 0, 0, 0]);
        // Top of the table: q_step(255) = 1828 -> (32105164 - 421574 +
        // 131072) >> 18 = 121 -> clamps to 63; chroma 121/2 = 60.
        assert_eq!(pick_filter_levels_key_frame(255).levels, [63, 63, 60, 60]);
    }

    #[test]
    fn any_requires_luma() {
        assert!(!LfLevels { levels: [0, 0, 1, 1] }.any());
        assert!(LfLevels { levels: [1, 0, 0, 0] }.any());
        assert!(LfLevels::default() == LfLevels { levels: [0; 4] });
    }
}

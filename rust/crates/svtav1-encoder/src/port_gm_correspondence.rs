//! Global-motion correspondence generation — a port of
//! `correspondence_from_mvs` and `gm_compute_correspondence`
//! (`Codec/global_motion.c:239` / `:341`).
//!
//! # Which arm is reachable
//!
//! `gm_compute_correspondence` dispatches on
//! `pcs->gm_ctrls.correspondence_method`: `CORNERS` (4) runs
//! `correspondence_from_corners`, everything below it runs
//! `correspondence_from_mvs`. **Only the MV arm is reachable here**: the only
//! reachable `gm` level is 4 (levels 1 and 2 need `enc_mode <= ENC_MR`, and
//! `ENC_MR` is -1 while the port's preset is a `u8`), and level 4 sets
//! `correspondence_method` to `MV_8x8` / `MV_16x16` / `MV_32x32` by
//! resolution — never `CORNERS`. The corner arm additionally needs
//! `Codec/corner_detect.c` (FAST) and `Codec/corner_match.c` (NCC matching),
//! neither of which is ported; [`gm_compute_correspondence`] therefore returns
//! an explicit `Err` for it rather than silently producing an empty set.
//!
//! # Evidence: TIER 4, and here is exactly why
//!
//! Both C functions are `static`/plain and take a `PictureParentControlSet*`,
//! reading `pcs->pa_me_data->me_results[b64_idx]->{total_me_candidate_index,
//! me_candidate_array, me_mv_array}` plus a dozen scalar PCS fields. Driving
//! them from a shim would mean assembling a real `PictureParentControlSet`
//! with a populated `MeResults` array — far past the "calloc a shell and set
//! three fields" pattern the other shims in this file use, and a shim that
//! populated it WRONG would produce a green differential against a
//! misconfigured oracle, which is worse than no differential.
//!
//! So this is HAND-DERIVED FROM THE C SOURCE (`WORKING-ON-THIS.md` §4 tier 4)
//! and the tests below are transcription checks over an explicit
//! [`MeResultsView`], not differentials. Anyone raising this to tier 1 should
//! build the `PictureParentControlSet` + `MeResults` shim rather than trusting
//! these.
//!
//! The two index-remap tables and the ME-slot geometry are RE-USED from the
//! ME port (`inter_me::tables`) rather than re-transcribed, so the two cannot
//! drift.

use alloc::vec::Vec;

use svtav1_types::motion::Mv;

use crate::inter_me::tables::{ME_IDX_16X16_TO_PARENT_32X32, ME_IDX_85_8X8_TO_16X16};
use crate::port_md::predicates::MeCandidateRef;
use crate::port_ransac::Correspondence;

/// C `MAX_SB64_PU_COUNT_NO_8X8` (me_sb_results.h:26) — square PUs from 64x64
/// down to 16x16.
pub const MAX_SB64_PU_COUNT_NO_8X8: u8 = 21;
/// C `MAX_SB64_PU_COUNT_WO_16X16` (me_sb_results.h:27) — square PUs for the
/// 64x64 and 32x32 sizes only.
pub const MAX_SB64_PU_COUNT_WO_16X16: u8 = 5;

/// C `CorrespondenceMethod` (pcs.h:504).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CorrespondenceMethod {
    #[cfg_attr(test, allow(dead_code))]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "C variant the port never selects (plan 1.4)")
    )]
    Mv64x64 = 0,
    Mv32x32 = 1,
    Mv16x16 = 2,
    Mv8x8 = 3,
    Corners = 4,
}

/// C `GM_FULL` / `GM_DOWN` / `GM_DOWN16` (definitions.h:257).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GmDownsampleLevel {
    Full = 0,
    Down = 1,
    Down16 = 2,
}

impl GmDownsampleLevel {
    /// The right-shift C applies to every coordinate
    /// (`global_motion.c:317-319`).
    #[inline]
    const fn shift(self) -> u32 {
        match self {
            GmDownsampleLevel::Full => 0,
            GmDownsampleLevel::Down => 1,
            GmDownsampleLevel::Down16 => 2,
        }
    }
}

/// The `pcs->pa_me_data->me_results` slice this function reads, flattened.
///
/// C indexes `me_results[b64_idx]->me_candidate_array[n_idx * max_cand]`, and
/// `me_results[b64_idx]->me_mv_array` at
/// `n_idx * max_refs + (list_idx ? max_l0 : 0) + ref_idx`. The same addressing
/// is spelled out here so a caller wiring real ME data cannot get the stride
/// wrong silently.
pub struct MeResultsView<'a> {
    /// `me_results[b64].total_me_candidate_index[n_idx]`, `pu_count` entries
    /// per b64.
    pub total_me_candidate_index: &'a [u8],
    /// `me_results[b64].me_candidate_array`, `pu_count * max_cand` entries per
    /// b64.
    pub me_candidate_array: &'a [MeCandidateRef],
    /// `me_results[b64].me_mv_array`, `pu_count * max_refs` entries per b64.
    pub me_mv_array: &'a [Mv],
    /// PUs per b64 (the `n_idx` domain; 85 for a full SB64 tree).
    pub pu_count: usize,
    pub max_cand: usize,
    pub max_refs: usize,
    pub max_l0: usize,
}

impl MeResultsView<'_> {
    #[inline]
    fn total(&self, b64: usize, n_idx: usize) -> usize {
        usize::from(self.total_me_candidate_index[b64 * self.pu_count + n_idx])
    }
    #[inline]
    fn cand(&self, b64: usize, n_idx: usize, i: usize) -> MeCandidateRef {
        self.me_candidate_array[b64 * self.pu_count * self.max_cand + n_idx * self.max_cand + i]
    }
    #[inline]
    fn mv(&self, b64: usize, n_idx: usize, list_idx: u8, ref_idx: u8) -> Mv {
        let slot = if list_idx != 0 { self.max_l0 } else { 0 } + usize::from(ref_idx);
        self.me_mv_array[b64 * self.pu_count * self.max_refs + n_idx * self.max_refs + slot]
    }
}

/// The picture-level fields `correspondence_from_mvs` reads.
#[derive(Debug, Clone, Copy)]
pub struct GmPictureGeometry {
    pub aligned_width: u32,
    pub aligned_height: u32,
    /// `scs->b64_size` — 64 in every shipping configuration.
    pub b64_size: u8,
    pub enable_me_8x8: bool,
    pub enable_me_16x16: bool,
    pub gm_downsample_level: GmDownsampleLevel,
}

impl GmPictureGeometry {
    /// C's `pic_b64_width` / `pic_b64_height` (`global_motion.c:252`).
    #[inline]
    pub fn b64_dims(&self) -> (u32, u32) {
        let bs = u32::from(self.b64_size);
        (
            self.aligned_width.div_ceil(bs),
            self.aligned_height.div_ceil(bs),
        )
    }
}

/// Port of `correspondence_from_mvs` (global_motion.c:239).
///
/// Walks every b64 in raster order and, inside each, every block of the
/// requested size in raster order, emitting one correspondence per block that
/// has an ME MV for `(list_idx, ref_idx)`.
///
/// Details that are easy to get wrong and are transcribed literally:
///
/// * **`starting_n_idx` is a hardcoded table**, not a formula:
///   `MV_64x64 -> 0`, `MV_32x32 -> 1`, `MV_16x16 -> 5`, `MV_8x8 -> 21`.
/// * **the index remap is conditional and CASCADES.** With `enable_me_8x8`
///   false, an `n_idx >= MAX_SB64_PU_COUNT_NO_8X8` is remapped through
///   `me_idx_85_8x8_to_16x16_conversion`; then, if `enable_me_16x16` is ALSO
///   false, the remapped index is remapped AGAIN through
///   `me_idx_16x16_to_parent_32x32_conversion` when it is still
///   `>= MAX_SB64_PU_COUNT_WO_16X16`. Doing only the first remap samples the
///   wrong MV.
/// * **bipred candidates are skipped** (`direction == 2`), and the
///   list-0/list-1 arms match on DIFFERENT fields (`ref0_list`/`ref_idx_l0`
///   vs `ref1_list`/`ref_idx_l1`) — but both then read the SAME
///   `me_mv_array` slot, computed from the requested `(list_idx, ref_idx)`,
///   not from the candidate's own.
/// * **the out-of-frame test uses the block's TOP-LEFT only** and is `>=`
///   against the ALIGNED dimensions, so a block that starts inside but
///   extends past the edge is still emitted.
/// * **the downsample shift is applied AFTER adding the MV**, to all four
///   coordinates, and it is an arithmetic shift on a value that can be
///   negative once the MV is added.
pub fn correspondence_from_mvs(
    me: &MeResultsView<'_>,
    geom: &GmPictureGeometry,
    method: CorrespondenceMethod,
    list_idx: u8,
    ref_idx: u8,
) -> Vec<Correspondence> {
    debug_assert!(method != CorrespondenceMethod::Corners);
    let mv_search_lvl = method as u32;
    let block_size = (64u32 >> mv_search_lvl) as i32;
    let blocks_per_line = 1u32 << mv_search_lvl;
    let num_blocks_per_sb = blocks_per_line * blocks_per_line;
    let starting_n_idx: u32 = match method {
        CorrespondenceMethod::Mv64x64 => 0,
        CorrespondenceMethod::Mv32x32 => 1,
        CorrespondenceMethod::Mv16x16 => 5,
        CorrespondenceMethod::Mv8x8 => 21,
        CorrespondenceMethod::Corners => unreachable!(),
    };

    let (pic_b64_width, pic_b64_height) = geom.b64_dims();
    let b64_size = i32::from(geom.b64_size);
    let shift = geom.gm_downsample_level.shift();

    let mut out = Vec::new();
    for b64_y in 0..pic_b64_height {
        for b64_x in 0..pic_b64_width {
            let b64_idx = (b64_y * pic_b64_width + b64_x) as usize;
            for i in 0..num_blocks_per_sb {
                let bx = (b64_x as i32) * b64_size + (i % blocks_per_line) as i32 * block_size;
                let by = (b64_y as i32) * b64_size + (i / blocks_per_line) as i32 * block_size;
                if bx >= geom.aligned_width as i32 || by >= geom.aligned_height as i32 {
                    continue;
                }

                let mut n_idx = (starting_n_idx + i) as u8;
                if !geom.enable_me_8x8 {
                    if n_idx >= MAX_SB64_PU_COUNT_NO_8X8 {
                        n_idx = ME_IDX_85_8X8_TO_16X16[(n_idx - MAX_SB64_PU_COUNT_NO_8X8) as usize];
                    }
                    if !geom.enable_me_16x16 && n_idx >= MAX_SB64_PU_COUNT_WO_16X16 {
                        n_idx = ME_IDX_16X16_TO_PARENT_32X32
                            [(n_idx - MAX_SB64_PU_COUNT_WO_16X16) as usize];
                    }
                }
                let n_idx = usize::from(n_idx);

                let total = me.total(b64_idx, n_idx);
                let mut found: Option<Mv> = None;
                for c in 0..total {
                    let cand = me.cand(b64_idx, n_idx, c);
                    debug_assert!(cand.direction <= 2);
                    // Bipred candidates never contribute.
                    if cand.direction == 2 {
                        continue;
                    }
                    let hit = if cand.direction == 0 {
                        list_idx == cand.ref0_list && ref_idx == cand.ref_idx_l0
                    } else {
                        list_idx == cand.ref1_list && ref_idx == cand.ref_idx_l1
                    };
                    if hit {
                        found = Some(me.mv(b64_idx, n_idx, list_idx, ref_idx));
                        break;
                    }
                }

                if let Some(mv) = found {
                    out.push(Correspondence {
                        x: bx >> shift,
                        y: by >> shift,
                        rx: (bx + i32::from(mv.x)) >> shift,
                        ry: (by + i32::from(mv.y)) >> shift,
                    });
                }
            }
        }
    }
    out
}

/// Why [`gm_compute_correspondence`] could not produce a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrespondenceError {
    /// `correspondence_method == CORNERS`. Unreachable at every preset this
    /// port can express (see the module doc), and it needs
    /// `Codec/corner_detect.c` + `Codec/corner_match.c`, neither ported.
    CornersUnported,
}

/// Port of `gm_compute_correspondence` (global_motion.c:341) — the dispatcher.
///
/// Returns `Err(CornersUnported)` for the corner arm rather than an empty
/// set: an empty correspondence list is a VALID result that makes RANSAC
/// early-out to the identity model, so returning one for an unported path
/// would be indistinguishable from "this frame genuinely has no matches" —
/// exactly the silent-wrong-answer shape `WORKING-ON-THIS.md` §6 forbids.
pub fn gm_compute_correspondence(
    me: &MeResultsView<'_>,
    geom: &GmPictureGeometry,
    method: CorrespondenceMethod,
    list_idx: u8,
    ref_idx: u8,
) -> Result<Vec<Correspondence>, CorrespondenceError> {
    if method == CorrespondenceMethod::Corners {
        return Err(CorrespondenceError::CornersUnported);
    }
    Ok(correspondence_from_mvs(me, geom, method, list_idx, ref_idx))
}

#[cfg(test)]
mod tests;

//! The CDEF **search** signal derivation of `Source/Lib/Codec/enc_mode_config.c`:
//! the `cdef_search_level` ladders of the three `svt_aom_sig_deriv_multi_processes_*`
//! arms, and the `set_cdef_search_controls` table (`:891`) they all feed.
//!
//! # Why this exists
//!
//! Before it, the port carried the RESOLVED **allintra** candidate sets per
//! preset, flattened into `crate::cdef::cdef_search_cfg_for_preset` — correct
//! for the still envelope and nothing else. A video-mode key frame takes the
//! `_default` arm, whose ladder is a different function of the preset
//! (`enc_mode <= ENC_M7 -> is_base ? 5 : 6`, where allintra gives 7 at M6 and
//! 10 at M7+), so the flattening silently gave a video key frame the wrong
//! candidate set — and above M6 no search at all.
//!
//! # Evidence
//!
//! **Tier 1.** `set_cdef_search_controls` is file-`static`, but the EXPORTED
//! `svt_aom_sig_deriv_multi_processes_{default,allintra}` reach it and leave
//! the result in `ppcs->cdef_search_ctrls`, which `shims/cdef_shims.c` reads
//! back. So `tests/c_parity_cdef_search_ctrls.rs` drives the real C ladder AND
//! the real C controls table — see `docs/WORKING-ON-THIS.md` §4, and
//! `shims/dlf_shims.c` for the same route on the deblock ladder.

use super::ResolutionRange;
use super::enc_mode::*;

/// C `TOTAL_STRENGTHS` = `CDEF_PRI_STRENGTHS * CDEF_SEC_STRENGTHS` = 16 * 4
/// (`cdef.h:50`) — the length of every candidate array in
/// [`CdefSearchControls`].
pub const TOTAL_STRENGTHS: usize = 64;

/// C `pf_gi[16]` (`enc_mode_config.c:16`): the primary-filter strength ids,
/// i.e. `pri_strength_index * CDEF_SEC_STRENGTHS` (sec code 0).
pub const PF_GI: [u8; 16] = [0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60];

/// C `DEFAULT` — the `static_config` "not overridden, derive it" sentinel.
pub const CONFIG_DEFAULT: i32 = -1;

/// C `CdefSearchControls` (`pcs.h:554`).
///
/// The arrays are sized `TOTAL_STRENGTHS` like C's. C writes only the entries
/// a level actually uses and leaves the rest at whatever the control set
/// already held (zero on a freshly allocated `PictureParentControlSet`), so
/// [`Default`] is all-zero to match, and **only indices below
/// `first_pass_fs_num` / `default_second_pass_fs_num` are meaningful**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdefSearchControls {
    /// `enabled`
    pub enabled: u8,
    /// `first_pass_fs_num` — primary filters searched in pass 1 (luma+chroma).
    pub first_pass_fs_num: u8,
    /// `default_first_pass_fs[TOTAL_STRENGTHS]`
    pub default_first_pass_fs: [u8; TOTAL_STRENGTHS],
    /// `default_second_pass_fs_num`
    pub default_second_pass_fs_num: u8,
    /// `default_second_pass_fs[TOTAL_STRENGTHS]`
    pub default_second_pass_fs: [u8; TOTAL_STRENGTHS],
    /// `default_first_pass_fs_uv[TOTAL_STRENGTHS]` — `-1` masks a slot out of
    /// the chroma search.
    pub default_first_pass_fs_uv: [i8; TOTAL_STRENGTHS],
    /// `default_second_pass_fs_uv[TOTAL_STRENGTHS]`
    pub default_second_pass_fs_uv: [i8; TOTAL_STRENGTHS],
    /// `use_reference_cdef_fs`
    pub use_reference_cdef_fs: i8,
    /// `subsampling_factor` — 1, 2 or 4 rows.
    pub subsampling_factor: u8,
    /// `search_best_ref_fs`
    pub search_best_ref_fs: u8,
    /// `skip_th`
    pub skip_th: u8,
    /// `uv_from_y`
    pub uv_from_y: bool,
    /// `use_qp_strength` — bypass the search and take
    /// `svt_pick_cdef_from_qp`.
    pub use_qp_strength: bool,
    /// `pred_y_f` — the packed luma strength to USE without searching, set by
    /// [`update_cdef_filters_on_ref_info`] when it takes the
    /// `use_reference_cdef_fs` arm. Only meaningful while
    /// `use_reference_cdef_fs != 0`.
    pub pred_y_f: i8,
    /// `pred_uv_f` — the chroma twin of [`Self::pred_y_f`].
    pub pred_uv_f: i8,
}

impl Default for CdefSearchControls {
    fn default() -> Self {
        Self {
            enabled: 0,
            first_pass_fs_num: 0,
            default_first_pass_fs: [0; TOTAL_STRENGTHS],
            default_second_pass_fs_num: 0,
            default_second_pass_fs: [0; TOTAL_STRENGTHS],
            default_first_pass_fs_uv: [0; TOTAL_STRENGTHS],
            default_second_pass_fs_uv: [0; TOTAL_STRENGTHS],
            use_reference_cdef_fs: 0,
            subsampling_factor: 0,
            search_best_ref_fs: 0,
            skip_th: 0,
            uv_from_y: false,
            pred_y_f: 0,
            pred_uv_f: 0,
            use_qp_strength: false,
        }
    }
}

/// C `set_cdef_search_controls` (`enc_mode_config.c:891`). static — reached
/// through the exported `svt_aom_sig_deriv_multi_processes_*` (tier 1).
///
/// `is_base` is C's `frame_is_boosted(pcs)` = `frame_is_kf_gf_arf` = intra-only
/// OR ARF OR GF update (`enc_mode_config.h:100-111`) — NOT
/// `temporal_layer_index == 0`, which is what the *ladders* call `is_base`.
/// `is_not_highest_layer` is `!frame_is_leaf(pcs)` = `update_type !=
/// LF_UPDATE` (`:113`). Both are TRUE for a KEY frame.
///
/// Returns `None` where C asserts (`default: assert(0)`), i.e. level > 10.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn set_cdef_search_controls(
    cdef_search_level: u8,
    is_base: bool,
    is_not_highest_layer: bool,
) -> Option<CdefSearchControls> {
    let mut c = CdefSearchControls::default();
    // C's shared tail for levels 1..=9: build the second-pass list by walking
    // the first-pass list outer and the deltas inner, mirror the first-pass
    // list into the chroma mask, and mask the whole chroma second pass out.
    // `sf_deltas` empty = level 9 (primary only).
    let fill =
        |c: &mut CdefSearchControls, first: &[usize], sf_deltas: &[u8], uv_first_real: bool| {
            c.enabled = 1;
            c.first_pass_fs_num = first.len() as u8;
            c.default_second_pass_fs_num = (first.len() * sf_deltas.len()) as u8;
            for (slot, &pf) in first.iter().enumerate() {
                c.default_first_pass_fs[slot] = PF_GI[pf];
            }
            let mut sf_idx = 0usize;
            for &pf in first {
                for &d in sf_deltas {
                    c.default_second_pass_fs[sf_idx] = PF_GI[pf] + d;
                    sf_idx += 1;
                }
            }
            for slot in 0..first.len() {
                c.default_first_pass_fs_uv[slot] = if uv_first_real {
                    c.default_first_pass_fs[slot] as i8
                } else {
                    -1
                };
            }
            for slot in 0..c.default_second_pass_fs_num as usize {
                c.default_second_pass_fs_uv[slot] = -1;
            }
        };

    match cdef_search_level {
        // OFF. C writes only these four; the candidate arrays keep their
        // previous contents, which `enabled = 0` makes unreadable.
        0 => {
            c.enabled = 0;
            c.use_reference_cdef_fs = 0;
            c.skip_th = 0;
            c.uv_from_y = false;
        }
        // pf {0..15}, sf {+1,+2,+3}. The ONLY level whose chroma second pass
        // is real — C's `= default_second_pass_fs[i]` here, `= -1` from
        // level 2 on.
        1 => {
            fill(
                &mut c,
                &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                &[1, 2, 3],
                true,
            );
            for slot in 0..c.default_second_pass_fs_num as usize {
                c.default_second_pass_fs_uv[slot] = c.default_second_pass_fs[slot] as i8;
            }
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = 0;
            c.subsampling_factor = 1;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,1,2,4,5,6,8,9,10,12,13,14}, sf {+1,+2,+3}.
        2 => {
            fill(
                &mut c,
                &[0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14],
                &[1, 2, 3],
                true,
            );
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = 0;
            c.subsampling_factor = 1;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,4,8,12,15}, sf {+1,+2,+3}.
        3 => {
            fill(&mut c, &[0, 4, 8, 12, 15], &[1, 2, 3], true);
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = 0;
            c.subsampling_factor = 1;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,7,15}, sf {+1,+2,+3}.
        4 => {
            fill(&mut c, &[0, 7, 15], &[1, 2, 3], true);
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = 0;
            c.subsampling_factor = 1;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,7,15}, sf {+2}.
        5 => {
            fill(&mut c, &[0, 7, 15], &[2], true);
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = u8::from(!is_not_highest_layer);
            c.subsampling_factor = 1;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,15}, sf {+2}. From here C writes the chroma masks by hand,
        // including a THIRD first-pass uv slot set to -1 ("when using
        // search_best_ref_fs, set at least 3 filters") that lies beyond
        // `first_pass_fs_num` — reproduced so the control set matches C
        // field for field.
        6 => {
            fill(&mut c, &[0, 15], &[2], true);
            c.default_first_pass_fs_uv[2] = -1;
            c.use_reference_cdef_fs = 0;
            c.search_best_ref_fs = u8::from(!is_not_highest_layer);
            c.subsampling_factor = 4;
            c.skip_th = 0;
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,15}, sf {+2}; the first level to consult the reference
        // frames' strengths.
        7 => {
            fill(&mut c, &[0, 15], &[2], true);
            c.default_first_pass_fs_uv[2] = -1;
            c.use_reference_cdef_fs = i8::from(!is_not_highest_layer);
            c.search_best_ref_fs = u8::from(!is_base);
            c.subsampling_factor = 4;
            c.skip_th = if is_base { 0 } else { 80 };
            c.uv_from_y = false;
            c.use_qp_strength = false;
        }
        // pf {0,15}, sf {+2}, chroma copied from luma.
        8 => {
            fill(&mut c, &[0, 15], &[2], false);
            c.default_first_pass_fs_uv[2] = -1;
            c.use_reference_cdef_fs = i8::from(!is_base);
            c.search_best_ref_fs = u8::from(!is_base);
            c.subsampling_factor = 4;
            c.skip_th = if is_base { 0 } else { 80 };
            c.uv_from_y = true;
            c.use_qp_strength = false;
        }
        // Primary-only: no secondary candidates at all.
        9 => {
            fill(&mut c, &[0, 15], &[], false);
            c.default_first_pass_fs_uv[2] = -1;
            c.default_second_pass_fs_uv[0] = -1;
            c.default_second_pass_fs_uv[1] = -1;
            c.use_reference_cdef_fs = i8::from(!is_base);
            c.search_best_ref_fs = u8::from(!is_base);
            c.subsampling_factor = 4;
            c.skip_th = if is_base { 0 } else { 80 };
            c.uv_from_y = true;
            c.use_qp_strength = false;
        }
        // The qp fast path (`svt_pick_cdef_from_qp`): no candidate arrays are
        // written at all, so they keep the control set's prior contents.
        10 => {
            c.enabled = 1;
            c.use_reference_cdef_fs = 0;
            c.use_qp_strength = true;
            c.skip_th = if is_base { 0 } else { 80 };
        }
        // C: `default: assert(0)`.
        _ => return None,
    }

    // "If chroma filters will be copied from luma, set chroma filters to -1 to
    // avoid testing" (enc_mode_config.c:1188-1196). Levels 8/9 already wrote
    // -1, so this is a no-op there; it exists for a config-forced level.
    if c.uv_from_y && !c.use_qp_strength {
        for slot in 0..c.first_pass_fs_num as usize {
            c.default_first_pass_fs_uv[slot] = -1;
        }
        for slot in 0..c.default_second_pass_fs_num as usize {
            c.default_second_pass_fs_uv[slot] = -1;
        }
    }
    Some(c)
}

/// The `cdef_search_level` ladder of C
/// `svt_aom_sig_deriv_multi_processes_default` (`enc_mode_config.c:2083`) —
/// the arm EVERY video-mode picture takes, key frame included.
///
/// `is_base` here is the ladder's own `pcs->temporal_layer_index == 0`, which
/// is NOT the `is_base` [`set_cdef_search_controls`] uses.
#[must_use]
pub fn cdef_search_level_default(
    enc_mode: i8,
    is_base: bool,
    seq_cdef_level: u8,
    allow_intrabc: bool,
    config_cdef_level: i32,
) -> u8 {
    if seq_cdef_level == 0 || allow_intrabc {
        0
    } else if config_cdef_level != CONFIG_DEFAULT {
        // C casts through int8_t.
        config_cdef_level as i8 as u8
    } else if enc_mode <= MR {
        1
    } else if enc_mode <= M2 {
        2
    } else if enc_mode <= M5 {
        5
    } else if enc_mode <= M7 {
        if is_base { 5 } else { 6 }
    } else {
        7
    }
}

/// The `cdef_search_level` ladder of C
/// `svt_aom_sig_deriv_multi_processes_allintra` (`enc_mode_config.c:2396`) —
/// the arm a still/AVIF encode takes.
///
/// The port previously carried this ladder's RESOLVED candidate sets per
/// preset (`crate::cdef::cdef_search_cfg_for_preset` +
/// `allintra_preset_uses_cdef_search`); this is the same mapping written as
/// the C function it came from, and `cdef.rs`'s
/// `allintra_flattening_matches_the_ladder` test pins the two together so the
/// still envelope cannot move.
#[must_use]
pub fn cdef_search_level_allintra(
    enc_mode: i8,
    fast_decode: u8,
    input_resolution: ResolutionRange,
    seq_cdef_level: u8,
    allow_intrabc: bool,
    config_cdef_level: i32,
) -> u8 {
    if seq_cdef_level == 0 || allow_intrabc {
        0
    } else if config_cdef_level != CONFIG_DEFAULT {
        config_cdef_level as i8 as u8
    } else if fast_decode == 0 || input_resolution <= ResolutionRange::R360p {
        if enc_mode <= MR {
            1
        } else if enc_mode <= M0 {
            2
        } else if enc_mode <= M3 {
            3
        } else if enc_mode <= M5 {
            5
        } else if enc_mode <= M6 {
            7
        } else {
            // "For fd1/fd2, disable CDEF search if fd0 uses level 10 or 0."
            10
        }
    } else if enc_mode <= M3 {
        3
    } else if enc_mode <= M5 {
        5
    } else if enc_mode <= M7 {
        7
    } else {
        0
    }
}

/// One reference picture's CHOSEN CDEF strengths, as
/// `EbReferenceObject::ref_cdef_strengths[2][..num]`
/// (`reference_object.h:51-52`, written by `rest_process.c:207-210` from the
/// frame header's `cdef_y_strength[]` / `cdef_uv_strength[]`).
///
/// Packed `gi` values (`pri * 4 + sec_code`), the same domain
/// `default_first_pass_fs` is in — not a (pri, sec) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefCdefStrengths {
    /// `ref_cdef_strengths[0][0]` — the luma strength of slot 0. This is the
    /// one C reads on the `search_best_ref_fs` path, which indexes `[0][0]`
    /// literally.
    pub y0: u8,
    /// `ref_cdef_strengths[1][0]` — the chroma strength of slot 0.
    pub uv0: u8,
    /// `min(ref_cdef_strengths[0][..num])` — the `use_reference_cdef_fs` path
    /// walks EVERY slot, not just slot 0, so the two extremes are carried
    /// separately rather than assuming `num == 1`. With one strength (every
    /// `cdef_bits == 0` frame) both equal [`Self::y0`].
    pub y_min: u8,
    /// `max(ref_cdef_strengths[0][..num])` — see [`Self::y_min`].
    pub y_max: u8,
}

/// What [`update_cdef_filters_on_ref_info`] decided beyond the controls it
/// mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RefCdefUpdate {
    /// C sets `pcs->ppcs->cdef_level = 0` — CDEF off for this frame — when the
    /// reference-derived prediction is "no filtering", or when the
    /// `search_best_ref_fs` arm ends with a single candidate.
    pub force_cdef_off: bool,
}

/// C `update_cdef_filters_on_ref_info` (`md_config_process.c:681-772`) —
/// static, tier 4.
///
/// Rewrites the CDEF candidate set from the REFERENCE pictures' own chosen
/// strengths. It is not a threshold or a bias: on the `use_reference_cdef_fs`
/// arm it removes the search entirely and hands the frame the reference's
/// strength, and on the `search_best_ref_fs` arm it replaces the level's
/// candidate list with (default, ref-l0, ref-l1).
///
/// **Why an inter frame reaches it and no key frame can.**
/// `set_cdef_search_controls` level 5 sets
/// `search_best_ref_fs = is_not_highest_layer ? 0 : 1`
/// (`enc_mode_config.c:1073`), and `is_not_highest_layer` is
/// `update_type != LF_UPDATE` — true for every KEY frame. So this whole
/// function is unreachable on the still/key envelope and reachable on the
/// first inter frame of a flat low-delay GOP, which is exactly where it was
/// found: it is ALL of the residual CDEF divergence in
/// `docs/INTER-ENCODE-PLAN.md` §1q.
///
/// C's caller (`md_config_process.c:983-985`) invokes it only when
/// `use_reference_cdef_fs || search_best_ref_fs`, and only after
/// `me_based_cdef_skip` declined to switch CDEF off; that skip needs ME
/// distortion this pipeline does not produce and is NOT modelled here — see
/// the plan doc.
///
/// `ref_l1` is `None` when the picture is not a B slice or
/// `ref_list1_count_try == 0`, which is exactly C's guard.
pub fn update_cdef_filters_on_ref_info(
    c: &mut CdefSearchControls,
    ref_l0: RefCdefStrengths,
    ref_l1: Option<RefCdefStrengths>,
) -> RefCdefUpdate {
    let mut out = RefCdefUpdate::default();
    if c.use_reference_cdef_fs != 0 {
        // Luma: the midpoint of the LOWEST and HIGHEST strength across both
        // reference lists, over EVERY slot of each — which is why
        // `RefCdefStrengths` carries `y_min`/`y_max` rather than slot 0 alone.
        // C's seeds (`TOTAL_STRENGTHS - 1` / `0`) only matter when a list has
        // no strengths at all, which cannot happen here: the caller cannot
        // build a `RefCdefStrengths` from an empty reference.
        let mut lowest = ref_l0.y_min;
        let mut highest = ref_l0.y_max;
        if let Some(l1) = ref_l1 {
            lowest = lowest.min(l1.y_min);
            highest = highest.max(l1.y_max);
        }
        let mid = ((u16::from(lowest) + u16::from(highest)) / 2).min(63);
        c.pred_y_f = mid as i8;
        c.pred_uv_f = 0;
        c.first_pass_fs_num = 0;
        c.default_second_pass_fs_num = 0;
        if c.pred_y_f == 0 && c.pred_uv_f == 0 {
            out.force_cdef_off = true;
        }
        return out;
    }
    if c.search_best_ref_fs == 0 {
        return out;
    }

    c.first_pass_fs_num = 1;
    c.default_second_pass_fs_num = 0;

    // Add list 0's filter, if it is not already the default.
    if ref_l0.y0 != c.default_first_pass_fs[0] {
        c.default_first_pass_fs[1] = ref_l0.y0;
        c.first_pass_fs_num += 1;
    }

    if let Some(l1) = ref_l1 {
        // Add list 1's, if different from BOTH the default and the last added.
        if l1.y0 != c.default_first_pass_fs[0]
            && l1.y0 != c.default_first_pass_fs[c.first_pass_fs_num as usize - 1]
        {
            c.default_first_pass_fs[c.first_pass_fs_num as usize] = l1.y0;
            c.first_pass_fs_num += 1;
            if ref_l0.uv0 == u8::try_from(c.default_first_pass_fs_uv[0]).unwrap_or(u8::MAX)
                && l1.uv0 == u8::try_from(c.default_first_pass_fs_uv[0]).unwrap_or(u8::MAX)
            {
                c.default_first_pass_fs_uv[0] = -1;
                c.default_first_pass_fs_uv[1] = -1;
            }
        } else if c.first_pass_fs_num == 2 && ref_l0.y0 == l1.y0 {
            // BOTH lists chose the same filter: skip the search entirely and
            // take it. This is the arm the campaign's first inter frame lands
            // on — every DPB slot still holds the key frame, so list 0 and
            // list 1 ARE the same picture.
            c.use_reference_cdef_fs = 1;
            c.pred_y_f = ref_l0.y0 as i8;
            c.pred_uv_f = ((u16::from(ref_l0.uv0) + u16::from(l1.uv0)) / 2).min(63) as i8;
            c.first_pass_fs_num = 0;
            c.default_second_pass_fs_num = 0;
        }
    } else if ref_l0.uv0 == u8::try_from(c.default_first_pass_fs_uv[0]).unwrap_or(u8::MAX) {
        c.default_first_pass_fs_uv[0] = -1;
        c.default_first_pass_fs_uv[1] = -1;
    }

    // "Set cdef to off if pred luma is" — C's comment; the test is on the
    // candidate COUNT, not on a strength.
    if c.first_pass_fs_num == 1 {
        out.force_cdef_off = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two ladders disagree at every preset a still encode can reach —
    /// which is the whole reason the video arm needed its own wiring. KEY
    /// frame, so the default arm's `is_base` is true.
    #[test]
    fn default_and_allintra_ladders_differ_on_a_key_frame() {
        let allintra: Vec<u8> = (0..=13)
            .map(|p| {
                cdef_search_level_allintra(p, 0, ResolutionRange::R240p, 1, false, CONFIG_DEFAULT)
            })
            .collect();
        let default: Vec<u8> = (0..=13)
            .map(|p| cdef_search_level_default(p, true, 1, false, CONFIG_DEFAULT))
            .collect();
        assert_eq!(
            allintra,
            vec![2, 3, 3, 3, 5, 5, 7, 10, 10, 10, 10, 10, 10, 10]
        );
        assert_eq!(default, vec![2, 2, 2, 5, 5, 5, 5, 5, 7, 7, 7, 7, 7, 7]);
    }

    /// The reference cell of chunk C1a (`docs/INTER-ENCODE-PLAN.md`): a
    /// video-mode key frame at preset 6 searches level 5 (three primary
    /// candidates, no row subsampling), where the still path searches level 7
    /// (two candidates, every 4th row).
    #[test]
    fn video_key_frame_p6_is_level_5() {
        let lvl = cdef_search_level_default(6, true, 1, false, CONFIG_DEFAULT);
        assert_eq!(lvl, 5);
        let c = set_cdef_search_controls(lvl, true, true).unwrap();
        assert_eq!(c.enabled, 1);
        assert!(!c.use_qp_strength);
        assert_eq!(c.first_pass_fs_num, 3);
        assert_eq!(&c.default_first_pass_fs[..3], &[0, 28, 60]);
        assert_eq!(c.default_second_pass_fs_num, 3);
        assert_eq!(&c.default_second_pass_fs[..3], &[2, 30, 62]);
        assert_eq!(c.subsampling_factor, 1);
        assert_eq!(c.search_best_ref_fs, 0);
        assert_eq!(c.skip_th, 0);
    }

    /// Every level C can assign is representable; 11+ is C's `assert(0)`.
    #[test]
    fn level_domain_is_zero_through_ten() {
        for lvl in 0..=10u8 {
            assert!(
                set_cdef_search_controls(lvl, true, true).is_some(),
                "level {lvl}"
            );
        }
        assert!(set_cdef_search_controls(11, true, true).is_none());
    }

    /// Level 1 is the only one whose chroma second pass carries real
    /// candidates.
    #[test]
    fn only_level_one_keeps_a_real_chroma_second_pass() {
        for lvl in 1..=9u8 {
            let c = set_cdef_search_controls(lvl, true, true).unwrap();
            let n = c.default_second_pass_fs_num as usize;
            let real = (0..n).any(|i| c.default_second_pass_fs_uv[i] >= 0);
            assert_eq!(real, lvl == 1, "level {lvl}");
        }
    }

    // -----------------------------------------------------------------
    // update_cdef_filters_on_ref_info (md_config_process.c:681-772)
    // -----------------------------------------------------------------

    /// The arm the campaign's first INTER frame takes, and the exact numbers
    /// it takes it with.
    ///
    /// `gradient 64x64 q40 p6`, frame 1 of a 2-frame low-delay-P GOP: level 5,
    /// `is_not_highest_layer = false` (an LF_UPDATE), so
    /// `search_best_ref_fs = 1`. Every DPB slot still holds the key frame, so
    /// list 0 and list 1 are the SAME picture and both report the key frame's
    /// signalled strengths: packed y = 2 (`pri 0, sec 2`) and uv = 28
    /// (`pri 7, sec 0`). C must then take `use_reference_cdef_fs` and hand the
    /// frame exactly those, with no search — which is what the C encoder's own
    /// frame-1 header says (`docs/INTER-ENCODE-PLAN.md` §1r).
    #[test]
    fn ref_info_takes_the_use_reference_arm_when_both_lists_agree() {
        let mut c = set_cdef_search_controls(
            5, /*is_base=*/ false, /*is_not_highest_layer=*/ false,
        )
        .expect("level 5");
        assert_eq!(c.search_best_ref_fs, 1, "level 5 on a non-leaf-layer frame");
        let r = RefCdefStrengths {
            y0: 2,
            uv0: 28,
            y_min: 2,
            y_max: 2,
        };
        let out = update_cdef_filters_on_ref_info(&mut c, r, Some(r));
        assert_eq!(c.use_reference_cdef_fs, 1);
        assert_eq!(c.pred_y_f, 2, "the reference's own luma strength");
        assert_eq!(c.pred_uv_f, 28, "the MEAN of the two lists' chroma");
        assert_eq!(c.first_pass_fs_num, 0, "no search runs");
        assert_eq!(c.default_second_pass_fs_num, 0);
        assert!(!out.force_cdef_off);
    }

    /// A KEY frame can never reach the function: both flags are derived from
    /// `!is_base` / `!is_not_highest_layer`, and a key frame has both true.
    /// This is what makes the whole change byte-inert for the still envelope
    /// BY CONSTRUCTION rather than only by measurement.
    #[test]
    fn a_key_frame_never_asks_for_a_reference_derived_set() {
        for lvl in 0..=10u8 {
            let c = set_cdef_search_controls(
                lvl, /*is_base=*/ true, /*is_not_highest_layer=*/ true,
            )
            .unwrap_or_else(|| panic!("level {lvl}"));
            assert_eq!(c.search_best_ref_fs, 0, "level {lvl}");
            assert_eq!(c.use_reference_cdef_fs, 0, "level {lvl}");
        }
    }

    /// Two DIFFERENT references: the candidate list grows to three (the
    /// level's default plus each list's), the search still runs, and
    /// `use_reference_cdef_fs` stays off.
    #[test]
    fn ref_info_grows_the_candidate_list_when_the_lists_disagree() {
        let mut c = set_cdef_search_controls(5, false, false).expect("level 5");
        let default0 = c.default_first_pass_fs[0];
        let a = RefCdefStrengths {
            y0: default0.wrapping_add(1),
            uv0: 5,
            y_min: default0.wrapping_add(1),
            y_max: default0.wrapping_add(1),
        };
        let b = RefCdefStrengths {
            y0: default0.wrapping_add(2),
            uv0: 9,
            y_min: default0.wrapping_add(2),
            y_max: default0.wrapping_add(2),
        };
        let out = update_cdef_filters_on_ref_info(&mut c, a, Some(b));
        assert_eq!(c.use_reference_cdef_fs, 0);
        assert_eq!(c.first_pass_fs_num, 3);
        assert_eq!(c.default_first_pass_fs[1], a.y0);
        assert_eq!(c.default_first_pass_fs[2], b.y0);
        assert!(!out.force_cdef_off);
    }

    /// One list only, and its filter IS the level's default: nothing is added,
    /// the count stays 1, and C switches CDEF off for the frame
    /// ("Set cdef to off if pred luma is", `md_config_process.c:768`). The
    /// test is on the candidate COUNT, not on a strength value.
    #[test]
    fn ref_info_forces_cdef_off_when_only_the_default_survives() {
        let mut c = set_cdef_search_controls(5, false, false).expect("level 5");
        let same = RefCdefStrengths {
            y0: c.default_first_pass_fs[0],
            uv0: 3,
            y_min: c.default_first_pass_fs[0],
            y_max: c.default_first_pass_fs[0],
        };
        let out = update_cdef_filters_on_ref_info(&mut c, same, None);
        assert_eq!(c.first_pass_fs_num, 1);
        assert!(out.force_cdef_off);
    }
}

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

/// C `CDEF_QP_STRENGTH_UV` — chroma takes the qp-derived strength while luma
/// is searched (Ghost Robot `e6ff85f0d`, `pcs.h:556`).
const CDEF_QP_STRENGTH_UV: u8 = 1;
/// C `CDEF_QP_STRENGTH_YUV` — luma and chroma both take the qp-derived
/// strength (the `use_qp_strength` bool's equivalent).
const CDEF_QP_STRENGTH_YUV: u8 = 2;

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
    /// `svt_pick_cdef_from_qp`. On Ghost Robot this field is DERIVED: C's
    /// `e6ff85f0d` replaced it with `qp_strength_level` (OFF/UV/YUV), and
    /// the port stores that enum on [`Self::qp_strength_level`]; the bool
    /// then reads `level == CDEF_QP_STRENGTH_YUV`, preserving the old "the
    /// whole block takes the qp strength" meaning the search paths consult.
    /// The UV level — chroma-only qp strength under a real luma search — has
    /// no bool spelling and is only observable through the level.
    pub use_qp_strength: bool,
    /// Ghost Robot's `qp_strength_level` (`e6ff85f0d`): `CDEF_QP_STRENGTH_*
    /// ` = 0/1/2 (OFF/UV/YUV). Under the other references C has no such
    /// field; the port normalizes it to `use_qp_strength ? YUV : OFF` so
    /// the slot means the same thing in every compare.
    pub qp_strength_level: u8,
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
            qp_strength_level: 0,
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
///
/// Ghost Robot's `e6ff85f0d` replaced the `use_qp_strength` bool with the
/// `qp_strength_level` enum (OFF/UV/YUV) and zeroed `skip_th` on levels
/// 7..=10 — levels 7/8/9 now take `UV` (chroma-only qp strength) and 10
/// `YUV`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn set_cdef_search_controls(
    cdef_search_level: u8,
    is_base: bool,
    is_not_highest_layer: bool,
    reference: crate::reference::SvtReference,
) -> Option<CdefSearchControls> {
    let gr = reference == crate::reference::SvtReference::GhostRobot;
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
            // Ghost Robot e6ff85f0d: UV qp strength, skip_th flat 0.
            c.skip_th = if gr || is_base { 0 } else { 80 };
            c.uv_from_y = false;
            if gr {
                c.qp_strength_level = CDEF_QP_STRENGTH_UV;
            } else {
                c.use_qp_strength = false;
            }
        }
        // pf {0,15}, sf {+2}, chroma copied from luma.
        8 => {
            fill(&mut c, &[0, 15], &[2], false);
            c.default_first_pass_fs_uv[2] = -1;
            c.use_reference_cdef_fs = i8::from(!is_base);
            c.search_best_ref_fs = u8::from(!is_base);
            c.subsampling_factor = 4;
            c.skip_th = if gr || is_base { 0 } else { 80 };
            c.uv_from_y = true;
            if gr {
                c.qp_strength_level = CDEF_QP_STRENGTH_UV;
            } else {
                c.use_qp_strength = false;
            }
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
            c.skip_th = if gr || is_base { 0 } else { 80 };
            c.uv_from_y = true;
            if gr {
                c.qp_strength_level = CDEF_QP_STRENGTH_UV;
            } else {
                c.use_qp_strength = false;
            }
        }
        // The qp fast path (`svt_pick_cdef_from_qp`): no candidate arrays are
        // written at all, so they keep the control set's prior contents.
        10 => {
            c.enabled = 1;
            c.use_reference_cdef_fs = 0;
            c.skip_th = if gr || is_base { 0 } else { 80 };
            if gr {
                c.qp_strength_level = CDEF_QP_STRENGTH_YUV;
            } else {
                c.use_qp_strength = true;
            }
        }
        // C: `default: assert(0)`.
        _ => return None,
    }

    // Normalize the two field spellings: Ghost Robot stores
    // `qp_strength_level`; the older references store `use_qp_strength`.
    if gr {
        c.use_qp_strength = c.qp_strength_level == CDEF_QP_STRENGTH_YUV;
    } else {
        c.qp_strength_level = if c.use_qp_strength {
            CDEF_QP_STRENGTH_YUV
        } else {
            0
        };
    }

    // "If chroma filters will be copied from luma, set chroma filters to -1 to
    // avoid testing" (enc_mode_config.c:1188-1196). Levels 8/9 already wrote
    // -1, so this is a no-op there; it exists for a config-forced level.
    // Ghost Robot e6ff85f0d reads the LEVEL (`< YUV` — a UV-strength row
    // still copies luma's list).
    let below_yuv = if gr {
        c.qp_strength_level < CDEF_QP_STRENGTH_YUV
    } else {
        !c.use_qp_strength
    };
    if c.uv_from_y && below_yuv {
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

/// C `md_config_process.c:973-980` — the QP adjustment on
/// `cdef_search_ctrls.skip_th`, then the `skip_perc >= cdef_skip_th` gate that
/// switches CDEF off for the whole frame.
///
/// ```text
/// uint8_t cdef_skip_th = 0;
/// if (cdef_ctrls->skip_th) {
///     cdef_skip_th = CLIP3(25, 100,
///         (int)cdef_ctrls->skip_th + ((int)base_q_idx - 128) / 4);
/// }
/// if (... || (cdef_ctrls->skip_th && skip_perc >= cdef_skip_th) || ...)
///     pcs->ppcs->cdef_level = 0;
/// ```
///
/// Two details that are easy to lose. The guard is on the RAW `skip_th`, not
/// on the adjusted threshold — a `skip_th` of 0 disables the gate outright
/// rather than clipping to 25. And C's `/ 4` is C integer division on a
/// SIGNED value, so it truncates TOWARD ZERO: at `base_q_idx` 100 the term is
/// `-28 / 4 = -7`, and at 127 it is `-1 / 4 = 0`, not -1.
///
/// # Evidence
///
/// TIER 4. The expression is inline in `svt_aom_sig_deriv_md_config` and
/// exports nothing of its own; the tests below are hand-derived from the four
/// lines above, and the value that matters is pinned end to end by
/// `benchmarks/frame2_cdef_skip_2026-09-03.md`'s measurement against C's own
/// frame-2 header.
#[must_use]
pub fn cdef_skip_gate(skip_th: u8, base_q_idx: u8, ref_skip_percentage: u8) -> bool {
    if skip_th == 0 {
        return false;
    }
    let adjusted = (i32::from(skip_th) + (i32::from(base_q_idx) - 128) / 4).clamp(25, 100) as u8;
    ref_skip_percentage >= adjusted
}

/// C `disable_cdef_th` (`md_config_process.c:775`), indexed
/// `[zero_filter_strength_lvl][input_resolution]`.
const DISABLE_CDEF_TH: [[u32; 7]; 4] = [
    [0, 0, 0, 0, 0, 0, 0],
    [100, 200, 500, 800, 1000, 1000, 1000],
    [900, 1000, 2000, 3000, 4000, 4000, 4000],
    [6000, 7000, 8000, 9000, 10000, 10000, 10000],
];

/// The one per-reference field `me_based_cdef_skip` reads beyond
/// `tmp_layer_idx`: `EbReferenceObject::cdef_dist_dev`
/// (`reference_object.h:50`), which `rest_process.c:205` copies from the
/// picture at end of pipeline. **-1 is "never computed"** — `cdef_process.c`
/// seeds it there and only the RD search (`enc_cdef.c:1057`) or the
/// all-zero-strengths rule (`cdef_process.c:699-702`) overwrite it — and the
/// reader SKIPS a -1 slot rather than averaging it in.
pub struct RefCdefDist {
    /// `ref_obj->cdef_dist_dev`.
    pub cdef_dist_dev: i32,
    /// `ref_obj->tmp_layer_idx` — a higher-layer reference is not allowed to
    /// demote a lower-layer picture's CDEF.
    pub tmp_layer_idx: u8,
}

/// Inputs to [`me_based_cdef_skip`], the first of C's three CDEF-off gates.
pub struct CdefMeSkipInputs<'a> {
    /// `pcs->slice_type == I_SLICE` (early `return false`).
    pub is_intra_slice: bool,
    /// `ppcs->hierarchical_levels` — selects the `mult` ladder.
    pub hierarchical_levels: u8,
    /// `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// `frame_is_boosted(ppcs)` — the flat-GOP `mult` arm.
    pub frame_is_boosted: bool,
    /// `frame_is_leaf(ppcs)` — `update_type == LF_UPDATE`.
    pub frame_is_leaf: bool,
    /// `ppcs->input_resolution`.
    pub input_resolution: crate::port_enc_mode_config::ResolutionRange,
    /// `cdef_recon_ctrls.zero_filter_strength_lvl` — a 0 level disables the
    /// whole gate (C reads `disable_cdef_th[0][..]` which is all zero).
    pub zero_filter_strength_lvl: u8,
    /// `cdef_recon_ctrls.prev_cdef_dist_th`.
    pub prev_cdef_dist_th: u16,
    /// The picture's SINGLE-reference entries (C `ref_frame_type_arr`
    /// restricted to `rf[1] == NONE_FRAME`), in `set_all_ref_frame_type`
    /// order — list 0 then list 1.
    pub refs: &'a [RefCdefDist],
    /// C `average_me_sad` = `sum(ppcs->rc_me_distortion[b64]) /
    /// b64_total_count` (`md_config_process.c:797-803`).
    pub avg_me_sad: u32,
}

/// C `me_based_cdef_skip` (`md_config_process.c:781-834`), the first disjunct
/// of the `pcs->ppcs->cdef_level = 0` gate at `:980`. Returns true = CDEF off.
///
/// Structure mirrors `dlf_arm::me_based_dlf_skip` — the DLF twin — but note
/// the differences: the `disable_cdef_th` table, and the `prev_cdef_dist`
/// loop DOES carry C's `tmp_layer_idx <= temporal_layer_index` clause
/// (`:819`), the one the DLF reader keeps and the OTHER DLF loop drops.
#[must_use]
pub fn me_based_cdef_skip(i: &CdefMeSkipInputs<'_>) -> bool {
    if i.is_intra_slice {
        return false;
    }
    // "For flat, mult should be based on update_type since all pics are
    // temporal layer 0" (C's own comment, md_config_process.c:788).
    let mult: i32 = if i.hierarchical_levels != 0 {
        i32::from(i.temporal_layer_index) + 1
    } else if i.frame_is_boosted {
        1
    } else if i.frame_is_leaf {
        3
    } else {
        2
    };
    let row = DISABLE_CDEF_TH
        .get(i.zero_filter_strength_lvl as usize)
        .unwrap_or(&DISABLE_CDEF_TH[0]);
    let use_zero_strength_th = row[i.input_resolution.as_u8() as usize] * mult as u32;
    if use_zero_strength_th == 0 {
        return false;
    }

    let mut prev_cdef_dist: i32 = 0;
    if i.prev_cdef_dist_th != 0 {
        let mut tot_refs = 0i32;
        for r in i.refs {
            if r.cdef_dist_dev >= 0 && r.tmp_layer_idx <= i.temporal_layer_index {
                prev_cdef_dist += r.cdef_dist_dev;
                tot_refs += 1;
            }
        }
        if tot_refs != 0 {
            prev_cdef_dist /= tot_refs;
        }
    }

    if i.prev_cdef_dist_th == 0 || prev_cdef_dist < i32::from(i.prev_cdef_dist_th) * mult {
        return i.avg_me_sad < use_zero_strength_th;
    }
    false
}

#[cfg(test)]
mod skip_gate_tests {
    use super::cdef_skip_gate;

    /// EVIDENCE TIER 4 — hand-derived from `md_config_process.c:973-980`.
    #[test]
    fn a_zero_skip_th_disables_the_gate_rather_than_clipping_to_25() {
        // The `if (cdef_ctrls->skip_th)` guard is on the RAW value. With
        // `skip_th = 0` the CLIP3 never runs, so a 100 % skip reference must
        // NOT switch CDEF off — which is C's every-preset-<=-M7 behaviour and
        // its every-base-frame behaviour above that.
        assert!(!cdef_skip_gate(0, 160, 100));
        assert!(!cdef_skip_gate(0, 255, 100));
    }

    /// The cell this gate was found on: `diag 64x64 q40 p8` frame 2, whose
    /// list-0 reference is a 22-byte all-skip frame.
    #[test]
    fn the_measured_frame_two_cell_switches_cdef_off() {
        // level 7, non-base -> skip_th 80; base_q_idx 160 -> +8 -> 88.
        assert!(cdef_skip_gate(80, 160, 100));
        assert!(!cdef_skip_gate(80, 160, 87));
        assert!(cdef_skip_gate(80, 160, 88));
    }

    /// C's `/ 4` is signed integer division and truncates TOWARD ZERO, so a
    /// remainder below 128 does NOT round down a further step. A port written
    /// with an arithmetic shift would give 99 here instead of 100.
    #[test]
    fn the_qp_term_truncates_toward_zero() {
        // base_q_idx 127 -> (127 - 128) / 4 == 0 in C, -1 with a >> 2.
        assert!(cdef_skip_gate(100, 127, 100));
        // base_q_idx 100 -> -28 / 4 == -7 -> 93.
        assert!(cdef_skip_gate(100, 100, 93));
        assert!(!cdef_skip_gate(100, 100, 92));
    }

    /// Both CLIP3 rails.
    #[test]
    fn the_threshold_clamps_to_25_and_100() {
        // A tiny skip_th at qindex 0 would go to 1 - 32 = -31; clipped to 25.
        assert!(cdef_skip_gate(1, 0, 25));
        assert!(!cdef_skip_gate(1, 0, 24));
        // A large one at qindex 255 would go to 100 + 31 = 131; clipped to 100.
        assert!(cdef_skip_gate(100, 255, 100));
        assert!(!cdef_skip_gate(100, 255, 99));
    }
}

#[cfg(test)]
mod me_skip_tests {
    use super::{CdefMeSkipInputs, RefCdefDist, ResolutionRange, me_based_cdef_skip};

    /// The measured `gradient 64x64 q40 p13` poc-4 cell, hierarchical
    /// low-delay: `cdef_recon_level` 3 (`zlvl = 3`, `prev_cdef_dist_th = 10`),
    /// `input_resolution` 240p, a temporal-layer-0 picture so `mult = 1` and
    /// `use_zero_strength_th = 6000`. C's `me_based_cdef_skip` fired on every
    /// inter frame of this sequence (`avg_me_sad = 0` — the content tracks
    /// perfectly), which is what left `cdef_damping` at its 0 initialiser and
    /// wrote the `01` field the port used to get wrong.
    fn p13_base_frame(avg_me_sad: u32, refs: &[RefCdefDist]) -> CdefMeSkipInputs<'_> {
        CdefMeSkipInputs {
            is_intra_slice: false,
            hierarchical_levels: 4,
            temporal_layer_index: 0,
            frame_is_boosted: false,
            frame_is_leaf: false,
            input_resolution: ResolutionRange::R240p,
            zero_filter_strength_lvl: 3,
            prev_cdef_dist_th: 10,
            refs,
            avg_me_sad,
        }
    }

    #[test]
    fn the_measured_p13_base_frame_skips_cdef() {
        // poc 4: the references all measured dist_dev 0 (their own CDEF was
        // off), so prev_cdef_dist = 0 < 10 * 1, and avg_me_sad 0 < 6000.
        let refs = [RefCdefDist {
            cdef_dist_dev: 0,
            tmp_layer_idx: 0,
        }];
        assert!(me_based_cdef_skip(&p13_base_frame(0, &refs)));
        // A nonzero ME residual under the threshold still skips.
        assert!(me_based_cdef_skip(&p13_base_frame(5999, &refs)));
        assert!(!me_based_cdef_skip(&p13_base_frame(6000, &refs)));
    }

    #[test]
    fn an_i_slice_never_skips() {
        let refs = [RefCdefDist {
            cdef_dist_dev: 0,
            tmp_layer_idx: 0,
        }];
        let mut i = p13_base_frame(0, &refs);
        i.is_intra_slice = true;
        assert!(!me_based_cdef_skip(&i));
    }

    #[test]
    fn a_zero_strength_level_disables_the_gate() {
        let refs = [RefCdefDist {
            cdef_dist_dev: 0,
            tmp_layer_idx: 0,
        }];
        let mut i = p13_base_frame(0, &refs);
        i.zero_filter_strength_lvl = 0;
        assert!(!me_based_cdef_skip(&i));
    }

    /// `prev_cdef_dist_th == 0` collapses the reference-history clause — the
    /// skip decision rests on `avg_me_sad` alone (C `md_config_process.c:830`,
    /// `!prev_cdef_dist_th ||`).
    #[test]
    fn a_zero_prev_dist_th_ignores_reference_history() {
        let refs = [RefCdefDist {
            cdef_dist_dev: 999,
            tmp_layer_idx: 0,
        }];
        let mut i = p13_base_frame(0, &refs);
        i.prev_cdef_dist_th = 0;
        assert!(me_based_cdef_skip(&i));
        i.avg_me_sad = 6000;
        assert!(!me_based_cdef_skip(&i));
    }

    /// A strong recent CDEF gain KEEPS CDEF on: `prev_cdef_dist >=
    /// prev_cdef_dist_th * mult` fails the whole gate even when the picture
    /// itself is perfectly predicted.
    #[test]
    fn a_reference_with_real_cdef_gain_blocks_the_skip() {
        let refs = [RefCdefDist {
            cdef_dist_dev: 271, // poc 0's measured dev on this very cell
            tmp_layer_idx: 0,
        }];
        // 271 >= 10 * 1 -> gate does not fire.
        assert!(!me_based_cdef_skip(&p13_base_frame(0, &refs)));
    }

    /// The two exclusions in C's average: a `-1` ("never computed") slot is
    /// skipped entirely, and a reference from a HIGHER temporal layer cannot
    /// demote a lower-layer picture (`tmp_layer_idx <= temporal_layer_index`,
    /// `md_config_process.c:818-819`).
    #[test]
    fn minus_one_and_higher_layer_refs_do_not_count() {
        // tl=1 picture, mult=2, prev_cdef_dist_th=10 -> needs pcd < 20.
        let refs = [
            // Higher layer than the picture: excluded.
            RefCdefDist {
                cdef_dist_dev: 0,
                tmp_layer_idx: 2,
            },
            // Never computed: excluded.
            RefCdefDist {
                cdef_dist_dev: -1,
                tmp_layer_idx: 0,
            },
            // Eligible: counted.
            RefCdefDist {
                cdef_dist_dev: 30,
                tmp_layer_idx: 1,
            },
        ];
        let mut i = p13_base_frame(0, &refs);
        i.temporal_layer_index = 1;
        // Only the dev=30 ref counts -> pcd=30 >= 20 -> no skip.
        assert!(!me_based_cdef_skip(&i));
        // Drop the counted ref to 19 -> 19 < 20 -> skip fires.
        let refs = [
            RefCdefDist {
                cdef_dist_dev: 0,
                tmp_layer_idx: 2,
            },
            RefCdefDist {
                cdef_dist_dev: -1,
                tmp_layer_idx: 0,
            },
            RefCdefDist {
                cdef_dist_dev: 19,
                tmp_layer_idx: 1,
            },
        ];
        assert!(me_based_cdef_skip(&p13_base_frame_with_tl(0, &refs, 1)));
    }

    fn p13_base_frame_with_tl<'a>(
        avg_me_sad: u32,
        refs: &'a [RefCdefDist],
        tl: u8,
    ) -> CdefMeSkipInputs<'a> {
        let mut i = p13_base_frame(avg_me_sad, refs);
        i.temporal_layer_index = tl;
        i
    }

    /// The flat-GOP `mult` ladder (`update_type`, since every picture is tl=0):
    /// boosted 1, leaf 3, else 2 — C `md_config_process.c:788-791`.
    #[test]
    fn the_flat_gop_mult_ladder() {
        let refs = [RefCdefDist {
            cdef_dist_dev: 0,
            tmp_layer_idx: 0,
        }];
        let mut i = p13_base_frame(0, &refs);
        i.hierarchical_levels = 0;
        // Leaf (mult 3): th = 6000*3 = 18000, pcd bound = 10*3 = 30.
        i.frame_is_leaf = true;
        assert!(me_based_cdef_skip(&i));
        // Boosted (mult 1): th = 6000, bound 10 — same as the p13 base cell.
        i.frame_is_leaf = false;
        i.frame_is_boosted = true;
        assert!(me_based_cdef_skip(&i));
        // Neither (mult 2): th = 12000 — an avg_me_sad of 7000 distinguishes
        // this arm from boosted (6000): skip under mult 2, not under mult 1.
        i.frame_is_boosted = false;
        i.avg_me_sad = 7000;
        assert!(me_based_cdef_skip(&i));
        i.frame_is_boosted = true;
        assert!(!me_based_cdef_skip(&i));
    }
}

#[cfg(test)]
mod tests;

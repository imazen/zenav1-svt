//! `svt_av1_pick_filter_level` — the frame's deblocking LEVELS, both arms.
//!
//! C `Codec/deblocking_filter.c`:
//! * [`pick_filter_level_by_q`] — `svt_av1_pick_filter_level_by_q` (`:1026`),
//!   reached from `svt_av1_pick_filter_level`'s `method >= LPF_PICK_FROM_Q`
//!   branch, which `enc_dec_process.c:3132` takes when
//!   `dlf_ctrls.sb_based_dlf` is set.
//! * [`pick_filter_level_full_image`] — the `else` branch of
//!   `svt_av1_pick_filter_level` (`:1160-1284`), which `dlf_process.c:97`
//!   takes when `sb_based_dlf` is clear.
//! * [`me_based_dlf_skip`] (`:964`), which both consult.
//!
//! ## Why this module exists — the INTER arms were never reachable
//!
//! Before it, `deblock.rs` carried the two pickers specialized to a KEY
//! frame, and `pipeline.rs` handed every non-key frame `LfLevels::default()`
//! — i.e. the port switched the deblocking filter OFF on every inter frame.
//! MEASURED 2026-09-02 on the inter campaign's 96-cell grid: **20 of the 40
//! remaining frame-1 divergences differ FIRST at `loop_filter_level[0]`**,
//! C writing 8/9/12/16/20/24 against the port's 0
//! (`docs/INTER-ENCODE-PLAN.md` §1z²¹).
//!
//! The two arms answer that split exactly, and the split is a PRESET split
//! because `get_dlf_level_default` is:
//!
//! * `<= M6` -> `is_not_last_layer ? 3 : 6`, and on the campaign's flat GOP
//!   `is_highest_layer` is false (`pd_process.c:5560` ANDs in
//!   `hierarchical_levels != 0` precisely so a flat GOP does not mark every
//!   picture highest), so `is_not_last_layer` is TRUE and the level is **3**
//!   -> `sb_based_dlf = 0` -> [`pick_filter_level_full_image`];
//! * `<= M9` -> `is_not_last_layer ? 6 : 0` = **6** -> `sb_based_dlf = 1` ->
//!   [`pick_filter_level_by_q`].
//!
//! ## The three inter-only behaviours a key-frame specialization cannot have
//!
//! Each is measured on the grid rather than argued from the source:
//!
//! 1. **`dlf_avg` + `use_ref_avg_y/uv` mean the level is COPIED from the
//!    reference, with no search at all.** At level 3 every cell whose C
//!    frame-1 header differs carries EXACTLY its own frame-0 level
//!    (`diag 16x16 q20 p6` 8 -> 8; `diag 72x72 q40 p6` 12 -> 12;
//!    `diag 128x128 q55 p6` 24 -> 24), luma and chroma alike.
//! 2. **`prev_dlf_dist < 5` turns the copy back off.** `gradient 72x72 q40
//!    p6` has frame-0 level 8 and C writes **0** on frame 1 — the reference's
//!    `dlf_dist_dev` (how much the reference's own deblock actually reduced
//!    SSE, `dlf_process.c:119`) was under 5, so C declines to filter.
//! 3. **`me_based_dlf_skip` zeroes by MOTION, at level 6 only.** Level 6 sets
//!    `zero_filter_strength_lvl = 2`, i.e. `disable_dlf_th[2][in_res] * mult`
//!    with `mult = 3` for a leaf (`LF_UPDATE`) picture on a flat GOP. On
//!    `uniform` content — an exactly-predictable translate, mean ME SAD 0 —
//!    C writes 0 on every frame 1 despite a nonzero reference level, and on
//!    `screen 16x16 q40 p8` it writes **luma 9 with chroma 0**, which is the
//!    two thresholds (`th` and `2 * th`) firing separately.
//!
//! ## Evidence tier
//!
//! Tier 4 (`docs/WORKING-ON-THIS.md` §4) for the arithmetic — every function
//! here is `static` in C bar `svt_av1_pick_filter_level*`, which this host's
//! linker cannot wrap (Apple ld64 has no `-Wl,--wrap`). Tier 2 for the
//! result: the levels are read back out of C's own frame headers by
//! `tools/fh_fields.py` on all 96 cells, and `tools/inter_byte_gate.sh`
//! asserts whole streams.

use crate::deblock::LfLevels;
use crate::port_enc_mode_config::ResolutionRange;
use crate::port_enc_mode_config::ctrls::DlfCtrls;

/// AV1 `MAX_LOOP_FILTER`.
const MAX_LOOP_FILTER: i32 = 63;

/// C `disable_dlf_th[DLF_MAX_LVL][INPUT_SIZE_COUNT]`
/// (`deblocking_filter.c:29`), indexed by
/// `dlf_ctrls.zero_filter_strength_lvl` then `input_resolution`.
///
/// Row 0 is all zeros, which is what makes [`me_based_dlf_skip`] a no-op at
/// every `dlf_level` whose controls leave `zero_filter_strength_lvl` at 0
/// (levels 0..=3 and 7 — `set_dlf_controls`).
const DISABLE_DLF_TH: [[u32; 7]; 4] = [
    [0, 0, 0, 0, 0, 0, 0],
    [100, 200, 500, 800, 1000, 1000, 1000],
    [900, 1000, 2000, 3000, 4000, 4000, 4000],
    [6000, 7000, 8000, 9000, 10000, 10000, 10000],
];

/// C `inter_frame_multiplier[INPUT_SIZE_COUNT]`
/// (`deblocking_filter.c:28`) — the slope of the by-q linear fit for a
/// NON-key frame. The key-frame slope is the literal 17563 below.
const INTER_FRAME_MULTIPLIER: [i32; 7] = [6017, 6017, 6017, 12034, 12034, 12034, 12034];

/// What one entry of C's `ppcs->ref_frame_type_arr` contributes, for the
/// single-reference entries (`rf[1] == NONE_FRAME`) that are the only ones
/// either picker reads.
///
/// C reads these off `EbReferenceObject` (`reference_object.h:44-49`), which
/// `rest_process.c:200-204` fills from the reference picture's own
/// `frm_hdr->loop_filter_params` and `pcs->dlf_dist_dev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefDlfState {
    /// `EbReferenceObject::filter_level[0..2]`, `filter_level_u`,
    /// `filter_level_v` — this reference's OWN signalled loop-filter levels,
    /// in the frame header's order `[y_vert, y_horz, u, v]`.
    pub filter_level: [i32; 4],
    /// `EbReferenceObject::dlf_dist_dev` — `1000 - 1000 * best_sse /
    /// zero_sse` for the reference's own deblock, or **-1** when it was never
    /// computed (`dlf_process.c:92`: SB-based DLF does not compute the
    /// distortion, so a reference coded with `sb_based_dlf` carries the
    /// sentinel and every reader must skip it rather than average it in).
    pub dlf_dist_dev: i32,
}

/// Everything both pickers read that is not a pixel.
#[derive(Debug, Clone, Copy)]
pub struct DlfPickInputs<'a> {
    /// `pcs->ppcs->dlf_ctrls`, from `set_dlf_controls(get_dlf_level_*(..))`.
    pub ctrls: DlfCtrls,
    /// `frm_hdr->frame_type == KEY_FRAME` — the by-q coefficient selector.
    pub frame_type_is_key: bool,
    /// `pcs->slice_type == I_SLICE` — [`me_based_dlf_skip`]'s early return.
    /// Equal to [`Self::frame_type_is_key`] in this port (there is no
    /// intra-only non-key frame), carried separately because C reads two
    /// different fields.
    pub is_intra_slice: bool,
    /// `frame_is_boosted(ppcs)` = `frame_is_kf_gf_arf` (`enc_mode_config.h
    /// :108`) — intra-only, ARF or GF update.
    pub frame_is_boosted: bool,
    /// `frame_is_leaf(ppcs)` = `update_type == SVT_AV1_LF_UPDATE`
    /// (`enc_mode_config.h:113`).
    pub frame_is_leaf: bool,
    /// `pcs->ppcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// `pcs->ppcs->input_resolution`.
    pub input_resolution: ResolutionRange,
    /// The single-reference entries of `ppcs->ref_frame_type_arr`, in C's
    /// iteration order. EMPTY on an I_SLICE, which is what makes
    /// `tot_ref_frame_types == 0` true for every guard that reads it.
    pub refs: &'a [RefDlfState],
    /// C's `average_me_sad` = `sum(ppcs->rc_me_distortion[b64]) /
    /// b64_total_count` (`deblocking_filter.c:982-986`). Unread on an
    /// I_SLICE and at any `zero_filter_strength_lvl` of 0.
    pub avg_me_sad: u32,
    /// `frm_hdr->quantization_params.base_q_idx`.
    pub base_qindex: u8,
    /// `scs->static_config.encoder_bit_depth`.
    pub bit_depth: u8,
}

/// What the full-image picker produced, including the two SSEs
/// `dlf_process.c` turns into the next frame's `dlf_dist_dev`.
///
/// `-1` in either SSE is C's "not evaluated" sentinel (`dlf_process.c:89-91`
/// initializes both to it, and `search_filter_level` only assigns when the
/// corresponding `ss_err` entry is non-negative). The ref-average arms of
/// [`pick_filter_level_full_image`] never call the search at all, so a frame
/// that takes them returns two sentinels — which is exactly the state
/// `dlf_process.c:103` and `:115` then recompute from the recon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlfPick {
    pub levels: LfLevels,
    /// `pcs->zero_filt_sse`.
    pub zero_filt_sse: i64,
    /// `pcs->best_filt_sse`.
    pub best_filt_sse: i64,
}

/// C `me_based_dlf_skip` (`deblocking_filter.c:964`) — returns
/// `(do_y, do_uv)`.
///
/// `prev_dlf_dist_th` is `dlf_ctrls.prev_dlf_dist_th`; it is 0 at every level
/// but 4, and when it is 0 the whole `prev_dlf_dist` accumulation is skipped
/// and the two ME comparisons run unconditionally.
#[must_use]
pub fn me_based_dlf_skip(i: &DlfPickInputs<'_>) -> (bool, bool) {
    if i.is_intra_slice {
        return (true, true);
    }
    // "For flat, mult should be based on update_type since all pics are
    // temporal layer 0" (C's own comment).
    let mult: u32 = if i.hierarchical_levels != 0 {
        u32::from(i.temporal_layer_index) + 1
    } else if i.frame_is_boosted {
        1
    } else if i.frame_is_leaf {
        3
    } else {
        2
    };
    let lvl = i.ctrls.zero_filter_strength_lvl as usize;
    // C indexes `disable_dlf_th[DLF_MAX_LVL]` with no bound check; every
    // `set_dlf_controls` row assigns 0..=2, so a level outside the table is a
    // port bug rather than a reachable state. Saturate instead of panicking.
    let row = DISABLE_DLF_TH.get(lvl).unwrap_or(&DISABLE_DLF_TH[0]);
    let use_zero_strength_th = row[i.input_resolution.as_u8() as usize] * mult;
    if use_zero_strength_th == 0 {
        return (true, true);
    }

    let mut prev_dlf_dist: i32 = 0;
    if i.ctrls.prev_dlf_dist_th != 0 {
        let mut tot_refs = 0i32;
        for r in i.refs {
            // C additionally requires `ref_obj->tmp_layer_idx <=
            // pcs->temporal_layer_index` HERE (`:1001`) — the OTHER
            // `dlf_dist_dev` loop, at `:1244`, does not. On a flat GOP every
            // picture is layer 0 so the extra clause is always satisfied;
            // it is written out rather than dropped so a hierarchy inherits
            // it. `ref_tmp_layer_idx` is not carried because no reachable
            // configuration can make it differ; when one exists, add it to
            // `RefDlfState` and test it here.
            if r.dlf_dist_dev >= 0 {
                prev_dlf_dist += r.dlf_dist_dev;
                tot_refs += 1;
            }
        }
        if tot_refs != 0 {
            prev_dlf_dist /= tot_refs;
        }
    }

    let mut do_y = true;
    let mut do_uv = true;
    if i.ctrls.prev_dlf_dist_th == 0
        || prev_dlf_dist < i32::from(i.ctrls.prev_dlf_dist_th) * mult as i32
    {
        if i.avg_me_sad < use_zero_strength_th {
            do_y = false;
        }
        if i.avg_me_sad < use_zero_strength_th * 2 {
            do_uv = false;
        }
    }
    (do_y, do_uv)
}

/// C `svt_av1_pick_filter_level_by_q` (`deblocking_filter.c:1026`) — the
/// `LPF_PICK_FROM_Q` closed form, both frame types.
///
/// The three parts a key-frame specialization does not have:
/// * the INTER slope/intercept (`q * inter_frame_multiplier[in_res] +
///   650707` against the key `q * 17563 - 421574`);
/// * `min_ref_filter_level`, the per-plane MIN over the references' own
///   levels — a zero there forces this frame's level to 0 unless the frame
///   is boosted, which is C's "loop-filter is shut for one of the sub-layer
///   references" rule;
/// * [`me_based_dlf_skip`], which is a no-op on an I_SLICE and live here.
#[must_use]
pub fn pick_filter_level_by_q(i: &DlfPickInputs<'_>) -> LfLevels {
    let mut min_ref: [i32; 4] = [MAX_LOOP_FILTER; 4];
    for r in i.refs {
        for p in 0..4 {
            min_ref[p] = min_ref[p].min(r.filter_level[p]);
        }
    }

    let q = ac_quant_qtx(i.base_qindex, i.bit_depth);
    // ROUND_POWER_OF_TWO(v, n) == (v + (1 << (n-1))) >> n, arithmetic.
    let mut filt_guess: i32 = match i.bit_depth {
        8 => {
            if i.frame_type_is_key {
                (q * 17563 - 421574 + (1 << 17)) >> 18
            } else {
                let m = INTER_FRAME_MULTIPLIER[i.input_resolution.as_u8() as usize];
                (q * m + 650707 + (1 << 17)) >> 18
            }
        }
        10 => (q * 20723 + 4060632 + (1 << 19)) >> 20,
        // bd12 is out of scope for this port (docs/bd10-port-map.md); C's arm
        // is `ROUND_POWER_OF_TWO(q * 20723 + 16242526, 22)` with the bd12 AC
        // qlookup.
        _ => unreachable!("bit_depth must be 8 or 10 (bd12 out of scope, bd10-port-map.md)"),
    };
    if i.bit_depth != 8 && i.frame_type_is_key {
        filt_guess -= 4;
    }
    let mut filt_guess_chroma = filt_guess / 2;

    let (do_y, do_uv) = me_based_dlf_skip(i);
    if !do_y {
        filt_guess = 0;
    }
    if !do_uv {
        filt_guess_chroma = 0;
    }

    let pick = |min: i32, guess: i32| -> u8 {
        if min != 0 || i.frame_is_boosted {
            guess.clamp(0, MAX_LOOP_FILTER) as u8
        } else {
            0
        }
    };
    LfLevels {
        levels: [
            pick(min_ref[0], filt_guess),
            pick(min_ref[1], filt_guess),
            pick(min_ref[2], filt_guess_chroma),
            pick(min_ref[3], filt_guess_chroma),
        ],
    }
}

/// C `svt_av1_pick_filter_level`'s `LPF_PICK_FROM_FULL_IMAGE` branch
/// (`deblocking_filter.c:1160-1284`), with the pixel work factored out.
///
/// `search` is C's `search_filter_level(srcBuffer, temp_lf_recon, pcs,
/// /*partial*/ false, last_frame_filter_level, NULL, plane, dir)`; it must
/// return `(filt_best, ss_err[0], ss_err[filt_best])` with `-1` for an
/// `ss_err` entry the hill-climb never evaluated, which is what C stores into
/// `pcs->{zero,best}_filt_sse` for plane 0.
///
/// Every branch below is C's, in C's order. The two that a key frame can
/// never take — the `dlf_avg` seed and the `use_ref_avg_*` copies — are the
/// whole reason this is not `deblock::pick_filter_levels_full_search`'s
/// body; see the module doc for the measurements behind them.
pub fn pick_filter_level_full_image<F>(i: &DlfPickInputs<'_>, mut search: F) -> DlfPick
where
    F: FnMut(usize, i32, [i32; 4]) -> (i32, i64, i64),
{
    let mut level = [0i32; 4];
    if i.ctrls.dlf_avg && !i.refs.is_empty() {
        let mut tot = [0i32; 4];
        let n = i.refs.len() as i32;
        for r in i.refs {
            for p in 0..4 {
                tot[p] += r.filter_level[p];
            }
        }
        for p in 0..4 {
            level[p] = tot[p] / n;
        }
    }

    let (do_y, do_uv) = me_based_dlf_skip(i);
    let last = level;

    let mut zero_filt_sse: i64 = -1;
    let mut best_filt_sse: i64 = -1;

    if !do_y {
        level[0] = 0;
        level[1] = 0;
    } else if i.frame_is_boosted || !i.ctrls.use_ref_avg_y || i.refs.is_empty() {
        let (filt, zero, best) = search(0, 2, last);
        level[0] = filt;
        level[1] = filt;
        // C `search_filter_level` assigns `pcs->zero_filt_sse` / `best_filt_sse`
        // only for plane 0 and only when the value is non-negative.
        if zero >= 0 {
            zero_filt_sse = zero;
        }
        if best >= 0 {
            best_filt_sse = best;
        }
    } else if last[0] != 0 || last[1] != 0 {
        // "If improvement from DLF of ref frames is small, disable DLF for
        // the current frame." Note this loop has NO `tmp_layer_idx` clause,
        // unlike the otherwise identical one in `me_based_dlf_skip`.
        let mut prev_dlf_dist = 0i32;
        let mut tot_refs = 0i32;
        for r in i.refs {
            if r.dlf_dist_dev >= 0 {
                prev_dlf_dist += r.dlf_dist_dev;
                tot_refs += 1;
            }
        }
        if tot_refs != 0 {
            prev_dlf_dist /= tot_refs;
        }
        if tot_refs != 0 && prev_dlf_dist < 5 {
            level[0] = 0;
            level[1] = 0;
        }
    }

    if !do_uv || (level[0] == 0 && level[1] == 0) {
        level[2] = 0;
        level[3] = 0;
    } else if !i.frame_is_boosted && i.ctrls.use_ref_avg_uv && !i.refs.is_empty() {
        level[2] = last[2];
        level[3] = last[3];
    } else {
        level[2] = search(1, 0, last).0;
        level[3] = search(2, 0, last).0;
    }

    DlfPick {
        levels: LfLevels {
            levels: [
                level[0].clamp(0, MAX_LOOP_FILTER) as u8,
                level[1].clamp(0, MAX_LOOP_FILTER) as u8,
                level[2].clamp(0, MAX_LOOP_FILTER) as u8,
                level[3].clamp(0, MAX_LOOP_FILTER) as u8,
            ],
        },
        zero_filt_sse,
        best_filt_sse,
    }
}

/// C `pcs->dlf_dist_dev` (`dlf_process.c:119`).
///
/// `zero` / `best` are the SSEs of the UNFILTERED and FILTERED recon against
/// the source, already resolved from their `-1` sentinels by the caller (C
/// recomputes each with `picture_sse_calculations` when the search left it
/// unset — `:103` and `:115`, both gated on the frame actually filtering).
///
/// Returns C's -1 sentinel ("not computed") when the frame does not filter,
/// which is the value `dlf_process.c:92` leaves in place when the whole
/// block is skipped.
#[must_use]
pub fn dlf_dist_dev(levels: LfLevels, zero: i64, best: i64) -> i32 {
    if levels.levels[0] == 0 && levels.levels[1] == 0 {
        return 0;
    }
    if zero == 0 {
        return 0;
    }
    (1000 - ((1000 * best) / zero)) as i32
}

/// C `svt_aom_ac_quant_qtx(qindex, 0, bd)` — the AC step for delta 0.
fn ac_quant_qtx(qindex: u8, bit_depth: u8) -> i32 {
    match bit_depth {
        8 => i32::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[qindex as usize]),
        10 => i32::from(crate::bd10::AC_QLOOKUP_10[qindex as usize]),
        _ => unreachable!("bit_depth must be 8 or 10 (bd12 out of scope, bd10-port-map.md)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_enc_mode_config::ctrls::set_dlf_controls;

    fn base<'a>(refs: &'a [RefDlfState], lvl: u8) -> DlfPickInputs<'a> {
        DlfPickInputs {
            ctrls: set_dlf_controls(lvl).expect("a level in 0..=7"),
            frame_type_is_key: false,
            is_intra_slice: false,
            frame_is_boosted: false,
            frame_is_leaf: true,
            hierarchical_levels: 0,
            temporal_layer_index: 0,
            input_resolution: ResolutionRange::R240p,
            refs,
            avg_me_sad: 1_000_000,
            base_qindex: 160,
            bit_depth: 8,
        }
    }

    /// The campaign's level-3 arm: the reference's levels are COPIED, and no
    /// search runs. Measured counterpart: C's `diag 72x72 q40 p6` frame 1
    /// carries frame 0's 12 (`docs/INTER-ENCODE-PLAN.md` §1z²¹).
    #[test]
    fn level3_copies_the_reference_levels_without_searching() {
        let refs = [RefDlfState {
            filter_level: [12, 12, 0, 0],
            dlf_dist_dev: 40,
        }];
        let i = base(&refs, 3);
        assert_eq!(i.ctrls.dlf_avg, true);
        assert_eq!(i.ctrls.use_ref_avg_y, true);
        let mut searched = 0;
        let p = pick_filter_level_full_image(&i, |_, _, _| {
            searched += 1;
            (63, 0, 0)
        });
        assert_eq!(p.levels.levels, [12, 12, 0, 0]);
        assert_eq!(searched, 0, "the ref-average arms must not call the search");
        assert_eq!((p.zero_filt_sse, p.best_filt_sse), (-1, -1));
    }

    /// The same arm with a reference whose own deblock barely helped: C
    /// declines to filter. Measured counterpart: `gradient 72x72 q40 p6`,
    /// frame 0 level 8 -> frame 1 level 0.
    #[test]
    fn level3_zeroes_when_the_references_dlf_gain_was_under_five() {
        let refs = [RefDlfState {
            filter_level: [8, 8, 4, 4],
            dlf_dist_dev: 4,
        }];
        let p = pick_filter_level_full_image(&base(&refs, 3), |_, _, _| {
            panic!("no search on this arm")
        });
        assert_eq!(p.levels.levels, [0, 0, 0, 0], "chroma follows luma to 0");

        // The sentinel is NOT a small number: a reference coded with
        // sb_based_dlf never computed the deviation, and C skips it rather
        // than reading -1 as "under 5".
        let refs = [RefDlfState {
            filter_level: [8, 8, 4, 4],
            dlf_dist_dev: -1,
        }];
        let p = pick_filter_level_full_image(&base(&refs, 3), |_, _, _| {
            panic!("no search on this arm")
        });
        assert_eq!(p.levels.levels, [8, 8, 4, 4]);
    }

    /// A KEY frame reduces to the still path: no references, so `dlf_avg` is
    /// inert, `frame_is_boosted` forces the luma search, and chroma searches
    /// whenever luma is nonzero. This is what lets
    /// `deblock::pick_filter_levels_full_search` delegate here without
    /// moving a still byte.
    #[test]
    fn a_key_frame_takes_the_search_arms_only() {
        let mut i = base(&[], 3);
        i.frame_type_is_key = true;
        i.is_intra_slice = true;
        i.frame_is_boosted = true;
        i.frame_is_leaf = false;
        let mut seen: alloc::vec::Vec<(usize, i32, [i32; 4])> = alloc::vec::Vec::new();
        let p = pick_filter_level_full_image(&i, |plane, dir, last| {
            seen.push((plane, dir, last));
            (7, 99, 11)
        });
        assert_eq!(p.levels.levels, [7, 7, 7, 7]);
        assert_eq!(seen, [(0, 2, [0; 4]), (1, 0, [0; 4]), (2, 0, [0; 4])]);
        assert_eq!((p.zero_filt_sse, p.best_filt_sse), (99, 11));
    }

    /// Level 6's `zero_filter_strength_lvl = 2` is what makes the by-q arm
    /// motion-sensitive, and the two thresholds are `th` and `2 * th` — the
    /// state C shows on `screen 16x16 q40 p8`, luma 9 with chroma 0.
    #[test]
    fn level6_by_q_has_a_luma_threshold_and_a_separate_chroma_one() {
        let refs = [RefDlfState {
            filter_level: [3, 3, 1, 1],
            dlf_dist_dev: -1,
        }];
        // disable_dlf_th[2][R240p] = 900, mult = 3 (leaf, flat) -> 2700.
        let mut i = base(&refs, 6);
        assert_eq!(i.ctrls.zero_filter_strength_lvl, 2);

        i.avg_me_sad = 2699;
        assert_eq!(pick_filter_level_by_q(&i).levels, [0, 0, 0, 0], "below th");

        i.avg_me_sad = 2700;
        let l = pick_filter_level_by_q(&i).levels;
        assert_eq!((l[0], l[2]), (9, 0), "luma on, chroma still below 2*th");

        i.avg_me_sad = 5400;
        assert_eq!(pick_filter_level_by_q(&i).levels, [9, 9, 4, 4], "both on");
    }

    /// A reference whose loop filter was OFF forces this frame's off too,
    /// unless the frame is boosted. C's "loop-filter is shut for one of the
    /// sub-layer reference frames" rule (`deblocking_filter.c:1097`).
    #[test]
    fn by_q_min_ref_zero_shuts_the_filter_off() {
        let refs = [RefDlfState {
            filter_level: [0, 0, 0, 0],
            dlf_dist_dev: -1,
        }];
        let mut i = base(&refs, 6);
        assert_eq!(pick_filter_level_by_q(&i).levels, [0, 0, 0, 0]);
        i.frame_is_boosted = true;
        assert_eq!(
            pick_filter_level_by_q(&i).levels,
            [9, 9, 4, 4],
            "boosted overrides the reference's zero"
        );
    }

    /// `mult` is a function of the update type on a FLAT GOP and of the
    /// temporal layer otherwise — C's own comment says so, and getting it
    /// wrong scales the threshold by 3x.
    #[test]
    fn me_skip_mult_follows_the_update_type_only_when_the_gop_is_flat() {
        let refs = [RefDlfState {
            filter_level: [3, 3, 1, 1],
            dlf_dist_dev: -1,
        }];
        // Every assertion below is at a sad value that DISCRIMINATES the
        // mult it is testing from the neighbouring one — a threshold test
        // that passes at every mult tests nothing.
        let mut i = base(&refs, 6);
        // flat + leaf -> mult 3 -> th 2700, 2*th 5400.
        i.avg_me_sad = 2699;
        assert_eq!(me_based_dlf_skip(&i), (false, false));
        i.avg_me_sad = 2700;
        assert_eq!(me_based_dlf_skip(&i), (true, false));
        i.avg_me_sad = 5400;
        assert_eq!(me_based_dlf_skip(&i), (true, true));
        // flat + non-leaf, non-boosted -> mult 2 -> th 1800. 1800 is BELOW
        // the leaf threshold, so this fails if `frame_is_leaf` is ignored.
        i.frame_is_leaf = false;
        i.avg_me_sad = 1800;
        assert_eq!(me_based_dlf_skip(&i), (true, false));
        i.avg_me_sad = 1799;
        assert_eq!(me_based_dlf_skip(&i), (false, false));
        // flat + boosted -> mult 1 -> th 900, below the non-leaf threshold.
        i.frame_is_boosted = true;
        i.avg_me_sad = 900;
        assert_eq!(me_based_dlf_skip(&i), (true, false));
        i.avg_me_sad = 899;
        assert_eq!(me_based_dlf_skip(&i), (false, false));
        // hierarchy -> mult = temporal_layer_index + 1, and the update type
        // is IGNORED: boosted would be mult 1 on a flat GOP, this is mult 4.
        i.hierarchical_levels = 3;
        i.temporal_layer_index = 3;
        i.avg_me_sad = 3599;
        assert_eq!(me_based_dlf_skip(&i), (false, false), "th 3600");
        i.avg_me_sad = 3600;
        assert_eq!(me_based_dlf_skip(&i), (true, false));
        i.avg_me_sad = 7200;
        assert_eq!(me_based_dlf_skip(&i), (true, true), "2*th 7200");
    }

    /// An I_SLICE returns before the table lookup, which is what keeps every
    /// still cell out of this function's reach.
    #[test]
    fn an_i_slice_never_skips() {
        let mut i = base(&[], 6);
        i.is_intra_slice = true;
        i.avg_me_sad = 0;
        assert_eq!(me_based_dlf_skip(&i), (true, true));
    }

    /// `dlf_dist_dev`'s two zero-producing corners and its arithmetic.
    #[test]
    fn dlf_dist_dev_matches_cs_three_arms() {
        let off = LfLevels { levels: [0; 4] };
        let on = LfLevels {
            levels: [8, 8, 0, 0],
        };
        assert_eq!(dlf_dist_dev(off, 1000, 500), 0, "no filtering -> 0");
        assert_eq!(dlf_dist_dev(on, 0, 0), 0, "zero SSE -> 0");
        assert_eq!(dlf_dist_dev(on, 1000, 900), 100);
        assert_eq!(dlf_dist_dev(on, 1000, 996), 4, "the < 5 boundary");
        assert_eq!(dlf_dist_dev(on, 1000, 995), 5);
    }
}

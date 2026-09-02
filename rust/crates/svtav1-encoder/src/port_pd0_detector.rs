//! Port of `Codec/enc_dec_process.c`'s **light-PD0 classifiers** —
//! `pd0_detector` (:2406) and `pd0_detector_allintra` (:2341).
//!
//! These run once per superblock, BEFORE mode decision, and decide how light
//! a PD0 path that SB gets. The level they land on selects a different search
//! entirely, so on an inter frame this is the first thing that makes two
//! superblocks of the same picture take different code paths — which is why
//! it belongs in the inter-encode critical set rather than in a speed-tuning
//! backlog.
//!
//! **The walk is a LADDER, not a switch.** C loops `pd0_lvl` DOWN from the
//! highest level and, whenever the current level fails a test, decrements
//! `pd0_level` and `continue`s — landing on the next-lower level, which the
//! same loop then re-tests with ITS OWN thresholds. A single SB can therefore
//! step down several levels in one call. Writing it as a match on the
//! incoming level would collapse that.
//!
//! **EVIDENCE: TIER 4.** Both functions are `static` in C and were inlined
//! away by the Release build — `nm` on `enc_dec_process.c.o` does not list
//! either — and their only caller is `svt_aom_mode_decision_kernel`, the
//! encode-decode THREAD body, which cannot be driven without a whole encode.
//! So these are hand-derived vectors traced against the C source and they say
//! so per test. What they call out to is better pinned: the qp-based
//! threshold scaling comes from the EXPORTED
//! `svt_aom_get_qp_based_th_scaling_factors` (`enc_mode_config.c:25`), which
//! belongs to a different file and is taken here as a parameter rather than
//! re-derived — see [`QpThScaling`].
//!
//! **Preprocessor check** (`docs/WORKING-ON-THIS.md` §5 trap #1, and this one
//! is LIVE here): `pd0_detector_allintra` has a real `#if SVT_HDR_MODE` arm.
//! Mainline accumulates the variances in `int32_t` and normalises with `>>`;
//! the fork accumulates in `double` and normalises with `/`. [`accumulate`]
//! implements **the mainline arm** and [`accumulate_fork`] the other, so a
//! caller has to choose rather than inherit whichever one someone read first.

/// C `Pd0Level` (definitions.h:762). `PD0_LVL_6` is the lightest path and
/// does no transform at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Pd0Level {
    #[default]
    Lvl0 = 0,
    Lvl1 = 1,
    Lvl2 = 2,
    Lvl3 = 3,
    Lvl4 = 4,
    Lvl5 = 5,
    /// The lightest PD0 path; does not perform TX, and supports INTER
    /// compensation only.
    Lvl6 = 6,
}

/// C `PD0_LEVELS`.
pub const PD0_LEVELS: usize = 7;

impl Pd0Level {
    /// C's `pd0_lvl - 1`. `None` at level 0, where C's loop bound
    /// (`pd0_lvl > PD0_LVL_0`) means the decrement can never be reached.
    #[must_use]
    pub fn decremented(self) -> Option<Self> {
        Self::from_index(self as usize + 1 - 1)
            .filter(|_| self != Self::Lvl0)
            .and_then(|_| Self::from_index(self as usize - 1))
    }

    /// `None` for an out-of-range index.
    #[must_use]
    pub fn from_index(i: usize) -> Option<Self> {
        Some(match i {
            0 => Self::Lvl0,
            1 => Self::Lvl1,
            2 => Self::Lvl2,
            3 => Self::Lvl3,
            4 => Self::Lvl4,
            5 => Self::Lvl5,
            6 => Self::Lvl6,
            _ => return None,
        })
    }
}

/// C `Pd0Ctrls` (md_process.h:691) — the per-level threshold arrays plus the
/// level itself, which the detector MUTATES.
#[derive(Clone, Copy, Debug)]
pub struct Pd0Ctrls {
    /// `pd0_level` — in AND out.
    pub pd0_level: Pd0Level,
    /// `use_pd0_detector[level]`.
    pub use_pd0_detector: [bool; PD0_LEVELS],
    /// `use_ref_info[level]`: 0 off, 1 safest, 2, 3 most aggressive.
    pub use_ref_info: [u8; PD0_LEVELS],
    /// `me_8x8_cost_variance_th[level]`. C asserts this stays below
    /// `((uint32_t)~0) >> 1` so the `>> 5` times a QP up to 63 cannot
    /// overflow; values at or above that disable the test.
    pub me_8x8_cost_variance_th: [u32; PD0_LEVELS],
    /// `edge_dist_th[level]`.
    pub edge_dist_th: [u32; PD0_LEVELS],
    /// `neigh_me_dist_shift[level]`. `u16::MAX` disables the test.
    pub neigh_me_dist_shift: [u16; PD0_LEVELS],
}

impl Default for Pd0Ctrls {
    fn default() -> Self {
        Self {
            pd0_level: Pd0Level::Lvl0,
            use_pd0_detector: [false; PD0_LEVELS],
            use_ref_info: [0; PD0_LEVELS],
            me_8x8_cost_variance_th: [0; PD0_LEVELS],
            edge_dist_th: [0; PD0_LEVELS],
            neigh_me_dist_shift: [u16::MAX; PD0_LEVELS],
        }
    }
}

/// C `SliceType` — re-exported so callers of this module do not have to reach
/// into the rate-control port for it.
pub use crate::port_rc_process::SliceType;

/// One reference list's contribution to the detector.
///
/// C reads `sb_intra[sb_index]` off the `EbReferenceObject` and gates it on
/// three things: the list has a `count_try`, the reference is the SAME SIZE as
/// this picture (`svt_aom_is_ref_same_size` — reference scaling makes the SB
/// grids mismatch, so the colocated info would be wrong), and the reference is
/// at or below this frame's temporal layer. All three are the CALLER's to
/// evaluate, because two of them are picture-manager facts rather than
/// detector logic; `None` means "no usable reference in this list".
#[derive(Clone, Copy, Debug, Default)]
pub struct RefSbInfo {
    /// `ref_obj->sb_intra[sb_index]` for the usable reference, or `None`.
    pub was_intra: Option<u8>,
}

impl RefSbInfo {
    /// C's `l0_refs` / `l1_refs` counter, which is 0 or 1.
    fn refs(&self) -> bool {
        self.was_intra.is_some()
    }
    /// C's `l0_was_intra` / `l1_was_intra` accumulator, 0 when absent.
    fn was_intra(&self) -> u8 {
        self.was_intra.unwrap_or(0)
    }
}

/// The per-SB inputs `pd0_detector` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pd0SbInput {
    pub slice_type_is_intra: bool,
    /// `ppcs->transition_present == 1`.
    pub transition_present: bool,
    /// `ppcs->picture_qp`.
    pub picture_qp: u32,
    /// `pcs->ref_intra_percentage`.
    pub ref_intra_percentage: u32,
    /// `ppcs->me_8x8_cost_variance[sb_index]`.
    pub me_8x8_cost_variance: u32,
    /// `ppcs->me_64x64_distortion[sb_index]`.
    pub me_64x64_distortion: u32,
    /// `md_ctx->sb_origin_x == 0 || md_ctx->sb_origin_y == 0` — an SB on the
    /// picture's top or left edge has no left/top neighbour.
    pub is_edge_sb: bool,
    /// `ppcs->me_8x8_cost_variance[left_sb_index]`.
    pub left_me_8x8_cost_variance: u32,
    /// `ppcs->me_8x8_cost_variance[top_sb_index]`.
    pub top_me_8x8_cost_variance: u32,
    /// `ppcs->me_64x64_distortion[left_sb_index]`.
    pub left_me_64x64_distortion: u32,
    /// `ppcs->me_64x64_distortion[top_sb_index]`.
    pub top_me_64x64_distortion: u32,
    /// `pcs->sb_intra[left_sb_index]`.
    pub left_sb_intra: bool,
    /// `pcs->sb_intra[top_sb_index]`.
    pub top_sb_intra: bool,
    /// `pcs->sb_skip[left_sb_index]`.
    pub left_sb_skip: bool,
    /// `pcs->sb_skip[top_sb_index]`.
    pub top_sb_skip: bool,
    /// List 0's usable colocated reference, per [`RefSbInfo`].
    pub ref_l0: RefSbInfo,
    /// List 1's.
    pub ref_l1: RefSbInfo,
}

/// C `set_pd0_ctrls` (`enc_mode_config.c:5415`) — the `pcs->pic_pd0_lvl`
/// (0..=8) to [`Pd0Ctrls`] table the detector then walks down.
///
/// NOTE the two places where the LEVEL NUMBER and the `Pd0Level` disagree, and
/// they are C's, not a transcription slip: `lpd0_lvl` 5 AND 6 both land on
/// `PD0_LVL_5`, and 7 AND 8 both land on `PD0_LVL_6`. Each pair differs only in
/// the detector rows — 5 arms the `PD0_LVL_4` fallback row and `use_ref_info`
/// 1 at LVL_5, 6 disarms LVL_4 and sets `use_ref_info` 0 with double the
/// variance threshold.
///
/// `ctx->hbd_md` FORCES `PD0_LVL_0` before the switch runs; that is the
/// caller's fact (the bd10 path already routes to
/// [`crate::pd0::pd0_pick_sb_partition_lvl0`]) so it is not reproduced here.
///
/// # Evidence
///
/// TIER 4, like the rest of this module: `set_pd0_ctrls` is `static` in C and
/// its only caller is `svt_aom_sig_deriv_enc_dec_pd0`, which writes into a
/// `ModeDecisionContext` a shim would have to synthesise. The values are
/// transcribed from the switch and each row is pinned by
/// `ctrls_for_level_matches_c`.
///
/// # Panics
/// On a level outside 0..=8 — C `assert(0)`s there.
#[must_use]
pub fn pd0_ctrls_for_level(lpd0_lvl: u8) -> Pd0Ctrls {
    let mut c = Pd0Ctrls {
        pd0_level: Pd0Level::Lvl0,
        // C zeroes `use_pd0_detector` for every level at or below the chosen
        // one and leaves the rest untouched; the struct starts zeroed, so
        // "leaves untouched" and "false" are the same thing here.
        use_pd0_detector: [false; PD0_LEVELS],
        use_ref_info: [0; PD0_LEVELS],
        me_8x8_cost_variance_th: [0; PD0_LEVELS],
        edge_dist_th: [0; PD0_LEVELS],
        neigh_me_dist_shift: [u16::MAX; PD0_LEVELS],
    };
    const L4: usize = Pd0Level::Lvl4 as usize;
    const L5: usize = Pd0Level::Lvl5 as usize;
    const L6: usize = Pd0Level::Lvl6 as usize;
    match lpd0_lvl {
        0 => c.pd0_level = Pd0Level::Lvl0,
        1 => c.pd0_level = Pd0Level::Lvl1,
        2 => c.pd0_level = Pd0Level::Lvl2,
        3 => c.pd0_level = Pd0Level::Lvl3,
        4 | 5 => {
            c.pd0_level = if lpd0_lvl == 4 {
                Pd0Level::Lvl4
            } else {
                Pd0Level::Lvl5
            };
            c.use_pd0_detector[L4] = true;
            c.use_ref_info[L4] = 2;
            c.me_8x8_cost_variance_th[L4] = 250_000;
            c.edge_dist_th[L4] = 16384;
            c.neigh_me_dist_shift[L4] = 3;
            if lpd0_lvl == 5 {
                c.use_pd0_detector[L5] = true;
                c.use_ref_info[L5] = 1;
                c.me_8x8_cost_variance_th[L5] = 250_000 >> 1;
                c.edge_dist_th[L5] = 16384;
                c.neigh_me_dist_shift[L5] = 2;
            }
        }
        6 => {
            c.pd0_level = Pd0Level::Lvl5;
            c.use_pd0_detector[L5] = true;
            c.use_ref_info[L5] = 0;
            c.me_8x8_cost_variance_th[L5] = 500_000;
            c.edge_dist_th[L5] = 16384;
            c.neigh_me_dist_shift[L5] = 2;
        }
        7 | 8 => {
            c.pd0_level = Pd0Level::Lvl6;
            c.use_pd0_detector[L5] = true;
            c.use_ref_info[L5] = 0;
            c.me_8x8_cost_variance_th[L5] = 500_000 << 1;
            c.edge_dist_th[L5] = u32::MAX;
            c.neigh_me_dist_shift[L5] = u16::MAX;
            c.use_pd0_detector[L6] = true;
            c.use_ref_info[L6] = if lpd0_lvl == 7 { 1 } else { 2 };
            c.me_8x8_cost_variance_th[L6] = 250_000;
            c.edge_dist_th[L6] = 16384;
            c.neigh_me_dist_shift[L6] = 2;
        }
        other => panic!("set_pd0_ctrls: lpd0_lvl {other} is outside 0..=8 (C asserts)"),
    }
    c
}

/// C `pd0_detector` (enc_dec_process.c:2406) — the light-PD0 classifier.
///
/// Walks the level ladder downward; at each level that matches the current
/// `pd0_level`, applies that level's tests and steps down on any failure.
///
/// The tests, in C's order:
/// 1. **`PD0_LVL_6` needs inter.** An I_SLICE, or a frame with
///    `transition_present`, cannot use the lightest path at all, because it
///    only supports INTER compensation. This test runs whether or not the
///    detector is enabled.
/// 2. **Reference agreement** (`use_ref_info`, inter only), in three
///    increasingly aggressive forms — see the arms below.
/// 3. **ME cost variance** against a QP-scaled threshold.
/// 4. **Neighbour comparison**, or an absolute distortion threshold for an SB
///    on the picture edge that has no neighbours.
///
/// Returns the final level. C's closing assert — an I_SLICE can never end at
/// `PD0_LVL_6` — is reproduced as a `debug_assert`.
#[must_use]
pub fn pd0_detector(ctrls: &Pd0Ctrls, sb: &Pd0SbInput) -> Pd0Level {
    let mut level = ctrls.pd0_level;
    // C: `for (pd0_lvl = PD0_LEVELS - 1; pd0_lvl > PD0_LVL_0; pd0_lvl--)`,
    // with the body a no-op unless `pd0_level == pd0_lvl`. Because the body
    // only ever DECREMENTS, the loop is equivalent to "re-test at each level
    // we land on, walking down", which is what this expresses.
    for lvl_idx in (1..PD0_LEVELS).rev() {
        let lvl = Pd0Level::from_index(lvl_idx).expect("in range");
        if level != lvl {
            continue;
        }
        let lower = Pd0Level::from_index(lvl_idx - 1).expect("lvl_idx >= 1");

        // 1. VERY_LIGHT_PD0 supports INTER compensation only.
        if (sb.slice_type_is_intra || sb.transition_present) && lvl == Pd0Level::Lvl6 {
            level = lower;
            continue;
        }

        if !ctrls.use_pd0_detector[lvl_idx] {
            continue;
        }

        // 2. Reference agreement.
        let use_ref_info = ctrls.use_ref_info[lvl_idx];
        if use_ref_info != 0 && !sb.slice_type_is_intra {
            let (l0_refs, l0_was_intra) = (sb.ref_l0.refs(), sb.ref_l0.was_intra());
            let (l1_refs, l1_was_intra) = (sb.ref_l1.refs(), sb.ref_l1.was_intra());
            let stepped = match use_ref_info {
                // Level 1 (safest): EITHER usable reference was intra here.
                1 => (l0_refs && l0_was_intra != 0) || (l1_refs && l1_was_intra != 0),
                // Level 2: at least one usable reference, and EVERY usable one
                // was intra. Note `!l0_refs || ...` makes an ABSENT list vacuously
                // agree — that is the difference from level 1 and it is easy to
                // read as a bug.
                2 => {
                    (l0_refs || l1_refs)
                        && (!l0_refs || l0_was_intra != 0)
                        && (!l1_refs || l1_was_intra != 0)
                }
                // Level 3 (most aggressive): level 2 AND the picture-level
                // intra percentage is above a QP-falling floor.
                _ => {
                    (l0_refs || l1_refs)
                        && (!l0_refs || l0_was_intra != 0)
                        && (!l1_refs || l1_was_intra != 0)
                        && sb.ref_intra_percentage > 1.max(50u32.saturating_sub(sb.picture_qp >> 1))
                }
            };
            if stepped {
                level = lower;
                continue;
            }
        }

        // 3 and 4 need ME info, which an I_SLICE does not have.
        if sb.slice_type_is_intra {
            continue;
        }

        // 3. ME cost variance against a QP-scaled threshold.
        // C guards on `th < ((uint32_t)~0) >> 1` so `(th >> 5) * picture_qp`
        // (QP at most 63) cannot overflow; a threshold at or above that
        // sentinel disables the test rather than passing it.
        let th = ctrls.me_8x8_cost_variance_th[lvl_idx];
        if th < (u32::MAX >> 1) && sb.me_8x8_cost_variance > (th >> 5) * sb.picture_qp {
            level = lower;
            continue;
        }

        // 4. Neighbours, or the edge fallback.
        if sb.is_edge_sb {
            if sb.me_64x64_distortion > ctrls.edge_dist_th[lvl_idx] {
                level = lower;
            }
        } else {
            let shift = ctrls.neigh_me_dist_shift[lvl_idx];
            let neigh_enabled = shift != u16::MAX;
            // The two neighbour sums are `uint32_t` additions in C and can
            // wrap; `wrapping_add` and `wrapping_shl` keep that rather than
            // panicking in debug on a pathological input.
            let dist_sum = sb
                .left_me_64x64_distortion
                .wrapping_add(sb.top_me_64x64_distortion)
                .wrapping_shl(u32::from(shift));
            let var_sum = sb
                .left_me_8x8_cost_variance
                .wrapping_add(sb.top_me_8x8_cost_variance)
                .wrapping_shl(u32::from(shift));
            // C spells this as three `else if` arms whose first two bodies
            // are identical (`pd0_level = pd0_lvl - 1`), which is the same
            // predicate OR'd — merged here so the third arm's "neither
            // neighbour test fired" condition stays obvious.
            if neigh_enabled
                && (sb.me_64x64_distortion > dist_sum || sb.me_8x8_cost_variance > var_sum)
            {
                level = lower;
            } else if use_ref_info != 0 {
                // Use info from neighbouring SBs. Again two identical C
                // bodies: BOTH neighbours intra, or exactly one intra with
                // NEITHER skipped.
                let both_intra = sb.left_sb_intra && sb.top_sb_intra;
                let one_intra_none_skipped =
                    !sb.left_sb_skip && !sb.top_sb_skip && (sb.left_sb_intra || sb.top_sb_intra);
                if both_intra || one_intra_none_skipped {
                    level = lower;
                }
            }
        }
    }
    debug_assert!(
        !(sb.slice_type_is_intra && level == Pd0Level::Lvl6),
        "an I_SLICE must never end at PD0_LVL_6"
    );
    level
}

/// The `(q_weight, q_weight_denom)` pair C's EXPORTED
/// `svt_aom_get_qp_based_th_scaling_factors` (`enc_mode_config.c:25`)
/// produces.
///
/// It lives in a different file, so it is an INPUT here rather than something
/// this module re-derives — the same file-boundary rule the rest of this lane
/// follows. `scaling_enabled == false` yields `(1, 1)`, i.e. no scaling.
#[derive(Clone, Copy, Debug)]
pub struct QpThScaling {
    pub q_weight: u32,
    pub q_weight_denom: u32,
}

impl Default for QpThScaling {
    fn default() -> Self {
        Self {
            q_weight: 1,
            q_weight_denom: 1,
        }
    }
}

impl QpThScaling {
    /// C `DIVIDE_AND_ROUND(x * q_weight, q_weight_denom)` — round-half-up on
    /// a non-negative numerator, done in 64 bits because `x * q_weight` with
    /// the exponential branch's 10000-scale weights overflows `i32` for large
    /// thresholds.
    #[must_use]
    pub fn scale(&self, x: i32) -> i32 {
        let num = i64::from(x) * i64::from(self.q_weight);
        let den = i64::from(self.q_weight_denom);
        ((num + den / 2) / den) as i32
    }
}

/// C `ME_TIER_ZERO_PU_*` variance slots this detector reads: one 64x64, four
/// 32x32 and sixteen 16x16.
#[derive(Clone, Copy, Debug, Default)]
pub struct SbVariance {
    /// `sb_var[ME_TIER_ZERO_PU_64x64]`.
    pub var64: u32,
    /// `sb_var[ME_TIER_ZERO_PU_32x32_0 ..= _3]`.
    pub var32: [u32; 4],
    /// `sb_var[ME_TIER_ZERO_PU_16x16_0 ..= _15]`.
    pub var16: [u32; 16],
}

/// The normalised per-pixel variances `pd0_detector_allintra` compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NormalisedVariance {
    pub norm_v64: i32,
    pub norm_v32: i32,
    pub norm_v16: i32,
}

/// C's MAINLINE variance accumulation (`#else` arm, enc_dec_process.c:2353).
///
/// `int32_t` accumulators, normalised per block with a SHIFT (`>> 2`, `>> 4`)
/// and then per pixel with a multiply (`* 4`, `* 16`).
///
/// The shift is what distinguishes this from the fork: `>> 2` on an `int32_t`
/// TRUNCATES TOWARD NEGATIVE INFINITY for a negative sum and toward zero for a
/// positive one, while the fork's `/ 4` on a `double` does neither. Variances
/// are non-negative in practice, so the two agree — but they are not the same
/// rule and the port does not pretend they are.
#[must_use]
pub fn accumulate(var: &SbVariance) -> NormalisedVariance {
    let var64 = var.var64 as i32;
    let mut var32 = 0_i32;
    for &v in &var.var32 {
        var32 = var32.wrapping_add(v as i32);
    }
    let mut var16 = 0_i32;
    for &v in &var.var16 {
        var16 = var16.wrapping_add(v as i32);
    }
    // Normalize per block.
    let var32 = var32 >> 2; // 4 x 32x32
    let var16 = var16 >> 4; // 16 x 16x16
    // Normalize per pixel.
    const SCALE_32: i32 = (64 * 64) / (32 * 32); // 4
    const SCALE_16: i32 = (64 * 64) / (16 * 16); // 16
    NormalisedVariance {
        norm_v64: var64,
        norm_v32: var32.wrapping_mul(SCALE_32),
        norm_v16: var16.wrapping_mul(SCALE_16),
    }
}

/// C's `SVT_HDR_MODE` variance accumulation (enc_dec_process.c:2350).
///
/// `double` accumulators normalised with `/ 4` and `/ 16`, then cast to
/// `int32_t` (which truncates TOWARD ZERO, unlike the mainline shift). NOT
/// what a mainline build compiles — see the module header.
#[must_use]
pub fn accumulate_fork(var: &SbVariance) -> NormalisedVariance {
    let var64 = f64::from(var.var64);
    let var32: f64 = var.var32.iter().map(|&v| f64::from(v)).sum::<f64>() / 4.0;
    let var16: f64 = var.var16.iter().map(|&v| f64::from(v)).sum::<f64>() / 16.0;
    const SCALE_32: f64 = 4.0;
    const SCALE_16: f64 = 16.0;
    NormalisedVariance {
        norm_v64: var64 as i32,
        norm_v32: (var32 * SCALE_32) as i32,
        norm_v16: (var16 * SCALE_16) as i32,
    }
}

/// C `DELTA_VAR_TH` (enc_dec_process.c:2396, a local) — the threshold below
/// which no depth dominates.
pub const DELTA_VAR_TH: i32 = 7500;

/// C `pd0_detector_allintra` (enc_dec_process.c:2341).
///
/// Steps the level down by ONE when the 64/32/16 normalised variances are all
/// within `delta_var_th` of each other — i.e. no block size dominates, so the
/// lightest path's fixed depth choice would be a guess.
///
/// Only runs at `PD0_LVL_6` or above; C returns immediately below that.
///
/// `norm` selects the mainline ([`accumulate`]) or fork
/// ([`accumulate_fork`]) accumulation — see the module header for why that is
/// the caller's choice.
#[must_use]
pub fn pd0_detector_allintra(
    level: Pd0Level,
    norm: &NormalisedVariance,
    qp_scaling: &QpThScaling,
) -> Pd0Level {
    if level < Pd0Level::Lvl6 {
        return level;
    }
    let delta_var_th = qp_scaling.scale(DELTA_VAR_TH);
    if (norm.norm_v32 - norm.norm_v64).abs() < delta_var_th
        && (norm.norm_v16 - norm.norm_v32).abs() < delta_var_th
    {
        // C `md_ctx->pd0_ctrls.pd0_level--`.
        return Pd0Level::from_index(level as usize - 1).expect("level >= Lvl6 > 0");
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVIDENCE TIER 4** for every test here: both C functions are `static`
    /// and were inlined away, and their only caller is the encode-decode
    /// thread body. Vectors are hand-derived from the C source at the cited
    /// lines.
    const _: () = ();

    /// `set_pd0_ctrls` (enc_mode_config.c:5415), row for row.
    ///
    /// The two aliasing pairs are the point of this test: 5 and 6 both give
    /// `Lvl5`, 7 and 8 both give `Lvl6`, and they are told apart only by the
    /// detector rows below them.
    #[test]
    fn ctrls_for_level_matches_c() {
        for (lvl, want) in [
            (0u8, Pd0Level::Lvl0),
            (1, Pd0Level::Lvl1),
            (2, Pd0Level::Lvl2),
            (3, Pd0Level::Lvl3),
            (4, Pd0Level::Lvl4),
            (5, Pd0Level::Lvl5),
            (6, Pd0Level::Lvl5),
            (7, Pd0Level::Lvl6),
            (8, Pd0Level::Lvl6),
        ] {
            assert_eq!(pd0_ctrls_for_level(lvl).pd0_level, want, "lpd0_lvl {lvl}");
        }
        // 0..=3 arm NO detector at all.
        for lvl in 0..=3u8 {
            let c = pd0_ctrls_for_level(lvl);
            assert!(
                c.use_pd0_detector.iter().all(|d| !d),
                "lpd0_lvl {lvl} must arm no detector"
            );
        }
        let l4 = pd0_ctrls_for_level(4);
        assert!(l4.use_pd0_detector[Pd0Level::Lvl4 as usize]);
        assert_eq!(l4.use_ref_info[Pd0Level::Lvl4 as usize], 2);
        assert_eq!(l4.me_8x8_cost_variance_th[Pd0Level::Lvl4 as usize], 250_000);
        assert_eq!(l4.neigh_me_dist_shift[Pd0Level::Lvl4 as usize], 3);
        assert!(!l4.use_pd0_detector[Pd0Level::Lvl5 as usize]);

        let l5 = pd0_ctrls_for_level(5);
        assert!(l5.use_pd0_detector[Pd0Level::Lvl4 as usize]);
        assert!(l5.use_pd0_detector[Pd0Level::Lvl5 as usize]);
        assert_eq!(l5.use_ref_info[Pd0Level::Lvl5 as usize], 1);
        assert_eq!(l5.me_8x8_cost_variance_th[Pd0Level::Lvl5 as usize], 125_000);
        assert_eq!(l5.neigh_me_dist_shift[Pd0Level::Lvl5 as usize], 2);

        // 6 DISARMS the LVL_4 row that 5 arms, and drops use_ref_info to 0.
        let l6 = pd0_ctrls_for_level(6);
        assert!(!l6.use_pd0_detector[Pd0Level::Lvl4 as usize]);
        assert_eq!(l6.use_ref_info[Pd0Level::Lvl5 as usize], 0);
        assert_eq!(l6.me_8x8_cost_variance_th[Pd0Level::Lvl5 as usize], 500_000);

        let l7 = pd0_ctrls_for_level(7);
        assert_eq!(l7.edge_dist_th[Pd0Level::Lvl5 as usize], u32::MAX);
        assert_eq!(l7.neigh_me_dist_shift[Pd0Level::Lvl5 as usize], u16::MAX);
        assert_eq!(l7.use_ref_info[Pd0Level::Lvl6 as usize], 1);
        assert_eq!(
            pd0_ctrls_for_level(8).use_ref_info[Pd0Level::Lvl6 as usize],
            2
        );
    }

    /// The wiring fact `pipeline.rs` depends on, as a test rather than a
    /// comment: on the port's low-delay-P envelope the L0 reference is the KEY
    /// frame, whose every SB is intra, and the ladder then walks
    /// `Lvl5 -> Lvl4 -> Lvl3` on the REFERENCE tests alone — no ME threshold is
    /// ever consulted, so the answer does not depend on per-SB ME data.
    ///
    /// The I-slice control is the other half: on frame 0 the same picture
    /// levels are a NO-OP, which is why the key frame keeps 3 / 4 / 5.
    #[test]
    fn an_all_intra_l0_reference_walks_every_level_down_to_lvl3() {
        let inter_sb = |was_intra: u8| Pd0SbInput {
            slice_type_is_intra: false,
            ref_l0: RefSbInfo {
                was_intra: Some(was_intra),
            },
            // Deliberately EXTREME, so that if the reference arm did not fire
            // the ME arm certainly would and the test could not pass by
            // accident on the same answer.
            me_8x8_cost_variance: u32::MAX / 4,
            me_64x64_distortion: u32::MAX / 4,
            picture_qp: 40,
            is_edge_sb: true,
            ..Pd0SbInput::default()
        };
        for lpd0_lvl in [3u8, 4, 5] {
            let ctrls = pd0_ctrls_for_level(lpd0_lvl);
            assert_eq!(
                pd0_detector(&ctrls, &inter_sb(1)),
                Pd0Level::Lvl3,
                "lpd0_lvl {lpd0_lvl} with an all-intra L0 reference"
            );
        }
        // Positive control: with a NON-intra reference the ladder does NOT
        // stop at Lvl3 for every input — level 5 still steps (the ME arm
        // fires on the extreme values above), which proves the assert above is
        // reading the reference arm and not a constant.
        let ctrls5 = pd0_ctrls_for_level(5);
        assert_ne!(pd0_detector(&ctrls5, &inter_sb(0)), Pd0Level::Lvl5);

        // I-slice: levels 3/4/5 are untouched, because every test in the body
        // is gated on `slice_type != I_SLICE`.
        for (lpd0_lvl, want) in [
            (3u8, Pd0Level::Lvl3),
            (4, Pd0Level::Lvl4),
            (5, Pd0Level::Lvl5),
        ] {
            let ctrls = pd0_ctrls_for_level(lpd0_lvl);
            let sb = Pd0SbInput {
                slice_type_is_intra: true,
                me_8x8_cost_variance: u32::MAX / 4,
                me_64x64_distortion: u32::MAX / 4,
                picture_qp: 40,
                is_edge_sb: true,
                ..Pd0SbInput::default()
            };
            assert_eq!(
                pd0_detector(&ctrls, &sb),
                want,
                "I-slice lpd0_lvl {lpd0_lvl}"
            );
        }
    }

    fn ctrls_at(level: Pd0Level) -> Pd0Ctrls {
        Pd0Ctrls {
            pd0_level: level,
            use_pd0_detector: [true; PD0_LEVELS],
            use_ref_info: [0; PD0_LEVELS],
            // Disable tests 3 and 4 unless a test turns them on.
            me_8x8_cost_variance_th: [u32::MAX; PD0_LEVELS],
            edge_dist_th: [u32::MAX; PD0_LEVELS],
            neigh_me_dist_shift: [u16::MAX; PD0_LEVELS],
        }
    }

    fn inter_sb() -> Pd0SbInput {
        Pd0SbInput {
            slice_type_is_intra: false,
            picture_qp: 32,
            ..Default::default()
        }
    }

    /// `PD0_LVL_6` supports INTER compensation only, so an I_SLICE (or a
    /// transition frame) is knocked down BEFORE the detector even runs — and
    /// C's closing assert says so.
    #[test]
    fn lvl6_requires_inter() {
        let mut c = ctrls_at(Pd0Level::Lvl6);
        c.use_pd0_detector = [false; PD0_LEVELS];
        let mut sb = inter_sb();
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl6);

        sb.slice_type_is_intra = true;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);

        let mut sb = inter_sb();
        sb.transition_present = true;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);
    }

    /// The walk is a LADDER: one call can step down several levels, because
    /// each level it lands on is re-tested with its own thresholds.
    #[test]
    fn the_detector_can_step_down_several_levels_in_one_call() {
        let mut c = ctrls_at(Pd0Level::Lvl6);
        // Every level's ME-variance test fails.
        c.me_8x8_cost_variance_th = [0; PD0_LEVELS];
        let mut sb = inter_sb();
        sb.me_8x8_cost_variance = 1;
        // th == 0 -> (0 >> 5) * qp == 0, and 1 > 0, so every level steps down;
        // the loop bound stops at level 0.
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
    }

    /// `use_pd0_detector == false` skips tests 2-4 but NOT the LVL_6 inter
    /// requirement, which is outside the guard.
    #[test]
    fn detector_disabled_still_enforces_the_lvl6_inter_rule() {
        let mut c = ctrls_at(Pd0Level::Lvl6);
        c.use_pd0_detector = [false; PD0_LEVELS];
        c.me_8x8_cost_variance_th = [0; PD0_LEVELS];
        let mut sb = inter_sb();
        sb.me_8x8_cost_variance = u32::MAX;
        // Tests 2-4 are skipped, so an inter SB keeps LVL_6...
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl6);
        // ...but an intra one is still knocked down once.
        sb.slice_type_is_intra = true;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl5);
    }

    /// The three `use_ref_info` arms differ, and level 2's `!l0_refs || ...`
    /// makes an ABSENT list vacuously agree — which reads like a bug and is
    /// not.
    #[test]
    fn use_ref_info_arms_differ() {
        // The arm is set at level 4 ONLY, so a step-down lands on level 3
        // where `use_ref_info == 0` and the ladder stops. Setting it at every
        // level instead walks all the way to 0 — which is correct C behaviour
        // and is what a first draft of this test measured by accident.
        let mk = |arm: u8, l0: Option<u8>, l1: Option<u8>, ref_intra_pct: u32| {
            let mut c = ctrls_at(Pd0Level::Lvl4);
            c.use_ref_info[4] = arm;
            let mut sb = inter_sb();
            sb.ref_l0 = RefSbInfo { was_intra: l0 };
            sb.ref_l1 = RefSbInfo { was_intra: l1 };
            sb.ref_intra_percentage = ref_intra_pct;
            pd0_detector(&c, &sb)
        };
        // Arm 1: EITHER present-and-intra reference steps down.
        assert_eq!(mk(1, Some(1), None, 0), Pd0Level::Lvl3);
        assert_eq!(mk(1, Some(0), None, 0), Pd0Level::Lvl4);
        // Arm 2: one list present and intra, the OTHER ABSENT -> still steps
        // down, because an absent list satisfies `!lN_refs`.
        assert_eq!(mk(2, Some(1), None, 0), Pd0Level::Lvl3);
        // ...but a present-and-NOT-intra list blocks it.
        assert_eq!(mk(2, Some(1), Some(0), 0), Pd0Level::Lvl4);
        // Arm 1 would have stepped down on that same input.
        assert_eq!(mk(1, Some(1), Some(0), 0), Pd0Level::Lvl3);
        // Arm 3 adds the intra-percentage floor: at qp 32 the floor is
        // max(1, 50 - 16) == 34.
        assert_eq!(mk(3, Some(1), None, 34), Pd0Level::Lvl4);
        assert_eq!(mk(3, Some(1), None, 35), Pd0Level::Lvl3);
    }

    /// The ME-variance threshold's sentinel: at or above `u32::MAX >> 1` the
    /// test is DISABLED, not trivially passed.
    #[test]
    fn me_variance_threshold_sentinel_disables_the_test() {
        let mut c = ctrls_at(Pd0Level::Lvl4);
        c.me_8x8_cost_variance_th = [u32::MAX >> 1; PD0_LEVELS];
        let mut sb = inter_sb();
        sb.me_8x8_cost_variance = u32::MAX;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl4);
        // One below the sentinel the test is live, and `(th >> 5) * qp`
        // is the comparison.
        c.me_8x8_cost_variance_th = [(u32::MAX >> 1) - 1; PD0_LEVELS];
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
    }

    /// An edge SB has no neighbours, so it uses the absolute
    /// `edge_dist_th` instead of the neighbour comparison.
    #[test]
    fn edge_sb_uses_the_absolute_distortion_threshold() {
        let mut c = ctrls_at(Pd0Level::Lvl4);
        c.edge_dist_th = [1000; PD0_LEVELS];
        c.neigh_me_dist_shift = [0; PD0_LEVELS];
        let mut sb = inter_sb();
        sb.is_edge_sb = true;
        sb.me_64x64_distortion = 1001;
        // Steps down at 4, then again at 3, 2, 1 — the same absolute test
        // fails at every level.
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
        sb.me_64x64_distortion = 1000;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl4);
        // A non-edge SB with the same numbers uses the neighbour sum instead,
        // which is 0 + 0 here, so it steps down for a different reason.
        sb.is_edge_sb = false;
        sb.me_64x64_distortion = 1;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
    }

    /// `neigh_me_dist_shift == u16::MAX` disables BOTH neighbour comparisons
    /// but leaves the `use_ref_info` neighbour-intra fallback live.
    #[test]
    fn neighbour_shift_sentinel_leaves_the_ref_info_fallback_live() {
        let mut c = ctrls_at(Pd0Level::Lvl4);
        c.neigh_me_dist_shift = [u16::MAX; PD0_LEVELS];
        c.use_ref_info = [1; PD0_LEVELS];
        let mut sb = inter_sb();
        sb.me_64x64_distortion = u32::MAX;
        // Both neighbours intra -> step down, even with the shift disabled.
        sb.left_sb_intra = true;
        sb.top_sb_intra = true;
        assert_eq!(pd0_detector(&c, &sb), Pd0Level::Lvl0);
        // One intra, neither skipped -> also steps down.
        let mut sb2 = inter_sb();
        sb2.left_sb_intra = true;
        assert_eq!(pd0_detector(&c, &sb2), Pd0Level::Lvl0);
        // One intra but a neighbour IS skipped -> stays.
        let mut sb3 = inter_sb();
        sb3.left_sb_intra = true;
        sb3.left_sb_skip = true;
        assert_eq!(pd0_detector(&c, &sb3), Pd0Level::Lvl4);
    }

    /// The mainline accumulation shifts; the fork divides. They agree on
    /// non-negative input, which is why citing the wrong arm is easy.
    #[test]
    fn mainline_and_fork_accumulation_agree_on_real_variances() {
        let v = SbVariance {
            var64: 1000,
            var32: [900, 1100, 1000, 1000],
            var16: [1000; 16],
        };
        let m = accumulate(&v);
        assert_eq!(m.norm_v64, 1000);
        assert_eq!(m.norm_v32, (4000 >> 2) * 4);
        assert_eq!(m.norm_v16, (16000 >> 4) * 16);
        assert_eq!(accumulate_fork(&v), m);
        // They diverge where the shift and the divide disagree: a per-block
        // sum that is not a multiple of the block count.
        let v2 = SbVariance {
            var64: 0,
            var32: [1, 1, 1, 0],
            var16: [0; 16],
        };
        // mainline: (3 >> 2) * 4 == 0; fork: (3/4) * 4 == 3.0 -> 3.
        assert_eq!(accumulate(&v2).norm_v32, 0);
        assert_eq!(accumulate_fork(&v2).norm_v32, 3);
    }

    /// `pd0_detector_allintra` steps down by ONE when no depth dominates, and
    /// does nothing below `PD0_LVL_6`.
    #[test]
    fn allintra_steps_down_when_no_depth_dominates() {
        let flat = NormalisedVariance {
            norm_v64: 1000,
            norm_v32: 1100,
            norm_v16: 1200,
        };
        let peaky = NormalisedVariance {
            norm_v64: 1000,
            norm_v32: 20_000,
            norm_v16: 1200,
        };
        let s = QpThScaling::default();
        assert_eq!(
            pd0_detector_allintra(Pd0Level::Lvl6, &flat, &s),
            Pd0Level::Lvl5
        );
        assert_eq!(
            pd0_detector_allintra(Pd0Level::Lvl6, &peaky, &s),
            Pd0Level::Lvl6
        );
        // Below LVL_6 it returns immediately.
        assert_eq!(
            pd0_detector_allintra(Pd0Level::Lvl5, &flat, &s),
            Pd0Level::Lvl5
        );
    }

    /// `DIVIDE_AND_ROUND` is round-half-up, and the 10000-scale weights need
    /// 64-bit intermediates.
    #[test]
    fn qp_scaling_rounds_half_up_in_64_bits() {
        let s = QpThScaling {
            q_weight: 1,
            q_weight_denom: 2,
        };
        assert_eq!(s.scale(3), 2, "1.5 rounds up");
        assert_eq!(s.scale(5), 3, "2.5 rounds up");
        // The exponential branch's shape: q_weight ~ 7000, denom 10000.
        let s = QpThScaling {
            q_weight: 7000,
            q_weight_denom: 10000,
        };
        // 7500 * 7000 == 52_500_000, which fits i32 — but a caller with a
        // larger threshold would overflow it, so the port widens.
        assert_eq!(s.scale(DELTA_VAR_TH), 5250);
        assert_eq!(s.scale(i32::MAX / 2), 751_619_276);
    }
}

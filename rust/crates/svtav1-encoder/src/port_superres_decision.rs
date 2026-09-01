//! Coding-resolution DECISION — a port of the decision half of
//! `Codec/resize.c`: which superres denominator to use, which resize
//! denominator to use, and how the two are reconciled when they conflict.
//!
//! The scaling kernels themselves already exist
//! (`svtav1_dsp::resize` / `svtav1_dsp::superres`, `docs/superres-port-map.md`
//! chunks A and B.1). What was missing is everything that decides *whether and
//! by how much* to scale, which is why the port could only ever do
//! `SUPERRES_FIXED`.
//!
//! | Rust | C (`Codec/resize.c`) |
//! |---|---|
//! | [`analyze_hor_freq`] | `analyze_hor_freq` (1155) — static |
//! | [`energy_by_q2_thresh`] | `get_energy_by_q2_thresh` (1206) — static |
//! | [`superres_in_recode_allowed`] | `av1_superres_in_recode_allowed` (1223) — static |
//! | [`frame_update_type`] | `svt_aom_get_frame_update_type` (1246) — **EXPORTED** |
//! | [`denom_from_qindex_energy`] | `get_superres_denom_from_qindex_energy` (1232) — static |
//! | [`denom_for_qindex`] | `get_superres_denom_for_qindex` (1267) — static |
//! | [`calc_superres_params`] | `calc_superres_params` (1311) — static |
//! | [`denom_idx`] | `svt_aom_get_denom_idx` (1425) — **EXPORTED** |
//! | [`calculate_next_resize_scale`] | `calculate_next_resize_scale` (1855) — static |
//! | [`dimension_is_ok`] | `dimension_is_ok` (1906) — static |
//! | [`dimensions_are_ok`] | `dimensions_are_ok` (1910) — static |
//! | [`validate_size_scales`] | `validate_size_scales` (1916) — static |
//!
//! # How the decision works
//!
//! [`analyze_hor_freq`] runs a horizontal-only 16x4 DCT over the luma plane
//! and accumulates a per-frequency energy histogram, then converts it to a
//! CUMULATIVE tail: `energy[k]` is the energy at or above frequency `k`.
//! [`denom_from_qindex_energy`] walks that tail down from the highest
//! frequency and stops at the first band whose energy exceeds a
//! quantizer-scaled threshold — that band is the highest frequency worth
//! coding, and the denominator follows from it. A frame whose horizontal
//! detail dies out early can be coded narrower and upscaled for free.
//!
//! # Evidence
//!
//! Tier 1 for the two exported symbols ([`frame_update_type`], [`denom_idx`])
//! via `crates/svtav1-cref/shims/refmgmt_shims.c`. Tier 4 for the rest, which
//! are `static`; `tests/c_parity_superres_decision.rs` carries the
//! derivations, and [`analyze_hor_freq`] is additionally anchored on the
//! already-tier-1 forward transform it is built from.

use svtav1_dsp::superres::SCALE_NUMERATOR;
use svtav1_types::transform::{TxSize, TxType};

use crate::port_picstruct::FrameUpdateType;

/// C `SUPERRES_ENERGY_BY_Q2_THRESH_KEYFRAME_SOLO` (`resize.c:1200`).
pub const ENERGY_BY_Q2_THRESH_KEYFRAME_SOLO: f64 = 0.012;
/// C `SUPERRES_ENERGY_BY_Q2_THRESH_KEYFRAME` (`resize.c:1201`).
pub const ENERGY_BY_Q2_THRESH_KEYFRAME: f64 = 0.008;
/// C `SUPERRES_ENERGY_BY_Q2_THRESH_ARFFRAME` (`resize.c:1202`).
pub const ENERGY_BY_Q2_THRESH_ARFFRAME: f64 = 0.008;
/// C `SUPERRES_ENERGY_BY_AC_THRESH` (`resize.c:1203`).
pub const ENERGY_BY_AC_THRESH: f64 = 0.2;
/// C `NUM_SR_SCALES` (`resize.h`) — the eight scalable denominators 9..=16.
pub const NUM_SR_SCALES: usize = 8;

/// C `SUPERRES_MODE` (`API/EbSvtAv1Enc.h:108-121`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperresMode {
    /// C `SUPERRES_NONE = 0` — the default; no scaling.
    None = 0,
    /// C `SUPERRES_FIXED = 1` — one configured denominator.
    Fixed = 1,
    /// C `SUPERRES_RANDOM = 2` — a pseudo-random denominator per frame.
    Random = 2,
    /// C `SUPERRES_QTHRESH = 3` — energy-derived above a quantizer threshold.
    Qthresh = 3,
    /// C `SUPERRES_AUTO = 4` — an RD search over denominators.
    Auto = 4,
}

/// C `SUPERRES_AUTO_SEARCH_TYPE` (`API/EbSvtAv1Enc.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperresAutoSearch {
    /// C `SUPERRES_AUTO_SOLO = 0` — try one energy-derived denominator only.
    Solo = 0,
    /// C `SUPERRES_AUTO_DUAL = 1` — that denominator plus full resolution.
    Dual = 1,
    /// C `SUPERRES_AUTO_ALL = 2` — every denominator plus full resolution.
    All = 2,
}

/// The `scs->static_config` fields the superres decision reads.
#[derive(Debug, Clone, Copy)]
pub struct SuperresConfig {
    /// C `superres_mode`.
    pub mode: SuperresMode,
    /// C `superres_denom` — the non-key-frame denominator in FIXED mode.
    pub denom: u8,
    /// C `superres_kf_denom` — the key-frame denominator in FIXED mode.
    pub kf_denom: u8,
    /// C `superres_qthres` — a QP (not a qindex); C converts it.
    pub qthres: u8,
    /// C `superres_kf_qthres` — likewise.
    pub kf_qthres: u8,
    /// C `superres_auto_search_type`.
    pub auto_search_type: SuperresAutoSearch,
}

/// The per-picture inputs the decision reads.
#[derive(Debug, Clone, Copy)]
pub struct SuperresPicInput {
    /// C `pcs->frm_hdr.allow_intrabc`.
    pub allow_intrabc: bool,
    /// C `pcs->frm_hdr.allow_screen_content_tools`.
    pub allow_screen_content_tools: bool,
    /// C `frame_is_intra_only(pcs)`.
    pub is_intra_only: bool,
    /// C `pcs->picture_qp` — a QP; C converts it through
    /// `quantizer_to_qindex`.
    pub picture_qp: u8,
    /// C `svt_aom_get_frame_update_type(pcs)`.
    pub update_type: FrameUpdateType,
    /// C `scs->enc_ctx->rc.frames_to_key`.
    pub frames_to_key: i32,
}

/// What [`calc_superres_params`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperresDecision {
    /// C `spr_params->superres_denom`.
    pub denom: u8,
    /// C `pcs->superres_total_recode_loop`.
    pub total_recode_loop: u8,
    /// C `pcs->superres_recode_loop`.
    pub recode_loop: u8,
    /// C `pcs->superres_denom_array[NUM_SR_SCALES + 1]`.
    pub denom_array: [u8; NUM_SR_SCALES + 1],
}

impl Default for SuperresDecision {
    fn default() -> Self {
        Self {
            denom: SCALE_NUMERATOR as u8,
            total_recode_loop: 0,
            recode_loop: 0,
            denom_array: [0; NUM_SR_SCALES + 1],
        }
    }
}

/// C `svt_aom_get_frame_update_type` (`resize.c:1246`) — **EXPORTED**.
///
/// Deliberately NOT `gf_group->update_type`: C's own comment records that the
/// group array is only valid in the second pass of a two-pass encode and is
/// filled in the rate-control process, which runs after this. So the
/// derivation is repeated from the picture's own layer position.
///
/// It is the same shape as
/// [`crate::port_picstruct::set_frame_update_type`] EXCEPT at
/// `hierarchical_levels == 0`, where this returns `LF_UPDATE` unconditionally
/// while the picture-decision version splits on `frame_offset % 4`. Two
/// derivations of the same idea that disagree on flat GOPs is C's, not the
/// port's, and it is why this cannot just call that one.
#[must_use]
pub fn frame_update_type(
    is_key_frame: bool,
    hierarchical_levels: u8,
    temporal_layer_index: u8,
) -> FrameUpdateType {
    if is_key_frame {
        return FrameUpdateType::Kf;
    }
    if hierarchical_levels > 0 {
        if temporal_layer_index == 0 {
            FrameUpdateType::Arf
        } else if temporal_layer_index == hierarchical_levels {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    } else {
        FrameUpdateType::Lf
    }
}

/// C `svt_aom_get_denom_idx` (`resize.c:1425`) — **EXPORTED**.
///
/// Index into the downscaled-picture cache: 0 is the unscaled denominator 8.
/// C subtracts in `uint8_t`, so a denominator below 8 wraps rather than
/// saturating; reproduced with `wrapping_sub`.
#[must_use]
pub fn denom_idx(scale_denom: u8) -> u8 {
    scale_denom.wrapping_sub(SCALE_NUMERATOR as u8)
}

/// C `analyze_hor_freq` (`resize.c:1155`) — static.
///
/// A horizontal-only 16x4 DCT over the luma plane, accumulated into a
/// 16-entry per-frequency energy histogram and then converted to a cumulative
/// tail: on return `energy[k]` is the mean energy at frequencies `k..=15`.
/// Entry 0 is never written — C leaves the DC bin alone and its consumers
/// start at 1.
///
/// Traps, all transcribed:
///
/// * the loops are `i < height - 4` and `j < width - 16`, so the LAST partial
///   block row and column are skipped, and a picture narrower than 17 or
///   shorter than 5 pixels produces `n == 0`;
/// * with `n == 0` C fills every band with `1e+20`, a sentinel large enough
///   that [`denom_from_qindex_energy`] never scales;
/// * the per-block energy is rounded with `ROUND_POWER_OF_TWO(x, 2)` — i.e.
///   `(x + 2) >> 2` — BEFORE accumulation, not after;
/// * the cumulative pass runs `k` from 14 DOWN to 1 and reads `energy[k + 1]`,
///   so `energy[15]` stays a plain per-band mean and every lower entry is a
///   tail sum.
///
/// 10-bit input is handled by C as 8-bit: `y_buffer` points at the high byte
/// plane, so this takes the same 8-bit samples.
pub fn analyze_hor_freq(luma: &[u8], stride: usize, width: usize, height: usize) -> [f64; 16] {
    let mut freq_energy = [0u64; 16];
    let mut n = 0u64;

    let mut src16 = [0i32; 16 * 4];
    let mut coeff = [0i32; 16 * 4];

    let mut i = 0usize;
    while i + 4 < height {
        let mut j = 0usize;
        while j + 16 < width {
            for ii in 0..4 {
                let row = (i + ii) * stride + j;
                for jj in 0..16 {
                    src16[ii * 16 + jj] = i32::from(luma[row + jj]);
                }
            }
            svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                &src16,
                &mut coeff,
                16,
                TxSize::Tx16x4,
                TxType::HDct,
            );
            for k in 1..16 {
                let c0 = i64::from(coeff[k]);
                let c1 = i64::from(coeff[k + 16]);
                let c2 = i64::from(coeff[k + 32]);
                let c3 = i64::from(coeff[k + 48]);
                let this_energy = (c0 * c0 + c1 * c1 + c2 * c2 + c3 * c3) as u64;
                // C `ROUND_POWER_OF_TWO(this_energy, 2)`.
                freq_energy[k] += (this_energy + 2) >> 2;
            }
            n += 1;
            j += 16;
        }
        i += 4;
    }

    let mut energy = [0f64; 16];
    if n != 0 {
        for k in 1..16 {
            energy[k] = freq_energy[k] as f64 / n as f64;
        }
        for k in (1..15).rev() {
            energy[k] += energy[k + 1];
        }
    } else {
        for e in energy.iter_mut().skip(1) {
            *e = 1e+20;
        }
    }
    energy
}

/// C `get_energy_by_q2_thresh` (`resize.c:1206`) — static.
///
/// `None` where C `assert(0)`s and then returns 0: superres is only ever
/// considered for key frames and ARF frames, so any other update type is a
/// caller bug rather than a runtime case. Under `NDEBUG` C returns 0, which
/// would make EVERY band exceed the threshold and force the maximum
/// denominator — a silent worst case. Returning `None` refuses instead
/// (`docs/WORKING-ON-THIS.md` §6).
#[must_use]
pub fn energy_by_q2_thresh(frames_to_key: i32, update_type: FrameUpdateType) -> Option<f64> {
    match update_type {
        FrameUpdateType::Arf => Some(ENERGY_BY_Q2_THRESH_ARFFRAME),
        FrameUpdateType::Kf => Some(if frames_to_key <= 1 {
            ENERGY_BY_Q2_THRESH_KEYFRAME_SOLO
        } else {
            ENERGY_BY_Q2_THRESH_KEYFRAME
        }),
        _ => None,
    }
}

/// C `av1_superres_in_recode_allowed` (`resize.c:1223`) — static.
///
/// C's own comment: the `frames_to_key > 1` half of the condition is
/// COMMENTED OUT ("Empirically found to not be beneficial for image coding"),
/// so the test really is just the mode and the search type. Reproduced as
/// written, with the dead clause noted rather than silently reinstated.
#[must_use]
pub fn superres_in_recode_allowed(cfg: &SuperresConfig) -> bool {
    cfg.mode == SuperresMode::Auto && cfg.auto_search_type != SuperresAutoSearch::Solo
}

/// C `get_superres_denom_from_qindex_energy` (`resize.c:1232`) — static.
///
/// Walks the cumulative energy tail down from `2 * SCALE_NUMERATOR` and stops
/// at the first band above the threshold; the denominator is
/// `3 * SCALE_NUMERATOR - k`, so a break at the top gives 8 (unscaled) and
/// running to the bottom gives 16 (half width).
///
/// The threshold is the SMALLER of a quantizer-squared term and a fraction of
/// the total AC energy, so a high quantizer and a flat picture both push
/// toward scaling.
#[must_use]
pub fn denom_from_qindex_energy(qindex: i32, energy: &[f64; 16], threshq: f64, threshp: f64) -> u8 {
    let q = crate::rate_control::convert_qindex_to_q(qindex, 8);
    let tq = threshq * q * q;
    let tp = threshp * energy[1];
    let thresh = tq.min(tp);
    let mut k = (SCALE_NUMERATOR * 2) as usize;
    while k > SCALE_NUMERATOR as usize {
        if energy[k - 1] > thresh {
            break;
        }
        k -= 1;
    }
    (3 * SCALE_NUMERATOR as usize - k) as u8
}

/// C `get_superres_denom_for_qindex` (`resize.c:1267`) — static.
///
/// Superres is applied to key frames and ARF frames only — every other update
/// type returns the unscaled denominator without touching the picture. The
/// `sr_kf` / `sr_arf` switches then gate each of those independently.
///
/// The recode clamp at the end is the subtle part: under
/// [`superres_in_recode_allowed`] the denominator is raised to at least
/// `SCALE_NUMERATOR + 1`, so superres is *tried* even when the energy analysis
/// said not to — because the recode loop is going to encode full resolution
/// anyway and can compare.
#[must_use]
pub fn denom_for_qindex(
    cfg: &SuperresConfig,
    pic: &SuperresPicInput,
    luma: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    qindex: i32,
    sr_kf: bool,
    sr_arf: bool,
) -> u8 {
    let unscaled = SCALE_NUMERATOR as u8;
    match pic.update_type {
        FrameUpdateType::Kf if !sr_kf => return unscaled,
        FrameUpdateType::Arf if !sr_arf => return unscaled,
        FrameUpdateType::Kf | FrameUpdateType::Arf => {}
        _ => return unscaled,
    }

    let energy = analyze_hor_freq(luma, stride, width, height);
    let Some(threshq) = energy_by_q2_thresh(pic.frames_to_key, pic.update_type) else {
        return unscaled;
    };
    let mut denom = denom_from_qindex_energy(qindex, &energy, threshq, ENERGY_BY_AC_THRESH);

    if superres_in_recode_allowed(cfg) {
        denom = denom.max(unscaled + 1);
    }
    denom
}

/// C `lcg_rand16` (`utility.h`) — the 16-bit linear congruential generator
/// `SUPERRES_RANDOM` draws from.
///
/// Kept here rather than shared with `palette`'s copy because the two carry
/// independent seeds and C keeps the `SUPERRES_RANDOM` seed in a
/// function-level `static` (see [`calc_superres_params`]).
fn lcg_rand16(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
    (*state / 65536) % 2048
}

/// C `calc_superres_params` (`resize.c:1311`) — static.
///
/// Picks the denominator for one picture and, in the AUTO modes, the whole
/// recode schedule.
///
/// `seed` is C's function-level `static unsigned int seed = 34567`, hoisted to
/// a caller-owned value: a hidden mutable static is not expressible in safe
/// Rust, and making it explicit also makes the `SUPERRES_RANDOM` sequence
/// reproducible in a test. Threading it is not a behaviour change — C's static
/// is process-global and advanced once per RANDOM-mode picture, which is
/// exactly what a caller passing one `&mut u32` through the sequence gets.
///
/// Two early exits worth naming: `allow_intrabc` disables superres outright
/// (the two tools cannot coexist), and in `SUPERRES_QTHRESH` screen-content
/// tools do the same.
#[must_use]
pub fn calc_superres_params(
    cfg: &SuperresConfig,
    pic: &SuperresPicInput,
    luma: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    seed: &mut u32,
) -> SuperresDecision {
    let unscaled = SCALE_NUMERATOR as u8;
    let mut out = SuperresDecision::default();

    // Superres can only be enabled while intra block copy is off.
    if pic.allow_intrabc {
        return out;
    }

    let qindex =
        i32::from(crate::rate_control::QUANTIZER_TO_QINDEX[usize::from(pic.picture_qp.min(63))]);

    match cfg.mode {
        SuperresMode::None => out.denom = unscaled,
        SuperresMode::Fixed => {
            out.denom = if pic.update_type == FrameUpdateType::Kf {
                cfg.kf_denom
            } else {
                cfg.denom
            };
        }
        SuperresMode::Random => {
            out.denom = (lcg_rand16(seed) % 9 + 8) as u8;
        }
        SuperresMode::Qthresh => {
            if pic.allow_screen_content_tools {
                return out;
            }
            let qthresh = i32::from(
                crate::rate_control::QUANTIZER_TO_QINDEX[usize::from(
                    if pic.is_intra_only {
                        cfg.kf_qthres
                    } else {
                        cfg.qthres
                    }
                    .min(63),
                )],
            );
            out.denom = if qindex <= qthresh {
                unscaled
            } else {
                denom_for_qindex(cfg, pic, luma, stride, width, height, qindex, true, true)
            };
        }
        SuperresMode::Auto => {
            let qthresh = if cfg.auto_search_type == SuperresAutoSearch::Solo {
                128
            } else {
                0
            };
            if qindex <= qthresh {
                out.denom = unscaled;
                return out;
            }
            match cfg.auto_search_type {
                SuperresAutoSearch::Solo => {
                    out.denom =
                        denom_for_qindex(cfg, pic, luma, stride, width, height, qindex, true, true);
                }
                SuperresAutoSearch::Dual => {
                    out.denom_array[0] =
                        denom_for_qindex(cfg, pic, luma, stride, width, height, qindex, true, true);
                    if out.denom_array[0] != unscaled {
                        out.denom_array[1] = unscaled;
                        out.denom = out.denom_array[0];
                        out.total_recode_loop = 2;
                    }
                }
                SuperresAutoSearch::All => {
                    if matches!(pic.update_type, FrameUpdateType::Kf | FrameUpdateType::Arf) {
                        for (i, slot) in out.denom_array.iter_mut().enumerate() {
                            *slot = if i < SCALE_NUMERATOR as usize {
                                unscaled + 1 + i as u8
                            } else {
                                unscaled
                            };
                        }
                        out.denom = out.denom_array[0];
                        out.total_recode_loop = unscaled + 1;
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Frame resize (distinct from superres) and the conformance reconciliation
// ---------------------------------------------------------------------------

/// C `RESIZE_MODE` (`API/EbSvtAv1Enc.h`).
///
/// Frame resize scales BOTH dimensions and is not undone by the decoder, in
/// contrast to superres, which scales width only and is normatively upscaled.
/// The two can be configured at once, which is what
/// [`validate_size_scales`] exists to reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    /// C `RESIZE_NONE = 0`.
    None = 0,
    /// C `RESIZE_FIXED = 1`.
    Fixed = 1,
    /// C `RESIZE_RANDOM = 2`.
    Random = 2,
    /// C `RESIZE_DYNAMIC = 3` — the denominator comes from the rate control's
    /// pending-params block rather than from the configuration.
    Dynamic = 3,
    /// C `RESIZE_RANDOM_ACCESS = 4` — an application event carries its own
    /// nested mode.
    RandomAccess = 4,
}

/// C `superres_params_type` (`resize.h`) — the coded size and the superres
/// denominator that produced it, mutated in place by
/// [`validate_size_scales`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperresParams {
    /// C `encoding_width` — the width the frame is actually coded at.
    pub encoding_width: u16,
    /// C `encoding_height`.
    pub encoding_height: u16,
    /// C `superres_denom`.
    pub superres_denom: u8,
}

/// The `pcs->resize_evt` nested event of `RESIZE_RANDOM_ACCESS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeEvent {
    /// C `resize_evt.scale_mode` — only NONE, FIXED and RANDOM are legal here.
    pub scale_mode: ResizeMode,
    /// C `resize_evt.scale_denom`.
    pub scale_denom: u8,
    /// C `resize_evt.scale_kf_denom`.
    pub scale_kf_denom: u8,
}

/// The configuration [`calculate_next_resize_scale`] reads.
#[derive(Debug, Clone, Copy)]
pub struct ResizeConfig {
    /// C `static_config.resize_mode`.
    pub mode: ResizeMode,
    /// C `static_config.resize_denom`.
    pub denom: u8,
    /// C `static_config.resize_kf_denom`.
    pub kf_denom: u8,
    /// C `scs->resize_pending_params.resize_denom` — the DYNAMIC input.
    pub pending_denom: u8,
    /// C `pcs->resize_evt` — the RANDOM_ACCESS input.
    pub event: ResizeEvent,
}

/// C `calculate_next_resize_scale` (`resize.c:1855`) — static.
///
/// `None` where C calls `svt_aom_assert_err(0, ...)`: an unknown mode, or a
/// `RESIZE_RANDOM_ACCESS` event carrying a nested mode other than NONE, FIXED
/// or RANDOM. In a Release build that assert does not abort and C falls
/// through returning `SCALE_NUMERATOR`, which silently disables resize; the
/// port refuses rather than doing that quietly
/// (`docs/WORKING-ON-THIS.md` §6).
///
/// As with `SUPERRES_RANDOM`, C's `static unsigned int seed = 56789` is
/// hoisted to a caller-owned value. It is a SEPARATE seed from the superres
/// one, which matters: the two generators do not interleave in C, and sharing
/// one here would change both sequences.
#[must_use]
pub fn calculate_next_resize_scale(
    cfg: &ResizeConfig,
    is_key_frame: bool,
    seed: &mut u32,
) -> Option<u8> {
    let unscaled = SCALE_NUMERATOR as u8;
    Some(match cfg.mode {
        ResizeMode::None => unscaled,
        ResizeMode::Fixed => {
            if is_key_frame {
                cfg.kf_denom
            } else {
                cfg.denom
            }
        }
        ResizeMode::Random => (lcg_rand16(seed) % 9 + 8) as u8,
        ResizeMode::Dynamic => cfg.pending_denom,
        ResizeMode::RandomAccess => match cfg.event.scale_mode {
            ResizeMode::None => unscaled,
            ResizeMode::Fixed => {
                if is_key_frame {
                    cfg.event.scale_kf_denom
                } else {
                    cfg.event.scale_denom
                }
            }
            ResizeMode::Random => (lcg_rand16(seed) % 9 + 8) as u8,
            _ => return None,
        },
    })
}

/// C `dimension_is_ok` (`resize.c:1906`) — static.
///
/// The AV1 conformance rule that a coded dimension may not be less than half
/// the original after BOTH scalings: `resized * 8 >= orig * denom / 2`.
///
/// Integer note: C evaluates this in `int` with a truncating `/ 2` on the
/// right-hand side, so the comparison is against `floor(orig * denom / 2)`,
/// not against a rational half. Reproduced with `i32`.
#[must_use]
pub fn dimension_is_ok(orig_dim: i32, resized_dim: i32, denom: i32) -> bool {
    resized_dim * SCALE_NUMERATOR as i32 >= orig_dim * denom / 2
}

/// C `dimensions_are_ok` (`resize.c:1910`) — static.
///
/// Only the WIDTH is checked, because superres scales horizontally only; C
/// `(void)`-casts the height away and that is reproduced by not taking it.
#[must_use]
pub fn dimensions_are_ok(owidth: i32, rsz: &SuperresParams) -> bool {
    dimension_is_ok(
        owidth,
        i32::from(rsz.encoding_width),
        i32::from(rsz.superres_denom),
    )
}

/// C `validate_size_scales` (`resize.c:1916`) — static.
///
/// When resize and superres are both on, their denominators multiply and can
/// take the coded width below the conformance floor. This walks one or both
/// back until the result is legal, and which one it is allowed to touch
/// depends on which of the two was configured RANDOM — a fixed denominator the
/// application asked for is never silently changed.
///
/// Returns `(ok, resize_denom)`, mutating `rsz` in place, exactly as C does
/// through its `superres_params_type*` and `uint8_t*` out-parameters. The
/// fourth arm — neither mode RANDOM — cannot alter anything and returns
/// `false`, which is C's `return 0` and the caller's signal to give up on the
/// combination.
///
/// The `RESIZE_RANDOM && SUPERRES_RANDOM` arm is a `do`/`while`, so it always
/// decrements at least once even when the dimensions were already fine — but
/// it is only entered after the early `dimensions_are_ok` return, so "already
/// fine" cannot reach it.
pub fn validate_size_scales(
    resize_mode: ResizeMode,
    superres_mode: SuperresMode,
    owidth: i32,
    oheight: i32,
    rsz: &mut SuperresParams,
    resize_denom: &mut u8,
) -> bool {
    let unscaled = SCALE_NUMERATOR as u8;
    if dimensions_are_ok(owidth, rsz) {
        return true; // Nothing to do.
    }

    // C `DIVIDE_AND_ROUND(x, y)` = `(x + y / 2) / y`.
    let round_div = |x: i32, y: i32| (x + y / 2) / y;
    *resize_denom = round_div(
        owidth * SCALE_NUMERATOR as i32,
        i32::from(rsz.encoding_width),
    )
    .max(round_div(
        oheight * SCALE_NUMERATOR as i32,
        i32::from(rsz.encoding_height),
    ))
    .clamp(0, 255) as u8;

    let scale = |dim: &mut u16, denom: u8| *dim = svtav1_dsp::superres::scaled_size(*dim, denom);
    let resize_random = resize_mode == ResizeMode::Random;
    let superres_random = superres_mode == SuperresMode::Random;

    if !resize_random && superres_random {
        // Alter the superres scale to enforce conformity.
        rsz.superres_denom =
            ((2 * SCALE_NUMERATOR * SCALE_NUMERATOR) / u32::from(*resize_denom).max(1)) as u8;
        if !dimensions_are_ok(owidth, rsz) && rsz.superres_denom > unscaled {
            rsz.superres_denom -= 1;
        }
    } else if resize_random && !superres_random {
        // Alter the resize scale instead.
        *resize_denom =
            ((2 * SCALE_NUMERATOR * SCALE_NUMERATOR) / u32::from(rsz.superres_denom).max(1)) as u8;
        rsz.encoding_width = owidth as u16;
        rsz.encoding_height = oheight as u16;
        scale(&mut rsz.encoding_width, *resize_denom);
        scale(&mut rsz.encoding_height, *resize_denom);
        if !dimensions_are_ok(owidth, rsz) && *resize_denom > unscaled {
            *resize_denom -= 1;
            rsz.encoding_width = owidth as u16;
            rsz.encoding_height = oheight as u16;
            scale(&mut rsz.encoding_width, *resize_denom);
            scale(&mut rsz.encoding_height, *resize_denom);
        }
    } else if resize_random && superres_random {
        // Walk whichever is currently larger back, one step at a time.
        loop {
            if *resize_denom > rsz.superres_denom {
                *resize_denom -= 1;
            } else {
                rsz.superres_denom -= 1;
            }
            rsz.encoding_width = owidth as u16;
            rsz.encoding_height = oheight as u16;
            scale(&mut rsz.encoding_width, *resize_denom);
            scale(&mut rsz.encoding_height, *resize_denom);
            if dimensions_are_ok(owidth, rsz)
                || !(*resize_denom > unscaled || rsz.superres_denom > unscaled)
            {
                break;
            }
        }
    } else {
        // Neither may be altered.
        return false;
    }
    dimensions_are_ok(owidth, rsz)
}

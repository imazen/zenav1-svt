//! Picture-decision reference-structure derivation — a port of the GOP /
//! DPB / reference-list logic in `Codec/pd_process.c`.
//!
//! This is the machinery that decides, per picture, **which DPB slot each of
//! the seven AV1 references points at**, **which slots the frame refreshes**,
//! and **how many references each list actually signals**. Every one of those
//! is either a written frame-header field (`ref_frame_idx[]`,
//! `refresh_frame_flags`, `ref_order_hint[]`, `skip_mode_present`) or a gate
//! on the mode-decision candidate set, so an invented value here is a wrong
//! bitstream on every inter frame — not a quality regression.
//!
//! | Rust | C (`Codec/pd_process.c` unless noted) |
//! |---|---|
//! | [`is_pic_used_as_ref`] | `svt_aom_is_pic_used_as_ref` (1770-1803) — EXPORTED |
//! | [`is_highest_layer`] | `picture_decision_kernel`'s assignment (5559-5561) — inline, static |
//! | [`is_incomp_mg_frame`] | `svt_aom_is_incomp_mg_frame` (4986-4989) — EXPORTED |
//! | [`update_count_try`] | `update_count_try` (4507-4517) — EXPORTED |
//! | [`setup_skip_mode_allowed`] | `svt_av1_setup_skip_mode_allowed` (102-166) — EXPORTED |
//! | [`get_gm_needed_resolutions`] | `svt_aom_get_gm_needed_resolutions` (990-994) — EXPORTED |
//! | [`prune_refs`] | `prune_refs` (1100-1131) — static |
//! | [`update_ref_poc_array`] | `update_ref_poc_array` (1901-1910) — static |
//! | [`update_dpb`] | `update_dpb` (5179-5191) — static |
//! | [`set_key_frame_rps`] | `set_key_frame_rps` (1480-1490) — static |
//! | [`set_ref_list_counts`] | `set_ref_list_counts` (1804-1900) — static |
//! | [`set_all_ref_frame_type`] | `set_all_ref_frame_type` (1044-1099) — static |
//! | [`set_frame_display_params`] | `set_frame_display_params` (1132-1161) — static |
//! | [`set_ref_frame_sign_bias`] | `set_ref_frame_sign_bias` (4894-4909) — static |
//! | [`set_frame_update_type`] / [`set_layer_depth`] / [`set_gf_group_param`] | 4576-4615 — static |
//! | [`generate_rps_info`] | `av1_generate_rps_info` (1911-3506) — static |
//!
//! **Configuration facts measured in the C tree, not inferred** (they decide
//! which arms are live and are recorded here so nobody re-derives them):
//!
//! * `svt_aom_is_incomp_mg_frame` is true only when the *sequence* is
//!   `RANDOM_ACCESS` while *this picture's* pred struct is `LOW_DELAY` — the
//!   incomplete mini-GOP at a GOP boundary. In a pure low-delay sequence it is
//!   always false.
//! * `frame_is_boosted` is `frame_is_kf_gf_arf` (`enc_mode_config.h:100-110`):
//!   intra-only, or `update_type` in {`ARF_UPDATE`, `GF_UPDATE`}. It is NOT
//!   "temporal_layer == 0"; in flat low delay the base-layer P frames are
//!   `LF_UPDATE`/`GF_UPDATE` depending on `frame_offset`, so the
//!   base-vs-non-base MRP caps in [`set_ref_list_counts`] key off the update
//!   type, not the layer index.
//! * Temporal filtering, TPL and dynamic-GOP are all OFF in low delay
//!   (`enc_handle.c:3339-3343`, `3657-3668`, `4294-4300`), so the low-delay
//!   arms below are the whole story for the campaign's first cell.

use crate::inter_mvp::{MODE_CTX_REF_FRAMES, OrderHintInfo, av1_ref_frame_type, get_relative_dist};

/// `REF_FRAME_MINUS1` (`Codec/pred_structure.h:63`) — index into
/// [`Av1RpsNode::ref_dpb_index`] / [`Av1RpsNode::ref_poc_array`].
pub const LAST: usize = 0;
/// See [`LAST`].
pub const LAST2: usize = 1;
/// See [`LAST`].
pub const LAST3: usize = 2;
/// See [`LAST`].
pub const GOLD: usize = 3;
/// See [`LAST`].
pub const BWD: usize = 4;
/// See [`LAST`].
pub const ALT2: usize = 5;
/// See [`LAST`].
pub const ALT: usize = 6;

pub use svtav1_types::reference::INTER_REFS_PER_FRAME;
pub use svtav1_types::reference::REF_FRAMES;
pub use svtav1_types::reference::{
    ALTREF_FRAME, ALTREF2_FRAME, BWDREF_FRAME, GOLDEN_FRAME, LAST_FRAME, LAST2_FRAME, LAST3_FRAME,
};
/// C `INVALID_IDX` used by the skip-mode params.
pub const INVALID_IDX: i32 = -1;

/// C `LAY1_OFF` (`pd_process.c:45`).
pub const LAY1_OFF: u8 = 3;
/// C `LAY2_OFF` (`pd_process.c:46`).
pub const LAY2_OFF: u8 = 5;
/// C `LAY3_OFF` (`pd_process.c:47`).
pub const LAY3_OFF: u8 = 6;

/// C `LAY4_OFF` (`pd_process.c:48`) — the single layer-4 DPB slot.
pub const LAY4_OFF: u8 = 7;

/// C `CIRC_INC(val, start, end)` (`pd_process.c:167`).
///
/// Note the C macro's `(int)(val + 1)` — the increment happens in the
/// argument's own type before the widening cast, which for the `uint8_t`
/// toggles used here can never overflow (`end <= 7`).
#[inline]
#[must_use]
pub fn circ_inc(val: u8, start: u8, end: u8) -> u8 {
    if i32::from(val) + 1 > i32::from(end) {
        start
    } else {
        val + 1
    }
}

/// C `CIRC_DEC(val, start, end)` (`pd_process.c:168`).
#[inline]
#[must_use]
pub fn circ_dec(val: u8, start: u8, end: u8) -> u8 {
    if i32::from(val) - 1 < i32::from(start) {
        end
    } else {
        val - 1
    }
}

/// C `SliceType` — unified: the single definition lives in `svtav1_types::frame`.
pub use svtav1_types::frame::SliceType;

/// C `PredStructure` — unified: the single definition lives in `svtav1_types::frame`.
pub use svtav1_types::frame::PredStructure;

/// C `SVT_AV1_RC_MODE_*` (`API/EbSvtAv1Enc.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcMode {
    /// C `SVT_AV1_RC_MODE_CQP_OR_CRF = 0`.
    CqpOrCrf = 0,
    /// C `SVT_AV1_RC_MODE_VBR = 1`.
    Vbr = 1,
    /// C `SVT_AV1_RC_MODE_CBR = 2`.
    Cbr = 2,
}

/// The API-surface [`crate::rate_control::RcMode`] splits C's single
/// `CQP_OR_CRF` bucket into its two knobs; the picstruct arms only ever
/// test `== Cbr`/`!= Cbr`, so both fold back to `CqpOrCrf`.
impl From<crate::rate_control::RcMode> for RcMode {
    fn from(m: crate::rate_control::RcMode) -> Self {
        match m {
            crate::rate_control::RcMode::Cqp | crate::rate_control::RcMode::Crf => Self::CqpOrCrf,
            crate::rate_control::RcMode::Vbr => Self::Vbr,
            crate::rate_control::RcMode::Cbr => Self::Cbr,
        }
    }
}

/// C `FrameUpdateType` — unified: the single definition lives in `crate::port_frame_update`.
pub use crate::port_frame_update::FrameUpdateType;

/// C `ReferenceMode` — unified: the single definition lives in `svtav1_types::reference`.
pub use svtav1_types::reference::ReferenceMode;

/// C `Av1RpsNode` (`Codec/pred_structure.h:65-69`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Av1RpsNode {
    /// Bitmask of DPB slots this frame writes into (`refresh_frame_flags`).
    pub refresh_frame_mask: u8,
    /// DPB slot per reference, indexed by [`LAST`]..=[`ALT`].
    pub ref_dpb_index: [u8; INTER_REFS_PER_FRAME],
    /// Full (un-truncated) POC per reference, indexed by [`LAST`]..=[`ALT`].
    pub ref_poc_array: [u64; INTER_REFS_PER_FRAME],
}

/// C `DpbEntry` (`Codec/pd_process.h:52-56`) — one shadow-DPB slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DpbEntry {
    /// Display-order POC of the picture in the slot.
    pub picture_number: u64,
    /// Decode-order index of the picture in the slot.
    pub decode_order: u64,
    /// Temporal layer of the picture in the slot.
    pub temporal_layer_index: u8,
}

/// C `MrpCtrls` (`Codec/definitions.h:108-153`) — the multi-reference caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MrpCtrls {
    /// 0: no top-layer refs; 1: all; 2: some (position-dependent).
    pub referencing_scheme: u8,
    /// List-0 cap for boosted (KF/GF/ARF) frames.
    pub base_ref_list0_count: u8,
    /// List-1 cap for boosted frames.
    pub base_ref_list1_count: u8,
    /// List-0 cap for non-boosted frames.
    pub non_base_ref_list0_count: u8,
    /// List-1 cap for non-boosted frames.
    pub non_base_ref_list1_count: u8,
    /// Extra 5L references.
    pub more_5l_refs: u8,
    /// Brightness/ZZ-SAD reference limiter (0 off).
    pub safe_limit_nref: u8,
    /// Threshold for `safe_limit_nref == 1`.
    pub safe_limit_zz_th: u32,
    /// Low-delay DPB-buffer reduction level (0, 1 or 2).
    pub ld_reduce_ref_buffs: u8,
    /// Reference count for the flat RTC structure.
    pub flat_max_refs: u8,
    /// HME L0 MRP detector threshold (percent). 0 disables the prune.
    pub early_hme_l0_prune_th: u16,
    /// C `mrp_ctrls.only_l_bwd` — restrict bipred pairs to (L0,BWD).
    pub only_l_bwd: u8,
    /// C `mrp_ctrls.pme_ref0_only` — PME searches ref 0 only.
    pub pme_ref0_only: u8,
    /// C `mrp_ctrls.use_best_references` — the best-reference selection level
    /// `get_enable_use_best_me` reads.
    pub use_best_references: u8,
}

impl Default for MrpCtrls {
    /// Not a C default — C fills this per preset in `set_mrp_ctrl`
    /// (`enc_handle.c:3574`). This is the neutral "no caps" shape the unit
    /// tests start from.
    fn default() -> Self {
        Self {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 0,
            safe_limit_nref: 0,
            safe_limit_zz_th: 0,
            ld_reduce_ref_buffs: 0,
            flat_max_refs: 4,
            early_hme_l0_prune_th: 0,
            only_l_bwd: 0,
            pme_ref0_only: 0,
            use_best_references: 0,
        }
    }
}

/// C `set_mrp_ctrl_with_level` (`enc_handle.c:3362-3569`) — the twelve-row MRP
/// table, the LOW_DELAY+CBR list-1 disable, and the LOW_DELAY
/// `flat_max_refs`/`ld_reduce_ref_buffs` derivations.
///
/// The row contents are transcribed literally. The counts are what
/// [`set_ref_list_counts`]/`update_count_try` cap against: at level 0 both
/// list-1 caps are **0**, which is what makes an M10 low-delay frame search
/// list 0 only in `me_process` — not a dedup outcome.
fn mrp_ctrls_with_level(level: u8) -> MrpCtrls {
    let m = match level {
        0 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 1,
            base_ref_list1_count: 0,
            non_base_ref_list0_count: 1,
            non_base_ref_list1_count: 0,
            more_5l_refs: 0,
            safe_limit_nref: 0,
            safe_limit_zz_th: 0,
            only_l_bwd: 0,
            pme_ref0_only: 0,
            use_best_references: 0,
            early_hme_l0_prune_th: 0,
            ..Default::default()
        },
        1 => MrpCtrls {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 1,
            ..Default::default()
        },
        2 => MrpCtrls {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 1,
            only_l_bwd: 1,
            ..Default::default()
        },
        3 => MrpCtrls {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 1,
            only_l_bwd: 1,
            use_best_references: 2,
            ..Default::default()
        },
        4 => MrpCtrls {
            referencing_scheme: 1,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            more_5l_refs: 1,
            safe_limit_nref: 1,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            ..Default::default()
        },
        5 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 4,
            base_ref_list1_count: 3,
            non_base_ref_list0_count: 4,
            non_base_ref_list1_count: 3,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            ..Default::default()
        },
        6 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 3,
            base_ref_list1_count: 2,
            non_base_ref_list0_count: 3,
            non_base_ref_list1_count: 2,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            early_hme_l0_prune_th: 170,
            ..Default::default()
        },
        7 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 3,
            base_ref_list1_count: 2,
            non_base_ref_list0_count: 3,
            non_base_ref_list1_count: 2,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            early_hme_l0_prune_th: 150,
            ..Default::default()
        },
        8 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 3,
            base_ref_list1_count: 2,
            non_base_ref_list0_count: 2,
            non_base_ref_list1_count: 2,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            ..Default::default()
        },
        9 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 3,
            base_ref_list1_count: 2,
            non_base_ref_list0_count: 1,
            non_base_ref_list1_count: 1,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            early_hme_l0_prune_th: 150,
            ..Default::default()
        },
        10 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 2,
            base_ref_list1_count: 2,
            non_base_ref_list0_count: 1,
            non_base_ref_list1_count: 1,
            safe_limit_nref: 2,
            safe_limit_zz_th: 60000,
            only_l_bwd: 1,
            pme_ref0_only: 1,
            use_best_references: 3,
            ..Default::default()
        },
        11 => MrpCtrls {
            referencing_scheme: 0,
            base_ref_list0_count: 1,
            base_ref_list1_count: 1,
            non_base_ref_list0_count: 1,
            non_base_ref_list1_count: 1,
            ..Default::default()
        },
        other => panic!("set_mrp_ctrl_with_level: level {other} outside 0..=11 (C asserts)"),
    };
    m
}

/// C `set_mrp_ctrl` (`enc_handle.c:3574-3613`) — the `enc_mode`/`rtc`/
/// `hierarchical_levels`/`pred_structure`/`encoder_bit_depth` cascade to an
/// `mrp_level`, then [`mrp_ctrls_with_level`], then the two LOW_DELAY
/// post-passes (`enc_handle.c:3538-3568` — hoisted into this function in C).
///
/// `enc_mode` is C's `scs->static_config.enc_mode` — the CONFIGURED preset,
/// not the per-frame arm clamp. The caps matter even when the DPB holds a
/// single distinct POC: `set_ref_list_counts` counts list 1 as 1 whenever the
/// `j + 1 > ref_list0_count` guard skips the BWD duplicate check, and the
/// level-0 row's zero list-1 cap is the only thing that then removes it.
#[must_use]
pub fn set_mrp_ctrl(
    enc_mode: i8,
    rtc: bool,
    hierarchical_levels: u8,
    pred_structure: PredStructure,
    eight_bit: bool,
    rate_control_mode: RcMode,
) -> MrpCtrls {
    use crate::port_enc_mode_config::enc_mode::{M8, M9, M10, MR};
    let level = if rtc {
        if hierarchical_levels == 0 {
            if enc_mode <= M8 { 6 } else { 0 }
        } else if enc_mode <= M9 {
            6
        } else if enc_mode <= M10 {
            9
        } else {
            0
        }
    } else if enc_mode <= MR {
        1
    } else if enc_mode <= 2 {
        2
    } else if enc_mode <= 4 {
        4
    } else if enc_mode <= M8 {
        6
    } else if enc_mode <= M9 {
        if pred_structure == PredStructure::RandomAccess {
            7
        } else {
            9
        }
    } else if eight_bit {
        if pred_structure == PredStructure::RandomAccess {
            11
        } else {
            0
        }
    } else {
        if pred_structure == PredStructure::RandomAccess {
            7
        } else {
            0
        }
    };
    let mut m = mrp_ctrls_with_level(level);
    // For low delay CBR mode, list1 references are not used
    // (enc_handle.c:3538-3550).
    if pred_structure == PredStructure::LowDelay && rate_control_mode == RcMode::Cbr {
        m.base_ref_list1_count = 0;
        m.non_base_ref_list1_count = 0;
        if rtc && hierarchical_levels == 0 {
            m.referencing_scheme = 0;
            m.more_5l_refs = 0;
            m.safe_limit_nref = 0;
            m.only_l_bwd = 0;
            m.pme_ref0_only = 0;
            m.use_best_references = 0;
        }
    }
    if pred_structure == PredStructure::LowDelay {
        if rtc && hierarchical_levels == 0 {
            m.flat_max_refs = m
                .base_ref_list0_count
                .max(m.base_ref_list1_count)
                .max(m.non_base_ref_list0_count)
                .max(m.non_base_ref_list1_count);
        }
        m.ld_reduce_ref_buffs = if m.base_ref_list0_count <= 1
            && m.base_ref_list1_count <= 1
            && m.non_base_ref_list0_count <= 1
            && m.non_base_ref_list1_count <= 1
        {
            2
        } else if m.base_ref_list0_count <= 2
            && m.base_ref_list1_count <= 2
            && m.non_base_ref_list0_count <= 2
            && m.non_base_ref_list1_count <= 2
        {
            1
        } else {
            0
        };
    } else {
        m.ld_reduce_ref_buffs = 0;
    }
    m
}

/// The sequence-level inputs the picture-decision arms read.
///
/// Mirrors the `SequenceControlSet` fields `av1_generate_rps_info` and its
/// callees touch; a struct rather than a god-object so a unit test can state
/// the whole configuration inline.
#[derive(Debug, Clone, Copy)]
pub struct SeqPicParams {
    /// C `scs->static_config.pred_structure`.
    pub pred_structure: PredStructure,
    /// C `scs->static_config.rate_control_mode`.
    pub rate_control_mode: RcMode,
    /// C `scs->static_config.rtc`.
    pub rtc: bool,
    /// C `scs->allintra`.
    pub allintra: bool,
    /// C `scs->mrp_ctrls`.
    pub mrp_ctrls: MrpCtrls,
    /// C `scs->seq_header.order_hint_info`.
    pub order_hint_info: OrderHintInfo,
    /// C `scs->static_config.hierarchical_levels` — the SEQUENCE's pyramid
    /// depth, which an incomplete mini-GOP's own
    /// [`PicParams::hierarchical_levels`] may be lower than.
    pub hierarchical_levels: u8,
    /// C `scs->static_config.max_managed_refs` — how many long-term anchors
    /// the application may hold at once (see [`crate::port_ref_mgmt`]).
    pub max_managed_refs: u8,
    /// C `scs->static_config.enable_tf_key` — whether a key frame may be
    /// temporally filtered. C's config default is 1 (`enc_settings.c`).
    pub enable_tf_key: bool,
    /// C `tf_level` — the preset-selected TF level `derive_tf_params`
    /// (`enc_handle.c:3333`) produces; 0 disables the whole table.
    pub tf_level: u8,
    /// C `scs->tf_params_per_type[0..3]` — `[I_SLICE, BASE, L1]`, filled by
    /// [`derive_tf_params`].
    pub tf_params_per_type: [TfCtrls; 3],
}

impl Default for SeqPicParams {
    fn default() -> Self {
        Self {
            pred_structure: PredStructure::LowDelay,
            rate_control_mode: RcMode::CqpOrCrf,
            rtc: false,
            allintra: false,
            mrp_ctrls: MrpCtrls::default(),
            order_hint_info: OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: 7,
            },
            hierarchical_levels: 0,
            max_managed_refs: 0,
            enable_tf_key: true,
            tf_level: 0,
            tf_params_per_type: [TfCtrls::default(); 3],
        }
    }
}

pub use svtav1_types::math::shift_u32::divide_and_round_i32 as divide_and_round;
mod tf;
pub use tf::*;

mod scene;
pub use scene::*;

mod params;
pub use params::*;

mod refs;
pub use refs::*;

mod rps;
pub use rps::*;

mod mini_gop;
pub use mini_gop::*;

mod tpl_group;
pub use tpl_group::*;

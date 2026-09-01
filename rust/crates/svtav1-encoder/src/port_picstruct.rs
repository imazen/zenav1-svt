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

/// C `REF_FRAMES` — DPB slot count.
pub const REF_FRAMES: usize = 8;
/// C `INTER_REFS_PER_FRAME`.
pub const INTER_REFS_PER_FRAME: usize = 7;
/// C `LAST_FRAME` (`MvReferenceFrame` numbering: `INTRA_FRAME` is 0).
pub const LAST_FRAME: i8 = 1;
/// C `LAST2_FRAME`.
pub const LAST2_FRAME: i8 = 2;
/// C `LAST3_FRAME`.
pub const LAST3_FRAME: i8 = 3;
/// C `GOLDEN_FRAME`.
pub const GOLDEN_FRAME: i8 = 4;
/// C `BWDREF_FRAME`.
pub const BWDREF_FRAME: i8 = 5;
/// C `ALTREF2_FRAME`.
pub const ALTREF2_FRAME: i8 = 6;
/// C `ALTREF_FRAME`.
pub const ALTREF_FRAME: i8 = 7;
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

/// C `SliceType` (`definitions.h:1890-1894`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceType {
    /// C `B_SLICE = 0` — any inter frame (P frames are B frames with an empty list 1).
    B = 0,
    /// C `I_SLICE = 1`.
    I = 1,
}

/// C `PredStructure` (`API/EbSvtAv1Enc.h:136`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredStructure {
    /// C `ALL_INTRA = 0`.
    AllIntra = 0,
    /// C `LOW_DELAY = 1`.
    LowDelay = 1,
    /// C `RANDOM_ACCESS = 2`.
    RandomAccess = 2,
}

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

/// C `SvtAv1FrameUpdateType` (`API/EbSvtAv1Enc.h:183-191`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameUpdateType {
    /// C `SVT_AV1_KF_UPDATE = 0`.
    Kf = 0,
    /// C `SVT_AV1_LF_UPDATE = 1`.
    Lf = 1,
    /// C `SVT_AV1_GF_UPDATE = 2`.
    Gf = 2,
    /// C `SVT_AV1_ARF_UPDATE = 3`.
    Arf = 3,
    /// C `SVT_AV1_OVERLAY_UPDATE = 4`.
    Overlay = 4,
    /// C `SVT_AV1_INTNL_OVERLAY_UPDATE = 5`.
    IntnlOverlay = 5,
    /// C `SVT_AV1_INTNL_ARF_UPDATE = 6`.
    IntnlArf = 6,
}

/// C `ReferenceMode` (`definitions.h:1490-1495`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceMode {
    /// C `SINGLE_REFERENCE = 0`.
    Single = 0,
    /// C `COMPOUND_REFERENCE = 1`.
    Compound = 1,
    /// C `REFERENCE_MODE_SELECT = 2`.
    Select = 2,
    /// C writes `(ReferenceMode)0xFF` on I slices — not a real mode, and the
    /// header writer never emits it.
    IntraSentinel = 0xFF,
}

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
}

impl Default for MrpCtrls {
    /// Not a C default — C fills this per preset in `enc_mode_config.c`. This
    /// is the neutral "no caps" shape the unit tests start from.
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
        }
    }
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
        }
    }
}

/// The per-picture state `av1_generate_rps_info` reads and writes.
///
/// Mirrors the `PictureParentControlSet` fields in scope for this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PicParams {
    /// C `pcs->picture_number` — display-order POC.
    pub picture_number: u64,
    /// C `pcs->decode_order`.
    pub decode_order: u64,
    /// C `pcs->slice_type`.
    pub slice_type: SliceType,
    /// C `pcs->frm_hdr.frame_type == KEY_FRAME`.
    pub is_key_frame: bool,
    /// C `frame_is_intra_only(pcs)`.
    pub is_intra_only: bool,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->is_overlay`.
    pub is_overlay: bool,
    /// C `pcs->pred_struct_ptr->pred_type` — the *picture's* structure, which
    /// can be `LOW_DELAY` inside a `RANDOM_ACCESS` sequence (an incomplete MG).
    pub pred_struct_type: PredStructure,
    /// C `pcs->pred_struct_ptr->pred_struct_entry_count`.
    pub pred_struct_entry_count: u32,
    /// C `pcs->update_type`.
    pub update_type: FrameUpdateType,
    /// C `pcs->layer_depth`.
    pub layer_depth: u8,
    /// C `pcs->frame_offset`.
    pub frame_offset: u64,
    /// C `pcs->is_ref`.
    pub is_ref: bool,
    /// C `pcs->av1_ref_signal`.
    pub rps: Av1RpsNode,
    /// C `pcs->ref_list0_count`.
    pub ref_list0_count: u8,
    /// C `pcs->ref_list1_count`.
    pub ref_list1_count: u8,
    /// C `pcs->ref_list0_count_try`.
    pub ref_list0_count_try: u8,
    /// C `pcs->ref_list1_count_try`.
    pub ref_list1_count_try: u8,
    /// C `pcs->ref_order_hint[7]` — the written frame-header field.
    pub ref_order_hint: [u32; INTER_REFS_PER_FRAME],
    /// C `pcs->cur_order_hint`.
    pub cur_order_hint: u32,
    /// C `pcs->frm_hdr.show_frame`.
    pub show_frame: bool,
    /// C `pcs->has_show_existing`.
    pub has_show_existing: bool,
    /// C `pcs->frm_hdr.show_existing_frame` — the DPB slot a
    /// `show_existing_frame` header re-displays. Only meaningful while
    /// [`PicParams::has_show_existing`] is set.
    pub show_existing_frame: u8,
    /// C `pcs->frm_hdr.reference_mode`.
    pub reference_mode: ReferenceMode,
    /// C `pcs->allow_comp_inter_inter`.
    pub allow_comp_inter_inter: bool,
    /// C `pcs->frm_hdr.skip_mode_params`.
    pub skip_mode: SkipModeInfo,
    /// C `pcs->av1_cm->ref_frame_sign_bias[8]`.
    pub ref_frame_sign_bias: [i32; REF_FRAMES],
    /// C `pcs->ref_frame_type_arr` (`MvReferenceFrame`, `MODE_CTX_REF_FRAMES` long).
    pub ref_frame_type_arr: [i8; MODE_CTX_REF_FRAMES],
    /// C `pcs->tot_ref_frame_types`.
    pub tot_ref_frame_types: u8,
    /// C `pcs->transition_present`.
    pub transition_present: u8,
    /// C `pcs->av1_cm->mi_cols`.
    pub mi_cols: u32,
    /// C `pcs->av1_cm->mi_rows`.
    pub mi_rows: u32,
    /// C `pcs->aligned_width`.
    pub aligned_width: u32,
    /// C `pcs->aligned_height`.
    pub aligned_height: u32,
    /// C `pcs->ref_mgmt` — the long-term-reference events the application
    /// queued on this picture (see [`crate::port_ref_mgmt`]).
    pub ref_mgmt: crate::port_ref_mgmt::RefMgmtEvents,
}

impl Default for PicParams {
    fn default() -> Self {
        Self {
            picture_number: 0,
            decode_order: 0,
            slice_type: SliceType::B,
            is_key_frame: false,
            is_intra_only: false,
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            is_overlay: false,
            pred_struct_type: PredStructure::LowDelay,
            pred_struct_entry_count: 1,
            update_type: FrameUpdateType::Lf,
            layer_depth: 0,
            frame_offset: 0,
            is_ref: true,
            rps: Av1RpsNode::default(),
            ref_list0_count: 0,
            ref_list1_count: 0,
            ref_list0_count_try: 0,
            ref_list1_count_try: 0,
            ref_order_hint: [0; INTER_REFS_PER_FRAME],
            cur_order_hint: 0,
            show_frame: true,
            has_show_existing: false,
            show_existing_frame: 0,
            reference_mode: ReferenceMode::Select,
            allow_comp_inter_inter: false,
            skip_mode: SkipModeInfo::default(),
            ref_frame_sign_bias: [0; REF_FRAMES],
            ref_frame_type_arr: [0; MODE_CTX_REF_FRAMES],
            tot_ref_frame_types: 0,
            transition_present: 0,
            mi_cols: 0,
            mi_rows: 0,
            aligned_width: 0,
            aligned_height: 0,
            ref_mgmt: crate::port_ref_mgmt::RefMgmtEvents::default(),
        }
    }
}

/// C `SkipModeInfo` (`frame_header`): spec 5.9.22 skip-mode params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkipModeInfo {
    /// C `skip_mode_allowed`.
    pub skip_mode_allowed: i32,
    /// C `skip_mode_flag` — the written header bit.
    pub skip_mode_flag: i32,
    /// C `ref_frame_idx_0`.
    pub ref_frame_idx_0: i32,
    /// C `ref_frame_idx_1`.
    pub ref_frame_idx_1: i32,
}

impl Default for SkipModeInfo {
    fn default() -> Self {
        Self {
            skip_mode_allowed: 0,
            skip_mode_flag: 0,
            ref_frame_idx_0: INVALID_IDX,
            ref_frame_idx_1: INVALID_IDX,
        }
    }
}

/// The `PictureDecisionContext` state the RPS branches carry across frames.
///
/// The toggles are the whole reason this is stateful: a slot assignment that
/// is right for frame N is wrong for frame N+1 unless the toggle advanced
/// exactly as C's did, so this struct must be threaded through the whole
/// sequence rather than rebuilt per picture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PicDecisionCtx {
    /// C `ctx->lay0_toggle`.
    pub lay0_toggle: u8,
    /// C `ctx->lay1_toggle`.
    pub lay1_toggle: u8,
    /// C `ctx->lay2_toggle`.
    pub lay2_toggle: u8,
    /// C `ctx->dpb[REF_FRAMES]` — the shadow DPB.
    pub dpb: [DpbEntry; REF_FRAMES],
    /// C `ctx->last_long_base_pic`.
    pub last_long_base_pic: u64,
    /// C `ctx->sframe_poc` (0 when no S-frame is pending).
    pub sframe_poc: u64,
    /// C `ctx->cut_short_ra_mg`.
    pub cut_short_ra_mg: u8,
    /// C `ctx->transition_detected`.
    pub transition_detected: i32,
    /// C `ctx->list0_only`.
    pub list0_only: bool,
    /// C `ctx->mini_gop_length[]`.
    pub mini_gop_length: [u32; 8],
    /// C `ctx->mini_gop_start_index[]`.
    pub mini_gop_start_index: [u32; 8],
    /// C `ctx->mini_gop_end_index[]`.
    pub mini_gop_end_index: [u32; 8],
    /// C `ctx->mg_size`.
    pub mg_size: u32,
    /// C `ctx->pic_id_per_dpb_slot` — the application `pic_id` pinned into
    /// each DPB slot, or `None` for a slot the short-term allocator owns.
    /// C stores `0` for "no id"; [`core::num::NonZeroU32`] makes that
    /// sentinel unrepresentable (see [`crate::port_ref_mgmt`]).
    pub pic_id_per_dpb_slot: [Option<core::num::NonZeroU32>; REF_FRAMES],
}

impl PicDecisionCtx {
    /// The state C's `svt_aom_picture_decision_context_ctor` leaves
    /// (`pd_process.c:236-252`).
    ///
    /// The only field that differs from [`Default`] is `transition_detected`,
    /// which C initialises to **-1**, not 0. Nothing this module does reads it
    /// except `init_pic_settings`' `== 1` test, so the two are behaviourally
    /// identical here — it is reproduced so a later consumer that treats 0 as
    /// "no transition yet" does not inherit a value C never had.
    #[must_use]
    pub fn new() -> Self {
        Self {
            transition_detected: -1,
            ..Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Exported C symbols (tier 1 reachable)
// ---------------------------------------------------------------------------

/// C `svt_aom_is_pic_used_as_ref` (`pd_process.c:1770-1803`) — EXPORTED.
///
/// Whether a picture enters the DPB at all. `referencing_scheme` 0 forbids
/// top-layer refs, 1 allows all, 2 allows some by position. Sub-top-layer
/// pictures are always refs; overlays never are.
///
/// C's `default:` arm asserts and then falls through to `return true`; with
/// `NDEBUG` (the Release build the oracle uses) that is a plain `true`, which
/// is what this reproduces.
#[must_use]
pub fn is_pic_used_as_ref(
    hierarchical_levels: u32,
    temporal_layer: u32,
    picture_index: u32,
    referencing_scheme: u32,
    is_overlay: bool,
) -> bool {
    if is_overlay {
        return false;
    }
    // Frames below top layer are always used as ref
    if temporal_layer < hierarchical_levels {
        return true;
    }
    match hierarchical_levels {
        0 => true,
        1 => referencing_scheme != 0,
        // hierarchical_levels 2 and 3 share an arm in C.
        2 | 3 => {
            if referencing_scheme == 0 {
                false
            } else if referencing_scheme == 1 {
                true
            } else {
                picture_index == 0
            }
        }
        4 => {
            if referencing_scheme == 0 {
                false
            } else if referencing_scheme == 1 {
                true
            } else {
                picture_index == 0 || picture_index == 8
            }
        }
        5 => false,
        _ => true,
    }
}

/// C `svt_aom_is_incomp_mg_frame` (`pd_process.c:4986-4989`) — EXPORTED.
///
/// True for a picture whose own prediction structure is low delay inside a
/// random-access sequence: the incomplete mini-GOP at a GOP boundary. Read by
/// [`set_ref_list_counts`] (forces list 1 empty) and by the `reference_mode`
/// derivation (forces `SINGLE_REFERENCE`).
#[must_use]
pub fn is_incomp_mg_frame(pic: &PicParams, seq: &SeqPicParams) -> bool {
    pic.pred_struct_type == PredStructure::LowDelay
        && seq.pred_structure == PredStructure::RandomAccess
}

/// C `frame_is_kf_gf_arf` / `frame_is_boosted` (`enc_mode_config.h:100-110`).
#[must_use]
pub fn frame_is_boosted(pic: &PicParams) -> bool {
    pic.is_intra_only
        || pic.update_type == FrameUpdateType::Arf
        || pic.update_type == FrameUpdateType::Gf
}

/// C `update_count_try` (`pd_process.c:4507-4517`) — EXPORTED.
///
/// Caps the *try* counts (what MD enumerates) against the MRP per-layer
/// limits. These, not `ref_list{0,1}_count`, decide how many references the
/// candidate loop walks.
pub fn update_count_try(pic: &mut PicParams, seq: &SeqPicParams) {
    let mrp = &seq.mrp_ctrls;
    if frame_is_boosted(pic) {
        pic.ref_list0_count_try = pic.ref_list0_count.min(mrp.base_ref_list0_count);
        pic.ref_list1_count_try = pic.ref_list1_count.min(mrp.base_ref_list1_count);
    } else {
        pic.ref_list0_count_try = pic.ref_list0_count.min(mrp.non_base_ref_list0_count);
        pic.ref_list1_count_try = pic.ref_list1_count.min(mrp.non_base_ref_list1_count);
    }
}

/// C `svt_aom_get_gm_needed_resolutions` (`pd_process.c:990-994`) — EXPORTED.
///
/// Maps the global-motion downsample level to which pyramid levels must exist.
/// Returns `(full, quarter, sixteenth)`. C's `GM_FULL`/`GM_DOWN`/`GM_DOWN16`/
/// `GM_ADAPT_0`/`GM_ADAPT_1` are 0/1/2/3/4.
#[must_use]
pub fn get_gm_needed_resolutions(ds_lvl: u8) -> (bool, bool, bool) {
    const GM_FULL: u8 = 0;
    const GM_DOWN: u8 = 1;
    const GM_DOWN16: u8 = 2;
    const GM_ADAPT_0: u8 = 3;
    const GM_ADAPT_1: u8 = 4;
    let need_full = ds_lvl == GM_FULL || ds_lvl == GM_ADAPT_0;
    let need_quart = ds_lvl == GM_DOWN || ds_lvl == GM_ADAPT_0 || ds_lvl == GM_ADAPT_1;
    let need_sixteen = ds_lvl == GM_DOWN16 || ds_lvl == GM_ADAPT_1;
    (need_full, need_quart, need_sixteen)
}

/// C `svt_av1_setup_skip_mode_allowed` (`pd_process.c:102-166`) — EXPORTED.
///
/// Derives `skip_mode_allowed` and the two skip-mode reference indices from
/// `ref_order_hint[]` against `cur_order_hint`. `skip_mode_present` is a frame
/// header bit, so a wrong answer here flips a bit on every inter frame.
///
/// Index trap: C's `ref_frame_idx_{0,1}` are `LAST_FRAME + min/max(i, j)`
/// where `i`/`j` index `ref_order_hint[0..7]` — i.e. they are
/// `MvReferenceFrame` values, not list positions.
pub fn setup_skip_mode_allowed(pic: &mut PicParams, seq: &SeqPicParams) {
    let ohi = seq.order_hint_info;
    pic.skip_mode.skip_mode_allowed = 0;
    pic.skip_mode.ref_frame_idx_0 = INVALID_IDX;
    pic.skip_mode.ref_frame_idx_1 = INVALID_IDX;

    if !ohi.enable_order_hint
        || pic.slice_type == SliceType::I
        || pic.reference_mode == ReferenceMode::Single
    {
        return;
    }

    let cur_order_hint = pic.cur_order_hint as i32;
    // C seeds the forward slot with -1 and the backward slot with INT_MAX.
    let mut ref_order_hints: [i32; 2] = [-1, i32::MAX];
    let mut ref_idx: [i32; 2] = [INVALID_IDX, INVALID_IDX];

    for i in 0..INTER_REFS_PER_FRAME {
        let ref_hint = pic.ref_order_hint[i] as i32;
        let d = get_relative_dist(ohi, ref_hint, cur_order_hint);
        if d < 0 {
            if ref_order_hints[0] == -1 || get_relative_dist(ohi, ref_hint, ref_order_hints[0]) > 0
            {
                ref_order_hints[0] = ref_hint;
                ref_idx[0] = i as i32;
            }
        } else if d > 0
            && (ref_order_hints[1] == i32::MAX
                || get_relative_dist(ohi, ref_hint, ref_order_hints[1]) < 0)
        {
            ref_order_hints[1] = ref_hint;
            ref_idx[1] = i as i32;
        }
    }

    if ref_idx[0] != INVALID_IDX && ref_idx[1] != INVALID_IDX {
        // Bi-directional prediction.
        pic.skip_mode.skip_mode_allowed = 1;
        pic.skip_mode.ref_frame_idx_0 = i32::from(LAST_FRAME) + ref_idx[0].min(ref_idx[1]);
        pic.skip_mode.ref_frame_idx_1 = i32::from(LAST_FRAME) + ref_idx[0].max(ref_idx[1]);
    } else if ref_idx[0] != INVALID_IDX && ref_idx[1] == INVALID_IDX {
        // Forward prediction only: find the second-nearest forward reference.
        ref_order_hints[1] = -1;
        for i in 0..INTER_REFS_PER_FRAME {
            let ref_hint = pic.ref_order_hint[i] as i32;
            if (ref_order_hints[0] != -1
                && get_relative_dist(ohi, ref_hint, ref_order_hints[0]) < 0)
                && (ref_order_hints[1] == -1
                    || get_relative_dist(ohi, ref_hint, ref_order_hints[1]) > 0)
            {
                ref_order_hints[1] = ref_hint;
                ref_idx[1] = i as i32;
            }
        }
        if ref_order_hints[1] != -1 {
            pic.skip_mode.skip_mode_allowed = 1;
            pic.skip_mode.ref_frame_idx_0 = i32::from(LAST_FRAME) + ref_idx[0].min(ref_idx[1]);
            pic.skip_mode.ref_frame_idx_1 = i32::from(LAST_FRAME) + ref_idx[0].max(ref_idx[1]);
        }
    }
}

// ---------------------------------------------------------------------------
// Static C helpers (tier 4: hand-derived vectors traced against the C source)
// ---------------------------------------------------------------------------

/// C `prune_refs` (`pd_process.c:1100-1131`) — static.
///
/// Collapses unused list slots onto LAST (list 0) and BWD (list 1) in BOTH the
/// slot array and the POC array. Without it the header's unused `ref_frame_idx`
/// entries point at stale DPB slots and the frame header diverges even when the
/// used references are right.
///
/// Order matters: `ALT` is folded onto `BWD` *before* `ALT2` is, and `BWD` may
/// itself already have been folded onto `LAST`, so the C sequence is
/// transcribed literally rather than "simplified".
pub fn prune_refs(rps: &mut Av1RpsNode, ref_list0_count: u32, ref_list1_count: u32) {
    if ref_list0_count < 4 {
        rps.ref_dpb_index[GOLD] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[GOLD] = rps.ref_poc_array[LAST];
    }
    if ref_list0_count < 3 {
        rps.ref_dpb_index[LAST3] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[LAST3] = rps.ref_poc_array[LAST];
    }
    if ref_list0_count < 2 {
        rps.ref_dpb_index[LAST2] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[LAST2] = rps.ref_poc_array[LAST];
    }
    if ref_list1_count < 1 {
        rps.ref_dpb_index[BWD] = rps.ref_dpb_index[LAST];
        rps.ref_poc_array[BWD] = rps.ref_poc_array[LAST];
    }
    if ref_list1_count < 3 {
        rps.ref_dpb_index[ALT] = rps.ref_dpb_index[BWD];
        rps.ref_poc_array[ALT] = rps.ref_poc_array[BWD];
    }
    if ref_list1_count < 2 {
        rps.ref_dpb_index[ALT2] = rps.ref_dpb_index[BWD];
        rps.ref_poc_array[ALT2] = rps.ref_poc_array[BWD];
    }
}

/// C `update_ref_poc_array` (`pd_process.c:1901-1910`) — static.
///
/// Reads the seven reference POCs out of the shadow DPB. Source of
/// `pcs->ref_order_hint[]` (a written frame-header field) and of every MVP
/// temporal-distance scale.
pub fn update_ref_poc_array(rps: &mut Av1RpsNode, dpb: &[DpbEntry; REF_FRAMES]) {
    for i in 0..INTER_REFS_PER_FRAME {
        rps.ref_poc_array[i] = dpb[rps.ref_dpb_index[i] as usize].picture_number;
    }
}

/// C `update_dpb` (`pd_process.c:5179-5191`) — static.
///
/// Applies `refresh_frame_mask` to the shadow DPB. Skipping it makes every
/// frame after the first inter frame read wrong reference POCs.
pub fn update_dpb(pic: &PicParams, ctx: &mut PicDecisionCtx) {
    if pic.rps.refresh_frame_mask != 0 {
        for i in 0..REF_FRAMES {
            if (pic.rps.refresh_frame_mask >> i) & 1 == 1 {
                ctx.dpb[i].picture_number = pic.picture_number;
                ctx.dpb[i].decode_order = pic.decode_order;
                ctx.dpb[i].temporal_layer_index = pic.temporal_layer_index;
            }
        }
    }
}

/// C `set_key_frame_rps` (`pd_process.c:1480-1490`) — static.
///
/// Resets the toggles and the managed-ref slot state at each key frame.
/// Without it the toggle sequence that picks DPB slots desynchronises from C
/// after the first GOP.
///
/// The C body also calls `ref_mgmt_reset_state`, which clears
/// `ctx->pic_id_per_dpb_slot`: a key frame refreshes all eight DPB slots, so
/// every long-term anchor the application was holding is destroyed and its id
/// must stop resolving.
pub fn set_key_frame_rps(pic: &mut PicParams, ctx: &mut PicDecisionCtx) {
    ctx.lay0_toggle = 0;
    ctx.lay1_toggle = 0;
    crate::port_ref_mgmt::reset_state(ctx);
    pic.show_frame = true;
    pic.has_show_existing = false;
}

/// C `set_ref_list_counts` (`pd_process.c:1804-1900`) — static.
///
/// Derives `ref_list0_count` / `ref_list1_count` by de-duplicating
/// `ref_poc_array` and capping against [`MrpCtrls`]. Feeds [`prune_refs`],
/// [`update_count_try`] and [`set_all_ref_frame_type`], so the number of
/// signalled references and the whole MD candidate set are wrong without it.
///
/// Two index traps transcribed literally:
/// * the list-1 inner loop starts at `LAST2` when `i == BWD` and at `LAST`
///   otherwise — BWD and LAST are *allowed* to match (they do in base layer);
/// * the `j <= GOLD && j + 1 > ref_list0_count` guard skips list-0 entries
///   that this frame will not actually signal.
pub fn set_ref_list_counts(pic: &mut PicParams, seq: &SeqPicParams, ctx: &PicDecisionCtx) {
    if pic.slice_type == SliceType::I {
        pic.ref_list0_count = 0;
        pic.ref_list1_count = 0;
        return;
    }

    let mrp = &seq.mrp_ctrls;
    let is_base = frame_is_boosted(pic);
    let poc = &pic.rps.ref_poc_array;

    // list 0
    let mut list0_count: u8 = 1;
    let mut breakout = false;
    for i in LAST2..=GOLD {
        if breakout {
            break;
        }
        for j in LAST..i {
            if poc[i] == poc[j] {
                breakout = true;
                break;
            }
        }
        if !breakout {
            list0_count += 1;
        }
    }
    pic.ref_list0_count = list0_count.min(if is_base {
        mrp.base_ref_list0_count
    } else {
        mrp.non_base_ref_list0_count
    });

    if is_incomp_mg_frame(pic, seq) || pic.is_overlay {
        pic.ref_list1_count = 0;
        return;
    }

    // list 1
    let mut list1_count: u8 = 0;
    breakout = false;
    for i in BWD..=ALT {
        if breakout {
            break;
        }
        let jstart = if i == BWD { LAST2 } else { LAST };
        for j in jstart..i {
            if j <= GOLD && (j as u8) + 1 > pic.ref_list0_count {
                continue;
            }
            // S-frame mini-GOP: list 0 gets pruned in set_all_ref_frame_type,
            // so the duplicate check must not also empty list 1.
            if seq.pred_structure == PredStructure::RandomAccess
                && pic.picture_number < ctx.sframe_poc
                && j <= GOLD
                && poc[j] == ctx.sframe_poc
            {
                continue;
            }
            if poc[i] == poc[j] {
                breakout = true;
                break;
            }
        }
        if !breakout {
            list1_count += 1;
        }
    }
    pic.ref_list1_count = list1_count.min(if is_base {
        mrp.base_ref_list1_count
    } else {
        mrp.non_base_ref_list1_count
    });
}

/// C `svt_get_ref_frame_type` (`mode_decision.c:265`) via `to_ref_frame`.
///
/// List 0 is {LAST, LAST2, LAST3, GOLDEN}, list 1 is {BWDREF, ALTREF2, ALTREF}.
#[must_use]
pub fn get_ref_frame_type(list: u8, ref_idx: u8) -> i8 {
    const TO_REF_FRAME: [[i8; 4]; 2] = [
        [LAST_FRAME, LAST2_FRAME, LAST3_FRAME, GOLDEN_FRAME],
        [BWDREF_FRAME, ALTREF2_FRAME, ALTREF_FRAME, 0],
    ];
    TO_REF_FRAME[list as usize][ref_idx as usize]
}

/// C `set_all_ref_frame_type` (`pd_process.c:1044-1099`) — static.
///
/// Builds the exact ordered set of single and compound reference candidates MD
/// walks: single list-0, single list-1, every list0 x list1 bi-directional
/// compound, then (B slices only) the uni-directional compounds. A different
/// set is a different RD winner on essentially every block.
///
/// The S-frame pruning tail (`prune_sframe_refs`) is NOT applied here — it is
/// gated on `ctx->sframe_poc > 0 && scs->mfmv_enabled`, and S-frames are
/// outside this port's envelope. [`set_all_ref_frame_type`] therefore matches C
/// exactly whenever no S-frame is pending, which is every configuration the
/// port encodes today.
pub fn set_all_ref_frame_type(pic: &PicParams) -> ([i8; MODE_CTX_REF_FRAMES], u8) {
    let mut arr = [0i8; MODE_CTX_REF_FRAMES];
    let mut tot: usize = 0;

    // single ref - list 0
    for ref_idx0 in 0..pic.ref_list0_count_try {
        arr[tot] = get_ref_frame_type(0, ref_idx0);
        tot += 1;
    }
    // single ref - list 1
    for ref_idx1 in 0..pic.ref_list1_count_try {
        arr[tot] = get_ref_frame_type(1, ref_idx1);
        tot += 1;
    }
    // compound bi-dir
    for ref_idx0 in 0..pic.ref_list0_count_try {
        for ref_idx1 in 0..pic.ref_list1_count_try {
            let rf = [
                get_ref_frame_type(0, ref_idx0),
                get_ref_frame_type(1, ref_idx1),
            ];
            arr[tot] = av1_ref_frame_type(rf);
            tot += 1;
        }
    }
    if pic.slice_type == SliceType::B {
        // compound uni-dir
        if pic.ref_list0_count_try > 1 {
            arr[tot] = av1_ref_frame_type([LAST_FRAME, LAST2_FRAME]);
            tot += 1;
            if pic.ref_list0_count_try > 2 {
                arr[tot] = av1_ref_frame_type([LAST_FRAME, LAST3_FRAME]);
                tot += 1;
                if pic.ref_list0_count_try > 3 {
                    arr[tot] = av1_ref_frame_type([LAST_FRAME, GOLDEN_FRAME]);
                    tot += 1;
                }
            }
        }
        if pic.ref_list1_count_try > 2 {
            arr[tot] = av1_ref_frame_type([BWDREF_FRAME, ALTREF_FRAME]);
            tot += 1;
        }
    }
    (arr, tot as u8)
}

/// C `set_frame_display_params` (`pd_process.c:1132-1161`) — static.
///
/// Returns false exactly where C does (a B frame of a complete random-access
/// mini-GOP), which tells the caller to derive show/show-existing from the
/// picture index instead.
pub fn set_frame_display_params(
    pic: &mut PicParams,
    ctx: &PicDecisionCtx,
    mini_gop_index: usize,
) -> bool {
    if pic.pred_struct_type == PredStructure::LowDelay || pic.is_overlay {
        pic.show_frame = true;
        pic.has_show_existing = false;
    } else if pic.slice_type == SliceType::I {
        // Key frames are handled before this; the remaining I cases are a
        // mini-GOP broken by a scene change / intra refresh (shown) vs a
        // complete one (hidden, emitted later as show_existing).
        if ctx.mini_gop_length[mini_gop_index] < pic.pred_struct_entry_count {
            pic.show_frame = true;
            pic.has_show_existing = false;
        } else {
            pic.show_frame = false;
            pic.has_show_existing = false;
        }
    } else {
        return false;
    }
    true
}

/// C `set_ref_frame_sign_bias` (`pd_process.c:4894-4909`) — static.
///
/// Fills `ref_frame_sign_bias[8]` via `get_relative_dist`. Consumed by the MVP
/// stack, compound-mode allowance and MFMV projection; a wrong sign bias
/// silently mis-signs every temporal MV candidate.
///
/// Index trap: `ref_frame_sign_bias` is indexed by `MvReferenceFrame`
/// (`LAST_FRAME`=1..`ALTREF_FRAME`=7) while `ref_order_hint` is indexed by
/// `ref_frame - 1`. Slot 0 (`INTRA_FRAME`) always stays 0.
pub fn set_ref_frame_sign_bias(pic: &mut PicParams, seq: &SeqPicParams) {
    pic.ref_frame_sign_bias = [0; REF_FRAMES];
    if seq.order_hint_info.enable_order_hint {
        for ref_frame in LAST_FRAME..=ALTREF_FRAME {
            let hint = pic.ref_order_hint[(ref_frame - 1) as usize] as i32;
            pic.ref_frame_sign_bias[ref_frame as usize] = i32::from(
                get_relative_dist(seq.order_hint_info, hint, pic.cur_order_hint as i32) > 0,
            );
        }
    }
}

/// C `set_layer_depth` (`pd_process.c:4576-4583`) — static.
pub fn set_layer_depth(pic: &mut PicParams) {
    pic.layer_depth = if pic.is_key_frame {
        0
    } else {
        pic.temporal_layer_index + 1
    };
}

/// C `set_frame_update_type` (`pd_process.c:4591-4611`) — static.
///
/// The video-mode qindex derivation (`svt_av1_frame_type_qdelta`) keys directly
/// off `update_type`, so a wrong value here is a wrong `base_q_idx` on every
/// frame.
///
/// Trap: the flat (`hierarchical_levels == 0`) arm uses
/// `frame_offset % MAX(4, 1 << hierarchical_levels)`, which is
/// `frame_offset % 4` — the `1 << 0 == 1` term never wins. Every 4th frame from
/// the last IDR is a `GF_UPDATE`, odd offsets are `LF_UPDATE`, and the
/// remaining even offsets are `INTNL_ARF_UPDATE`.
pub fn set_frame_update_type(pic: &mut PicParams) {
    pic.update_type = if pic.is_key_frame {
        FrameUpdateType::Kf
    } else if pic.hierarchical_levels > 0 {
        if pic.temporal_layer_index == 0 {
            FrameUpdateType::Arf
        } else if pic.temporal_layer_index == pic.hierarchical_levels {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    } else {
        let m = u64::from(4u32.max(1u32 << pic.hierarchical_levels));
        if pic.frame_offset.is_multiple_of(m) {
            FrameUpdateType::Gf
        } else if pic.frame_offset & 1 == 1 {
            FrameUpdateType::Lf
        } else {
            FrameUpdateType::IntnlArf
        }
    };
}

/// C `set_gf_group_param` (`pd_process.c:4612-4615`) — static.
///
/// Trivial, but it fixes the ORDER: update type first, then layer depth. Both
/// feed the video qindex derivation.
pub fn set_gf_group_param(pic: &mut PicParams) {
    set_frame_update_type(pic);
    set_layer_depth(pic);
}

// ---------------------------------------------------------------------------
// av1_generate_rps_info (pd_process.c:1911-3506) — static, tier 4
// ---------------------------------------------------------------------------

/// A prediction-structure branch of `av1_generate_rps_info` this port has not
/// translated yet.
///
/// Returned instead of a guess, per `docs/WORKING-ON-THIS.md` §6: a
/// plausible-but-wrong reference structure is indistinguishable from a correct
/// one at the integration seam and would produce a decodable stream that
/// predicts from the wrong pictures. Refusing is the correct behaviour until
/// the arm is transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpsBranchUnsupported {
    /// `pcs->hierarchical_levels` of the refused picture.
    pub hierarchical_levels: u8,
    /// `pcs->temporal_layer_index` of the refused picture.
    pub temporal_layer: u8,
}

/// Why [`generate_rps_info`] declined to produce a reference structure.
///
/// Both variants are refusals in the sense of `docs/WORKING-ON-THIS.md` §6 —
/// the alternative is a stream that decodes while predicting from the wrong
/// pictures, which is indistinguishable from a correct one at the integration
/// seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpsError {
    /// A prediction-structure branch this port has not translated, or one C
    /// itself rejects (`Unsupported MG structure!`, `pd_process.c:3484`).
    UnsupportedBranch {
        /// `pcs->hierarchical_levels` of the refused picture.
        hierarchical_levels: u8,
        /// `pcs->temporal_layer_index` of the refused picture.
        temporal_layer: u8,
    },
    /// A `(temporal_layer, pic_idx)` pair the branch's table does not cover.
    ///
    /// C logs `Error in MG indexing - HL%d, temporal layer %d` here and then
    /// **falls through with the previous picture's `ref_dpb_index`**, so it
    /// emits an RPS built from stale slots. This port refuses instead.
    MiniGopIndex {
        /// `pcs->hierarchical_levels` of the refused picture.
        hierarchical_levels: u8,
        /// `pcs->temporal_layer_index` of the refused picture.
        temporal_layer: u8,
        /// `pic_idx` — the position inside the mini-GOP.
        pic_idx: u32,
    },
}

impl From<RpsBranchUnsupported> for RpsError {
    fn from(e: RpsBranchUnsupported) -> Self {
        Self::UnsupportedBranch {
            hierarchical_levels: e.hierarchical_levels,
            temporal_layer: e.temporal_layer,
        }
    }
}

impl core::fmt::Display for RpsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::UnsupportedBranch {
                hierarchical_levels,
                temporal_layer,
            } => write!(
                f,
                "unsupported RPS branch: hierarchical_levels {hierarchical_levels}, temporal layer {temporal_layer}"
            ),
            Self::MiniGopIndex {
                hierarchical_levels,
                temporal_layer,
                pic_idx,
            } => write!(
                f,
                "mini-GOP index {pic_idx} is not a coded position at hierarchical_levels {hierarchical_levels}, temporal layer {temporal_layer}"
            ),
        }
    }
}

impl std::error::Error for RpsError {}

/// C `av1_generate_rps_info` (`pd_process.c:1911-3506`) — static, tier 4.
///
/// THE reference-structure derivation: fills `ref_dpb_index[7]`,
/// `ref_poc_array[7]`, `refresh_frame_mask` and `is_ref`. Without it a port
/// has no DPB slot mapping and no refresh mask, so `ref_frame_idx[]` and
/// `refresh_frame_flags` in every inter frame header are invented.
///
/// **Coverage — 4 of the 8 top-level branches are translated here.**
/// Translated: the RTC flat branch (`rtc && hierarchical_levels == 0`), the
/// low-delay CQP/CRF branch, the low-delay CBR branch (hierarchical levels 1
/// and 2, the only two LD CBR supports), and the random-access flat branch
/// (`hierarchical_levels == 0`). NOT translated, and refused rather than
/// guessed: the random-access hierarchical branches at `hierarchical_levels`
/// 1, 2, 3, 4 and 5 (`pd_process.c:2270-3483`, ~1200 lines of per-layer
/// per-position tables). Those are needed for random access, not for the
/// campaign's first cell (low-delay P, flat, CQP).
///
/// Not translated at all, in any branch, because they are outside the port's
/// envelope: the S-frame paths (`set_sframe_type`, `set_sframe_rps`,
/// `decide_sframe_mg`) and the app-driven reference-management events
/// (`apply_ref_mgmt_events`, which can mask `refresh_frame_mask` bits held by
/// a STORE). Both are no-ops when no S-frame and no STORE/CLEAR/USE event is
/// pending, which is every configuration this port encodes.
///
/// # Errors
///
/// Returns [`RpsError`] for a branch this port does not translate, or for a
/// mini-GOP position the branch's table does not cover.
pub fn generate_rps_info(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsError> {
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;

    pic.is_ref = if seq.allintra {
        false
    } else {
        is_pic_used_as_ref(
            u32::from(hier),
            u32::from(temporal_layer),
            pic_idx,
            u32::from(seq.mrp_ctrls.referencing_scheme),
            pic.is_overlay,
        )
    };

    // Set frame type
    if pic.slice_type == SliceType::I {
        pic.rps.refresh_frame_mask = 0xFF;
        if pic.is_key_frame {
            set_key_frame_rps(pic, ctx);
            set_ref_list_counts(pic, seq, ctx);
            // C's key-frame early return still runs the ref-management
            // dispatcher (`pd_process.c:1265-1268`): the key frame refreshes
            // all eight slots, and if the application STOREd it, that must be
            // recorded now.
            crate::port_ref_mgmt::apply_events(pic, seq, ctx);
            return Ok(());
        }
    }

    if seq.rtc && hier == 0 {
        rps_rtc_flat(pic, seq, ctx, mg_idx);
    } else if seq.pred_structure == PredStructure::LowDelay
        && seq.rate_control_mode == RcMode::CqpOrCrf
    {
        rps_low_delay_cqp(pic, seq, ctx, pic_idx, mg_idx);
    } else if seq.pred_structure == PredStructure::LowDelay && seq.rate_control_mode == RcMode::Cbr
    {
        rps_low_delay_cbr(pic, seq, ctx, pic_idx, mg_idx)?;
    } else if hier == 0 {
        rps_random_access_flat(pic, seq, ctx, mg_idx);
    } else {
        crate::port_picstruct_ra::rps_random_access_hier(pic, seq, ctx, pic_idx, mg_idx)?;
    }

    // C's tail (`pd_process.c:3487-3502`): the S-frame RPS (out of envelope,
    // see the module doc), then the ref-management events, then the overlay
    // reset. Phase 3 of the dispatcher runs unconditionally, so this is NOT
    // skippable even when the application queued nothing — it is a no-op only
    // while no slot is held.
    crate::port_ref_mgmt::apply_events(pic, seq, ctx);
    if pic.is_overlay {
        pic.rps.refresh_frame_mask = 0;
    }
    Ok(())
}

/// C `av1_generate_rps_info`'s `scs->static_config.rtc && hierarchical_levels == 0`
/// branch (`pd_process.c:1954-1986`).
///
/// Up to `flat_max_refs` consecutive previous frames as list-0 references;
/// list 1 mirrors LAST. The refresh mask deliberately also sets the bits of
/// the slots this configuration never uses (`0xf0` plus every bit at or above
/// `max_refs`) so old pictures are dropped and their buffers freed.
fn rps_rtc_flat(pic: &mut PicParams, seq: &SeqPicParams, ctx: &mut PicDecisionCtx, mg_idx: usize) {
    let max_refs = seq.mrp_ctrls.flat_max_refs;
    let pic0_idx = ctx.lay0_toggle; // newest pic
    let pic1_idx = circ_dec(pic0_idx, 0, max_refs - 1);
    let pic2_idx = circ_dec(pic1_idx, 0, max_refs - 1);
    let pic3_idx = circ_dec(pic2_idx, 0, max_refs - 1);

    pic.rps.ref_dpb_index[LAST] = pic0_idx;
    pic.rps.ref_dpb_index[LAST2] = pic1_idx;
    pic.rps.ref_dpb_index[LAST3] = pic2_idx;
    pic.rps.ref_dpb_index[GOLD] = pic3_idx;
    pic.rps.ref_dpb_index[BWD] = pic.rps.ref_dpb_index[LAST];
    pic.rps.ref_dpb_index[ALT2] = pic.rps.ref_dpb_index[LAST];
    pic.rps.ref_dpb_index[ALT] = pic.rps.ref_dpb_index[LAST];

    // Layer0 toggle 0->1->2->3
    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, max_refs - 1);
    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
    let mut i = 3i32;
    while i >= i32::from(max_refs) {
        pic.rps.refresh_frame_mask |= 1u8 << i;
        i -= 1;
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
}

/// C `av1_generate_rps_info`'s low-delay CQP/CRF branch
/// (`pd_process.c:1987-2064`) — the campaign's first cell.
///
/// The structure is the previous 3 non-base frames + the previous 3 base
/// frames + one long-term reference in slot 7, refreshed every 128 pictures.
///
/// Trap: `lay1_pic_idx` is `(1 << (hierarchical_levels - 1)) - 1` and is
/// special-cased to 0 at `hierarchical_levels == 0` — the shift would be
/// `1 << -1` otherwise. It selects whether a non-base picture past the layer-1
/// picture takes the layer-1 picture as LAST instead of the previous base.
fn rps_low_delay_cqp(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) {
    let mrp = &seq.mrp_ctrls;
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;

    let base2_idx = ctx.lay0_toggle; // newest L0 in the DPB
    let base1_idx = circ_dec(base2_idx, 0, 2); // middle L0
    let base0_idx = circ_dec(base1_idx, 0, 2); // oldest L0

    let lay1_offset = if mrp.ld_reduce_ref_buffs == 0 {
        LAY1_OFF
    } else {
        1
    };
    let lay1_2_idx = if mrp.ld_reduce_ref_buffs == 2 {
        1
    } else {
        lay1_offset + ctx.lay1_toggle
    };
    let lay1_1_idx = circ_dec(lay1_2_idx, lay1_offset, lay1_offset + 2);
    let lay1_0_idx = circ_dec(lay1_1_idx, lay1_offset, lay1_offset + 2);
    const LONG_BASE_IDX: u8 = 7;
    const LONG_BASE_PIC: u64 = 128;

    let is_base = temporal_layer == 0;
    let ref_list1_count = if is_base {
        mrp.base_ref_list1_count
    } else {
        mrp.non_base_ref_list1_count
    };

    let lay1_pic_idx: u32 = if hier == 0 {
        0
    } else {
        (1u32 << (hier - 1)) - 1
    };
    // When list1 is unused, pictures after the layer-1 picture take the
    // layer-1 picture as LAST instead of the previous base.
    pic.rps.ref_dpb_index[LAST] = if pic_idx > lay1_pic_idx && !is_base && ref_list1_count == 0 {
        lay1_2_idx
    } else {
        base2_idx
    };
    pic.rps.ref_dpb_index[LAST2] = lay1_1_idx;
    pic.rps.ref_dpb_index[LAST3] = LONG_BASE_IDX;
    pic.rps.ref_dpb_index[GOLD] = base0_idx;
    pic.rps.ref_dpb_index[BWD] = lay1_2_idx;
    pic.rps.ref_dpb_index[ALT2] = lay1_0_idx;
    pic.rps.ref_dpb_index[ALT] = base1_idx;

    if temporal_layer == 0 {
        if mrp.ld_reduce_ref_buffs == 2 {
            // Only 2 DPB entries used; refresh the rest to free ref buffers.
            pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
        } else if mrp.ld_reduce_ref_buffs == 1 {
            pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
        } else {
            // Layer0 toggle 0->1->2
            ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
            pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
        }
    } else if pic.is_ref {
        if mrp.ld_reduce_ref_buffs == 2 {
            pic.rps.refresh_frame_mask = 1u8 << 1;
        } else {
            // Layer1 toggle 0->1->2
            ctx.lay1_toggle = circ_inc(ctx.lay1_toggle, 0, 2);
            pic.rps.refresh_frame_mask = 1u8 << (lay1_offset + ctx.lay1_toggle);
        }
    } else {
        pic.rps.refresh_frame_mask = 0;
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    // Keep the long-term base reference in the base layer.
    if pic.picture_number - ctx.last_long_base_pic >= LONG_BASE_PIC && pic.temporal_layer_index == 0
    {
        pic.rps.refresh_frame_mask |= 1u8 << LONG_BASE_IDX;
        ctx.last_long_base_pic = pic.picture_number;
    }
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
}

/// C `av1_generate_rps_info`'s low-delay CBR branch (`pd_process.c:2065-2237`).
///
/// LD CBR supports only `hierarchical_levels` 1 and 2 (C asserts it); anything
/// else is refused here rather than falling into the wrong table.
///
/// # Errors
///
/// Returns [`RpsBranchUnsupported`] for a hierarchical level or temporal layer
/// C itself logs as unexpected.
fn rps_low_delay_cbr(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsBranchUnsupported> {
    let mrp = &seq.mrp_ctrls;
    let hier = pic.hierarchical_levels;
    let temporal_layer = pic.temporal_layer_index;
    let lay0_toggle = ctx.lay0_toggle;
    let lay1_toggle = ctx.lay1_toggle;

    let base2_idx = lay0_toggle;
    let base1_idx = circ_dec(base2_idx, 0, 2);
    let base0_idx = circ_dec(base1_idx, 0, 2);

    // Index trap: at ld_reduce_ref_buffs == 2 C writes `!lay0_toggle`, the
    // LOGICAL negation of the toggle (0 -> 1, anything else -> 0), NOT a
    // bitwise complement and not `LAY1_OFF + something`.
    let lay1_1_idx = if mrp.ld_reduce_ref_buffs == 2 {
        u8::from(lay0_toggle == 0)
    } else if mrp.ld_reduce_ref_buffs == 1 {
        LAY1_OFF
    } else {
        LAY1_OFF + lay1_toggle
    };
    let lay1_0_idx = circ_dec(lay1_1_idx, LAY1_OFF, LAY1_OFF + 1);
    let lay2_idx = LAY2_OFF;
    const LONG_BASE_IDX: u8 = 7;
    const LONG_BASE_PIC: u64 = 128;

    let idx = &mut pic.rps.ref_dpb_index;
    if hier == 1 {
        match temporal_layer {
            0 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = base1_idx;
                idx[LAST3] = LONG_BASE_IDX;
                idx[GOLD] = base0_idx;
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
                } else {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
                }
            }
            1 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = if mrp.referencing_scheme == 0 {
                    base1_idx
                } else {
                    lay1_1_idx
                };
                idx[LAST3] = base1_idx;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                pic.rps.refresh_frame_mask = 0;
                if pic.is_ref {
                    if mrp.ld_reduce_ref_buffs == 2 {
                        pic.rps.refresh_frame_mask = (1u8 << u8::from(ctx.lay0_toggle == 0)) | 0xfc;
                    } else if mrp.ld_reduce_ref_buffs == 1 {
                        pic.rps.refresh_frame_mask = (1u8 << LAY1_OFF) | 0xf0;
                    } else {
                        // Layer1 toggle 0->1
                        ctx.lay1_toggle = 1 - ctx.lay1_toggle;
                        pic.rps.refresh_frame_mask = 1u8 << (LAY1_OFF + ctx.lay1_toggle);
                    }
                }
            }
            _ => {
                return Err(RpsBranchUnsupported {
                    hierarchical_levels: hier,
                    temporal_layer,
                });
            }
        }
    } else if hier == 2 {
        match temporal_layer {
            0 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = base0_idx;
                idx[LAST3] = LONG_BASE_IDX;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = (1u8 << ctx.lay0_toggle) | 0xf0;
                } else {
                    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 2);
                    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;
                }
            }
            1 => {
                idx[LAST] = base2_idx;
                idx[LAST2] = lay1_1_idx;
                idx[LAST3] = base1_idx;
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                if mrp.ld_reduce_ref_buffs == 2 {
                    pic.rps.refresh_frame_mask = (1u8 << u8::from(ctx.lay0_toggle == 0)) | 0xfc;
                } else if mrp.ld_reduce_ref_buffs == 1 {
                    pic.rps.refresh_frame_mask = (1u8 << LAY1_OFF) | 0xf0;
                } else {
                    ctx.lay1_toggle = 1 - ctx.lay1_toggle;
                    pic.rps.refresh_frame_mask = 1u8 << (LAY1_OFF + ctx.lay1_toggle);
                }
            }
            2 => {
                if pic_idx == 0 {
                    idx[LAST] = base2_idx;
                    idx[LAST2] = lay1_1_idx;
                    idx[LAST3] = base1_idx;
                } else if pic_idx == 2 {
                    idx[LAST] = lay1_1_idx;
                    idx[LAST2] = base2_idx;
                    idx[LAST3] = lay1_0_idx;
                } else {
                    // C logs "Error in MG indexing - LD CBR HL2" and leaves the
                    // indices at whatever they were. Refuse instead.
                    return Err(RpsBranchUnsupported {
                        hierarchical_levels: hier,
                        temporal_layer,
                    });
                }
                idx[GOLD] = idx[LAST];
                idx[BWD] = idx[LAST];
                idx[ALT2] = idx[LAST];
                idx[ALT] = idx[LAST];

                pic.rps.refresh_frame_mask = if pic.is_ref { 1u8 << lay2_idx } else { 0 };
                // Redundant in C, kept to avoid a hang on a bad setting.
                if mrp.ld_reduce_ref_buffs != 0 {
                    pic.rps.refresh_frame_mask = 0;
                }
            }
            _ => {
                return Err(RpsBranchUnsupported {
                    hierarchical_levels: hier,
                    temporal_layer,
                });
            }
        }
    } else {
        // C asserts hierarchical_levels == 2 here.
        return Err(RpsBranchUnsupported {
            hierarchical_levels: hier,
            temporal_layer,
        });
    }

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    if seq.pred_structure == PredStructure::LowDelay
        && pic.picture_number - ctx.last_long_base_pic >= LONG_BASE_PIC
        && pic.temporal_layer_index == 0
    {
        pic.rps.refresh_frame_mask |= 1u8 << LONG_BASE_IDX;
        ctx.last_long_base_pic = pic.picture_number;
    }
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );
    set_frame_display_params(pic, ctx, mg_idx);
    Ok(())
}

/// C `av1_generate_rps_info`'s `hierarchical_levels == 0` branch
/// (`pd_process.c:2238-2269`) — random access, flat.
///
/// Walks all 8 DPB slots: list 0 takes the 1st/3rd/5th/8th newest base
/// pictures and list 1 the 2nd/4th/6th, matching the `{1,3,5,7}` /
/// `{2,4,6,0}` GOP tables in the C comment.
///
/// Note the ORDER: the toggle advances AFTER `prune_refs`, unlike the
/// low-delay branches where it advances before `update_ref_poc_array`. A port
/// that hoists the toggle to the top of the branch reads the wrong slot for
/// LAST on every frame.
fn rps_random_access_flat(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    mg_idx: usize,
) {
    let base0_idx = ctx.lay0_toggle;
    let base1_idx = circ_dec(base0_idx, 0, 7);
    let base2_idx = circ_dec(base1_idx, 0, 7);
    let base3_idx = circ_dec(base2_idx, 0, 7);
    let base4_idx = circ_dec(base3_idx, 0, 7);
    let base5_idx = circ_dec(base4_idx, 0, 7);
    let base7_idx = circ_dec(base5_idx, 0, 7);

    pic.rps.ref_dpb_index[LAST] = base0_idx;
    pic.rps.ref_dpb_index[LAST2] = base2_idx;
    pic.rps.ref_dpb_index[LAST3] = base4_idx;
    pic.rps.ref_dpb_index[GOLD] = base7_idx;
    pic.rps.ref_dpb_index[BWD] = base1_idx;
    pic.rps.ref_dpb_index[ALT2] = base3_idx;
    pic.rps.ref_dpb_index[ALT] = base5_idx;

    update_ref_poc_array(&mut pic.rps, &ctx.dpb);
    set_ref_list_counts(pic, seq, ctx);
    prune_refs(
        &mut pic.rps,
        u32::from(pic.ref_list0_count),
        u32::from(pic.ref_list1_count),
    );

    ctx.lay0_toggle = circ_inc(ctx.lay0_toggle, 0, 7);
    pic.rps.refresh_frame_mask = 1u8 << ctx.lay0_toggle;

    // Flat mode outputs every frame; C calls set_frame_display_params and then
    // unconditionally overrides both fields.
    set_frame_display_params(pic, ctx, mg_idx);
    pic.show_frame = true;
    pic.has_show_existing = false;
}

// ---------------------------------------------------------------------------
// init_pic_settings + the per-picture call sequence
// ---------------------------------------------------------------------------

/// C `init_pic_settings` (`pd_process.c:4910-4965`) — static, tier 4.
///
/// The single per-picture inter-settings funnel: `reference_mode`,
/// `mi_cols`/`mi_rows`, `ref_order_hint[]`/`cur_order_hint`, sign bias, the
/// try counts, the skip-mode params and `ref_frame_type_arr`. Every one is
/// either a written header field or an MD gate.
///
/// **Two C calls in this body are deliberately NOT made here** because they
/// belong to other modules and other lanes own them:
/// * `copy_tf_params(scs, pcs, ctx)` — the temporal-filter control mapping.
///   Measured: in `LOW_DELAY` `tf_level` is forced to 0 before any preset
///   logic (`enc_handle.c:3339-3343`), so this is a no-op for the campaign's
///   first cell; it is live in random access.
/// * `svt_aom_sig_deriv_multi_processes_{allintra,rtc,default}` — the
///   per-preset feature-level derivation (a different file group entirely).
///
/// Everything else in the C body is reproduced, in C's order.
///
/// Index trap: `ref_order_hint[i]` is `ref_poc_array[i] % (1 << order_hint_bits)`
/// — the array is indexed by `REF_FRAME_MINUS1` (0..6), while
/// [`set_ref_frame_sign_bias`] indexes the SAME data by `MvReferenceFrame`
/// (1..7). Getting the two off by one silently mis-signs every temporal MV.
pub fn init_pic_settings(pic: &mut PicParams, seq: &SeqPicParams, ctx: &mut PicDecisionCtx) {
    pic.allow_comp_inter_inter = pic.slice_type != SliceType::I;
    pic.reference_mode = if pic.slice_type == SliceType::I {
        ReferenceMode::IntraSentinel
    } else if is_incomp_mg_frame(pic, seq) {
        ReferenceMode::Single
    } else {
        ReferenceMode::Select
    };

    // mi_cols/mi_rows come from the ALIGNED dimensions, not the display ones.
    pic.mi_cols = pic.aligned_width >> MI_SIZE_LOG2;
    pic.mi_rows = pic.aligned_height >> MI_SIZE_LOG2;

    // Initialize the order hints.
    let bits = seq.order_hint_info.order_hint_bits;
    let modulus = 1u64 << bits;
    for i in 0..INTER_REFS_PER_FRAME {
        pic.ref_order_hint[i] = (pic.rps.ref_poc_array[i] % modulus) as u32;
    }
    pic.cur_order_hint = (pic.picture_number % modulus) as u32;

    set_ref_frame_sign_bias(pic, seq);

    // copy_tf_params + sig_deriv_multi_processes: see the doc comment.

    update_count_try(pic, seq);

    if ctx.transition_detected == 1 && pic.temporal_layer_index == 0 {
        pic.transition_present = 1;
        ctx.transition_detected = 0;
    }

    if ctx.list0_only && pic.slice_type == SliceType::B && pic.temporal_layer_index == 0 {
        pic.ref_list1_count_try = 0;
    }
    debug_assert!(pic.ref_list0_count_try <= pic.ref_list0_count);
    debug_assert!(pic.ref_list1_count_try <= pic.ref_list1_count);

    // Skip mode syntax, spec 5.9.22.
    setup_skip_mode_allowed(pic, seq);
    pic.skip_mode.skip_mode_flag = pic.skip_mode.skip_mode_allowed;

    let (arr, tot) = set_all_ref_frame_type(pic);
    pic.ref_frame_type_arr = arr;
    pic.tot_ref_frame_types = tot;
}

/// C `MI_SIZE_LOG2`.
pub const MI_SIZE_LOG2: u32 = 2;

/// The per-picture call sequence from `svt_aom_picture_decision_kernel_iter`
/// (`pd_process.c:5672-5692`), transcribed.
///
/// **The order is load-bearing and is NOT what a reading of the file suggests.**
/// Measured in the C source at the call site rather than inferred:
///
/// ```text
/// frm_hdr.frame_type = ...     (5674-5679)
/// set_gf_group_param(pcs)      (5680)   <-- BEFORE the RPS, not after
/// av1_generate_rps_info(...)   (5681)
/// update_dpb(pcs, ctx)         (5688)
/// init_pic_settings(...)       (5691)
/// ```
///
/// `set_gf_group_param` running FIRST is what makes `frame_is_boosted` — and
/// therefore the base-vs-non-base MRP caps inside
/// [`set_ref_list_counts`], which `av1_generate_rps_info` calls — read THIS
/// picture's `update_type` rather than a stale one. A port that ran the RPS
/// first would silently cap both lists with the wrong row of [`MrpCtrls`].
///
/// # Errors
///
/// Propagates [`RpsError`] from [`generate_rps_info`].
pub fn picture_decision_per_picture(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsError> {
    set_gf_group_param(pic);
    generate_rps_info(pic, seq, ctx, pic_idx, mg_idx)?;
    update_dpb(pic, ctx);
    init_pic_settings(pic, seq, ctx);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mini-GOP structure (pd_process.c:759-988, 4720-4893) + utility.c's table
// ---------------------------------------------------------------------------

/// C `MINI_GOP_MAX_COUNT` (`utility.h:168`).
pub const MINI_GOP_MAX_COUNT: usize = 31;
/// C `MIN_HIERARCHICAL_LEVEL` (`utility.h:171`).
pub const MIN_HIERARCHICAL_LEVEL: u8 = 1;
/// C `MAX_HIERARCHICAL_LEVEL` (`API/EbSvtAv1Enc.h:34`).
pub const MAX_HIERARCHICAL_LEVEL: u8 = 6;
/// C `mini_gop_offset` (`utility.h:172`), indexed by
/// `hierarchical_levels - MIN_HIERARCHICAL_LEVEL`.
pub const MINI_GOP_OFFSET: [u8; (MAX_HIERARCHICAL_LEVEL - MIN_HIERARCHICAL_LEVEL) as usize] =
    [1, 3, 7, 15, 31];

/// C `MinigopIndex` (`utility.h:183-214`) — the entries the activity array is
/// addressed by name.
pub const L6_INDEX: usize = 0;
/// See [`L6_INDEX`].
pub const L5_0_INDEX: usize = 1;
/// See [`L6_INDEX`].
pub const L4_0_INDEX: usize = 2;
/// See [`L6_INDEX`].
pub const L3_0_INDEX: usize = 3;
/// See [`L6_INDEX`].
pub const L2_0_INDEX: usize = 4;
/// See [`L6_INDEX`].
pub const L3_2_INDEX: usize = 10;
/// See [`L6_INDEX`].
pub const L2_2_INDEX: usize = 7;
/// See [`L6_INDEX`].
pub const L2_4_INDEX: usize = 11;
/// See [`L6_INDEX`].
pub const L2_6_INDEX: usize = 14;
/// See [`L6_INDEX`].
pub const L5_1_INDEX: usize = 16;
/// See [`L6_INDEX`].
pub const L4_2_INDEX: usize = 17;
/// See [`L6_INDEX`].
pub const L3_4_INDEX: usize = 18;
/// See [`L6_INDEX`].
pub const L2_8_INDEX: usize = 19;
/// See [`L6_INDEX`].
pub const L2_10_INDEX: usize = 22;
/// See [`L6_INDEX`].
pub const L3_6_INDEX: usize = 25;
/// See [`L6_INDEX`].
pub const L2_12_INDEX: usize = 26;
/// See [`L6_INDEX`].
pub const L2_14_INDEX: usize = 29;

/// C `MiniGopStats` (`utility.h:174-179`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniGopStats {
    /// C `hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `start_index` into the pre-assignment buffer.
    pub start_index: u8,
    /// C `end_index` (inclusive).
    pub end_index: u8,
    /// C `length` (== `end_index - start_index + 1`).
    pub length: u8,
}

/// C `mini_gop_stats_array` (`utility.c:129-161`) — the 31 candidate mini-GOP
/// shapes a 32-picture pre-assignment buffer can be cut into.
const MINI_GOP_STATS_ARRAY: [MiniGopStats; MINI_GOP_MAX_COUNT] = {
    const fn s(h: u8, a: u8, b: u8, l: u8) -> MiniGopStats {
        MiniGopStats {
            hierarchical_levels: h,
            start_index: a,
            end_index: b,
            length: l,
        }
    }
    [
        s(5, 0, 31, 32),
        s(4, 0, 15, 16),
        s(3, 0, 7, 8),
        s(2, 0, 3, 4),
        s(1, 0, 1, 2),
        s(1, 2, 3, 2),
        s(2, 4, 7, 4),
        s(1, 4, 5, 2),
        s(1, 6, 7, 2),
        s(3, 8, 15, 8),
        s(2, 8, 11, 4),
        s(1, 8, 9, 2),
        s(1, 10, 11, 2),
        s(2, 12, 15, 4),
        s(1, 12, 13, 2),
        s(1, 14, 15, 2),
        s(4, 16, 31, 16),
        s(3, 16, 23, 8),
        s(2, 16, 19, 4),
        s(1, 16, 17, 2),
        s(1, 18, 19, 2),
        s(2, 20, 23, 4),
        s(1, 20, 21, 2),
        s(1, 22, 23, 2),
        s(3, 24, 31, 8),
        s(2, 24, 27, 4),
        s(1, 24, 25, 2),
        s(1, 26, 27, 2),
        s(2, 28, 31, 4),
        s(1, 28, 29, 2),
        s(1, 30, 31, 2),
    ]
};

/// C `svt_aom_get_mini_gop_stats` (`utility.c:168-170`) — EXPORTED.
///
/// # Panics
///
/// Panics for `mini_gop_index >= MINI_GOP_MAX_COUNT`; C indexes the array
/// unchecked, so an out-of-range index is caller misuse in both.
#[must_use]
pub fn get_mini_gop_stats(mini_gop_index: usize) -> MiniGopStats {
    MINI_GOP_STATS_ARRAY[mini_gop_index]
}

/// The `EncodeContext` fields the mini-GOP and pred-struct derivation reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EncCtxPicParams {
    /// C `enc_ctx->pre_assignment_buffer_count`.
    pub pre_assignment_buffer_count: u32,
    /// C `enc_ctx->pre_assignment_buffer_intra_count`.
    pub pre_assignment_buffer_intra_count: u32,
    /// C `enc_ctx->pre_assignment_buffer_idr_count`.
    pub pre_assignment_buffer_idr_count: u32,
    /// C `enc_ctx->previous_mini_gop_hierarchical_levels`.
    pub previous_mini_gop_hierarchical_levels: u32,
    /// C `enc_ctx->mini_gop_cnt_per_gop`.
    pub mini_gop_cnt_per_gop: u32,
    /// C `enc_ctx->pred_struct_position`.
    pub pred_struct_position: u32,
    /// C `enc_ctx->last_idr_picture`.
    pub last_idr_picture: u64,
    /// C `enc_ctx->elapsed_non_cra_count`.
    pub elapsed_non_cra_count: u32,
}

/// The mini-GOP window state `set_mini_gop_structure` fills.
///
/// C keeps these as parallel arrays on `PictureDecisionContext`; kept here as
/// one struct so the whole window map moves together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniGopMap {
    /// C `ctx->mini_gop_activity_array[MINI_GOP_MAX_COUNT]`.
    pub activity: [bool; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_start_index[]`.
    pub start_index: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_end_index[]`.
    pub end_index: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_length[]`.
    pub length: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_hierarchical_levels[]`.
    pub hierarchical_levels: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_intra_count[]`.
    pub intra_count: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->mini_gop_idr_count[]`.
    pub idr_count: [u32; MINI_GOP_MAX_COUNT],
    /// C `ctx->total_number_of_mini_gops`.
    pub total_number_of_mini_gops: usize,
    /// C `ctx->enable_startup_mg`.
    pub enable_startup_mg: bool,
    /// C `ctx->is_startup_gop`.
    pub is_startup_gop: bool,
    /// C `ctx->sframe_hier_lvls`.
    pub sframe_hier_lvls: i32,
    /// C `ctx->list0_only`.
    pub list0_only: bool,
}

impl Default for MiniGopMap {
    fn default() -> Self {
        Self {
            activity: [false; MINI_GOP_MAX_COUNT],
            start_index: [0; MINI_GOP_MAX_COUNT],
            end_index: [0; MINI_GOP_MAX_COUNT],
            length: [0; MINI_GOP_MAX_COUNT],
            hierarchical_levels: [0; MINI_GOP_MAX_COUNT],
            intra_count: [0; MINI_GOP_MAX_COUNT],
            idr_count: [0; MINI_GOP_MAX_COUNT],
            total_number_of_mini_gops: 0,
            enable_startup_mg: false,
            is_startup_gop: false,
            // C's picture_decision_context_ctor sets 0 here
            // (pd_process.c:249) -- but that value is DEAD: the kernel
            // overwrites it with the configured hierarchy at picture_number 0
            // (pd_process.c:5407-5409) before set_mini_gop_structure ever
            // reads it. Use `MiniGopMap::for_sequence` so the live value is
            // the one in play; the ctor value is reproduced here only so a
            // reader who greps the C ctor finds the same number.
            sframe_hier_lvls: 0,
            list0_only: false,
        }
    }
}

impl MiniGopMap {
    /// The state as of `picture_number == 0`, where the kernel initialises
    /// `sframe_hier_lvls` from the configured hierarchy
    /// (`pd_process.c:5407-5409`).
    ///
    /// Using [`Default`] instead leaves `sframe_hier_lvls` at C's *ctor* value
    /// of 0, which makes [`set_mini_gop_structure`]'s S-frame override fire on
    /// every non-zero configured hierarchy and collapse the mini-GOP to level
    /// 0. That is a real trap: the ctor line is the one a grep finds first and
    /// it is not the value the code runs with.
    #[must_use]
    pub fn for_sequence(config_hierarchical_levels: u8) -> Self {
        Self {
            sframe_hier_lvls: i32::from(config_hierarchical_levels),
            ..Self::default()
        }
    }
}

/// C `initialize_mini_gop_activity_array` (`pd_process.c:759-848`) — static.
///
/// Chooses which mini-GOP shapes are permitted for the current pre-assignment
/// buffer count. `activity[i] == true` means "this shape is still a candidate
/// and must be subdivided"; the nested cascade clears the flag on the LARGEST
/// shape that fits, then recurses into the remainder.
///
/// Trap: every arm is `count >= N && !(count == N && idr_flag)`. The IDR guard
/// means a buffer holding EXACTLY N pictures whose first is an IDR does NOT
/// take the N-picture shape — an off-by-one that only shows up at a GOP
/// boundary, which is the case a short test cell hits first.
///
/// Returns `true` when the caller must run the dynamic-GOP 6L-vs-5L split
/// (`eval_sub_mini_gop`), which is NOT ported: it needs `early_hme` /
/// `calc_mini_gop_activity`, a different chunk. Measured: `scs->enable_dg` is
/// 1 for single-pass CQP/CRF `RANDOM_ACCESS` below 4K
/// (`enc_handle.c:4294-4300`), so this is on by default there — the caller
/// must not treat a `true` return as an exotic case.
pub fn initialize_mini_gop_activity_array(
    map: &mut MiniGopMap,
    enc: &EncCtxPicParams,
    idr_flag: bool,
    enable_dg: bool,
    list0_only_base: bool,
) -> bool {
    for gopindex in 0..MINI_GOP_MAX_COUNT {
        map.activity[gopindex] =
            get_mini_gop_stats(gopindex).hierarchical_levels > MIN_HIERARCHICAL_LEVEL;
    }

    let n = enc.pre_assignment_buffer_count;
    // `fits(k)` is C's `count >= k && !(count == k && idr_flag)`.
    let fits = |count: u32, k: u32| count >= k && !(count == k && idr_flag);

    if fits(n, 32) {
        map.activity[L6_INDEX] = false;
    } else if fits(n, 16) {
        map.activity[L5_0_INDEX] = false;
        if fits(n - 16, 8) {
            map.activity[L4_2_INDEX] = false;
            if fits(n - 16 - 8, 4) {
                map.activity[L3_6_INDEX] = false;
                if fits(n - 16 - 8 - 4, 2) {
                    map.activity[L2_14_INDEX] = false;
                }
            } else if fits(n - 16 - 8, 2) {
                map.activity[L2_12_INDEX] = false;
            }
        } else if fits(n - 16, 4) {
            map.activity[L3_4_INDEX] = false;
            if fits(n - 16 - 4, 2) {
                map.activity[L2_10_INDEX] = false;
            }
        } else if fits(n - 16, 2) {
            map.activity[L2_8_INDEX] = false;
        }
    } else if fits(n, 8) {
        map.activity[L4_0_INDEX] = false;
        if fits(n - 8, 4) {
            map.activity[L3_2_INDEX] = false;
            if fits(n - 8 - 4, 2) {
                map.activity[L2_6_INDEX] = false;
            }
        } else if fits(n - 8, 2) {
            map.activity[L2_4_INDEX] = false;
        }
    } else if fits(n, 4) {
        map.activity[L3_0_INDEX] = false;
        if fits(n - 4, 2) {
            map.activity[L2_2_INDEX] = false;
        }
    } else if fits(n, 2) {
        map.activity[L2_0_INDEX] = false;
    }

    map.list0_only = list0_only_base;

    // 6L vs 5L: C calls eval_sub_mini_gop here.
    enable_dg && !map.activity[L6_INDEX]
}

/// C `generate_picture_window_split` (`pd_process.c:857-891`) — static.
///
/// Turns the activity array into the concrete list of mini-GOPs. The loop's
/// STRIDE is the subtle part: an ACTIVE (still-to-subdivide) shape advances by
/// 1 so its children are visited, while an INACTIVE (chosen) shape skips its
/// whole subtree via `mini_gop_offset[levels - MIN_HIERARCHICAL_LEVEL]`.
pub fn generate_picture_window_split(map: &mut MiniGopMap, enc: &EncCtxPicParams) {
    map.total_number_of_mini_gops = 0;
    let mut gopindex = 0usize;
    while gopindex < MINI_GOP_MAX_COUNT {
        let stats = get_mini_gop_stats(gopindex);
        if u32::from(stats.end_index) < enc.pre_assignment_buffer_count && !map.activity[gopindex] {
            let t = map.total_number_of_mini_gops;
            map.start_index[t] = u32::from(stats.start_index);
            map.end_index[t] = u32::from(stats.end_index);
            map.length[t] = u32::from(stats.length);
            map.hierarchical_levels[t] = u32::from(stats.hierarchical_levels);
            map.intra_count[t] = 0;
            map.idr_count[t] = 0;
            map.total_number_of_mini_gops += 1;
        }
        gopindex += if map.activity[gopindex] {
            1
        } else {
            usize::from(
                MINI_GOP_OFFSET[(stats.hierarchical_levels - MIN_HIERARCHICAL_LEVEL) as usize],
            )
        };
    }
    if map.total_number_of_mini_gops != 0 {
        let last = map.total_number_of_mini_gops - 1;
        map.intra_count[last] = enc.pre_assignment_buffer_intra_count;
        map.idr_count[last] = enc.pre_assignment_buffer_idr_count;
    }
}

/// C `handle_incomplete_picture_window_map` (`pd_process.c:892-927`) — static.
///
/// Fixes up the last, short mini-GOP at a GOP boundary — exactly the
/// end-of-sequence case a 2- or 5-frame test cell hits first.
pub fn handle_incomplete_picture_window_map(
    hierarchical_level: u32,
    map: &mut MiniGopMap,
    enc: &EncCtxPicParams,
) {
    if map.total_number_of_mini_gops == 0 {
        let hier = hierarchical_level.min(u32::from(MIN_HIERARCHICAL_LEVEL));
        let t = map.total_number_of_mini_gops;
        map.start_index[t] = 0;
        map.end_index[t] = enc.pre_assignment_buffer_count - 1;
        map.length[t] = enc.pre_assignment_buffer_count - map.start_index[t];
        map.hierarchical_levels[t] = hier;
        map.total_number_of_mini_gops += 1;
    } else if map.end_index[map.total_number_of_mini_gops - 1] < enc.pre_assignment_buffer_count - 1
    {
        let t = map.total_number_of_mini_gops;
        map.start_index[t] = map.end_index[t - 1] + 1;
        map.end_index[t] = enc.pre_assignment_buffer_count - 1;
        map.length[t] = enc.pre_assignment_buffer_count - map.start_index[t];
        map.hierarchical_levels[t] = u32::from(MIN_HIERARCHICAL_LEVEL);
        // C zeroes the PREVIOUS entry's counts here, then writes the buffer
        // totals into the NEW last entry two lines later.
        map.intra_count[t - 1] = 0;
        map.idr_count[t - 1] = 0;
        map.total_number_of_mini_gops += 1;
    }
    let last = map.total_number_of_mini_gops - 1;
    map.intra_count[last] = enc.pre_assignment_buffer_intra_count;
    map.idr_count[last] = enc.pre_assignment_buffer_idr_count;
}

/// C `set_mini_gop_structure` (`pd_process.c:4720-4768`) — static.
///
/// Sets the single default mini-GOP covering the whole pre-assignment buffer,
/// then subdivides it (activity array -> window split -> incomplete fixup)
/// only when the buffer holds more than one picture, or holds no intra picture
/// in random access. In low delay `pre_assignment_buffer_count` is 1, so the
/// subdivision never runs and the default MG stands — that is why this
/// function "degenerates" in the campaign's first cell rather than being
/// irrelevant to it.
///
/// Returns `true` when the caller must run the un-ported dynamic-GOP split;
/// see [`initialize_mini_gop_activity_array`].
#[allow(clippy::too_many_arguments)]
pub fn set_mini_gop_structure(
    map: &mut MiniGopMap,
    enc: &mut EncCtxPicParams,
    seq: &SeqPicParams,
    pic: &PicParams,
    config_hierarchical_levels: u32,
    startup_mg_size: u32,
    idr_flag: bool,
    enable_dg: bool,
    list0_only_base: bool,
) -> bool {
    let mut next_mg_hierarchical_levels = config_hierarchical_levels;
    // S-frame mini-GOP size override.
    if map.sframe_hier_lvls != config_hierarchical_levels as i32 {
        next_mg_hierarchical_levels = map.sframe_hier_lvls as u32;
    }
    if map.enable_startup_mg {
        next_mg_hierarchical_levels = startup_mg_size;
    }
    // RTC (implies LOW_DELAY + CBR) supports on-the-fly hierarchy changes.
    if seq.pred_structure == PredStructure::LowDelay
        && seq.rtc
        && seq.rate_control_mode == RcMode::Cbr
    {
        next_mg_hierarchical_levels = u32::from(pic.hierarchical_levels);
    }

    map.start_index[0] = 0;
    map.end_index[0] = enc.pre_assignment_buffer_count - 1;
    map.length[0] = enc.pre_assignment_buffer_count;
    map.hierarchical_levels[0] = next_mg_hierarchical_levels;
    map.intra_count[0] = enc.pre_assignment_buffer_intra_count;
    map.idr_count[0] = enc.pre_assignment_buffer_idr_count;
    map.total_number_of_mini_gops = 1;

    enc.previous_mini_gop_hierarchical_levels = if pic.picture_number == 0 {
        next_mg_hierarchical_levels
    } else {
        enc.previous_mini_gop_hierarchical_levels
    };
    enc.mini_gop_cnt_per_gop = if enc.pre_assignment_buffer_idr_count != 0 {
        0
    } else {
        enc.mini_gop_cnt_per_gop + 1
    };

    let mut needs_dg = false;
    if enc.pre_assignment_buffer_count > 1
        || (enc.pre_assignment_buffer_intra_count == 0
            && seq.pred_structure == PredStructure::RandomAccess)
    {
        needs_dg =
            initialize_mini_gop_activity_array(map, enc, idr_flag, enable_dg, list0_only_base);
        generate_picture_window_split(map, enc);
        handle_incomplete_picture_window_map(next_mg_hierarchical_levels, map, enc);
    }
    needs_dg
}

/// One picture's slice of `get_pred_struct_for_all_frames`
/// (`pd_process.c:942-988`) — static.
///
/// Sets `pred_structure`, `hierarchical_levels` and `is_startup_gop`.
/// Everything downstream (RPS branch selection, layer depth, TF params) keys
/// off these.
///
/// Trap: an IDR takes the SEQUENCE's `hierarchical_levels`, every other
/// picture takes the MINI-GOP's. A port that used one value for the whole
/// buffer picks the wrong RPS branch on the frame after every key frame.
///
/// C also assigns `pcs->pred_struct_ptr` from the prediction-structure group;
/// that table lives in `pred_structure.c` and is not this module's.
pub fn get_pred_struct_for_frame(
    pic: &mut PicParams,
    map: &mut MiniGopMap,
    mini_gop_index: usize,
    seq_pred_structure: PredStructure,
    config_hierarchical_levels: u8,
    startup_mg_size: u32,
    idr_flag: bool,
    cra_flag: bool,
) {
    pic.pred_struct_type = seq_pred_structure;
    pic.hierarchical_levels = if idr_flag {
        config_hierarchical_levels
    } else {
        map.hierarchical_levels[mini_gop_index] as u8
    };

    if startup_mg_size != 0 {
        if idr_flag || cra_flag {
            map.enable_startup_mg = true;
        } else if map.enable_startup_mg {
            map.enable_startup_mg = false;
        }
    }
    if idr_flag && pic.picture_number == 0 {
        map.is_startup_gop = true;
    } else if idr_flag || cra_flag {
        map.is_startup_gop = false;
    }
}

/// C `is_pic_cutting_short_ra_mg` (`pd_process.c:928-941`) — EXPORTED.
///
/// Detects a random-access mini-GOP cut short by an intra picture, which
/// switches the picture to the LOW_DELAY prediction structure mid-stream.
#[must_use]
pub fn is_pic_cutting_short_ra_mg(
    map: &MiniGopMap,
    pic: &PicParams,
    mg_idx: usize,
    idr_flag: bool,
    cra_flag: bool,
) -> bool {
    (map.length[mg_idx] < pic.pred_struct_entry_count || map.idr_count[mg_idx] > 0)
        && pic.pred_struct_type == PredStructure::RandomAccess
        && !idr_flag
        && !cra_flag
}

/// C `svt_aom_is_delayed_intra` (`pd_process.c:3620-3635`) — EXPORTED.
///
/// Whether an intra picture is held back to join the next mini-GOP. Selects
/// `tf_params_per_type[0]` in `copy_tf_params` and gates the delayed-intra
/// handling in the picture-decision sequence.
#[must_use]
pub fn is_delayed_intra(
    idr_flag: bool,
    cra_flag: bool,
    pred_structure: PredStructure,
    intra_period_length: i32,
    end_of_sequence_flag: bool,
    pre_assignment_buffer_count: u32,
    pred_struct_entry_count: u32,
) -> bool {
    if (idr_flag || cra_flag) && pred_structure == PredStructure::RandomAccess {
        if intra_period_length == 0 || end_of_sequence_flag {
            false
        } else {
            idr_flag || (cra_flag && pre_assignment_buffer_count < pred_struct_entry_count)
        }
    } else {
        false
    }
}

/// C `search_this_pic` (`pd_process.c:3606-3619`) — EXPORTED.
///
/// Locates a picture by POC in a picture buffer; returns -1 when absent. This
/// is the lookup `derive_tf_window_params` uses to assemble its window.
#[must_use]
pub fn search_this_pic(buf: &[u64], input_pic: u64) -> i32 {
    for (i, &poc) in buf.iter().enumerate() {
        if poc == input_pic {
            return i as i32;
        }
    }
    -1
}

/// C `avail_past_pictures` (`pd_process.c:3592-3605`) — static.
///
/// Counts how many pictures in the buffer precede `input_pic`, which caps the
/// temporal-filter window size at the start of a sequence.
#[must_use]
pub fn avail_past_pictures(buf: &[u64], input_pic: u64) -> i32 {
    buf.iter().filter(|&&poc| poc < input_pic).count() as i32
}

/// C `perform_sc_detection`'s INHERITANCE half (`pd_process.c:4769-4813`) —
/// static.
///
/// Inter frames do NOT run screen-content detection: they inherit
/// `sc_class0..5` and `is_luma_dominant_input` from the last I picture. An I
/// picture runs detection (single-threaded mode only) and then publishes its
/// classes into the context.
///
/// Only the inheritance and publication are ported here; the detection calls
/// themselves (`svt_aom_is_screen_content`,
/// `svt_aom_is_screen_content_antialiasing_aware`,
/// `svt_aom_is_input_luma_dominant`) live in the screen-content module. The
/// caller passes the freshly detected classes for an I picture.
///
/// This matters because without it a port re-detects per frame and flips
/// palette / IntraBC / SC-tuned thresholds mid-GOP.
pub fn perform_sc_detection(
    is_i_slice: bool,
    detected: ScClasses,
    last_i: &mut ScClasses,
) -> ScClasses {
    if is_i_slice {
        *last_i = detected;
        detected
    } else {
        *last_i
    }
}

/// The screen-content classification a picture carries
/// (`pcs->sc_class0..5` + `pcs->is_luma_dominant_input`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScClasses {
    /// C `pcs->sc_class0..5`.
    pub class: [u8; 6],
    /// C `pcs->is_luma_dominant_input`.
    pub is_luma_dominant_input: bool,
}

/// C `store_mg_picture_arrays` (`pd_process.c:4966-4985`) — static.
///
/// Sorts the mini-GOP into DECODE order and keeps a display-order copy. It
/// literally determines the order frames are coded into the bitstream in
/// random access.
///
/// Trap: C's inner swap writes `mg_pics[i] = ctx->mg_pictures_array[j]` — the
/// SAME array `mg_pics` aliases, so it is an ordinary selection sort and not,
/// as the two spellings suggest, a copy from a second array. Reproduced as a
/// stable-by-construction selection sort on `decode_order`.
///
/// `decode_orders[k]` is the `decode_order` of the k-th picture **in display
/// order** (C's incoming `ctx->mg_pictures_array`). Returns
/// `(decode_order_permutation, display_order_permutation)` as index lists into
/// that same input.
#[must_use]
pub fn store_mg_picture_arrays(
    decode_orders: &[u64],
) -> (alloc::vec::Vec<usize>, alloc::vec::Vec<usize>) {
    let n = decode_orders.len();
    let display: alloc::vec::Vec<usize> = (0..n).collect();
    let mut decode: alloc::vec::Vec<usize> = (0..n).collect();
    for i in 0..n.saturating_sub(1) {
        for j in (i + 1)..n {
            if decode_orders[decode[j]] < decode_orders[decode[i]] {
                decode.swap(i, j);
            }
        }
    }
    (decode, display)
}

/// C `get_pic_idx_in_mg` (`pd_process.c:4872-4893`) — static.
///
/// Produces the RPS branch selector (`pic_idx_in_mg`) and, in low delay, also
/// writes `pcs->frame_offset` — the `set_frame_update_type` selector. Two
/// distinct downstream decisions ride on this one call.
///
/// Trap: in low delay `pic_idx_in_mg` is `(pred_struct_position - 1) %
/// entry_count`, with a special case for position 0 — NOT the position
/// itself. `frame_offset` is the distance to the last IDR, which is a
/// different quantity from `pic_idx_in_mg` and is written even when the
/// S-frame branch overrides the index.
///
/// The `IS_SFRAME_FLEXIBLE_INSERT` override is not ported (S-frames are
/// outside the port's envelope) and is named here rather than dropped.
pub fn get_pic_idx_in_mg(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    enc: &EncCtxPicParams,
    map: &MiniGopMap,
    pic_idx: u32,
    mini_gop_index: usize,
) -> u32 {
    match seq.pred_structure {
        PredStructure::RandomAccess => pic_idx - map.start_index[mini_gop_index],
        PredStructure::LowDelay => {
            let mg_pos = u64::from(enc.pred_struct_position);
            let idx = if mg_pos == 0 {
                0
            } else {
                ((mg_pos - 1) % u64::from(pic.pred_struct_entry_count)) as u32
            };
            pic.frame_offset = pic.picture_number - enc.last_idr_picture;
            idx
        }
        PredStructure::AllIntra => 0,
    }
}

/// C `update_pred_struct_and_pic_type` (`pd_process.c:4814-4871`) — static.
///
/// Walks `enc_ctx->pred_struct_position` and picks the slice type. A wrong
/// position means the wrong prediction-structure entry, i.e. the wrong
/// temporal layer for the frame.
///
/// Returns the derived [`SliceType`]. Sets `pic.pred_struct_type` to
/// `LOW_DELAY` and `map.cut_short_ra_mg` when the mini-GOP is cut short (C
/// re-fetches the LOW_DELAY prediction structure at that point).
///
/// Trap: the position rules are an if/else CHAIN with a specific priority —
/// mini-GOP switch, then IDR, then CRA-with-short-MG, then
/// "directly after a CRA" (`elapsed_non_cra_count == 0`, which sets
/// `init_pic_index + 1`, not `init_pic_index`), and only then the ordinary
/// increment. Reordering any two arms changes the position on a real frame.
#[allow(clippy::too_many_arguments)]
pub fn update_pred_struct_and_pic_type(
    pic: &mut PicParams,
    enc: &mut EncCtxPicParams,
    map: &mut MiniGopMap,
    ctx: &mut PicDecisionCtx,
    mini_gop_index: usize,
    pre_assignment_buffer_first_pass_flag: bool,
    idr_flag: bool,
    cra_flag: bool,
    init_pred_struct_position_flag: bool,
    init_pic_index: u32,
) -> SliceType {
    let picture_type;
    if is_pic_cutting_short_ra_mg(map, pic, mini_gop_index, idr_flag, cra_flag) {
        // Correct the pred index before switching structures.
        if pre_assignment_buffer_first_pass_flag {
            enc.pred_struct_position -= init_pic_index;
        }
        pic.pred_struct_type = PredStructure::LowDelay;
        picture_type = SliceType::B;
        ctx.cut_short_ra_mg = 1;
    } else {
        picture_type = if idr_flag || cra_flag {
            SliceType::I
        } else {
            SliceType::B
        };
    }

    if init_pred_struct_position_flag {
        enc.pred_struct_position = init_pic_index;
    }

    // The first two arms assign the same value in C too; they are kept
    // separate (rather than or-ed) because their GUARDS differ and the chain's
    // priority is the load-bearing part.
    #[allow(clippy::if_same_then_else)]
    if idr_flag {
        enc.pred_struct_position = init_pic_index;
    } else if cra_flag && map.length[mini_gop_index] < pic.pred_struct_entry_count {
        enc.pred_struct_position = init_pic_index;
    } else if enc.elapsed_non_cra_count == 0 {
        // Directly after a CRA: skip the entry that would violate it.
        enc.pred_struct_position = init_pic_index + 1;
    } else {
        enc.pred_struct_position += 1;
    }

    if idr_flag {
        enc.last_idr_picture = pic.picture_number;
    }

    if enc.pred_struct_position == pic.pred_struct_entry_count {
        enc.pred_struct_position -= pic.pred_struct_entry_count;
    }
    picture_type
}

// ---------------------------------------------------------------------------
// TPL group selection — Codec/initial_rc_process.c:161-526
// ---------------------------------------------------------------------------
//
// Measured reachability (`enc_handle.c:3657-3668`): `get_tpl` returns 0 for
// LOW_DELAY, for allintra and for `aq_mode == 0`, so everything in this
// section is DEAD for the campaign's first cell and LIVE for the default
// random-access video config. TPL sets `r0` and the per-SB qindex offsets in
// random access, so no RA frame is byte-identical without it — the port keeps
// it translated per `docs/WORKING-ON-THIS.md` §7 rather than dropping it
// because the first cell does not reach it.

/// C `TplControls` (`pcs.h:459-486`) — the subset `svt_aom_set_tpl_group` and
/// `set_tpl_params` write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TplControls {
    /// C `enable` — 0: TPL off.
    pub enable: u8,
    /// C `compute_rate`.
    pub compute_rate: u8,
    /// C `enable_tpl_qps`.
    pub enable_tpl_qps: u8,
    /// C `disable_intra_pred_nref`.
    pub disable_intra_pred_nref: u8,
    /// C `intra_mode_end` (`PredictionMode`; `DC_PRED` = 0, `PAETH_PRED` = 12).
    pub intra_mode_end: u8,
    /// C `pf_shape` (`TxCoeffShape`; `DEFAULT_SHAPE` 0, `N2_SHAPE` 1, `N4_SHAPE` 2).
    pub pf_shape: u8,
    /// C `use_sad_in_src_search`.
    pub use_sad_in_src_search: u8,
    /// C `reduced_tpl_group` — the temporal-layer cutoff, -1 for "all".
    pub reduced_tpl_group: i8,
    /// C `r0_adjust_factor`.
    pub r0_adjust_factor: f64,
    /// C `dispenser_search_level`.
    pub dispenser_search_level: u8,
    /// C `subsample_tx`.
    pub subsample_tx: u8,
    /// C `synth_blk_size`.
    pub synth_blk_size: u8,
    /// C `subpel_depth` (`SUBPEL_FORCE_STOP`; `EIGHTH_PEL` 0, `QUARTER_PEL` 1,
    /// `HALF_PEL` 2, `FULL_PEL` 3).
    pub subpel_depth: u8,
    /// C `subpel_diag_refinement`.
    pub subpel_diag_refinement: u8,
}

impl Default for TplControls {
    /// C initialises the struct with `TplControls tpl_ctrls_struct = {0}`,
    /// so every unwritten field is zero — including `reduced_tpl_group`, whose
    /// zero means "temporal layer 0 only", NOT "all frames" (-1). The level-0
    /// arm writes only `enable`, so a level-0 `TplControls` really does carry
    /// `reduced_tpl_group == 0`.
    fn default() -> Self {
        Self {
            enable: 0,
            compute_rate: 0,
            enable_tpl_qps: 0,
            disable_intra_pred_nref: 0,
            intra_mode_end: 0,
            pf_shape: 0,
            use_sad_in_src_search: 0,
            reduced_tpl_group: 0,
            r0_adjust_factor: 0.0,
            dispenser_search_level: 0,
            subsample_tx: 0,
            synth_blk_size: 0,
            subpel_depth: 0,
            subpel_diag_refinement: 0,
        }
    }
}

/// C `DC_PRED`.
pub const DC_PRED: u8 = 0;
/// C `PAETH_PRED`.
pub const PAETH_PRED: u8 = 12;
/// C `DEFAULT_SHAPE` / `N2_SHAPE` / `N4_SHAPE` (`definitions.h:2062-2064`).
pub const DEFAULT_SHAPE: u8 = 0;
/// See [`DEFAULT_SHAPE`].
pub const N2_SHAPE: u8 = 1;
/// See [`DEFAULT_SHAPE`].
pub const N4_SHAPE: u8 = 2;
/// C `SUBPEL_FORCE_STOP` (`definitions.h:868`).
pub const EIGHTH_PEL: u8 = 0;
/// See [`EIGHTH_PEL`].
pub const QUARTER_PEL: u8 = 1;
/// See [`EIGHTH_PEL`].
pub const HALF_PEL: u8 = 2;
/// See [`EIGHTH_PEL`].
pub const FULL_PEL: u8 = 3;
/// C `INPUT_SIZE_480p_RANGE` (`definitions.h:1826`).
pub const INPUT_SIZE_480P_RANGE: u8 = 2;

/// The sequence/picture inputs `svt_aom_set_tpl_group` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TplPicParams {
    /// C `pcs->slice_type` — used as a C truthiness test (`I_SLICE` == 1 is
    /// TRUE, `B_SLICE` == 0 is FALSE), which reads backwards from its name.
    pub slice_type: SliceType,
    /// C `pcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// C `pcs->scs->input_resolution` (`ResolutionRange`).
    pub input_resolution: u8,
    /// C `pcs->scs->tpl_lad_mg`.
    pub tpl_lad_mg: u8,
    /// C `pcs->scs->static_config.rate_control_mode`.
    pub rate_control_mode: RcMode,
}

/// C `svt_aom_get_tpl_group_level` (`initial_rc_process.c:190-202`) — EXPORTED.
///
/// Maps `(scs->tpl, enc_mode)` to the TPL group level.
#[must_use]
pub fn get_tpl_group_level(tpl: u8, enc_mode: i8) -> u8 {
    const ENC_M5: i8 = 5;
    const ENC_M8: i8 = 8;
    if tpl == 0 {
        0
    } else if enc_mode <= ENC_M5 {
        1
    } else if enc_mode <= ENC_M8 {
        3
    } else {
        4
    }
}

/// C `svt_aom_set_tpl_group` (`initial_rc_process.c:204-306`) — EXPORTED.
///
/// Fills [`TplControls`] `enable` / `reduced_tpl_group` / `synth_blk_size` and
/// the `r0_adjust_factor`. Returns `(controls, synth_blk_size)`.
///
/// `pic` is `None` for C's `pcs == NULL` probe call, which returns only the
/// synthesizer block size and writes nothing back.
///
/// **The `slice_type` trap.** C writes `pcs->slice_type ? A : B`. `SliceType`
/// is `B_SLICE = 0, I_SLICE = 1`, so the TRUE arm is the **I slice** and the
/// FALSE arm is the inter slice — the opposite of what "slice_type ?" reads
/// like. Every conditional below preserves that orientation explicitly.
///
/// # Panics
///
/// Panics on an unknown `tpl_group_level` (C asserts there).
#[must_use]
pub fn set_tpl_group(
    pic: Option<&TplPicParams>,
    tpl_group_level: u8,
    source_width: u32,
    source_height: u32,
) -> (TplControls, u8) {
    let mut t = TplControls::default();
    let is_i = |p: &TplPicParams| p.slice_type == SliceType::I;
    let small = |p: &TplPicParams| p.input_resolution <= INPUT_SIZE_480P_RANGE;

    match tpl_group_level {
        0 => t.enable = 0,
        1 => {
            t.enable = 1;
            t.reduced_tpl_group = -1;
            t.synth_blk_size = 16;
        }
        2 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) if is_i(p) => -1,
                Some(p) => {
                    if p.hierarchical_levels == 5 {
                        4
                    } else {
                        3
                    }
                }
            };
            t.synth_blk_size = 16;
        }
        3 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) => {
                    if p.hierarchical_levels == 5 {
                        4
                    } else {
                        3
                    }
                }
            };
            t.synth_blk_size = 16;
        }
        4 => {
            t.enable = 1;
            t.reduced_tpl_group = match pic {
                None => -1,
                Some(p) => match p.hierarchical_levels {
                    5 => {
                        if is_i(p) {
                            2
                        } else if small(p) {
                            3
                        } else {
                            1
                        }
                    }
                    // C's hierarchical_levels == 4 arm really does yield 2
                    // for BOTH the I-slice and the <= 480p inter case; the two
                    // branches are kept apart because the C source spells them
                    // out separately and they diverge at every other level.
                    #[allow(clippy::if_same_then_else)]
                    4 => {
                        if is_i(p) {
                            2
                        } else if small(p) {
                            2
                        } else {
                            1
                        }
                    }
                    _ => {
                        if is_i(p) {
                            3
                        } else if small(p) {
                            2
                        } else {
                            0
                        }
                    }
                },
            };
            t.synth_blk_size = if source_width.min(source_height) >= 720 {
                32
            } else {
                16
            };
        }
        _ => panic!("svt_aom_set_tpl_group: unknown tpl_group_level {tpl_group_level}"),
    }

    let Some(p) = pic else {
        return (t, t.synth_blk_size);
    };

    if i32::from(p.hierarchical_levels) <= i32::from(t.reduced_tpl_group) {
        t.reduced_tpl_group = -1;
    }

    if t.reduced_tpl_group >= 0 {
        // The r0 compensation for TPL not using every available frame.
        t.r0_adjust_factor = match i32::from(p.hierarchical_levels) - i32::from(t.reduced_tpl_group)
        {
            1 => {
                if p.hierarchical_levels <= 2 {
                    0.4
                } else if p.hierarchical_levels <= 3 {
                    0.8
                } else {
                    1.6
                }
            }
            2 => {
                if p.hierarchical_levels <= 2 {
                    0.6
                } else if p.hierarchical_levels <= 3 {
                    1.2
                } else {
                    2.4
                }
            }
            3 => {
                if p.hierarchical_levels <= 3 {
                    1.4
                } else {
                    2.8
                }
            }
            4 => 4.0,
            5 => 6.0,
            // C's `case 0: default:` share an arm, so a NEGATIVE difference
            // lands here too.
            _ => 0.0,
        };
        if p.tpl_lad_mg == 0 {
            t.r0_adjust_factor *= 1.25;
        }
    } else {
        t.r0_adjust_factor = 0.0;
        if p.tpl_lad_mg == 0 {
            t.r0_adjust_factor = if is_i(p) {
                0.0
            } else if p.hierarchical_levels <= 2 {
                0.4
            } else if p.hierarchical_levels <= 3 {
                0.8
            } else {
                1.6
            };
        }
    }
    if p.rate_control_mode == RcMode::Vbr {
        t.r0_adjust_factor *= 1.25;
        t.r0_adjust_factor = t.r0_adjust_factor.min(3.0);
    }
    (t, t.synth_blk_size)
}

/// C `get_tpl_params_level` (`initial_rc_process.c:307-318`) — static.
#[must_use]
pub fn get_tpl_params_level(enc_mode: i8) -> u8 {
    const ENC_M2: i8 = 2;
    const ENC_M7: i8 = 7;
    if enc_mode <= ENC_M2 {
        1
    } else if enc_mode <= ENC_M7 {
        4
    } else {
        5
    }
}

/// C `set_tpl_params` (`initial_rc_process.c:319-405`) — static.
///
/// Sets what TPL computes and therefore what qindex each SB gets. Note that
/// this MUTATES an existing [`TplControls`] (C writes through
/// `&pcs->tpl_ctrls`) rather than starting from zero, so `enable`,
/// `reduced_tpl_group`, `r0_adjust_factor` and `synth_blk_size` — all set by
/// [`set_tpl_group`] — survive.
///
/// # Panics
///
/// Panics on an unknown `tpl_level` (C asserts there).
pub fn set_tpl_params(t: &mut TplControls, tpl_level: u8, input_resolution: u8) {
    let small_shape = if input_resolution <= INPUT_SIZE_480P_RANGE {
        N2_SHAPE
    } else {
        N4_SHAPE
    };
    match tpl_level {
        0 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 0;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = DEFAULT_SHAPE;
            t.use_sad_in_src_search = 0;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 0;
        }
        1 => {
            t.compute_rate = 1;
            t.enable_tpl_qps = 1;
            t.disable_intra_pred_nref = 0;
            t.intra_mode_end = PAETH_PRED;
            t.pf_shape = DEFAULT_SHAPE;
            t.use_sad_in_src_search = 0;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 0;
        }
        2 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = PAETH_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 0;
        }
        3 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = QUARTER_PEL;
            t.subpel_diag_refinement = 4;
        }
        4 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 0;
            t.subsample_tx = 0;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 4;
        }
        5 => {
            t.compute_rate = 0;
            t.enable_tpl_qps = 0;
            t.disable_intra_pred_nref = 1;
            t.intra_mode_end = DC_PRED;
            t.pf_shape = small_shape;
            t.use_sad_in_src_search = 1;
            t.dispenser_search_level = 1;
            t.subsample_tx = 2;
            t.subpel_depth = FULL_PEL;
            t.subpel_diag_refinement = 4;
        }
        _ => panic!("set_tpl_params: unknown tpl_level {tpl_level}"),
    }
}

/// C `is_frame_already_exists` (`initial_rc_process.c:161-170`) — static.
///
/// De-duplication inside the TPL group build. Omitting it double-counts a
/// picture in the propagation.
#[must_use]
pub fn is_frame_already_exists(tpl_group_pocs: &[u64], end_index: usize, pic_num: u64) -> bool {
    tpl_group_pocs[..end_index].contains(&pic_num)
}

/// C `validate_pic_for_tpl` (`initial_rc_process.c:171-189`) — EXPORTED.
///
/// Admission test for a picture into the TPL group. Returns whether the
/// picture is valid; the caller increments `used_tpl_frame_num` when it is.
///
/// Trap: the `reduced_tpl_group` test is `temporal_layer_index <=
/// reduced_tpl_group`, and it applies only when `reduced_tpl_group >= 0`. A
/// value of 0 means "base layer only", NOT "no reduction" — that is -1.
#[must_use]
pub fn validate_pic_for_tpl(
    tpl_group_pocs: &[u64],
    tpl_group_layers: &[u8],
    pic_index: usize,
    reduced_tpl_group: i8,
    is_pic_skipped: bool,
) -> bool {
    if is_frame_already_exists(tpl_group_pocs, pic_index, tpl_group_pocs[pic_index])
        || is_pic_skipped
    {
        return false;
    }
    if reduced_tpl_group >= 0 {
        i32::from(tpl_group_layers[pic_index]) <= i32::from(reduced_tpl_group)
    } else {
        true
    }
}

/// One member of the extended lookahead group, as
/// `store_extended_group` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtGroupPic {
    /// C `pcs->picture_number`.
    pub picture_number: u64,
    /// C `pcs->slice_type`.
    pub slice_type: SliceType,
    /// C `pcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `pcs->ext_mg_id`.
    pub ext_mg_id: i64,
    /// C `svt_aom_is_delayed_intra(pcs)`, precomputed by the caller.
    pub is_delayed_intra: bool,
    /// C `svt_aom_is_pic_skipped(pcs)`, precomputed by the caller.
    pub is_skipped: bool,
}

/// The TPL group `store_extended_group` produces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TplGroup {
    /// Indices into the extended group, in order — C's `pcs->tpl_group[]`.
    pub members: alloc::vec::Vec<usize>,
    /// C `pcs->tpl_valid_pic[]`, indexed like the EXTENDED group (not like
    /// `members`) — C writes `tpl_valid_pic[i]` where `i` is the ext-group
    /// index, which is the same as the member index only while nothing is
    /// skipped.
    pub valid: alloc::vec::Vec<u8>,
    /// C `pcs->used_tpl_frame_num`.
    pub used_tpl_frame_num: u32,
}

/// C `store_extended_group`'s GROUP-SELECTION half
/// (`initial_rc_process.c:439-497`) — EXPORTED symbol, ported at tier 4.
///
/// Selects TPL group MEMBERSHIP: different membership is a different `r0` and
/// a different qindex on every SB.
///
/// The first half of the C function (walking `ctx->lad_queue` to build
/// `pcs->ext_group`) is NOT ported — it is queue plumbing over a circular
/// buffer of in-flight PCS objects, which this port replaces by design. The
/// caller passes the extended group directly.
///
/// Traps, all transcribed literally:
/// * `tpl_valid_pic[0]` is forced to 1 BEFORE the loop, so picture 0 is
///   admitted even if `validate_pic_for_tpl` would reject it — but
///   `used_tpl_frame_num` is NOT incremented for it unless validation passes.
/// * `limited_tpl_group_size` counts `1 + (tpl_lad_mg + 1) * mg_size` for an I
///   slice and `(tpl_lad_mg + 1) * mg_size` otherwise, then clamps to the
///   extended group size.
/// * A non-delayed intra at `i != 0` is ADDED and then closes the GOP
///   (`is_gop_end = 1`); a DELAYED intra at `i != 0` breaks immediately
///   WITHOUT being added. The two arms look symmetric and are not.
/// * After `is_gop_end`, only pictures with the SAME `ext_mg_id` as that intra
///   continue; the first one with a different id breaks.
#[must_use]
pub fn store_extended_group(
    ext_group: &[ExtGroupPic],
    slice_type: SliceType,
    hierarchical_levels: u8,
    tpl_lad_mg: u32,
    reduced_tpl_group: i8,
) -> TplGroup {
    let mut g = TplGroup {
        members: alloc::vec::Vec::new(),
        valid: alloc::vec![0u8; ext_group.len()],
        used_tpl_frame_num: 0,
    };
    if ext_group.is_empty() {
        return g;
    }
    g.valid[0] = 1;

    let mg_size = 1u32 << hierarchical_levels;
    let limited = if slice_type == SliceType::I {
        (1 + (tpl_lad_mg + 1) * mg_size).min(ext_group.len() as u32)
    } else {
        ((tpl_lad_mg + 1) * mg_size).min(ext_group.len() as u32)
    } as usize;

    let mut is_gop_end = false;
    let mut last_intra_mg_id: i64 = 0;
    // Group POCs/layers accumulated so far, for the de-duplication test.
    let mut pocs: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let mut layers: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    let admit = |g: &mut TplGroup,
                 pocs: &mut alloc::vec::Vec<u64>,
                 layers: &mut alloc::vec::Vec<u8>,
                 i: usize| {
        let cur = ext_group[i];
        g.members.push(i);
        // C's validate_pic_for_tpl indexes pcs->tpl_group[pic_index] with the
        // EXT-group index, and by construction the picture just appended sits
        // at that index while nothing has been skipped.
        pocs.push(cur.picture_number);
        layers.push(cur.temporal_layer_index);
        let at = pocs.len() - 1;
        if validate_pic_for_tpl(pocs, layers, at, reduced_tpl_group, cur.is_skipped) {
            g.valid[i] = 1;
            g.used_tpl_frame_num += 1;
        }
    };

    // The two `admit` arms after `is_gop_end` are identical in body and
    // different in guard; C spells them out separately and the guards are the
    // whole point, so they are not merged.
    #[allow(clippy::if_same_then_else)]
    for i in 0..limited {
        let cur = ext_group[i];
        if cur.slice_type == SliceType::I {
            if cur.is_delayed_intra {
                if i == 0 {
                    admit(&mut g, &mut pocs, &mut layers, i);
                } else {
                    break;
                }
            } else if i == 0 {
                admit(&mut g, &mut pocs, &mut layers, i);
            } else {
                admit(&mut g, &mut pocs, &mut layers, i);
                last_intra_mg_id = cur.ext_mg_id;
                is_gop_end = true;
            }
        } else if !is_gop_end {
            admit(&mut g, &mut pocs, &mut layers, i);
        } else if cur.ext_mg_id == last_intra_mg_id {
            admit(&mut g, &mut pocs, &mut layers, i);
        } else {
            break;
        }
    }
    g
}

// ---------------------------------------------------------------------------
// Reference binding + primary_ref_frame — Codec/pic_manager_process.c
// ---------------------------------------------------------------------------

/// C `PRIMARY_REF_NONE` (`definitions.h:1470`).
pub const PRIMARY_REF_NONE: u8 = 7;
/// C `REFRESH_FRAME_CONTEXT_BACKWARD`.
pub const REFRESH_FRAME_CONTEXT_BACKWARD: u8 = 1;

/// C `get_list_idx` (`inter_prediction.h:531-535`) via `ref_type_to_list_idx`.
#[must_use]
pub fn get_list_idx(ref_type: u8) -> u8 {
    const REF_TYPE_TO_LIST_IDX: [u8; 8] = [0, 0, 0, 0, 0, 1, 1, 1];
    REF_TYPE_TO_LIST_IDX[ref_type as usize]
}

/// C `get_ref_frame_idx` (`inter_prediction.h:537-541`) via `ref_type_to_ref_idx`.
#[must_use]
pub fn get_ref_frame_idx(ref_type: u8) -> u8 {
    const REF_TYPE_TO_REF_IDX: [u8; 8] = [0, 0, 1, 2, 3, 0, 1, 2];
    REF_TYPE_TO_REF_IDX[ref_type as usize]
}

/// One entry of C's `enc_ctx->ref_pic_list` — a reference the picture manager
/// can bind to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefQueueEntry {
    /// C `ref_pic_entry->picture_number`.
    pub picture_number: u64,
    /// C `ref_pic_entry->is_valid`.
    pub is_valid: bool,
    /// C `ref_pic_entry->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// C `ref_obj->base_q_idx`.
    pub base_q_idx: u8,
    /// C `ref_obj->slice_type`.
    pub slice_type: SliceType,
    /// C `ref_obj->r0`.
    pub r0: f64,
}

/// C `search_ref_in_ref_queue` (`pic_manager_process.c:178-188`) — EXPORTED.
///
/// Resolves a reference POC to its reference-queue entry, skipping invalid
/// slots. Returns the index, or `None`.
///
/// Trap: the C loop assigns `ref_pic_entry` on every iteration and returns
/// only on a match, so a caller that reads the variable after a miss sees the
/// LAST scanned entry — but the function itself returns NULL, which is what
/// this reproduces.
#[must_use]
pub fn search_ref_in_ref_queue(ref_pic_list: &[RefQueueEntry], ref_poc: u64) -> Option<usize> {
    ref_pic_list
        .iter()
        .position(|e| e.is_valid && e.picture_number == ref_poc)
}

/// What the picture manager binds onto a child PCS for one picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefBinding {
    /// C `child_pcs->ref_base_q_idx[list][idx]`.
    pub ref_base_q_idx: [[u8; 4]; 2],
    /// C `child_pcs->ref_slice_type[list][idx]`.
    pub ref_slice_type: [[SliceType; 4]; 2],
    /// C `child_pcs->ref_pic_r0[list][idx]`.
    pub ref_pic_r0: [[f64; 4]; 2],
    /// C `child_pcs->ppcs->frm_hdr.primary_ref_frame` — a written header field.
    pub primary_ref_frame: u8,
    /// C `child_pcs->ppcs->refresh_frame_context`.
    pub refresh_frame_context: u8,
    /// The queue index each reference resolved to, indexed by
    /// [`LAST`]..=[`ALT`]; `None` where the reference was not in the queue.
    pub resolved: [Option<usize>; INTER_REFS_PER_FRAME],
}

impl Default for RefBinding {
    fn default() -> Self {
        Self {
            ref_base_q_idx: [[0; 4]; 2],
            ref_slice_type: [[SliceType::B; 4]; 2],
            ref_pic_r0: [[0.0; 4]; 2],
            primary_ref_frame: PRIMARY_REF_NONE,
            refresh_frame_context: REFRESH_FRAME_CONTEXT_BACKWARD,
            resolved: [None; INTER_REFS_PER_FRAME],
        }
    }
}

/// C `svt_aom_picture_manager_kernel_iter`'s EB_PIC_INPUT reference-binding
/// block (`pic_manager_process.c:798-874`) — this is inter campaign chunk
/// C1b's deliverable.
///
/// Derives `frm_hdr.primary_ref_frame` and `refresh_frame_context`, and binds
/// `ref_base_q_idx` / `ref_slice_type` / `ref_pic_r0` per reference. CDF
/// continuation off the wrong primary ref desynchronises the entropy decoder,
/// so this is a hard bitstream field, not a heuristic.
///
/// **EVIDENCE TIER 4, and the reason is worth stating.**
/// `svt_aom_picture_manager_kernel_iter` IS an exported symbol (`nm -g` on
/// `Bin/Release/libSvtAv1Enc.a` finds it) but is NOT callable in isolation:
/// the first thing it does is `EB_GET_FULL_OBJECT` on a fifo, so a shim would
/// block rather than return. "A symbol exists" and "tier 1 is reachable" are
/// different facts here. The block is therefore gated by hand-derived vectors
/// traced against the C source, and a byte-identity gate on the inter frame
/// header (tier 2) is the upgrade path.
///
/// **The primary-ref rule, stated precisely because it is easy to get wrong:**
/// walk LAST..ALT in order and keep the reference with the LARGEST
/// `temporal_layer_index` that is still `<= this picture's` temporal layer.
/// Ties keep the FIRST such reference (the comparison is strict `<`), so LAST
/// wins over a later reference at the same layer. The result is stored as a
/// `REF_FRAME_MINUS1` (0..6), NOT as an `MvReferenceFrame` — C asserts
/// `ref_index == (int)ref` on exactly that point.
///
/// Not ported, and named rather than dropped: the `ref_global_motion[]` copy
/// inside the same guard (it needs the reference object's warp params, which
/// live in the global-motion module) and the `ref_pic_ptr_array` /
/// live-count bookkeeping, which is buffer plumbing this port replaces.
///
/// # Panics
///
/// Panics if a B-slice reference POC is absent from the queue. C asserts and
/// raises `EB_ENC_PM_ERROR10` there; a missing reference means the DPB model
/// and the queue have already diverged, and continuing would bind a wrong
/// picture.
#[must_use]
pub fn bind_refs_and_primary_ref_frame(
    pic: &PicParams,
    ref_pic_list: &[RefQueueEntry],
    frame_end_cdf_update_mode: bool,
    is_s_frame: bool,
) -> RefBinding {
    let mut b = RefBinding::default();
    let mut ref_index: i8 = 0;

    if pic.slice_type == SliceType::B {
        let mut max_temporal_index: i8 = -1;
        for r in LAST..=ALT {
            // The overlay frame hardcodes its own POC as the reference.
            let ref_poc = if pic.is_overlay {
                pic.picture_number
            } else {
                pic.rps.ref_poc_array[r]
            };
            let ref_type = (r as u8) + 1;
            let list_idx = get_list_idx(ref_type) as usize;
            let ref_idx = get_ref_frame_idx(ref_type) as usize;

            let found = search_ref_in_ref_queue(ref_pic_list, ref_poc).unwrap_or_else(|| {
                panic!(
                    "picture manager: reference POC {ref_poc} for ref {r} of picture \
                     {} is not in the reference queue (C asserts + EB_ENC_PM_ERROR10)",
                    pic.picture_number
                )
            });
            b.resolved[r] = Some(found);
            let e = ref_pic_list[found];

            if frame_end_cdf_update_mode
                && max_temporal_index < e.temporal_layer_index as i8
                && e.temporal_layer_index <= pic.temporal_layer_index
            {
                max_temporal_index = e.temporal_layer_index as i8;
                // Stored as REF_FRAME_MINUS1, not as an MvReferenceFrame.
                ref_index = get_ref_frame_type(list_idx as u8, ref_idx as u8) - LAST_FRAME;
                debug_assert_eq!(ref_index as usize, r);
                // C also copies ref_global_motion[] here; see the doc comment.
            }

            b.ref_base_q_idx[list_idx][ref_idx] = e.base_q_idx;
            b.ref_slice_type[list_idx][ref_idx] = e.slice_type;
            b.ref_pic_r0[list_idx][ref_idx] = e.r0;
        }
    }

    if frame_end_cdf_update_mode {
        b.primary_ref_frame = if pic.slice_type != SliceType::I && !is_s_frame {
            ref_index as u8
        } else {
            PRIMARY_REF_NONE
        };
    } else {
        b.primary_ref_frame = PRIMARY_REF_NONE;
    }
    // C sets REFRESH_FRAME_CONTEXT_BACKWARD in BOTH arms; the comment there
    // says it is never disabled so the feature can be on in higher layers
    // while off in low ones.
    b.refresh_frame_context = REFRESH_FRAME_CONTEXT_BACKWARD;
    b
}

// ---------------------------------------------------------------------------
// send_picture_out's reference-count adjustment + temporal-filter params
// ---------------------------------------------------------------------------

/// C `INVALID_LUMA` (`definitions.h:90`).
///
/// It is **256**, not -1 and not 0. `avg_luma` is a `uint64_t` holding an
/// 8-bit mean, so 256 is the out-of-range sentinel; a port that guessed -1
/// would treat a genuinely invalid reference as valid and compare against
/// garbage. Read from the header, not inferred from the name.
pub const INVALID_LUMA: u64 = 256;

/// C `get_similar_ref_brightness` (`pd_process.c:4251-4267`) — EXPORTED.
///
/// Produces `pcs->similar_brightness_refs`, read by `motion_estimation.c:2231`
/// to take the safe-limit ME path. `avg_luma` of `INVALID_LUMA` on EITHER
/// reference disables the test entirely.
///
/// The luma means are `uint64_t` in C and the comparison casts BOTH sides to
/// `int` before subtracting, which is reproduced here.
#[must_use]
pub fn get_similar_ref_brightness(
    slice_type: SliceType,
    hierarchical_levels: u8,
    ref_list1_count_try: u8,
    ref0_avg_luma: u64,
    ref1_avg_luma: u64,
    cur_avg_luma: u64,
) -> bool {
    if slice_type == SliceType::B
        && hierarchical_levels > 0
        && ref_list1_count_try > 0
        && ref0_avg_luma != INVALID_LUMA
        && ref1_avg_luma != INVALID_LUMA
    {
        const LUMA_TH: i32 = 5;
        let cur = cur_avg_luma as i32;
        return (ref0_avg_luma as i32 - cur).abs() < LUMA_TH
            && (ref1_avg_luma as i32 - cur).abs() < LUMA_TH;
    }
    false
}

/// C `send_picture_out`'s reference-count adjustment block
/// (`pd_process.c:4276-4313`) — static. Only this block is in scope; the fifo
/// posting around it is buffer plumbing.
///
/// Two independent limiters, in C's order:
/// 1. the RTC early-HME prune, which drops LAST2 (flat) or LAST3
///    (hierarchical base) when it is at least `early_hme_l0_prune_th` percent
///    worse than LAST — and then RE-RUNS `set_all_ref_frame_type`, because the
///    candidate set is derived from the try counts;
/// 2. `safe_limit_nref == 2`, which caps both lists at 1 when the references
///    have similar brightness.
///
/// Trap, and C flags it itself with a TODO: limiter 2 lowers the try counts
/// AFTER `set_all_ref_frame_type` has already run, so `ref_frame_type_arr`
/// keeps candidates for references MD will not enumerate. That is reproduced,
/// not fixed — byte-identity means reproducing it (`WORKING-ON-THIS.md` §7).
///
/// `hme_dist` returns `(last_dist, other_dist)` for the pair the current
/// hierarchy compares; it is `None` when the prune does not apply.
pub fn send_picture_out_ref_counts(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    hme_dist: Option<(u64, u64)>,
    similar_brightness_refs: bool,
) {
    let mrp = &seq.mrp_ctrls;
    if seq.rtc && mrp.early_hme_l0_prune_th != 0 && pic.ref_list0_count_try > 1 {
        // `cap` is the count the prune drops to: 1 for the flat structure
        // (LAST2 gone) and 2 for the hierarchical base (LAST3 gone).
        let cap = if pic.hierarchical_levels == 0 {
            Some(1u8)
        } else if pic.temporal_layer_index == 0 && pic.ref_list0_count_try >= 3 {
            Some(2u8)
        } else {
            None
        };
        if let (Some(cap), Some((last_dist, other_dist))) = (cap, hme_dist)
            && other_dist * 100 >= last_dist * u64::from(mrp.early_hme_l0_prune_th)
        {
            pic.ref_list0_count_try = pic.ref_list0_count_try.min(cap);
            let (arr, tot) = set_all_ref_frame_type(pic);
            pic.ref_frame_type_arr = arr;
            pic.tot_ref_frame_types = tot;
        }
    }

    if mrp.safe_limit_nref == 2
        && pic.slice_type == SliceType::B
        && pic.hierarchical_levels > 0
        && pic.temporal_layer_index >= pic.hierarchical_levels - 1
        && similar_brightness_refs
    {
        // C's own TODO: these run AFTER set_all_ref_frame_type.
        pic.ref_list0_count_try = pic.ref_list0_count_try.min(1);
        pic.ref_list1_count_try = pic.ref_list1_count_try.min(1);
    }
}

/// Which entry of `scs->tf_params_per_type[]` a picture takes, or `Disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfParamsChoice {
    /// C `pcs->tf_ctrls.enabled = 0`.
    Disabled,
    /// C `scs->tf_params_per_type[0]` — the delayed-intra entry.
    DelayedIntra,
    /// C `scs->tf_params_per_type[1]` — BASE.
    Base,
    /// C `scs->tf_params_per_type[2]` — L1.
    L1,
}

/// C `copy_tf_params` (`pd_process.c:4468-4497`) — static.
///
/// The gate that decides whether temporal filtering runs at all, and which
/// parameter set it uses.
///
/// **MEASURED, and the negative result must be reproduced rather than
/// assumed:** in `LOW_DELAY` `tf_level` is forced to 0 before any preset logic
/// (`enc_handle.c:3339-3343`), so `tf_params_per_type[1]` is itself disabled
/// and the correct port yields TF OFF for every low-delay picture — including
/// the base-layer inter pictures this function nominally maps to entry 1.
/// This function returns [`TfParamsChoice::Base`] for those pictures because
/// that is what C selects; the disabling happens one level up, in the
/// parameter table.
///
/// The `IS_SFRAME_FLEXIBLE_INSERT` guard that can force `enabled = 0` when
/// `ctx->tf_pic_arr_cnt == 0` is not ported (S-frames are outside the
/// envelope) and is named here rather than dropped.
#[must_use]
pub fn copy_tf_params(
    seq_pred_structure: PredStructure,
    slice_type: SliceType,
    is_key_frame: bool,
    temporal_layer_index: u8,
    hierarchical_levels: u8,
    is_overlay: bool,
    enable_tf_key: bool,
    is_delayed_intra: bool,
) -> TfParamsChoice {
    if seq_pred_structure == PredStructure::LowDelay {
        return if slice_type != SliceType::I && temporal_layer_index == 0 {
            TfParamsChoice::Base
        } else {
            TfParamsChoice::Disabled
        };
    }
    // No TF for overlays, for a key frame with enable_tf_key off, or for the
    // highest layer (which matters at 2L).
    if (is_key_frame && !enable_tf_key) || is_overlay || temporal_layer_index == hierarchical_levels
    {
        TfParamsChoice::Disabled
    } else if is_delayed_intra {
        TfParamsChoice::DelayedIntra
    } else if temporal_layer_index == 0 {
        TfParamsChoice::Base
    } else if temporal_layer_index == 1 {
        TfParamsChoice::L1
    } else {
        TfParamsChoice::Disabled
    }
}

// ---------------------------------------------------------------------------
// Histogram-based scene detection — pd_process.c:55-84, 256-378, 4682-4719,
// 5192-5215
// ---------------------------------------------------------------------------
//
// Reachability, measured rather than inferred. `static_config.scene_change_detection`
// is force-zeroed with a warning (`enc_settings.c:839-843`), which makes it
// tempting to call the whole detector dead. It is NOT:
// `vq_ctrls.sharpness_ctrls.scene_transition` is set to 1 in BOTH arms of
// `derive_vq_params` (`enc_handle.c:3282, 3291`) and only zeroed for
// LOW_DELAY (`3324-3326`), so the transition path runs in random access and
// its output becomes `pcs->transition_present`. `scs->calc_hist` follows the
// same shape (`enc_handle.c:1353` makes it 1 whenever any TF type is
// enabled), so `calc_ahd_pd` — and hence `pcs->ahd_error`, read at
// `motion_estimation.c:1245` — is live there too.

/// C `HISTOGRAM_NUMBER_OF_BINS` (`pcs.h:39`).
pub const HISTOGRAM_NUMBER_OF_BINS: usize = 256;
/// C `MAX_NUMBER_OF_REGIONS_IN_WIDTH` (`pcs.h:40`).
pub const MAX_NUMBER_OF_REGIONS_IN_WIDTH: usize = 4;
/// C `MAX_NUMBER_OF_REGIONS_IN_HEIGHT` (`pcs.h:41`).
pub const MAX_NUMBER_OF_REGIONS_IN_HEIGHT: usize = 4;
/// C `FLASH_TH` (`pd_process.c:170`).
pub const FLASH_TH: u8 = 5;
/// C `FADE_TH` (`pd_process.c:171`).
pub const FADE_TH: u8 = 3;
/// C `SCENE_TH` (`pd_process.c:172`).
pub const SCENE_TH: u32 = 3000;

/// C `NUM64x64INPIC(w, h)` (`pd_process.c:173`).
///
/// The macro is `((w * h) >> (svt_log2f(BLOCK_SIZE_64) << 1))`, i.e.
/// `(w * h) >> 12` — `log2(64) == 6`, doubled is 12. Written out rather than
/// re-deriving the shift at each call site.
#[inline]
#[must_use]
pub fn num_64x64_in_pic(w: u32, h: u32) -> u32 {
    (w.wrapping_mul(h)) >> 12
}

/// A per-region luma histogram plane, as `pcs->picture_histogram` holds it.
pub type RegionHistograms = [[[u32; HISTOGRAM_NUMBER_OF_BINS]; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
    MAX_NUMBER_OF_REGIONS_IN_WIDTH];
/// `pcs->average_intensity_per_region` (`pcs.h:852`).
///
/// It is `uint64_t[4][4]`, NOT `uint8_t[4][4]` — the values are 8-bit luma
/// means but the storage is 64-bit, and `scene_transition_detector` narrows
/// each one with an explicit `(int16_t)` cast before subtracting. Modelling it
/// as `u8` would silently change what an out-of-range value does.
pub type RegionIntensities =
    [[u64; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH];

/// C `calc_ahd` (`pd_process.c:55-84`) — static.
///
/// Accumulated histogram difference between two pictures, plus a count of the
/// regions whose own difference exceeds their pixel count. Fills
/// `tf_ahd_error_to_central`, which `temporal_filtering.c:2879` uses to drop
/// dissimilar pictures from the filter window.
///
/// Returns `(ahd, active_region_cnt_increment)`. C takes `active_region_cnt`
/// as an in/out pointer and only ever INCREMENTS it, so the caller adds.
///
/// Note the region size: `ref_pcs->enhanced_pic->{width,height}` divided by
/// the region counts, with NO remainder handling — unlike
/// [`scene_transition_detector`], which does add the remainder (and does it in
/// a way that accumulates; see there).
#[must_use]
pub fn calc_ahd(
    input_hist: &RegionHistograms,
    ref_hist: &RegionHistograms,
    ref_width: u32,
    ref_height: u32,
    regions_per_width: usize,
    regions_per_height: usize,
) -> (u32, u8) {
    let region_width = ref_width / regions_per_width as u32;
    let region_height = ref_height / regions_per_height as u32;
    let mut ahd: u32 = 0;
    let mut active_region_cnt: u8 = 0;
    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            let mut ahd_per_region: u32 = 0;
            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd_per_region = ahd_per_region.wrapping_add(
                    (input_hist[w][h][bin] as i32 - ref_hist[w][h][bin] as i32).unsigned_abs(),
                );
            }
            ahd = ahd.wrapping_add(ahd_per_region);
            if ahd_per_region > region_width.wrapping_mul(region_height) {
                active_region_cnt = active_region_cnt.wrapping_add(1);
            }
        }
    }
    (ahd, active_region_cnt)
}

/// C `calc_ahd_pd` (`pd_process.c:5192-5215`) — static.
///
/// Fills `pcs->ahd_error`, read by `motion_estimation.c:1245` as an ME gating
/// threshold. Live whenever `scs->calc_hist` is set, which is whenever any TF
/// type is enabled — i.e. in the default random-access config.
///
/// Unlike [`calc_ahd`] this one sums a single running total with no per-region
/// bookkeeping and no region-size test.
#[must_use]
pub fn calc_ahd_pd(
    cur_hist: &RegionHistograms,
    prev_hist: &RegionHistograms,
    regions_per_width: usize,
    regions_per_height: usize,
) -> u32 {
    let mut ahd: u32 = 0;
    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd = ahd.wrapping_add(
                    (cur_hist[w][h][bin] as i32 - prev_hist[w][h][bin] as i32).unsigned_abs(),
                );
            }
        }
    }
    ahd
}

/// The cross-picture detector state `scene_transition_detector` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneDetectState {
    /// C `ctx->ahd_running_avg[w][h]`.
    pub ahd_running_avg: [[u32; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH],
    /// C `ctx->reset_running_avg`.
    pub reset_running_avg: bool,
    /// C `ctx->prev_picture_histogram`.
    pub prev_picture_histogram: alloc::boxed::Box<RegionHistograms>,
    /// C `ctx->prev_average_intensity_per_region`.
    pub prev_average_intensity_per_region: RegionIntensities,
}

impl Default for SceneDetectState {
    fn default() -> Self {
        Self {
            ahd_running_avg: [[0; MAX_NUMBER_OF_REGIONS_IN_HEIGHT]; MAX_NUMBER_OF_REGIONS_IN_WIDTH],
            // C's ctor zeroes the context, so reset_running_avg starts FALSE
            // and the first picture therefore folds its ahd into a zero
            // running average rather than seeding it. That asymmetry is C's.
            reset_running_avg: false,
            prev_picture_histogram: alloc::boxed::Box::new(
                [[[0u32; HISTOGRAM_NUMBER_OF_BINS]; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
                    MAX_NUMBER_OF_REGIONS_IN_WIDTH],
            ),
            prev_average_intensity_per_region: [[0; MAX_NUMBER_OF_REGIONS_IN_HEIGHT];
                MAX_NUMBER_OF_REGIONS_IN_WIDTH],
        }
    }
}

/// C `scene_transition_detector` (`pd_process.c:256-378`) — static.
///
/// Its output becomes `pcs->transition_present` via `init_pic_settings`, which
/// sharpness-tuned MD reads. Random access only.
///
/// **THE TRAP, and it is a real C quirk that must be reproduced, not tidied.**
/// `region_width` and `region_height` are declared OUTSIDE the region loops
/// and updated inside with `region_width += region_width_offset;`. The offsets
/// are non-zero only on the last row/column, so:
/// * `region_height` grows by the height remainder on EVERY last-height
///   iteration, i.e. once per width column, and the growth persists into the
///   next column;
/// * `region_width` grows by the width remainder on every iteration of the
///   FINAL width column, i.e. `regions_per_height` times.
///
/// `region_threshold` is computed from those accumulating values, so the
/// threshold is different in later regions than in earlier ones even for
/// identically sized regions. Transcribed literally.
///
/// Returns whether a scene change was detected; updates
/// `state.ahd_running_avg` and `state.reset_running_avg` in place.
#[must_use]
pub fn scene_transition_detector(
    state: &mut SceneDetectState,
    current_hist: &RegionHistograms,
    current_intensity: &RegionIntensities,
    future_intensity: &RegionIntensities,
    picture_width: u32,
    picture_height: u32,
    regions_per_width: usize,
    regions_per_height: usize,
) -> bool {
    let mut is_abrupt_change_count: u32 = 0;
    let mut is_scene_change_count: u32 = 0;

    // C: (uint32_t)(((float)((w_regions * h_regions) * 50) / 100) + 0.5)
    let region_count_threshold =
        ((((regions_per_width * regions_per_height) * 50) as f32 / 100.0) + 0.5) as u32;

    // Declared OUTSIDE the loops in C and mutated inside — see the doc.
    let mut region_width = picture_width / regions_per_width as u32;
    let mut region_height = picture_height / regions_per_height as u32;

    for w in 0..regions_per_width {
        for h in 0..regions_per_height {
            let mut is_abrupt_change = false;
            let mut is_scene_change = false;

            let mut ahd: u32 = 0;

            let region_width_offset = if w == regions_per_width - 1 {
                picture_width.wrapping_sub(regions_per_width as u32 * region_width)
            } else {
                0
            };
            let region_height_offset = if h == regions_per_height - 1 {
                picture_height.wrapping_sub(regions_per_height as u32 * region_height)
            } else {
                0
            };
            region_width = region_width.wrapping_add(region_width_offset);
            region_height = region_height.wrapping_add(region_height_offset);

            let region_threshold =
                SCENE_TH.wrapping_mul(num_64x64_in_pic(region_width, region_height));

            for bin in 0..HISTOGRAM_NUMBER_OF_BINS {
                ahd = ahd.wrapping_add(
                    (current_hist[w][h][bin] as i32
                        - state.prev_picture_histogram[w][h][bin] as i32)
                        .unsigned_abs(),
                );
            }

            if state.reset_running_avg {
                state.ahd_running_avg[w][h] = ahd;
            }

            let ahd_error = (state.ahd_running_avg[w][h] as i32 - ahd as i32).unsigned_abs();

            if ahd_error > region_threshold && ahd >= ahd_error {
                is_abrupt_change = true;
            }
            if is_abrupt_change {
                // Average intensity differences, all narrowed to uint8_t by C
                // AFTER the abs — a difference above 255 wraps, which is
                // reproduced with the same truncation.
                // C: `(uint8_t)ABS((int16_t)a - (int16_t)b)`. Each 64-bit
                // value is TRUNCATED to int16 first, the subtraction happens
                // in `int` after integer promotion, and the absolute value is
                // then truncated to uint8. All three narrowings are
                // reproduced exactly; collapsing them changes the result for
                // any value outside 0..=255.
                let aid = |a: u64, b: u64| -> u8 {
                    let d = i32::from(a as i16) - i32::from(b as i16);
                    d.unsigned_abs() as u8
                };
                let prev = state.prev_average_intensity_per_region[w][h];
                let aid_future_past = aid(future_intensity[w][h], prev);
                let aid_future_present = aid(future_intensity[w][h], current_intensity[w][h]);
                let aid_present_past = aid(current_intensity[w][h], prev);

                if aid_future_past < FLASH_TH
                    && aid_future_present >= FLASH_TH
                    && aid_present_past >= FLASH_TH
                {
                    // A flash, not a scene change.
                } else if aid_future_present < FADE_TH && aid_present_past < FADE_TH {
                    // A fade, not a scene change.
                } else {
                    is_scene_change = true;
                }
            } else {
                state.ahd_running_avg[w][h] = (3u32
                    .wrapping_mul(state.ahd_running_avg[w][h])
                    .wrapping_add(ahd))
                    / 4;
            }
            is_abrupt_change_count += u32::from(is_abrupt_change);
            is_scene_change_count += u32::from(is_scene_change);
        }
    }

    state.reset_running_avg = is_abrupt_change_count >= region_count_threshold;
    is_scene_change_count >= region_count_threshold
}

/// What `perform_scene_change_detection` decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneChangeOutcome {
    /// C `pcs->scene_change_flag`.
    pub scene_change_flag: bool,
    /// C `pcs->cra_flag` after the update.
    pub cra_flag: bool,
    /// C `ctx->transition_detected`.
    pub transition_detected: i32,
    /// C `ctx->is_scene_change_detected`.
    pub is_scene_change_detected: bool,
}

/// C `perform_scene_change_detection` (`pd_process.c:4682-4700`) — static.
///
/// Which of the two detector calls fires is the whole content of this
/// function, and both are gated on settings measured above:
/// * `static_config.scene_change_detection` is force-zeroed
///   (`enc_settings.c:839-843`), so the first arm is dead in mainline;
/// * `vq_ctrls.sharpness_ctrls.scene_transition` is 1 in both arms of
///   `derive_vq_params` and zeroed only for LOW_DELAY, so the SECOND arm is
///   live in random access.
///
/// The second arm also only runs while `transition_detected` is -1 or 0 — once
/// a transition is latched at 1 it stays until `init_pic_settings` consumes it
/// at a base-layer picture.
///
/// `run_detector` is the [`scene_transition_detector`] result, computed by the
/// caller because it needs the picture window; `None` means the caller
/// determined neither arm fires.
#[must_use]
pub fn perform_scene_change_detection(
    scene_change_detection_enabled: bool,
    sharpness_scene_transition: bool,
    transition_detected_in: i32,
    cra_flag_in: bool,
    run_detector: impl FnOnce() -> bool,
) -> SceneChangeOutcome {
    let mut transition_detected = transition_detected_in;
    let scene_change_flag = if scene_change_detection_enabled {
        run_detector()
    } else {
        if sharpness_scene_transition
            && (transition_detected_in == -1 || transition_detected_in == 0)
        {
            transition_detected = i32::from(run_detector());
        }
        false
    };
    SceneChangeOutcome {
        scene_change_flag,
        cra_flag: if scene_change_flag { true } else { cra_flag_in },
        transition_detected,
        is_scene_change_detected: scene_change_flag,
    }
}

/// C `copy_histograms` (`pd_process.c:4703-4719`) — static.
///
/// Carries the current picture's histogram forward for the next input
/// picture. Without it [`calc_ahd_pd`] and [`scene_transition_detector`] read
/// zeros and every downstream threshold decision flips.
///
/// Trap: the loops run over `MAX_NUMBER_OF_REGIONS_IN_{WIDTH,HEIGHT}` (4 and
/// 4), NOT over `scs->picture_analysis_number_of_regions_per_*`. So regions
/// the detector never reads are copied anyway — reproduced, because a port
/// that copied only the active regions would leave stale data in the rest and
/// diverge the moment the region count changes.
pub fn copy_histograms(
    state: &mut SceneDetectState,
    picture_histogram: &RegionHistograms,
    average_intensity_per_region: &RegionIntensities,
) {
    for w in 0..MAX_NUMBER_OF_REGIONS_IN_WIDTH {
        for h in 0..MAX_NUMBER_OF_REGIONS_IN_HEIGHT {
            state.prev_picture_histogram[w][h] = picture_histogram[w][h];
            state.prev_average_intensity_per_region[w][h] = average_intensity_per_region[w][h];
        }
    }
}

// ---------------------------------------------------------------------------
// Dynamic-GOP detector — pd_process.c:403-758
// ---------------------------------------------------------------------------
//
// Reachability, measured (`enc_handle.c:4294-4300`): `scs->enable_dg` is 0 for
// VBR, CBR, >= 4K, non-RANDOM_ACCESS and multi-pass, and 1 otherwise — so this
// whole detector is ON BY DEFAULT for single-pass CQP/CRF random access below
// 4K. It is not an exotic knob, and a one-SAD difference here flips the
// mini-GOP SIZE, i.e. the temporal layer of every frame in it.

/// C `FULL_SAD_SEARCH` (`definitions.h:1821`).
pub const FULL_SAD_SEARCH: u8 = 1;
/// C `INPUT_SIZE_360p_RANGE` (`definitions.h:1825`).
pub const INPUT_SIZE_360P_RANGE: u8 = 1;
/// C `HIGH_DIST_TH` (`pd_process.c:684`) — `16 * 16 * 18`.
pub const HIGH_DIST_TH: u64 = 16 * 16 * 18;
/// C `LOW_DIST_TH` (`pd_process.c:685`) — `16 * 16 * 2`.
pub const LOW_DIST_TH: u64 = 16 * 16 * 2;

/// A borrowed view of a sixteenth-downsampled luma plane, as
/// `EbPictureBufferDesc` presents one to the detector.
///
/// `y_buffer` is the interior origin: the search can address NEGATIVE offsets
/// (up to `border - 1` pixels left/up), so the backing allocation must extend
/// `border` pixels in every direction and `origin` is where (0, 0) sits.
#[derive(Debug, Clone, Copy)]
pub struct DsPlane<'a> {
    /// The whole padded allocation.
    pub data: &'a [u8],
    /// Index of pixel (0, 0) within `data`.
    pub origin: usize,
    /// C `y_stride`.
    pub stride: usize,
    /// C `width` (the un-padded picture width).
    pub width: u16,
    /// C `height`.
    pub height: u16,
    /// C `border` — padding on each side.
    pub border: u16,
}

/// C `early_hme_b64` (`pd_process.c:403-491`) — static.
///
/// The per-64x64 search kernel underneath the dynamic-GOP detector. Needs a
/// bit-exact port because a one-SAD difference flips the GOP-size decision.
///
/// Returns `(best_sad, sr_center)`.
///
/// Traps transcribed literally:
/// * `sa_width` is rounded UP to a multiple of 8 on entry, then clamped
///   against the picture, then rounded DOWN to a multiple of 8 — but only when
///   it is at least 8 (`sa_width < 8 ? sa_width : sa_width & ~7`).
/// * the left-edge correction `sa_width = sa_width - (-pad_width - (org_x +
///   sa_origin_x))` is evaluated AFTER `sa_origin_x` has already been
///   reassigned on the line above, so the parenthesised term is identically
///   zero and the width is unchanged. That is C's code, not a transcription
///   slip; a "fixed" version would clamp differently.
/// * `best_sad` is doubled for the non-`FULL_SAD_SEARCH` method because only
///   every other line was summed, and the centre is scaled by 4 because the
///   search ran at sixteenth (quarter-per-axis) resolution.
///
/// # Panics
///
/// Panics if the search region falls outside `plane.data`; that is a caller
/// error in the padding setup, and reading the wrong memory would silently
/// produce a wrong SAD.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn early_hme_b64(
    src: &[u8],
    src_stride: usize,
    hme_search_method: u8,
    org_x: i16,
    org_y: i16,
    block_width: u32,
    block_height: u32,
    sa_width_in: i16,
    sa_height_in: i16,
    ref_plane: &DsPlane<'_>,
) -> (u64, svtav1_types::motion::Mv) {
    // Round the search width up to a multiple of 8: the SAD kernel costs the
    // same for widths 1..8.
    let mut sa_width = (sa_width_in + 7) & !0x07;
    let mut sa_height = sa_height_in;
    let pad_width = ref_plane.border as i16 - 1;
    let pad_height = ref_plane.border as i16 - 1;

    let mut sa_origin_x = -(sa_width >> 1);
    let mut sa_origin_y = -(sa_height >> 1);

    // Left edge. NOTE: C reassigns sa_origin_x first, so the width adjustment
    // that follows subtracts zero. Reproduced exactly.
    if org_x + sa_origin_x < -pad_width {
        sa_origin_x = -pad_width - org_x;
        sa_width -= -pad_width - (org_x + sa_origin_x);
    }
    // Right edge.
    if org_x + sa_origin_x > ref_plane.width as i16 - 1 {
        sa_origin_x -= (org_x + sa_origin_x) - (ref_plane.width as i16 - 1);
    }
    if org_x + sa_origin_x + sa_width > ref_plane.width as i16 {
        sa_width = 1.max(sa_width - ((org_x + sa_origin_x + sa_width) - ref_plane.width as i16));
    }
    // Round DOWN to a multiple of 8, but only at 8 or more.
    sa_width = if sa_width < 8 {
        sa_width
    } else {
        sa_width & !0x07
    };

    // Top edge — same shape as the left edge, same zero-subtraction.
    if org_y + sa_origin_y < -pad_height {
        sa_origin_y = -pad_height - org_y;
        sa_height -= -pad_height - (org_y + sa_origin_y);
    }
    if org_y + sa_origin_y > ref_plane.height as i16 - 1 {
        sa_origin_y -= (org_y + sa_origin_y) - (ref_plane.height as i16 - 1);
    }
    if org_y + sa_origin_y + sa_height > ref_plane.height as i16 {
        sa_height =
            1.max(sa_height - ((org_y + sa_origin_y + sa_height) - ref_plane.height as i16));
    }

    let x_top_left = org_x + sa_origin_x;
    let y_top_left = org_y + sa_origin_y;
    let search_region_index =
        i64::from(x_top_left) + i64::from(y_top_left) * ref_plane.stride as i64;

    let full = hme_search_method == FULL_SAD_SEARCH;
    let r = crate::inter_me::sad::sad_loop_kernel(
        src,
        if full { src_stride } else { src_stride * 2 },
        ref_plane.data,
        ref_plane.origin as i64 + search_region_index,
        if full {
            ref_plane.stride
        } else {
            ref_plane.stride * 2
        },
        if full {
            block_height as usize
        } else {
            (block_height >> 1) as usize
        },
        block_width as usize,
        ref_plane.stride,
        0,
        sa_width,
        sa_height,
    );

    let best_sad = if full { r.best_sad } else { r.best_sad * 2 };
    let mut mv = svtav1_types::motion::Mv { x: 0, y: 0 };
    // Operating on 1/4 resolution per axis, hence the x4.
    mv.x = (r.x_search_center + sa_origin_x) * 4;
    mv.y = (r.y_search_center + sa_origin_y) * 4;
    (best_sad, mv)
}

/// C `DGDetectorMetrics` (`pcs.h:710-716`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DgDetectorMetrics {
    /// C `tot_dist`.
    pub tot_dist: u64,
    /// C `tot_cplx`.
    pub tot_cplx: u32,
    /// C `tot_active`.
    pub tot_active: u32,
    /// C `sum_in_vectors`.
    pub sum_in_vectors: i32,
    /// C `seg_completed`.
    pub seg_completed: u16,
}

/// C `dg_detector_hme_level0` (`pd_process.c:532-629`) — EXPORTED.
///
/// Segment-level entry to the dynamic-GOP HME. Accumulates distortion,
/// complexity, activity and an inward/outward motion-vector balance over the
/// segment's 64x64 blocks.
///
/// Traps:
/// * `hme_level0_sad` and `sr_center` are declared OUTSIDE the block loop and
///   passed by pointer to [`early_hme_b64`], which OVERWRITES both, so no
///   value carries between blocks — but the initial `~0` seed does reach the
///   first call. `sad_loop_kernel` ignores its incoming value, so this is
///   inert; it is preserved so a reader is not tempted to "fix" it.
/// * the `sum_in_vectors` update SKIPS the middle row and column exactly
///   (`< n/2` and `> n/2`, never `==`), so an odd block count leaves one row
///   and one column contributing nothing.
///
/// `metrics` is accumulated in place, matching C's mutex-guarded shared
/// struct.
#[allow(clippy::too_many_arguments)]
pub fn dg_detector_hme_level0(
    metrics: &mut DgDetectorMetrics,
    src_plane: &DsPlane<'_>,
    ref_plane: &DsPlane<'_>,
    input_resolution: u8,
    aligned_width: u32,
    aligned_height: u32,
    b64_size: u32,
    seg_idx: u32,
    me_segments_column_count: u32,
    me_segments_row_count: u32,
) {
    let (sa_width, sa_height) = if input_resolution <= INPUT_SIZE_360P_RANGE {
        (16i16, 16i16)
    } else if input_resolution <= INPUT_SIZE_480P_RANGE {
        (64, 64)
    } else {
        (128, 128)
    };

    let pic_width_in_b64 = aligned_width.div_ceil(b64_size);
    let pic_height_in_b64 = aligned_height.div_ceil(b64_size);

    // SEGMENT_CONVERT_IDX_TO_XY(seg_idx, x, y, me_segments_column_count)
    let y_seg_idx = seg_idx / me_segments_column_count;
    let x_seg_idx = seg_idx - y_seg_idx * me_segments_column_count;
    let x_b64_start = (x_seg_idx * pic_width_in_b64) / me_segments_column_count;
    let x_b64_end = ((x_seg_idx + 1) * pic_width_in_b64) / me_segments_column_count;
    let y_b64_start = (y_seg_idx * pic_height_in_b64) / me_segments_row_count;
    let y_b64_end = ((y_seg_idx + 1) * pic_height_in_b64) / me_segments_row_count;

    for y_b64_idx in y_b64_start..y_b64_end {
        for x_b64_idx in x_b64_start..x_b64_end {
            let b64_origin_x = x_b64_idx * 64;
            let b64_origin_y = y_b64_idx * 64;
            let buffer_index =
                (b64_origin_y >> 2) as usize * src_plane.stride + (b64_origin_x >> 2) as usize;

            let (sad, sr_center) = early_hme_b64(
                &src_plane.data[src_plane.origin + buffer_index..],
                src_plane.stride,
                FULL_SAD_SEARCH,
                (b64_origin_x as i16) >> 2,
                (b64_origin_y as i16) >> 2,
                16,
                16,
                sa_width,
                sa_height,
                ref_plane,
            );

            metrics.tot_dist += sad;
            metrics.tot_cplx += u32::from(sad > (16 * 16 * 30));
            metrics.tot_active += u32::from(sr_center.x.abs() > 0 || sr_center.y.abs() > 0);

            // Row balance: the MIDDLE row is skipped (never `==`).
            if y_b64_idx < pic_height_in_b64 / 2 {
                if sr_center.y > 0 {
                    metrics.sum_in_vectors -= 1;
                } else if sr_center.y < 0 {
                    metrics.sum_in_vectors += 1;
                }
            } else if y_b64_idx > pic_height_in_b64 / 2 {
                if sr_center.y > 0 {
                    metrics.sum_in_vectors += 1;
                } else if sr_center.y < 0 {
                    metrics.sum_in_vectors -= 1;
                }
            }
            // Column balance: same shape, same skipped middle.
            if x_b64_idx < pic_width_in_b64 / 2 {
                if sr_center.x > 0 {
                    metrics.sum_in_vectors -= 1;
                } else if sr_center.x < 0 {
                    metrics.sum_in_vectors += 1;
                }
            } else if x_b64_idx > pic_width_in_b64 / 2 {
                if sr_center.x > 0 {
                    metrics.sum_in_vectors += 1;
                } else if sr_center.x < 0 {
                    metrics.sum_in_vectors -= 1;
                }
            }
        }
    }
    metrics.seg_completed += 1;
}

/// The per-pair result `early_hme` leaves on the picture-decision context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EarlyHmeResult {
    /// C `ctx->mv_in_out_count`.
    pub mv_in_out_count: i16,
    /// C `ctx->norm_dist`.
    pub norm_dist: u64,
    /// C `ctx->perc_cplx`.
    pub perc_cplx: u8,
    /// C `ctx->perc_active`.
    pub perc_active: u8,
}

/// C `early_hme`'s reduction (`pd_process.c:669-684`) — static.
///
/// Turns the accumulated [`DgDetectorMetrics`] into the four per-pair numbers
/// `calc_mini_gop_activity` consumes. The segment dispatch around it is
/// threading this port replaces; the caller runs
/// [`dg_detector_hme_level0`] over its segments and passes the totals.
///
/// Trap: the block counts here use a HARDCODED 64
/// (`(aligned_width + 63) / 64`), not `scs->b64_size` as
/// `dg_detector_hme_level0` does. The two agree at the default b64_size of 64
/// and would not at 128 — reproduced rather than unified.
#[must_use]
pub fn early_hme_reduce(
    metrics: &DgDetectorMetrics,
    aligned_width: u32,
    aligned_height: u32,
) -> EarlyHmeResult {
    let pic_width_in_b64 = aligned_width.div_ceil(64);
    let pic_height_in_b64 = aligned_height.div_ceil(64);
    let blocks = i64::from(pic_height_in_b64 * pic_width_in_b64);
    EarlyHmeResult {
        mv_in_out_count: (i64::from(metrics.sum_in_vectors) * 100 / blocks) as i16,
        norm_dist: metrics.tot_dist / (blocks as u64),
        perc_cplx: ((u64::from(metrics.tot_cplx) * 100) / blocks as u64) as u8,
        perc_active: ((u64::from(metrics.tot_active) * 100) / blocks as u64) as u8,
    }
}

/// C `calc_mini_gop_activity` (`pd_process.c:686-712`) — static.
///
/// The 6L-vs-5L split decision. Returns `true` when the top layer should be
/// re-activated (i.e. SPLIT into the two sub layers); the caller then sets
/// `activity[top] = true` and `activity[sub0] = activity[sub1] = false`.
///
/// Traps:
/// * `bias` is 25 when the previous mini-GOP in this GOP was 5L and this is
///   not the first mini-GOP, else 75 — a hysteresis toward staying at 6L.
/// * `top_layer_mv_in_out_count` is `(void)`-cast UNUSED in C. It is accepted
///   here for call-site fidelity and deliberately not read.
/// * the CALL SITE passes the two sub-layer mv counts in the order
///   (end-mid, mid-start) into parameters named `..._count1` and `..._count2`,
///   so `count1` belongs to `sub_layer_idx1` and `count2` to `sub_layer_idx0`
///   — the names are inverted relative to the indices. `cond3` takes a MIN and
///   a MAX so the order does not change the result, which is presumably why it
///   was never noticed.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn calc_mini_gop_activity(
    mini_gop_cnt_per_gop: u32,
    previous_mini_gop_hierarchical_levels: u32,
    top_layer_dist: u64,
    top_layer_perc_active: u8,
    top_layer_perc_cplx: u8,
    sub_layer_dist0: u64,
    sub_layer0_perc_active: u8,
    sub_layer0_perc_cplx: u8,
    sub_layer_dist1: u64,
    sub_layer1_perc_active: u8,
    sub_layer1_perc_cplx: u8,
    _top_layer_mv_in_out_count: i16,
    sub_layer_mv_in_out_count1: i16,
    sub_layer_mv_in_out_count2: i16,
) -> bool {
    let bias: u64 = if mini_gop_cnt_per_gop > 1 && previous_mini_gop_hierarchical_levels == 5 {
        25
    } else {
        75
    };

    let cond1 = top_layer_perc_active >= 95
        && !(sub_layer0_perc_active >= 95 && sub_layer1_perc_active < 75)
        && !(sub_layer0_perc_active < 75 && sub_layer1_perc_active >= 95);
    let cond2 = top_layer_dist > LOW_DIST_TH
        && sub_layer_dist0 < HIGH_DIST_TH
        && sub_layer_dist1 < HIGH_DIST_TH
        && top_layer_perc_cplx > 0
        && sub_layer0_perc_cplx < 25
        && sub_layer1_perc_cplx < 25
        && ((sub_layer_dist0 + sub_layer_dist1) / 2) < ((bias * top_layer_dist) / 100);
    let cond3 = sub_layer_mv_in_out_count1.min(sub_layer_mv_in_out_count2) > 40
        && sub_layer_mv_in_out_count1.max(sub_layer_mv_in_out_count2) > 55;

    cond1 && (cond2 || cond3)
}

/// C `eval_sub_mini_gop` (`pd_process.c:713-758`) — static.
///
/// Runs three `early_hme` passes — (end, start), (end, mid), (mid, start) —
/// and commits the split. The caller supplies the three reductions because the
/// HME itself needs picture buffers; this is the decision half.
///
/// Argument-order trap, and it is the reason this wrapper exists at all: the
/// C call maps `(end,start)` to the TOP layer, `(mid,start)` to sub layer 0
/// and `(end,mid)` to sub layer 1 — NOT in the order the three passes are run.
/// Getting that mapping wrong silently swaps the two sub layers' statistics.
#[must_use]
pub fn eval_sub_mini_gop(
    mini_gop_cnt_per_gop: u32,
    previous_mini_gop_hierarchical_levels: u32,
    end_start: EarlyHmeResult,
    end_mid: EarlyHmeResult,
    mid_start: EarlyHmeResult,
) -> bool {
    calc_mini_gop_activity(
        mini_gop_cnt_per_gop,
        previous_mini_gop_hierarchical_levels,
        // top layer <- (end, start)
        end_start.norm_dist,
        end_start.perc_active,
        end_start.perc_cplx,
        // sub layer 0 <- (mid, start)
        mid_start.norm_dist,
        mid_start.perc_active,
        mid_start.perc_cplx,
        // sub layer 1 <- (end, mid)
        end_mid.norm_dist,
        end_mid.perc_active,
        end_mid.perc_cplx,
        end_start.mv_in_out_count,
        end_mid.mv_in_out_count,
        mid_start.mv_in_out_count,
    )
}

/// Apply [`eval_sub_mini_gop`]'s verdict to the activity array, as C does at
/// `pd_process.c:707-711`.
pub fn commit_sub_mini_gop_split(
    map: &mut MiniGopMap,
    split: bool,
    top_layer_idx: usize,
    sub_layer_idx0: usize,
    sub_layer_idx1: usize,
) {
    if split {
        map.activity[top_layer_idx] = true;
        map.activity[sub_layer_idx0] = false;
        map.activity[sub_layer_idx1] = false;
    }
}

// ---------------------------------------------------------------------------
// Temporal-filter window — pd_process.c:3642-4250
// ---------------------------------------------------------------------------
//
// Reachability, measured: in LOW_DELAY `tf_level` is forced to 0 before any
// preset logic (`enc_handle.c:3339-3343`), so this whole group is dead for the
// campaign's first cell. It is ON BY DEFAULT in random access (tf_level 5 at
// M3-M7), where the temporal filter REWRITES THE SOURCE PIXELS of base-layer
// frames — so no random-access frame can be byte-identical without it.

/// C `ALTREF_MAX_NFRAMES` (`definitions.h:338`).
pub const ALTREF_MAX_NFRAMES: usize = 33;
/// C `TF_MAX_EXTENSION` (`definitions.h:340`).
pub const TF_MAX_EXTENSION: i32 = 6;
/// C `TF_MAX_BASE_REF_PICS` (`definitions.h:341`).
pub const TF_MAX_BASE_REF_PICS: u8 = 7;
/// C `TF_MAX_L1_REF_PICS_6L` (`definitions.h:342`).
pub const TF_MAX_L1_REF_PICS_6L: u8 = 2;
/// C `TF_MAX_L1_REF_PICS_SUB_6L` (`definitions.h:343`).
pub const TF_MAX_L1_REF_PICS_SUB_6L: u8 = 1;
/// C `VQ_NOISE_LVL_TH` (`definitions.h:83`).
pub const VQ_NOISE_LVL_TH: i32 = 15000;

/// C `DIVIDE_AND_ROUND(x, y)` (`utility.h:96`) — `((x) + ((y) >> 1)) / (y)`.
#[inline]
#[must_use]
pub fn divide_and_round(x: i32, y: i32) -> i32 {
    (x + (y >> 1)) / y
}

/// C `svt_aom_tf_max_ref_per_struct` (`enc_handle.c:2506-2519`) — EXPORTED.
///
/// The per-side cap on temporal-filter reference pictures.
///
/// Two traps: `direction` is `(void)`-cast UNUSED (past and future share a
/// cap), and the `type` encoding is 0 = I_SLICE, 1 = BASE, 2 = L1 — the
/// I_SLICE arm is `1 << hierarchical_levels`, which is the ONLY arm that grows
/// with the hierarchy.
#[must_use]
pub fn tf_max_ref_per_struct(hierarchical_levels: u32, ty: u8, _direction: bool) -> u8 {
    if ty == 0 {
        // C computes `1 << hierarchical_levels` into a uint8_t, so a hierarchy
        // of 8 or more wraps. Reproduced with a wrapping shift.
        (1u32 << hierarchical_levels) as u8
    } else if ty == 1 {
        TF_MAX_BASE_REF_PICS
    } else if hierarchical_levels < 5 {
        TF_MAX_L1_REF_PICS_SUB_6L
    } else {
        TF_MAX_L1_REF_PICS_6L
    }
}

/// The `TfControls` fields the window derivation reads (`pcs.h`'s
/// `TfControls`, the subset `pd_process.c` uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TfWindowCtrls {
    /// C `tf_ctrls.enabled`.
    pub enabled: bool,
    /// C `tf_ctrls.modulate_pics` (0 disables modulation entirely).
    pub modulate_pics: u8,
    /// C `tf_ctrls.num_past_pics`.
    pub num_past_pics: u8,
    /// C `tf_ctrls.num_future_pics`.
    pub num_future_pics: u8,
    /// C `tf_ctrls.max_num_past_pics`.
    pub max_num_past_pics: u8,
    /// C `tf_ctrls.max_num_future_pics`.
    pub max_num_future_pics: u8,
    /// C `tf_ctrls.qp_opt`.
    pub qp_opt: bool,
    /// C `tf_ctrls.use_intra_for_noise_est`.
    pub use_intra_for_noise_est: bool,
    /// C `tf_ctrls.chroma_lvl`.
    pub chroma_lvl: u8,
}

/// C `ref_pics_modulation` (`pd_process.c:3642-3745`) — static.
///
/// Modulates the temporal-filter reference count from the noise level (I
/// slices) or the filtered-vs-unfiltered intra distortion ratio (inter). It
/// changes how many pictures the filter averages, hence the SOURCE PIXELS.
///
/// Traps:
/// * the I-slice arm reads noise DIRECTLY against three Q16 log1p constants
///   (26572 = log1p(0.5), 45426 = log1p(1.0), 71998 = log1p(2.0)) and yields
///   6 / 4 / 2 / 0 — a LOWER noise level gets MORE frames, which is the
///   opposite of the intuitive direction and is what the C comment explains.
/// * the inter arms divide by the noise level, so `noise == 0` short-circuits
///   the ratio to 0 rather than dividing.
/// * base-layer and non-base use DIFFERENT `modulate_pics` tables, and the
///   non-base one has no `case 4`, so `modulate_pics == 4` falls through to
///   `offset = 0` there while base-layer gives 0/1/2.
///
/// `q_weight` / `q_weight_denom` come from
/// `svt_aom_get_qp_based_th_scaling_factors`, which belongs to the
/// signal-derivation module; the caller supplies them and they are applied
/// here only when `qp_opt`.
#[must_use]
pub fn ref_pics_modulation(
    is_i_slice: bool,
    temporal_layer_index: u8,
    ctrls: &TfWindowCtrls,
    noise_levels_log1p_fp16: i32,
    filt_to_unfilt_diff: u32,
    q_weight: u32,
    q_weight_denom: u32,
) -> i32 {
    let mut offset: i32 = 0;

    if is_i_slice {
        // Q16 log1p thresholds; LOWER noise buys MORE filtering frames.
        if noise_levels_log1p_fp16 < 26572 {
            offset = 6;
        } else if noise_levels_log1p_fp16 < 45426 {
            offset = 4;
        } else if noise_levels_log1p_fp16 < 71998 {
            offset = 2;
        }
    } else {
        // C computes the ratio in `int`; the guard avoids a divide by zero.
        let ratio: i32 = if noise_levels_log1p_fp16 != 0 {
            ((filt_to_unfilt_diff as i32).wrapping_mul(100)) / noise_levels_log1p_fp16
        } else {
            0
        };
        if temporal_layer_index == 0 {
            offset = match ctrls.modulate_pics {
                1 => {
                    if ratio < 100 {
                        5
                    } else {
                        TF_MAX_EXTENSION
                    }
                }
                2 => {
                    if ratio < 50 {
                        3
                    } else if ratio < 100 {
                        5
                    } else {
                        TF_MAX_EXTENSION
                    }
                }
                3 => {
                    if ratio < 50 {
                        3
                    } else if ratio < 100 {
                        4
                    } else {
                        5
                    }
                }
                4 => {
                    if ratio < 50 {
                        0
                    } else if ratio < 100 {
                        1
                    } else {
                        2
                    }
                }
                // case 0 and default.
                _ => 0,
            };
        } else {
            offset = match ctrls.modulate_pics {
                1 => i32::from(ratio >= 25),
                2 => i32::from(ratio >= 50),
                3 => i32::from(ratio >= 75),
                // case 0 and default — note there is NO case 4 here.
                _ => 0,
            };
        }
    }

    if ctrls.qp_opt {
        offset = divide_and_round(offset * q_weight as i32, q_weight_denom as i32);
    }
    offset
}

/// Which arm of `derive_tf_window_params` a picture takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfWindowArm {
    /// `pred_structure != RANDOM_ACCESS` (`pd_process.c:3851-3921`).
    LowDelay,
    /// `svt_aom_is_delayed_intra(pcs)` (`:3922-3965`).
    DelayedIntra,
    /// `pcs->idr_flag` inside random access (`:3966-4002`).
    RandomAccessIdr,
    /// Everything else inside random access (`:4004-4100`).
    RandomAccessInter,
}

/// The past/future picture COUNTS `derive_tf_window_params` derives, before
/// the buffers are searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TfWindowCounts {
    /// C `num_past_pics`.
    pub num_past_pics: i32,
    /// C `num_future_pics`.
    pub num_future_pics: i32,
}

/// C `derive_tf_window_params`' COUNT derivation, per arm
/// (`pd_process.c:3851`, `:3930`, `:3973`, `:4005-4015`) — static.
///
/// This is the half that decides how wide the filter window may be; the other
/// half searches the reorder queue / mini-GOP array for pictures to fill it,
/// which is buffer plumbing this port replaces.
///
/// The four arms differ in ways that are easy to blur together:
/// * LOW DELAY adds `offset` only when `modulate_pics` is set, and caps
///   against `max_num_{past,future}_pics` — no `tf_max_ref_per_struct` cap.
/// * DELAYED INTRA has NO past pictures at all and caps future against
///   `tf_max_ref_per_struct(hier, 0, 1)` — the I_SLICE row, `1 << hier`.
/// * RANDOM-ACCESS IDR is the same shape as delayed intra.
/// * RANDOM-ACCESS INTER takes `MAX(1, base + offset)` on BOTH sides — the
///   `MAX(1, ...)` floor exists only here, so a negative modulation cannot
///   empty the window — then caps against `max_num_*` and against
///   `tf_max_ref_per_struct(hier, temporal_layer ? 2 : 1, dir)`.
///
/// Note that the inter arm adds `offset` UNCONDITIONALLY, while the low-delay
/// arm adds it only under `modulate_pics`. `offset` is itself zero when
/// `modulate_pics` is 0 (the caller gates `ref_pics_modulation` on it), so the
/// two agree in practice — but only because of that outer gate.
#[must_use]
pub fn derive_tf_window_counts(
    arm: TfWindowArm,
    ctrls: &TfWindowCtrls,
    offset: i32,
    hierarchical_levels: u32,
    temporal_layer_index: u8,
) -> TfWindowCounts {
    match arm {
        TfWindowArm::LowDelay => {
            let modulation = if ctrls.modulate_pics != 0 { offset } else { 0 };
            let num_past = (i32::from(ctrls.num_past_pics) + modulation)
                .min(i32::from(ctrls.max_num_past_pics));
            let num_future = (i32::from(ctrls.num_future_pics) + modulation)
                .min(i32::from(ctrls.max_num_future_pics));
            TfWindowCounts {
                num_past_pics: num_past,
                num_future_pics: num_future,
            }
        }
        TfWindowArm::DelayedIntra | TfWindowArm::RandomAccessIdr => {
            let modulation = if ctrls.modulate_pics != 0 { offset } else { 0 };
            // C computes this in a uint32_t, so a negative modulation wraps
            // before the MIN clamps it back down. Reproduced with a saturating
            // cast through u32 exactly as C does.
            let raw = (u32::from(ctrls.num_future_pics)).wrapping_add(modulation as u32);
            let num_future = raw.min(u32::from(ctrls.max_num_future_pics));
            let num_future = num_future.min(u32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                0,
                true,
            )));
            TfWindowCounts {
                num_past_pics: 0,
                num_future_pics: num_future as i32,
            }
        }
        TfWindowArm::RandomAccessInter => {
            // The MAX(1, ...) floor exists ONLY on this arm.
            let mut num_past = 1.max(i32::from(ctrls.num_past_pics) + offset);
            let mut num_future = 1.max(i32::from(ctrls.num_future_pics) + offset);
            num_past = num_past.min(i32::from(ctrls.max_num_past_pics));
            num_future = num_future.min(i32::from(ctrls.max_num_future_pics));
            let ty = if temporal_layer_index != 0 { 2 } else { 1 };
            num_past = num_past.min(i32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                ty,
                false,
            )));
            num_future = num_future.min(i32::from(tf_max_ref_per_struct(
                hierarchical_levels,
                ty,
                true,
            )));
            TfWindowCounts {
                num_past_pics: num_past,
                num_future_pics: num_future,
            }
        }
    }
}

/// C's past-window compaction (`pd_process.c:3914-3920` and `:4091-4098`) —
/// static.
///
/// When fewer past pictures were found than requested, the list is shifted
/// LEFT by the shortfall so the centre lands at index `actual_past_pics`.
///
/// **This block is UNREACHABLE in C** and is translated anyway, per
/// `docs/WORKING-ON-THIS.md` §7. `actual_past_pics` is initialised to
/// `num_past_pics` at `:3873` and `:4042` and never modified — only
/// `actual_future_pics` is incremented — so `actual_past_pics != num_past_pics`
/// is always false. Written up as `docs/SUSPECTED-C-BUGS.md` #18, with the
/// second-order finding that even a fixed counter would not fix the block:
/// C's loop is `while (list[pic_i] != NULL)` from index 0, and the situation
/// the block was written for is exactly the one that puts a NULL at index 0.
///
/// The caller must reproduce C's `actual_past_pics == num_past_pics` and
/// therefore never invoke this.
pub fn compact_tf_past_window(
    list: &mut [Option<usize>; ALTREF_MAX_NFRAMES],
    num_past_pics: usize,
    actual_past_pics: usize,
) {
    if actual_past_pics == num_past_pics {
        return;
    }
    let shift = num_past_pics - actual_past_pics;
    let mut i = 0usize;
    while i < ALTREF_MAX_NFRAMES && list[i].is_some() {
        list[i] = if i + shift < ALTREF_MAX_NFRAMES {
            list[i + shift]
        } else {
            None
        };
        i += 1;
    }
}

/// C's `tf_avg_luma` / `tf_avg_ahd_error` reduction
/// (`pd_process.c:4101-4118`) — static.
///
/// Averages the window's luma means and AHD errors, EXCLUDING the centre
/// picture (the one at index `past_altref_nframes`).
///
/// Returns `(tf_avg_luma, tf_avg_ahd_error)`, both zero when the window is
/// empty — C leaves `tf_avg_ahd_error` at 0 and does not touch `tf_avg_luma`
/// in that case, which this reproduces by returning the zero pair.
#[must_use]
pub fn tf_window_averages(
    window_avg_luma: &[u64],
    window_ahd_error: &[i32],
    past_altref_nframes: usize,
    future_altref_nframes: usize,
) -> (u64, i32) {
    let n = past_altref_nframes + future_altref_nframes;
    if n == 0 {
        return (0, 0);
    }
    let mut tot_luma: u64 = 0;
    let mut tot_err: i32 = 0;
    for i in 0..=n {
        if i != past_altref_nframes {
            tot_luma = tot_luma.wrapping_add(window_avg_luma[i]);
            tot_err = tot_err.wrapping_add(window_ahd_error[i]);
        }
    }
    (tot_luma / n as u64, tot_err / n as i32)
}

/// C `low_delay_store_tf_pictures`' STORE PREDICATE
/// (`pd_process.c:4127-4147`) — static.
///
/// A non-base low-delay picture joins the ring only when it is close enough to
/// the end of the mini-GOP to be a past reference for the upcoming base:
/// `temporal_layer_index != 0 && pic_idx_in_mg + 1 + tot_past >= mg_size`.
///
/// Reachability: in current mainline low-delay TF is disabled
/// (`enc_handle.c:3339-3343`), so this is dead for the first cell. It becomes
/// live the moment `tf_ld_controls` is given a non-zero level, which is why it
/// is translated (`docs/WORKING-ON-THIS.md` §7). The live-count bookkeeping
/// around it is buffer plumbing this port replaces.
#[must_use]
pub fn low_delay_should_store_tf_picture(
    temporal_layer_index: u8,
    pic_idx_in_mg: u32,
    max_num_past_pics: u8,
    hierarchical_levels: u32,
) -> bool {
    let mg_size = 1u32 << hierarchical_levels;
    temporal_layer_index != 0 && pic_idx_in_mg + 1 + u32::from(max_num_past_pics) >= mg_size
}

/// C `mctf_frame`'s decision half (`pd_process.c:4194-4250`) — static.
///
/// Everything in `mctf_frame` that is not fifo posting or semaphore waiting:
/// which of the two low-delay ring operations run, whether TF runs at all,
/// the motion-direction verdict and `is_noise_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MctfFrameDecision {
    /// Run `low_delay_store_tf_pictures` before filtering.
    pub store_ld_tf_pictures: bool,
    /// Run `derive_tf_window_params` + the filter.
    pub run_tf: bool,
    /// C `pcs->do_tf` — set FALSE when TF is off; C never sets it true here.
    pub do_tf_cleared: bool,
    /// C `pcs->is_noise_level`.
    pub is_noise_level: bool,
    /// Release the low-delay ring after filtering.
    pub release_ld_tf_pictures: bool,
}

/// C `mctf_frame` (`pd_process.c:4194-4250`) — static, decision half.
///
/// Trap: the STORE gate and the RELEASE gate are NOT symmetric. Both require
/// `pred_structure != RANDOM_ACCESS && tf_params_per_type[1].enabled`, but the
/// RELEASE additionally requires `temporal_layer_index == 0` — the ring is
/// filled by non-base pictures and drained by the base picture that consumed
/// it.
#[must_use]
pub fn mctf_frame_decision(
    seq_pred_structure: PredStructure,
    base_tf_params_enabled: bool,
    tf_ctrls_enabled: bool,
    temporal_layer_index: u8,
    last_i_noise_levels_log1p_fp16: i32,
) -> MctfFrameDecision {
    let ld = seq_pred_structure != PredStructure::RandomAccess;
    MctfFrameDecision {
        store_ld_tf_pictures: ld && base_tf_params_enabled,
        run_tf: tf_ctrls_enabled,
        do_tf_cleared: !tf_ctrls_enabled,
        is_noise_level: last_i_noise_levels_log1p_fp16 >= VQ_NOISE_LVL_TH,
        release_ld_tf_pictures: ld && base_tf_params_enabled && temporal_layer_index == 0,
    }
}

/// C `mctf_frame`'s motion-direction verdict (`pd_process.c:4232-4238`).
///
/// `0` horizontal, `1` vertical, `-1` neither. The comparison is
/// `horz > vert * 6 / 4`, i.e. a 1.5x margin, evaluated with INTEGER division
/// on the right-hand side — `vert * 6 / 4` truncates, so at `vert == 1` the
/// threshold is 1, not 1.5.
#[must_use]
pub fn tf_motion_direction(tf_tot_horz_blks: u32, tf_tot_vert_blks: u32) -> i8 {
    if tf_tot_horz_blks > tf_tot_vert_blks * 6 / 4 {
        0
    } else if tf_tot_vert_blks > tf_tot_horz_blks * 6 / 4 {
        1
    } else {
        -1
    }
}

/// One step of C `mctf_frame_st` (`pd_process.c:4175-4193`) — static.
///
/// The single-threaded temporal-filter dispatch. It is small, but it is where
/// the TF call sits in the per-frame sequence, and the ORDER is the content:
/// every step reads state a previous one wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MctfStStep {
    /// `me_ctx->me_type = ME_MCTF` — must precede the signal derivation, which
    /// branches on it.
    SetMeTypeMctf,
    /// `svt_aom_sig_deriv_me_tf(pcs, me_ctx)`.
    SigDerivMeTf,
    /// `svt_aom_gm_pre_processor(pcs, pcs->temp_filt_pcs_list)` — CONDITIONAL
    /// on `pcs->gm_ctrls.pp_enabled && pcs->gm_pp_enabled`.
    GmPreProcessor,
    /// `svt_av1_init_temporal_filtering(...)` once per segment, in index order.
    InitTemporalFilteringSegment(u16),
    /// Consume the semaphore the last segment posted.
    ConsumeDoneSemaphore,
}

/// C `mctf_frame_st` (`pd_process.c:4175-4193`) — static.
///
/// Returns the exact step sequence for `tf_segments_total_count` segments.
/// The callees themselves live in other modules (ME signal derivation, global
/// motion, temporal filtering), so what this port owns is the ORDER — which is
/// the part a reimplementation gets wrong, because `me_type` must be set
/// before the signal derivation reads it and the global-motion pre-pass must
/// run before the first segment.
#[must_use]
pub fn mctf_frame_st_sequence(
    tf_segments_total_count: u16,
    gm_pp_enabled: bool,
) -> alloc::vec::Vec<MctfStStep> {
    let mut steps = alloc::vec::Vec::new();
    steps.push(MctfStStep::SetMeTypeMctf);
    steps.push(MctfStStep::SigDerivMeTf);
    if gm_pp_enabled {
        steps.push(MctfStStep::GmPreProcessor);
    }
    for seg in 0..tf_segments_total_count {
        steps.push(MctfStStep::InitTemporalFilteringSegment(seg));
    }
    steps.push(MctfStStep::ConsumeDoneSemaphore);
    steps
}

/// The low-delay temporal-filter picture ring
/// (`ctx->tf_pic_array` + `ctx->tf_pic_arr_cnt`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LowDelayTfRing {
    /// Picture identifiers in store order; C holds PPCS pointers.
    pub pics: alloc::vec::Vec<u64>,
}

/// C `low_delay_store_tf_pictures`' ring append
/// (`pd_process.c:4127-4147`) — static.
///
/// Appends only when [`low_delay_should_store_tf_picture`] says so. C also
/// increments the live count of five wrappers here (`p_pcs_wrapper_ptr`,
/// `input_pic_wrapper`, `pa_ref_pic_wrapper`, `scs_wrapper` and, when present,
/// `y8b_wrapper`) so the resources survive until TF consumes them; that is
/// buffer plumbing this port replaces, and it is named rather than dropped.
pub fn low_delay_store_tf_picture(
    ring: &mut LowDelayTfRing,
    picture_number: u64,
    temporal_layer_index: u8,
    pic_idx_in_mg: u32,
    max_num_past_pics: u8,
    hierarchical_levels: u32,
) {
    if low_delay_should_store_tf_picture(
        temporal_layer_index,
        pic_idx_in_mg,
        max_num_past_pics,
        hierarchical_levels,
    ) {
        ring.pics.push(picture_number);
    }
}

/// C `low_delay_release_tf_pictures` (`pd_process.c:4151-4174`) — static.
///
/// Drains the ring: C releases each stored picture's wrappers and then
/// `memset`s the array and zeroes the count.
///
/// The RELEASE ORDER is load-bearing in C and is recorded here even though
/// this port has no wrappers to release: `input_pic_wrapper`, then
/// `y8b_wrapper` if present, then `pa_ref_pic_wrapper`, then `scs_wrapper`,
/// and the PPCS **last** — the comment says so explicitly, because the PPCS
/// owns the handles the earlier releases read.
///
/// Note also that C `memset`s only `tf_pic_arr_cnt` entries, not the whole
/// array, so entries past the count keep stale pointers. Harmless because the
/// count gates every read, and reproduced here by clearing the whole vector
/// (which cannot expose a stale entry at all).
pub fn low_delay_release_tf_pictures(ring: &mut LowDelayTfRing) {
    ring.pics.clear();
}

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
/// `ctx->pic_id_per_dpb_slot` — the app-driven STORE/CLEAR reference
/// management this port does not expose. There is no state here to clear, and
/// that is recorded rather than silently dropped.
pub fn set_key_frame_rps(pic: &mut PicParams, ctx: &mut PicDecisionCtx) {
    ctx.lay0_toggle = 0;
    ctx.lay1_toggle = 0;
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
/// Returns [`RpsBranchUnsupported`] for a random-access hierarchical branch.
pub fn generate_rps_info(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsBranchUnsupported> {
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
        return Err(RpsBranchUnsupported {
            hierarchical_levels: hier,
            temporal_layer,
        });
    }

    // C's tail: S-frame RPS and the app ref-mgmt events (both out of envelope,
    // see the module doc), then the overlay reset.
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
/// Propagates [`RpsBranchUnsupported`] from [`generate_rps_info`].
pub fn picture_decision_per_picture(
    pic: &mut PicParams,
    seq: &SeqPicParams,
    ctx: &mut PicDecisionCtx,
    pic_idx: u32,
    mg_idx: usize,
) -> Result<(), RpsBranchUnsupported> {
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

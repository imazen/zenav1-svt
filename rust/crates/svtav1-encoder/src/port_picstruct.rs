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

use crate::inter_mvp::{OrderHintInfo, av1_ref_frame_type, get_relative_dist};

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
    pub ref_frame_type_arr: [i8; 31],
    /// C `pcs->tot_ref_frame_types`.
    pub tot_ref_frame_types: u8,
    /// C `pcs->transition_present`.
    pub transition_present: u8,
    /// C `pcs->av1_cm->mi_cols`.
    pub mi_cols: u32,
    /// C `pcs->av1_cm->mi_rows`.
    pub mi_rows: u32,
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
            ref_frame_type_arr: [0; 31],
            tot_ref_frame_types: 0,
            transition_present: 0,
            mi_cols: 0,
            mi_rows: 0,
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
pub fn set_all_ref_frame_type(pic: &PicParams) -> ([i8; 31], u8) {
    let mut arr = [0i8; 31];
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

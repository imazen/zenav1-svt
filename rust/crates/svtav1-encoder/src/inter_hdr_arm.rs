//! The INTER frame header's picture-level fields, assembled from ports that
//! already exist.
//!
//! Everything here is glue: it takes the reference structure
//! ([`crate::port_picstruct`]) and the picture-level tool ladders
//! ([`crate::port_enc_mode_config::md_config::sig_deriv_mode_decision_config_default`],
//! the EXPORTED `svt_aom_sig_deriv_mode_decision_config_default`, gated at
//! tier 1) and produces the [`crate::entropy::obu::InterSignal`] the frame
//! header writer consumes. No C rule is transcribed a second time here — the
//! point is that the header reads its fields from the SAME derivations the
//! rest of the encoder uses, so the two can never disagree.
//!
//! # What is refused rather than guessed
//!
//! `use_ref_frame_mvs` at `mfmv_level >= 2` needs the TPL `r0` and the
//! REFERENCE pictures' own `is_mfmv_used` flags (`mfmv_controls`,
//! `enc_mode_config.c:8852-8896`), neither of which this pipeline produces.
//! Levels 0 and 1 are closed forms (0 and 1); anything else returns
//! [`InterHdrError::MfmvLevelNotDerivable`] instead of inventing a bit.

use crate::entropy::obu::InterSignal;
use crate::port_enc_mode_config::md_config::MdConfigSignals;
use crate::port_picstruct::{PicDecisionCtx, PicParams, REF_FRAMES, RefQueueEntry, SliceType};

/// A field of the inter frame header this port cannot derive yet.
///
/// Every variant is a REFUSAL, not a fallback: a wrong header bit shifts every
/// following field and produces an undecodable stream
/// (`docs/WORKING-ON-THIS.md` §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterHdrError {
    /// `mfmv_level` 2, 3 or 4: `use_ref_frame_mvs` depends on the TPL `r0`
    /// and on the references' `is_mfmv_used`.
    MfmvLevelNotDerivable(u8),
    /// A global-motion model was selected; `global_motion_params()`'s type and
    /// parameter coding is not implemented.
    GlobalMotionNotImplemented,
}

/// The SEQUENCE header tool bits the inter frame header's field PRESENCE
/// depends on (spec 5.9.2). They must be the same values the sequence header
/// actually wrote, which is why they are passed in rather than re-derived.
#[derive(Debug, Clone, Copy)]
pub struct SeqInterTools {
    /// SH `enable_order_hint`.
    pub enable_order_hint: bool,
    /// SH `enable_ref_frame_mvs` — with `error_resilient_mode`, gates whether
    /// `use_ref_frame_mvs` is written at all.
    pub enable_ref_frame_mvs: bool,
    /// SH `enable_warped_motion` — with `error_resilient_mode`, gates whether
    /// `allow_warped_motion` is written at all.
    pub enable_warped_motion: bool,
}

/// Assemble the inter frame header's fields.
///
/// `pic` must already have been through
/// [`crate::port_picstruct::picture_decision_per_picture`] (which fills
/// `rps.ref_dpb_index`, `rps.refresh_frame_mask` and `skip_mode`), and `sigs`
/// must be the SAME `MdConfigSignals` the encode used.
///
/// # Errors
///
/// [`InterHdrError`] for a field this port refuses to guess.
pub fn inter_signal(
    pic: &PicParams,
    sigs: &MdConfigSignals,
    primary_ref_frame: u8,
    order_hint_bits: u32,
    seq: SeqInterTools,
) -> Result<InterSignal, InterHdrError> {
    // C never sets `error_resilient_mode` on a coded picture
    // (`resource_coordination_process.c:418` writes 0; only the S-frame path
    // at `pd_process.c:1727` sets 1, and S-frames are outside this envelope).
    let error_resilient_mode = false;

    // use_ref_frame_mvs — `mfmv_controls` (enc_mode_config.c:8852).
    let use_ref_frame_mvs_value = match sigs.mfmv_level {
        0 => false,
        1 => true,
        other => return Err(InterHdrError::MfmvLevelNotDerivable(other)),
    };
    // ...and its PRESENCE — C `frame_might_allow_ref_frame_mvs`
    // (entropy_coding.h:71).
    let use_ref_frame_mvs =
        (!error_resilient_mode && seq.enable_ref_frame_mvs && seq.enable_order_hint)
            .then_some(use_ref_frame_mvs_value);

    // allow_warped_motion's PRESENCE — C `frame_might_allow_warped_motion`
    // (entropy_coding.h:77).
    let allow_warped_motion =
        (!error_resilient_mode && seq.enable_warped_motion).then_some(sigs.allow_warped_motion);

    // skip_mode_params() — `svt_av1_setup_skip_mode_allowed` already ran
    // inside `picture_decision_per_picture`. `skip_mode_flag` itself is left
    // at C's initialisation value (0, `resource_coordination_process.c:355`);
    // nothing in the encoder assigns it.
    let skip_mode_present = (pic.skip_mode.skip_mode_allowed != 0).then_some(false);

    let mut ref_frame_idx = [0u8; 7];
    for (dst, src) in ref_frame_idx.iter_mut().zip(pic.rps.ref_dpb_index.iter()) {
        *dst = *src;
    }

    // `reference_select` is `frm_hdr->reference_mode == REFERENCE_MODE_SELECT`
    // (entropy_coding.c:3616); `init_pic_settings` (pd_process.c:4912) gives a
    // non-I slice SINGLE_REFERENCE only on an incomplete mini-GOP.
    let reference_select = pic.reference_mode == crate::port_picstruct::ReferenceMode::Select;

    let order_hint_mask = (1u64 << order_hint_bits) - 1;
    let order_hint = u8::try_from(u64::from(pic.cur_order_hint) & order_hint_mask)
        .expect("order_hint_bits <= 8 in this envelope");

    Ok(InterSignal {
        error_resilient_mode,
        order_hint,
        primary_ref_frame,
        refresh_frame_flags: pic.rps.refresh_frame_mask,
        ref_frame_idx,
        allow_high_precision_mv: sigs.allow_high_precision_mv != 0,
        // `SWITCHABLE` is `BILINEAR + 1` == 4, NOT `SWITCHABLE_FILTERS` == 3
        // (`definitions.h:844-846`). Reusing the port's own constant rather
        // than a literal: spelling it 3 here made the header write
        // `is_filter_switchable = 0` followed by a 2-bit filter index C
        // never wrote, which shifted every field after bit 50.
        interpolation_filter: (sigs.interpolation_filter
            != crate::port_enc_mode_config::md_config::SWITCHABLE)
            .then_some(sigs.interpolation_filter),
        is_motion_mode_switchable: sigs.is_motion_mode_switchable,
        use_ref_frame_mvs,
        reference_select,
        skip_mode_present,
        allow_warped_motion,
        // Global motion is not searched on this path; C writes seven
        // `is_global = 0` bits when `gm_ctrls` produce no model.
        is_global: [false; 7],
    })
}

/// Build C's reference QUEUE out of the picture-decision shadow DPB.
///
/// `bind_refs_and_primary_ref_frame` resolves each reference POC through
/// `search_ref_in_ref_queue`, so it needs one entry per picture currently in a
/// DPB slot. C's queue is a separate list maintained by the picture manager;
/// the shadow DPB carries the same POCs and temporal layers, which are the only
/// two fields the `primary_ref_frame` rule reads.
///
/// `base_q_idx` / `slice_type` / `r0` are the reference-binding OUTPUTS
/// (`ref_base_q_idx[][]` etc.), not inputs to `primary_ref_frame`; they are
/// filled with the encode's own values so nothing downstream reads a sentinel.
#[must_use]
pub fn ref_queue_from_dpb(ctx: &PicDecisionCtx, base_q_idx: u8) -> alloc::vec::Vec<RefQueueEntry> {
    let mut out: alloc::vec::Vec<RefQueueEntry> = alloc::vec::Vec::with_capacity(REF_FRAMES);
    for slot in 0..REF_FRAMES {
        let e = ctx.dpb[slot];
        if out.iter().any(|q| q.picture_number == e.picture_number) {
            continue;
        }
        out.push(RefQueueEntry {
            picture_number: e.picture_number,
            is_valid: true,
            temporal_layer_index: e.temporal_layer_index,
            base_q_idx,
            slice_type: if e.picture_number == 0 {
                SliceType::I
            } else {
                SliceType::B
            },
            r0: 0.0,
        });
    }
    out
}

/// What the pipeline knows and `MdConfigInputs` needs.
///
/// A narrow struct rather than 38 positional arguments, and every field is
/// something `EncodePipeline` genuinely holds — the ones it does NOT hold are
/// resolved inside [`md_config_inputs`] with their provenance stated there.
#[derive(Debug, Clone, Copy)]
pub struct PipelineMdInputs {
    /// `pcs->enc_mode` — the preset.
    pub enc_mode: i8,
    /// `scs->static_config.qp` — the CLI QP, NOT the derived qindex.
    pub sq_qp: u32,
    /// `frm_hdr->quantization_params.base_q_idx`.
    pub base_q_idx: u8,
    /// `ppcs->picture_qp`.
    pub picture_qp: u32,
    /// `ppcs->temporal_layer_index`.
    pub temporal_layer_index: u8,
    /// `ppcs->hierarchical_levels`.
    pub hierarchical_levels: u8,
    /// `ppcs->is_ref`.
    pub is_ref: bool,
    /// `pcs->slice_type == I_SLICE`.
    pub is_islice: bool,
    /// `ppcs->sc_class5`.
    pub sc_class5: u8,
    /// `ppcs->input_resolution` / `scs->input_resolution` — equal here, since
    /// the port encodes one resolution per pipeline.
    pub input_resolution: crate::port_enc_mode_config::ResolutionRange,
    /// `scs->static_config.encoder_bit_depth`.
    pub encoder_bit_depth: u8,
    /// `scs->super_block_size`.
    pub super_block_size: u16,
    /// `scs->seq_header.enable_interintra_compound`.
    pub enable_interintra_compound: bool,
    /// `ppcs->frame_superres_enabled`.
    pub frame_superres_enabled: bool,
    /// `ppcs->ref_list0_count_try`.
    pub ref_list0_count_try: u32,
    /// `ppcs->ref_list1_count_try`.
    pub ref_list1_count_try: u32,
    /// The reference-picture slice types, list 0 / list 1 index 0. Only the
    /// slice type is needed: `get_ref_hp_percentage` returns its -1 sentinel
    /// for an I-slice reference regardless of the coded areas.
    pub ref_l0_is_islice: bool,
    /// See [`Self::ref_l0_is_islice`].
    pub ref_l1_is_islice: bool,
}

/// Build C's `MdConfigInputs` for an inter frame.
///
/// The fields the pipeline does not model are pinned to C's own sequence-level
/// derivations rather than invented:
///
/// * `mfmv_enabled` — `svt_aom_set_mfmv_config` (`enc_mode_config.c:10134`):
///   1 for `enc_mode <= ENC_M10` when not RTC, which is this envelope.
/// * `seq_qp_mod` — `enc_handle.c:3994` assigns a literal 2.
/// * `fast_decode`, `resize_mode`, `rc_stat_gen_pass_mode`,
///   `extended_crf_qindex_offset` — the encoder defaults (0), which is what
///   the C driver runs with.
/// * `tune` — 1 (PSNR), the driver's configuration.
/// * `transition_present`, `r0_gen`, `r0` — scene-transition and TPL state
///   this pipeline does not produce. They feed signals this header does not
///   read; `mfmv_level >= 2` is the one place `r0` would matter, and
///   [`inter_signal`] REFUSES there rather than trusting the placeholder.
/// * `ref_intra_percentage` / `ref_skip_percentage` — reference statistics
///   (`rc_process.c:96`) this pipeline does not accumulate. `ref_skip_percentage`
///   reaches `interpolation_search_level` only above `ENC_M8`, so this returns
///   `None` there rather than answering from a placeholder.
/// * `coeff_lvl` — `InputCoeffLvl::Normal`, C's value before the coefficient
///   analysis runs.
///
/// # Errors
///
/// `None` when a field the header reads would depend on a statistic this
/// pipeline does not compute.
#[must_use]
pub fn md_config_inputs(
    p: PipelineMdInputs,
) -> Option<crate::port_enc_mode_config::md_config::MdConfigInputs> {
    use crate::port_enc_mode_config::{InputCoeffLvl, md_config::MdConfigInputs};
    // NB: `port_rc_process` has its OWN `SliceType` — the two are distinct
    // types in this crate, and mixing them is a compile error rather than a
    // silent mismatch.
    use crate::port_rc_process::{RefObjStats, SliceType, get_ref_hp_percentage};

    // `interpolation_search_level` consults `ref_skip_percentage` only on the
    // `enc_mode > ENC_M8`, non-base arm (`enc_mode_config.c:9088-9096`).
    // M8 is enc_mode 8.
    if p.enc_mode > 8 && p.temporal_layer_index != 0 {
        return None;
    }
    let mk = |is_i: bool| RefObjStats {
        slice_type: if is_i { SliceType::I } else { SliceType::B },
        intra_coded_area: 0,
        skip_coded_area: 0,
        hp_coded_area: 0,
    };
    let l0 = mk(p.ref_l0_is_islice);
    let l1 = mk(p.ref_l1_is_islice);
    let ref_hp_percentage = get_ref_hp_percentage(
        if p.is_islice {
            SliceType::I
        } else {
            SliceType::B
        },
        u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX),
        Some(&l0),
        Some(&l1),
    );
    // A non-sentinel percentage would have come from `hp_coded_area`, which is
    // a placeholder here — so only the -1 "no usable reference" answer is
    // trustworthy, and anything else is refused.
    if ref_hp_percentage != -1 {
        return None;
    }
    Some(MdConfigInputs {
        enc_mode: p.enc_mode,
        is_ref: p.is_ref,
        temporal_layer_index: p.temporal_layer_index,
        input_resolution: p.input_resolution,
        is_islice: p.is_islice,
        sc_class5: p.sc_class5,
        fast_decode: 0,
        hierarchical_levels: u32::from(p.hierarchical_levels),
        transition_present: false,
        is_not_last_layer: p.temporal_layer_index != p.hierarchical_levels,
        sq_qp: p.sq_qp,
        mfmv_enabled: u8::from(p.enc_mode <= 10),
        error_resilient_mode: false,
        base_q_idx: i32::from(p.base_q_idx),
        ref_hp_percentage,
        scs_input_resolution: p.input_resolution,
        frame_is_intra: p.is_islice,
        frame_superres_enabled: p.frame_superres_enabled,
        frame_resize_enabled: false,
        seq_qp_mod: 2,
        resize_mode: 0,
        ref_intra_percentage: 0,
        rc_stat_gen_pass_mode: 0,
        ref_skip_percentage: 0,
        coeff_lvl: InputCoeffLvl::Normal,
        ref_list0_count_try: p.ref_list0_count_try,
        ref_list1_count_try: p.ref_list1_count_try,
        enable_interintra_compound: p.enable_interintra_compound,
        encoder_bit_depth: p.encoder_bit_depth,
        segmentation_enabled: false,
        super_block_size: p.super_block_size,
        hbd_md: 0,
        r0_gen: false,
        r0: 0.0,
        pcs_temporal_layer_index: p.temporal_layer_index,
        tune: 1,
        picture_qp: p.picture_qp,
        extended_crf_qindex_offset: 0,
    })
}

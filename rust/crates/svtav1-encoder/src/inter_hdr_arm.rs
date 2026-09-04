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
//! `use_ref_frame_mvs` is read from the SHARED port of C `mfmv_controls`
//! ([`crate::port_enc_mode_config::tail::mfmv_controls`], tier-1
//! C-parity-tested through the exported `sig_deriv_mode_decision_config_default`
//! shim), not re-derived here. That function's levels 2/3/4 arm sets
//! `r0_th = scs->tpl ? 0.1x : 0` and then tests `if (r0_th)`, so with TPL OFF
//! it leaves the bit at 0 and never reads `r0` or the references'
//! `is_mfmv_used` at all (`enc_mode_config.c:8853-8896`). This port's envelope
//! is TPL-less BY CONSTRUCTION — see [`scs_tpl`] — so levels 2/3/4 are as
//! closed a form as 0 and 1 here.
//!
//! What is still refused: those same levels with TPL ON, where `r0` and the
//! reference objects' `is_mfmv_used` really are needed and this pipeline
//! produces neither. That returns [`InterHdrError::MfmvLevelNotDerivable`]
//! instead of inventing a bit.

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
    /// `mfmv_level` 2, 3 or 4 **with TPL ON**: `use_ref_frame_mvs` then
    /// depends on the TPL `r0` and on the references' `is_mfmv_used`, neither
    /// of which this pipeline produces. With TPL off (every config this port
    /// can reach — see [`scs_tpl`]) C's own `r0_th` is 0 and the bit is a
    /// closed 0, so those levels are NOT refused.
    MfmvLevelNotDerivable(u8),
    /// A global-motion model was selected; `global_motion_params()`'s type and
    /// parameter coding is not implemented.
    ///
    /// C `svt_aom_derive_gm_level` (`enc_mode_config.c:194`) returns a
    /// non-zero level only on a NON-I-slice at `enc_mode <= ENC_M4`, so this
    /// fires exactly on an inter frame at preset <= 4. Above that C's own
    /// `gm_level` is 0 and every reference is IDENTITY, which is what the
    /// header and the MVP environment both write.
    GlobalMotionNotImplemented,
}

/// C `svt_aom_get_gm_core_level` (`enc_mode_config.c:180`).
///
/// `enc_mode <= ENC_MR` (-1) => 2, `<= ENC_M4` => 4, above => 0; and 0
/// unconditionally when superres is on. `svt_aom_derive_gm_level` (`:194`)
/// wraps it with "I_SLICE => 0".
#[must_use]
pub fn gm_core_level(preset: u8, super_res_off: bool) -> u8 {
    if !super_res_off {
        return 0;
    }
    // ENC_MR is -1 in C's EncMode; this port's `preset` is the 0..=13 CLI
    // domain, so MR is unreachable here and the `<= ENC_M4` arm is the live
    // one.
    if preset <= 4 { 4 } else { 0 }
}

/// C `get_tpl` (`Globals/enc_handle.c:3657`) — is TPL on for this sequence?
///
/// C disables TPL for all-intra, for `aq_mode == 0`, for `LOW_DELAY`, for a
/// non-auto superres mode and for reference scaling. This port refuses
/// `aq_mode != 0` outright (`EncodePipeline::knob_config_error`, issue #9 item
/// 8), so the second clause alone makes TPL **structurally off in every
/// configuration this encoder can encode**. The other clauses are ported
/// anyway so the answer stays right if that refusal is ever lifted.
///
/// This matters because it is what makes `mfmv_level >= 2` a closed form here:
/// `mfmv_controls` reads `r0_th = tpl ? 0.1x : 0` and skips its whole `r0` /
/// `is_mfmv_used` block when the threshold is 0.
#[must_use]
pub fn scs_tpl(allintra: bool, aq_mode: u8, low_delay: bool, superres_fixed: bool) -> bool {
    !(allintra || aq_mode == 0 || low_delay || superres_fixed)
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
/// `tpl` is C `scs->tpl` — pass [`scs_tpl`]'s answer for the pipeline's own
/// configuration, never a literal. It is the ONLY input `mfmv_controls` needs
/// beyond the level, because a false `tpl` zeroes C's `r0_th` and short-circuits
/// the `r0` / `is_mfmv_used` block entirely.
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
    tpl: bool,
    gm_level: u8,
) -> Result<InterSignal, InterHdrError> {
    // GLOBAL MOTION, refused here as well as at the pipeline's choke point.
    //
    // Two guards for one rule is deliberate and is this file's existing habit
    // ("Assert rather than assume", below): the pipeline's `gm_config_error`
    // is the friendly early refusal a caller sees, and this one makes
    // `GlobalMotionNotImplemented` a variant that can actually be constructed.
    // It could not before — it existed, a comment in `inter_syntax_state`
    // claimed this function raised it, and no code anywhere in the crate ever
    // did. `is_global: [false; 7]` below is only sound while `gm_level == 0`.
    if gm_level != 0 {
        return Err(InterHdrError::GlobalMotionNotImplemented);
    }
    // C never sets `error_resilient_mode` on a coded picture
    // (`resource_coordination_process.c:418` writes 0; only the S-frame path
    // at `pd_process.c:1727` sets 1, and S-frames are outside this envelope).
    let error_resilient_mode = false;

    // use_ref_frame_mvs — the SHARED port of C `mfmv_controls`
    // (enc_mode_config.c:8853), not a second transcription of it. The rule this
    // file used to carry inline ("levels 0 and 1 are closed forms, refuse the
    // rest") refused three levels that are equally closed once `tpl` is false:
    // C computes `r0_th = tpl ? 0.1x : 0` and guards the whole `r0` +
    // `is_mfmv_used` block behind `if (r0_th)`, so a TPL-less encode leaves the
    // bit at 0 without reading either. `mfmv_level == 2` is what preset <= M8
    // derives above 360p, which is why every cell at 568x568 and larger sat
    // refused.
    //
    // With TPL ON those inputs are real and unported, so the refusal stays —
    // it is now conditioned on the thing that actually makes them needed.
    if tpl && sigs.mfmv_level >= 2 {
        return Err(InterHdrError::MfmvLevelNotDerivable(sigs.mfmv_level));
    }
    let use_ref_frame_mvs_value = crate::port_enc_mode_config::tail::mfmv_controls(
        crate::port_enc_mode_config::tail::MfmvInputs {
            mfmv_level: sigs.mfmv_level,
            is_base: pic.temporal_layer_index == 0,
            tpl,
            // Inert while `tpl` is false (C's `if (r0_th)` is not taken); the
            // guard above refuses the only case where they would be read.
            r0_gen: false,
            r0: 0.0,
            is_b_slice: pic.slice_type == SliceType::B,
            ref_list1_count_try: u32::from(pic.ref_list1_count_try),
            ref_l0_is_mfmv_used: false,
            ref_l1_is_mfmv_used: false,
        },
    )
    .ok_or(InterHdrError::MfmvLevelNotDerivable(sigs.mfmv_level))?
        != 0;
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
    // inside `picture_decision_per_picture`.
    //
    // `skip_mode_flag` used to be `false` here, on the reading that it "is
    // left at C's initialisation value (0,
    // `resource_coordination_process.c:355`); nothing in the encoder assigns
    // it". **`pd_process.c:4958` assigns it**:
    //
    // ```c
    // frm_hdr->skip_mode_params.skip_mode_flag =
    //     frm_hdr->skip_mode_params.skip_mode_allowed;
    // ```
    //
    // The constant was right by accident on every frame this repo's gates
    // cover: `skip_mode_allowed` needs two references at DIFFERENT order
    // hints, and on the campaign's first inter frame every DPB slot still
    // holds the key frame. It becomes 1 at the SECOND inter frame, where C
    // then signals the header bit AND codes a `skip_mode` symbol on every
    // block whose `bsize` allows compound (`entropy_coding.c:5119`).
    // MEASURED by the every-frame `fctx_gate`: at poc 2 of
    // `diag 64x64 q40 p8 frames=3` C's `skip_mode` CDF has adapted to 138
    // while the port's is still the 147 default — proof C coded the symbol.
    // See `docs/INTER-ENCODE-PLAN.md` 1z30.
    let skip_mode_flag = pic.skip_mode.skip_mode_allowed != 0;
    let skip_mode_present = skip_mode_flag.then_some(skip_mode_flag);

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
#[derive(Debug, Clone)]
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
    /// The list-0 / list-1 index-0 reference objects' coded-area statistics,
    /// as `copy_statistics_to_ref_obj_ect` stored them (rest_process.c:190).
    /// `None` when that list has no reference.
    ///
    /// These USED to be a pair of `is_islice` bools with the three areas
    /// pinned to zero, and [`md_config_inputs`] refused any `ref_hp_percentage`
    /// other than the -1 "every reference was an I_SLICE" sentinel because of
    /// it. The port now carries the real values on its DPB entry
    /// (`crate::picture::ReferenceFrame::intra_coded_area`), verified against
    /// C's own `SVT_REFSTATS_OUT` — see
    /// `benchmarks/ref_coded_area_stats_2026-09-02.md`.
    pub ref_l0: Option<crate::port_rc_process::RefObjStats>,
    /// See [`Self::ref_l0`].
    pub ref_l1: Option<crate::port_rc_process::RefObjStats>,
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
/// The three reference-picture coded-area statistics
/// (`ref_{intra,skip,hp}_percentage`) are NO LONGER placeholders: the caller
/// supplies the DPB entries' real values and this derives all three through
/// the ported `rc_process.c:66/96/118` readers. The refusal that used to
/// stand here — "any `ref_hp_percentage` other than -1" — is gone with them.
/// * `coeff_lvl` — `InputCoeffLvl::Normal`, C's value before the coefficient
///   analysis runs.
///
/// # Errors
///
/// `None` when a field the header reads would depend on a statistic this
/// pipeline does not compute. ONE such refusal is left, and it is the
/// `enc_mode > ENC_M8 && !is_base` arm of `interpolation_search_level`: it
/// reads `ref_skip_percentage`, which this pipeline now HAS — the remaining
/// gap is `is_base`, i.e. `temporal_layer_index`, which is 0 on every flat
/// GOP this port builds and so cannot be exercised. Kept rather than lifted
/// because "the arm is unreachable here" is a claim about the envelope.
#[must_use]
pub fn md_config_inputs(
    p: PipelineMdInputs,
) -> Option<crate::port_enc_mode_config::md_config::MdConfigInputs> {
    use crate::port_enc_mode_config::{InputCoeffLvl, md_config::MdConfigInputs};
    // NB: `port_rc_process` has its OWN `SliceType` — the two are distinct
    // types in this crate, and mixing them is a compile error rather than a
    // silent mismatch.
    use crate::port_rc_process::{SliceType, get_ref_hp_percentage};

    // `interpolation_search_level` consults `ref_skip_percentage` only on the
    // `enc_mode > ENC_M8`, non-base arm (`enc_mode_config.c:9088-9096`).
    // M8 is enc_mode 8.
    if p.enc_mode > 8 && p.temporal_layer_index != 0 {
        return None;
    }
    let slice = if p.is_islice {
        SliceType::I
    } else {
        SliceType::B
    };
    let l1_count = u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX);
    let ref_hp_percentage =
        get_ref_hp_percentage(slice, l1_count, p.ref_l0.as_ref(), p.ref_l1.as_ref());
    let ref_intra_percentage = crate::port_rc_process::get_ref_intra_percentage(
        slice,
        l1_count,
        p.ref_l0.as_ref(),
        p.ref_l1.as_ref(),
    );
    let ref_skip_percentage = crate::port_rc_process::get_ref_skip_percentage(
        slice,
        l1_count,
        p.ref_l0.as_ref(),
        p.ref_l1.as_ref(),
    );
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
        ref_intra_percentage,
        rc_stat_gen_pass_mode: 0,
        ref_skip_percentage,
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

/// C `svt_aom_sig_deriv_enc_dec_default`'s `set_cand_reduction_ctrls` call
/// (`enc_mode_config.c:7826-7834`), with that function's own fixed arguments.
///
/// The REGULAR-PD1 arm passes `pcs->cand_reduction_level` straight through and
/// pins four of the eight remaining inputs to constants at the call site:
/// `me_8x8_cost_variance` and `me_64x64_distortion` are `(uint32_t)~0`,
/// `l0_was_skip` and `l1_was_skip` are 0 (`:7819-7821`), and `is_lpd1` is
/// false because `ctx->lpd1_ctrls.pd1_level` is `REGULAR_PD1` on this path.
/// Those four are read only by the level-4/5/6 `dc_only_th` and
/// `reduce_unipred_candidates` arms, which the default arm's
/// `cand_reduction_level` (0, 1 or 2 — `enc_mode_config.c:9039-9050`) never
/// reaches; they are reproduced anyway rather than dropped, because "the arm
/// is unreachable here" is a claim about the envelope and not about C.
///
/// `use_flat_ipp` is `scs->static_config.rtc && hierarchical_levels == 0` and
/// this port never sets `rtc`.
///
/// Returns `None` exactly when [`crate::port_enc_mode_config::encdec::
/// set_cand_reduction_ctrls`] does, i.e. on a level outside C's switch.
#[must_use]
pub fn enc_dec_cand_reduction(
    p: &PipelineMdInputs,
    cand_reduction_level: u8,
) -> Option<crate::port_enc_mode_config::encdec::CandReductionCtrls> {
    let ref_skip_percentage = ref_skip_percentage(p);
    crate::port_enc_mode_config::encdec::set_cand_reduction_ctrls(
        crate::port_enc_mode_config::encdec::CandReductionInputs {
            level: cand_reduction_level,
            is_lpd1: false,
            is_not_last_layer: p.temporal_layer_index != p.hierarchical_levels,
            use_flat_ipp: false,
            picture_qp: p.picture_qp,
            me_8x8_cost_variance: u32::MAX,
            me_64x64_distortion: u32::MAX,
            l0_was_skip: 0,
            l1_was_skip: 0,
            ref_skip_perc: ref_skip_percentage,
            ref_list0_count_try: p.ref_list0_count_try,
            ref_list1_count_try: p.ref_list1_count_try,
            use_best_me_unipred_cand_only: 0,
        },
    )
}

#[cfg(test)]
mod cand_reduction_tests {
    use super::*;

    /// A minimal inter-frame `PipelineMdInputs` at a given preset: the shape
    /// the campaign's 96-cell grid encodes (flat low-delay P, one list-0
    /// reference, base layer, 8-bit 4:2:0, 64px superblocks).
    fn inputs(enc_mode: i8) -> PipelineMdInputs {
        PipelineMdInputs {
            enc_mode,
            sq_qp: 40,
            base_q_idx: 160,
            picture_qp: 40,
            temporal_layer_index: 0,
            hierarchical_levels: 0,
            is_ref: true,
            is_islice: false,
            sc_class5: 0,
            input_resolution: crate::port_enc_mode_config::ResolutionRange::R240p,
            encoder_bit_depth: 8,
            super_block_size: 64,
            enable_interintra_compound: true,
            frame_superres_enabled: false,
            ref_list0_count_try: 1,
            ref_list1_count_try: 0,
            ref_l0: None,
            ref_l1: None,
        }
    }

    /// **The premise `inter_md_arm`'s header carried until 2026-09-03, and it
    /// was wrong.** It read "C caps the NEAR DRL loop to ZERO unless this
    /// control is enabled ... so `NEARMV` is absent exactly the way C makes
    /// it absent" — but `near_count_ctrls.enabled` is **1 in every arm** of
    /// `set_cand_reduction_ctrls` (`enc_mode_config.c:4113` onward), and the
    /// default arm's `pcs->cand_reduction_level` is 0, 1 or 2 (`:9039-9050`),
    /// all three of which carry `near_count = 3`.
    ///
    /// So on EVERY preset this port can express, C injects up to three
    /// `NEARMV` candidates per single reference. This test drives the real
    /// ladder — `md_config_inputs` -> `sig_deriv_mode_decision_config_default`
    /// -> `enc_dec_cand_reduction` — rather than asserting the table, so a
    /// change to any link in it fails here.
    #[test]
    fn the_near_drl_loop_is_live_at_every_preset_this_port_reaches() {
        let mut seen = 0usize;
        for preset in 0i8..=13 {
            let mi = inputs(preset);
            let Some(sigs) = md_config_inputs(mi.clone()).and_then(
                crate::port_enc_mode_config::md_config::sig_deriv_mode_decision_config_default,
            ) else {
                continue;
            };
            assert!(
                sigs.cand_reduction_level <= 2,
                "preset {preset}: the default arm's cand_reduction_level is 0/1/2 \
                 (enc_mode_config.c:9039-9050), got {}",
                sigs.cand_reduction_level
            );
            let cr = enc_dec_cand_reduction(&mi, sigs.cand_reduction_level)
                .expect("a level of 0..=2 is inside C's switch");
            assert_eq!(
                (
                    cr.near_count_ctrls.enabled,
                    cr.near_count_ctrls.near_count,
                    cr.near_count_ctrls.near_near_count
                ),
                (1, 3, 3),
                "preset {preset} (cand_reduction_level {}): C's NEAR DRL loop \
                 is capped to MIN(near_count, max_drl_index), NOT to zero",
                sigs.cand_reduction_level
            );
            seen += 1;
        }
        // Positive control: the loop above must actually have run. A ladder
        // that started refusing every preset would otherwise pass vacuously
        // — `docs/WORKING-ON-THIS.md` §5's "prove the probe fires".
        assert!(
            seen >= 9,
            "the preset ladder produced only {seen} inter configurations; \
             this test would have been vacuous"
        );
    }

    /// The one level that DOES cap the NEAR loop to zero is 6, which
    /// `sig_deriv_mode_decision_config_default` assigns only under
    /// `scs->rc_stat_gen_pass_mode` (`enc_mode_config.c:9052`) — a mode this
    /// port never runs. Pinned so the test above cannot be read as "the
    /// control is always 3"; it is 3 *on this envelope*.
    #[test]
    fn level_six_is_the_only_arm_that_caps_the_near_loop_to_zero() {
        let mi = inputs(6);
        for level in 0u8..=6 {
            let cr = enc_dec_cand_reduction(&mi, level).expect("levels 0..=6 are C's switch");
            assert_eq!(cr.near_count_ctrls.enabled, 1, "level {level}");
            assert_eq!(
                cr.near_count_ctrls.near_count == 0,
                level == 6,
                "level {level}: near_count {}",
                cr.near_count_ctrls.near_count
            );
        }
    }
}

/// C `pcs->ref_skip_percentage` (`rc_process.c:96`, through
/// [`crate::port_rc_process::get_ref_skip_percentage`]) for this picture.
///
/// Read at eight sites in `enc_mode_config.c` plus `md_config_process.c`'s
/// CDEF skip gate; hoisted here so the several consumers share ONE call with
/// ONE set of arguments rather than each re-spelling the slice/list-count pair.
#[must_use]
pub fn ref_skip_percentage(p: &PipelineMdInputs) -> u8 {
    use crate::port_rc_process::SliceType;
    let slice = if p.is_islice {
        SliceType::I
    } else {
        SliceType::B
    };
    crate::port_rc_process::get_ref_skip_percentage(
        slice,
        u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX),
        p.ref_l0.as_ref(),
        p.ref_l1.as_ref(),
    )
}

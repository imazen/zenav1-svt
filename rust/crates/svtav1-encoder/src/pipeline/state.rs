use super::*;

/// One random-access input frame held for its mini-GOP window.
///
/// Planes are tightly packed at TRUE dims (luma `true_width × true_height`,
/// chroma `⌈true_width/2⌉ × ⌈true_height/2⌉` for 4:2:0) — the same contract
/// `try_encode_frame_420` hands to `encode_frame_420_prepared`, which pads
/// per frame at encode time. `u`/`v` are empty on the monochrome arm.
pub(super) struct RaBufferedFrame {
    pub(super) y: Vec<u8>,
    pub(super) u: Vec<u8>,
    pub(super) v: Vec<u8>,
    /// C `pcs->picture_number` — display-order POC.
    pub(super) display_order: u64,
    /// C `pcs->input_ptr->pic_type == EB_AV1_KEY_PICTURE` — the drain stamps
    /// `pcs->idr_flag` from it (`pd_process.c:5418-5425`).
    pub(super) is_key: bool,
    /// C `pcs->end_of_sequence_flag` — the last input picture's flag, set by
    /// `try_flush`. Feeds `check_window_availability`'s `eos_reached` and
    /// `svt_aom_is_delayed_intra`'s not-delayed-at-EOS arm.
    pub(super) is_eos: bool,
}

/// C `ctx->prev_delayed_intra` (`pd_process.c:5103-5132`): an intra picture
/// that passed picture decision inside its own `[tail + intra]` release but
/// whose `store_gf_group`/`mctf_frame`/`send_picture_out` C defers to the
/// NEXT release's `process_pics`, because its `temp_filt_pcs_list` future
/// slots come from the pictures that follow it (`pd_process.c:3979-3995`).
/// Its packet lands ahead of that next release's — the release ordering C
/// produces at `pd_process.c:5129-5131`.
pub(super) struct DelayedIntra {
    pub(super) frame: RaBufferedFrame,
    /// `gather_tf_stats` for this frame — the centre histogram `calc_ahd`
    /// diffs each window member against.
    pub(super) stats: crate::port_preanalysis::PictureStatistics,
    /// The padded PA/TF buffer set, ready for `ra_mctf_filter` to filter in
    /// place; emitted through `encode_frame_impl` afterwards.
    pub(super) bufs: crate::port_tf_driver::TfPicBufs,
    /// The intra's picture decision — run inside its own release's passes
    /// like every other member, then held here.
    pub(super) pic: crate::port_picstruct::PicParams,
}

/// The TPL stage's per-window output: one [`crate::port_tpl::FrameTplIn`]
/// per emitted `ra_input` member plus one for the held key — C's
/// `pcs->r0`/`tpl_is_valid`/`tpl_group_size`/`tpl_ctrls` and
/// `pa_me_data->tpl_beta`/`tpl_rdmult_scaling_factors`/`me_results`, carried
/// across the stage boundary so `encode_frame_impl` consumes them without
/// re-deriving.
pub(super) struct TplStageOut {
    /// `FrameTplIn` per `ra_input` slot (`None` = this member ran no TPL —
    /// e.g. `tpl_ctrls.enable == 0` at its hierarchy level).
    pub(super) frames: Vec<Option<crate::port_tpl::FrameTplIn>>,
    /// The held key's `FrameTplIn` (`Some` when a delayed intra is staged
    /// and its group ran).
    pub(super) key: Option<crate::port_tpl::FrameTplIn>,
}

/// What the RA emit loop hands `encode_frame_impl` for one picture: the
/// picture-decision output (replacing the per-frame `run_picture_decision`
/// call the sequential path still makes) plus, when the TPL stage ran, the
/// `pa_me_data`-equivalent results C computes in `initial_rc_process` —
/// `r0`, the per-SB `tpl_beta` offsets, the pre-`sb_setup_lambda` rdmult
/// grid, and the picture's own open-loop ME results.
pub(super) struct FrameDecision {
    /// Display-order POC (the `pcs->picture_number` analogue).
    pub(super) display_order: u64,
    /// `PicParams` produced by `run_ra_picture_decision`.
    pub(super) pic: crate::port_picstruct::PicParams,
    /// This picture's TPL stage outputs; `None` on every path where C's
    /// `scs->tpl` is off (still, low delay, `aq_mode == 0`, tiny dims).
    pub(super) tpl: Option<crate::port_tpl::FrameTplIn>,
}

/// Encoder pipeline state.
pub struct EncodePipeline {
    /// Pinned source identity, independent of HDR mode. Constructors default
    /// to Mainline420 (pristine v4.2.0); Hybrid3115 is legacy (plan 3.7).
    pub reference: crate::reference::SvtReference,
    /// Explicit Zen experiments, separate from the reference identity.
    /// Empty by default. Nonempty settings do not claim C parity.
    pub enhancements: crate::enhancements::ZenEnhancements,
    /// SVT_HDR_MODE mirror and fork knobs, separate from the source identity.
    /// Defaults to Mainline = all fork behavior off; callers opt in with
    /// `pipe.hdr = HdrForkConfig::hdr_fork()` after construction.
    pub hdr: crate::hdr_mode::HdrForkConfig,
    /// C denoiser, supplied table, and INTER grain reuse controls.
    pub film_grain: crate::film_grain_config::FilmGrainConfig,
    /// `__expert`: fixed per-plane chroma delta-q that REPLACES the derived
    /// deltas on every frame with chroma planes (see
    /// [`crate::chroma_q::ChromaQOverride`]). `None`, the default, keeps the
    /// derived deltas and the bytes. No C counterpart, no C-parity claim; a
    /// monochrome frame with an override set is refused.
    #[cfg(feature = "__expert")]
    pub chroma_q_override: Option<crate::chroma_q::ChromaQOverride>,
    /// The `separate_uv_delta_q` the last key frame's sequence header
    /// signalled. Inter frames must agree with it: the SH is written on key
    /// frames only, so a mid-sequence override change could otherwise switch
    /// the FH chroma-q form under a SH that no longer matches.
    #[cfg(feature = "__expert")]
    pub(super) sh_separate_uv_delta_q: Option<bool>,
    pub(super) prepared_grain: Option<crate::entropy::obu::FilmGrainParams>,
    pub(super) grain_references: [Option<crate::entropy::obu::FilmGrainParams>; 8],
    pub(super) grain_sequence_present: Option<bool>,
    /// Speed configuration.
    pub speed_config: SpeedConfig,
    /// Rate control configuration.
    pub rc_config: RcConfig,
    /// Rate control state.
    pub rc_state: RcState,
    /// C `enc_ctx->rc`/`rc_cfg` + the `scs`/`frame_info` RC subset — the
    /// C-shaped rate-control state (`svt_aom_set_rc_param`,
    /// pass2_strategy.c:907) that drives VBR/CBR. Populated on the first
    /// frame of a VBR/CBR encode from `rc_config`; `None` under CQP/CRF,
    /// whose flow runs entirely off `rc_state`/`rc_config` exactly as before.
    /// See [`crate::port_rc_driver`].
    pub(super) rc_vbr_cbr: Option<crate::port_rc_driver::RcVbrCbr>,
    /// Decoded picture buffer.
    pub dpb: DecodedPictureBuffer,
    /// GOP structure.
    pub gop: GopStructure,
    /// `true` while the constructor's `hierarchical_levels` argument carried
    /// C's `HIERARCHICAL_LEVELS_AUTO` sentinel and the real level has not
    /// been resolved yet — resolution runs against the final
    /// `pred_structure` in [`Self::with_pred_structure`], or at the first
    /// [`Self::encode_frame_impl`] when no setter ever ran. See
    /// [`Self::resolve_hierarchical_levels_auto`].
    pub(super) hier_auto: bool,
    /// C `scs->static_config.pred_structure` — `LowDelay` (the default, and
    /// the only structure the sequential frame-in/frame-out contract can
    /// express directly) or `RandomAccess`, which buffers input frames into
    /// `ra_input` until a whole mini-GOP is present, runs the ported
    /// picture-decision kernel over the window, then encodes the pictures
    /// in DECODE order (C's `store_mg_picture_arrays` permutation),
    /// interleaving `show_existing_frame` OBUs at each hidden picture's
    /// display position. See [`Self::try_encode_frame_420_ra`].
    pub pred_structure: crate::port_picstruct::PredStructure,
    /// C `static_config.enable_tf` — whether motion-compensated temporal
    /// filtering may run at all. C's default is 1 (`enc_settings.c`), and
    /// `derive_tf_params` additionally requires RANDOM_ACCESS and
    /// `hierarchical_levels >= 1`, so this is inert on a low-delay pipeline.
    pub enable_tf: bool,
    /// C `static_config.enable_tf_key` — whether a key frame may be
    /// temporally filtered (`copy_tf_params`, `pd_process.c:4483`). C's
    /// default is 1.
    pub enable_tf_key: bool,
    /// Random-access input staging, in display order, planes tightly packed
    /// at TRUE dims (the encode pads per frame). Populated only while
    /// `pred_structure == RandomAccess`; emptied by
    /// [`Self::encode_ra_window`].
    pub(super) ra_input: alloc::vec::Vec<RaBufferedFrame>,
    /// The picture-analysis statistics `svt_aom_gathering_picture_statistics`
    /// produces per buffered input — region histograms and `avg_luma`,
    /// indexed exactly like `ra_input`. `None` per slot while `calc_hist` is
    /// off — in random access `calc_hist` is always 1
    /// (`vq_ctrls.sharpness_ctrls.scene_transition` is set in both
    /// `derive_vq_params` arms and only cleared for low delay / first pass,
    /// `enc_handle.c:3282-3326`), so every buffered input gathers.
    /// Filled at input alongside `ra_input`, drained with it.
    pub(super) ra_stats:
        alloc::vec::Vec<Option<alloc::boxed::Box<crate::port_preanalysis::PictureStatistics>>>,
    /// C `ctx->prev_delayed_intra` — the intra member whose TF window and
    /// packet C defers to the next release's `process_pics`
    /// (`pd_process.c:5103-5131`). Set when a `[tail + intra]` release
    /// drains: the intra member's `PicParams` is decided in-pass like every
    /// member, then moved here with its frame/stats until
    /// `filter_delayed_intra` runs.
    pub(super) delayed_intra: Option<DelayedIntra>,
    /// The filtered output of `delayed_intra`, staged for emit by
    /// `filter_delayed_intra` so `encode_ra_window` can emit it ahead of the
    /// release's member packets (`pd_process.c:5129`).
    pub(super) delayed_intra_out: Option<(
        crate::port_tf_driver::TfPicBufs,
        crate::port_picstruct::PicParams,
    )>,
    /// The delayed intra's TPL stage output — `run_tpl_stage` produces it
    /// from the `[key] + window` group while `delayed_intra_out` is still
    /// staged; `encode_delayed_intra` takes it so the intra's `initial_rc`
    /// results ride into its encode exactly like a window member's.
    pub(super) delayed_intra_tpl: Option<crate::port_tpl::FrameTplIn>,
    /// The display-order POC the next accepted input takes under random
    /// access. `frame_count` stays the count of CODED frames (it increments
    /// inside `encode_frame_impl`); under RA the two orders differ inside a
    /// mini-GOP, so a separate counter assigns input POCs.
    pub(super) ra_display_next: u64,
    /// Frame counter.
    pub frame_count: u64,
    /// ALIGNED (mi-grid) frame width — the true width rounded up to a
    /// multiple of `MIN_BLOCK_SIZE` (8). The whole encode (SB grid, mi
    /// grid, partition tree, tile geometry, frame header) runs on these
    /// dims. For a natively 8-aligned input `width == true_width`.
    /// Task #95 chunk 1 scopes this to inputs whose aligned dims are also
    /// a multiple of 64 (full SBs — no partial-SB edge coding yet).
    pub width: u32,
    /// ALIGNED (mi-grid) frame height (see [`Self::width`]).
    pub height: u32,
    /// TRUE / CODED frame width — the value the caller passed, carried to
    /// the sequence header (`max_frame_width_minus_1`, spec 5.5.1) and the
    /// recon output crop. Can differ from the aligned [`Self::width`] by
    /// up to 7 px. Equals `width` for 8-aligned inputs.
    pub true_width: u32,
    /// TRUE / CODED frame height (see [`Self::true_width`]).
    pub true_height: u32,
    /// Bit depth (8, 10, or 12).
    pub bit_depth: u8,
    /// UPSCALED frame width — what the sequence header advertises
    /// (`max_frame_width_minus_1`) and what a decoder outputs. Equals
    /// [`Self::true_width`] unless superres is on, in which case
    /// `true_width` is the reduced CODED width the whole encode runs at and
    /// this is the width the decoder normatively upscales back to
    /// (superres chunk B.3, `rust/docs/superres-port-map.md`).
    pub upscaled_width: u32,
    /// `SuperresDenom` in 9..=16 when superres is on, `None` (denominator 8,
    /// unscaled) otherwise. Off by default, exactly like C
    /// (`superres_mode = SUPERRES_NONE`, enc_settings.c:1095).
    pub superres_denom: Option<u8>,
    /// Emit non-still sequence/frame headers for an all-intra image sequence.
    /// This changes syntax, independently of the all-intra coding policy.
    pub(super) image_sequence: bool,
    /// Superres chunk B.3: the FULL-RESOLUTION luma the caller handed in,
    /// stashed by the downscale so the frame-level PICTURE STATISTICS can be
    /// derived from it.
    ///
    /// C computes those in `picture_analysis_process` and only scales the
    /// picture later, in `pd_process` (`svt_aom_init_resize_picture`,
    /// pd_process.c:4344) — so `pic_avg_variance` (and the screen-content
    /// derivation) see the ORIGINAL width, not the coded one. MEASURED: with
    /// the port deriving them from the downscaled source instead, a superres
    /// encode of textured content diverges from C late in the tile even though
    /// encoding the identical downscaled pixels WITHOUT superres is
    /// byte-identical (gradient 128x128 q32 p10 d16: 724B port vs 727B C with
    /// superres; both 724B on the same pixels without it). Taken at the head
    /// of `encode_frame_impl` like `hbd_source`, so it cannot leak.
    pub(super) superres_stats_luma: Option<(alloc::vec::Vec<u8>, usize, usize)>,
    /// Native 10-bit (u16) SOURCE planes for the NEXT frame — task #6 chunk 1.
    ///
    /// Set by [`Self::try_encode_frame_420_hbd`] / [`Self::try_encode_frame_hbd`]
    /// and TAKEN (not cloned) at the head of `encode_frame_impl`, so it can
    /// never leak into a following u8 frame. `None` on every u8 entry point,
    /// which is what keeps the whole u8 path — and every bd10-on-8-bit-source
    /// gate cell — byte-identical.
    ///
    /// Layout: ALIGNED frame, luma `aligned_w × aligned_h` at stride
    /// `aligned_w`, chroma `aligned_w/2 × aligned_h/2` at stride `aligned_w/2`
    /// (empty on the monochrome entry point). The bd10 consumers are all
    /// 64-aligned-gated, so this stride equals the funnel's SB-extended one.
    pub(super) hbd_source: Option<HbdSource>,
    /// Native 10-bit FULL-WIDTH source for the superres downscale — set only
    /// by [`Self::try_encode_frame_420_hbd`] when `superres_denom` is on.
    ///
    /// C's 10-bit resize (`svt_aom_resize_frame`, resize.c) packs the u16
    /// source, filters at full precision, and only THEN unpacks to the u8
    /// MSB plane: the u8 canvas is `filtered_u16 >> 2`, not
    /// `truncated_u8` filtered. So the bd10 arm of [`Self::superres_downscale_420`]
    /// cannot reuse the u8 input planes; it takes this staged u16 source,
    /// produces the coded-width u16 planes (which then become `hbd_source`
    /// for the frame's bd10 consumers) and the u8 canvas in one pass.
    /// `None` on every u8 and every non-superres path; taken (not cloned)
    /// inside the downscale so it cannot leak into a following frame.
    pub(super) hbd_superres_src: Option<HbdSuperresSrc>,
    /// The PREVIOUS frame's PA (picture-analysis) picture — its padded SOURCE
    /// luma at full, 1/4 and 1/16 resolution.
    ///
    /// SVT's motion estimation is OPEN LOOP: `me_process.c:185-203` searches
    /// against the PA reference, which `reference_object.c:242-250` documents
    /// as pointing directly at the app's luma input, NOT at a recon. So the
    /// search needs the previous SOURCE and the DPB's padded recon is a
    /// different buffer for a different job (motion compensation). `None`
    /// until the first frame has been encoded.
    pub(super) pa_ref: Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>,
    /// The PA pyramids the GLOBAL-MOTION search can reach, keyed by DPB slot
    /// exactly as `self.dpb` is.
    ///
    /// C's `pcs->pa_ref_pic_ptr_array[list][ref]` resolves EVERY reference in
    /// `ref_list<N>_count_try`, not just the nearest one, and
    /// `svt_aom_global_motion_estimation` fits a model per reference
    /// (`global_me.c:190`). The port used to hold a single previous-frame
    /// pyramid, so the second reference had no plane and the search refused —
    /// MEASURED: `gradient 72x72 q40 p2` encodes 2 frames and REFUSES frame 2,
    /// because that is the first frame whose `ref_list0_count_try` is 2.
    ///
    /// Populated only when the preset's `gm_level` is non-zero (presets 0..4;
    /// `derive_gm_level`). Above that nothing reads a PA plane other than the
    /// previous frame's, and retaining up to eight pyramids would be memory
    /// spent on a buffer with no reader — the same reasoning that keeps the
    /// pyramid out of a still encode entirely.
    ///
    /// `Arc`, not `Box`: a refresh mask names several slots for one picture
    /// and low-delay GOPs alias heavily, so the slots share one pyramid rather
    /// than copying it. Eviction reclaims the allocation into `pa_scratch`
    /// when the evicted pyramid was the last reference.
    pub(super) pa_slots: [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
    /// The PA pyramid two frames back, kept as a RECYCLABLE allocation rather
    /// than freed.
    ///
    /// C constructs its PA reference objects ONCE into a pool at
    /// `svt_av1_enc_init` (`svt_aom_pa_reference_object_ctor`,
    /// `reference_object.c`) and every picture checks one out of the pool;
    /// its per-frame heap cost for them is zero. The port allocated, zeroed
    /// and freed three padded planes per frame — `PaPicture::from_source`
    /// 9.54 M and `PaPlane::decimate` 2.96 M in
    /// `benchmarks/mem_heaptrack_2026-09-03.txt`, plus the matching
    /// `_xzm_free` / `calloc` / `__bzero` in the video-key CPU attribution's
    /// ALLOC and LIBC_MEM classes (2.69 ms + 2.04 ms against C's 0.000 and
    /// 0.489). This is the pool, sized one: the frame-before-last's pyramid,
    /// which nothing reads any more, is refilled in place instead.
    pub(super) pa_scratch: Option<alloc::boxed::Box<crate::inter_me_arm::PaPicture>>,
    /// The previous frame's open-loop ME result set, kept as a RECYCLABLE
    /// allocation. Same argument as [`Self::pa_scratch`]: C checks its
    /// `MeResults` out of a pool built once at `svt_av1_enc_init`, the port
    /// allocated three `Vec`s per b64 per frame
    /// (`inter_me::context::MeB64Output::new` 12.53 M over 6,144 calls in
    /// `benchmarks/mem_heaptrack_2026-09-03.txt`). Nothing reads it once the
    /// frame that produced it has been packed.
    pub(super) me_scratch: Option<crate::inter_me_arm::FrameMe>,
    /// CICP color description.
    pub color_description: crate::entropy::obu::ColorDescription,
    /// SH `chroma_sample_position` (spec 6.4.2: 0 = CSP_UNKNOWN, 1 =
    /// CSP_VERTICAL — chroma sited horizontally between luma samples,
    /// vertically on them, the MPEG-2/H.264 "left" siting; 2 =
    /// CSP_COLOCATED — on the top-left luma sample). C
    /// `static_config.chroma_sample_position`, written verbatim into the
    /// 4:2:0 color_config (entropy_coding.c:2743); default `EB_CSP_UNKNOWN`
    /// (enc_settings.c:1112). Pure signalling — the encode itself is
    /// siting-agnostic — so it changes only the two SH bits. Issue #9 item 5.
    pub chroma_sample_position: u8,
    /// Produce the decoder-exact reconstruction (`last_recon*`) for this
    /// pipeline. Off by default; see [`Self::with_recon_output`].
    pub(crate) recon_output: bool,
    /// Opt-in 4:2:0 chroma mode (default false = monochrome).
    ///
    /// When set, frames are encoded via [`Self::encode_frame_420`] with
    /// NumPlanes=3: the sequence header signals mono_chrome=0 (profile-0
    /// 4:2:0), every coded block carries a UV_DC chroma pair, and the
    /// partition search is clamped to min luma dim 8 so chroma blocks are
    /// exactly (w/2, h/2) >= 4x4 (sub-8x8 chroma-ref rules deferred).
    /// Still/key frames only.
    ///
    /// Superseded view: [`Self::chroma_format`]. `chroma_420` remains the
    /// "chroma planes present AND 4:2:0" predicate every existing gate
    /// checks — `self.chroma_format == Some(ChromaFormat::Yuv420)`. Any
    /// OTHER format must keep hitting those gates' refusals until its
    /// path is proven decoder-exact, which is the port's extension
    /// contract (C refuses non-420 outright: `verify_settings`,
    /// `Globals/enc_settings.c:470`).
    pub chroma_420: bool,
    /// The chroma subsampling format the pipeline is configured for, or
    /// `None` when chroma planes are not consumed at all. `Some(Yuv420)`
    /// is exactly `chroma_420 = true`; `with_chroma_420` writes both
    /// fields so existing callers are byte-unchanged. `Some(Yuv422)` /
    /// `Some(Yuv444)` are the Zen-extension arms — see
    /// [`Self::with_chroma_format`] for the parity contract.
    pub chroma_format: Option<svtav1_types::chroma::ChromaFormat>,
    /// Reconstruction of the most recently encoded frame (Y, U, V planes;
    /// U/V empty in mono mode). This is what a conforming decoder must
    /// reproduce BIT-EXACTLY — the recon-parity gate compares it against
    /// aomdec's output.
    ///
    /// **`None` unless [`Self::with_recon_output`] was set** (default off,
    /// matching the C reference, whose API also produces no reconstruction
    /// unless the caller asks — `SvtAv1EncApp -o recon`). Materialising it
    /// is not free: on a still frame with loop restoration off (preset >= 7)
    /// the deblock and CDEF *application* passes exist ONLY to produce it,
    /// and they cost 27-39 % of the encode. See `with_recon_output`.
    pub last_recon: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// The display order of the coded frame `last_recon`/`last_recon10_final`
    /// belongs to. Under `PredStructure::RandomAccess` the coded order differs
    /// from input order — a caller that consumed `last_recon` per input frame
    /// would alias it to the wrong picture, so the index travels with the
    /// buffer. Equals the input index under low-delay.
    pub last_recon_display_order: Option<u64>,
    /// Every coded frame's `(display_order, recon)` since the caller last
    /// drained it — the multi-frame counterpart of [`Self::last_recon`].
    /// Under `PredStructure::RandomAccess` one `try_encode_frame_420` call
    /// codes several frames (a whole mini-GOP), so the single `last_recon`
    /// slot silently drops all but the last; this queue loses none. Pushed
    /// only when [`Self::with_recon_output`] is set.
    pub recon_frames: alloc::collections::VecDeque<(u64, (Vec<u8>, Vec<u8>, Vec<u8>))>,
    /// The same reconstruction BEFORE the in-loop deblocking filter was
    /// applied (equals `last_recon` when the picked levels are all zero).
    /// Evidence/analysis aid: lets tools quantify what deblocking
    /// contributes (before/after PSNR) without re-deriving the unfiltered
    /// state. Cheap (one copy per frame) on a bring-up encoder.
    pub last_recon_unfiltered: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// The reconstruction after deblocking but BEFORE CDEF (equals
    /// `last_recon` when CDEF didn't fire) — evidence aid for CDEF's
    /// before/after contribution.
    pub last_recon_pre_cdef: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// bd10 u16 MD path (task #94): the true-10-bit LUMA recon produced by the
    /// re-encode pass (`bd10_reencode_luma`), pre-filter, w*h raster. `None` on
    /// the bd8 path. Diagnostic aid to compare the encoder's internal 10-bit
    /// recon against the decoder's prefilter output (self-consistency check).
    pub last_recon10_y: Option<Vec<u16>>,
    /// bd10 u16 MD path: the true-10-bit CHROMA recon from
    /// `bd10_reencode_chroma`, pre-filter, `(w/2)*(h/2)` rasters. Together
    /// with `last_recon10_y` this is the complete 10-bit post-MD canvas that
    /// the bd10 post-filter chain (deblock -> CDEF search -> LR search) runs
    /// on — C's 16-bit recon picture. `None` on bd8 and whenever the bd10
    /// re-encode was skipped (out-of-envelope tree / partial SB), in which
    /// case the port falls back to the u8 filter chain.
    pub last_recon10_uv: Option<(Vec<u16>, Vec<u16>)>,
    /// bd10: the FINAL 10-bit reconstruction — the post-MD canvas above with
    /// the whole in-loop chain applied (deblock -> CDEF -> loop restoration),
    /// `(Y, U, V)` at the ALIGNED strides (`w` luma, `w/2` chroma). This is
    /// what a conforming decoder outputs for a 10-bit stream, bit-exact, and
    /// the 10-bit twin of [`Self::last_recon`].
    ///
    /// Issue #13: before this existed the 10-bit canvas fed the LR SEARCH and
    /// then only the u8 chain received the apply, so no consumer could ever
    /// see the 10-bit pixels a decoder produces when Wiener is signalled.
    /// `None` unless [`Self::with_recon_output`] is set AND the frame produced
    /// a complete 10-bit recon (same condition as `last_recon10_y`).
    pub last_recon10_final: Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    /// CDEF evidence counters for the last encoded frame (non-vacuity
    /// reporting: how many pixels the signaled strengths actually touched).
    pub last_cdef_stats: crate::cdef::CdefStats,
    /// The CDEF strength set 0 actually signaled in the last frame header
    /// (`cdef_damping` / `cdef_y_strength[0]` / `cdef_uv_strength[0]`),
    /// `None` until a frame has been encoded. Evidence surface for gating
    /// WHICH arm of `svt_pick_cdef_from_qp` (enc_cdef.c:823) the pipeline
    /// selected — the packed strengths are fixed-width header fields, so a
    /// wrong arm changes no byte COUNT and is invisible to a length or
    /// "streams differ" check.
    pub last_cdef_signaled: Option<crate::cdef::CdefFrameParams>,
    /// Loop-restoration evidence for the last encoded frame: per-plane
    /// frame types (0 NONE / 1 WIENER) + the number of RUs that signaled
    /// wiener. Zeroed when the search does not run.
    pub last_lr_stats: ([u8; 3], usize),
    /// Selected restoration unit size; None when the restoration search did not run.
    pub last_lr_unit_size: Option<usize>,
    /// Requested `TileRowsLog2` (C `static_config.tile_rows` —
    /// EbSvtAv1Enc.h:607-611: "0 means no tiling, 1 means split into 2").
    /// Default 0 = single tile row (unchanged pre-task-#86 behavior).
    /// The actually-encoded value is [`crate::entropy::obu::
    /// resolve_tile_rows_log2`] of this against the frame dims — a
    /// too-large request degrades exactly like C instead of panicking.
    /// Pairs with [`Self::tile_cols_log2`].
    pub tile_rows_log2: u8,
    /// Requested `TileColsLog2` (C `static_config.tile_columns` —
    /// EbSvtAv1Enc.h:610-611, same log2 domain as the rows). Default 0 =
    /// single tile column. C validation caps it at 4
    /// (`enc_settings.c:377`) on top of the geometry clamp; a request
    /// beyond what the frame supports degrades exactly like C.
    pub tile_cols_log2: u8,
    /// SUPERBLOCK SIZE IN PIXELS — 64 or 128 (task #91). C derives this in
    /// `Globals/enc_handle.c:4071-4111`; the port replays that rule in
    /// [`crate::sb128_geom::derive_super_block_size`] at construction from
    /// the ALIGNED dims + preset, so it agrees with the C oracle without
    /// any harness flag (there is NO `super_block_size` field in
    /// `EbSvtAv1EncConfiguration` — C's value is purely derived).
    ///
    /// For every pre-existing gate cell this is 64 and nothing changes: the
    /// C rule forces 64 below 165,120 aligned luma samples (largest current
    /// cell is 256x256 = 65,536) AND for every allintra preset above M1.
    ///
    /// When the derivation asks for 128 but the SB128 encode path cannot
    /// yet code the cell, [`Self::sb128_fallback`] records it and this stays
    /// 64 — a clean, decodable (if non-matching) stream rather than a panic.
    ///
    /// RE-DERIVED at the top of `encode_frame_impl`: the C rule also reads
    /// `static_config.enable_variance_boost` (VB forces 64), which is only
    /// final once the tune overrides have run — they apply inside
    /// `encode_frame_impl`, mirroring `copy_api_from_app` ordering.
    pub sb_size: usize,
    /// Explicit SB-size override (`SVTAV1_SB` in the harness). `None` =
    /// derive from the C rule. Set to `Some(64)`/`Some(128)` to pin one —
    /// used by the anti-vacuity witness, which needs to force the port to
    /// the WRONG size on an SB128 cell and observe the divergence.
    pub sb_size_override: Option<usize>,
    /// What C's rule alone asked for, BEFORE the override and the
    /// capability fallback. Stored rather than recovered from `sb_size` +
    /// `sb128_fallback`: an explicit `Some(128)` is indistinguishable from
    /// a derived 128 once the override is applied. Keeping the derived value
    /// lets `with_sb_size(None)` restore the original grid. The current
    /// `sb128_encode_supported` returns true, so no capability fallback runs.
    pub derived_sb_size: usize,
    /// True when [`Self::sb_size`] was forced back to 64 because the C rule
    /// asked for 128 on a cell the SB128 encode path does not support yet.
    /// The emitted stream is valid and decodable but will NOT byte-match C.
    pub sb128_fallback: bool,
    /// Feature 4 — bounded threading: the maximum number of OS threads the
    /// tile-parallel encode may run at once. `0` (default) = auto
    /// (`std::thread::available_parallelism`). The value only bounds
    /// CONCURRENCY: every tile's result is reassembled in tile-index order,
    /// so the emitted bytes are IDENTICAL for any `thread_count`. Set via
    /// [`Self::with_thread_count`]. On a single-tile frame it is inert.
    pub thread_count: usize,
    /// Feature 1 — cooperative cancellation token, checked once at the entry
    /// of the fallible [`Self::try_encode_frame`] / [`Self::try_encode_frame_420`]
    /// methods. The default is a no-op (`Unstoppable`) that never stops, so
    /// the infallible `encode_frame*` methods are unaffected. Set via
    /// [`Self::with_stop`].
    pub stop: almost_enough::StopToken,
    /// C's picture-decision state (`PictureDecisionContext` — the shadow DPB,
    /// the layer-0/1 toggle rings and the reference-order-hint map), carried
    /// across frames so [`crate::port_picstruct::picture_decision_per_picture`]
    /// derives the SAME reference structure C does.
    ///
    /// Only touched on a MULTI-FRAME (GOP) encode; a still/key encode never
    /// reads or writes it, so every existing cell is byte-inert by
    /// construction.
    pub(super) pd_ctx: crate::port_picstruct::PicDecisionCtx,
    /// C `enc_ctx` pred-struct counters (`EncCtxPicParams`):
    /// `pred_struct_position` walks the low-delay entry table (the
    /// temporal-layer column of `PRED_STRUCT_TEMPORAL_LAYER`),
    /// `elapsed_non_cra_count` picks the "directly after an I slice" arm of
    /// `update_pred_struct_and_pic_type`, and `last_idr_picture` anchors
    /// `frame_offset`. On the flat GOP this never leaves position 0.
    pub(super) enc_pic: crate::port_picstruct::EncCtxPicParams,
    /// C `ctx->mini_gop_*` window map, carried for
    /// [`crate::port_picstruct::get_pic_idx_in_mg`]'s signature — its
    /// low-delay arm does not read it, but the ported function takes it.
    pub(super) mg_map: crate::port_picstruct::MiniGopMap,
    /// C `scs->mrp_ctrls` — the preset-derived multi-reference caps
    /// (`set_mrp_ctrl`, `enc_handle.c:3574`). Recomputed per frame in
    /// `run_picture_decision` from the live config, where it also feeds
    /// `SeqPicParams`; the frame-ME site reads it for `only_l_bwd` and the
    /// `safe_limit_*` pair.
    pub(super) mrp_ctrls: crate::port_picstruct::MrpCtrls,
}

/// Tighten a strided plane to `w * h` contiguous bytes.
///
/// The public entry points all take a LUMA STRIDE, and the TRUE != ALIGNED
/// path has always honoured it (`pad_plane_replicate` reads at `src_stride`).
/// The 8-ALIGNED pass-through did not: it was `y_plane[..w * h].to_vec()`,
/// which reinterprets a padded buffer as tightly packed and shears the image.
/// Nothing caught it because no gate ever passed `y_stride != width` — the
/// project's pixel-buffer rule ("any multi-row function handles a strided
/// row") was documented on the API and unenforced by measurement.
/// `tools/alignment_gate.sh` passes a POISONED padded stride, so it does now.
pub(super) fn gather_rows(
    src: &[u8],
    src_stride: usize,
    w: usize,
    h: usize,
) -> crate::EncodeResult<alloc::vec::Vec<u8>> {
    debug_assert!(src_stride >= w, "stride {src_stride} < width {w}");
    if src_stride == w {
        // The contiguous fast path stays a single copy — the strided branch is
        // an addition, never a cost on the packed case.
        return Ok(src[..w * h].to_vec());
    }
    let mut out = svtav1_types::try_vec![0u8; w * h]?;
    for r in 0..h {
        out[r * w..r * w + w].copy_from_slice(&src[r * src_stride..r * src_stride + w]);
    }
    Ok(out)
}

/// Edge-replicate a plane from a valid `sw x sh` region (read at
/// `src_stride`) up to `dw x dh` (tightly packed at stride `dw`). The
/// per-pixel `min`-clamp reproduces C `pad_input_picture`'s
/// replicate-last-column-then-last-row for a rectangular pad
/// (pic_operators.c:561-604). Requires `dw >= sw`, `dh >= sh`, `sw>=1`,
/// `sh>=1`.
pub(super) fn pad_plane_replicate(
    src: &[u8],
    src_stride: usize,
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> crate::EncodeResult<alloc::vec::Vec<u8>> {
    let mut out = svtav1_types::try_vec![0u8; dw * dh]?;
    // Interior columns are a verbatim row copy; only the replicated right
    // edge needs the clamp. `c.min(sw - 1)` for c < sw is just `c`.
    let cw = sw.min(dw);
    for r in 0..dh {
        let sr = r.min(sh - 1);
        let base = sr * src_stride;
        let orow = r * dw;
        out[orow..orow + cw].copy_from_slice(&src[base..base + cw]);
        out[orow + cw..orow + dw].fill(src[base + sw - 1]);
    }
    Ok(out)
}

/// u16 twin of [`pad_plane_replicate`] — the TRUE->ALIGNED edge replication
/// for a native 10-bit source plane (task #6 chunk 1). Same gather, same
/// clamp order, so a widened-u8 hbd plane pads to exactly the widening of the
/// u8 pad (which is what the chunk-1 equivalence gate proves end-to-end).
pub(super) fn pad_plane_replicate_u16(
    src: &[u16],
    src_stride: usize,
    sw: usize,
    sh: usize,
    dw: usize,
    dh: usize,
) -> crate::EncodeResult<alloc::vec::Vec<u16>> {
    let mut out = svtav1_types::try_vec![0u16; dw * dh]?;
    let cw = sw.min(dw);
    for r in 0..dh {
        let sr = r.min(sh - 1);
        let base = sr * src_stride;
        let orow = r * dw;
        out[orow..orow + cw].copy_from_slice(&src[base..base + cw]);
        out[orow + cw..orow + dw].fill(src[base + sw - 1]);
    }
    Ok(out)
}

/// Native 10-bit SOURCE planes for one frame, already padded TRUE->ALIGNED
/// (task #6 chunk 1). See [`EncodePipeline::hbd_source`] for the layout.
pub(super) struct HbdSource {
    pub(super) y: alloc::vec::Vec<u16>,
    /// Chroma planes; both empty on the monochrome entry point.
    pub(super) u: alloc::vec::Vec<u16>,
    pub(super) v: alloc::vec::Vec<u16>,
}

/// Native 10-bit FULL-WIDTH (upscaled) source planes, staged by
/// [`EncodePipeline::try_encode_frame_420_hbd`] for the superres downscale —
/// luma `upscaled_w × true_h`, chroma `upscaled_w/2 × true_h/2` (4:2:0
/// ceiling), each tightly packed at its own stride. See
/// [`EncodePipeline::hbd_superres_src`].
pub(super) struct HbdSuperresSrc {
    pub(super) y: alloc::vec::Vec<u16>,
    pub(super) u: alloc::vec::Vec<u16>,
    pub(super) v: alloc::vec::Vec<u16>,
}

// `scs->static_config.fast_decode`. The port has no fast-decode
// config; C's default is 0. Both dlf ladders take their first arm on
// `fast_decode <= 1`, so the dlf resolution is currently unread —
// it is passed faithfully so the fast-decode arm stays correct if that
// config ever lands.
pub(super) const DLF_FAST_DECODE: u8 = 0;

// `scs->seq_header.cdef_level` is 1 in this port (obu.rs writes
// `enable_cdef = 1` unconditionally) and there is no `--cdef-level`
// config, so both ladders take their derived arm.
pub(super) const SEQ_CDEF_LEVEL: u8 = 1;

// `scs->static_config.fast_decode` — the port carries no fast-decode
// config and C's default is 0, the same value as `DLF_FAST_DECODE`.
pub(super) const CDEF_FAST_DECODE: u8 = 0;

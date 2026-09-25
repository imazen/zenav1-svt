//! Encoding pipeline orchestrator — wires all stages together.
//!
//! Spec 00 (architecture.md): Full encoding pipeline orchestrator.
//!
//! This is the top-level encoding function that coordinates:
//! 1. Picture analysis (noise estimation, scene detection)
//! 2. Reference frame management (DPB, GOP structure)
//! 3. Motion estimation
//! 4. Mode decision + partition search
//! 5. Encoding loop (transform, quantize, entropy)
//! 6. Loop filtering (deblock, CDEF, restoration)
//! 7. Reconstruction and reference frame update
//! 8. Bitstream packetization (OBU output)

use alloc::vec;
use crate::picture::{DecodedPictureBuffer, GopStructure, PictureControlSet, ReferenceFrame};
use crate::rate_control::{RcConfig, RcState, assign_picture_qp, update_rc_state};
use crate::speed_config::SpeedConfig;
use crate::{EncodeError, EncodeResult};
mod bd10_reencode;
use bd10_reencode::{bd10_reencode_chroma, bd10_reencode_luma};

use alloc::vec::Vec;
// `StopToken::check` is a method of the `enough::Stop` trait; bring the trait
// into scope so the frame-entry cancellation check resolves.
use enough::Stop;

/// One random-access input frame held for its mini-GOP window.
///
/// Planes are tightly packed at TRUE dims (luma `true_width × true_height`,
/// chroma `⌈true_width/2⌉ × ⌈true_height/2⌉` for 4:2:0) — the same contract
/// `try_encode_frame_420` hands to `encode_frame_420_prepared`, which pads
/// per frame at encode time. `u`/`v` are empty on the monochrome arm.
struct RaBufferedFrame {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    /// C `pcs->picture_number` — display-order POC.
    display_order: u64,
    /// C `pcs->input_ptr->pic_type == EB_AV1_KEY_PICTURE` — the drain stamps
    /// `pcs->idr_flag` from it (`pd_process.c:5418-5425`).
    is_key: bool,
    /// C `pcs->end_of_sequence_flag` — the last input picture's flag, set by
    /// `try_flush`. Feeds `check_window_availability`'s `eos_reached` and
    /// `svt_aom_is_delayed_intra`'s not-delayed-at-EOS arm.
    is_eos: bool,
}

/// C `ctx->prev_delayed_intra` (`pd_process.c:5103-5132`): an intra picture
/// that passed picture decision inside its own `[tail + intra]` release but
/// whose `store_gf_group`/`mctf_frame`/`send_picture_out` C defers to the
/// NEXT release's `process_pics`, because its `temp_filt_pcs_list` future
/// slots come from the pictures that follow it (`pd_process.c:3979-3995`).
/// Its packet lands ahead of that next release's — the release ordering C
/// produces at `pd_process.c:5129-5131`.
struct DelayedIntra {
    frame: RaBufferedFrame,
    /// `gather_tf_stats` for this frame — the centre histogram `calc_ahd`
    /// diffs each window member against.
    stats: crate::port_preanalysis::PictureStatistics,
    /// The padded PA/TF buffer set, ready for `ra_mctf_filter` to filter in
    /// place; emitted through `encode_frame_impl` afterwards.
    bufs: crate::port_tf_driver::TfPicBufs,
    /// The intra's picture decision — run inside its own release's passes
    /// like every other member, then held here.
    pic: crate::port_picstruct::PicParams,
}

/// The TPL stage's per-window output: one [`crate::port_tpl::FrameTplIn`]
/// per emitted `ra_input` member plus one for the held key — C's
/// `pcs->r0`/`tpl_is_valid`/`tpl_group_size`/`tpl_ctrls` and
/// `pa_me_data->tpl_beta`/`tpl_rdmult_scaling_factors`/`me_results`, carried
/// across the stage boundary so `encode_frame_impl` consumes them without
/// re-deriving.
struct TplStageOut {
    /// `FrameTplIn` per `ra_input` slot (`None` = this member ran no TPL —
    /// e.g. `tpl_ctrls.enable == 0` at its hierarchy level).
    frames: Vec<Option<crate::port_tpl::FrameTplIn>>,
    /// The held key's `FrameTplIn` (`Some` when a delayed intra is staged
    /// and its group ran).
    key: Option<crate::port_tpl::FrameTplIn>,
}

/// What the RA emit loop hands `encode_frame_impl` for one picture: the
/// picture-decision output (replacing the per-frame `run_picture_decision`
/// call the sequential path still makes) plus, when the TPL stage ran, the
/// `pa_me_data`-equivalent results C computes in `initial_rc_process` —
/// `r0`, the per-SB `tpl_beta` offsets, the pre-`sb_setup_lambda` rdmult
/// grid, and the picture's own open-loop ME results.
struct FrameDecision {
    /// Display-order POC (the `pcs->picture_number` analogue).
    display_order: u64,
    /// `PicParams` produced by `run_ra_picture_decision`.
    pic: crate::port_picstruct::PicParams,
    /// This picture's TPL stage outputs; `None` on every path where C's
    /// `scs->tpl` is off (still, low delay, `aq_mode == 0`, tiny dims).
    tpl: Option<crate::port_tpl::FrameTplIn>,
}

/// Encoder pipeline state.
pub struct EncodePipeline {
    /// Pinned source identity, independent of HDR mode. Legacy constructors
    /// retain Hybrid3115; select Mainline420 for pristine chroma ranking.
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
    sh_separate_uv_delta_q: Option<bool>,
    prepared_grain: Option<crate::entropy::obu::FilmGrainParams>,
    grain_references: [Option<crate::entropy::obu::FilmGrainParams>; 8],
    grain_sequence_present: Option<bool>,
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
    rc_vbr_cbr: Option<crate::port_rc_driver::RcVbrCbr>,
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
    hier_auto: bool,
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
    ra_input: alloc::vec::Vec<RaBufferedFrame>,
    /// The picture-analysis statistics `svt_aom_gathering_picture_statistics`
    /// produces per buffered input — region histograms and `avg_luma`,
    /// indexed exactly like `ra_input`. `None` per slot while `calc_hist` is
    /// off — in random access `calc_hist` is always 1
    /// (`vq_ctrls.sharpness_ctrls.scene_transition` is set in both
    /// `derive_vq_params` arms and only cleared for low delay / first pass,
    /// `enc_handle.c:3282-3326`), so every buffered input gathers.
    /// Filled at input alongside `ra_input`, drained with it.
    ra_stats:
        alloc::vec::Vec<Option<alloc::boxed::Box<crate::port_preanalysis::PictureStatistics>>>,
    /// C `ctx->prev_delayed_intra` — the intra member whose TF window and
    /// packet C defers to the next release's `process_pics`
    /// (`pd_process.c:5103-5131`). Set when a `[tail + intra]` release
    /// drains: the intra member's `PicParams` is decided in-pass like every
    /// member, then moved here with its frame/stats until
    /// `filter_delayed_intra` runs.
    delayed_intra: Option<DelayedIntra>,
    /// The filtered output of `delayed_intra`, staged for emit by
    /// `filter_delayed_intra` so `encode_ra_window` can emit it ahead of the
    /// release's member packets (`pd_process.c:5129`).
    delayed_intra_out: Option<(
        crate::port_tf_driver::TfPicBufs,
        crate::port_picstruct::PicParams,
    )>,
    /// The delayed intra's TPL stage output — `run_tpl_stage` produces it
    /// from the `[key] + window` group while `delayed_intra_out` is still
    /// staged; `encode_delayed_intra` takes it so the intra's `initial_rc`
    /// results ride into its encode exactly like a window member's.
    delayed_intra_tpl: Option<crate::port_tpl::FrameTplIn>,
    /// The display-order POC the next accepted input takes under random
    /// access. `frame_count` stays the count of CODED frames (it increments
    /// inside `encode_frame_impl`); under RA the two orders differ inside a
    /// mini-GOP, so a separate counter assigns input POCs.
    ra_display_next: u64,
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
    image_sequence: bool,
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
    superres_stats_luma: Option<(alloc::vec::Vec<u8>, usize, usize)>,
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
    hbd_source: Option<HbdSource>,
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
    hbd_superres_src: Option<HbdSuperresSrc>,
    /// The PREVIOUS frame's PA (picture-analysis) picture — its padded SOURCE
    /// luma at full, 1/4 and 1/16 resolution.
    ///
    /// SVT's motion estimation is OPEN LOOP: `me_process.c:185-203` searches
    /// against the PA reference, which `reference_object.c:242-250` documents
    /// as pointing directly at the app's luma input, NOT at a recon. So the
    /// search needs the previous SOURCE and the DPB's padded recon is a
    /// different buffer for a different job (motion compensation). `None`
    /// until the first frame has been encoded.
    pa_ref: Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>,
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
    pa_slots: [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
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
    pa_scratch: Option<alloc::boxed::Box<crate::inter_me_arm::PaPicture>>,
    /// The previous frame's open-loop ME result set, kept as a RECYCLABLE
    /// allocation. Same argument as [`Self::pa_scratch`]: C checks its
    /// `MeResults` out of a pool built once at `svt_av1_enc_init`, the port
    /// allocated three `Vec`s per b64 per frame
    /// (`inter_me::context::MeB64Output::new` 12.53 M over 6,144 calls in
    /// `benchmarks/mem_heaptrack_2026-09-03.txt`). Nothing reads it once the
    /// frame that produced it has been packed.
    me_scratch: Option<crate::inter_me_arm::FrameMe>,
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
    pd_ctx: crate::port_picstruct::PicDecisionCtx,
    /// C `enc_ctx` pred-struct counters (`EncCtxPicParams`):
    /// `pred_struct_position` walks the low-delay entry table (the
    /// temporal-layer column of `PRED_STRUCT_TEMPORAL_LAYER`),
    /// `elapsed_non_cra_count` picks the "directly after an I slice" arm of
    /// `update_pred_struct_and_pic_type`, and `last_idr_picture` anchors
    /// `frame_offset`. On the flat GOP this never leaves position 0.
    enc_pic: crate::port_picstruct::EncCtxPicParams,
    /// C `ctx->mini_gop_*` window map, carried for
    /// [`crate::port_picstruct::get_pic_idx_in_mg`]'s signature — its
    /// low-delay arm does not read it, but the ported function takes it.
    mg_map: crate::port_picstruct::MiniGopMap,
    /// C `scs->mrp_ctrls` — the preset-derived multi-reference caps
    /// (`set_mrp_ctrl`, `enc_handle.c:3574`). Recomputed per frame in
    /// `run_picture_decision` from the live config, where it also feeds
    /// `SeqPicParams`; the frame-ME site reads it for `only_l_bwd` and the
    /// `safe_limit_*` pair.
    mrp_ctrls: crate::port_picstruct::MrpCtrls,
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
fn gather_rows(
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
fn pad_plane_replicate(
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
fn pad_plane_replicate_u16(
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
struct HbdSource {
    y: alloc::vec::Vec<u16>,
    /// Chroma planes; both empty on the monochrome entry point.
    u: alloc::vec::Vec<u16>,
    v: alloc::vec::Vec<u16>,
}

/// Native 10-bit FULL-WIDTH (upscaled) source planes, staged by
/// [`EncodePipeline::try_encode_frame_420_hbd`] for the superres downscale —
/// luma `upscaled_w × true_h`, chroma `upscaled_w/2 × true_h/2` (4:2:0
/// ceiling), each tightly packed at its own stride. See
/// [`EncodePipeline::hbd_superres_src`].
struct HbdSuperresSrc {
    y: alloc::vec::Vec<u16>,
    u: alloc::vec::Vec<u16>,
    v: alloc::vec::Vec<u16>,
}

impl EncodePipeline {
    /// Create a new encoding pipeline.
    pub fn new(
        width: u32,
        height: u32,
        preset: u8,
        rc_config: RcConfig,
        hierarchical_levels: u8,
        intra_period: u32,
    ) -> Self {
        Self::new_with_preset(
            width,
            height,
            crate::speed_config::NativePreset::new(preset.min(13) as i8).unwrap(),
            rc_config,
            hierarchical_levels,
            intra_period,
        )
    }

    /// Construct with a checked native C preset, including research mode -1.
    ///
    /// Research-mode translation is in progress; accepting -1 is not a parity
    /// guarantee. See `docs/research-preset-port-map.md` for missing consumers.
    ///
    /// `hierarchical_levels` mirrors C's `cfg.hierarchical_levels`: an
    /// explicit level (the envelope C supports is 0-5, refused above by
    /// [`Self::gop_config_error`]) or
    /// [`crate::port_picstruct::HIERARCHICAL_LEVELS_AUTO`], the API default
    /// C ships — resolved in-library by
    /// [`Self::resolve_hierarchical_levels_auto`].
    pub fn new_with_preset(
        width: u32,
        height: u32,
        preset: crate::speed_config::NativePreset,
        rc_config: RcConfig,
        hierarchical_levels: u8,
        intra_period: u32,
    ) -> Self {
        let preset = preset.value();
        // C `hierarchical_levels == HIERARCHICAL_LEVELS_AUTO` survives as a
        // field until `enc_handle.c:4556` resolves it; here `gop`/`mg_map`
        // need a literal immediately, so the sentinel parks a provisional
        // flat structure until [`Self::resolve_hierarchical_levels_auto`]
        // swaps in the resolved one — before any consumer can read it.
        let hier_auto =
            hierarchical_levels == crate::port_picstruct::HIERARCHICAL_LEVELS_AUTO;
        let hierarchical_levels = if hier_auto { 0 } else { hierarchical_levels };
        // TWO boundary systems (frame_geom::FrameDims): the caller passes
        // TRUE dims; the encode runs on ALIGNED (8-rounded) dims. The
        // full-SB (aligned % 64 == 0) scope constraint is enforced on the
        // 4:2:0 pad path ([`Self::encode_frame_420`]) where it matters —
        // NOT here, so the monochrome state-tracking path keeps working at
        // its historical sub-64 dims (aligned == true, no padding).
        let dims = crate::frame_geom::FrameDims::new(width as usize, height as usize);
        // Task #91: replay C's `super_block_size` derivation
        // (Globals/enc_handle.c:4071-4111) on the ALIGNED dims — C
        // classifies resolution on `max_input_luma_width/height` AFTER the
        // 8-pad fold (enc_handle.c:3920, verified empirically). `allintra`
        // mirrors the identity oracle, which passes `avif = true`
        // (capture_c_trace.c) and therefore always lands in C's allintra
        // branch; the port's still/key pipeline is the same shape.
        let sb_inputs = crate::sb128_geom::SbSizeInputs {
            qp: rc_config.qp,
            allintra: intra_period == 1,
            ..Default::default()
        };
        let derived_sb = crate::sb128_geom::derive_super_block_size(
            dims.aligned_w,
            dims.aligned_h,
            preset as i8,
            &sb_inputs,
        );
        let (sb_size, sb128_fallback) = Self::resolve_sb_size(derived_sb, None, preset);
        Self {
            hdr: crate::hdr_mode::HdrForkConfig::default(),
            reference: crate::reference::SvtReference::Hybrid3115,
            enhancements: crate::enhancements::ZenEnhancements::default(),
            film_grain: Default::default(),
            #[cfg(feature = "__expert")]
            chroma_q_override: None,
            #[cfg(feature = "__expert")]
            sh_separate_uv_delta_q: None,
            prepared_grain: None,
            grain_references: core::array::from_fn(|_| None),
            grain_sequence_present: None,
            pd_ctx: crate::port_picstruct::PicDecisionCtx::new(),
            enc_pic: crate::port_picstruct::EncCtxPicParams::default(),
            mg_map: crate::port_picstruct::MiniGopMap::for_sequence(hierarchical_levels),
            mrp_ctrls: crate::port_picstruct::MrpCtrls::default(),
            speed_config: SpeedConfig::from_native_preset(
                crate::speed_config::NativePreset::new(preset).unwrap(),
            ),
            rc_config,
            rc_state: RcState::default(),
            rc_vbr_cbr: None,
            enable_tf: true,
            enable_tf_key: true,
            dpb: DecodedPictureBuffer::new(),
            gop: GopStructure::new(hierarchical_levels, intra_period),
            hier_auto,
            pred_structure: crate::port_picstruct::PredStructure::LowDelay,
            ra_input: alloc::vec::Vec::new(),
            ra_stats: alloc::vec::Vec::new(),
            delayed_intra: None,
            delayed_intra_out: None,
            delayed_intra_tpl: None,
            ra_display_next: 0,
            frame_count: 0,
            width: dims.aligned_w as u32,
            height: dims.aligned_h as u32,
            true_width: width,
            true_height: height,
            bit_depth: 8,
            upscaled_width: width,
            superres_denom: None,
            image_sequence: false,
            superres_stats_luma: None,
            hbd_source: None,
            hbd_superres_src: None,
            pa_ref: None,
            pa_slots: [const { None }; 8],
            pa_scratch: None,
            me_scratch: None,
            // C-matched default: CICP "unspecified" (cp/tc/mc = 2/2/2,
            // studio range) — the library defaults of enc_settings.c:1043.
            // The SH then carries color_description_present_flag=0 and
            // color_range=0, byte-matching C at matched configs. Callers
            // that know their color space (AVIF path) override via
            // with_color_description.
            color_description: crate::entropy::obu::ColorDescription::default(),
            chroma_sample_position: 0,
            chroma_420: false,
            chroma_format: None,
            recon_output: false,
            last_recon: None,
            last_recon_display_order: None,
            recon_frames: alloc::collections::VecDeque::new(),
            last_recon_unfiltered: None,
            last_recon_pre_cdef: None,
            last_recon10_y: None,
            last_recon10_uv: None,
            last_recon10_final: None,
            last_cdef_stats: crate::cdef::CdefStats::default(),
            last_cdef_signaled: None,
            last_lr_stats: ([0; 3], 0),
            last_lr_unit_size: None,
            tile_rows_log2: 0,
            tile_cols_log2: 0,
            sb_size,
            sb_size_override: None,
            derived_sb_size: derived_sb,
            sb128_fallback,
            // Feature 4: auto by default (byte-inert regardless of value).
            thread_count: 0,
            // Feature 1: no-op token (never stops) — zero-cost `None` variant.
            stop: almost_enough::StopToken::new(enough::Unstoppable),
        }
    }

    /// Whether the SB128 encode path can code a frame at this preset.
    ///
    /// SB128 is a whole second geometry (128 partition root with the
    /// 8-symbol alphabet, the b64<->sb stat bridges, the CDEF 4-quadrant
    /// contract, per-128-region CDF seeding — see docs/sb128-port-map.md).
    /// Until every one of those lands, a cell C would code at 128 is coded
    /// at 64 instead: a valid, decodable stream that does NOT byte-match.
    /// That is deliberate — the alternative is a panic or an undecodable
    /// stream, both worse. `sb128_fallback` reports when it happened.
    ///
    /// Flipped per-capability as the chunks land.
    ///
    /// LANDED (task #91 chunk 3): the 128 partition ROOT. The SB is walked
    /// as its b64 coding units in Z-order and coded as a `PARTITION_SPLIT`
    /// at the 128 square (8-symbol alphabet, ctx 16..19) — see
    /// `merge_sb_units` and `sb128_geom::sb_coding_units`. Everything below
    /// the root is the byte-proven per-64 path.
    ///
    /// STILL UNPORTED (the supported path below uses forced SPLIT):
    /// a genuine 128-level NONE/HORZ/VERT RD search (this path is
    /// forced-SPLIT), the b64<->sb stat bridges (`get_sb128_variance` /
    /// `get_sb128_me_data`), and the CDEF 4-quadrant three-phase contract.
    fn sb128_encode_supported(preset: i8) -> bool {
        // All presets are admitted; no content gate is applied here.
        // Presets 0/1 are the only ones C ever codes at 128 in
        // allintra (`derive_super_block_size`), so anything else reaching
        // here is an `SVTAV1_SB=128` override — honour it, the walk is
        // preset-agnostic.
        let _ = preset;
        true
    }

    /// Apply the override + capability gate to a derived SB size.
    /// Returns `(sb_size, fell_back)`.
    /// Run C's picture decision for one picture and return its
    /// [`crate::port_picstruct::PicParams`].
    ///
    /// This is the port's only caller of `av1_generate_rps_info`; everything
    /// the INTER frame header says about references comes from here, so the
    /// header and the DPB can never describe different structures.
    ///
    /// # Errors
    ///
    /// [`EncodeError::UnsupportedConfig`] for a prediction-structure branch
    /// [`crate::port_picstruct::generate_rps_info`] does not translate. That
    /// is a refusal, not a fallback: an invented reference row would put
    /// `ref_frame_idx[]` in the header pointing at the wrong DPB slots.
    fn run_picture_decision(
        &mut self,
        display_order: u64,
        is_key: bool,
    ) -> EncodeResult<crate::port_picstruct::PicParams> {
        use crate::port_picstruct as pp;

        let hier = self.gop.hierarchical_levels;
        // C `set_mrp_ctrl` (`enc_handle.c:3574`) — the preset-derived MRP
        // caps. NOT inert even on a two-frame cell: `set_ref_list_counts`
        // counts list 1 as 1 whenever its BWD-duplicate guard skips the
        // comparisons, and at M10/low-delay/8-bit the level-0 row's zero
        // list-1 caps are the only thing that makes `ref_list1_count_try`
        // 0 — which is what puts ME on C's single-reference candidate path
        // and keeps `me_distortion` L0-only. MEASURED 2026-09-14 on the
        // `gradient 104x104 q32 p10` cell: the neutral defaults let list 1
        // search, found a perfect (-3,0) match in two of four b64s, and the
        // resulting `norm_me_dist` halved C's, shifting the picture PD0
        // level 7->6 and demoting one SB to level 4 (29B vs C's 417B).
        let mrp_ctrls = pp::set_mrp_ctrl(
            self.speed_config.preset,
            false,
            hier,
            pp::PredStructure::LowDelay,
            self.bit_depth == 8,
            self.rc_config.mode.into(),
        );
        self.mrp_ctrls = mrp_ctrls;
        let (tf_level, tf_params_per_type) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            hier,
            self.enable_tf,
            /*lossless=*/ false,
        );
        let seq = pp::SeqPicParams {
            // The campaign's GOP: low-delay P, CQP/CRF. C's driver is given
            // `SVT_PRED_STRUCT=1` (LOW_DELAY) for the same cells. Under RA a
            // mid-stream key reaches this path via the drain release, where
            // `scs->static_config.pred_structure` stays RANDOM_ACCESS — so
            // this is `self.pred_structure`, not a literal. The RC mode is
            // the caller's: under CBR `av1_generate_rps_info` takes the
            // lay0/lay1-toggle L0-only arm (pd_process.c:2066) instead of the
            // CQP/CRF 7-slot rotation — visible in `ref_frame_idx[4..6]`.
            pred_structure: self.pred_structure,
            rate_control_mode: self.rc_config.mode.into(),
            rtc: false,
            allintra: false,
            mrp_ctrls,
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
            enable_tf_key: self.enable_tf_key,
            tf_level,
            tf_params_per_type,
        };
        let mini_gop = 1u32 << hier;
        self.pd_ctx.mini_gop_length[0] = mini_gop;
        // C `ctx->list0_only` is a persistent `PictureDecisionContext` field
        // written ONLY inside `initialize_mini_gop_activity_array`
        // (`pd_process.c:846-848`), which runs only when the pre-assignment
        // buffer holds more than one picture or a non-intra RA buffer
        // (`:4756-4758`). Low delay releases one picture at a time, so the
        // init never runs and the field keeps its ctor value 0 —
        // `send_picture_out`'s tl0 list-1 clamp (`:4950-4952`) never fires
        // for low delay and `ref_list1_count_try` keeps the
        // `update_count_try` value. `mg_map.list0_only` is the port's copy of
        // the same persistent field; mirror it here.
        self.pd_ctx.list0_only = self.mg_map.list0_only;
        let mut pic = pp::PicParams {
            picture_number: display_order,
            decode_order: display_order,
            slice_type: if is_key {
                pp::SliceType::I
            } else {
                pp::SliceType::B
            },
            is_key_frame: is_key,
            is_intra_only: is_key,
            // C `svt_aom_is_delayed_intra` (`pd_process.c:3620`): an IDR/CRA
            // inside a RANDOM_ACCESS sequence with a live intra period. The
            // port has no per-input `end_of_sequence_flag` (C's is an input
            // buffer field), so that clause is always-false here.
            is_delayed_intra: is_key
                && self.pred_structure == pp::PredStructure::RandomAccess
                && self.gop.intra_period != 0,
            hierarchical_levels: hier,
            pred_struct_type: pp::PredStructure::LowDelay,
            pred_struct_entry_count: mini_gop,
            frame_offset: display_order,
            aligned_width: self.width,
            aligned_height: self.height,
            ..Default::default()
        };
        // C `update_pred_struct_and_pic_type` (`pd_process.c:4814-4871`) plus
        // the kernel's elapsed-counter update (`:5563-5590`). On a pure
        // low-delay stream the ported chain reduces to: IDR -> position 0;
        // the picture directly after an I slice -> `init_pic_index + 1` = 1;
        // otherwise position += 1, wrapping at `pred_struct_entry_count`.
        // `init_pic_index` is 0 for every C pred structure
        // (`pred_structure.c:679`), CRA/S-frame/cut-short arms are unreachable
        // (every key is an IDR, `pred_struct_type` is LOW_DELAY, the
        // hierarchy never changes), so the ported function is driven with
        // exactly those flags rather than re-deriving the chain.
        let slice = pp::update_pred_struct_and_pic_type(
            &mut pic,
            &mut self.enc_pic,
            &mut self.mg_map,
            &mut self.pd_ctx,
            /*mini_gop_index=*/ 0,
            /*pre_assignment_buffer_first_pass_flag=*/ false,
            /*idr_flag=*/ is_key,
            /*cra_flag=*/ false,
            /*init_pred_struct_position_flag=*/ false,
            /*init_pic_index=*/ 0,
        );
        debug_assert_eq!(slice, pic.slice_type);
        // `pd_process.c:5568-5588`: I_SLICE zeroes the elapsed-CRA counter
        // (and `get_pic_idx_in_mg`'s caller already moved `last_idr_picture`),
        // B_SLICE increments it clipped.
        if is_key {
            self.enc_pic.elapsed_non_cra_count = 0;
        } else {
            self.enc_pic.elapsed_non_cra_count =
                self.enc_pic.elapsed_non_cra_count.saturating_add(1);
        }
        // `pd_process.c:4869/5559`:
        // `pred_position_ptr = entry_array[pred_struct_position]` and
        // `temporal_layer_index = pred_position_ptr->temporal_layer_index`.
        pic.temporal_layer_index = pp::PRED_STRUCT_TEMPORAL_LAYER[usize::from(hier)]
            [self.enc_pic.pred_struct_position as usize];
        // `pd_process.c:5548`: the RPS branch selector. Its low-delay arm is
        // `(pos - 1) % entry_count` with a special case at 0 — NOT the
        // position itself — and it also writes `frame_offset`.
        let pic_idx = pp::get_pic_idx_in_mg(&mut pic, &seq, &self.enc_pic, &self.mg_map, 0, 0);
        // C `ref_pa_pic_ptr_array[list][0]`'s `avg_luma` — the PA reference
        // object's luma mean (`pic_analysis_process.c:2003`). Every
        // reference this path can name is an already-encoded frame, so its
        // DPB-mirrored PA slot holds the value `get_similar_ref_brightness`
        // reads; `INVALID_LUMA` when the slot's pyramid is stale or absent.
        let pa_slots = &self.pa_slots;
        let pa_luma = |poc: u64, slot: usize| -> u64 {
            pa_slots
                .get(slot)
                .and_then(|s| s.as_deref())
                .filter(|p| p.picture_number == poc)
                .map_or(pp::INVALID_LUMA, |p| p.avg_luma)
        };
        pp::picture_decision_per_picture(&mut pic, &seq, &mut self.pd_ctx, pic_idx, 0, &pa_luma).map_err(
            |_| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "this GOP shape's reference structure is not implemented: every \
                     top-level branch of C's av1_generate_rps_info is translated, so \
                     this is a case C itself rejects — LD-CBR outside hierarchical \
                     levels 1-2, or a mini-GOP position outside the ported tables \
                     [C: logs the same config as an error]",
                ))
            },
        )?;
        Ok(pic)
    }

    // ------------------------------------------------------------------
    // Random access (`SVT_PRED_STRUCT=2`): the mini-GOP window driver
    // ------------------------------------------------------------------
    //
    // C's picture decision never runs on a single frame under random access.
    // Input frames accumulate in a PRE-ASSIGNMENT BUFFER in display order
    // until a whole mini-GOP (`1 << hierarchical_levels` pictures) is present
    // or the end of sequence cuts it short; `set_mini_gop_structure` then
    // maps the window, the kernel walks it once in display order to assign
    // each picture's pred-structure position, once more to assign
    // `decode_order` (`decode_base_number + entry.decode_order` for a
    // complete mini-GOP, else the `picture_number_alt` counter), and a third
    // time in DECODE order for the RPS/DPB state (`pd_process.c:5470-5693`).
    // `process_pics` emits the pictures in that same decode order, with a
    // `show_existing_frame` OBU_FRAME_HEADER interleaved wherever a hidden
    // picture's display position is reached.
    //
    // `ra_input` is the port's pre-assignment buffer.

    /// The configuration envelope random access adds to `gop_config_error`'s.
    /// `None` means a window may run.
    fn ra_config_error(&self) -> Option<&'static str> {
        if self.gop.intra_period <= 1 {
            return Some(
                "pred_structure RandomAccess needs a GOP: intra_period == 1 makes every \
                 frame a key frame, so there is no mini-GOP to reorder, and \
                 intra_period == 0 (single key then low-delay inter) is untested on \
                 the RA path [C: accepts but degenerates to all-intra at == 1]",
            );
        }
        if self.superres_denom.is_some() {
            return Some(
                "pred_structure RandomAccess with superres is untested: references at \
                 a different coded width inside the window need decoder verification \
                 first [C: accepts]",
            );
        }
        if self.film_grain.enabled() || self.hdr.noise_strength > 0 {
            return Some(
                "pred_structure RandomAccess with film grain is untested: C's \
                 show_existing headers would have to re-signal grain state \
                 [C: accepts]",
            );
        }
        None
    }

    /// Entry-point guard for the non-420 arms: random access buffers and
    /// reorders input, which only `try_encode_frame_420` implements today —
    /// a mono/444/hbd frame fed through the sequential path would silently
    /// break the mini-GOP's display order, so refuse instead of emitting it.
    fn ra_entry_error(&self, _entry: &str) -> Option<whereat::At<EncodeError>> {
        (self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess).then(|| {
            whereat::at!(EncodeError::UnsupportedConfig(
                "pred_structure RandomAccess is wired on try_encode_frame_420 only; \
                 this entry takes the sequential path, which cannot buffer a \
                 mini-GOP [C: accepts]",
            ))
        })
    }

    /// The random-access arm of [`Self::try_encode_frame_420`].
    ///
    /// Buffers one input at `ra_display_next`'s display position, and only
    /// encodes when a whole mini-GOP is present — returning `Ok(Vec::new())`
    /// until then. A key frame (intra-period boundary) drains the partial
    /// window FIRST, then codes itself through the ordinary sequential path,
    /// exactly like C's `pre_assignment_buffer_idr_count` release.
    fn try_encode_frame_420_ra(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if let Some(why) = self.ra_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        let display_order = self.ra_display_next;
        self.ra_display_next += 1;

        // Stage the frame at TRUE dims; the encode pads per picture. The
        // PA statistics gather is unconditional — `calc_hist` is 1 in RA.
        // A key frame is just another reorder-queue entry here: C's drain
        // stamps `pcs->idr_flag` on it, which both terminates the current
        // release (`pre_assignment_buffer_intra_count > 0`) and, under a
        // live intra period, defers its TF window + packet to the NEXT
        // release (`svt_aom_is_delayed_intra` → `ctx->prev_delayed_intra`,
        // `pd_process.c:3620-3635, 5103-5131`).
        let is_key = self.gop.is_key_frame(display_order);
        let frame = self.pack_ra_frame(y, u, v, y_stride, display_order, is_key);
        let stats = self.gather_tf_stats_frame(&frame);
        self.ra_input.push(frame);
        self.ra_stats
            .push(Some(alloc::boxed::Box::new(stats)));
        self.ra_try_release()
    }

    /// Fire every pending release whose window the newly pushed frame just
    /// completed (`check_window_availability`'s `pd_window[2..2+scd_delay)`
    /// requirement — `pd_process.c:4659-4676` + the release trigger at
    /// `:5478-5484`). Queue order: an intra-terminated `[tail + intra]`
    /// release fires the moment the intra is committed — but only once the
    /// INTRA's own `scd_delay` future entries are queued, since its
    /// detector turn runs in the same drain.
    fn ra_try_release(&mut self) -> EncodeResult<Vec<u8>> {
        let mut out = alloc::vec::Vec::new();
        loop {
            let len = self.ra_input.len();
            let mg = self.gop.mini_gop_size as usize;
            let scd = self.ra_scd_delay() as usize;
            let fire = match self.ra_input.iter().position(|f| f.is_key) {
                // The release is `[tail + intra]` — `kp + 1` entries — once
                // the intra's `scd_delay` futures sit behind it:
                // `kp + scd < len`.
                Some(kp) if kp < mg => len > kp + scd,
                // A plain release commits `mg` entries; the deepest one's
                // window needs `mg - 1 + scd < len`.
                _ => len >= mg + scd,
            };
            if !fire {
                break;
            }
            out.extend_from_slice(&self.encode_ra_window()?);
        }
        Ok(out)
    }

    /// Pack one input into a [`RaBufferedFrame`] at TRUE dims.
    fn pack_ra_frame(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
        display_order: u64,
        is_key: bool,
    ) -> RaBufferedFrame {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
        let mut yb = alloc::vec::Vec::with_capacity(tw * th);
        for r in 0..th {
            yb.extend_from_slice(&y[r * y_stride..r * y_stride + tw]);
        }
        RaBufferedFrame {
            y: yb,
            u: u[..cw * ch].to_vec(),
            v: v[..cw * ch].to_vec(),
            display_order,
            is_key,
            is_eos: false,
        }
    }

    /// The `enhanced_pic` + `pa_ref_pic_wrapper` pair one buffered input
    /// owns in C: the padded aligned-dims planes the TF window's members
    /// are read through, fused into one [`crate::port_tf_driver::TfPicBufs`].
    fn ra_tf_bufs(&self, f: &RaBufferedFrame) -> crate::port_tf_driver::TfPicBufs {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
        let (acw, ach) = (aw.div_ceil(2), ah.div_ceil(2));
        let y_pad = if aw == tw && ah == th {
            f.y.clone()
        } else {
            pad_plane_replicate(&f.y, tw, tw, th, aw, ah)
                .expect("aligned dims are never smaller than true dims")
        };
        let (u_pad, v_pad) = if f.u.is_empty() {
            (alloc::vec::Vec::new(), alloc::vec::Vec::new())
        } else {
            (
                pad_plane_replicate(&f.u, cw, cw, ch, acw, ach)
                    .expect("aligned dims are never smaller than true dims"),
                pad_plane_replicate(&f.v, cw, cw, ch, acw, ach)
                    .expect("aligned dims are never smaller than true dims"),
            )
        };
        crate::port_tf_driver::TfPicBufs::from_aligned_planes(
            &y_pad,
            &u_pad,
            &v_pad,
            aw,
            ah,
            f.display_order,
        )
    }

    /// `scs->scd_delay` (`enc_handle.c:4005-4038`) for this stream: the
    /// future-picture depth `check_window_availability` requires before a
    /// queue entry may pass picture decision — and the extra depth
    /// `ra_input` must buffer past each release.
    fn ra_scd_delay(&self) -> u32 {
        let (_tf_level, tf_params_per_type) = crate::port_picstruct::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            self.gop.hierarchical_levels,
            self.enable_tf,
            /*lossless=*/ false,
        );
        crate::port_picstruct::derive_scd_delay(
            /*intra_period_is_zero=*/ self.gop.intra_period == 0,
            &tf_params_per_type,
            // `vq_ctrls.sharpness_ctrls.scene_transition` is 1 for every
            // single-pass RANDOM_ACCESS config (`enc_handle.c:3282-3291`,
            // cleared only for low delay / first pass at `:3324-3326`).
            /*scene_transition_armed=*/ true,
            /*lap_rc=*/ false,
        )
    }

    /// One release's queue turns of `perform_scene_change_detection` +
    /// `copy_histograms` (`pd_process.c:5378-5386, 5449-5452`), in display
    /// order over the release's `committed` queue entries.
    ///
    /// `check_window_availability` requires `scd_delay` future queue entries
    /// past each picture, none of them `end_of_sequence_flag`
    /// (`pd_process.c:4664-4674`). Mid-stream the release triggers guarantee
    /// every committed picture clears it; at EOS the missing tail entries
    /// are exactly what C's `eos_reached` resolves to `window_avail ==
    /// false` — the entry still commits, its detection is skipped.
    fn ra_pump_release(&mut self, committed: usize) {
        use crate::port_picstruct as pp;
        let scd = self.ra_scd_delay() as usize;
        let eff_len = self.ra_input.len();
        // `enhanced_pic->width/height` is the 8-aligned input size
        // (`max_input_luma_*` after `MIN_BLOCK_SIZE` pad,
        // resource_coordination_process.c:715-727); the region split keys
        // on the same field (`enc_handle.c:4392-4397`).
        let (tw, th) = (self.width, self.height);
        let (rw, rh) = (
            if tw >= 64 { 4usize } else { 1 },
            if th >= 64 { 4usize } else { 1 },
        );
        // Take the detector state out of `pd_ctx` so the stats borrows on
        // `self` can coexist with the detector's `&mut`.
        let mut sd = core::mem::take(&mut self.pd_ctx.scene_detect);
        let mut td = self.pd_ctx.transition_detected;
        for pos in 0..committed {
            // `:4664-4674` — every slot in `pd_window[2..2+scd)` must be
            // filled AND none may be `end_of_sequence_flag`; an EOS frame
            // inside the window also marks it unavailable.
            let window_avail = pos + scd < eff_len
                && !self.ra_input[pos + 1..=pos + scd]
                    .iter()
                    .any(|f| f.is_eos);
            let display_order = self.ra_input[pos].display_order;
            if window_avail && display_order > 0 {
                let cur = self.ra_stats[pos]
                    .as_deref()
                    .expect("committed positions always carry stats");
                let fut = self.ra_stats[pos + 1]
                    .as_deref()
                    .expect("window_avail implies at least one future");
                let out = pp::perform_scene_change_detection(
                    // `static_config.scene_change_detection` is force-zeroed
                    // in this SVT (`enc_settings.c:839-843`) — only the
                    // sharpness arm ever runs the detector.
                    /*scene_change_detection_enabled=*/ false,
                    /*sharpness_scene_transition=*/ true,
                    td,
                    /*cra_flag_in=*/ false,
                    || {
                        pp::scene_transition_detector(
                            &mut sd,
                            &cur.picture_histogram,
                            &cur.average_intensity_per_region,
                            &fut.average_intensity_per_region,
                            tw,
                            th,
                            rw,
                            rh,
                        )
                    },
                );
                td = out.transition_detected;
            }
            let cur = self.ra_stats[pos]
                .as_deref()
                .expect("committed positions always carry stats");
            pp::copy_histograms(
                &mut sd,
                &cur.picture_histogram,
                &cur.average_intensity_per_region,
            );
        }
        self.pd_ctx.scene_detect = sd;
        self.pd_ctx.transition_detected = td;
    }

    /// Drain any pending output. Under `PredStructure::RandomAccess` this
    /// encodes a partial trailing mini-GOP — C's `pre_assignment_buffer_eos_flag`
    /// release, which `is_pic_cutting_short_ra_mg` codes low-delay. Under the
    /// default low-delay structure there is never pending output and this is
    /// a no-op. Must be called after the last input, or the tail of an RA
    /// stream is silently un-emitted.
    pub fn try_flush(&mut self) -> EncodeResult<Vec<u8>> {
        if self.pred_structure != crate::port_picstruct::PredStructure::RandomAccess
            || (self.ra_input.is_empty() && self.delayed_intra.is_none())
        {
            return Ok(alloc::vec::Vec::new());
        }
        // The last input carries `end_of_sequence_flag` — C reads it in
        // `check_window_availability` (window never resolves past it) and
        // `svt_aom_is_delayed_intra` (a trailing intra is NOT delayed).
        if let Some(last) = self.ra_input.last_mut() {
            last.is_eos = true;
        }
        // C releases the whole reorder queue at EOS in queue order — an
        // intra-terminated release first when an intra is queued, then
        // `mini_gop_size` windows, then the cut-short remainder
        // (`pre_assignment_buffer_eos_flag`). Each `encode_ra_window` call
        // commits exactly one release and always makes progress.
        let mut out = alloc::vec::Vec::new();
        while !self.ra_input.is_empty() || self.delayed_intra.is_some() {
            out.extend_from_slice(&self.encode_ra_window()?);
        }
        Ok(out)
    }

    /// One random-access pre-assignment buffer's worth of packets.
    ///
    /// Runs [`Self::run_ra_picture_decision`] — the ported window kernel —
    /// then encodes the pictures in DECODE order, appending a
    /// `show_existing_frame` OBU_FRAME_HEADER right after any picture whose
    /// decision produced one (`pic.has_show_existing`), which is exactly the
    /// display-order point C emits it at.
    fn encode_ra_window(&mut self) -> EncodeResult<Vec<u8>> {
        let mg = self.gop.mini_gop_size as usize;
        // A held `prev_delayed_intra` with an empty queue still gets its
        // deferred `mctf_frame` + `send_picture_out` — C's `process_pics`
        // runs it ahead of any member work; here there are no members.
        if self.ra_input.is_empty() && self.delayed_intra.is_some() {
            let scs_tf = self.ra_tf_scs();
            let (q_weight, q_weight_denom) =
                crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
                    self.speed_config.preset > -1,
                    u32::from(self.rc_config.qp),
                );
            let rw = if self.width >= 64 { 4 } else { 1 };
            let rh = if self.height >= 64 { 4 } else { 1 };
            self.filter_delayed_intra(
                &scs_tf,
                q_weight,
                q_weight_denom,
                &mut alloc::vec::Vec::new(),
                /*first_future_hier=*/ None,
                rw,
                rh,
                /*mg_len=*/ 0,
            );
            return Ok(self.encode_delayed_intra()?);
        }
        // Queue order decides the release shape: an intra inside the next
        // `mg` entries terminates the release at `[tail + intra]`
        // (`pre_assignment_buffer_intra_count > 0`, `pd_process.c:5478`);
        // otherwise the release is the next `min(mg, len)` entries.
        let kp = self.ra_input.iter().position(|f| f.is_key);
        let (n, key_member) = match kp {
            Some(kp) if kp < mg => (kp, true),
            _ => (mg.min(self.ra_input.len()), false),
        };
        // One display-order pass of detection + histogram carry for every
        // committed queue entry (`pd_process.c:5378-5386, 5449-5452`).
        self.ra_pump_release(n + usize::from(key_member));
        let (mut pics, emit, frames_tf) = self.run_ra_picture_decision(n, key_member)?;
        #[cfg(feature = "std")]
        if crate::dbgenv::medbg() {
            std::eprintln!(
                "RAEMIT emit={emit:?} pics={:?}",
                pics.iter()
                    .map(|p| p
                        .as_ref()
                        .map(|p| (p.picture_number, p.decode_order, p.temporal_layer_index)))
                    .collect::<alloc::vec::Vec<_>>()
            );
        }
        // Drain only this release's members: the `scd_delay` spillover (and
        // any post-intra frames past an intra-terminated release) stay
        // buffered for the next release. Holding the drained frames locally
        // keeps `&mut self` free for the per-picture encode.
        let m = n + usize::from(key_member);
        let mut frames: alloc::vec::Vec<RaBufferedFrame> =
            self.ra_input.drain(..m).collect();
        let stats_drained: alloc::vec::Vec<_> = self.ra_stats.drain(..m).collect();
        if key_member && pics[n].as_ref().is_some_and(|p| p.is_delayed_intra) {
            // `ctx->prev_delayed_intra = pcs` (`pd_process.c:5151-5154`): the
            // intra member's send is replaced by the hold — its TF window and
            // packet run inside the NEXT release's `process_pics`.
            debug_assert!(frames.last().is_some_and(|f| f.is_key));
            let kf = frames.pop().expect("the intra is the last member");
            let stats = stats_drained[n]
                .as_deref()
                .expect("the intra gathered stats at push")
                .clone();
            self.delayed_intra = Some(DelayedIntra {
                bufs: self.ra_tf_bufs(&kf),
                frame: kf,
                stats,
                pic: pics[n]
                    .clone()
                    .expect("the intra's decision ran in the release"),
            });
        }
        // C `initial_rc_process` (src_ops): TPL runs between picture
        // decision and the encode loop — `tpl_prep_info` + `tpl_mc_flow` +
        // `generate_r0beta` per group base, producing each picture's `r0`,
        // `tpl_beta` and `tpl_rdmult_scaling_factors` plus the open-loop ME
        // results the encode then reuses. `None` on every configuration
        // where C's `get_tpl` disables TPL.
        let mut tpl_stage = self.run_tpl_stage(&frames, &pics, &emit, &frames_tf)?;
        self.delayed_intra_tpl = tpl_stage.as_mut().and_then(|s| s.key.take());
        let mut out = alloc::vec::Vec::new();
        // `send_picture_out(ctx->prev_delayed_intra)` (`pd_process.c:5129`):
        // the delayed intra's packet lands inside THIS release, after its
        // `mctf_frame` in pass 4 staged `delayed_intra_out`, ahead of the
        // member packets.
        if self.delayed_intra_out.is_some() {
            out.extend_from_slice(&self.encode_delayed_intra()?);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let tiles_log2 = self.tile_rows_log2 + self.tile_cols_log2;
        let td = crate::entropy::obu::write_temporal_delimiter();
        // C `count_frames_in_next_tu` (packetization_process.c:90-115): a
        // temporal unit is [hidden frames]* + ONE displayable frame — the TD
        // opens each TU and a `show_frame` picture closes it. The emit loop
        // is in decode order, so a shown picture simply ends the open TU.
        let mut tu_open = false;
        for idx in emit {
            let frame = &frames[idx];
            let pic = pics[idx]
                .take()
                .expect("emit order visits each picture once");
            let display_order = frame.display_order;
            let show_frame = pic.show_frame;
            let has_show_existing = pic.has_show_existing;
            let show_slot = pic.show_existing_frame;
            let decided = Some(FrameDecision {
                display_order,
                pic,
                tpl: tpl_stage.as_mut().and_then(|s| s.frames[idx].take()),
            });
            if !tu_open {
                out.extend_from_slice(&td);
                tu_open = true;
            }
            let pkt = if let Some(tf_pic) = frames_tf.get(idx).filter(|t| t.do_tf) {
                // The temporally filtered planes ARE the aligned-dims
                // source C's `enhanced_pic` carries out of
                // `svt_av1_init_temporal_filtering` — feed them to the
                // encode directly (the true->aligned pad is already inside
                // them, so `pad_plane_replicate` must not run again).
                let y_f = tf_pic.extract_luma();
                let u_f = tf_pic.extract_u();
                let v_f = tf_pic.extract_v();
                self.encode_frame_impl(&y_f, aw, Some((&u_f, &v_f)), decided)?
            } else if aw == tw && ah == th {
                self.encode_frame_impl(&frame.y, tw, Some((&frame.u, &frame.v)), decided)?
            } else {
                let (cw, ch) = (tw.div_ceil(2), th.div_ceil(2));
                let (acw, ach) = (aw.div_ceil(2), ah.div_ceil(2));
                let y_pad = pad_plane_replicate(&frame.y, tw, tw, th, aw, ah)?;
                let u_pad = pad_plane_replicate(&frame.u, cw, cw, ch, acw, ach)?;
                let v_pad = pad_plane_replicate(&frame.v, cw, cw, ch, acw, ach)?;
                self.encode_frame_impl(&y_pad, aw, Some((&u_pad, &v_pad)), decided)?
            };
            // `encode_frame_impl` prepends the sequential path's
            // one-TU-per-frame TD; an RA TU spans hidden+shown frames, so the
            // loop above owns TD placement and each packet's copy is dropped.
            debug_assert_eq!(
                pkt[..td.len()],
                td[..],
                "frame packet must begin with the TD it always emits"
            );
            out.extend_from_slice(&pkt[td.len()..]);
            if show_frame {
                tu_open = false;
            }
            if has_show_existing {
                // C emits each show_existing as its own temporal unit
                // (TD + OBU_FRAME_HEADER); without the TD a decoder treats
                // it as part of the preceding frame's TU and drops it.
                out.extend_from_slice(&td);
                out.extend_from_slice(&crate::entropy::obu::write_show_existing_obu(
                    show_slot, tiles_log2,
                ));
            }
        }
        Ok(out)
    }

    /// C `svt_aom_picture_decision_kernel`'s mini-GOP loop
    /// (`pd_process.c:5489-5693`), transcribed for one full pre-assignment
    /// buffer. Returns one [`PicParams`] per input picture (indexed by
    /// `ra_input` position) plus the emit order: buffer indices in DECODE
    /// order, which `store_mg_picture_arrays` computes per mini-GOP.
    ///
    /// The three passes match C exactly: pred-structure/slice-type per
    /// picture in DISPLAY order, `decode_order` assignment in DISPLAY order,
    /// then `picture_decision_per_picture` — RPS + DPB + settings — in
    /// DECODE order, because `update_dpb` applies each picture's refresh
    /// mask to the toggle ring in coded order.
    /// `n` non-intra members, plus — when `key_member` — the intra queue
    /// entry at member index `n`. C's pre-assignment buffer holds the intra
    /// like any other committed entry: `set_mini_gop_structure` splits the
    /// `n + 1` buffer with `pcs->idr_flag` exceptions, and the intra goes
    /// through passes 1-3 (`update_pred_struct_and_pic_type`,
    /// `av1_generate_rps_info`, `init_pic_settings` — where its TL0
    /// `transition_present` consumes `ctx->transition_detected`). Its
    /// `mctf_frame`/`send_picture_out` defer to the next release via
    /// `ctx->prev_delayed_intra` — the caller moves the member into
    /// `self.delayed_intra` when `pics[n].is_delayed_intra`.
    fn run_ra_picture_decision(
        &mut self,
        n: usize,
        key_member: bool,
    ) -> EncodeResult<(
        alloc::vec::Vec<Option<crate::port_picstruct::PicParams>>,
        alloc::vec::Vec<usize>,
        alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
    )> {
        use crate::port_picstruct as pp;
        let m = n + usize::from(key_member);
        let hier = self.gop.hierarchical_levels;
        // C `set_mrp_ctrl` (`enc_handle.c:3574`) — same call as the
        // low-delay path but with the RANDOM_ACCESS structure, whose caps
        // differ (backward references are real under RA).
        let mrp_ctrls = pp::set_mrp_ctrl(
            self.speed_config.preset,
            false,
            hier,
            pp::PredStructure::RandomAccess,
            self.bit_depth == 8,
            self.rc_config.mode.into(),
        );
        self.mrp_ctrls = mrp_ctrls;
        let (tf_level, tf_params_per_type) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            hier,
            self.enable_tf,
            /*lossless=*/ false,
        );
        let seq = pp::SeqPicParams {
            pred_structure: pp::PredStructure::RandomAccess,
            rate_control_mode: self.rc_config.mode.into(),
            rtc: false,
            allintra: false,
            mrp_ctrls,
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
            enable_tf_key: self.enable_tf_key,
            tf_level,
            tf_params_per_type,
        };

        // The buffer C releases is `pre_assignment_buffer_count` committed
        // queue entries — `n` non-intra members plus, for an
        // intra-terminated release, the intra itself (member `n`): the
        // `pcs->idr_flag` exceptions in the split see it.
        self.enc_pic.pre_assignment_buffer_count = m as u32;
        self.enc_pic.pre_assignment_buffer_intra_count = u32::from(key_member);
        self.enc_pic.pre_assignment_buffer_idr_count = u32::from(key_member);
        // `set_mini_gop_structure` reads `pic.picture_number` — the LAST
        // committed entry's POC (`pcs` at `pd_process.c:5486` is the entry
        // just appended) — and `pic.hierarchical_levels` (RTC arm only —
        // `seq.rtc` is false here).
        let stub = pp::PicParams {
            picture_number: self.ra_input[m - 1].display_order,
            hierarchical_levels: hier,
            ..Default::default()
        };
        // `enable_dg = false`: C's dynamic-GOP split is unported and refused
        // above for the only level that can reach it (hier 5 / 32-pic MG).
        // `list0_only_base = enc_mode > ENC_M2` (`enc_handle.c:4292`) — C's
        // `scs->list0_only_base`, which `initialize_mini_gop_activity_array`
        // copies to `ctx->list0_only` (`pd_process.c:846-848`) for
        // `send_picture_out`'s tl0 list-1 clamp (`:4950`). That init only
        // runs when the pre-assignment buffer holds more than one picture
        // (`:4756-4758`), so the field persists otherwise — mirror the map's
        // value rather than re-deriving it.
        let list0_only = self.speed_config.preset > crate::port_enc_mode_config::enc_mode::M2;
        let needs_dg = pp::set_mini_gop_structure(
            &mut self.mg_map,
            &mut self.enc_pic,
            &seq,
            &stub,
            u32::from(hier),
            /*startup_mg_size=*/ 0,
            // The last committed entry's `pcs->idr_flag` — the intra's own
            // flag on an intra-terminated release.
            /*idr_flag=*/ key_member,
            /*enable_dg=*/ false,
            list0_only,
        );
        self.pd_ctx.list0_only = self.mg_map.list0_only;
        debug_assert!(
            !needs_dg,
            "eval_sub_mini_gop is unported; hier < 5 cannot reach it"
        );

        let mut pics: alloc::vec::Vec<Option<pp::PicParams>> = (0..m).map(|_| None).collect();
        let mut emit: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

        // `scs->calc_hist` (enc_handle.c:1353): the region histograms +
        // `avg_luma` are gathered for EVERY buffered input at push time —
        // the sharpness scene-transition arm makes `calc_hist` 1 in every
        // RA config, not just TF-enabled ones.
        debug_assert_eq!(
            self.ra_stats.len(),
            self.ra_input.len(),
            "ra_stats must be parallel to ra_input"
        );

        // The `enhanced_pic` + `pa_ref_pic_wrapper` pair every buffered
        // input owns in C: the padded aligned-dims planes the TF window's
        // members are read through, fused into one [`TfPicBufs`] per slot.
        // Built under the same `calc_hist` gate — without it no picture can
        // be TF-enabled, so `frames_tf` stays empty and the encode loop
        // takes the unfiltered arm for every slot.
        let mut frames_tf: alloc::vec::Vec<crate::port_tf_driver::TfPicBufs> =
            alloc::vec::Vec::new();
        if tf_params_per_type.iter().any(|c| c.enabled) {
            // EVERY queued entry owns buffers — C's reorder-queue entries
            // each carry `enhanced_pic`/`pa_ref_pic_wrapper`, and a member's
            // TF window reaches past the release into the `scd_delay`
            // spillover (`mg_pictures_array` + queue lookahead).
            for f in &self.ra_input {
                frames_tf.push(self.ra_tf_bufs(f));
            }
        }

        for mg_idx in 0..self.mg_map.total_number_of_mini_gops {
            let start = self.mg_map.start_index[mg_idx] as usize;
            let end = self.mg_map.end_index[mg_idx] as usize;
            let mg_len = self.mg_map.length[mg_idx];
            // `pd_process.c:5494-5504`: the first picture's
            // `hierarchical_layers_diff` vs the PREVIOUS mini-GOP decides
            // `init_pred_struct_position_flag` (and `is_mini_gop_changed`,
            // which nothing downstream reads here).
            let init_pos_flag = self.enc_pic.previous_mini_gop_hierarchical_levels
                != self.mg_map.hierarchical_levels[mg_idx];
            self.enc_pic.previous_mini_gop_hierarchical_levels =
                self.mg_map.hierarchical_levels[mg_idx];
            self.pd_ctx.cut_short_ra_mg = 0;

            // ---- Pass 1, DISPLAY order (`pd_process.c:5506-5608`) ----
            // pred structure + slice type + pred_struct_position per picture.
            let mut pred_struct_index = alloc::vec::Vec::with_capacity(end - start + 1);
            let mut pic_idx_in_mg = alloc::vec::Vec::with_capacity(end - start + 1);
            for pic_idx in start..=end {
                let first = pic_idx == start;
                // The intra-terminated release's last member IS the intra:
                // `pcs->idr_flag` runs the IDR arms of the pred-struct
                // helpers and `picture_decision_per_picture`, and pass 3
                // stamps `is_delayed_intra`.
                let intra_member = key_member && pic_idx == m - 1;
                let mut pic = pp::PicParams {
                    picture_number: self.ra_input[pic_idx].display_order,
                    // `frm_hdr.frame_type` (`pd_process.c:5674-5679`): the
                    // intra member is a KEY_FRAME — `generate_rps_info`'s
                    // `set_key_frame_rps` arm resets the DPB on it.
                    is_key_frame: intra_member,
                    is_intra_only: intra_member,
                    slice_type: pp::SliceType::B,
                    pred_struct_type: pp::PredStructure::RandomAccess,
                    aligned_width: self.width,
                    aligned_height: self.height,
                    // `pcs.c:422`: the configured input dims, not the aligned
                    // ones — the TF window's resolution-change exclusion
                    // compares these.
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // `resource_coordination_process.c:414-416`: non-CBR
                    // `frame_offset = picture_number`. `get_pic_idx_in_mg`
                    // overwrites it only in low delay.
                    frame_offset: self.ra_input[pic_idx].display_order,
                    // C `pcs->avg_luma` — `svt_aom_gathering_picture_statistics`'s
                    // output (`pic_analysis_process.c:628-635`), `INVALID_LUMA`
                    // whenever `calc_hist` is off: `ra_stats` is `None` per slot
                    // under exactly that gate.
                    avg_luma: self
                        .ra_stats
                        .get(pic_idx)
                        .and_then(|s| s.as_deref())
                        .map_or(pp::INVALID_LUMA, |s| s.avg_luma),
                    ..Default::default()
                };
                pp::get_pred_struct_for_frame(
                    &mut pic,
                    &mut self.mg_map,
                    mg_idx,
                    seq.pred_structure,
                    hier,
                    /*startup_mg_size=*/ 0,
                    /*idr_flag=*/ intra_member,
                    /*cra_flag=*/ false,
                );
                // `pred_struct_ptr` follows the (possibly cut-short) type
                // and the MINI-GOP's level, not the sequence's.
                pic.pred_struct_entry_count = 1u32 << pic.hierarchical_levels;
                pic.slice_type = pp::update_pred_struct_and_pic_type(
                    &mut pic,
                    &mut self.enc_pic,
                    &mut self.mg_map,
                    &mut self.pd_ctx,
                    mg_idx,
                    /*pre_assignment_buffer_first_pass_flag=*/ first,
                    /*idr_flag=*/ intra_member,
                    /*cra_flag=*/ false,
                    /*init_pred_struct_position_flag=*/ first && init_pos_flag,
                    /*init_pic_index=*/ 0,
                );
                if intra_member {
                    // `svt_aom_is_delayed_intra` (`pd_process.c:3620-3635`),
                    // stamped where C computes it — before `init_pic_settings`'s
                    // `copy_tf_params` reads it. An IDR in RA with a live
                    // intra period is held for the next release unless it IS
                    // the sequence's last input (`end_of_sequence_flag`).
                    pic.is_delayed_intra = pp::is_delayed_intra(
                        /*idr_flag=*/ true,
                        /*cra_flag=*/ false,
                        seq.pred_structure,
                        self.gop.intra_period as i32,
                        self.ra_input[pic_idx].is_eos,
                        self.enc_pic.pre_assignment_buffer_count,
                        pic.pred_struct_entry_count,
                    );
                }
                // `pd_process.c:5563-5590`'s slice-type switch: an I slice
                // RESETS `elapsed_non_cra_count` (and, for an IDR, stamps
                // `ctx->key_poc`); a B slice bumps both elapsed counters,
                // clipped. (`elapsed_non_idr_count` has no reader in the
                // ported subset; the CRA one feeds the position walk.)
                if pic.slice_type == pp::SliceType::I {
                    self.enc_pic.elapsed_non_cra_count = 0;
                    if intra_member {
                        self.pd_ctx.key_poc = pic.picture_number;
                    }
                } else {
                    self.enc_pic.elapsed_non_cra_count =
                        self.enc_pic.elapsed_non_cra_count.saturating_add(1);
                }
                // `pd_process.c:5548/4869/5559`: position AFTER the walk
                // indexes the entry; the entry carries the temporal layer.
                let pos = self.enc_pic.pred_struct_position as usize;
                pred_struct_index.push(pos as u8);
                pic.temporal_layer_index =
                    pp::PRED_STRUCT_TEMPORAL_LAYER[usize::from(pic.hierarchical_levels)][pos];
                pic_idx_in_mg.push(pp::get_pic_idx_in_mg(
                    &mut pic,
                    &seq,
                    &self.enc_pic,
                    &self.mg_map,
                    pic_idx as u32,
                    mg_idx,
                ));
                pics[pic_idx] = Some(pic);
            }

            // ---- Pass 2, DISPLAY order (`pd_process.c:5612-5667`) ----
            // `picture_number_alt++` every picture; the RA permutation arm
            // needs a COMPLETE mini-GOP (`length == entry_count`) with no
            // intra inside; everything else falls to the alt counter.
            for pic_idx in start..=end {
                let pic = pics[pic_idx].as_mut().unwrap();
                let alt = self.enc_pic.picture_number_alt;
                self.enc_pic.picture_number_alt += 1;
                pic.decode_order = if self.mg_map.idr_count[mg_idx] == 0
                    && self.mg_map.length[mg_idx] == pic.pred_struct_entry_count
                    && seq.pred_structure == pp::PredStructure::RandomAccess
                {
                    self.enc_pic.decode_base_number
                        + u64::from(
                            pp::PRED_STRUCT_DECODE_ORDER[usize::from(pic.hierarchical_levels)]
                                [usize::from(pred_struct_index[pic_idx - start])],
                        )
                } else {
                    alt
                };
            }
            // `pd_process.c:5660-5662`: decode base advances by the mini-GOP
            // length (overlays are off in this envelope — `has_overlay = 0`).
            self.enc_pic.decode_base_number += u64::from(mg_len);

            // ---- `store_mg_picture_arrays` + pass 3, DECODE order ----
            let decode_orders: alloc::vec::Vec<u64> = (start..=end)
                .map(|i| pics[i].as_ref().unwrap().decode_order)
                .collect();
            let (decode_perm, _display_perm) = pp::store_mg_picture_arrays(&decode_orders);
            // C `ref_pa_pic_ptr_array[list][0]`'s `avg_luma` — the PA
            // reference object's luma mean (`pic_analysis_process.c:2003`),
            // resolved for `get_similar_ref_brightness` inside
            // `init_pic_settings`. A reference can name two kinds of
            // picture here: an in-window member (its `ra_stats` slot — the
            // same gather `avg_luma` was stamped from) or a prior window's
            // coded picture (the DPB-mirrored `pa_slots` pyramid).
            // `INVALID_LUMA` when the reference can't be resolved — which
            // is also C's answer whenever `calc_hist` is off, since every
            // arm is gated upstream. Scoped to pass 3 so the field borrows
            // end before pass 4's `&mut self` filter prep.
            let pa_luma = |poc: u64, slot: usize| -> u64 {
                if let Some(i) = self.ra_input.iter().position(|f| f.display_order == poc) {
                    return self
                        .ra_stats
                        .get(i)
                        .and_then(|s| s.as_deref())
                        .map_or(pp::INVALID_LUMA, |s| s.avg_luma);
                }
                // The delayed intra (C's `prev_delayed_intra`) is a viable
                // reference target for the NEXT release's members — the
                // new GOP's L0 anchor. Its PA mean sits on `DelayedIntra`
                // before pass 4's `mctf_frame`, then on the staged output's
                // `PicParams` (`pcs->avg_luma` — stamped at member
                // construction from the same gather) after the hold moves.
                if let Some(d) = self
                    .delayed_intra
                    .as_ref()
                    .filter(|d| d.frame.display_order == poc)
                {
                    return d.stats.avg_luma;
                }
                if let Some((_, p)) = self
                    .delayed_intra_out
                    .as_ref()
                    .filter(|(_, p)| p.picture_number == poc)
                {
                    return p.avg_luma;
                }
                self.pa_slots
                    .get(slot)
                    .and_then(|s| s.as_deref())
                    .filter(|p| p.picture_number == poc)
                    .map_or(pp::INVALID_LUMA, |p| p.avg_luma)
            };
            for &local in &decode_perm {
                let pic_idx = start + local;
                let pic = pics[pic_idx].as_mut().unwrap();
                pp::picture_decision_per_picture(
                    pic,
                    &seq,
                    &mut self.pd_ctx,
                    pic_idx_in_mg[local],
                    mg_idx,
                    &pa_luma,
                )
                .map_err(|_| {
                    whereat::at!(EncodeError::UnsupportedConfig(
                        "this GOP shape's reference structure is not implemented: every \
                         top-level branch of C's av1_generate_rps_info is translated, \
                         so this is a case C itself rejects — LD-CBR outside \
                         hierarchical levels 1-2, or a mini-GOP position outside \
                         the ported tables [C: logs the same config as an error]",
                    ))
                })?;
                // `pd_process.c:5151-5154`: the delayed intra's send is
                // replaced by the `ctx->prev_delayed_intra` hold — its
                // packet emits inside the NEXT release.
                if !pic.is_delayed_intra {
                    emit.push(pic_idx);
                }
            }

            // ---- Pass 4, DISPLAY order (`pd_process.c:5090-5132`) ----
            // `process_pics`' MCTF loop: per-picture `filt_to_unfilt_diff`
            // inheritance, `derive_tf_window_params` (noise selection,
            // reference-count modulation, window assembly), the FILTER
            // itself (`mctf_frame` -> `svt_av1_init_temporal_filtering`)
            // and the publish carry.
            self.run_ra_tf_prep(&mut pics, start, end, &mut frames_tf);
        }
        Ok((pics, emit, frames_tf))
    }

    /// `svt_aom_gathering_picture_statistics` for one buffered RA input
    /// (`pic_analysis_process.c:1960-2006`): pad the luma to aligned dims,
    /// decimate to the 1/16 plane, then gather the region histograms and
    /// `avg_luma` the TF window's `calc_ahd` reads — and, now that
    /// `calc_hist` is always on in RA, the scene-transition detector's
    /// per-region histograms and intensities.
    fn gather_tf_stats_frame(
        &self,
        frame: &RaBufferedFrame,
    ) -> crate::port_preanalysis::PictureStatistics {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        // `svt_aom_pad_input_pictures` + `downsample_filtering_input_picture`:
        // the LIVE route decimates the ALIGNED padded luma twice by 2
        // (`PaPicture::from_source` owns that exact chain).
        let y_pad = if aw == tw && ah == th {
            frame.y.clone()
        } else {
            pad_plane_replicate(&frame.y, tw, tw, th, aw, ah)
                .expect("aligned dims are never smaller than true dims")
        };
        let pa =
            crate::inter_me_arm::PaPicture::from_source(&y_pad, aw, aw, ah, frame.display_order);
        // `scs->picture_analysis_number_of_regions_per_*` keys on
        // `max_input_luma_*` AFTER the `MIN_BLOCK_SIZE` pad — the aligned
        // dims (`enc_handle.c:4392-4397`).
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        let mut stats = crate::port_preanalysis::PictureStatistics::default();
        crate::port_preanalysis::gathering_picture_statistics(
            /*calc_hist=*/ true,
            /*calculate_variance=*/ false,
            regions_per_width,
            regions_per_height,
            /*scene_change_detection=*/ false,
            &pa.sixteenth.buf[pa.sixteenth.org..],
            pa.sixteenth.stride,
            pa.sixteenth.width,
            pa.sixteenth.height,
            None,
            &mut stats,
        );
        stats
    }

    /// C `process_pics`' display-order MCTF loop (`pd_process.c:5090-5132`)
    /// for one mini-GOP, minus the filter dispatch itself.
    ///
    /// Per picture, in display order:
    /// 1. inherit `ctx->filt_to_unfilt_diff` (`:5122`);
    /// 2. `mctf_frame` — here: `derive_tf_window_params`, i.e. the noise
    ///    selection/carry, `ref_pics_modulation`, window counts and the
    ///    `temp_filt_pcs_list` assembly (`:4194-4241`); when `tf_ctrls` is
    ///    off the picture's `do_tf` is cleared (`:4239`) — no window is
    ///    built, which is the port's equivalent;
    /// 3. `is_noise_level` is stamped on EVERY picture (`:4240-4241`);
    /// 4. an I slice publishes its filt/unfilt difference back to the
    ///    context (`:5124`).
    ///
    /// The delayed-intra slot (`:5103-5110`): an `is_delayed_intra` member
    /// skips this loop — its `mctf_frame` runs inside the NEXT release's
    /// `process_pics` via `filter_delayed_intra`, exactly C's
    /// `prev_delayed_intra` ordering. `gm_pp_enabled`'s base-layer toggle
    /// (`:5117-5120`) has no ported consumer yet and is noted rather than
    /// reproduced.
    /// The `RaTfScs` bundle `run_ra_tf_prep` builds — extracted so the held
    /// key frame's lone-release path (EOS, or a back-to-back intra) runs the
    /// same scaffolding.
    fn ra_tf_scs(&self) -> crate::port_tf_driver::RaTfScs {
        // Same `max_input_luma_*` (post-`MIN_BLOCK_SIZE`-pad) key as the
        // gather — `enc_handle.c:4392-4397`.
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        // `derive_vq_params`' `vq_ctrls.sharpness_ctrls.tf`
        // (`enc_handle.c:3277-3293`): on for the subjective tunes only.
        let vq_sharpness_tf =
            matches!(self.hdr.tune, 0 | 5) || (self.hdr.alt_ssim_tuning && self.hdr.tune == 2);
        // `scs->calculate_variance` (`enc_handle.c:4361-4365`) for the RA
        // envelope: `allintra`, `rtc` and `scene_change_detection` are all
        // off here, `aq_mode != 0` is refused upstream.
        let calculate_variance = vq_sharpness_tf || self.hdr.enable_variance_boost;
        let (seg_rows, seg_cols) =
            crate::port_tf_driver::tf_segment_counts(self.width, self.height);
        crate::port_tf_driver::RaTfScs {
            bit_depth: self.bit_depth,
            qp: u32::from(self.rc_config.qp),
            enable_tf: u8::from(self.enable_tf),
            tf_strength: self.hdr.tf_strength,
            // `SVT_HDR_MODE` (temporal_filtering.c:2785/3320): the fork's
            // `kf_tf_strength` replaces the mainline kf arm entirely. `None`
            // in mainline mode so a stray configured value can't leak in —
            // mirrors the compile-time `#if`.
            kf_tf_strength: self.hdr.is_fork().then_some(self.hdr.kf_tf_strength),
            tf_ref_qp_based_th_scaling: self.speed_config.preset > -1,
            vq_sharpness_tf,
            calculate_variance,
            compute_psnr: false,
            compute_ssim: false,
            enc_mode: self.speed_config.preset,
            sb_size: self.sb_size as i32,
            input_resolution: crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                self.width * self.height,
            ),
            regions_per_width,
            regions_per_height,
            tf_segment_row_count: seg_rows,
            tf_segment_column_count: seg_cols,
            true_width: self.true_width,
            true_height: self.true_height,
            aligned_width: self.width,
            aligned_height: self.height,
        }
    }

    /// `ctx->prev_delayed_intra`'s `mctf_frame` (`pd_process.c:5103-5132`),
    /// run at the head of the FOLLOWING release's `process_pics` — ahead of
    /// the member loop so its `:5124` publish lands before any member
    /// inherits `:5122`. The future members C pulls from the released
    /// buffer's `mg_pictures_array` are this release's `mg_len` `frames`;
    /// `mg_len == 0` is the lone-release case (EOS), where C's buffer was
    /// just as empty. The filtered buffers land in `delayed_intra_out` for
    /// `encode_ra_window` to emit ahead of the member packets.
    fn filter_delayed_intra(
        &mut self,
        scs_tf: &crate::port_tf_driver::RaTfScs,
        q_weight: u32,
        q_weight_denom: u32,
        frames: &mut alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
        first_future_hier: Option<u8>,
        regions_per_width: usize,
        regions_per_height: usize,
        mg_len: usize,
    ) {
        use crate::port_picstruct as pp;
        let Some(key) = self.delayed_intra.take() else {
            return;
        };
        // The member's decision IS the pcs C carries — slice I, delayed
        // intra, decided in its own release.
        let mut pic = key.pic;
        // `derive_tf_window_params`' key-frame pred-structure fixup
        // (`pd_process.c:3936-3943`): when the first future picture's
        // hierarchy differs from the delayed intra's, the intra adopts the
        // member's level BEFORE the `tf_max_ref_per_struct` cap is taken.
        if let Some(hier) = first_future_hier
            && hier != pic.hierarchical_levels
        {
            pic.hierarchical_levels = hier;
        }
        // `copy_tf_params` (`pd_process.c:4468-4497`): an RA intra with a live
        // intra period is a DELAYED intra — `tf_params_per_type[0]`, not the
        // BASE row.
        let (_tf_level, table) = pp::derive_tf_params(
            self.pred_structure,
            self.speed_config.preset,
            self.gop.hierarchical_levels,
            self.enable_tf,
            /*lossless=*/ false,
        );
        pic.tf_ctrls = match pp::copy_tf_params(
            self.pred_structure,
            pic.slice_type,
            pic.is_key_frame,
            pic.temporal_layer_index,
            pic.hierarchical_levels,
            /*is_overlay=*/ false,
            self.enable_tf_key,
            pic.is_delayed_intra,
        ) {
            pp::TfParamsChoice::DelayedIntra => table[0],
            pp::TfParamsChoice::Base => table[1],
            pp::TfParamsChoice::L1 => table[2],
            pp::TfParamsChoice::Disabled => pp::TfCtrls::default(),
        };
        // `:5122` — inherit the carried filt/unfilt difference.
        pp::tf_inherit_filt_to_unfilt_diff(&self.pd_ctx, &mut pic);
        let ctrls = pic.tf_ctrls;
        if ctrls.enabled {
            let (tw, th) = (self.true_width as usize, self.true_height as usize);
            let cw = tw.div_ceil(2);
            // `derive_tf_window_params`' noise half (`:3755-3849`). An I
            // slice always estimates fresh (`do_noise_est` is true whenever
            // `is_i_slice`) and publishes to the carry slot.
            let fresh_y = Some(crate::port_temporal_filtering::noise_log1p_fp16(
                crate::temporal_filter::estimate_noise_fp16(&key.frame.y, tw, th, tw),
            ));
            let fresh_uv = if ctrls.chroma_lvl != 0 {
                [
                    crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(
                            &key.frame.u,
                            tw >> 1,
                            th >> 1,
                            cw,
                        ),
                    ),
                    crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(
                            &key.frame.v,
                            tw >> 1,
                            th >> 1,
                            cw,
                        ),
                    ),
                ]
            } else {
                [0, 0]
            };
            let noise = pp::tf_window_noise(
                ctrls.use_intra_for_noise_est,
                /*is_i_slice=*/ true,
                ctrls.chroma_lvl != 0,
                fresh_y,
                fresh_uv,
                &mut self.pd_ctx.last_i_noise_levels_log1p_fp16,
            );
            // `:3850` — reference-count modulation.
            let offset = if ctrls.modulate_pics != 0 {
                pp::ref_pics_modulation(
                    /*is_i_slice=*/ true,
                    /*temporal_layer_index=*/ 0,
                    &ctrls,
                    noise.levels_log1p_fp16[0],
                    pic.filt_to_unfilt_diff,
                    q_weight,
                    q_weight_denom,
                )
            } else {
                0
            };
            // `pcs->idr_flag` arm of `derive_tf_window_params`
            // (`:3964-3998`): centre at slot 0, future from the released
            // buffer's `mg_pictures_array` — this release's `mg_len`
            // members (`ra_input[..mg_len]` while they still sit in the
            // queue, `frames` for their bufs).
            let empty_hist: alloc::boxed::Box<pp::RegionHistograms> =
                alloc::boxed::Box::new([[[0u32; 256]; 4]; 4]);
            let mut key_cands: alloc::vec::Vec<pp::TfWindowCand<'_>> =
                alloc::vec::Vec::with_capacity(mg_len + 1);
            key_cands.push(pp::TfWindowCand {
                picture_number: key.frame.display_order,
                frame_width: self.true_width,
                frame_height: self.true_height,
                hierarchical_levels: pic.hierarchical_levels,
                avg_luma: key.stats.avg_luma,
                picture_histogram: &key.stats.picture_histogram,
            });
            key_cands.extend((0..mg_len).map(|i| {
                let st = self.ra_stats.get(i).and_then(|s| s.as_deref());
                pp::TfWindowCand {
                    picture_number: self.ra_input[i].display_order,
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // Only the delayed-intra pred-structure fixup reads this
                    // field, and only on the poc+1 member — which
                    // `first_future_hier` already carries.
                    hierarchical_levels: first_future_hier.unwrap_or(self.gop.hierarchical_levels),
                    avg_luma: st.map_or(crate::port_preanalysis::INVALID_LUMA, |s| s.avg_luma),
                    picture_histogram: st.map_or(&*empty_hist, |s| &s.picture_histogram),
                }
            }));
            // `pd_process.c:3922-3965` — the delayed-intra arm: future
            // pictures poc-matched out of the released buffer (the
            // `mg_pictures_array` that follows the held intra), capped by
            // `tf_max_ref_per_struct` on the post-fixup hierarchy.
            let counts = pp::derive_tf_window_counts(
                pp::TfWindowArm::DelayedIntra,
                &ctrls,
                offset,
                u32::from(pic.hierarchical_levels),
                pic.temporal_layer_index,
            );
            let window = pp::assemble_tf_window(
                pp::TfWindowArm::DelayedIntra,
                &counts,
                /*centre_idx=*/ 0,
                &key_cands,
                /*mg_lo=*/ 0,
                /*mg_hi=*/ key_cands.len(),
                /*ld_past=*/ &[],
                /*avail_past=*/ 0,
                regions_per_width,
                regions_per_height,
            );
            pic.noise_levels_log1p_fp16 = noise.levels_log1p_fp16;
            pic.past_altref_nframes = window.past_altref_nframes as u8;
            pic.future_altref_nframes = window.future_altref_nframes as u8;
            pic.tf_avg_luma = window.tf_avg_luma;
            pic.tf_avg_ahd_error = window.tf_avg_ahd_error;
            pic.tf_window = Some(alloc::boxed::Box::new(window));
            // The window's member indexes address the combined buffer view:
            // the intra at 0, this release's members after it.
            frames.insert(0, key.bufs);
            let mut member_stats: alloc::vec::Vec<
                Option<alloc::boxed::Box<crate::port_preanalysis::PictureStatistics>>,
            > = alloc::vec::Vec::with_capacity(mg_len + 1);
            member_stats.push(Some(alloc::boxed::Box::new(key.stats)));
            member_stats.extend(self.ra_stats.iter().take(mg_len).cloned());
            let out = crate::port_tf_driver::ra_mctf_filter(
                scs_tf,
                /*centre_slot=*/ 0,
                &pic,
                &member_stats,
                frames,
            );
            self.pd_ctx.tf_motion_direction = out.motion_direction;
            if let Some(diff) = out.filt_to_unfilt_diff {
                pic.filt_to_unfilt_diff = diff;
            }
            let bufs = frames.remove(0);
            // `:4240-4241` — stamped on every picture, TF enabled or not.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            // `:5124` — an I slice publishes its measured difference.
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, &pic);
            self.delayed_intra_out = Some((bufs, pic));
        } else {
            // Disabled ctrls still take the stamp + publish (`:4240`, `:5124`);
            // the bufs pass through unfiltered.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, &pic);
            self.delayed_intra_out = Some((key.bufs, pic));
        }
    }

    /// Emit the delayed intra's packet — the filtered (or pass-through)
    /// aligned planes, C's `send_picture_out(ctx->prev_delayed_intra)`
    /// (`pd_process.c:5129`). The decision ran in the intra's own release
    /// as a buffer member, so it arrives as `decided` like any window
    /// picture; re-running it here would advance the `pd_ctx` toggles a
    /// second time.
    fn encode_delayed_intra(&mut self) -> EncodeResult<Vec<u8>> {
        let Some((bufs, pic)) = self.delayed_intra_out.take() else {
            return Ok(alloc::vec::Vec::new());
        };
        let aw = self.width as usize;
        let y_f = bufs.extract_luma();
        let u_f = bufs.extract_u();
        let v_f = bufs.extract_v();
        let tpl = self.delayed_intra_tpl.take();
        self.encode_frame_impl(
            &y_f,
            aw,
            Some((&u_f, &v_f)),
            Some(FrameDecision {
                display_order: pic.picture_number,
                pic,
                tpl,
            }),
        )
    }

    fn run_ra_tf_prep(
        &mut self,
        pics: &mut alloc::vec::Vec<Option<crate::port_picstruct::PicParams>>,
        mg_lo: usize,
        mg_hi: usize,
        frames: &mut alloc::vec::Vec<crate::port_tf_driver::TfPicBufs>,
    ) {
        use crate::port_picstruct as pp;
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let cw = tw.div_ceil(2);
        // `scs->picture_analysis_number_of_regions_per_*` — aligned dims
        // (`enc_handle.c:4392-4397`).
        let regions_per_width = if self.width >= 64 { 4 } else { 1 };
        let regions_per_height = if self.height >= 64 { 4 } else { 1 };
        let scs_tf = self.ra_tf_scs();
        // `set_qp_based_th_scaling_ctrls_default` (enc_handle.c:3785-3816):
        // `tf_ref_qp_based_th_scaling` is off only at ENC_MR (preset -1).
        let (q_weight, q_weight_denom) =
            crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
                self.speed_config.preset > -1,
                u32::from(self.rc_config.qp),
            );

        // `ctx->prev_delayed_intra`'s `mctf_frame` + `send_picture_out`
        // run at the head of THIS release's `process_pics`
        // (`pd_process.c:5103-5131`) — ahead of the member loop, so the
        // intra's `filt_to_unfilt_diff` publish and noise carry land before
        // the members inherit `:5122`, and `delayed_intra_out` stages for
        // `encode_ra_window`'s emit-ahead.
        self.filter_delayed_intra(
            &scs_tf,
            q_weight,
            q_weight_denom,
            frames,
            pics[0].as_ref().map(|p| p.hierarchical_levels),
            regions_per_width,
            regions_per_height,
            // The intra's future window is this release's `mg_pictures_array`
            // — the `pics` member count, not the deeper queue `frames`
            // covers.
            pics.len(),
        );

        // The candidate view `assemble_tf_window` searches: every buffered
        // input in display order. C's `mg_pictures_array` / reorder queue /
        // pre-assignment buffer are all entries of this one buffer under the
        // port's driver.
        let empty_hist: alloc::boxed::Box<pp::RegionHistograms> =
            alloc::boxed::Box::new([[[0u32; 256]; 4]; 4]);
        let cands: alloc::vec::Vec<pp::TfWindowCand> = (0..self.ra_input.len())
            .map(|i| {
                let st = self.ra_stats.get(i).and_then(|s| s.as_deref());
                pp::TfWindowCand {
                    picture_number: self.ra_input[i].display_order,
                    frame_width: self.true_width,
                    frame_height: self.true_height,
                    // Members carry their decided level; spillover entries
                    // deeper in the queue have no decision yet — C's pcs
                    // ctor value, `scs->static_config.hierarchical_levels`.
                    hierarchical_levels: pics
                        .get(i)
                        .and_then(|o| o.as_ref())
                        .map_or(self.gop.hierarchical_levels, |p| p.hierarchical_levels),
                    avg_luma: st.map_or(crate::port_preanalysis::INVALID_LUMA, |s| s.avg_luma),
                    picture_histogram: st.map_or(&*empty_hist, |s| &s.picture_histogram),
                }
            })
            .collect();
        let mg_pocs: alloc::vec::Vec<u64> = (mg_lo..=mg_hi)
            .map(|i| self.ra_input[i].display_order)
            .collect();

        for pic_idx in mg_lo..=mg_hi {
            let (is_delayed_intra, tf_enabled, is_i, tl, hier, poc, ctrls, is_key) = {
                let p = pics[pic_idx].as_ref().unwrap();
                (
                    p.is_delayed_intra,
                    p.tf_ctrls.enabled,
                    p.slice_type == pp::SliceType::I,
                    p.temporal_layer_index,
                    p.hierarchical_levels,
                    p.picture_number,
                    p.tf_ctrls,
                    p.is_key_frame,
                )
            };
            // `pd_process.c:5115`: delayed intra runs through the
            // `prev_delayed_intra` slot, not this loop.
            if is_delayed_intra {
                continue;
            }
            // `:5122` — inherit the carried filt/unfilt difference.
            pp::tf_inherit_filt_to_unfilt_diff(&self.pd_ctx, pics[pic_idx].as_mut().unwrap());
            if tf_enabled {
                // `derive_tf_window_params`' noise half (`:3755-3849`).
                // 8-bit arm only: the RA driver is the `try_encode_frame_420`
                // (u8) path; the highbd estimator is ported for when a 10-bit
                // RA entry exists.
                let do_est = !ctrls.use_intra_for_noise_est || is_i;
                let fresh_y = if do_est {
                    let f = &self.ra_input[pic_idx];
                    Some(crate::port_temporal_filtering::noise_log1p_fp16(
                        crate::temporal_filter::estimate_noise_fp16(&f.y, tw, th, tw),
                    ))
                } else {
                    None
                };
                let fresh_uv = if ctrls.chroma_lvl != 0 {
                    let f = &self.ra_input[pic_idx];
                    // `pd_process.c:3826-3841`: the estimate runs on
                    // `width >> ss_x` (FLOOR) — differs from the packed
                    // plane's `div_ceil` stride only at odd widths.
                    [
                        crate::port_temporal_filtering::noise_log1p_fp16(
                            crate::temporal_filter::estimate_noise_fp16(&f.u, tw >> 1, th >> 1, cw),
                        ),
                        crate::port_temporal_filtering::noise_log1p_fp16(
                            crate::temporal_filter::estimate_noise_fp16(&f.v, tw >> 1, th >> 1, cw),
                        ),
                    ]
                } else {
                    [0, 0]
                };
                let noise = pp::tf_window_noise(
                    ctrls.use_intra_for_noise_est,
                    is_i,
                    ctrls.chroma_lvl != 0,
                    fresh_y,
                    fresh_uv,
                    &mut self.pd_ctx.last_i_noise_levels_log1p_fp16,
                );
                // `:3850` — reference-count modulation (qp-scaled when the
                // control table's `qp_opt` is set).
                let filt_diff = pics[pic_idx].as_ref().unwrap().filt_to_unfilt_diff;
                let offset = if ctrls.modulate_pics != 0 {
                    pp::ref_pics_modulation(
                        is_i,
                        tl,
                        &ctrls,
                        noise.levels_log1p_fp16[0],
                        filt_diff,
                        q_weight,
                        q_weight_denom,
                    )
                } else {
                    0
                };
                // `pcs->idr_flag` selects the IDR arm; a buffered I slice is
                // an IDR in this envelope (CRAs are refused upstream).
                let arm = if is_key {
                    pp::TfWindowArm::RandomAccessIdr
                } else {
                    pp::TfWindowArm::RandomAccessInter
                };
                let counts = pp::derive_tf_window_counts(arm, &ctrls, offset, u32::from(hier), tl);
                let avail_past = pp::avail_past_pictures(&mg_pocs, poc);
                let window = pp::assemble_tf_window(
                    arm,
                    &counts,
                    pic_idx,
                    &cands,
                    mg_lo,
                    mg_hi + 1,
                    // `pd_ctx->tf_pic_array` — empty under random access.
                    &[],
                    avail_past,
                    regions_per_width,
                    regions_per_height,
                );
                let pic = pics[pic_idx].as_mut().unwrap();
                pic.noise_levels_log1p_fp16 = noise.levels_log1p_fp16;
                if let Some(lvl) = window.hier_fixup {
                    pic.hierarchical_levels = lvl;
                }
                pic.past_altref_nframes = window.past_altref_nframes as u8;
                pic.future_altref_nframes = window.future_altref_nframes as u8;
                pic.tf_avg_luma = window.tf_avg_luma;
                pic.tf_avg_ahd_error = window.tf_avg_ahd_error;
                pic.tf_window = Some(alloc::boxed::Box::new(window));
            }
            if tf_enabled {
                // `mctf_frame`'s second half — `svt_av1_init_temporal_filtering`
                // itself (`pd_process.c:5122`'s `mctf_frame` call). It MUST
                // sit inside this display-order loop, before the publish at
                // `:5124`: an I slice's measured `filt_to_unfilt_diff` has
                // to reach `pic.filt_to_unfilt_diff` here or the next
                // picture inherits the stale carried value at `:5122`. The
                // filtered pixels land in `frames[pic_idx]`, which the
                // encode loop substitutes for the source.
                let out = crate::port_tf_driver::ra_mctf_filter(
                    &scs_tf,
                    pic_idx,
                    pics[pic_idx].as_ref().unwrap(),
                    &self.ra_stats,
                    frames,
                );
                self.pd_ctx.tf_motion_direction = out.motion_direction;
                if let Some(diff) = out.filt_to_unfilt_diff {
                    pics[pic_idx].as_mut().unwrap().filt_to_unfilt_diff = diff;
                }
            }
            let pic = pics[pic_idx].as_mut().unwrap();
            // `:4240-4241` — stamped on every picture, TF enabled or not.
            pic.is_noise_level =
                self.pd_ctx.last_i_noise_levels_log1p_fp16[0] >= pp::VQ_NOISE_LVL_TH;
            // `:5124` — only an I slice publishes its measured difference.
            pp::tf_publish_filt_to_unfilt_diff(&mut self.pd_ctx, pic);
        }
    }

    /// The TPL stage for one released window — C's
    /// `store_extended_group` + `set_tpl_group`/`set_tpl_params` per picture
    /// (`initial_rc_process.c`), `tpl_prep_info`/`tpl_mc_flow`
    /// (`src_ops_process.c`), and `svt_aom_generate_r0beta`
    /// (`rc_init_frame_stats`), all at `scs->tpl_lad_mg == 0`: C's
    /// lookahead-backed extended group (`ctx->lad_queue`, which is what
    /// makes a group span `tpl_lad_mg + 1` mini-GOPs) degenerates to exactly
    /// the in-flight mini-GOP this pipeline already buffers, so the whole
    /// stage runs inside `encode_ra_window` with no pipeline restructure.
    ///
    /// Runs AFTER `run_ra_picture_decision` (every member's `PicParams`
    /// exists) and BEFORE the emit loop. Two groups are processed, matching
    /// C's per-BASE-picture processing in `initial_rc_process`:
    ///
    /// * The held key frame's group — `[key] + this window in decode order`
    ///   (C's delayed intra is released into the next mini-GOP's batch, so
    ///   its `ext_group` is the queue's whole contents;
    ///   `limited_tpl_group_size` for an I-slice base is
    ///   `1 + (tpl_lad_mg + 1) * mg_size`).
    /// * The window base's group — the window in decode order
    ///   (`(tpl_lad_mg + 1) * mg_size` members).
    ///
    /// Each member's OWN `r0`/`tpl_beta`/`tpl_rdmult_scaling_factors` come
    /// from the group run whose base it is — C recomputes them per base
    /// picture and later runs overwrite `pa_me_data->tpl_stats`.
    ///
    /// The per-member open-loop ME results the dispenser consumes are the
    /// SAME `FrameMe` the picture's own encode computes — identical current
    /// and reference pyramids — so each member's [`FrameTplIn`] carries its
    /// `frame_me` for `encode_frame_impl` to reuse, matching C's
    /// `pa_me_data->me_results` ownership (PA ME runs once per picture,
    /// ahead of both src-ops and enc-dec).
    fn run_tpl_stage(
        &mut self,
        frames: &[RaBufferedFrame],
        pics: &[Option<crate::port_picstruct::PicParams>],
        emit: &[usize],
        frames_tf: &[crate::port_tf_driver::TfPicBufs],
    ) -> EncodeResult<Option<TplStageOut>> {
        use crate::port_picstruct as pp;
        use crate::port_tpl as pt;

        // `scs->tpl` (`get_tpl`, enc_handle.c:3657) — the SEQ gate; every
        // picture's own `tpl_ctrls.enable` is derived below per picture.
        if !self.scs_tpl() {
            return Ok(None);
        }
        // `scs->tpl_lad_mg` — C's lookahead in mini-GOP units
        // (`initial_rc_process.c`'s lad-queue window). This pipeline's
        // one-mini-GOP RA buffer is the `tpl_lad_mg == 0` shape; the
        // two-mini-GOP `tpl_lad_mg == 1` shape needs a deeper lookahead
        // queue and is refused upstream for now.
        let tpl_lad_mg = 0u8;
        // C's `scs->tpl_lad_mg` CONFIG value (enc_handle.c:4041-4063) — NOT
        // the port's group shape. `allintra || LOW_DELAY -> 0`; else
        // `look_ahead < mg_size -> 0`, `scs->tpl -> 1` (an `scs->tpl` short
        // of this stage already returned above). The RA window this
        // pipeline buffers IS one full mini-GOP, so C's
        // `look_ahead_distance < mg_size` test is false wherever TPL runs.
        // MEASURED (johnny 256x256 9f qp40 p6 hier3 aq2): C's
        // `r0_adjust_factor` is 0 for every picture because
        // `!scs->tpl_lad_mg` is false — the port's structural 0 wrongly fed
        // `set_tpl_group` here and divided every inter `r0` by
        // `0.8 * tpl_hl_base_frame_div_factor[3]` = 1.6, dropping
        // `qstep_ratio`/`base_q_idx` (poc8: port 104 vs C 128) and halving
        // the inter SB lambda (poc8 sb1: port 37775 vs C 76453).
        let scs_tpl_lad_mg: u8 = if self.gop.intra_period == 1
            || self.pred_structure == crate::port_picstruct::PredStructure::LowDelay
        {
            0
        } else {
            1
        };
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let sb_size = self.sb_size;
        let enc_mode_allintra = self.gop.intra_period == 1;
        let input_resolution =
            crate::port_enc_mode_config::ResolutionRange::from_luma_area((aw * ah) as u32) as u8;

        // `svt_aom_get_tpl_group_level`/`get_tpl_params_level` are
        // enc_mode-keyed; `eff_enc_mode` applies C's M11 clamp.
        let tpl_group_level = |is_i: bool| {
            let sc_arm = if enc_mode_allintra {
                crate::sc_detect::ScArm::Allintra
            } else {
                crate::sc_detect::ScArm::Video { is_islice: is_i }
            };
            let em = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
            pp::get_tpl_group_level(1, em)
        };
        let tpl_params_level = |is_i: bool| {
            let sc_arm = if enc_mode_allintra {
                crate::sc_detect::ScArm::Allintra
            } else {
                crate::sc_detect::ScArm::Video { is_islice: is_i }
            };
            let em = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
            pp::get_tpl_params_level(em)
        };

        // Per-picture `tpl_ctrls` — `set_tpl_group` + `set_tpl_params` on the
        // picture's own `TplPicParams`, exactly C's `initial_rc_process`
        // sequence. `(ctrls, synth_blk_size)` per ra_input member plus the
        // held key.
        let pic_tpl = |pic: &pp::PicParams| -> (pp::TplControls, u8) {
            let (mut t, synth) = pp::set_tpl_group(
                Some(&pp::TplPicParams {
                    slice_type: pic.slice_type,
                    hierarchical_levels: pic.hierarchical_levels,
                    input_resolution,
                    // C `pcs->scs->tpl_lad_mg` — the CONFIG knob C's
                    // `r0_adjust_factor` gate reads, not the port's
                    // group-shape `tpl_lad_mg` (which stays 0).
                    tpl_lad_mg: scs_tpl_lad_mg,
                    rate_control_mode: self.rc_config.mode.into(),
                }),
                tpl_group_level(pic.slice_type == pp::SliceType::I),
                self.width,
                self.height,
            );
            pp::set_tpl_params(
                &mut t,
                tpl_params_level(pic.slice_type == pp::SliceType::I),
                input_resolution,
            );
            (t, synth)
        };

        let n = frames.len();
        let member_tpl: alloc::vec::Vec<(pp::TplControls, u8)> = (0..n)
            .map(|i| pic_tpl(pics[i].as_ref().expect("PD ran for every member")))
            .collect();
        let key_ctx: Option<(
            pp::PicParams,
            pp::TplControls,
            u8,
            &crate::inter_me_arm::PaPicture,
        )> = self.delayed_intra_out.as_ref().map(|(bufs, pic)| {
            let (t, synth) = pic_tpl(pic);
            (pic.clone(), t, synth, &bufs.pa)
        });

        // ---------------------------------------------------------------
        // Member PA pyramids — the `EbPaReferenceObject->input_padded_pic`
        // equivalent. A TF-filtered member reuses `frames_tf[i].pa` (the
        // filtered pyramid the encode also sees); an unfiltered member gets
        // its own pyramid built on the true->aligned replicated pad, the
        // same bytes `encode_frame_impl` pads for it.
        // ---------------------------------------------------------------
        let mut owned_pa: alloc::vec::Vec<Option<crate::inter_me_arm::PaPicture>> =
            (0..n).map(|_| None).collect();
        for (i, f) in frames.iter().enumerate() {
            if frames_tf.get(i).is_none() {
                let y_pad = if aw == tw && ah == th {
                    f.y.clone()
                } else {
                    pad_plane_replicate(&f.y, tw, tw, th, aw, ah)?
                };
                owned_pa[i] = Some(crate::inter_me_arm::PaPicture::from_source(
                    &y_pad,
                    aw,
                    aw,
                    ah,
                    f.display_order,
                ));
            }
        }
        let ra_pa: alloc::vec::Vec<&crate::inter_me_arm::PaPicture> = (0..n)
            .map(|i| {
                frames_tf
                    .get(i)
                    .map(|t| &t.pa)
                    .or(owned_pa[i].as_ref())
                    .expect("member PA built above")
            })
            .collect();

        // Resolve a reference POC to its PA source: an in-window member's
        // own `ra_pa` entry, the held key's pyramid (staged but not yet in
        // `pa_slots` — `run_tpl_stage` runs before `encode_delayed_intra`),
        // else the coded picture's `pa_slots` DPB pyramid — the three
        // origins C's `ref_pa_pic_ptr_array` merges.
        let pa_for_poc = |poc: u64, slot: usize| -> Option<&crate::inter_me_arm::PaPicture> {
            ra_pa
                .iter()
                .find(|p| p.picture_number == poc)
                .copied()
                .or_else(|| key_ctx.as_ref().map(|k| k.3).filter(|p| p.picture_number == poc))
                .or_else(|| self.pa_slots.get(slot).and_then(|s| s.as_deref()))
        };

        // ---------------------------------------------------------------
        // Member open-loop ME — `pa_me`'s output, shared between the TPL
        // dispenser (as `me_results`) and the picture's own encode.
        // `run_frame_me_into` on the same reference pyramids the emit path
        // resolves, so reusing it is byte-identical.
        // ---------------------------------------------------------------
        let mut frame_mes: alloc::vec::Vec<Option<crate::inter_me_arm::FrameMe>> =
            (0..n).map(|_| None).collect();
        for i in 0..n {
            let pic = pics[i].as_ref().unwrap();
            if pic.slice_type == pp::SliceType::I {
                continue;
            }
            let mut refs = crate::inter_me::context::MeRefs::default();
            for rt in 1i8..=7 {
                let (li, ri) = (
                    crate::inter_mvp::get_list_idx(rt),
                    crate::inter_mvp::get_ref_frame_idx(rt),
                );
                let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                let poc = pic.rps.ref_poc_array[usize::from(rt as u8 - 1)];
                if let Some(pa) = pa_for_poc(poc, slot) {
                    refs.arr[li][ri] = Some(pa.ds_ref());
                }
            }
            let num_to_search = [pic.ref_list0_count_try, pic.ref_list1_count_try];
            let complete = (0..2)
                .all(|li| (0..usize::from(num_to_search[li])).all(|ri| refs.arr[li][ri].is_some()));
            #[cfg(feature = "std")]
            if crate::dbgenv::medbg() {
                std::eprintln!(
                    "MERPS poc={} rps_poc={:?} rps_idx={:?} ns={:?} refs=[{:?},{:?},{:?},{:?}] complete={complete}",
                    pic.picture_number,
                    pic.rps.ref_poc_array,
                    pic.rps.ref_dpb_index,
                    num_to_search,
                    refs.arr[0][0].map(|d| d.picture_number),
                    refs.arr[0][1].map(|d| d.picture_number),
                    refs.arr[1][0].map(|d| d.picture_number),
                    refs.arr[1][1].map(|d| d.picture_number),
                );
            }
            if !complete {
                continue;
            }
            let sc_arm = crate::sc_detect::ScArm::Video { is_islice: false };
            // The member's own `sc_class5` — `derive_sc` on the same bytes
            // the encode sees.
            let y_src = frames_tf
                .get(i)
                .map(|t| t.extract_luma())
                .unwrap_or_else(|| {
                    owned_pa[i]
                        .as_ref()
                        .map(|p| {
                            let v = &p.full;
                            let mut out = alloc::vec![0u8; v.width * v.height];
                            for r in 0..v.height {
                                out[r * v.width..r * v.width + v.width].copy_from_slice(
                                    &v.buf[v.org + r * v.stride..v.org + r * v.stride + v.width],
                                );
                            }
                            out
                        })
                        .unwrap_or_default()
                });
            let sc_derivation =
                crate::sc_detect::derive_sc(sc_arm, self.speed_config.preset, &y_src, aw, aw, ah);
            let mut out = self
                .me_scratch
                .take()
                .unwrap_or_else(crate::inter_me_arm::FrameMe::empty);
            crate::inter_me_arm::run_frame_me_into(
                &mut out,
                ra_pa[i],
                &refs,
                num_to_search,
                crate::inter_me_arm::FrameMeParams {
                    enc_mode: crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                    qp: self.rc_config.qp,
                    width: aw,
                    height: ah,
                    picture_number: frames[i].display_order,
                    frame_is_boosted: pp::frame_is_boosted(pic),
                    hierarchical_levels: pic.hierarchical_levels,
                    temporal_layer_index: pic.temporal_layer_index,
                    is_ref: pic.is_ref,
                    sc_class5: u8::from(sc_derivation.classes.sc_class5),
                    only_l_bwd: self.mrp_ctrls.only_l_bwd != 0,
                    safe_limit_nref: self.mrp_ctrls.safe_limit_nref,
                    safe_limit_zz_th: self.mrp_ctrls.safe_limit_zz_th,
                    // C `pcs->similar_brightness_refs` / `frame_is_leaf(pcs)`
                    // — picture decision's outputs, gating the safe-limit ME
                    // arm (`motion_estimation.c:2231`).
                    similar_brightness_refs: pic.similar_brightness_refs,
                    frame_is_leaf: pp::frame_is_leaf(pic.update_type),
                },
            );
            frame_mes[i] = Some(out);
        }
        // The held key's ME — an I slice never searches; C's
        // `pa_me_data->me_results` for an I picture is untouched and the
        // dispenser never reads it, so the row carries `None`.

        // ---------------------------------------------------------------
        // Group flow — one `tpl_prep_info` + `tpl_mc_flow` +
        // `generate_r0beta` per group base.
        // ---------------------------------------------------------------
        let stats_len = |synth: u8| -> usize {
            let mb_w = aw.div_ceil(16);
            let mb_h = ah.div_ceil(16);
            match synth {
                8 => (mb_w << 1) * (mb_h << 1),
                32 => aw.div_ceil(32) * ah.div_ceil(32),
                _ => mb_w * mb_h,
            }
        };
        let factor_grid_len = |synth_blk_32: bool, mi_rows: i32| -> usize {
            let n = if synth_blk_32 { 8 } else { 4 };
            let mi_cols_sr = ((aw as i32 + 15) / 16) << 2;
            let cols = (mi_cols_sr + n - 1) / n;
            let rows = (mi_rows + n - 1) / n;
            (rows * cols) as usize
        };
        let sb_cnt = aw.div_ceil(sb_size) * ah.div_ceil(sb_size);
        // The dispenser's 64x64-block geometry — C iterates `scs->b64_geom`
        // (always 64 px, `is_full_64x64` gates the search level).
        let b64_geom: alloc::vec::Vec<(u32, u32, bool)> = (0..(aw.div_ceil(64) * ah.div_ceil(64))
            as u32)
            .map(|i| {
                let g = crate::port_pcs_geom::b64_geom(aw as u16, ah as u16, 64, i);
                (u32::from(g.org_x), u32::from(g.org_y), g.is_complete_b64)
            })
            .collect();
        let sb_orgs: alloc::vec::Vec<(u32, u32)> = (0..sb_cnt as u32)
            .map(|i| {
                let g = crate::port_pcs_geom::sb_geom(aw as u16, ah as u16, sb_size as u16, i);
                (u32::from(g.org_x), u32::from(g.org_y))
            })
            .collect();

        // ---------------------------------------------------------------
        // The two group runs, in C's decode-order processing sequence:
        // the held key's `[key] + window` group first (its initial_rc runs
        // first), then the window base's. Each fills its members'
        // `tpl_stats` grids; the window run is the LAST writer, which is
        // what `generate_r0beta` for each member then reads — matching C,
        // where every picture's own initial_rc later re-dispenses the
        // group and overwrites `pa_me_data->tpl_stats`.
        // ---------------------------------------------------------------

        /// One member of a TPL group run: its PA source pyramid, its
        /// picture-decision row, its own `tpl_ctrls`, and its open-loop ME
        /// results (`None` only for an I slice).
        struct GroupRow<'a> {
            pa: &'a crate::inter_me_arm::PaPicture,
            pic: &'a pp::PicParams,
            ctrls: &'a pp::TplControls,
            me: Option<&'a crate::inter_me_arm::FrameMe>,
        }

        /// `ext_group` + `store_extended_group` over a queue slice — the
        /// group C's `initial_rc_process` builds when the head of `rows`
        /// is the base. Every row shares `ext_mg_id` (C's `mg_progress_id`
        /// is one release batch).
        fn group_for(rows: &[GroupRow<'_>], ext_mg_id: i64, tpl_lad_mg: u8) -> pp::TplGroup {
            let ext_group: alloc::vec::Vec<pp::ExtGroupPic> = rows
                .iter()
                .map(|r| pp::ExtGroupPic {
                    picture_number: r.pic.picture_number,
                    slice_type: r.pic.slice_type,
                    temporal_layer_index: r.pic.temporal_layer_index,
                    ext_mg_id,
                    is_delayed_intra: r.pic.is_delayed_intra,
                    is_skipped: false,
                })
                .collect();
            pp::store_extended_group(
                &ext_group,
                rows[0].pic.slice_type,
                rows[0].pic.hierarchical_levels,
                u32::from(tpl_lad_mg),
                rows[0].ctrls.reduced_tpl_group,
            )
        }

        /// `tpl_prep_info` + `tpl_mc_flow` + `generate_r0beta` for ONE
        /// group base — the `src_ops_process.c`/`rc_init_frame_stats` half.
        /// `rows` is the picture's `ext_group` in decode order; `rows[0]` is
        /// the base whose `initial_rc` is being modeled.
        ///
        /// C's `tpl_mc_flow` runs the dispenser+synthesizer ONLY when the
        /// group base's `tpl_data.tpl_temporal_layer_index == 0`; a non-tl0
        /// base's `initial_rc` builds the group but dispenses nothing, so
        /// this returns `(group, [])` for those. When the flow does run it
        /// rewrites EVERY group member's stats (invalid members keep the
        /// memset zeros), and `generate_r0beta` is evaluated per member —
        /// exactly the `pa_me_data` state a member's own
        /// `rc_init_frame_stats` would later read, since the LAST tl0 run
        /// containing a member is what its stats hold by then.
        /// Returns `(group, [(rows index, r0beta, factors, beta)])`.
        #[allow(clippy::too_many_arguments)]
        fn run_group<'pa>(
            base_synth: u8,
            rows: &[GroupRow<'_>],
            tpl_lad_mg: u8,
            qp: u8,
            ext_off: i32,
            sb_size: usize,
            aw: usize,
            ah: usize,
            tw: usize,
            th: usize,
            b64_geom: &[(u32, u32, bool)],
            sb_orgs: &[(u32, u32)],
            stats_len: usize,
            factor_grid_len: usize,
            sb_cnt: usize,
            pa_slots: &'pa [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
            ra_pa: &[&'pa crate::inter_me_arm::PaPicture],
            key_pa: Option<&'pa crate::inter_me_arm::PaPicture>,
        ) -> (
            pp::TplGroup,
            alloc::vec::Vec<(usize, pt::R0Beta, alloc::vec::Vec<f64>, alloc::vec::Vec<f64>)>,
        ) {
            let g = group_for(rows, 0, tpl_lad_mg);
            let mut out: alloc::vec::Vec<(
                usize,
                pt::R0Beta,
                alloc::vec::Vec<f64>,
                alloc::vec::Vec<f64>,
            )> = alloc::vec::Vec::new();
            if g.members.is_empty() {
                return (g, out);
            }
            /// A `PaPicture`'s enhanced luma as a [`pt::TplPic`] view — the
            /// `EbPaReferenceObject->input_padded_pic` equivalent.
            fn pa_as_tpl<'pa>(pa: &'pa crate::inter_me_arm::PaPicture) -> pt::TplPic<'pa> {
                let f = &pa.full;
                pt::TplPic {
                    y: &f.buf,
                    y_stride: f.stride,
                    width: f.width as u32,
                    height: f.height as u32,
                    max_width: (f.width + 2 * f.border) as u32,
                    max_height: (f.height + 2 * f.border) as u32,
                    origin: f.org,
                }
            }
            /// Resolve a ref POC to its PA pyramid — an in-window member's
            /// `ra_pa` entry, the held key's, or the coded picture's
            /// `pa_slots` DPB pyramid.
            fn resolve_pa<'pa>(
                poc: u64,
                slot: usize,
                ra_pa: &[&'pa crate::inter_me_arm::PaPicture],
                key_pa: Option<&'pa crate::inter_me_arm::PaPicture>,
                pa_slots: &'pa [Option<alloc::sync::Arc<crate::inter_me_arm::PaPicture>>; 8],
            ) -> Option<&'pa crate::inter_me_arm::PaPicture> {
                ra_pa
                    .iter()
                    .find(|p| p.picture_number == poc)
                    .copied()
                    .or_else(|| key_pa.filter(|p| p.picture_number == poc))
                    .or_else(|| pa_slots.get(slot).and_then(|s| s.as_deref()))
            }

            // `tpl_ref_ds` PA-source table — every (list, ref) resolves to
            // its PA pyramid. C stores the `EbPaReferenceObject` directly;
            // the port carries indices into this dense table.
            let mut ref_pics: alloc::vec::Vec<pt::TplPic<'_>> = alloc::vec::Vec::new();

            let prep: alloc::vec::Vec<pt::TplPrepPic> = g
                .members
                .iter()
                .map(|&ext_i| {
                    let row = &rows[ext_i];
                    let pic = row.pic;
                    let mut ref_pic_poc = [[0u64; 8]; 2];
                    let mut ref_pic_index = [[0usize; 8]; 2];
                    let mut ref_poc_ds = [[0u64; 8]; 2];
                    for (list, count) in [pic.ref_list0_count_try, pic.ref_list1_count_try]
                        .iter()
                        .enumerate()
                    {
                        for ri in 0..usize::from(*count) {
                            let rt = list * 4 + ri;
                            let poc = pic.rps.ref_poc_array[rt];
                            let slot = pic.rps.ref_dpb_index[rt] as usize;
                            ref_pic_poc[list][ri] = poc;
                            ref_poc_ds[list][ri] = poc;
                            ref_pic_index[list][ri] =
                                match resolve_pa(poc, slot, ra_pa, key_pa, pa_slots) {
                                    Some(pa) => {
                                        ref_pics.push(pa_as_tpl(pa));
                                        ref_pics.len() - 1
                                    }
                                    None => {
                                        // A ref the RA structure offered but no
                                        // source resolves: cannot happen on a
                                        // consistent RPS — keep a degenerate
                                        // entry rather than emit wrong stats
                                        // silently.
                                        debug_assert!(
                                            false,
                                            "tpl: ref poc {poc} slot {slot} unresolved"
                                        );
                                        ref_pics.push(pt::TplPic {
                                            y: &[],
                                            y_stride: 0,
                                            width: 0,
                                            height: 0,
                                            max_width: 0,
                                            max_height: 0,
                                            origin: 0,
                                        });
                                        ref_pics.len() - 1
                                    }
                                };
                        }
                    }
                    pt::TplPrepPic {
                        slice_type: pic.slice_type,
                        temporal_layer_index: pic.temporal_layer_index,
                        is_ref: pic.is_ref,
                        decode_order: pic.decode_order as i32,
                        picture_number: pic.picture_number,
                        ref_list_count_try: [pic.ref_list0_count_try, pic.ref_list1_count_try],
                        ref_pic_poc,
                        ref_pic_index,
                        ref_poc_ds,
                    }
                })
                .collect();
            let group_pocs: alloc::vec::Vec<u64> = prep.iter().map(|p| p.picture_number).collect();
            let tpl_datas = pt::tpl_prep_info(&group_pocs, &prep);

            // C gates the WHOLE dispenser+synthesizer on the BASE's tpl
            // temporal layer (`src_ops_process.c`: `tpl_group[0]->tpl_data
            // .tpl_temporal_layer_index == 0`). A non-tl0 base's `initial_rc`
            // built the group (tpl_group_size is still consumed later) but
            // rewrote no stats — members keep what the last tl0 run left.
            if tpl_datas[0].tpl_temporal_layer_index != 0 {
                return (g, out);
            }

            // Per-member TplWindowFrame — owned stats grids sized for the
            // BASE's synth block size (the whole group shares the base's
            // `tpl_ctrls.synth_blk_size` inside `tpl_mc_flow`).
            let mut stats_store: alloc::vec::Vec<alloc::vec::Vec<pt::TplStats>> = g
                .members
                .iter()
                .map(|_| alloc::vec![pt::TplStats::default(); stats_len])
                .collect();
            let mut src_stats_store: alloc::vec::Vec<alloc::vec::Vec<pt::TplSrcStats>> =
                g.members.iter().map(|_| alloc::vec::Vec::new()).collect();
            // Recon planes — the `mc_flow_rec_picture_buffer` pool. One
            // buffer per member: stride aw+64, 32-px top/left border
            // (TPL_PAD is 32).
            let recon_stride = aw + 64;
            let mut recon_store: alloc::vec::Vec<alloc::vec::Vec<u8>> = g
                .members
                .iter()
                .map(|_| alloc::vec![0u8; recon_stride * (ah + 64)])
                .collect();

            // The base's `tpl_valid_pic`, MEMBER-ordered — `ref_tpl_group_
            // idx` is a position in `group_pocs`, not an ext index.
            let member_valid: alloc::vec::Vec<u8> = g.members.iter().map(|&i| g.valid[i]).collect();

            let qp_qindex = crate::rate_control::QUANTIZER_TO_QINDEX[qp.min(63) as usize];
            let quant_for = |row: &GroupRow<'_>| -> crate::quant::QuantTable {
                let qi = pt::tpl_qindex(
                    qp,
                    ext_off,
                    row.ctrls.enable_tpl_qps != 0,
                    row.pic.slice_type,
                    row.pic.hierarchical_levels,
                    row.pic.temporal_layer_index,
                );
                crate::quant::build_quant_table(qi.clamp(0, i32::from(u8::MAX)) as u8)
            };

            // Member metadata the dispense closure binds per frame_idx.
            struct MemberCtx<'a> {
                row: &'a GroupRow<'a>,
                quant: crate::quant::QuantTable,
            }
            let member_ctxs: alloc::vec::Vec<MemberCtx<'_>> = g
                .members
                .iter()
                .map(|&ext_i| MemberCtx {
                    row: &rows[ext_i],
                    quant: quant_for(&rows[ext_i]),
                })
                .collect();

            let mut window: alloc::vec::Vec<pt::TplWindowFrame<'_>> = g
                .members
                .iter()
                .enumerate()
                .zip(
                    stats_store
                        .iter_mut()
                        .zip(src_stats_store.iter_mut())
                        .zip(tpl_datas.into_iter()),
                )
                .map(
                    |((k, &ext_i), ((stats, src_stats), tpl_data))| pt::TplWindowFrame {
                        picture_number: rows[ext_i].pic.picture_number,
                        valid: member_valid[k] != 0,
                        tpl_data,
                        stats,
                        src_stats,
                        aligned_width: aw as u32,
                        mi_rows: rows[ext_i].pic.mi_rows as i32,
                        mi_cols: rows[ext_i].pic.mi_cols as i32,
                        tpl_src_data_ready: false,
                    },
                )
                .collect();
            let mut recons: alloc::vec::Vec<pt::TplPicMut<'_>> = recon_store
                .iter_mut()
                .map(|buf| pt::TplPicMut {
                    y: buf,
                    y_stride: recon_stride,
                    width: aw as u32,
                    height: ah as u32,
                    border: 32,
                    origin: 32 * recon_stride + 32,
                })
                .collect();

            let mut base_rdmults = alloc::vec![0i32; g.members.len()];
            let sf_identity = svtav1_dsp::port_scale_factors::ScaleFactors::setup_for_frame(
                aw as i32, ah as i32, aw as i32, ah as i32,
            );
            let qp_cfg = qp;
            let ext_off_cfg = ext_off;
            pt::tpl_mc_flow(
                &mut window,
                &mut recons,
                base_synth,
                rows[0].ctrls.compute_rate != 0,
                tpl_lad_mg,
                |args| {
                    let mc = &member_ctxs[args.frame_idx];
                    let row = mc.row;
                    let pic = row.pic;
                    let me = row.me;
                    base_rdmults[args.frame_idx] = pt::tpl_mc_flow_dispenser(
                        qp_cfg,
                        ext_off_cfg,
                        b64_geom,
                        |_sb_index| pt::TplSbCtx {
                            input_pic: {
                                let f = &row.pa.full;
                                pt::TplPic {
                                    y: &f.buf,
                                    y_stride: f.stride,
                                    width: f.width as u32,
                                    height: f.height as u32,
                                    max_width: (f.width + 2 * f.border) as u32,
                                    max_height: (f.height + 2 * f.border) as u32,
                                    origin: f.org,
                                }
                            },
                            tpl_ctrls: *row.ctrls,
                            tpl_data: &args.frame.tpl_data,
                            temporal_layer_index: pic.temporal_layer_index,
                            hierarchical_levels: pic.hierarchical_levels,
                            slice_type: pic.slice_type,
                            update_type: pic.update_type,
                            qp_qindex,
                            extended_crf_qindex_offset: ext_off_cfg,
                            tpl_src_data_ready: args.frame.tpl_src_data_ready,
                            enable_me_16x16: me.map_or(true, |m| m.enable_me_16x16),
                            tpl_lad_mg,
                            sb_origin: (0, 0),
                            aligned_width: aw as u32,
                            aligned16_width: aw.div_ceil(16),
                            mi_rows: pic.mi_rows as i32,
                            mi_cols: pic.mi_cols as i32,
                            max_input_luma_width: tw as u32,
                            max_input_luma_height: th as u32,
                            super_block_size: sb_size as i32,
                            sf_identity,
                            quant: mc.quant,
                            poc_map_idx: args.poc_map_idx,
                            base_tpl_valid_pic: &member_valid,
                        },
                        args.recon,
                        args.ref_pics,
                        &ref_pics,
                        |sb| {
                            let Some(me) = me else {
                                return pt::TplMeResults {
                                    total_me_candidate_index: &[],
                                    me_candidate_array: &[],
                                    me_mv_array: &[],
                                    max_cand: 0,
                                    max_refs: 0,
                                    max_l0: 0,
                                };
                            };
                            let b64 = &me.per_b64[sb];
                            pt::TplMeResults {
                                total_me_candidate_index: &b64.total_me_candidate_index,
                                me_candidate_array: &b64.me_candidate_array,
                                me_mv_array: &b64.me_mv_array,
                                max_cand: me.max_cand,
                                max_refs: me.max_refs,
                                max_l0: me.max_l0,
                            }
                        },
                        args.frame.src_stats,
                        args.frame.stats,
                    );
                    if crate::dbgenv::dispdbg() {
                        let (mut s, mut r, mut d) = (0i64, 0i64, 0i64);
                        for st in args.frame.stats.iter() {
                            s += st.srcrf_dist;
                            r += st.recrf_dist;
                            d += st.mc_dep_dist;
                        }
                        eprintln!(
                            "DISPDBG idx={} poc={} src={} rec={} dep={} rdmult={} nref={},{}",
                            args.frame_idx,
                            pic.picture_number,
                            s,
                            r,
                            d,
                            base_rdmults[args.frame_idx],
                            args.frame.tpl_data.tpl_ref0_count,
                            args.frame.tpl_data.tpl_ref1_count,
                        );
                        for l in 0..2 {
                            for ri in 0..pt::REF_LIST_MAX_DEPTH {
                                eprintln!(
                                    "  REF l{} r{} poc={} sw={} gidx={}",
                                    l,
                                    ri,
                                    args.frame.tpl_data.tpl_ref_ds[l][ri].picture_number,
                                    args.frame.tpl_data.ref_in_slide_window[l][ri] as i32,
                                    args.frame.tpl_data.ref_tpl_group_idx[l][ri],
                                );
                            }
                        }
                    }
                    0
                },
            );

            // `generate_r0beta` per member — each member's own
            // `rc_init_frame_stats` reads the stats this run left in ITS
            // `pa_me_data` (the run only executes for tl0 bases, so these
            // are the values a member actually sees: the last tl0 run that
            // contained it).
            for (k, &ext_i) in g.members.iter().enumerate() {
                let row = &rows[ext_i];
                let flags = crate::rate_control::r0_flags(
                    row.ctrls.enable != 0,
                    row.ctrls.reduced_tpl_group,
                    row.pic.temporal_layer_index,
                    row.pic.hierarchical_levels,
                    row.pic.slice_type == pp::SliceType::I,
                );
                let mut member_factors = alloc::vec![0.0f64; factor_grid_len];
                let mut member_beta = alloc::vec![0.0f64; sb_cnt];
                let rb = if flags.r0_gen {
                    pt::generate_r0beta(
                        base_synth,
                        sb_size as u32,
                        aw as u32,
                        ah as u32,
                        aw as u32,
                        ah as u32,
                        8,
                        row.pic.mi_rows as i32,
                        i64::from(base_rdmults[k]),
                        &stats_store[k],
                        sb_orgs,
                        &mut member_factors,
                        &mut member_beta,
                    )
                } else {
                    member_factors.clear();
                    member_beta.clear();
                    pt::R0Beta {
                        r0: 0.0,
                        tpl_is_valid: false,
                    }
                };
                out.push((ext_i, rb, member_factors, member_beta));
            }
            (g, out)
        }

        // ---------------------------------------------------------------
        // C's ordering: EVERY picture's `initial_rc` builds its own
        // `tpl_group` from its queue tail, but `tpl_mc_flow` only dispenses
        // for tl0 bases. A member's `pa_me_data` stats therefore hold
        // whatever the LAST tl0-base run containing it wrote — the runs
        // below execute in decode order so later writes win, exactly like
        // the shared `pa_me_data` buffers in C.
        // ---------------------------------------------------------------
        let mut stage = TplStageOut {
            frames: (0..n).map(|_| None).collect(),
            key: None,
        };
        // Per-member "the stats a member's rc_init_frame_stats would read":
        // (rb, factors, beta) from the most recent tl0 run containing it.
        let mut last: alloc::vec::Vec<Option<(pt::R0Beta, alloc::vec::Vec<f64>, alloc::vec::Vec<f64>)>> =
            (0..n).map(|_| None).collect();
        // Each member's OWN group shape — `pcs->tpl_group_size` /
        // `used_tpl_frame_num` come from the member's own
        // `store_extended_group`, which runs for every picture regardless
        // of the tl0 flow gate.
        let mut own: alloc::vec::Vec<(u32, u32)> = (0..n).map(|_| (0, 0)).collect();
        let make_fti = |row: &GroupRow<'_>,
                        base_synth: u8,
                        rb: &pt::R0Beta,
                        factors: alloc::vec::Vec<f64>,
                        beta: alloc::vec::Vec<f64>,
                        own: (u32, u32)|
         -> pt::FrameTplIn {
            let pic = row.pic;
            pt::FrameTplIn {
                tpl_ctrls: *row.ctrls,
                synth_blk_size: base_synth,
                flags: crate::rate_control::r0_flags(
                    row.ctrls.enable != 0,
                    row.ctrls.reduced_tpl_group,
                    pic.temporal_layer_index,
                    pic.hierarchical_levels,
                    pic.slice_type == pp::SliceType::I,
                ),
                r0: rb.r0,
                tpl_is_valid: rb.tpl_is_valid,
                tpl_group_size: own.0,
                used_tpl_frame_num: own.1,
                tpl_beta: beta,
                tpl_rdmult_scaling_factors: factors,
                // Bound in the post-run sweep — `rows` still borrows
                // `frame_mes`/`key_frame_me` here.
                frame_me: None,
            }
        };

        // ---------------------------------------------------------------
        // The held key's group: `[key] + window` — the key's lad-queue
        // contents when its `initial_rc` ran (a delayed intra leaves the
        // whole queued mini-GOP behind it).
        // ---------------------------------------------------------------
        if let Some((key_pic, key_ctrls, key_synth, key_pa)) = key_ctx.as_ref() {
            let mut rows: alloc::vec::Vec<GroupRow<'_>> = alloc::vec::Vec::with_capacity(n + 1);
            rows.push(GroupRow {
                pa: key_pa,
                pic: key_pic,
                ctrls: key_ctrls,
                me: None,
            });
            for &i in emit {
                rows.push(GroupRow {
                    pa: ra_pa[i],
                    pic: pics[i].as_ref().unwrap(),
                    ctrls: &member_tpl[i].0,
                    me: frame_mes[i].as_ref(),
                });
            }
            let base_synth = *key_synth;
            let (g, results) = run_group(
                base_synth,
                &rows,
                tpl_lad_mg,
                self.rc_config.qp,
                i32::from(self.rc_config.extended_crf_qindex_offset),
                sb_size,
                aw,
                ah,
                tw,
                th,
                &b64_geom,
                &sb_orgs,
                stats_len(base_synth),
                factor_grid_len(base_synth == 32, key_pic.mi_rows as i32),
                sb_cnt,
                &self.pa_slots,
                &ra_pa,
                Some(key_pa),
            );
            // The key's own `store_extended_group` output IS `g`; member
            // results land in `last` — a later tl0 run may still overwrite
            // them (its group contains the whole window).
            for (ext_i, rb, factors, beta) in results {
                if ext_i == 0 {
                    stage.key = Some(make_fti(
                        &rows[0],
                        base_synth,
                        &rb,
                        factors,
                        beta,
                        (g.members.len() as u32, g.used_tpl_frame_num),
                    ));
                } else {
                    last[emit[ext_i - 1]] = Some((rb, factors, beta));
                }
            }
        }

        // ---------------------------------------------------------------
        // Per-member runs — every picture's `initial_rc` builds its own
        // group from the queue tail `emit[k..]`. `run_group` only writes
        // results for tl0 bases (the `tpl_mc_flow` gate), so `last` ends
        // holding exactly what C's shared `pa_me_data` would: each member's
        // stats as the LAST tl0 run containing it left them.
        // ---------------------------------------------------------------
        for (k, &i) in emit.iter().enumerate() {
            let rows: alloc::vec::Vec<GroupRow<'_>> = emit[k..]
                .iter()
                .map(|&j| GroupRow {
                    pa: ra_pa[j],
                    pic: pics[j].as_ref().unwrap(),
                    ctrls: &member_tpl[j].0,
                    me: frame_mes[j].as_ref(),
                })
                .collect();
            let base_synth = member_tpl[i].1;
            let (g, results) = run_group(
                base_synth,
                &rows,
                tpl_lad_mg,
                self.rc_config.qp,
                i32::from(self.rc_config.extended_crf_qindex_offset),
                sb_size,
                aw,
                ah,
                tw,
                th,
                &b64_geom,
                &sb_orgs,
                stats_len(base_synth),
                factor_grid_len(base_synth == 32, rows[0].pic.mi_rows as i32),
                sb_cnt,
                &self.pa_slots,
                &ra_pa,
                // Out-of-window refs still resolve to the held key — the
                // base frame's L0 reference is the previous GOP's key
                // picture, which is not yet in `pa_slots` at stage time.
                key_ctx.as_ref().map(|k| k.3),
            );
            // `g` is member `i`'s OWN group — record its shape for the fti
            // even when the tl0 gate skipped the flow.
            own[i] = (g.members.len() as u32, g.used_tpl_frame_num);
            for (ext_i, rb, factors, beta) in results {
                last[emit[k + ext_i]] = Some((rb, factors, beta));
            }
        }

        // Materialize each member's `FrameTplIn` from `last` + `own`.
        for (i, f) in stage.frames.iter_mut().enumerate() {
            if let Some((rb, factors, beta)) = last[i].take() {
                let row = GroupRow {
                    pa: ra_pa[i],
                    pic: pics[i].as_ref().unwrap(),
                    ctrls: &member_tpl[i].0,
                    me: None,
                };
                *f = Some(make_fti(&row, member_tpl[i].1, &rb, factors, beta, own[i]));
            }
        }

        // Move each member's `FrameMe` into its `FrameTplIn` — the group
        // `rows` are all dropped now, so the borrows have ended.
        for (i, f) in stage.frames.iter_mut().enumerate() {
            if let Some(fti) = f {
                if let Some(me) = frame_mes[i].take() {
                    fti.frame_me = Some(me);
                }
            }
        }
        // The held key's `frame_me` stays `None` — an I slice ran no
        // open-loop ME.

        Ok(Some(stage))
    }

    fn resolve_sb_size(derived: usize, override_: Option<usize>, preset: i8) -> (usize, bool) {
        let want = override_
            .filter(|n| matches!(n, 64 | 128))
            .unwrap_or(derived);
        debug_assert!(
            want == 64 || want == 128,
            "sb_size must be 64 or 128, got {want}"
        );
        if want == 128 && !Self::sb128_encode_supported(preset) {
            (64, true)
        } else {
            (want, false)
        }
    }

    /// SH `separate_uv_delta_q`. The fork always signals it; an `__expert`
    /// chroma override with distinct U/V deltas needs it too. The SH bit and
    /// the FH [`crate::entropy::obu::ChromaQSignal`] form both read this, so
    /// they cannot disagree.
    fn separate_uv_delta_q(&self) -> bool {
        #[cfg(feature = "__expert")]
        if self
            .chroma_q_override
            .is_some_and(crate::chroma_q::ChromaQOverride::needs_separate_uv)
        {
            return true;
        }
        self.hdr.is_fork()
    }

    /// Produce the decoder-exact reconstruction in `last_recon` /
    /// `last_recon_unfiltered` / `last_recon_pre_cdef` (default: OFF).
    ///
    /// WHY IT IS OFF BY DEFAULT (measured 2026-08-11, Apple M4 Pro): the
    /// reconstruction is not an input to the bitstream on a still frame whose
    /// loop restoration is disabled — which is every preset >= 7, since
    /// `seq_tools_for_preset` turns Wiener off there — so the in-loop deblock
    /// and CDEF *application* passes run purely to materialise it. Skipping
    /// them is byte-inert (90/90 cells of {64,128,256} x p{7,8,9,10,13} x
    /// qp{20,40,55} x {gradient,uniform} unchanged) and buys **1.36-1.39x at
    /// p10/p13 and 1.11-1.15x at p7** on the whole encode (n=9 interleaved
    /// paired rounds/cell, identity control band 0.99-1.02). At preset <= 6
    /// the passes stay on regardless: the CDEF search and the Wiener search
    /// both read the filtered recon, so they feed the bitstream there — and
    /// the same experiment shows 13/36 of those cells change bytes when the
    /// passes are removed.
    ///
    /// The C reference behaves the same way: `svt_av1_enc_get_packet` yields
    /// no reconstruction, and `SvtAv1EncApp` only produces one under `-o`.
    /// Its profile at preset 10 contains zero CDEF and zero loop-filter
    /// samples while emitting byte-identical output.
    ///
    /// Turn it ON for recon parity, PSNR/evidence tooling, or anything that
    /// reads the `last_recon*` fields — they are `None` otherwise.
    pub fn with_recon_output(mut self, enabled: bool) -> Self {
        self.recon_output = enabled;
        self
    }

    /// Pin the superblock size instead of deriving it (`SVTAV1_SB`).
    /// `Some(128)` on a cell whose encode path is unsupported still falls
    /// back to 64 and sets [`Self::sb128_fallback`] — the override chooses
    /// what to ASK for, not what to bypass.
    pub fn with_sb_size(mut self, sb: Option<usize>) -> Self {
        self.sb_size_override = sb;
        let (sb_size, fell_back) =
            Self::resolve_sb_size(self.derived_sb_size, sb, self.speed_config.preset);
        self.sb_size = sb_size;
        self.sb128_fallback = fell_back;
        self
    }

    /// Enable super-resolution at `denom` (9..=16) — superres chunk B.3.
    ///
    /// The frame is then ENCODED at the reduced width `upscaled_w * 8 / denom`
    /// (C `calculate_scaled_size_helper`) and a conforming decoder upscales it
    /// back to the width the caller passed to [`Self::new`], with the
    /// normative 8-tap filter ([`svtav1_dsp::superres`]). The caller keeps
    /// handing in FULL-width planes; the pipeline downscales them
    /// ([`svtav1_dsp::resize`], C `svt_av1_resize_plane_horizontal`).
    ///
    /// Height is unchanged — superres is horizontal only.
    ///
    /// Off by default (denominator 8), matching C. Re-derives the aligned
    /// encode dims and the superblock size from the CODED width, exactly as
    /// [`Self::new`] would have for a frame of that width.
    pub fn with_superres(mut self, denom: u8) -> Self {
        // Preserve an invalid request for the fallible encode entry point;
        // do not divide by it or let it change the working geometry.
        if !(9..=16).contains(&denom) {
            self.superres_denom = Some(denom);
            return self;
        }
        let coded = u32::from(svtav1_dsp::superres::scaled_size(
            self.upscaled_width as u16,
            denom,
        ));
        self.superres_denom = Some(denom);
        self.true_width = coded;
        let dims = crate::frame_geom::FrameDims::new(coded as usize, self.true_height as usize);
        self.width = dims.aligned_w as u32;
        self.height = dims.aligned_h as u32;
        // The SB derivation keys off the ALIGNED dims, which just changed.
        let sb_inputs = crate::sb128_geom::SbSizeInputs {
            qp: self.rc_config.qp,
            allintra: self.gop.intra_period == 1,
            ..Default::default()
        };
        self.derived_sb_size = crate::sb128_geom::derive_super_block_size(
            dims.aligned_w,
            dims.aligned_h,
            self.speed_config.preset as i8,
            &sb_inputs,
        );
        let (sb_size, fell_back) = Self::resolve_sb_size(
            self.derived_sb_size,
            self.sb_size_override,
            self.speed_config.preset,
        );
        self.sb_size = sb_size;
        self.sb128_fallback = fell_back;
        self
    }

    /// Use full AV1 headers for an animated image sequence. All-intra coding
    /// remains selected by the GOP configuration. An all-intra GOP produces
    /// a sync sample for each picture.
    pub fn with_image_sequence(mut self) -> Self {
        self.image_sequence = true;
        self
    }

    /// Set bit depth (8, 10, or 12).
    pub fn with_bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = depth;
        self
    }

    /// Request `TileRowsLog2` tile rows (`1 << log2` tile rows; 0 = single
    /// tile row, the default). Out-of-range requests are clamped exactly
    /// like C (see [`Self::tile_rows_log2`]) rather than rejected.
    pub fn with_tile_rows_log2(mut self, log2: u8) -> Self {
        self.tile_rows_log2 = log2;
        self
    }

    /// Request `TileColsLog2` tile columns (`1 << log2`; 0 = single tile
    /// column, the default). Clamped exactly like C — see
    /// [`Self::tile_cols_log2`].
    pub fn with_tile_cols_log2(mut self, log2: u8) -> Self {
        self.tile_cols_log2 = log2;
        self
    }

    /// Set CICP color description for wide gamut / HDR signaling.
    pub fn with_color_description(mut self, cd: crate::entropy::obu::ColorDescription) -> Self {
        self.color_description = cd;
        self
    }

    /// Set the SH `chroma_sample_position` (0 unknown, 1 vertical, 2
    /// colocated) — see [`Self::chroma_sample_position`]. Values > 2 are
    /// refused at encode time.
    pub fn with_chroma_sample_position(mut self, csp: u8) -> Self {
        self.chroma_sample_position = csp;
        self
    }

    /// C `scs->static_config.pred_structure` (`SVT_PRED_STRUCT`): `LowDelay`
    /// (the default) or `RandomAccess`.
    ///
    /// Random access changes the public contract: `try_encode_frame*` accepts
    /// an input frame and returns `Ok(Vec::new())` until a whole mini-GOP is
    /// buffered, then returns that window's packets concatenated in DECODE
    /// order (which is also the correct bitstream order — AV1 packets are
    /// emitted in decode order, with `order_hint` carrying display order).
    /// Call [`Self::try_flush`] after the last input to drain a partial
    /// trailing mini-GOP, which C codes low-delay (`is_pic_cutting_short_ra_mg`).
    pub fn with_pred_structure(mut self, p: crate::port_picstruct::PredStructure) -> Self {
        self.pred_structure = p;
        // The AUTO arm reads `pred_structure`, so it resolves here where the
        // structure becomes final — mirroring enc_handle, where resolution
        // runs after the config fields are all copied into static_config.
        self.resolve_hierarchical_levels_auto();
        self
    }

    /// C's `hierarchical_levels == HIERARCHICAL_LEVELS_AUTO` resolution
    /// (`Globals/enc_handle.c:4556-4567`), run once the AUTO inputs are
    /// final:
    ///
    /// ```text
    /// pred_structure == LOW_DELAY && (rc == CBR || !(enc_mode <= ENC_M9)) -> 2
    /// pred_structure == LOW_DELAY -> 3
    /// rc == VBR || rc == CBR
    ///   || (input_resolution >= 1080p_RANGE && enc_mode >= ENC_M8)
    ///   || !(enc_mode <= ENC_M8) || input_resolution >= 8K_RANGE -> 4
    /// otherwise -> 5
    /// ```
    ///
    /// then the all-intra clamp (`enc_handle.c:4576-4579`, applied to AUTO
    /// and explicit levels alike in C): `allintra` — `intra_period == 0 ||
    /// avif || pred_structure == ALL_INTRA` in C, `intra_period == 1 ||
    /// PredStructure::AllIntra` here — forces 2. C's single-pass low-delay
    /// CBR clamp (`:4568-4573`) cannot move an AUTO result (the low-delay
    /// arm is already `<= 2` whenever `rc == CBR`), so it is documented,
    /// not run. `input_resolution` is the same class
    /// [`crate::pd0::input_resolution_class`] derives on the ALIGNED luma
    /// count (`scs->max_input_luma_width * height`, enc_handle.c:3992).
    fn resolve_hierarchical_levels_auto(&mut self) {
        if !self.hier_auto {
            return;
        }
        let ld = self.pred_structure == crate::port_picstruct::PredStructure::LowDelay;
        let mode = self.rc_config.mode;
        let em = self.speed_config.preset;
        let res = crate::pd0::input_resolution_class(
            self.width as usize * self.height as usize,
        );
        let mut hier: u8 = if ld && (mode == crate::rate_control::RcMode::Cbr || em > 9) {
            2
        } else if ld {
            3
        } else if matches!(mode, crate::rate_control::RcMode::Vbr | crate::rate_control::RcMode::Cbr)
            || (res >= crate::port_md_winner::INPUT_SIZE_1080P_RANGE && em >= 8)
            || em > 8
            || res >= crate::port_lr_level::INPUT_SIZE_8K_RANGE
        {
            4
        } else {
            5
        };
        if self.gop.intra_period == 1
            || self.pred_structure == crate::port_picstruct::PredStructure::AllIntra
        {
            hier = 2;
        }
        self.gop = GopStructure::new(hier, self.gop.intra_period);
        self.mg_map = crate::port_picstruct::MiniGopMap::for_sequence(hier);
        self.hier_auto = false;
    }

    /// Enable/disable the opt-in 4:2:0 chroma mode (see `chroma_420` field).
    /// `true` is exactly `with_chroma_format(Yuv420)`; `false` withdraws
    /// chroma entirely (mono entry points only).
    pub fn with_chroma_420(mut self, enabled: bool) -> Self {
        self.chroma_420 = enabled;
        self.chroma_format = enabled.then_some(svtav1_types::chroma::ChromaFormat::Yuv420);
        self
    }

    /// Configure the chroma subsampling format directly.
    ///
    /// # Parity contract (measured, not aspirational)
    ///
    /// * [`ChromaFormat::Yuv420`] — the byte-parity surface; identical to
    ///   `with_chroma_420(true)`.
    /// * [`ChromaFormat::Yuv444`] — Zen extension, SHIPPING on the
    ///   8-bit key/still + `sb_size 64` + no-superres envelope:
    ///   decoder-verified (`aomdec`/`dav1d` reconstruction equality,
    ///   `tools/rd_ext_sweep.sh` for the SSIM2/RD oracle), never a
    ///   claim of C parity — C v4.2.0 REFUSES it
    ///   (`Globals/enc_settings.c:470`).
    /// * [`ChromaFormat::Yuv422`] — Zen extension, still staged:
    ///   `try_encode_frame_422` keeps refusing until the arm is proven
    ///   decoder-exact.
    /// * [`ChromaFormat::Yuv400`] — the existing monochrome extension
    ///   (`encode_frame_mono_core`), same no-oracle tier.
    ///
    /// `None` withdraws chroma (same as `with_chroma_420(false)`).
    pub fn with_chroma_format(
        mut self,
        format: Option<svtav1_types::chroma::ChromaFormat>,
    ) -> Self {
        self.chroma_format = format;
        self.chroma_420 = format == Some(svtav1_types::chroma::ChromaFormat::Yuv420);
        self
    }

    /// C `subsampling_x`/`subsampling_y` pair for the configured format
    /// (`enc_handle.c:4636-4637`); `(1, 1)` when chroma is absent — the
    /// value C's own geometry helpers produce on the mono path.
    pub fn subsampling(&self) -> (u8, u8) {
        self.chroma_format
            .map(|f| (f.subsampling_x(), f.subsampling_y()))
            .unwrap_or((1, 1))
    }

    /// Feature 4: bound the tile-parallel encode to at most `n` concurrent OS
    /// threads (`0` = auto via `available_parallelism`). See
    /// [`Self::thread_count`]. Byte-inert — tiles are always reassembled in
    /// tile order — so this only trades throughput against core pressure.
    pub fn with_thread_count(mut self, n: usize) -> Self {
        self.thread_count = n;
        self
    }

    /// Feature 1: install a cooperative cancellation token. Any
    /// [`enough::Stop`] implementation works (e.g. `almost_enough::Stopper`);
    /// it is checked once at the entry of [`Self::try_encode_frame`] /
    /// [`Self::try_encode_frame_420`]. The infallible `encode_frame*` methods
    /// ignore it. See [`Self::stop`].
    pub fn with_stop(mut self, stop: impl enough::Stop + 'static) -> Self {
        self.stop = almost_enough::StopToken::new(stop);
        self
    }

    /// Feature 1 convenience: bound every frame this pipeline encodes with a
    /// wall-clock deadline measured from this call. Composes with any token
    /// already installed via [`Self::with_stop`] (`OrStop` — whichever fires
    /// first wins), so an explicit cancel source is never displaced.
    ///
    /// Expiry surfaces from the fallible `try_encode_frame*` entries as
    /// `Err(Cancelled(StopReason::TimedOut))` — distinguishable from a
    /// caller's explicit `Cancelled`, which is what makes an encode wedged
    /// on adversarial input (or an encoder-side infinite loop) *detectable*
    /// rather than merely killed by an outer `timeout(1)`.
    pub fn with_timeout(self, budget: core::time::Duration) -> Self {
        let existing = self.stop.clone();
        self.with_stop(almost_enough::OrStop::new(
            existing,
            almost_enough::WithTimeout::new(enough::Unstoppable, budget),
        ))
    }

    /// Encode a single frame through the full pipeline (monochrome).
    ///
    /// Returns the encoded bitstream data and updates internal state.
    ///
    /// `y_plane` is (true_w x true_h) at `y_stride`, where the TRUE dims are
    /// what the caller passed to [`Self::new`]. When those differ from the
    /// ALIGNED encode dims the plane is edge-replicated up to the aligned grid
    /// here, exactly as [`Self::encode_frame_420`] does it (C
    /// `pad_input_picture`, pic_operators.c:561); for 8-aligned inputs this is
    /// a zero-copy pass-through and the emitted bytes are unchanged.
    pub fn encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> Vec<u8> {
        // The non-PD0 search now carries a square root through partial SBs
        // and codes the same forced-edge syntax as the entropy walk.
        // Additive fallible core (Feature 1+3). This wrapper KEEPS its exact
        // signature and panicking contract: with the default `Unstoppable`
        // token and the infallible-alloc feature default, the core cannot
        // return `Err` on the trusted path, so `.expect()` never fires and the
        // emitted bytes are unchanged. Callers wanting graceful OOM /
        // cancellation use `try_encode_frame`.
        //
        // "Trusted path" EXCLUDES an unsupported configuration. The config
        // choke point refuses `RcMode::Vbr` (first-pass statistics are not
        // ported) and `RcMode::Cbr` outside LOW_DELAY (C's own envelope),
        // so a caller that builds the pipeline with one and then uses this
        // infallible wrapper panics HERE instead of silently receiving a
        // stream the port never meant to emit. That is the intended trade —
        // a panic is loud, a mislabeled bitstream is not — but the message
        // must say so rather than claim infallibility.
        self.encode_frame_mono_core(y_plane, y_stride).expect(
            "encode_frame is infallible on the default/trusted path; an \
                 UnsupportedConfig here means the pipeline was built with a \
                 configuration this port refuses (e.g. RcMode::Vbr or \
                 non-LOW_DELAY Cbr, issue #22) — use try_encode_frame to \
                 handle it as an error",
        )
    }

    /// Fallible core of the MONOCHROME path: the same TRUE -> ALIGNED edge
    /// replication [`Self::encode_frame_420_core`] performs, then the shared
    /// `encode_frame_impl`. Shared by [`Self::encode_frame`] and
    /// [`Self::try_encode_frame`], which both validate before calling in.
    ///
    /// # Why this exists (and what it replaced)
    ///
    /// Until now the mono entry points REFUSED `width != true_width` outright
    /// — "arbitrary-dims padding is wired on the 4:2:0 path only". The padding
    /// is not 4:2:0-specific: `encode_frame_impl` already takes the padded
    /// plane at the ALIGNED stride and signals `true_width`/`true_height` in
    /// the frame header, and the mono arm differs from the 4:2:0 arm only in
    /// plane count. The refusal cost the AVIF alpha case: an alpha plane is a
    /// MONOCHROME AV1 image at the picture's own, arbitrary size, and
    /// `AvifEncoder::encode_y8` worked around the refusal by pre-padding to a
    /// multiple of 64 while still reporting the caller's true size — so the
    /// coded frame and the size the container would announce DISAGREED for
    /// every non-64-multiple gray image.
    ///
    /// # Evidence tier
    ///
    /// TIER 3, and it can never be better: C v4.2.0 has no monochrome mode at
    /// all (`verify_settings` rejects any `encoder_color_format` other than
    /// `EB_YUV420`, `Globals/enc_settings.c:473`), so there is no byte oracle
    /// for ANY mono cell. The oracle used here is the one the repo already
    /// uses for mono geometry bugs (`tools/regression_spotcheck.sh`'s
    /// `monoReconEq`): the encoder's FINAL reconstruction must equal the
    /// reference decoder's output, plus decodability under aomdec and dav1d.
    fn encode_frame_mono_core(
        &mut self,
        y: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Vec<u8>> {
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        if aw == tw && ah == th {
            // Natively 8-aligned: pass through unchanged (byte-identical to
            // every mono stream this encoder has ever emitted).
            return self.encode_frame_impl(y, y_stride, None, None);
        }
        let y_pad = pad_plane_replicate(y, y_stride, tw, th, aw, ah)?;
        self.encode_frame_impl(&y_pad, aw, None, None)
    }

    /// Encode a single 4:2:0 still/key frame (NumPlanes=3).
    ///
    /// `u`/`v` are (true_w/2 x true_h/2) planes at stride `true_w/2`, and
    /// `y` is (true_w x true_h) at `y_stride`, where the TRUE dims are what
    /// the caller passed to [`Self::new`]. When those differ from the
    /// ALIGNED encode dims (task #95), the planes are edge-replicated up to
    /// the aligned grid here (C `pad_input_picture`, pic_operators.c:561);
    /// for 8-aligned inputs this is a zero-copy pass-through.
    /// Requires `chroma_420` to be enabled via [`Self::with_chroma_420`].
    pub fn encode_frame_420(&mut self, y: &[u8], u: &[u8], v: &[u8], y_stride: usize) -> Vec<u8> {
        assert!(
            self.chroma_420,
            "encode_frame_420 requires the pipeline to be built with with_chroma_420(true)"
        );
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        // TRUE chroma dims (4:2:0 ceiling, matching the input .yuv layout).
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        let cn_true = tcw * tch;
        assert!(
            u.len() >= cn_true && v.len() >= cn_true,
            "u/v planes must be (true_w/2 x true_h/2)"
        );
        // Additive fallible core (Feature 1+3), shared with `try_encode_frame_420`.
        // KEEPS the exact panicking contract: on the default/trusted path the
        // core cannot return `Err`, so `.expect()` never fires and the bytes are
        // unchanged. "Trusted path" EXCLUDES an unsupported configuration — see
        // the note on `encode_frame`; a `RcMode::Vbr` or non-LOW_DELAY `Cbr`
        // pipeline panics here rather than emitting a stream C would refuse
        // or the port cannot compute honestly.
        self.encode_frame_420_core(y, u, v, y_stride).expect(
            "encode_frame_420 is infallible on the default/trusted path; an \
                 UnsupportedConfig here means the pipeline was built with a \
                 configuration this port refuses (e.g. RcMode::Vbr or \
                 non-LOW_DELAY Cbr, issue #22) — use try_encode_frame_420 to \
                 handle it as an error",
        )
    }

    fn film_grain_seed(&self) -> u16 {
        // The zero transition restarts at 7391, so use the actual recurrence's
        // period rather than applying a one-frame correction after wrapping.
        const PERIOD: u64 = 63421;
        7391u16.wrapping_add(3381u16.wrapping_mul((self.frame_count % PERIOD) as u16))
    }
    fn film_grain_reference(
        &self,
        fg: &crate::entropy::obu::FilmGrainParams,
        map: [u8; 7],
        is_b: bool,
    ) -> Option<u8> {
        if self.film_grain.ignore_ref {
            return None;
        }
        // C compares LIST_1[0] (BWDREF), but signals ALTREF's map index.
        // Preserve entropy_coding.c:3126-3131, including its explicit TODO.
        for (compare_kind, signal_kind) in [(0usize, 0usize), (4, 6)] {
            if compare_kind == 4 && !is_b {
                break;
            }
            let reference = self.grain_references[map[compare_kind] as usize].as_ref();
            if reference.is_some_and(|r| crate::film_grain_config::parameters_equal(fg, r)) {
                return Some(map[signal_kind]);
            }
        }
        None
    }
    fn validate_film_grain(&self) -> EncodeResult<()> {
        self.film_grain
            .validate()
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        let enabled =
            self.film_grain.enabled() || (self.hdr.is_fork() && self.hdr.noise_strength > 0);
        if enabled && (!self.chroma_420 || !matches!(self.bit_depth, 8 | 10)) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "C film grain requires 8/10-bit 4:2:0"
            )));
        }
        // bd10 + superres: the denoise pass runs on the u8 canvas BEFORE the
        // downscale (`prepare_film_grain` -> `superres_downscale_420`), but the
        // native-10-bit arm produces that canvas only AFTER filtering u16 —
        // the order C's packed-buffer pipeline guarantees. Feeding the
        // denoiser a canvas that does not exist yet has no honest answer, so
        // the combination is refused rather than denoising a different
        // picture than the one that gets coded. A supplied grain table or the
        // HDR-fork noise path never denoises (same early-out
        // `prepare_film_grain` takes), so they stay allowed.
        let denoise_would_run = self.film_grain.table.is_none()
            && !(self.hdr.is_fork() && self.hdr.noise_strength > 0)
            && self.film_grain.denoise_strength > 0;
        if denoise_would_run && self.bit_depth == 10 && self.superres_denom.is_some() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "film-grain denoise with 10-bit superres is not implemented: the native-input \
                 u8 canvas exists only after the u16 downscale, which runs after denoise — \
                 C's packed-buffer order has no u16 equivalent here [C: accepts]",
            )));
        }
        if self
            .grain_sequence_present
            .is_some_and(|present| present != enabled)
            && !self.gop.is_key_frame(self.frame_count)
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "film grain sequence presence can change only on a key frame"
            )));
        }
        Ok(())
    }
    fn prepare_film_grain(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Option<([Vec<u8>; 3], usize)>> {
        if self.film_grain.table.is_some()
            || (self.hdr.is_fork() && self.hdr.noise_strength > 0)
            || self.film_grain.denoise_strength == 0
        {
            return Ok(None);
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let tw = self.upscaled_width as usize;
        let th = self.true_height as usize;
        // C pads to the mi grid before picture_pre_processing_operations.
        let w = tw.div_ceil(8) * 8;
        let h = th.div_ceil(8) * 8;
        let shift = self.bit_depth - 8;
        let strides = [w, w / 2, w / 2];
        let source = if let Some(src) = self.hbd_source.as_ref() {
            [src.y.clone(), src.u.clone(), src.v.clone()]
        } else {
            let padded = [
                pad_plane_replicate(y, y_stride, tw, th, w, h)?,
                pad_plane_replicate(
                    u,
                    tw.div_ceil(2),
                    tw.div_ceil(2),
                    th.div_ceil(2),
                    w / 2,
                    h / 2,
                )?,
                pad_plane_replicate(
                    v,
                    tw.div_ceil(2),
                    tw.div_ceil(2),
                    th.div_ceil(2),
                    w / 2,
                    h / 2,
                )?,
            ];
            padded.map(|p| p.into_iter().map(|v| u16::from(v) << shift).collect())
        };
        let (denoised, params, _status) = crate::film_grain_denoise::denoise_and_model(
            source.each_ref().map(Vec::as_slice),
            w,
            h,
            strides,
            self.bit_depth,
            self.film_grain.denoise_strength,
            self.film_grain.adaptive,
            self.film_grain_seed(),
        );
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let apply = self.film_grain.denoise_apply && params.apply_grain;
        self.prepared_grain = Some(params);
        if !apply {
            return Ok(None);
        }
        // Keep C's denoised padding for the encode; repadding a cropped
        // denoised picture would replace actual filter output at the edges.
        let (ow, oh) = if self.superres_denom.is_some() {
            (tw, th)
        } else {
            (w, h)
        };
        let tight: [Vec<u8>; 3] = core::array::from_fn(|c| {
            let sub = usize::from(c > 0);
            let pw = ow.div_ceil(1 << sub);
            let ph = oh.div_ceil(1 << sub);
            let mut out = Vec::with_capacity(pw * ph);
            for row in 0..ph {
                out.extend(
                    denoised[c][row * strides[c]..row * strides[c] + pw]
                        .iter()
                        .map(|&v| (v >> shift) as u8),
                );
            }
            out
        });
        if self.bit_depth == 10 {
            let [y, u, v] = denoised;
            self.hbd_source = Some(HbdSource { y, u, v });
        }
        Ok(Some((tight, ow)))
    }

    /// Fallible core of the 4:2:0 path (TRUE->ALIGNED padding + the shared
    /// `encode_frame_impl`). Shared by the panicking [`Self::encode_frame_420`]
    /// wrapper and the fallible [`Self::try_encode_frame_420`]; both validate
    /// the chroma flag + plane sizes before calling in.
    fn encode_frame_420_core(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        let result = self.encode_frame_420_prepared(y, u, v, y_stride);
        // Also clear on errors before encode_frame_impl takes these fields.
        self.prepared_grain = None;
        self.hbd_source = None;
        self.hbd_superres_src = None;
        result
    }

    fn encode_frame_420_prepared(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Vec<u8>> {
        // Superres chunk B.3: the caller hands in FULL-width planes; the
        // encode runs at the reduced CODED width. Downscale horizontally with
        // C's `svt_av1_resize_plane_horizontal` (svtav1-dsp::resize, pinned
        // byte-exact vs C) after grain preprocessing, then the existing
        // TRUE->ALIGNED padding and the whole pipeline operate on the coded
        // planes. No-op when superres is off.
        if let Some(why) = self.chroma_format_support_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        self.validate_film_grain()?;
        self.prepared_grain = None;
        let prepared = self.prepare_film_grain(y, u, v, y_stride)?;
        if self.superres_denom.is_none() {
            if let Some((planes, stride)) = prepared.as_ref() {
                return self.encode_frame_impl(
                    &planes[0],
                    *stride,
                    Some((&planes[1], &planes[2])),
                    None,
                );
            }
        }
        let (y, u, v, y_stride) = match prepared.as_ref() {
            Some((planes, stride)) => (
                planes[0].as_slice(),
                planes[1].as_slice(),
                planes[2].as_slice(),
                *stride,
            ),
            None => (y, u, v, y_stride),
        };
        let downscaled = self.superres_downscale_420(y, u, v, y_stride)?;
        let (y, u, v, y_stride) = match downscaled.as_ref() {
            Some((yd, ud, vd)) => (
                yd.as_slice(),
                ud.as_slice(),
                vd.as_slice(),
                self.true_width as usize,
            ),
            None => (y, u, v, y_stride),
        };
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (aw, ah) = (self.width as usize, self.height as usize);
        // Task #95 chunk 2: partial SBs are now supported. Every 4:2:0 KEY
        // frame routes through the PD0 fixed-tree path (use_funnel is always
        // live for 4:2:0 key), which starts from a 64x64 root carrying the
        // spec-5.11.4 forced edge splits and codes the partition symbols with
        // the edge-aware alphabets (encode_partition_av1). The only invariant
        // is that the ALIGNED dims are a multiple of MIN_BLOCK_SIZE (8), which
        // FrameDims guarantees by construction.
        debug_assert!(
            aw % crate::frame_geom::MIN_BLOCK_SIZE == 0
                && ah % crate::frame_geom::MIN_BLOCK_SIZE == 0,
            "aligned dims must be 8-aligned; got {aw}x{ah} for true {tw}x{th}"
        );
        // TRUE chroma dims — format-derived (ChromaFormat's ceiling);
        // at Yuv420 this is exactly `div_ceil(2)`, byte-identical.
        let fmt = self
            .chroma_format
            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
        let (tcw, tch) = (fmt.chroma_width(tw), fmt.chroma_height(th));
        if aw == tw && ah == th {
            // Natively 8-aligned: pass through unchanged (byte-identical to
            // the pre-#95 path).
            return self.encode_frame_impl(y, y_stride, Some((u, v)), None);
        }
        // Pad TRUE -> ALIGNED. C replicates the last valid column, then the
        // last valid row (incl. the new right pad); the per-pixel min-clamp
        // in `pad_plane_replicate` is equivalent for a rectangular region.
        let (acw, ach) = (fmt.chroma_width(aw), fmt.chroma_height(ah));
        let y_pad = pad_plane_replicate(y, y_stride, tw, th, aw, ah)?;
        let u_pad = pad_plane_replicate(u, tcw, tcw, tch, acw, ach)?;
        let v_pad = pad_plane_replicate(v, tcw, tcw, tch, acw, ach)?;
        self.encode_frame_impl(&y_pad, aw, Some((&u_pad, &v_pad)), None)
    }

    /// Whether the configured [`Self::chroma_format`] is consumable by
    /// this build. The staged-444 contract: `Some(Yuv420)` is the only
    /// C-parity surface; every other format refuses HERE, at the entry
    /// choke point, until its path is proven decoder-exact — C refuses
    /// all of them anyway (`verify_settings`, enc_settings.c:470), so a
    /// refusal is both the honest answer and the C-matching one.
    fn chroma_format_support_error(&self) -> Option<&'static str> {
        match self.chroma_format {
            None | Some(svtav1_types::chroma::ChromaFormat::Yuv420) => None,
            Some(svtav1_types::chroma::ChromaFormat::Yuv400) => Some(
                "ChromaFormat::Yuv400 is the monochrome extension — use the mono \
                 entry points (try_encode_frame / encode_y8), which carry no \
                 chroma planes",
            ),
            // 4:4:4 is ungated at the entry — `encode_frame_impl` carries the
            // real envelope gate (8-bit key/still, SB64, no superres) with the
            // refusal text naming each unported piece.
            Some(svtav1_types::chroma::ChromaFormat::Yuv444) => None,
            Some(f) => Some(match f {
                svtav1_types::chroma::ChromaFormat::Yuv422 => {
                    "ChromaFormat::Yuv422 is not yet ported: chroma geometry is \
                     still derived for 4:2:0 (C itself refuses it at \
                     verify_settings, enc_settings.c:470 — no byte oracle)"
                }
                _ => unreachable!(),
            }),
        }
    }

    /// Fallible twin of [`Self::encode_frame`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame`] on success. The difference is
    /// purely at the boundary: the legacy `assert!`s become typed
    /// [`EncodeError`]s, and the cooperative cancellation token
    /// ([`Self::stop`]) is checked once at entry. The legacy method is left
    /// untouched. Internally this calls the same fallible `encode_frame_impl`;
    /// its configuration, allocation and cancellation errors propagate.
    pub fn try_encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> EncodeResult<Vec<u8>> {
        if let Some(e) = self.ra_entry_error("try_encode_frame") {
            return Err(e);
        }
        // (a) Validate the true input extent. Padding is performed in
        // encode_frame_mono_core; both partition paths handle partial SBs.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        if y_plane.len() < (th - 1) * y_stride + tw {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "monochrome luma plane must cover the true dims at y_stride",
            }));
        }
        // (b) Feature 1 entry stop-check (frame-granular).
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        // (c) The fallible core (asserts above pre-satisfied). Its own in-loop
        // stop-checks + fallible allocations propagate here as `Err` instead of
        // panicking/aborting; on success the bytes match `encode_frame`.
        self.encode_frame_mono_core(y_plane, y_stride)
    }

    /// Fallible twin of [`Self::encode_frame_420`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame_420`] on success. The legacy
    /// `assert!`s (chroma flag, u/v plane sizes, still/key-only) become typed
    /// [`EncodeError`]s and the cancellation token is checked at entry;
    /// otherwise it delegates to the fallible core, which pads the true
    /// dimensions to the aligned canvas and calls `encode_frame_impl`.
    pub fn try_encode_frame_420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        // (a) Validate — mirror the `encode_frame_420` + impl asserts.
        if !self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "encode_frame_420 requires the pipeline to be built with with_chroma_420(true)",
            )));
        }
        // Random access changes the contract: an input may produce no packet
        // until its mini-GOP completes; `try_flush` drains the tail.
        if self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess {
            return self.try_encode_frame_420_ra(y, u, v, y_stride);
        }
        // INTER FRAMES ARE SHIPPED HERE as of 2026-09-11. This used to be a
        // blanket "still/key frames only" refusal, lifted only by
        // `SVTAV1_INTER_EXPERIMENTAL`; both that refusal and that variable are
        // gone. What replaced them is `tools/video_selfcheck_gate.sh`: the
        // port's own final reconstruction is byte-identical to `aomdec`'s for
        // every frame of an 8-frame encode, on all six public-domain derf
        // clips at qp {20,40,55} — 18 of 18 cells. The configuration envelope
        // is still enforced, by `gop_config_error` (flat low-delay P only) and
        // the other `*_config_error` guards, so an unwired GOP shape is
        // refused exactly as before.
        //
        // See `encode_frame_impl` for the parity claim this does NOT make.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        let cn_true = tcw * tch;
        if u.len() < cn_true || v.len() < cn_true {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "u/v planes must each be at least (true_w/2 x true_h/2)",
            }));
        }
        // (b) Feature 1 entry stop-check (frame-granular).
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        // (c) The fallible core (padding + encode_frame_impl), NOT the panicking
        // `encode_frame_420` wrapper — so a fallible alloc / cancellation
        // surfaces as `Err` here instead of unwinding through `.expect()`.
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// Does this configuration actually CONSUME a native 10-bit source?
    ///
    /// The full-RD color funnel and the level re-encode post-pass consume
    /// real u16 samples, including partial superblocks. Monochrome has only
    /// the latter consumer at every preset. Reject configurations without a
    /// consumer so the caller's low bits cannot be silently discarded.
    fn hbd_source_consumed(&self, chroma_420: bool) -> bool {
        self.bd10_levels_native(chroma_420)
    }

    /// FH `frm_hdr->tx_mode == TX_MODE_SELECT` for this frame.
    ///
    /// ONE source for the header writer and the pack walk: the signalled mode
    /// and the coded symbols must agree or the stream does not decode (see
    /// `EntropyCtx::tx_mode_select`). `crate::txs_arm::tx_mode_select` is the
    /// ladder; the allintra arm is unconditional TX_MODE_SELECT, the video arm
    /// signals it only while `pcs->txs_level != 0`.
    fn frame_tx_mode_select(&self, temporal_layer: u8) -> bool {
        let arm = if self.gop.intra_period == 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: true }
        };
        crate::txs_arm::tx_mode_select(
            arm,
            crate::rate_arm::eff_enc_mode(arm, self.speed_config.preset),
            // enc_mode_config.c:9177-9185: the video ladder's `is_base` is
            // `temporal_layer_index == 0`, NOT `frame_is_boosted` — a
            // non-base layer drops `txs_level` to 0 and the header signals
            // TX_MODE_LARGEST.
            temporal_layer == 0,
            u32::from(self.rc_config.qp),
        )
    }

    /// Whether the full-RD color funnel or native level re-encode post-pass
    /// produces 10-bit coded levels. Shared by native-input and bit-depth guards.
    fn bd10_levels_native(&self, chroma_420: bool) -> bool {
        if self.bit_depth != 10 {
            return false;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // Monochrome builds no funnel (`use_funnel` requires 4:2:0). The
        // native level pass carries real coefficient contexts and the coded
        // parent partition's directional availability at every preset. The
        // sequence header disables mono edge filtering. Mode decision still
        // uses the upper eight bits; coded levels use all ten.
        let preset = self.speed_config.preset;
        // NO GEOMETRY TERM (2026-08-04). Both bd10 level producers are now
        // partial-SB aware: the full-RD funnel (preset <= 8) rides the shared,
        // already-correct partition search and leaf funnel, and the level-only
        // re-encode post-pass (preset >= 9) got SB-extent recon buffers,
        // straddle-clipped writes, SB-extent-padded sources, and the pack's
        // skip-off-frame-quadrant child walk. Both are gated per-CELL below
        // rather than by dimension.
        if !chroma_420 {
            return true;
        }
        // A capability QUERY, not a per-frame decision: it answers "can this
        // configuration serve 10-bit at all", so it asks the I-slice arm --
        // which is the one an all-intra caller takes on every frame.
        preset >= 9 || bd10_full_rd_supported(false, self.bit_depth, preset, chroma_420, true, w, h)
    }

    /// C `scs->tpl` for THIS pipeline's configuration
    /// ([`crate::inter_hdr_arm::scs_tpl`], a port of `get_tpl`,
    /// `Globals/enc_handle.c:3657`).
    ///
    /// Computed rather than written as a literal so the TPL-dependent
    /// refusals (`mfmv_level >= 2`) engage exactly when C's machinery does:
    /// `aq_mode == 2` under random access, where `run_tpl_stage` now runs
    /// `tpl_mc_flow` and `generate_r0beta` per base picture.
    fn scs_tpl(&self) -> bool {
        crate::inter_hdr_arm::scs_tpl(
            self.gop.intra_period == 1,
            self.rc_config.aq_mode,
            // C's third disabling condition is `pred_structure == LOW_DELAY`.
            // The RA window path is the only non-low-delay encode this
            // pipeline runs, so TPL can engage exactly there.
            self.pred_structure != crate::port_picstruct::PredStructure::RandomAccess,
            self.superres_denom.is_some(),
        )
    }

    /// GLOBAL MOTION — the frame's own decision, not the preset's.
    ///
    /// C `svt_aom_derive_gm_level` (`enc_mode_config.c:194`) gives a NON-I
    /// slice `svt_aom_get_gm_core_level(enc_mode, super_res_off)`: 2 at
    /// `enc_mode <= ENC_MR`, 4 at `<= ENC_M4`, 0 above. A non-zero level only
    /// means C BUILDS `gm_ctrls`, though — whether it SEARCHES is
    /// `svt_aom_global_motion_estimation`'s own derivation
    /// (`crate::port_global_me`), and that is gated on
    /// `average_me_sad = sum(rc_me_distortion) / (w*h) >= 1` plus
    /// `bypass_based_on_me`.
    ///
    /// This site used to refuse on the LEVEL alone, i.e. on the preset, which
    /// is wrong in both directions of usefulness: it refused every inter frame
    /// at preset <= 4 including the ones where C provably codes seven
    /// `is_global = 0` bits — which is exactly what this port writes. MEASURED
    /// 2026-09-05 with `SVT_GM_OUT`: `avg_me_sad = 0` and `is_gm_on = 0` on
    /// {gradient, diag, screen} x {64, 128, 256, 512} and on `crop:` CID22
    /// photo at 256/512 with shifts 3/13/37 — the whole existing grid. With
    /// the refusal replaced by this derivation those cells encode, and 26 of
    /// them are byte-identical to C on both frames.
    ///
    /// When the derivation says C WOULD search, the port now RUNS the search
    /// and codes its result: `port_global_me::set_global_motion_field` builds
    /// the frame's `global_motion[]`, `port_entropy_inter::gm` writes
    /// `global_motion_params()`, and the same array feeds the MVP walk's
    /// `gm_mv`, the injector's GLOBALMV candidates and the prediction's
    /// `is_wm`. Only a frame whose search cannot RUN is still refused. See
    /// `tools/global_motion_gate.sh`.
    ///
    /// `super_res_off` is `true` at every call here: superres refuses inter
    /// frames at `superres_config_error`, and C's own `gm_level` only DROPS
    /// to 0 when superres is on, so `true` is the conservative reading.
    fn gm_level_for_frame(&self, is_key: bool) -> u8 {
        crate::port_enc_mode_config::leaf::derive_gm_level(
            i8::try_from(self.speed_config.preset).unwrap_or(i8::MAX),
            is_key,
            /*super_res_off=*/ true,
        )
    }

    /// The refusal itself, once the frame's ME results exist.
    ///
    /// `None` = every reference keeps IDENTITY, which is what the header and
    /// the MVP environment already write.
    fn gm_search_config_error(
        gm: Option<&crate::port_global_me::GmEstimation>,
        models: Option<&crate::port_global_me::GmModels>,
    ) -> Option<&'static str> {
        // No derivation at all (a key frame, or a preset where `gm_ctrls` are
        // disabled) — C memsets `is_global_motion` false and every model is
        // IDENTITY, which is what the header writes.
        let Some(g) = gm else { return None };
        // The derivation alone proved no search runs.
        if g.all_identity() {
            return None;
        }
        // The search RAN and left every reference IDENTITY. C's own
        // `pcs->is_gm_on` is 0 there, `global_motion[]` is identity, and the
        // header's seven zero bits are correct — so this frame encodes.
        if models.is_some_and(|m| !m.is_gm_on) {
            return None;
        }
        if crate::dbgenv::gm_experimental() {
            return None;
        }
        // THE SEARCH FINDING A MODEL IS NO LONGER A REFUSAL. `global_motion[]`
        // is built by `port_global_me::set_global_motion_field`, the header
        // codes it through `port_entropy_inter::gm::write_global_motion`, and
        // mode decision prices GLOBALMV against it — see the wiring below.
        //
        // What remains refused is the SEARCH not running at all, which is a
        // harness gap (a missing picture-analysis reference, or a downsample
        // level whose `svt_aom_upscale_wm_params` is unported) rather than a
        // picture-decision outcome. Treating it as IDENTITY would claim C found
        // nothing when the port simply did not look.
        if models.is_none() {
            return Some(
                "global motion is not implemented for this frame: C's \
                 svt_aom_global_motion_estimation would search (global_me.c:190), and this \
                 port could not run the search — the picture-analysis reference for a \
                 (list, ref) slot the search needs is missing, or the derived downsample \
                 level is not GM_FULL (crate::port_global_me::GmSearchError) [C: accepts]",
            );
        }
        None
    }

    /// Config knobs C rejects in `svt_av1_verify_settings`, refused here so
    /// the port never encodes a config the oracle cannot (issue #9):
    ///
    /// * `hdr.max_tx_size` must be 32 or 64 (enc_settings.c:922);
    /// * `rc_config.extended_crf_qindex_offset`: C's `str_to_crf` only ever
    ///   produces 0..=3 below qp 63, and `verify_settings` (:270) caps the
    ///   qp-63 extended range at 7*4 = 28 (CRF 70);
    /// * `chroma_sample_position` must be 0 (unknown), 1 (vertical) or 2
    ///   (colocated) — 3 is reserved and rejected (:762-770).
    fn knob_config_error(&self) -> Option<&'static str> {
        if !matches!(self.hdr.max_tx_size, 32 | 64) {
            return Some("max_tx_size must be 32 or 64 (C verify_settings, enc_settings.c:922)");
        }
        let off = self.rc_config.extended_crf_qindex_offset;
        if (self.rc_config.qp < 63 && off > 3) || off > 28 {
            return Some(
                "extended_crf_qindex_offset must be 0..=3 (a quarter-step fractional CRF) or, at \
                 qp 63, at most 28 (CRF 70) — C verify_settings, enc_settings.c:270",
            );
        }
        if self.chroma_sample_position > 2 {
            return Some(
                "chroma_sample_position must be 0 (unknown), 1 (vertical) or 2 (colocated); 3 is \
                 reserved (C verify_settings, enc_settings.c:762)",
            );
        }
        // The fork/mainline tuning-knob bounds, `svt_av1_verify_settings`
        // (enc_settings.c:894-980). Without them a value past C's range
        // wraps or clamps silently inside a formula (`10 + (4 - s)` goes
        // negative for s > 4) — C's check exists precisely so the computed
        // shift factor stays in 0..=14.
        if self.hdr.tf_strength > 4 {
            return Some("tf_strength must be 0..=4 (C verify_settings, enc_settings.c:894)");
        }
        if self.hdr.noise_norm_strength > 4 {
            return Some(
                "noise_norm_strength must be 0..=4 (C verify_settings, enc_settings.c:945)",
            );
        }
        if self.hdr.kf_tf_strength > 4 {
            return Some("kf_tf_strength must be 0..=4 (C verify_settings, enc_settings.c:950)");
        }
        if self.hdr.sharp_tx > 1 {
            return Some("sharp_tx must be 0 or 1 (C verify_settings, enc_settings.c:955)");
        }
        if self.hdr.tx_bias > 3 {
            return Some("tx_bias must be 0..=3 (C verify_settings, enc_settings.c:960)");
        }
        if self.hdr.complex_hvs > 1 {
            return Some("complex_hvs must be 0 or 1 (C verify_settings, enc_settings.c:965)");
        }
        if self.hdr.noise_adaptive_filtering > 4 {
            return Some(
                "noise_adaptive_filtering must be 0..=4 (C verify_settings, enc_settings.c:970)",
            );
        }
        if !(1..=30).contains(&self.hdr.cdef_scaling) {
            return Some("cdef_scaling must be 1..=30 (C verify_settings, enc_settings.c:975)");
        }
        // Issue #9 item 8 — `aq_mode` semantics. C's `--aq-mode` default is
        // 2, which selects the TPL-gated per-SB deltaq
        // (`svt_aom_sb_qp_derivation_tpl_la`, rc_aq.c:899): live wherever
        // `tpl_ctrls.enable && r0 != 0` — i.e. random-access structures —
        // and INERT on stills and low-delay, exactly as C's `get_tpl`
        // disables the machinery there (enc_handle.c:3657). The port now
        // runs the same dispatch: `run_tpl_stage` produces r0/beta/scaling
        // for the RA window and the `rc_init_sb_qindex` twin consumes it.
        //
        // `aq_mode` 1 (variance AQ) and 3 (complexity AQ) are C's OTHER
        // per-SB deltaq derivations and are not ported — refuse rather than
        // silently take the TPL path for a knob that means something else.
        if !matches!(self.rc_config.aq_mode, 0 | 2) {
            return Some(
                "aq_mode must be 0 or 2: 2 is C's default TPL-gated per-SB deltaq \
                 (rc_aq.c:899), ported and live under random access; 1 (variance AQ) \
                 and 3 (complexity AQ) are different C derivations that are not \
                 ported [C: accepts]",
            );
        }
        None
    }

    /// Bit-depth configurations this encoder cannot encode faithfully, refused
    /// at the [`Self::encode_frame_impl`] choke point rather than emitted.
    ///
    /// Two distinct failure shapes:
    ///
    /// 1. **Any depth other than 8 or 10.** C v4.2.0 rejects those itself
    ///    (`svt_av1_verify_settings`, `Globals/enc_settings.c:460`), and this
    ///    port has no 12-bit path at all: `deblock::pick_filter_levels_key_frame`
    ///    hits `unreachable!()` at preset >= 6, and below that the sequence
    ///    header would advertise `seq_profile = 2` without the spec-5.5.2
    ///    subsampling bits that profile requires — an unparseable SH.
    ///
    /// 2. **`bit_depth == 10` with no bd10 producer** (see
    ///    [`Self::bd10_levels_native`]). Outside that envelope the entire
    ///    encode runs in the 8-bit domain — `quant::build_quant_table` takes no
    ///    bit-depth parameter, so the levels are Q8 — while the sequence header
    ///    signals `high_bitdepth = 1`. The decoder then dequantizes with the Q10
    ///    tables and reconstructs a picture the encoder never saw; because Q10
    ///    is only *approximately* 4x Q8, the error compounds through intra
    ///    prediction across the frame. On top of that the deblock levels
    ///    (preset >= 6) and CDEF strengths (preset >= 7) are signalled from the
    ///    bd10 closed forms while being applied by the encoder with the u8
    ///    kernels — three independent scale errors in one stream.
    ///
    ///    That output is decodable and looks like a successful encode at the
    ///    integration seam, which is exactly the class `rust/CLAUDE.md`
    ///    ("Refuse out-of-envelope configs; never emit a plausible-but-wrong
    ///    stream") forbids. The native producers now cover both formats at
    ///    every preset; retain the defensive check for future configuration gaps.
    fn bit_depth_config_error(&self, chroma_420: bool) -> Option<&'static str> {
        match self.bit_depth {
            8 => return None,
            10 => {}
            _ => {
                // WORDING IS LOAD-BEARING HERE. `tools/refusal_inventory.sh`
                // classifies a refusal by its words, and the previous text
                // ended "and this port has no 12-bit kernels" — which matched
                // the CAPABILITY keyword list and filed a PERMANENT UPSTREAM
                // constraint in the table whose header says "this is DEBT".
                // It is not debt: C rejects the config at
                // `svt_av1_verify_settings`, so no oracle exists for any other
                // depth and implementing 12-bit kernels would put this encoder
                // OUTSIDE the envelope it is measured against. The identical
                // rule in `svtav1/src/avif.rs` was already filed as CONTRACT,
                // so the same constraint sat in both halves of the ledger.
                return Some(
                    "bit depth must be 8 or 10 — C v4.2.0 rejects every other depth at encoder \
                     init (svt_av1_verify_settings, Globals/enc_settings.c:460), so no oracle \
                     exists at any other depth: this is C's envelope, not this port's backlog \
                     [C: rejects]",
                );
            }
        }
        if self.bd10_levels_native(chroma_420) {
            return None;
        }
        // DEFENSIVE, AND PROVEN UNREACHABLE TODAY. Native mono levels cover
        // all presets; color has `preset >= 9 || preset <= 8`, so no
        // caller can land here. It is kept because the alternative is an
        // implicit `None` that would let a FUTURE bd10 gap encode 8-bit
        // levels under a 10-bit sequence header, which is the exact failure
        // this predicate exists to stop. `bd10_config_error_third_arm_is_
        // unreachable_over_the_whole_product` pins the claim, so if a new gap
        // ever makes it reachable that test fails and demands a message
        // naming the real gap instead of this catch-all.
        Some(
            "this 10-bit configuration has no bd10 stage to produce the coded levels; the encode \
             would be 8-bit-quantized under a 10-bit sequence header (defensive catch-all — \
             unreachable in the shipped envelope, see the unreachability test) [C: accepts]",
        )
    }

    /// Superres chunk B.3: horizontally downscale the caller's FULL-width
    /// 4:2:0 planes to the coded width. `None` when superres is off (the
    /// planes are used as passed — zero copies, byte-identical path).
    ///
    /// Chroma is resized at its OWN widths (4:2:0 ceiling on both sides), so
    /// the coded chroma width is `(coded_w + 1) / 2` — the same rounding the
    /// rest of the pipeline uses.
    #[allow(clippy::type_complexity)] // ported C signature: a `type` alias here would hide the shape and churn the byte-identity gate for no benefit
    fn superres_downscale_420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> crate::EncodeResult<Option<(Vec<u8>, Vec<u8>, Vec<u8>)>> {
        let Some(_denom) = self.superres_denom else {
            return Ok(None);
        };
        // Native 10-bit arm (bd10 + superres): the staged u16 source is
        // downscaled at full precision and only THEN truncated to the u8
        // canvas — C's `svt_aom_resize_frame` pack -> `highbd_resize` ->
        // unpack order (resize.c). The u8 plane arguments are unused on this
        // arm; see `superres_downscale_420_hbd`.
        if let Some(src) = self.hbd_superres_src.take() {
            return self.superres_downscale_420_hbd(&src).map(Some);
        }
        let (uw, th) = (self.upscaled_width as usize, self.true_height as usize);
        let cw = self.true_width as usize;
        let (ucw, uch) = (uw.div_ceil(2), th.div_ceil(2));
        let ccw = cw.div_ceil(2);
        let mut yd = svtav1_types::try_vec![0u8; cw * th]?;
        let mut ud = svtav1_types::try_vec![0u8; ccw * uch]?;
        let mut vd = svtav1_types::try_vec![0u8; ccw * uch]?;
        svtav1_dsp::resize::resize_plane_horizontal(y, th, uw, y_stride, &mut yd, cw, cw);
        svtav1_dsp::resize::resize_plane_horizontal(u, uch, ucw, ucw, &mut ud, ccw, ccw);
        svtav1_dsp::resize::resize_plane_horizontal(v, uch, ucw, ucw, &mut vd, ccw, ccw);
        // Keep the ORIGINAL luma (tightened to `uw` stride) for the picture
        // statistics C derives before scaling — see `superres_stats_luma`.
        let mut orig = svtav1_types::try_vec![0u8; uw * th]?;
        for r in 0..th {
            orig[r * uw..(r + 1) * uw].copy_from_slice(&y[r * y_stride..r * y_stride + uw]);
        }
        self.superres_stats_luma = Some((orig, uw, th));
        Ok(Some((yd, ud, vd)))
    }

    /// Superres chunk B.3, native 10-bit arm: horizontally downscale the
    /// staged u16 source to the coded width with C's highbd resize ladder
    /// (`svt_av1_highbd_resize_plane_horizontal`, pinned byte-exact by
    /// `c_parity_resize_hbd`), then derive BOTH source representations the
    /// rest of the frame needs:
    ///
    /// * `hbd_source` — the coded-width u16 planes padded TRUE -> ALIGNED,
    ///   the same contract the non-superres bd10 arm establishes for the
    ///   bd10 consumers (MD funnel + level re-encode). This is C's unpacked
    ///   `enhanced_downscaled_pic` u16 buffer.
    /// * the returned u8 planes — `filtered_u16 >> 2`, C's `y_buffer` MSB
    ///   plane of the packed picture. Deriving them from the FILTERED u16
    ///   (not by u8-resizing an already-truncated source) is the
    ///   byte-parity-critical ordering — the two orders differ by the
    ///   low bits the filters accumulate.
    ///
    /// Picture statistics still read the FULL-resolution luma (truncated to
    /// u8, the same canvas C's `enhanced_pic->y_buffer` carries), matching
    /// the 8-bit arm's `superres_stats_luma` contract.
    fn superres_downscale_420_hbd(
        &mut self,
        src: &HbdSuperresSrc,
    ) -> crate::EncodeResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let (uw, th) = (self.upscaled_width as usize, self.true_height as usize);
        let cw = self.true_width as usize;
        let (ucw, uch) = (uw.div_ceil(2), th.div_ceil(2));
        let ccw = cw.div_ceil(2);
        let bd = i32::from(self.bit_depth);
        // The filtered coded-width u16 planes.
        let mut yd = svtav1_types::try_vec![0u16; cw * th]?;
        let mut ud = svtav1_types::try_vec![0u16; ccw * uch]?;
        let mut vd = svtav1_types::try_vec![0u16; ccw * uch]?;
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.y, th, uw, uw, &mut yd, cw, cw, bd,
        );
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.u, uch, ucw, ucw, &mut ud, ccw, ccw, bd,
        );
        svtav1_dsp::port_resize_hbd::highbd_resize_plane_horizontal(
            &src.v, uch, ucw, ucw, &mut vd, ccw, ccw, bd,
        );
        // The coded u16 canvas is what every bd10 consumer reads — pad
        // TRUE(coded) -> ALIGNED exactly as the non-superres arm does.
        let (aw, ah) = (self.width as usize, self.height as usize);
        self.hbd_source = Some(HbdSource {
            y: pad_plane_replicate_u16(&yd, cw, cw, th, aw, ah)?,
            u: pad_plane_replicate_u16(&ud, ccw, ccw, uch, aw / 2, ah / 2)?,
            v: pad_plane_replicate_u16(&vd, ccw, ccw, uch, aw / 2, ah / 2)?,
        });
        // The u8 canvas, unpacked AFTER filtering (C's MSB plane of the
        // resized packed picture).
        let shift = u32::from(self.bit_depth - 8);
        let mut y8 = svtav1_types::try_vec![0u8; cw * th]?;
        for (d, &s) in y8.iter_mut().zip(yd.iter()) {
            *d = (s >> shift) as u8;
        }
        let mut u8p = svtav1_types::try_vec![0u8; ccw * uch]?;
        let mut v8p = svtav1_types::try_vec![0u8; ccw * uch]?;
        for ((du, dv), (&su, &sv)) in u8p
            .iter_mut()
            .zip(v8p.iter_mut())
            .zip(ud.iter().zip(vd.iter()))
        {
            *du = (su >> shift) as u8;
            *dv = (sv >> shift) as u8;
        }
        // Full-resolution luma for the pre-scaling picture statistics —
        // the same u8-truncated canvas the 8-bit arm stores.
        let mut orig = svtav1_types::try_vec![0u8; uw * th]?;
        for r in 0..th {
            for c in 0..uw {
                orig[r * uw + c] = (src.y[r * uw + c] >> shift) as u8;
            }
        }
        self.superres_stats_luma = Some((orig, uw, th));
        Ok((y8, u8p, v8p))
    }

    /// Superres chunk B.3 — the combinations whose SIGNALLED stream would not
    /// match what the encoder actually produced. Rejecting beats emitting a
    /// stream that says "upscale me" over content the encoder handled with the
    /// wrong geometry.
    /// Issue #5: the coded-lossless (QP 0 / `base_q_idx` 0) envelope this port
    /// has byte-verified against the C oracle, as a refusal predicate for
    /// everything outside it. Each arm names the missing piece.
    fn lossless_config_error(&self, is_key: bool) -> Option<&'static str> {
        if self.hdr.is_fork() {
            // The fork's chroma-q deltas (Cb +12) put every segment's chroma
            // qindex above 0 while base_q_idx stays 0, so the frame is NOT
            // CodedLossless yet quantizes luma at qindex 0 — and C's variance
            // boost is internally inconsistent there (SUSPECTED-C-BUGS.md #1).
            return Some(
                "QP 0 (coded-lossless) in HDR-fork mode is not implemented: the fork's chroma-q \
                 deltas leave the frame outside CodedLossless (spec 5.9.2) with base_q_idx 0 — \
                 use mainline mode or QP >= 1",
            );
        }
        if !is_key || self.gop.intra_period != 1 {
            // The encode can contain INTER frames. Coded-lossless inter is
            // decoder-verified only on the 8-bit 4:2:0 funnel arm: real
            // inter-coded blocks emit WHT residuals aomdec reconstructs
            // byte-exactly (qp0-inter sweep 2026-09-21: sb64+sb128). At
            // 10 bits the inter-frame lossless path predicts intra
            // per-BLOCK while the decoder predicts per-TXB (measured:
            // encoder recon == source but aomdec diverges), and the
            // monochrome / 4:4:4 arms have no inter WHT residual path at
            // all — their qp0 streams are legal but silently all-intra
            // (measured 2026-09-21: zero inter block decisions). Refuse
            // rather than emit them.
            if self.bit_depth != 8 || !self.chroma_420 {
                return Some(
                    "QP 0 (coded-lossless) inter frames are not implemented outside \
                     8-bit 4:2:0: the 10-bit path mis-predicts intra per-block vs \
                     the decoder's per-TXB rule and the monochrome / 4:4:4 arms \
                     have no inter WHT residual path [C: accepts] — use QP >= 1",
                );
            }
        }
        if self.superres_denom.is_some() {
            return Some(
                "QP 0 (coded-lossless) with superres is not implemented (the frame is not \
                 AllLossless at the upscaled size) — use QP >= 1 [C: accepts]",
            );
        }
        None
    }

    fn sb_size_config_error(&self) -> Option<&'static str> {
        match self.sb_size_override {
            None | Some(64 | 128) => None,
            Some(_) => Some("superblock size override must be 64 or 128"),
        }
    }

    /// BITRATE-TARGETED RATE CONTROL — C's own envelope plus this port's
    /// coverage inside it.
    ///
    /// C admits `SVT_AV1_RC_MODE_CBR` only under LOW_DELAY and
    /// `SVT_AV1_RC_MODE_VBR` only outside it (enc_settings.c:157/:177). The
    /// port wires the one-pass CBR driver (`cbr_frame_qindex` →
    /// `port_rc_vbr_cbr_*` → `cbr_postencode`) on exactly that envelope.
    /// VBR stays refused: its ported arm consumes first-pass statistics and
    /// `firstpass.c` is not ported, so it would emit CRF-shaped output under
    /// a bitrate label — the plausible-but-wrong class the issue #22 refusal
    /// was written against.
    fn rate_control_config_error(&self) -> Option<&'static str> {
        match self.rc_config.mode {
            crate::rate_control::RcMode::Cqp | crate::rate_control::RcMode::Crf => None,
            crate::rate_control::RcMode::Vbr => Some(
                "VBR rate control is not implemented: the ported two-pass arm \
                 (`svt_aom_process_rc_stat`/`av1_set_target_rate`, \
                 pass2_strategy.c) needs first-pass statistics, and \
                 firstpass.c is not ported — wiring VBR without them would \
                 emit CRF-shaped output under a bitrate label. Use \
                 RcMode::Cbr (LOW_DELAY) or RcMode::Cqp/Crf [C: accepts VBR \
                 only outside LOW_DELAY, enc_settings.c:177]",
            ),
            crate::rate_control::RcMode::Cbr => {
                // C's own envelope (enc_settings.c:157): CBR exists only
                // under LOW_DELAY. Inside it the ported one-pass driver
                // runs; outside it refuse rather than emit a stream C
                // would never produce.
                if self.pred_structure == crate::port_picstruct::PredStructure::LowDelay {
                    None
                } else {
                    Some(
                        "CBR rate control requires pred_structure == LOW_DELAY \
                         — C's own constraint (enc_settings.c:157, \"CBR Rate \
                         control is currently not supported for \
                         RANDOM_ACCESS/ALL_INTRA, use VBR mode\") [C: refuses]",
                    )
                }
            }
        }
    }

    /// The LD+CBR frame qindex and per-SB plan — C's `RC_INPUT` task body for
    /// `rc_cfg.mode == AOM_CBR` (rc_process.c:833-877):
    /// `rc_init_frame_stats` → `svt_av1_rc_process_rate_allocation` →
    /// `svt_av1_rc_calc_qindex_rate_control`, then `generate_sb_qindex`'s
    /// CBR arm (`svt_av1_rc_init_sb_qindex`, rc_aq.c:879-885).
    ///
    /// Returns `(base_qindex, frame_rc, sb_plan)`. `frame_rc` is the PPCS
    /// half the post-encode update consumes — carry it to
    /// [`Self::cbr_postencode`]. `sb_plan` is `Some` only when cyclic
    /// refresh armed; `None` is C's flat arm (every SB takes the frame
    /// `base_q_idx`, `delta_q_present` stays 0).
    fn cbr_frame_qindex(
        &mut self,
        pic: Option<&crate::port_picstruct::PicParams>,
        is_key: bool,
        display_order: u64,
        frame_hier: u8,
        sc_class1: bool,
        frame_me: Option<&crate::inter_me_arm::FrameMe>,
    ) -> crate::EncodeResult<(
        u8,
        crate::port_rc_vbr_cbr_state::FrameRc,
        Option<crate::sb_qindex::SbQindexPlan>,
    )> {
        // `svt_aom_set_rc_param` + `set_param_based_on_input`, built once from
        // the encode config. The config gate admits CBR only on LOW_DELAY,
        // which is also the envelope `RcVbrCbr::new_cbr` documents.
        if self.rc_vbr_cbr.is_none() {
            self.rc_vbr_cbr = Some(crate::port_rc_driver::RcVbrCbr::new_cbr(
                &self.rc_config,
                self.width,
                self.height,
                self.bit_depth,
                // C `scs->static_config.intra_period_length` is the CLI
                // `--intra-period` MINUS one (the app prints length+1); -1
                // is C's "no periodic intra" sentinel.
                if self.gop.intra_period == 0 {
                    -1
                } else {
                    self.gop.intra_period as i32 - 1
                },
                frame_hier,
                self.sb_size as u16,
                (self.width as usize).div_ceil(self.sb_size) as u16,
                (self.height as usize).div_ceil(self.sb_size) as u16,
            ));
        }
        let rcs = self.rc_vbr_cbr.as_mut().unwrap();
        let b64_count = (self.width.div_ceil(64) * self.height.div_ceil(64)) as u16;
        let mut frame = crate::port_rc_driver::frame_rc(
            pic,
            is_key,
            display_order,
            self.width,
            self.height,
            self.upscaled_width,
            b64_count,
            frame_hier,
            sc_class1,
        );
        // `pcs->me_64x64_distortion[]` — the RC path's copy of the open-loop
        // per-b64 distortions. Empty on a key frame, where ME never runs;
        // `rc_init_frame_stats` then leaves both averages alone, exactly as
        // C does on an I_SLICE.
        let me_64x64_dist: alloc::vec::Vec<u32> = frame_me.map_or_else(alloc::vec::Vec::new, |m| {
            m.per_b64.iter().map(|b| b.me_64x64_distortion).collect()
        });
        let slice_type = if is_key {
            crate::port_rc_process::SliceType::I
        } else {
            crate::port_rc_process::SliceType::B
        };
        let ref_l0_stats = pic
            .filter(|p| p.ref_list0_count_try > 0)
            .and_then(|p| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize));
        let ref_l1_stats = pic
            .filter(|p| p.ref_list1_count_try > 0)
            .and_then(|p| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize));
        // C `rc_init_frame_stats` (rc_process.c:836 → :604). Of its outputs
        // only `avg_base_me_dist` is consumed on this path — the three
        // ref-object percentages are the port's separately-computed
        // `ref_obj_stats`/`md_ref_intra_percentage` inputs, and
        // `rate_average_periodin_frames` is a two-pass field the ported CBR
        // arm never reads.
        let stats = crate::port_rc_process::rc_init_frame_stats(
            &crate::port_rc_process::FrameStatsInput {
                slice_type,
                ref_list1_count_try: pic.map_or(0, |p| p.ref_list1_count_try),
                ref_l0: ref_l0_stats.as_ref(),
                ref_l1: ref_l1_stats.as_ref(),
                passes: 1,
                max_bit_rate: u64::from(self.rc_config.max_bitrate) * 1000,
                total_stats_count: 0,
                me_64x64_distortion: &me_64x64_dist,
            },
        );
        if let Some(avg) = stats.avg_base_me_dist {
            rcs.rc.prev_avg_base_me_dist = rcs.rc.cur_avg_base_me_dist;
            rcs.rc.cur_avg_base_me_dist = avg;
        }
        // C `svt_av1_rc_process_rate_allocation` (rc_process.c:857 →
        // rc_vbr_cbr.c). The two `FnOnce` are VBR's `svt_aom_process_rc_stat`
        // / `av1_set_target_rate` pair — unreachable under AOM_CBR and wired
        // to nothing because first-pass stats are not ported.
        let bw = rcs.frame_bandwidth();
        let (best_q, worst_q) = rcs.best_worst_allowed_q();
        let mode = rcs.cfg.mode;
        let hier = i32::from(rcs.scs.hierarchical_levels);
        crate::port_rc_vbr_cbr_update::process_rate_allocation(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            &mut frame,
            // `ppcs->tpl_ctrls.enable` is 0 under LOW_DELAY (`get_tpl`), the
            // only structure CBR is admitted on — `TplCtrlsRc::default()`
            // says exactly that.
            &crate::port_rc_vbr_cbr_qpick::TplCtrlsRc::default(),
            &mut rcs.resize_pending,
            crate::port_rc_vbr_cbr_update::RtResizeMode::None,
            false,
            |rc| crate::port_rc_driver::apply_rc_init(rc, mode, best_q, worst_q, hier, bw),
            |_rc, _f| {},
            |_rc, _f| {},
        );
        // `pcs->ref_pic_ptr_array` — C fills list 0 from
        // `ref_dpb_index[LAST..=GOLD]` (rps indices 0..4) and list 1 from
        // `[BWD..=ALT]` (4..7); under LD the second list is empty.
        let mut l0: alloc::vec::Vec<crate::port_rc_vbr_cbr_qpick::RefPicRc> =
            alloc::vec::Vec::new();
        let mut l1: alloc::vec::Vec<crate::port_rc_vbr_cbr_qpick::RefPicRc> =
            alloc::vec::Vec::new();
        if let Some(p) = pic {
            for i in 0..usize::from(p.ref_list0_count_try) {
                if let Some(rf) = self.dpb.get(p.rps.ref_dpb_index[i] as usize) {
                    l0.push(crate::port_rc_driver::ref_pic_rc(rf));
                }
            }
            for i in 0..usize::from(p.ref_list1_count_try) {
                if let Some(rf) = self.dpb.get(p.rps.ref_dpb_index[4 + i] as usize) {
                    l1.push(crate::port_rc_driver::ref_pic_rc(rf));
                }
            }
        }
        let refs = crate::port_rc_vbr_cbr_qpick::RefLists {
            l0: &l0,
            l1: &l1,
            l0_count_try: pic.map_or(0, |p| usize::from(p.ref_list0_count_try)),
            l1_count_try: pic.map_or(0, |p| usize::from(p.ref_list1_count_try)),
        };
        // `pcs->norm_me_dist` (initial_rc_process.c:718-726) — the per-b64
        // 8x8 distortion mean the cyclic-refresh motion gates threshold
        // against; 0 on an I slice, exactly as C leaves it.
        let norm_me_dist = if is_key {
            0u64
        } else {
            frame_me.map_or(0, |m| {
                let n = m.per_b64.len() as u64;
                if n == 0 {
                    0
                } else {
                    m.per_b64
                        .iter()
                        .fold(0u64, |a, b| a + u64::from(b.me_8x8_distortion))
                        / n
                }
            })
        };
        let new_qindex = crate::port_rc_vbr_cbr_qpick::rc_calc_qindex_rate_control(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            &rcs.twopass,
            &mut frame,
            &refs,
            slice_type,
            // `MeDistortion` is read only by the VBR reference-qindex floor —
            // unreachable under AOM_CBR.
            None,
            &mut rcs.cr_sb_end,
            &mut rcs.cr,
            |cr| {
                if let Some(me) = frame_me {
                    crate::sb_qindex::cyclic_refresh_setup(
                        cr,
                        u32::from(b64_count),
                        norm_me_dist,
                        &me.per_b64,
                    );
                } else {
                    // C runs the setup over zeroed ME arrays on a key frame;
                    // its only possible outcome is what
                    // `cyclic_refresh_init` already left — refresh stays off
                    // for an I slice.
                    cr.apply_cyclic_refresh = false;
                }
            },
        );
        let Some(new_qindex) = new_qindex else {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "CBR rate control could not resolve this frame's qindex bounds: \
                 `rc_pick_q_and_bounds_no_stats_cbr` reads the LAST reference \
                 unconditionally and its DPB slot is empty"
            )));
        };
        #[cfg(feature = "std")]
        if std::env::var_os("SVTAV1_RCDBG").is_some() {
            std::eprintln!(
                "RCDBG pic={} ft={} this_tgt={} base_tgt={} qidx={} cr={} \
                 buf={} bot={} avg_bw={} lastq=[{},{}] active_worst={} \
                 roll_t={} roll_a={} since_key={} to_key={} band={}",
                frame.picture_number,
                frame.update_type as i32,
                frame.this_frame_target,
                frame.base_frame_target,
                new_qindex,
                rcs.cr.apply_cyclic_refresh as i32,
                rcs.rc.buffer_level,
                rcs.rc.bits_off_target,
                rcs.rc.avg_frame_bandwidth,
                rcs.rc.last_q[0],
                rcs.rc.last_q[1],
                rcs.rc.active_worst_quality,
                rcs.rc.rolling_target_bits,
                rcs.rc.rolling_actual_bits,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.cr_sb_end,
            );
        }
        // `generate_sb_qindex`'s CBR arm — `svt_av1_rc_init_sb_qindex`
        // (rc_aq.c:879-885): cyclic refresh assigns per-SB qindexes and flips
        // `delta_q_present`; otherwise every SB takes the frame base. C skips
        // `svt_av1_normalize_sb_delta_q` at `delta_q_res == 1`
        // (rc_process.c:741-744), which `SbQindexPlan.delta_q_res = 1`
        // carries into the signal side.
        let sb_plan = if rcs.cr.apply_cyclic_refresh && let Some(me) = frame_me {
            let mut sb_qindex = alloc::vec![0u8; usize::from(b64_count)];
            crate::sb_qindex::cyclic_sb_qp_assignment(
                &rcs.cr,
                new_qindex,
                norm_me_dist,
                &me.per_b64,
                &mut sb_qindex,
            );
            Some(crate::sb_qindex::SbQindexPlan {
                base_qindex: new_qindex as u8,
                sb_qindex,
                delta_q_res: 1,
            })
        } else {
            None
        };
        Ok((new_qindex as u8, frame, sb_plan))
    }

    /// C `rc_process_packetization_feedback`'s one-pass CBR arm
    /// (rc_process.c:758-792): `svt_av1_rc_postencode_update` consumes the
    /// coded bit count and the normalized zero-MV area, then
    /// `svt_aom_update_rc_counts` advances the frame counters.
    fn cbr_postencode(
        &mut self,
        frame: &mut crate::port_rc_vbr_cbr_state::FrameRc,
        total_num_bits: u64,
        avg_cnt_zeromv: u64,
    ) {
        let Some(rcs) = self.rc_vbr_cbr.as_mut() else {
            return;
        };
        crate::port_rc_vbr_cbr_update::postencode_update(
            &mut rcs.rc,
            &rcs.cfg,
            &rcs.scs,
            frame,
            &rcs.cr,
            total_num_bits,
            avg_cnt_zeromv,
        );
        let (frames_since_key, frames_to_key, frames_since_cdf_update) =
            crate::port_rc_process::update_rc_counts(
                frame.showable_frame,
                // `ppcs->frm_hdr.disable_cdf_update` — always signalled 0 by
                // this encoder (obu.rs:1600/:2515/:3450).
                false,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.rc.frames_since_cdf_update,
            );
        rcs.rc.frames_since_key = frames_since_key;
        rcs.rc.frames_to_key = frames_to_key;
        rcs.rc.frames_since_cdf_update = frames_since_cdf_update;
        #[cfg(feature = "std")]
        if std::env::var_os("SVTAV1_RCDBG").is_some() {
            std::eprintln!(
                "RCDBG-POST pic={} bits={} proj={} zeromv={} buf={} bot={} \
                 since_key={} to_key={} low_motion={}",
                frame.picture_number,
                total_num_bits,
                frame.projected_frame_size,
                avg_cnt_zeromv,
                rcs.rc.buffer_level,
                rcs.rc.bits_off_target,
                rcs.rc.frames_since_key,
                rcs.rc.frames_to_key,
                rcs.rc.avg_frame_low_motion,
            );
        }
    }

    /// Hierarchical levels above C's own ceiling are refused.
    /// `enc_settings.c:275` rejects `hierarchical_levels > 5`
    /// ("Hierarchical Levels supported: [0-5]"), and the pred-struct tables
    /// this port drives end at level 5 (`PRED_STRUCT_TEMPORAL_LAYER`) — the
    /// table index itself would panic, so this guard is unconditional, key
    /// frames included.
    fn gop_config_error(&self, _is_key: bool) -> Option<&'static str> {
        if self.gop.hierarchical_levels > 5 {
            return Some(
                "hierarchical_levels > 5 is outside C's own supported range                  (enc_settings.c:275, \"Hierarchical Levels supported: [0-5]\") and                  the pred-struct tables end at level 5. Use hierarchical_levels                  <= 5 [C: refuses]",
            );
        }
        None
    }

    fn superres_config_error(&self) -> Option<&'static str> {
        let denom = self.superres_denom?;
        if !(9..=16).contains(&denom) {
            return Some("SuperresDenom must be 9..=16");
        }
        // Loop restoration runs on the UPSCALED frame in C
        // (`svt_av1_superres_upscale_frame` sits between CDEF and LR,
        // cdef_process.c:152); this port still searches/applies LR at the
        // coded width, so the two would disagree. Restoration is off for
        // allintra presets >= 7 (`seq_tools_for_preset`: wn = 0 there), which
        // is where the superres gate lives until the upscaled-LR wiring lands.
        let tools = crate::speed_config::seq_tools_for_preset(
            self.speed_config.preset,
            self.gop.intra_period == 1,
            self.width as usize * self.height as usize,
        );
        if tools.enable_restoration
            && !crate::frame_geom::small_frame_disables_restoration(
                &crate::frame_geom::FrameDims::new(
                    self.upscaled_width as usize,
                    self.true_height as usize,
                ),
            )
        {
            return Some(
                "superres is not wired for frames that run loop restoration (allintra preset <= 6, \
                 except small frames where restoration is disabled) — C runs LR on the \
                 UPSCALED frame; use preset >= 7",
            );
        }
        if !matches!(self.bit_depth, 8 | 10) {
            return Some(
                "superres supports 8/10-bit only — C v4.2.0 rejects every other depth at \
                 encoder init (svt_av1_verify_settings, Globals/enc_settings.c:460), so no \
                 oracle exists outside that envelope [C: rejects]",
            );
        }
        // Mono pipelines never reach `superres_downscale_420` — the mono core
        // feeds the full-width luma straight to `encode_frame_impl`, which
        // would code a left-cropped canvas and signal the normative upscale
        // over it: plausible-but-wrong output, the class this encoder refuses
        // everywhere else. (Measured 2026-09-21: `SVTAV1_MONO=1
        // SVTAV1_SUPERRES=16` emitted a 269-byte stream whose decode showed
        // the left half stretched, not the downscaled source.) C has no mono
        // mode at all, so this is the port's own extension surface.
        let mono = !self.chroma_420
            && !matches!(
                self.chroma_format,
                Some(svtav1_types::chroma::ChromaFormat::Yuv444)
                    | Some(svtav1_types::chroma::ChromaFormat::Yuv422)
            );
        if mono {
            return Some(
                "superres is not wired for monochrome — the mono entry has no downscale arm \
                 and would code a left-cropped plane under an upscale header (C has no mono \
                 mode at any depth; this is a port-extension gap, not a C envelope)",
            );
        }
        // Inter frames under superres are refused: the measured surface is
        // stills/KEY frames (`tools/superres_gate.sh` and its bd10 arm drive
        // single-frame encodes, and the raw-sequence harness never applies a
        // denominator). C accepts superres on inter frames
        // (`--superres-denom` vs `--superres-kf-denom`), so this is capability
        // debt, not a C envelope: the per-reference geometry under a changing
        // coded width (`frame_size_with_refs`, render-vs-frame-size) is
        // decoder-ungated here. A key frame in a GOP pipeline is still fine —
        // the predicate is the frame type, not `intra_period`.
        if !self.gop.is_key_frame(self.frame_count) {
            return Some(
                "superres on an INTER frame is not implemented: stills are the measured \
                 surface and the reference-geometry signaling under a changing coded width \
                 is decoder-ungated (C accepts it — this is a port capability gap, not a \
                 C envelope) [C: accepts]",
            );
        }
        None
    }

    /// Native 10-bit (u16) 4:2:0 entry point — task #6 chunk 1
    /// (`rust/docs/hbd-input-port-map.md`).
    ///
    /// `y`/`u`/`v` carry REAL 10-bit samples (0..=1023) in the same TRUE-dim
    /// layout [`Self::try_encode_frame_420`] takes for `u8` (`y` at
    /// `y_stride`, chroma at `(true_w+1)/2`). Requires the pipeline to be
    /// built with `with_bit_depth(10)` and `with_chroma_420(true)`.
    ///
    /// # What chunk 1 threads — and what it does not
    ///
    /// The low 2 bits reach the **mode decision and the coded levels**: the
    /// bd10 MD funnel (MDS0 SATD, and the MDS1/MDS3 full-RD inputs for luma
    /// AND chroma) plus the bd10 level re-encode post-pass all read the real
    /// u16 samples. The **post-filter searches** (deblock level, CDEF
    /// strength, Wiener taps) and the recon SSE still run on the
    /// MSB-truncated u8 planes — that is chunk 2. The emitted bitstream is a
    /// valid 10-bit stream either way; the band-limit is on filter DECISIONS,
    /// not on the coded residual.
    ///
    /// # Errors
    ///
    /// [`EncodeError::UnsupportedConfig`] if the pipeline is not bd10/4:2:0,
    /// if the frame is not a key frame, if a sample exceeds 10 bits, or if
    /// this preset/dimension combination has no consumer for the native
    /// source (see [`Self::hbd_source_consumed`]) — rejecting beats silently
    /// encoding the MSB-truncated content. Also propagates cancellation and
    /// fallible-allocation failures like the u8 entry points.
    pub fn try_encode_frame_420_hbd(
        &mut self,
        y: &[u16],
        u: &[u16],
        v: &[u16],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if !self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_420_hbd requires the pipeline to be built with \
                 with_chroma_420(true)",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_420_hbd") {
            return Err(e);
        }
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_420_hbd requires with_bit_depth(10) (8-bit sources use \
                 encode_frame_420; 12-bit is outside C's shipping envelope)",
            )));
        }
        if !self.hbd_source_consumed(true) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit input needs a bd10 consumer: either preset \
                 >= 9 or a full-RD-capable preset <= 8 (non-screen content) — see \
                 docs/hbd-input-port-map.md chunk 2",
            )));
        }
        // Inter frames ship on the 10-bit entry point too, on the same
        // evidence as the 8-bit one above plus `tools/bd10_video_gate.sh`
        // (24/24 encode AND decode). The blanket refusal and
        // `SVTAV1_INTER_EXPERIMENTAL` are both gone; the configuration
        // envelope is still enforced by the `*_config_error` guards.
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        // The caller hands FULL-width (upscaled) planes under superres,
        // coded-width planes otherwise — the same contract as
        // [`Self::try_encode_frame_420`]. Chroma is 4:2:0 ceiling either way.
        let (iw, icw) = if self.superres_denom.is_some() {
            let uw = self.upscaled_width as usize;
            (uw, uw.div_ceil(2))
        } else {
            (tw, tw.div_ceil(2))
        };
        let ich = th.div_ceil(2);
        if y.len() < (th - 1) * y_stride + iw || u.len() < icw * ich || v.len() < icw * ich {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: iw as u32,
                height: self.true_height,
                reason: "hbd planes must cover the input dims (y at y_stride, u/v at w/2; \
                         FULL width under superres)",
            }));
        }
        let max = 1u16 << self.bit_depth;
        if y.iter().chain(u.iter()).chain(v.iter()).any(|&s| s >= max) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "hbd source carries a sample above the configured bit depth",
            )));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        if self.superres_denom.is_some() {
            // Stage the native FULL-width source for the downscale inside
            // `encode_frame_420_prepared`. That arm produces BOTH outputs in
            // C's order — filtered u16 -> `hbd_source` (coded dims, aligned),
            // `filtered_u16 >> 2` -> the u8 canvas — so building either here
            // would duplicate the step or force the wrong
            // truncate-then-filter order. The u8 arguments are unused on
            // this arm (film-grain denoise + bd10 + superres is refused in
            // `validate_film_grain` before the planes are read).
            let mut sy = svtav1_types::try_vec![0u16; iw * th]?;
            for r in 0..th {
                sy[r * iw..(r + 1) * iw].copy_from_slice(&y[r * y_stride..r * y_stride + iw]);
            }
            let mut su = svtav1_types::try_vec![0u16; icw * ich]?;
            su.copy_from_slice(&u[..icw * ich]);
            let mut sv = svtav1_types::try_vec![0u16; icw * ich]?;
            sv.copy_from_slice(&v[..icw * ich]);
            self.hbd_superres_src = Some(HbdSuperresSrc {
                y: sy,
                u: su,
                v: sv,
            });
            // `encode_frame_420_core` clears `hbd_superres_src` and the
            // produced `hbd_source` unconditionally — error paths included —
            // so nothing staged can leak into the next frame.
            return self.encode_frame_420_core(&[], &[], &[], 0);
        }
        // Stash the ALIGNED-padded u16 planes for `encode_frame_impl` to take,
        // and drive the existing core with the MSB-truncated u8 planes — the
        // sites chunk 1 does not thread (post-filter searches, recon SSE) keep
        // reading those. Truncation and edge replication are both per-sample
        // gathers, so truncate-then-pad == pad-then-truncate: the u8 planes the
        // core builds are exactly the u8 planes an 8-bit caller would pass.
        let shift = u32::from(self.bit_depth - 8);
        let (aw, ah) = (self.width as usize, self.height as usize);
        let hbd = HbdSource {
            y: pad_plane_replicate_u16(y, y_stride, tw, th, aw, ah)?,
            u: pad_plane_replicate_u16(u, icw, icw, ich, aw / 2, ah / 2)?,
            v: pad_plane_replicate_u16(v, icw, icw, ich, aw / 2, ah / 2)?,
        };
        let mut y8 = svtav1_types::try_vec![0u8; tw * th]?;
        for r in 0..th {
            for c in 0..tw {
                y8[r * tw + c] = (y[r * y_stride + c] >> shift) as u8;
            }
        }
        let mut u8p = svtav1_types::try_vec![0u8; icw * ich]?;
        let mut v8p = svtav1_types::try_vec![0u8; icw * ich]?;
        for i in 0..icw * ich {
            u8p[i] = (u[i] >> shift) as u8;
            v8p[i] = (v[i] >> shift) as u8;
        }
        self.hbd_source = Some(hbd);
        let out = self.encode_frame_420_core(&y8, &u8p, &v8p, tw);
        // Never leave a stale source behind for the next frame (the happy
        // path already took it; this covers the early-error paths).
        self.hbd_source = None;
        out
    }

    /// 4:4:4 chroma entry point — Zen extension (C refuses non-420 at
    /// `verify_settings`, `enc_settings.c:470`; no byte oracle).
    ///
    /// `y`/`u`/`v` are all FULL-resolution (each `true_w × true_h`, `y`
    /// at `y_stride`). Requires `with_chroma_format(Some(Yuv444))`.
    ///
    /// Support envelope (decoder-verified — C refuses non-4:2:0 at
    /// `verify_settings`, enc_settings.c:470, so no byte oracle exists):
    /// 8-bit, `sb_size 64`, no superres — key AND inter frames (the
    /// non-funnel inter arm predicts chroma by motion compensation and
    /// residual-codes against it; no `uv_mode` is signalled). Everything
    /// else (10-bit, sb128, superres, IntraBC) takes the honest refusal
    /// at `encode_frame_impl`'s envelope gate. Chroma loop filters are
    /// signalled off (lf levels 0 / CDEF uv 0 / LR RESTORE_NONE) until the
    /// filter kernels are ported — decoder-consistent by construction.
    /// Quality oracle: `tools/rd_ext_sweep.sh` (SSIMULACRA2 + per-plane
    /// PSNR against aomenc `--i444 --profile=1`).
    ///
    /// # Errors
    /// [`EncodeError::UnsupportedConfig`] outside the envelope;
    /// [`EncodeError::InvalidDimensions`] on short planes.
    pub fn try_encode_frame_444(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if self.chroma_format != Some(svtav1_types::chroma::ChromaFormat::Yuv444) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_444 requires the pipeline to be built with \
                 with_chroma_format(Some(ChromaFormat::Yuv444))",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_444") {
            return Err(e);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let n = tw * th;
        if y.len() < (th - 1) * y_stride + tw || u.len() < n || v.len() < n {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "4:4:4 planes must each cover the true dims (u/v full-resolution)",
            }));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// 4:2:2 chroma entry point — Zen extension, same staging contract
    /// as [`Self::try_encode_frame_444`]. `u`/`v` are `true_w/2 × true_h`
    /// (horizontally subsampled only).
    ///
    /// # Errors
    /// As [`Self::try_encode_frame_444`].
    pub fn try_encode_frame_422(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        y_stride: usize,
    ) -> EncodeResult<Vec<u8>> {
        if self.chroma_format != Some(svtav1_types::chroma::ChromaFormat::Yuv422) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_422 requires the pipeline to be built with \
                 with_chroma_format(Some(ChromaFormat::Yuv422))",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_422") {
            return Err(e);
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, n) = (tw.div_ceil(2), tw.div_ceil(2) * th);
        if y.len() < (th - 1) * y_stride + tw || u.len() < n || v.len() < n {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "4:2:2 planes: y at y_stride over true dims, u/v at (true_w+1)/2 x true_h",
            }));
        }
        let _ = tcw;
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        self.encode_frame_420_core(y, u, v, y_stride)
    }

    /// Native 10-bit (u16) monochrome entry point — the mono twin of
    /// [`Self::try_encode_frame_420_hbd`] (task #6 chunk 1).
    ///
    /// Monochrome builds no MD funnel, so the only consumer of the real u16
    /// samples is the bd10 level re-encode post-pass (every preset): the coded
    /// LEVELS are computed at true 10 bits, while the mode decision itself
    /// still runs on the MSB-truncated plane. Rejects any config where even
    /// that consumer is absent.
    ///
    /// # Errors
    ///
    /// As [`Self::try_encode_frame_420_hbd`], plus the monochrome dimension
    /// rules of [`Self::try_encode_frame`].
    pub fn try_encode_frame_hbd(&mut self, y: &[u16], y_stride: usize) -> EncodeResult<Vec<u8>> {
        if self.chroma_420 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_hbd is the monochrome entry point; use \
                 try_encode_frame_420_hbd on a 4:2:0 pipeline",
            )));
        }
        if let Some(e) = self.ra_entry_error("try_encode_frame_hbd") {
            return Err(e);
        }
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_hbd requires with_bit_depth(10)",
            )));
        }
        if !self.hbd_source_consumed(false) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit monochrome input requires a native level producer (defensive check; all current presets are supported)",
            )));
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        if y.len() < (th - 1) * y_stride + tw {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "hbd luma plane must cover the true dims at y_stride",
            }));
        }
        let max = 1u16 << self.bit_depth;
        if y.iter().any(|&s| s >= max) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "hbd source carries a sample above the configured bit depth",
            )));
        }
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        let shift = u32::from(self.bit_depth - 8);
        let mut y8 = svtav1_types::try_vec![0u8; tw * th]?;
        for r in 0..th {
            for c in 0..tw {
                y8[r * tw + c] = (y[r * y_stride + c] >> shift) as u8;
            }
        }
        // Match the u8 mono core's TRUE -> ALIGNED padding on both source
        // representations; the native level producer consumes the u16 plane.
        self.hbd_source = Some(HbdSource {
            y: pad_plane_replicate_u16(
                y,
                y_stride,
                tw,
                th,
                self.width as usize,
                self.height as usize,
            )?,
            u: alloc::vec::Vec::new(),
            v: alloc::vec::Vec::new(),
        });
        let out = self.encode_frame_mono_core(&y8, tw);
        self.hbd_source = None;
        out
    }

    /// Shared frame encode body. `chroma = Some((u, v))` selects the 4:2:0
    /// path; `None` is the unchanged monochrome path.
    fn encode_frame_impl(
        &mut self,
        y_plane: &[u8],
        y_stride: usize,
        chroma: Option<(&[u8], &[u8])>,
        decided: Option<FrameDecision>,
    ) -> crate::EncodeResult<Vec<u8>> {
        // An AUTO `hierarchical_levels` that survived to encode time (no
        // `with_pred_structure` call — the low-delay default) resolves here,
        // the last point before `self.gop` is first read. A no-op after
        // `with_pred_structure` resolved it, or when the level was explicit.
        self.resolve_hierarchical_levels_auto();
        // `chroma_420` is the configured-format check (refuses 4:4:4 where
        // `chroma.is_some()` would pass); `chroma.is_some()` is the per-frame
        // check (refuses the mono entry on a chroma-configured pipeline).
        self.enhancements
            .validate(
                self.speed_config.preset,
                self.gop.intra_period == 1,
                self.chroma_420 && chroma.is_some(),
                self.bit_depth,
            )
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        let zen_intra_edge_filter = self
            .enhancements
            .contains(crate::enhancements::ZenEnhancement::AomIntraEdgeFilter);
        self.reference
            .validate_hdr_config(&self.hdr)
            .map_err(|why| whereat::at!(EncodeError::UnsupportedConfig(why)))?;
        if self.reference == crate::reference::SvtReference::Mainline420 && chroma.is_none() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "pristine mainline SVT supports 4:2:0 only; monochrome is a Rust extension",
            )));
        }
        #[cfg(feature = "__expert")]
        if self.chroma_q_override.is_some() && chroma.is_none() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "chroma_q_override is set but this frame is monochrome: there is no U/V \
                 quantizer to override; clear the override for mono frames [C: no mono]",
            )));
        }
        // TUNE overrides (C `svt_av1_enc_set_parameter`, enc_handle.c:4889).
        // `--tune 3` (IQ, "still image only") and `--tune 4` (MS-SSIM) are not
        // single RD knobs: C rewrites qm on/min/max (luma AND chroma),
        // sharpness, variance boost on/strength/curve, and — for IQ —
        // `max_tx_size` and `screen_content_mode`. Applying them here, once,
        // against the CLI-domain qp is what makes `hdr.tune = TUNE_IQ` in this
        // port mean the same thing as `--tune 3` in C. A no-op for every other
        // tune, so the default path is byte-unchanged.
        //
        // These four (tune, QM, variance boost, sharpness) are MAINLINE v4.2.0
        // features, not fork additions — they used to be gated behind
        // `is_fork()` here, which silently ignored them in mainline mode.
        //
        // The application point MUST precede the superblock derivation below:
        // C applies these in `copy_api_from_app` (enc_handle.c:4890-4918) and
        // derives `super_block_size` later in `set_param_based_on_input`
        // (:4995 -> :4077), so the tune-IQ `enable_variance_boost = 1` is
        // already visible when the sb-size rule runs — and that rule forces
        // SB64 whenever variance boost is on. `new()` cannot reproduce that
        // ordering because `self.hdr` is caller-mutated after construction.
        self.hdr.apply_tune_overrides(self.rc_config.qp);
        // C derives `super_block_size` in `svt_av1_enc_init_handle` — AFTER
        // the full static_config exists — and `enable_variance_boost` forces
        // 64 (enc_handle.c:4075-4078). This port mutates `self.hdr` AFTER
        // `new()` ran the derivation with `variance_boost: false`, so an
        // encode-time VB request (tune IQ, `with_variance_boost`) could
        // leave a sb128 derivation in place that C never emits — and whose
        // delta-q emission then desynced the tile. Re-run the derivation at
        // the encode choke point with the true VB input.
        // An explicit `with_sb_size(128)` override still wins (that config
        // is a port extension C cannot express; the emission path below now
        // codes it decodably, just not byte-identically to anything C makes).
        {
            let sb_inputs = crate::sb128_geom::SbSizeInputs {
                qp: self.rc_config.qp,
                allintra: self.gop.intra_period == 1,
                variance_boost: self.hdr.enable_variance_boost,
                ..Default::default()
            };
            let derived = crate::sb128_geom::derive_super_block_size(
                self.width as usize,
                self.height as usize,
                self.speed_config.preset as i8,
                &sb_inputs,
            );
            let (sb_size, fell_back) =
                Self::resolve_sb_size(derived, self.sb_size_override, self.speed_config.preset);
            self.sb_size = sb_size;
            self.sb128_fallback = fell_back;
        }
        if let Some(why) = self.sb_size_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        if let Some(reason) =
            crate::entropy::obu::TileLimits::for_frame(self.width, self.height, self.sb_size as u32)
                .untileable_reason(self.tile_cols_log2)
        {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason,
            }));
        }
        self.validate_film_grain()?;
        // `decided` carries a random-access frame: its display-order POC and
        // the `PicParams` the mini-GOP window decision already produced
        // (`encode_ra_window`). `None` is the sequential contract — display
        // order is the coded-frame count and picture decision runs inline.
        let display_order = decided
            .as_ref()
            .map_or(self.frame_count, |d| d.display_order);
        // Superres chunk B.3: refuse any combination whose SIGNALLED geometry
        // would not match what this encoder actually produced (see
        // `superres_config_error`). Checked at the single choke point every
        // entry point funnels through, so no path can slip past it.
        if let Some(why) = self.superres_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Same choke point, same rule, for the bit-depth axis: a 10-bit request
        // that no bd10 stage can serve would emit 8-bit-quantized levels under a
        // 10-bit sequence header. See `bit_depth_config_error`. This covers the
        // u8 entry points too — the `hbd_source_consumed` screen on the `*_hbd`
        // entries only fires when the caller passed a native u16 source, so
        // `with_bit_depth(10)` + `encode_frame_420` used to slip past every guard.
        if let Some(why) = self.bit_depth_config_error(chroma.is_some()) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Issue #9 items 3-5: the three config knobs C validates at init
        // (`svt_av1_verify_settings`) and this port therefore refuses at the
        // same choke point rather than encoding something C would never emit.
        if let Some(why) = self.knob_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // Issue #22: VBR/CBR were ACCEPTED here and silently encoded at qp 30.
        if let Some(why) = self.gop_config_error(self.gop.is_key_frame(display_order)) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        if let Some(why) = self.rate_control_config_error() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        // GLOBAL MOTION is NOT refused here: whether C searches depends on this
        // frame's ME residual, which does not exist yet at this choke point.
        // See `gm_search_config_error`, called immediately after `frame_me` below.
        // MULTI-FRAME IS NOT ENCODABLE — refuse it rather than emit a corrupt
        // stream. The 4:2:0 path already asserted this below; the MONOCHROME
        // path did not, so `EncodePipeline::new(w, h, preset, rc, hier,
        // /*intra_period=*/64).encode_frame(..)` produced inter frames with:
        //   - the sequence header's `order_hint_bits_minus_1` written BEFORE
        //     `seq_choose_screen_content_tools` / `seq_choose_integer_mv`,
        //     where spec 5.5.1 and C (entropy_coding.c:2812-2838) put it after;
        //   - `initial_display_delay_present_flag` hardcoded 0, omitting the
        //     five bits C always writes (enc_handle.c:4981-4993,
        //     entropy_coding.c:3731,:3749-3755);
        //   - an illegal 8-bit `refresh_frame_flags` on a SHOWN key frame
        //     (C writes it only `if (!show_frame)`, entropy_coding.c:3404-3407);
        //   - no `disable_frame_end_update_cdf`, which shifts `tile_info()` and
        //     every following field by one bit (entropy_coding.c:3553-3559);
        //   - MV coding against FRESH per-block CDFs, and an inter frame header
        //     with an admittedly incomplete `tile_info()` (obu.rs).
        //
        // MEASURED 2026-08-03 on a 5-frame 64x64 gradient encode through the
        // public API: aomdec reports "Corrupt frame detected: Failed to decode
        // tile data" at frame 1, and dav1d reports "Overrun in OBU bit buffer"
        // then "No data decoded". So this is not a byte-parity gap — it is the
        // zero-tolerance corruption class, on the public entry point.
        //
        // The fix is deliberately a REFUSAL, not a header patch: the inter path
        // beyond the header is unported too (no chroma in the DPB, homegrown ME,
        // fresh-CDF MV coding), so correcting the four fields would buy a
        // better-formed header on top of a stream that still cannot decode.
        //
        // The predicate is the FRAME TYPE, not `intra_period` — see the check
        // beside `is_key` below. A caller may legitimately construct a pipeline
        // with a GOP structure and encode only its key frame; that stream is a
        // valid still and is what several tests do. Only an actual INTER frame
        // is unencodable.
        // Task #6 chunk 1: TAKE the native 10-bit source (set only by the
        // `*_hbd` entry points) so it can never leak into a following u8
        // frame. `None` on every u8 path -> every bd10 stage keeps widening
        // `u8 << 2` exactly as before.
        let hbd_source = self.hbd_source.take();
        // Superres chunk B.3: the pre-scaling picture-statistics source (see
        // `superres_stats_luma`). `None` on every non-superres path.
        let stats_src = self.superres_stats_luma.take();
        // Superres chunk B.4: C's per-b64 variance array (`pcs->variance`) is
        // built by picture analysis on the FULL-RESOLUTION picture, and
        // `scale_pcs_params` (resize.c:1434) re-inits the b64/SB geometry for
        // the coded size WITHOUT recomputing it — so every PD0 / dc-only gate
        // downstream reads full-res variances through the SMALLER coded-grid
        // indices. Reproduce that exactly: build the array over the full-res
        // grid in raster order here, and index it with the coded grid's linear
        // SB index at the search. `None` on every non-superres path -> the
        // variance is recomputed from the coded source, unchanged.
        let stale_vars: Option<alloc::vec::Vec<crate::pd0::SbVariance>> =
            stats_src.as_ref().map(|(orig, ow, oh)| {
                let (ext_w, ext_h) = (ow.div_ceil(64) * 64, oh.div_ceil(64) * 64);
                let mut padded = alloc::vec![0u8; ext_w * ext_h];
                for r in 0..*oh {
                    padded[r * ext_w..r * ext_w + ow].copy_from_slice(&orig[r * ow..(r + 1) * ow]);
                }
                crate::frame_geom::pad_input_plane(
                    &mut padded,
                    &crate::frame_geom::FrameDims::new(*ow, *oh),
                    64,
                );
                let (cols, rows) = (ext_w / 64, ext_h / 64);
                let mut v = alloc::vec::Vec::with_capacity(cols * rows);
                for by in 0..rows {
                    for bx in 0..cols {
                        v.push(crate::pd0::compute_b64_variance(
                            &padded,
                            ext_w,
                            bx * 64,
                            by * 64,
                        ));
                    }
                }
                v
            });
        // Did a bd10 consumer actually READ it? The entry points pre-screen
        // the config (`hbd_source_consumed`), but the post-pass additionally
        // requires every SB's tree to be bd10-supported at RUNTIME — so this
        // flag is what turns "the low bits were silently dropped" into an
        // explicit error instead of a quietly-8-bit stream.
        let mut hbd_used = false;
        // The funnel side reports through an atomic because the tile loop
        // runs the funnel inside `std::thread::scope` (byte-inert: the flag
        // is write-only and never read by the search).
        let hbd_used_flag = core::sync::atomic::AtomicBool::new(false);
        // Feature 1: snapshot the cooperative-cancellation token once (a cheap
        // Arc clone; `Send + Sync`) so the per-SB loops here, the entropy-walk
        // closure, and `encode_tile_rows` all check the same token. The default
        // `Unstoppable` token's `may_stop()` is `false`, so every guarded check
        // below is a byte-inert false-branch.
        let stop = self.stop.clone();

        // Step 1: Determine frame type from GOP structure
        let is_key = self.gop.is_key_frame(display_order);
        // `__expert` chroma override: the SH (written on key frames only)
        // fixes separate_uv_delta_q until the next key frame, so an override
        // change that flips U/V separation mid-sequence is refused rather
        // than signalled under a mismatched SH.
        #[cfg(feature = "__expert")]
        {
            let separate = self.separate_uv_delta_q();
            if is_key {
                self.sh_separate_uv_delta_q = Some(separate);
            } else if self.sh_separate_uv_delta_q.is_some_and(|s| s != separate) {
                return Err(whereat::at!(EncodeError::UnsupportedConfig(
                    "chroma_q_override changed U/V separation after the key frame; the \
                     sequence header fixes separate_uv_delta_q until the next key frame",
                )));
            }
        }
        // C picks a DIFFERENT derivation function per arm of `scs->allintra`
        // (enc_handle.c:4406 — `intra_period_length == 0 || avif ||
        // pred_structure == ALL_INTRA`); the port's proxy for it is the same
        // conjunction. On the video arm `is_islice` marks the video-mode
        // I-slice — `is_key` covers every case this reaches today.
        //
        // Bound at frame level because `eff_enc_mode(sc_arm, ..)` is C's
        // `pcs->enc_mode` — the POST-clamp `static_config.enc_mode`
        // (enc_handle.c:4415-4436: allintra above M9 -> M9, non-RTC video
        // above M11 -> M11, runtime_enc_mode = static_config at
        // resource_coordination_process.c:139/:1267). Every C `enc_mode`
        // ladder must see the clamped value: at CLI p13 a video frame's
        // pic_lpd1_lvl, ME search area, prehme level and
        // stats_based_sb_lambda_modulation all derive differently from
        // raw 13 than from C's 11.
        let sc_arm = if self.gop.intra_period == 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: is_key }
        };
        // INTER FRAMES SHIP ON THE 4:2:0 PATH AND ARE REFUSED ON THE
        // MONOCHROME ONE, and that asymmetry is the whole of what this guard
        // now says.
        //
        // The blanket "inter frames are not implemented for the public API"
        // refusal, and the `SVTAV1_INTER_EXPERIMENTAL` variable that lifted
        // it, are both gone as of 2026-09-11. What replaced them is a
        // measurement the refusal itself asked for: `video_selfcheck_gate.sh`
        // decodes the port's own 8-frame stream with `aomdec` and requires the
        // port's final reconstruction to be byte-identical to the decoder's on
        // EVERY frame — 18 of 18 cells over six public-domain derf clips at
        // qp {20,40,55}. The stream the refusal called "silently wrong" is
        // measurably not.
        //
        // WHAT THIS DOES NOT CLAIM. Byte-identity to C on the 4:2:0 inter path
        // is NOT universal, and the README's video rows say so. MEASURED
        // 2026-09-11 on the campaign's 96-cell frontier grid
        // ({uniform,gradient,diag,screen} x {16,64,72,128} x {q20,q40,q55} x
        // {p6,p8}) at frames=4 low-delay P: 95 cells identical on frame 0, 95
        // on frame 1, 60 on frame 2 and 58 on frame 3. The chain frames'
        // divergence is concentrated in two known frontiers — 72x72 is a
        // PARTIAL superblock (17 of its 24 cells differ at frame 2) and
        // `gradient` content (19 of 24) — and `uniform` is 24 of 24 identical
        // on every frame. Re-measure those numbers in the SAME change whenever
        // `tools/inter_byte_matrix.sh` moves.
        //
        // THE MONOCHROME ARM SHIPS INTER — measured 2026-09-21. The corrupt
        // stream this refusal once cited was produced before the inter
        // correctness landings (write-time `overlappable_neighbors` /
        // `num_proj_ref` derivation, recon-only eob preservation, OBMC on
        // `SimpleTranslation`), all of which are format-agnostic: with them
        // in place a mono inter frame is byte-identical between this
        // encoder's reconstruction, `aomdec` AND `dav1d` across presets
        // {0,6,8,13}, sizes 64x64..256x128, sb64+sb128, qp0-lossless and
        // 10-bit, with real nonzero MVs — `tools/mono_inter_gate.sh` is the
        // standing witness. `chroma.is_none()` simply means the frame has
        // no chroma planes to predict, code or filter: the seq header
        // already signals mono_chrome, and the decoder derives a single
        // plane throughout.
        //
        // Keyed on the FRAME TYPE rather than `intra_period` so that
        // constructing a pipeline with a GOP structure and encoding only its
        // key frame keeps working: that stream is a valid still.
        // The 10-bit inter floor lifted 2026-09-18: the `hbd_md = 2` MDS3
        // bump mirror (product_coding_loop.c:9649 — `LeafBd10::mds3_hbd`)
        // closed the recon drift the refusal here used to cite, and
        // `bd10_video_selfcheck_gate.sh` pins the replacement measurement:
        // 396/396 cells x 8 frames reconstruct identically to `aomdec`
        // (presets -1..13 at 256x256, -1..5 at 128x128, qp {20,40,55}, six
        // derf clips).
        // Step 1b: C's PICTURE DECISION — the reference structure.
        //
        // `picture_decision_per_picture` fills `rps.ref_dpb_index[]` (the
        // header's `ref_frame_idx[]`), `rps.refresh_frame_mask` (its
        // `refresh_frame_flags`), the skip-mode allowance and the shadow DPB.
        // It has to run on the KEY frame too — `set_key_frame_rps` seeds the
        // layer-0 toggle ring that every later frame's refresh mask advances
        // from, so skipping it would put frame 1 in the wrong DPB slot.
        //
        // Byte-inert for a still encode: it is skipped entirely when no GOP is
        // configured, and even when it runs it only writes `self.pd_ctx` and
        // the local `PicParams` — nothing downstream of a KEY frame reads
        // either.
        let decided_is_some = decided.is_some();
        let (pic_decision, mut tpl_in) = if self.gop.intra_period != 1 {
            match decided {
                // Random access: the window decision already ran RPS + DPB
                // state for this picture — re-running it per frame would
                // advance the toggles twice. `tpl` is the TPL stage's
                // per-picture bundle (`None` whenever `scs->tpl` was off).
                Some(d) => (Some(d.pic), d.tpl),
                None => (
                    Some(self.run_picture_decision(display_order, is_key)?),
                    None,
                ),
            }
        } else {
            (None, decided.and_then(|d| d.tpl))
        };
        // `pcs->temporal_layer_index = pred_position_ptr->temporal_layer_index`
        // (`pd_process.c:5559`) — the pred-struct ENTRY's layer, read back from
        // the picture decision, not a position-derived paraphrase (the flat
        // GOP's only entry is layer 0, which is also the `None` answer).
        let temporal_layer = pic_decision.as_ref().map_or(0, |p| p.temporal_layer_index);
        // C reads `ppcs->hierarchical_levels` — the MINI-GOP's level for this
        // picture, which a cut-short RA window subdivides below the
        // configured `gop.hierarchical_levels` (`get_pred_struct_for_frame`).
        // Equals the configured level on every low-delay frame, so this is
        // byte-inert outside random access.
        //
        // Two stamps land on the same field: `pd_process.c:968` gives an IDR
        // the CONFIGURED level at decision time, then a DELAYED intra's TF
        // setup re-stamps it with the following mini-GOP's level
        // (`pd_process.c:3936-3943`, "Update the key frame pred structure" —
        // `filter_delayed_intra` is the port's copy). The picture's own field
        // therefore carries the right answer in both cases; reading
        // `gop.hierarchical_levels` for a key instead skips the delayed-intra
        // overwrite — measured 2026-09-25: a hier-5 RA cell coded the key's
        // `percents[0][0]` (75) arm as qindex 70 where C takes the re-stamped
        // level's `percents[1][0]` (76) arm and writes 67.
        let frame_hier = pic_decision
            .as_ref()
            .map_or(self.gop.hierarchical_levels, |p| p.hierarchical_levels);

        // C `av1_lambda_assign_md`'s TWO update-type selectors
        // (`pd0::inter_full_lambda_8bit`): the rdmult BASE reads
        // `ppcs->update_type`, which the picture decision already resolved,
        // and the frame-type FACTOR reads `update_lambda`'s own
        // `gf_update_type`. On a flat low-delay P GOP they DISAGREE (Lf vs
        // Arf), so both are carried and neither is derived from the other.
        // Hoisted here because the funnel's `c_quant` and PD0's own
        // `full_sb_lambda_md` must be the same number.
        let md_lambda_base_update_type = pic_decision.as_ref().map(|pic| match pic.update_type {
            crate::port_picstruct::FrameUpdateType::Kf => {
                crate::port_rc_process::FrameUpdateType::KfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Lf => {
                crate::port_rc_process::FrameUpdateType::LfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Gf => {
                crate::port_rc_process::FrameUpdateType::GfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Arf => {
                crate::port_rc_process::FrameUpdateType::ArfUpdate
            }
            crate::port_picstruct::FrameUpdateType::Overlay => {
                crate::port_rc_process::FrameUpdateType::OverlayUpdate
            }
            crate::port_picstruct::FrameUpdateType::IntnlOverlay => {
                crate::port_rc_process::FrameUpdateType::IntnlOverlayUpdate
            }
            crate::port_picstruct::FrameUpdateType::IntnlArf => {
                crate::port_rc_process::FrameUpdateType::IntnlArfUpdate
            }
        });
        let md_lambda_factor_update_type =
            crate::port_rc_process::lambda_gf_update_type(is_key, frame_hier, temporal_layer);
        let md_alt_lambda_factors = self.hdr.is_fork() && self.hdr.alt_lambda_factors;

        // C `pcs->ref_intra_percentage` (`get_ref_intra_percentage`,
        // rc_process.c:66, set in `rc_init_frame_stats` at :613): the mean
        // `intra_coded_area` of the two nearest references, I-slice refs
        // skipped. Hoisted to frame level — it feeds `av1_lambda_assign_md`'s
        // LAMBDA_MOD_INTRA arm (md_process.c:738) and PD0's
        // `use_ref_info`/skip_intra controls alike. 0 on a key frame (no
        // decision), matching C's `ref_cnt == 0` answer.
        let md_ref_intra_percentage = pic_decision.as_ref().map_or(0, |p| {
            crate::port_rc_process::get_ref_intra_percentage(
                crate::port_rc_process::SliceType::B,
                u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX),
                (p.ref_list0_count_try > 0)
                    .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize))
                    .flatten()
                    .as_ref(),
                (p.ref_list1_count_try > 0)
                    .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize))
                    .flatten()
                    .as_ref(),
            )
        });
        // C `av1_lambda_assign_md`'s LAMBDA_MOD_INTRA arm
        // (md_process.c:730-745): `!rtc && stats_based_sb_lambda_modulation`
        // — `static_config.enc_mode <= M11` (enc_handle.c:4375) on the
        // POST-clamp mode, so live at EVERY video preset (p12/p13 encode as
        // M11) — `&& temporal_layer_index > 0 &&
        // ref_intra_percentage < (alt_lambda_factors ? 65 : 50)` scales the
        // SB's full and fast lambdas by 138>>7. MEASURED: `diag 64x64 q40 p8
        // hier=3` poc=4 (tl=1) prices its trellis at rdmult 206 584, i.e.
        // `full_lambda_md` 51 646, not the 47 905 the unmodulated chain
        // gives — C dropped a raster-3 coefficient the port kept (`eob` 6 vs
        // 7). 128 is the arm-not-taken identity; on a flat LD GOP every frame
        // is tl 0, so this is byte-inert there.
        let lambda_mod_intra: i64 = if temporal_layer > 0
            && crate::port_rc_process::stats_based_sb_lambda_modulation(
                crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                false,
            )
            && md_ref_intra_percentage < if md_alt_lambda_factors { 65 } else { 50 }
        {
            138
        } else {
            128
        };

        // Step 2: Create PCS
        let mut pcs = if is_key {
            PictureControlSet::new_key_frame(self.width, self.height, display_order)
        } else {
            PictureControlSet::new_inter_frame(
                self.width,
                self.height,
                display_order,
                // `pcs->decode_order` — the RA window's permutation; display
                // order under low delay (the same value `pic_decision` carries).
                pic_decision
                    .as_ref()
                    .map_or(display_order, |p| p.decode_order),
                temporal_layer,
            )
        };
        // `pcs->frm_hdr.show_frame` — a hidden RA picture is coded but not
        // shown; `inter_signal` reads it through `pic_decision`, and
        // anything else wanting it must see the same value.
        if let Some(pic) = pic_decision.as_ref() {
            pcs.show_frame = pic.show_frame;
        }

        // C `frm_hdr->refresh_frame_flags = ppcs->rps.refresh_frame_mask`
        // — the SAME value the frame header signals (`inter_hdr_arm.rs`:
        // `refresh_frame_flags: pic.rps.refresh_frame_mask`).
        //
        // `PictureControlSet::new_inter_frame` hard-codes 0, and that constant
        // reached `self.dpb.refresh(..)` — so the HEADER announced C's real
        // mask while THIS ENCODER'S DPB never received an inter frame at all.
        // Invisible at two frames (nothing reads the DPB after frame 1) and
        // fatal at three: MEASURED 2026-09-03 on
        // `gradient 64x64 q32 p8 frames=3`, at poc 2 the port's
        // `rps.ref_dpb_index[0]` is slot 1 and every slot still held the KEY
        // frame, so LAST resolved to poc 0 where C's is poc 1. Every
        // frame-2 reading taken before this fix is void.
        //
        // A key frame keeps 0xFF from `new_key_frame`: it has no picture
        // decision on the allintra path, and C's own key-frame mask is 0xFF.
        if let Some(pic) = pic_decision.as_ref() {
            pcs.refresh_frame_flags = pic.rps.refresh_frame_mask;
        }

        // Step 3: Rate control — assign QP
        //
        // `temporal_layer` is passed as 0 on purpose: in C's CQP/CRF path
        // `picture_qp` stays the CLI qp for EVERY temporal layer — the
        // per-layer scaling lives inside `cqp_qindex_calc` on the qindex
        // (rc_crf_cqp.c:393), not on the CLI-domain value. Letting
        // `assign_picture_qp`'s homegrown `TEMPORAL_LAYER_QP_DELTA` apply
        // here too would double-count the boost on a hierarchical GOP. Byte-
        // inert on the flat GOP: its only temporal layer is 0 and
        // `delta[0] == 0`. (VBR/CBR are refused upstream.)
        pcs.qp = assign_picture_qp(&self.rc_config, &self.rc_state, 0);

        // Step 3b: temporal filtering is NOT done here. C's
        // `produce_temporally_filtered_pic` is ported as
        // `crate::port_tf_driver` and runs upstream on the random-access
        // buffer (`ra_tf_bufs` / `TfPicBufs`), so this function receives the
        // already-filtered source; low-delay has no TF in C
        // (`derive_tf_params`, enc_handle.c:3339-3343). The homegrown
        // recon-blending `temporal_filter::temporal_filter` that used to sit
        // here behind a literal `false` gate was removed on 2026-09-25.
        let w = self.width as usize;
        let h = self.height as usize;
        let n = w * h;
        // The frame's chroma format drives EVERY chroma-geometry derivation
        // below — `ChromaFormat::{chroma_width,chroma_height}` are the C
        // `>> subsampling` rules generalized, never a local `/2`.
        let fmt = self
            .chroma_format
            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
        let (ss_x, ss_y) = (fmt.subsampling_x() as usize, fmt.subsampling_y() as usize);
        // Aligned-extent chroma dims — the internal planes' stride/height.
        let (acw, ach) = (fmt.chroma_width(w), fmt.chroma_height(h));
        // Non-4:2:0 formats ship only where the generalized path is proven.
        // 4:4:4 is decoder-verified on the still arm at sb64 AND on the
        // non-funnel inter arm (motion-compensated chroma prediction +
        // residual, no `uv_mode` — the funnel stays 4:2:0-only, so inter
        // decisions come from `RefFrameCtx`'s luma-ME + explicit chroma
        // MC). sb128's interleaved multi-cell chroma walk, 10-bit 444
        // planes, IntraBC-on-444 and chroma superres remain unported — a
        // 444 stream outside the envelope takes the honest refusal.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444
            && (self.sb_size != 64 || self.bit_depth != 8 || self.superres_denom.is_some())
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "ChromaFormat::Yuv444 is decoder-verified only for 8-bit \
                 frames at sb_size 64 without superres: the sb128 multi-cell \
                 chroma walk, 10-bit 444 planes, IntraBC chroma prediction \
                 and chroma superres are not yet ported (C itself refuses \
                 444 at verify_settings, enc_settings.c:470 — no byte oracle)",
            )));
        }
        // 4:4:4 staged bring-up: the chroma loop-filter KERNELS are still
        // 4:2:0-shaped, so at 444 every chroma filter arm is skipped and the
        // header signals chroma off (lf levels 0 / CDEF uv strengths 0 /
        // LR planes RESTORE_NONE) — decoder-consistent by construction.
        // `filter_chroma` is the apply-side "chroma arm runs" flag; at
        // 4:2:0 it is exactly `chroma.is_some()`.
        let filter_chroma = chroma.is_some() && fmt == svtav1_types::chroma::ChromaFormat::Yuv420;
        let encode_input = gather_rows(y_plane, y_stride, w, h)?;

        // Task #95 chunk 2 — partial-SB variance source. `compute_b64_variance`
        // walks a full 64x64 grid per b64, so on a partial SB (aligned dims not
        // a multiple of 64) it reads PAST the aligned extent into C's replicated
        // border (`pad_input_picture` + `svt_aom_generate_padding` net content =
        // the TRUE edge pixel, docs/arbitrary-dims-port-map.md). Build a source
        // buffer padded out to the SB extent and read the PD0 partition /
        // variance source from it. For a 64-aligned frame the extent equals the
        // aligned extent, so no padding is needed and `encode_input` is used
        // directly at stride `w` — fully byte-neutral for every full-SB cell.
        let grain_denoised = self.film_grain.denoise_apply
            && self.prepared_grain.as_ref().is_some_and(|p| p.apply_grain);
        let dims95 = if grain_denoised {
            crate::frame_geom::FrameDims::new(w, h)
        } else {
            crate::frame_geom::FrameDims::new(self.true_width as usize, self.true_height as usize)
        };
        let sb95 = 64usize;
        let ext_w = w.div_ceil(sb95) * sb95;
        let ext_h = h.div_ceil(sb95) * sb95;
        let sb_input_owned: Option<alloc::vec::Vec<u8>> = if ext_w == w && ext_h == h {
            None
        } else {
            let mut buf = svtav1_types::try_vec![0u8; ext_w * ext_h]?;
            for r in 0..h {
                buf[r * ext_w..r * ext_w + w].copy_from_slice(&encode_input[r * w..r * w + w]);
            }
            crate::frame_geom::pad_input_plane(&mut buf, &dims95, sb95);
            Some(buf)
        };
        let sb_input: &[u8] = sb_input_owned.as_deref().unwrap_or(&encode_input);
        let in_stride = if sb_input_owned.is_some() { ext_w } else { w };
        // Task #95 chunk 2: chroma SOURCE padded to the SB-extent height (aligned
        // chroma width/stride, extra rows edge-replicated) so a straddling
        // boundary block's chroma TX read stays in bounds — mirrors the luma
        // sb_input. Full-SB frames need no extension (byte-neutral).
        let sb_chroma_owned: Option<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> = chroma
            .map(
                |(u, v)| -> crate::EncodeResult<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> {
                    let (ext_cw, ext_ch_h) = (fmt.chroma_width(ext_w), fmt.chroma_height(ext_h));
                    Ok(if ext_ch_h == ach && ext_cw == acw {
                        // Full-SB (or 64-aligned) frame: exact aligned chroma,
                        // byte-identical to the pre-#95 source.
                        (u.to_vec(), v.to_vec())
                    } else {
                        // Partial SB: `acw`-strided rows, edge-replicating the last
                        // real chroma row. Enough rows to cover BOTH a height-
                        // straddle read (reaches `ext_ch_h`) AND a right-straddle
                        // read that wraps down into later stride rows. For gradient
                        // (uniform chroma) every padded byte equals the true edge,
                        // so the reads match C's SB-extent pad; other content is
                        // decodable (the boundary chroma differs from C's crop).
                        let n_rows = ext_ch_h + ext_cw.div_ceil(acw) + 2;
                        let cap = n_rows * acw;
                        let mut up = svtav1_types::try_vec![0u8; cap]?;
                        let mut vp = svtav1_types::try_vec![0u8; cap]?;
                        for r in 0..n_rows {
                            let sr = r.min(ach - 1);
                            up[r * acw..(r + 1) * acw]
                                .copy_from_slice(&u[sr * acw..(sr + 1) * acw]);
                            vp[r * acw..(r + 1) * acw]
                                .copy_from_slice(&v[sr * acw..(sr + 1) * acw]);
                        }
                        (up, vp)
                    })
                },
            )
            .transpose()?;
        // Task #94 partial-SB: the SB-extent twins of the two buffers above,
        // for the NATIVE 10-bit source. `HbdSource` is padded TRUE->ALIGNED
        // only (`try_encode_frame_420_hbd`), so on a partial-SB frame a
        // straddling block's `blk_y_src10` gather would run past the plane
        // (bottom-right) or wrap into the next row (right edge) — exactly the
        // two failure modes `sb_input_owned` / `sb_chroma_owned` exist to kill
        // on the u8 side. Build the same shapes here so `FunnelSrc10` can carry
        // `in_stride` / `w/2` and index identically to the u8 gather.
        //
        // Luma: `ext_w * ext_h` at stride `ext_w`. The aligned plane already
        // replicates the TRUE edge into `[true_w, w)` / `[true_h, h)`, so
        // replicating its ALIGNED edge outward reproduces `pad_input_plane`'s
        // TRUE-edge fill byte-for-byte. Chroma: aligned stride `w/2` with the
        // same extra-row count as `sb_chroma_owned` (a right-straddle chroma
        // read wraps down into later stride rows, so rows alone are not
        // enough). 64-aligned frames take neither branch — byte-neutral.
        let hbd_sb_owned: Option<(
            alloc::vec::Vec<u16>,
            alloc::vec::Vec<u16>,
            alloc::vec::Vec<u16>,
        )> = match hbd_source.as_ref() {
            Some(hbd) if sb_input_owned.is_some() => {
                let y = pad_plane_replicate_u16(&hbd.y, w, w, h, ext_w, ext_h)?;
                let (u, v) = if hbd.u.is_empty() {
                    (alloc::vec::Vec::new(), alloc::vec::Vec::new())
                } else {
                    let (ext_cw, ext_ch_h) = (fmt.chroma_width(ext_w), fmt.chroma_height(ext_h));
                    let n_rows = ext_ch_h + ext_cw.div_ceil(acw) + 2;
                    let mut up = svtav1_types::try_vec![0u16; n_rows * acw]?;
                    let mut vp = svtav1_types::try_vec![0u16; n_rows * acw]?;
                    for r in 0..n_rows {
                        let sr = r.min(ach - 1);
                        up[r * acw..(r + 1) * acw]
                            .copy_from_slice(&hbd.u[sr * acw..(sr + 1) * acw]);
                        vp[r * acw..(r + 1) * acw]
                            .copy_from_slice(&hbd.v[sr * acw..(sr + 1) * acw]);
                    }
                    (up, vp)
                };
                Some((y, u, v))
            }
            _ => None,
        };

        // Screen-content derivation (allintra): scm 3 auto-detect at
        // preset <= 7 (enc_handle.c:4514-4527), off at M8+; palette level
        // + FH allow_screen_content_tools from sc_class5
        // (enc_mode_config.c:2374-2393). Runs on the SOURCE luma (C
        // pcs->enhanced_pic) before everything downstream: the flag gates
        // the per-block no-palette flag coding in the tile pack, the MD
        // rates (via the tile driver's own identical derivation), and the
        // FH bits.
        // Superres: screen-content detection runs on the CODED (downscaled)
        // picture, NOT the full-resolution one — unlike `pic_avg_variance`.
        // C's picture-decision process resizes at pd_process.c:4344 and only
        // then detects (`svt_aom_is_screen_content_antialiasing_aware`,
        // pd_process.c:4787), so the detector sees the scaled picture.
        // MEASURED: running it on the full-res source instead diverges from C
        // at preset 7 — the only allintra preset where scm-3 auto-detection is
        // live (enc_handle.c:4514-4527; M8+ has it off) — on the superres cell
        // gradient 64x64 q32 d10.
        // C derives the allintra screen-content mode from the preset
        // (scm 3 at <= M7, off at M8+, enc_handle.c:4641-4651) UNLESS the
        // config forces it — tune IQ sets `screen_content_mode = 3`
        // regardless of preset (enc_handle.c:4914). Model the force by
        // running the detector at a preset it is live for.
        let sc_preset = match self.hdr.screen_content_mode {
            Some(3) => self.speed_config.preset.min(7),
            // ZenEnhancement::AomScreenTools: run detection at every preset
            // (same min-7 clamp as an SCM-3 force) — C's own gate stops
            // running the detector entirely above M7 on the allintra arm.
            _ if self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::AomScreenTools) =>
            {
                self.speed_config.preset.min(7)
            }
            _ => self.speed_config.preset,
        };
        // `sc_arm` is bound at frame level above; it matters HERE because C
        // picks a DIFFERENT derivation function per arm of `scs->allintra`.
        // On the video arm the intra-BC ladder is
        // `sig_deriv_multi_processes_default`'s (:2033-2052) instead of the
        // allintra one (:2346-2369) — which is what makes a video-mode
        // screen-content key frame set `frm_hdr->allow_intrabc` at M6, where
        // the still arm leaves it clear.
        let mut sc_derivation = match self.hdr.screen_content_mode {
            Some(mode @ 0..=1) => {
                // C forces every classification, not just the header flag.
                // Keep the real preset for palette/IntraBC tool selection.
                let forced = mode == 1;
                crate::sc_detect::derive_sc_classes(
                    sc_arm,
                    self.speed_config.preset,
                    crate::sc_detect::ScClasses {
                        sc_class0: forced,
                        sc_class1: forced,
                        sc_class2: forced,
                        sc_class3: forced,
                        sc_class4: forced,
                        sc_class5: forced,
                    },
                )
            }
            _ => crate::sc_detect::derive_sc(sc_arm, sc_preset, &encode_input, w, w, h),
        };
        // ZenEnhancement::AomScreenTools: libaom keeps palette and IntraBC
        // ENABLED whenever the detector says screen — the allintra ladders
        // switch them off by preset (palette at M8+, IntraBC at M5+). When
        // the detector fires on the allintra arm AND the ladder left BOTH
        // tools dead, the arm substitutes the VIDEO ladder's levels for the
        // same preset index (clamped at its last nonzero row: palette holds
        // M10's, IntraBC M9's). The fill is deliberately all-or-nothing:
        // the measured wins are the M8+ zone where C signals no screen
        // tools at all, while the M5-M7 partial zone (palette live, only
        // IntraBC dead) regressed on the imazen-26 frontier cells
        // (+1.95% mean BD at p6 — IBC-5 costs on content palette-7 already
        // handles). A level the allintra arm already set is never lowered.
        if sc_arm == crate::sc_detect::ScArm::Allintra
            && self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::AomScreenTools)
            && sc_derivation.classes.sc_class5
            && sc_derivation.palette_level == 0
            && sc_derivation.intrabc_level == 0
        {
            use crate::port_enc_mode_config::multi_processes::{
                intrabc_level_default, palette_level_default,
            };
            let preset = self.speed_config.preset;
            sc_derivation.palette_level = palette_level_default(preset.min(10), true, true);
            sc_derivation.intrabc_level = intrabc_level_default(preset.min(9), true, true, true);
            sc_derivation.allow_intrabc =
                crate::intrabc::IbcCtrls::for_level(sc_derivation.intrabc_level).enabled;
            sc_derivation.allow_screen_content_tools =
                sc_derivation.palette_level != 0 || sc_derivation.allow_intrabc;
        }
        // 4:4:4 staged bring-up: IntraBC chroma prediction is not ported
        // (the IBC search is funnel-only and the funnel is 4:2:0-gated), so
        // the frame must NOT advertise the tool — `allow_intrabc=1` would
        // also suppress the LF/CDEF/LR FH param blocks (spec 5.9.11/19/20)
        // that this stream still needs to signal.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            sc_derivation.allow_intrabc = false;
        }
        // Hoisted out of the walk: `&self` is borrowed across the pack loop, and
        // this is a frame-constant. One value for the header writer and the
        // walk (see `EntropyCtx::tx_mode_select`).
        let frame_tx_mode_select = self.frame_tx_mode_select(temporal_layer);

        // Step 3c: the CLI-domain qp `svt_av1_rc_calc_qindex_crf_cqp`
        // reads (`scs_qp`, rc_crf_cqp.c:471-473 — no `startup_qp_offset`
        // in this port's envelope). C's `aq_mode` does not shift this
        // value: `aq-mode` is TPL's enable, and TPL's output lands in the
        // QINDEX domain below (`crf_qindex_calc` / `sb_qp_derivation_tpl_
        // la`), never back on `static_config.qp`.
        //
        // There USED to be a homegrown VAQ + "TPL" qp shift here behind
        // `aq_mode != 0` — a port of nothing, kept only while `aq_mode !=
        // 0` was refused outright. The real TPL path replaces it.
        #[allow(unused_mut)]
        let mut tpl_adjusted_qp = pcs.qp;

        // --- The INTER branch of MODE DECISION (docs/INTER-ENCODE-PLAN.md
        // §1s items 1b/2/3/6). `None` on a key frame, which is what keeps the
        // whole still envelope byte-identical by construction.
        //
        // The open-loop search runs against the PREVIOUS FRAME'S SOURCE, not
        // the DPB recon — SVT's ME is open loop (`me_process.c:185-203` reads
        // the PA reference, `reference_object.c:242-250`). The recon side is
        // `ref_padded_luma`, which the motion COMPENSATION indexes.
        //
        // The PA picture is built only in VIDEO mode: on a still/AVIF encode
        // nothing can ever reference this frame, and the pyramid is a padded
        // copy plus two decimations of the whole luma plane — real work to
        // spend on a buffer with no reader.
        let mut pa_cur = (self.gop.intra_period != 1).then(|| match self.pa_scratch.take() {
            // Recycle the frame-before-last's pyramid. `refill_from_source`
            // rewrites every byte and every descriptor field, so this is
            // byte-identical to the fresh allocation it replaces.
            Some(mut recycled) => {
                recycled.refill_from_source(&encode_input, w, w, h, display_order);
                recycled
            }
            None => alloc::boxed::Box::new(crate::inter_me_arm::PaPicture::from_source(
                &encode_input,
                w,
                w,
                h,
                display_order,
            )),
        });
        // C `pa_ref_obj->avg_luma = input_pcs->avg_luma`
        // (`pic_analysis_process.c:2003`) — stamped onto the reference object
        // a later picture's `get_similar_ref_brightness` reads through
        // `pa_slots`. `INVALID_LUMA` whenever `calc_hist` was off, because
        // the picture decision's `avg_luma` was already gated there.
        if let Some(pa) = pa_cur.as_mut() {
            pa.avg_luma = pic_decision.as_ref().map_or(
                crate::port_picstruct::INVALID_LUMA,
                |p| p.avg_luma,
            );
        }
        // The TPL stage already ran this picture's open-loop ME against the
        // same reference pyramids with the same `FrameMeParams` — C's
        // `pa_me_data->me_results`, shared between `tpl_mc_flow` and the
        // picture's own encode. Reuse it verbatim; a member the stage
        // skipped (I slice, or a reference pyramid it could not resolve)
        // falls through to the sequential computation, whose `None`/`Some`
        // answer is then identical.
        let frame_me = match tpl_in.as_mut().and_then(|f| f.frame_me.take()) {
            me @ Some(_) => me,
            None => match (is_key, pa_cur.as_deref(), pic_decision.as_ref()) {
                (false, Some(cur), Some(pic)) => {
                    // C `pcs->ref_pa_pic_ptr_array[list][ref]` — EVERY reference
                    // the picture decision offered, resolved to its DPB slot's PA
                    // pyramid (`assign_and_release_pa_refs`, pd_process.c:4990).
                    // This used to feed only `pa_ref` — the PREVIOUS frame — to
                    // both lists, which is the [1,1] shape frame 1 happens to
                    // produce but leaves a frame with `ref_list0_count_try > 1`
                    // (frame 2 onward on a flat GOP) searching LAST2's MV slot
                    // against LAST's picture.
                    let mut refs = crate::inter_me::context::MeRefs::default();
                    for rt in 1i8..=7 {
                        let (li, ri) = (
                            crate::inter_mvp::get_list_idx(rt),
                            crate::inter_mvp::get_ref_frame_idx(rt),
                        );
                        let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                        if let Some(pa) = self.pa_slots.get(slot).and_then(|s| s.as_deref()) {
                            refs.arr[li][ri] = Some(pa.ds_ref());
                        }
                    }
                    #[cfg(feature = "std")]
                    if crate::dbgenv::medbg() {
                        let mut s = alloc::string::String::new();
                        for (i, sl) in self.pa_slots.iter().enumerate() {
                            s.push_str(&alloc::format!(
                                "{i}:{} ",
                                sl.as_deref().map_or(-1, |p| p.picture_number as i64)
                            ));
                        }
                        std::eprintln!(
                            "PASLOTS poc={} dpb={:?} slots=[{s}]",
                            pic.picture_number,
                            pic.rps.ref_dpb_index
                        );
                    }
                    // `me_process.c:212-213` — the counts the picture decision
                    // offered. `MeRefs::get` panics on a hole a search reaches,
                    // so a missing pyramid means no ME rather than a wrong one —
                    // the same shape the `pa_ref == None` arm produced before.
                    let num_to_search = [pic.ref_list0_count_try, pic.ref_list1_count_try];
                    let complete = (0..2).all(|li| {
                        (0..usize::from(num_to_search[li])).all(|ri| refs.arr[li][ri].is_some())
                    });
                    if !complete {
                        None
                    } else {
                        // Recycle the previous frame's result set.
                        // `run_frame_me_into` resets every per-b64 entry to
                        // exactly what `MeB64Output::new` builds and reassigns
                        // every scalar, so this is byte-identical to a fresh
                        // `run_frame_me`.
                        let mut out = self
                            .me_scratch
                            .take()
                            .unwrap_or_else(crate::inter_me_arm::FrameMe::empty);
                        crate::inter_me_arm::run_frame_me_into(
                            &mut out,
                            cur,
                            &refs,
                            num_to_search,
                            crate::inter_me_arm::FrameMeParams {
                                // C `pcs->enc_mode` — post-clamp
                                // (enc_handle.c:4433): the `sig_deriv_me`
                                // ladders branch at M11 (search area, prehme).
                                enc_mode: crate::rate_arm::eff_enc_mode(
                                    sc_arm,
                                    self.speed_config.preset,
                                ),
                                qp: self.rc_config.qp,
                                width: w,
                                height: h,
                                picture_number: display_order,
                                // C `frame_is_boosted(pcs)` (enc_mode_config.h:108)
                                // = `frame_is_kf_gf_arf` = intra-only || ARF || GF
                                // update. A flat low-delay P GOP DOES still emit
                                // GF_UPDATE frames (picture_decision marks the
                                // base of each mini-GOP `SVT_AV1_GF_UPDATE`), so
                                // `sig_deriv_me`'s `is_base ? 1 : 6` arm is live
                                // here — the `96x96 q20 p6` cell's poc4 is one.
                                frame_is_boosted: crate::port_picstruct::frame_is_boosted(pic),
                                hierarchical_levels: frame_hier,
                                // C `me_process.c:214-215` — `pcs->temporal_layer_index`
                                // / `pcs->is_ref`, straight off the picture decision.
                                temporal_layer_index: pic.temporal_layer_index,
                                is_ref: pic.is_ref,
                                sc_class5: u8::from(sc_derivation.classes.sc_class5),
                                // C `scs->mrp_ctrls` — set this frame by
                                // `run_picture_decision` above.
                                only_l_bwd: self.mrp_ctrls.only_l_bwd != 0,
                                safe_limit_nref: self.mrp_ctrls.safe_limit_nref,
                                safe_limit_zz_th: self.mrp_ctrls.safe_limit_zz_th,
                                // C `pcs->similar_brightness_refs` /
                                // `frame_is_leaf(pcs)` — picture decision's
                                // outputs, gating the safe-limit ME arm
                                // (`motion_estimation.c:2231`).
                                similar_brightness_refs: pic.similar_brightness_refs,
                                frame_is_leaf: crate::port_picstruct::frame_is_leaf(
                                    pic.update_type,
                                ),
                            },
                        );
                        Some(out)
                    }
                }
                _ => None,
            },
        };

        // THE single CLI-qp -> qindex conversion (C: quantizer_to_qindex
        // lookup on picture_qp, rc_crf_cqp.c). Everything above this line
        // (assign_picture_qp, VAQ, TPL) works in the CLI 0..63 domain where
        // those deltas were calibrated — one CLI step maps to ~4 qindex
        // steps through the table. Everything below (quantizer step
        // tables, CDF q bucket, EC base_q_idx, chroma quantization,
        // deblock level picker, FH base_q_idx) consumes ONLY this qindex.
        // Lambda is the documented exception: it stays CLI-qp-calibrated
        // (see qp_to_lambda) until C's lambda_rate_tables.h port lands.
        // Issue #9 item 4 (fractional CRF): the quarter-step remainder rides
        // in as a qindex offset exactly where C adds it (rc_crf_cqp.c:471).
        //
        // C then keeps TWO qp values and they are NOT interchangeable once the
        // offset is non-zero:
        //   * `scs->static_config.qp` — the CLI value, UNCHANGED by the
        //     offset. Every qp-keyed LEVEL derivation reads it:
        //     `svt_aom_get_nsq_search_level_allintra` (enc_mode_config.c:10014),
        //     the qp-based-threshold scaling (:338), the coeff-level complexity
        //     (md_config_process.c:620/651), the max-can-bsize picks
        //     (enc_dec_process.c:1645/1723/2393), the IntraBC mesh scaling
        //     (pd_process.c:3740) — all `static_config.qp`.
        //   * `ppcs->picture_qp = clamp_qp((base_q_idx + 2) >> 2)`
        //     (rc_process.c:861) — re-derived FROM the offset qindex, and read
        //     only by the frame `lambda_weight` ladder
        //     (enc_mode_config.c:10093-10108, both the tune-IQ curve and the
        //     PSNR 0/150/175 tiers).
        // `tpl_adjusted_qp` is this port's `static_config.qp` analogue, so it
        // stays in the CLI domain and `picture_qp` is derived alongside it.
        // MEASURED: collapsing both onto the qindex-derived value (the first
        // cut of this change) diverged from C at preset 2 / qp 20 /
        // offsets 2-3 — exactly the offsets where `(80+off+2)>>2` rolls from
        // 20 to 21 — as `tools/issue9_knobs_gate.sh` cells
        // `crf20.2-gradient-128-p2` / `crf20.3-...` (port 2664 B vs C 2628 B,
        // first divergence FH `loop_filter_level[0]` C=4 Rust=5). With
        // offset 0 the two values are equal, so every pre-existing cell is
        // byte-identical either way.
        // BITRATE RC dispatch — C's `rc_cfg.mode != AOM_Q` fork
        // (rc_process.c:850-859): a VBR/CBR frame's qindex comes from
        // `svt_av1_rc_process_rate_allocation` +
        // `svt_av1_rc_calc_qindex_rate_control`, NEVER from the
        // `svt_av1_rc_calc_qindex_crf_cqp` chain below — the `!allintra`
        // block is skipped alongside it. `cbr_frame_rc` carries the PPCS
        // RC fields forward to the packetization-feedback update at the
        // end of this function; `cbr_sb_plan` is cyclic refresh's per-SB
        // map (`None` = C's flat arm). Only `RcMode::Cbr` reaches this:
        // VBR is refused at `rate_control_config_error` (first-pass
        // statistics are not ported), and LD+CBR is C's own envelope.
        let mut cbr_frame_rc: Option<crate::port_rc_vbr_cbr_state::FrameRc> = None;
        let mut cbr_sb_plan: Option<crate::sb_qindex::SbQindexPlan> = None;
        #[allow(unused_mut)]
        let mut base_qindex = if self.rc_config.mode == crate::rate_control::RcMode::Cbr {
            let (q, frame, plan) = self.cbr_frame_qindex(
                pic_decision.as_ref(),
                is_key,
                display_order,
                frame_hier,
                sc_derivation.classes.sc_class1,
                frame_me.as_ref(),
            )?;
            cbr_frame_rc = Some(frame);
            cbr_sb_plan = plan;
            q
        } else {
            crate::rate_control::qp_to_qindex_with_offset(
                tpl_adjusted_qp,
                self.rc_config.extended_crf_qindex_offset,
            )
        };
        // VIDEO-MODE QP SCALING (inter campaign C1a). C's `cqp_qindex_calc`
        // (rc_crf_cqp.c:393, the mainline `#else` arm) returns the qindex
        // untouched when `scs->allintra` — the early return the entire still
        // envelope takes — and scales it otherwise. `allintra` here is the
        // still predicate the rest of this function already uses.
        //
        // MEASURED, gradient 64x64 in video mode (SVT_AVIF=0): C writes
        // base_q_idx 67 at cli qp40 where the still path writes 160, because a
        // video key frame is coded far finer than a still — later frames
        // reference it. The derivation is tier-1 verified against C's exported
        // `svt_av1_convert_qindex_to_q` and `svt_av1_compute_qdelta`, and
        // against the base_q_idx C actually writes on four cells.
        //
        // `is_ref`/`idr_flag` are true for the key frame this reaches today;
        // the non-base temporal-layer arm needs a DPB the port does not have,
        // and `cqp_qindex_calc` documents that it must not be used there yet.
        let allintra = self.gop.intra_period == 1;
        // C `svt_av1_rc_calc_qindex_crf_cqp`'s dispatch
        // (rc_crf_cqp.c:481-489): `enable_qp_scaling_flag` is `!allintra`
        // (enc_handle.c:4390) with `use_fixed_qindex_offsets == 1` as the
        // one subtract — that config is refused upstream — and inside it
        // `tpl_ctrls.enable` selects `crf_qindex_calc` over
        // `cqp_qindex_calc`. `tpl_r0` carries `ppcs->r0` through the
        // in-place adjustments `crf_qindex_calc` makes, because
        // `sb_qp_derivation_tpl_la` gates on the POST-adjust value
        // (rc_aq.c:899).
        let tpl_fti = tpl_in.as_ref().filter(|f| f.tpl_ctrls.enable != 0);
        // `R0Flags` is Copy — hoist the four capability flags so `tpl_in`'s
        // `frame_me` can be taken by the ME site below while the flags stay
        // readable for the lambda context and the ref-object stamp.
        let tpl_flags = tpl_fti.map(|f| f.flags);
        let r0_delta_qp_md = tpl_flags.is_some_and(|f| f.r0_delta_qp_md);
        let mut tpl_r0 = tpl_fti.map_or(0.0, |f| f.r0);
        // CBR never enters the CQP/CRF dispatch — its qindex is the one
        // `rc_calc_qindex_rate_control` already produced above (C's own
        // `mode != AOM_Q` fork, rc_process.c:853-859).
        if !allintra && self.rc_config.mode != crate::rate_control::RcMode::Cbr {
            let new_qindex: i32 = if let Some(fti) = tpl_fti {
                // `rc->active_worst_quality` — `scs_qindex` forever in the
                // 1-pass envelope: `svt_av1_rc_init` seeds it at
                // `scs_qindex` (rc_crf_cqp.c:486-488) and the ONLY writer
                // past that point is `svt_aom_crf_assign_max_rate`, the
                // `max_bit_rate` arm that is refused upstream.
                let pic = pic_decision.as_ref();
                let ref0 = pic.and_then(|p| self.dpb.get(p.rps.ref_dpb_index[0] as usize));
                let ref1 = pic.and_then(|p| {
                    (p.slice_type == crate::port_picstruct::SliceType::B
                        && p.ref_list1_count_try != 0)
                        .then(|| self.dpb.get(p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                });
                let crf_out = crate::rate_control::crf_qindex_calc(
                    i32::from(base_qindex),
                    &crate::rate_control::CrfQindexInputs {
                        is_intra_only: is_key,
                        temporal_layer_index: temporal_layer,
                        hierarchical_levels: frame_hier,
                        is_highest_layer: crate::port_picstruct::is_highest_layer(
                            temporal_layer,
                            frame_hier,
                        ),
                        r0_qps: fti.flags.r0_qps,
                        r0: fti.r0,
                        r0_adjust_factor: fti.tpl_ctrls.r0_adjust_factor,
                        used_tpl_frame_num: fti.used_tpl_frame_num,
                        tpl_group_size: fti.tpl_group_size,
                        // `scs->lad_mg != 0` — C's CONFIG value
                        // (enc_handle.c:4041-4065), `= scs->tpl_lad_mg`
                        // under CQP_OR_CRF: 0 for allintra/LOW_DELAY, else
                        // 1 whenever TPL is in play (the RA window this
                        // pipeline buffers is a full mini-GOP, so C's
                        // `look_ahead < mg_size` test is false). It gates
                        // the base-layer weight bump `weight = min(w+0.1,1)`
                        // (rc_crf_cqp.c:285-287) — with it off, a base
                        // frame's `r0_weight[1]=0.9` never bumps to 1.0
                        // and `qstep_ratio`/`base_q_idx` come out low
                        // (johnny 9f p6: poc8 port 121 vs C 128).
                        scs_lad_mg: !allintra
                            && self.pred_structure
                                != crate::port_picstruct::PredStructure::LowDelay,
                        input_resolution: i32::from(
                            crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                                self.width * self.height,
                            ) as u8,
                        ),
                        bit_depth: self.bit_depth,
                        sc_class1: sc_derivation.classes.sc_class1,
                        // `SVT_QP_SCALE_WEIGHT`/`_ON` (definitions.h:245-253)
                        // — mainline's 4-entry table vs the fork's linear
                        // formula on `hdr.qp_scale_compress_strength`.
                        qp_scale_weight: if self.hdr.is_fork() {
                            1.0 + self.hdr.qp_scale_compress_strength * 0.125
                        } else {
                            crate::rate_control::QP_SCALE_COMPRESS_WEIGHT
                                [self.hdr.qp_scale_compress_strength.clamp(0.0, 3.0) as usize]
                        },
                        // Mainline's field is a `uint8_t` index — sub-1.0
                        // values truncate to 0, i.e. OFF, exactly like C.
                        qp_scale_on: if self.hdr.is_fork() {
                            self.hdr.qp_scale_compress_strength > 0.0
                        } else {
                            self.hdr.qp_scale_compress_strength >= 1.0
                        },
                        // `rc->best_quality`/`worst_quality` =
                        // `quantizer_to_qindex[min/max_qp_allowed]` — C's
                        // defaults are 0..63, i.e. the full u8 range.
                        best_quality: 0,
                        worst_quality: 255,
                        // `rc->arf_q` = `ref_base_q_idx[L0][0]`, max'd
                        // with L1[0] for a B slice with list1 refs
                        // (rc_crf_cqp.c:200-204).
                        arf_q: ref0
                            .map_or(i32::from(base_qindex), |r| i32::from(r.base_q_idx))
                            .max(ref1.map_or(0, |r| i32::from(r.base_q_idx))),
                        ref0_tmp_layer: ref0.map_or(0, |r| r.temporal_layer),
                        ref1_tmp_layer: ref1.map(|r| r.temporal_layer),
                        ref_intra_percentage: i32::from(md_ref_intra_percentage),
                    },
                );
                tpl_r0 = crf_out.r0;
                crf_out.qindex
            } else {
                // rc_crf_cqp.c:439-444 — the LOW_DELAY non-base boost reads
                // the L0 reference's per-SB intra counts (`get_ref_obj(pcs,
                // REF_LIST_0, 0)` == the picture `ref_dpb_index[LAST]` names).
                // `None` whenever there is no picture decision or this is a
                // base-layer/key frame — the arm is gated on
                // `temporal_layer_index != 0` inside `cqp_qindex_calc` too.
                // C gates the boost on `scs->static_config.pred_structure ==
                // LOW_DELAY` (rc_crf_cqp.c:439) — the SEQUENCE's configured
                // structure. A cut-short RA mini-GOP flips its pictures to
                // `pred_struct_type LowDelay` but the sequence stays
                // RANDOM_ACCESS, so the boost must not fire under RA even
                // though the picture's own pred-struct type says low-delay.
                let ld_boost = (self.pred_structure
                    == crate::port_picstruct::PredStructure::LowDelay)
                    .then(|| {
                        pic_decision.as_ref().and_then(|p| {
                            let rf = self.dpb.get(p.rps.ref_dpb_index[0] as usize)?;
                            Some(crate::rate_control::non_base_boost(
                                rf.is_islice,
                                &rf.sb_intra,
                            ))
                        })
                    })
                    .flatten();
                crate::rate_control::cqp_qindex_calc(
                    i32::from(base_qindex),
                    allintra,
                    /*slice_is_intra=*/ is_key,
                    /*is_ref=*/ pic_decision.as_ref().is_none_or(|p| p.is_ref),
                    /*idr_flag=*/ is_key,
                    temporal_layer,
                    frame_hier,
                    self.bit_depth,
                    ld_boost,
                )
            };
            base_qindex = new_qindex.clamp(0, 255) as u8;
            // C's extended-CRF arm (rc_crf_cqp.c:510-513) applies to the
            // POST-dispatch qindex whenever `qp == 63` and the offset is
            // nonzero — outside the `enable_qp_scaling_flag` gate, so it
            // covers the TPL arm's output too.
            if self.rc_config.qp == 63 && self.rc_config.extended_crf_qindex_offset != 0 {
                let off = i32::from(self.rc_config.extended_crf_qindex_offset);
                base_qindex = (i32::from(base_qindex) + (255 - i32::from(base_qindex)) * off / 56)
                    .clamp(0, 255) as u8;
            }
        }
        let mut picture_qp = crate::rate_control::picture_qp_from_qindex(base_qindex);
        if crate::dbgenv::qtrace() {
            eprintln!(
                "QTRACE display={display_order} base_qindex={base_qindex} temporal_layer={temporal_layer} hier={frame_hier} is_ref={:?} update_type={:?}",
                pic_decision.as_ref().map(|p| p.is_ref),
                pic_decision.as_ref().map(|p| p.update_type),
            );
        }
        // C's EXTENDED-CRF lambda bump (enc_mode_config.c:10109-10114): for
        // CRF 63.25..70 only — `static_config.qp == MAX_QP_VALUE (63)` with a
        // non-zero `extended_crf_qindex_offset` — the frame `lambda_weight`
        // gains `offset * 28`. This is the ONLY effect the offset has at qp 63:
        // the qindex itself saturates (`quantizer_to_qindex[63] == 255`, and
        // `clamp_qindex` caps at the max-qp qindex), so `(MAXQ - new_qindex) *
        // offset / 56` in rc_crf_cqp.c:511 evaluates to 0 there. 0 on every
        // other config, hence byte-inert everywhere else.
        let lw_bump: u32 = if self.rc_config.qp == 63 {
            u32::from(self.rc_config.extended_crf_qindex_offset) * 28
        } else {
            0
        };
        // [SVT_HDR_MODE] fork Variance Boost: derive the per-SB qindex plan
        // (sb_qindex.rs = C variance_adjust_qp(readjust=true) chain). The
        // recentered base REPLACES base_qindex BEFORE every downstream
        // consumer (lambda, CDF bucket, deblock, FH) — C order: rc_aq runs
        // in rc_init_sb_qindex ahead of MD. picture_qp follows C's
        // (base+2)>>2 update.
        let mut sb_plan = if self.rc_config.mode == crate::rate_control::RcMode::Cbr {
            // C `svt_av1_rc_init_sb_qindex` (rc_aq.c:879-885): under AOM_CBR
            // the cyclic-refresh decision made inside
            // `rc_calc_qindex_rate_control` is the ONLY per-SB plan —
            // variance boost and the TPL arm below are skipped entirely
            // ("mutually exclusive with other AQ modes"). `None` is C's
            // flat arm — every SB takes the frame `base_q_idx` and
            // `delta_q_present` stays 0.
            cbr_sb_plan
        } else if self.hdr.enable_variance_boost {
            let sb_cols_p = w.div_ceil(64);
            let sb_rows_p = h.div_ceil(64);
            // C iterates the per-SB plan `sb_addr < scs->sb_total_count`
            // (rc_aq.c:465 / :233) over `ppcs->variance[sb_addr]` — but that
            // array is the per-B64 map picture analysis fills
            // (pic_analysis_process.c:414). At sb_size 128 sb_total_count is
            // a QUARTER of the b64 count, so C's plan consumes only the
            // first sb_cnt b64 entries (raster order = the frame's top-left
            // quadrant) for every real SB. Mirror the quirk by truncating
            // the b64 map to the real SB count; at sb_size 64 the counts are
            // equal and this is byte-neutral.
            let sb_cnt = w.div_ceil(self.sb_size) * h.div_ceil(self.sb_size);
            let mut vars = svtav1_types::try_with_capacity![sb_cols_p * sb_rows_p]?;
            for r in 0..sb_rows_p {
                for c in 0..sb_cols_p {
                    vars.push(crate::sb_qindex::compute_sb_variances(
                        &encode_input,
                        w,
                        w,
                        h,
                        c * 64,
                        r * 64,
                    ));
                }
            }
            vars.truncate(sb_cnt);
            // C has TWO boost paths and they take DIFFERENT variance domains:
            // mainline (rc_aq.c:350/454) reads the INTEGER per-b64 map that
            // picture analysis builds (`pd0::compute_b64_variance`) and leaves
            // the frame base alone; the fork build (rc_aq.c:87/226) reads f64
            // maps, takes a mean, and resignals the recentered base. Feeding
            // the fork kernel on a mainline encode computes the boost in the
            // wrong domain and returns 0 — which is what made mainline tune IQ
            // emit a flat delta-q plan where C emits a real one.
            let plan = if self.hdr.is_fork() {
                crate::sb_qindex::variance_adjust_qp(
                    base_qindex,
                    &vars,
                    self.hdr.variance_boost_strength,
                    self.hdr.variance_octile,
                    self.hdr.variance_boost_curve,
                    tpl_adjusted_qp,
                    self.bit_depth,
                )
            } else {
                let ivars: alloc::vec::Vec<crate::pd0::SbVariance> = (0..sb_rows_p)
                    .flat_map(|r| (0..sb_cols_p).map(move |c| (r, c)))
                    .map(|(r, c)| {
                        crate::pd0::compute_b64_variance(sb_input, in_stride, c * 64, r * 64)
                    })
                    .take(sb_cnt)
                    .collect();
                crate::sb_qindex::variance_adjust_qp_mainline(
                    base_qindex,
                    &ivars,
                    self.hdr.variance_boost_strength,
                    self.hdr.variance_octile,
                    self.hdr.variance_boost_curve,
                    tpl_adjusted_qp,
                    self.bit_depth,
                )
            };
            base_qindex = plan.base_qindex;
            // The fork's recentered base moves BOTH: C's variance-boost path
            // resignals `frm_hdr.base_q_idx` before rate control's
            // `picture_qp` update, and the port has always carried the
            // recentre into its single CLI-domain qp. Keep that (identical to
            // the pre-split behaviour whenever the CRF offset is 0, which is
            // every fork cell the gates cover).
            picture_qp = crate::rate_control::picture_qp_from_qindex(plan.base_qindex);
            tpl_adjusted_qp = picture_qp;
            Some(plan)
        } else {
            None
        };

        // C `svt_av1_rc_init_sb_qindex`'s TPL arm (rc_aq.c:897-901): under
        // `aq_mode == 2 && tpl_ctrls.enable && ppcs->r0 != 0` — the r0 the
        // qindex dispatch just adjusted, not the stage's raw one — each
        // superblock takes `get_deltaq_offset(beta)` on top of whatever
        // qindex map is live (the frame base, or the variance plan's), and
        // `sb_setup_lambda` folds the SB's rdmult ratio into
        // `tpl_sb_rdmult_scaling_factors`, the grid `blk_lambda_tuning`
        // reads per block. `delta_q_present` flips ONLY on
        // `r0_delta_qp_quant` (rc_aq.c:790-791); the SB loop itself needs
        // `r0_delta_qp_md && tpl_is_valid` (:799). `r0_delta_qp_md` without
        // `r0_delta_qp_quant` still quantizes at the per-SB qindex while
        // signalling nothing — C's own encoder/decoder disagreement,
        // reproduced rather than "fixed".
        let mut delta_q_present = sb_plan.is_some();
        let mut tpl_rdmult: Option<alloc::sync::Arc<crate::port_md_lambda::TplRdmult>> = None;
        if let Some(fti) = tpl_fti
            && self.rc_config.aq_mode == 2
            && tpl_r0 != 0.0
        {
            if fti.flags.r0_delta_qp_quant {
                delta_q_present = true;
            }
            if fti.flags.r0_delta_qp_md && fti.tpl_is_valid {
                let sb_cols = w.div_ceil(self.sb_size);
                let sb_rows = h.div_ceil(self.sb_size);
                let sb_cnt = sb_cols * sb_rows;
                debug_assert_eq!(fti.tpl_beta.len(), sb_cnt);
                // The SB map starts from whatever `rc_init_sb_qindex`
                // left: the variance plan's post-normalization values, else
                // the frame base on every SB.
                let mut sb_qindex: Vec<u8> = sb_plan
                    .as_ref()
                    .map_or_else(|| alloc::vec![base_qindex; sb_cnt], |p| p.sb_qindex.clone());
                crate::sb_qindex::sb_qp_derivation_tpl_la(
                    self.bit_depth,
                    is_key,
                    &fti.tpl_beta,
                    &mut sb_qindex,
                );
                // `generate_b64_me_qindex_map` (rc_process.c:747) feeds
                // `svt_aom_get_me_qindex`, `sb_setup_lambda`'s `me_qindex`
                // input. Under `r0_delta_qp_md` `update_lambda`'s arm reads
                // `q_index` so the value is inert INSIDE this pass — but
                // the same map is live for the frame's per-SB MD lambdas
                // below, which is why it is built here and not skipped.
                let b64_me_qindex = fti.frame_me.as_ref().map(|me| {
                    let mev: Vec<u32> = me.per_b64.iter().map(|o| o.me_8x8_cost_variance).collect();
                    crate::port_rc_process::generate_b64_me_qindex_map(
                        &mev,
                        i32::from(base_qindex),
                        is_key,
                    )
                });
                let lctx = crate::port_rc_process::LambdaContext {
                    frame_type: i32::from(!is_key),
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: md_lambda_base_update_type
                        .unwrap_or(crate::port_rc_process::FrameUpdateType::KfUpdate),
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    stats_based_sb_lambda_modulation:
                        crate::port_rc_process::stats_based_sb_lambda_modulation(
                            crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                            false,
                        ),
                    base_q_idx: i32::from(base_qindex),
                    delta_q_present,
                    r0_delta_qp_md: true,
                    // `scs->static_config.lambda_scale_factors` — the port
                    // does not expose the knob; 128 is C's identity.
                    lambda_scale_factors: [128; 7],
                };
                // `pcs->hbd_md` (enc_mode_config.c:2151-2164): bd10 &&
                // preset <= M5 selects the 10-bit MD quantizer for the
                // `compute_rd_mult` inside `sb_setup_lambda`; 8 elsewhere.
                let hbd_md_depth = match self.bit_depth {
                    10 if self.speed_config.preset <= 5 => 10,
                    _ => 8,
                };
                let mi_rows = (h >> 2) as i32;
                let mut sb_factors = fti.tpl_rdmult_scaling_factors.clone();
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(&stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let me_q = b64_me_qindex.as_ref().map_or(base_qindex, |m| {
                            crate::port_md_rate_estimation::get_me_qindex(
                                m,
                                u16::try_from(w).unwrap_or(u16::MAX),
                                u16::try_from(h).unwrap_or(u16::MAX),
                                u32::try_from(sb_idx).unwrap_or(u32::MAX),
                                u32::try_from(sb_col * self.sb_size).unwrap_or(u32::MAX),
                                u32::try_from(sb_row * self.sb_size).unwrap_or(u32::MAX),
                                self.sb_size == 128,
                            )
                        });
                        crate::sb_qindex::sb_setup_lambda(
                            u32::try_from(sb_col * self.sb_size).unwrap_or(u32::MAX),
                            u32::try_from(sb_row * self.sb_size).unwrap_or(u32::MAX),
                            self.superres_denom.unwrap_or(8),
                            u32::try_from(w).unwrap_or(u32::MAX),
                            self.sb_size == 128,
                            fti.synth_blk_size,
                            mi_rows,
                            base_qindex,
                            sb_qindex[sb_idx],
                            me_q,
                            &lctx,
                            hbd_md_depth,
                            &fti.tpl_rdmult_scaling_factors,
                            &mut sb_factors,
                        );
                    }
                }
                #[cfg(feature = "std")]
                if std::env::var_os("SVTAV1_TPLQP").is_some() {
                    let poc = pic_decision
                        .as_ref()
                        .map_or(u32::MAX, |p| p.picture_number as u32);
                    let res_dbg = sb_plan.as_ref().map_or_else(
                        || crate::sb_qindex::delta_q_res_for(self.rc_config.qp, false),
                        |p| p.delta_q_res,
                    );
                    std::eprintln!(
                        "TPLQP pic={poc} dq={} res={} qmd={} qquant={} valid={} n_sb={}",
                        u8::from(delta_q_present),
                        res_dbg,
                        u8::from(fti.flags.r0_delta_qp_md),
                        u8::from(fti.flags.r0_delta_qp_quant),
                        u8::from(fti.tpl_is_valid),
                        sb_cnt
                    );
                    for (i, (&q, &b)) in sb_qindex.iter().zip(fti.tpl_beta.iter()).enumerate() {
                        std::eprintln!("SBQP pic={poc} sb={i} qindex={q} beta={b:.10}");
                    }
                    for (i, (&bse, &sbf)) in fti
                        .tpl_rdmult_scaling_factors
                        .iter()
                        .zip(sb_factors.iter())
                        .enumerate()
                    {
                        std::eprintln!("SBF pic={poc} i={i} base={bse:.10} sb={sbf:.10}");
                    }
                }
                // `reset_enc_dec`'s picture lambdas
                // (enc_dec_process.c:176-187): `svt_aom_lambda_assign` at
                // the FH base qindex, `multiply_lambda=true`, at both MD
                // depths. These are the bases `svt_aom_set_tuned_blk_lambda`
                // scales per block — NO `lambda_weight`, no per-SB stats
                // modulation (its `q_index == base_q_idx` makes the qdiff
                // factor the identity under every arm).
                let (pic_fast8, pic_full8) =
                    crate::port_rc_process::lambda_assign(&lctx, 8, base_qindex, true);
                let (_pic_fast10, pic_full10) =
                    crate::port_rc_process::lambda_assign(&lctx, 10, base_qindex, true);
                tpl_rdmult = Some(alloc::sync::Arc::new(crate::port_md_lambda::TplRdmult {
                    factors: sb_factors,
                    mi_rows,
                    unscaled_width: w as i32,
                    superres_denom: i32::from(self.superres_denom.unwrap_or(8)),
                    synth_blk_32: fti.synth_blk_size == 32,
                    sb_size_is_128: self.sb_size == 128,
                    pic_full8,
                    pic_full10,
                    pic_fast8,
                }));
                // C's ONE normalize call in `generate_sb_qindex`
                // (rc_process.c:741-744) sees the map AFTER TPL modified
                // it. The variance arm normalized inside its own port, so
                // applying TPL on top undoes the residue snap — a live
                // `delta_q_res != 1` re-normalizes here. On the
                // variance+TPL combination this sits within one res step
                // of C's var(raw) -> tpl -> normalize order; the measured
                // parity envelope is variance-off cells.
                let res = sb_plan.as_ref().map_or_else(
                    || crate::sb_qindex::delta_q_res_for(self.rc_config.qp, false),
                    |p| p.delta_q_res,
                );
                if delta_q_present && res != 1 {
                    let mut map_i32: Vec<i32> = sb_qindex.iter().map(|&q| i32::from(q)).collect();
                    crate::sb_qindex::normalize_sb_delta_q(base_qindex, res, &mut map_i32);
                    sb_qindex = map_i32.iter().map(|&q| q.clamp(0, 255) as u8).collect();
                }
                match sb_plan.as_mut() {
                    Some(p) => p.sb_qindex = sb_qindex,
                    None => {
                        sb_plan = Some(crate::sb_qindex::SbQindexPlan {
                            base_qindex,
                            sb_qindex,
                            // `get_delta_q_res(qp, enable_variance_boost =
                            // false)` is `DEFAULT_DELTA_Q_RES` — 1. This
                            // field only reaches the FH when
                            // `delta_q_present` (i.e. `r0_delta_qp_quant`).
                            delta_q_res: 1,
                        });
                    }
                }
            }
        }

        // Issue #5: `base_qindex == 0` signals CODED-LOSSLESS in the frame
        // header (spec 5.9.2 — with the zero chroma deltas and no
        // segmentation of this port's mainline path, base_q_idx 0 IS
        // CodedLossless), and the whole encode follows C's lossless rules:
        // the header writes no deblock/CDEF/LR/tx_mode bits (chunk 1,
        // 2026-08-27), every block is 4x4 or 8x8 coded at TX_4X4 with the
        // Walsh-Hadamard transform, no tx_size / tx_type symbols, RDOQ and
        // the tx-type search off, only DCT-chroma candidates injected, and no
        // in-loop filter runs (chunk 2, this arm's consumers below +
        // leaf_funnel). The envelope that is byte-verified against the C
        // oracle is `lossless_config_error`'s complement; everything outside
        // it is REFUSED rather than encoded wrong (the pre-chunk-2 measurement
        // of what "encoded wrong" looked like: ssim2 -200..-1100 vs source).
        let coded_lossless = base_qindex == 0;
        if coded_lossless && let Some(why) = self.lossless_config_error(is_key) {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(why)));
        }
        // The fork's tune-SSIM parallel full cost (`FunnelFrame::tune_ssim`,
        // armed by `alt_ssim_tuning`) has no INTER skip arm in the port
        // (leaf_funnel/mds3.rs asserts it away). Refuse the inter frame instead
        // of reaching that assert. MEASURED 2026-09-25 on i265: identity_run
        // `gradient 128 128 40 5` with SVT_HDR_MODE=1 SVT_FORK_ALT_SSIM_TUNING=1
        // SVTAV1_FRAMES=4 panicked in release on an inter frame.
        if !is_key && self.hdr.is_fork() && self.hdr.alt_ssim_tuning {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(
                "alt_ssim_tuning on inter frames is not ported (the tune-SSIM full cost has no \
                 inter skip arm) [C: accepts]"
            )));
        }
        // The header omits delta-q at base_q_idx 0. C's full_loop.c selects
        // the frame qindex whenever delta_q_present is false, even if the
        // variance planner produced positive per-SB indices. Preserve that
        // planner, but only feed a signaled plan to MD, quantization and pack.
        //
        // TWO plans, C's split: `delta_q_plan` is the SIGNAL side — what the
        // frame header's `delta_q_present` and the per-SB delta-q symbols
        // carry — while `md_sb_qindex` is `pcs->sb_ptr_array[..].qindex` at
        // the end of `generate_sb_qindex`, the map MD and quantization
        // consume (`ctx->qp_index = delta_q_present || r0_delta_qp_md ?
        // sb_qp : base_q_idx`, md_process.c:800-803). They differ exactly
        // when `r0_delta_qp_md` is on without `r0_delta_qp_quant`: C
        // quantizes per-SB and signals nothing.
        let delta_q_plan = sb_plan
            .as_ref()
            .filter(|_| delta_q_present && !coded_lossless);
        let md_sb_qindex = sb_plan.as_ref().filter(|_| !coded_lossless);

        // C-exact coding quantizer for the still/PD1 path (quant.rs): the
        // frame-level rdoq_level from `derive_intra_coeff_level`
        // (pic_avg_variance = mean of the per-B64 64x64 variances,
        // pic_analysis_process.c:608, truncated to u16) via the allintra
        // policy, the KF full lambda, and the default-CDF coefficient cost
        // tables. Only key/still frames at presets >= 4 (the PD0
        // fixed-tree paths: eff-M9 above 8, PD0_LVL_1 at 4..8 — the C
        // rdoq policy line `<=M5 -> 1, else f(coeff_lvl)` covers both,
        // enc_mode_config.c:14931) on 64-aligned dims — everywhere else
        // the legacy dead-zone quantizer stays.
        // MOVED UP with the inter MD derivations below (§1s item 8): MODE
        // DECISION prices against `md_frame_context`, which C copies from
        // this same reference (md_config_process.c:299-310) — so the binding
        // has to exist before MD runs, not only before the entropy walk.
        // CDF CONTINUATION, RESTORE side — C `reset_entropy_coding_picture`
        // (`ec_process.c:101-112`):
        //
        //     if (primary_ref_frame != PRIMARY_REF_NONE)
        //         svt_memcpy(ec->fc, &ref->frame_context, sizeof(FRAME_CONTEXT));
        //     else
        //         svt_aom_reset_entropy_coder(...);
        //
        // The DPB slot is the one the FRAME HEADER names:
        // `ref_frame_idx[primary_ref_frame]`, i.e. `rps.ref_dpb_index[]`, which
        // is what a DECODER resolves. (C indexes its own
        // `ref_pic_ptr_array[list][idx]` via `get_list_idx`/`get_ref_frame_idx`
        // instead; the two agree, and the spec mapping is the one conformance
        // depends on, so that is the one used here.)
        //
        // `binding` is recomputed at the header-assembly site below from the
        // same pure inputs; `primary_ref_frame_for_cdf` is carried down so the
        // two are ASSERTED equal rather than assumed — a tile coded against
        // slot A while the header announces slot B is a decoder desync, and it
        // is exactly the kind of divergence no byte count would explain.
        let (primary_ref_frame_for_cdf, primary_ref_cdfs) = if is_key {
            (crate::port_picstruct::PRIMARY_REF_NONE, None)
        } else if let Some(pic) = pic_decision.as_ref() {
            let ref_queue =
                crate::inter_hdr_arm::ref_queue_from_dpb(&pic.ref_queue_dpb, base_qindex);
            let b = crate::port_picstruct::bind_refs_and_primary_ref_frame(
                pic, &ref_queue, /*frame_end_cdf_update_mode=*/ true,
                /*is_s_frame=*/ false,
            );
            let prf = b.primary_ref_frame;
            let cdfs = if prf == crate::port_picstruct::PRIMARY_REF_NONE {
                None
            } else {
                let slot = pic.rps.ref_dpb_index[prf as usize] as usize;
                let stored = self.dpb.get(slot).and_then(|rf| rf.frame_cdfs.clone());
                if stored.is_none() {
                    // REFUSE rather than fall back to the defaults. The header
                    // this frame is about to write says "start from slot N's
                    // end-of-frame CDFs"; coding against the defaults instead
                    // produces a stream a conforming decoder turns into
                    // garbage, which is the one failure mode `docs/WORKING-ON-
                    // THIS.md` §6 exists to forbid. It is also the POSITIVE
                    // CONTROL for this wiring: if the store ever stops
                    // running, every inter cell fails loudly instead of
                    // quietly regressing to default-CDF bytes.
                    return Err(whereat::at!(EncodeError::UnsupportedConfig(
                        "the frame header names a primary_ref_frame, but the DPB slot it \
                         resolves to carries no saved CDF state — the referenced frame's \
                         entropy walk never ran (crate::port_frame_cdf)",
                    )));
                }
                stored
            };
            (prf, cdfs)
        } else {
            (crate::port_picstruct::PRIMARY_REF_NONE, None)
        };

        // GLOBAL MOTION, decided per FRAME (see `gm_level_for_frame` /
        // `gm_search_config_error`). It lives here because C decides it here too:
        // `me_process.c:264-272` calls `svt_aom_global_motion_estimation` the
        // moment the last b64's ME lands, and every input it reads
        // (`rc_me_distortion`, `rc_me_allow_gm`) is an ME output.
        //
        // `input_width`/`input_height` are C's `pcs->enhanced_pic`
        // (`me_process.c:136`) — the SOURCE picture — so the TRUE dims, not
        // the SB-aligned pair `frame_me` was run over. On a 64-aligned cell
        // the two agree; on a partial-SB cell they do not, and the integer
        // divide is what the whole decision turns on.
        let gm_estimation = frame_me.as_ref().and_then(|me| {
            let gm_level = self.gm_level_for_frame(is_key);
            let ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
                gm_level,
                crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                    self.width * self.height,
                ),
            )?;
            let dist: alloc::vec::Vec<u32> =
                me.per_b64.iter().map(|b| b.rc_me_distortion).collect();
            let allow: alloc::vec::Vec<u8> = me.per_b64.iter().map(|b| b.rc_me_allow_gm).collect();
            crate::port_global_me::global_motion_estimation(
                &crate::port_global_me::GmEstimationInputs {
                    gm_ctrls: ctrls,
                    rc_me_distortion: &dist,
                    rc_me_allow_gm: &allow,
                    input_width: self.true_width,
                    input_height: self.true_height,
                    ref_list0_count_try: pic_decision
                        .as_ref()
                        .map_or(0, |p| u32::from(p.ref_list0_count_try)),
                    ref_list1_count_try: pic_decision
                        .as_ref()
                        .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                    temporal_layer_index: temporal_layer,
                    // `pp_enabled` is false at every level this port can
                    // express, so this is never read; `false` is C's own
                    // initial value.
                    gm_pp_detected: false,
                },
            )
        });
        // C's SEARCH, when the derivation above says C would run one
        // (`global_me.c:190-300`). `None` when it would not — every reference
        // then keeps the IDENTITY model C initialised, and no correspondence
        // set, RANSAC fit or warp is computed, exactly as in C.
        //
        // The SOURCE plane is C's `input_pic` (`pcs->enhanced_pic`,
        // `me_process.c:136`) and the REFERENCE is the PA reference's
        // `input_padded_pic` — the same pyramid ME searched. Both are read
        // through clamped addressing (`port_warp`'s `sample_x.clamp(0, width -
        // 1)`) and from the picture ORIGIN, so a tightly-packed plane gives the
        // same answer as C's bordered one; the strides differ and the pixels do
        // not.
        let gm_models = match gm_estimation.as_ref() {
            Some(g) if !g.all_identity() => {
                let me = frame_me.as_ref().expect("gm_estimation implies frame_me");
                let ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
                    self.gm_level_for_frame(is_key),
                    crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                        self.width * self.height,
                    ),
                );
                let pa_ref = self.pa_ref.as_deref();
                match (ctrls, pa_ref) {
                    (Some(ctrls), Some(pr)) => {
                        // C's `me_results[b64]` arrays are one allocation per
                        // b64; `MeResultsView` wants them flat across the
                        // picture, which is how C's `pa_me_data` indexes them.
                        let pu = crate::inter_me::context::SQUARE_PU_COUNT;
                        let mut totals: alloc::vec::Vec<u8> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu);
                        let mut cands: alloc::vec::Vec<crate::port_md::predicates::MeCandidateRef> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu * me.max_cand);
                        let mut mvs: alloc::vec::Vec<svtav1_types::motion::Mv> =
                            alloc::vec::Vec::with_capacity(me.per_b64.len() * pu * me.max_refs);
                        for b in &me.per_b64 {
                            totals.extend_from_slice(&b.total_me_candidate_index);
                            cands.extend(b.me_candidate_array.iter().map(|c| {
                                crate::port_md::predicates::MeCandidateRef {
                                    direction: c.direction(),
                                    ref_idx_l0: c.ref_idx_l0(),
                                    ref_idx_l1: c.ref_idx_l1(),
                                    ref0_list: c.ref0_list(),
                                    ref1_list: c.ref1_list(),
                                }
                            }));
                            mvs.extend_from_slice(&b.me_mv_array);
                        }
                        let view = crate::port_gm_correspondence::MeResultsView {
                            total_me_candidate_index: &totals,
                            me_candidate_array: &cands,
                            me_mv_array: &mvs,
                            pu_count: pu,
                            max_cand: me.max_cand,
                            max_refs: me.max_refs,
                            max_l0: me.max_l0,
                        };
                        let geom = crate::port_gm_correspondence::GmPictureGeometry {
                            aligned_width: w as u32,
                            aligned_height: h as u32,
                            b64_size: 64,
                            enable_me_8x8: me.enable_me_8x8,
                            enable_me_16x16: me.enable_me_16x16,
                            gm_downsample_level:
                                crate::port_gm_correspondence::GmDownsampleLevel::Full,
                        };
                        // `encode_input` is at stride `w` (the ALIGNED width);
                        // `in_stride` belongs to `sb_input`, the SB-extent
                        // padded twin, and they differ on any frame whose
                        // aligned dims are not a multiple of 64.
                        let src_plane = crate::port_global_me::GmPlane {
                            buf: &encode_input,
                            stride: w,
                            width: self.true_width,
                            height: self.true_height,
                        };
                        let rp = &pr.full;
                        let ref_plane = crate::port_global_me::GmPlane {
                            buf: &rp.buf[rp.org..],
                            stride: rp.stride,
                            width: self.true_width,
                            height: self.true_height,
                        };
                        // C `pcs->pa_ref_pic_ptr_array[list][ref]`. The
                        // (list, ref) pair names a REFERENCE FRAME, and the
                        // DPB slot it resolves to is the one the header's
                        // `ref_frame_idx[]` carries: list 0 is
                        // LAST..GOLDEN (entries 0..3) and list 1 is
                        // BWDREF..ALTREF (entries 4..6), which is C's
                        // `get_list_idx` / `get_ref_frame_idx` read backwards.
                        //
                        // Resolved into planes HERE rather than inside the
                        // closure because the closure also borrows `self`
                        // through `true_width`/`true_height`.
                        let mut slot_pics = [0u64; 7];
                        let slot_planes: [Option<crate::port_global_me::GmPlane<'_>>; 7] =
                            core::array::from_fn(|i| {
                                let slot = pic_decision.as_ref()?.rps.ref_dpb_index[i] as usize;
                                let pa = self.pa_slots.get(slot)?.as_ref()?;
                                slot_pics[i] = pa.picture_number;
                                Some(crate::port_global_me::GmPlane {
                                    buf: &pa.full.buf[pa.full.org..],
                                    stride: pa.full.stride,
                                    width: self.true_width,
                                    height: self.true_height,
                                })
                            });
                        if crate::dbgenv::gmdbg() {
                            eprintln!(
                                "GMSLOTS poc={display_order} ref_dpb={:?} pics={:?} pa_ref={:?}",
                                pic_decision.as_ref().map(|p| p.rps.ref_dpb_index),
                                slot_pics,
                                self.pa_ref.as_ref().map(|p| p.picture_number)
                            );
                        }
                        let mut sink = |a: core::fmt::Arguments<'_>| {
                            if crate::dbgenv::gmdbg() {
                                eprintln!("GMSEARCH poc={display_order} {a}");
                            }
                        };
                        crate::port_global_me::global_motion_search(
                            g,
                            &ctrls,
                            &view,
                            &geom,
                            src_plane,
                            &|l, r| {
                                // Every (list, ref) resolves through the
                                // DPB table first: `ref_pa_pic_ptr_array`
                                // names the picture the RPS chose, which is
                                // NOT `pa_ref` once hierarchical reference
                                // selection puts an older frame in the
                                // nearest list-0 slot (measured 2026-09-25:
                                // LD+hl3 poc3's list0 ref0 is a DIFFERENT
                                // picture than the previous frame — warping
                                // pa_ref computed pic_sad 1072640 where C's
                                // own dump reads 1879552, rejecting a
                                // translation model C accepts).
                                //
                                // `pa_ref` remains the (0,0) fallback for
                                // the flat single-reference path, whose
                                // slots are populated only at the presets
                                // where `gm_level` is non-zero.
                                let idx = if l == 0 { r } else { 4 + r };
                                slot_planes
                                    .get(idx)
                                    .copied()
                                    .flatten()
                                    .or(if l == 0 && r == 0 && !decided_is_some {
                                        Some(ref_plane)
                                    } else {
                                        None
                                    })
                            },
                            // C's `allow_high_precision_mv` argument is
                            // `pcs->frm_hdr.allow_high_precision_mv`
                            // (`global_me.c:270`), and at ME time that field
                            // is STILL ZERO: it is assigned in
                            // `svt_aom_sig_deriv_mode_decision_config`
                            // (md_config_process), which runs AFTER
                            // me_process. MEASURED with `SVT_GMSEARCH_OUT` on
                            // the 33/32-zoom photo 256 cell at q10 and q20 —
                            // two quantizers whose final
                            // `allow_high_precision_mv` differs — C's
                            // `GMCOST` line reads `hp=0` in both. So this is
                            // C's own ordering, not a value to derive.
                            /*allow_high_precision_mv=*/
                            false,
                            temporal_layer,
                            [
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list0_count_try)),
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                            ],
                            [
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list0_count)),
                                pic_decision
                                    .as_ref()
                                    .map_or(0, |p| u32::from(p.ref_list1_count)),
                            ],
                            Some(&mut sink),
                        )
                        .ok()
                    }
                    _ => None,
                }
            }
            _ => Some(crate::port_global_me::GmModels::default()),
        };
        // Printed BEFORE the refusal below, deliberately: the frame whose
        // derivation a join gate most needs to see is exactly the one the
        // refusal stops (`tools/gm_join_gate.sh`).
        if crate::dbgenv::gmdbg() {
            if let Some(g) = gm_estimation.as_ref() {
                eprintln!(
                    "GMPORT poc={display_order} b64={} total_me_sad={} avg_me_sad={} \
                     total_gm_sbs={} level={} ds={} searches={} all_identity={}",
                    frame_me.as_ref().map_or(0, |m| m.per_b64.len()),
                    frame_me.as_ref().map_or(0u32, |m| m
                        .per_b64
                        .iter()
                        .fold(0u32, |a, b| a.wrapping_add(b.rc_me_distortion))),
                    g.average_me_sad,
                    g.total_gm_sbs,
                    g.estimation_level,
                    g.downsample_level,
                    g.max_searches,
                    u8::from(g.all_identity()),
                );
                eprintln!(
                    "GMPORTMODELS poc={display_order} searched={} is_gm_on={}",
                    // Did C's per-reference search actually RUN, or did the
                    // frame-level derivation short-circuit it?
                    u8::from(!g.all_identity() && gm_models.is_some()),
                    gm_models.as_ref().map_or(2u8, |m| u8::from(m.is_gm_on)),
                );
                if let Some(m) = gm_models.as_ref() {
                    for (l, row) in m.models.iter().enumerate() {
                        for (r, wm) in row.iter().enumerate() {
                            if wm.wm_type != svtav1_types::motion::TransformationType::Identity {
                                eprintln!(
                                    "GMPORTREF poc={display_order} list={l} ref={r} wmtype={} \
                                     wmmat={:?} is_global={}",
                                    wm.wm_type as u8,
                                    wm.wmmat,
                                    u8::from(m.is_global_motion[l][r]),
                                );
                            }
                        }
                    }
                }
            }
        }

        if let Some(why) = Self::gm_search_config_error(gm_estimation.as_ref(), gm_models.as_ref())
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        let mut c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>> =
            // Task #95 chunk 2: was gated on 64-aligned dims; the padded
            // `sb_input` now lets the per-b64 walk read C's replicated border
            // on partial SBs, so the still/PD0 coding quantizer is built for any
            // 8-aligned key frame. pic_avg_variance averages over the ALIGNED
            // b64 grid (sb_cols x sb_rows), matching C. Full-SB is unchanged.
            if is_key {
                // Superres chunk B.3: C's picture analysis runs BEFORE the
                // superres downscale (pd_process.c:4344), so `pic_avg_variance`
                // is derived from the FULL-RESOLUTION picture. Walk that grid
                // when a superres source was stashed; otherwise this is the
                // unchanged coded-source walk.
                let pic_avg_variance = if let Some(vars) = stale_vars.as_ref() {
                    // Reuse the full-resolution, border-padded statistics.
                    // The tight pre-scaling source cannot serve a full b64
                    // read when either original dimension is partial.
                    (vars.iter().map(|v| u64::from(v.0[0])).sum::<u64>()
                        / vars.len() as u64) as u16
                } else {
                    let mut tot = 0u64;
                    let mut cnt = 0u64;
                    for sy in (0..h).step_by(64) {
                        for sx in (0..w).step_by(64) {
                            tot += u64::from(crate::pd0::compute_b64_variance(
                                sb_input, in_stride, sx, sy,
                            ).0[0]);
                            cnt += 1;
                        }
                    }
                    (tot / cnt) as u16
                };
                let coeff_lvl = crate::quant::derive_intra_coeff_level(
                    pic_avg_variance,
                    tpl_adjusted_qp as u32,
                    w,
                    h,
                );
                // C's per-arm preset clamp (enc_handle.c:4415-4436): allintra
                // above M9 -> M9, video (non-RTC) above M11 -> M11. The still
                // path's `preset.min(9)` is the allintra arm of the same rule
                // (`rate_arm::allintra_flattening_matches_the_ladder` pins it).
                let eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // Coded-lossless: `perform_rdoq = !svt_av1_is_lossless_segment
                // && ...` (full_loop.c:1756) — RDOQ never runs at qp 0.
                //
                // The VIDEO arm's ladder (`rdoq_level_default`, :8933) is a
                // flat 1 up to M10 and ignores `coeff_lvl` entirely — which is
                // why C can leave `pcs->coeff_lvl` at INVALID_LVL for a
                // video-mode I-slice. The allintra arm (:9904) is the
                // coeff-driven one, and is unchanged here.
                let rdoq_level = if coded_lossless {
                    0
                } else {
                    // ZenEnhancement::DeepSearch (allintra 4:2:0 only —
                    // `filter_chroma` keeps 4:4:4 and mono out of the arm):
                    // the -1 tier's `rdoq_level = 1` — full RDOQ at every
                    // preset, instead of the coeff-driven 0/2/3 ladder
                    // above M5.
                    crate::rate_arm::rdoq_level(
                        sc_arm,
                        if self
                            .enhancements
                            .contains(crate::enhancements::ZenEnhancement::DeepSearch)
                            && matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                            && filter_chroma
                        {
                            -1
                        } else {
                            eff_mode
                        },
                        coeff_lvl,
                    )
                };
                let lambda = crate::pd0::kf_full_lambda_8bit_tuned(
                    base_qindex,
                    picture_qp as u32,
                    self.hdr.is_fork() && self.hdr.alt_lambda_factors,
                    0,
                    // The frame `lambda_weight`, resolved exactly as C's
                    // allintra block does (enc_mode_config.c:10093-10115):
                    // the tune-IQ curve OR the PSNR ladder, then the
                    // extended-CRF bump. Both key on `picture_qp`.
                    Some(crate::pd0::frame_lambda_weight_for_preset(self.speed_config.preset,
                        picture_qp as u32,
                        self.hdr.tune == crate::tune::TUNE_IQ,
                        lw_bump,
                    )),
                );
                let mut cq = crate::quant::CodingQuantCfg::new(
                    rdoq_level,
                    lambda,
                    base_qindex,
                );
                cq.input_coeff_level = coeff_lvl;
                // C `svt_av1_optimize_b`'s `allintra || rtc` (full_loop.c:1046)
                // — the first index of `PLANE_RD_MULT`. `scs->allintra` is set
                // only for `intra_period_length == 0 || avif` (enc_handle.c:518),
                // which is exactly `ScArm::Allintra` here; `rtc` is never set by
                // this port. Video frames therefore weight CHROMA rate at 20,
                // not 13.
                cq.allintra_rd_mult = matches!(sc_arm, crate::sc_detect::ScArm::Allintra);
                Some(alloc::sync::Arc::new(cq))
            } else if let Some(me) = frame_me.as_ref() {
                // The INTER frame's coding quantizer (docs/INTER-ENCODE-PLAN.md
                // §1s item 1b). Without it `use_funnel` is false on every frame
                // with a reference and the C-exact MD path is unreachable no
                // matter what the two `ref_*.is_none()` gates say — which is
                // what item 1's measurement could not see.
                //
                // C `derive_inter_coeff_level` (md_config_process.c:650) keys
                // on `ppcs->norm_me_dist`, the MEAN of the open-loop ME's
                // per-b64 8x8 distortion (initial_rc_process.c:718-726) — so
                // the search has to have run, which is why this sits below it.
                let dist: u64 = me.per_b64.iter().map(|o| u64::from(o.me_8x8_distortion)).sum();
                let norm_me_dist = dist / me.per_b64.len().max(1) as u64;
                let coeff_lvl = crate::quant::derive_inter_coeff_level(
                    norm_me_dist,
                    tpl_adjusted_qp as u32,
                    w,
                    h,
                );
                if crate::dbgenv::coeffdbg() {
                    eprintln!(
                        "COEFFDBG nmd={} qp={} w={} h={} -> {:?}",
                        norm_me_dist, tpl_adjusted_qp, w, h, coeff_lvl
                    );
                }
                let eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // The VIDEO arm's RDOQ ladder (`rdoq_level_default`,
                // enc_mode_config.c:8933) is a flat 1 through M10 and ignores
                // `coeff_lvl` — which is why C can leave a video-mode I-slice
                // at INVALID_LVL. The level is still derived, because the
                // coeff-driven arms above M10 read it.
                let rdoq_level = crate::rate_arm::rdoq_level(sc_arm, eff_mode, coeff_lvl);
                // C `av1_lambda_assign_md` (md_process.c:725) for a non-key
                // frame. The rdmult BASE and the frame-type FACTOR read
                // DIFFERENT update types — `ppcs->update_type` and
                // `update_lambda`'s own `gf_update_type` — and on a flat
                // low-delay P GOP they disagree (LF vs ARF). MEASURED against
                // C's `svt_aom_full_cost_pd0` lambda: 241 378 on
                // `diag 64x64 q40 p8` frame 1, where one update type for both
                // gave 244 792.
                let lambda = crate::pd0::inter_full_lambda_8bit(
                    base_qindex,
                    md_lambda_base_update_type
                        .expect("an inter frame always has a picture decision"),
                    md_lambda_factor_update_type,
                    md_alt_lambda_factors,
                    0,
                    lambda_mod_intra,
                    crate::pd0::frame_lambda_weight_for_preset(self.speed_config.preset,
                        picture_qp as u32,
                        self.hdr.tune == crate::tune::TUNE_IQ,
                        lw_bump,
                    ),
                );
                let mut cq =
                    crate::quant::CodingQuantCfg::new(rdoq_level, lambda, base_qindex);
                // `pcs->coeff_lvl` — the depth-refinement ladder
                // (enc_mode_config.c:9370-9390) and the NSQ-search ladder read
                // it on inter frames.
                cq.input_coeff_level = coeff_lvl;
                // C `svt_av1_optimize_b`'s `allintra || rtc` (full_loop.c:1046):
                // an inter frame is never `allintra`, so chroma rate weighs 20.
                cq.allintra_rd_mult = false;
                Some(alloc::sync::Arc::new(cq))
            } else {
                None
            };

        // Step 4: Encode the frame superblock-by-superblock in raster order.
        // This ensures each SB can read above/left neighbors from previously
        // reconstructed SBs, matching the AV1 decode order.
        // (Spec 00: "The main encoding loop processes SBs in raster order")
        let mut recon = svtav1_types::try_vec![128u8; n]?;
        // AV1 spec: use_128x128_superblock=0 in SH → sb_size=64.
        // The decoder always uses 64x64 SBs when this flag is 0.
        // The encoder's max_partition_depth controls how deep the
        // partition search goes WITHIN each 64x64 SB, not the SB size.
        // SUPERBLOCK SIZE (task #91). Derived once in `EncodePipeline::new`
        // by replaying C's rule (Globals/enc_handle.c:4071-4111) — see the
        // `sb_size` field. 64 for every cell the gates currently cover; 128
        // only once the SB128 encode path is capability-enabled, at which
        // point the seq header's `use_128x128_superblock` and the tile
        // limits follow it (both parameterized below).
        let sb_size = self.sb_size;
        // Lambda stays CLI-qp-calibrated (see qp_to_lambda's domain note);
        // tpl_adjusted_qp is the CLI-domain value base_qindex is derived
        // from, so this is qp_to_lambda(qindex_to_qp(base_qindex)).
        let lambda = (crate::rate_control::qp_to_lambda(tpl_adjusted_qp)
            * self.speed_config.lambda_scale()) as u64;

        let sb_cols = w.div_ceil(sb_size);
        let sb_rows = h.div_ceil(sb_size);

        // C `svt_aom_get_ref_pic_buffer(pcs, LAST_FRAME)` resolves LAST
        // through `pcs->ppcs->ref_pic_ptr_array[REF_LIST_0][0]`, i.e. this
        // picture's own RPS — `pic.rps.ref_dpb_index[LAST]`.
        //
        // The three sites below read `self.dpb.get(0)` until 2026-09-03, a
        // hard-coded DPB SLOT. That is right for every frame this port's
        // gates cover and WRONG the moment a second inter frame exists:
        // frame 1 refreshes slot 1 (`refresh_frame_mask` 0x02), so at poc 2
        // C's LAST is slot 1 = poc 1 while `get(0)` is still the KEY frame.
        // MEASURED on `gradient 64x64 q32 p8 frames=3` before this: the
        // port's frame-2 MD searched `mv=(2,-36)` against poc 0, where the
        // true poc-1 displacement is `(0,-24)`, and coded 100 % intra at
        // 466 B against C's 21. That is the SEVENTH "a caller passes a
        // constant where the derivation is already ported" finding of this
        // campaign, and it is invisible at two frames because slot 0 IS
        // LAST there.
        let last_ref_slot: Option<usize> = if is_key {
            None
        } else {
            pic_decision
                .as_ref()
                .map(|pic| pic.rps.ref_dpb_index[crate::port_picstruct::LAST] as usize)
                .or(Some(0))
        };
        // Get reference frame for inter prediction (if available). This is
        // the DPB slot's own shared handle, NOT a copy: until 2026-09-04 the
        // two bindings below deep-cloned `y_plane` and `padded` on every
        // inter frame (4.19 + 7.15 MB at 2048x2048), which was the whole of
        // `4e29d8fa7`'s +8.83 MB peak-heap step on both ISAs
        // (benchmarks/mem_refclone_2026-09-04.meta). The slot is read-only
        // once stored and is not refreshed until the end of this frame, so
        // the shared allocation reads the bytes the clone read.
        let last_ref: Option<alloc::sync::Arc<crate::picture::ReferenceFrame>> =
            last_ref_slot.and_then(|slot| self.dpb.get_shared(slot));
        let ref_frame_data: Option<&[u8]> = last_ref.as_ref().map(|rf| rf.y_plane.as_slice());

        // The padded twin of the reference plane above (C
        // `pad_ref_and_set_flags`, enc_dec_process.c:1072) — what INTER
        // PREDICTION indexes, because a legal MV reads outside the frame.
        let ref_padded_luma: Option<&crate::picture::PaddedRef> =
            last_ref.as_ref().and_then(|rf| rf.padded.as_deref());

        // MV map for spatial MV prediction (8x8 block grid)
        let mv_map_stride = w.div_ceil(8);
        let mv_map_size = mv_map_stride * h.div_ceil(8);
        // Read by the legacy (non-funnel) partition search's MV predictor and
        // always ZERO: an old post-encode per-SB full-pel search filled it
        // after the only reader had run, so it was removed (2026-09-25).
        let mv_map = svtav1_types::try_vec![svtav1_types::motion::Mv::ZERO; mv_map_size]?;

        // Task #86: real tile ROWS for the allintra KEY path. Per AV1 spec
        // a tile is prediction-independent — above/left neighbor context
        // (and the entropy coder + FrameContext) resets at every tile
        // boundary — so per-tile-row MD search with its own local recon
        // (the `encode_tile_rows` closure below) and per-tile-row entropy
        // walks (see `run_entropy_walk` further down: it loops tile rows
        // internally, resetting writer/frame_ctx/coeff_fc/ectx per tile)
        // are exactly what a conforming decoder expects — NOT a
        // continuity break. `tile_rows_log2` is resolved (clamped) the
        // same way C's `svt_aom_set_tile_info` clamps a nonsense request
        // (entropy_coding.c:2450-2579): out-of-range requests degrade to
        // the largest the frame supports rather than panicking or
        // producing a bitstream inconsistent with what was encoded.
        //
        // Task #96: the grid is resolved through `TileGrid::resolve`, the
        // shared port of C's get_tile_limits + calculate_tile_cols +
        // calculate_tile_rows. The load-bearing part is that
        // `grid.tile_rows` is the ACTUAL tile count, which C's algorithm
        // makes SMALLER than `1 << TileRowsLog2` whenever the SB-row
        // count is not a multiple of it (6 SB rows at log2=2 -> height 2
        // -> 3 tiles, not 4). Deriving the count as `1 << log2` instead
        // both encoded a trailing EMPTY tile and wrote an out-of-range
        // `context_update_tile_id`, which conforming decoders REJECT
        // ("Invalid context_update_tile"). See TileGrid's doc comment.
        let tile_grid = crate::entropy::obu::TileGrid::resolve(
            self.width,
            self.height,
            // Task #91: the tile limits are SB-derived (spec 5.9.15) —
            // max_tile_width_sb HALVES and max_tile_area_sb QUARTERS at
            // SB128 (C svt_av1_get_tile_limits shifts by the PIXEL
            // sb_size_log2). Identical to the old 64 constant whenever
            // sb_size == 64, i.e. for every currently gated cell.
            self.sb_size as u32,
            self.tile_rows_log2,
            self.tile_cols_log2,
        );
        let tile_rows_log2 = tile_grid.tile_rows_log2;
        let tile_cols_log2 = tile_grid.tile_cols_log2;

        // [SVT_HDR_MODE] fork chroma-q: derive the FH per-plane deltas and
        // the plane qindexes the quantizer must use. Mainline: all zero.
        let chroma_deltas = if self.hdr.is_fork() {
            crate::chroma_q::fork_chroma_q_deltas_tuned(
                base_qindex,
                &self.color_description,
                self.hdr.tune,
            )
        } else {
            // MAINLINE chroma-q (rc_crf_cqp.c's `#else` arm): all-zero at
            // every tune but IQ, where C boosts chroma by
            // `CLIP3(0, 16, new_qindex/2 - 14)`. This used to be hardcoded
            // to zero, which made tune IQ 0/6 byte-identical to the C oracle
            // on `tools/issue9_knobs_gate.sh` — the ONLY divergence, with the
            // tile payload already matching byte-for-byte in size.
            crate::chroma_q::mainline_chroma_q_deltas(base_qindex, self.hdr.tune)
        };
        // `__expert` override REPLACES the derivation (mono frames were
        // refused above, so this only ever reaches frames with chroma).
        #[cfg(feature = "__expert")]
        let chroma_deltas = self
            .chroma_q_override
            .map_or(chroma_deltas, crate::chroma_q::ChromaQOverride::deltas);
        let qindex_u = (i32::from(base_qindex) + i32::from(chroma_deltas.u_ac)).clamp(0, 255) as u8;
        let qindex_v = (i32::from(base_qindex) + i32::from(chroma_deltas.v_ac)).clamp(0, 255) as u8;
        // Stills are I-slices at temporal layer 0: effective = ac_bias * 0.3.
        let ac_bias_eff = svtav1_dsp::ac_bias::effective_ac_bias(self.hdr.ac_bias, true, 0);
        // [SVT_HDR_MODE] per-SB delta-q signaling (variance boost). This
        // chunk arms the FULL SYNTAX chain with a UNIFORM plan (every SB at
        // base qindex -> all delta symbols are 0): decoder-valid, exercises
        // FH delta_q_params + the per-SB delta_q_cdf symbols end to end.
        // The variance plan (sb_qindex::variance_adjust_qp) swaps in when
        // per-SB quantization threading lands (docs/HDR-ON-4.2.md).
        // Diagnostic: SVTAV1_VB_DUMP=<path> writes the per-SB qindex plan to a
        // FILE (never stderr — the identity harness parses this process's
        // stderr as its symbol trace). Answers "did the boost fire, and by how
        // much" without perturbing a byte-comparison run.
        #[cfg(feature = "std")]
        if let Ok(path) = std::env::var("SVTAV1_VB_DUMP") {
            let txt = match delta_q_plan {
                Some(p) => std::format!(
                    "base={base_qindex} res={} plan={:?}\n",
                    p.delta_q_res,
                    p.sb_qindex
                ),
                None => std::format!("base={base_qindex} plan=NONE (variance boost off)\n"),
            };
            let _ = std::fs::write(path, txt);
        }
        let delta_q_res_signal = delta_q_plan.map(|p| p.delta_q_res);
        // sharp-tx RDOQ activates only with per-SB delta-q present (C gate
        // `(use_sharpness || sharp_tx) && delta_q_present && plane==0`).
        // [SVT_HDR_MODE] tune SSIM/IQ/MS_SSIM: per-16x16 SSIM rdmult
        // scaling factors (aom_av1_set_mb_ssim_rdmult_scaling; the
        // alt_ssim_tuning multi-scale perceptual variant when that knob is
        // on) + the PICTURE lambdas C's per-block `aom_av1_set_ssim_rdmult`
        // scales them into `full_lambda_md`/`fast_lambda_md`
        // (mode_decision.c:4060-4110). Those bases are `reset_enc_dec`'s
        // `svt_aom_lambda_assign` outputs (enc_dec_process.c:176-187) —
        // NO `lambda_weight` and NO per-SB stats/qindex modulation; the
        // scale replaces the `av1_lambda_assign_md` result outright.
        let ssim_rdmult: Option<crate::tune::SsimRdmult> =
            if crate::tune::tune_uses_ssim_rdmult(self.hdr.tune) {
                let (factors, num_cols, num_rows) = crate::tune::ssim_rdmult_factors(
                    &encode_input,
                    w,
                    w,
                    h,
                    self.hdr.alt_ssim_tuning,
                );
                let pic_lctx = crate::port_rc_process::LambdaContext {
                    frame_type: i32::from(!is_key),
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: md_lambda_base_update_type
                        .unwrap_or(crate::port_rc_process::FrameUpdateType::KfUpdate),
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    stats_based_sb_lambda_modulation:
                        crate::port_rc_process::stats_based_sb_lambda_modulation(
                            crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                            false,
                        ),
                    base_q_idx: i32::from(base_qindex),
                    // The picture lambda evaluates `update_lambda` at
                    // `q_index == base`, where the qdiff factor is the
                    // identity under every arm — these flags are still the
                    // frame's real ones, not literals.
                    delta_q_present,
                    r0_delta_qp_md,
                    // `scs->static_config.lambda_scale_factors` — the port
                    // does not expose the knob; 128 is the identity C ships.
                    lambda_scale_factors: [128; 7],
                };
                // `svt_aom_lambda_assign(pcs, &fast, &full, bd, base_q_idx,
                // multiply_lambda=true)` at both depths — `(fast, full)`.
                let (pic_fast8, pic_full8) =
                    crate::port_rc_process::lambda_assign(&pic_lctx, 8, base_qindex, true);
                let (_pic_fast10, pic_full10) =
                    crate::port_rc_process::lambda_assign(&pic_lctx, 10, base_qindex, true);
                Some(crate::tune::SsimRdmult {
                    factors,
                    num_cols,
                    num_rows,
                    pic_full8,
                    pic_full10,
                    pic_fast8,
                })
            } else {
                None
            };
        // Per-tune LF sharpness (deblocking_filter.c:1120): this is live
        // in mainline and the HDR fork. KEY VQ/FILM_GRAIN adds 2 (max 7);
        // IQ/MS_SSIM caps by qindex on every frame type.
        // Applied to the SEARCH input, the SIGNALED bits, and the walk's
        // application consistently (one effective value).
        let lf_sharp_eff: u8 = {
            let base = self.hdr.sharpness.clamp(0, 7) as u8;
            if !is_key
                && matches!(
                    self.hdr.tune,
                    crate::tune::TUNE_VQ | crate::tune::TUNE_FILM_GRAIN
                )
            {
                base
            } else {
                crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
            }
        };
        let sharp_tx_active =
            self.hdr.is_fork() && self.hdr.sharp_tx == 1 && delta_q_plan.is_some();
        // [SVT_HDR_MODE] frame QM levels (svt_av1_qm_init,
        // md_config_process.c:249): the linear qindex map (default tune =
        // PSNR in the fork); chroma levels derive from base + the FH
        // chroma AC deltas. [15;3] = QM off (identity).
        // A lossless segment uses identity matrices in the decoder even when
        // using_qmatrix is signaled. C applies nonidentity weights here at QP0
        // and produces wrong decoded samples (SUSPECTED-C-BUGS.md #31). Keep
        // the raw matrix helpers C-exact, but use the decoder's identity rule
        // for lossless MD, quantization, reconstruction and header signaling.
        let qm_levels: [u8; 3] = if self.hdr.enable_qm && !coded_lossless {
            // TUNE_IQ / TUNE_MS_SSIM use the still-image polynomial
            // (svt_av1_qm_init switch, md_config_process.c:255).
            let still = matches!(
                self.hdr.tune,
                crate::tune::TUNE_IQ | crate::tune::TUNE_MS_SSIM
            );
            let lvl = move |q: i32, lo: u8, hi: u8| {
                if still {
                    crate::qm::still_get_qmlevel(q, i32::from(lo), i32::from(hi)) as u8
                } else {
                    crate::qm::aom_get_qmlevel(q, i32::from(lo), i32::from(hi)) as u8
                }
            };
            [
                lvl(
                    i32::from(base_qindex),
                    self.hdr.min_qm_level,
                    self.hdr.max_qm_level,
                ),
                lvl(
                    i32::from(base_qindex) + i32::from(chroma_deltas.u_ac),
                    self.hdr.min_chroma_qm_level,
                    self.hdr.max_chroma_qm_level,
                ),
                lvl(
                    i32::from(base_qindex) + i32::from(chroma_deltas.v_ac),
                    self.hdr.min_chroma_qm_level,
                    self.hdr.max_chroma_qm_level,
                ),
            ]
        } else {
            [15; 3]
        };
        // [SVT_HDR_MODE] photon-noise film grain (--noise*): synthesize
        // the table per frame; seed 7391 + 3381*frame (C resource_
        // coordination assign_film_grain_random_seed; zero is bumped).
        let film_grain = if let Some(mut fg) = self.film_grain.table.clone() {
            fg.apply_grain = true;
            fg.random_seed = self.film_grain_seed();
            Some(fg)
        } else if self.hdr.is_fork() && self.hdr.noise_strength > 0 {
            let mut fg = crate::noise_gen::generate_noise_table(
                self.width,
                self.height,
                u32::from(self.hdr.noise_strength),
                self.hdr.noise_strength_chroma,
                self.hdr.noise_chroma_from_luma as i8,
                self.hdr.noise_size,
                self.color_description.full_range,
            );
            fg.random_seed = self.film_grain_seed();
            Some(fg)
        } else {
            self.prepared_grain.take()
        };
        // Stamp the fork RDOQ knobs onto the encode-pass quant config (C
        // reads them off static_config inside svt_av1_optimize_txb; the
        // sharp-tx gate `(use_sharpness||sharp_tx) && delta_q_present &&
        // plane==0` is unconditional for sharp_tx=1, full_loop.c:1070-1078).
        if self.hdr.is_fork()
            && let Some(cq) = c_quant.as_mut()
        {
            let cfg = alloc::sync::Arc::get_mut(cq)
                .expect("c_quant is unshared before tile encoding starts");
            cfg.hdr_fork = true;
            cfg.sharpness = self.hdr.sharpness;
            cfg.noise_norm_strength = self.hdr.noise_norm_strength;
            cfg.sharp_tx_active = sharp_tx_active;
            cfg.qm_levels = qm_levels;
        }
        // MOVED UP (inter campaign, docs/INTER-ENCODE-PLAN.md §1s item 1b):
        // these three derivations used to sit beside the header assembly.
        // MODE DECISION now needs them — the inter branch of MD prices with
        // the same tables the pack codes with, and reads the same MVP
        // environment — and MD runs before the header is written. Nothing
        // between their old and new positions produced any of their inputs
        // (checked field by field: every one is a `self` field or a local
        // from above this point), so the move is byte-neutral by
        // construction and pinned by the still gates.
        // Sequence-level tool bits (C svt_aom_sig_deriv_pre_analysis_scs):
        // per-preset for the still/allintra path, off for multi-frame.
        // Threaded to the SH + FH writers AND the entropy walk below —
        // the per-block use_filter_intra symbol exists exactly when the
        // SH signals the tool, so all three consumers MUST see one value.
        let is_single_frame = self.gop.intra_period == 1;
        let seq_tools = {
            let mut t = crate::speed_config::seq_tools_for_preset(
                self.speed_config.preset,
                is_single_frame,
                self.width as usize * self.height as usize,
            );
            // Task #91: C derives `use_128x128_superblock` at SH-write time
            // from `sb_size == BLOCK_128X128` (entropy_coding.c:2800). The
            // port's `sb_size` comes from the same rule
            // (sb128_geom::derive_super_block_size), so the bit follows it.
            t.use_128x128_superblock = self.sb_size == 128;
            // Superres chunk B.3: the SH tool bit must agree with what the
            // frame header signals (`SuperresParams::enabled_in_seq`) or the
            // decoder's bit walk desyncs. Off by default -> unchanged bit.
            t.enable_superres = self.superres_denom.is_some();
            // Issue #9 item 5: C writes `static_config.chroma_sample_position`
            // into the 4:2:0 color_config (entropy_coding.c:2743).
            t.chroma_sample_position = self.chroma_sample_position;
            // Inter campaign C1a: the non-reduced header's
            // `initial_display_delay` is `min(hierarchical_levels + 1, 10)`
            // (enc_handle.c:4975-4993). Unread on the still path, where those
            // bits are not written at all.
            t.hierarchical_levels = self.gop.hierarchical_levels;
            // ZenEnhancement::DeepSearch (allintra 4:2:0 — `validate`
            // refuses anything else): the funnel's filter-intra and
            // intra-edge-filter search config evaluates at enc_mode -1,
            // so the two symbol-gating sequence bits must take the -1
            // values or the decoder's bit walk desyncs
            // (`enable_filter_intra`) or its prediction semantics
            // disagree with what MD priced (`enable_intra_edge_filter`).
            // The other preset-derived bits — wn/sg/`enable_restoration`
            // — stay at the caller's preset: they are post-filters the
            // deep-search arm does not touch.
            if self
                .enhancements
                .contains(crate::enhancements::ZenEnhancement::DeepSearch)
                && self.gop.intra_period == 1
                && self.chroma_420
            {
                t.enable_filter_intra =
                    crate::intra_arm::filter_intra_level(crate::sc_detect::ScArm::Allintra, -1)
                        != 0;
                t.enable_intra_edge_filter =
                    crate::intra_arm::intra_edge_filter(crate::sc_detect::ScArm::Allintra, -1);
            }
            // [SVT_HDR_MODE] the fork ALWAYS signals separate_uv_delta_q
            // (its FH writes independent U/V deltas — entropy_coding.c
            // fork block hardcodes both flags true). An `__expert` chroma
            // override with distinct U/V deltas needs it too.
            if self.separate_uv_delta_q() {
                t.separate_uv_delta_q = true;
            }
            if self.hdr.is_fork() {
                // Photon noise signals grain tables per frame.
                t.film_grain_params_present = self.hdr.noise_strength > 0;
            }
            t.film_grain_params_present |= self.film_grain.enabled();
            // enable_intra_edge_filter's C-parity surface is still/420
            // (the C matched config). The mono extension keeps 0: C cannot
            // emit mono, and the mono leaf coder predicts without edge
            // filtering — signaling 0 keeps our recon decoder-exact on
            // that self-consistent surface.
            t.enable_intra_edge_filter &= self.chroma_420;
            t.enable_intra_edge_filter |= zen_intra_edge_filter;
            // Small-frame implementation limit (enc_settings.c:214-232):
            // when the TRUE source width OR height is < 64, C force-clears
            // enable_restoration_filtering (and aq_mode, already off on the
            // allintra path) BEFORE the SH derivation, so the SH bit is 0.
            // Uses the TRUE (unaligned) dims — a 60x60 frame aligns to
            // 64x64 but still trips this.
            if self.true_width < 64 || self.true_height < 64 {
                t.enable_restoration = false;
            }
            t
        };

        // The picture-level MD inputs, BOUND rather than passed inline because
        // TWO derivations read them: `md_config_inputs` ->
        // `sig_deriv_mode_decision_config_default` (the tool ladders) and
        // `enc_dec_cand_reduction` (C's `svt_aom_sig_deriv_enc_dec_default`
        // half, which the inter candidate injector reads). Binding them means
        // the two cannot drift apart on an input.
        //
        // Only computed for an inter frame — a key frame's header carries none
        // of these fields, so both derivations are byte-inert for every still
        // cell.
        let pipeline_md_inputs = if is_key {
            None
        } else {
            let pd = pic_decision.as_ref();
            Some(crate::inter_hdr_arm::PipelineMdInputs {
                // C `pcs->enc_mode` — post-clamp (enc_handle.c:4433): the
                // md-config ladders branch at M11 (pic_lpd1_lvl is the
                // observed one — raw p13 took the M12+ `is_base ? 3 : 7`
                // row where C's 11 takes `is_base ? 0 : 7`).
                enc_mode: crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                sq_qp: u32::from(self.rc_config.qp),
                base_q_idx: base_qindex,
                // C `ppcs->picture_qp` — the RC-derived value, NOT the CLI
                // qp: `rc_process.c:901` recomputes it as
                // `(base_q_idx + 2) >> 2` every frame, which is what the
                // pd0/lpd1/me-variance thresholds inside index.
                picture_qp: u32::from(picture_qp),
                temporal_layer_index: temporal_layer,
                hierarchical_levels: frame_hier,
                // C `ppcs->update_type` — the picture decision's
                // `set_frame_update_type` output, which `enc_dec_cand_reduction`
                // reads as `frame_is_leaf` (`enc_mode_config.c:4100`).
                update_type: pd.map(|p| p.update_type),
                is_ref: pd.is_some_and(|p| p.is_ref),
                is_islice: false,
                sc_class5: u8::from(sc_derivation.classes.sc_class5),
                input_resolution: crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                    self.width * self.height,
                ),
                encoder_bit_depth: self.bit_depth,
                super_block_size: self.sb_size as u16,
                enable_interintra_compound: seq_tools.enable_interintra_compound,
                frame_superres_enabled: self.superres_denom.is_some(),
                // C `ppcs->ref_list{0,1}_count_try` — the MRP-CAPPED counts
                // `update_count_try` (pd_process.c:4507) produces, NOT
                // `ref_list{0,1}_count`. This site read the uncapped pair
                // until 2026-09-03; the two agree on every cell this port has
                // encoded (both are `min(found, base_ref_listN_count)` on a
                // base-layer frame) but they diverge the moment `list0_only`
                // or a non-base layer applies, and `ref_list1_count_try` is
                // exactly the field C's own dump reports going 1 -> 0 at
                // frame 2 (`benchmarks/ref_coded_area_stats_2026-09-02.md`).
                ref_list0_count_try: pd.map_or(0, |p| u32::from(p.ref_list0_count_try)),
                ref_list1_count_try: pd.map_or(0, |p| u32::from(p.ref_list1_count_try)),
                // C `pcs->ref_pic_ptr_array[REF_LIST_{0,1}][0]->object_ptr`,
                // through the RPS's DPB indices — the same slots
                // `port_picstruct::bind_refs_and_primary_ref_frame` binds.
                // `None` when that list is empty, which is C's own guard in
                // every `get_ref_*_percentage` reader.
                ref_l0: pd.and_then(|p| {
                    (p.ref_list0_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize))
                        .flatten()
                }),
                ref_l1: pd.and_then(|p| {
                    (p.ref_list1_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                }),
                // C `pcs->coeff_lvl` — `derive_inter_coeff_level`'s output
                // (md_config_process.c:650), which runs BEFORE
                // `sig_deriv_mode_decision_config_default` reads it. The
                // coding quantizer above carries it; `Normal` on a `None`
                // matches C's `INVALID_LVL` under every equality check the
                // ladders make (neither `low_coeff` nor `high_coeff`).
                coeff_lvl: c_quant
                    .as_ref()
                    .map_or(crate::port_enc_mode_config::InputCoeffLvl::Normal, |q| {
                        crate::part_arm::input_coeff_lvl(q.input_coeff_level)
                    }),
            })
        };
        // C `svt_aom_sig_deriv_mode_decision_config_default` — the picture-level
        // tool ladders. It is EXPORTED and gated at tier 1; the frame header
        // reads its `allow_high_precision_mv`, `allow_warped_motion`,
        // `is_motion_mode_switchable`, `mfmv_level` and `interpolation_filter`
        // so the header can never disagree with the tools the encode ran.
        let md_config_signals = pipeline_md_inputs
            .clone()
            .and_then(crate::inter_hdr_arm::md_config_inputs)
            .and_then(
                crate::port_enc_mode_config::md_config::sig_deriv_mode_decision_config_default,
            );
        // C `set_global_motion_field` (md_config_process.c:37): the frame's
        // `global_motion[LAST..ALTREF]`, built from the search's own verdict.
        // It lives HERE, after the mode-decision signal derivation, because the
        // TRANSLATION arm reads `frm_hdr.allow_high_precision_mv` — which C
        // assigns in `svt_aom_sig_deriv_mode_decision_config`, i.e. AFTER the
        // ME-time search that used a hardcoded 0 (see the search call above).
        //
        // IDENTITY throughout on a key frame and wherever the search declined,
        // which is exactly what the header wrote unconditionally before.
        let gm_field: [svtav1_types::motion::WarpedMotionParams; 8] =
            match (gm_models.as_ref(), md_config_signals.as_ref()) {
                (Some(m), Some(sigs)) => crate::port_global_me::set_global_motion_field(
                    m,
                    sigs.allow_high_precision_mv != 0,
                ),
                _ => [svtav1_types::motion::WarpedMotionParams::default(); 8],
            };
        // C `pcs->ppcs->gm_ctrls` — `svt_aom_set_gm_controls` at this frame's
        // `gm_level`. `skip_identity` (level 4 only) makes the injector skip
        // IDENTITY references; `enabled` is what C assigns to
        // `ctx->global_mv_injection`, gating `inject_global_candidates`
        // outright at every preset where the level is 0.
        let gm_ctrls = crate::port_enc_mode_config::ctrls::set_gm_controls(
            self.gm_level_for_frame(is_key),
            crate::port_enc_mode_config::ResolutionRange::from_luma_area(self.width * self.height),
        );
        let gm_skip_identity = gm_ctrls.is_some_and(|c| c.skip_identity != 0);
        let gm_enabled = gm_ctrls.is_some_and(|c| c.enabled != 0);
        // C `pcs->child_pcs->ref_global_motion[]` (pic_manager_process.c:831):
        // the PRIMARY-REF picture's own saved models, which every parameter is
        // delta-coded against. An I_SLICE reference contributes IDENTITY, and
        // so does `PRIMARY_REF_NONE` — the same slot the CDF continuation
        // resolves, so the two cannot disagree about which picture this frame
        // is coded against.
        let ref_gm_field: [svtav1_types::motion::WarpedMotionParams; 8] =
            if primary_ref_frame_for_cdf == crate::port_picstruct::PRIMARY_REF_NONE {
                [svtav1_types::motion::WarpedMotionParams::default(); 8]
            } else {
                pic_decision
                    .as_ref()
                    .map(|p| p.rps.ref_dpb_index[primary_ref_frame_for_cdf as usize] as usize)
                    .and_then(|slot| self.dpb.get(slot))
                    .map_or(
                        [svtav1_types::motion::WarpedMotionParams::default(); 8],
                        |rf| rf.global_motion,
                    )
            };

        // C `svt_aom_sig_deriv_enc_dec_default` (`enc_mode_config.c:7826`) —
        // the per-superblock EncDec derivation, of which the injector reads
        // `cand_reduction_ctrls`. It is derived here beside the picture-level
        // ladders because every input it takes is picture-level: C calls it
        // per superblock but passes `pcs->cand_reduction_level` and, on this
        // arm, constants for everything else (see `enc_dec_cand_reduction`).
        //
        // A `None` here is REFUSED rather than folded into the inter path's
        // `None`: `inter_md_frame` being absent on an inter frame does not
        // refuse anything, it codes the frame all-intra — a plausible-but-wrong
        // stream, which is the one outcome `docs/WORKING-ON-THIS.md` §6 rules
        // out. `set_cand_reduction_ctrls` answers `None` only for a level
        // outside C's switch, which this arm cannot produce (it assigns 0, 1,
        // 2 or 6), so this is a guard on a claim about the ladder rather than
        // a reachable path.
        let inter_cand_reduction = match (pipeline_md_inputs.as_ref(), md_config_signals.as_ref()) {
            (Some(mi), Some(sigs)) => Some(
                crate::inter_hdr_arm::enc_dec_cand_reduction(mi, sigs.cand_reduction_level)
                    .ok_or_else(|| {
                        whereat::at!(EncodeError::UnsupportedConfig(
                            "cand_reduction_level is outside C's set_cand_reduction_ctrls \
                             switch (crate::inter_hdr_arm::enc_dec_cand_reduction)",
                        ))
                    })?,
            ),
            _ => None,
        };

        // The frame-level INTER syntax the pack's inter mode-info writer
        // reads (`docs/INTER-ENCODE-PLAN.md` §1s item 7). It is derived HERE,
        // beside `primary_ref_frame_for_cdf`, rather than at the header
        // assembly below, because the TILE is coded before the header is
        // written and the writer needs these values while it codes. The
        // header re-derives the same fields from the same inputs and the two
        // are asserted equal there, exactly like `primary_ref_frame`.
        //
        // `md_config_signals` moved up with it for the same reason; nothing
        // between its old and new position reads it.
        let inter_syntax_state: Option<InterSyntaxState> = md_config_signals.map(|sigs| {
            let mut ref_order_hint = [0i32; 7];
            if let Some(pic) = pic_decision.as_ref() {
                for (i, oh) in ref_order_hint.iter_mut().enumerate() {
                    let slot = pic.rps.ref_dpb_index[i] as usize;
                    *oh = self.dpb.get(slot).map_or(0, |r| r.order_hint as i32);
                }
            }
            InterSyntaxState {
                // C `pd_process.c:4958`, and NOT the constant this port used
                // to imply: `skip_mode_flag` IS `skip_mode_allowed`.
                skip_mode_flag: pic_decision
                    .as_ref()
                    .is_some_and(|p| p.skip_mode.skip_mode_allowed != 0),
                // C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` —
                // `setup_skip_mode_allowed` leaves them `INVALID_IDX` (-1)
                // when the frame's reference structure has no skip-mode
                // pair (single reference, or order-hint signalling off).
                skip_mode_ref_frame_idx_0: pic_decision
                    .as_ref()
                    .map_or(-1, |p| p.skip_mode.ref_frame_idx_0 as i8),
                skip_mode_ref_frame_idx_1: pic_decision
                    .as_ref()
                    .map_or(-1, |p| p.skip_mode.ref_frame_idx_1 as i8),
                // C `frm_hdr->reference_mode`. The port has no compound
                // candidate yet, but the SYMBOL layout depends on this bit
                // and the header writes it, so it must be the header's value
                // and not a convenient constant.
                // C `frm_hdr->reference_mode`, i.e. the header's
                // `reference_select` bit — `inter_hdr_arm::inter_signal`
                // derives it from `pic.reference_mode` and this reads the
                // same field, so the tile and the header cannot disagree.
                reference_mode: match pic_decision.as_ref().map(|p| p.reference_mode) {
                    Some(crate::port_picstruct::ReferenceMode::Select) => {
                        crate::port_entropy_inter::refframe::ReferenceMode::Select
                    }
                    _ => crate::port_entropy_inter::refframe::ReferenceMode::Single,
                },
                interpolation_filter: sigs.interpolation_filter,
                enable_dual_filter: seq_tools.enable_dual_filter,
                enable_interintra_compound: seq_tools.enable_interintra_compound,
                enable_masked_compound: seq_tools.enable_masked_compound,
                enable_jnt_comp: seq_tools.enable_jnt_comp,
                enable_order_hint: seq_tools.enable_order_hint,
                order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
                is_motion_mode_switchable: sigs.is_motion_mode_switchable,
                allow_warped_motion: sigs.allow_warped_motion,
                allow_high_precision_mv: sigs.allow_high_precision_mv != 0,
                // C keeps `frm_hdr->force_integer_mv = 0` unconditionally
                // (resource_coordination_process.c:362), which is also the
                // bit `write_uncompressed_header` emits (obu.rs:1421). Read
                // from the same place rather than from a signal that has no
                // such field.
                force_integer_mv: false,
                // C `pcs->ppcs->global_motion[ref].wmtype`, read by the
                // entropy walk to decide whether a block's mode is a GLOBALMV
                // that codes no MV. It is the SAME array the frame header
                // wrote (`gm_field`, above), converted through the one
                // `WarpParams` conversion, so the header and the per-block
                // walk cannot disagree about a reference's model.
                gm_wmtype: core::array::from_fn(|i| {
                    crate::port_entropy_inter::gm::WarpParams::from(gm_field[i]).wmtype
                }),
                cur_order_hint: display_order as i32,
                ref_order_hint,
                // C `mfmv_controls` (enc_mode_config.c:8853) for the VALUE
                // and `frame_might_allow_ref_frame_mvs`
                // (entropy_coding.h:71) for its PRESENCE — the same two
                // rules `inter_hdr_arm::inter_signal` applies to write the
                // header bit, asserted equal to it below.
                //
                // This used to spell the VALUE as `mfmv_level == 1`, a THIRD
                // transcription of a function ported once in
                // `port_enc_mode_config::tail` and re-derived in
                // `inter_hdr_arm`. It agreed with C only because every level
                // above 1 was refused before it could be reached; the moment
                // level 2 was allowed it would have been a silent
                // disagreement in a bit that moves a `newmv` CDF row. It now
                // calls the same shared port the header does.
                use_ref_frame_mvs: !crate::dbgenv::mfmv_off()
                    && seq_tools.enable_ref_frame_mvs
                    && seq_tools.enable_order_hint
                    && crate::port_enc_mode_config::tail::mfmv_controls(
                        crate::port_enc_mode_config::tail::MfmvInputs {
                            mfmv_level: sigs.mfmv_level,
                            is_base: pic_decision
                                .as_ref()
                                .is_some_and(|p| p.temporal_layer_index == 0),
                            tpl: self.scs_tpl(),
                            r0_gen: false,
                            r0: 0.0,
                            is_b_slice: pic_decision.as_ref().is_some_and(|p| {
                                p.slice_type == crate::port_picstruct::SliceType::B
                            }),
                            ref_list1_count_try: pic_decision
                                .as_ref()
                                .map_or(0, |p| u32::from(p.ref_list1_count_try)),
                            ref_l0_is_mfmv_used: false,
                            ref_l1_is_mfmv_used: false,
                        },
                    )
                    .is_some_and(|v| v != 0),
            }
        });

        // The frame-constant MVP environment the pack derives `predmv` /
        // `inter_mode_ctx` / `drl_ctx` from (§1s items 2 and 3). Same
        // provenance rule as `inter_syntax_state` above: every field is the
        // one the HEADER announces, so the contexts the tile codes are the
        // ones a decoder rebuilds.
        //
        // C `av1_setup_motion_field` (md_config_process.c:523, called from
        // `:933`) builds BOTH the temporal field and `pcs->ref_frame_side`
        // from the DPB, once per picture. Its two products go to different
        // consumers — `tpl_mvs` to the MVP scan below, `ref_frame_side` to the
        // walk's `av1_copy_frame_mvs` — so it is computed once here and both
        // are carried, rather than derived twice.
        let mut inter_ref_frame_side = [0i8; 8];
        let mut inter_mvp_env: Option<crate::partition::InterMdEnv> =
            inter_syntax_state.as_ref().map(|st| {
                let (mi_cols, mi_rows) = (w.div_ceil(4) as i32, h.div_ceil(4) as i32);
                let tpl_stride = (mi_cols + 1) >> 1;
                crate::partition::InterMdEnv {
                    mi_stride: mi_cols,
                    mi_rows,
                    mi_cols,
                    tile: crate::intrabc::TileMiBounds {
                        mi_col_start: 0,
                        mi_col_end: mi_cols,
                        mi_row_start: 0,
                        mi_row_end: mi_rows,
                    },
                    sb_mi_size: (sb_size / 4) as i32,
                    global_motion: gm_field,
                    allow_high_precision_mv: st.allow_high_precision_mv,
                    force_integer_mv: st.force_integer_mv,
                    use_ref_frame_mvs: st.use_ref_frame_mvs,
                    order_hint_info: crate::inter_mvp::OrderHintInfo {
                        enable_order_hint: st.enable_order_hint,
                        order_hint_bits: st.order_hint_bits,
                    },
                    cur_order_hint: st.cur_order_hint,
                    // `inter_mvp` indexes by `MvReferenceFrame`
                    // (LAST = 1 ..= ALTREF = 7, slot 0 unused); the entropy
                    // side's array is `ref_frame - 1`.
                    ref_order_hint: {
                        let mut a = [0i32; 8];
                        a[1..8].copy_from_slice(&st.ref_order_hint);
                        a
                    },
                    // C `pcs->av1_cm->ref_frame_sign_bias[8]`
                    // (`svt_av1_setup_frame_sign_bias`,
                    // pd_process.c:4894-4909) — already derived into
                    // `pic_decision` by `port_picstruct::set_ref_frame_sign_bias`
                    // on every frame; zeroed on a key or when order hints
                    // are off.
                    ref_frame_sign_bias: pic_decision
                        .as_ref()
                        .map_or([0; 8], |p| p.ref_frame_sign_bias),
                    // Stamped below with `pic_decision` — the picture-level
                    // `temporal_layer_index > 0 && RANDOM_ACCESS` half of
                    // C's `symteric_refs` gate; the list half is evaluated
                    // per call inside `generate_av1_mvp_table`.
                    symmetric_refs_eligible: false,
                    // C `av1_setup_motion_field`, run over this picture's own
                    // DPB references. On a two-frame cell every projection
                    // returns 0 — LAST is the KEY frame and C aborts on
                    // `start_frame_buf->frame_type == KEY_FRAME`
                    // (md_config_process.c:441) — so every cell stays
                    // `INVALID_MV` and `add_tpl_ref_mv` returns 0, which is
                    // what sets the GLOBALMV bit of `mode_context` (§1t).
                    // From the SECOND inter frame on, LAST has a saved motion
                    // field and this is where it enters the ref-MV stack.
                    tpl_mvs: {
                        let mut tpl = alloc::vec![
                            crate::inter_mvp::TplMvRef::default();
                            (((mi_rows + 32) >> 1) * tpl_stride) as usize
                        ];
                        let refs = crate::inter_mvp::MotionFieldRefs {
                            refs: core::array::from_fn(|idx| {
                                let pic = pic_decision.as_ref()?;
                                let slot = pic.rps.ref_dpb_index[idx] as usize;
                                let rf = self.dpb.get(slot)?;
                                // C's field always exists at the reference's
                                // own half-mi extent. A DPB entry that carries
                                // none is one this port wrote before the
                                // writeback existed, or an allintra picture
                                // that can never be a reference in a GOP; C's
                                // own `mi_rows != cm->mi_rows` abort covers a
                                // different-SIZE reference, but nothing in
                                // that function can see a SHORT slice, so the
                                // length is checked here rather than indexed
                                // on faith.
                                let rf_mi_rows = (rf.height as i32 + 3) >> 2;
                                let rf_mi_cols = (rf.width as i32 + 3) >> 2;
                                if rf.mvs.len()
                                    != (((rf_mi_rows + 1) >> 1) * ((rf_mi_cols + 1) >> 1)) as usize
                                {
                                    return None;
                                }
                                Some(crate::inter_mvp::RefMotionField {
                                    mvs: &rf.mvs,
                                    order_hint: rf.order_hint as i32,
                                    ref_order_hint: rf.ref_order_hint,
                                    is_intra_only: rf.is_islice,
                                    mi_rows: rf_mi_rows,
                                    mi_cols: rf_mi_cols,
                                })
                            }),
                        };
                        inter_ref_frame_side = crate::inter_mvp::setup_motion_field(
                            &mut tpl,
                            tpl_stride,
                            mi_rows,
                            mi_cols,
                            st.cur_order_hint,
                            crate::inter_mvp::OrderHintInfo {
                                enable_order_hint: st.enable_order_hint,
                                order_hint_bits: st.order_hint_bits,
                            },
                            st.use_ref_frame_mvs,
                            &refs,
                        );
                        #[cfg(feature = "std")]
                        if let Some(path) = std::env::var_os("SVTAV1_TPL_OUT") {
                            // Diagnostic twin of the vendored-libaom `TPL`
                            // dump (AOM_TPL_OUT): the projected temporal-MV
                            // field this frame's ref-MV scan consumes,
                            // emitted in libaom's `as_int` packing
                            // (row | col<<16) so the two can be diffed.
                            use std::io::Write as _;
                            if let Ok(file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&path)
                            {
                                let mut w = std::io::BufWriter::new(file);
                                let mut line = alloc::string::String::with_capacity(
                                    32 + tpl.len() * 12,
                                );
                                core::fmt::Write::write_fmt(
                                    &mut line,
                                    format_args!("TPL {} {}", st.cur_order_hint, tpl.len()),
                                )
                                .ok();
                                for t in &tpl {
                                    let c_int = (t.mfmv0.y as u16 as u32)
                                        | ((t.mfmv0.x as u16 as u32) << 16);
                                    core::fmt::Write::write_fmt(
                                        &mut line,
                                        format_args!(
                                            " {c_int},{}",
                                            t.ref_frame_offset as i8
                                        ),
                                    )
                                    .ok();
                                }
                                line.push('\n');
                                let _ = w.write_all(line.as_bytes());
                            }
                        }
                        #[cfg(feature = "std")]
                        if crate::dbgenv::mfmv_dbg() {
                            let mut valid = 0usize;
                            let mut hash: u64 = 1469598103934665603;
                            for t in &tpl {
                                if t.ref_frame_offset != 0 || t.mfmv0.as_int() != crate::intrabc_mvp::INVALID_MV {
                                    valid += 1;
                                    hash = (hash ^ (t.mfmv0.as_int() as u32 as u64))
                                        .wrapping_mul(1099511628211);
                                    hash = (hash ^ u64::from(t.ref_frame_offset))
                                        .wrapping_mul(1099511628211);
                                }
                            }
                            std::eprintln!(
                                "RS_MFMV poc={display_order} cur={} use={} mi={mi_rows}x{mi_cols} \
                                 stride={} roh={},{},{},{},{},{},{} side={},{},{},{},{},{},{}",
                                st.cur_order_hint,
                                u8::from(st.use_ref_frame_mvs),
                                tpl_stride * 2,
                                st.ref_order_hint[0], st.ref_order_hint[1], st.ref_order_hint[2],
                                st.ref_order_hint[3], st.ref_order_hint[4], st.ref_order_hint[5],
                                st.ref_order_hint[6],
                                inter_ref_frame_side[1], inter_ref_frame_side[2],
                                inter_ref_frame_side[3], inter_ref_frame_side[4],
                                inter_ref_frame_side[5], inter_ref_frame_side[6],
                                inter_ref_frame_side[7],
                            );
                            std::eprintln!(
                                "RS_MFMVSUM poc={display_order} size={} valid={valid} hash={hash:x}",
                                tpl.len(),
                            );
                        }
                        #[cfg(feature = "std")]
                        if crate::dbgenv::zz_tpl() {
                            let valid = tpl
                                .iter()
                                .filter(|t| t.ref_frame_offset != 0)
                                .count();
                            std::eprintln!(
                                "ZZTPL poc={display_order} cells={} valid={} use_mvs={} refs_present={}",
                                tpl.len(),
                                valid,
                                st.use_ref_frame_mvs,
                                refs.refs.iter().filter(|r| r.is_some()).count(),
                            );
                        }
                        tpl
                    },
                    tpl_stride,
                    // C `ctx->sb64_sq_no4xn_geom` selects the SIMPLIFIED MFMV
                    // block walk, and it means all three of its parts: a 64x64
                    // superblock, SQUARE-only shapes, and no 4xN. This was set
                    // from `sb_size == 64` ALONE, which is true at every preset
                    // this port ships, so the simplified walk ran even where
                    // rectangular blocks exist.
                    //
                    // The simplified walk uses `n4_w` for BOTH the row and the
                    // column extent (`inter_mvp.rs`, the `sb64_sq_no4xn_geom`
                    // arm). On a square block that is the same number; on a
                    // 16x32 it scans four rows instead of eight and never sees
                    // the lower half's temporal candidates. NEARESTMV/NEARMV
                    // derive their MV from that stack, so the encoder and a
                    // decoder pick DIFFERENT motion vectors for the same block.
                    //
                    // MEASURED: with this corrected, vidyo1/vidyo3/vidyo4 all go
                    // from drifting (or failing to decode) to 8 of 8 frames
                    // byte-identical to aomdec.
                    sb_size_64: sb_size == 64,
                }
            });

        // C `ctx->ref_frame_type_arr` (`set_all_ref_frame_type`,
        // pd_process.c:1044) — single-reference entries AND the compound
        // pairs (bi-dir L0xL1 plus the unidir LAST_LAST2 set a B slice
        // earns), each naming the DPB pictures it predicts from. The
        // constituent-aware availability filter runs below, once
        // `inter_padded_by_ref` exists; a compound entry survives exactly
        // when BOTH its single references do.
        let inter_ref_types: alloc::vec::Vec<i8> = match pic_decision.as_ref() {
            Some(pic) if !is_key => {
                let (arr, tot) = crate::port_picstruct::set_all_ref_frame_type(pic);
                arr[..usize::from(tot)].to_vec()
            }
            _ => alloc::vec::Vec::new(),
        };
        // The padded DPB picture per `MvReferenceFrame`. C's
        // `svt_aom_get_ref_pic_buffer(pcs, rf)` resolves EACH reference through
        // its own `ref_dpb_index` entry — LAST through `[0]`, LAST2 through
        // `[1]`, BWDREF through `[4]`. On a flat low-delay-P GOP those slots
        // hold DIFFERENT pictures from frame 2 on (LAST is frame N-1, LAST2
        // frame N-2), and binding every reference to LAST's recon — which is
        // what this did — both starved LAST2 out of `inter_ref_types` and
        // would have predicted it from the wrong pixels had it survived.
        //
        // The `Arc` clones exist for borrow shape only: the `&PaddedRef`
        // table cannot borrow `self.dpb` directly because the encode below
        // mutates other `self` fields while the references are live. Eight
        // refcount bumps per inter frame is the whole cost; the buffers are
        // shared.
        let mut inter_ref_frames: [Option<alloc::sync::Arc<crate::picture::ReferenceFrame>>; 8] =
            Default::default();
        if let Some(pic) = pic_decision.as_ref()
            && !is_key
        {
            for rt in 1i8..=7 {
                let slot = pic.rps.ref_dpb_index[usize::from(rt as u8 - 1)] as usize;
                inter_ref_frames[rt as usize] = self.dpb.get_shared(slot);
            }
        }
        let mut inter_padded_by_ref: [Option<&crate::picture::PaddedRef>; 8] = [None; 8];
        for (rt, rf) in inter_ref_frames.iter().enumerate() {
            inter_padded_by_ref[rt] = rf.as_deref().and_then(|r| r.padded.as_deref());
        }
        // A single entry survives when ITS reference has a padded
        // reconstruction; a compound pair survives when BOTH constituents
        // do. `padded_by_ref` is indexed by `MvReferenceFrame` (1..=7) — a
        // pair type (8..) has no slot of its own and must never be indexed
        // into it.
        let inter_ref_types: alloc::vec::Vec<i8> = inter_ref_types
            .into_iter()
            .filter(|&rt| {
                let rf = crate::inter_mvp::av1_set_ref_frame(rt);
                inter_padded_by_ref[rf[0].max(0) as usize].is_some()
                    && (rf[1] == crate::inter_mvp::NONE_FRAME
                        || inter_padded_by_ref[rf[1].max(0) as usize].is_some())
            })
            .collect();
        // C's `symteric_refs` preconditions (adaptive_mv_pred.c:1339-1341):
        // a RA B picture above temporal layer 0. The LIST half of C's gate
        // ({LAST, BWDREF, LAST_BWD}) is evaluated per call inside
        // `generate_av1_mvp_table` on the block's own ref array —
        // `determine_best_references` may rebuild a block's
        // `ref_frame_type_arr` in an order the gate rejects even when the
        // picture-level list passes, so stamping the whole gate here is
        // wrong. `inter_mvp_fields` reads the same eligibility so the
        // coded stack matches MD's.
        if let (Some(env), Some(pic)) = (inter_mvp_env.as_mut(), pic_decision.as_ref()) {
            env.symmetric_refs_eligible = pic.temporal_layer_index > 0
                && self.pred_structure == crate::port_picstruct::PredStructure::RandomAccess;
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::rpsdbg()
            && let Some(pic) = pic_decision.as_ref()
            && !is_key
        {
            let mut s = alloc::string::String::new();
            for i in 0..7usize {
                let slot = pic.rps.ref_dpb_index[i] as usize;
                let poc = self.dpb.get(slot).map_or(-1, |r| r.display_order as i64);
                s.push_str(&alloc::format!("{i}:{slot}->poc{poc} "));
            }
            std::eprintln!(
                "RPSDBG poc={} tl={} is_ref={} totrf={} rt={:?} refresh={:#x} dpb=[{}]",
                pic.picture_number,
                pic.temporal_layer_index,
                pic.is_ref,
                inter_ref_types.len(),
                inter_ref_types,
                pic.rps.refresh_frame_mask,
                s
            );
        }

        let inter_md_frame = match (
            frame_me.as_ref(),
            ref_padded_luma,
            inter_syntax_state.as_ref(),
            inter_mvp_env.as_ref(),
            md_config_signals.as_ref(),
            inter_cand_reduction.as_ref(),
        ) {
            (Some(me), Some(padded), Some(st), Some(env), Some(sigs), Some(cand_red)) => {
                // §1s item 8, the inter half: the same `md_frame_context`
                // the intra rate tables are built from.
                let default_fc = crate::entropy::context::FrameContext::new_default();
                let default_ic = crate::port_entropy_inter::InterCdfs::new_default();
                let (fc, ic) = match primary_ref_cdfs.as_deref() {
                    Some(prev) => (&prev.fc, &prev.fc.inter),
                    None => (&default_fc, &default_ic),
                };
                let (fac, ref_fac) = crate::inter_md_arm::build_inter_rates(fc, ic);
                // C `svt_aom_estimate_mv_rate` (md_rate_estimation.c:458-465):
                // under `pcs->approx_inter_rate` the nmv cost tables are
                // memset to zero and `nmvcoststack` repointed at them, so
                // every MV prices at 0 in the motion searches.
                let nmv = if sigs.approx_inter_rate != 0 {
                    crate::intrabc::MvCostTables::zeroed()
                } else {
                    crate::inter_md_arm::nmv_cost_table(
                        &fc.nmvc,
                        crate::inter_mv_code::mv_precision(
                            st.allow_high_precision_mv,
                            st.force_integer_mv,
                        ),
                    )
                };
                let search = crate::inter_search_arm::frame_cfg(
                    &crate::inter_search_arm::SearchFrameInputs {
                        md_pme_level: sigs.md_pme_level,
                        me_subpel_level: sigs.me_subpel_level,
                        pme_subpel_level: sigs.pme_subpel_level,
                        md_nsq_mv_search_level: sigs.md_nsq_mv_search_level,
                        interpolation_search_level: sigs.interpolation_search_level,
                        dist_based_ref_pruning: sigs.dist_based_ref_pruning,
                        cli_qp: u32::from(self.rc_config.qp),
                        // `ppcs->picture_qp` — `perform_md_reference_pruning`'s
                        // check-closest threshold; inert while
                        // `check_closest_multiplier` is 0 (level <= 3).
                        picture_qp,
                        // `set_qp_based_th_scaling_ctrls_default`
                        // (enc_handle.c:3812) — 1 at every preset above
                        // `ENC_MR`, which is every preset this port reaches
                        // on the video arm.
                        pme_qp_based_th_scaling: self.speed_config.preset > 0,
                        base_q_idx: base_qindex,
                        allow_high_precision_mv: st.allow_high_precision_mv,
                        approx_inter_rate: sigs.approx_inter_rate,
                        pic_width: w as u32,
                        pic_height: h as u32,
                    },
                )
                .ok_or_else(|| {
                    whereat::at!(EncodeError::UnsupportedConfig(
                        "a picture-level MD search level is outside the range its C control \
                         table accepts (crate::inter_search_arm::frame_cfg)",
                    ))
                })?;
                // `sharpness_ctrls.ifs` (enc_handle.c:3279-3285) arms the
                // IFS smooth bias together with `pcs->ppcs->is_noise_level`
                // (enc_inter_prediction.c:2166). `is_noise_level` IS derived
                // (port_picstruct::tf_window_noise + the `:4240` stamps): C's
                // `last_i_noise_levels_log1p_fp16` updates only inside
                // `derive_tf_window_params`, which runs on no LD picture —
                // the flag is 0 for the whole low-delay envelope on both
                // sides — and is real under RA+TF. Every subjective-tune arm
                // (`ifs`, `unipred_bias`, `cdef`, `restoration`) ANDs with
                // it, so a 0 makes all of them inert — except
                // `sharpness_ctrls.rdoq`, which is NOT noise-gated:
                // `(use_sharpness || sharp_tx) && delta_q_present`
                // (full_loop.c:1070). The port's `optimize_b` has no
                // `use_sharpness` term, so a sharpness tune meeting a live
                // delta-q plan diverges; and under is_noise_level = 1 the
                // unipred/cdef/restoration arms are unwired. Refuse the
                // union rather than guess either — the admitted surface is
                // exactly where every arm is provably inert.
                let sharp_tune =
                    crate::tune::sharpness_ifs(self.hdr.tune, self.hdr.alt_ssim_tuning);
                let is_noise = pic_decision.as_ref().is_some_and(|p| p.is_noise_level);
                if sharp_tune && (is_noise || delta_q_plan.is_some()) {
                    return Err(whereat::at!(EncodeError::UnsupportedConfig(
                        "subjective-tune sharpness arms (tune vq / film-grain, or alt-ssim \
                         tuning) on an inter frame are supported only where they are inert: \
                         is_noise_level == 0 (always true on low delay) and no delta-q plan \
                         — this frame sets one, where C's unipred_bias/cdef/restoration or \
                         `use_sharpness` rdoq arms fire and this port does not model them",
                    )));
                }
                Some(crate::inter_md_arm::InterMdFrame {
                    skip_mode_flag: st.skip_mode_flag,
                    skip_mode_ref_frame_idx_0: st.skip_mode_ref_frame_idx_0,
                    skip_mode_ref_frame_idx_1: st.skip_mode_ref_frame_idx_1,
                    cand_reduction: *cand_red,
                    wm_level: sigs.wm_level,
                    bit_depth: self.bit_depth,
                    padded,
                    padded_by_ref: inter_padded_by_ref,
                    // The SB-EXTENT-padded source, NOT `encode_input` at
                    // stride `w`. C's MD searches read
                    // `input_pic->y_buffer + blk_org_y * y_stride + blk_org_x`
                    // over the BLOCK's full extent, and on a frame whose dims
                    // are not a multiple of 64 a straddling block runs past
                    // the aligned edge into C's replicated border
                    // (`pad_input_picture` + `svt_aom_generate_padding`).
                    // `sb_input` is that buffer and the port already reads PD0's
                    // b64 variance and every straddling leaf's residual out of
                    // it; wiring the inter search to the unpadded plane instead
                    // was an out-of-bounds READ, not a different number.
                    //
                    // MEASURED 2026-09-02: the port PANICKED at
                    // `port_md/md_search.rs`'s source gather ("the len is 5184
                    // but the index is 5184", 5184 = 72*72) on 18 of the 96
                    // grid cells — every 72x72 cell of uniform, diag and screen
                    // content. For a 64-aligned frame `sb_input == encode_input`
                    // and `in_stride == w`, so this is byte-neutral on the other
                    // 72 cells by construction.
                    src: sb_input,
                    src_stride: in_stride,
                    ref_frame_type_arr: &inter_ref_types,
                    search,
                    // The `md_subpel`-shape view of the same nmv storage —
                    // `nmv`'s `approx_inter_rate` zero arm applies here too.
                    search_tables: if sigs.approx_inter_rate != 0 {
                        crate::intrabc::MvCostTables::zeroed()
                    } else {
                        crate::intrabc::build_nmv_cost_table(
                            &fc.nmvc,
                            crate::inter_mv_code::mv_precision(
                                st.allow_high_precision_mv,
                                st.force_integer_mv,
                            ),
                        )
                    },
                    me,
                    fac,
                    ref_fac,
                    nmv,
                    interpolation_filter: st.interpolation_filter,
                    is_motion_mode_switchable: st.is_motion_mode_switchable,
                    allow_warped_motion: st.allow_warped_motion,
                    force_integer_mv: st.force_integer_mv,
                    allow_high_precision_mv: st.allow_high_precision_mv,
                    enable_dual_filter: st.enable_dual_filter,
                    enable_masked_compound: st.enable_masked_compound,
                    enable_jnt_comp: st.enable_jnt_comp,
                    enable_interintra_compound: st.enable_interintra_compound,
                    reference_mode_is_select: matches!(
                        st.reference_mode,
                        crate::port_entropy_inter::refframe::ReferenceMode::Select
                    ),
                    allow_screen_content_tools: sc_derivation.allow_screen_content_tools,
                    order_hint: crate::inter_md_arm::OrderHints {
                        enable_order_hint: st.enable_order_hint,
                        order_hint_bits: st.order_hint_bits,
                        cur_order_hint: st.cur_order_hint,
                        ref_order_hint: st.ref_order_hint,
                    },
                    mvp_env: env.mvp_env(),
                    mi_rows: env.mi_rows,
                    mi_cols: env.mi_cols,
                    tile: env.tile,
                    sb_mi_size: env.sb_mi_size,
                    frame_w: w,
                    frame_h: h,
                    sb_size,
                    gm_wmtype: st.gm_wmtype,
                    global_motion: gm_field,
                    gm_skip_identity,
                    gm_enabled,
                    // C `pcs->inter_compound_mode` — same signal derivation
                    // as `pic_obmc_level` below.
                    inter_compound_mode: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.inter_compound_mode),
                    // C `pcs->inter_intra_level` — `svt_aom_get_inter_
                    // intra_level`'s ladder, the input to
                    // `set_inter_intra_ctrls` at injection.
                    inter_intra_level: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.inter_intra_level),
                    // C `pcs->hbd_md` (`sig_deriv_multi_processes_default`,
                    // enc_mode_config.c:2151-2164): bd10 && preset<=MR → 1,
                    // bd10 && preset<=M5 → 2 (`is_base` — a flat GOP makes
                    // every frame base), else 0 on an inter frame. The
                    // `hbd_mds` CLI override has no port input — it is
                    // DEFAULT here. This is the depth the inter-intra and
                    // masked-compound searches run at; the u16 arms are
                    // live wherever the ladder returns non-zero.
                    hbd_md: match self.bit_depth {
                        10 if self.speed_config.preset <= -1 => 1,
                        10 if self.speed_config.preset <= 5 => 2,
                        _ => 0,
                    },
                    // The frame-owned mask tables C keeps file-scope
                    // (`init_ii_masks` / `svt_av1_init_wedge_masks`): the
                    // smooth inter-intra blends and the master wedge
                    // table both searches and the masked compound arm
                    // read.
                    ii_masks: svtav1_dsp::port_interintra::IiMasks::new(),
                    wedge_masks: svtav1_dsp::port_wedge_masks::WedgeMasks::new(),
                    // C `ppcs->pic_obmc_level`, straight off the mode-decision
                    // signal derivation that already computes it.
                    pic_obmc_level: md_config_signals
                        .as_ref()
                        .map_or(0, |sigs| sigs.pic_obmc_level),
                    ifs: crate::inter_md_arm::IfsFrameKnobs {
                        // `sharpness_ctrls.ifs && is_noise_level` — the
                        // refusal above admits only `is_noise_level == 0`
                        // frames under a sharpness tune, so this is 0 for
                        // exactly the same reason C's gate is.
                        smooth_bias: sharp_tune && is_noise,
                        tx_bias: self.hdr.tx_bias > 0,
                        // C `ppcs->picture_qp` — the index into
                        // `ifs_smooth_bias` (enc_inter_prediction.c:2171);
                        // the RC-derived value, not the CLI qp.
                        picture_qp,
                        // Inter picture, temporal layer 0 (hier_levels 0).
                        ac_bias_eff: svtav1_dsp::ac_bias::effective_ac_bias(
                            self.hdr.ac_bias,
                            false,
                            0,
                        ),
                    },
                    base_update_type: md_lambda_base_update_type
                        .expect("an inter frame always has a picture decision"),
                    factor_update_type: md_lambda_factor_update_type,
                    alt_lambda_factors: md_alt_lambda_factors,
                    lambda_mod_intra,
                    // C `scs->mrp_ctrls.use_best_references` + the
                    // `determine_best_references` inputs — the per-block
                    // `ctx->ref_frame_type_arr` rebuild gate.
                    use_best_references: self.mrp_ctrls.use_best_references,
                    temporal_layer_index: pic_decision
                        .as_ref()
                        .map_or(0, |p| p.temporal_layer_index),
                    ref_list0_count_try: pic_decision
                        .as_ref()
                        .is_some_and(|p| p.ref_list0_count_try > 0),
                    ref_list1_count_try: pic_decision
                        .as_ref()
                        .is_some_and(|p| p.ref_list1_count_try > 0),
                    sframe_ref_pruned: pic_decision
                        .as_ref()
                        .is_some_and(|p| p.sframe_ref_pruned),
                    // C `pcs->ppcs->max_can_count` —
                    // `svt_aom_get_max_can_count(enc_mode, rtc)`; the rtc
                    // half is false on every config this port accepts
                    // (the same `false` `sig_deriv_multi_processes`
                    // passes at multi_processes.rs).
                    max_can_count: crate::port_enc_mode_config::leaf::get_max_can_count(
                        i8::try_from(self.speed_config.preset).unwrap_or(i8::MAX),
                        false,
                    ),
                })
            }
            _ => None,
        };

        // C `av1_lambda_assign_md` (md_process.c:725) run PER SUPERBLOCK, as
        // `svt_aom_mode_decision_configure_sb` (md_process.c:796) calls it —
        // `ctx->me_q_index = svt_aom_get_me_qindex(pcs, sb_ptr, ..)`
        // (enc_dec_process.c:2926) is a per-SB input, so `full_lambda_md[0]`
        // and `fast_lambda_md[0]` are per-SB even with no delta-q signalled.
        //
        // The chain, all already ported and tier-tested:
        //   `me_8x8_cost_variance[b64]`
        //     -> `port_rc_process::generate_b64_me_qindex_map` (rc_aq.c:656)
        //     -> `port_md_rate_estimation::get_me_qindex` (md_rate_estimation.c:1084)
        //     -> `update_lambda`'s `stats_based_sb_lambda_modulation` factor
        //        (rc_process.c:437-446), which is `me_q_index - base_q_idx`.
        //
        // MEASURED against C's own `SVT_PD0CFG_OUT` on `diag 72x72 q40 p6`
        // frame 1: `fastlam` 5182 / 5182 / 5182 / 7773 across the four
        // superblocks of ONE frame, where the port reported a flat 6633.
        //
        // BYTE-INERT ON A KEY FRAME BY CONSTRUCTION: the map's I-slice arm
        // writes `base_q_idx` into every entry, so `qdiff` is 0 and the
        // factor is the identity 128 — and this whole binding is `None`
        // unless `inter_md_frame` is `Some`, which is exactly "non-key with a
        // DPB reference".
        let sb_inter_lambda: Option<Vec<crate::pd0::SbInterLambda>> = match (
            frame_me.as_ref(),
            inter_md_frame.as_ref(),
        ) {
            // C `scs->stats_based_sb_lambda_modulation` (enc_handle.c:4375)
            // reads the POST-clamp `static_config.enc_mode` — so at CLI
            // p12/p13 the non-RTC video arm still sees M11 and the
            // modulation stays ON. When it is off, `generate_sb_qindex`
            // never builds `b64_me_qindex` at all (rc_process.c:747).
            // Skipping the whole binding there leaves every consumer on
            // the frame lambda, which is what C prices with.
            (Some(me), Some(imf))
                if crate::port_rc_process::stats_based_sb_lambda_modulation(
                    crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                    false,
                ) =>
            {
                let mev: Vec<u32> = me.per_b64.iter().map(|o| o.me_8x8_cost_variance).collect();
                let map = crate::port_rc_process::generate_b64_me_qindex_map(
                    &mev,
                    i32::from(base_qindex),
                    /*is_islice=*/ false,
                );
                let lw = crate::pd0::frame_lambda_weight_for_preset(
                    self.speed_config.preset,
                    picture_qp as u32,
                    self.hdr.tune == crate::tune::TUNE_IQ,
                    lw_bump,
                );
                let lctx = crate::port_rc_process::LambdaContext {
                    frame_type: 1, // not KEY_FRAME
                    temporal_layer_index: temporal_layer,
                    hierarchical_levels: frame_hier,
                    update_type: imf.base_update_type,
                    alt_lambda_factors: md_alt_lambda_factors,
                    rtc: false,
                    // C `scs->stats_based_sb_lambda_modulation`
                    // (enc_handle.c:4375). The match guard above already
                    // proved it true for this frame; passing a literal
                    // here would be a second spelling of the same rule.
                    stats_based_sb_lambda_modulation: true,
                    base_q_idx: i32::from(base_qindex),
                    delta_q_present,
                    r0_delta_qp_md,
                    lambda_scale_factors: [128; 7],
                };
                // C `svt_aom_mode_decision_configure_sb` (md_process.c:800-803):
                // `ctx->qp_index = delta_q_present || r0_delta_qp_md ?
                // sb_qp : base_q_idx`. `sb_qp` is the `generate_sb_qindex`
                // map — TPL-derived when `r0_delta_qp_md` ran.
                let qp_mod_arm = delta_q_present || r0_delta_qp_md;
                let mut out = Vec::with_capacity(sb_cols * sb_rows);
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(&stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let me_q = crate::port_md_rate_estimation::get_me_qindex(
                            &map,
                            u16::try_from(w).unwrap_or(u16::MAX),
                            u16::try_from(h).unwrap_or(u16::MAX),
                            u32::try_from(sb_idx).unwrap_or(u32::MAX),
                            u32::try_from(sb_col * sb_size).unwrap_or(u32::MAX),
                            u32::try_from(sb_row * sb_size).unwrap_or(u32::MAX),
                            sb_size == 128,
                        );
                        let qp_idx = if qp_mod_arm {
                            md_sb_qindex.map_or(base_qindex, |p| p.sb_qindex[sb_idx])
                        } else {
                            base_qindex
                        };
                        // `update_lambda` picks its stats-factor arm on the
                        // lctx flags: `delta_q_present || r0_delta_qp_md`
                        // uses `q_index - base` at +-8 (rc_process.c:430-441),
                        // else `me_q_index - base` at +-4 (:442-446) — the
                        // same factor PD0's lambda fold below carries as
                        // `me_qdiff`.
                        let me_qdiff = if qp_mod_arm {
                            i32::from(qp_idx) - i32::from(base_qindex)
                        } else {
                            i32::from(me_q) - i32::from(base_qindex)
                        };
                        let raw =
                            crate::port_rc_process::compute_fast_lambda(&lctx, qp_idx, me_q, 8);
                        // C scales `fast_lambda_md` by the same
                        // LAMBDA_MOD_INTRA arm before `lambda_weight`
                        // (md_process.c:740).
                        let raw = ((u64::from(raw) * lambda_mod_intra as u64) >> 7) as u32;
                        // `full_lambda_md[0]` — `svt_aom_compute_rd_mult`
                        // then LAMBDA_MOD_INTRA then `lambda_weight`
                        // (md_process.c:725-751).
                        let mut full_8bit = ((u64::from(crate::port_rc_process::compute_rd_mult(
                            &lctx, qp_idx, me_q, 8,
                        )) * lambda_mod_intra as u64)
                            >> 7) as u32;
                        if lw != 0 {
                            full_8bit = ((u64::from(full_8bit) * u64::from(lw)) >> 7) as u32;
                        }
                        // `full_lambda_md[EB_10_BIT_MD]` — the same chain at
                        // 10 bits, ending in `*16` (md_process.c:728/753).
                        let mut full_10bit = ((u64::from(crate::port_rc_process::compute_rd_mult(
                            &lctx, qp_idx, me_q, 10,
                        )) * lambda_mod_intra as u64)
                            >> 7) as u32;
                        if lw != 0 {
                            full_10bit = ((u64::from(full_10bit) * u64::from(lw)) >> 7) as u32;
                        }
                        // `*full_lambda *= 16` on C's `uint32_t` wraps.
                        let full_10bit = full_10bit.wrapping_mul(16);
                        #[cfg(feature = "std")]
                        if crate::dbgenv::lamdump() {
                            std::eprintln!(
                                "LAM poc={} tl={} sb={} baseq={} but={:?} fut={:?} alt={} meq={} meqd={} lmi={} lw={} -> full={} fast={}",
                                pic_decision
                                    .as_ref()
                                    .map_or(-1, |p| p.picture_number as i64),
                                temporal_layer,
                                sb_idx,
                                base_qindex,
                                imf.base_update_type,
                                md_lambda_factor_update_type,
                                md_alt_lambda_factors,
                                me_q,
                                me_qdiff,
                                lambda_mod_intra,
                                lw,
                                full_8bit,
                                raw,
                            );
                        }
                        out.push(crate::pd0::SbInterLambda {
                            full_8bit,
                            fast_8bit: if lw == 0 {
                                raw
                            } else {
                                ((u64::from(raw) * u64::from(lw)) >> 7) as u32
                            },
                            me_qdiff,
                            full_10bit,
                        });
                    }
                }
                Some(out)
            }
            _ => None,
        };

        // C `set_blocks_to_be_tested`'s per-SB `min_sq_size`
        // (enc_dec_process.c:1485), which `depth_removal_ctrls` decides.
        // Frame-level here because every input is: the level is a picture
        // signal, the ME distortions are `frame_me`'s per-b64 outputs, and the
        // reference's `sb_min_sq_size` is a DPB field. `None` on a key frame,
        // where `set_depth_removal_level_controls` returns `enabled = 0`
        // outright — which is why the still path has never carried this and is
        // byte-neutral by construction.
        //
        // MEASURED against C's own resolved controls (`SVT_PD0CFG_OUT`'s `dr`
        // field, added for this chunk) on `diag 64x64 q40 p8 frames=2`:
        // frame 0 `dr=0/0/0/0` and frame 1 `dr=1/0/1/1` — enabled, and
        // `disallow_below_32x32` set, so C's PD0 may not test below 32x32
        // there while `Pd0Ctx::min_sq` was a flat 8.
        // The per-SB `DepthRemovalResult` alongside it — `sig_deriv_enc_dec_pd0`'s
        // `subres_level` ladder (enc_mode_config.c:7344-7352) reads the
        // `depth_removal_ctrls` flags and the post-call `disallow_4x4`, which
        // the min_sq fold alone would discard.
        let mut pd0_dr_res: Option<Vec<crate::port_enc_mode_config::common::DepthRemovalResult>> =
            None;
        let pd0_min_sq: Option<Vec<u8>> = match (
            md_config_signals.as_ref(),
            frame_me.as_ref(),
            inter_md_frame.as_ref(),
        ) {
            (Some(sigs), Some(me), Some(_imf)) => {
                use crate::port_enc_mode_config::common as pcommon;
                // C `ctx->disallow_8x8` on the VIDEO arm
                // (`sig_deriv_enc_dec_common`, enc_mode_config.c:7122).
                let disallow_8x8 = crate::port_enc_mode_config::leaf::get_disallow_8x8_default();
                // C `pd0_depth_removal`'s reference read is
                // `ref_obj_l0->sb_min_sq_size[sb_index]`, i.e. LAST's — see
                // `last_ref_slot`, not DPB slot 0. C only reads it when the
                // reference is POC-ADJACENT (`abs(picture_number - ref_poc)
                // <= 1`, enc_mode_config.c:3175-3177); a farther ref leaves
                // `sb_min_sq_size` at `(uint8_t)~0` and the deviation
                // thresholds get NO bump — feeding the value unconditionally
                // was the `96x96 hier` over-disallow on poc>=2. The L1
                // `MIN()` arm (:3179-3186) fires on an RA B slice where
                // `ref_list1_count_try` and a same-size BWD reference
                // (`ref_dpb_index[BWD]`) exist, on ITS OWN POC adjacency —
                // unreachable on low-delay, which never populates list 1.
                // `svt_aom_is_ref_same_size` (enc_mode_config.c:2857) gates
                // each list: `is_not_scaled` short-circuits true.
                let ref_same_size = |rf: &crate::picture::ReferenceFrame| {
                    self.superres_denom.is_none()
                        || (rf.width == self.width as u32 && rf.height == self.height as u32)
                };
                let ref_l0_adj = last_ref_slot
                    .and_then(|slot| self.dpb.get(slot))
                    .filter(|rf| ref_same_size(rf))
                    .filter(|rf| display_order.abs_diff(rf.display_order) <= 1);
                let ref_l1_adj = pic_decision
                    .as_ref()
                    .filter(|p| {
                        p.slice_type == crate::port_picstruct::SliceType::B
                            && p.ref_list1_count_try > 0
                    })
                    .and_then(|p| {
                        self.dpb
                            .get(p.rps.ref_dpb_index[crate::port_picstruct::BWD] as usize)
                    })
                    .filter(|rf| ref_same_size(rf))
                    .filter(|rf| display_order.abs_diff(rf.display_order) <= 1);
                let mut out = Vec::with_capacity(sb_cols * sb_rows);
                let mut dr_out = Vec::with_capacity(sb_cols * sb_rows);
                for sb_row in 0..sb_rows {
                    for sb_col in 0..sb_cols {
                        crate::stop_check(&stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let (x0, y0) = (sb_col * sb_size, sb_row * sb_size);
                        let b = me.per_b64.get(sb_idx);
                        // C `ctx->fast_lambda_md[EB_8_BIT_MD]` for THIS
                        // superblock, from the one derivation above — the
                        // depth-removal thresholds are scaled by it
                        // (`set_depth_removal_level_controls`), and C's own
                        // `SVT_PD0CFG_OUT` `fastlam` field is this value.
                        let fast_lambda = sb_inter_lambda
                            .as_ref()
                            .and_then(|v| v.get(sb_idx))
                            .map_or(0, |l| l.fast_8bit);
                        let res = pcommon::set_depth_removal_level_controls(
                            pcommon::DepthRemovalInputs {
                                level: sigs.pic_depth_removal_level,
                                is_islice: false,
                                fast_lambda_8bit: fast_lambda,
                                delta_q_present,
                                r0_delta_qp_md,
                                // `sb_ptr->qindex` — the generate_sb_qindex
                                // map — vs the signalled frame base; the
                                // level modulation arm reads their diff.
                                sb_qindex: md_sb_qindex.map_or(i32::from(base_qindex), |p| {
                                    i32::from(p.sb_qindex[sb_idx])
                                }),
                                picture_qindex: i32::from(base_qindex),
                                picture_qp: i32::from(picture_qp),
                                dist_64: b.map_or(0, |o| o.me_64x64_distortion),
                                dist_32: b.map_or(0, |o| o.me_32x32_distortion),
                                dist_16: b.map_or(0, |o| o.me_16x16_distortion),
                                dist_8: b.map_or(0, |o| o.me_8x8_distortion),
                                me_8x8_cost_variance: b.map_or(0, |o| o.me_8x8_cost_variance),
                                sb_width: u16::try_from(sb_size.min(w - x0)).unwrap_or(u16::MAX),
                                sb_height: u16::try_from(sb_size.min(h - y0)).unwrap_or(u16::MAX),
                                disallow_4x4_in: true,
                                // C `(uint8_t)~0` when neither list's
                                // reference is POC-adjacent — `None` takes
                                // the no-adjustment arm. With both present
                                // C keeps the `MIN()`.
                                ref_sb_min_sq_size: [
                                    ref_l0_adj.and_then(|rf| rf.sb_min_sq_size.get(sb_idx)),
                                    ref_l1_adj.and_then(|rf| rf.sb_min_sq_size.get(sb_idx)),
                                ]
                                .into_iter()
                                .flatten()
                                .copied()
                                .reduce(u8::min),
                            },
                        );
                        let c = res.map(|r| r.ctrls).unwrap_or_default();
                        // C `set_blocks_to_be_tested` (enc_dec_process.c:1485).
                        let min_sq = if c.enabled != 0 && c.disallow_below_64x64 != 0 {
                            64usize
                        } else if c.enabled != 0 && c.disallow_below_32x32 != 0 {
                            32
                        } else if disallow_8x8 || (c.enabled != 0 && c.disallow_below_16x16 != 0) {
                            16
                        } else {
                            // `pic_disallow_4x4` is 1 at every preset this
                            // port reaches, so C's `: 4` arm is unreachable.
                            8
                        };
                        // `SVTAV1_PD0DBG`: the port-side twin of C's
                        // `SVT_PD0CFG_OUT` `dr=<enabled>/<64>/<32>/<16>`
                        // field, same order, so the two join per superblock.
                        #[cfg(feature = "std")]
                        if crate::dbgenv::pd0dbg() {
                            eprintln!(
                                "PD0DR poc={display_order} nb64={} sb={sb_idx} org=({x0},{y0}) dr={}/{}/{}/{} minsq={min_sq} \
                                 drlvl={} fastlam={fast_lambda} pqp={picture_qp} \
                                 med={}/{}/{}/{} mev={} refmin={}/{}",
                                me.per_b64.len(),
                                c.enabled,
                                c.disallow_below_64x64,
                                c.disallow_below_32x32,
                                c.disallow_below_16x16,
                                sigs.pic_depth_removal_level,
                                b.map_or(0, |o| o.me_64x64_distortion),
                                b.map_or(0, |o| o.me_32x32_distortion),
                                b.map_or(0, |o| o.me_16x16_distortion),
                                b.map_or(0, |o| o.me_8x8_distortion),
                                b.map_or(0, |o| o.me_8x8_cost_variance),
                                ref_l0_adj
                                    .and_then(|rf| rf.sb_min_sq_size.get(sb_idx))
                                    .map_or(255, |v| u32::from(*v)),
                                ref_l1_adj
                                    .and_then(|rf| rf.sb_min_sq_size.get(sb_idx))
                                    .map_or(255, |v| u32::from(*v)),
                            );
                        }
                        out.push(
                            u8::try_from(min_sq.min(usize::from(self.hdr.max_tx_size)))
                                .unwrap_or(8),
                        );
                        // The whole result — `sig_deriv_enc_dec_pd0`'s
                        // subres ladder reads `depth_removal_ctrls` and the
                        // post-call `disallow_4x4` per superblock, not just
                        // the folded `min_sq`.
                        dr_out.push(res.unwrap_or(pcommon::DepthRemovalResult {
                            ctrls: pcommon::DepthRemovalCtrls::default(),
                            disallow_4x4: true,
                        }));
                    }
                }
                pd0_dr_res = Some(dr_out);
                Some(out)
            }
            _ => None,
        };

        // C `pd0_detector`'s reference-side inputs (enc_dec_process.c:
        // 2142-2168): `ref_pic_ptr_array[list][0]`'s `sb_intra`, admitted
        // only while all three of C's guards hold — the MRP-capped
        // `ref_list{0,1}_count_try`, `svt_aom_is_ref_same_size`
        // (enc_mode_config.c:2857; `is_not_scaled` short-circuits true, else
        // B-slice + non-null + matching dims), and the reference's
        // `tmp_layer_idx <= temporal_layer_index`. `None` per list is
        // exactly C's `l{0,1}_refs == 0` arm. List 0's slot-0 reference is
        // LAST (`ref_dpb_index[0]`); list 1's is BWD (`ref_dpb_index[4]`) —
        // the same binding `pipeline_md_inputs.ref_l{0,1}` makes.
        let pd0_ref_sb = |list1: bool| -> crate::part_arm::RefSbStats<'_> {
            let Some(pic) = pic_decision.as_ref() else {
                return Default::default();
            };
            let (count_try, rps_idx) = if list1 {
                (pic.ref_list1_count_try, crate::port_picstruct::BWD as usize)
            } else {
                (
                    pic.ref_list0_count_try,
                    crate::port_picstruct::LAST as usize,
                )
            };
            if count_try == 0 {
                return Default::default();
            }
            let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[rps_idx] as usize) else {
                return Default::default();
            };
            let same_size = self.superres_denom.is_none()
                || (rf.width == self.width as u32 && rf.height == self.height as u32);
            if !(same_size && rf.temporal_layer <= temporal_layer) {
                return Default::default();
            }
            crate::part_arm::RefSbStats {
                avail: true,
                sb_intra: (!rf.sb_intra.is_empty()).then_some(rf.sb_intra.as_slice()),
                sb_skip: (!rf.sb_skip.is_empty()).then_some(rf.sb_skip.as_slice()),
                me_64x64_dist: (!rf.sb_me_64x64_dist.is_empty())
                    .then_some(rf.sb_me_64x64_dist.as_slice()),
                me_8x8_cost_var: (!rf.sb_me_8x8_cost_var.is_empty())
                    .then_some(rf.sb_me_8x8_cost_var.as_slice()),
                mvp_64x64: (!rf.sb_64x64_mvp.is_empty()).then_some(rf.sb_64x64_mvp.as_slice()),
                is_islice: rf.is_islice,
            }
        };
        let pd0_det_frame = crate::part_arm::Pd0DetFrame {
            transition_present: pic_decision
                .as_ref()
                .is_some_and(|p| p.transition_present != 0),
            // `!frame_is_leaf(ppcs)` (enc_mode_config.h:113) — a KEY frame's
            // `KF_UPDATE` is not `LF_UPDATE`, so this is true on frame 0 and
            // false on a flat-GOP inter frame.
            is_not_last_layer: pic_decision
                .as_ref()
                .is_none_or(|p| p.update_type != crate::port_picstruct::FrameUpdateType::Lf),
            // The same value `pipeline_md_inputs` recomputes for
            // `PipelineMdInputs`-keyed consumers — hoisted to frame level for
            // the LAMBDA_MOD_INTRA arm above.
            ref_intra_percentage: md_ref_intra_percentage,
            l0: pd0_ref_sb(false),
            l1: pd0_ref_sb(true),
        };
        // The FRAME-level half of `resolve_sb_lpd1` — the `md_encode_block`
        // Light-PD1 dispatch's inputs that are constant across the picture's
        // superblocks. `None` on a key frame (`md_config_signals` absent) and
        // on every allintra cell, which is what keeps the still envelope
        // byte-neutral by construction.
        #[cfg(feature = "std")]
        if crate::dbgenv::lpd1dbg() {
            eprintln!(
                "RLPD1-FRAME tl={temporal_layer} key={is_key} md_cfg={:?} pmd={}",
                md_config_signals.as_ref().map(|m| m.pic_lpd1_lvl),
                pipeline_md_inputs.is_some(),
            );
        }
        let lpd1_frame = md_config_signals
            .as_ref()
            .zip(pipeline_md_inputs.as_ref())
            .map(|(mc, p)| {
                let enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                Lpd1FrameIn {
                    pic_lpd1_lvl: mc.pic_lpd1_lvl,
                    enc_mode,
                    is_b_slice: pic_decision
                        .as_ref()
                        .is_some_and(|d| d.slice_type == crate::port_picstruct::SliceType::B),
                    input_resolution: p.input_resolution,
                    picture_qp: p.picture_qp,
                    ref_list0_count_try: p.ref_list0_count_try,
                    ref_list1_count_try: p.ref_list1_count_try,
                    ref_skip_percentage: crate::inter_hdr_arm::ref_skip_percentage(p),
                    use_best_me_unipred_cand_only: u8::from(
                        enc_mode > crate::port_enc_mode_config::enc_mode::M1,
                    ),
                    merge_inter_cands_mult: if mc.nic_level >= 6 { 4 } else { u8::MAX },
                    cand_reduction_level: mc.cand_reduction_level,
                    rdoq_level: mc.rdoq_level,
                    coeff_shaving_level: mc.coeff_shaving_level,
                    me_subpel_level: mc.me_subpel_level,
                    rate_est_level: mc.rate_est_level,
                    approx_inter_rate: mc.approx_inter_rate,
                    intra_level: mc.intra_level,
                }
            });

        // `update_pred_th_offset`'s `use_ref_info` read
        // (enc_dec_process.c:1611-1621) takes `ref_obj_l0`'s
        // `sb_min_sq_size`/`sb_max_sq_size`, then — on a B slice with
        // `ref_list1_count_try` and a same-size L1 reference — merges
        // `MIN(min, l1_min)` / `MAX(max, l1_max)`. Unlike the depth-removal
        // read above there is NO POC-adjacency gate. Owned vectors because
        // the merge produces a new array; `None` on a key frame.
        let ref_min_max_sq: Option<(Vec<u8>, Vec<u8>)> = last_ref
            .as_ref()
            .filter(|rf| {
                self.superres_denom.is_none()
                    || (rf.width == self.width as u32 && rf.height == self.height as u32)
            })
            .map(|rf| {
                let mut mn = rf.sb_min_sq_size.clone();
                let mut mx = rf.sb_max_sq_size.clone();
                let l1 = pic_decision
                    .as_ref()
                    .filter(|p| {
                        p.slice_type == crate::port_picstruct::SliceType::B
                            && p.ref_list1_count_try > 0
                    })
                    .and_then(|p| {
                        self.dpb
                            .get(p.rps.ref_dpb_index[crate::port_picstruct::BWD] as usize)
                    })
                    .filter(|rf| {
                        self.superres_denom.is_none()
                            || (rf.width == self.width as u32 && rf.height == self.height as u32)
                    });
                if let Some(l1) = l1 {
                    for (m, v) in mn.iter_mut().zip(l1.sb_min_sq_size.iter()) {
                        *m = (*m).min(*v);
                    }
                    for (m, v) in mx.iter_mut().zip(l1.sb_max_sq_size.iter()) {
                        *m = (*m).max(*v);
                    }
                }
                (mn, mx)
            });

        let tile_recons = encode_tile_rows(
            &encode_input,
            sb_input,
            in_stride,
            w,
            h,
            sb_size,
            sb_cols,
            sb_rows,
            tile_grid,
            base_qindex,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            md_sb_qindex.map(|p| p.sb_qindex.as_slice()),
            // `frm_hdr.delta_q_params.delta_q_present` — the SIGNALLING
            // side. `md_sb_qindex` exists under `r0_delta_qp_md` even when
            // nothing is signalled, so it cannot proxy this flag.
            delta_q_plan.is_some(),
            tpl_rdmult.clone(),
            (chroma_deltas.u_ac, chroma_deltas.v_ac),
            sharp_tx_active,
            if self.hdr.is_fork() {
                self.hdr.noise_norm_strength
            } else {
                0
            },
            qm_levels,
            if self.hdr.is_fork() {
                self.hdr.tx_bias
            } else {
                0
            },
            self.hdr.is_fork() && self.hdr.complex_hvs == 1,
            // Share the frame's resolved detection with every tile.
            sc_derivation,
            sc_arm,
            self.hdr.is_fork() && self.hdr.alt_ssim_tuning,
            self.hdr.is_fork() && self.hdr.alt_lambda_factors,
            if self.hdr.tune == crate::tune::TUNE_IQ {
                Some(crate::tune::iq_lambda_weight(picture_qp as u32))
            } else if self.speed_config.preset == -1 {
                Some(0)
            } else {
                None
            },
            ssim_rdmult.as_ref(),
            base_qindex,
            frame_tx_mode_select,
            tpl_adjusted_qp,
            picture_qp,
            lw_bump,
            self.hdr.tune == crate::tune::TUNE_IQ,
            self.hdr.sharpness,
            lambda,
            &self.speed_config,
            ref_frame_data,
            crate::port_picstruct::is_highest_layer(temporal_layer, frame_hier),
            temporal_layer,
            // The padded twin of the plane above, from the SAME DPB slot.
            ref_padded_luma,
            inter_md_frame.as_ref(),
            inter_syntax_state.as_ref(),
            inter_mvp_env.as_ref(),
            pd0_min_sq.as_deref(),
            pd0_dr_res.as_deref(),
            // `ref_obj_l0->{sb_min_sq_size,sb_max_sq_size}` for the
            // use_ref_info refinement arm — LAST ref (see `last_ref_slot`),
            // `None` on a key frame.
            ref_min_max_sq
                .as_ref()
                .map(|(mn, mx)| (mn.as_slice(), mx.as_slice())),
            sb_inter_lambda.as_deref(),
            primary_ref_cdfs.as_deref(),
            &mv_map,
            mv_map_stride,
            // "chroma present AND the C-parity surface" — `filter_chroma`,
            // so the 4:2:0-only funnel never arms at 4:4:4.
            filter_chroma,
            self.chroma_format
                .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
            c_quant.clone(),
            sb_chroma_owned
                .as_ref()
                .map(|(u, v)| (u.as_slice(), v.as_slice())),
            self.bit_depth,
            // Task #6 chunk 1: the native 10-bit source for the bd10 MD
            // funnel (`None` on every u8 path).
            // The SB-extent-padded twins when the frame has a partial SB (see
            // `hbd_sb_owned`), else the aligned planes — identical on every
            // 64-aligned frame.
            match hbd_sb_owned.as_ref() {
                Some((y, u, v)) => Some((y.as_slice(), u.as_slice(), v.as_slice())),
                None => hbd_source
                    .as_ref()
                    .map(|h| (h.y.as_slice(), h.u.as_slice(), h.v.as_slice())),
            },
            &hbd_used_flag,
            // Superres chunk B.4: C's stale full-res variance array.
            stale_vars.as_deref(),
            self.hdr.max_tx_size,
            coded_lossless,
            self.enhancements
                .contains(crate::enhancements::ZenEnhancement::DeepSearch),
            self.reference,
            zen_intra_edge_filter,
            self.thread_count,
            pd0_det_frame,
            lpd1_frame,
            &stop,
        )?;
        hbd_used |= hbd_used_flag.load(core::sync::atomic::Ordering::Relaxed);

        // Task #96: `all_trees` is indexed by RASTER sb_idx
        // (`sb_row * sb_cols + sb_col`) by every consumer — the entropy
        // walk, the CDEF/LR re-walks, the deblock geometry pass. Tile
        // order equals raster order only while tiles are full-width row
        // bands; with tile COLUMNS it does not, so each tile's trees are
        // placed at their raster positions instead of appended.
        let mut tree_slots: Vec<Option<crate::partition::PartitionTree>> =
            (0..sb_cols * sb_rows).map(|_| None).collect();
        // Same raster layout as `tree_slots`: the per-SB encode-pass RDOQ
        // enable (`rdoq_ctrls->enabled`) collected during the walk — see the
        // `encode_tile_rows` return tuple. Filled to the frame default so an
        // SB whose slot never received a tree reads C's regular arm.
        let mut sb_enc_rdoq: Vec<bool> =
            vec![c_quant.as_ref().map_or(false, |q| q.rdoq_level != 0); sb_cols * sb_rows];

        // ---- bd10 FULL-RD 10-bit post-MD canvas (frame scope) ----------
        // Each tile returns its own frame-extent canvas with only its SB
        // region written; merge the per-tile regions into ONE tight w*h /
        // (w/2)*(h/2) pair. This is the port's true 10-bit reconstruction of
        // the coded frame — C's 16-bit recon picture
        // (`svt_aom_get_recon_pic(pcs, &recon, is_16bit)`) — and it is what
        // the bd10 post-filter searches (CDEF strength, Wiener LR) must read.
        //
        // Source-of-truth note: at p6 this canvas, NOT `bd10_reencode_luma`'s
        // output, is the live one. The level-only re-encode post-pass below
        // declines whenever any leaf has `tx_depth > 0` (bd10_tree_supported),
        // which real photographic content at p6 always has; the FULL-RD
        // funnel has its own 10-bit tx-depth loop and commits the winner's
        // 10-bit recon per block (`commit_leaf`, leaf_funnel.rs). Where the
        // post-pass DOES run (eff-M9 band) it overwrites the coded levels, so
        // its recon wins — handled after the post-pass below.
        // STRIDE (task #94 partial-SB): the per-tile canvases are SB-extent
        // SIZED but ALIGNED-STRIDED — `commit_leaf` writes them at `y_stride`
        // (= the aligned `w`) / `fx.c_stride` (= `w/2`), the SB-extent product
        // existing only so a right-straddle write wraps into slack instead of
        // out of bounds (see `ext_w`/`ext_h` at the allocation site). Reading
        // them back at the SB-EXTENT stride was byte-inert only because every
        // gated bd10 cell had `ext_w == w`; on a partial-SB frame it scrambled
        // the merged 10-bit recon that the bd10 deblock/CDEF/LR searches read.
        let mut canvas10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = tile_recons
            .first()
            .and_then(|t| t.2.as_ref())
            .map(|_| -> crate::EncodeResult<(Vec<u16>, Vec<u16>, Vec<u16>)> {
                Ok((
                    svtav1_types::try_vec![0u16; w * h]?,
                    svtav1_types::try_vec![0u16; acw * ach]?,
                    svtav1_types::try_vec![0u16; acw * ach]?,
                ))
            })
            .transpose()?;
        if let Some((cy, cu, cv)) = canvas10.as_mut() {
            for (tile_idx, t) in tile_recons.iter().enumerate() {
                let Some((ty, tu, tv)) = t.2.as_ref() else {
                    continue;
                };
                let (r0, r1) = tile_grid.row_span(tile_idx / tile_grid.tile_cols);
                let (c0, c1) = tile_grid.col_span(tile_idx % tile_grid.tile_cols);
                let (y0, y1) = (r0 * sb_size, (r1 * sb_size).min(h));
                let (x0, x1) = (c0 * sb_size, (c1 * sb_size).min(w));
                for r in y0..y1 {
                    cy[r * w + x0..r * w + x1].copy_from_slice(&ty[r * w + x0..r * w + x1]);
                }
                let (cw, cxs, cxe) = (acw, x0 >> ss_x, x1 >> ss_x);
                let cst = cw;
                for r in (y0 >> ss_y)..(y1 >> ss_y) {
                    cu[r * cw + cxs..r * cw + cxe]
                        .copy_from_slice(&tu[r * cst + cxs..r * cst + cxe]);
                    cv[r * cw + cxs..r * cw + cxe]
                        .copy_from_slice(&tv[r * cst + cxs..r * cst + cxe]);
                }
            }
        }

        // Merge tile recons into frame buffer and update MV map.
        //
        // CONSUMES `tile_recons`: every SB tree is MOVED into its raster slot
        // instead of deep-cloned. The clone used to duplicate every
        // `BlockDecision` in the frame — each carries up to nine owned `Vec`s
        // (`qcoeffs`, the per-txb `Vec<Vec<i32>>`, the six-`Vec` `chroma_dec`,
        // the palette pair) — so it was a whole extra allocate+memcpy+free of
        // the frame's entire decision set for a value that is dropped a few
        // lines later. `tile_recons` is not read after this loop.
        for (tile_idx, (tile_recon, tile_trees, _canvas10, tile_rdoq)) in
            tile_recons.into_iter().enumerate()
        {
            let (tile_sb_row_start, tile_sb_row_end) =
                tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (tile_sb_col_start, tile_sb_col_end) =
                tile_grid.col_span(tile_idx % tile_grid.tile_cols);
            let mut tile_trees = tile_trees.into_iter();
            let mut tile_rdoq = tile_rdoq.into_iter();
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check (no-op
                // for the default `Unstoppable` token — `may_stop()` is false).
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    crate::stop_check(&stop)?;
                    tree_slots[sb_row * sb_cols + sb_col] = tile_trees.next();
                    if let Some(en) = tile_rdoq.next() {
                        sb_enc_rdoq[sb_row * sb_cols + sb_col] = en;
                    }
                }
            }
            let mut offset = 0;
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check.
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    crate::stop_check(&stop)?;
                    let x0 = sb_col * sb_size;
                    let y0 = sb_row * sb_size;
                    let cur_w = sb_size.min(w - x0);
                    let cur_h = sb_size.min(h - y0);
                    for r in 0..cur_h {
                        recon[(y0 + r) * w + x0..(y0 + r) * w + x0 + cur_w].copy_from_slice(
                            &tile_recon[offset + r * cur_w..offset + (r + 1) * cur_w],
                        );
                    }
                    offset += cur_w * cur_h;

                }
            }
        }
        let mut all_trees: Vec<crate::partition::PartitionTree> = tree_slots
            .into_iter()
            .map(|t| t.expect("every SB is covered by exactly one tile"))
            .collect();
        // C `pcs->sb_min_sq_size[sb]` (`coding_loop.c:1640`), folded here
        // rather than during the pack because this is the point where the
        // trees are in RASTER SB order — which is the order
        // `EbReferenceObject::sb_min_sq_size` is indexed in by the NEXT
        // frame's `set_depth_removal_level_controls`
        // (`enc_mode_config.c:3173-3196`). Cheap: one walk of the decided
        // tree, no `BlockDecision` touched.
        let sb_min_sq_sizes: Vec<u8> = all_trees
            .iter()
            .map(|t| u8::try_from(t.min_sq_size(sb_size)).unwrap_or(u8::MAX))
            .collect();
        // C `pcs->sb_max_sq_size[sb]` (`coding_loop.c:1641`) — the `MAX`
        // twin of the fold above, stored on the reference object for the
        // NEXT frame's `use_ref_info` arm in `update_pred_th_offset`
        // (enc_dec_process.c:1614-1630).
        let sb_max_sq_sizes: Vec<u8> = all_trees
            .iter()
            .map(|t| u8::try_from(t.max_sq_size(sb_size)).unwrap_or(u8::MAX))
            .collect();

        crate::stop_check(&stop)?;

        // Step 4c: bd10 LUMA re-encode (task #94, the u16 MD path). The u8
        // funnel above produced C's partition/mode/tx decisions (RD is
        // ~16x-scale-invariant for `sample << 2` content); this pass recomputes
        // the bit-depth-SENSITIVE coded luma levels + 10-bit recon at true
        // 10-bit (Q10 tables + bd10 lambda), mutating the per-SB trees in place
        // so the (unchanged) entropy walk codes the 10-bit levels. bd8 skips
        // this entirely. HARNESS SCOPE: the port receives the u8 (MSB-shifted)
        // content, so the true 10-bit source is `u8 << 2` — exactly the u16
        // .yuv the C reference encodes at bd10 (identity_run writes both from
        // one gradient). Native u16 (non-<<2) ingestion is a follow-up.
        // Stale-canvas guard: the 10-bit recon is per-frame and the gate
        // below can decline (out-of-envelope tree / partial SB). Clearing
        // here means the post-filter chain's `Some(..)` test is exactly
        // "this frame produced a complete 10-bit recon", never a leftover.
        self.last_recon10_y = None;
        self.last_recon10_uv = None;
        self.last_recon10_final = None;
        // The FULL-RD funnel's committed 10-bit canvas is the baseline (the
        // p0..p8 band). Where the level-only post-pass below also runs it
        // REPLACES the coded levels, so its recon supersedes this — the
        // post-pass overwrites both fields at its own end.
        if let Some((cy, cu, cv)) = canvas10 {
            self.last_recon10_y = Some(cy);
            self.last_recon10_uv = Some((cu, cv));
        }
        if self.bit_depth == 10 {
            // The native level pass accepts depth-zero transforms, including
            // filter-intra and directional prediction without edge filtering.
            // Use the actual sequence-header tool value. Monochrome disables
            // edge filtering; color's full-RD funnel handles lower presets.
            let bd10_edge_filter = seq_tools.enable_intra_edge_filter;
            // PARTIAL SB (2026-08-04): this used to be gated on
            // `w % 64 == 0 && h % 64 == 0` with the rationale that
            // "`tx_unit_hbd` is not partial-SB-aware". That named the wrong
            // function — `tx_unit_hbd` takes explicit `(w, h, stride, off)` and
            // is handed `rd: None` here, so it has no geometry term at all.
            // The real exposure was in the CALLERS, and all of it is now fixed:
            // `recon10` is SB-extent-sized (was ALIGNED-sized, so a straddling
            // write ran past the buffer or wrapped a row), the recon writes are
            // straddle-clipped like `commit_leaf`'s, the sources are the
            // SB-extent-padded `sb_input`/`sb_chroma_owned` twins, and the Split
            // arms walk quadrant SLOTS skipping off-frame origins instead of
            // zipping a fixed `(type, len)` offset table that a pruned child
            // list does not satisfy. See `bd10_reencode_luma` /
            // `bd10_reencode_node`.
            // HISTORICAL MEASUREMENT (task #94, before native luma context
            // wiring below): the bd10 FULL-RD funnel also
            // produces 10-bit coded levels, computed with each txb's REAL
            // entropy contexts — whereas this post-pass hardcodes the RDOQ
            // contexts to 0/0 (only correct where `real_coeff_ctx` is off).
            // Skipping the post-pass in favour of the funnel's levels was
            // therefore expected to be strictly better; it was A/B MEASURED on
            // the p6 bd10 grid and is NOT (4/20 byte-exact with the post-pass,
            // 3/20 without — `gradient 64x64 q12` regresses to a CDEF-strength
            // divergence). So the post-pass stays authoritative for the coded
            // levels until that is root-caused. The funnel's 10-bit levels are
            // still live where the post-pass does not reach: the neighbour
            // `cul` bytes that drive later blocks' coefficient contexts, and
            // the u8 chroma recon the CDEF/LR searches read.
            // The resulting gate remains: where the FULL-RD funnel ran, it
            // ALREADY produced this frame's
            // coded 10-bit levels and the committed 10-bit recon, computed
            // with each txb's REAL entropy contexts. This level-only post-pass
            // originally hardcoded RDOQ contexts to 0/0 — correct only where
            // `real_coeff_ctx` is off — so letting it run on top REPLACED
            // correct levels with ones quantized under the wrong contexts, and
            // the recon it writes then disagrees with the bitstream the funnel
            // decided. That is exactly the invariant `bd10_full_rd_supported`
            // documents ("the winner's 10-bit levels ARE the coded ones, so
            // the level-only re-encode post-pass is skipped"); it was
            // documented but never actually implemented in this gate.
            //
            // MEASURED (bd10, 128x128 gradient, presets 3 and 5, q12/q32/q55):
            // with both running, the port's 10-bit recon differs from C's by
            // 8194-11766 bytes and the tile payload diverges; with the
            // post-pass correctly skipped, the recon is byte-identical to C's
            // `svt_aom_get_recon_pic` dump. The eff-M9 band (preset >= 9) is
            // NOT full-RD, so the post-pass stays authoritative there — which
            // is why removing it wholesale regressed that band (the A/B noted
            // in docs/bd10-port-map.md) while removing it *conditionally* does
            // not.
            let bd10_full_rd = bd10_full_rd_supported(
                coded_lossless,
                self.bit_depth,
                self.speed_config.preset,
                chroma.is_some(),
                is_key,
                w,
                h,
            );
            // `pcs->hbd_md` gates only the MD quantization depth
            // (`is_islice ? 2 : 0` at M6+, `is_base ? 2 : 0` at M0..M5 —
            // TRACED 2026-09-18: the johnny p6 cell's P-frame derives 0). The
            // residual 10-bit re-quantize this post-pass models is C's at TWO
            // different sites depending on `pic_bypass_encdec`:
            // - bypass off (bd10 video <= M7): the ENCODE pass
            //   (`av1_encode_loop` -> `svt_aom_quantize_inv_quantize` with
            //   `is_encode_pass = true`, `ed_ctx->bit_depth =
            //   encoder_bit_depth`).
            // - bypass on (bd10 video M8+, `get_bypass_encdec_default`,
            //   enc_mode_config.c:8426-8433): `encode_b` early-returns
            //   through `update_b` and ships the MD-committed levels — but
            //   `product_coding_loop.c:9649` first bumps `ctx->hbd_md = 2`
            //   for `bypass_encdec && encoder_bit_depth > 8 &&
            //   pd_pass == PD_PASS_1 && perform_md_recon`, so MDS3 itself
            //   quantizes the winner at TRUE 10-bit (`full_lambda_md[
            //   EB_10_BIT_MD]`). TRACED 2026-10-08 on vidyo4 p8: zero
            //   `is_encode_pass` quantize calls, and the MD-side `lam`
            //   equals the 10-bit chain exactly.
            // Either way the coded coefficients are a 10-bit re-quantize of
            // the committed TUs, so the post-pass runs in both modes.
            let bd10_postpass_runs = !bd10_full_rd
                && all_trees
                    .iter()
                    .all(|t| bd10_tree_supported(t, bd10_edge_filter, coded_lossless));
            // `update_skip_ctx_dc_sign_ctx` for THIS arm — C gates the
            // per-TU `get_txb_ctx` derivation (and its RDOQ input) on it
            // (`rate_est_ctrls` at enc_mode_config.c:6428). `for_preset`
            // bakes the allintra ladder (real ctx only <= M6); the video arm
            // is a flat `rate_est_level = 1` (:8942), so P/B-frames derive
            // REAL coefficient contexts at every preset — MEASURED on the
            // vidyo4 p8 cell, where C's MDS3 quantize read tsc=2 on a
            // split-TX TU whose coded left neighbour feeds the
            // `skip_contexts` table.
            let postpass_real_ctx =
                crate::rate_arm::rate_est_ctrls(crate::rate_arm::rate_est_level(
                    sc_arm,
                    crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset),
                ))
                .1;
            // Diagnostic: which 10-bit canvas the post-filter searches (DLF
            // level, CDEF strength, Wiener LR) end up reading. The two
            // producers — the FULL-RD funnel's committed per-block recon and
            // this level-only post-pass — are gated differently, so "which one
            // is live" is the first question any recon-parity investigation
            // has to answer and it is not otherwise observable from outside.
            #[cfg(feature = "std")]
            if crate::dbgenv::bd10_postpass() {
                let unsupported = all_trees
                    .iter()
                    .filter(|t| !bd10_tree_supported(t, bd10_edge_filter, coded_lossless))
                    .count();
                eprintln!(
                    "BD10_POSTPASS runs={bd10_postpass_runs} \
                     unsupported_sbs={unsupported}/{} edge_filter={bd10_edge_filter}",
                    all_trees.len()
                );
            }
            if let Some(cq) = c_quant.as_ref().filter(|_| bd10_postpass_runs) {
                let shift = (self.bit_depth - 8) as u32;
                // Task #6 chunk 1: the REAL 10-bit source when the caller
                // entered through `try_encode_frame_*_hbd` (so the coded
                // levels carry the low 2 bits), else the `u8 << shift`
                // widening this site always did.
                // It is the SB-EXTENT-padded plane (`sb_input` / `hbd_sb_owned`
                // at `in_stride`), not the aligned one: a straddling leaf's
                // residual gather reads the full block width. Identical to the
                // aligned plane on every 64-aligned frame, where
                // `sb_input == encode_input` and `in_stride == w`.
                let src10: alloc::vec::Vec<u16> = match hbd_sb_owned
                    .as_ref()
                    .map(|(y, _, _)| y)
                    .or_else(|| hbd_source.as_ref().map(|h| &h.y))
                {
                    Some(y10) => {
                        debug_assert_eq!(y10.len(), sb_input.len());
                        hbd_used = true;
                        y10.clone()
                    }
                    None => sb_input.iter().map(|&s| (s as u16) << shift).collect(),
                };
                // bd10 ENCODE-PASS lambda (C `pic_full_lambda[EB_10_BIT_MD]`,
                // assigned in `reset_enc_dec` via `svt_aom_lambda_assign(..,
                // EB_TEN_BIT, base_q_idx, multiply_lambda = true)`,
                // enc_dec_process.c:184-188). That is `compute_rd_mult` at
                // 10 bits — the update-type base multiplier + the
                // `rd_frame_type_factor[1]` row + scale — then `*= 16`. It
                // does NOT take `av1_lambda_assign_md`'s `lambda_weight` or
                // `lambda_mod_intra` (md_process.c:730-753): those are the MD
                // ladders, and this post-pass models EncDec.
                //
                // On a key frame `kf_full_lambda_bd10` computes the same
                // chain with the KF base multiplier (3.3) — equal to
                // `lambda_assign` here once `pcs->lambda_weight` is 0, which
                // is every measured key-frame cell; keep the proven call.
                let lambda_bd10 = u64::from(if is_key || coded_lossless {
                    crate::pd0::kf_full_lambda_bd10(
                        base_qindex,
                        picture_qp as u32,
                        self.speed_config.preset,
                    )
                } else {
                    // `ed_ctx->md_ctx->full_lambda_md[EB_10_BIT_MD]` — the
                    // `av1_lambda_assign_md` chain, NOT `pic_full_lambda`:
                    // coding_loop.c:436 hands the encode-pass quantizer the
                    // MD lambda, so `lambda_weight`/`lambda_mod_intra` DO
                    // apply (at q40 the weight is 150 → λ ×1.17).
                    crate::pd0::inter_full_lambda_bd10(
                        base_qindex,
                        md_lambda_base_update_type
                            .expect("an inter frame always has a picture decision"),
                        md_lambda_factor_update_type,
                        md_alt_lambda_factors,
                        0,
                        lambda_mod_intra,
                        crate::pd0::frame_lambda_weight_for_preset(
                            self.speed_config.preset,
                            picture_qp as u32,
                            self.hdr.tune == crate::tune::TUNE_IQ,
                            lw_bump,
                        ),
                    )
                });
                // `ed_ctx->md_skip_blk` (coding_loop.c:387/464): C's encode
                // pass force-zeroes every TU — luma AND chroma — when MD
                // committed the block as skip. The chroma pass runs after the
                // luma walk overwrites the leaf eobs, so the funnel's
                // commitment is collected into this set on the way through.
                let mut committed_skip: alloc::collections::BTreeSet<(u32, u32)> =
                    alloc::collections::BTreeSet::new();
                let recon10 = bd10_reencode_luma(
                    &mut all_trees,
                    sb_cols,
                    sb_size,
                    &tile_grid,
                    w,
                    h,
                    &src10,
                    in_stride,
                    base_qindex,
                    cq.rdoq_level,
                    lambda_bd10,
                    cq.allintra_rd_mult,
                    postpass_real_ctx,
                    bd10_edge_filter,
                    self.bit_depth,
                    qm_levels[0],
                    self.hdr.sharpness,
                    // The DPB's reference pictures, whose 10-bit twin the INTER
                    // arm predicts from. `None` on a key frame, where no leaf
                    // can be inter.
                    inter_md_frame.as_ref().map(|f| &f.padded_by_ref),
                    // The encode-pass RDOQ rate table is estimated from
                    // `pcs->md_frame_context`, seeded from the primary ref's
                    // saved CDFs (enc_dec_process.c:2817 +
                    // md_config_process.c:292) — same source as `fun_rates`.
                    primary_ref_cdfs.as_deref(),
                    &mut committed_skip,
                    // Per-SB `full_lambda_md[EB_10_BIT_MD]` — C's encode pass
                    // re-runs `av1_lambda_assign_md` for every superblock
                    // (mode_decision_configure_sb), so the RDOQ lambda varies
                    // by SB through `me_q_index - base_q_idx` even with
                    // `delta_q_present == 0`.
                    sb_inter_lambda.as_deref(),
                    // Per-SB `rdoq_ctrls->enabled` — the light-PD1 encode arm
                    // can turn RDOQ off where `pcs->rdoq_level` kept it
                    // (`sig_deriv_enc_dec_light_pd1_default`, lpd1 > L4 →
                    // level 0). Uniform `rdoq_level != 0` on key frames, so
                    // the stills gates are byte-inert.
                    Some(&sb_enc_rdoq),
                )?;
                // bd10 CHROMA re-encode (task #94): recompute chroma levels at
                // bd10 too — the luma pass above leaves chroma at the u8 MD
                // decision, which diverges on content whose subsampled chroma
                // carries a coded residual (e.g. `diag`). Gated identically
                // (complete-SB + bd10_tree_supported, which rejects CfL /
                // directional-uv-with-edge-filter). Flat-chroma content
                // (gradient/uniform) re-encodes to the same zero result, so bd8
                // and the existing bd10 gate cells stay byte-unchanged. Chroma
                // qindex == base_qindex in mainline (all FH chroma deltas 0),
                // matching the walk's `base_q_idx` chroma coding.
                if let Some((u_src, v_src)) = sb_chroma_owned.as_ref() {
                    // Task #6 chunk 1: real 10-bit chroma when supplied. Both
                    // sides are the SB-extent shape (`sb_chroma_owned` /
                    // `hbd_sb_owned`), which is the untouched aligned chroma on
                    // a 64-aligned frame — so the two planes match
                    // element-for-element either way.
                    let hbd_uv = hbd_sb_owned
                        .as_ref()
                        .map(|(_, u, v)| (u, v))
                        .or_else(|| hbd_source.as_ref().map(|h| (&h.u, &h.v)))
                        .filter(|(u, _)| !u.is_empty());
                    let (u10, v10): (alloc::vec::Vec<u16>, alloc::vec::Vec<u16>) = match hbd_uv {
                        Some((hu, hv)) => {
                            debug_assert_eq!(hu.len(), u_src.len());
                            hbd_used = true;
                            (hu.clone(), hv.clone())
                        }
                        None => (
                            u_src.iter().map(|&s| (s as u16) << shift).collect(),
                            v_src.iter().map(|&s| (s as u16) << shift).collect(),
                        ),
                    };
                    // The pass's residual gather reads the full TX width at
                    // `cstride`, so a right-straddle TU on an `acw`-strided
                    // plane WRAPS into the next row's real samples. C reads
                    // its chroma picture at a border-inclusive stride whose
                    // pad holds the replicated right edge
                    // (`svt_aom_generate_padding16_bit`, resize.c:1064 —
                    // `pad_input_pictures` on the non-resize path). Give the
                    // source the same SB-extent-stride, edge-replicated shape
                    // the funnel builds at `padded_chroma10`
                    // (~pipeline.rs:14106). MEASURED: uniform 128 q20 d10 at
                    // p9/p10/p13 — C coded a chroma txb the next-row read
                    // quantized to eob 0 (27B vs C's 28B OBU, ±1 chroma LSB
                    // in the right-region recon).
                    let cstride = fmt.chroma_width(w.div_ceil(sb_size) * sb_size);
                    let (u10, v10) = if cstride != acw {
                        let md_ch = fmt.chroma_height(h.div_ceil(sb_size) * sb_size);
                        (
                            pad_plane_replicate_u16(&u10, acw, acw, ach, cstride, md_ch)?,
                            pad_plane_replicate_u16(&v10, acw, acw, ach, cstride, md_ch)?,
                        )
                    } else {
                        (u10, v10)
                    };
                    let uv10 = bd10_reencode_chroma(
                        &mut all_trees,
                        sb_cols,
                        sb_size,
                        &tile_grid,
                        w,
                        h,
                        &u10,
                        &v10,
                        cstride,
                        // The 10-bit LUMA recon the pass above just produced —
                        // the CfL AC source for UV_CFL_PRED leaves. C reads the
                        // same thing (`cfl_temp_luma_recon16bit`), and it is
                        // fully committed here because the luma re-encode walks
                        // the entire frame before chroma starts.
                        &recon10,
                        w,
                        // base_qindex sources the frame-level coeff-rate context
                        // (`cfc`); qindex_u/qindex_v drive the per-plane chroma
                        // quant tables (== base in mainline). See the fn doc.
                        base_qindex,
                        qindex_u,
                        qindex_v,
                        cq.rdoq_level,
                        lambda_bd10,
                        cq.allintra_rd_mult,
                        postpass_real_ctx,
                        bd10_edge_filter,
                        self.bit_depth,
                        [qm_levels[1], qm_levels[2]],
                        self.hdr.sharpness,
                        inter_md_frame.as_ref().map(|f| &f.padded_by_ref),
                        primary_ref_cdfs.as_deref(),
                        &committed_skip,
                        sb_inter_lambda.as_deref(),
                        Some(&sb_enc_rdoq),
                    )?;
                    // Crop the SB-extent canvases to the in-frame planes every
                    // downstream consumer expects (the bd10 deblock-level /
                    // CDEF-strength / Wiener-LR searches compare them against
                    // `w*h` and `(w>>ss_x)*(h>>ss_y)` sources at the ALIGNED stride).
                    // The canvases are `cstride`-strided (== `acw` on a
                    // 64-aligned frame, where the crop degenerates to a prefix).
                    let mut cu = svtav1_types::try_vec![0u16; acw * ach]?;
                    let mut cv = svtav1_types::try_vec![0u16; acw * ach]?;
                    for r in 0..ach {
                        cu[r * acw..(r + 1) * acw]
                            .copy_from_slice(&uv10.0[r * cstride..r * cstride + acw]);
                        cv[r * acw..(r + 1) * acw]
                            .copy_from_slice(&uv10.1[r * cstride..r * cstride + acw]);
                    }
                    self.last_recon10_uv = Some((cu, cv));
                }
                self.last_recon10_y = Some(recon10[..w * h].to_vec());
            }
            // At hbd_md == 0 (non-I-slice, enc_mode > M5 — the `is_key`/`is_base`
            // terms C encodes in `pcs->hbd_md`, TRACED 2026-09-18) C's mode
            // decision runs entirely on the MSB-truncated u8 picture — the u16
            // source's consumption IS that truncation
            // (`svt_convert_8bit_to_16bit` is a plain copy, pack_unpack_c.c:198;
            // the 16-bit pipeline then carries 0..255 values). Mark it consumed
            // so the no-silent-truncation guard does not fire on exactly the
            // frames where truncation is the C-faithful behavior.
            if hbd_source.is_some() && !is_key && !bd10_full_rd {
                hbd_used = true;
            }
        }

        crate::stop_check(&stop)?;

        // Step 5: Post-reconstruction filters.
        //
        // Deblocking is SIGNALED and applied decoder-exactly further down
        // (after the entropy walk records the block/TX/skip geometry the
        // edge walk needs — see `deblock_geom` / apply_deblock_frame).
        //
        // CDEF is SIGNALED and applied decoder-exactly after deblocking
        // (step 6a'). Wiener loop restoration is SIGNALED and applied
        // decoder-exactly after CDEF (step 6a''): the C-exact search picks
        // per-RU taps against the post-CDEF recon, and when any plane
        // signals RESTORE_WIENER the tile is re-walked with the per-SB LR
        // syntax and the output copy gets the decoder's stripe-boundary
        // filter pass. sgrproj is never searched on the ALL-INTRA arm
        // (sg_filter_lvl = 0 — C enc_mode_config.c:2000); the VIDEO arm
        // searches it at M0..M3 and the chain below carries it.

        crate::stop_check(&stop)?;

        // Step 6: Entropy coding — recursive partition tree encoding.
        // Walk each SB's partition tree in spec order (depth-first),
        // writing partition type at each node before recursing into children.
        //
        // For 4:2:0 the chroma blocks are predicted, transformed and
        // reconstructed INSIDE this walk (encode_block_syntax), so the
        // chroma coding order is structurally identical to the decoder's
        // parse order — the UV_DC prediction reads exactly the chroma
        // neighbors the decoder will have reconstructed.
        let cw = acw;
        // SB-extent chroma buffer (task #95 chunk 2): the pack reconstructs a
        // straddling boundary block's chroma past the aligned chroma extent, so
        // size u_recon/v_recon to the extent PRODUCT (aligned stride `cw`, a
        // right-straddle write wraps down into the slack). `ext == aligned` on a
        // 64-aligned frame → no-op. The final-recon crop + deblock/CDEF read
        // only the in-frame region at stride `cw`, unaffected by the slack.
        let ext_cbuf = fmt.chroma_width(w.div_ceil(sb_size) * sb_size)
            * fmt.chroma_height(h.div_ceil(sb_size) * sb_size);
        // Debug aid: SVTAV1_DUMP_TREE=1 prints every winning leaf
        // (abs rect, mode, tx_type, eob) in coding order — the fastest way
        // to correlate a recon-parity diff position with the block that
        // produced it.
        #[cfg(feature = "std")]
        if crate::dbgenv::dump_tree() {
            for (sb_idx, tree) in all_trees.iter().enumerate() {
                let bx = (sb_idx % sb_cols) * sb_size;
                let by = (sb_idx / sb_cols) * sb_size;
                dump_tree_leaves(tree, bx, by);
            }
        }

        // Task #95 goal 1 (odd true dims): the loop-restoration RU grid is
        // sized off the TRUE (coded) dims — C `whole_frame_rect` uses
        // frame_height / superres_upscaled_width, CEILING for chroma
        // (restoration.c:51-62). The aligned SB/mi grid drives everything else
        // in the walk; only the LR corner computation (`write_lr_for_sb` ->
        // `corners_in_sb`) and the search extent take the true dims. For
        // 8-aligned dims true == aligned, so this is byte-neutral.
        let lr_true_w = self.true_width as usize;
        let lr_true_h = self.true_height as usize;

        // The entropy walk as a re-runnable pass: decisions are already
        // fixed (trees + luma recon from MD; chroma decisions are pure
        // functions of the sources), so a second invocation reproduces the
        // identical symbol stream — plus, when `lr` is set, the per-SB
        // loop-restoration syntax C codes at the head of write_modes_sb
        // (entropy_coding.c:5500-5521; decoder decode_partition,
        // libaom decodeframe.c:1325-1341). The restoration search needs
        // the post-CDEF recon, so the tile must be re-written AFTER
        // deblock+CDEF when any plane signals wiener — C's pipeline order
        // (rest_process before the EC kernel) gives it the same view.

        // CDF CONTINUATION (`crate::port_frame_cdf`): the end-of-frame entropy
        // state this frame hands to whatever later frame names it in
        // `primary_ref_frame`. C saves it at
        // `packetization_process.c:741-744`, from `pcs->ec_info[tile_idx]->ec->fc`
        // — a loop over tiles that OVERWRITES, so the LAST tile's context is
        // what lands on the reference object. Single-tile frames (every cell in
        // the inter campaign) make that tile 0, which is also the
        // `context_update_tile_id` a decoder would use; the two can only differ
        // on a multi-tile frame, and that is recorded in the module docs rather
        // than silently resolved here.
        //
        // The walk runs up to THREE times (base, +CDEF syntax, +LR syntax) and
        // each rerun replaces `tile_data`, so the cell is overwritten every
        // time and ends holding the state of the walk whose bytes actually
        // ship. Anything else would save the CDFs of a bitstream nobody sent.
        // C `pcs->{intra,skip,hp}_coded_area` + `pcs->sb_{intra,skip}[]`, the
        // per-picture sums of every tile's `update_b` accumulators. `None`
        // unless the walk armed them, which is the VIDEO arm only — so an
        // allintra cell carries the same zeros it always did.
        //
        // A `RefCell` for the same reason `walk_end_cdfs` is one: the walk
        // runs inside a closure that borrows `&self`.
        let frame_coded_area: core::cell::RefCell<Option<CodedAreaAcc>> =
            core::cell::RefCell::new(None);
        let walk_end_cdfs: core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>> =
            core::cell::RefCell::new(None);
        #[allow(clippy::type_complexity)]
        // inline tuple documents the shape; a `type` alias would hide it
        // `recon_only` — see the call-site comment at the recon walk below:
        // the traversal and its reconstruction side effects are identical,
        // but no symbol is written and no CDF/coded-area state is kept.
        let run_entropy_walk = |lr: Option<&crate::restoration::FrameRestInfo>,
                                cdef_walk: Option<&crate::cdef::CdefPick>,
                                recon_only: bool|
         -> crate::EncodeResult<(
            Vec<u8>,
            crate::deblock::DeblockGeom,
            Vec<u8>,
            Vec<u8>,
            u8,
        )> {
            let (mut u_recon, mut v_recon) = if chroma.is_some() {
                (
                    svtav1_types::try_vec![128u8; ext_cbuf]?,
                    svtav1_types::try_vec![128u8; ext_cbuf]?,
                )
            } else {
                (Vec::new(), Vec::new())
            };
            // Per-4x4 block/TX/skip geometry for the deblocking edge walk,
            // recorded in coding order (== the decoder's parse order).
            // SHARED across every tile (absolute-position indexed,
            // deblock.rs): deblock/CDEF/LR apply post-tile-merge at frame
            // scope, unaffected by tile-row boundaries, so this — like
            // u_recon/v_recon above — is allocated ONCE and each tile's
            // walk below only ever writes its own rows into it.
            let mut deblock_geom = crate::deblock::DeblockGeom::new(w, h, lr_true_w, lr_true_h);
            // Mode/skip context tracking at 4x4 granularity — frame-wide
            // sizing (not tile-height): block coords (bx, by) passed to
            // encode_partition_tree are ABSOLUTE frame positions, so a
            // fresh EntropyCtx sized to the whole frame keeps those
            // indices valid across every tile while still giving the
            // C-exact "above unavailable at tile top" reset (a fresh
            // EntropyCtx starts every array at its unavailable/default
            // state — exactly entropy_coding_reset_neighbor_arrays,
            // ec_process.c:60-67).
            let w4 = w.div_ceil(4);
            let h4 = h.div_ceil(4);

            debug_assert_eq!(
                all_trees.len(),
                sb_cols * sb_rows,
                "tree count {} != SB count {}x{}={}",
                all_trees.len(),
                sb_cols,
                sb_rows,
                sb_cols * sb_rows,
            );

            // One independent entropy walk PER TILE ROW (task #86): C
            // resets every tile to a fresh FrameContext (`primary_ref_
            // frame == PRIMARY_REF_NONE` always holds for KEY frames) and
            // fresh neighbor-context arrays before its own arithmetic
            // coder starts (`reset_entropy_coding_picture`,
            // ec_process.c:72-117) — mirrored here by constructing fresh
            // writer/frame_ctx/coeff_fc/ectx/lr_refs per tile_idx.
            //
            // Task #96: and per tile COLUMN too. The tile group's tile
            // order is raster over the grid (row-major), which is the
            // order a decoder consumes the size-prefixed payloads in.
            let mut tile_bitstreams: Vec<Vec<u8>> = Vec::with_capacity(tile_grid.num_tiles());
            for tile_idx in 0..tile_grid.num_tiles() {
                // Feature 1: byte-inert cooperative-cancellation check, once per
                // tile of each entropy re-walk (this closure runs up to 3x).
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                let (tile_sb_row_start, tile_sb_row_end) =
                    tile_grid.row_span(tile_idx / tile_grid.tile_cols);
                let (tile_sb_col_start, tile_sb_col_end) =
                    tile_grid.col_span(tile_idx % tile_grid.tile_cols);

                let mut writer = crate::entropy::writer::AomWriter::new(n + 256);
                // CDF updates enabled — matches the frame header's disable_cdf_update=0.
                //
                // C `reset_entropy_coding_picture` (ec_process.c:101-112) does
                // this per TILE, and so does this loop: with
                // `primary_ref_frame != PRIMARY_REF_NONE` every tile starts
                // from the SAME restored reference context (not from the
                // previous tile's end state), which is what makes tiles
                // independently decodable.
                let (mut frame_ctx, mut coeff_fc) = match primary_ref_cdfs.as_ref() {
                    Some(prev) => (prev.fc.clone(), prev.coeff.clone()),
                    // C-exact coefficient CDFs for the base_q_idx bucket
                    // (svt_av1_default_coef_probs semantics) — qindex domain.
                    None => (
                        crate::entropy::context::FrameContext::new_default(),
                        crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                    ),
                };
                let mut ectx = EntropyCtx::new(
                    w4,
                    h4,
                    seq_tools.enable_filter_intra,
                    // The SAME bit the frame header writes — see
                    // `EntropyCtx::tx_mode_select`.
                    frame_tx_mode_select,
                    sc_derivation.allow_screen_content_tools,
                    self.bit_depth,
                    self.chroma_format
                        .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420),
                );
                // IBC chunk 1: arm the per-block use_intrabc flag coding
                // (C write_intrabc_info gate) from the same sc derivation
                // that set the FH bit — signaling and coding MUST agree or
                // the stream is undecodable.
                ectx.allow_intrabc = sc_derivation.allow_intrabc;
                // The frame-level inter syntax the pack's inter arm reads
                // (docs/INTER-ENCODE-PLAN.md §1s item 7). `None` on a key
                // frame, where the arm is unreachable.
                ectx.inter_syntax = inter_syntax_state.clone();
                // C `update_b`'s accumulators, armed on the VIDEO arm only —
                // C's own gate is `!pcs->scs->allintra` (coding_loop.c:1603),
                // a SEQUENCE flag, so both frame types of a video encode
                // accumulate and no allintra cell does. That is what keeps
                // the still envelope untouched by construction.
                ectx.coded_area =
                    matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }).then(|| {
                        CodedAreaAcc::new(
                            // C `frm_hdr->allow_high_precision_mv`, from the
                            // same derivation the header signals. FALSE on a
                            // key frame, where `inter_syntax_state` is `None`
                            // and no block carries an MV anyway.
                            inter_syntax_state
                                .as_ref()
                                .is_some_and(|st| st.allow_high_precision_mv),
                            sb_size,
                            w.div_ceil(sb_size),
                            h.div_ceil(sb_size),
                            h.div_ceil(4) as i32,
                            w.div_ceil(4) as i32,
                            // C `coding_loop.c:1748`:
                            // `scs->mfmv_enabled && slice_type != I_SLICE &&
                            //  ppcs->is_ref`. `mfmv_enabled` is
                            // `svt_aom_set_mfmv_config`'s sequence flag, which
                            // `md_config_inputs` already derives as
                            // `enc_mode <= ENC_M10`; `is_ref` is the picture
                            // decision's. On a key frame `md_config_signals`
                            // is `None`, which is C's `I_SLICE` arm.
                            md_config_signals.is_some()
                                && self.speed_config.preset <= 10
                                && pic_decision.as_ref().is_some_and(|p| p.is_ref),
                            inter_ref_frame_side,
                        )
                    });
                if let Some(env) = inter_mvp_env.clone() {
                    ectx.arm_inter_mvp(env);
                }
                // Task #86: this tile's own top row — gates "above"
                // availability in tx_size_ctx and (via chroma_pass's
                // encode_chroma_block_dc calls below) chroma prediction.
                ectx.tile_top_px = tile_sb_row_start * sb_size;
                // Task #96: ditto for this tile's own left column.
                ectx.tile_left_px = tile_sb_col_start * sb_size;
                // Same rect in LUMA mi, ends included, for the MD
                // prediction path. Ends are clamped to the frame exactly
                // like C's av1_tile_set_{col,row}
                // (`AOMMIN(mi_col_end, cm->mi_params.mi_cols)`).
                ectx.tile_mi = crate::intra_edge::TileMi {
                    mi_row_start: tile_sb_row_start * sb_size / 4,
                    mi_row_end: (tile_sb_row_end * sb_size / 4).min(h4),
                    mi_col_start: tile_sb_col_start * sb_size / 4,
                    mi_col_end: (tile_sb_col_end * sb_size / 4).min(w4),
                };
                // [SVT_HDR_MODE] arm per-SB delta-q: prev starts at the FH base
                // (C prev_qindex tile-init); uniform plan = every SB at base.
                if let Some(res) = delta_q_res_signal {
                    ectx.delta_q_state = Some((res, i32::from(base_qindex), sb_size));
                    ectx.delta_q_sb_qindex = i32::from(base_qindex);
                }
                let mut chroma_pass = sb_chroma_owned.as_ref().map(|(u_src, v_src)| ChromaPass {
                    u_src: u_src.as_slice(),
                    v_src: v_src.as_slice(),
                    u_recon: &mut u_recon,
                    v_recon: &mut v_recon,
                    stride: cw,
                    qindex_u,
                    qindex_v,
                    qm_u: qm_levels[1],
                    qm_v: qm_levels[2],
                    c_quant: c_quant.as_deref(),
                    ref_uv: ref_padded_luma
                        .and_then(|p| p.uv.as_ref())
                        .map(|(u, v)| (u, v)),
                    sb_size,
                    frame_w: w,
                    frame_h: h,
                });
                // LR tap references reset at the tile start (C
                // svt_av1_reset_loop_restoration, ec_process.c:199).
                let mut lr_refs = crate::restoration::LrWalkRefs::default();
                let mut prev_sb_row = usize::MAX;

                for sb_row in tile_sb_row_start..tile_sb_row_end {
                    // Feature 1: byte-inert cooperative-cancellation check, once
                    // per SB row of the entropy walk.
                    if stop.may_stop() {
                        stop.check()
                            .map_err(EncodeError::from)
                            .map_err(whereat::at)?;
                    }
                    for sb_col in tile_sb_col_start..tile_sb_col_end {
                        crate::stop_check(&stop)?;
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let tree = &all_trees[sb_idx];
                        // Per-SB delta-q / TPL: the SB's qindex drives the
                        // delta symbol (only when `delta_q_present` armed the
                        // state), the chroma dequant, and (via the search,
                        // which used the same map) the coded coefficients.
                        // `md_sb_qindex` is the QUANT map — live under
                        // `r0_delta_qp_md` even when nothing is signalled.
                        if let Some(plan) = md_sb_qindex {
                            let sbq = i32::from(plan.sb_qindex[sb_idx]);
                            if delta_q_plan.is_some() {
                                ectx.delta_q_sb_qindex = sbq;
                            }
                            if let Some(cp) = chroma_pass.as_mut() {
                                cp.qindex_u =
                                    (sbq + i32::from(chroma_deltas.u_ac)).clamp(0, 255) as u8;
                                cp.qindex_v =
                                    (sbq + i32::from(chroma_deltas.v_ac)).clamp(0, 255) as u8;
                            }
                        }
                        let bx = sb_col * sb_size;
                        let by = sb_row * sb_size;

                        // Reset left partition context at the start of each SB row,
                        // matching rav1d's per-tile-row left context reset.
                        if sb_row != prev_sb_row {
                            ectx.reset_left_for_sb_row();
                            prev_sb_row = sb_row;
                        }

                        // Arm the per-SB cdef_idx emission (C write_cdef resets
                        // cdef_transmitted at the SB's top-left, then the first
                        // non-skip block emits `cdef_bits` literal bits). 64x64
                        // SBs: one filter block per SB.
                        // C write_cdef resets `cdef_transmitted[4]` at the
                        // SB top-left, then each 64x64 quadrant's first
                        // non-skip block emits its own literal. The strength
                        // is read off the B64 grid (C's mbmi at
                        // `(mi & ~15)`), which is what `fb_idx` is indexed
                        // by — NOT by the SB grid. At SB64 the two grids
                        // coincide and only quadrant 0 is ever used, so this
                        // reduces exactly to the previous
                        // `fb_idx[sb_row * nhfb + sb_col]`.
                        ectx.cdef_sb = cdef_walk.and_then(|p| {
                            (p.bits > 0).then(|| {
                                let fb_per_sb = sb_size / 64;
                                let mut strengths = [0u8; 4];
                                for (q, st) in strengths.iter_mut().enumerate() {
                                    let fbc = sb_col * fb_per_sb + (q & 1);
                                    let fbr = sb_row * fb_per_sb + (q >> 1);
                                    // Off-frame quadrants of a partial SB
                                    // code nothing, so their slot is never
                                    // read; 0 keeps the lookup total.
                                    *st = p
                                        .fb_idx
                                        .get(fbr * p.nhfb + fbc)
                                        .copied()
                                        .filter(|_| fbc < p.nhfb)
                                        .unwrap_or(0);
                                }
                                CdefSbState {
                                    bits: p.bits,
                                    strengths,
                                    transmitted: [false; 4],
                                    sb128: sb_size == 128,
                                }
                            })
                        });

                        // Loop-restoration coefficients for every RU cornered in
                        // this SB — BEFORE the SB's partition tree, matching the
                        // decoder's read order.
                        if let Some(info) = lr {
                            crate::restoration::write_lr_for_sb(
                                &mut writer,
                                &mut frame_ctx,
                                info,
                                &mut lr_refs,
                                (by / 4) as i32,
                                (bx / 4) as i32,
                                (sb_size / 4) as i32,
                                // TRUE dims: the RU grid / corner computation is
                                // coded off the coded frame size, not the aligned
                                // grid (byte-neutral when 8-aligned).
                                lr_true_w,
                                lr_true_h,
                                chroma.is_none(),
                                self.superres_denom,
                            );
                        }

                        encode_partition_tree(
                            tree,
                            &mut writer,
                            &mut frame_ctx,
                            &mut coeff_fc,
                            base_qindex,
                            &mut ectx,
                            is_key,
                            bx,
                            by,
                            &mut chroma_pass,
                            &mut deblock_geom,
                            recon_only,
                        );
                    }
                }

                tile_bitstreams.push(writer.done().to_vec());
                // C `enc_dec_process.c:3166-3170`: each EncDec context's
                // coded-area totals are summed into the picture under
                // `pcs->intra_mutex`. One tile per context here.
                // The recon-only walk keeps NO coded-area / CDF state: its
                // sums would be re-added by the bit-producing walk that
                // follows, inflating `intra_area`/`skip_area`/`hp_area` past
                // C's single `update_b` pass. (This is also the latent fix
                // for the pre-split walks double-merging on re-walk frames —
                // only ONE bit-producing walk now runs per frame.)
                if !recon_only && let Some(acc) = ectx.coded_area.as_ref() {
                    let mut slot = frame_coded_area.borrow_mut();
                    match slot.as_mut() {
                        Some(f) => f.merge(acc),
                        None => *slot = Some(acc.clone()),
                    }
                }
                // See `walk_end_cdfs`: overwritten per tile AND per walk, so
                // it ends holding the last tile of the last walk — C's own
                // "last tile wins" save order.
                if !recon_only {
                    *walk_end_cdfs.borrow_mut() = Some(crate::port_frame_cdf::FrameCdfs {
                        fc: frame_ctx,
                        coeff: coeff_fc,
                    });
                }
            }

            // Shared derivation for the frame header's tile_info() trailer
            // AND the tile group's size prefixes — computed once from the
            // real per-tile byte lengths so the two can never disagree
            // (see tile_size_bytes_minus_1_for's doc comment).
            let non_last_lens: Vec<usize> = tile_bitstreams
                [..tile_bitstreams.len().saturating_sub(1)]
                .iter()
                .map(|t| t.len())
                .collect();
            let tile_size_bytes_minus_1 =
                crate::entropy::obu::tile_size_bytes_minus_1_for(&non_last_lens);

            Ok((
                crate::entropy::obu::build_tile_group_multi(
                    &tile_bitstreams,
                    tile_size_bytes_minus_1,
                ),
                deblock_geom,
                u_recon,
                v_recon,
                tile_size_bytes_minus_1,
            ))
        };
        // Whether the walk's side effects (the recon planes + deblock
        // geometry) are consumed downstream: the CDEF search (when this
        // preset runs it), the loop-restoration search, both preset <= 6 —
        // or the LR stripe-boundary save; (2) the caller, via `last_recon*` /
        // a later frame predicting from this recon through the DPB. When NONE
        // of those exist the filtered pixels are dead: nothing reads them and
        // the bitstream is already written. C behaves identically (its
        // preset-10 profile contains zero CDEF/LPF samples for
        // byte-identical output).
        //
        // Byte-inertness is measured, not assumed: skipping the two apply
        // passes changed 0/90 cells at presets 7..13 and 13/36 at presets 2/6
        // (tools/byteid_fingerprint.sh, {64,128,256} x qp{20,40,55} x
        // {gradient,uniform}) — see benchmarks/perf_postfilter_2026-08-11.meta.
        //
        // Hoisted above the entropy walk: the same condition selects the
        // walk SPLIT below — when side effects are consumed, a cheap
        // recon-only walk produces them and the ONE bit-producing walk runs
        // after every filter parameter is known (C order: rest_process
        // before the EC kernel), so a CDEF/LR re-walk never discards a full
        // pass of symbol work.
        let postfilter_consumed = seq_tools.enable_restoration
            || crate::cdef::allintra_preset_uses_cdef_search(self.speed_config.preset)
            || self.recon_output
            // A later frame may predict from this recon via the DPB. Only an
            // all-key sequence (`intra_period == 1`) provably has no such
            // reader — every `self.dpb.get(..)` site is gated on `!is_key`.
            || !is_single_frame;
        let (
            mut tile_data,
            mut deblock_geom,
            mut u_recon,
            mut v_recon,
            mut tile_size_bytes_minus_1,
        ) = if postfilter_consumed {
            // Recon-only walk: identical traversal and reconstruction side
            // effects (the chroma recon planes + deblock geometry the
            // searches below read), but no symbols — its tile bytes would be
            // discarded whenever a CDEF/LR re-walk followed, and the
            // bit-producing walk now runs once, at the end, armed with every
            // syntax the searches picked.
            let (_t, geom, u, v, _s) = run_entropy_walk(None, None, true)?;
            (Vec::new(), geom, u, v, 0)
        } else {
            // Nothing downstream reads the walk's side effects: a single
            // full walk is the final bitstream (unchanged fast path).
            run_entropy_walk(None, None, false)?
        };

        crate::stop_check(&stop)?;

        // Step 6a: Deblocking — pick the levels the frame header will
        // signal (C svt_av1_pick_filter_level_by_q closed form) and apply
        // the filter decoder-exactly to the OUTPUT reconstruction. The
        // prediction sources are untouched: intra prediction read the live
        // unfiltered buffers (tile_frame_recon for luma, u/v_recon during
        // the walk) and the walk is complete by now — the filtered copy
        // becomes last_recon and the DPB frame, exactly the decoder's
        // split (it predicts intra from unfiltered pixels and stores the
        // filtered frame for output/reference).
        //
        // Inter frames keep levels 0 (write_inter_frame signals 0): the
        // q-based picker is only wired for key frames, and signaling
        // nothing while applying nothing stays self-consistent.
        //
        // Preset split (C get_dlf_level_allintra, enc_mode_config.c:2214,
        // fast_decode 0): presets <= M5 get dlf_level 1/2 -> sb_based_dlf=0
        // -> dlf_process runs svt_av1_pick_filter_level with
        // LPF_PICK_FROM_FULL_IMAGE (real SSE trials on the post-encode
        // recon); presets >= M6 get dlf_level 5 -> sb_based_dlf=1 -> the
        // LPF_PICK_FROM_Q closed form. early_exit_convergence is 0 at
        // dlf_level 1 (<= M3) and 1 at dlf_level 2 (M4/M5).
        // Pre-DLF recon dump (SVTAV1_RECONDBG) — before the preset split so
        // it fires at every preset (#90); matches C's dlf_process.c:101
        // dump point (recon final, not yet deblocked).
        #[cfg(feature = "std")]
        {
            let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
            crate::deblock::recondbg_dump(
                &encode_input,
                su,
                sv,
                &recon,
                &u_recon,
                &v_recon,
                w,
                h,
                // Debug dump assumes 4:2:0 dims internally (`width/2`) —
                // dump luma only at 4:4:4 rather than mis-shape chroma.
                filter_chroma,
            );
        }
        #[cfg(feature = "std")]
        if let Ok(prefix) = std::env::var("SVTAV1_RECON10_BIN") {
            if let (Some(y), Some((u, v))) =
                (self.last_recon10_y.as_ref(), self.last_recon10_uv.as_ref())
            {
                for (plane, samples) in [y, u, v].into_iter().enumerate() {
                    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                    std::fs::write(format!("{prefix}.p{plane}"), bytes)
                        .expect("write native pre-filter reconstruction");
                }
            }
        }
        // ---- bd10 post-filter canvas ------------------------------------
        // At 10 bits C runs the WHOLE post-MD filter chain on the 16-bit
        // recon against the 16-bit source, and the THREE SEARCHES in that
        // chain — the deblock LEVEL search, the CDEF strength search and the
        // Wiener LR taps — each write frame-header syntax. Running them at 8
        // bits is therefore a bitstream divergence, not just a recon
        // approximation. This carries the true 10-bit planes through the
        // chain in parallel with the u8 ones; the u8 chain still produces the
        // output/DPB recon, unchanged, and bd8 never enters any of it.
        //
        // Built BEFORE the LF-level decision because the deblock-level search
        // reads the UNFILTERED recon (each trial filters a scratch copy — C
        // re-instates the frame from `temp_lf_recon_buffer` after every
        // try_filter_frame, deblocking_filter.c:828).
        //
        // `Some` iff this frame produced a complete 10-bit recon (the bd10
        // re-encode gate above). When it declined, the searches fall back to
        // the u8 chain exactly as before.
        let mut recon10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = match (
            self.bit_depth,
            self.last_recon10_y.as_ref(),
            self.last_recon10_uv.as_ref(),
        ) {
            (10, Some(y10), Some((u10, v10))) if chroma.is_some() => {
                Some((y10.clone(), u10.clone(), v10.clone()))
            }
            (10, Some(y10), _) if chroma.is_none() => Some((y10.clone(), Vec::new(), Vec::new())),
            _ => None,
        };
        // ---- deblock signal derivation inputs (C enc_mode_config.c) -----
        //
        // `dlf_enc_mode`: C starts from `pcs->enc_mode` and, when
        // `enable_dlf_flag == 2`, re-derives as if three presets lower
        // (`AOMMAX(ENC_MR, enc_mode - 3)`). The port carries no
        // `enable_dlf_flag` config — it is always 1 — so the adjustment is
        // translated but cannot fire; `EncMode` is `int8_t`-ranged with
        // `ENC_MR = -1`, hence the `i8`.
        //
        // `pcs->enc_mode` is the ARM-CLAMPED preset: C rewrites
        // `scs->static_config.enc_mode` once in `svt_av1_enc_set_parameter`
        // (`enc_handle.c:4415-4436`) — allintra `> M9 -> M9`, video non-RTC
        // `> M11 -> M11` — so every downstream ladder reads the clamped value.
        // MEASURED: without the clamp, `get_dlf_level_default(12)` falls into
        // the `else` arm and returns 0 (deblock OFF) where C, seeing M11,
        // returns 6 on a base picture -> `sb_based_dlf` -> the by-q closed
        // form -> `loop_filter_level = 3`. That one field is what made every
        // video-mode key frame at preset 12/13 exactly ONE byte short of C's
        // on all five synthetic content classes at once.
        let dlf_enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset) as i8;
        // `scs->static_config.fast_decode`. The port has no fast-decode
        // config; C's default is 0. Both dlf ladders take their first arm on
        // `fast_decode <= 1`, so the resolution below is currently unread —
        // it is passed faithfully so the fast-decode arm stays correct if that
        // config ever lands.
        const DLF_FAST_DECODE: u8 = 0;
        // `ppcs->input_resolution` — `svt_aom_derive_input_resolution` over
        // `scs->max_input_luma_width * scs->max_input_luma_height`, which is
        // the source size padded up to MIN_BLOCK_SIZE (8) on each axis
        // (`enc_handle.c:3918-3930`, then `:3992`).
        let dlf_resolution = crate::port_enc_mode_config::ResolutionRange::from_luma_area(
            self.true_width.next_multiple_of(8) * self.true_height.next_multiple_of(8),
        );
        // `ppcs->temporal_layer_index` / `ppcs->is_highest_layer`. A KEY frame
        // is always temporal layer 0, and C's
        // `is_highest_layer = (temporal_layer_index == hierarchical_levels) &&
        // hierarchical_levels != 0` (`pd_process.c:5560`) is therefore false
        // for it at every hierarchy depth INCLUDING flat (the second clause
        // exists precisely so a flat GOP does not mark every picture highest).
        // Written out rather than folded to constants so the inter chunks
        // inherit the rule instead of re-deriving it.
        //
        // INTER (2026-09-02): this is now the REAL `temporal_layer`, not the
        // key frame's literal 0. On the campaign's flat low-delay-P GOP it is
        // still 0 for every picture — which is the point: `is_highest_layer`
        // ANDs in `hierarchical_levels != 0` (`pd_process.c:5560`, its own
        // comment says "for flat, set is_highest_layer to false to avoid using
        // aggressive settings for all pictures"), so `is_not_last_layer` is
        // TRUE on a flat GOP and `get_dlf_level_default` gives 3 at <= M6 and
        // 6 at <= M9. Both are ENABLED levels; the port used to signal 0.
        let dlf_temporal_layer_index: u8 = temporal_layer;
        let dlf_is_base = dlf_temporal_layer_index == 0;
        let dlf_is_highest_layer =
            crate::port_picstruct::is_highest_layer(dlf_temporal_layer_index, frame_hier);
        let dlf_is_not_last_layer = u8::from(!dlf_is_highest_layer);

        // IBC (chunk 1): C kills the deblock filter at SIGNAL-DERIVATION on
        // IntraBC frames — `dlf_level` stays 0 unless `enable_dlf_flag &&
        // frm_hdr->allow_intrabc == 0` (enc_mode_config.c:10117-10127), so
        // neither the level pick nor the frame apply runs and the FH codes
        // no loop-filter params (obu.rs suppresses them on the same flag).
        // Only sc_class5 presets <= 4 frames take this arm.
        // Coded-lossless: `dlf_ctrls.enabled = 0`, `cdef_level = 0` and (at
        // AllLossless, which every unscaled lossless frame is) `enable_restoration
        // = 0` (md_config_process.c:1022-1035): no search, no application, and
        // the frame header carries none of the three (chunk 1). Same shape as
        // the IntraBC frame-level suppression this predicate already handles.
        //
        // WHICH picker runs is not a preset rule but a two-step C derivation
        // that FORKS on `scs->allintra` (`md_config_process.c:924-930`):
        //
        //   allintra -> svt_aom_sig_deriv_mode_decision_config_allintra
        //               -> get_dlf_level_allintra  (enc_mode_config.c:1540)
        //   video    -> svt_aom_sig_deriv_mode_decision_config_default
        //               -> get_dlf_level_default   (enc_mode_config.c:1466)
        //
        // and then maps that LEVEL through `svt_aom_set_dlf_controls` (:1561).
        // `sb_based_dlf` is what selects the picker: set, `enc_dec_process.c
        // :3132` runs LPF_PICK_FROM_Q (the closed form); clear,
        // `dlf_process.c:97` runs LPF_PICK_FROM_FULL_IMAGE (the SSE search).
        //
        // Before the inter campaign the port encoded the ALLINTRA resolution of
        // that chain inline (`preset <= 5` -> search, else closed form, with
        // `early_exit_convergence` 0 below M4). That flattening is exactly
        // right for the still envelope and is reproduced bit-for-bit by the
        // table below — but it was gated on `is_single_frame`, so a VIDEO-mode
        // key frame fell through to the closed form, which is not the arm C
        // takes. At preset 6 / qindex 67 that signalled `loop_filter_level = 3`
        // where C signals 0.
        // C `pcs->ref_skip_percentage` (`rc_process.c:96`, `rc_init_frame_stats`
        // — the mean `skip_coded_area` of the nearest L0/L1 references,
        // I-slice refs counting 0). Feeds `dlf_level_modulation` inside
        // `get_dlf_level_default`; a high-skip reference set is what shuts
        // the loop filter off on well-predicted hierarchical frames. 0 on a
        // key frame (no refs — C's I-slice early-out).
        let dlf_ref_skip_percentage = pic_decision.as_ref().map_or(0, |p| {
            if is_key {
                0
            } else {
                crate::port_rc_process::get_ref_skip_percentage(
                    crate::port_rc_process::SliceType::B,
                    u8::try_from(p.ref_list1_count_try).unwrap_or(u8::MAX),
                    (p.ref_list0_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[0] as usize))
                        .flatten()
                        .as_ref(),
                    (p.ref_list1_count_try > 0)
                        .then(|| ref_obj_stats(&self.dpb, p.rps.ref_dpb_index[4] as usize))
                        .flatten()
                        .as_ref(),
                )
            }
        });
        let dlf_level = if is_single_frame {
            // `get_dlf_level_allintra(dlf_enc_mode, fast_decode, resolution)`.
            crate::port_enc_mode_config::leaf::get_dlf_level_allintra(
                dlf_enc_mode,
                DLF_FAST_DECODE,
                dlf_resolution,
            )
        } else {
            // `get_dlf_level_default(pcs, dlf_enc_mode, is_not_last_layer,
            //  fast_decode, resolution, is_base)`.
            //
            // `coeff_lvl` is read only in the M10..M11 arm, and there both
            // branches yield 6 when `is_base` — which every KEY frame is
            // (`temporal_layer_index == 0`) — so the value passed cannot
            // change a key frame's level. `ref_skip_percentage` feeds
            // `dlf_level_modulation`, which C runs only when `!is_base`;
            // modulation mode 3 can zero an otherwise-enabled level when the
            // references are >95% skip (MEASURED: hier-2 LD-CBR poc6, a TL1
            // frame whose level-6 became 0 on refs at 100% skip).
            crate::port_enc_mode_config::leaf::get_dlf_level_default(
                dlf_enc_mode,
                dlf_is_not_last_layer,
                DLF_FAST_DECODE,
                dlf_resolution,
                dlf_is_base,
                crate::port_enc_mode_config::InputCoeffLvl::Normal,
                dlf_ref_skip_percentage,
            )
        };
        // C's `default:` arm is `assert(0)`; the port refuses rather than
        // inventing a control set.
        let dlf_ctrls = crate::port_enc_mode_config::ctrls::set_dlf_controls(dlf_level).ok_or(
            EncodeError::UnsupportedConfig("dlf level outside svt_aom_set_dlf_controls' 0..=7"),
        )?;

        // C `ppcs->ref_frame_type_arr[0 .. tot_ref_frame_types]`, restricted
        // to the SINGLE-reference entries (`rf[1] == NONE_FRAME`) — the only
        // ones either picker reads. `set_all_ref_frame_type`
        // (`pd_process.c:1044`, ported at `port_picstruct`) lays list 0's
        // singles down first, then list 1's, then the compounds, so the
        // singles are exactly the first `ref_list0_count_try +
        // ref_list1_count_try` entries and the DPB slot for each is
        // `rps.ref_dpb_index[LAST + i]` / `[BWD + i]`.
        //
        // EMPTY on a key frame, which is what makes `tot_ref_frame_types == 0`
        // and leaves every reference-dependent branch inert.
        let dlf_refs: alloc::vec::Vec<crate::dlf_arm::RefDlfState> = match pic_decision.as_ref() {
            Some(pic) if !is_key => {
                let mut v = alloc::vec::Vec::new();
                let mut push = |idx: usize| {
                    if let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[idx] as usize) {
                        v.push(crate::dlf_arm::RefDlfState {
                            filter_level: [
                                i32::from(rf.lf_levels[0]),
                                i32::from(rf.lf_levels[1]),
                                i32::from(rf.lf_levels[2]),
                                i32::from(rf.lf_levels[3]),
                            ],
                            dlf_dist_dev: rf.dlf_dist_dev,
                        });
                    }
                };
                for i in 0..usize::from(pic.ref_list0_count_try) {
                    push(crate::port_picstruct::LAST + i);
                }
                for i in 0..usize::from(pic.ref_list1_count_try) {
                    push(crate::port_picstruct::BWD + i);
                }
                v
            }
            _ => alloc::vec::Vec::new(),
        };
        // C `average_me_sad` (`deblocking_filter.c:982-986`): the MEAN of
        // `ppcs->rc_me_distortion[b64]` over `b64_total_count`, which
        // `motion_estimation.c:2778` fills with the b64's 8x8 SAD sum at
        // <= 480p and its 16x16 sum above. Read ONLY by `me_based_dlf_skip`,
        // i.e. only at a `dlf_level` whose controls set
        // `zero_filter_strength_lvl` (5 / 6 / 7), and never on an I_SLICE.
        //
        // MEASURED 2026-09-02, and it is why this is not optional at p8:
        // `uniform` content translates exactly, so every b64's SAD is 0 and C
        // writes `loop_filter_level = 0` on every one of its inter frames
        // DESPITE a nonzero reference level — while `gradient 16x16 q40 p8`
        // clears the threshold and C writes 9. `screen 16x16 q40 p8` lands
        // BETWEEN the two thresholds and C writes luma 9 with chroma 0.
        let dlf_avg_me_sad: u32 = match frame_me.as_ref() {
            Some(me) if !me.per_b64.is_empty() => {
                let total: u64 = me
                    .per_b64
                    .iter()
                    .map(|b| u64::from(b.rc_me_distortion))
                    .sum();
                (total / me.per_b64.len() as u64) as u32
            }
            _ => 0,
        };
        let dlf_pick_inputs = crate::dlf_arm::DlfPickInputs {
            ctrls: dlf_ctrls,
            frame_type_is_key: is_key,
            // `pcs->slice_type == I_SLICE`. This port has no intra-only
            // non-key frame, so it equals `is_key`; C reads two fields and so
            // does `DlfPickInputs`.
            is_intra_slice: is_key,
            // `frame_is_boosted` / `frame_is_leaf` come from the picture
            // decision's `update_type`, the same source `cdef_frame_is_boosted`
            // below already uses. A KEY frame is intra-only and KF_UPDATE, so
            // boosted is true and leaf is false either way.
            frame_is_boosted: pic_decision
                .as_ref()
                .map_or(is_key, crate::port_picstruct::frame_is_boosted),
            frame_is_leaf: pic_decision
                .as_ref()
                .is_some_and(|pic| pic.update_type == crate::port_picstruct::FrameUpdateType::Lf),
            hierarchical_levels: frame_hier,
            temporal_layer_index: dlf_temporal_layer_index,
            input_resolution: dlf_resolution,
            refs: &dlf_refs,
            avg_me_sad: dlf_avg_me_sad,
            base_qindex,
            bit_depth: self.bit_depth,
        };
        // C `dlf_process.c:89-91` seeds all three to -1 ("not computed"); only
        // the non-SB-based path overwrites them, so a frame that takes the
        // by-q arm genuinely has no measurement and its `dlf_dist_dev` must
        // stay -1 for the NEXT frame to skip rather than average.
        let mut dlf_zero_filt_sse: i64 = -1;
        let mut dlf_best_filt_sse: i64 = -1;
        let mut dlf_full_image_ran = false;
        let mut lf_levels = if sc_derivation.allow_intrabc || coded_lossless {
            crate::deblock::LfLevels::default()
        } else {
            if dlf_ctrls.enabled == 0 {
                // `enable_dlf_flag == 0` or a level-0 ladder entry: C neither
                // picks nor applies, and the header codes zeros.
                crate::deblock::LfLevels::default()
            } else if dlf_ctrls.sb_based_dlf == 0 {
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let early_exit_convergence = i32::from(dlf_ctrls.early_exit_convergence);
                let pick = match recon10.as_ref() {
                    // bd10: search on the true 10-bit unfiltered recon
                    // against the true 10-bit source, with the highbd lpf
                    // kernels and `svt_full_distortion_kernel16_bits`
                    // (C `picture_sse_calculations` at is_16bit,
                    // deblocking_filter.c:768).
                    Some((y10, u10, v10)) => {
                        let sh = (self.bit_depth - 8) as u32;
                        let widen = |p: &[u8]| -> Vec<u16> {
                            p.iter().map(|&s| (s as u16) << sh).collect()
                        };
                        // Task #6 chunk 2: the deblock level search compares the
                        // 10-bit recon against the 10-bit SOURCE. With a native
                        // HBD source that is the caller's real u16 (so the low 2
                        // bits participate in the SSE that picks the level);
                        // otherwise the same `u8 << sh` widening as before.
                        let (sy10, su10, sv10) = match hbd_source.as_ref() {
                            Some(hbd) => {
                                hbd_used = true;
                                (hbd.y.clone(), hbd.u.clone(), hbd.v.clone())
                            }
                            None => (widen(&encode_input), widen(su), widen(sv)),
                        };
                        let input = crate::deblock::DlfSearchInput::<u16> {
                            sharpness: lf_sharp_eff,
                            y_src: &sy10,
                            u_src: &su10,
                            v_src: &sv10,
                            y_recon: y10,
                            u_recon: u10,
                            v_recon: v10,
                            width: w,
                            height: h,
                            chroma_420: filter_chroma,
                            geom: &deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_image_with_stop(
                            &input,
                            &dlf_pick_inputs,
                            &stop,
                        )?
                    }
                    None => {
                        let input = crate::deblock::DlfSearchInput::<u8> {
                            sharpness: lf_sharp_eff,
                            y_src: &encode_input,
                            u_src: su,
                            v_src: sv,
                            y_recon: &recon,
                            u_recon: &u_recon,
                            v_recon: &v_recon,
                            width: w,
                            height: h,
                            chroma_420: filter_chroma,
                            geom: &deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_image_with_stop(
                            &input,
                            &dlf_pick_inputs,
                            &stop,
                        )?
                    }
                };
                dlf_full_image_ran = true;
                dlf_zero_filt_sse = pick.zero_filt_sse;
                dlf_best_filt_sse = pick.best_filt_sse;
                pick.levels
            } else {
                // `sb_based_dlf = 1` -> LPF_PICK_FROM_Q
                // (`enc_dec_process.c:3132`). This path computes no SSE, so
                // `dlf_dist_dev` stays -1 and the NEXT frame skips this one
                // rather than reading a zero.
                crate::dlf_arm::pick_filter_level_by_q(&dlf_pick_inputs)
            }
        };
        // The in-loop post-filters (deblock -> CDEF) apply only when
        // `postfilter_consumed` says the filtered pixels have a reader —
        // see the derivation comment at the entropy walk above.
        if self.recon_output {
            self.last_recon_unfiltered = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        // C `dlf_process.c:103-112`, and it is NOT dead code on the inter
        // path even though the port's key-frame comment used to say the guard
        // "can never fire": `search_filter_level` seeds its hill climb at
        // `last_frame_filter_level`, which is 0 on a key frame (so `ss_err[0]`
        // is always evaluated and `zero_filt_sse` is always set) but is the
        // REFERENCE'S level on an inter frame, so level 0 can go unvisited.
        // C then measures the unfiltered SSE explicitly, and shuts the filter
        // off if filtering did not actually beat not filtering.
        //
        // The ref-average arms reach here with BOTH sentinels intact (they
        // never call the search at all), which is exactly the state where C
        // recomputes `zero` and leaves `best` for after the filter.
        if dlf_full_image_ran && dlf_zero_filt_sse == -1 && lf_levels.any() {
            dlf_zero_filt_sse = crate::deblock::plane_sse(&encode_input, &recon, w, h);
            if dlf_best_filt_sse != -1 && dlf_zero_filt_sse <= dlf_best_filt_sse {
                lf_levels = crate::deblock::LfLevels::default();
            }
        }
        // 4:4:4 staged bring-up (`filter_chroma` above): signal chroma
        // loop-filter levels 0 so the decoder skips chroma filtering — the
        // ss=0 edge kernels are not yet ported. Luma is untouched; a level-0
        // chroma entry is identity in every `lpf_params` gate.
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            lf_levels.levels[2] = 0;
            lf_levels.levels[3] = 0;
        }
        if let Some((y10, u10, v10)) = recon10.as_mut()
            && lf_levels.any()
            && postfilter_consumed
        {
            crate::deblock::apply_deblock_frame_hbd_with_stop(
                y10,
                u10,
                v10,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff,
                self.bit_depth,
                &stop,
            )?;
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::lfdbg() {
            std::eprintln!(
                "LFDBG key={} lf_levels={:?} sharp={} postfilter_consumed={}",
                u8::from(is_key),
                lf_levels.levels,
                lf_sharp_eff,
                postfilter_consumed,
            );
        }
        if lf_levels.any() && postfilter_consumed {
            crate::deblock::apply_deblock_frame_with_stop(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff, // = signaled loop_filter_sharpness
                &stop,
            )?;
            // C `dlf_process.c:114-117`: the FILTERED SSE, measured after
            // `svt_av1_loop_filter_frame`, when the search did not leave one.
            if dlf_full_image_ran && dlf_best_filt_sse == -1 {
                dlf_best_filt_sse = crate::deblock::plane_sse(&encode_input, &recon, w, h);
            }
        }
        // C `pcs->dlf_dist_dev` (`dlf_process.c:119`) — the per-mille SSE
        // improvement this frame's own deblock bought, which the NEXT frame
        // reads off the reference object to decide whether to filter at all.
        // -1 ("never computed") everywhere the SB-based arm ran, per
        // `dlf_process.c:92`.
        let dlf_dist_dev = if dlf_full_image_ran {
            crate::dlf_arm::dlf_dist_dev(lf_levels, dlf_zero_filt_sse, dlf_best_filt_sse)
        } else {
            -1
        };

        crate::stop_check(&stop)?;

        // Step 6a': CDEF — decoder order is deblock -> CDEF (-> restoration,
        // unported). Key frames signal the qp-picked strengths
        // (svt_pick_cdef_from_qp intra branch) and apply the decoder-exact
        // frame pass (libaom av1_cdef_frame) to the SAME output copy; the
        // per-64x64 cdef_idx costs ZERO arithmetic-coder bits because
        // cdef_bits = 0 (libaom read_cdef does aom_read_literal(r, 0) —
        // a no-iteration loop, bitreader.h:161 — so the entropy walk needs
        // no syntax change). Inter frames signal zero strengths and apply
        // nothing — consistent.
        // IBC (chunk 1): C kills CDEF at SIGNAL-DERIVATION on IntraBC frames
        // — `if (!scs->seq_header.cdef_level || frm_hdr->allow_intrabc)
        // cdef_search_level = 0` (allintra: enc_mode_config.c:2396-2398) and
        // cdef_process re-zeroes cdef_params (cdef_process.c:692-697). The
        // all-zero-strength default makes apply_cdef_frame a structural
        // no-op and cdef_bits stays 0 (no per-SB syntax, no FH params).
        //
        // WHICH CDEF policy runs is the same two-step C derivation the deblock
        // levels above take, forking on `scs->allintra`:
        //
        //   allintra -> svt_aom_sig_deriv_multi_processes_allintra
        //               -> its cdef_search_level ladder (enc_mode_config.c:2396)
        //   video    -> svt_aom_sig_deriv_multi_processes_default
        //               -> its cdef_search_level ladder (:2083)
        //
        // and then maps that LEVEL through `set_cdef_search_controls` (:891).
        // `use_qp_strength` is what selects the fast path: level 10 sets it,
        // levels 1..=9 clear it and carry a candidate set to RD-search.
        //
        // Before the inter campaign the port encoded the ALLINTRA resolution of
        // that chain inline (`preset <= 6` -> search, else the qp closed form,
        // with the candidate set flattened per preset). That flattening is
        // exactly right for the still envelope and is reproduced entry for
        // entry by the ladder below (`cdef.rs`'s
        // `allintra_flattening_matches_the_ladder`) — but it was gated on
        // `is_single_frame`, so a VIDEO-mode key frame fell through to the qp
        // fast path, which is not the arm C takes. C's video ladder gives
        // `is_base ? 5 : 6` at M6..M7 and 7 above, i.e. a video key frame
        // SEARCHES at every preset; at preset 6 / qindex 67 the port signalled
        // y=(pri 1, sec 0) / uv=(pri 1, sec 0) where C signals y=(0, 2) /
        // uv=(7, 0) — the level-5 candidate set {0, 28, 60} + {2, 30, 62}.
        //
        // `scs->seq_header.cdef_level` is 1 in this port (obu.rs writes
        // `enable_cdef = 1` unconditionally) and there is no `--cdef-level`
        // config, so both ladders take their derived arm.
        const SEQ_CDEF_LEVEL: u8 = 1;
        // `scs->static_config.fast_decode` — the port carries no fast-decode
        // config and C's default is 0, the same constant the deblock ladder
        // above passes.
        const CDEF_FAST_DECODE: u8 = 0;
        let cdef_level = if is_single_frame {
            crate::port_enc_mode_config::cdef_search::cdef_search_level_allintra(
                self.speed_config.preset as i8,
                CDEF_FAST_DECODE,
                dlf_resolution,
                SEQ_CDEF_LEVEL,
                sc_derivation.allow_intrabc,
                crate::port_enc_mode_config::cdef_search::CONFIG_DEFAULT,
            )
        } else {
            // The ladder's own `is_base` is `temporal_layer_index == 0`, which
            // every KEY frame is — NOT the `frame_is_boosted` one the controls
            // table below uses.
            crate::port_enc_mode_config::cdef_search::cdef_search_level_default(
                self.speed_config.preset as i8,
                dlf_is_base,
                SEQ_CDEF_LEVEL,
                sc_derivation.allow_intrabc,
                crate::port_enc_mode_config::cdef_search::CONFIG_DEFAULT,
            )
        };
        // `set_cdef_search_controls`' `is_base` is `frame_is_boosted` =
        // `frame_is_kf_gf_arf` = intra-only OR ARF OR GF update, and
        // `is_not_highest_layer` is `!frame_is_leaf` = `update_type !=
        // LF_UPDATE` (enc_mode_config.h:100-116). A KEY frame is intra-only
        // and KF_UPDATE, so both are true; written out rather than folded to
        // literals so the inter chunks inherit the rule.
        // C `frame_is_boosted` = `frame_is_kf_gf_arf`, and `is_not_highest_layer`
        // = `!frame_is_leaf` = `update_type != LF_UPDATE`
        // (`enc_mode_config.h:100-116`). Both were literal `is_key` while only
        // key frames were encodable. They now come from the picture decision's
        // `update_type` (`port_picstruct::set_frame_update_type`,
        // `pd_process.c:4591`), which is what C reads.
        //
        // A KEY frame is intra-only and KF_UPDATE, so both stay true there —
        // byte-inert for every existing cell, by construction rather than by
        // measurement alone.
        let (cdef_frame_is_boosted, cdef_is_not_highest_layer) = match pic_decision.as_ref() {
            Some(pic) => (
                crate::port_picstruct::frame_is_boosted(pic),
                pic.update_type != crate::port_picstruct::FrameUpdateType::Lf,
            ),
            None => (is_key, is_key),
        };
        // C's `default:` arm is `assert(0)`; the port refuses rather than
        // inventing a control set.
        // C `cdef_recon_level` -> `set_cdef_recon_controls` (enc_mode_config.c
        // :1200). ANOTHER arm ladder, and the port ran neither side of it: the
        // allintra arm is `enc_mode <= M7 ? 0 : 1` (`:2432`) and the video arm
        // `<= M8 ? 0 : <= M10 ? 1 : 2` (`:2102`), both at C's default
        // `fast_decode == 0` (the `fast_decode` branches are unreachable here
        // for the same reason the CDEF search ladder's are). Only
        // `zero_fs_cost_bias` is live on a KEY frame — see `CdefSearchCfg`.
        //
        // The allintra M10..M13 -> M9 clamp does not move this: every preset
        // from M8 up lands on level 1 either way.
        let cdef_recon_level: u8 = if is_single_frame {
            u8::from(self.speed_config.preset > 7)
        } else if self.speed_config.preset <= 8 {
            0
        } else if self.speed_config.preset <= 10 {
            1
        } else {
            2
        };
        let cdef_recon_ctrls = crate::port_enc_mode_config::tail::set_cdef_recon_controls(
            cdef_recon_level,
        )
        .ok_or(EncodeError::UnsupportedConfig(
            "cdef recon level outside set_cdef_recon_controls' 0..=4",
        ))?;
        let cdef_zero_fs_cost_bias = cdef_recon_ctrls.zero_fs_cost_bias;
        let mut cdef_ctrls = crate::port_enc_mode_config::cdef_search::set_cdef_search_controls(
            cdef_level,
            cdef_frame_is_boosted,
            cdef_is_not_highest_layer,
        )
        .ok_or(EncodeError::UnsupportedConfig(
            "cdef search level outside set_cdef_search_controls' 0..=10",
        ))?;
        // C `md_config_process.c:983-985`: when the level asked for either
        // reference-derived mode, the candidate set is REWRITTEN from the
        // reference pictures' own chosen strengths. Unreachable on a key frame
        // — `search_best_ref_fs` is `is_not_highest_layer ? 0 : 1` and a key
        // frame's `is_not_highest_layer` is true — so this is byte-inert for
        // the whole still envelope by construction.
        //
        // C's ORDER, and it is an `else if` (`md_config_process.c:980-985`):
        // the three CDEF-OFF gates are tested FIRST, and the reference-derived
        // rewrite runs only when none of them fired.
        //
        //   me_based_cdef_skip(pcs)
        //   || (cdef_ctrls->skip_th && skip_perc >= cdef_skip_th)
        //   || (vq sharpness && is_noise_level)      -> cdef_level = 0
        //   else if (use_reference_cdef_fs || search_best_ref_fs)
        //                                            -> update_cdef_filters_on_ref_info
        //
        // The SECOND gate is wired here, and it is what C's frame 2 takes:
        // `skip_th` is `is_base ? 0 : 80` from level 7 up, the QP adjustment is
        // `CLIP3(25, 100, skip_th + (base_q_idx - 128) / 4)`, and
        // `ref_skip_percentage` is the value `md_config_inputs` already
        // derives. MEASURED on `diag 64x64 q40 p8 frames=3`: at poc 2 the
        // reference is a 22-byte all-skip frame, so `skip_perc` is 100 against
        // a threshold of 88 and C switches CDEF OFF for the whole frame —
        // which is why its header codes `cdef_damping - 3` as the low two bits
        // of **-3** (the `never_picked` quirk: `cdef_damping` keeps its
        // `resource_coordination_process.c:423` initialiser 0 because
        // `finish_cdef_search` never runs). The port coded 2 there.
        //
        // The FIRST gate IS modelled: `me_based_cdef_skip`
        // (`md_config_process.c:781`). It is inert below preset 9 —
        // `cdef_recon_ctrls.zero_filter_strength_lvl` is 0 there by C's own
        // table (`set_cdef_recon_controls(0)` = every video preset <= 8) —
        // and LIVE at M9+, where it needs this frame's `rc_me_distortion`
        // mean (the same `avg_me_sad` `me_based_dlf_skip` reads) and the
        // references' `cdef_dist_dev`. The proving cell: `gradient 64x64 q40
        // p13` poc 4, a base frame where `skip_th` is 0 and
        // `use_reference_cdef_fs`/`search_best_ref_fs` are both 0 — without
        // this gate the port coded `cdef_damping = 5` where C's
        // `cdef_level = 0` leaves the field at its 0 initialiser.
        //
        // The ref list is C's `ref_frame_type_arr` restricted to
        // `rf[1] == NONE_FRAME` — the same single-ref slots `dlf_refs`
        // walks, carrying `cdef_dist_dev` + `tmp_layer_idx` instead of the
        // deblock fields.
        let cdef_dist_refs: alloc::vec::Vec<crate::port_enc_mode_config::cdef_search::RefCdefDist> =
            match pic_decision.as_ref() {
                Some(pic) if !is_key => {
                    let mut v = alloc::vec::Vec::new();
                    let mut push = |idx: usize| {
                        if let Some(rf) = self.dpb.get(pic.rps.ref_dpb_index[idx] as usize) {
                            v.push(crate::port_enc_mode_config::cdef_search::RefCdefDist {
                                cdef_dist_dev: rf.cdef_dist_dev,
                                tmp_layer_idx: rf.temporal_layer,
                            });
                        }
                    };
                    for i in 0..usize::from(pic.ref_list0_count_try) {
                        push(crate::port_picstruct::LAST + i);
                    }
                    for i in 0..usize::from(pic.ref_list1_count_try) {
                        push(crate::port_picstruct::BWD + i);
                    }
                    v
                }
                _ => alloc::vec::Vec::new(),
            };
        // The THIRD gate needs `vq_ctrls.sharpness_ctrls`, which this port
        // does not configure (C defaults it off).
        let mut cdef_force_off = !is_key
            && (crate::port_enc_mode_config::cdef_search::me_based_cdef_skip(
                &crate::port_enc_mode_config::cdef_search::CdefMeSkipInputs {
                    is_intra_slice: is_key,
                    hierarchical_levels: frame_hier,
                    temporal_layer_index: dlf_temporal_layer_index,
                    frame_is_boosted: cdef_frame_is_boosted,
                    frame_is_leaf: !cdef_is_not_highest_layer,
                    input_resolution: dlf_resolution,
                    zero_filter_strength_lvl: cdef_recon_ctrls.zero_filter_strength_lvl,
                    prev_cdef_dist_th: cdef_recon_ctrls.prev_cdef_dist_th,
                    refs: &cdef_dist_refs,
                    avg_me_sad: dlf_avg_me_sad,
                },
            ) || crate::port_enc_mode_config::cdef_search::cdef_skip_gate(
                cdef_ctrls.skip_th,
                base_qindex,
                pipeline_md_inputs
                    .as_ref()
                    .map_or(0, crate::inter_hdr_arm::ref_skip_percentage),
            ));
        if !cdef_force_off
            && !is_key
            && (cdef_ctrls.use_reference_cdef_fs != 0 || cdef_ctrls.search_best_ref_fs != 0)
            && let Some(pic) = pic_decision.as_ref()
        {
            use crate::port_enc_mode_config::cdef_search::RefCdefStrengths;
            // C reads `ref_pic_ptr_array[REF_LIST_0][0]` and
            // `[REF_LIST_1][0]` — the FIRST entry of each list, which is
            // LAST_FRAME's and BWDREF's DPB slot.
            let strengths_of = |slot: usize| -> Option<RefCdefStrengths> {
                let rf = self.dpb.get(slot)?;
                Some(RefCdefStrengths {
                    y0: *rf.cdef_y_strengths.first()?,
                    uv0: *rf.cdef_uv_strengths.first()?,
                    // C's `use_reference_cdef_fs` arm walks every slot
                    // (`ref_cdef_strengths_num`), not just slot 0, so the two
                    // extremes are computed here rather than assumed equal.
                    y_min: rf.cdef_y_strengths.iter().copied().min()?,
                    y_max: rf.cdef_y_strengths.iter().copied().max()?,
                })
            };
            const LAST: usize = 0;
            const BWD: usize = 4;
            if let Some(l0) = strengths_of(pic.rps.ref_dpb_index[LAST] as usize) {
                // C's list-1 guard: `slice_type == B_SLICE && ref_list1_count_try`.
                let l1 = (pic.ref_list1_count_try != 0)
                    .then(|| strengths_of(pic.rps.ref_dpb_index[BWD] as usize))
                    .flatten();
                let upd = crate::port_enc_mode_config::cdef_search::update_cdef_filters_on_ref_info(
                    &mut cdef_ctrls,
                    l0,
                    l1,
                );
                cdef_force_off = upd.force_cdef_off;
            }
        }
        // C `pcs->cdef_dist_dev` (`cdef_process.c:682-702`): seeded -1,
        // overwritten ONLY by the RD search (`enc_cdef.c:1057` — the
        // `use_qp_strength` and `use_reference_cdef_fs` arms of
        // `finish_cdef_search` return before it), and forced to 0 whenever
        // the signalled strengths end all-zero. The second element carries
        // the pre-override value; the override is applied once below.
        let (cdef_params, cdef_dist_dev) = if sc_derivation.allow_intrabc || coded_lossless {
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default()),
                0,
            )
        } else if cdef_force_off {
            // C `pcs->ppcs->cdef_level = 0` inside
            // `update_cdef_filters_on_ref_info`: no search, no application,
            // and the header codes zero strengths — plus the DAMPING quirk
            // that comes with never calling `finish_cdef_search`
            // (`CdefFrameParams::never_picked`).
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::never_picked()),
                0,
            )
        } else if cdef_ctrls.use_reference_cdef_fs != 0 {
            // The reference-derived prediction REPLACES the search
            // (`md_config_process.c:713-722` / `:750-758`). Damping is still
            // this frame's own `CDEF_DAMPING_FROM_QP` (`enc_cdef.c:1446`) —
            // only the strengths come from the reference. C's
            // `finish_cdef_search` early-returns on `use_reference_cdef_fs`
            // (enc_cdef.c:937-941), so `cdef_dist_dev` keeps -1.
            (
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams {
                    damping: 3 + (base_qindex >> 6),
                    y_strength: cdef_ctrls.pred_y_f as u8,
                    uv_strength: cdef_ctrls.pred_uv_f as u8,
                }),
                -1,
            )
        } else {
            // C runs the CDEF pick on EVERY coded frame; this used to be
            // `else if is_key`, and an inter frame fell through to
            // `CdefFrameParams::default()` — damping 3 and zero strengths,
            // which was the ONLY divergence left in the inter frame header
            // (`docs/INTER-ENCODE-PLAN.md` §1q).
            //
            // C splits the strength policy per preset (allintra
            // enc_mode_config.c:3543-3600): presets <= M6 run the CDEF
            // RDO search, >= M7 the use_qp_strength fast path we ported.
            // Of the search, exactly ONE outcome is ported so far: the
            // sb_count == 0 case — every filter block all-skip, e.g.
            // flat content — where finish_cdef_search deterministically
            // signals cdef_bits=0 with zero strengths (see
            // pick_cdef_params_all_skip_search provenance). Search
            // presets with any non-skip filter block keep the qp fast
            // path for now: still self-consistent (signal == apply),
            // but their signaled strengths diverge from C's searched
            // ones (gap 2a, narrowed to the non-all-skip case).
            if cdef_ctrls.enabled != 0 && !cdef_ctrls.use_qp_strength {
                if deblock_geom.cdef_frame_all_skip() {
                    (
                        crate::cdef::CdefPick::single(
                            crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
                        ),
                        // All-zero strengths -> the cdef_process.c:699-702
                        // rule; written 0 directly for clarity.
                        0,
                    )
                } else {
                    // The live-block RDO search (svt_av1_cdef_search +
                    // finish_cdef_search, per-preset candidate sets:
                    // level 2 at M0, 3 at M1-M3, 5 at M4-M5, 7 at M6):
                    // filter the POST-DEBLOCK recon per candidate strength
                    // and RD-pick against the source. The multi-strength
                    // outcome (cdef_bits>0 needs per-SB cdef_idx syntax
                    // the tile writer lacks) falls back to the qp fast
                    // path — self-consistent, documented divergence.
                    let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                    let cfg = crate::cdef::cdef_search_cfg_from_ctrls(
                        &cdef_ctrls,
                        cdef_zero_fs_cost_bias,
                    );
                    // bd10: search the TRUE 10-bit post-deblock recon against
                    // the true 10-bit source (C `cdef_seg_search` at
                    // is_16bit). The 10-bit source is `u8 << (bd - 8)` by
                    // construction — the harness writes exactly that .yuv for
                    // both encoders, so widening here is not an approximation.
                    let searched = match recon10.as_ref() {
                        Some((y10, u10, v10)) => {
                            let sh = (self.bit_depth - 8) as u32;
                            let widen = |p: &[u8]| -> Vec<u16> {
                                p.iter().map(|&s| (s as u16) << sh).collect()
                            };
                            // Task #6 chunk 2: real u16 source for the CDEF
                            // strength search's distortion (see the deblock
                            // site); identical widening on every other path.
                            let (sy10, su10, sv10) = match hbd_source.as_ref() {
                                Some(hbd) => {
                                    hbd_used = true;
                                    (hbd.y.clone(), hbd.u.clone(), hbd.v.clone())
                                }
                                None => (widen(&encode_input), widen(su), widen(sv)),
                            };
                            crate::cdef::cdef_search_still_hbd_with_stop(
                                &cfg,
                                y10,
                                u10,
                                v10,
                                &sy10,
                                &su10,
                                &sv10,
                                w,
                                h,
                                filter_chroma,
                                &deblock_geom,
                                base_qindex,
                                self.bit_depth,
                                &stop,
                            )?
                        }
                        None => crate::cdef::cdef_search_still_with_stop(
                            &cfg,
                            &recon,
                            &u_recon,
                            &v_recon,
                            &encode_input,
                            su,
                            sv,
                            w,
                            h,
                            filter_chroma,
                            &deblock_geom,
                            base_qindex,
                            &stop,
                        )?,
                    };
                    match searched {
                        crate::cdef::CdefSearchPick::Picked {
                            pick: mut p,
                            dist_dev,
                        } => {
                            // [SVT_HDR_MODE] fork cdef-scaling: search-path
                            // only (finish_cdef_search, enc_cdef.c:1444).
                            if self.hdr.is_fork() {
                                crate::cdef::scale_strengths(&mut p, self.hdr.cdef_scaling);
                            }
                            (p, dist_dev)
                        }
                        crate::cdef::CdefSearchPick::AllSkip => (
                            crate::cdef::CdefPick::single(
                                crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
                            ),
                            0,
                        ),
                    }
                }
            } else if cdef_ctrls.use_qp_strength {
                // C's `use_qp_strength` fast path takes the screen-content
                // arm of `svt_pick_cdef_from_qp` when
                // `allintra ? ppcs->sc_class5 : ppcs->sc_class1` is set
                // (enc_cdef.c:913-918) — allintra here, so sc_class5. This
                // is the FRAME-level derivation the frame header is written
                // from (the same `sc_derivation` that gates palette/IBC
                // above), not a tile-local one. Reachable at preset M7
                // exactly under a default config: use_qp_strength needs
                // cdef_search_level == 10 (allintra M7+,
                // enc_mode_config.c:3543-3600) and screen detection is
                // force-disabled at M8+ (enc_handle.c:4641-4651, mirrored by
                // `derive_allintra_sc`'s `preset <= 7` gate); it extends to
                // M8-M13 when a tune forces screen_content_mode = 3.
                // C `finish_cdef_search` early-returns on `use_qp_strength`
                // (enc_cdef.c:912-926), before `cdef_dist_dev` is computed —
                // it keeps the -1 seed.
                (
                    crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_key_frame(
                        base_qindex,
                        self.bit_depth,
                        sc_derivation.classes.sc_class5,
                    )),
                    -1,
                )
            } else {
                // `cdef_search_level == 0`: CDEF is off for this frame, so
                // neither arm runs and the header codes zero strengths. Only
                // reachable through a level-0 ladder entry, since the
                // IntraBC/lossless suppression is the outer branch above.
                // Same C state as the `cdef_force_off` arm — `cdef_level == 0`
                // means `finish_cdef_search` is never called, so `cdef_damping`
                // keeps its 0 initialisation and the header signals 1.
                (
                    crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::never_picked()),
                    0,
                )
            }
        };
        // 4:4:4 staged bring-up (`filter_chroma`): chroma CDEF is signaled
        // OFF — zero every uv strength so the header declares "chroma
        // unfiltered" and the apply call below skips the ss=0 kernels.
        let mut cdef_params = cdef_params;
        if fmt == svtav1_types::chroma::ChromaFormat::Yuv444 {
            for s in cdef_params.strengths.iter_mut() {
                s.1 = 0;
            }
        }
        // C `cdef_process.c:699-702`: whatever the pick left, a frame whose
        // signalled strengths are all zero (and `nb_cdef_strengths == 1`)
        // records `cdef_dist_dev = 0` — "no filtering happened" — which is
        // what the NEXT frame's `me_based_cdef_skip` reads as a shut-off
        // precedent.
        let cdef_dist_dev =
            if cdef_params.bits == 0 && cdef_params.strengths.first() == Some(&(0, 0)) {
                0
            } else {
                cdef_dist_dev
            };
        // Non-vacuity evidence (same role as `last_cdef_stats` /
        // `last_lr_stats`): the strength set 0 actually WRITTEN into the
        // frame header. Without this a gate cannot observe which arm of
        // `svt_pick_cdef_from_qp` the pipeline selected, so dropping the
        // `sc_class5` argument would be invisible to the whole suite.
        self.last_cdef_signaled = Some(crate::cdef::CdefFrameParams {
            damping: cdef_params.damping,
            y_strength: cdef_params.strengths[0].0,
            uv_strength: cdef_params.strengths[0].1,
        });
        // cdef_bits > 0 adds per-SB cdef_idx literals to the tile — the
        // walk is re-run with the emission armed, but NOT yet: when the LR
        // search below also signals, its walk carries the cdef syntax too
        // (`cdef_walk_opt`) and the intermediate walk's bytes would be
        // overwritten unread. The re-walk runs once, after both searches.
        // The pre-CDEF snapshot is load-bearing when LR is on (its stripe
        // boundaries are saved from it below), and an evidence aid otherwise.
        if seq_tools.enable_restoration || self.recon_output {
            self.last_recon_pre_cdef = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        if postfilter_consumed {
            self.last_cdef_stats = crate::cdef::apply_cdef_frame_with_stop(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &cdef_params,
                &stop,
            )?;
        }
        // bd10: the post-deblock / pre-CDEF 10-bit planes are the `after_cdef
        // = 0` stripe-boundary context for the 10-bit LR apply (issue #13) —
        // the 10-bit twin of `last_recon_pre_cdef` above, taken at the same
        // point in the chain (dlf_process.c:134 saves them here in C).
        let recon10_pre_cdef: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> =
            if seq_tools.enable_restoration {
                recon10.clone()
            } else {
                None
            };
        // bd10: apply CDEF to the 10-bit canvas too. Not for output — the u8
        // chain above still produces that — but because the Wiener LR search
        // reads the POST-CDEF recon, and at 10 bits that must be the 10-bit
        // one (C: rest_process runs after cdef_process on the same 16-bit
        // recon picture CDEF just filtered in place).
        if let (Some((y10, u10, v10)), true) = (recon10.as_mut(), postfilter_consumed) {
            crate::cdef::apply_cdef_frame_hbd_with_stop(
                y10,
                u10,
                v10,
                w,
                h,
                filter_chroma,
                &deblock_geom,
                &cdef_params,
                self.bit_depth,
                &stop,
            )?;
        }

        crate::stop_check(&stop)?;

        // Step 6a'': Wiener loop restoration — C order deblock -> CDEF ->
        // LR. The C-exact search (restoration_seg_search +
        // rest_finish_search at the allintra wn_filter controls) picks
        // per-RU taps against the POST-CDEF recon; when any plane signals
        // RESTORE_WIENER the tile is RE-walked with the per-SB lr syntax
        // (the flag+taps precede the first partition symbol, so the whole
        // arithmetic stream shifts — exactly like C, whose EC kernel runs
        // after rest_process), the FH carries the real lr_params, and the
        // output copy gets the decoder-exact stripe-boundary filter pass
        // (svt_av1_loop_restoration_filter_frame). Prediction sources are
        // untouched — the decoder's split.
        self.last_lr_stats = ([0; 3], 0);
        self.last_lr_unit_size = None;
        let mut lr_signal = crate::entropy::obu::LrSignal::none(seq_tools.enable_restoration);
        // The search result the bit-producing walk below is armed with —
        // `Some` exactly when the old code would have run an LR re-walk
        // (`rest_info.any_non_none()`); the walk itself now runs once,
        // after both searches.
        let mut walk_rest_info: Option<crate::restoration::FrameRestInfo> = None;
        let decoder_chroma_recon = self.recon_output
            && chroma.is_some()
            && (!(self.true_width as usize).is_multiple_of(1 << ss_x)
                || !(self.true_height as usize).is_multiple_of(1 << ss_y));
        let mut output_restoration = None;
        // IBC (chunk 1): unlike DLF/CDEF, C suppresses loop restoration at
        // PIPELINE EXECUTION, not signal-derivation — `if (ppcs->
        // enable_restoration && frm_hdr->allow_intrabc == 0)` gates BOTH the
        // search (rest_process.c:262) and the apply/finish (:325, else-arm
        // forces all planes RESTORE_NONE). enable_restoration itself (and
        // the SH bit) stays UNCHANGED — do NOT fold this into the
        // derivation (docs/ibc-port-map.md §A.7).
        //
        // The gate is NOT `is_key`: `ppcs->enable_restoration` is the
        // PICTURE-level `(wn > 0 || sg > 0)` (enc_mode_config.c:2142), and
        // the video ladder keeps Wiener live on an inter frame —
        // `wn_filter_level_default` gives level 5 (luma-only) at M4..M8
        // whenever `is_not_last_layer`, which a flat GOP's `hierarchical_
        // levels != 0` clause makes true for EVERY picture
        // (pd_process.c:5560). The `ctrls.enabled || sg_ctrls.enabled`
        // check inside IS that per-picture term, so `is_key` here only
        // ever wrongly disabled the stage on inter frames — the exact
        // `lr_type[0]` C=2 vs 0 divergence on `johnny_256x256` q40 p6
        // frame 1.
        if seq_tools.enable_restoration && !sc_derivation.allow_intrabc && !coded_lossless {
            // LOOP-RESTORATION LEVEL LADDERS — the `scs->allintra` fork
            // (`pd_process.c:4935-4938`), the same selector `sc_detect`, the
            // deblock ladder and the rate ladders already take.
            //
            // The all-intra arm is `wn_filter_level_allintra` (3 / 4 / off) with
            // `sg_filter_level_allintra` == 0 at normal presets 0 through 13,
            // which is why the port has only ever run Wiener. The VIDEO arm is
            // `_default`: Wiener 4 at <= M3 and 5 at <= M8 on a non-last layer
            // (level 5 is LUMA-ONLY), and SGR level 3 at <= M3 — so a video-mode
            // key frame at presets 0..3 can emit RESTORE_SGRPROJ and, on a plane
            // with more than one restoration unit, RESTORE_SWITCHABLE.
            //
            // The two arms must move TOGETHER: the video Wiener ladder is
            // nonzero at p7/p8 where the all-intra one is off, so wiring `sg`
            // alone would leave the frame RD comparing an SGR candidate against
            // a Wiener candidate C never searched, and wiring `wn` alone cannot
            // close the p3 cell whose gap is `sg`.
            let lr_enc_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
            // `ppcs->input_resolution`, derived exactly as the deblock ladder
            // above derives it.
            let lr_resolution = crate::port_enc_mode_config::ResolutionRange::from_luma_area(
                self.true_width.next_multiple_of(8) * self.true_height.next_multiple_of(8),
            )
            .as_u8();
            // `is_not_last_layer = !ppcs->is_highest_layer` — the same value the
            // deblock ladder derived above, reused rather than re-derived so the
            // two cannot drift.
            let lr_is_not_last_layer = dlf_is_not_last_layer != 0;
            let (ctrls, sg_ctrls) = match sc_arm {
                crate::sc_detect::ScArm::Allintra => (
                    crate::restoration::wn_filter_ctrls_allintra(self.speed_config.preset),
                    crate::port_lr_level::set_sg_filter_ctrls(
                        crate::port_enc_mode_config::leaf::get_sg_filter_level_allintra(
                            lr_enc_mode,
                        ),
                    ),
                ),
                crate::sc_detect::ScArm::Video { .. } => {
                    let wn = crate::port_lr_level::wn_filter_level_default(
                        lr_enc_mode,
                        lr_resolution,
                        lr_is_not_last_layer,
                    );
                    // `scs->static_config.fast_decode` is 0 for every
                    // configuration this port and the inter harness produce.
                    let sg = crate::port_lr_level::sg_filter_level_default(
                        lr_enc_mode,
                        lr_resolution,
                        false,
                    );
                    (
                        crate::restoration::WnFilterCtrls::from(
                            crate::port_lr_level::set_wn_filter_ctrls(wn),
                        ),
                        crate::port_lr_level::set_sg_filter_ctrls(sg),
                    )
                }
            };
            if ctrls.enabled || sg_ctrls.enabled {
                // C `x->rdmult` = `pic_full_lambda[bit_depth == EB_TEN_BIT ?
                // EB_10_BIT_MD : EB_8_BIT_MD]` (enc_dec_process.c:3246-3247),
                // i.e. `svt_aom_lambda_assign(.., multiply_lambda = true)` —
                // whose `*= 16` arm is 10-bit-ONLY, so bd8 is the unweighted
                // value and bd10 is 16x the bd10 one. (Contrast the CDEF
                // search, enc_cdef.c:958, which passes false.)
                let rdmult = match (self.bit_depth, recon10.as_ref()) {
                    (10, Some(_)) => crate::pd0::kf_full_lambda_bd10_pic(base_qindex) as i64,
                    _ => crate::pd0::kf_full_lambda_8bit_unweighted(base_qindex) as i64,
                };
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                // PORT-NOTE(VERIFIED whole-frame — do NOT make per-tile):
                // this call (and the per-SB `write_lr_for_sb` walk below)
                // computes the restoration-unit grid across the WHOLE FRAME
                // (`svtav1_dsp::restoration::count_units_in_tile(unit_size,
                // pw)` — restoration.rs:425-426 — with the full plane
                // width/height), which is EXACTLY what C does regardless of
                // tile count: `svt_aom_foreach_rest_unit_in_frame` /
                // `_frame_seg` (restoration.c:1274-1297 / 1379-1394) build
                // the grid from `whole_frame_rect`, call `on_tile(0,0)`
                // exactly once, and the stripe-derivation tile loop is
                // hardcoded `for i < 1 /*cm->tile_rows*/` (restoration.c:1699).
                // So the LR RU grid / tap-delta chain is tile-INDEPENDENT.
                // (The earlier task-#86 "genuinely PER-TILE" hypothesis was
                // WRONG — read the C source, not the "in_tile" name.) The
                // task-#86 2-tile-row `lr-taps` divergence was a downstream
                // SYMPTOM: a recon difference reprices the whole-frame Wiener
                // taps, and that recon difference was the M6 PD0 partition
                // search predicting DC across the tile boundary (pd0.rs
                // `lvl1_block_cost_rect`, now fixed via `extract_neighbors_
                // tiled`). With that fixed the LR taps match C byte-for-byte
                // on the full multi-tile sweep (162/162), confirming this
                // whole-frame grid is correct as-is.
                // Task #95 goal 1 (odd true dims): the search runs on the TRUE
                // luma / CEILING chroma extent, reading the recon at its aligned
                // buffer stride while `extend_frame` replicates the true edge —
                // so it never sees the aligned padding (matching C, whose
                // extend replicates the frame edge into the LR border). Extract
                // tight true/ceil buffers from the aligned-strided recon +
                // source (luma stride `w`, chroma stride `cw`); on an 8-aligned
                // frame true == aligned, so these are byte-neutral copies.
                let (lr_tcw, lr_tch) = (fmt.chroma_width(lr_true_w), fmt.chroma_height(lr_true_h));
                let extract_tight = |src: &[u8], src_stride: usize, pw: usize, ph: usize| {
                    let mut out = alloc::vec![0u8; pw * ph];
                    for r in 0..ph {
                        out[r * pw..(r + 1) * pw]
                            .copy_from_slice(&src[r * src_stride..r * src_stride + pw]);
                    }
                    out
                };
                let lr_src_y = extract_tight(&encode_input, w, lr_true_w, lr_true_h);
                let lr_rec_y = extract_tight(&recon, w, lr_true_w, lr_true_h);
                let (lr_src_u, lr_src_v, lr_rec_u, lr_rec_v) = if chroma.is_some() {
                    (
                        extract_tight(su, cw, lr_tcw, lr_tch),
                        extract_tight(sv, cw, lr_tcw, lr_tch),
                        extract_tight(&u_recon, cw, lr_tcw, lr_tch),
                        extract_tight(&v_recon, cw, lr_tcw, lr_tch),
                    )
                } else {
                    (
                        alloc::vec::Vec::new(),
                        alloc::vec::Vec::new(),
                        alloc::vec::Vec::new(),
                        alloc::vec::Vec::new(),
                    )
                };
                // bd10: run the search on the TRUE 10-bit post-CDEF recon
                // against the true 10-bit source. Same tight true/ceil
                // extraction as the u8 arm — the 10-bit canvas is already
                // tight (`w` / `w/2` stride), and the 10-bit source is
                // `u8 << (bd - 8)` by construction (the harness writes exactly
                // that .yuv for both encoders).
                // LR search-input dump (SVTAV1_LRREC_BIN) — the tight
                // post-CDEF planes the Wiener/SGR search reads, one set per
                // frame. Pairs with the C `SVT_LFRECON_BIN` interposer
                // dump (post-deblock == post-CDEF whenever every coded CDEF
                // strength is 0).
                #[cfg(feature = "std")]
                if let Ok(prefix) = std::env::var("SVTAV1_LRREC_BIN") {
                    static CALL: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    let call = CALL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    for (plane, buf) in [&lr_rec_y, &lr_rec_u, &lr_rec_v].into_iter().enumerate() {
                        if !buf.is_empty() {
                            std::fs::write(format!("{prefix}.f{call}.p{plane}"), buf)
                                .expect("write LR search-input recon");
                        }
                    }
                }
                let rest_info = match recon10.as_ref() {
                    Some((y10, u10, v10)) => {
                        let sh = (self.bit_depth - 8) as u32;
                        let widen_tight =
                            |src: &[u8], src_stride: usize, pw: usize, ph: usize| -> Vec<u16> {
                                if src.is_empty() {
                                    return Vec::new();
                                }
                                let mut out = alloc::vec![0u16; pw * ph];
                                for r in 0..ph {
                                    for c in 0..pw {
                                        out[r * pw + c] = (src[r * src_stride + c] as u16) << sh;
                                    }
                                }
                                out
                            };
                        let tight10 =
                            |src: &[u16], src_stride: usize, pw: usize, ph: usize| -> Vec<u16> {
                                if src.is_empty() {
                                    return Vec::new();
                                }
                                let mut out = alloc::vec![0u16; pw * ph];
                                for r in 0..ph {
                                    out[r * pw..(r + 1) * pw]
                                        .copy_from_slice(&src[r * src_stride..r * src_stride + pw]);
                                }
                                out
                            };
                        // Task #6 chunk 2: with a native HBD source the Wiener
                        // tap search sees the caller's real u16 samples (same
                        // tight true/ceil extraction, just from the u16 plane);
                        // otherwise the identical `u8 << sh` widening as before.
                        let (lr_sy10, lr_su10, lr_sv10) = match hbd_source.as_ref() {
                            Some(hbd) => {
                                hbd_used = true;
                                (
                                    tight10(&hbd.y, w, lr_true_w, lr_true_h),
                                    tight10(&hbd.u, cw, lr_tcw, lr_tch),
                                    tight10(&hbd.v, cw, lr_tcw, lr_tch),
                                )
                            }
                            None => (
                                widen_tight(&encode_input, w, lr_true_w, lr_true_h),
                                widen_tight(su, cw, lr_tcw, lr_tch),
                                widen_tight(sv, cw, lr_tcw, lr_tch),
                            ),
                        };
                        crate::restoration::search_restoration_still_configured_with_stop(
                            &ctrls,
                            &sg_ctrls,
                            &lr_sy10,
                            &lr_su10,
                            &lr_sv10,
                            &tight10(y10, w, lr_true_w, lr_true_h),
                            &tight10(u10, acw, lr_tcw, lr_tch),
                            &tight10(v10, acw, lr_tcw, lr_tch),
                            lr_true_w,
                            lr_true_h,
                            filter_chroma,
                            rdmult,
                            self.bit_depth,
                            self.enhancements.contains(
                                crate::enhancements::ZenEnhancement::AomRestorationUnitSearch,
                            ),
                            self.sb_size,
                            &stop,
                        )?
                    }
                    None => {
                        crate::restoration::search_restoration_still_configured_with_stop::<u8>(
                            &ctrls,
                            &sg_ctrls,
                            &lr_src_y,
                            &lr_src_u,
                            &lr_src_v,
                            &lr_rec_y,
                            &lr_rec_u,
                            &lr_rec_v,
                            lr_true_w,
                            lr_true_h,
                            filter_chroma,
                            rdmult,
                            8,
                            self.enhancements.contains(
                                crate::enhancements::ZenEnhancement::AomRestorationUnitSearch,
                            ),
                            self.sb_size,
                            &stop,
                        )?
                    }
                };
                #[cfg(feature = "std")]
                if crate::dbgenv::dump_lr() {
                    for (p, pr) in rest_info.planes.iter().enumerate() {
                        eprintln!(
                            "LR plane={p} frame_rtype={} units={:?}",
                            pr.frame_rtype,
                            pr.units
                                .iter()
                                .map(|u| (u.rtype, u.wiener.vfilter, u.wiener.hfilter))
                                .collect::<alloc::vec::Vec<_>>()
                        );
                    }
                }
                if rest_info.any_non_none() {
                    // Decoder-exact application to the output copy: stripe
                    // boundaries from the post-deblock (pre-CDEF) and
                    // post-CDEF planes (dlf_process.c:134 after_cdef=0,
                    // cdef_process.c:707 after_cdef=1).
                    let (pre_y, pre_u, pre_v) = self
                        .last_recon_pre_cdef
                        .as_ref()
                        .expect("pre-CDEF recon captured above");
                    // Task #95 goal 1 / issue #11: the boundary save and the
                    // unit walk take the SAME TRUE extent the search sized the
                    // RU grid from (C drives all three off one
                    // `whole_frame_rect`), read at the ALIGNED canvas strides
                    // the planes are stored at. Passing the aligned extent here
                    // while the grid was counted on the true one made the walk
                    // visit more units than the grid holds — an out-of-bounds
                    // index whenever alignment crossed a `count_units_in_tile`
                    // boundary (e.g. true 383 -> 1 unit, aligned 384 -> 2).
                    // Byte-neutral for 8-aligned dims (true == aligned).
                    let bounds = crate::restoration::save_lr_boundaries(
                        pre_y,
                        pre_u,
                        pre_v,
                        &recon,
                        &u_recon,
                        &v_recon,
                        lr_true_w,
                        lr_true_h,
                        w,
                        cw,
                        filter_chroma,
                    );
                    crate::restoration::apply_restoration_frame_bd_with_stop(
                        &mut recon,
                        &mut u_recon,
                        &mut v_recon,
                        lr_true_w,
                        lr_true_h,
                        w,
                        cw,
                        filter_chroma,
                        &rest_info,
                        &bounds,
                        8,
                        &stop,
                    )?;
                    // Issue #13: the 10-bit canvas gets the SAME apply. The
                    // search above picked these taps on the 10-bit recon and
                    // the frame header signals them, so a decoder applies
                    // them to its 10-bit output — until now no 10-bit plane
                    // in the port ever received them. Same true extent, same
                    // ALIGNED strides the 10-bit canvas is stored at (`w`
                    // luma, `w / 2` chroma — see `tight10` above), boundary
                    // lines from the 10-bit post-deblock (pre-CDEF) and
                    // post-CDEF planes (C: rest_process.c on the 16-bit
                    // recon picture, highbd = 1).
                    if let (Some((y10, u10, v10)), Some((py10, pu10, pv10))) =
                        (recon10.as_mut(), recon10_pre_cdef.as_ref())
                    {
                        let bounds10 = crate::restoration::save_lr_boundaries_bd::<u16>(
                            py10,
                            pu10,
                            pv10,
                            y10,
                            u10,
                            v10,
                            lr_true_w,
                            lr_true_h,
                            w,
                            acw,
                            filter_chroma,
                        );
                        crate::restoration::apply_restoration_frame_bd_with_stop::<u16>(
                            y10,
                            u10,
                            v10,
                            lr_true_w,
                            lr_true_h,
                            w,
                            acw,
                            filter_chroma,
                            &rest_info,
                            &bounds10,
                            self.bit_depth,
                            &stop,
                        )?;
                    }
                }
                self.last_lr_unit_size = Some(rest_info.planes[0].unit_size as usize);
                self.last_lr_stats = (
                    [
                        rest_info.planes[0].frame_rtype,
                        rest_info.planes[1].frame_rtype,
                        rest_info.planes[2].frame_rtype,
                    ],
                    rest_info
                        .planes
                        .iter()
                        .flat_map(|p| p.units.iter())
                        .filter(|u| u.rtype == svtav1_dsp::restoration::RESTORE_WIENER)
                        .count(),
                );
                lr_signal = crate::entropy::obu::LrSignal {
                    enabled: true,
                    frame_types: [
                        rest_info.planes[0].frame_rtype,
                        rest_info.planes[1].frame_rtype,
                        rest_info.planes[2].frame_rtype,
                    ],
                    unit_size: rest_info.planes[0].unit_size as u16,
                    // C: rst_info[1].size != rst_info[0].size — always
                    // equal (set_restoration_unit_size s = 0).
                    uv_size_differs: false,
                };
                if decoder_chroma_recon {
                    output_restoration = Some(rest_info.clone());
                }
                // Arm the bit-producing walk below with the LR syntax —
                // `Some` here is exactly the old `if rest_info.any_non_none()`
                // re-walk gate.
                walk_rest_info = Some(rest_info);
            }
        }

        // The ONE bit-producing walk — C order (rest_process before the EC
        // kernel): armed with whatever CDEF/LR syntax the searches picked,
        // or none. Runs only on the recon-pass split; the no-consumer path
        // above already produced its single full walk. Exactly one of these
        // cases holds: LR syntax (`walk_rest_info`), CDEF syntax
        // (`cdef_params.bits > 0`), both, or neither — the old code ran up
        // to three walks for the same matrix.
        if postfilter_consumed {
            let cdef_walk_opt = (cdef_params.bits > 0).then_some(&cdef_params);
            let (tile_f, _geom_f, u_f, v_f, tsb_f) =
                run_entropy_walk(walk_rest_info.as_ref(), cdef_walk_opt, false)?;
            // The final walk reproduces the PRE-filter recon; u_recon/v_recon
            // were deblocked (and possibly CDEF'd/restored) IN PLACE above,
            // so compare against the pre-deblock copy (the old `== u_recon`
            // form only held on content where chroma deblock was a no-op —
            // it fired spuriously on flat+textured content at mid qp,
            // mainline included, pre-dating the fork work).
            #[cfg(debug_assertions)]
            if let Some((_, u_unf, v_unf)) = self.last_recon_unfiltered.as_ref() {
                debug_assert_eq!(&u_f, u_unf, "final walk chroma recon must be identical");
                debug_assert_eq!(&v_f, v_unf, "final walk chroma recon must be identical");
            }
            let _ = (&u_f, &v_f);
            tile_data = tile_f;
            tile_size_bytes_minus_1 = tsb_f;
        }

        crate::stop_check(&stop)?;

        // Step 7: Build OBU bitstream
        // Use full (non-reduced) sequence header for multi-frame sequences,
        // still-picture header only for single-frame mode. is_single_frame
        // + seq_tools were derived before the entropy walk (the walk codes
        // use_filter_intra flags iff the SH will signal the tool).
        // FH screen-content bits from the pre-walk derivation (see the
        // EntropyCtx::new site): MD palette/IBC candidates are NOT ported
        // yet (#71) — frames the detector fires on still diverge in the
        // tile, but their FH + no-palette flag stream now match C for the
        // palette-only presets M5-M7; M2-M4 additionally need the IBC
        // vertical. Frames it does not fire on are unaffected.
        let sc_signal = crate::entropy::obu::ScSignal {
            allow_screen_content_tools: sc_derivation.allow_screen_content_tools,
            allow_intrabc: sc_derivation.allow_intrabc,
            // Superres chunk B.3: signal what the encode actually did. Off by
            // default -> `SuperresParams::default()` -> zero bits written,
            // i.e. the pre-superres header layout exactly.
            superres: crate::entropy::obu::SuperresParams {
                enabled_in_seq: self.superres_denom.is_some(),
                denom: self.superres_denom,
            },
        };

        // The INTER frame header's picture-level fields, from the SAME
        // derivations the encode used: the reference structure out of
        // `run_picture_decision` and the tool ladders out of
        // `svt_aom_sig_deriv_mode_decision_config_default`. See
        // `crate::inter_hdr_arm`.
        let mut inter_signal: Option<crate::entropy::obu::InterSignal> = if is_key {
            None
        } else {
            let pic = pic_decision.as_ref().ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "an inter frame reached header signalling without a picture decision — \
                     unreachable since run_picture_decision covers every intra_period != 1 \
                     config (defensive; remove the caller's is_key guard instead of emitting \
                     a header that disagrees with the encode) [C: accepts]",
                ))
            })?;
            let ref_queue =
                crate::inter_hdr_arm::ref_queue_from_dpb(&pic.ref_queue_dpb, base_qindex);
            let binding = crate::port_picstruct::bind_refs_and_primary_ref_frame(
                pic, &ref_queue,
                // C `frame_end_cdf_update_mode` — the picture manager assigns
                // REFRESH_FRAME_CONTEXT_BACKWARD to coded pictures, which is
                // the same fact the header's `disable_frame_end_update_cdf = 0`
                // records (obu.rs).
                true, /*is_s_frame=*/ false,
            );
            // THE FRAME-2 WALL, MOVED 2026-09-03 and now naming the gap
            // that is actually left.
            //
            // It used to be `md_config_inputs` returning `None` for any
            // `get_ref_hp_percentage` answer other than its -1 sentinel,
            // because the port carried NONE of the three coded-area
            // statistics C reads off a reference. It carries all three now —
            // `ReferenceFrame::{intra,skip,hp}_coded_area`, accumulated in
            // the walk by `CodedAreaAcc` exactly where C's `update_b` does,
            // and VERIFIED against C's own reference objects
            // (`SVT_REFSTATS_OUT` on `gradient 64x64 q32 p8 frames=3`): C
            // reads its frame-2 list-0 reference as `0/0/100/0`
            // (slice/intra/skip/hp) and this port writes exactly
            // `slice=0 intra=0 skip=100 hp=0` onto frame 1's DPB entry.
            //
            // WHAT IS STILL MISSING — RE-KEYED 2026-09-03, because the
            // mechanism this refusal used to name was NOT the first
            // divergence and the number it quoted was a DIFFERENT defect's.
            //
            // It said frame 2 needs `pd0_detector`'s per-superblock
            // `ref_obj_l0->sb_intra[sb_index]` and cited "466 B against C's
            // 21". That 466 B was the port PREDICTING FRAME 2 FROM THE WRONG
            // PICTURE: `ref_frame_data` / `ref_padded_luma` / the PD0
            // `sb_min_sq_size` read read a hard-coded DPB SLOT 0, where C
            // resolves LAST through `pic.rps.ref_dpb_index[LAST]` — slot 1 at
            // poc 2. With that fixed (see `last_ref_slot`) the same cell codes
            // **22 B against C's 21**, and the frame-2 frontier is a byte, not
            // a rewrite.
            //
            // The measured first divergence NOW is the TEMPORAL MOTION FIELD.
            // `fh_fields.py` on frame 2 of `diag 64x64 q40 p8 frames=3` shows
            // `use_ref_frame_mvs = 1` on BOTH sides, so C's MFMV block is live
            // and so is the port's — but the port's `tpl_mvs` are all
            // `INVALID_MV` (see `inter_mvp_env` above). On the 2-frame
            // envelope that is FAITHFUL — the reference is a key frame and C's
            // own projection returns 0 for it
            // (`start_frame_buf->frame_type == KEY_FRAME`,
            // md_config_process.c:441) — and at poc 2 it is a gap: C's
            // `SVT_CINTER_OUT` codes `mode=13 NEARESTMV mv=(0,-24)` off a
            // stack whose only source is that field (0 spatial matches,
            // `imc=8` on both sides), while the port's `SVTAV1_CANDDBG`
            // reports `refmvcnt=0` and a NEARESTMV of `(0,0)`.
            //
            // THAT FIELD IS WIRED NOW (§1z²⁸) and the refusal survived it.
            // `ReferenceFrame::mvs` carries C's per-8x8 `MV_REF` grid, the
            // walk folds it through `port_coding_loop::copy_frame_mvs` under
            // C's own gate, and `inter_mvp_env.tpl_mvs` is
            // `inter_mvp::setup_motion_field`'s output rather than a constant.
            // MEASURED: the port's frame-2 `NEARESTMV` is C's `(0,-24)` off a
            // stack of 1 where it was `(0,0)` off an empty one, and SIX of
            // eight `frames=3` cells match C's frame-2 byte COUNT. None is
            // byte-identical, so what is left is the RECON — the first
            // diverging frame-header field is a CDEF search output, and on two
            // cells no header field differs at all.
            //
            // `part_arm::VideoPic`'s missing `InterOnInterRef` arm — §1z²⁵'s
            // mechanism — is still a real gap and still unported. It is a
            // candidate for that residual: it picks `pic_pd0_lvl`, the level
            // the whole partition search runs at, and the DPB already carries
            // the `sb_intra` / `sb_skip` it needs.
            //
            // See `docs/INTER-ENCODE-PLAN.md` 1z27 and 1z28.
            #[cfg(feature = "std")]
            if crate::dbgenv::refstats() {
                std::eprintln!(
                    "PORTREFBIND poc={display_order} l0cnt={} l1cnt={} dpb0={} l0slot={} \
                     l0_is_islice={:?} prf={} refresh=0x{:02x}",
                    pic.ref_list0_count_try,
                    pic.ref_list1_count_try,
                    self.dpb.occupied_slots(),
                    pic.rps.ref_dpb_index[0],
                    self.dpb
                        .get(pic.rps.ref_dpb_index[0] as usize)
                        .map(|rf| rf.is_islice),
                    binding.primary_ref_frame,
                    pic.rps.refresh_frame_mask,
                );
            }
            // THE CHAIN REFUSAL IS GONE (2026-09-11). It used to reject any
            // inter frame whose LIST-0 reference is itself an inter frame —
            // i.e. everything past frame 1 — and it was lifted only by
            // `SVTAV1_INTER_CHAIN_EXPERIMENTAL`, now deleted along with it.
            //
            // Its text said "NONE is byte-identical" and named the temporal
            // motion field as the suspect. Both halves have since been
            // settled by measurement. The field's defect was
            // `sb64_sq_no4xn_geom` being derived from `sb_size == 64` alone,
            // so C's SIMPLIFIED MFMV block walk ran on rectangular blocks and
            // used `n4_w` for both extents; with that corrected,
            // `tools/video_selfcheck_gate.sh` reports 18 of 18 real-clip cells
            // reconstructing byte-identically to `aomdec` across all 8 frames.
            // And "NONE is byte-identical" is simply no longer true: on the
            // 96-cell frontier grid at frames=4, 60 cells match C exactly on
            // frame 2 and 58 on frame 3 (MEASURED 2026-09-11).
            //
            // What is left is a parity frontier, not a correctness one, and it
            // is recorded in the README's video rows rather than as a refusal
            // — a refusal that describes a closed gap tells the next reader
            // not to look.
            let sigs = md_config_signals.ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "an inter frame's mode-decision configuration is outside this port's \
                     envelope: sig_deriv_mode_decision_config_default declined a level \
                     (crate::inter_hdr_arm::md_config_inputs) [C: accepts]",
                ))
            })?;
            // The tile above was coded from whatever `primary_ref_frame_for_cdf`
            // resolved to; the header must announce the SAME reference or the
            // decoder restores different CDFs than the encoder used.
            assert_eq!(
                binding.primary_ref_frame, primary_ref_frame_for_cdf,
                "the header's primary_ref_frame must equal the one the tile's CDFs came from",
            );
            Some(
                crate::inter_hdr_arm::inter_signal(
                    pic,
                    &sigs,
                    binding.primary_ref_frame,
                    crate::entropy::obu::ORDER_HINT_BITS,
                    crate::inter_hdr_arm::SeqInterTools {
                        enable_order_hint: seq_tools.enable_order_hint,
                        enable_ref_frame_mvs: seq_tools.enable_ref_frame_mvs,
                        enable_warped_motion: seq_tools.enable_warped_motion,
                    },
                    self.scs_tpl(),
                    // Retained as an ASSERTION input, not a refusal: the
                    // header now codes the real models. `gm_models` is `None`
                    // only when a search was needed and could not run, which
                    // `gm_search_config_error` still refuses above.
                    gm_models.as_ref().is_none_or(|m| !m.is_gm_on),
                )
                .map_err(|e| {
                    // One message per variant. These used to share one, so a
                    // GLOBAL-MOTION refusal was reported as an mfmv/TPL one —
                    // a refusal that names the wrong feature sends the next
                    // reader to the wrong file.
                    whereat::at!(EncodeError::UnsupportedConfig(match e {
                        crate::inter_hdr_arm::InterHdrError::MfmvLevelNotDerivable(_) =>
                            "an inter frame header field is not implemented for this \
                             configuration: use_ref_frame_mvs at mfmv_level >= 2 needs the TPL \
                             r0 and the references' own is_mfmv_used (crate::inter_hdr_arm::\
                             InterHdrError). This port's TPL is structurally off (aq_mode 0), \
                             so reaching this means the aq_mode refusal was lifted without \
                             porting r0 [C: accepts]",
                        // RETIRED: `inter_signal` no longer raises this — global
                        // motion is coded. The arm stays because the variant is
                        // public API and a `match` must be total.
                        crate::inter_hdr_arm::InterHdrError::GlobalMotionNotImplemented =>
                            "global motion is not implemented: the inter frame header writer \
                             reached global_motion_params() with a model it could not code. \
                             This refusal is RETIRED — `port_entropy_inter::gm::\
                             write_global_motion` codes the frame's real models — and reaching \
                             it means a caller constructed the variant by hand [C: accepts]",
                    }))
                })?,
            )
        };

        // The header's `global_motion_params()`. `inter_signal` leaves both
        // arrays IDENTITY (it has no access to the search); they are filled
        // here from the same two values the tile's mode decision uses, so the
        // header and the pack cannot disagree about the model a GLOBALMV block
        // was priced against.
        if let Some(signal) = inter_signal.as_mut() {
            for i in 0..8 {
                signal.global_motion[i] = gm_field[i].into();
                signal.ref_global_motion[i] = ref_gm_field[i].into();
            }
            signal.sync_is_global();
        }

        if let (Some(signal), Some(fg)) = (inter_signal.as_mut(), film_grain.as_ref()) {
            let is_b = pic_decision
                .as_ref()
                .is_some_and(|p| p.slice_type == crate::port_picstruct::SliceType::B);
            signal.film_grain_ref_idx = self.film_grain_reference(fg, signal.ref_frame_idx, is_b);
        }

        // The tile above coded its MVP contexts from `inter_mvp_env`, which
        // derived `use_ref_frame_mvs` from the same two rules
        // `inter_hdr_arm::inter_signal` applies. Assert rather than assume:
        // that bit is the ONLY term that sets the GLOBALMV bit of
        // `mode_context` on a block with no coded neighbours (§1t), so a
        // disagreement silently moves a `newmv` CDF row and is invisible in
        // any byte count.
        if let (Some(sig), Some(env)) = (inter_signal.as_ref(), inter_mvp_env.as_ref()) {
            assert_eq!(
                sig.use_ref_frame_mvs.unwrap_or(false),
                env.use_ref_frame_mvs,
                "the header's use_ref_frame_mvs must equal the one the tile's MVP used",
            );
        }

        // ONE assembly path for both frame types. It used to fork into a
        // separate, monochrome-shaped `write_inter_frame` that shared none of
        // the key frame's derivations — so the inter header could not carry
        // the deblock levels, the CDEF strengths, the LR types, real
        // tile_info() or the chroma quantizer deltas the encode actually used.
        // Signaling and application must agree on every one of those or the
        // recon desyncs from a conforming decoder.
        let bitstream = {
            let mut bs = alloc::vec::Vec::new();
            bs.extend_from_slice(&crate::entropy::obu::write_temporal_delimiter());
            // The sequence header is written once, on the key frame — C emits
            // it on the first packet only (verified: `c.obu.pts1` of the
            // 2-frame cell is a temporal delimiter plus one OBU_FRAME).
            if is_key {
                bs.extend_from_slice(&crate::entropy::obu::write_sequence_header_ex(
                    // TRUE (unaligned) dims flow to the sequence header:
                    // max_frame_width/height_minus_1 carry the coded size, and
                    // the level derivation keys off the real picture size (C
                    // captures max_frame_width BEFORE 8-alignment,
                    // enc_handle.c:4792). Everything else in the encode uses
                    // the aligned self.width/height.
                    // Superres: the sequence header advertises the UPSCALED
                    // width (what a decoder outputs); the encode itself ran at
                    // the reduced `true_width`. Equal when superres is off.
                    self.upscaled_width,
                    self.true_height,
                    is_single_frame && !self.image_sequence,
                    self.bit_depth,
                    &self.color_description,
                    chroma.is_none(), // mono_chrome unless the 4:2:0 path is active
                    // seq_level_idx auto-derivation input (C: scs->frame_rate).
                    self.rc_config.framerate,
                    {
                        let mut t = seq_tools;
                        // The coded chroma format selects seq_profile +
                        // subsampling bits (spec 5.5.2); at Yuv420 this is
                        // profile 0, identical to every existing cell.
                        t.chroma_format = self
                            .chroma_format
                            .unwrap_or(svtav1_types::chroma::ChromaFormat::Yuv420);
                        t
                    },
                ));
            }
            // Frame header (raw bytes) + tile group with proper header.
            // base_qindex is the SAME value used for quantization, CDF
            // bucket selection and the deblock picker above — the decoder's
            // dequant/CDF init must match the encoder's exactly.
            let fh_bytes = crate::entropy::obu::write_frame_header_full_lr_sb(
                self.width,
                self.height,
                base_qindex,
                is_single_frame && !self.image_sequence,
                chroma.is_none(),
                // The levels applied to the output recon above — signaling
                // and application MUST agree or the recon desyncs from
                // every conforming decoder.
                lf_levels.levels,
                // Signaled loop_filter_sharpness — must match the value the
                // deblock search + application used (fork default 1).
                lf_sharp_eff,
                // The CDEF strengths applied to the output recon above —
                // like the deblock levels, signaling and application MUST
                // agree or the recon desyncs from every conforming decoder.
                &cdef_params.signal(),
                // lr_params: `enabled` MUST equal the SH's
                // enable_restoration bit (spec 5.9.20 gates on it — same
                // SeqTools the SH got); the per-plane types/taps are the
                // ones the tile signals and the output recon had applied.
                &lr_signal,
                sc_signal,
                // Chroma-q deltas: the quantizer above used qindex_u /
                // qindex_v built from EXACTLY these deltas, so signaling and
                // application agree (chroma_q.rs). BOTH modes derive them now
                // — the fork block unconditionally, MAINLINE only under tune
                // IQ (rc_crf_cqp.c's `#else` arm). `None` selects the
                // zero-delta bit pattern, which is what every non-tune-IQ
                // mainline encode still gets.
                //
                // Separate is checked FIRST: under a SH that signalled
                // separate_uv_delta_q = 1 the decoder reads diff_uv_delta (and,
                // with QM, qm_v) even when every delta is zero, and `None`
                // writes neither — a desync. The fork's derived deltas are
                // never all zero (U = V + 12), so this ordering is byte-inert
                // for it; an `__expert` override of (0, 0) on the fork is the
                // case that reached it (tools/chroma_q_override_gate.sh,
                // 2026-09-24: aomdec and dav1d both rejected the stream).
                if self.separate_uv_delta_q() {
                    // The fork's SH signals separate_uv_delta_q = 1, so the FH
                    // carries diff_uv_delta + four independent deltas (its U
                    // delta has a further +12, so U and V really do differ).
                    // An `__expert` override with U != V takes this form too.
                    Some(crate::entropy::obu::ChromaQSignal::Separate([
                        chroma_deltas.u_dc,
                        chroma_deltas.u_ac,
                        chroma_deltas.v_dc,
                        chroma_deltas.v_ac,
                    ]))
                } else if chroma_deltas.is_zero() {
                    None
                } else {
                    // MAINLINE: the SH signals separate_uv_delta_q = 0, so the
                    // FH must NOT write a diff_uv_delta bit — one (dc, ac)
                    // pair, reused for V. C assigns the same value to
                    // delta_q_{dc,ac}[1] and [2] (rc_crf_cqp.c:600-601), which
                    // this asserts rather than assumes.
                    debug_assert_eq!(
                        (chroma_deltas.u_dc, chroma_deltas.u_ac),
                        (chroma_deltas.v_dc, chroma_deltas.v_ac),
                        "mainline chroma-q must be plane-symmetric (SH separate_uv_delta_q = 0)"
                    );
                    Some(crate::entropy::obu::ChromaQSignal::Shared {
                        dc: chroma_deltas.u_dc,
                        ac: chroma_deltas.u_ac,
                    })
                },
                // [SVT_HDR_MODE] per-SB delta-q res (variance boost). The
                // same value gates the walk's per-SB delta symbols.
                delta_q_res_signal,
                // C hardwires `delta_lf_present = 0`
                // (resource_coordination_process.c:434-441).
                false,
                // [SVT_HDR_MODE] frame QM levels (fork enable_qm); None in
                // mainline mode. The quantizers used the SAME levels.
                if qm_levels == [15; 3] {
                    None
                } else {
                    Some(qm_levels)
                },
                film_grain.as_ref(),
                // task #86: real tile rows. tile_rows_log2 was resolved
                // (clamped) before encode_tile_rows/run_entropy_walk ran;
                // tile_size_bytes_minus_1 comes from the SAME walk that
                // produced tile_data (updated alongside every re-walk
                // reassignment above), so the FH's declared TileSizeBytes
                // always matches the tile group's actual size prefixes.
                tile_rows_log2,
                tile_cols_log2,
                tile_size_bytes_minus_1,
                // Task #91: must match the SH's use_128x128_superblock
                // (the FH's tile_info() limits are SB-derived).
                self.sb_size as u32,
                // `frm_hdr->tx_mode`, for THIS arm (`crate::txs_arm`). The
                // allintra arm signals TX_MODE_SELECT unconditionally; the
                // video arm signals it only while `pcs->txs_level != 0`,
                // which is false from preset 10 up — where this used to emit
                // a literal 1 and then code per-block tx_depth symbols that
                // TX_MODE_LARGEST forbids.
                frame_tx_mode_select,
                // `None` on a key frame -> exactly the previous bit layout.
                inter_signal.as_ref(),
            );
            // Diagnostic (SVTAV1_FHDUMP=<path>): dump the raw frame-header
            // bytes (the OBU_FRAME payload prefix before tile data — the FH
            // is byte-aligned at its end, so a prefix compare against the C
            // stream's frame OBU is exact FH byte identity). Consumed by
            // tools/screen_ibc_fh_gate.sh (IBC chunk 1).
            #[cfg(feature = "std")]
            if let Some(path) = std::env::var_os("SVTAV1_FHDUMP") {
                let _ = std::fs::write(path, &fh_bytes);
            }
            // tile_data is already a complete tile_group (with TG header)
            let mut frame_payload = alloc::vec::Vec::new();
            frame_payload.extend_from_slice(&fh_bytes);
            frame_payload.extend_from_slice(&tile_data);
            bs.extend_from_slice(&crate::entropy::obu::write_obu(
                crate::entropy::obu::ObuType::Frame,
                &frame_payload,
            ));
            bs
        };

        // C's deblock search truncates odd chroma dimensions. A decoder
        // filters the ceiling-sized plane. Replay the signaled filters on
        // the output copy after all decisions and entropy coding are complete.
        let mut decoder_output8 = None;
        if decoder_chroma_recon {
            deblock_geom.use_decoder_chroma_bounds();
            macro_rules! decoder_recon {
                ($input:expr, $deblock:path, $cdef:path $(, $depth:expr)?) => {{
                    let (mut y, mut u, mut v) = $input;
                    $deblock(&mut y, &mut u, &mut v, w, h, filter_chroma, &deblock_geom,
                        &lf_levels, lf_sharp_eff $(, $depth)?);
                    let before_cdef = output_restoration.as_ref().map(|_| (y.clone(), u.clone(), v.clone()));
                    $cdef(&mut y, &mut u, &mut v, w, h, filter_chroma, &deblock_geom,
                        &cdef_params $(, $depth)?);
                    if let (Some(info), Some((py, pu, pv))) = (output_restoration.as_ref(), before_cdef) {
                        let bounds = crate::restoration::save_lr_boundaries_bd(
                            &py, &pu, &pv, &y, &u, &v, self.true_width as usize,
                            self.true_height as usize, w, acw, filter_chroma);
                        crate::restoration::apply_restoration_frame_bd(
                            &mut y, &mut u, &mut v, self.true_width as usize,
                            self.true_height as usize, w, acw, filter_chroma, info, &bounds, self.bit_depth);
                    }
                    (y, u, v)
                }};
            }
            if self.bit_depth == 10 {
                if let (Some(y), Some((u, v))) =
                    (self.last_recon10_y.as_ref(), self.last_recon10_uv.as_ref())
                {
                    recon10 = Some(decoder_recon!(
                        (y.clone(), u.clone(), v.clone()),
                        crate::deblock::apply_deblock_frame_hbd,
                        crate::cdef::apply_cdef_frame_hbd,
                        self.bit_depth
                    ));
                }
            } else if let Some(unfiltered) = self.last_recon_unfiltered.as_ref() {
                decoder_output8 = Some(decoder_recon!(
                    unfiltered.clone(),
                    crate::deblock::apply_deblock_frame,
                    crate::cdef::apply_cdef_frame
                ));
            }
        }

        crate::stop_check(&stop)?;

        // Step 7: Publish recon for the recon-parity gate, then update DPB.
        //
        // Superres chunk B.3: what a DECODER outputs is the coded-width recon
        // normatively upscaled back to `upscaled_width` (C
        // `svt_av1_superres_upscale_frame`, cdef_process.c:152 — after CDEF,
        // before loop restoration; LR is off for every config this port lets
        // superres run at, see `superres_config_error`, so "after CDEF" is
        // here). The BITSTREAM is unaffected: nothing downstream of this point
        // codes symbols. No-op when superres is off.
        // The OUTPUT-side upscaled planes (the `Some` arms exist only under
        // superres). `recon` / `decoder_output8` / `recon10` themselves stay
        // at the CODED geometry — the DPB reference built below must carry
        // the picture a decoder predicts from, not the display picture.
        // (This used to upscale `recon` in place; `padded_ref` then read the
        // upscaled buffer at the coded stride — a diagonal smear that made
        // every inter prediction under superres score the wrong reference.)
        let mut out8: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = None;
        let mut out10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = None;
        if self.superres_denom.is_some() {
            let (cw, uw, hh) = (
                self.width as usize,
                self.upscaled_width as usize,
                self.height as usize,
            );
            // Phase uses the coded plane width, while the tile rectangle
            // extends to mi_col_end. Its reconstructed padding participates
            // in the final taps (C av1_upscale_normative_rows).
            let upscale = |src: &[u8],
                           stride: usize,
                           coded: usize,
                           dst: &mut [u8],
                           out: usize,
                           rows: usize| {
                let step = svtav1_dsp::superres::upscale_convolve_step(coded as i32, out as i32);
                let x0 = svtav1_dsp::superres::upscale_convolve_x0(coded as i32, out as i32, step);
                for r in 0..rows {
                    svtav1_dsp::superres::upscale_normative_row(
                        &src[r * stride..],
                        0,
                        stride,
                        &mut dst[r * out..],
                        out,
                        step,
                        x0,
                        svtav1_dsp::superres::TileColPad::FRAME,
                    );
                }
            };
            let upscale_frame =
                |y: &[u8], u: &[u8], v: &[u8]| -> EncodeResult<(Vec<u8>, Vec<u8>, Vec<u8>)> {
                    let mut y_up = svtav1_types::try_vec![0u8; uw * hh]?;
                    upscale(y, cw, self.true_width as usize, &mut y_up, uw, hh);
                    let (mut u_up, mut v_up) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
                    if chroma.is_some() {
                        let (ccw, cuw, chh) = (
                            fmt.chroma_width(cw),
                            fmt.chroma_width(uw),
                            fmt.chroma_height(hh),
                        );
                        let coded = fmt.chroma_width(self.true_width as usize);
                        u_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                        v_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                        upscale(u, ccw, coded, &mut u_up, cuw, chh);
                        upscale(v, ccw, coded, &mut v_up, cuw, chh);
                    }
                    Ok((y_up, u_up, v_up))
                };
            // The decoder-equivalent output takes the replayed-filters
            // canvas when it exists, else the search recon — same preference
            // `last_recon` expresses below.
            out8 = Some(match decoder_output8.as_ref() {
                Some((y, u, v)) => upscale_frame(y, u, v)?,
                None => upscale_frame(&recon, &u_recon, &v_recon)?,
            });
            // The 10-bit output: C runs `av1_highbd_convolve_horiz_rs_c` on
            // the unpacked recon (the `high_bd` arm of
            // `svt_av1_upscale_normative_rows`, super_res.c:289). Same
            // normative taps and border policy on u16; `recon10` stays at
            // coded geometry for `padded_ref_hbd`/`recon_msb8` below.
            if let Some((y10, u10, v10)) = recon10.as_ref() {
                let bd = i32::from(self.bit_depth);
                let upscale_hbd = |src: &[u16],
                                   stride: usize,
                                   coded: usize,
                                   out: usize,
                                   rows: usize|
                 -> EncodeResult<Vec<u16>> {
                    let step =
                        svtav1_dsp::superres::upscale_convolve_step(coded as i32, out as i32);
                    let x0 =
                        svtav1_dsp::superres::upscale_convolve_x0(coded as i32, out as i32, step);
                    let mut dst = svtav1_types::try_vec![0u16; out * rows]?;
                    for r in 0..rows {
                        svtav1_dsp::superres::highbd_upscale_normative_row(
                            &src[r * stride..],
                            0,
                            stride,
                            &mut dst[r * out..],
                            out,
                            step,
                            x0,
                            svtav1_dsp::superres::TileColPad::FRAME,
                            bd,
                        );
                    }
                    Ok(dst)
                };
                let (ccw, cuw, chh) = (
                    fmt.chroma_width(cw),
                    fmt.chroma_width(uw),
                    fmt.chroma_height(hh),
                );
                let coded = fmt.chroma_width(self.true_width as usize);
                out10 = Some((
                    upscale_hbd(y10, cw, self.true_width as usize, uw, hh)?,
                    upscale_hbd(u10, ccw, coded, cuw, chh)?,
                    upscale_hbd(v10, ccw, coded, cuw, chh)?,
                ));
            }
        }

        // C `pad_ref_and_set_flags` again, on the 16-bit picture: at
        // `bit_depth > 8` C's reference IS the 10-bit buffer and
        // `svt_aom_generate_padding16_bit` pads it from the same call site
        // (enc_dec_process.c:1088-1112). Built HERE, before the `recon_output`
        // block, for two reasons: that block MOVES `recon10` into
        // `last_recon10_final`, and it also applies FILM GRAIN, which is an
        // output-only transform that must never reach the DPB.
        let padded_ref_hbd = recon10.as_ref().map(|(y10, u10, v10)| {
            let rb = crate::picture::ref_pic_border(self.sb_size, self.superres_denom.is_some());
            let y = crate::picture::PaddedPlaneHbd::from_plane(y10, w, h, rb);
            let uv = if chroma.is_some() {
                let cb = rb.div_ceil(1 << ss_x);
                let mut cv = crate::picture::PaddedPlaneHbd::from_plane(v10, acw, ach, cb);
                let mut cu = crate::picture::PaddedPlaneHbd::from_plane(u10, acw, ach, cb);
                // Same contiguous `[u][v]` layout as the 8-bit reference —
                // see the `padded_ref` block below.
                cu.extend_tail(&cv.buf);
                let tail = cv.buf.len();
                cv.extend_tail_zeros(tail);
                Some((cu, cv))
            } else {
                None
            };
            crate::picture::PaddedRefHbd { y, uv }
        });

        // The 8-bit reference canvas at `bit_depth > 8`. C does not keep a
        // separately-reconstructed 8-bit recon: the packed 10-bit picture's
        // `y_buffer` holds `recon10 >> 2`, and every 8-bit reader — ME's
        // `enhanced_pic` references, `svt_inter_predictor_light_pd1`'s u8
        // prediction at `hbd_md = 0` — MCs against those MSBs
        // (resource_coordination_process.c:512-560 "10bit packed"). The port
        // instead stored the u8-domain funnel recon (u8 pred + u8-domain
        // dequant), which differs from `recon10 >> 2` by a few LSBs per
        // sample; the resulting inter predictions landed ~2% off C's
        // distortion and flipped near-tie candidate rankings (MEASURED
        // johnny 128x128 p6 bd10 frame 1, mi=(16,8): C ranked compound
        // NEW_NEWMV 36223238 ahead of NEARMV 36574531; the port's canvas
        // scored them 35619078 vs 35276099 and chose the unipred).
        // Downconverting the SAME post-filter 10-bit canvas C packs makes
        // the u8 reference identical by construction.
        let recon_msb8: Option<(
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<u8>,
        )> = recon10.as_ref().map(|(y10, u10, v10)| {
            // Monochrome carries empty chroma planes — keep them empty.
            let down = |p: &[u16], n: usize| -> alloc::vec::Vec<u8> {
                if p.is_empty() {
                    alloc::vec::Vec::new()
                } else {
                    p[..n].iter().map(|&s| (s >> 2) as u8).collect()
                }
            };
            let cn = acw * ach;
            (down(y10, w * h), down(u10, cn), down(v10, cn))
        });

        if self.recon_output {
            // Output-only replay must not alter the DPB or later frame decisions.
            self.last_recon_display_order = Some(display_order);
            let rec_planes = match out8 {
                // Under superres `out8` already holds the upscaled planes of
                // whichever canvas a decoder displays (replayed filters when
                // they ran, search recon otherwise).
                Some(planes) => planes,
                None => decoder_output8
                    .unwrap_or_else(|| (recon.clone(), u_recon.clone(), v_recon.clone())),
            };
            self.recon_frames
                .push_back((display_order, rec_planes.clone()));
            self.last_recon = Some(rec_planes);
            // Issue #13: the 10-bit final recon (deblock -> CDEF -> LR all
            // applied to the 10-bit canvas), normatively upscaled to the
            // output geometry under superres (`out10`); the coded canvas is
            // the output geometry itself when superres is off.
            self.last_recon10_final = out10.or(recon10);
            if let Some(fg) = film_grain.as_ref().filter(|fg| fg.apply_grain) {
                let stride = if self.superres_denom.is_some() {
                    self.upscaled_width as usize
                } else {
                    w
                };
                if self.bit_depth == 10 {
                    if let Some((y, u, v)) = self.last_recon10_final.as_mut() {
                        crate::film_grain_synthesis::add_grain_for_output(
                            fg,
                            [y, u, v],
                            [stride, stride >> ss_x, stride >> ss_x],
                            stride,
                            h,
                            10,
                        );
                    }
                } else if let Some((y, u, v)) = self.last_recon.as_mut() {
                    let mut planes = [
                        y.iter().map(|&v| u16::from(v)).collect::<Vec<_>>(),
                        u.iter().map(|&v| u16::from(v)).collect(),
                        v.iter().map(|&v| u16::from(v)).collect(),
                    ];
                    let [gy, gu, gv] = &mut planes;
                    crate::film_grain_synthesis::add_grain_for_output(
                        fg,
                        [gy, gu, gv],
                        [stride, stride >> ss_x, stride >> ss_x],
                        stride,
                        h,
                        8,
                    );
                    for (dst, src) in [y, u, v].into_iter().zip(planes) {
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s as u8;
                        }
                    }
                }
            }
        }
        // C `pad_ref_and_set_flags` (enc_dec_process.c:1072-1112): the recon
        // is padded with a replicated margin BEFORE it becomes a reference,
        // because inter prediction indexes negative offsets from pixel
        // (0,0). Built here, once, from the same buffers stored below.
        let padded_ref = {
            let (rw, rh) = (w, h);
            // C's reference-picture border is `super_block_size + 32`
            // (enc_handle.c:1212-1217), NOT `scs->border` — see
            // [`crate::picture::ref_pic_border`].
            let rb = crate::picture::ref_pic_border(self.sb_size, self.superres_denom.is_some());
            // The u8 planes the reference exposes are the 10-bit recon's
            // MSBs when a 10-bit canvas exists (`recon_msb8` above); the
            // u8-domain `recon` is only ever the reference on a bd8 encode.
            let (y8, u8r, v8r) = match recon_msb8.as_ref() {
                Some((my, mu, mv)) => (my.as_slice(), mu.as_slice(), mv.as_slice()),
                None => (recon.as_slice(), u_recon.as_slice(), v_recon.as_slice()),
            };
            let y = crate::picture::PaddedPlane::from_plane(y8, rw, rh, rb);
            let uv = if chroma.is_some() {
                // C `(border + (1 << ss_x) - 1) >> ss_x` (:1102-1112).
                let cb = rb.div_ceil(1 << ss_x);
                let mut cv =
                    crate::picture::PaddedPlane::from_plane(v8r, rw >> ss_x, rh >> ss_y, cb);
                let mut cu =
                    crate::picture::PaddedPlane::from_plane(u8r, rw >> ss_x, rh >> ss_y, cb);
                // C's recon `buffer_alloc` is `[y][u][v]` contiguous: a
                // maximally UMV-clamped chroma read past `u`'s region
                // answers with `v`'s margin bytes. `v` is the last
                // region, so its overread runs off the calloc — the
                // zero tail keeps the port deterministic there.
                cu.extend_tail(&cv.buf);
                let tail = cv.buf.len();
                cv.extend_tail_zeros(tail);
                Some((cu, cv))
            } else {
                None
            };
            alloc::boxed::Box::new(crate::picture::PaddedRef {
                y,
                uv,
                hbd: padded_ref_hbd,
            })
        };
        // C `rest_process.c:347-349`, run on EVERY coded picture (the RC
        // reads them even for a non-reference frame) — `intra_coded_area` is
        // forced to 0 on an I_SLICE there, which is why a key frame stores
        // only its skip/hp areas.
        let coded_area_pct = frame_coded_area
            .borrow()
            .as_ref()
            .map_or((0, 0, 0), |a| a.percentages(w, h, is_key));
        // The join to C's `SVT_REFSTATS_OUT` interposer, which prints the
        // reference object's `slice_type/intra/skip/hp` for the frame that
        // READS it. This prints the same four for the frame that WRITES it,
        // so `REFSTATS poc=N ... l0=a/b/c/d` on the C side must equal
        // `PORTREFSTATS poc=N-1 slice=a intra=b skip=c hp=d` here.
        #[cfg(feature = "std")]
        if crate::dbgenv::refstats() {
            let (i_pct, s_pct, h_pct) = coded_area_pct;
            let acc = frame_coded_area.borrow();
            std::eprintln!(
                "PORTREFSTATS poc={display_order} slice={} intra={i_pct} skip={s_pct} hp={h_pct} \
                 mfmv={}/{} none={} sbintra=[{}] sbskip=[{}]",
                u8::from(is_key),
                // The MFMV writeback's census: cells naming a real reference,
                // out of the field's length. It is the positive control that
                // `av1_copy_frame_mvs` actually fired — a wire whose only
                // observable is a LATER frame the port still refuses would
                // otherwise be untestable. Zero on a key frame BY C'S GATE.
                acc.as_ref().map_or(0, |a| a
                    .mvs
                    .iter()
                    .filter(|m| m.ref_frame > crate::port_coding_loop::INTRA_FRAME)
                    .count()),
                acc.as_ref().map_or(0, |a| a.mvs.len()),
                // NONE cells: an intra block must reset its cells so a later
                // frame does not project stale motion through them.
                acc.as_ref().map_or(0, |a| a
                    .mvs
                    .iter()
                    .filter(|m| m.ref_frame <= crate::port_coding_loop::INTRA_FRAME)
                    .count()),
                acc.as_ref().map_or(alloc::string::String::new(), |a| a
                    .sb_intra
                    .iter()
                    .map(|v| alloc::format!("{v}"))
                    .collect::<alloc::vec::Vec<_>>()
                    .join(",")),
                acc.as_ref().map_or(alloc::string::String::new(), |a| a
                    .sb_skip
                    .iter()
                    .map(|v| alloc::format!("{v}"))
                    .collect::<alloc::vec::Vec<_>>()
                    .join(",")),
            );
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            let acc = frame_coded_area.borrow();
            eprintln!(
                "REFSTORE poc={display_order} intra={:?} skip={:?}",
                acc.as_ref().map(|a| a.sb_intra.as_slice()),
                acc.as_ref().map(|a| a.sb_skip.as_slice()),
            );
        }
        let ref_frame = ReferenceFrame {
            padded: Some(padded_ref),
            // C `EbReferenceObject::global_motion` — what a later frame that
            // names this picture in `primary_ref_frame` delta-codes against.
            // C substitutes IDENTITY for an I_SLICE reference at READ time
            // (pic_manager_process.c:833); this stores IDENTITY for a key frame
            // at WRITE time, which is the same value with one fewer place to
            // get the slice type wrong.
            global_motion: if is_key {
                [svtav1_types::motion::WarpedMotionParams::default(); 8]
            } else {
                gm_field
            },
            // The stored planes are the same canvas `padded` pads — the
            // 10-bit recon's MSBs at bd10 (`recon_msb8`), the u8-domain
            // recon otherwise.
            y_plane: recon_msb8
                .as_ref()
                .map_or_else(|| recon.clone(), |(my, _, _)| my.clone()),
            // 4:2:0 chroma recon, empty on the monochrome path. Inter
            // prediction needs all three planes; see `ReferenceFrame::u_plane`.
            u_plane: if chroma.is_some() {
                recon_msb8
                    .as_ref()
                    .map_or_else(|| u_recon.clone(), |(_, mu, _)| mu.clone())
            } else {
                alloc::vec::Vec::new()
            },
            v_plane: if chroma.is_some() {
                recon_msb8
                    .as_ref()
                    .map_or_else(|| v_recon.clone(), |(_, _, mv)| mv.clone())
            } else {
                alloc::vec::Vec::new()
            },
            // C `rest_process.c:207-210`: the strengths the FRAME HEADER
            // signalled, not the ones the search proposed. A later frame's
            // CDEF candidate set is rewritten from these
            // (`update_cdef_filters_on_ref_info`).
            sb_min_sq_size: sb_min_sq_sizes,
            sb_max_sq_size: sb_max_sq_sizes,
            // C `copy_statistics_to_ref_obj_ect` (rest_process.c:190-220):
            // the NORMALISED percentages (`:347-349`) and the per-superblock
            // flags this picture's walk accumulated. All zero / empty on
            // every allintra cell, where the walk never armed the
            // accumulator — and C's own readers return 0 / 0 / -1 for an
            // I_SLICE reference regardless of what it stored.
            intra_coded_area: coded_area_pct.0,
            skip_coded_area: coded_area_pct.1,
            hp_coded_area: coded_area_pct.2,
            sb_intra: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_intra.clone())
                .unwrap_or_default(),
            sb_skip: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_skip.clone())
                .unwrap_or_default(),
            // C `copy_statistics_to_ref_obj_ect` (rest_process.c:190-220)
            // also stores this picture's open-loop ME per-SB distortions, so a
            // later frame's `lpd1_detector_skip_pd0` can compare its own
            // `me_64x64_distortion`/`me_8x8_cost_variance` against them.
            // Empty on a key frame, where `frame_me` never ran.
            sb_me_64x64_dist: frame_me
                .as_ref()
                .map(|m| m.per_b64.iter().map(|b| b.me_64x64_distortion).collect())
                .unwrap_or_default(),
            sb_me_8x8_cost_var: frame_me
                .as_ref()
                .map(|m| m.per_b64.iter().map(|b| b.me_8x8_cost_variance).collect())
                .unwrap_or_default(),
            // C `rest_process.c:216` copies `pcs->sb_64x64_mvp` to the ref —
            // the same `update_b`-accumulated array as `sb_intra`/`sb_skip`.
            sb_64x64_mvp: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.sb_64x64_mvp.clone())
                .unwrap_or_default(),
            // C `EbReferenceObject::slice_type`.
            is_islice: is_key,
            // C `enc_dec_process.c:1248-1252` — the ref object takes the
            // signalled `base_q_idx` and `ppcs->r0` (post `crf_qindex_calc`
            // adjustment; 0 when TPL did not run this frame) so a LATER
            // frame's `ref_base_q_idx`/`ref_pic_r0` reads them back.
            base_q_idx: base_qindex,
            r0: tpl_r0,
            cdef_y_strengths: cdef_params.strengths.iter().map(|s| s.0).collect(),
            cdef_uv_strengths: cdef_params.strengths.iter().map(|s| s.1).collect(),
            // C `packetization_process.c:741-744`: reset the CDF symbol
            // counters, THEN copy into the reference object. The reset is not
            // cosmetic — `update_cdf` reads `cdf[nsymbs]` to choose the
            // adaptation RATE, so a saved state that kept a frame's final
            // counts would make the next frame adapt at the slow late-frame
            // rate from its first symbol.
            frame_cdfs: walk_end_cdfs.borrow_mut().take().map(|mut c| {
                c.reset_symbol_counters();
                #[cfg(feature = "std")]
                if let Some(path) = std::env::var_os("SVTAV1_FCTX_OUT") {
                    // Same format and field order as the C oracle's
                    // `__wrap_svt_av1_reset_cdf_symbol_counters`
                    // (tools/capture_c_trace/wrap_recon.c), so
                    // tools/fctx_diff.py can compare them directly.
                    c.dump_to(&path, display_order as u32);
                }
                alloc::sync::Arc::new(c)
            }),
            // C `rest_process.c:200-204`: the loop-filter levels this frame's
            // HEADER signalled, plus the SSE-improvement measure. The next
            // frame's deblock level is derived from BOTH — see
            // `crate::dlf_arm`.
            lf_levels: lf_levels.levels,
            dlf_dist_dev,
            // C `rest_process.c:205`: `obj->cdef_dist_dev =
            // pcs->cdef_dist_dev` — the search-path measurement, -1 on the
            // fast paths, 0 whenever the signalled strengths are all zero.
            cdef_dist_dev,
            width: self.width,
            height: self.height,
            display_order,
            order_hint: display_order as u32,
            // C `av1_copy_frame_mvs`'s output, which `update_b` wrote into
            // this picture's own reference object during the walk. EMPTY on a
            // key frame and on every allintra cell, exactly where C's gate is
            // false — see `CodedAreaAcc::mfmv_active`.
            mvs: frame_coded_area
                .borrow()
                .as_ref()
                .map(|a| a.mvs.clone())
                .unwrap_or_default(),
            // C `EbReferenceObject::ref_order_hint[0..7]`
            // (`rest_process.c` / `pad_ref_and_set_flags`): the order hints of
            // THIS picture's own references, which a LATER picture's
            // `motion_field_projection` reads to scale a saved MV. All zero on
            // a key frame, which has none.
            ref_order_hint: {
                let mut a = [0i32; 7];
                if let Some(pic) = pic_decision.as_ref() {
                    for (i, oh) in a.iter_mut().enumerate() {
                        let slot = pic.rps.ref_dpb_index[i] as usize;
                        *oh = self.dpb.get(slot).map_or(0, |r| r.order_hint as i32);
                    }
                }
                a
            },
            // C `EbReferenceObject::tmp_layer_idx` — this picture's own
            // `temporal_layer_index` (enc_dec_process.c:2149's reader
            // compares a LATER frame's layer against it). 0 on every flat
            // low-delay frame.
            temporal_layer,
        };
        #[cfg(feature = "std")]
        if let Some(path) = std::env::var_os("SVTAV1_MVS_OUT") {
            // Diagnostic twin of the vendored-libaom `MVS` dump: the stored
            // per-8x8 motion field plus the saved ref_order_hint array, so a
            // field-by-field diff can separate "stored state diverged" from
            // "projection consumed it differently".
            use std::io::Write as _;
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let mut w = std::io::BufWriter::new(file);
                let mut line = alloc::string::String::with_capacity(32 + ref_frame.mvs.len() * 14);
                core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!("MVS {} {}", ref_frame.order_hint, ref_frame.mvs.len()),
                )
                .ok();
                for m in &ref_frame.mvs {
                    core::fmt::Write::write_fmt(
                        &mut line,
                        format_args!(" {},{}", m.ref_frame, m.mv.as_int()),
                    )
                    .ok();
                }
                line.push('\n');
                core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!("MVSROH {}", ref_frame.order_hint),
                )
                .ok();
                for oh in ref_frame.ref_order_hint {
                    core::fmt::Write::write_fmt(&mut line, format_args!(" {oh}")).ok();
                }
                line.push('\n');
                let _ = w.write_all(line.as_bytes());
            }
        }
        #[cfg(feature = "std")]
        if crate::dbgenv::mfmv_dbg() {
            let mut named = 0usize;
            let mut hash: u64 = 1469598103934665603;
            for m in &ref_frame.mvs {
                if m.ref_frame > 0 {
                    named += 1;
                }
                hash = (hash ^ (m.mv.as_int() as u32 as u64)).wrapping_mul(1099511628211);
                hash = (hash ^ u64::from(m.ref_frame as u8)).wrapping_mul(1099511628211);
            }
            std::eprintln!(
                "RS_MVSAVE poc={display_order} ftype={} oh={} roh={},{},{},{},{},{},{} \
                 cells={} named={named} hash={hash:x}",
                u8::from(!is_key),
                ref_frame.order_hint,
                ref_frame.ref_order_hint[0],
                ref_frame.ref_order_hint[1],
                ref_frame.ref_order_hint[2],
                ref_frame.ref_order_hint[3],
                ref_frame.ref_order_hint[4],
                ref_frame.ref_order_hint[5],
                ref_frame.ref_order_hint[6],
                ref_frame.mvs.len(),
            );
        }
        for slot in 0..8 {
            if pcs.refresh_frame_flags & (1 << slot) != 0 {
                self.grain_references[slot] = film_grain.clone();
            }
        }
        self.grain_sequence_present = Some(seq_tools.film_grain_params_present);
        self.dpb.refresh(pcs.refresh_frame_flags, ref_frame);
        // The PA (picture-analysis) reference the NEXT frame's open-loop
        // motion search reads — this frame's padded SOURCE pyramid, not its
        // recon. `None` in still mode, where no later frame exists.
        if let Some(cur) = pa_cur {
            let cur = alloc::sync::Arc::from(cur);
            // The pyramid `pa_ref` displaces is two frames back: the search
            // only ever reads `pa_ref` (the PREVIOUS frame) against `pa_cur`,
            // so nothing can still be looking at it UNLESS a GM slot below
            // still names it. `Arc::into_inner` answers exactly that question,
            // and gives the allocation back when the answer is no.
            if let Some(old) = self.pa_ref.take() {
                self.pa_scratch = alloc::sync::Arc::into_inner(old).map(alloc::boxed::Box::new);
            }
            // Mirror the DPB refresh into the PA slots, so a later frame's
            // global-motion search can reach the plane for ANY reference its
            // `ref_dpb_index` names — not only the nearest one.
            //
            // This used to be gated on `gm_level_for_frame(false) != 0`,
            // because GM was the slots' only reader and the pyramids are not
            // free to retain. From 2026-09-13 the open-loop ME itself is a
            // reader: C searches `ref_list0_count_try` references
            // (`me_process.c:212`), so a frame 2 with `l0cnt = 2` needs BOTH
            // frame 1's and frame 0's pyramids — not only `pa_ref`'s.
            for slot in 0..8 {
                if pcs.refresh_frame_flags & (1 << slot) != 0 {
                    let evicted = self.pa_slots[slot].replace(alloc::sync::Arc::clone(&cur));
                    if self.pa_scratch.is_none()
                        && let Some(e) = evicted
                    {
                        self.pa_scratch =
                            alloc::sync::Arc::into_inner(e).map(alloc::boxed::Box::new);
                    }
                }
            }
            self.pa_ref = Some(cur);
        }
        // This frame's ME results have been consumed by mode decision and the
        // pack; keep the allocation for the next frame's search.
        self.me_scratch = frame_me;

        crate::stop_check(&stop)?;

        // C `rc_process_packetization_feedback`'s one-pass CBR arm
        // (rc_process.c:758-792): the packetized bit count
        // (`output_stream_ptr->n_filled_len << 3`,
        // packetization_process.c:818) feeds `svt_av1_rc_postencode_update`,
        // then `svt_aom_update_rc_counts` advances the counters.
        // `n_filled_len` is the bitstream buffer WITHOUT the temporal
        // delimiter — `svt_aom_encode_td_av1` writes it into the reorder
        // queue, not `pcs->bitstream_ptr` — so the 2-byte `12 00` TD header
        // this port prepends to every TU comes back out of the count.
        // `avg_cnt_zeromv` is the `rest_process.c:350` normalization of the
        // `update_b`-accumulated zero-MV area.
        if let Some(frame) = cbr_frame_rc.as_mut() {
            let avg_cnt_zeromv = frame_coded_area
                .borrow()
                .as_ref()
                .map_or(0, |a| {
                    let n = (w * h) as u64;
                    if n == 0 { 0 } else { 100 * a.zeromv_area / n }
                });
            self.cbr_postencode(
                frame,
                bitstream.len().saturating_sub(2) as u64 * 8,
                avg_cnt_zeromv,
            );
        }
        // Step 8: Update rate control state
        update_rc_state(&mut self.rc_state, bitstream.len() as u64 * 8, pcs.qp);

        // Task #6 chunk 1 — no silent 8-bit fallback. If the caller supplied a
        // native 10-bit source and NO bd10 stage read it (an out-of-envelope
        // tree turned the level post-pass off at runtime, say), the bytes
        // above encode the MSB-truncated content. Emitting them would look
        // exactly like a real 10-bit encode, so fail loudly instead. The u8
        // path never takes this branch (`hbd_source` is `None` there), and the
        // frame counter is left un-advanced so the caller can retry a
        // supported config on the same pipeline.
        if hbd_source.is_some() && !hbd_used {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit source went unconsumed (the bd10 level re-encode was skipped for \
                 this frame's partition trees) — the encode would have silently truncated to 8 \
                 bits; see docs/hbd-input-port-map.md chunk 2",
            )));
        }
        self.frame_count += 1;
        Ok(bitstream)
    }
}

/// Encode tile rows, returning per-tile recon buffers.
///
/// When the `std` feature is enabled and there are multiple tile rows,
/// uses `std::thread::scope` for parallel encoding. Otherwise sequential.
/// C `svt_get_palette_cache_y` (palette.c:164-210): merge the above/left
/// neighbors' luma palettes into one sorted, deduped color cache for the
/// palette-color writer/cost fn. Above is DROPPED when `block_y` is at an
/// SB (64px) row top (C: `row % (1 << MIN_SB_SIZE_LOG2)` via
/// `-xd->mb_to_top_edge`, `MIN_SB_SIZE_LOG2 == 6`) — a rule specific to
/// this cache, NOT to [`EntropyCtx::palette_neighbor_ctx`]'s flag context.
/// Ties in the merge advance both cursors, keeping the ABOVE value (C's
/// `else` branch runs first and additionally drains `left` on equality).
// Consumed on BOTH sides now (#71, 2026-07-18): the MD `evaluate_leaf`
// reads this cache (via `commit_leaf`'s per-block `record_palette` stamp,
// coding order) into `search_palette_luma` + the cache-aware colour cost,
// and the PACK walk reads it for the palette-colour writer. On
// screen-content frames (EPICA) `above_palette`/`left_palette` DO carry
// nonzero sizes and the merge loop runs; on non-sc content no leaf wins a
// palette so it stays on the empty-cache early return (`above_n == 0 &&
// left_n == 0`), keeping those gates byte-identical.
pub(crate) fn palette_cache(
    ectx: &EntropyCtx,
    block_x: usize,
    block_y: usize,
) -> alloc::vec::Vec<u16> {
    let x4 = block_x / 4;
    let y4 = block_y / 4;
    let mut above_n = if !block_y.is_multiple_of(64) && x4 < ectx.above_palette.len() {
        ectx.above_palette[x4] as usize
    } else {
        0
    };
    let mut left_n = if y4 < ectx.left_palette.len() {
        ectx.left_palette[y4] as usize
    } else {
        0
    };
    if above_n == 0 && left_n == 0 {
        return alloc::vec::Vec::new();
    }
    let above_colors: &[u16] = if above_n > 0 {
        &ectx.above_palette_colors[x4][..above_n]
    } else {
        &[]
    };
    let left_colors: &[u16] = if left_n > 0 {
        &ectx.left_palette_colors[y4][..left_n]
    } else {
        &[]
    };
    let mut cache = alloc::vec::Vec::with_capacity(above_n + left_n);
    fn add(cache: &mut alloc::vec::Vec<u16>, v: u16) {
        // palette_add_to_cache (palette.c:154-161): skip a value equal to
        // the LAST entry already in the (ascending) cache.
        if cache.last() == Some(&v) {
            return;
        }
        cache.push(v);
    }
    let (mut ai, mut li) = (0usize, 0usize);
    while above_n > 0 && left_n > 0 {
        let v_above = above_colors[ai];
        let v_left = left_colors[li];
        if v_left < v_above {
            add(&mut cache, v_left);
            li += 1;
            left_n -= 1;
        } else {
            add(&mut cache, v_above);
            ai += 1;
            above_n -= 1;
            if v_left == v_above {
                li += 1;
                left_n -= 1;
            }
        }
    }
    while above_n > 0 {
        add(&mut cache, above_colors[ai]);
        ai += 1;
        above_n -= 1;
    }
    while left_n > 0 {
        add(&mut cache, left_colors[li]);
        li += 1;
        left_n -= 1;
    }
    debug_assert!(cache.len() <= 2 * svtav1_types::prediction::PALETTE_MAX_SIZE);
    cache
}

#[cfg(test)]
mod tests;

/// The INTER TILE byte gate — inter campaign, the first byte-level evidence
/// about frame 1's tile.
///
/// # What is under test, and what is NOT
///
/// Under test: the CDF CONTINUATION restore ([`crate::port_frame_cdf`]) and
/// the inter mode-info writer
/// ([`crate::port_entropy_inter::block::write_inter_mode_info`]) together, end
/// to end, against C's actual tile bytes.
///
/// **NOT** under test: MODE DECISION. The block decision fed in below is C's
/// own, MEASURED off the reference encoder — not guessed from the bytes and
/// not produced by this port. That separation is the point: until the inter
/// branch of MD exists there is no other way to learn whether the entropy path
/// is right, and "three bytes came out equal after trying some motion vectors"
/// would be curve-fitting, not evidence.
///
/// # Where the decision came from
///
/// `tools/capture_c_trace/wrap_recon.c`'s `SVT_CINTER_OUT` dump, added with
/// this gate, prints the committed `BlockModeInfo` + `BlkStruct` fields that
/// `write_inter_mode_info` reads, from inside `svt_aom_update_mi_map`. On
/// `gradient 64x64 q40 p6 frames=2` (`tools/identity_diff_inter.sh`,
/// `SVTAV1_FRAME_SHIFT=3`) the committed 64x64 block is:
///
/// ```text
/// CINTER poc=1 mi=(0,0) bsize=12 part=0 mode=16 rf=1,-1 mv0=0,-24 pmv0=0,0
///        interp=0x0 mm=0 npr=0 ovl=0 imc=8 drl=0 drlctx=-1,-1 drlnear=0,0
///        iiu=0 skip=1 skipmode=0 cgi=0 cidx=0
/// ```
///
/// One 64x64 `PARTITION_NONE` block, `NEWMV` off `LAST_FRAME`, MV `(0, -24)`
/// eighth-pel — exactly the 3-pixel horizontal translation the harness
/// applies — predicted from `(0, 0)`, `EIGHTTAP_REGULAR`, `skip = 1`. The
/// whole frame.
///
/// That is independently corroborated by which CDFs C's tile ADAPTS: comparing
/// C's saved frame-0 and frame-1 contexts (the `SVT_FCTX_OUT` oracle) shows
/// exactly `partition`, `skip`, `intra_inter`, `comp_inter`, `single_ref`,
/// `newmv`, `switchable_interp`, `nmvc.joints` and `nmvc.comp1.*` moving — no
/// `refmv`, no `drl`, no `motion_mode`, no coefficient CDF, and only the
/// COLUMN component of the MV context. A dump and a completely different
/// measurement agreeing on the symbol set is what makes this a decision rather
/// than a fit.
#[cfg(test)]
mod inter_tile_byte_gate;

/// **How far does the PORT'S OWN decision get on the inter cell?**
///
/// `inter_tile_byte_gate` above proves the ENTROPY path by feeding C's
/// MEASURED block decision through the port's writers. That leaves one
/// question open, and it is the whole remaining campaign: does the port,
/// running its OWN ported machinery, arrive at that decision?
///
/// This module answers it field by field, for the two pieces that are
/// ported and gated but unwired — the MVP stack (`crate::inter_mvp`,
/// tier-1 against `svt_av1_find_best_ref_mvs_from_stack` /
/// `setup_ref_mv_list`) and the DRL/pred-MV chooser
/// (`crate::port_md::drl`, tier-1 against
/// `svt_aom_choose_best_av1_mv_pred`). It does NOT prove the pipeline
/// reaches them; `docs/INTER-ENCODE-PLAN.md` §1s item 1 is that wiring.
/// What it proves is that when they ARE reached with this cell's inputs,
/// they produce C's numbers — so a divergence found later is a WIRING
/// defect, not a translation one.
///
/// The C values it is checked against are the `SVT_CINTER_OUT` dump quoted
/// on `inter_tile_byte_gate`:
///
/// ```text
/// mode=16 rf=1,-1 mv0=0,-24 pmv0=0,0 imc=8 drl=0 drlctx=-1,-1 drlnear=0,0
/// ```
#[cfg(test)]
mod inter_decision_probe;

#[cfg(test)]
#[path = "film_grain_pipeline_tests.rs"]
mod film_grain_tests;

mod entropy_ctx;
pub(crate) use entropy_ctx::*;

mod lpd1;
pub(crate) use lpd1::*;

mod block_syntax;
use block_syntax::*;

mod partition_walk;
pub(crate) use partition_walk::*;

mod tile_walk;
use tile_walk::*;

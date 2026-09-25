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

use crate::picture::{DecodedPictureBuffer, GopStructure, PictureControlSet, ReferenceFrame};
use crate::rate_control::{RcConfig, RcState, assign_picture_qp, update_rc_state};
use crate::speed_config::SpeedConfig;
use crate::{EncodeError, EncodeResult};
use alloc::vec;
mod bd10_reencode;
use bd10_reencode::{bd10_reencode_chroma, bd10_reencode_luma};

use alloc::boxed::Box;
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

// `scs->static_config.fast_decode`. The port has no fast-decode
// config; C's default is 0. Both dlf ladders take their first arm on
// `fast_decode <= 1`, so the dlf resolution is currently unread —
// it is passed faithfully so the fast-decode arm stays correct if that
// config ever lands.
const DLF_FAST_DECODE: u8 = 0;

// `scs->seq_header.cdef_level` is 1 in this port (obu.rs writes
// `enable_cdef = 1` unconditionally) and there is no `--cdef-level`
// config, so both ladders take their derived arm.
const SEQ_CDEF_LEVEL: u8 = 1;

// `scs->static_config.fast_decode` — the port carries no fast-decode
// config and C's default is 0, the same value as `DLF_FAST_DECODE`.
const CDEF_FAST_DECODE: u8 = 0;

impl EncodePipeline {
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
        let stale_vars = derive_stale_vars(stats_src);
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
        let sb_chroma_owned = build_sb_chroma(chroma, fmt, acw, ach, ext_w, ext_h)?;
        let hbd_sb_owned = build_hbd_sb(
            &hbd_source,
            w,
            h,
            fmt,
            acw,
            ach,
            ext_w,
            ext_h,
            &sb_input_owned,
        )?;

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
        let mut sc_derivation = self.derive_screen_content(sc_arm, w, h, &encode_input, sc_preset);
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
            pa.avg_luma = pic_decision
                .as_ref()
                .map_or(crate::port_picstruct::INVALID_LUMA, |p| p.avg_luma);
        }
        // The TPL stage already ran this picture's open-loop ME against the
        // same reference pyramids with the same `FrameMeParams` — C's
        // `pa_me_data->me_results`, shared between `tpl_mc_flow` and the
        // picture's own encode. Reuse it verbatim; a member the stage
        // skipped (I slice, or a reference pyramid it could not resolve)
        // falls through to the sequential computation, whose `None`/`Some`
        // answer is then identical.
        let frame_me = self.resolve_frame_me(
            display_order,
            is_key,
            sc_arm,
            &pic_decision,
            &mut tpl_in,
            frame_hier,
            w,
            h,
            sc_derivation,
            &pa_cur,
        );

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
        let mut base_qindex = self.derive_base_qindex(
            display_order,
            is_key,
            &pic_decision,
            frame_hier,
            sc_derivation,
            tpl_adjusted_qp,
            &frame_me,
            &mut cbr_frame_rc,
            &mut cbr_sb_plan,
        )?;
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
        self.apply_tpl_qp(
            is_key,
            &pic_decision,
            temporal_layer,
            frame_hier,
            md_ref_intra_percentage,
            sc_derivation,
            &mut base_qindex,
            allintra,
            tpl_fti,
            &mut tpl_r0,
        );
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
        let mut sb_plan = self.plan_sb_qindex(
            w,
            h,
            &encode_input,
            sb_input,
            in_stride,
            &mut tpl_adjusted_qp,
            cbr_sb_plan,
            &mut base_qindex,
            &mut picture_qp,
        )?;

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
        let (primary_ref_frame_for_cdf, primary_ref_cdfs) =
            match self.resolve_primary_ref_cdfs(is_key, &pic_decision, base_qindex) {
                Ok(value) => value,
                Err(value) => return value,
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
        let gm_estimation = self.estimate_gm(is_key, &pic_decision, temporal_layer, &frame_me);
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
        let gm_models = self.resolve_gm_models(
            display_order,
            is_key,
            decided_is_some,
            &pic_decision,
            temporal_layer,
            w,
            h,
            &encode_input,
            &frame_me,
            gm_estimation,
        );
        // Printed BEFORE the refusal below, deliberately: the frame whose
        // derivation a join gate most needs to see is exactly the one the
        // refusal stops (`tools/gm_join_gate.sh`).
        dump_gm(display_order, &frame_me, gm_estimation, gm_models);

        if let Some(why) = Self::gm_search_config_error(gm_estimation.as_ref(), gm_models.as_ref())
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(why)));
        }
        let mut c_quant = self.build_coding_quant(
            &stale_vars,
            is_key,
            sc_arm,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            filter_chroma,
            sb_input,
            in_stride,
            tpl_adjusted_qp,
            &frame_me,
            base_qindex,
            picture_qp,
            lw_bump,
            coded_lossless,
        );

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
        let ssim_rdmult = self.derive_ssim_rdmult(
            is_key,
            sc_arm,
            temporal_layer,
            frame_hier,
            md_lambda_base_update_type,
            md_alt_lambda_factors,
            w,
            h,
            &encode_input,
            base_qindex,
            r0_delta_qp_md,
            delta_q_present,
        );
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
        let qm_levels = self.derive_qm_levels(base_qindex, coded_lossless, chroma_deltas);
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
        let seq_tools = self.derive_seq_tools(zen_intra_edge_filter, is_single_frame);

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
        let pipeline_md_inputs = self.build_md_inputs(
            is_key,
            sc_arm,
            &pic_decision,
            temporal_layer,
            frame_hier,
            sc_derivation,
            base_qindex,
            picture_qp,
            &c_quant,
            seq_tools,
        );
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
        let ref_gm_field = self.derive_ref_gm_field(&pic_decision, primary_ref_frame_for_cdf);

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
        let inter_syntax_state = self.build_inter_syntax_state(
            display_order,
            &pic_decision,
            seq_tools,
            md_config_signals,
            gm_field,
        );

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
        let mut inter_mvp_env = self.build_inter_mvp_env(
            display_order,
            &pic_decision,
            w,
            h,
            sb_size,
            gm_field,
            &inter_syntax_state,
            &mut inter_ref_frame_side,
        );

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

        let inter_md_frame = Self::build_inter_md_frame(
            self.speed_config.preset,
            self.bit_depth,
            self.rc_config.qp,
            self.mrp_ctrls.use_best_references,
            self.hdr.tx_bias,
            self.hdr.tune,
            self.hdr.alt_ssim_tuning,
            self.hdr.ac_bias,
            &pic_decision,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            sb_input,
            in_stride,
            sc_derivation,
            &frame_me,
            base_qindex,
            picture_qp,
            delta_q_plan,
            &primary_ref_cdfs,
            sb_size,
            ref_padded_luma,
            md_config_signals,
            gm_field,
            gm_skip_identity,
            gm_enabled,
            inter_cand_reduction,
            &inter_syntax_state,
            &inter_mvp_env,
            inter_padded_by_ref,
            &inter_ref_types,
        )?;

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
        let sb_inter_lambda = self.derive_sb_inter_lambda(
            &stop,
            sc_arm,
            &pic_decision,
            temporal_layer,
            frame_hier,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            &frame_me,
            base_qindex,
            r0_delta_qp_md,
            picture_qp,
            lw_bump,
            delta_q_present,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            &inter_md_frame,
        )?;

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
        let pd0_min_sq = self.derive_pd0_min_sq(
            display_order,
            &stop,
            &pic_decision,
            w,
            h,
            &frame_me,
            base_qindex,
            r0_delta_qp_md,
            picture_qp,
            delta_q_present,
            md_sb_qindex,
            sb_size,
            sb_cols,
            sb_rows,
            last_ref_slot,
            md_config_signals,
            &inter_md_frame,
            &sb_inter_lambda,
            &mut pd0_dr_res,
        )?;

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
        let lpd1_frame = self.build_lpd1_frame(
            sc_arm,
            &pic_decision,
            &pipeline_md_inputs,
            md_config_signals,
        );

        let ref_min_max_sq = self.derive_ref_min_max_sq(&pic_decision, &last_ref);

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
        fill_canvas10(
            w,
            h,
            ss_x,
            ss_y,
            acw,
            sb_size,
            tile_grid,
            &tile_recons,
            &mut canvas10,
        );

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
        Self::bd10_post_pass(
            self.bit_depth,
            self.speed_config.preset,
            self.hdr.sharpness,
            self.hdr.tune,
            &mut self.last_recon10_y,
            &mut self.last_recon10_uv,
            chroma,
            &hbd_source,
            &mut hbd_used,
            is_key,
            sc_arm,
            md_lambda_base_update_type,
            md_lambda_factor_update_type,
            md_alt_lambda_factors,
            lambda_mod_intra,
            w,
            h,
            fmt,
            acw,
            ach,
            sb_input,
            in_stride,
            &sb_chroma_owned,
            hbd_sb_owned,
            base_qindex,
            picture_qp,
            lw_bump,
            coded_lossless,
            &primary_ref_cdfs,
            &c_quant,
            sb_size,
            sb_cols,
            tile_grid,
            qindex_u,
            qindex_v,
            qm_levels,
            seq_tools,
            inter_md_frame,
            sb_inter_lambda,
            sb_enc_rdoq,
            &mut all_trees,
        )?;

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
            Self::entropy_walk(
                self.speed_config.preset,
                self.superres_denom,
                self.chroma_format,
                self.bit_depth,
                chroma,
                &stop,
                is_key,
                sc_arm,
                &pic_decision,
                w,
                h,
                n,
                &sb_chroma_owned,
                sc_derivation,
                frame_tx_mode_select,
                base_qindex,
                delta_q_plan,
                md_sb_qindex,
                &primary_ref_cdfs,
                &c_quant,
                sb_size,
                sb_cols,
                sb_rows,
                ref_padded_luma,
                tile_grid,
                chroma_deltas,
                qindex_u,
                qindex_v,
                delta_q_res_signal,
                qm_levels,
                seq_tools,
                md_config_signals,
                &inter_syntax_state,
                inter_ref_frame_side,
                &inter_mvp_env,
                &all_trees,
                cw,
                ext_cbuf,
                lr_true_w,
                lr_true_h,
                &frame_coded_area,
                &walk_end_cdfs,
                lr,
                cdef_walk,
                recon_only,
            )
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
            deblock_geom,
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
        self.dump_recon10_bin();
        let mut recon10 = self.take_recon10(chroma);
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
        let dlf_level = derive_dlf_level(
            is_single_frame,
            dlf_enc_mode,
            dlf_resolution,
            dlf_is_base,
            dlf_is_not_last_layer,
            dlf_ref_skip_percentage,
        );
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
        let dlf_refs = self.collect_dlf_refs(is_key, &pic_decision);
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
        let dlf_pick_inputs = self.build_dlf_pick_inputs(
            is_key,
            &pic_decision,
            frame_hier,
            base_qindex,
            dlf_resolution,
            dlf_temporal_layer_index,
            dlf_ctrls,
            &dlf_refs,
            dlf_avg_me_sad,
        );
        // C `dlf_process.c:89-91` seeds all three to -1 ("not computed"); only
        // the non-SB-based path overwrites them, so a frame that takes the
        // by-q arm genuinely has no measurement and its `dlf_dist_dev` must
        // stay -1 for the NEXT frame to skip rather than average.
        let mut dlf_zero_filt_sse: i64 = -1;
        let mut dlf_best_filt_sse: i64 = -1;
        let mut dlf_full_image_ran = false;
        let mut lf_levels = self.pick_lf_levels(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            w,
            h,
            filter_chroma,
            &encode_input,
            sc_derivation,
            coded_lossless,
            &recon,
            lf_sharp_eff,
            &deblock_geom,
            &u_recon,
            &v_recon,
            &recon10,
            dlf_ctrls,
            dlf_pick_inputs,
            &mut dlf_zero_filt_sse,
            &mut dlf_best_filt_sse,
            &mut dlf_full_image_ran,
        )?;
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
        let cdef_level =
            self.derive_cdef_level(sc_derivation, is_single_frame, dlf_resolution, dlf_is_base);
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
        let (cdef_params, cdef_dist_dev) = self.pick_cdef(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            w,
            h,
            filter_chroma,
            &encode_input,
            sc_derivation,
            base_qindex,
            coded_lossless,
            &recon,
            &deblock_geom,
            &u_recon,
            &v_recon,
            &recon10,
            cdef_zero_fs_cost_bias,
            cdef_ctrls,
            cdef_force_off,
        )?;
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
        // `&self`, not `&mut self`: `run_entropy_walk` holds a shared borrow of
        // `self` across this stage, so the LR records come back out here.
        let (mut last_lr_unit_size, mut last_lr_stats) =
            (self.last_lr_unit_size, self.last_lr_stats);
        self.search_restoration(
            chroma,
            &hbd_source,
            &mut hbd_used,
            &stop,
            sc_arm,
            w,
            fmt,
            acw,
            filter_chroma,
            encode_input,
            sc_derivation,
            base_qindex,
            coded_lossless,
            &mut recon,
            seq_tools,
            cw,
            lr_true_w,
            lr_true_h,
            &mut u_recon,
            &mut v_recon,
            &mut recon10,
            dlf_is_not_last_layer,
            recon10_pre_cdef,
            &mut lr_signal,
            &mut walk_rest_info,
            decoder_chroma_recon,
            &mut output_restoration,
            &mut last_lr_unit_size,
            &mut last_lr_stats,
        )?;
        self.last_lr_unit_size = last_lr_unit_size;
        self.last_lr_stats = last_lr_stats;

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
        let mut inter_signal = self.derive_inter_signal(
            display_order,
            is_key,
            &pic_decision,
            base_qindex,
            primary_ref_frame_for_cdf,
            gm_models,
            seq_tools,
            md_config_signals,
        )?;

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
        let bitstream = self.assemble_bitstream(
            chroma,
            is_key,
            frame_tx_mode_select,
            base_qindex,
            tile_rows_log2,
            tile_cols_log2,
            chroma_deltas,
            delta_q_res_signal,
            lf_sharp_eff,
            qm_levels,
            &film_grain,
            is_single_frame,
            seq_tools,
            tile_data,
            tile_size_bytes_minus_1,
            lf_levels,
            &cdef_params,
            lr_signal,
            sc_signal,
            inter_signal,
        );

        // C's deblock search truncates odd chroma dimensions. A decoder
        // filters the ceiling-sized plane. Replay the signaled filters on
        // the output copy after all decisions and entropy coding are complete.
        let mut decoder_output8 = None;
        self.decoder_chroma_recon_stage(
            w,
            h,
            acw,
            filter_chroma,
            lf_sharp_eff,
            deblock_geom,
            &mut recon10,
            lf_levels,
            &cdef_params,
            decoder_chroma_recon,
            output_restoration,
            &mut decoder_output8,
        );

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
        self.superres_upscale_stage(
            chroma,
            fmt,
            &recon,
            &u_recon,
            &v_recon,
            &recon10,
            &decoder_output8,
            &mut out8,
            &mut out10,
        )?;

        let padded_ref_hbd = self.build_padded_ref_hbd(chroma, w, h, ss_x, acw, ach, &recon10);

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

        self.emit_recon_output(
            display_order,
            w,
            h,
            ss_x,
            &recon,
            &film_grain,
            &u_recon,
            &v_recon,
            recon10,
            decoder_output8,
            out8,
            out10,
        );
        // C `pad_ref_and_set_flags` (enc_dec_process.c:1072-1112): the recon
        // is padded with a replicated margin BEFORE it becomes a reference,
        // because inter prediction indexes negative offsets from pixel
        // (0,0). Built here, once, from the same buffers stored below.
        let padded_ref = self.build_padded_ref(
            chroma,
            w,
            h,
            ss_x,
            ss_y,
            &recon,
            &u_recon,
            &v_recon,
            padded_ref_hbd,
            &recon_msb8,
        );
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
        dump_refstats(display_order, is_key, &frame_coded_area, coded_area_pct);
        #[cfg(feature = "std")]
        if crate::dbgenv::pd0dbg() {
            let acc = frame_coded_area.borrow();
            eprintln!(
                "REFSTORE poc={display_order} intra={:?} skip={:?}",
                acc.as_ref().map(|a| a.sb_intra.as_slice()),
                acc.as_ref().map(|a| a.sb_skip.as_slice()),
            );
        }
        let ref_frame = self.build_reference_frame(
            chroma,
            display_order,
            is_key,
            pic_decision,
            temporal_layer,
            &frame_me,
            base_qindex,
            tpl_r0,
            recon,
            gm_field,
            sb_min_sq_sizes,
            sb_max_sq_sizes,
            &frame_coded_area,
            walk_end_cdfs,
            u_recon,
            v_recon,
            lf_levels,
            dlf_dist_dev,
            cdef_params,
            cdef_dist_dev,
            recon_msb8,
            padded_ref,
            coded_area_pct,
        );
        dump_mvs(&ref_frame);
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
        self.stash_pa_picture(&pcs, pa_cur);
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
            let avg_cnt_zeromv = frame_coded_area.borrow().as_ref().map_or(0, |a| {
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

mod ra;

mod tpl_stage;

mod setup;

mod entry;

mod grain;

mod config_check;

mod superres;

mod cbr;

mod frame_setup;

mod inter_setup;

mod restoration_stage;

mod frame_output;

mod md_setup;

mod loop_filters;

mod recon_output;

mod diagnostics;
use diagnostics::*;

mod walk_driver;

mod bd10_post;

mod inter_md_stage;

mod frame_prep;
use frame_prep::*;

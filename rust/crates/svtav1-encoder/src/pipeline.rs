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
use alloc::vec::Vec;
// `StopToken::check` is a method of the `enough::Stop` trait; bring the trait
// into scope so the frame-entry cancellation check resolves.
use enough::Stop;

/// Encoder pipeline state.
pub struct EncodePipeline {
    /// SVT_HDR_MODE mirror: which C oracle this encode targets (mainline
    /// v4.2.0 vs the svt-av1-hdr fork hybrid MODE1) + the fork knobs.
    /// Defaults to Mainline = all fork behavior off; callers opt in with
    /// `pipe.hdr = HdrForkConfig::hdr_fork()` after construction.
    pub hdr: crate::hdr_mode::HdrForkConfig,
    /// Speed configuration.
    pub speed_config: SpeedConfig,
    /// Rate control configuration.
    pub rc_config: RcConfig,
    /// Rate control state.
    pub rc_state: RcState,
    /// Decoded picture buffer.
    pub dpb: DecodedPictureBuffer,
    /// GOP structure.
    pub gop: GopStructure,
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
    /// The PREVIOUS frame's PA (picture-analysis) picture — its padded SOURCE
    /// luma at full, 1/4 and 1/16 resolution.
    ///
    /// SVT's motion estimation is OPEN LOOP: `me_process.c:185-203` searches
    /// against the PA reference, which `reference_object.c:242-250` documents
    /// as pointing directly at the app's luma input, NOT at a recon. So the
    /// search needs the previous SOURCE and the DPB's padded recon is a
    /// different buffer for a different job (motion compensation). `None`
    /// until the first frame has been encoded.
    pa_ref: Option<alloc::boxed::Box<crate::inter_me_arm::PaPicture>>,
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
    pub chroma_420: bool,
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
    pub sb_size: usize,
    /// Explicit SB-size override (`SVTAV1_SB` in the harness). `None` =
    /// derive from the C rule. Set to `Some(64)`/`Some(128)` to pin one —
    /// used by the anti-vacuity witness, which needs to force the port to
    /// the WRONG size on an SB128 cell and observe the divergence.
    pub sb_size_override: Option<usize>,
    /// What C's rule alone asked for, BEFORE the override and the
    /// capability fallback. Stored rather than recovered from `sb_size` +
    /// `sb128_fallback`: once `sb128_encode_supported` stops being a
    /// constant false, `(sb_size, fallback)` no longer determines the
    /// derived value (an explicit `Some(128)` on a supported preset would
    /// be indistinguishable from a derived 128), and a later
    /// `with_sb_size(None)` would silently resolve to the wrong grid.
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
    for r in 0..dh {
        let sr = r.min(sh - 1);
        let base = sr * src_stride;
        let orow = r * dw;
        for c in 0..dw {
            out[orow + c] = src[base + c.min(sw - 1)];
        }
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
    for r in 0..dh {
        let sr = r.min(sh - 1);
        let base = sr * src_stride;
        let orow = r * dw;
        for c in 0..dw {
            out[orow + c] = src[base + c.min(sw - 1)];
        }
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
            allintra: intra_period <= 1,
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
            pd_ctx: crate::port_picstruct::PicDecisionCtx::new(),
            speed_config: SpeedConfig::from_preset(preset),
            rc_config,
            rc_state: RcState::default(),
            dpb: DecodedPictureBuffer::new(),
            gop: GopStructure::new(hierarchical_levels, intra_period),
            frame_count: 0,
            width: dims.aligned_w as u32,
            height: dims.aligned_h as u32,
            true_width: width,
            true_height: height,
            bit_depth: 8,
            upscaled_width: width,
            superres_denom: None,
            superres_stats_luma: None,
            hbd_source: None,
            pa_ref: None,
            // C-matched default: CICP "unspecified" (cp/tc/mc = 2/2/2,
            // studio range) — the library defaults of enc_settings.c:1043.
            // The SH then carries color_description_present_flag=0 and
            // color_range=0, byte-matching C at matched configs. Callers
            // that know their color space (AVIF path) override via
            // with_color_description.
            color_description: crate::entropy::obu::ColorDescription::default(),
            chroma_sample_position: 0,
            chroma_420: false,
            recon_output: false,
            last_recon: None,
            last_recon_unfiltered: None,
            last_recon_pre_cdef: None,
            last_recon10_y: None,
            last_recon10_uv: None,
            last_recon10_final: None,
            last_cdef_stats: crate::cdef::CdefStats::default(),
            last_cdef_signaled: None,
            last_lr_stats: ([0; 3], 0),
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
    /// STILL UNPORTED, so still gated (see `sb128_root_always_split`):
    /// a genuine 128-level NONE/HORZ/VERT RD search (this path is
    /// forced-SPLIT), the b64<->sb stat bridges (`get_sb128_variance` /
    /// `get_sb128_me_data`), and the CDEF 4-quadrant three-phase contract.
    fn sb128_encode_supported(preset: u8) -> bool {
        // Preset gate only; the CONTENT gate (forced-SPLIT validity) is
        // applied per-frame in `encode_frame_internal`, which can see the
        // pixels. Presets 0/1 are the only ones C ever codes at 128 in
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
        temporal_layer: u8,
    ) -> EncodeResult<crate::port_picstruct::PicParams> {
        use crate::port_picstruct as pp;

        let hier = self.gop.hierarchical_levels;
        let seq = pp::SeqPicParams {
            // The campaign's GOP: low-delay P, CQP/CRF. C's driver is given
            // `SVT_PRED_STRUCT=1` (LOW_DELAY) for the same cells.
            pred_structure: pp::PredStructure::LowDelay,
            rate_control_mode: pp::RcMode::CqpOrCrf,
            rtc: false,
            allintra: false,
            // NOT the per-preset `set_mrp_ctrl` table (`enc_handle.c:3573`),
            // which is not ported. Its caps only bind once a frame has more
            // than one DISTINCT reference POC in a list, and
            // `set_ref_list_counts` collapses both lists to 1 while every DPB
            // slot still holds the key frame — so on the 2-frame cell the caps
            // are provably inert. A longer GOP needs the real table; that is
            // why `generate_rps_info` is called through a function that can
            // refuse rather than inlined.
            mrp_ctrls: pp::MrpCtrls::default(),
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            },
            hierarchical_levels: hier,
            max_managed_refs: 0,
        };
        let mini_gop = 1u32 << hier;
        self.pd_ctx.mini_gop_length[0] = mini_gop;
        // Flat GOP (`hierarchical_levels == 0`): every picture is the base of
        // its own one-picture mini-GOP, so `pic_idx` is always 0.
        let pic_idx = if is_key {
            0
        } else {
            (display_order % u64::from(mini_gop)) as u32
        };
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
            temporal_layer_index: temporal_layer,
            hierarchical_levels: hier,
            pred_struct_type: pp::PredStructure::LowDelay,
            pred_struct_entry_count: mini_gop,
            frame_offset: display_order,
            aligned_width: self.width,
            aligned_height: self.height,
            ..Default::default()
        };
        pp::picture_decision_per_picture(&mut pic, &seq, &mut self.pd_ctx, pic_idx, 0).map_err(
            |_| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "this GOP shape's reference structure is not implemented \
                     (port_picstruct::generate_rps_info translates 4 of C's 8 branches)",
                ))
            },
        )?;
        Ok(pic)
    }

    fn resolve_sb_size(derived: usize, override_: Option<usize>, preset: u8) -> (usize, bool) {
        let want = override_.unwrap_or(derived);
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
        assert!(
            (9..=16).contains(&denom),
            "SuperresDenom must be 9..=16 (8 = unscaled = no superres); got {denom}"
        );
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
            allintra: self.gop.intra_period <= 1,
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

    /// Enable/disable the opt-in 4:2:0 chroma mode (see `chroma_420` field).
    pub fn with_chroma_420(mut self, enabled: bool) -> Self {
        self.chroma_420 = enabled;
        self
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

    /// Encode a single frame through the full pipeline (monochrome).
    ///
    /// Returns the encoded bitstream data and updates internal state.
    /// The monochrome path does not yet pad TRUE->ALIGNED (task #95 chunk
    /// 1 wired only the 4:2:0 path); mono callers must pass 8-aligned dims.
    pub fn encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> Vec<u8> {
        assert!(
            self.width == self.true_width && self.height == self.true_height,
            "monochrome encode_frame requires 8-aligned dims (arbitrary-dims padding is wired \
             on the 4:2:0 path only so far — task #95)"
        );
        // Task #95 chunk 2: partial SBs (8-aligned but not 64-aligned) are
        // supported ONLY on the PD0 fixed-tree path (preset >= 6), which starts
        // from a 64x64 root carrying spec-5.11.4 forced edge splits and codes
        // the partition symbols with the edge-aware alphabets. Presets < 6 that
        // use the homegrown search still root at the CLAMPED extent and would
        // emit an undecodable stream, so they stay restricted to full 64x64
        // SBs — rejecting out-of-scope dims beats mis-coding them.
        assert!(
            (self.width.is_multiple_of(64) && self.height.is_multiple_of(64))
                || self.speed_config.preset >= 6,
            "monochrome encode_frame supports partial SBs only on the PD0 path (preset >= 6); \
             got {}x{} at preset {} — use a multiple of 64 or preset >= 6",
            self.width,
            self.height,
            self.speed_config.preset
        );
        // Additive fallible core (Feature 1+3). This wrapper KEEPS its exact
        // signature and panicking contract: with the default `Unstoppable`
        // token and the infallible-alloc feature default, the core cannot
        // return `Err` on the trusted path, so `.expect()` never fires and the
        // emitted bytes are unchanged. Callers wanting graceful OOM /
        // cancellation use `try_encode_frame`.
        self.encode_frame_impl(y_plane, y_stride, None)
            .expect("encode_frame is infallible on the default/trusted path")
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
        // unchanged.
        self.encode_frame_420_core(y, u, v, y_stride)
            .expect("encode_frame_420 is infallible on the default/trusted path")
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
    ) -> crate::EncodeResult<Vec<u8>> {
        // Superres chunk B.3: the caller hands in FULL-width planes; the
        // encode runs at the reduced CODED width. Downscale horizontally with
        // C's `svt_av1_resize_plane_horizontal` (svtav1-dsp::resize, pinned
        // byte-exact vs C) before anything else, then the existing
        // TRUE->ALIGNED padding and the whole pipeline operate on the coded
        // planes. No-op when superres is off.
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
        // TRUE chroma dims (4:2:0 ceiling, matching the input .yuv layout).
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        if aw == tw && ah == th {
            // Natively 8-aligned: pass through unchanged (byte-identical to
            // the pre-#95 path).
            return self.encode_frame_impl(y, y_stride, Some((u, v)));
        }
        // Pad TRUE -> ALIGNED. C replicates the last valid column, then the
        // last valid row (incl. the new right pad); the per-pixel min-clamp
        // in `pad_plane_replicate` is equivalent for a rectangular region.
        let (acw, ach) = (aw / 2, ah / 2);
        let y_pad = pad_plane_replicate(y, y_stride, tw, th, aw, ah)?;
        let u_pad = pad_plane_replicate(u, tcw, tcw, tch, acw, ach)?;
        let v_pad = pad_plane_replicate(v, tcw, tcw, tch, acw, ach)?;
        self.encode_frame_impl(&y_pad, aw, Some((&u_pad, &v_pad)))
    }

    /// Fallible twin of [`Self::encode_frame`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame`] on success. The difference is
    /// purely at the boundary: the legacy `assert!`s become typed
    /// [`EncodeError`]s, and the cooperative cancellation token
    /// ([`Self::stop`]) is checked once at entry. The legacy method is left
    /// untouched. Internally this calls the SAME infallible
    /// `encode_frame_impl`, so it cannot change the emitted bytes.
    pub fn try_encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> EncodeResult<Vec<u8>> {
        // (a) Validate — mirror the `encode_frame` asserts.
        if self.width != self.true_width || self.height != self.true_height {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "monochrome encode requires 8-aligned dims (arbitrary-dims padding is \
                         wired on the 4:2:0 path only)",
            }));
        }
        if (!self.width.is_multiple_of(64) || !self.height.is_multiple_of(64))
            && self.speed_config.preset < 6
        {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "monochrome encode supports partial SBs only on the PD0 path (preset >= 6); use a \
                 multiple of 64 or preset >= 6",
            )));
        }
        // (b) Feature 1 entry stop-check (frame-granular).
        self.stop
            .check()
            .map_err(EncodeError::from)
            .map_err(whereat::at)?;
        // (c) The fallible core (asserts above pre-satisfied). Its own in-loop
        // stop-checks + fallible allocations propagate here as `Err` instead of
        // panicking/aborting; on success the bytes match `encode_frame`.
        self.encode_frame_impl(y_plane, y_stride, None)
    }

    /// Fallible twin of [`Self::encode_frame_420`] (Feature 1 + 2).
    ///
    /// Byte-identical to [`Self::encode_frame_420`] on success. The legacy
    /// `assert!`s (chroma flag, u/v plane sizes, still/key-only) become typed
    /// [`EncodeError`]s and the cancellation token is checked at entry;
    /// otherwise it delegates to the untouched infallible method (which
    /// performs the TRUE->ALIGNED padding and calls `encode_frame_impl`), so
    /// the emitted bytes are unchanged.
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
        // The 4:2:0 path is still/key-only (mirrors the `encode_frame_impl`
        // `chroma.is_none() || is_key` assert).
        //
        // `SVTAV1_INTER_EXPERIMENTAL` lifts it for the differential harness
        // only — see `crate::dbgenv::inter_experimental`. The refusal is the
        // shipped behaviour and stays until the inter tile is byte-identical.
        if !self.gop.is_key_frame(self.frame_count) && !crate::dbgenv::inter_experimental() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "chroma_420 pipeline supports still/key frames only (intra_period <= 1)",
            )));
        }
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
    /// Task #6 chunk 1 threads real u16 into the bd10 MD funnel and the bd10
    /// level re-encode post-pass. Both are gated (`bd10_full_rd_supported`,
    /// `bd10_luma_funnel`, `bd10_postpass_runs`) on bd10 + 64-aligned dims,
    /// and they are mutually exclusive by preset. Outside that envelope the
    /// encode would silently truncate the caller's low bits to 8, so the hbd
    /// entry points reject instead of emitting a quietly-8-bit stream (the
    /// "no silent corruption" bar in `rust/CLAUDE.md`).
    fn hbd_source_consumed(&self, chroma_420: bool) -> bool {
        self.bd10_levels_native(chroma_420)
    }

    /// Will this configuration produce TRUE 10-bit coded levels?
    ///
    /// Exactly two stages can: the full-RD mode-decision funnel
    /// ([`bd10_full_rd_supported`], preset <= 8) and the level-only u16
    /// re-encode post-pass (preset >= 9, `bd10_postpass_runs`). BOTH require
    /// 4:2:0 and 64-aligned dims — the funnel because `use_funnel` is gated on
    /// `chroma_420` and `tx_unit_hbd` is not partial-SB-aware, the post-pass
    /// because `bd10_tree_supported` cannot map an edge/straddle footprint.
    ///
    /// This is the single source of truth for "is bd10 real here", consumed
    /// both by the hbd entry points (which must not silently drop the caller's
    /// low bits) and by [`Self::bit_depth_config_error`] (which must not let a
    /// u8-input 10-bit encode emit 8-bit-quantized levels under a 10-bit
    /// sequence header).
    /// FH `frm_hdr->tx_mode == TX_MODE_SELECT` for this frame.
    ///
    /// ONE source for the header writer and the pack walk: the signalled mode
    /// and the coded symbols must agree or the stream does not decode (see
    /// `EntropyCtx::tx_mode_select`). `crate::txs_arm::tx_mode_select` is the
    /// ladder; the allintra arm is unconditional TX_MODE_SELECT, the video arm
    /// signals it only while `pcs->txs_level != 0`.
    fn frame_tx_mode_select(&self) -> bool {
        let arm = if self.gop.intra_period <= 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: true }
        };
        crate::txs_arm::tx_mode_select(
            arm,
            crate::rate_arm::eff_enc_mode(arm, self.speed_config.preset),
            true,
            u32::from(self.rc_config.qp),
        )
    }

    fn bd10_levels_native(&self, chroma_420: bool) -> bool {
        if self.bit_depth != 10 {
            return false;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // Monochrome builds no funnel (`use_funnel` requires 4:2:0). The
        // post-pass cannot stand in for it below preset 9 either: it hardcodes
        // the RDOQ txb_skip/dc_sign contexts to 0/0, which is correct only
        // where `real_coeff_ctx` is off — i.e. only in the eff-M9 band. So
        // mono bd10 is faithful at preset >= 9 and nowhere else.
        let preset = self.speed_config.preset;
        // NO GEOMETRY TERM (2026-08-04). Both bd10 level producers are now
        // partial-SB aware: the full-RD funnel (preset <= 8) rides the shared,
        // already-correct partition search and leaf funnel, and the level-only
        // re-encode post-pass (preset >= 9) got SB-extent recon buffers,
        // straddle-clipped writes, SB-extent-padded sources, and the pack's
        // skip-off-frame-quadrant child walk. Both are gated per-CELL below
        // rather than by dimension.
        if !chroma_420 {
            return preset >= 9;
        }
        preset >= 9 || bd10_full_rd_supported(self.bit_depth, preset, chroma_420, w, h)
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
        // Issue #9 item 8 — the `aq_mode` SEMANTIC divergence, refused rather
        // than documented, because documentation does not stop a caller from
        // copying C's default and silently getting different pixels.
        //
        // C's `--aq-mode` default is 2, and for a single still it is INERT:
        // aq-mode-2's deltaq (`svt_aom_sb_qp_derivation_tpl_la`, rc_aq.c:899)
        // is gated on `tpl_ctrls.enable && r0 != 0`, i.e. TPL lookahead, which
        // one frame has none of (`r0` inits 0, pcs.c:1299). This port's
        // non-zero `aq_mode` instead runs a HOMEGROWN frame-level VAQ + TPL
        // shift (the `rc_config.aq_mode != 0` branch below) that is a port of
        // nothing — so `aq_mode = 2`, the value a caller copies straight out
        // of C's documentation, means "C: no change" and "port: shift the
        // whole frame's qindex". That is the exact shape of divergence this
        // encoder refuses everywhere else.
        //
        // 0 (the default) is the value that MATCHES C for a still. C's
        // segmentation-side `aq_mode` is a different parameter and stays
        // C-parity-tested (`segmentation::setup_segmentation`,
        // tests/c_parity_segmentation.rs).
        if self.rc_config.aq_mode != 0 {
            return Some(
                "aq_mode must be 0: C's aq-mode deltaq is TPL-gated and therefore INERT for a \
                 single still (rc_aq.c:899), so C's own default of 2 changes nothing there, while \
                 this port's non-zero aq_mode runs a homegrown frame-level VAQ/TPL qindex shift \
                 that is a port of nothing — see issue #9 item 8",
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
    ///    stream") forbids. Reachable today from `AvifEncoder::with_bit_depth(10)`
    ///    at every speed whose preset is <= 8, including the DEFAULT speed.
    fn bit_depth_config_error(&self, chroma_420: bool) -> Option<&'static str> {
        match self.bit_depth {
            8 => return None,
            10 => {}
            _ => {
                return Some(
                    "bit depth must be 8 or 10 — C v4.2.0 rejects every other depth at encoder \
                     init (svt_av1_verify_settings, Globals/enc_settings.c:460) and this port has \
                     no 12-bit kernels",
                );
            }
        }
        if self.bd10_levels_native(chroma_420) {
            return None;
        }
        if !chroma_420 && self.speed_config.preset < 9 {
            return Some(
                "10-bit monochrome needs preset >= 9: below that neither bd10 producer runs (the \
                 full-RD funnel requires 4:2:0, and the level-only post-pass would miscode with \
                 its 0/0 RDOQ contexts), so the encode would be 8-bit-quantized under a 10-bit \
                 sequence header",
            );
        }
        Some(
            "this 10-bit configuration has no bd10 stage to produce the coded levels; the encode \
             would be 8-bit-quantized under a 10-bit sequence header",
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

    /// Superres chunk B.3 — the combinations whose SIGNALLED stream would not
    /// match what the encoder actually produced. Rejecting beats emitting a
    /// stream that says "upscale me" over content the encoder handled with the
    /// wrong geometry.
    /// Issue #5: the coded-lossless (QP 0 / `base_q_idx` 0) envelope this port
    /// has byte-verified against the C oracle, as a refusal predicate for
    /// everything outside it. Each arm names the missing piece.
    fn lossless_config_error(
        &self,
        chroma_420: bool,
        is_key: bool,
        allow_screen_content_tools: bool,
    ) -> Option<&'static str> {
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
        if !chroma_420 {
            return Some(
                "QP 0 (coded-lossless) is not implemented on the monochrome path (the mono leaf \
                 coder has no WHT / TX_4X4 arm and C v4.2.0 cannot produce a mono oracle) — \
                 use the 4:2:0 path or QP >= 1",
            );
        }
        if self.bit_depth != 8 {
            return Some(
                "QP 0 (coded-lossless) is 8-bit only so far: neither bd10 level producer has a \
                 WHT / TX_4X4 arm — use QP >= 1 at 10-bit",
            );
        }
        if !is_key || self.gop.intra_period > 1 {
            return Some(
                "QP 0 (coded-lossless) is not implemented for inter frames — encode a single \
                 key frame",
            );
        }
        if allow_screen_content_tools {
            return Some(
                "QP 0 (coded-lossless) with screen-content tools (palette / IntraBC) is not \
                 byte-verified against C so far — use QP >= 1 on this content",
            );
        }
        if self.superres_denom.is_some() {
            return Some(
                "QP 0 (coded-lossless) with superres is not implemented (the frame is not \
                 AllLossless at the upscaled size) — use QP >= 1",
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
            self.gop.intra_period <= 1,
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
                "superres with loop restoration enabled (allintra preset <= 6) is not wired yet                  — C runs LR on the UPSCALED frame; use preset >= 7",
            );
        }
        if self.bit_depth != 8 {
            return Some("superres is 8-bit only so far (the u16 source downscale is unported)");
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
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_420_hbd requires with_bit_depth(10) (8-bit sources use \
                 encode_frame_420; 12-bit is outside C's shipping envelope)",
            )));
        }
        if !self.hbd_source_consumed(true) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit input needs a bd10 consumer: 64-aligned dims and either preset \
                 >= 9 or a full-RD-capable preset <= 8 (non-screen content) — see \
                 docs/hbd-input-port-map.md chunk 2",
            )));
        }
        if !self.gop.is_key_frame(self.frame_count) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "chroma_420 pipeline supports still/key frames only (intra_period <= 1)",
            )));
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, tch) = (tw.div_ceil(2), th.div_ceil(2));
        if y.len() < (th - 1) * y_stride + tw || u.len() < tcw * tch || v.len() < tcw * tch {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "hbd planes must cover the true dims (y at y_stride, u/v at true_w/2)",
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
            u: pad_plane_replicate_u16(u, tcw, tcw, tch, aw / 2, ah / 2)?,
            v: pad_plane_replicate_u16(v, tcw, tcw, tch, aw / 2, ah / 2)?,
        };
        let mut y8 = svtav1_types::try_vec![0u8; tw * th]?;
        for r in 0..th {
            for c in 0..tw {
                y8[r * tw + c] = (y[r * y_stride + c] >> shift) as u8;
            }
        }
        let mut u8p = svtav1_types::try_vec![0u8; tcw * tch]?;
        let mut v8p = svtav1_types::try_vec![0u8; tcw * tch]?;
        for i in 0..tcw * tch {
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

    /// Native 10-bit (u16) monochrome entry point — the mono twin of
    /// [`Self::try_encode_frame_420_hbd`] (task #6 chunk 1).
    ///
    /// Monochrome builds no MD funnel, so the only consumer of the real u16
    /// samples is the bd10 level re-encode post-pass (preset >= 9): the coded
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
        if self.bit_depth != 10 {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "try_encode_frame_hbd requires with_bit_depth(10)",
            )));
        }
        if self.width != self.true_width || self.height != self.true_height {
            return Err(whereat::at!(EncodeError::InvalidDimensions {
                width: self.true_width,
                height: self.true_height,
                reason: "monochrome encode requires 8-aligned dims (arbitrary-dims padding is \
                         wired on the 4:2:0 path only)",
            }));
        }
        if !self.hbd_source_consumed(false) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "native 10-bit monochrome input needs the bd10 level re-encode post-pass: \
                 64-aligned dims at preset >= 9 — see docs/hbd-input-port-map.md chunk 2",
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
        // Mono aligned == true dims (checked above), so the hbd plane only
        // needs tightening to stride `tw`.
        self.hbd_source = Some(HbdSource {
            y: pad_plane_replicate_u16(y, y_stride, tw, th, tw, th)?,
            u: alloc::vec::Vec::new(),
            v: alloc::vec::Vec::new(),
        });
        let out = self.encode_frame_impl(&y8, tw, None);
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
    ) -> crate::EncodeResult<Vec<u8>> {
        let display_order = self.frame_count;
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
        self.hdr.apply_tune_overrides(self.rc_config.qp);
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
        // INTER FRAMES ARE NOT ENCODABLE ON EITHER PATH — refuse, don't emit.
        //
        // The 4:2:0 arm has always asserted this (it would additionally need
        // chroma in the DPB and a chroma-aware inter frame header). The
        // MONOCHROME arm did not, and it is the one that shipped a corrupt
        // stream: see the measured aomdec/dav1d failures documented at the
        // `bit_depth_config_error` call site above. Both arms now take the same
        // typed `Err` — a caller mistake must never become a decoder's problem.
        //
        // Keyed on the FRAME TYPE rather than `intra_period` so that
        // constructing a pipeline with a GOP structure and encoding only its
        // key frame keeps working: that stream is a valid still.
        //
        // `SVTAV1_INTER_EXPERIMENTAL` lifts the refusal for the differential
        // harness ONLY (`crate::dbgenv::inter_experimental`). Under it the
        // frame header is derived from the real reference structure and the
        // real picture-level ladders, and the TILE is still the pre-campaign
        // homegrown path — so the stream is measurable, not correct. It must
        // never leave `tools/identity_diff_inter.sh`.
        if !is_key && !crate::dbgenv::inter_experimental() {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "inter frames are not implemented: the frame HEADER is byte-identical to the C \
                 encoder's, but the TILE is not ported — no CDF continuation from the frame the \
                 header names in primary_ref_frame, and no inter syntax in the tile walk — so \
                 the stream does not decode. This encoder is still-image only: encode a single \
                 key frame",
            )));
        }
        let temporal_layer = if is_key {
            0
        } else {
            let pos = (display_order % self.gop.mini_gop_size as u64) as u32;
            self.gop.get_temporal_layer(pos)
        };

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
        let pic_decision = if self.gop.intra_period > 1 {
            Some(self.run_picture_decision(display_order, is_key, temporal_layer)?)
        } else {
            None
        };

        // Step 2: Create PCS
        let mut pcs = if is_key {
            PictureControlSet::new_key_frame(self.width, self.height, display_order)
        } else {
            PictureControlSet::new_inter_frame(
                self.width,
                self.height,
                display_order,
                display_order,
                temporal_layer,
            )
        };

        // Step 3: Rate control — assign QP
        pcs.qp = assign_picture_qp(&self.rc_config, &self.rc_state, temporal_layer);

        // Step 3b: Temporal filtering (if enabled and we have reference frames)
        let w = self.width as usize;
        let h = self.height as usize;
        let n = w * h;
        let encode_input =
            if self.speed_config.enable_temporal_filter && !is_key && self.dpb.occupied_slots() > 0
            {
                // Collect available reference frames for TF
                let mut ref_frames: alloc::vec::Vec<&[u8]> = alloc::vec::Vec::new();
                for slot in 0..svtav1_types::reference::REF_FRAMES {
                    if let Some(rf) = self.dpb.get(slot)
                        && rf.y_plane.len() == n
                    {
                        ref_frames.push(&rf.y_plane);
                    }
                    if ref_frames.len() >= 3 {
                        break;
                    }
                }
                if !ref_frames.is_empty() {
                    let tf_config = crate::temporal_filter::TfConfig::default();
                    let tf_result = crate::temporal_filter::temporal_filter(
                        y_plane,
                        &ref_frames,
                        w,
                        h,
                        y_stride,
                        &tf_config,
                    )?;
                    tf_result.filtered
                } else {
                    gather_rows(y_plane, y_stride, w, h)?
                }
            } else {
                gather_rows(y_plane, y_stride, w, h)?
            };

        // Task #95 chunk 2 — partial-SB variance source. `compute_b64_variance`
        // walks a full 64x64 grid per b64, so on a partial SB (aligned dims not
        // a multiple of 64) it reads PAST the aligned extent into C's replicated
        // border (`pad_input_picture` + `svt_aom_generate_padding` net content =
        // the TRUE edge pixel, docs/arbitrary-dims-port-map.md). Build a source
        // buffer padded out to the SB extent and read the PD0 partition /
        // variance source from it. For a 64-aligned frame the extent equals the
        // aligned extent, so no padding is needed and `encode_input` is used
        // directly at stride `w` — fully byte-neutral for every full-SB cell.
        let dims95 =
            crate::frame_geom::FrameDims::new(self.true_width as usize, self.true_height as usize);
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
                    let (acw, ach) = (w / 2, h / 2);
                    let (ext_ch_h, ext_cw) = (ext_h / 2, ext_w / 2);
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
                    let (acw, ach) = (w / 2, h / 2);
                    let (ext_ch_h, ext_cw) = (ext_h / 2, ext_w / 2);
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
            _ => self.speed_config.preset,
        };
        // C picks a DIFFERENT derivation function per arm of `scs->allintra`
        // (enc_handle.c:4406 — `intra_period_length == 0 || avif ||
        // pred_structure == ALL_INTRA`); the port's proxy for it is the same
        // `intra_period <= 1` predicate the video qindex derivation below
        // uses. On the video arm the intra-BC ladder is
        // `sig_deriv_multi_processes_default`'s (:2033-2052) instead of the
        // allintra one (:2346-2369) — which is what makes a video-mode
        // screen-content key frame set `frm_hdr->allow_intrabc` at M6, where
        // the still arm leaves it clear.
        //
        // Every frame this reaches on the video arm today is the KEY frame
        // (`encode_frame_impl` refuses non-key frames on the 4:2:0 path), so
        // `is_islice` is `is_key`; passing it rather than `true` keeps the
        // gate honest for when that refusal lifts.
        let sc_arm = if self.gop.intra_period <= 1 {
            crate::sc_detect::ScArm::Allintra
        } else {
            crate::sc_detect::ScArm::Video { is_islice: is_key }
        };
        let sc_derivation = crate::sc_detect::derive_sc(sc_arm, sc_preset, &encode_input, w, w, h);
        // Hoisted out of the walk: `&self` is borrowed across the pack loop, and
        // this is a frame-constant. One value for the header writer and the
        // walk (see `EntropyCtx::tx_mode_select`).
        let frame_tx_mode_select = self.frame_tx_mode_select();

        // Step 3c: Frame-level adaptive QP — OPT-IN via RcConfig.aq_mode.
        //
        // aq_mode == 0 (the default, matching the C encoder's
        // `--rc 0 --aq-mode 0` CQP semantics) means the assigned QP is used
        // UNCHANGED: C's CQP path is a straight `quantizer_to_qindex[qp]`
        // lookup with no content-adaptive shift (rc_process.c CQP branch).
        // The frame-level VAQ + TPL adjustments below are homegrown
        // heuristics (not ports of C's segment-based aq-mode 1/2) and used
        // to fire unconditionally, shifting base_q_idx on every stream —
        // the F1 divergence in docs/IDENTITY-STATUS.md.
        #[allow(unused_mut)]
        let mut tpl_adjusted_qp = if self.rc_config.aq_mode != 0 {
            // Compute VAQ activity map for adaptive QP
            let activity_map = crate::perceptual::ActivityMap::compute(&encode_input, w, h, w);

            // Adjust QP based on frame-level activity (VAQ)
            let vaq_adjusted_qp = if activity_map.frame_avg > 0.0 {
                let frame_activity_factor = (activity_map.frame_avg / 10.0).log2().clamp(-2.0, 2.0);
                (pcs.qp as f64 + frame_activity_factor).clamp(0.0, 63.0) as u8
            } else {
                pcs.qp
            };

            // TPL temporal complexity adjustment for inter frames:
            // Compare source to reference to estimate motion complexity,
            // then adjust QP — static scenes get lower QP (better quality),
            // high-motion scenes get higher QP (save bits for key frames).
            if !is_key && self.dpb.occupied_slots() > 0 {
                if let Some(rf) = self.dpb.get(0) {
                    let tpl_delta =
                        crate::rate_control::tpl_qp_adjustment(&encode_input, &rf.y_plane, w, h, w);
                    (vaq_adjusted_qp as i16 + tpl_delta as i16).clamp(0, 63) as u8
                } else {
                    vaq_adjusted_qp
                }
            } else {
                vaq_adjusted_qp
            }
        } else {
            pcs.qp
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
        #[allow(unused_mut)]
        let mut base_qindex = crate::rate_control::qp_to_qindex_with_offset(
            tpl_adjusted_qp,
            self.rc_config.extended_crf_qindex_offset,
        );
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
        let allintra = self.gop.intra_period <= 1;
        if !allintra {
            base_qindex = crate::rate_control::cqp_qindex_calc(
                i32::from(base_qindex),
                allintra,
                /*slice_is_intra=*/ is_key,
                /*is_ref=*/ true,
                /*idr_flag=*/ is_key,
                temporal_layer,
                self.gop.hierarchical_levels,
                self.bit_depth,
            )
            .clamp(0, 255) as u8;
        }
        let mut picture_qp = crate::rate_control::picture_qp_from_qindex(base_qindex);
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
        let sb_plan = if self.hdr.enable_variance_boost {
            let sb_cols_p = w.div_ceil(64);
            let sb_rows_p = h.div_ceil(64);
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

        // Issue #5: `base_qindex == 0` signals CODED-LOSSLESS in the frame
        // header (spec 5.9.2 — with the zero chroma deltas and no
        // segmentation of this port's mainline path, base_q_idx 0 IS
        // CodedLossless), and the whole encode follows C's lossless rules:
        // the header writes no deblock/CDEF/LR/tx_mode bits (chunk 1,
        // 2026-08-27), every block is an 8x8 coded at TX_4X4 with the
        // Walsh-Hadamard transform, no tx_size / tx_type symbols, RDOQ and
        // the tx-type search off, only DCT-chroma candidates injected, and no
        // in-loop filter runs (chunk 2, this arm's consumers below +
        // leaf_funnel). The envelope that is byte-verified against the C
        // oracle is `lossless_config_error`'s complement; everything outside
        // it is REFUSED rather than encoded wrong (the pre-chunk-2 measurement
        // of what "encoded wrong" looked like: ssim2 -200..-1100 vs source).
        let coded_lossless = base_qindex == 0;
        if coded_lossless
            && let Some(why) = self.lossless_config_error(
                chroma.is_some(),
                is_key,
                sc_derivation.allow_screen_content_tools,
            )
        {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(why)));
        }

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
            let ref_queue = crate::inter_hdr_arm::ref_queue_from_dpb(&self.pd_ctx, base_qindex);
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
        let pa_cur = (self.gop.intra_period > 1).then(|| {
            alloc::boxed::Box::new(crate::inter_me_arm::PaPicture::from_source(
                &encode_input,
                w,
                w,
                h,
                display_order,
            ))
        });
        let frame_me = match (is_key, pa_cur.as_deref(), self.pa_ref.as_deref()) {
            (false, Some(cur), Some(prev)) => {
                Some(crate::inter_me_arm::run_frame_me(
                    cur,
                    prev,
                    crate::inter_me_arm::FrameMeParams {
                        enc_mode: self.speed_config.preset,
                        qp: self.rc_config.qp,
                        width: w,
                        height: h,
                        picture_number: display_order,
                        // C `frame_is_boosted(pcs)` (enc_mode_config.h:108):
                        // true only for the base layer of a hierarchy, which
                        // a flat low-delay P GOP never has.
                        frame_is_boosted: temporal_layer == 0 && self.gop.hierarchical_levels > 0,
                        hierarchical_levels: self.gop.hierarchical_levels,
                    },
                ))
            }
            _ => None,
        };

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
                let (stat_src, stat_stride, stat_w, stat_h) = match stats_src.as_ref() {
                    Some((orig, ow, oh)) => (orig.as_slice(), *ow, *ow, *oh),
                    None => (sb_input, in_stride, w, h),
                };
                let mut tot: u64 = 0;
                let mut cnt: u64 = 0;
                for sy in (0..stat_h).step_by(64) {
                    for sx in (0..stat_w).step_by(64) {
                        tot += crate::pd0::compute_b64_variance(
                            stat_src, stat_stride, sx, sy,
                        )
                        .0[0] as u64;
                        cnt += 1;
                    }
                }
                let pic_avg_variance = (tot / cnt) as u16;
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
                    crate::rate_arm::rdoq_level(sc_arm, eff_mode, coeff_lvl)
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
                    Some(crate::pd0::frame_lambda_weight(
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
                let eff_mode = crate::rate_arm::eff_enc_mode(sc_arm, self.speed_config.preset);
                // The VIDEO arm's RDOQ ladder (`rdoq_level_default`,
                // enc_mode_config.c:8933) is a flat 1 through M10 and ignores
                // `coeff_lvl` — which is why C can leave a video-mode I-slice
                // at INVALID_LVL. The level is still derived, because the
                // coeff-driven arms above M10 read it.
                let rdoq_level = crate::rate_arm::rdoq_level(sc_arm, eff_mode, coeff_lvl);
                // C `av1_lambda_assign_md` (md_process.c:725) for a non-key
                // frame; a flat low-delay P GOP puts every frame at temporal
                // layer 0, which `update_lambda` (rc_process.c:406-410) maps
                // to ARF_UPDATE.
                let lambda = crate::pd0::inter_full_lambda_8bit(
                    base_qindex,
                    if temporal_layer == 0 {
                        crate::port_rc_process::FrameUpdateType::ArfUpdate
                    } else {
                        crate::port_rc_process::FrameUpdateType::LfUpdate
                    },
                    self.hdr.is_fork() && self.hdr.alt_lambda_factors,
                    0,
                    crate::pd0::frame_lambda_weight(
                        picture_qp as u32,
                        self.hdr.tune == crate::tune::TUNE_IQ,
                        lw_bump,
                    ),
                );
                let mut cq =
                    crate::quant::CodingQuantCfg::new(rdoq_level, lambda, base_qindex);
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

        // Get reference frame for inter prediction (if available)
        let ref_frame_data: Option<alloc::vec::Vec<u8>> = if !is_key {
            self.dpb.get(0).map(|rf| rf.y_plane.clone())
        } else {
            None
        };

        // The padded twin of the reference plane above (C
        // `pad_ref_and_set_flags`, enc_dec_process.c:1072) — what INTER
        // PREDICTION indexes, because a legal MV reads outside the frame.
        let ref_padded_luma: Option<alloc::boxed::Box<crate::picture::PaddedRef>> = if is_key {
            None
        } else {
            self.dpb.get(0).and_then(|rf| rf.padded.clone())
        };

        // MV map for spatial MV prediction (8x8 block grid)
        let mv_map_stride = w.div_ceil(8);
        let mv_map_size = mv_map_stride * h.div_ceil(8);
        let mut mv_map = svtav1_types::try_vec![svtav1_types::motion::Mv::ZERO; mv_map_size]?;

        // Compute per-SB TPL QP offsets for spatial bit allocation
        let sb_qp_offsets = if !is_key {
            if let Some(ref rf) = ref_frame_data {
                crate::rate_control::tpl_sb_qp_offsets(&encode_input, rf, w, h, w, sb_size)
            } else {
                svtav1_types::try_vec![0i8; sb_cols * sb_rows]?
            }
        } else {
            svtav1_types::try_vec![0i8; sb_cols * sb_rows]?
        };

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
            let txt = match sb_plan.as_ref() {
                Some(p) => std::format!(
                    "base={base_qindex} res={} plan={:?}\n",
                    p.delta_q_res,
                    p.sb_qindex
                ),
                None => std::format!("base={base_qindex} plan=NONE (variance boost off)\n"),
            };
            let _ = std::fs::write(path, txt);
        }
        let delta_q_res_signal = sb_plan.as_ref().map(|p| p.delta_q_res);
        // sharp-tx RDOQ activates only with per-SB delta-q present (C gate
        // `(use_sharpness || sharp_tx) && delta_q_present && plane==0`).
        // [SVT_HDR_MODE] tune SSIM/IQ/MS_SSIM: per-16x16 SSIM rdmult
        // scaling factors (aom_av1_set_mb_ssim_rdmult_scaling; the
        // alt_ssim_tuning multi-scale perceptual variant when that knob is
        // on). Applied per SB below — C scales per BLOCK from the PICTURE
        // lambda (set_ssim_rdmult ignores the per-SB qindex lambda);
        // PORT-NOTE(unverified): SB-granularity approximation of the
        // per-block geometric mean — refine with a C-side lambda dump.
        let ssim_factors: Option<(alloc::vec::Vec<f64>, usize, usize)> =
            if crate::tune::tune_uses_ssim_rdmult(self.hdr.tune) {
                Some(crate::tune::ssim_rdmult_factors(
                    &encode_input,
                    w,
                    w,
                    h,
                    self.hdr.alt_ssim_tuning,
                ))
            } else {
                None
            };
        // [SVT_HDR_MODE] per-tune LF sharpness (deblocking_filter.c:1157,
        // KEY frames): VQ/FILM_GRAIN +2 (min 7); IQ/MS_SSIM qindex cap.
        // Applied to the SEARCH input, the SIGNALED bits, and the walk's
        // application consistently (one effective value).
        let lf_sharp_eff: u8 = {
            let base = self.hdr.sharpness.clamp(0, 7) as u8;
            if self.hdr.is_fork() {
                crate::tune::lf_sharpness_for_tune(base, self.hdr.tune, base_qindex)
            } else {
                base
            }
        };
        let sharp_tx_active = self.hdr.is_fork() && self.hdr.sharp_tx == 1 && sb_plan.is_some();
        // [SVT_HDR_MODE] frame QM levels (svt_av1_qm_init,
        // md_config_process.c:249): the linear qindex map (default tune =
        // PSNR in the fork); chroma levels derive from base + the FH
        // chroma AC deltas. [15;3] = QM off (identity).
        let qm_levels: [u8; 3] = if self.hdr.enable_qm {
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
        let film_grain: Option<crate::entropy::obu::FilmGrainParams> =
            if self.hdr.is_fork() && self.hdr.noise_strength > 0 {
                let mut fg = crate::noise_gen::generate_noise_table(
                    self.width,
                    self.height,
                    u32::from(self.hdr.noise_strength),
                    self.hdr.noise_strength_chroma,
                    self.hdr.noise_chroma_from_luma as i8,
                    self.hdr.noise_size,
                    self.color_description.full_range,
                );
                let mut seed = 7391u16.wrapping_add(3381u16.wrapping_mul(self.frame_count as u16));
                if seed == 0 {
                    seed = 7391;
                }
                fg.random_seed = seed;
                Some(fg)
            } else {
                None
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
        let is_single_frame = self.gop.intra_period <= 1;
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
            // [SVT_HDR_MODE] the fork ALWAYS signals separate_uv_delta_q
            // (its FH writes independent U/V deltas — entropy_coding.c
            // fork block hardcodes both flags true).
            if self.hdr.is_fork() {
                t.separate_uv_delta_q = true;
                // Photon noise signals grain tables per frame.
                t.film_grain_params_present = self.hdr.noise_strength > 0;
            }
            // enable_intra_edge_filter's C-parity surface is still/420
            // (the C matched config). The mono extension keeps 0: C cannot
            // emit mono, and the mono leaf coder predicts without edge
            // filtering — signaling 0 keeps our recon decoder-exact on
            // that self-consistent surface.
            t.enable_intra_edge_filter &= self.chroma_420;
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

        // C `svt_aom_sig_deriv_mode_decision_config_default` — the picture-level
        // tool ladders. It is EXPORTED and gated at tier 1; the frame header
        // reads its `allow_high_precision_mv`, `allow_warped_motion`,
        // `is_motion_mode_switchable`, `mfmv_level` and `interpolation_filter`
        // so the header can never disagree with the tools the encode ran.
        //
        // Only computed for an inter frame — a key frame's header carries none
        // of those fields, so this is byte-inert for every still cell.
        let md_config_signals = if is_key {
            None
        } else {
            let pd = pic_decision.as_ref();
            crate::inter_hdr_arm::md_config_inputs(crate::inter_hdr_arm::PipelineMdInputs {
                enc_mode: self.speed_config.preset as i8,
                sq_qp: u32::from(self.rc_config.qp),
                base_q_idx: base_qindex,
                picture_qp: u32::from(pcs.qp),
                temporal_layer_index: temporal_layer,
                hierarchical_levels: self.gop.hierarchical_levels,
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
                ref_list0_count_try: pd.map_or(0, |p| u32::from(p.ref_list0_count)),
                ref_list1_count_try: pd.map_or(0, |p| u32::from(p.ref_list1_count)),
                // Every DPB slot still holds the key frame on the 2-frame
                // cell; the shadow DPB's POC 0 IS that key frame.
                ref_l0_is_islice: self.pd_ctx.dpb
                    [pd.map_or(0, |p| p.rps.ref_dpb_index[0] as usize)]
                .picture_number
                    == 0,
                ref_l1_is_islice: self.pd_ctx.dpb
                    [pd.map_or(0, |p| p.rps.ref_dpb_index[4] as usize)]
                .picture_number
                    == 0,
            })
            .and_then(
                crate::port_enc_mode_config::md_config::sig_deriv_mode_decision_config_default,
            )
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
                // Global-motion PARAMETER coding is unported and
                // `inter_hdr_arm::inter_signal` refuses a non-identity model,
                // so every reference is IDENTITY here by the same rule the
                // header is written under — not by assumption.
                gm_wmtype: [crate::port_entropy_inter::modes::TransformationType::Identity; 8],
                cur_order_hint: display_order as i32,
                ref_order_hint,
                // C `mfmv_controls` (enc_mode_config.c:8852) for the VALUE
                // and `frame_might_allow_ref_frame_mvs`
                // (entropy_coding.h:71) for its PRESENCE — the same two
                // rules `inter_hdr_arm::inter_signal` applies to write the
                // header bit, asserted equal to it below.
                use_ref_frame_mvs: seq_tools.enable_ref_frame_mvs
                    && seq_tools.enable_order_hint
                    && sigs.mfmv_level == 1,
            }
        });

        // The frame-constant MVP environment the pack derives `predmv` /
        // `inter_mode_ctx` / `drl_ctx` from (§1s items 2 and 3). Same
        // provenance rule as `inter_syntax_state` above: every field is the
        // one the HEADER announces, so the contexts the tile codes are the
        // ones a decoder rebuilds.
        let inter_mvp_env: Option<crate::partition::InterMdEnv> =
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
                    global_motion: [svtav1_types::motion::WarpedMotionParams::default(); 8],
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
                    // C's `av1_setup_motion_field` reset leaves every cell
                    // INVALID_MV; the temporal field itself is unported, so
                    // every `add_tpl_ref_mv` returns 0 — which is what sets
                    // the GLOBALMV bit of `mode_context` (§1t).
                    tpl_mvs: alloc::vec![
                        crate::inter_mvp::TplMvRef::default();
                        (((mi_rows + 32) >> 1) * tpl_stride) as usize
                    ],
                    tpl_stride,
                    sb64_sq_no4xn_geom: sb_size == 64,
                }
            });

        let inter_md_frame = match (
            frame_me.as_ref(),
            ref_padded_luma.as_deref(),
            inter_syntax_state.as_ref(),
            inter_mvp_env.as_ref(),
        ) {
            (Some(me), Some(padded), Some(st), Some(env)) => {
                // §1s item 8, the inter half: the same `md_frame_context`
                // the intra rate tables are built from.
                let default_fc = crate::entropy::context::FrameContext::new_default();
                let default_ic = crate::port_entropy_inter::InterCdfs::new_default();
                let (fc, ic) = match primary_ref_cdfs.as_deref() {
                    Some(prev) => (&prev.fc, &prev.fc.inter),
                    None => (&default_fc, &default_ic),
                };
                let (fac, ref_fac) = crate::inter_md_arm::build_inter_rates(fc, ic);
                let nmv = crate::inter_md_arm::nmv_cost_table(
                    &fc.nmvc,
                    crate::inter_mv_code::mv_precision(
                        st.allow_high_precision_mv,
                        st.force_integer_mv,
                    ),
                );
                Some(crate::inter_md_arm::InterMdFrame {
                    padded,
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
                })
            }
            _ => None,
        };

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
            sb_plan.as_ref().map(|p| p.sb_qindex.as_slice()),
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
            // The SAME resolved detector preset the frame-level derivation above
            // used, so the MD walk and the pack cannot disagree about screen
            // content (see the parameter's doc on `encode_tile_rows`).
            sc_preset,
            sc_arm,
            self.hdr.is_fork() && self.hdr.alt_ssim_tuning,
            self.hdr.is_fork() && self.hdr.alt_lambda_factors,
            (self.hdr.tune == crate::tune::TUNE_IQ)
                .then(|| crate::tune::iq_lambda_weight(picture_qp as u32)),
            ssim_factors.as_ref(),
            base_qindex,
            frame_tx_mode_select,
            tpl_adjusted_qp,
            picture_qp,
            lw_bump,
            self.hdr.tune == crate::tune::TUNE_IQ,
            self.hdr.sharpness,
            lambda,
            &self.speed_config,
            ref_frame_data.as_deref(),
            // The padded twin of the plane above, from the SAME DPB slot.
            ref_padded_luma.as_deref().map(|p| &p.y),
            inter_md_frame.as_ref(),
            primary_ref_cdfs.as_deref(),
            &mv_map,
            mv_map_stride,
            &sb_qp_offsets,
            chroma.is_some(),
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
            self.thread_count,
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
                    svtav1_types::try_vec![0u16; (w / 2) * (h / 2)]?,
                    svtav1_types::try_vec![0u16; (w / 2) * (h / 2)]?,
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
                let (cw, cxs, cxe) = (w / 2, x0 / 2, x1 / 2);
                let cst = cw;
                for r in y0 / 2..y1 / 2 {
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
        for (tile_idx, (tile_recon, tile_trees, _canvas10)) in tile_recons.into_iter().enumerate() {
            let (tile_sb_row_start, tile_sb_row_end) =
                tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (tile_sb_col_start, tile_sb_col_end) =
                tile_grid.col_span(tile_idx % tile_grid.tile_cols);
            let mut tile_trees = tile_trees.into_iter();
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check (no-op
                // for the default `Unstoppable` token — `may_stop()` is false).
                if stop.may_stop() {
                    stop.check()
                        .map_err(EncodeError::from)
                        .map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    tree_slots[sb_row * sb_cols + sb_col] = tile_trees.next();
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
                    let x0 = sb_col * sb_size;
                    let y0 = sb_row * sb_size;
                    let cur_w = sb_size.min(w - x0);
                    let cur_h = sb_size.min(h - y0);
                    for r in 0..cur_h {
                        for c in 0..cur_w {
                            recon[(y0 + r) * w + x0 + c] = tile_recon[offset + r * cur_w + c];
                        }
                    }
                    offset += cur_w * cur_h;

                    // Update MV map from reference
                    if let Some(ref rf) = ref_frame_data {
                        let sb_mv = crate::motion_est::full_pel_search(
                            &encode_input[y0 * w + x0..],
                            w,
                            rf,
                            w,
                            x0 as i32,
                            y0 as i32,
                            cur_w.min(16),
                            cur_h.min(16),
                            svtav1_types::motion::Mv::ZERO,
                            8,
                            8,
                            w,
                            h,
                        );
                        let bx0 = x0 / 8;
                        let by0 = y0 / 8;
                        let bx1 = (x0 + cur_w).div_ceil(8);
                        let by1 = (y0 + cur_h).div_ceil(8);
                        for by in by0..by1.min(h.div_ceil(8)) {
                            for bx in bx0..bx1.min(mv_map_stride) {
                                mv_map[by * mv_map_stride + bx] = sb_mv.mv;
                            }
                        }
                    }
                }
            }
        }
        let mut all_trees: Vec<crate::partition::PartitionTree> = tree_slots
            .into_iter()
            .map(|t| t.expect("every SB is covered by exactly one tile"))
            .collect();

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
            // Run the u16 re-encode ONLY on frames within the ported bd10
            // envelope (every luma leaf tx_depth 0, non-directional,
            // non-filter-intra). Outside it, fall back to the (non-panicking)
            // u8 output rather than crash the public encode_frame_420 API —
            // predict_unit_hbd / bd10_reencode_node panic loudly on unported
            // modes/tx_depth (task #94 follow-ups: dr_predict_hbd,
            // predict_filter_intra_hbd, tx_depth>0 re-encode). The supported
            // subset (currently the DC-family first cell) is exact; the rest is
            // WIP, so this keeps the encoder panic-free while the port grows.
            // SH intra edge filter for this frame (the FunnelCfg the u8 tree was
            // searched with). Directional bd10 leaves are only in-envelope when
            // it is off (the re-encode passes filt_type=0); for {M3,M6,M10,M13}
            // it is false, for M5 (4:2:0 still) true.
            let bd10_edge_filter =
                crate::leaf_funnel::FunnelCfg::for_preset(self.speed_config.preset).edge_filter;
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
            // NOTE (measured, task #94): the bd10 FULL-RD funnel now also
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
            // Where the FULL-RD funnel ran, it ALREADY produced this frame's
            // coded 10-bit levels and the committed 10-bit recon, computed
            // with each txb's REAL entropy contexts. This level-only post-pass
            // hardcodes the RDOQ contexts to 0/0 — correct only where
            // `real_coeff_ctx` is off — so letting it run on top REPLACES
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
                self.bit_depth,
                self.speed_config.preset,
                chroma.is_some(),
                w,
                h,
            );
            let bd10_postpass_runs = !bd10_full_rd
                && all_trees
                    .iter()
                    .all(|t| bd10_tree_supported(t, bd10_edge_filter));
            // INVARIANT (D4): the level-only post-pass hardcodes the RDOQ
            // txb_skip_ctx / dc_sign_ctx to 0/0 (leaf_funnel.rs), which is
            // correct ONLY where `real_coeff_ctx` is false — the eff-M9 band
            // (preset >= 9), the only band where `bd10_full_rd` is false for an
            // aligned frame. Enforce that coupling so it cannot silently re-open
            // if `bd10_full_rd_supported` is ever widened downward (e.g. a
            // preset <= 6 aligned bd10 SCREEN frame turns `bd10_full_rd` off via
            // palette_level != 0, where `real_coeff_ctx` is TRUE — the post-pass
            // must NOT run there). Debug-only; the reachable envelope satisfies it.
            debug_assert!(
                !bd10_postpass_runs
                    || !crate::leaf_funnel::FunnelCfg::for_preset(self.speed_config.preset)
                        .real_coeff_ctx,
                "bd10 level-only post-pass would run where real_coeff_ctx is true \
                 (preset {}): its 0/0 RDOQ contexts would miscode the levels",
                self.speed_config.preset
            );
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
                    .filter(|t| !bd10_tree_supported(t, bd10_edge_filter))
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
                // bd10 full MD lambda (C full_lambda_md[1], md_process.c:725-759):
                // computed from the bd10 rdmult base (dc_qlookup_10 + ROUND_
                // POWER_OF_TWO(,4) + frame-type-factor 128 + the *16), NOT a
                // ×16 of the bd8 lambda — see kf_full_lambda_bd10.
                let lambda_bd10 = u64::from(crate::pd0::kf_full_lambda_bd10(
                    base_qindex,
                    picture_qp as u32,
                ));
                let recon10 = bd10_reencode_luma(
                    &mut all_trees,
                    sb_cols,
                    sb_size,
                    w,
                    h,
                    &src10,
                    in_stride,
                    base_qindex,
                    cq.rdoq_level,
                    lambda_bd10,
                    cq.allintra_rd_mult,
                    bd10_edge_filter,
                    self.bit_depth,
                    qm_levels[0],
                    self.hdr.sharpness,
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
                    let uv10 = bd10_reencode_chroma(
                        &mut all_trees,
                        sb_cols,
                        sb_size,
                        w,
                        h,
                        &u10,
                        &v10,
                        w / 2,
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
                        bd10_edge_filter,
                        self.bit_depth,
                        [qm_levels[1], qm_levels[2]],
                        self.hdr.sharpness,
                    )?;
                    // Crop the SB-extent canvases to the in-frame planes every
                    // downstream consumer expects (the bd10 deblock-level /
                    // CDEF-strength / Wiener-LR searches compare them against
                    // `w*h` and `(w/2)*(h/2)` sources at the ALIGNED stride).
                    // Both are already aligned-strided, so the crop is a prefix.
                    let cn = (w / 2) * (h / 2);
                    self.last_recon10_uv = Some((uv10.0[..cn].to_vec(), uv10.1[..cn].to_vec()));
                }
                self.last_recon10_y = Some(recon10[..w * h].to_vec());
            }
        }

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

        // Step 6: Entropy coding — recursive partition tree encoding.
        // Walk each SB's partition tree in spec order (depth-first),
        // writing partition type at each node before recursing into children.
        //
        // For 4:2:0 the chroma blocks are predicted, transformed and
        // reconstructed INSIDE this walk (encode_block_syntax), so the
        // chroma coding order is structurally identical to the decoder's
        // parse order — the UV_DC prediction reads exactly the chroma
        // neighbors the decoder will have reconstructed.
        let cw = w / 2;
        // SB-extent chroma buffer (task #95 chunk 2): the pack reconstructs a
        // straddling boundary block's chroma past the aligned chroma extent, so
        // size u_recon/v_recon to the extent PRODUCT (aligned stride `cw`, a
        // right-straddle write wraps down into the slack). `ext == aligned` on a
        // 64-aligned frame → no-op. The final-recon crop + deblock/CDEF read
        // only the in-frame region at stride `cw`, unaffected by the slack.
        let ext_cbuf = (w.div_ceil(sb_size) * sb_size / 2) * (h.div_ceil(sb_size) * sb_size / 2);
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
        let walk_end_cdfs: core::cell::RefCell<Option<crate::port_frame_cdf::FrameCdfs>> =
            core::cell::RefCell::new(None);
        #[allow(clippy::type_complexity)]
        // inline tuple documents the shape; a `type` alias would hide it
        let run_entropy_walk = |lr: Option<&crate::restoration::FrameRestInfo>,
                                cdef_walk: Option<&crate::cdef::CdefPick>|
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
                    ectx.delta_q_state = Some((res, i32::from(base_qindex)));
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
                        let sb_idx = sb_row * sb_cols + sb_col;
                        let tree = &all_trees[sb_idx];
                        // [SVT_HDR_MODE] per-SB delta-q: the SB's planned qindex
                        // drives both the delta symbol and (via the search, which
                        // used the same plan) the coded coefficients. Chroma dequant
                        // per SB = sb_qindex + the FRAME chroma deltas.
                        if let Some(plan) = sb_plan.as_ref() {
                            let sbq = i32::from(plan.sb_qindex[sb_idx]);
                            ectx.delta_q_sb_qindex = sbq;
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
                        );
                    }
                }

                tile_bitstreams.push(writer.done().to_vec());
                // See `walk_end_cdfs`: overwritten per tile AND per walk, so
                // it ends holding the last tile of the last walk — C's own
                // "last tile wins" save order.
                *walk_end_cdfs.borrow_mut() = Some(crate::port_frame_cdf::FrameCdfs {
                    fc: frame_ctx,
                    coeff: coeff_fc,
                });
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
        let (mut tile_data, deblock_geom, mut u_recon, mut v_recon, mut tile_size_bytes_minus_1) =
            run_entropy_walk(None, None)?;

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
                chroma.is_some(),
            );
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
        let dlf_temporal_layer_index: u8 = 0;
        let dlf_is_base = dlf_temporal_layer_index == 0;
        let dlf_is_highest_layer = dlf_temporal_layer_index == self.gop.hierarchical_levels
            && self.gop.hierarchical_levels != 0;
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
            // `dlf_level_modulation`, which C runs only when `!is_base`.
            crate::port_enc_mode_config::leaf::get_dlf_level_default(
                dlf_enc_mode,
                dlf_is_not_last_layer,
                DLF_FAST_DECODE,
                dlf_resolution,
                dlf_is_base,
                crate::port_enc_mode_config::InputCoeffLvl::Normal,
                0,
            )
        };
        // C's `default:` arm is `assert(0)`; the port refuses rather than
        // inventing a control set.
        let dlf_ctrls = crate::port_enc_mode_config::ctrls::set_dlf_controls(dlf_level).ok_or(
            EncodeError::UnsupportedConfig("dlf level outside svt_aom_set_dlf_controls' 0..=7"),
        )?;
        let lf_levels = if sc_derivation.allow_intrabc || coded_lossless {
            crate::deblock::LfLevels::default()
        } else if is_key {
            if dlf_ctrls.enabled == 0 {
                // `enable_dlf_flag == 0` or a level-0 ladder entry: C neither
                // picks nor applies, and the header codes zeros.
                crate::deblock::LfLevels::default()
            } else if dlf_ctrls.sb_based_dlf == 0 {
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let early_exit_convergence = i32::from(dlf_ctrls.early_exit_convergence);
                match recon10.as_ref() {
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
                            chroma_420: true,
                            geom: &deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_search(&input)?
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
                            chroma_420: chroma.is_some(),
                            geom: &deblock_geom,
                            early_exit_convergence,
                            bit_depth: self.bit_depth,
                        };
                        crate::deblock::pick_filter_levels_full_search(&input)?
                    }
                }
            } else {
                // `sb_based_dlf = 1` -> LPF_PICK_FROM_Q.
                crate::deblock::pick_filter_levels_key_frame(base_qindex, self.bit_depth)
            }
        } else {
            crate::deblock::LfLevels::default()
        };
        // The in-loop post-filters (deblock -> CDEF) are applied here for two
        // possible consumers: (1) an in-frame search that measures distortion
        // on the filtered pixels — the CDEF search and the Wiener loop-
        // restoration search, both preset <= 6 — or the LR stripe-boundary
        // save; (2) the caller, via `last_recon*` / a later frame predicting
        // from this recon through the DPB. When NONE of those exist the
        // filtered pixels are dead: nothing reads them and the bitstream is
        // already written. C behaves identically (its preset-10 profile
        // contains zero CDEF/LPF samples for byte-identical output).
        //
        // Byte-inertness is measured, not assumed: skipping the two apply
        // passes changed 0/90 cells at presets 7..13 and 13/36 at presets 2/6
        // (tools/byteid_fingerprint.sh, {64,128,256} x qp{20,40,55} x
        // {gradient,uniform}) — see benchmarks/perf_postfilter_2026-08-11.meta.
        let postfilter_consumed = seq_tools.enable_restoration
            || crate::cdef::allintra_preset_uses_cdef_search(self.speed_config.preset)
            || self.recon_output
            // A later frame may predict from this recon via the DPB. Only an
            // all-key sequence (`intra_period <= 1`) provably has no such
            // reader — every `self.dpb.get(..)` site is gated on `!is_key`.
            || !is_single_frame;
        if self.recon_output {
            self.last_recon_unfiltered = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        if let Some((y10, u10, v10)) = recon10.as_mut()
            && lf_levels.any()
            && postfilter_consumed
        {
            crate::deblock::apply_deblock_frame_hbd(
                y10,
                u10,
                v10,
                w,
                h,
                true,
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff,
                self.bit_depth,
            );
        }
        if lf_levels.any() && postfilter_consumed {
            crate::deblock::apply_deblock_frame(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                chroma.is_some(),
                &deblock_geom,
                &lf_levels,
                lf_sharp_eff, // = signaled loop_filter_sharpness
            );
        }

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
        let cdef_zero_fs_cost_bias =
            crate::port_enc_mode_config::tail::set_cdef_recon_controls(cdef_recon_level)
                .ok_or(EncodeError::UnsupportedConfig(
                    "cdef recon level outside set_cdef_recon_controls' 0..=4",
                ))?
                .zero_fs_cost_bias;
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
        // NOT modelled, and named rather than dropped: C reaches this only
        // after `me_based_cdef_skip` (`md_config_process.c:781`) declined to
        // switch CDEF off, which needs ME distortion this pipeline does not
        // produce. `me_based_cdef_skip` returns false immediately on an
        // I_SLICE, so the omission is invisible on every key frame and is a
        // real gap on inter frames whose ME distortion would have tripped it.
        let mut cdef_force_off = false;
        if !is_key
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
        let cdef_params = if sc_derivation.allow_intrabc || coded_lossless {
            crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default())
        } else if cdef_force_off {
            // C `pcs->ppcs->cdef_level = 0` inside
            // `update_cdef_filters_on_ref_info`: no search, no application,
            // and the header codes zero strengths.
            crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default())
        } else if cdef_ctrls.use_reference_cdef_fs != 0 {
            // The reference-derived prediction REPLACES the search
            // (`md_config_process.c:713-722` / `:750-758`). Damping is still
            // this frame's own `CDEF_DAMPING_FROM_QP` (`enc_cdef.c:1446`) —
            // only the strengths come from the reference.
            crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams {
                damping: 3 + (base_qindex >> 6),
                y_strength: cdef_ctrls.pred_y_f as u8,
                uv_strength: cdef_ctrls.pred_uv_f as u8,
            })
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
                    crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_all_skip_search(
                        base_qindex,
                    ))
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
                            crate::cdef::cdef_search_still_hbd(
                                &cfg,
                                y10,
                                u10,
                                v10,
                                &sy10,
                                &su10,
                                &sv10,
                                w,
                                h,
                                true,
                                &deblock_geom,
                                base_qindex,
                                self.bit_depth,
                            )?
                        }
                        None => crate::cdef::cdef_search_still(
                            &cfg,
                            &recon,
                            &u_recon,
                            &v_recon,
                            &encode_input,
                            su,
                            sv,
                            w,
                            h,
                            chroma.is_some(),
                            &deblock_geom,
                            base_qindex,
                        )?,
                    };
                    match searched {
                        crate::cdef::CdefSearchPick::Picked(mut p) => {
                            // [SVT_HDR_MODE] fork cdef-scaling: search-path
                            // only (finish_cdef_search, enc_cdef.c:1444).
                            if self.hdr.is_fork() {
                                crate::cdef::scale_strengths(&mut p, self.hdr.cdef_scaling);
                            }
                            p
                        }
                        crate::cdef::CdefSearchPick::AllSkip => crate::cdef::CdefPick::single(
                            crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
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
                crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_key_frame(
                    base_qindex,
                    self.bit_depth,
                    sc_derivation.classes.sc_class5,
                ))
            } else {
                // `cdef_search_level == 0`: CDEF is off for this frame, so
                // neither arm runs and the header codes zero strengths. Only
                // reachable through a level-0 ladder entry, since the
                // IntraBC/lossless suppression is the outer branch above.
                crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default())
            }
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
        // walk is re-run with the emission armed (recon is untouched by
        // the extra syntax; C's EC pass simply runs after the cdef
        // search, ours re-runs the deterministic walk).
        if cdef_params.bits > 0 {
            let (tile_cdef, _geom_c, u_c, v_c, tsb_c) = run_entropy_walk(None, Some(&cdef_params))?;
            // The re-walk reproduces the PRE-filter recon; u_recon/v_recon
            // were deblocked IN PLACE above, so compare against the
            // pre-deblock copy (the old `== u_recon` form only held on
            // content where chroma deblock was a no-op — it fired
            // spuriously on flat+textured content at mid qp, mainline
            // included, pre-dating the fork work).
            #[cfg(debug_assertions)]
            if let Some((_, u_unf, v_unf)) = self.last_recon_unfiltered.as_ref() {
                debug_assert_eq!(&u_c, u_unf, "cdef re-walk chroma recon must be identical");
                debug_assert_eq!(&v_c, v_unf, "cdef re-walk chroma recon must be identical");
            }
            let _ = (&u_c, &v_c);
            tile_data = tile_cdef;
            tile_size_bytes_minus_1 = tsb_c;
        }
        // The pre-CDEF snapshot is load-bearing when LR is on (its stripe
        // boundaries are saved from it below), and an evidence aid otherwise.
        if seq_tools.enable_restoration || self.recon_output {
            self.last_recon_pre_cdef = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        }
        if postfilter_consumed {
            self.last_cdef_stats = crate::cdef::apply_cdef_frame(
                &mut recon,
                &mut u_recon,
                &mut v_recon,
                w,
                h,
                chroma.is_some(),
                &deblock_geom,
                &cdef_params,
            );
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
            crate::cdef::apply_cdef_frame_hbd(
                y10,
                u10,
                v10,
                w,
                h,
                true,
                &deblock_geom,
                &cdef_params,
                self.bit_depth,
            );
        }

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
        let mut lr_signal = crate::entropy::obu::LrSignal::none(seq_tools.enable_restoration);
        // IBC (chunk 1): unlike DLF/CDEF, C suppresses loop restoration at
        // PIPELINE EXECUTION, not signal-derivation — `if (ppcs->
        // enable_restoration && frm_hdr->allow_intrabc == 0)` gates BOTH the
        // search (rest_process.c:262) and the apply/finish (:325, else-arm
        // forces all planes RESTORE_NONE). enable_restoration itself (and
        // the SH bit) stays UNCHANGED — do NOT fold this into the
        // derivation (docs/ibc-port-map.md §A.7).
        if is_key && seq_tools.enable_restoration && !sc_derivation.allow_intrabc && !coded_lossless
        {
            // LOOP-RESTORATION LEVEL LADDERS — the `scs->allintra` fork
            // (`pd_process.c:4935-4938`), the same selector `sc_detect`, the
            // deblock ladder and the rate ladders already take.
            //
            // The all-intra arm is `wn_filter_level_allintra` (3 / 4 / off) with
            // `sg_filter_level_allintra` == 0 at every representable preset,
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
                    crate::port_lr_level::SgFilterCtrls::default(),
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
                let (lr_tcw, lr_tch) = (lr_true_w.div_ceil(2), lr_true_h.div_ceil(2));
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
                let rest_info = match recon10.as_ref() {
                    Some((y10, u10, v10)) => {
                        let sh = (self.bit_depth - 8) as u32;
                        let widen_tight =
                            |src: &[u8], src_stride: usize, pw: usize, ph: usize| -> Vec<u16> {
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
                        crate::restoration::search_restoration_still_bd(
                            &ctrls,
                            &sg_ctrls,
                            &lr_sy10,
                            &lr_su10,
                            &lr_sv10,
                            &tight10(y10, w, lr_true_w, lr_true_h),
                            &tight10(u10, w / 2, lr_tcw, lr_tch),
                            &tight10(v10, w / 2, lr_tcw, lr_tch),
                            lr_true_w,
                            lr_true_h,
                            true,
                            rdmult,
                            self.bit_depth,
                        )?
                    }
                    None => crate::restoration::search_restoration_still_bd::<u8>(
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
                        chroma.is_some(),
                        rdmult,
                        8,
                    )?,
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
                    // Tile pass 2: identical symbol stream + LR syntax.
                    let cdef_walk_opt = (cdef_params.bits > 0).then_some(&cdef_params);
                    let (tile_lr, _geom2, u2, v2, tsb_lr) =
                        run_entropy_walk(Some(&rest_info), cdef_walk_opt)?;
                    // Same pre-deblock reference as the CDEF re-walk assert:
                    // u_recon/v_recon have been deblocked (and CDEF'd) in
                    // place by now; the walk reproduces the pre-filter state.
                    #[cfg(debug_assertions)]
                    if let Some((_, u_unf, v_unf)) = self.last_recon_unfiltered.as_ref() {
                        debug_assert_eq!(&u2, u_unf, "LR re-walk chroma recon must be identical");
                        debug_assert_eq!(&v2, v_unf, "LR re-walk chroma recon must be identical");
                    }
                    let _ = (&u2, &v2);
                    tile_data = tile_lr;
                    tile_size_bytes_minus_1 = tsb_lr;

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
                        chroma.is_some(),
                    );
                    crate::restoration::apply_restoration_frame(
                        &mut recon,
                        &mut u_recon,
                        &mut v_recon,
                        lr_true_w,
                        lr_true_h,
                        w,
                        cw,
                        chroma.is_some(),
                        &rest_info,
                        &bounds,
                    );
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
                            w / 2,
                            true,
                        );
                        crate::restoration::apply_restoration_frame_bd::<u16>(
                            y10,
                            u10,
                            v10,
                            lr_true_w,
                            lr_true_h,
                            w,
                            w / 2,
                            true,
                            &rest_info,
                            &bounds10,
                            self.bit_depth,
                        );
                    }
                }
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
            }
        }

        // Step 6b: Film grain estimation (compare source to reconstruction)
        let _grain_params = crate::film_grain::estimate_film_grain(&encode_input, &recon, w, h, w);
        // grain_params would be signaled in the frame header OBU
        // and used by the decoder to re-synthesize grain

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
        let inter_signal: Option<crate::entropy::obu::InterSignal> = if is_key {
            None
        } else {
            let pic = pic_decision.as_ref().ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "an inter frame needs the picture decision, which this port so far runs \
                     only when a GOP is configured (intra_period > 1)",
                ))
            })?;
            let ref_queue = crate::inter_hdr_arm::ref_queue_from_dpb(&self.pd_ctx, base_qindex);
            let binding = crate::port_picstruct::bind_refs_and_primary_ref_frame(
                pic, &ref_queue,
                // C `frame_end_cdf_update_mode` — the picture manager assigns
                // REFRESH_FRAME_CONTEXT_BACKWARD to coded pictures, which is
                // the same fact the header's `disable_frame_end_update_cdf = 0`
                // records (obu.rs).
                true, /*is_s_frame=*/ false,
            );
            let sigs = md_config_signals.ok_or_else(|| {
                whereat::at!(EncodeError::UnsupportedConfig(
                    "the inter frame header needs sig_deriv_mode_decision_config_default's \
                     signals, and the reference statistics they read (ref_hp_percentage, \
                     ref_skip_percentage) are unported so far — so this configuration is \
                     refused rather than answered from a placeholder",
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
                )
                .map_err(|_| {
                    whereat::at!(EncodeError::UnsupportedConfig(
                        "an inter frame header field is not implemented for this \
                         configuration: use_ref_frame_mvs at mfmv_level >= 2 needs the TPL r0 \
                         and the references' own is_mfmv_used, and global motion's parameter \
                         coding is unported (crate::inter_hdr_arm::InterHdrError)",
                    ))
                })?,
            )
        };

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
                    is_single_frame,
                    self.bit_depth,
                    &self.color_description,
                    chroma.is_none(), // mono_chrome unless the 4:2:0 path is active
                    // seq_level_idx auto-derivation input (C: scs->frame_rate).
                    self.rc_config.framerate,
                    seq_tools,
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
                is_single_frame,
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
                if chroma_deltas.is_zero() {
                    None
                } else if self.hdr.is_fork() {
                    // The fork's SH signals separate_uv_delta_q = 1, so the FH
                    // carries diff_uv_delta + four independent deltas (its U
                    // delta has a further +12, so U and V really do differ).
                    Some(crate::entropy::obu::ChromaQSignal::Separate([
                        chroma_deltas.u_dc,
                        chroma_deltas.u_ac,
                        chroma_deltas.v_dc,
                        chroma_deltas.v_ac,
                    ]))
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

        // Step 7: Publish recon for the recon-parity gate, then update DPB.
        //
        // Superres chunk B.3: what a DECODER outputs is the coded-width recon
        // normatively upscaled back to `upscaled_width` (C
        // `svt_av1_superres_upscale_frame`, cdef_process.c:152 — after CDEF,
        // before loop restoration; LR is off for every config this port lets
        // superres run at, see `superres_config_error`, so "after CDEF" is
        // here). The BITSTREAM is unaffected: nothing downstream of this point
        // codes symbols. No-op when superres is off.
        if self.superres_denom.is_some() {
            let (cw, uw, hh) = (
                self.width as usize,
                self.upscaled_width as usize,
                self.height as usize,
            );
            let mut y_up = svtav1_types::try_vec![0u8; uw * hh]?;
            svtav1_dsp::superres::upscale_normative_plane(&recon, cw, cw, &mut y_up, uw, uw, hh);
            recon = y_up;
            if chroma.is_some() {
                let (ccw, cuw, chh) = (cw / 2, uw.div_ceil(2), hh / 2);
                let mut u_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                let mut v_up = svtav1_types::try_vec![0u8; cuw * chh]?;
                svtav1_dsp::superres::upscale_normative_plane(
                    &u_recon, ccw, ccw, &mut u_up, cuw, cuw, chh,
                );
                svtav1_dsp::superres::upscale_normative_plane(
                    &v_recon, ccw, ccw, &mut v_up, cuw, cuw, chh,
                );
                u_recon = u_up;
                v_recon = v_up;
            }
        }
        if self.recon_output {
            self.last_recon = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
            // Issue #13: the 10-bit final recon (deblock -> CDEF -> LR all
            // applied to the 10-bit canvas). No superres arm: bd10 + superres
            // is refused (`superres_config_error`), so the canvas is already
            // at the output geometry.
            self.last_recon10_final = recon10;
        }
        // C `pad_ref_and_set_flags` (enc_dec_process.c:1072-1112): the recon
        // is padded with a replicated margin BEFORE it becomes a reference,
        // because inter prediction indexes negative offsets from pixel
        // (0,0). Built here, once, from the same buffers stored below.
        let padded_ref = {
            let (rw, rh) = (w, h);
            let y =
                crate::picture::PaddedPlane::from_plane(&recon, rw, rh, crate::picture::REF_BORDER);
            let uv = if chroma.is_some() {
                // C `(border + ss_x) >> ss_x` at 4:2:0 (:1102-1112).
                let cb = (crate::picture::REF_BORDER + 1) >> 1;
                Some((
                    crate::picture::PaddedPlane::from_plane(&u_recon, rw / 2, rh / 2, cb),
                    crate::picture::PaddedPlane::from_plane(&v_recon, rw / 2, rh / 2, cb),
                ))
            } else {
                None
            };
            alloc::boxed::Box::new(crate::picture::PaddedRef { y, uv })
        };
        let ref_frame = ReferenceFrame {
            padded: Some(padded_ref),
            y_plane: recon,
            // 4:2:0 chroma recon, empty on the monochrome path. Inter
            // prediction needs all three planes; see `ReferenceFrame::u_plane`.
            u_plane: if chroma.is_some() {
                u_recon.clone()
            } else {
                alloc::vec::Vec::new()
            },
            v_plane: if chroma.is_some() {
                v_recon.clone()
            } else {
                alloc::vec::Vec::new()
            },
            // C `rest_process.c:207-210`: the strengths the FRAME HEADER
            // signalled, not the ones the search proposed. A later frame's
            // CDEF candidate set is rewritten from these
            // (`update_cdef_filters_on_ref_info`).
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
            width: self.width,
            height: self.height,
            display_order,
            order_hint: display_order as u32,
        };
        self.dpb.refresh(pcs.refresh_frame_flags, &ref_frame);
        // The PA (picture-analysis) reference the NEXT frame's open-loop
        // motion search reads — this frame's padded SOURCE pyramid, not its
        // recon. `None` in still mode, where no later frame exists.
        if pa_cur.is_some() {
            self.pa_ref = pa_cur;
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

#[allow(clippy::too_many_arguments)]
/// Mode tracking for the encoder's entropy coding context.
///
/// Tracks intra mode and skip status at 4x4 block granularity, matching
/// the decoder's above/left BlockContext arrays. This is required for
/// correct CDF context derivation in keyframe y_mode and skip coding.
///
/// Also tracks partition context at 8x8 granularity, matching the rav1d
/// decoder's `BlockContext.partition` arrays. This is essential for multi-SB
/// frames where the partition context of one SB depends on its neighbors.
#[derive(Clone)]
pub(crate) struct EntropyCtx {
    /// Above row modes (at 4x4 granularity), indexed by column in 4x4 units.
    /// Updated after each block is encoded.
    above_mode: Vec<u8>,
    /// Left column modes (at 4x4 granularity), indexed by row in 4x4 units.
    left_mode: Vec<u8>,
    /// Above/left UV modes (4x4 granularity) — C's chroma_above/left_mbmi
    /// uv_mode inputs to `get_filt_type(xd, plane > 0)` (the intra edge
    /// filter's smooth-neighbour strength selector). With min-8x8 blocks
    /// every mi of a neighbour block carries the same uv mode, so the
    /// luma-granular arrays reproduce C's bottom-right-of-group pick.
    above_uv_mode: Vec<u8>,
    left_uv_mode: Vec<u8>,
    /// Above row skip flags.
    above_skip: Vec<bool>,
    /// Left column skip flags.
    left_skip: Vec<bool>,
    /// Above partition context at 8x8 granularity (full frame width).
    /// Each byte stores partition depth bits, matching rav1d's `a.partition`.
    above_partition: Vec<u8>,
    /// Left partition context at 8x8 granularity (one SB column height).
    /// Reset at the start of each SB row, matching rav1d's `t.l.partition`.
    left_partition: Vec<u8>,
    /// Above coefficient neighbor bytes at 4x4 granularity:
    /// `(dc_sign << 6) | min(cul_level, 63)`, 0xFF = unavailable (frame edge).
    above_coeff: Vec<u8>,
    /// Left coefficient neighbor bytes at 4x4 granularity.
    left_coeff: Vec<u8>,
    /// Above coefficient neighbor bytes for the chroma planes (U = 0,
    /// V = 1), in CHROMA-plane 4x4 units (each unit covers 8x8 luma
    /// pixels). Same encoding and INVALID convention as the luma arrays;
    /// the decoder keeps per-plane entropy context arrays exactly like
    /// this (libaom pd->above/left_entropy_context, zeroed per tile;
    /// 0xFF-skip == zero contribution, matching svt_aom_get_txb_ctx).
    above_coeff_uv: [Vec<u8>; 2],
    /// Left coefficient neighbor bytes for the chroma planes.
    left_coeff_uv: [Vec<u8>; 2],
    /// Above TXFM context at 4x4 granularity: the WIDTH in pixels of the
    /// last coded TX in each mi column (C TXFM_CONTEXT / txfm_context_array
    /// top array, maintained by set_txfm_ctxs, entropy_coding.c:4614).
    /// Init value is never read: get_tx_size_context gates on
    /// availability, and every available cell was written by a previous
    /// block (blocks are coded in z-order).
    above_txfm: Vec<u8>,
    /// IBC chunk 9: per-4x4 above-neighbour INTER block dims (0 = intra)
    /// — the get_tx_size_context is_inter override state (Root 6).
    above_inter_bw: Vec<u8>,
    /// Left TXFM context at 4x4 granularity: the HEIGHT in pixels of the
    /// last coded TX in each mi row.
    left_txfm: Vec<u8>,
    /// Per-4x4 left-neighbour INTER block dims (0 = intra).
    left_inter_bh: Vec<u8>,
    /// Above row luma palette_size (4x4 granularity), 0 = no palette — C's
    /// `above_mbmi->palette_mode_info.palette_size` read back by
    /// `svt_aom_get_palette_mode_ctx` / `svt_get_palette_cache_y`. Full
    /// frame width, like `above_mode` (NOT reset per SB row — the SB-row
    /// drop rule for the color cache lives in [`palette_cache`], not here).
    above_palette: Vec<u8>,
    /// Left column luma palette_size (4x4 granularity), 0 = no palette.
    left_palette: Vec<u8>,
    /// Above row palette colors (4x4 granularity), aligned with
    /// `above_palette`: the first `above_palette[i]` entries of
    /// `above_palette_colors[i]` are that neighbor's ascending palette
    /// (C `above_mbmi->palette_mode_info.palette_colors`); the rest are
    /// stale/zero and MUST NOT be read.
    above_palette_colors: Vec<[u16; svtav1_types::prediction::PALETTE_MAX_SIZE]>,
    /// Left column palette colors (4x4 granularity), aligned with
    /// `left_palette`.
    left_palette_colors: Vec<[u16; svtav1_types::prediction::PALETTE_MAX_SIZE]>,
    /// The sequence header's `enable_filter_intra` bit (C
    /// `scs->seq_header.filter_intra_level`, read by the block walk at
    /// entropy_coding.c:5099-5100): when set, every eligible intra block
    /// (DC_PRED, no palette, both dims <= 32) codes a `use_filter_intra`
    /// symbol. Sequence-level walk config, not per-block state — carried
    /// here because the walk already threads this context everywhere.
    seq_filter_intra: bool,
    /// FH `tx_mode == TX_MODE_SELECT` (C `frm_hdr->tx_mode`, written by
    /// `crate::txs_arm::tx_mode_select`). `av1_code_tx_size`
    /// (entropy_coding.c:4650) codes the per-block `tx_depth` symbol ONLY at
    /// TX_MODE_SELECT; at TX_MODE_LARGEST the decoder INFERS the largest tx
    /// size and the symbol must not appear.
    ///
    /// This is frame-level walk config for the same reason `seq_filter_intra`
    /// is. It was `is_key` until 2026-09-01 — a stale ALLINTRA premise (that
    /// arm signals TX_MODE_SELECT unconditionally), so on a VIDEO-mode key
    /// frame at preset >= 10, where `txs_level == 0` makes the video arm
    /// signal TX_MODE_LARGEST, the header said LARGEST and the walk still
    /// wrote a `tx_size_cdf` symbol per block. That is an undecodable stream,
    /// not merely a parity gap.
    tx_mode_select: bool,
    /// FH `allow_screen_content_tools` — gates the per-block no-palette
    /// flag coding (C write_palette_mode_info gate, entropy_coding.c:5026).
    allow_sct: bool,
    /// FH `allow_intrabc` — gates the per-block `use_intrabc` flag coding
    /// (C write_modes_b -> write_intrabc_info, entropy_coding.c:5021-5023;
    /// the flag is coded for EVERY block on an IBC frame). Default false;
    /// stamped post-construction (like `tile_top_px`) by the real pack walk
    /// AND the funnel chain sim — both must code it or the intrabc CDF (and
    /// every later symbol's arithmetic state) desyncs from C.
    allow_intrabc: bool,
    /// Encoder bit depth (8 or 10) — C
    /// `ppcs->scs->static_config.encoder_bit_depth`.
    ///
    /// The pack walk needs it for the palette COLOUR literals, whose width IS
    /// the encoder bit depth (`write_palette_colors_y`,
    /// entropy_coding.c:4369 -> :4256-4288). It was hardcoded to 8, which
    /// desyncs the arithmetic decoder on the first 10-bit palette block.
    /// Passed to [`EntropyCtx::new`] rather than stamped afterwards like
    /// `allow_intrabc`: a forgotten stamp here is a silent bitstream
    /// corruption, and the compiler can enforce a parameter.
    /// ALIGNED frame extent in PIXELS (`width_4x4 * 4`). Needed by any packer
    /// that must clip a block to the part inside the frame -- see the palette
    /// map tokens, which C writes over `rows_within_bounds x cols_within_bounds`
    /// (`svt_aom_get_block_dimensions`, palette.c:217-245), not the full block.
    aligned_w_px: usize,
    aligned_h_px: usize,
    bit_depth: u8,
    /// [SVT_HDR_MODE] per-SB delta-q emission state (C write_modes_b,
    /// entropy_coding.c:4997): `Some((delta_q_res, prev_qindex))` when the
    /// FH signaled delta_q_present. The walk arms `delta_q_pending` with
    /// the SB's target qindex at each SB start; the FIRST block whose
    /// origin is the SB corner (and bsize != SB size || !skip) emits
    /// `(cur - prev) / res` via av1_write_delta_q_index and updates prev.
    pub delta_q_state: Option<(u8, i32)>,
    /// The current SB's target qindex, set by the walk at SB start.
    pub delta_q_sb_qindex: i32,
    /// Pending `cdef_idx` emission for the CURRENT superblock — C
    /// `write_cdef` (entropy_coding.c:3986-4017). Set at SB start by the
    /// walk when `cdef_bits > 0`, `None` otherwise.
    cdef_sb: Option<CdefSbState>,
    /// Task #86: the Y-origin (LUMA pixel domain) of the current tile's
    /// own top row — see `PartitionSearchConfig::tile_top_px`'s doc for
    /// why this must gate "above" availability instead of frame-absolute
    /// y=0. 0 = single tile row (default, set by `EntropyCtx::new`); the
    /// per-tile entropy walk sets it explicitly per tile_idx.
    pub(crate) tile_top_px: usize,
    /// Task #96: the X-origin (LUMA pixel domain) of the current tile's
    /// own left column — the column analogue of [`Self::tile_top_px`].
    /// AV1 intra prediction and every above/left CONTEXT lookup stop at a
    /// tile boundary in BOTH axes; a block at a tile's own left column has
    /// no "left" neighbour even when it is not the frame's left column.
    /// 0 = single tile column (default), which is what every pre-#96 cell
    /// encodes, so gating on this is byte-neutral there.
    pub(crate) tile_left_px: usize,
    /// The same tile rect in LUMA mi units, INCLUDING the ends, for the MD
    /// prediction path (`intra_edge::DrGeom`'s four availability
    /// predicates need `mi_col_end` / `mi_row_end`, which the two px
    /// origins above cannot express). Defaults to the whole frame, so a
    /// single-tile encode is byte-identical. The origins and this field
    /// are assigned together at each of the (few) tile-walk sites and a
    /// debug_assert keeps them consistent.
    pub(crate) tile_mi: crate::intra_edge::TileMi,
    /// C `xd->above_mbmi` / `xd->left_mbmi` as
    /// [`crate::port_entropy_inter::block::write_inter_mode_info`] reads
    /// them, at 4x4 granularity — `docs/INTER-ENCODE-PLAN.md` §1s item 2's
    /// mi grid, restricted to the fields the inter contexts touch. Same
    /// shapes and same stamping cadence as `above_mode`/`left_mode`: the
    /// above row is frame-wide, the left column is one SB column high, and
    /// every coded block writes its own span.
    ///
    /// A `Default` entry is C's zeroed `MbModeInfo`, i.e. `DC_PRED` with
    /// `ref_frame = {0, 0}`. That is never READ: every lookup is gated on
    /// `tile_top_px` / `tile_left_px` first, so an unwritten cell is
    /// unreachable, exactly like `above_txfm`'s.
    above_nmi: Vec<crate::port_entropy_inter::NeighborMi>,
    left_nmi: Vec<crate::port_entropy_inter::NeighborMi>,
    /// The FULL mode-info grid `inter_mvp::setup_ref_mv_list` scans — C's
    /// `pcs->mi_grid_base` as `svt_aom_update_mi_map` leaves it
    /// (`docs/INTER-ENCODE-PLAN.md` §1s item 2). The above/left rows above
    /// are the two cells the entropy CONTEXTS read; the MVP walk reads rows
    /// -1..-3, columns -1..-3 and the top-right cell, so it needs the grid.
    ///
    /// Empty on a key frame, where every MVP scan is the IntraBC one
    /// (`crate::intrabc_mvp`, which keeps its own grid in the funnel).
    mvp_grid: Vec<crate::intrabc_mvp::MvpMiEntry>,
    /// The frame-constant half of the MVP environment. `Some` exactly when
    /// [`Self::inter_syntax`] is.
    pub(crate) mvp_env: Option<crate::partition::InterMdEnv>,
    /// The frame-level inter syntax the pack's inter arm needs. `Some`
    /// exactly on a non-key frame; the arm refuses without it rather than
    /// inventing a header it cannot have read.
    pub(crate) inter_syntax: Option<InterSyntaxState>,
}

/// The owned twin of
/// [`crate::port_entropy_inter::block::InterFrameSyntax`], which borrows its
/// two tables. Held per frame by [`EntropyCtx`] and lent out per block.
#[derive(Clone, Debug)]
pub(crate) struct InterSyntaxState {
    pub reference_mode: crate::port_entropy_inter::refframe::ReferenceMode,
    pub interpolation_filter: u8,
    pub enable_dual_filter: bool,
    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_jnt_comp: bool,
    pub enable_order_hint: bool,
    pub order_hint_bits: u32,
    pub is_motion_mode_switchable: bool,
    pub allow_warped_motion: bool,
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub gm_wmtype: [crate::port_entropy_inter::modes::TransformationType; 8],
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 7],
    /// C `frm_hdr->use_ref_frame_mvs`. NOT part of
    /// [`crate::port_entropy_inter::block::InterFrameSyntax`] — the entropy
    /// walk never reads it — but the MVP walk does, and it is the same
    /// frame-header bit derived from the same signals, so it is carried
    /// with them rather than re-derived somewhere the header cannot see.
    pub use_ref_frame_mvs: bool,
}

impl InterSyntaxState {
    pub(crate) fn syntax(&self) -> crate::port_entropy_inter::block::InterFrameSyntax<'_> {
        crate::port_entropy_inter::block::InterFrameSyntax {
            reference_mode: self.reference_mode,
            interpolation_filter: self.interpolation_filter,
            enable_dual_filter: self.enable_dual_filter,
            enable_interintra_compound: self.enable_interintra_compound,
            enable_masked_compound: self.enable_masked_compound,
            enable_jnt_comp: self.enable_jnt_comp,
            enable_order_hint: self.enable_order_hint,
            order_hint_bits: self.order_hint_bits,
            is_motion_mode_switchable: self.is_motion_mode_switchable,
            allow_warped_motion: self.allow_warped_motion,
            allow_high_precision_mv: self.allow_high_precision_mv,
            force_integer_mv: self.force_integer_mv,
            gm_wmtype: &self.gm_wmtype,
            cur_order_hint: self.cur_order_hint,
            ref_order_hint: &self.ref_order_hint,
        }
    }
}

/// C `write_cdef`'s per-superblock state (entropy_coding.c:3986-4017).
///
/// The CDEF filter block is 64x64 **always**, so an SB128 superblock covers
/// FOUR of them and C emits up to four `cdef_bits` literals per SB — one at
/// the first non-skip coding block of each quadrant — latched by
/// `cdef_transmitted[4]`:
///
/// ```text
/// const int32_t mask  = 1 << (6 - MI_SIZE_LOG2);            // 16 mi = 64 px
/// const int32_t index = sb_size == BLOCK_128X128
///     ? !!(mi_col & mask) + 2 * !!(mi_row & mask) : 0;
/// if (!ctx->cdef_transmitted[index] && !skip) {
///     aom_write_literal(w, mbmi->cdef_strength, cdef_bits);
///     ctx->cdef_transmitted[index] = true;
/// }
/// ```
///
/// The strength itself is read off the **b64 grid** — C takes the mbmi at
/// `(mi_row & ~15, mi_col & ~15)`, i.e. the 64-aligned mi — which is what
/// [`Self::strengths`] caches per quadrant.
///
/// At SB64 there is exactly one quadrant, `index` is always 0, and this is
/// bit-for-bit the previous single-slot behaviour.
///
/// NOTE the three-phase CDEF contract that docs/sb128-port-map.md flags as
/// the highest-risk SB128 chunk (search skips stale quadrants / strengths
/// fan out to covered quadrants / dirinit forced fresh) collapses to a
/// no-op here, because on a KEY frame the 128 root is ALWAYS split (see
/// `merge_sb_units`) so NO coding block is ever a 128-variant. Every 64x64
/// filter block owns its own blocks and its own searched strength, exactly
/// as at SB64. Only this WRITE side differs.
#[derive(Clone, Copy, Debug)]
struct CdefSbState {
    /// C `cdef_bits` (> 0, else the walk stores `None`).
    bits: u8,
    /// Per-quadrant strength index, b64-grid order (0=TL, 1=TR, 2=BL, 3=BR).
    strengths: [u8; 4],
    /// C `ctx->cdef_transmitted[4]`, reset at each SB top-left.
    transmitted: [bool; 4],
    /// SB128: quadrant index varies. SB64: always slot 0.
    sb128: bool,
}

/// Live state for the 4:2:0 chroma pass, threaded through the entropy walk
/// so every leaf's chroma blocks are predicted from — and reconstructed
/// into — the chroma planes in exact coding order (identical to the
/// decoder's parse order; the walk IS the bitstream order).
struct ChromaPass<'a> {
    u_src: &'a [u8],
    v_src: &'a [u8],
    u_recon: &'a mut [u8],
    v_recon: &'a mut [u8],
    /// Chroma plane stride (= frame_width / 2).
    stride: usize,
    /// Per-plane chroma quantization qindexes: clamp(base + FH
    /// delta_q_ac[plane]). Both == base_qindex in mainline mode (all FH
    /// chroma deltas 0); the fork's chroma-q path sets them independently
    /// and the FH signals the deltas (chroma_q.rs).
    qindex_u: u8,
    qindex_v: u8,
    /// [SVT_HDR_MODE] per-plane chroma QM levels (15 = off).
    qm_u: u8,
    qm_v: u8,
    /// Frame-level C-exact coding quantizer (still path) — C's MDS3 RDOQ
    /// covers chroma too (skip_uv cleared when enc-dec is bypassed).
    c_quant: Option<&'a crate::quant::CodingQuantCfg>,
}

/// Partition context update lookup table, matching rav1d's `dav1d_al_part_ctx`.
///
/// Indexed as `AL_PART_CTX[direction][block_level][partition_type]`.
/// direction: 0 = above, 1 = left.
/// block_level: 0 = Bl128x128, 1 = Bl64x64, 2 = Bl32x32, 3 = Bl16x16, 4 = Bl8x8.
/// partition_type: 0=NONE, 1=HORZ, 2=VERT, 3=SPLIT, 4-9=extended.
/// Value 0xff marks invalid combinations (SPLIT doesn't update directly).
static AL_PART_CTX: [[[u8; 10]; 5]; 2] = [
    // Above context
    [
        [0x00, 0x00, 0x10, 0xff, 0x00, 0x10, 0x10, 0x10, 0xff, 0xff], // Bl128x128
        [0x10, 0x10, 0x18, 0xff, 0x10, 0x18, 0x18, 0x18, 0x10, 0x1c], // Bl64x64
        [0x18, 0x18, 0x1c, 0xff, 0x18, 0x1c, 0x1c, 0x1c, 0x18, 0x1e], // Bl32x32
        [0x1c, 0x1c, 0x1e, 0xff, 0x1c, 0x1e, 0x1e, 0x1e, 0x1c, 0x1f], // Bl16x16
        [0x1e, 0x1e, 0x1f, 0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // Bl8x8
    ],
    // Left context
    [
        [0x00, 0x10, 0x00, 0xff, 0x10, 0x10, 0x00, 0x10, 0xff, 0xff], // Bl128x128
        [0x10, 0x18, 0x10, 0xff, 0x18, 0x18, 0x10, 0x18, 0x1c, 0x10], // Bl64x64
        [0x18, 0x1c, 0x18, 0xff, 0x1c, 0x1c, 0x18, 0x1c, 0x1e, 0x18], // Bl32x32
        [0x1c, 0x1e, 0x1c, 0xff, 0x1e, 0x1e, 0x1c, 0x1e, 0x1f, 0x1c], // Bl16x16
        [0x1e, 0x1f, 0x1e, 0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], // Bl8x8
    ],
];

impl EntropyCtx {
    pub(crate) fn new(
        width_4x4: usize,
        height_4x4: usize,
        seq_filter_intra: bool,
        // FH `tx_mode == TX_MODE_SELECT` — see the field's doc.
        tx_mode_select: bool,
        allow_sct: bool,
        bit_depth: u8,
    ) -> Self {
        let width_8x8 = width_4x4.div_ceil(2);
        let height_8x8 = height_4x4.div_ceil(2);
        // Chroma-plane 4x4 units: (w/2)/4 = width_4x4/2 (frames are
        // 64-aligned so this divides exactly; div_ceil for safety).
        let width_c4 = width_4x4.div_ceil(2);
        let height_c4 = height_4x4.div_ceil(2);
        Self {
            aligned_w_px: width_4x4 * 4,
            aligned_h_px: height_4x4 * 4,
            above_mode: alloc::vec![0u8; width_4x4], // DC_PRED = 0
            left_mode: alloc::vec![0u8; height_4x4],
            above_uv_mode: alloc::vec![0u8; width_4x4],
            left_uv_mode: alloc::vec![0u8; height_4x4],
            above_skip: alloc::vec![false; width_4x4],
            left_skip: alloc::vec![false; height_4x4],
            above_partition: alloc::vec![0u8; width_8x8],
            left_partition: alloc::vec![0u8; height_8x8],
            // 0xFF = INVALID_NEIGHBOR_DATA at frame edges, like C's
            // neighbor-array init.
            above_coeff: alloc::vec![0xFFu8; width_4x4],
            left_coeff: alloc::vec![0xFFu8; height_4x4],
            above_coeff_uv: [alloc::vec![0xFFu8; width_c4], alloc::vec![0xFFu8; width_c4]],
            left_coeff_uv: [
                alloc::vec![0xFFu8; height_c4],
                alloc::vec![0xFFu8; height_c4],
            ],
            // C inits the TXFM neighbour arrays to NEIGHBOR_ARRAY_INVALID
            // (0xFF, neighbor_arrays.h:30 / svt_aom_neighbor_array_unit_reset).
            // The intra tx_size ctx never sees it (availability-gated), but
            // the IBC var-tx `txfm_partition_context` reads the RAW byte
            // with NO availability gate (`*above_ctx < txw`,
            // entropy_coding.c:4490) — a 0 init flips a/l to 1 at
            // tile-top/left blocks and desyncs the txfm_partition CDF row
            // vs the decoder (the chunk-8 gui corruption root).
            above_txfm: alloc::vec![0xFFu8; width_4x4],
            above_inter_bw: alloc::vec![0u8; width_4x4],
            left_txfm: alloc::vec![0xFFu8; height_4x4],
            left_inter_bh: alloc::vec![0u8; height_4x4],
            above_palette: alloc::vec![0u8; width_4x4],
            left_palette: alloc::vec![0u8; height_4x4],
            above_palette_colors: alloc::vec![
                [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
                width_4x4
            ],
            left_palette_colors: alloc::vec![
                [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
                height_4x4
            ],
            seq_filter_intra,
            tx_mode_select,
            allow_sct,
            bit_depth,
            allow_intrabc: false,
            delta_q_state: None,
            delta_q_sb_qindex: 0,
            cdef_sb: None,
            tile_top_px: 0,
            tile_left_px: 0,
            tile_mi: crate::intra_edge::TileMi {
                mi_row_start: 0,
                mi_row_end: height_4x4,
                mi_col_start: 0,
                mi_col_end: width_4x4,
            },
            above_nmi: alloc::vec![
                crate::port_entropy_inter::NeighborMi::default();
                width_4x4
            ],
            left_nmi: alloc::vec![
                crate::port_entropy_inter::NeighborMi::default();
                height_4x4
            ],
            inter_syntax: None,
            mvp_grid: Vec::new(),
            mvp_env: None,
        }
    }

    /// Allocate the mode-info grid for a frame that has references
    /// (`docs/INTER-ENCODE-PLAN.md` §1s item 2). Separate from `new` because
    /// a key frame must NOT pay for it — every still cell in the 1,100-cell
    /// envelope goes through the same constructor.
    pub(crate) fn arm_inter_mvp(&mut self, env: crate::partition::InterMdEnv) {
        self.mvp_grid = alloc::vec![
            crate::intrabc_mvp::MvpMiEntry::default();
            (env.mi_rows * env.mi_stride) as usize
        ];
        self.mvp_env = Some(env);
    }

    /// The three fields C caches on `BlkStruct` for the entropy coder —
    /// `predmv`, `inter_mode_ctx` and `drl_ctx`/`drl_ctx_near` — derived
    /// HERE from the committed mode-info map instead of carried from MD.
    /// See [`crate::partition::InterDecision`] for why.
    ///
    /// Returns `None` when the frame has no MVP environment, which is
    /// exactly when no inter block can exist.
    pub(crate) fn inter_mvp_fields(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        d: &crate::partition::InterDecision,
    ) -> Option<(
        [svtav1_types::motion::Mv; 2],
        i16,
        crate::port_entropy_inter::modes::DrlBlock,
    )> {
        let env = self.mvp_env.as_ref()?;
        let (mi_row, mi_col) = ((y / 4) as i32, (x / 4) as i32);
        let bsize = crate::leaf_funnel::c_bsize_index(w, h);
        let ctx = crate::intrabc_mvp::derive_block_ctx(
            mi_row,
            mi_col,
            bsize,
            env.mi_rows,
            env.mi_cols,
            env.tile,
            env.sb_mi_size,
        );
        let grid = crate::intrabc_mvp::MvpGrid {
            entries: &self.mvp_grid,
            stride: env.mi_stride,
            base: mi_row * env.mi_stride + mi_col,
        };
        // C `svt_aom_generate_av1_mvp_table`'s `gm_mv` for an IDENTITY
        // global-motion model is the zero MV; the header refuses any other
        // model (`inter_hdr_arm::inter_signal`), so this is the model the
        // frame actually signalled rather than an assumption.
        let stack = crate::inter_mvp::setup_ref_mv_list(
            &grid,
            &ctx,
            &env.mvp_env(),
            crate::inter_mvp::av1_ref_frame_type(d.ref_frame),
            [svtav1_types::motion::Mv::ZERO; 2],
        );
        let pred = crate::inter_mvp::get_av1_mv_pred_drl(
            &stack,
            d.ref_frame[1] > 0,
            d.mode as u8,
            usize::from(d.drl_index),
            crate::inter_mvp::DrlMvPred::default(),
        );
        // C `mode_decision.c:3709-3728` — the two loops that fill
        // `drl_ctx` (0..2, NEWMV family) and `drl_ctx_near` (1..3, NEARMV
        // family), already ported as `port_md_winner::winner_signals`'s
        // `drl_contexts`.
        let (drl_ctx, drl_ctx_near) =
            crate::port_md_winner::drl_contexts_for(d.mode as u8, stack.count, &stack.stack);
        Some((
            pred.ref_mv,
            stack.mode_context,
            crate::port_entropy_inter::modes::DrlBlock {
                drl_ctx,
                drl_ctx_near,
                drl_index: d.drl_index,
            },
        ))
    }

    /// C `set_mi_row_col`'s `above_mbmi` / `left_mbmi` pair for a block at
    /// `(x, y)`, with the tile-boundary availability the rest of this type
    /// already models (`tile_top_px` / `tile_left_px`).
    ///
    /// The pointer and the availability flag are SEPARATE knobs in C, and
    /// [`crate::port_entropy_inter::Neighbors`] keeps them separate for the
    /// reason its doc gives; a caller that collapsed them would change the
    /// reference-count contexts.
    pub(crate) fn inter_neighbors(
        &self,
        x: usize,
        y: usize,
    ) -> crate::port_entropy_inter::Neighbors {
        let up = y > self.tile_top_px;
        let le = x > self.tile_left_px;
        crate::port_entropy_inter::Neighbors {
            above: if up {
                self.above_nmi.get(x / 4).copied()
            } else {
                None
            },
            left: if le {
                self.left_nmi.get(y / 4).copied()
            } else {
                None
            },
            up_available: up,
            left_available: le,
        }
    }

    /// Coefficient neighbor spans for a transform at (x, y) of w x h pixels,
    /// in 4x4 units, clipped to the frame like C svt_aom_get_txb_ctx.
    pub(crate) fn coeff_neighbors(&self, x: usize, y: usize, w: usize, h: usize) -> (&[u8], &[u8]) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = (w / 4).min(self.above_coeff.len().saturating_sub(x4));
        let h4 = (h / 4).min(self.left_coeff.len().saturating_sub(y4));
        (
            &self.above_coeff[x4..x4 + w4],
            &self.left_coeff[y4..y4 + h4],
        )
    }

    /// Record a coded transform block's `(dc_sign << 6) | cul_level` byte
    /// over its 4x4 span (C: neighbor array unit write after
    /// av1_write_coeffs_txb_1d).
    pub(crate) fn record_coeff(&mut self, x: usize, y: usize, w: usize, h: usize, val: u8) {
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_coeff.len()) {
            self.above_coeff[i] = val;
        }
        for i in y4..(y4 + h / 4).min(self.left_coeff.len()) {
            self.left_coeff[i] = val;
        }
    }

    /// Chroma-plane coefficient neighbor spans for a transform at chroma
    /// coords (cx, cy) of cw x ch chroma pixels, in chroma 4x4 units,
    /// clipped to the plane like the luma variant. `uv`: 0 = U, 1 = V.
    pub(crate) fn coeff_neighbors_uv(
        &self,
        uv: usize,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) -> (&[u8], &[u8]) {
        let x4 = cx / 4;
        let y4 = cy / 4;
        let w4 = (cw / 4).min(self.above_coeff_uv[uv].len().saturating_sub(x4));
        let h4 = (ch / 4).min(self.left_coeff_uv[uv].len().saturating_sub(y4));
        (
            &self.above_coeff_uv[uv][x4..x4 + w4],
            &self.left_coeff_uv[uv][y4..y4 + h4],
        )
    }

    /// Record a chroma transform block's neighbor byte over its chroma
    /// 4x4 span (per-plane, like the decoder's per-plane entropy contexts).
    pub(crate) fn record_coeff_uv(
        &mut self,
        uv: usize,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
        val: u8,
    ) {
        let x4 = cx / 4;
        let y4 = cy / 4;
        for i in x4..(x4 + cw / 4).min(self.above_coeff_uv[uv].len()) {
            self.above_coeff_uv[uv][i] = val;
        }
        for i in y4..(y4 + ch / 4).min(self.left_coeff_uv[uv].len()) {
            self.left_coeff_uv[uv][i] = val;
        }
    }

    /// Reset left context at the start of each SB row.
    /// In rav1d, `t.l` is reset per tile row (= SB row for single-tile).
    pub(crate) fn reset_left_for_sb_row(&mut self) {
        self.left_partition.fill(0);
    }

    /// Convert block width to our bsl (block size level).
    ///
    /// Task #91: the `_ => 3` catch-all used to fold 128 into the 64 level,
    /// which capped `partition_ctx` at ctx 15 and made the ctx 16..19 rows
    /// — the ONLY rows whose alphabet is the 8-symbol 128 set (C
    /// `svt_aom_partition_cdf_length`, entropy_coding.c:922) — unreachable
    /// dead code. A 128-wide node would have coded its partition symbol
    /// against the 64x64 CDF row with a 10-symbol alphabet: wrong
    /// probabilities AND wrong alphabet length. Byte-neutral at SB64
    /// (no node is ever 128 wide there).
    fn bsl(width: usize) -> usize {
        match width {
            w if w <= 8 => 0,
            w if w <= 16 => 1,
            w if w <= 32 => 2,
            w if w <= 64 => 3,
            _ => 4,
        }
    }

    /// Convert our bsl to rav1d BlockLevel.
    /// bsl=0 (8x8) → bl=4, bsl=1 (16x16) → bl=3, bsl=2 (32x32) → bl=2,
    /// bsl=3 (64x64) → bl=1, bsl=4 (128x128) → bl=0 (BL_128X128).
    fn bsl_to_block_level(bsl: usize) -> usize {
        4 - bsl
    }

    /// Compute partition context (sub, 0-3) from tracked above/left values.
    /// Uses the same bit-extraction logic as rav1d's `get_partition_ctx`.
    fn partition_sub(&self, x: usize, y: usize, bsl: usize) -> usize {
        let xb8 = x / 8;
        let yb8 = y / 8;
        let above_val = if xb8 < self.above_partition.len() {
            self.above_partition[xb8]
        } else {
            0
        };
        let left_val = if yb8 < self.left_partition.len() {
            self.left_partition[yb8]
        } else {
            0
        };
        // Extract bit at position bsl (matching rav1d's (4 - bl) = bsl)
        let above_bit = ((above_val >> bsl) & 1) as usize;
        let left_bit = ((left_val >> bsl) & 1) as usize;
        above_bit + 2 * left_bit
    }

    /// Get the partition context (ctx, nsymbs) for a block at (x, y) with given width.
    pub(crate) fn partition_ctx(&self, x: usize, y: usize, width: usize) -> (usize, usize) {
        let bsl = Self::bsl(width);
        let sub = self.partition_sub(x, y, bsl);
        let ctx = bsl * 4 + sub;
        // C `svt_aom_partition_cdf_length` (entropy_coding.c:922-930):
        // 4 at 8x8 (ctx 0..3 — only NONE/H/V/SPLIT fit), 8 at 128x128
        // (ctx 16..19 — EXT minus the geometrically impossible H4/V4),
        // 10 everywhere between. Cross-checked against
        // `sb128_geom::partition_cdf_length`, which is keyed on the square
        // size rather than the ctx; the two must agree.
        let nsymbs = match ctx {
            0..=3 => 4,
            4..=15 => 10,
            _ => 8,
        };
        (
            ctx.min(crate::entropy::context::PARTITION_CONTEXTS - 1),
            nsymbs,
        )
    }

    /// Update partition context after encoding a non-SPLIT partition.
    /// For SPLIT, the children update the context — don't call this for SPLIT.
    /// MD leaf commit: C `mode_decision_update_neighbor_arrays` writes
    /// `partition_context_lookup[bsize]` over the block span
    /// (product_coding_loop.c:179-192). For RECT leaves the above byte is
    /// the WIDTH's NONE row and the left byte the HEIGHT's — i.e. the
    /// per-dimension levels, not max(w, h) for both.
    pub(crate) fn update_partition_ctx_leaf(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) {
        // C partition_context_lookup[bsize].above/.left — a pure function
        // of the corresponding DIMENSION (the AL_PART_CTX NONE columns
        // extended by the 4px value 0x1f). Sub-8 dims write the covering
        // 8x8 cell (both siblings write the same byte, matching C's
        // 4x4-granular arrays on readback).
        fn dim_byte(dim: usize) -> u8 {
            match dim {
                4 => 0x1f,
                8 => 0x1e,
                16 => 0x1c,
                32 => 0x18,
                64 => 0x10,
                _ => 0x00, // 128
            }
        }
        let above_val = dim_byte(width);
        let left_val = dim_byte(height);
        let xb8 = x / 8;
        let yb8 = y / 8;
        for i in xb8..(xb8 + (width / 8).max(1)).min(self.above_partition.len()) {
            self.above_partition[i] = above_val;
        }
        for i in yb8..(yb8 + (height / 8).max(1)).min(self.left_partition.len()) {
            self.left_partition[i] = left_val;
        }
    }

    pub(crate) fn update_partition_ctx(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        partition_type: crate::partition::PartitionType,
    ) {
        let bsl = Self::bsl(width.max(height));
        let bl = Self::bsl_to_block_level(bsl);
        let pt = partition_type as usize;
        if pt >= 10 || bl >= 5 {
            return;
        }
        let above_val = AL_PART_CTX[0][bl][pt];
        let left_val = AL_PART_CTX[1][bl][pt];
        // 0xff means invalid (SPLIT) — don't update
        if above_val == 0xff || left_val == 0xff {
            return;
        }
        let hsz_8 = width / 8; // half-size in 8x8 units = width/8
        let xb8 = x / 8;
        let yb8 = y / 8;
        for i in xb8..(xb8 + hsz_8).min(self.above_partition.len()) {
            self.above_partition[i] = above_val;
        }
        let vsz_8 = height / 8;
        for i in yb8..(yb8 + vsz_8).min(self.left_partition.len()) {
            self.left_partition[i] = left_val;
        }
    }

    /// Record a block's mode and skip status in the context maps.
    pub(crate) fn record_block(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        mode: u8,
        uv_mode: u8,
        skip: bool,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = w / 4;
        let h4 = h / 4;
        // Fill above row with this block's mode
        for i in x4..(x4 + w4).min(self.above_mode.len()) {
            self.above_mode[i] = mode;
            self.above_uv_mode[i] = uv_mode;
            self.above_skip[i] = skip;
        }
        // Fill left column with this block's mode
        for i in y4..(y4 + h4).min(self.left_mode.len()) {
            self.left_mode[i] = mode;
            self.left_uv_mode[i] = uv_mode;
            self.left_skip[i] = skip;
        }
    }

    /// Stamp one block's span of the inter mi grid — C's
    /// `svt_aom_update_mi_map` (product_coding_loop.c:670) restricted to the
    /// fields [`crate::port_entropy_inter`]'s context functions read.
    pub(crate) fn record_inter_mi(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        mi: crate::port_entropy_inter::NeighborMi,
        mv: [svtav1_types::motion::Mv; 2],
        partition_type: u8,
    ) {
        let (x4, y4) = (x / 4, y / 4);
        for i in x4..(x4 + w / 4).min(self.above_nmi.len()) {
            self.above_nmi[i] = mi;
        }
        for i in y4..(y4 + h / 4).min(self.left_nmi.len()) {
            self.left_nmi[i] = mi;
        }
        // ...and the FULL grid the MVP walk scans, when this frame has one.
        // C stamps both from the same `svt_aom_update_mi_map` call, so they
        // can never disagree here either.
        if let Some(env) = self.mvp_env.as_ref() {
            let e = crate::intrabc_mvp::MvpMiEntry {
                bsize: mi.bsize,
                mode: mi.mode,
                use_intrabc: mi.use_intrabc,
                ref_frame: mi.ref_frame,
                mv,
                partition: partition_type,
                interp_filters: mi.interp_filters,
            };
            let stride = env.mi_stride as usize;
            for r in y4..(y4 + h / 4).min(env.mi_rows as usize) {
                for c in x4..(x4 + w / 4).min(env.mi_cols as usize) {
                    self.mvp_grid[r * stride + c] = e;
                }
            }
        }
    }

    /// Record a block's luma palette (C's `mbmi->palette_mode_info`, read
    /// back by `svt_aom_get_palette_mode_ctx` / `svt_get_palette_cache_y`).
    /// `colors` is `None` for a non-palette block (palette_size 0 — every
    /// current leaf, until #71 chunk 3/4 injection wires a winning
    /// candidate through `BlockDecision.palette`). Stamped over the
    /// block's full mi span, exactly like [`Self::record_block`].
    pub(crate) fn record_palette(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        colors: Option<&[u16]>,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = w / 4;
        let h4 = h / 4;
        let n = colors.map_or(0, <[u16]>::len) as u8;
        debug_assert!((n as usize) <= svtav1_types::prediction::PALETTE_MAX_SIZE);
        let mut buf = [0u16; svtav1_types::prediction::PALETTE_MAX_SIZE];
        if let Some(c) = colors {
            buf[..c.len()].copy_from_slice(c);
        }
        for i in x4..(x4 + w4).min(self.above_palette.len()) {
            self.above_palette[i] = n;
            self.above_palette_colors[i] = buf;
        }
        for i in y4..(y4 + h4).min(self.left_palette.len()) {
            self.left_palette[i] = n;
            self.left_palette_colors[i] = buf;
        }
    }

    /// C `svt_aom_get_palette_mode_ctx` (entropy_coding.c:4240-4251): count
    /// of above/left neighbor blocks (when available — frame-edge gated,
    /// like every other above/left context lookup here) whose luma
    /// `palette_size > 0`. NO SB-row drop (unlike [`palette_cache`], which
    /// has C's `svt_get_palette_cache_y` above-row exception) — this reads
    /// the immediate neighbor exactly like `above_mode_ctx`/`left_mode_ctx`.
    pub(crate) fn palette_neighbor_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = y > 0 && x4 < self.above_palette.len() && self.above_palette[x4] > 0;
        let left =
            x > self.tile_left_px && y4 < self.left_palette.len() && self.left_palette[y4] > 0;
        usize::from(above) + usize::from(left)
    }

    /// C `get_filt_type(xd, plane = 0)` (enc_intra_prediction.c:20): 1
    /// when the above OR left neighbour block's Y mode is smooth
    /// (SMOOTH/SMOOTH_V/SMOOTH_H), else 0. Neighbours are the blocks at
    /// (mi_row - 1, mi_col) / (mi_row, mi_col - 1); unavailable -> 0.
    pub(crate) fn filt_type_y(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let ab = y > 0 && smooth(self.above_mode[x / 4]);
        let le = x > self.tile_left_px && smooth(self.left_mode[y / 4]);
        i32::from(ab || le)
    }

    /// C `get_filt_type(xd, plane > 0)`: same over the neighbours' UV
    /// modes (chroma_above/left_mbmi; min-8x8 blocks make the +1-mi
    /// group offsets land in the same neighbour block).
    pub(crate) fn filt_type_uv(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let ab = y > 0 && smooth(self.above_uv_mode[x / 4]);
        let le = x > self.tile_left_px && smooth(self.left_uv_mode[y / 4]);
        i32::from(ab || le)
    }

    /// Get the above mode context at position (x, y) in pixel coordinates.
    pub(crate) fn above_mode_ctx(&self, x: usize) -> usize {
        let x4 = x / 4;
        let mode = if x4 < self.above_mode.len() {
            self.above_mode[x4]
        } else {
            0
        };
        crate::entropy::context::intra_mode_context(mode)
    }

    /// Get the left mode context at position (x, y) in pixel coordinates.
    pub(crate) fn left_mode_ctx(&self, y: usize) -> usize {
        let y4 = y / 4;
        let mode = if y4 < self.left_mode.len() {
            self.left_mode[y4]
        } else {
            0
        };
        crate::entropy::context::intra_mode_context(mode)
    }

    /// Get the skip context at position (x, y).
    pub(crate) fn skip_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = x4 < self.above_skip.len() && self.above_skip[x4];
        let left = y4 < self.left_skip.len() && self.left_skip[y4];
        crate::entropy::context::get_skip_context(above, left)
    }

    /// tx_size context for a block at (x, y) of w x h pixels.
    ///
    /// C `get_tx_size_context(xd)` (entropy_coding.c:4642-4676):
    /// `above = above_txfm_context[0] >= tx_size_wide[max_tx_size]`,
    /// `left = left_txfm_context[0] >= tx_size_high[max_tx_size]`, each
    /// gated on availability; both available → sum, one → that one,
    /// none → 0. For every bsize <= 64x64 the largest TX has the block's
    /// own dims, so max_tx_wide/high == w/h. The C is_inter neighbor
    /// override (use the neighbor's BLOCK dims instead of its TX dims)
    /// can't fire here: tx_depth is only coded on key frames, where every
    /// neighbor is intra.
    pub(crate) fn tx_size_ctx(&self, x: usize, y: usize, w: usize, h: usize) -> usize {
        // Availability == C xd->up_available / left_available
        // (set_mi_row_col: mi_row/col > TILE start — task #86: `above_txfm`
        // is allocated frame-wide but reset fresh per tile, so a
        // never-written cell already reads 0 (`0 >= w` is false for any
        // w > 0), making `has_above` numerically inert at a tile's own
        // top row EITHER way; gating on `tile_top_px` here anyway keeps
        // this consistent with `extract_neighbors`/`PartitionSearchConfig
        // ::tile_top_px` rather than relying on that coincidence).
        let has_above = y > self.tile_top_px;
        let has_left = x > self.tile_left_px;
        // IBC chunk 9 (aom-rs Root 6): C substitutes an is_inter
        // neighbour's BLOCK dims for its TXFM-context byte
        // (get_tx_size_context, entropy_coding.c:4626-4637). IntraBC
        // blocks are the only inter-classified neighbours on this port;
        // `above_inter_bw`/`left_inter_bh` hold their block dims (0 =
        // intra neighbour, the plain txfm-ctx compare).
        let above = if self.above_inter_bw[x / 4] != 0 {
            (self.above_inter_bw[x / 4] as usize >= w) as usize
        } else {
            (self.above_txfm[x / 4] as usize >= w) as usize
        };
        let left = if self.left_inter_bh[y / 4] != 0 {
            (self.left_inter_bh[y / 4] as usize >= h) as usize
        } else {
            (self.left_txfm[y / 4] as usize >= h) as usize
        };
        match (has_above, has_left) {
            (true, true) => above + left,
            (true, false) => above,
            (false, true) => left,
            (false, false) => 0,
        }
    }

    /// IBC chunk 9: stamp the inter-neighbour dims state over a block's
    /// footprint — the coded BLOCK dims for an IntraBC block (u8-safe:
    /// block dims <= 128), 0 for every intra block.
    pub(crate) fn record_inter_dims(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        use_intrabc: bool,
    ) {
        let (bw, bh) = if use_intrabc {
            (w as u8, h as u8)
        } else {
            (0, 0)
        };
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_inter_bw.len()) {
            self.above_inter_bw[i] = bw;
        }
        for i in y4..(y4 + h / 4).min(self.left_inter_bh.len()) {
            self.left_inter_bh[i] = bh;
        }
    }

    /// The frame height in pixels this context spans (the C
    /// `mb_to_bottom_edge` clip base for the var-tx walk).
    pub(crate) fn frame_h_px(&self) -> usize {
        self.left_txfm.len() * 4
    }

    /// Update the TXFM context arrays after coding a block.
    ///
    /// C `set_txfm_ctxs(tx_size, n8_w, n8_h, skip && is_inter, xd)`
    /// (entropy_coding.c:4614-4625): above cells over the block's mi
    /// columns take tx_size_wide, left cells over its mi rows take
    /// tx_size_high. Runs for EVERY block (both branches of
    /// av1_code_tx_size), signaling or not. Our blocks always use the
    /// full-block TX and the skip||inter override stores block dims —
    /// identical values here either way.
    /// C `set_txfm_ctxs(tx_size, n8_w, n8_h, 0, xd)` with an explicit
    /// CHOSEN tx size — above cells take tx_size_wide, left cells
    /// tx_size_high, over the block's mi span (entropy_coding.c:4614;
    /// MD mirror mode_decision_update_neighbor_arrays,
    /// product_coding_loop.c:246-256).
    pub(crate) fn record_txfm_dims(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        tx_w: usize,
        tx_h: usize,
    ) {
        let x4 = x / 4;
        let y4 = y / 4;
        for i in x4..(x4 + w / 4).min(self.above_txfm.len()) {
            self.above_txfm[i] = tx_w as u8;
        }
        for i in y4..(y4 + h / 4).min(self.left_txfm.len()) {
            self.left_txfm[i] = tx_h as u8;
        }
    }

    /// The block's above TXFM-context span (tx dims in px per 4x4 unit) —
    /// the seed of the inter var-tx walk's local copy (IBC chunk 7; C
    /// `svt_aom_get_tx_size_bits` memcpy, rd_cost.c:1790-1795).
    pub(crate) fn txfm_above_span(&self, x: usize, w: usize) -> &[u8] {
        let x4 = x / 4;
        &self.above_txfm[x4..(x4 + w / 4).min(self.above_txfm.len())]
    }

    /// The block's left TXFM-context span (IBC chunk 7).
    pub(crate) fn txfm_left_span(&self, y: usize, h: usize) -> &[u8] {
        let y4 = y / 4;
        &self.left_txfm[y4..(y4 + h / 4).min(self.left_txfm.len())]
    }

    /// The block's above coefficient-context byte span (4x4 units),
    /// clipped to the frame — the seed of the MD TX-local overlay
    /// (C tx_reset_neighbor_arrays copies the committed arrays).
    pub(crate) fn above_coeff_span(&self, x: usize, w: usize) -> &[u8] {
        let x4 = x / 4;
        &self.above_coeff[x4..(x4 + w / 4).min(self.above_coeff.len())]
    }

    /// The block's left coefficient-context byte span (4x4 units).
    pub(crate) fn left_coeff_span(&self, y: usize, h: usize) -> &[u8] {
        let y4 = y / 4;
        &self.left_coeff[y4..(y4 + h / 4).min(self.left_coeff.len())]
    }
}

/// C `av1_use_angle_delta(bsize)` (reconintra.h:59): `bsize >= BLOCK_8X8` in
/// enum order — true for every block size except BLOCK_4X4, BLOCK_4X8 and
/// BLOCK_8X4 (the 4:1 rects 4x16/16x4 come AFTER BLOCK_128X128 in the enum).
fn use_angle_delta(width: u16, height: u16) -> bool {
    !matches!((width, height), (4, 4) | (4, 8) | (8, 4))
}

/// Write one chroma plane's transform block (`uv`: 0 = U, 1 = V) with the
/// C-exact coefficient writer, using that plane's own neighbor context
/// arrays but the SHARED plane_type=1 CDF tables (AV1 PLANE_TYPES = 2:
/// U and V share tables, contexts stay per-plane — libaom keeps
/// pd->above/left_entropy_context per plane while indexing every CDF with
/// `plane_type = plane > 0`).
///
/// The chroma tx type is NOT signaled: the decoder derives it from UVMode
/// via Mode_To_Txfm (spec compute_tx_type, plane > 0 intra) —
/// UV_DC_PRED -> DCT_DCT, which also selects the default scan. The writer
/// only emits tx_type symbols for plane_type == 0.
#[allow(clippy::too_many_arguments)]
fn write_chroma_txb(
    writer: &mut crate::entropy::writer::AomWriter,
    coeff_fc: &mut crate::entropy::coeff_c::CoeffFc,
    ectx: &mut EntropyCtx,
    uv: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    qcoeffs: &[i32],
    base_q_idx: u8,
    uv_tx_type: usize,
) {
    use crate::entropy::coeff_c;
    let tx_size = coeff_c::tx_size_from_dims(cw, ch);
    let (above, left) = ectx.coeff_neighbors_uv(uv, cx, cy, cw, ch);
    // plane != 0: txb_skip_ctx = (above nonzero) + (left nonzero) + 7,
    // because the chroma plane bsize equals the (full-block) chroma tx
    // size here — never "chroma larger" (C svt_aom_get_txb_ctx else-branch;
    // libaom get_txb_ctx num_pels comparison). The 4th arg is the luma-only
    // fast-path flag, unused for plane != 0.
    let (txb_skip_ctx, dc_sign_ctx) = coeff_c::get_txb_ctx(1, above, left, true, false);
    // eob relative to the scan of the DERIVED chroma tx type (the decoder
    // computes it from UVMode via Mode_To_Txfm — spec compute_tx_type,
    // plane > 0 intra: UV_DC -> DCT_DCT, UV_V -> ADST_DCT,
    // UV_H -> DCT_ADST, UV_SMOOTH -> ADST_ADST; DCT-only above 16x16).
    let scan = crate::entropy::scan_tables::scan(
        tx_size,
        crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tx_type] as usize,
    );
    let mut eob = 0i32;
    for (i, &pos) in scan.iter().enumerate() {
        if qcoeffs[pos as usize] != 0 {
            eob = i as i32 + 1;
        }
    }
    let cul_level = coeff_c::write_coeffs_txb_1d(
        coeff_fc,
        writer,
        tx_size,
        uv_tx_type,
        1, // plane_type: U and V both use the chroma tables
        txb_skip_ctx,
        dc_sign_ctx,
        qcoeffs,
        eob,
        0, // intra_dir: unused for plane_type != 0 (no tx_type signaling)
        base_q_idx,
        false,
        false, // is_inter: dead for plane_type != 0 (no tx_type symbol)
    );
    ectx.record_coeff_uv(uv, cx, cy, cw, ch, cul_level as u8);
}

/// Encode block syntax (skip, mode, coefficients) WITHOUT a partition symbol.
///
/// This is the core block encoding used by both PARTITION_NONE leaves and
/// HORZ/VERT children. In AV1, HORZ/VERT children are always leaf blocks
/// that the decoder reads directly — no partition symbol is expected for them.
/// IBC chunk 9: bridge the inter var-tx tx_size writer with the
/// EntropyCtx txfm spans (copied out to end the immutable borrow before
/// the CDF-adapting write).
#[allow(clippy::too_many_arguments)]
fn writer_tx_size_vartx_bridge(
    writer: &mut crate::entropy::writer::AomWriter,
    frame_ctx: &mut crate::entropy::context::FrameContext,
    ectx: &EntropyCtx,
    block_x: usize,
    block_y: usize,
    w: usize,
    h: usize,
    depth: u8,
) {
    let above: alloc::vec::Vec<u8> = ectx.txfm_above_span(block_x, w).to_vec();
    let left: alloc::vec::Vec<u8> = ectx.txfm_left_span(block_y, h).to_vec();
    crate::vartx::write_tx_size_vartx(
        writer,
        frame_ctx,
        &above,
        &left,
        w,
        h,
        depth,
        block_y,
        ectx.frame_h_px(),
    );
}

#[allow(clippy::too_many_arguments)]
fn encode_block_syntax(
    decision: &crate::partition::BlockDecision,
    writer: &mut crate::entropy::writer::AomWriter,
    frame_ctx: &mut crate::entropy::context::FrameContext,
    coeff_fc: &mut crate::entropy::coeff_c::CoeffFc,
    base_q_idx: u8,
    ectx: &mut EntropyCtx,
    is_key: bool,
    block_x: usize,
    block_y: usize,
    chroma: &mut Option<ChromaPass<'_>>,
    geom: &mut crate::deblock::DeblockGeom,
) {
    // Diagnostic (SVTAV1_TRACEMARK=1): a block-boundary marker written INTO
    // the symtrace op stream on stderr, so a first-diverging-op index maps
    // straight onto a coded block. Opt-in — off for every existing caller —
    // because that stream is parsed as data by identity_diff.py, which
    // ignores unknown `#` lines (as does any C-side counterpart marker).
    #[cfg(feature = "std")]
    if crate::dbgenv::tracemark() {
        std::eprintln!(
            "# BLK mi=({},{}) bsize={} ibc={}",
            block_y / 4,
            block_x / 4,
            crate::entropy::context::block_size_index(
                decision.width as usize,
                decision.height as usize
            ),
            u8::from(decision.use_intrabc),
        );
    }
    // Diagnostic (SVTAV1_PACKTREE=<path>): one line per coded leaf — the
    // port's FINAL tree, file-only (no stderr noise; token-frugal drills).
    // `off=` is the entropy writer's byte position at block entry, which maps
    // a byte-level OBU divergence (`cmp -l`) straight onto a coded block; the
    // companion `PDV` line carries the IntraBC DV + its predictor + the tx
    // type (all invisible in the PTREE row, and the first things to rule out
    // when an IBC block diverges).
    // tools/tree_diff.py joins it against the C-side CTREE dump (the
    // svt_aom_update_mi_map --wrap, valid at every preset) and prints only
    // the flips. Field domains mirror the C wrap: C BlockSize enum id via
    // block_size_index; fi 5 = none; uv 13 = CFL; skip is derived on the
    // diff side from yeob/ueob/veob (C dumps the all-plane skip bit).
    #[cfg(feature = "std")]
    if let Some(path) = crate::dbgenv::packtree() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let (ueob, veob) = decision
                .chroma_dec
                .as_ref()
                .map(|c| (c.2, c.3))
                .unwrap_or((0, 0));
            let _ = writeln!(
                f,
                "PTREE mi=({},{}) off={} bsize={} part={} mode={} uv={} fi={} ady={} aduv={} txd={} yeob={} ueob={} veob={} cflidx={} cflsgn={} pal={} ibc={}",
                block_y / 4,
                block_x / 4,
                writer.bytes_written(),
                crate::entropy::context::block_size_index(
                    decision.width as usize,
                    decision.height as usize
                ),
                decision.partition_type as u8,
                decision.intra_mode,
                decision.uv_mode,
                decision.filter_intra_mode,
                decision.angle_delta,
                decision.uv_angle_delta,
                decision.tx_depth,
                decision.eob,
                ueob,
                veob,
                decision.cfl_alpha_idx,
                decision.cfl_alpha_signs,
                decision.palette.as_ref().map(|p| p.0.len()).unwrap_or(0),
                // IntraBC is invisible in this dump otherwise, which made it
                // impossible to assert that a screen-content gate cell actually
                // exercised IBC rather than merely enabling it.
                u8::from(decision.use_intrabc),
            );
            let _ = writeln!(
                f,
                "PDV mi=({},{}) dvr={} dvc={} dvrefr={} dvrefc={} txt={} inter={} mvr={} mvc={} rf={} mode={}",
                block_y / 4,
                block_x / 4,
                decision.dv.y,
                decision.dv.x,
                decision.dv_ref.y,
                decision.dv_ref.x,
                decision.tx_type,
                // The INTER half of the committed decision, so a PTREE dump
                // can be joined against C's `SVT_CINTER_OUT` the way the
                // intra half already joins against `SVT_CTREE_OUT`. Without
                // it an inter leaf is indistinguishable from a DC intra one
                // in this dump (`mode` is `intra_mode`, which an inter block
                // leaves at 0) — which is exactly how a decode failure on an
                // inter frame had no per-block evidence behind it.
                u8::from(decision.is_inter),
                decision.inter.as_deref().map_or(0, |b| b.mv[0].y),
                decision.inter.as_deref().map_or(0, |b| b.mv[0].x),
                decision.inter.as_deref().map_or(0, |b| b.ref_frame[0]),
                decision.inter.as_deref().map_or(0, |b| b.mode as u8),
            );
        }
    }
    // Diagnostic (SVTAV1_BLKMARK=1): the same identity, but on STDERR and
    // therefore INTERLEAVED with the `symtrace` op log — which is what turns
    // an "op index N diverges" verdict into "block mi=(r,c) diverges". The
    // file dump above cannot do that: it is a separate stream with no
    // ordering relation to the op trace. Emitted at the top of
    // `encode_block_syntax`, i.e. after the block's partition symbol and
    // before every one of its mode/coeff symbols.
    #[cfg(feature = "std")]
    if crate::dbgenv::blkmark() {
        std::eprintln!(
            "W BLKMARK mi=({},{}) {}x{} mode={} uv={} pal={} ibc={}",
            block_y / 4,
            block_x / 4,
            decision.width,
            decision.height,
            decision.intra_mode,
            decision.uv_mode,
            decision.palette.as_ref().map(|p| p.0.len()).unwrap_or(0),
            u8::from(decision.use_intrabc),
        );
    }
    // Diagnostic (SVTAV1_PACKTREE_COEFF): the block's PACKED nonzero
    // luma+chroma levels as (raster_idx:level) pairs — the port counterpart
    // of the C QLEV/CCOEF wrap dumps (final coded levels). Two modes:
    //   * value contains a comma ("mi_row,mi_col") → pin ONE block, stderr.
    //   * value is a PATH (no comma) → append EVERY coded leaf to that file
    //     (coding order), for a whole-frame join vs the C SVT_QLEVELS_OUT
    //     dump. Backward-compatible: existing "r,c" callers are unchanged.
    #[cfg(feature = "std")]
    if let Some(xy) = crate::dbgenv::packtree_coeff() {
        let is_pin = xy.contains(',');
        let want: alloc::vec::Vec<usize> = xy
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let pinned = is_pin && want.len() == 2 && want[0] == block_y / 4 && want[1] == block_x / 4;
        if pinned || !is_pin {
            let fmt_nz = |q: &[i32], cap: usize| -> alloc::string::String {
                let mut s = alloc::string::String::new();
                let mut n = 0;
                for (i, &v) in q.iter().enumerate() {
                    if v != 0 && n < cap {
                        if n > 0 {
                            s.push(',');
                        }
                        s.push_str(&alloc::format!("{i}:{v}"));
                        n += 1;
                    }
                }
                s
            };
            let (unz, vnz) = decision
                .chroma_dec
                .as_ref()
                .map(|c| (fmt_nz(&c.0, 48), fmt_nz(&c.1, 48)))
                .unwrap_or_default();
            let line = alloc::format!(
                "PCOEF mi=({},{}) yeob={} txt={} ynz=[{}] unz=[{}] vnz=[{}]",
                block_y / 4,
                block_x / 4,
                decision.eob,
                decision.tx_type,
                fmt_nz(&decision.qcoeffs, 48),
                unz,
                vnz
            );
            if is_pin {
                eprintln!("{line}");
            } else {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(xy)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }
    // Diagnostic (SVTAV1_PART_DUMP): every coded leaf's geometry + skip, to
    // diff the partition tree against the C entropy coder. No output change.
    #[cfg(feature = "std")]
    if crate::dbgenv::part_dump() {
        eprintln!(
            "RSPART x{block_x} y{block_y} {}x{} skip={} ymode={} uvmode={} txd={}",
            decision.width,
            decision.height,
            decision.eob == 0,
            decision.intra_mode,
            decision.uv_mode,
            decision.tx_depth
        );
    }
    // 4:2:0: encode this block's chroma pair FIRST (prediction reads the
    // live chroma recon written by previous blocks in coding order). The
    // min-8x8 luma policy guarantees the chroma block is exactly
    // (w/2, h/2) >= 4x4 and every block is a chroma reference.
    // C `is_chroma_reference` (common_utils.h:315): sub-8 blocks carry
    // chroma only at odd mi in the sub-8 dimension; the chroma unit is
    // then the PAIR block (bsize_uv dims max(dim,8)/2 at the ROUND_UV
    // origin). Non-ref blocks code NO chroma txbs and leave the chroma
    // entropy contexts untouched (spec residual(): the chroma loop is
    // skipped entirely).
    let blk_has_uv = {
        let bw_mi = decision.width as usize / 4;
        let bh_mi = decision.height as usize / 4;
        ((block_y / 4) % 2 == 1 || bh_mi.is_multiple_of(2))
            && ((block_x / 4) % 2 == 1 || bw_mi.is_multiple_of(2))
    };
    // Task #86: chroma-plane tile-row origin (exact halving — see
    // encode_chroma_block_dc's doc comment). Copied out of `ectx` before
    // the closure below so the closure doesn't need to borrow `ectx` too.
    let chroma_tile_top = ectx.tile_top_px / 2;
    let chroma_tile_left = ectx.tile_left_px / 2; // task #96, same halving rule
    // The chroma plane's ALIGNED extent, for the reference-sample clamp
    // (`extract_neighbors_tiled`'s `plane_w`/`plane_h`). Same copy-out-of-ectx
    // reason as the tile origins above.
    let chroma_plane_w = ectx.aligned_w_px / 2;
    let chroma_plane_h = ectx.aligned_h_px / 2;
    let chroma_blocks = chroma.as_mut().filter(|_| blk_has_uv).map(|cp| {
        let cw = (decision.width as usize).max(8) / 2;
        let ch = (decision.height as usize).max(8) / 2;
        let cx = ((block_x >> 3) << 3) / 2
            + if decision.width >= 8 {
                (block_x % 8) / 2
            } else {
                0
            };
        let cy = ((block_y >> 3) << 3) / 2
            + if decision.height >= 8 {
                (block_y % 8) / 2
            } else {
                0
            };
        if let Some((u_q, v_q, u_eob, v_eob, u_rec, v_rec)) = decision.chroma_dec.as_ref() {
            // Funnel-decided chroma (M6 leaf funnel): the decision phase
            // already predicted (per the decided uv_mode), quantized and
            // reconstructed both planes with the C MDS3 path — copy its
            // recon into the walk planes so the plane evolution is
            // byte-identical, and code the decided coefficients.
            for r in 0..ch {
                let dst = (cy + r) * cp.stride + cx;
                cp.u_recon[dst..dst + cw].copy_from_slice(&u_rec[r * cw..(r + 1) * cw]);
                cp.v_recon[dst..dst + cw].copy_from_slice(&v_rec[r * cw..(r + 1) * cw]);
            }
            (u_q.clone(), *u_eob, v_q.clone(), *v_eob)
        } else {
            let (u_q, u_eob) = crate::partition::encode_chroma_block_dc(
                cp.u_src,
                cp.u_recon,
                cp.stride,
                cx,
                cy,
                cw,
                ch,
                cp.qindex_u,
                cp.c_quant,
                cp.qm_u,
                chroma_tile_top,
                chroma_tile_left,
                chroma_plane_w,
                chroma_plane_h,
            );
            let (v_q, v_eob) = crate::partition::encode_chroma_block_dc(
                cp.v_src,
                cp.v_recon,
                cp.stride,
                cx,
                cy,
                cw,
                ch,
                cp.qindex_v,
                cp.c_quant,
                cp.qm_v,
                chroma_tile_top,
                chroma_tile_left,
                chroma_plane_w,
                chroma_plane_h,
            );
            (u_q, u_eob, v_q, v_eob)
        }
    });

    // The block-level skip flag means ALL planes are zero (the decoder
    // reads no txbs at all for skip blocks and zeroes every plane's
    // entropy context — spec reset_block_context / libaom
    // av1_reset_entropy_context). Per-plane eob==0 inside a non-skip
    // block is carried by that plane's own txb_skip symbol instead.
    let skip = decision.eob == 0
        && chroma_blocks
            .as_ref()
            .is_none_or(|(_, u_eob, _, v_eob)| *u_eob == 0 && *v_eob == 0);
    let skip_ctx = ectx.skip_ctx(block_x, block_y);
    crate::entropy::context::write_skip(writer, frame_ctx, skip_ctx, skip);

    // cdef_idx (C write_cdef, entropy_coding.c:3986-4017; spec read_cdef):
    // at the FIRST NON-SKIP coded block of each 64x64 FILTER BLOCK,
    // `cdef_bits` raw literal bits carry that filter block's strength
    // index. Armed by the walk at SB start only when cdef_bits > 0
    // (aom_write_literal with 0 bits is a no-iteration loop).
    //
    // The filter block is 64x64 ALWAYS, so an SB128 superblock emits up to
    // FOUR literals — C's `cdef_transmitted[4]` latch, indexed
    // `!!(mi_col & 16) + 2 * !!(mi_row & 16)` (mi 16 == 64 px). Emitting
    // just one per SB (the pre-SB128 model) leaves the decoder expecting
    // literals the encoder never wrote: a CORRUPT tile, not merely a
    // mismatched one. At SB64 `index` is always 0 and this is bit-for-bit
    // the previous behaviour.
    if !skip && let Some(st) = ectx.cdef_sb.as_mut() {
        let index = if st.sb128 {
            ((block_x >> 6) & 1) + 2 * ((block_y >> 6) & 1)
        } else {
            0
        };
        if !st.transmitted[index] {
            st.transmitted[index] = true;
            writer.write_literal(u32::from(st.strengths[index]), u32::from(st.bits));
        }
    }

    // [SVT_HDR_MODE] per-SB delta-q (C entropy_coding.c:4997, spec 5.11.41
    // mode_info -> read_delta_qindex): only at the SB's upper-left block,
    // and only when (bsize != sb_size || !skip). sb_size is 64 here.
    if let Some((res, prev)) = ectx.delta_q_state {
        let super_block_upper_left = block_x.is_multiple_of(64) && block_y.is_multiple_of(64);
        let is_sb_sized = decision.width == 64 && decision.height == 64;
        if super_block_upper_left && (!is_sb_sized || !skip) {
            let cur = ectx.delta_q_sb_qindex;
            let reduced = (cur - prev) / i32::from(res);
            crate::entropy::mv_coding::write_delta_q_index(
                writer,
                &mut frame_ctx.delta_q_cdf,
                reduced,
            );
            ectx.delta_q_state = Some((res, cur));
        }
    }

    // use_intrabc flag (C write_modes_b -> write_intrabc_info,
    // entropy_coding.c:5021-5023 / :4405-4416, gated svt_aom_allow_intrabc;
    // spec intra_frame_mode_info): on an IBC frame the flag is coded for
    // EVERY block — the port codes use_intrabc = 0 until the DV search +
    // injection land (map chunks 5-9); the write adapts intrabc_cdf exactly
    // like C's aom_write_symbol, and the funnel chain sim shares this path
    // (the C MD-side twin: update_stats -> update_cdf(intrabc_cdf),
    // md_rate_estimation.c:854-855). Without the flag the FH's
    // allow_intrabc = 1 promises a symbol the tile lacks — an UNDECODABLE
    // stream, not merely a divergent one (aomdec outputs zero frames).
    // IBC chunk 9: the winner's real use_intrabc + DV. write_intrabc_info
    // codes the flag over intrabc_cdf (adapting) and, when set, the DV
    // diff vs dv_ref over ndvc at MV_SUBPEL_NONE (svt_av1_encode_dv).
    let use_intrabc = decision.use_intrabc;
    if is_key && ectx.allow_intrabc {
        crate::intrabc::write_intrabc_info(
            writer,
            &mut frame_ctx.intrabc_cdf,
            &mut frame_ctx.ndvc,
            use_intrabc,
            decision.dv,
            decision.dv_ref,
        );
    }

    // Mode syntax is ALWAYS coded — the skip flag only gates residuals
    // (AV1 intra_frame_mode_info reads y_mode regardless of skip).
    if !is_key {
        // C `write_is_inter` (entropy_coding.c:1147) takes
        // `svt_av1_get_intra_inter_context(xd)` — a FOUR-valued context off
        // the above/left neighbours' `is_inter_block`, not a constant.
        //
        // The port passed a hard-coded 0. That was inert while no inter
        // frame could reach the pack, and it is a DECODER DESYNC the moment
        // one does: the decoder computes the real context, reads a symbol
        // from a different CDF row, and every bit after it is misaligned.
        // FOUND by decoding the experimental 2-frame stream — `aomdec`
        // reported "Failed to decode tile data" on frame 1 and `dav1d`
        // "Invalid argument", with frame 0 decoding cleanly.
        //
        // `intra_inter_context` is already ported and tier-1 gated in
        // `port_entropy_inter`, and takes exactly the `Neighbors` the mi
        // grid now supplies.
        let ctx =
            crate::port_entropy_inter::intra_inter_context(&ectx.inter_neighbors(block_x, block_y));
        crate::entropy::context::write_intra_inter(writer, frame_ctx, ctx, decision.is_inter);
    }

    if use_intrabc {
        // C write_modes_b :5024-5089: y_mode + angle + uv mode-info +
        // palette + filter_intra are ALL suppressed for an IntraBC block
        // (each writer is nested under `use_intrabc == 0`).
    } else if decision.is_inter {
        // THE PRE-CAMPAIGN HOMEGROWN INTER ARM. It is reachable ONLY under
        // `SVTAV1_INTER_EXPERIMENTAL` (the public entry point refuses inter
        // frames above), and it is NOT a bitstream: measured 2026-09-01 on
        // `gradient 64x64 q40 p6 frames=2`, it commits 24 inter leaves and
        // for each one writes an MV and NOTHING ELSE. Four defects, named so
        // the chunk that replaces this line with
        // `port_entropy_inter::write_inter_mode_info` has the list:
        //
        // 1. **A FRESH `NmvContext` PER BLOCK.** `write_mv` builds
        //    `NmvContext::default()` on every call, so no MV symbol adapts
        //    the frame's context and no MV after the first is coded against
        //    the probabilities a decoder holds. C's `av1_encode_mv` takes the
        //    FRAME's single adapting `nmvc`. This is a decoder desync, not a
        //    size difference.
        // 2. **No `write_ref_frames`, no inter mode symbol, no DRL, no
        //    interp filter.** A decoder reading `is_inter = 1` reads all of
        //    those before the MV, so it consumes the MV's bits as a reference
        //    index.
        // 3. **`allow_hp = true` is hard-coded**, while this frame's header
        //    writes `allow_high_precision_mv = 0` (measured, §1r).
        // 4. **The MV is written raw, not as a difference from the MVP
        //    stack's predictor** — `inter_mvp.rs` is ported and unwired.
        //
        // Separately measured, and it is a MODE DECISION fact rather than a
        // syntax one: the homegrown ME lands on `mv.x = -22` eighth-pel on a
        // content translation of exactly 3 pixels, where C finds the integer
        // `-24`. A sub-pel refinement that prefers a fractional position over
        // an exact integer match is evidence about the ME, which chunk C4
        // replaces wholesale.
        //
        // REPLACED (docs/INTER-ENCODE-PLAN.md §1s item 7): the arm below is
        // the proven `port_entropy_inter::block::write_inter_mode_info`,
        // whose output is byte-identical to C's frame-1 tile when fed C's
        // measured decision (`inter_tile_byte_gate`). All four defects above
        // are gone by construction — the walk writes `write_ref_frames`, the
        // inter mode symbol, the DRL group and the interp filter in C's
        // order, differences the MV against `pred_mv`, and takes the FRAME's
        // single adapting `nmvc` at the header's own precision.
        let blk = decision.inter.as_deref().expect(
            "an is_inter block reached the pack with no InterModeInfo — the writer \
             REFUSES rather than falling back, because the fallback is not a decodable \
             bitstream (see BlockDecision::inter)",
        );
        let st = ectx
            .inter_syntax
            .as_ref()
            .expect("an inter block on a frame with no inter frame-syntax state");
        let frame = st.syntax();
        let nb = ectx.inter_neighbors(block_x, block_y);
        // `predmv` / `inter_mode_ctx` / `drl_ctx` are DERIVED from the
        // committed mode-info map here rather than carried from MD — see
        // `crate::partition::InterDecision`.
        let (pred_mv, inter_mode_ctx, drl) = ectx
            .inter_mvp_fields(
                block_x,
                block_y,
                decision.width as usize,
                decision.height as usize,
                blk,
            )
            .expect("an inter block on a frame with no MVP environment");
        let info = crate::port_entropy_inter::block::InterModeInfo {
            // `c_bsize_index` IS the C `BlockSize` enum order, which is the
            // discriminant `from_u8` decodes.
            bsize: svtav1_types::block::BlockSize::from_u8(crate::leaf_funnel::c_bsize_index(
                decision.width as usize,
                decision.height as usize,
            ) as u8)
            .expect("c_bsize_index yields a valid BlockSize discriminant"),
            mode: blk.mode,
            ref_frame: blk.ref_frame,
            mv: blk.mv,
            pred_mv,
            inter_mode_ctx,
            drl,
            // Inter-intra, compound and the two warped-motion inputs have no
            // candidate to come from yet; a candidate that sets them must
            // extend `InterDecision` rather than have them defaulted here.
            interintra: None,
            motion_mode: blk.motion_mode,
            num_proj_ref: blk.num_proj_ref,
            overlappable_neighbors: blk.overlappable_neighbors,
            compound: None,
            interp_filters: blk.interp_filters,
            skip_mode: blk.skip_mode,
        };
        // `FrameContext` carries `inter` and `nmvc` inline while the writer
        // takes them as separate `&mut`s (C reaches all three through one
        // `fc` pointer). Split the copies out and write them BACK — a caller
        // that dropped them would silently code every following block
        // against unadapted inter CDFs.
        // Diagnostic (SVTAV1_INTERDBG=1): the per-block inter decision AS THE
        // WRITER SEES IT — including the three fields derived here rather
        // than carried from MD (`pred_mv`, `inter_mode_ctx`, `drl_ctx`) and
        // the neighbour pair their contexts read. Field-for-field the C
        // `SVT_CINTER_OUT` dump plus the neighbours, so a divergence can be
        // localized to a block and a field instead of to a byte offset.
        //
        // It exists because a decode failure on an inter frame had NO
        // per-block evidence behind it: `SVTAV1_PACKTREE` shows `intra_mode`,
        // which an inter block leaves at 0, so an inter leaf was
        // indistinguishable from a DC intra one.
        #[cfg(feature = "std")]
        if std::env::var_os("SVTAV1_INTERDBG").is_some() {
            std::eprintln!(
                "IDBG mi=({},{}) bs={:?} mv=({},{}) pmv=({},{}) imc={} drl={:?} nb_up={} nb_left={} nbA={:?} nbL={:?}",
                block_y / 4,
                block_x / 4,
                info.bsize,
                info.mv[0].y,
                info.mv[0].x,
                info.pred_mv[0].y,
                info.pred_mv[0].x,
                info.inter_mode_ctx,
                info.drl,
                nb.up_available,
                nb.left_available,
                nb.above.map(|a| (a.mode, a.ref_frame, a.interp_filters)),
                nb.left.map(|a| (a.mode, a.ref_frame, a.interp_filters)),
            );
        }
        let mut ic = frame_ctx.inter.clone();
        let mut nmvc = frame_ctx.nmvc.clone();
        crate::port_entropy_inter::block::write_inter_mode_info(
            writer, frame_ctx, &mut ic, &mut nmvc, &nb, &frame, &info,
        );
        frame_ctx.inter = ic;
        frame_ctx.nmvc = nmvc;
    } else if is_key {
        let above_ctx = ectx.above_mode_ctx(block_x);
        let left_ctx = ectx.left_mode_ctx(block_y);
        crate::entropy::context::write_intra_mode_kf(
            writer,
            frame_ctx,
            above_ctx,
            left_ctx,
            decision.intra_mode,
        );
        // C av1_use_angle_delta(bsize) is `bsize >= BLOCK_8X8` in ENUM order
        // (reconintra.h:59): only BLOCK_4X4/4X8/8X4 are excluded — the 4:1
        // rects BLOCK_4X16/16X4 (enum 16/17) DO signal angle_delta. The
        // decoder reads the symbol for every directional mode on those
        // blocks; omitting it desyncs the tile.
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.intra_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.intra_mode,
                decision.angle_delta,
            );
        }
    } else {
        let bsize_group = crate::entropy::context::block_size_group(
            decision.width as usize,
            decision.height as usize,
        );
        crate::entropy::context::write_intra_mode_inter(
            writer,
            frame_ctx,
            bsize_group,
            decision.intra_mode,
        );
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.intra_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.intra_mode,
                decision.angle_delta,
            );
        }
    }

    // 4:2:0 chroma mode syntax — read by the decoder right after y_mode +
    // angle_delta_y when `!monochrome && is_chroma_ref` (libaom
    // read_intra_frame_mode_info, decodemv.c:824-836):
    //   uv_mode: cdf [cfl_allowed][y_mode], 14 syms if CFL allowed else 13
    //   (read_intra_mode_uv, decodemv.c:140). We always code UV_DC_PRED
    //   (symbol 0). CFL alphas only follow UV_CFL_PRED; angle_delta_uv only
    //   follows directional UV modes — UV_DC triggers neither.
    // CFL allowed = LUMA block w <= 32 && h <= 32 (is_cfl_allowed,
    // blockd.h, non-lossless path).
    //
    // ...and NOT for an INTER block. In C the whole chroma mode-info slice
    // lives inside `write_modes_b`'s intra branch (entropy_coding.c:5199-5215)
    // — an inter block's chroma mode is implied by its motion, so no symbol is
    // coded. The port's gate was `chroma_blocks.is_some() && !use_intrabc`
    // with a `debug_assert!(!decision.is_inter, "420 path is key/intra only")`
    // recording the assumption; the assumption stopped holding the moment the
    // inter arm reached the pack, and `identity_run` builds RELEASE, where the
    // assert is compiled out — so the extra `uv_mode` symbol was written
    // SILENTLY. FOUND by decoding the experimental 2-frame stream (`aomdec`:
    // "Failed to decode tile data"), not by any byte count.
    if chroma_blocks.is_some() && !use_intrabc && !decision.is_inter {
        let cfl_allowed = decision.width <= 32 && decision.height <= 32;
        crate::entropy::context::write_uv_mode(
            writer,
            frame_ctx,
            cfl_allowed,
            decision.intra_mode,
            decision.uv_mode,
        );
        // CfL alphas follow a UV_CFL_PRED chroma mode (encode_intra_chroma_
        // mode_av1, entropy_coding.c:1181; decoder read_cfl_alphas). CFL is
        // never directional, so angle_delta_uv is skipped for it.
        if decision.uv_mode == crate::entropy::context::UV_CFL_PRED {
            crate::entropy::context::write_cfl_alphas(
                writer,
                frame_ctx,
                decision.cfl_alpha_idx,
                decision.cfl_alpha_signs,
            );
        }
        // angle_delta_uv follows directional UV modes on >= 8x8 blocks
        // (read_intra_frame_mode_info, decodemv.c:833) — nonzero only
        // when the M5 ind-uv search picked a delta'd uv mode.
        if use_angle_delta(decision.width, decision.height)
            && crate::entropy::context::is_directional_mode(decision.uv_mode)
        {
            crate::entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.uv_mode,
                decision.uv_angle_delta,
            );
        }
    }

    // Palette flags: C codes them between the chroma mode-info slice and
    // the filter_intra flag (write_palette_mode_info, gated at
    // entropy_coding.c:5026 on !use_intrabc && svt_aom_allow_palette).
    // `decision.palette` is None on every current leaf (candidate
    // injection — #71 chunks 3/4 — doesn't wire a winner into
    // BlockDecision yet), so today this always takes the `None` arm:
    // BIT-IDENTICAL to the former write_no_palette_flags (symbol-0 y/uv
    // flags; the CDF updates + per-SB avg chain still run, keeping the
    // arithmetic stream aligned with C on screen-content frames). Once a
    // winner is wired, the `Some` arm below activates with no further
    // pack changes needed.
    //
    // cache/found/out_of_cache live in this outer scope (not just the
    // `if allow_palette` block) so the PALETTE MAP TOKENS write further
    // below — coded after filter_intra, per C order — can reuse them.
    let mut pal_found: alloc::vec::Vec<bool> = alloc::vec::Vec::new();
    let mut pal_out: alloc::vec::Vec<u16> = alloc::vec::Vec::new();
    let mut pal_n_out = 0usize;
    if let Some((colors, _idx_map)) = decision.palette.as_ref() {
        let pal_cache = palette_cache(ectx, block_x, block_y);
        pal_found = alloc::vec![false; pal_cache.len()];
        pal_out = alloc::vec![0u16; colors.len()];
        pal_n_out =
            crate::palette::index_color_cache(&pal_cache, colors, &mut pal_found, &mut pal_out);
    }
    if !decision.is_inter
        && !use_intrabc // C :5026: palette mode-info suppressed for IntraBC
        && crate::entropy::context::allow_palette(
            ectx.allow_sct,
            decision.width as usize,
            decision.height as usize,
        )
    {
        let neighbor_ctx = ectx.palette_neighbor_ctx(block_x, block_y);
        let palette_arg = decision.palette.as_ref().map(|(colors, _idx_map)| {
            (
                colors.as_slice(),
                pal_found.as_slice(),
                &pal_out[..pal_n_out],
            )
        });
        crate::entropy::context::write_palette_mode_info(
            writer,
            frame_ctx,
            decision.width as usize,
            decision.height as usize,
            decision.intra_mode,
            decision.uv_mode,
            chroma_blocks.is_some(),
            neighbor_ctx,
            palette_arg,
            u32::from(ectx.bit_depth),
        );
    }

    // use_filter_intra flag — C writes it right after the uv/palette
    // syntax and BEFORE code_tx_size, for every intra block passing
    // svt_aom_filter_intra_allowed (mode_decision.c:107): SH filter_intra
    // level != 0, mode == DC_PRED, **palette_size == 0**, and
    // block_size_wide/high[bsize] <= 32. Write order: entropy_coding.c:5050
    // (the flag is coded right after write_palette_mode_info, :5039). The
    // palette_size==0 gate is LOAD-BEARING: C codes NO filter_intra flag for
    // a palette block (palette forces the mode + tx), so a palette block that
    // priced/coded the flag emits an EXTRA symbol the decoder never reads,
    // desyncing the whole tile. (This was latent while palette was never
    // picked — allow_screen_content_tools=0; it fires the moment a
    // screen-content frame wins a DC-mode <=32x32 palette block.) We never
    // PREDICT with filter-intra so the flag is always 0 when coded, but on a
    // non-palette DC block the symbol MUST be coded or the decoder desyncs.
    if ectx.seq_filter_intra
        && !decision.is_inter
        && !use_intrabc // C :5050 nests under use_intrabc == 0
        && decision.intra_mode == 0 // DC_PRED
        && decision.palette.is_none() // palette_size == 0 (mode_decision.c:107)
        && decision.width <= 32
        && decision.height <= 32
    {
        let bsize_idx = crate::entropy::context::block_size_index(
            decision.width as usize,
            decision.height as usize,
        );
        let used = decision.filter_intra_mode != 5;
        crate::entropy::context::write_use_filter_intra(writer, frame_ctx, bsize_idx, used);
        if used {
            crate::entropy::context::write_filter_intra_mode(
                writer,
                frame_ctx,
                decision.filter_intra_mode,
            );
        }
    }

    // PALETTE MAP TOKENS — C's plane loop (entropy_coding.c:5064-5089):
    // `for plane in 0..2 { if palette_size[plane] > 0 { tokenize +
    // pack_map_tokens } }`, coded right after filter_intra and BEFORE
    // code_tx_size. Chroma palette is dead (`palette_size[1]` hard-0 at
    // injection — see docs/palette-port-map.md), so only plane 0 (Y) ever
    // fires; gated directly on `decision.palette` rather than re-deriving
    // `allow_palette` (a palette winner can only exist where it already
    // held, matching C's implicit invariant `palette_size > 0 =>
    // svt_aom_allow_palette` held at injection).
    if let Some((colors, idx_map)) = decision.palette.as_ref() {
        let w = decision.width as usize;
        let h = decision.height as usize;
        // C tokenizes and packs the map over the part of the block INSIDE the
        // frame -- `rows_within_bounds` / `cols_within_bounds` from
        // `svt_aom_get_block_dimensions` (palette.c:217-245), derived from
        // `mb_to_bottom_edge` / `mb_to_right_edge`, which go negative exactly
        // when the block straddles the aligned extent. The map STRIDE stays the
        // full block width; only the traversal shrinks.
        //
        // Writing the full block instead emits color-index symbols for rows and
        // columns the decoder never reads, which desyncs the tile. That was
        // latent while nothing straddled: 64-aligned frames have no straddling
        // block, and before the edge-aware PD1 walk a partial SB never reached
        // palette at presets 0..5. Enabling it turned this into three
        // DECODE-FAILs (screen 56x56 / 120x120 / 65x257 at q20 p0).
        let rows = h.min(ectx.aligned_h_px.saturating_sub(block_y));
        let cols = w.min(ectx.aligned_w_px.saturating_sub(block_x));
        debug_assert!(
            rows >= 1 && cols >= 1,
            "a coded block always has at least one in-frame row and column"
        );
        crate::entropy::context::write_palette_map_tokens(
            writer,
            frame_ctx,
            idx_map,
            w,
            rows,
            cols,
            colors.len(),
        );
    }

    // tx_size syntax — C av1_code_tx_size (entropy_coding.c:4697) called
    // from write_modes_b right after the uv/palette/filter_intra syntax
    // and before the residuals. The symbol exists ONLY at TX_MODE_SELECT
    // (`ectx.tx_mode_select`, the bit `crate::txs_arm::tx_mode_select` put in
    // the FH): then every INTRA block with bsize > 4x4 codes a tx_depth
    // symbol (the ACTUAL `decision.tx_depth` from the funnel's TXS search —
    // 0/1/2, NOT hardcoded to largest), and skip only suppresses it for inter
    // blocks. At TX_MODE_LARGEST the decoder INFERS the size and NOTHING is
    // coded. The neighbor context update (set_txfm_ctxs) runs for EVERY
    // block, signaling or not.
    //
    // This gate read `is_key` until 2026-09-01. That was the allintra arm's
    // rule (it signals TX_MODE_SELECT unconditionally) applied to every
    // frame, so a VIDEO-mode key frame at preset >= 10 — where the video arm
    // signals TX_MODE_LARGEST because `txs_level == 0` — declared LARGEST in
    // the header and then wrote one `tx_size_cdf` symbol per block anyway.
    // MEASURED on `diag 64x64 q40 p11` video, frame 0: the op-trace differ
    // (tools/ctrace-linux + identity_diff.py) put the FIRST divergence at the
    // first coded block, `CDF nsyms=2 icdf=[12800]` — TX_SIZE_CDF[0][0] —
    // present in the port and absent in C, with every partition, mode, uv
    // mode, luma tx type, luma eob and luma level already identical.
    {
        let w = decision.width as usize;
        let h = decision.height as usize;
        let depth = decision.tx_depth;
        // C `av1_code_tx_size` picks its arm on `is_inter_block(mbmi)`,
        // which is `use_intrabc || ref_frame[0] > INTRA_FRAME`
        // (block_structures.h:119) — an IntraBC block AND a genuinely inter
        // one both take the var-tx arm. While IntraBC was the only
        // inter-CLASSIFIED block this pack could emit, `use_intrabc` alone
        // was the same predicate; wiring a real inter block
        // (docs/INTER-ENCODE-PLAN.md §1s item 7) made the two differ, and an
        // inter block fell into the INTRA arm and coded a `tx_size` depth
        // symbol C does not write. MEASURED on the reference cell: the tile
        // came out `94 9a 9e` against C's `94 9a b0`, one extra 3-symbol
        // write at the end (`tx_size_cdf[2]`), everything before it
        // symbol-for-symbol and range-for-range identical.
        if use_intrabc || decision.is_inter {
            // C av1_code_tx_size inter arm (entropy_coding.c:4658-4676):
            // TX_MODE_SELECT && block_signals_txsize && !(is_inter && skip)
            // -> the var-tx walk over txfm_partition_cdf; the skip arm
            // codes NOTHING and stamps the BLOCK dims (set_txfm_ctxs with
            // skip && is_inter).
            // Left un-De-Morgan'd (clippy <=1.89 nonminimal_bool suggests
            // `!(skip || w == 4 && h == 4)`; current stable does not): the form
            // mirrors C's `!(is_inter && skip)` cited two lines above.
            #[allow(clippy::nonminimal_bool)]
            if !skip && !(w == 4 && h == 4) {
                writer_tx_size_vartx_bridge(writer, frame_ctx, ectx, block_x, block_y, w, h, depth);
                let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
                ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
            } else {
                // skip (or 4x4): context stamp only — block dims for the
                // skip-inter arm (C set_txfm_ctxs bw = n8_w * MI_SIZE).
                if skip {
                    ectx.record_txfm_dims(block_x, block_y, w, h, w, h);
                } else {
                    let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
                    ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
                }
            }
        } else {
            // C `av1_code_tx_size` (entropy_coding.c:4650-4658): the symbol is
            // coded only at TX_MODE_SELECT on a block that signals txsize AND
            // `!svt_av1_is_lossless_segment` — a coded-lossless frame
            // (base_q_idx 0 on this port's mainline path) derives TxMode
            // ONLY_4X4 and codes no depth. The 4x4 grid is still recorded for
            // the neighbours' contexts.
            if ectx.tx_mode_select && !(w == 4 && h == 4) && base_q_idx > 0 {
                let ctx = ectx.tx_size_ctx(block_x, block_y, w, h);
                crate::entropy::context::write_tx_depth(
                    writer,
                    frame_ctx,
                    w,
                    h,
                    ctx,
                    depth as usize,
                );
            }
            // set_txfm_ctxs records the CHOSEN tx dims (the C
            // tx_depth_to_tx_size chain — rect blocks halve the LONG dim
            // first) — the next blocks' tx_size contexts read them.
            let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
            ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
        }
    }

    if !skip {
        // Residual order per spec residual(): all of plane 0's txbs, then
        // plane 1 (U), then plane 2 (V) — one full-size txb per plane here
        // (libaom decode_token_recon_block intra loop,
        // decodeframe.c:936-960). A plane with eob == 0 inside a non-skip
        // block still writes its txb (as a txb_skip=1 symbol) — only the
        // block-level skip removes txbs entirely.
        //
        // C-exact coefficient coding (av1_write_coeffs_txb_1d port).
        // The block uses a single full-size transform (tx_depth 0), so
        // plane_bsize == txsize_to_bsize[tx_size] and the luma
        // txb_skip_ctx fast path applies; dc_sign_ctx comes from the
        // per-4x4 (dc_sign << 6 | cul_level) neighbor bytes like C.
        use crate::entropy::coeff_c;
        let w = decision.width as usize;
        let h = decision.height as usize;
        // C `av1_read_tx_type`/`av1_get_tx_type` (decodemv.c:637): the luma
        // tx_type CDF is indexed by the FILTER-INTRA-mapped intra dir for
        // filter-intra blocks (use_filter_intra), not the coded DC mode —
        // `fimode_to_intradir[filter_intra_mode]`. Using DC here selects a
        // different intra_ext_tx_cdf instance than the decoder, desyncing
        // the tile once a filter-intra block with a non-DC-mapped mode is
        // coded (M0 filter_intra level 1 injects all five fi modes).
        let tx_intra_dir = if decision.filter_intra_mode != 5 {
            crate::leaf_funnel::FIMODE_TO_INTRADIR[decision.filter_intra_mode as usize] as usize
        } else {
            decision.intra_mode as usize
        };
        if decision.tx_depth == 0 {
            let tx_size = coeff_c::tx_size_from_dims(w, h);
            let (above, left) = ectx.coeff_neighbors(block_x, block_y, w, h);
            let (txb_skip_ctx, dc_sign_ctx) = coeff_c::get_txb_ctx(0, above, left, true, false);
            // 64-dim transforms keep only the 32-capped low-frequency
            // quadrant; the C writer expects that quadrant packed at the
            // adjusted stride.
            let aw = coeff_c::txb_wide(tx_size);
            let ah = coeff_c::txb_high(tx_size);
            let packed;
            let coeffs: &[i32] = if aw == w && ah == h {
                &decision.qcoeffs
            } else {
                let mut v = alloc::vec![0i32; aw * ah];
                for r in 0..ah {
                    v[r * aw..r * aw + aw].copy_from_slice(&decision.qcoeffs[r * w..r * w + aw]);
                }
                packed = v;
                &packed
            };
            // The decision's eob was derived from the mode-decision scan;
            // the bitstream eob must be relative to the C scan order for
            // this (tx_size, tx_type).
            let tx_type = decision.tx_type as usize;
            let scan = crate::entropy::scan_tables::scan(
                tx_size,
                crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
            );
            let mut eob = 0i32;
            for (i, &pos) in scan.iter().enumerate() {
                if coeffs[pos as usize] != 0 {
                    eob = i as i32 + 1;
                }
            }
            // Diagnostic aid: SVTAV1_CODED_EOB=1 prints the TRUE coded
            // scan-order eob per depth-0 leaf (the tree dump's d.eob is a
            // raster-order artifact). No output change.
            #[cfg(feature = "std")]
            if crate::dbgenv::coded_eob() {
                let nz = coeffs.iter().filter(|&&c| c != 0).count();
                eprintln!("CODED x{block_x} y{block_y} {w}x{h} tx{tx_type} scan_eob={eob} nz={nz}");
            }
            let cul_level = coeff_c::write_coeffs_txb_1d(
                coeff_fc,
                writer,
                tx_size,
                tx_type,
                0,
                txb_skip_ctx,
                dc_sign_ctx,
                coeffs,
                eob,
                tx_intra_dir,
                base_q_idx,
                false,
                // `is_inter_block` = `use_intrabc || ref_frame[0] > INTRA_FRAME`
                // — the depth-0 twin of the depth>0 site below. This is the
                // FOURTH place the port spelled that predicate `use_intrabc`,
                // which was the same thing only while IntraBC was the one
                // inter-classified block the pack could emit.
                use_intrabc || decision.is_inter,
            );
            ectx.record_coeff(block_x, block_y, w, h, cul_level as u8);
        } else {
            // tx_depth > 0: the C tx grid at this depth
            // (tx_depth_to_tx_size / tx_blocks_per_depth, raster order —
            // spec residual() / C av1_write_coeffs_mb), each txb with its
            // own neighbor contexts and tx type; the per-txb contexts
            // read the bytes recorded by the previous txbs.
            let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, decision.tx_depth);
            let cols = w / txw;
            let txbs = cols * (h / txh);
            let tx_size = coeff_c::tx_size_from_dims(txw, txh);
            for txb in 0..txbs {
                // IntraBC (inter-classified) blocks write their residual
                // in the recursive var-tx z-order (C write_inter_txb_coeff
                // recursion == the decoder's read order == the search
                // walk's txb_org_inter); intra blocks in raster. At depth
                // <= 1 the two coincide; at depth 2 they differ on the
                // square/h-rect bsizes and a raster write self-desyncs.
                let (rel_x, rel_y) = if decision.use_intrabc || decision.is_inter {
                    crate::leaf_funnel::txb_org_inter(w, h, decision.tx_depth, txb)
                } else {
                    ((txb % cols) * txw, (txb / cols) * txh)
                };
                let tx_x = block_x + rel_x;
                let tx_y = block_y + rel_y;
                #[cfg(feature = "std")]
                if crate::dbgenv::packtxb() {
                    let nz = decision.txb_qcoeffs[txb]
                        .iter()
                        .filter(|&&v| v != 0)
                        .count();
                    eprintln!(
                        "PACKTXB blk=({block_x},{block_y}) {w}x{h} d={} ibc={} txb={txb} pos=({rel_x},{rel_y}) tt={} nz={nz}",
                        decision.tx_depth, decision.use_intrabc, decision.txb_tx_types[txb],
                    );
                }
                let (above, left) = ectx.coeff_neighbors(tx_x, tx_y, txw, txh);
                let (txb_skip_ctx, dc_sign_ctx) =
                    coeff_c::get_txb_ctx(0, above, left, false, false);
                let tx_type = decision.txb_tx_types[txb] as usize;
                let coeffs = &decision.txb_qcoeffs[txb];
                let scan = crate::entropy::scan_tables::scan(
                    tx_size,
                    crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
                );
                let mut eob = 0i32;
                for (i, &pos) in scan.iter().enumerate() {
                    if coeffs[pos as usize] != 0 {
                        eob = i as i32 + 1;
                    }
                }
                let cul_level = coeff_c::write_coeffs_txb_1d(
                    coeff_fc,
                    writer,
                    tx_size,
                    tx_type,
                    0,
                    txb_skip_ctx,
                    dc_sign_ctx,
                    coeffs,
                    eob,
                    tx_intra_dir,
                    base_q_idx,
                    false,
                    // C `get_ext_tx_set` / `av1_get_tx_type` select the
                    // tx-type CDF rows on `is_inter_block(mbmi)` =
                    // `use_intrabc || ref_frame[0] > INTRA_FRAME`
                    // (block_structures.h:119). The port tested `use_intrabc`
                    // alone, which was the same predicate while IntraBC was
                    // the only inter-classified block the pack could emit —
                    // the THIRD site with that bug (the other two are
                    // `av1_code_tx_size`'s arm and `record_inter_dims`).
                    use_intrabc || decision.is_inter,
                );
                ectx.record_coeff(tx_x, tx_y, txw, txh, cul_level as u8);
            }
        }

        // Chroma txbs: plane 1 (U) then plane 2 (V), each one full-size
        // (bsize_uv) transform with its own neighbor context state —
        // PAIR dims/origin for sub-8 chroma-ref blocks.
        if let Some((u_q, _u_eob, v_q, _v_eob)) = chroma_blocks.as_ref() {
            let cw = w.max(8) / 2;
            let ch = h.max(8) / 2;
            let cx = ((block_x >> 3) << 3) / 2 + if w >= 8 { (block_x % 8) / 2 } else { 0 };
            let cy = ((block_y >> 3) << 3) / 2 + if h >= 8 { (block_y % 8) / 2 } else { 0 };
            // IBC chunk 9: on an INTER-classified (IntraBC) block the
            // decoder DERIVES the chroma tx type from the co-located luma
            // type (av1_get_tx_type plane>0 inter arm) — the same
            // follows-luma rule MDS3 applied: luma txb-0's type when the
            // chroma inter ext set admits it, else DCT. Intra blocks keep
            // the uv-mode mapping.
            // ...and a genuinely INTER block takes the same arm: C's
            // predicate is `is_inter_block(mbmi)`, not `use_intrabc`. The
            // chroma tx type selects the SCAN ORDER, so an inter block that
            // fell into the uv-mode arm scanned its chroma levels in a
            // different order than the decoder — the SIXTH site of this
            // predicate confusion, and the one with the least visible
            // symptom, because chroma is derived and codes no tx-type symbol
            // of its own.
            let uv_tt = if use_intrabc || decision.is_inter {
                let luma_tt = if decision.tx_depth == 0 {
                    decision.tx_type
                } else {
                    decision.txb_tx_types.first().copied().unwrap_or(0)
                } as usize;
                let uv_tx = crate::entropy::coeff_c::adjusted_tx_size(
                    crate::entropy::coeff_c::tx_size_from_dims(cw, ch),
                );
                let uv_set = crate::entropy::coeff_c::ext_tx_set_type(uv_tx, true, false);
                if crate::leaf_funnel::ext_tx_used(uv_set, luma_tt) {
                    luma_tt
                } else {
                    0
                }
            } else {
                crate::leaf_funnel::uv_tx_type(decision.uv_mode, cw, ch)
            };
            #[cfg(feature = "std")]
            if crate::dbgenv::coded_eob() {
                let uv_ts = crate::entropy::coeff_c::tx_size_from_dims(cw, ch);
                let sidx = crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tt] as usize;
                let uv_scan = crate::entropy::scan_tables::scan(uv_ts, sidx);
                let eob_of = |q: &[i32]| {
                    let mut e = 0usize;
                    for (i, &p) in uv_scan.iter().enumerate() {
                        if q[p as usize] != 0 {
                            e = i + 1;
                        }
                    }
                    e
                };
                let sum_of = |q: &[i32]| q.iter().map(|c| c.unsigned_abs() as u64).sum::<u64>();
                eprintln!(
                    "CODEDUV x{block_x} y{block_y} cw{cw} ch{ch} u_eob={} v_eob={} u_sum={} v_sum={}",
                    eob_of(u_q),
                    eob_of(v_q),
                    sum_of(u_q),
                    sum_of(v_q),
                );
            }
            write_chroma_txb(
                writer, coeff_fc, ectx, 0, cx, cy, cw, ch, u_q, base_q_idx, uv_tt,
            );
            write_chroma_txb(
                writer, coeff_fc, ectx, 1, cx, cy, cw, ch, v_q, base_q_idx, uv_tt,
            );
        }
    } else {
        // Skipped blocks contribute zero cul_level neighbors (C writes the
        // txb through the same path with eob == 0 -> cul 0). For skip the
        // decoder zeroes EVERY plane's entropy context over the block span
        // (spec reset_block_context; libaom av1_reset_entropy_context) —
        // mirror that for the chroma planes too.
        ectx.record_coeff(
            block_x,
            block_y,
            decision.width as usize,
            decision.height as usize,
            0,
        );
        if chroma_blocks.is_some() {
            let cw = (decision.width as usize).max(8) / 2;
            let ch = (decision.height as usize).max(8) / 2;
            let cx = ((block_x >> 3) << 3) / 2
                + if decision.width >= 8 {
                    (block_x % 8) / 2
                } else {
                    0
                };
            let cy = ((block_y >> 3) << 3) / 2
                + if decision.height >= 8 {
                    (block_y % 8) / 2
                } else {
                    0
                };
            ectx.record_coeff_uv(0, cx, cy, cw, ch, 0);
            ectx.record_coeff_uv(1, cx, cy, cw, ch, 0);
        }
    }

    // Update context maps for subsequent blocks. The y_mode is signaled
    // for skip blocks too, and the decoder records it in its above/left
    // mode contexts — so must we.
    let mode = decision.intra_mode;
    ectx.record_block(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        mode,
        decision.uv_mode,
        skip,
    );
    // The inter mi grid (docs/INTER-ENCODE-PLAN.md §1s item 2), stamped for
    // EVERY block — an intra block inside a P frame is a neighbour the inter
    // reference-count and mode contexts read, so stamping only inter blocks
    // would leave the previous block's ref_frame standing in for it.
    ectx.record_inter_mi(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        crate::port_entropy_inter::NeighborMi {
            // C `block_mi.mode` — the INTER mode (`NEWMV` = 16 and up) on an
            // inter block, not `intra_mode`, which an inter block leaves at
            // 0 (`DC_PRED`).
            //
            // This is load-bearing and silent: `inter_mvp::setup_ref_mv_list`
            // counts `have_newmv_in_inter_mode(entry.mode)` over the scanned
            // neighbours into `newmv_count`, which selects the `mode_context`
            // a decoder ALSO derives from its own reconstructed mi map. Stamp
            // `DC_PRED` for an inter neighbour and the two derivations part
            // company, the `newmv` symbol is coded from a different CDF row
            // than the one read, and the tile desyncs from that block on.
            mode: decision.inter.as_deref().map_or(mode, |b| b.mode as u8),
            ref_frame: decision.inter.as_deref().map_or([0, -1], |b| b.ref_frame),
            interp_filters: decision.inter.as_deref().map_or(0, |b| b.interp_filters),
            use_intrabc: decision.use_intrabc,
            skip_mode: decision.inter.as_deref().is_some_and(|b| b.skip_mode),
            // Both are 0 until a COMPOUND candidate exists: C codes them
            // only under `has_second_ref`, and every candidate this port
            // injects is single-reference. They are stamped rather than
            // omitted because the neighbour contexts
            // (`comp_group_idx_context` / `comp_index_context`) read them
            // off the NEIGHBOUR, so leaving a stale value here would move a
            // future compound block's symbol.
            comp_group_idx: 0,
            compound_idx: 0,
            bsize: crate::entropy::context::block_size_index(
                decision.width as usize,
                decision.height as usize,
            ) as u8,
        },
        // C `block_mi.mv` — a DV for an IntraBC block, the inter MV for an
        // inter one, zero for a plain intra block.
        if decision.use_intrabc {
            [decision.dv, svtav1_types::motion::Mv::ZERO]
        } else {
            decision
                .inter
                .as_deref()
                .map_or([svtav1_types::motion::Mv::ZERO; 2], |b| b.mv)
        },
        decision.partition_type as u8,
    );
    // IBC chunk 9 (aom-rs Root 6): stamp the inter-neighbour override
    // state — get_tx_size_context substitutes an is_inter neighbour's
    // BLOCK dims for its TXFM-context byte (entropy_coding.c:4626-4637);
    // IntraBC blocks are the only inter-classified neighbours here.
    ectx.record_inter_dims(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        // Same `is_inter_block` predicate as the tx_size arm above: the
        // neighbour override is about INTER blocks, of which IntraBC is one
        // kind and a real inter block is the other.
        use_intrabc || decision.is_inter,
    );
    // Palette neighbor state (C mbmi->palette_mode_info, stamped for
    // EVERY block — palette or not, matching record_block above).
    ectx.record_palette(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        decision
            .palette
            .as_ref()
            .map(|(colors, _idx_map)| colors.as_slice()),
    );

    // Deblocking geometry: exactly what the decoder derives per mi from
    // the parsed block — dims (single TX per block), signaled skip, and
    // inter-ness (skip only suppresses deblocking for inter blocks).
    // The decoder's mi grid: BLOCK identity/dims (chroma TX + pu_edge
    // derive from these) + the LUMA TX grid (quartered at tx_depth 1 —
    // chroma never splits with luma tx_depth).
    geom.record_block(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        decision.is_inter,
        skip,
    );
    if decision.tx_depth > 0 {
        let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(
            decision.width as usize,
            decision.height as usize,
            decision.tx_depth,
        );
        let cols = decision.width as usize / txw;
        let txbs = cols * (decision.height as usize / txh);
        for txb in 0..txbs {
            geom.record_tx_dims(
                block_x + (txb % cols) * txw,
                block_y + (txb / cols) * txh,
                txw,
                txh,
            );
        }
    }
}

/// Extract the leaf decision from a partition tree node.
/// Panics if the node is not a Leaf (HORZ/VERT children must always be leaves).
fn expect_leaf(tree: &crate::partition::PartitionTree) -> &crate::partition::BlockDecision {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => d,
        crate::partition::PartitionTree::Split { .. } => {
            panic!("HORZ/VERT children must be leaf blocks, not split nodes")
        }
    }
}

/// Recursively encode a partition tree to the bitstream in AV1 spec order.
///
/// AV1 spec: for each SB, write partition_type, then:
/// - PARTITION_NONE: write partition symbol + block syntax
/// - PARTITION_SPLIT: write partition symbol, recurse into 4 children
/// - PARTITION_HORZ/VERT: write partition symbol, then block syntax for
///   each child directly (NO partition symbols for children — the decoder
///   reads them as leaf blocks without expecting a partition symbol)
///
/// Partition context is derived from tracked above/left partition arrays,
/// matching the rav1d decoder's context derivation exactly.
/// Frame-edge partition flags for a SQUARE partition node — C
/// `encode_partition_av1` (entropy_coding.c:941-943):
/// `hbs` = HALF the node width in pixels, then
/// `has_rows = (y + hbs) < aligned_height`, `has_cols = (x + hbs) < aligned_width`.
///
/// The ALIGNED frame extent is recovered from the deblock geometry, which is
/// built from those same aligned dims (`DeblockGeom::new(w, h, ..)`, ~:884) and is
/// already threaded through this whole walk — so the partition edge rules and
/// the deblock walk can never disagree about where the frame ends. Aligned dims
/// are always a multiple of 8, so `mi * 4` recovers the pixel extent exactly.
///
/// On a 64-aligned frame every node lies wholly inside the frame, so both flags
/// are always `true` and the callers below stay bit-identical to the pre-edge
/// port.
#[inline]
fn partition_edge_flags(
    geom: &crate::deblock::DeblockGeom,
    block_x: usize,
    block_y: usize,
    node_w: usize,
) -> (bool, bool) {
    crate::frame_geom::edge_has_rows_cols(
        geom.mi_cols * 4,
        geom.mi_rows * 4,
        block_x,
        block_y,
        node_w / 2,
    )
}

#[allow(clippy::too_many_arguments)]
/// Fold the per-b64 coding-unit results of ONE superblock into the SB's
/// result (task #91, SB128).
///
/// SB64 (`units.len() == 1`, `unit_size == sb_size`): the identity — the
/// single `PartitionResult` is moved out unchanged, so every SB64 caller is
/// byte-identical by construction.
///
/// SB128: the up-to-4 b64 quadrants become the children of a
/// `PARTITION_SPLIT` node rooted at the 128 square. That is exactly what C
/// codes — `encode_partition_av1` writes one partition symbol for the 128
/// node against the 8-symbol alphabet at CDF row `bsl = 4` (ctx 16..19,
/// `svt_aom_partition_cdf_length`, entropy_coding.c:922), then
/// `svt_aom_write_modes_sb` recurses into the quadrants in Z-order. The
/// entropy walk ([`encode_partition_tree`]) already handles a 128-wide
/// `Split` node: it derives ctx/nsymbs from the node width via
/// `EntropyCtx::partition_ctx` and passes `is_128 = w == 128` to
/// `write_partition_edge`, which is what selects the H4/V4-free gathers at
/// a frame edge.
///
/// Off-frame quadrants are already absent from `units`
/// (`sb128_geom::sb_coding_units` drops them, C's `mi_row + y_idx >=
/// mi_rows` `continue`), so `children` holds only the in-frame quadrants —
/// the packed layout the walk's Split arm expects.
///
/// WHY FORCED-SPLIT IS CORRECT HERE, NOT A HEURISTIC (verified first-hand
/// against reference/svt-av1/Source, 2026-07-19 — this supersedes the port map's
/// "UNVERIFIED for textured content" caveat):
///
/// C `set_blocks_to_be_tested` (Codec/enc_dec_process.c:1483-1499) computes
/// the MD scan's largest square candidate as
///
/// ```text
/// int max_sq_size = ctx->max_block_size;
/// if (pcs->mimic_only_tx_4x4)             max_sq_size = MIN(.., 8);
/// else if (static_config.max_tx_size==32) max_sq_size = MIN(.., 32);
/// else if (pcs->slice_type == I_SLICE)    max_sq_size = MIN(.., 64);
/// ```
///
/// — so on a KEY frame the largest square ever ENTERED INTO THE SCAN is
/// 64x64, whatever the superblock size. A BLOCK_128X128 is never an MD
/// candidate on an I_SLICE, so the 128 root has no codable outcome except
/// PARTITION_SPLIT. (`ctx->max_block_size` itself is `super_block_size`
/// unconditionally at M0..M7 — `get_max_block_size_allintra`,
/// enc_mode_config.c:7055-7080, sets `base_var_th_cap = (uint16_t)~0`, so
/// the `variance <= var_th_cap` test on a `uint16_t` variance is a
/// tautology; the clamp above is what actually decides.)
///
/// SCOPE OF THAT PROOF: it covers I_SLICE frames — which is the port's
/// target (ALLINTRA single-frame KEY, docs/ACCEPTANCE-CRITERIA.md). On an
/// INTER frame `max_sq_size` is NOT clamped to 64 and a genuine 128-level
/// NONE/HORZ/VERT RD search would be required; inter is unported
/// throughout, so this path is consistent with the rest of the encoder
/// rather than a new limitation. `debug_assert`ed below.
fn merge_sb_units(
    mut units: Vec<crate::partition::PartitionResult>,
    sb_size: usize,
    unit_size: usize,
    is_key: bool,
) -> crate::partition::PartitionResult {
    if sb_size == unit_size {
        debug_assert_eq!(units.len(), 1, "SB64 must have exactly one coding unit");
        return units.remove(0);
    }
    debug_assert_eq!((sb_size, unit_size), (128, 64));
    debug_assert!(
        is_key,
        "the forced-SPLIT 128 root is only PROVEN on an I_SLICE (C clamps the MD \
         scan's max square to 64 there, enc_dec_process.c:1497); an INTER frame \
         needs a real 128-level NONE/HORZ/VERT RD search"
    );
    let mut out = crate::partition::PartitionResult {
        partition_type: crate::partition::PartitionType::Split,
        rd_cost: 0,
        distortion: 0,
        rate: 0,
        num_blocks: 0,
        decisions: alloc::vec::Vec::new(),
        tree: None,
    };
    let mut children = alloc::vec::Vec::with_capacity(units.len());
    for u in units {
        out.distortion += u.distortion;
        out.rate += u.rate;
        out.num_blocks += u.num_blocks;
        out.decisions.extend(u.decisions);
        if let Some(t) = u.tree {
            children.push(t);
        }
    }
    out.rd_cost = out.distortion;
    out.tree = Some(crate::partition::PartitionTree::Split {
        partition_type: crate::partition::PartitionType::Split,
        width: sb_size as u16,
        height: sb_size as u16,
        children,
    });
    out
}

fn encode_partition_tree(
    tree: &crate::partition::PartitionTree,
    writer: &mut crate::entropy::writer::AomWriter,
    frame_ctx: &mut crate::entropy::context::FrameContext,
    coeff_fc: &mut crate::entropy::coeff_c::CoeffFc,
    base_q_idx: u8,
    ectx: &mut EntropyCtx,
    is_key: bool,
    block_x: usize,
    block_y: usize,
    chroma: &mut Option<ChromaPass<'_>>,
    geom: &mut crate::deblock::DeblockGeom,
) {
    match tree {
        crate::partition::PartitionTree::Leaf(decision) => {
            let w = decision.width as usize;
            let h = decision.height as usize;
            if w > 4 || h > 4 {
                let (ctx, nsymbs) = ectx.partition_ctx(block_x, block_y, w);
                let (has_rows, has_cols) = partition_edge_flags(geom, block_x, block_y, w);
                // A PARTITION_NONE leaf is only legal where the node lies wholly
                // inside the frame: at an edge the non-SPLIT outcome is VERT
                // (right edge) or HORZ (bottom edge), never NONE, and with BOTH
                // flags false the partition is forced to SPLIT. The edge-aware
                // search must therefore never hand us a NONE leaf at an edge.
                debug_assert!(
                    has_rows && has_cols,
                    "PARTITION_NONE leaf at a frame edge ({block_x},{block_y}) {w}x{h}: \
                     has_rows={has_rows} has_cols={has_cols} — illegal per spec 5.11.4"
                );
                crate::entropy::context::write_partition_edge(
                    writer,
                    frame_ctx,
                    ctx,
                    0,
                    nsymbs, // 0 = PARTITION_NONE
                    w == 128,
                    has_rows,
                    has_cols,
                );
            }

            // Update partition context for PARTITION_NONE
            ectx.update_partition_ctx(
                block_x,
                block_y,
                w,
                h,
                crate::partition::PartitionType::None,
            );

            encode_block_syntax(
                decision, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x, block_y,
                chroma, geom,
            );
        }
        crate::partition::PartitionTree::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let w = *width as usize;
            let h = *height as usize;
            let (ctx, nsymbs) = ectx.partition_ctx(block_x, block_y, w);
            let (has_rows, has_cols) = partition_edge_flags(geom, block_x, block_y, w);
            crate::entropy::context::write_partition_edge(
                writer,
                frame_ctx,
                ctx,
                *partition_type as u8,
                nsymbs,
                w == 128,
                has_rows,
                has_cols,
            );

            let half_w = w / 2;
            let half_h = h / 2;
            match (*partition_type, children.len()) {
                (crate::partition::PartitionType::Split, _) => {
                    // PARTITION_SPLIT: up to 4 quarter-size children in Z-order.
                    // On a partial SB the off-frame quadrants were pruned from
                    // `children` by encode_fixed_tree, so walk the 4 quadrant
                    // SLOTS, skip the off-frame ones by absolute position (C
                    // svt_aom_write_modes_sb's `mi_row+y_idx >= mi_rows ||
                    // mi_col+x_idx >= mi_cols` continue, entropy_coding.c:5498),
                    // and pull the packed in-frame children in order. A
                    // 64-aligned frame keeps all four in-frame → byte-identical.
                    // Don't update partition context here — children do it —
                    // EXCEPT the terminal 8x8 split (4x4 children write no
                    // partition bytes; the decoder sets the 8x8 cell to the
                    // SPLIT value, dav1d decode_sb BL_8X8). An 8x8 node is never
                    // a frame edge, so all four 4x4 quadrants are in-frame.
                    if half_w == 4 {
                        ectx.update_partition_ctx(
                            block_x,
                            block_y,
                            w,
                            h,
                            crate::partition::PartitionType::Split,
                        );
                    }
                    let aligned_w = geom.mi_cols * 4;
                    let aligned_h = geom.mi_rows * 4;
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = block_x + (i & 1) * half_w;
                        let cy = block_y + (i >> 1) * half_h;
                        if cx >= aligned_w || cy >= aligned_h {
                            continue;
                        }
                        encode_partition_tree(
                            &children[ci],
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            cx,
                            cy,
                            chroma,
                            geom,
                        );
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "packed in-frame child count must equal the in-frame quadrant count"
                    );
                }
                (crate::partition::PartitionType::Horz, _) => {
                    // PARTITION_HORZ: two children stacked vertically — OR, on
                    // a partial SB (task #95 chunk 2), a single in-frame top
                    // block (`children.len() == 1`), the bottom half being
                    // off-frame (C write_modes_sb codes block 1 only if
                    // `mi_row + hbs < mi_rows`, entropy_coding.c:5490).
                    // Update partition context for HORZ (children don't do it).
                    ectx.update_partition_ctx(
                        block_x,
                        block_y,
                        w,
                        h,
                        crate::partition::PartitionType::Horz,
                    );

                    // Children are leaf blocks — encode directly without
                    // partition symbols (decoder reads them as direct blocks).
                    let top = expect_leaf(&children[0]);
                    encode_block_syntax(
                        top, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x,
                        block_y, chroma, geom,
                    );
                    if let Some(bot_tree) = children.get(1) {
                        let bot = expect_leaf(bot_tree);
                        encode_block_syntax(
                            bot,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x,
                            block_y + half_h,
                            chroma,
                            geom,
                        );
                    }
                }
                (crate::partition::PartitionType::Vert, _) => {
                    // PARTITION_VERT: two children side by side — OR a single
                    // in-frame left block on a partial SB (task #95 chunk 2),
                    // the right half being off-frame.
                    // Update partition context for VERT.
                    ectx.update_partition_ctx(
                        block_x,
                        block_y,
                        w,
                        h,
                        crate::partition::PartitionType::Vert,
                    );

                    let left = expect_leaf(&children[0]);
                    encode_block_syntax(
                        left, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x,
                        block_y, chroma, geom,
                    );
                    if let Some(right_tree) = children.get(1) {
                        let right = expect_leaf(right_tree);
                        encode_block_syntax(
                            right,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x + half_w,
                            block_y,
                            chroma,
                            geom,
                        );
                    }
                }
                (ptype, n) => {
                    // Extended partitions: children are DIRECT leaf blocks at
                    // spec-defined offsets — no partition symbols of their own.
                    let quarter_w = w / 4;
                    let quarter_h = h / 4;
                    let offsets: &[(usize, usize)] = match (ptype, n) {
                        // 2 tops (w/2 x h/2) + full-width bottom (w x h/2)
                        (crate::partition::PartitionType::HorzA, 1..=3) => {
                            &[(0, 0), (half_w, 0), (0, half_h)]
                        }
                        // full-width top + 2 bottoms
                        (crate::partition::PartitionType::HorzB, 1..=3) => {
                            &[(0, 0), (0, half_h), (half_w, half_h)]
                        }
                        // 2 lefts (w/2 x h/2) + full-height right (w/2 x h)
                        (crate::partition::PartitionType::VertA, 1..=3) => {
                            &[(0, 0), (0, half_h), (half_w, 0)]
                        }
                        // full-height left + 2 rights
                        (crate::partition::PartitionType::VertB, 1..=3) => {
                            &[(0, 0), (half_w, 0), (half_w, half_h)]
                        }
                        // FEWER THAN 4 IS LEGAL AT A FRAME BOUNDARY. A node
                        // that is not itself a boundary node (has_rows &&
                        // has_cols both true) can still STRADDLE the aligned
                        // extent, and then its H4/V4 sub-blocks at the tail
                        // start outside the frame and code nothing
                        // (`svt_aom_write_modes_sb` early return). Children are
                        // dropped from the TAIL — highest y for H4, highest x
                        // for V4 — so zipping the surviving children against
                        // the full offset list pairs them correctly; a middle
                        // child cannot be dropped while a later one survives.
                        //
                        // This panicked as `unsupported partition shape
                        // (Horz4, 3)` on a 512x481 crop of gb82-sc/graph.png at
                        // preset 2. It went unnoticed because the identity
                        // harness rejects odd dims for `crop:` ("I420 needs
                        // even dims"), so no gate could reach an odd-height
                        // real-content frame — the panic was found by the new
                        // IntraBC tier-invariance test, which builds its own
                        // planes and therefore does not go through that check.
                        (crate::partition::PartitionType::Horz4, 1..=4) => &[
                            (0, 0),
                            (0, quarter_h),
                            (0, 2 * quarter_h),
                            (0, 3 * quarter_h),
                        ],
                        (crate::partition::PartitionType::Vert4, 1..=4) => &[
                            (0, 0),
                            (quarter_w, 0),
                            (2 * quarter_w, 0),
                            (3 * quarter_w, 0),
                        ],
                        other => panic!("unsupported partition shape {other:?}"),
                    };
                    ectx.update_partition_ctx(block_x, block_y, w, h, ptype);
                    for (child, &(dx, dy)) in children.iter().zip(offsets) {
                        let leaf = expect_leaf(child);
                        encode_block_syntax(
                            leaf,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x + dx,
                            block_y + dy,
                            chroma,
                            geom,
                        );
                    }
                }
            }
        }
    }
}

/// bd10 LUMA re-encode pass (task #94) — the "M4+ bypass_encdec re-predict
/// dance" (docs/bd10-port-map.md §5). The u8 MD funnel already produced the
/// partition / mode / tx DECISIONS; because RD is ~16x-scale-invariant between
/// bd8 and bd10 for `sample << 2` content (dist scales 16x, lambda x16, rate
/// bit-depth-independent), those decisions coincide with C's true-10-bit MD.
/// This pass recomputes ONLY the bit-depth-sensitive coded LUMA levels + the
/// 10-bit recon that feeds neighbour prediction, mutating each leaf's
/// `BlockDecision` in place; the (unchanged) entropy walk then codes the
/// 10-bit levels. bd8 never calls this, so the bd8 bitstream is untouched.
///
/// SCOPE (updated 2026-07-19): the bd10 full-RD funnel now covers the DC family
/// AND directional + filter-intra intra AND the chroma uv/CfL path. Only
/// `tx_depth > 0` still unconditionally falls back to u8 (directional
/// additionally when the SH edge filter is on). The `bd10_tree_supported` gate
/// below enumerates the current envelope; an out-of-envelope leaf falls back
/// rather than miscoding pixels. (The original scope was DC-only, tx_depth 0.)
#[allow(clippy::too_many_arguments)]
/// Read-only pre-pass: is every luma leaf of `tree` inside the ported bd10 u16
/// re-encode envelope? The u16 predict/tx path (`predict_unit_hbd`,
/// `bd10_reencode_node`) panics on the not-yet-ported cases so a loud "not
/// ported" beats silently miscoding 10-bit pixels. As of 2026-07-19 that is
/// ONLY `tx_depth > 0` (unconditional) plus directional intra WHEN the SH edge
/// filter is on (filt_type would need the live per-block smooth-neighbour
/// derivation); directional (edge filter off) and filter-intra are now ported
/// (`dr_predict_hbd` / `predict_filter_intra_hbd`). This gate ensures
/// `bd10_reencode_luma` runs ONLY when the whole frame is supported, so an
/// out-of-envelope bd10 frame falls back to the (non-panicking, if not yet
/// byte-exact) u8 output instead of crashing a public-API caller.
fn bd10_tree_supported(tree: &crate::partition::PartitionTree, edge_filter: bool) -> bool {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => {
            // Filter-intra IS ported (predict_filter_intra_hbd) and directional
            // intra IS ported (dr_predict_hbd) — but the re-encode passes
            // filt_type=0, valid only when the SH edge filter is off. So a
            // directional leaf is in-envelope ONLY when !edge_filter; with
            // edge_filter on it falls back (filt_type would need the live
            // per-block smooth-neighbour derivation — a future follow-up). Only
            // tx_depth>0 still unconditionally falls back.
            let directional = matches!(d.intra_mode, 3..=8)
                || (matches!(d.intra_mode, 1 | 2) && d.angle_delta != 0);
            // Chroma re-encode (task #94): the bd10 chroma pass predicts via
            // predict_unit_hbd, which supports DC/V/H/SMOOTH/PAETH + directional
            // (edge_filter off). UV_CFL_PRED (13) is NOT a predict_unit_hbd mode
            // — it is handled separately in `bd10_reencode_chroma_node`, which
            // rebuilds the CfL prediction from the 10-bit LUMA recon the luma
            // pass just produced (`cfl_luma_subsampling_420_hbd` +
            // `cfl_predict_hbd` on a DC base). That support MUST stay in
            // lockstep with the search's `cfl_gate`: a leaf the search can pick
            // but the post-pass rejects silently drops the WHOLE FRAME out of
            // the re-encode, which is a far worse (invisible) failure than a
            // visible mode divergence.
            let uv_directional =
                matches!(d.uv_mode, 3..=8) || (matches!(d.uv_mode, 1 | 2) && d.uv_angle_delta != 0);
            let uv_ok = !uv_directional || !edge_filter;
            // A palette or IntraBC leaf is NOT re-encodable by this post-pass:
            // `bd10_reencode_node` predicts every leaf with
            // `predict_unit_hbd(d.intra_mode, d.angle_delta, d.filter_intra)`
            // and never consults `d.palette` / `d.ibc`, so it would code
            // DC-based levels under palette syntax — a decoder desync, not a
            // quality loss.
            //
            // This is unreachable today (the post-pass runs only at preset >= 9,
            // where `sc_detect` yields palette_level = 0 and intrabc_level = 0
            // unconditionally), and it was unreachable BEFORE for a second
            // reason too: palette was gated out of the bd10 funnel entirely.
            // That second reason is now gone — palette is ported at bd10 — so
            // the invariant rests on the sc_detect preset table alone. Enforce
            // it structurally instead: a leaf the post-pass cannot code drops
            // the frame back to the u8 output, which is the same
            // fall-back-don't-miscode contract every other clause here has.
            let paletted = d.palette.is_some() || d.use_intrabc;
            d.tx_depth == 0 && (!directional || !edge_filter) && uv_ok && !paletted
        }
        crate::partition::PartitionTree::Split { children, .. } => {
            children.iter().all(|c| bd10_tree_supported(c, edge_filter))
        }
    }
}

/// Returns the frame's 10-bit luma recon as an **SB-extent-sized, ALIGNED-
/// strided** canvas — the same shape the funnel's `tile_frame_recon10` has, and
/// for the same reason: a boundary leaf may STRADDLE the aligned extent, and
/// C's recon picture has SB-extent stride so the straddle lands in place. Here
/// the stride stays aligned (`w`) and the slack absorbs a right-straddle write's
/// wrap; the caller crops the in-frame `w * h` region for `last_recon10_y`.
/// On a 64-aligned frame the extent equals the aligned dims, so the buffer and
/// every write are byte-identical to the pre-partial-SB pass.
#[allow(clippy::too_many_arguments)]
fn bd10_reencode_luma(
    all_trees: &mut [crate::partition::PartitionTree],
    sb_cols: usize,
    sb_size: usize,
    w: usize,
    h: usize,
    // The 10-bit SOURCE, padded to the SB extent at `src_stride` (the u16 twin
    // of `sb_input` / `in_stride`). A straddling leaf's residual gather reads
    // the full block width, so an ALIGNED-sized source would wrap into the next
    // row (right edge) or run past the plane (bottom right).
    src10: &[u16],
    src_stride: usize,
    base_qindex: u8,
    rdoq_level: u8,
    lambda_bd10: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    edge_filter: bool,
    bd: u8,
    qm_level: u8,
    // [SVT_HDR_MODE] fork loop_filter_sharpness (static_config.sharpness). 0 in
    // mainline → the quant table is byte-identical to build_quant_table_bd.
    sharpness: i8,
) -> crate::EncodeResult<alloc::vec::Vec<u16>> {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    let qt = crate::quant::build_quant_table_bd_sharp(base_qindex, bd, sharpness);
    let ext_w = w.div_ceil(sb_size) * sb_size;
    let ext_h = h.div_ceil(sb_size) * sb_size;
    // Seeded with the 10-bit DC default, NOT 0 — the seed the u8
    // `tile_frame_recon` (128) and the funnel's `tile_frame_recon10` (512)
    // both carry. The reason it is worth carrying: this buffer is now
    // SB-extent-SIZED, so `extract_neighbors_hbd`'s `idx < recon.len()` guard
    // admits slack-region indices that an ALIGNED-sized buffer rejected, and
    // rejecting meant "extend the last available sample" while admitting a
    // ZERO would mean predicting against black.
    // MEASURED byte-inert (2026-08-04) across the whole 198-cell partial-SB
    // eff-M9 grid — 0 of 198 cells changed verdict or byte count — so no read
    // reaches an unwritten cell today. Kept anyway: it costs nothing, it makes
    // the bd10 canvas agree with its u8 twin by construction instead of by
    // luck, and a `0` seed here is a silent wrong-pixels failure the moment one
    // does. (rust/CLAUDE.md: dead-looking translations stay, with the
    // measurement written down.)
    let mut recon10 = svtav1_types::try_vec![(128u16 << (bd - 8)); ext_w * ext_h]?;
    for (sb_idx, tree) in all_trees.iter_mut().enumerate() {
        let sb_col = sb_idx % sb_cols;
        let sb_row = sb_idx / sb_cols;
        bd10_reencode_node(
            sb_size / 4,
            tree,
            sb_col * sb_size,
            sb_row * sb_size,
            &mut recon10,
            w,
            src10,
            src_stride,
            &qt,
            rdoq_level,
            lambda_bd10,
            allintra_rd_mult,
            &rates,
            edge_filter,
            w,
            h,
            bd,
            qm_level,
        );
    }
    Ok(recon10)
}

#[allow(clippy::too_many_arguments)]
fn bd10_reencode_node(
    // C `seq_header.sb_mi_size` (16 SB64 / 32 SB128) — the intra
    // availability tables index by `mi & (sb_mi_size - 1)` (task #91).
    sb_mi_size: usize,
    tree: &mut crate::partition::PartitionTree,
    x: usize,
    y: usize,
    recon10: &mut [u16],
    stride: usize,
    src10: &[u16],
    src_stride: usize,
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    edge_filter: bool,
    frame_w: usize,
    frame_h: usize,
    bd: u8,
    qm_level: u8,
) {
    use crate::partition::PartitionTree as Tr;
    use crate::partition::PartitionType as PT;
    match tree {
        Tr::Leaf(d) => {
            let bw = d.width as usize;
            let bh = d.height as usize;
            assert_eq!(
                d.tx_depth, 0,
                "bd10 reencode: tx_depth {} not yet ported (DC-only first cell)",
                d.tx_depth
            );
            // Predict luma at 10-bit from the running 10-bit recon plane.
            let mut pred = alloc::vec![0u16; bw * bh];
            // Luma geom for directional prediction (ss=0; tx_depth 0 ⇒ tx==block,
            // row_off=col_off=0). filt_type is consulted only when edge_filter is
            // set, and the gate (`bd10_tree_supported`) admits directional leaves
            // ONLY when edge_filter is false — so 0 is inert here.
            let geom = crate::leaf_funnel::UnitGeom {
                mi_row: y >> 2,
                mi_col: x >> 2,
                bw_px: bw,
                bh_px: bh,
                sb_mi_size,
                ss: 0,
                frame_w,
                frame_h,
                // PORT-NOTE(task #96): the bd10 re-encode runs AFTER the
                // per-tile search merges, so it has no tile grid threaded and
                // treats the frame as one tile. Byte-neutral for every gated
                // bd10 cell (all single-tile).
                //
                // MEASURED CORRECTION (bd10 x tiles coverage, 2026-07-22): this
                // whole_frame TileMi is NOT the bd10 x multi-tile divergence
                // root. Threading per-tile bounds here was verified
                // BYTE-INERT on the diverging cells (stash "cov-combos:
                // byte-inert bd10 re-encode tile threading"). The actual root
                // is UPSTREAM: the port's eff-M9 partition search picks a
                // different tree at a tile boundary at bd10 (tree_diff on
                // gradient 256x256 q40 p10 r1c1: port keeps bsize 9 at the
                // y=128 tile-row-boundary SBs mi(32,16)/(32,48) where C — at
                // BOTH bit depths — splits to bsize 6; the port matches C at
                // bd8 tiles and at bd10 single-tile). See
                // docs/coverage-combos-map.md (axis "bd10 x tiles").
                tile: crate::intra_edge::TileMi::whole_frame(frame_w, frame_h),
            };
            crate::leaf_funnel::predict_unit_hbd(
                recon10,
                stride,
                x,
                y,
                bw,
                bh,
                d.intra_mode,
                d.angle_delta,
                d.filter_intra_mode,
                &geom,
                edge_filter,
                0,
                &mut pred,
                bd,
            );
            let src_off = y * src_stride + x;
            // RDOQ contexts are 0/0 at eff-M9 (rate_est_level 0).
            let out = crate::leaf_funnel::tx_unit_hbd(
                src10,
                src_stride,
                src_off,
                &pred,
                bw,
                0,
                bw,
                bh,
                d.tx_type as usize,
                0, // luma plane
                0, // txb_skip_ctx
                0, // dc_sign_ctx
                qt,
                rdoq_level,
                lambda,
                0, // sharpness
                allintra_rd_mult,
                rates,
                rdoq_level != 0,
                bd,
                qm_level,
                None, // level-only re-encode: no RD terms
            );
            // Overwrite the coded LUMA levels with the 10-bit result. The walk
            // re-derives the scan-order eob + skip from these coeffs.
            //
            // `out.qcoeff` is the TIGHT (32-capped) packed txb at stride pw; the
            // entropy walk (pipeline.rs `tx_depth==0` arm) — like the u8
            // `funnel_block_decision` (partition.rs) — expects `d.qcoeffs` as a
            // full w*h raster at stride w, from which it re-packs the low-freq
            // quadrant. Re-expand so 64-dim transforms (pw<w) don't read past
            // the tight buffer (was: a 64x64 DC leaf at high qindex panicked in
            // the walk's stride-w pack).
            let (pw, ph) = (bw.min(32), bh.min(32));
            let mut full = alloc::vec![0i32; bw * bh];
            for r in 0..ph {
                full[r * bw..r * bw + pw].copy_from_slice(&out.qcoeff[r * pw..r * pw + pw]);
            }
            d.qcoeffs = full;
            d.eob = out.eob;
            // Write the 10-bit recon back for neighbour prediction of the next
            // block in decode order.
            //
            // STRADDLE CLIP (task #94 partial-SB) — the same rule `commit_leaf`
            // applies to the funnel's canvases: a boundary leaf whose width
            // reaches past the ALIGNED extent would spill past the row boundary
            // and, this buffer being SB-extent-sized but aligned-strided, WRAP
            // into the next row's low columns, corrupting an already-committed
            // neighbour that a later block predicts from. Nothing ever READS
            // past the aligned extent, so clipping the write matches C's
            // readable recon exactly, and it is a no-op wherever
            // `x + bw <= stride` (every 64-aligned frame).
            let wr = bw.min(stride.saturating_sub(x));
            for r in 0..bh {
                let drow = (y + r) * stride + x;
                recon10[drow..drow + wr].copy_from_slice(&out.recon[r * bw..r * bw + wr]);
            }
        }
        Tr::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let nw = *width as usize;
            let nh = *height as usize;
            let hw = nw / 2;
            let hh = nh / 2;
            let qw = nw / 4;
            let qh = nh / 4;
            // Child origins, derived EXACTLY the way `encode_partition_tree`
            // derives them (the pack walk), because on a partial SB the child
            // list is no longer a fixed length:
            //   * SPLIT walks the four quadrant SLOTS and SKIPS any whose
            //     ORIGIN is outside the aligned frame, pulling the packed
            //     children in order. Zipping a pruned list against the full
            //     offset table mis-places them — a right-edge-only prune leaves
            //     [q0, q2] and would put the BOTTOM-LEFT child at the
            //     TOP-RIGHT offset.
            //   * HORZ/VERT may carry a single in-frame child (C codes block 1
            //     only if `mi_row + hbs < mi_rows`, entropy_coding.c:5490).
            //   * the extended shapes drop children from the TAIL, so a
            //     zip against the full list still pairs correctly.
            // The previous `(partition_type, children.len())` match would have
            // `panic!`ed on every one of those shapes.
            let mut recurse = |child: &mut crate::partition::PartitionTree, cx, cy| {
                bd10_reencode_node(
                    sb_mi_size,
                    child,
                    cx,
                    cy,
                    recon10,
                    stride,
                    src10,
                    src_stride,
                    qt,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    edge_filter,
                    frame_w,
                    frame_h,
                    bd,
                    qm_level,
                );
            };
            match *partition_type {
                PT::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= frame_w || cy >= frame_h {
                            continue;
                        }
                        recurse(&mut children[ci], cx, cy);
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "bd10 reencode: in-frame quadrant count must equal the packed child count"
                    );
                }
                PT::Horz => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(bot) = rest.first_mut() {
                        recurse(bot, x, y + hh);
                    }
                }
                PT::Vert => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(right) = rest.first_mut() {
                        recurse(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PT::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PT::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PT::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PT::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PT::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PT::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => panic!("bd10 reencode: unsupported partition {other:?}"),
                    };
                    for (child, &(dx, dy)) in children.iter_mut().zip(offs) {
                        recurse(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

/// bd10 CHROMA re-encode (task #94). The luma re-encode (`bd10_reencode_luma`)
/// recomputes only luma levels; chroma stays at the u8 MD decision
/// (`chroma_dec`). For content whose CHROMA has a coded residual (e.g. the
/// `diag` diagonal edge — its subsampled chroma is NOT flat), the u8 chroma
/// levels diverge from C's bd10 chroma quant: C's higher-precision chroma
/// prediction (the ~+20/px hbd-predictor rounding) yields a small DC residual
/// that quantizes to ±1 at bd10 where the MSB-truncated u8 path rounds to 0.
/// Decode-both localization proved the LUMA plane is already byte-identical
/// (`bd10_reencode_luma`) and every chroma divergence is exactly this (port
/// codes flat 512 where C codes a coded 511). This walk mirrors the luma pass
/// on the U and V planes: predict at bd10 (`predict_unit_hbd` on the running
/// bd10 chroma recon), residual/tx/quant at bd10 (`tx_unit_hbd`, plane 1, the
/// derived `uv_tx_type` + the bd10 chroma quant table), then OVERWRITE
/// `chroma_dec` with the bd10 levels/eob. Gated to complete-SB, in-envelope
/// trees (`bd10_tree_supported`, which now also rejects CfL / directional-uv-
/// with-edge-filter); flat-chroma content (gradient/uniform) re-encodes to the
/// SAME zero-coefficient result, so bd8 and the existing bd10 gate cells stay
/// byte-unchanged. The stored u8 recon in `chroma_dec` is inert (the walk only
/// copies it into the u8 chroma plane, which no `chroma_dec` block reads).
#[allow(clippy::too_many_arguments)]
fn bd10_reencode_chroma(
    all_trees: &mut [crate::partition::PartitionTree],
    sb_cols: usize,
    sb_size: usize,
    w: usize,
    h: usize,
    // The 10-bit CHROMA source, in the SB-extent shape `sb_chroma_owned` has
    // (aligned stride `cstride`, extra edge-replicated rows) so a straddling
    // block's residual gather stays in bounds.
    u_src10: &[u16],
    v_src10: &[u16],
    cstride: usize,
    // The frame's 10-bit LUMA recon from `bd10_reencode_luma` — the SB-EXTENT
    // canvas at stride `y_stride`, not the cropped `w*h`. It is the CfL AC
    // source for UV_CFL_PRED leaves, and `cfl_ac_from_frame_recon_hbd` reads
    // `max(bh, 8)` rows from the block origin, which straddles on a partial SB.
    y_recon10: &[u16],
    y_stride: usize,
    // Frame-level chroma qindex (== base_qindex) — sources ONLY the coeff-rate
    // context (`cfc`), which C builds once per frame from base_qindex (never
    // per plane). The per-plane quant TABLES use qindex_u/qindex_v below.
    chroma_qindex: u8,
    // [SVT_HDR_MODE] per-plane chroma quant qindex = base_qindex + the FH
    // u_ac/v_ac delta (chroma_q.rs / pipeline qindex_u/qindex_v). C dequantizes
    // chroma with the signaled per-plane deltas (separate_uv_delta_q=1), and the
    // bd8 walk already quantizes U/V at these qindices — the bd10 chroma
    // re-encode MUST too, or a small residual that survives at the finer plane
    // qindex is dropped at base (the diag q5 Cr off-by-one: V_PRED predicts the
    // no-neighbour default 511, source is flat 512, so +1/px; at qindex_v it
    // codes, at base it rounds to 0 -> the port codes 511 where C codes 512).
    // Using base for both also DESYNCS the port's own chroma recon from its
    // signaled bitstream (the decoder dequantizes at qindex_v). Mainline: both
    // == base_qindex (all FH chroma deltas 0) -> byte-inert.
    qindex_u: u8,
    qindex_v: u8,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    edge_filter: bool,
    bd: u8,
    // [SVT_HDR_MODE] per-plane QM levels [U, V] (15 = off). C derives them
    // separately via `aom_get_qmlevel(base_qindex + delta_q_ac[plane], ...)`
    // (md_config_process.c:271-279), so they can differ between Cb and Cr —
    // the fork's chroma path gives Cb a +12 delta.
    qm_uv: [u8; 2],
    // [SVT_HDR_MODE] fork loop_filter_sharpness (static_config.sharpness). 0 in
    // mainline → byte-identical to build_quant_table_bd. C applies the same
    // qzbin/qround sharpening to the chroma quantizer rows (u/v_zbin/round).
    sharpness: i8,
) -> crate::EncodeResult<(alloc::vec::Vec<u16>, alloc::vec::Vec<u16>)> {
    let fc = crate::entropy::context::FrameContext::new_default();
    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(chroma_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    // Per-plane chroma quant tables (== each other, and == the old single
    // base-qindex table, whenever the FH chroma deltas are 0 -> mainline inert).
    let qt_u = crate::quant::build_quant_table_bd_sharp(qindex_u, bd, sharpness);
    let qt_v = crate::quant::build_quant_table_bd_sharp(qindex_v, bd, sharpness);
    let (cframe_w, cframe_h) = (w / 2, h / 2);
    // SB-extent-sized, ALIGNED-strided — the chroma twin of the luma canvas
    // above (and of `fun_u_recon` / `fun_v_recon` in the funnel). The caller
    // crops the in-frame `cframe_w * cframe_h` region.
    let ext_cbuf = (w.div_ceil(sb_size) * sb_size / 2) * (h.div_ceil(sb_size) * sb_size / 2);
    // Seeded with the 10-bit DC default like the luma canvas above (and like
    // the funnel's `fun_u_recon` / `fun_v_recon`, which are 128u8) — see the
    // note there for why 0 is wrong once the buffer is SB-extent-sized.
    let seed: u16 = 128u16 << (bd - 8);
    let mut recon10_u = svtav1_types::try_vec![seed; ext_cbuf]?;
    let mut recon10_v = svtav1_types::try_vec![seed; ext_cbuf]?;
    for (sb_idx, tree) in all_trees.iter_mut().enumerate() {
        let sb_col = sb_idx % sb_cols;
        let sb_row = sb_idx / sb_cols;
        bd10_reencode_chroma_node(
            sb_size / 4,
            tree,
            sb_col * sb_size,
            sb_row * sb_size,
            &mut recon10_u,
            &mut recon10_v,
            cstride,
            u_src10,
            v_src10,
            y_recon10,
            y_stride,
            &qt_u,
            &qt_v,
            rdoq_level,
            lambda,
            allintra_rd_mult,
            &rates,
            edge_filter,
            cframe_w,
            cframe_h,
            bd,
            qm_uv,
        );
    }
    // The frame's true 10-bit CHROMA recon — the post-MD canvas the bd10
    // post-filter chain (deblock -> CDEF search -> LR search) reads, the
    // chroma twin of `bd10_reencode_luma`'s return. C keeps the same thing
    // in the 16-bit recon picture (`svt_aom_get_recon_pic(.., is_16bit)`).
    Ok((recon10_u, recon10_v))
}

/// Re-encode ONE chroma plane's leaf at bd10: predict -> residual/tx/quant ->
/// recon, writing the bd10 recon back into `recon10` for neighbour prediction.
/// Returns `(qcoeff raster, eob, u8-recon)`. `uv_tt`/geom/edge params mirror the
/// walk's chroma coding (`write_chroma_txb`, `uv_tx_type`). The u8 recon is a
/// sane truncation (`>> (bd-8)`) — it is inert (see `bd10_reencode_chroma`).
#[allow(clippy::too_many_arguments)]
fn bd10_reencode_chroma_plane(
    recon10: &mut [u16],
    src10: &[u16],
    cstride: usize,
    cx: usize,
    cy: usize,
    cw: usize,
    ch: usize,
    uv_mode: u8,
    uv_angle_delta: i8,
    uv_tt: usize,
    geom: &crate::leaf_funnel::UnitGeom,
    edge_filter: bool,
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    bd: u8,
    qm_level: u8,
    // `Some((ac_luma_q3, alpha_q3))` for a UV_CFL_PRED leaf. C predicts CfL as
    // `svt_cfl_predict_hbd(pred_buf_q3, dc_pred, alpha)` over a **DC** base
    // (`cfl_prediction` regenerates DC at :3798-3801 before calling), so the
    // mode passed to `predict_unit_hbd` is forced to UV_DC_PRED here.
    cfl: Option<(&[i16], i32)>,
) -> (alloc::vec::Vec<i32>, u16, alloc::vec::Vec<u8>) {
    let mut pred = alloc::vec![0u16; cw * ch];
    crate::leaf_funnel::predict_unit_hbd(
        recon10,
        cstride,
        cx,
        cy,
        cw,
        ch,
        if cfl.is_some() { 0 } else { uv_mode },
        if cfl.is_some() { 0 } else { uv_angle_delta },
        crate::leaf_funnel::FI_NONE,
        geom,
        edge_filter,
        0,
        &mut pred,
        bd,
    );
    if let Some((ac, alpha_q3)) = cfl {
        let dc = pred.clone();
        svtav1_dsp::hbd::cfl_predict_hbd(ac, &dc, cw, &mut pred, cw, alpha_q3, bd, cw, ch);
    }
    let src_off = cy * cstride + cx;
    let out = crate::leaf_funnel::tx_unit_hbd(
        src10,
        cstride,
        src_off,
        &pred,
        cw,
        0,
        cw,
        ch,
        uv_tt,
        1, // chroma plane
        0, // txb_skip_ctx (eff-M9 rate_est_level 0)
        0, // dc_sign_ctx
        qt,
        rdoq_level,
        lambda,
        0, // sharpness
        allintra_rd_mult,
        rates,
        rdoq_level != 0,
        bd,
        qm_level,
        None, // level-only re-encode: no RD terms
    );
    // Straddle clip — see the luma twin in `bd10_reencode_node`. A no-op
    // wherever `cx + cw <= cstride`.
    let cwr = cw.min(cstride.saturating_sub(cx));
    for r in 0..ch {
        let drow = (cy + r) * cstride + cx;
        recon10[drow..drow + cwr].copy_from_slice(&out.recon[r * cw..r * cw + cwr]);
    }
    let shift = (bd - 8) as u32;
    let rec_u8: alloc::vec::Vec<u8> = out
        .recon
        .iter()
        .map(|&s| (s >> shift).min(255) as u8)
        .collect();
    (out.qcoeff, out.eob, rec_u8)
}

#[allow(clippy::too_many_arguments)]
fn bd10_reencode_chroma_node(
    // C `seq_header.sb_mi_size` (16 SB64 / 32 SB128), task #91.
    sb_mi_size: usize,
    tree: &mut crate::partition::PartitionTree,
    x: usize,
    y: usize,
    recon10_u: &mut [u16],
    recon10_v: &mut [u16],
    cstride: usize,
    u_src10: &[u16],
    v_src10: &[u16],
    y_recon10: &[u16],
    y_stride: usize,
    // Per-plane chroma quant tables (base + FH u_ac / v_ac delta). Equal in
    // mainline (deltas 0) -> byte-inert.
    qt_u: &crate::quant::QuantTable,
    qt_v: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
    // C `scs->allintra || scs->static_config.rtc` — the RDOQ plane rate
    // weight arm (`crate::quant::PLANE_RD_MULT`). FALSE on a video frame.
    allintra_rd_mult: bool,
    rates: &crate::leaf_funnel::MdRates,
    edge_filter: bool,
    cframe_w: usize,
    cframe_h: usize,
    bd: u8,
    qm_uv: [u8; 2],
) {
    use crate::partition::PartitionTree as Tr;
    use crate::partition::PartitionType as PT;
    match tree {
        Tr::Leaf(d) => {
            let bw = d.width as usize;
            let bh = d.height as usize;
            // Chroma reference? (walk `blk_has_uv`, pipeline.rs). With the
            // min-8x8 luma policy every leaf is a reference; kept for safety.
            let bw_mi = bw / 4;
            let bh_mi = bh / 4;
            let has_uv = ((y / 4) % 2 == 1 || bh_mi.is_multiple_of(2))
                && ((x / 4) % 2 == 1 || bw_mi.is_multiple_of(2));
            if !has_uv {
                return;
            }
            // Chroma origin/dims — EXACTLY the walk's derivation.
            let cw = bw.max(8) / 2;
            let ch = bh.max(8) / 2;
            let cx = ((x >> 3) << 3) / 2 + if bw >= 8 { (x % 8) / 2 } else { 0 };
            let cy = ((y >> 3) << 3) / 2 + if bh >= 8 { (y % 8) / 2 } else { 0 };
            // UV_CFL_PRED: C's chroma tx_type is forced to DCT_DCT
            // (`cfl_prediction` :3796, `transform_type_uv = DCT_DCT`), and the
            // prediction comes from the 10-bit LUMA recon rather than the
            // chroma neighbours. `uv_tx_type` already maps mode 13 -> DCT_DCT,
            // so only the prediction changes.
            let uv_tt = crate::leaf_funnel::uv_tx_type(d.uv_mode, cw, ch);
            let cfl_ac: Option<alloc::vec::Vec<i16>> = if d.uv_mode == 13 {
                let mut ac = alloc::vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * ch.max(1)];
                crate::leaf_funnel::cfl_ac_from_frame_recon_hbd(
                    y_recon10, y_stride, x, y, bw, bh, cw, ch, &mut ac,
                );
                Some(ac)
            } else {
                None
            };
            let cfl_u = cfl_ac.as_ref().map(|ac| {
                (
                    &ac[..],
                    crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 0),
                )
            });
            let cfl_v = cfl_ac.as_ref().map(|ac| {
                (
                    &ac[..],
                    crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 1),
                )
            });
            let geom = crate::leaf_funnel::UnitGeom {
                mi_row: cy >> 2,
                mi_col: cx >> 2,
                bw_px: cw,
                bh_px: ch,
                sb_mi_size,
                ss: 0,
                frame_w: cframe_w,
                frame_h: cframe_h,
                // PORT-NOTE(task #96): see the luma twin above — bd10
                // re-encode is post-merge and frame-scoped. The MEASURED
                // CORRECTION there applies here too: whole_frame is NOT the
                // bd10 x tiles root (threading was byte-inert); the partition
                // search is. docs/coverage-combos-map.md.
                tile: crate::intra_edge::TileMi::whole_frame(cframe_w, cframe_h),
            };
            let (u_q, u_eob, u_rec) = bd10_reencode_chroma_plane(
                recon10_u,
                u_src10,
                cstride,
                cx,
                cy,
                cw,
                ch,
                d.uv_mode,
                d.uv_angle_delta,
                uv_tt,
                &geom,
                edge_filter,
                qt_u,
                rdoq_level,
                lambda,
                allintra_rd_mult,
                rates,
                bd,
                qm_uv[0],
                cfl_u,
            );
            let (v_q, v_eob, v_rec) = bd10_reencode_chroma_plane(
                recon10_v,
                v_src10,
                cstride,
                cx,
                cy,
                cw,
                ch,
                d.uv_mode,
                d.uv_angle_delta,
                uv_tt,
                &geom,
                edge_filter,
                qt_v,
                rdoq_level,
                lambda,
                allintra_rd_mult,
                rates,
                bd,
                qm_uv[1],
                cfl_v,
            );
            d.chroma_dec = Some((u_q, v_q, u_eob, v_eob, u_rec, v_rec));
        }
        Tr::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let nw = *width as usize;
            let nh = *height as usize;
            let hw = nw / 2;
            let hh = nh / 2;
            let qw = nw / 4;
            let qh = nh / 4;
            // Identical child-origin derivation to the luma twin — see the long
            // note in `bd10_reencode_node`. `x`/`y` here are LUMA coordinates
            // (the chroma origin is derived per leaf), so the in-frame test uses
            // the LUMA frame extent, which is `cframe_* * 2`.
            let (lframe_w, lframe_h) = (cframe_w * 2, cframe_h * 2);
            let mut recurse = |child: &mut crate::partition::PartitionTree, cx, cy| {
                bd10_reencode_chroma_node(
                    sb_mi_size,
                    child,
                    cx,
                    cy,
                    recon10_u,
                    recon10_v,
                    cstride,
                    u_src10,
                    v_src10,
                    y_recon10,
                    y_stride,
                    qt_u,
                    qt_v,
                    rdoq_level,
                    lambda,
                    allintra_rd_mult,
                    rates,
                    edge_filter,
                    cframe_w,
                    cframe_h,
                    bd,
                    qm_uv,
                );
            };
            match *partition_type {
                PT::Split => {
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = x + (i & 1) * hw;
                        let cy = y + (i >> 1) * hh;
                        if cx >= lframe_w || cy >= lframe_h {
                            continue;
                        }
                        recurse(&mut children[ci], cx, cy);
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "bd10 chroma reencode: in-frame quadrant count must equal the packed \
                         child count"
                    );
                }
                PT::Horz => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(bot) = rest.first_mut() {
                        recurse(bot, x, y + hh);
                    }
                }
                PT::Vert => {
                    let (first, rest) = children.split_at_mut(1);
                    recurse(&mut first[0], x, y);
                    if let Some(right) = rest.first_mut() {
                        recurse(right, x + hw, y);
                    }
                }
                ext => {
                    let offs: &[(usize, usize)] = match ext {
                        PT::HorzA => &[(0, 0), (hw, 0), (0, hh)],
                        PT::HorzB => &[(0, 0), (0, hh), (hw, hh)],
                        PT::VertA => &[(0, 0), (0, hh), (hw, 0)],
                        PT::VertB => &[(0, 0), (hw, 0), (hw, hh)],
                        PT::Horz4 => &[(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                        PT::Vert4 => &[(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                        other => panic!("bd10 chroma reencode: unsupported partition {other:?}"),
                    };
                    for (child, &(dx, dy)) in children.iter_mut().zip(offs) {
                        recurse(child, x + dx, y + dy);
                    }
                }
            }
        }
    }
}

/// Recursive leaf printer for `SVTAV1_DUMP_TREE` (coding order).
#[cfg(feature = "std")]
fn dump_tree_leaves(tree: &crate::partition::PartitionTree, x: usize, y: usize) {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => {
            eprintln!(
                "LEAF x{:4} y{:4} {}x{} mode {:2} uv {:2} tx {} eob {} txd {}",
                x, y, d.width, d.height, d.intra_mode, d.uv_mode, d.tx_type, d.eob, d.tx_depth
            );
        }
        crate::partition::PartitionTree::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let (w, h) = (*width as usize, *height as usize);
            let (hw, hh, qw, qh) = (w / 2, h / 2, w / 4, h / 4);
            use crate::partition::PartitionType as P;
            let offs: alloc::vec::Vec<(usize, usize)> = match partition_type {
                P::Split => alloc::vec![(0, 0), (hw, 0), (0, hh), (hw, hh)],
                P::Horz => alloc::vec![(0, 0), (0, hh)],
                P::Vert => alloc::vec![(0, 0), (hw, 0)],
                P::HorzA => alloc::vec![(0, 0), (hw, 0), (0, hh)],
                P::HorzB => alloc::vec![(0, 0), (0, hh), (hw, hh)],
                P::VertA => alloc::vec![(0, 0), (0, hh), (hw, 0)],
                P::VertB => alloc::vec![(0, 0), (hw, 0), (hw, hh)],
                P::Horz4 => alloc::vec![(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                P::Vert4 => alloc::vec![(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                P::None => alloc::vec![(0, 0)],
            };
            eprintln!("SPLIT x{x:4} y{y:4} {w}x{h} {partition_type:?}");
            for (child, (dx, dy)) in children.iter().zip(offs) {
                dump_tree_leaves(child, x + dx, y + dy);
            }
        }
    }
}

/// Is the bd10 FULL-RD mode funnel (MDS1 + MDS3 at true depth) usable for this
/// frame? (task #94, MODE axis — docs/bd10-port-map.md.)
///
/// Below eff-M9 the coded mode is the MDS1/MDS3 full-RD winner rather than the
/// MDS0 survivor, so a bd10 MDS0 alone closes nothing (measured). When this is
/// on, `evaluate_leaf` runs the whole full-RD chain — luma depth loop with
/// TXS/TXT, and the chroma loop — on 10-bit pixels with the bd10 quant tables
/// and `full_lambda_md[EB_10_BIT_MD]`, and the winner's 10-bit levels ARE the
/// coded ones (so the level-only re-encode post-pass is skipped: it hardcodes
/// RDOQ contexts 0/0, which is only correct where `real_coeff_ctx` is off).
///
/// Scope, deliberately narrow:
/// - **presets 0..=8**. p6..=8 was the MODE axis (landed first). p0..=5 take the
///   PD1 depth-refine + NSQ walk (`decide_sb_refined`), which is the **PART**
///   axis: C's PD1 runs at `hbd_md = 2`, so `test_depth` /
///   `test_split_partition` sum 10-bit MDS3 leaf costs when choosing the shape
///   and the depth. Feeding that walk 8-bit leaf costs picked C's *bd8*
///   geometry. LOCALIZED (docs/bd10-port-map.md): at p0..p2 C's PD0 pass is
///   bit-depth-IDENTICAL (bd8 `pic_pd0_lvl == 0` and bd10 forces `PD0_LVL_0`,
///   both run at `hbd_md = 0` on the MSB-truncated plane — measured
///   byte-identical `SVT_PD0COST_OUT` dumps), and the depth-refinement gates
///   also run inside C's `hbd_md = 0` window (enc_dec_process.c:2965 forces 0,
///   :3023 restores AFTER the :3017 refinement call), so the ONLY bit-depth
///   input to the geometry is the PD1 leaf cost.
///   eff-M9 (p9..p13) is CLOSED via the MDS0 funnel + post-pass and is left
///   EXACTLY as it is; widening to it is a follow-up that must be re-verified
///   against the whole gate.
/// - **any SB geometry, complete or partial** (2026-08-04). This used to be
///   complete-SB-only on the stated grounds that `tx_unit_hbd` is not
///   partial-SB-aware; that was MEASURED WRONG. `tx_unit_hbd` takes explicit
///   `(w, h, stride, off)` and its only geometry-sensitive term is
///   `TxRdArgs::crop`, which is the bd10 TWIN of the u8 cropped-TX distortion
///   and is already fed the same `blk_crop`/`uv_crop` (leaf_funnel.rs). The
///   real partial-SB machinery — the PD0 edge predicates, the edge-aware PD1
///   depth-refinement walk, the one-false shape injection, the SB-extent recon
///   canvases and `commit_leaf`'s straddle clip — is SHARED with the u8 path,
///   which is 36/36 at partial SB. So the bd10 full-RD funnel inherits it.
/// - **palette off** — a palette candidate has no 10-bit prediction here.
///
/// CfL is handled inside `evaluate_leaf` instead of here, because whether it is
/// reachable is a per-block runtime property (the chroma complexity detector),
/// not a config one: under the bd10 full-RD the CfL candidate is not offered,
/// which leaves a CfL block as a VISIBLE mode divergence rather than a
/// mixed-domain compare. See the comment at the `cfl_gate` site.
fn bd10_full_rd_supported(
    bit_depth: u8,
    preset: u8,
    chroma_420: bool,
    _w: usize,
    _h: usize,
) -> bool {
    // `chroma_420` is load-bearing, not decoration: the funnel this gate
    // enables is only ever constructed when `use_funnel` holds, and that
    // requires 4:2:0. Without this term the gate returned TRUE for a
    // monochrome bd10 frame at preset <= 8 — which then suppressed the level
    // post-pass (`bd10_postpass_runs = !bd10_full_rd`) while the funnel it
    // claimed to be deferring to never ran, leaving the whole encode in the
    // 8-bit domain under a 10-bit sequence header.
    //
    // The former `FunnelCfg::for_preset(preset).palette_level == 0` term is
    // gone: `for_preset` returns the constant 0 (leaf_funnel.rs), the real
    // per-frame level being stamped later from `sc_detect`, so the clause was
    // a compile-time tautology that read like a screen-content precondition.
    // Palette at bd10 is now handled inside the funnel (see
    // `search_palette_luma_hbd`), so no such precondition is needed.
    bit_depth == 10 && preset <= 8 && chroma_420
}

#[allow(clippy::type_complexity)] // ported C signature: a `type` alias here would hide the shape and churn the byte-identity gate for no benefit
fn encode_tile_rows(
    encode_input: &[u8],
    // Task #95 chunk 2: source padded to the SB extent (== `encode_input` for
    // full-SB frames) + its stride. The PD0 partition search and per-b64
    // variance read from THIS buffer so a partial SB sees C's replicated
    // border instead of stride-wrapping into the next row.
    sb_input: &[u8],
    in_stride: usize,
    w: usize,
    h: usize,
    sb_size: usize,
    sb_cols: usize,
    sb_rows: usize,
    // Task #96: the resolved tile grid — the SAME value the entropy walk
    // and the frame header use, so the MD search, the coded symbols and
    // the signalled geometry can never disagree about where the tile
    // boundaries are.
    tile_grid: crate::entropy::obu::TileGrid,
    base_qindex: u8,
    // Per-plane chroma qindexes (== base_qindex in mainline mode).
    qindex_u: u8,
    qindex_v: u8,
    // Effective AC bias for MD spatial distortion (0.0 = mainline default).
    ac_bias_eff: f64,
    // [SVT_HDR_MODE] per-SB qindex plan (variance boost) + frame chroma
    // AC deltas: the search must quantize each SB at its planned qindex.
    sb_qindex_plan: Option<&[u8]>,
    chroma_ac_deltas: (i8, i8),
    sharp_tx_active: bool,
    hdr_noise_norm: u8,
    qm_levels: [u8; 3],
    hdr_tx_bias: u8,
    hdr_complex_hvs: bool,
    // The preset the SCREEN-CONTENT detector runs at, resolved by the CALLER.
    //
    // Not `speed_config.preset`: tune-IQ forces `screen_content_mode = 3` at
    // every preset (enc_handle.c:4914), and the port models that force by
    // clamping the detector's preset to 7 (the highest preset where scm-3
    // auto-detection is live, enc_handle.c:4641-4651). The frame side already
    // did that; this side used the RAW preset, so at preset >= 8 under tune-IQ
    // the two disagreed -- the frame header and the real pack coded per-block
    // palette flags from the frame derivation while the funnel priced ZERO bits
    // for them and injected no palette candidate, and both simulated CDF
    // contexts drifted from the pack. Passing the resolved value makes the
    // tile-side comment below ("identical inputs -> identical result") true.
    sc_preset: u8,
    // Which arm of `enc_mode_config.c` this frame's screen-content derivation
    // is on (C `scs->allintra`), resolved by the CALLER for the same reason
    // `sc_preset` is: the funnel's derivation must be bit-for-bit the frame
    // header's, or the simulated CDF chains desync from the real pack.
    sc_arm: crate::sc_detect::ScArm,
    hdr_alt_ssim: bool,
    hdr_alt_lambda: bool,
    hdr_iq_lambda_weight: Option<u32>,
    ssim_factors: Option<&(alloc::vec::Vec<f64>, usize, usize)>,
    fh_base_qindex: u8,
    // FH `tx_mode == TX_MODE_SELECT` — the value `frame_tx_mode_select()`
    // computed for the header, PASSED IN rather than re-derived here. The
    // funnel walk and the per-SB CDF-chain simulation must code exactly the
    // symbols the header announced, and re-deriving would key the qp band on
    // this function's `cli_qp` (== `tpl_adjusted_qp`) where the header keys it
    // on `static_config.qp`: equal at `aq_mode == 0`, not guaranteed
    // otherwise, and a disagreement there is an undecodable stream.
    walk_tx_mode_select: bool,
    cli_qp: u8,
    // C `ppcs->picture_qp = clamp_qp((base_q_idx + 2) >> 2)` (rc_process.c:861)
    // — the qp the frame `lambda_weight` ladder is keyed on. Equal to `cli_qp`
    // unless a fractional CRF put a non-zero `extended_crf_qindex_offset` into
    // the qindex; every LEVEL derivation keeps reading `cli_qp`
    // (`static_config.qp`), which the offset does not move.
    picture_qp: u8,
    // C's extended-CRF lambda bump: `lambda_weight += extended_crf_qindex_offset
    // * 28` when `static_config.qp == MAX_QP_VALUE (63)` and the offset is
    // non-zero (enc_mode_config.c:10109-10114) — i.e. CRF 63.25..70 only.
    // 0 on every other config, which makes it byte-inert there.
    lw_bump: u32,
    // C `static_config.tune == TUNE_IQ`: picks the still-picture
    // `lambda_weight` curve over the PSNR ladder (enc_mode_config.c:10094).
    tune_iq: bool,
    hdr_sharpness: i8,
    _lambda: u64, // Per-SB lambda computed from sb_qp_offsets
    speed_config: &crate::speed_config::SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    // The same reference luma plane with C's replicated margin
    // (docs/INTER-ENCODE-PLAN.md §1s item 4). `Some` exactly when
    // `ref_frame_data` is; the inter arm cannot predict without it.
    ref_padded_y: Option<&crate::picture::PaddedPlane>,
    // The INTER branch of mode decision (`docs/INTER-ENCODE-PLAN.md` §1s
    // items 1b/2/3/6): the padded DPB reference, this frame's open-loop
    // motion search, the inter rate tables and the MVP environment. `None`
    // on a key frame.
    inter_md: Option<&crate::inter_md_arm::InterMdFrame<'_>>,
    // C `pcs->md_frame_context` (`init_frame_rate_tables`,
    // md_config_process.c:292-310) — §1s item 8. When the frame header names
    // a `primary_ref_frame`, MODE DECISION prices against THAT REFERENCE's
    // saved end-of-frame CDFs, not the defaults. `None` reproduces C's
    // `PRIMARY_REF_NONE` arm: `svt_av1_default_coef_probs(base_q_idx)` +
    // `svt_aom_init_mode_probs`, which is what the still path has always
    // built.
    md_frame_cdfs: Option<&crate::port_frame_cdf::FrameCdfs>,
    mv_map: &[svtav1_types::motion::Mv],
    mv_map_stride: usize,
    sb_qp_offsets: &[i8],
    chroma_420: bool,
    c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
    chroma_src: Option<(&[u8], &[u8])>,
    // Encode bit depth (8 or 10). At bd10 the partition search runs C's
    // hbd-forced PD0_LVL_0 (full-RD), NOT the preset's LVL_6/LVL_5 heuristic
    // (set_pd0_ctrls, enc_mode_config.c:5415). The tree is still decided at
    // 8-bit on the MSB-truncated plane; only the coded levels go 10-bit
    // (bd10_reencode_luma).
    bit_depth: u8,
    // Task #6 chunk 1: native 10-bit SOURCE planes (Y, U, V) for the bd10 MD
    // funnel — frame-strided over the ALIGNED frame (`w`, `w/2`). `Some` only
    // when the caller entered through `try_encode_frame_420_hbd`; every u8
    // path passes `None` and the funnel keeps widening `u8 << 2`.
    hbd_src: Option<(&[u16], &[u16], &[u16])>,
    // Set when the funnel actually consumed `hbd_src` (i.e. the bd10 luma
    // funnel armed). Read by `encode_frame_impl` to reject an encode that
    // would have silently dropped the caller's low bits.
    hbd_used: &core::sync::atomic::AtomicBool,
    // Superres chunk B.4: C's `pcs->variance` as picture analysis left it
    // (FULL-RESOLUTION b64 grid, raster order), read here through the CODED
    // grid's linear SB index — the indexing C itself does after
    // `scale_pcs_params` re-inits the geometry without rebuilding the array.
    // `None` on every non-superres path.
    stale_vars: Option<&[crate::pd0::SbVariance]>,
    // C `static_config.max_tx_size` (32 or 64) — tune IQ sets 32 at qp <= 45
    // (enc_handle.c:4914) and the partition search then refuses 64x64 squares
    // (enc_dec_process.c:1494-1495).
    max_tx_size: u8,
    // Issue #5: the frame is CODED-LOSSLESS (base_q_idx 0, spec 5.9.2). Selects
    // the forced 8x8 / TX_4X4 partition tree (`pd0::lossless_tree`) and the
    // funnel's lossless arms (`FunnelFrame::coded_lossless`,
    // `FunnelCfg::apply_coded_lossless`).
    coded_lossless: bool,
    // Feature 4 (bounded threading): the maximum number of OS threads the
    // tile loop below may run at once (0 = auto via `available_parallelism`).
    // Bounds CONCURRENCY only — tiles are always joined and appended in
    // tile-index order — so the returned per-tile results (and the emitted
    // bytes) are identical for any value.
    thread_count: usize,
    // Feature 1: cooperative cancellation, checked at the head of every SB
    // row of the MD search (the heaviest per-frame loop). Passed as
    // `&dyn Stop` so the threaded per-tile closure stays `Send` (the trait
    // is `Send + Sync`); the default `Unstoppable` token's `may_stop()` is
    // `false`, so the guarded check compiles to a cheap false-branch and the
    // search output stays byte-identical.
    stop: &dyn enough::Stop,
) -> crate::EncodeResult<
    Vec<(
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        // bd10 FULL-RD only: this tile's committed 10-bit winner recon, as
        // SB-extent-SIZED but ALIGNED-STRIDED (`w` / `w/2`) Y/U/V canvases with
        // only this tile's SB region written. The extra size absorbs a
        // right-straddle write's wrap; the stride is the aligned width, exactly
        // like the u8 `tile_frame_recon`. `None` outside the bd10 full-RD
        // envelope. See the merge site.
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    )>,
> {
    let encode_one_tile = |tile_idx: usize| -> crate::EncodeResult<(
        Vec<u8>,
        Vec<crate::partition::PartitionTree>,
        Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
    )> {
        let (tile_sb_row_start, tile_sb_row_end) =
            tile_grid.row_span(tile_idx / tile_grid.tile_cols);
        let (tile_sb_col_start, tile_sb_col_end) =
            tile_grid.col_span(tile_idx % tile_grid.tile_cols);
        let tile_sb_cols = tile_sb_col_end - tile_sb_col_start;

        let mut tile_recon = Vec::new();
        // PD0_LVL_1 rate tables (presets 6..8), built once per tile on
        // first use — default CDFs at the frame qindex (C md_frame_context).
        let mut m6_pd0_tables: Option<crate::pd0::M6Pd0Tables> = None;
        // M6 leaf funnel state (preset 6, 4:2:0 still): decision-phase
        // chroma recon planes + neighbor-context state + rate tables.
        // Single-SB frames use the default contexts (C md_frame_context);
        // multi-SB frames currently reuse them for every SB — C chains
        // per-SB contexts (ec_ctx_array averaging), a documented residual
        // gap for the 128-cell decisions.
        // The C-exact leaf intra funnel covers still/420 allintra presets
        // 2, 3, 4, 5, 6, 7, 8, and eff-M9 (presets >= 9 clamp to M9).
        // On the ALLINTRA arm presets 2/3 use update_cdf_level 1 and 4..=6
        // level 2 — for I-slices the two are identical (only update_mv
        // differs, forced 0 on I-slices; set_cdf_controls,
        // enc_mode_config.c:8495) — and 7/8/9+ use update_cdf_level 0
        // (static default tables all frame). The VIDEO arm keeps level 1 up
        // to M8, so the chain gate is arm-dependent and is now derived by
        // `rate_arm::update_cdf_level` rather than spelled as a preset range
        // (see the `funnel_chain` binding below and docs/rate-arm-port-map.md).
        // eff-M9 (intra_level 8) arms the is_dc_only gate inside the funnel.
        let use_funnel = chroma_420 && chroma_src.is_some() && c_quant.is_some();
        // Same sc derivation as the pack side (identical inputs -> identical
        // result): the MD walk's rates + its per-SB CDF evolution must see
        // the same allow_sct as the real pack or the chains desync on
        // screen-content frames.
        let tile_sc = crate::sc_detect::derive_sc(sc_arm, sc_preset, encode_input, w, w, h);
        let mut funnel_cfg = crate::leaf_funnel::FunnelCfg::for_preset(speed_config.preset);
        // `pcs->rate_est_level` -> `set_rate_est_ctrls` (enc_mode_config.c:6428),
        // for THIS arm. `for_preset` bakes the allintra ladder (1 through M6,
        // 4 at M7/M8, 0 above); the video arm assigns a flat 1 at every preset
        // (:8942), so a video KEY frame at M7/M8 keeps the real neighbour
        // contexts and the real luma coeff rate where the still arm switches
        // to the fast approximation. Byte-neutral on the still path by
        // construction — `rate_arm::allintra_flattening_matches_the_ladder`
        // pins the pair against `for_preset`'s baked values at every preset.
        let (rate_est_coeff_lvl, rate_est_real_ctx) =
            crate::rate_arm::rate_est_ctrls(crate::rate_arm::rate_est_level(
                sc_arm,
                crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            ));
        funnel_cfg.coeff_rate_est_lvl = rate_est_coeff_lvl;
        funnel_cfg.real_coeff_ctx = rate_est_real_ctx;
        // `pcs->pic_filter_intra_level` -> `set_filter_intra_ctrls` and
        // `(intra_level, dist_based_ang_intra_level)` -> `set_intra_ctrls`,
        // for THIS arm (`crate::intra_arm`). `for_preset` bakes the ALLINTRA
        // rows; the video arm drops filter-intra entirely at M6+ and takes a
        // lower intra_level (M6 video = intra_level 2 = the still path's M5
        // candidate shape). Byte-neutral on the still path by construction —
        // `intra_arm::allintra_flattening_matches_the_ladder` pins the six
        // stamped fields against `for_preset`'s baked values at every preset.
        //
        // `is_base` is true unconditionally: every video picture this port
        // encodes is a KEY frame at `temporal_layer_index == 0`, which is also
        // what makes `dist_based_ang_intra_level` 0 on both arms (the ladder's
        // non-zero rows are all `is_islice ? 0 :` / `is_base ? 0 :`).
        crate::intra_arm::apply(
            &mut funnel_cfg,
            sc_arm,
            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
            true,
        );
        // `pcs->txs_level` -> `set_txs_controls`, for THIS arm
        // (`crate::txs_arm`). The arms agree at M4..M7 and diverge at M8/M9,
        // where the video ladder keeps the tx-size search on (level 3 / 4)
        // and the allintra one turns it off at the picture level.
        crate::txs_arm::apply(
            &mut funnel_cfg,
            sc_arm,
            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            true,
            u32::from(cli_qp),
        );
        // `pcs->txt_level` -> `svt_aom_set_txt_controls` and `pcs->cfl_level`
        // -> `set_cfl_ctrls`, for THIS arm (`crate::funnel_arm`).
        crate::funnel_arm::apply(
            &mut funnel_cfg,
            sc_arm,
            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
            true,
        );
        // `pcs->nic_level` -> `svt_aom_set_nic_controls`, for THIS arm
        // (`crate::nic_arm`). At M6 the video arm is level 8 against the still
        // arm's 6: stage counts {2,1,1} instead of {6,6,6} and candidate
        // thresholds 300/3/3 instead of 1200/15/15. Byte-neutral on the still
        // path except for one baked-table correction the pin names
        // (`nic_arm::allintra_flattening_matches_the_ladder`).
        crate::nic_arm::apply(
            &mut funnel_cfg,
            sc_arm,
            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            true,
        );
        // `ctx->mds0_use_hadamard_sb`, for THIS arm (`crate::encdec_arm`).
        // Unlike every arm above it this one does NOT come from
        // `sig_deriv_mode_decision_config` — it is a literal in each
        // `svt_aom_sig_deriv_enc_dec_*` body, which is why §1c's field-for-field
        // divergence table cannot see it. The video arm's `false` sends MDS0's
        // luma distortion down C's two-buffer VARIANCE arm instead of the
        // Hadamard SATD, and variance is DC-invariant where SATD is not.
        crate::encdec_arm::apply(&mut funnel_cfg, sc_arm);
        // `pcs->mds0_level` -> `set_mds0_controls`, for THIS arm
        // (`crate::mds0_arm`). The arms agree on a key frame through M10 and
        // diverge above it: the video arm takes level 2 (`fast_loop_core`'s
        // global dist-to-cost prune, product_coding_loop.c:1325) where the
        // allintra arm is a literal 0 at every preset. Byte-neutral on the
        // still path by construction.
        crate::mds0_arm::apply(
            &mut funnel_cfg,
            sc_arm,
            crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
            true,
            matches!(sc_arm, crate::sc_detect::ScArm::Allintra)
                || matches!(sc_arm, crate::sc_detect::ScArm::Video { is_islice: true }),
        );
        // C `pcs->pic_pd0_lvl` -> `set_pd0_ctrls`, for THIS arm, as the
        // REFINEMENT path's PD0 model. The allintra arm is PD0_LVL_1 at every
        // preset this path serves; the video arm is PD0_LVL_3 at M3..M7 and
        // PD0_LVL_4 at M8 (both LVL_1's block cost plus subres step 1; LVL_4
        // additionally prices coefficients with `coeff_rate_est_lvl` 2). The
        // depth-early-exit threshold is 900 at those levels EXCEPT under
        // `pic_pred_depth_only`, which is why the flag is passed.
        // `refined_pd0_model` documents the levels it does not carry (0..=2)
        // and returns the pre-existing allintra model for them.
        let (pd0_refined_mode, pd0_refined_eexit_th, pd0_refined_rate_lvl) =
            crate::part_arm::refined_pd0_model(
                sc_arm,
                crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
                u32::from(cli_qp),
                w * h,
                // C `ctx->pic_pred_depth_only` = `depth_refinement_ctrls.mode
                // == PD0_DEPTH_PRED_PART_ONLY`, which only level 10 sets.
                crate::depth_refine::DrCtrls::for_arm(
                    sc_arm,
                    speed_config.preset,
                    tile_sc.classes.sc_class5,
                    u32::from(cli_qp),
                )
                .pred_depth_only,
            );
        // PD0's own coefficient-rate level. `None` on the allintra arm keeps
        // the frame-level `FunnelCfg` value the call sites already pass.
        let pd0_refined_rate_lvl = pd0_refined_rate_lvl.unwrap_or(funnel_cfg.coeff_rate_est_lvl);
        // C `ctx->pd0_use_src_samples = allintra || pcs->hbd_md`
        // (enc_mode_config.c:7309). FALSE on every video frame, so the video
        // arm's PD0 predicts from the recon it generates per block instead of
        // from the source — see `crate::pd0::Pd0ReconCanvas`. bd10 sets
        // `hbd_md`, which puts C back on the source samples, so the port keeps
        // the source path there too.
        let pd0_video_recon =
            matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }) && bit_depth != 10;
        if coded_lossless {
            funnel_cfg.apply_coded_lossless();
        }
        funnel_cfg.allow_sct = tile_sc.allow_screen_content_tools;
        // THE palette flip-on: with the level stamped, the funnel injects
        // palette candidates (chunk 4) and the pack codes the winners
        // (chunk 5). sc_derivation.palette_level is 0 on every non-sc
        // frame, so non-screen-content streams are untouched.
        funnel_cfg.palette_level = tile_sc.palette_level;
        // SVTAV1_SC_TOOLS (DIAGNOSTIC ONLY, absent = unchanged): force one of
        // the two screen-content tools off to localize a screen divergence to
        // palette or IntraBC without editing and rebuilding.
        //
        //   SVTAV1_SC_TOOLS=nopalette   palette_level = 0
        //   SVTAV1_SC_TOOLS=noibc       allow_intrabc = false
        //   SVTAV1_SC_TOOLS=none        both off
        //
        // These deliberately do NOT touch `allow_screen_content_tools` (the
        // frame-header bit), so the streams stay comparable: only the RD
        // candidate set changes. A bisect that also flipped the header would
        // move the syntax and prove nothing about which tool caused a flip.
        //
        // Exists because localizing the graph.png preset-0..4 divergence
        // otherwise meant hand-editing this line and rebuilding, once per
        // hypothesis -- and an edit-to-measure loop is where measurements stop
        // getting made.
        match std::env::var("SVTAV1_SC_TOOLS").as_deref() {
            Ok("nopalette") => funnel_cfg.palette_level = 0,
            Ok("none") => funnel_cfg.palette_level = 0,
            _ => {}
        }
        // IBC chunk 3: the frame-level svt_aom_allow_intrabc (always
        // I-slice + sct on this path) — arms the per-candidate
        // intrabc_fac_bits[0] charge in the funnel. False on every
        // non-screen / p5+ frame (byte-inert there).
        funnel_cfg.allow_intrabc = tile_sc.allow_intrabc;
        // See SVTAV1_SC_TOOLS above.
        match std::env::var("SVTAV1_SC_TOOLS").as_deref() {
            Ok("noibc") | Ok("none") => funnel_cfg.allow_intrabc = false,
            _ => {}
        }
        let cwid = w / 2;
        // SB extent (task #95 chunk 2): a boundary block whose square (or edge)
        // block STRADDLES the aligned extent writes past aligned into the
        // SB-extent pad (C codes such blocks). The recon working buffers KEEP
        // the aligned stride (`w` luma / `cwid` chroma) but are sized to the SB
        // extent PRODUCT (`ext_w * ext_h`), so a straddling write past the
        // aligned right/bottom lands in the slack rows rather than out of
        // bounds (a right-straddle write wraps down into the next stride row —
        // hence the full product, not just extra rows). For a 64-aligned frame
        // `ext_w == w` and `ext_h == h`, so the buffers are the same size as
        // before — byte-neutral.
        let ext_w = w.div_ceil(sb_size) * sb_size;
        let ext_h = h.div_ceil(sb_size) * sb_size;
        let ext_cbuf = (ext_w / 2) * (ext_h / 2); // chroma buffer capacity at `cwid` stride
        let mut fun_u_recon = svtav1_types::try_vec![128u8; if use_funnel { ext_cbuf } else { 0 }]?;
        let mut fun_v_recon = svtav1_types::try_vec![128u8; if use_funnel { ext_cbuf } else { 0 }]?;
        let mut fun_ectx = if use_funnel {
            let mut e = EntropyCtx::new(
                w / 4,
                h / 4,
                true,
                walk_tx_mode_select,
                tile_sc.allow_screen_content_tools,
                bit_depth,
            );
            // Task #86: consistent with the other EntropyCtx instances
            // this tile constructs — see the real pack walk's identical
            // assignment for the rationale (leaf_funnel.rs itself is a
            // separate, out-of-scope workstream file; this only sets a
            // field on an EntropyCtx pipeline.rs already owns).
            e.tile_top_px = tile_sb_row_start * sb_size;
            e.tile_left_px = tile_sb_col_start * sb_size; // task #96
            e.tile_mi = crate::intra_edge::TileMi {
                mi_row_start: tile_sb_row_start * sb_size / 4,
                mi_row_end: (tile_sb_row_end * sb_size / 4).min(h / 4),
                mi_col_start: tile_sb_col_start * sb_size / 4,
                mi_col_end: (tile_sb_col_end * sb_size / 4).min(w / 4),
            };
            Some(e)
        } else {
            None
        };
        let fun_rates = if use_funnel {
            // §1s item 8: C's `init_frame_rate_tables` (md_config_process.c:292)
            // seeds `md_frame_context` from the primary reference's SAVED
            // end-of-frame CDFs when the header names one, and only otherwise
            // from `svt_av1_default_coef_probs(base_q_idx)` +
            // `svt_aom_init_mode_probs`. Pricing an inter frame against the
            // defaults understates the coefficient rate by roughly half on
            // this campaign's reference cell — measured — and that is what
            // makes C's `blk_skip_decision` pick `skip` where a
            // default-priced MD picks a coded residual.
            match md_frame_cdfs {
                Some(prev) => Some(crate::leaf_funnel::build_md_rates(&prev.fc, &prev.coeff)),
                None => {
                    let fc = crate::entropy::context::FrameContext::new_default();
                    let cfc = crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                    Some(crate::leaf_funnel::build_md_rates(&fc, &cfc))
                }
            }
        } else {
            None
        };
        #[allow(unused_mut)]
        // The PICTURE lambda (pre per-SB overrides) — the base C's tune-SSIM
        // set_ssim_rdmult scales from (ed_ctx->pic_full_lambda).
        let pic_lambda: u64 = c_quant.as_ref().map_or(0, |cq| u64::from(cq.lambda));
        let mut fun_frame = if use_funnel {
            let cq = c_quant.as_ref().unwrap();
            Some(crate::leaf_funnel::FunnelFrame {
                // C `seq_header.sb_mi_size` (task #91): 16 at SB64, 32 at
                // SB128. 16 for every SB64 encode -> byte-neutral there.
                sb_mi_size: sb_size / 4,
                sharpness: hdr_sharpness,
                sharp_tx_active,
                noise_norm_strength: hdr_noise_norm,
                qm_levels,
                tx_bias: hdr_tx_bias,
                mds0_ssd: hdr_complex_hvs,
                tune_ssim: hdr_alt_ssim,
                tune_ssim_threshold: if w * h > 1_665 * 1_120 { 1.02 } else { 1.03 },
                lambda: cq.lambda as u64,
                cli_qp: cli_qp as u32,
                rdoq_level: cq.rdoq_level,
                // Same source as `cq.allintra_rd_mult` (set beside
                // `CodingQuantCfg::new`) so the MD funnel and the bd10
                // re-encode cannot disagree about the RDOQ rate-weight arm.
                rdoq_allintra_rd_mult: cq.allintra_rd_mult,
                base_qindex,
                bit_depth,
                qindex_u,
                qindex_v,
                ac_bias_eff,
                // IBC chunk 7: frame-constant DV RD tables (default ndvc at
                // MV_SUBPEL_NONE — `build_dv_cost_tables`'s cadence doc) +
                // the aligned frame height for the vartx bottom clip.
                dv_tables: crate::intrabc::build_dv_cost_tables(
                    &crate::entropy::mv_coding::NmvContext::default(),
                    funnel_cfg.allow_intrabc,
                    false, // approx_inter_rate: structurally 0 on allintra
                ),
                frame_h_px: h,
                // The ALIGNED frame width (C `pcs->ppcs->aligned_width`) — the
                // other half of the cropped-TX RD distortion bound. `w`/`h` in
                // this scope are already the aligned dims (see the `ext_w` /
                // `ext_h` SB-extent derivation above, which rounds them UP).
                frame_w_px: w,
                coded_lossless,
                cfg: funnel_cfg,
            })
        } else {
            None
        };
        // ---- IBC chunk 8: frame-level IntraBC state + the MD mi grid ----
        // C md_config_process.c:946-969 (gated frm_hdr->allow_intrabc):
        // the frame hash table over the SOURCE (enhanced_pic), the diamond
        // site config (source stride baked), the one-shot QP mesh rescale;
        // plus this port's search cost tables (nmvc @ LOW precision — the
        // I-slice frame-constant `svt_aom_estimate_mv_rate` build) and the
        // per-block scalars (sadperbit16 from base_q_idx, errorperbit from
        // the funnel lambda >> RD_EPB_SHIFT).
        let ibc_state: Option<alloc::boxed::Box<crate::leaf_funnel::IbcFrameState>> = if use_funnel
            && funnel_cfg.allow_intrabc
        {
            let mut ctrls = crate::intrabc::IbcCtrls::for_level(tile_sc.intrabc_level);
            // scs->qp_based_th_scaling_ctrls.intra_bc_mesh_qp_scaling is
            // true on the allintra path (scale_mesh_patterns_by_qp doc).
            crate::intrabc::scale_mesh_patterns_by_qp(&mut ctrls, true, cli_qp as u32);
            let hash = crate::intrabc_hash::generate_ibc_data(
                encode_input,
                w,
                w,
                h,
                ctrls.max_block_size_hash,
                ctrls.max_cand_per_bucket,
                // `pcs->pic_disallow_4x4` — arm-forked at M3
                // (`part_arm::disallow_4x4`), not the flat `preset >= 4`.
                crate::part_arm::disallow_4x4(sc_arm, speed_config.preset),
            );
            // svt_aom_get_sad_per_bit(base_q_idx, 0): init_me_luts_bd's
            // `(int)(0.0418*q + 2.4107)` with q = ac_qlookup/4.0
            // (rc_process.c:186-190, mode_decision.c:2052-2063).
            let q8 = f64::from(svtav1_dsp::quant_tables::AC_QLOOKUP_8[base_qindex as usize]) / 4.0;
            let sad_per_bit = (0.0418 * q8 + 2.4107) as i32;
            let error_per_bit = ((pic_lambda) >> crate::intrabc::RD_EPB_SHIFT).max(1) as i32;
            Some(alloc::boxed::Box::new(crate::leaf_funnel::IbcFrameState {
                ctrls,
                hash,
                sites: crate::intrabc::init_search_sites(w),
                search_tables: crate::intrabc::build_nmv_cost_table(
                    &crate::entropy::mv_coding::NmvContext::default(),
                    crate::entropy::mv_coding::MvSubpelPrecision::Low,
                ),
                sad_per_bit,
                error_per_bit,
                mi_rows: (h / 4) as i32,
                mi_cols: (w / 4) as i32,
                tile: crate::intrabc::TileMiBounds {
                    mi_row_start: (tile_sb_row_start * sb_size / 4) as i32,
                    mi_row_end: ((tile_sb_row_end * sb_size / 4).min(h / 4)) as i32,
                    mi_col_start: (tile_sb_col_start * sb_size / 4) as i32,
                    mi_col_end: ((tile_sb_col_end * sb_size / 4).min(w / 4)) as i32,
                },
                sb_mi_size: (sb_size / 4) as i32,
                sb_size_log2_mi: (sb_size as u32 / 4).trailing_zeros(),
                sb_size_px: sb_size as i32,
                disallow_4x4: crate::part_arm::disallow_4x4(sc_arm, speed_config.preset),
            }))
        } else {
            None
        };
        // The MD mode-info grid the MVP scans read (C mi_grid_base as MD
        // stamps it) — frame-wide, one entry per 4x4 cell.
        // The MD mode-info grid — C `mi_grid_base` as MD stamps it. Shared by
        // the IntraBC MVP scans and the inter ones; IntraBC is intra-frame
        // only and inter prediction needs a reference, so the two can never
        // both be live and there is exactly one grid.
        let mut ibc_mvp_grid: alloc::vec::Vec<crate::intrabc_mvp::MvpMiEntry> =
            if ibc_state.is_some() || inter_md.is_some() {
                alloc::vec![crate::intrabc_mvp::MvpMiEntry::default(); (w / 4) * (h / 4)]
            } else {
                alloc::vec::Vec::new()
            };

        // Per-SB CDF refresh chain (C update_cdf_level 2 at M4..M6:
        // ec_ctx_array[sb] copied per the left/top-right rule at SB
        // configure, evolved by that SB's coded symbols, and the MD rate
        // tables rebuilt from the copy — enc_dec_process.c:2991-3043).
        // The evolution is simulated by re-coding each decided SB through
        // the real entropy walk against the chain contexts (bypass-encdec
        // makes MD symbols == coded symbols, so the funnel-consumed CDF
        // rows — kf_y/uv/angle/fi/skip/tx_size/coeff — evolve exactly like
        // C's). For frames wider than 2 SBs the both-neighbors case seeds
        // each SB's rate CDF with avg_cdf_symbols (left 3x + top-right 1x,
        // FrameContext::avg_cdf_with + CoeffFc::avg_cdf_with) per the C
        // neighbor rule below — matching enc_dec_process.c:3002-3022.
        let multi_sb = sb_cols * sb_rows > 1;
        // The per-SB CDF-refresh chain is only C-correct at M4..M6
        // (update_cdf_level 2, svt_aom_get_update_cdf_level_allintra
        // enc_mode_config.c:12154). M7/M8/eff-M9 (update_cdf_level 0) keep
        // the static default rate tables for every SB, so they never chain.
        // Gated on use_funnel so it only fires for the chroma/420 funnel
        // path (chroma_src is Some) — mono never chains.
        // `update_cdf_level != 0` for THIS arm — C `set_cdf_controls`'
        // `cdf_ctrl.enabled`, which is what gates `rtime_alloc_ec_ctx_array`
        // and therefore the per-SB context chain at all (enc_mode_config.c:8496,
        // :8945/:9927). allintra: 1 at M0..M3, 2 at M4..M6, 0 above — the
        // `0..=6` this line used to spell inline. video, I-slice: 1 at
        // M0..M8, 0 above, so a video KEY frame CHAINS at presets 7 and 8
        // where the still arm does not.
        //
        // `is_base` is `pcs->temporal_layer_index == 0`, true for every
        // picture the port encodes on this arm today (a key frame). It only
        // selects 1-vs-2 in the M1..M3 band and both are nonzero, so this
        // gate does not depend on it.
        let funnel_chain = use_funnel
            && crate::rate_arm::update_cdf_level(
                sc_arm,
                crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
                /*is_base=*/ true,
            ) != 0
            && multi_sb;
        let mut chain_snaps: Vec<(
            crate::entropy::context::FrameContext,
            alloc::boxed::Box<crate::entropy::coeff_c::CoeffFc>,
        )> = Vec::new();
        let mut sim_ectx = if funnel_chain {
            // The chain simulation re-codes each SB's symbols to evolve the
            // per-SB frame contexts — it must code the same no-palette
            // flags as the real pack or the palette CDF rows drift.
            let mut e = EntropyCtx::new(
                w / 4,
                h / 4,
                true,
                walk_tx_mode_select,
                tile_sc.allow_screen_content_tools,
                bit_depth,
            );
            // IBC chunk 1: same use_intrabc flag coding as the real pack —
            // the chain's intrabc_cdf must evolve identically (the C
            // MD-side twin, update_stats md_rate_estimation.c:854-855).
            e.allow_intrabc = tile_sc.allow_intrabc;
            e.tile_top_px = tile_sb_row_start * sb_size; // task #86, see fun_ectx above
            e.tile_left_px = tile_sb_col_start * sb_size; // task #96
            e.tile_mi = crate::intra_edge::TileMi {
                mi_row_start: tile_sb_row_start * sb_size / 4,
                mi_row_end: (tile_sb_row_end * sb_size / 4).min(h / 4),
                mi_col_start: tile_sb_col_start * sb_size / 4,
                mi_col_end: (tile_sb_col_end * sb_size / 4).min(w / 4),
            };
            Some(e)
        } else {
            None
        };
        // WRITE-ONLY sink: the funnel chain's simulated entropy walk needs a
        // `DeblockGeom` to `record_block` into, but nothing ever filters
        // through it (the real one is built in `encode_frame_impl` and is the
        // only geom `apply_deblock_frame` / the DLF search ever see). The
        // true-dims pair is therefore inert here — passing the aligned dims
        // keeps this from pretending to carry a crop it never uses.
        let mut sim_geom = crate::deblock::DeblockGeom::new(w, h, w, h);
        let mut sim_u = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
        let mut sim_v = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
        let mut sim_prev_sb_row = usize::MAX;
        let mut fun_rates = fun_rates;
        let mut tile_trees: Vec<crate::partition::PartitionTree> = Vec::new();
        let mut tile_frame_recon = svtav1_types::try_vec![128u8; ext_w * ext_h]?;
        // bd10 LUMA mode funnel (task #94): a parallel TRUE 10-bit recon canvas
        // so the per-block mode decision (evaluate_leaf MDS0) is made on the
        // 10-bit recon rather than the MSB-truncated u8 recon (which scales
        // SATD ×4 on `sample<<2` content and cannot flip the survivor). bd8
        // allocates NOTHING and passes `None` into FunnelCtx → the funnel is
        // byte-IDENTICAL. Frame-persistent (a block reads its left/above SB's
        // committed 10-bit recon); each SB's FunnelCtx borrows it.
        //
        // The canvas is SB-extent SIZED at the ALIGNED stride, exactly like the
        // u8 `tile_frame_recon` above, and `commit_leaf` applies the SAME
        // straddle clip to it as to the u8 recon — so a partial SB is in bounds
        // by construction. This predicate used to carry `w % 64 == 0 && h % 64
        // == 0`; that was the fourth independent copy of the bd10 alignment
        // gate, and it was screening a hazard the buffer shape had already
        // removed.
        let bd10_canvas_ok = bit_depth == 10;
        // bd10 FULL-RD (task #94, MODE axis): below eff-M9 the coded mode is
        // the MDS1/MDS3 full-RD winner, not the MDS0 survivor, so the bd10
        // canvas alone is not enough — widening only MDS0 to M6..M8 was
        // measured to close ZERO cells (docs/bd10-port-map.md). `full_rd10`
        // runs the whole full-RD chain (luma depth loop with TXS/TXT + chroma)
        // at 10 bits. It is gated on the arms that ARE ported at 10 bits:
        //   - CfL off: the CfL compare inside MDS3 is 8-bit only, and mixing it
        //     into a 10-bit block cost would be silently wrong (there is a
        //     debug_assert backstop in evaluate_leaf).
        //   - palette off: a palette candidate has no 10-bit prediction here.
        //   - mainline tools only: ac-bias / noise-norm are fork features whose
        //     u16 psy kernels are unported (tx_unit_hbd applies neither).
        // Everything outside that envelope keeps the existing behaviour.
        let bd10_full_rd = bd10_full_rd_supported(bit_depth, speed_config.preset, chroma_420, w, h);
        let bd10_luma_funnel = bd10_canvas_ok && (speed_config.preset >= 9 || bd10_full_rd);
        // Task #6 chunk 1: hand the funnel the REAL 10-bit source when the
        // caller supplied one AND a bd10 stage is armed to read it. The planes
        // arrive already SB-extent-padded when the frame has a partial SB
        // (`hbd_sb_owned`), so the LUMA stride is `in_stride` — the same stride
        // the u8 `sb_input` gather uses — and a block's `(abs_x, abs_y)` indexes
        // them identically. Chroma keeps the ALIGNED stride `w/2` with extra
        // rows, matching `sb_chroma_owned`.
        let funnel_src10 = hbd_src.filter(|_| bd10_luma_funnel).map(|(y10, u10, v10)| {
            debug_assert!(
                y10.len() >= in_stride * h,
                "hbd luma plane must cover the frame"
            );
            crate::leaf_funnel::FunnelSrc10 {
                y: y10,
                y_stride: in_stride,
                u: u10,
                v: v10,
                c_stride: w / 2,
            }
        });
        if funnel_src10.is_some() {
            hbd_used.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        let mut tile_frame_recon10: alloc::vec::Vec<u16> = if bd10_luma_funnel {
            svtav1_types::try_vec![512u16; ext_w * ext_h]?
        } else {
            alloc::vec::Vec::new()
        };
        // bd10 chroma decision canvases (the chroma twins of the luma one).
        // 4:2:0 -> half dims; seeded with the 10-bit DC default like the luma.
        let (mut tile_frame_u_recon10, mut tile_frame_v_recon10): (
            alloc::vec::Vec<u16>,
            alloc::vec::Vec<u16>,
        ) = if bd10_full_rd {
            let n = (ext_w / 2) * (ext_h / 2);
            (
                svtav1_types::try_vec![512u16; n]?,
                svtav1_types::try_vec![512u16; n]?,
            )
        } else {
            (alloc::vec::Vec::new(), alloc::vec::Vec::new())
        };

        let mut part_config =
            crate::partition::PartitionSearchConfig::from_speed_config(speed_config);
        // Task #86: this tile's own top row (luma pixels) — MD search
        // prediction must not treat it as having a real "above" neighbor
        // just because it isn't the frame's own top row (AV1 intra
        // prediction never crosses a tile boundary).
        part_config.tile_top_px = tile_sb_row_start * sb_size;
        // Task #96: ditto for this tile's own left column — MD prediction
        // must not read across a tile-COLUMN boundary either.
        part_config.tile_left_px = tile_sb_col_start * sb_size;
        // C `seq_header.sb_mi_size` (task #91): 16 at SB64 (the struct
        // default, so every pre-SB128 path is byte-identical), 32 at SB128.
        part_config.sb_mi_size = sb_size / 4;
        // The ALIGNED luma extent (C `pcs->ppcs->aligned_width/height`): the
        // clamp for intra reference samples. `w`/`h` here ARE the aligned dims
        // (`encode_frame_420` pads TRUE -> ALIGNED before calling this), NOT
        // the SB extent the recon working buffers are sized to.
        part_config.aligned_w = w;
        part_config.aligned_h = h;
        if chroma_420 {
            // 4:2:0 policy: min luma block dim 8, so every coded block is a
            // chroma reference with chroma dims exactly (w/2, h/2) >= 4.
            part_config.min_block_dim = 8;
        }
        // Preset 5 signals SH enable_intra_edge_filter=1 on the still/420
        // surface (C-exact — the ONLY allintra preset with the bit). A
        // conforming decoder then edge-filters/upsamples directional
        // predictions whose p_angle != 90/180; the homegrown leaf coder
        // predicts UNFILTERED, so until the M5 funnel (which will predict
        // with the C edge filter) routes this preset, D45..D203 candidates
        // must not be emitted — V (exactly 90) and H (exactly 180) are
        // skipped by the decoder's filter and stay recon-exact.
        if speed_config.preset == 5 && chroma_420 && ref_frame_data.is_none() {
            part_config.enable_directional = false;
        }
        // Frame-level C-exact coding quantizer (still path — quant.rs).
        part_config.c_quant = c_quant.clone();

        for sb_row in tile_sb_row_start..tile_sb_row_end {
            // Feature 1: cooperative cancellation, checked once per SB row of
            // the MD search. `may_stop()` short-circuits to `false` for the
            // default `Unstoppable` token, so this is byte-inert unless a real
            // stop token was installed via `with_stop`.
            if stop.may_stop() {
                stop.check()
                    .map_err(EncodeError::from)
                    .map_err(whereat::at)?;
            }
            for sb_col in tile_sb_col_start..tile_sb_col_end {
                let sb_x0 = sb_col * sb_size;
                let sb_y0 = sb_row * sb_size;
                let sb_cur_w = sb_size.min(w - sb_x0);
                let sb_cur_h = sb_size.min(h - sb_y0);

                // [SVT_HDR_MODE] variance boost: this SB searches/quantizes
                // at its PLANNED qindex (luma + per-plane chroma) with the
                // matching lambda (C per-SB svt_aom_lambda_assign). The
                // frame-level CDF bucket stays at the FH base (C behavior).
                // [SVT_HDR_MODE] tune-SSIM per-SB lambda: C's
                // set_ssim_rdmult scales the PICTURE lambda per block,
                // REPLACING the qindex-derived lambda (coding_loop.c:374)
                // — so when factors are present they own the lambda and
                // the per-SB delta-q override below skips its lambda set
                // (quantization still follows the per-SB qindex).
                if let (Some((factors, num_cols, num_rows)), Some(f)) =
                    (ssim_factors, fun_frame.as_mut())
                {
                    let scale = crate::tune::ssim_scale_for_block(
                        factors,
                        *num_cols,
                        *num_rows,
                        (sb_row * sb_size) / 4,
                        (sb_col * sb_size) / 4,
                        sb_size / 4,
                        sb_size / 4,
                    );
                    f.lambda = (pic_lambda as f64 * scale + 0.5) as u64;
                }
                if let (Some(plan), Some(f)) = (sb_qindex_plan, fun_frame.as_mut()) {
                    let sbq = plan[sb_row * sb_cols + sb_col];
                    f.base_qindex = sbq;
                    f.qindex_u =
                        (i32::from(sbq) + i32::from(chroma_ac_deltas.0)).clamp(0, 255) as u8;
                    f.qindex_v =
                        (i32::from(sbq) + i32::from(chroma_ac_deltas.1)).clamp(0, 255) as u8;
                    // [SVT_HDR_MODE] per-SB lambda: alt KF factor (fork
                    // default) + the delta-q qdiff stats factor
                    // (rc_process.c:437-446; this path is fork-only).
                    #[cfg(feature = "std")]
                    if crate::dbgenv::lambda_dbg_set() {
                        std::eprintln!(
                            "sb lam alt={} sbq={} base={} -> {}",
                            hdr_alt_lambda,
                            sbq,
                            fh_base_qindex,
                            crate::pd0::kf_full_lambda_8bit_ex(
                                sbq,
                                u32::from(crate::rate_control::qindex_to_qp(sbq)),
                                hdr_alt_lambda,
                                i32::from(sbq) - i32::from(fh_base_qindex),
                            )
                        );
                    }
                    if ssim_factors.is_none() {
                        f.lambda = u64::from(crate::pd0::kf_full_lambda_8bit_tuned(
                            sbq,
                            u32::from(crate::rate_control::qindex_to_qp(sbq)),
                            hdr_alt_lambda,
                            i32::from(sbq) - i32::from(fh_base_qindex),
                            // Frame `lambda_weight` + the extended-CRF bump.
                            // `None` with a zero bump keeps the pre-existing
                            // per-SB PSNR ladder this site has always used.
                            match (hdr_iq_lambda_weight, lw_bump) {
                                (Some(w), b) => Some(w + b),
                                (None, 0) => None,
                                (None, b) => Some(crate::pd0::frame_lambda_weight(
                                    u32::from(crate::rate_control::qindex_to_qp(sbq)),
                                    false,
                                    b,
                                )),
                            },
                        ));
                    }
                }

                let ref_ctx = ref_frame_data.map(|rf| crate::partition::RefFrameCtx {
                    y_padded: ref_padded_y,
                    sb_size,
                    y_plane: rf,
                    stride: w,
                    pic_width: w,
                    pic_height: h,
                    mv_map: Some(mv_map),
                    mv_map_stride,
                });
                // Per-SB TPL QP offsets are DISABLED until delta_q signaling
                // is ported: the frame header currently writes
                // delta_q_present=0, so the decoder dequantizes every block
                // at base_q_idx — any per-SB offset here silently corrupts
                // reconstruction (encoder and decoder disagree on scale).
                // When delta_q lands, the offsets must be applied HERE in
                // qindex units (AV1 delta_q is qindex-domain); the old
                // clamp(0, 63) that lived here was the CLI/qindex
                // conflation and is gone — qindex saturates at u8 range.
                let _ = (sb_row, sb_col, &sb_qp_offsets);
                // C `svt_aom_mode_decision_configure_sb` (md_process.c:800-803):
                //     ctx->qp_index = delta_q_present || r0_delta_qp_md
                //                   ? sb_qp : base_q_idx;
                // and `ctx->qp_index` drives the WHOLE MD context — the PD0
                // tables and the partition search included, not just the leaf.
                //
                // This was pinned to `base_qindex`. The per-SB value was already
                // threaded into the leaf funnel a few lines above
                // (`f.base_qindex = sbq`), so with variance boost on the funnel
                // and the partition search were pricing against DIFFERENT
                // quantizers within the same superblock.
                //
                // `sb_qindex_plan.is_some()` is exactly the port's
                // `delta_q_present`: the plan is built only under
                // `hdr.enable_variance_boost`, and the same `Option` gates the
                // frame header's delta-q signalling (`delta_q_res_signal`). C's
                // second disjunct, `r0_delta_qp_md`, is TPL-driven and always
                // false for a single still (no lookahead), so it is not modelled
                // — see the CRF==CQP note in rust/CLAUDE.md.
                let sb_qindex = match sb_qindex_plan {
                    Some(plan) => plan[sb_row * sb_cols + sb_col],
                    None => base_qindex,
                };
                // ---------------------------------------------- SB128 (#91)
                // The b64 CODING UNITS of this superblock, in C's coding
                // order (`sb128_geom::sb_coding_units`). SVT's b64 grid is
                // ALWAYS 64x64 while the sb grid follows super_block_size, so
                // the per-64 machinery (PD0 tree, variance map, leaf funnel,
                // recon) is size-agnostic — only the visiting ORDER and the
                // extra 128-root partition symbol differ. At SB64 there is
                // exactly ONE unit (the SB itself) with `unit_size ==
                // sb_size`, so the loop below is byte-identical to the
                // pre-SB128 code by construction.
                //
                // Everything OUTSIDE the unit loop stays per-SB — notably the
                // `ec_ctx` chain base / rate tables, because C's
                // `ec_ctx_array[sb]` is genuinely SB-indexed: at SB128 the
                // rate-estimation CDF seed refreshes once per 128 REGION
                // (4x coarser), which is the map's §"Pipeline state"
                // behavioural delta. Keeping the chain here gets that right
                // for free.
                let units = crate::sb128_geom::sb_coding_units(sb_x0, sb_y0, sb_size, w, h);
                let unit_size = if sb_size == 128 { 64 } else { sb_size };
                // §1s item 1: BOTH gates used to carry a `ref_*.is_none()`
                // term, so any frame with a reference bypassed the C-exact
                // PD0 partition search AND the leaf funnel and ran the
                // pre-campaign `partition::partition_search_with_config`
                // recursion — the code every video-KEY chunk of this campaign
                // was built to replace. They are gone; the funnel's inter
                // candidate (item 1b) is what makes taking them off pay.
                let use_pd0 = speed_config.preset >= 6
                    || (matches!(speed_config.preset, 0..=5) && use_funnel);
                // CLI-qp-calibrated lambda via the exact inverse mapping
                // (see qp_to_lambda's domain note). On the PD0 fixed-tree
                // path the leaf funnel must be preset-INDEPENDENT like
                // C's (the C decision lambda is the same kf chain at M6
                // and eff-M9 — instrumented 1527856 at qindex 220 in
                // both), so it pins the scale the byte-identical M10/M13
                // cells validated instead of the per-preset homegrown
                // scale.
                let leaf_scale = if use_pd0 {
                    crate::speed_config::SpeedConfig::from_preset(13).lambda_scale()
                } else {
                    speed_config.lambda_scale()
                };
                let sb_lambda = (crate::rate_control::qp_to_lambda(
                    crate::rate_control::qindex_to_qp(sb_qindex),
                ) * leaf_scale) as u64;

                // C-exact partition source: at allintra presets >= 9 the C
                // library (which clamps allintra presets to M9) decides the
                // ENTIRE partition tree in PD0 with a fixed {NONE, SPLIT}
                // quadtree and no NSQ search (docs/IDENTITY-STATUS.md
                // 2026-07-13 diagnosis), and at M2..M8 the same
                // PRED_PART_ONLY architecture runs the prediction-based
                // PD0_LVL_1 block encode instead (M6 chunk diagnosis).
                // Key/still frames at presets >= 6 — and preset 5 when
                // the M5 leaf funnel is live (still/420) — take the
                // ported PD0 decisions (crate::pd0) and encode the fixed
                // tree; everything else keeps the homegrown search.
                // (Presets 2..4 also run PD0_LVL_1 in C, but their PD1
                // leaf configs are unported, so they stay on the
                // homegrown path until they land. M5 depth refinement is
                // ADAPTIVE level 9 — the refined depths lose the
                // inter-depth compare on every tracked cell, the coded
                // tree == the PD0 tree; see docs/IDENTITY-STATUS.md.)
                // The search reads intra neighbors from — and reconstructs
                // directly into — the live frame buffer, exactly like the
                // decoder (fixes within-SB predictions that previously fell
                // back to 128).
                // Chain: select this SB's context base per the C rule and
                // rebuild the funnel rate tables from it.
                // Only read by the std-gated CHAINDUMP / SEED debug dumps below.
                #[cfg(feature = "std")]
                let sb_index = sb_row * sb_cols + sb_col;
                // Superres chunk B.4: this SB's entry in C's STALE variance
                // array — the CODED grid's linear index into an array laid out
                // on the FULL-RES grid (exactly the indexing C does after
                // `scale_pcs_params`). `None` on every non-superres path, where
                // the variance is recomputed from the coded source instead.
                let sb_stale_vars: Option<&crate::pd0::SbVariance> =
                    stale_vars.and_then(|v| v.get(sb_row * sb_cols + sb_col));
                // PORT-NOTE(unverified): `chain_snaps` is a PER-TILE
                // accumulator (pushed once per SB in this tile's own
                // raster order, starting empty at tile_idx's first SB —
                // see the push site below), so it must be indexed
                // TILE-LOCALLY, not by the absolute frame-wide `sb_index`.
                // Before task #86 `tile_rows` was always 1 (tile_idx == 0,
                // tile_sb_row_start == 0), so local == absolute and this
                // bug was unreachable — real `--tile-rows` use is what
                // exposed it (`sb_index - 1` / `sb_index - sb_cols + 1`
                // underflowed/out-of-bounded on tile_idx >= 1, a hard
                // panic, not a byte divergence). `topright_avail`'s row
                // check now gates on the TILE's own top row
                // (`sb_row > tile_sb_row_start`), matching this being a
                // per-tile-reset rate-ESTIMATE chain (mirrors the real
                // entropy walk's per-tile above-context reset in
                // `run_entropy_walk`) — not verified against C's own
                // per-tile `ec_ctx_array` neighbor rule at a tile-row
                // boundary specifically (only the single-tile-frame shape
                // was ever C-cross-checked); this only affects MD RATE
                // ESTIMATES (candidate cost comparisons), never the
                // coded bitstream, whose entropy state comes from the
                // separately-reset `run_entropy_walk`.
                let local_sb_index =
                    (sb_row - tile_sb_row_start) * tile_sb_cols + (sb_col - tile_sb_col_start);
                let chain_base = if funnel_chain {
                    // C `ec_ctx_array[sb]` neighbor rule for the rate-estimation
                    // CDF (enc_dec_process.c:3002-3022). `pic_based_rate_est` is
                    // only ever false (enc_handle.c), so the weighted-average
                    // branch always runs. Availability predicates match C for a
                    // single-tile SB-aligned frame: left = not tile-left column,
                    // top-right = not tile-top row AND the SB one to the right
                    // exists (so the last column has no top-right).
                    let left_avail = sb_col > tile_sb_col_start;
                    let topright_avail = sb_row > tile_sb_row_start && sb_col + 1 < tile_sb_col_end;
                    if left_avail && topright_avail {
                        // both -> copy left, then avg with top-right (3:1).
                        // C AVG_CDF_WEIGHT_LEFT / AVG_CDF_WEIGHT_TOP
                        // (enc_dec_process.c:2665-2666, :3016-3021).
                        const WT_LEFT: i32 = 3;
                        const WT_TOP: i32 = 1;
                        let mut base = chain_snaps[local_sb_index - 1].clone();
                        let tr = &chain_snaps[local_sb_index - tile_sb_cols + 1];
                        base.0.avg_cdf_with(&tr.0, WT_LEFT, WT_TOP);
                        base.1.avg_cdf_with(tr.1.as_ref(), WT_LEFT, WT_TOP);
                        Some(base)
                    } else if left_avail {
                        // left only -> copy left (sb-1)
                        Some(chain_snaps[local_sb_index - 1].clone())
                    } else if topright_avail {
                        // top-right only -> copy top-right (sb - tile_sb_cols + 1)
                        Some(chain_snaps[local_sb_index - tile_sb_cols + 1].clone())
                    } else {
                        // neither -> md_frame_context (default)
                        None
                    }
                } else {
                    None
                };
                // Diagnostic aid: SVTAV1_CHAIN_DUMP=1 prints each SB's
                // post-configure (chain_base) coeff CDF — the exact
                // per-SB rate-estimation context C builds from
                // ec_ctx_array[sb] (enc_dec_process.c:3010-3022). Used to
                // verify the avg_cdf chain against instrumented C
                // (2026-07-15 M6 diagnosis: chain proven C-exact through
                // sb36; the recon divergence is a downstream leaf-coeff
                // issue, NOT the chain). No encoder-output change.
                #[cfg(feature = "std")]
                if funnel_chain && crate::dbgenv::chain_dump() {
                    let dflt_cfc;
                    let cfc: &crate::entropy::coeff_c::CoeffFc = match &chain_base {
                        Some((_, cfc)) => cfc.as_ref(),
                        None => {
                            dflt_cfc =
                                crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                            &dflt_cfc
                        }
                    };
                    eprint!("CHAINDUMP CFG sb={sb_index} col={sb_col} row={sb_row}");
                    eprint!(" cbeobY");
                    for c in 0..4 {
                        let e = &cfc.coeff_base_eob_cdf[c];
                        eprint!(" {},{}", e[0], e[1]);
                    }
                    eprint!(" cbeobU");
                    for c in 0..4 {
                        let e = &cfc.coeff_base_eob_cdf[4 + c];
                        eprint!(" {},{}", e[0], e[1]);
                    }
                    eprintln!();
                }
                // SVTAV1_SEED_DUMP=1: one line per SB with salient SYNTAX-CDF
                // seed rows, field-for-field matching the C-side SVT_SEED_OUT
                // interposer (wrap on svt_aom_estimate_syntax_rate). diff the
                // two files -> first SB whose rate seed diverges (the "every
                // leaf cost in the SB shifted" divergence class).
                #[cfg(feature = "std")]
                if funnel_chain && crate::dbgenv::seed_dump() {
                    let dflt;
                    let (fc, cfc): (
                        &crate::entropy::context::FrameContext,
                        &crate::entropy::coeff_c::CoeffFc,
                    ) = match &chain_base {
                        Some((fc, cfc)) => (fc, cfc.as_ref()),
                        None => {
                            dflt = (
                                crate::entropy::context::FrameContext::new_default(),
                                crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                            );
                            (&dflt.0, &dflt.1)
                        }
                    };
                    eprintln!(
                        "SEED sb={} part0={},{},{} kf00={},{},{} txs00={},{} skip0={} ang0={},{},{} cfls={},{},{} cfla0={},{},{} xtx={},{},{}",
                        sb_index,
                        fc.partition_cdf[0][0],
                        fc.partition_cdf[0][1],
                        fc.partition_cdf[0][2],
                        fc.kf_y_mode_cdf[0][0][0],
                        fc.kf_y_mode_cdf[0][0][1],
                        fc.kf_y_mode_cdf[0][0][2],
                        fc.tx_size_cdf[0][0][0],
                        fc.tx_size_cdf[1][0][0],
                        fc.skip_cdf[0][0],
                        fc.angle_delta_cdf[0][0],
                        fc.angle_delta_cdf[0][1],
                        fc.angle_delta_cdf[0][2],
                        fc.cfl_sign_cdf[0],
                        fc.cfl_sign_cdf[1],
                        fc.cfl_sign_cdf[2],
                        fc.cfl_alpha_cdf[0][0],
                        fc.cfl_alpha_cdf[0][1],
                        fc.cfl_alpha_cdf[0][2],
                        cfc.intra_ext_tx_cdf[52][0],
                        cfc.intra_ext_tx_cdf[52][1],
                        cfc.intra_ext_tx_cdf[52][2],
                    );
                }
                if funnel_chain {
                    fun_rates = Some(match &chain_base {
                        Some((fc, cfc)) => crate::leaf_funnel::build_md_rates(fc, cfc),
                        None => {
                            let fc = crate::entropy::context::FrameContext::new_default();
                            let cfc =
                                crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                            crate::leaf_funnel::build_md_rates(&fc, &cfc)
                        }
                    });
                }
                // Per-b64 coding units (SB128: up to 4 in Z-order; SB64: the
                // SB itself — see the `units` comment above).
                let mut unit_results: Vec<crate::partition::PartitionResult> =
                    Vec::with_capacity(units.len());
                // SB128 depth-refinement: C's `get_max_min_pd0_depths`
                // (enc_dec_process.c:1943) derives max/min PD0 block sizes over
                // the WHOLE 128 SB pc_tree (all four 64x64 quadrants), and feeds
                // them to `set_start_end_depth`'s `limit_max_min_to_pd0` gate.
                // The port's per-64-unit refined scan must see that SAME whole-SB
                // fold, not this one quadrant's — else a quadrant with PD0 max 16
                // caps its shallowest tested depth at 16x16 and force-splits the
                // 32x32 nodes a sibling quadrant's max-32 keeps. Folded once per
                // SB, lazily, from the same pure PD0 eval the unit loop recomputes
                // (`pd0_pick_sb_partition_m6_eval` reads only source pixels).
                // `None` at SB64 (units.len() == 1) → byte-identical.
                let mut sb_pd0_max_min: Option<(usize, usize)> = None;
                for &(x0, y0) in units.iter() {
                    let cur_w = unit_size.min(w - x0);
                    let cur_h = unit_size.min(h - y0);
                    // C-exact partition source gate.
                    // Task #95 chunk 2: partial units (cur_w/cur_h < unit_size)
                    // take the PD0 fixed-tree path too — C decides the ENTIRE
                    // partition tree in PD0 for every b64 including incomplete
                    // ones, starting from a 64x64 root that carries the
                    // spec-5.11.4 forced edge splits. Complete units are
                    // unaffected (cur_w == cur_h == unit_size).
                    // Retained (underscored) rather than deleted: the PD1 walk no
                    // longer needs it, but it is the canonical "is this unit
                    // complete?" predicate and the next edge-aware path will.
                    let _full_sb = cur_w == unit_size && cur_h == unit_size;
                    let sb_result = if use_pd0 {
                        if coded_lossless || speed_config.preset >= 9 {
                            let tree = if coded_lossless {
                                // Issue #5: every square above 8x8 is forced
                                // SPLIT and every 8x8 is a leaf
                                // (`mimic_only_tx_4x4` -> `max_sq_size` 8,
                                // enc_dec_process.c:1492); PD0 has nothing
                                // left to decide, at any preset.
                                crate::pd0::lossless_tree(x0, y0, unit_size, w, h)
                            } else if bit_depth == 10 {
                                // C `set_pd0_ctrls` (enc_mode_config.c:5415) FORCES
                                // PD0_LVL_0 (full-RD partition search) at bd10 (hbd_md
                                // set), regardless of preset — where bd8 uses the
                                // preset's LVL_6/LVL_5 variance heuristic. LVL_0 runs
                                // at 8-bit on the same MSB-truncated `sb_input`, so
                                // this is a pure partition change; the coded levels
                                // are recomputed at 10-bit by bd10_reencode_luma.
                                crate::pd0::pd0_pick_sb_partition_lvl0(
                                    sb_input,
                                    in_stride,
                                    x0,
                                    y0,
                                    cli_qp as u32,
                                    sb_qindex,
                                    // C `pcs->lambda_weight` for this frame — the PSNR
                                    // ladder keyed on `picture_qp` plus the extended-CRF
                                    // bump (pd0::frame_lambda_weight). Identical to the
                                    // old `kf_full_lambda_8bit(qindex, cli_qp)` ladder
                                    // whenever the CRF offset is 0.
                                    crate::pd0::frame_lambda_weight(
                                        u32::from(picture_qp),
                                        tune_iq,
                                        lw_bump,
                                    ),
                                    // [SVT_HDR_MODE] Frame luma QM level. C forces
                                    // PD0_LVL_0 at bd10 whose light encode applies
                                    // the matrix when using_qmatrix (fork default);
                                    // mainline/QM-off leave qm_levels = [15;3], so
                                    // this is the non-QM (byte-inert) path there.
                                    qm_levels[0],
                                    crate::pd0::input_resolution_factor(w * h),
                                    w,
                                    h,
                                    // Superres chunk B.4: this SB's STALE full-res variance entry.
                                    sb_stale_vars,
                                    // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                                    max_tx_size,
                                )
                            } else if matches!(sc_arm, crate::sc_detect::ScArm::Video { .. }) {
                                // The VIDEO arm's PD0, which is a different
                                // LEVEL, an uncapped max block size and NSQ
                                // geometry ON — see
                                // `pd0::pd0_pick_sb_partition_video`. The
                                // allintra arm below is untouched, so the still
                                // envelope is byte-neutral by construction.
                                let (pic_pd0_lvl, pd0_coeff_rate_est_lvl, accurate_part_ctx) =
                                    crate::part_arm::video_pd0_params(
                                        crate::rate_arm::eff_enc_mode(sc_arm, speed_config.preset),
                                        u32::from(cli_qp),
                                        w * h,
                                    );
                                let tables = m6_pd0_tables.get_or_insert_with(|| {
                                    crate::pd0::build_m6_pd0_tables(sb_qindex)
                                });
                                crate::pd0::pd0_pick_sb_partition_video(
                                    sb_input,
                                    in_stride,
                                    x0,
                                    y0,
                                    u32::from(cli_qp),
                                    sb_qindex,
                                    crate::pd0::frame_lambda_weight(
                                        u32::from(picture_qp),
                                        tune_iq,
                                        lw_bump,
                                    ),
                                    tables,
                                    pic_pd0_lvl,
                                    pd0_coeff_rate_est_lvl,
                                    accurate_part_ctx,
                                    crate::part_arm::nsq_geom_enabled(sc_arm, speed_config.preset),
                                    // This branch IS C's `pic_pred_depth_only`
                                    // case: `depth_refinement_ctrls.mode ==
                                    // PD0_DEPTH_PRED_PART_ONLY` is what makes
                                    // the port code the PD0 tree directly
                                    // instead of running the refinement walk,
                                    // and it is the same flag
                                    // `set_depth_early_exit_ctrls` reads
                                    // (enc_mode_config.c:7232).
                                    true,
                                    crate::pd0::input_resolution_factor(w * h),
                                    w,
                                    h,
                                    tile_sb_row_start * sb_size,
                                    tile_sb_col_start * sb_size,
                                    sb_stale_vars,
                                    max_tx_size,
                                    // C `pd0_use_src_samples` (video arm: recon)
                                    // — the same value the refinement path gets.
                                    pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                )
                            } else {
                                crate::pd0::pd0_pick_sb_partition(
                                    sb_input,
                                    in_stride,
                                    x0,
                                    y0,
                                    cli_qp as u32,
                                    sb_qindex,
                                    // C `pcs->lambda_weight` for this frame — the PSNR
                                    // ladder keyed on `picture_qp` plus the extended-CRF
                                    // bump (pd0::frame_lambda_weight). Identical to the
                                    // old `kf_full_lambda_8bit(qindex, cli_qp)` ladder
                                    // whenever the CRF offset is 0.
                                    crate::pd0::frame_lambda_weight(
                                        u32::from(picture_qp),
                                        tune_iq,
                                        lw_bump,
                                    ),
                                    // C `input_resolution_factor[input_resolution]`:
                                    // per-picture coeff-rate addend keyed on w*h.
                                    crate::pd0::input_resolution_factor(w * h),
                                    // ALIGNED dims — the spec-5.11.4 edge predicate grid.
                                    w,
                                    h,
                                    // Superres chunk B.4: this SB's STALE full-res variance entry.
                                    sb_stale_vars,
                                    // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                                    max_tx_size,
                                )
                            };
                            // The same per-SB variance map C's picture analysis
                            // feeds to is_dc_only_safe (pcs->ppcs->variance): the
                            // fixed-tree leaves use it to force the C-exact
                            // DC-only intra candidate set where the gate fires.
                            let sb_vars = sb_stale_vars.copied().unwrap_or_else(|| {
                                crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0)
                            });
                            let mut funnel_ctx = if use_funnel {
                                let (u_src, v_src) = chroma_src.unwrap();
                                Some(crate::leaf_funnel::FunnelCtx {
                                    u_src,
                                    v_src,
                                    src10: funnel_src10,
                                    u_recon: &mut fun_u_recon,
                                    v_recon: &mut fun_v_recon,
                                    c_stride: cwid,
                                    ectx: fun_ectx.as_mut().unwrap(),
                                    rates: fun_rates.as_deref().unwrap(),
                                    frame: fun_frame.as_ref().unwrap(),
                                    // bd10 luma mode funnel (task #94): true 10-bit
                                    // recon canvas for the per-block mode decision;
                                    // None (bd8 / other presets / partial-SB) is
                                    // byte-identical.
                                    y_recon10: if bd10_luma_funnel {
                                        Some(&mut tile_frame_recon10)
                                    } else {
                                        None
                                    },
                                    u_recon10: if bd10_full_rd {
                                        Some(&mut tile_frame_u_recon10)
                                    } else {
                                        None
                                    },
                                    v_recon10: if bd10_full_rd {
                                        Some(&mut tile_frame_v_recon10)
                                    } else {
                                        None
                                    },
                                    // IBC chunk 8: frame IntraBC state + the MD mi grid.
                                    ibc: ibc_state.as_deref(),
                                    inter: inter_md,
                                    ibc_mvp: if ibc_state.is_some() || inter_md.is_some() {
                                        Some(&mut ibc_mvp_grid)
                                    } else {
                                        None
                                    },
                                    ibc_gate: Default::default(),
                                    full_rd10: bd10_full_rd,
                                })
                            } else {
                                None
                            };
                            crate::partition::encode_fixed_tree(
                                &sb_input[y0 * in_stride + x0..],
                                in_stride,
                                &mut tile_frame_recon,
                                w,
                                &tree,
                                unit_size,
                                sb_qindex,
                                &part_config,
                                x0,
                                y0,
                                w,
                                h,
                                &sb_vars,
                                (x0, y0),
                                funnel_ctx.as_mut(),
                            )
                        } else {
                            // Per-SB PD0 rate tables from the chain (C rebuilds
                            // rate_est_table from ec_ctx_array[sb] BEFORE the
                            // SB's PD0 runs — the drifting SPLIT rates).
                            let chained_tables = if funnel_chain {
                                Some(match &chain_base {
                                    Some((fc, cfc)) => {
                                        crate::pd0::build_m6_pd0_tables_from_ctx(fc, cfc)
                                    }
                                    None => crate::pd0::build_m6_pd0_tables(sb_qindex),
                                })
                            } else {
                                None
                            };
                            let tables = match &chained_tables {
                                Some(t) => t,
                                None => m6_pd0_tables.get_or_insert_with(|| {
                                    crate::pd0::build_m6_pd0_tables(sb_qindex)
                                }),
                            };
                            // The PD1 depth-refinement walk IS edge-aware as of
                            // 2026-08-04 (depth_refine.rs: forced split at a
                            // both-false node, the single injected shape at a
                            // one-false node priced from the BINARY alphabet, and
                            // off-frame quadrants skipped), so partial SBs no
                            // longer fall back to the plain PD0 fixed tree. That
                            // fallback was the structural reason presets 0..=5
                            // could not byte-match C on non-64-aligned geometry:
                            // C runs its PD1 refinement on every SB, complete or
                            // not, so a partial SB taking a DIFFERENT SEARCH could
                            // only match by coincidence.
                            // C `ctx->pred_depth_only` (enc_mode_config.c:7095)
                            // is `mode == PD0_DEPTH_PRED_PART_ONLY`, i.e. the
                            // refinement walk runs whenever the level is NOT 10.
                            // This used to be `matches!(preset, 0..=5)`, which is
                            // the same predicate ON THE ALLINTRA ARM (that ladder
                            // returns 10 at M6 and above and an adaptive level
                            // below) but NOT on the video arm, where M6/M7 are
                            // levels 6/8 — adaptive. Deriving it from the ctrls
                            // keeps the still path byte-identical by construction
                            // and lets a video key frame take the refinement C
                            // runs for it.
                            let dr = crate::depth_refine::DrCtrls::for_arm(
                                sc_arm,
                                speed_config.preset,
                                tile_sc.classes.sc_class5,
                                cli_qp as u32,
                            );
                            let refined = dr.adaptive && use_funnel;
                            if refined {
                                // M4/M5 (`dr_mode = 1`, PD0_DEPTH_ADAPTIVE):
                                // PD1 re-decides depths around the PD0 tree —
                                // depth_refine.rs. The refinement gates run on
                                // the PD0 PART_N costs; the walk evaluates the
                                // admitted depths through the leaf funnel and
                                // compares with real partition rates
                                // (bias 995). M6+ (PRED_PART_ONLY) keeps the
                                // fixed-tree path below (identical outcome:
                                // s = e = 0 everywhere).
                                // C's allintra depth-refinement level is sc_class5-
                                // aware (enc_mode_config.c:10067-10090): screen
                                // content at M0-M2 uses a lower/more-thorough level
                                // (1/1/5) that admits the depth descent the
                                // !sc_class5 level-6 row over-prunes.
                                // ONE predicate, used by BOTH the PD0 eval that
                                // builds the refinement scan and the PD1 walk that
                                // consumes it. They MUST agree: if PD0 injects an
                                // edge shape at a one-false node while the walk
                                // force-splits it, the walk descends into a scan
                                // node that has no children and panics.
                                // `svt_aom_get_nsq_geom_level_allintra` returns 0
                                // -- geometry DISABLED -- only for allintra
                                // enc_mode > M6 (enc_mode_config.c:8240). This
                                // branch is presets 0..=5, so geometry is always on
                                // and a one-false node keeps its injected edge
                                // shape.
                                //
                                // Do NOT reach for `NsqCfg::for_preset_qp(..).
                                // enabled` here. That is `set_nsq_search_ctrls`
                                // (:6496-6786) -- the SEARCH heuristics -- and it
                                // returns `off()` at p4/p5, which is a different
                                // statement from "no NSQ shapes exist". MEASURED
                                // 2026-08-04: wiring it in force-split every
                                // one-false node at p4/p5 and cost 29 cells --
                                // partial-SB p4 28/36 -> 12/36 and p5 25/36 ->
                                // 13/36. Search-off is not geometry-off.
                                let nsq_geom_enabled =
                                    crate::part_arm::nsq_geom_enabled(sc_arm, speed_config.preset);
                                let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
                                    // SB-EXTENT padded plane, not the raw frame:
                                    // `compute_b64_variance` reads a full 64x64
                                    // unclamped, so a partial SB must read C's
                                    // replicated border rather than running off the
                                    // end (or stride-wrapping into the next row).
                                    // Identical to `encode_input`/`w` on a
                                    // 64-aligned frame -- `sb_input_owned` is None
                                    // there -- so this is byte-neutral.
                                    sb_input,
                                    in_stride,
                                    x0,
                                    y0,
                                    cli_qp as u32,
                                    sb_qindex,
                                    // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                                    // ladder on `picture_qp` + the extended-CRF bump.
                                    crate::pd0::frame_lambda_weight(
                                        u32::from(picture_qp),
                                        tune_iq,
                                        lw_bump,
                                    ),
                                    tables,
                                    if dr.disallow_4x4 { 8 } else { 4 },
                                    // M4/M5: rate_est_level 1 -> coeff_rate_est_lvl 1
                                    // (real PD0 coeff rate). M7/M8's level-2 PD0
                                    // approximation only fires when this is >= 2.
                                    pd0_refined_rate_lvl,
                                    pd0_refined_mode,
                                    pd0_refined_eexit_th,
                                    // max-block variance cap. Allintra: M8+
                                    // only (`get_max_block_size_allintra`'s
                                    // `base_var_th_cap` is `(uint16_t)~0`
                                    // through M7). Video: never
                                    // (`get_max_block_size_default` returns
                                    // `super_block_size` outright). Either way
                                    // false on this `refined` p<=5 branch —
                                    // routed through the arm helper so the two
                                    // PD0 call sites cannot drift apart.
                                    crate::part_arm::max_block_cap_active(
                                        sc_arm,
                                        speed_config.preset,
                                        true,
                                    ),
                                    // NSQ geometry: a one-false node keeps its
                                    // edge shape when NSQ shapes exist, and
                                    // force-splits when they do not (p4/p5, where
                                    // `NsqCfg::for_preset_qp` is `off()`).
                                    nsq_geom_enabled,
                                    // ALIGNED dims — this `refined` path is
                                    // full-SB-gated (see `refined` above), so the
                                    // edge/off branches never fire; passing the
                                    // frame dims keeps the predicate well-defined.
                                    w,
                                    h,
                                    // Tile pixel origin (full-SB refined path is
                                    // single-tile-only in the tested envelope; 0
                                    // when untiled → byte-inert).
                                    tile_sb_row_start * sb_size,
                                    tile_sb_col_start * sb_size,
                                    // Superres chunk B.4: this SB's STALE full-res variance entry.
                                    sb_stale_vars,
                                    // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                                    max_tx_size,
                                    // C `pd0_use_src_samples` (video arm: recon).
                                    pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                );
                                let cq = c_quant.as_ref().unwrap();
                                // 8-BIT lambda even at bd10 — deliberate, not an
                                // oversight. C's `perform_pred_depth_refinement`
                                // (enc_dec_process.c:3017) runs INSIDE the window
                                // where `hbd_md` is forced to 0 (:2965, restored at
                                // :3023), so `is_parent_to_current_deviation_small`
                                // / `is_child_to_current_deviation_small` select
                                // `full_lambda_md[EB_8_BIT_MD]` /
                                // `full_sb_lambda_md[EB_8_BIT_MD]` at BOTH bit
                                // depths, over PD0 costs that are themselves
                                // bit-depth-identical. The bd10 lambda belongs to
                                // the PD1 WALK below, not to this scan.
                                // Whole-128-SB PD0 max/min fold (C
                                // `get_max_min_pd0_depths`). At SB128 (units.len() >
                                // 1) fold every coding-unit quadrant's PD0 eval;
                                // cached across the unit loop. `None` at SB64 → the
                                // scan derives max/min from `eval` alone, unchanged.
                                let sb_max_min = if units.len() > 1 {
                                    if sb_pd0_max_min.is_none() {
                                        let mut mx = 0usize;
                                        let mut mn = 255usize;
                                        for &(ux, uy) in units.iter() {
                                            // Only fold FULL 64x64 units: the m6 PD0
                                            // eval reads a whole 64x64 source block,
                                            // so a partial edge unit (non-64-aligned
                                            // frame) would read out of bounds. Every
                                            // SB128 gate cell is 64-aligned → all
                                            // units full → this never skips. A
                                            // non-64-aligned SB128 frame needs the
                                            // partial-SB (#95) treatment anyway
                                            // (partial units take the fixed-tree
                                            // path, not this refined one).
                                            if ux + unit_size > w || uy + unit_size > h {
                                                continue;
                                            }
                                            crate::pd0::pd0_pick_sb_partition_m6_eval(
                                                sb_input,
                                                in_stride,
                                                ux,
                                                uy,
                                                cli_qp as u32,
                                                sb_qindex,
                                                // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                                                // ladder on `picture_qp` + the extended-CRF bump.
                                                crate::pd0::frame_lambda_weight(
                                                    u32::from(picture_qp),
                                                    false,
                                                    lw_bump,
                                                ),
                                                tables,
                                                if dr.disallow_4x4 { 8 } else { 4 },
                                                pd0_refined_rate_lvl,
                                                pd0_refined_mode,
                                                pd0_refined_eexit_th,
                                                // Same cap predicate as the
                                                // sibling call above; this
                                                // SB128 unit loop skips
                                                // incomplete units outright.
                                                crate::part_arm::max_block_cap_active(
                                                    sc_arm,
                                                    speed_config.preset,
                                                    true,
                                                ),
                                                nsq_geom_enabled,
                                                w,
                                                h,
                                                tile_sb_row_start * sb_size,
                                                tile_sb_col_start * sb_size,
                                                // Superres chunk B.4: this SB's STALE full-res variance entry.
                                                sb_stale_vars,
                                                // C `static_config.max_tx_size`.
                                                max_tx_size,
                                                // C `pd0_use_src_samples` (video arm: recon).
                                                pd0_video_recon
                                                    .then_some((&tile_frame_recon[..], w)),
                                            )
                                            .max_min_picked(&mut mx, &mut mn);
                                        }
                                        sb_pd0_max_min = Some((mx, mn));
                                    }
                                    sb_pd0_max_min
                                } else {
                                    None
                                };
                                let scan = crate::depth_refine::build_refined_scan_at(
                                    &eval,
                                    &dr,
                                    cq.lambda as u64,
                                    tables,
                                    x0,
                                    y0,
                                    sb_max_min,
                                    // C `static_config.max_tx_size` -- the same
                                    // value already threaded into the PD0 entries
                                    // just above, and the reason `max_sq_size` is
                                    // not always 64 (enc_dec_process.c:1814-1817).
                                    max_tx_size,
                                );
                                // Partition rates at the real contexts, from
                                // the same (possibly chained) frame context as
                                // the funnel's syntax rates.
                                let part_rates = match &chain_base {
                                    Some((fc, _)) => crate::depth_refine::PartRates::from_fc(fc),
                                    None => crate::depth_refine::PartRates::from_fc(
                                        &crate::entropy::context::FrameContext::new_default(),
                                    ),
                                };
                                let (u_src, v_src) = chroma_src.unwrap();
                                let mut fx = crate::leaf_funnel::FunnelCtx {
                                    u_src,
                                    v_src,
                                    src10: funnel_src10,
                                    u_recon: &mut fun_u_recon,
                                    v_recon: &mut fun_v_recon,
                                    c_stride: cwid,
                                    ectx: fun_ectx.as_mut().unwrap(),
                                    rates: fun_rates.as_deref().unwrap(),
                                    frame: fun_frame.as_ref().unwrap(),
                                    // bd10 PART axis (task #94): the PD1
                                    // depth-refine + NSQ walk compares LEAF block
                                    // costs, and C's PD1 runs at `hbd_md = 2` (true
                                    // 10-bit) — `test_depth` /
                                    // `test_split_partition` sum
                                    // `block_data[shape][nsi]->cost` from an MDS3
                                    // that predicted, quantized and measured
                                    // distortion at 10 bits. Running that walk on
                                    // 8-bit leaf costs picked C's *bd8* shape. The
                                    // same `full_rd10` chain that closed p7/p8
                                    // (MODE axis) now feeds this walk. bd8 and
                                    // every out-of-envelope bd10 frame keep `None`
                                    // / `false` → byte-IDENTICAL.
                                    y_recon10: if bd10_luma_funnel {
                                        Some(&mut tile_frame_recon10)
                                    } else {
                                        None
                                    },
                                    u_recon10: if bd10_full_rd {
                                        Some(&mut tile_frame_u_recon10)
                                    } else {
                                        None
                                    },
                                    v_recon10: if bd10_full_rd {
                                        Some(&mut tile_frame_v_recon10)
                                    } else {
                                        None
                                    },
                                    // IBC chunk 8: frame IntraBC state + the MD mi grid.
                                    ibc: ibc_state.as_deref(),
                                    inter: inter_md,
                                    ibc_mvp: if ibc_state.is_some() || inter_md.is_some() {
                                        Some(&mut ibc_mvp_grid)
                                    } else {
                                        None
                                    },
                                    ibc_gate: Default::default(),
                                    full_rd10: bd10_full_rd,
                                };
                                let nsq = crate::depth_refine::NsqCfg::for_arm(
                                    sc_arm,
                                    speed_config.preset,
                                    cli_qp as u32,
                                );
                                crate::depth_refine::decide_sb_refined(
                                    &scan,
                                    &mut fx,
                                    sb_input,
                                    in_stride,
                                    &mut tile_frame_recon,
                                    w,
                                    // PD1 partition-rate lambda. C `test_depth` /
                                    // `test_split_partition` /
                                    // `update_skip_nsq_based_on_split_rate` /
                                    // `update_skip_nsq_based_on_sq_recon_dist` all
                                    // select `full_sb_lambda_md[EB_10_BIT_MD]` (==
                                    // `full_lambda_md[EB_10_BIT_MD]`,
                                    // md_process.c:763-764) when `hbd_md != 0`
                                    // (product_coding_loop.c:9725, 9859, 10782,
                                    // 10887). It MUST move with the leaf costs: the
                                    // gates are ratio compares between an
                                    // RDCOST(λ, part_rate, 0) term and a block cost.
                                    // NOTE the refinement SCAN above deliberately
                                    // keeps the 8-bit lambda — see
                                    // `build_refined_scan_at`'s call site.
                                    if bd10_full_rd {
                                        u64::from(crate::pd0::kf_full_lambda_bd10(
                                            base_qindex,
                                            cli_qp as u32,
                                        ))
                                    } else {
                                        cq.lambda as u64
                                    },
                                    &part_rates,
                                    &nsq,
                                    dr.disallow_4x4,
                                    x0,
                                    y0,
                                    // ALIGNED extent for the spec-5.11.4 edge
                                    // predicate. Dead on a 64-aligned frame.
                                    w,
                                    h,
                                    // Whether NSQ geometry exists at this preset,
                                    // which decides what a ONE-FALSE boundary node
                                    // does: inject the single edge shape (H/V), or
                                    // force-split like a both-false node.
                                    //
                                    // NOT hardcoded true. `NsqCfg::for_preset_qp`
                                    // returns `off()` for presets 4 and 5 (its base
                                    // table is nonzero only for 0..=3), and
                                    // `shapes_for_size` already treats `!enabled` as
                                    // square-only -- so at p4/p5 there is no legal
                                    // edge shape to inject and C force-splits.
                                    //
                                    // MEASURED: with `true` here, p5 coded a 16x8 at
                                    // (16,80) on `gradient 72x88 q20` where C splits
                                    // to two 8x8s -- the ONLY structural difference
                                    // between the port's tree and C's on that frame.
                                    nsq_geom_enabled,
                                )
                            } else {
                                // Same computation as pd0_pick_sb_partition_m6
                                // (that fn is exactly _eval(min_sq=8).tree()),
                                // via the eval form so the per-node PD0 costs
                                // are dumpable (SVTAV1_PD0DBG + SVTAV1_DBG_MI)
                                // for depth-flip drills at M6-M8 — the C
                                // counterpart is the PICKPART wrap, which fires
                                // at every preset.
                                let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
                                    sb_input,
                                    in_stride,
                                    x0,
                                    y0,
                                    cli_qp as u32,
                                    sb_qindex,
                                    // C `pcs->lambda_weight` (pd0::frame_lambda_weight): the PSNR
                                    // ladder on `picture_qp` + the extended-CRF bump.
                                    crate::pd0::frame_lambda_weight(
                                        u32::from(picture_qp),
                                        tune_iq,
                                        lw_bump,
                                    ),
                                    tables,
                                    8,
                                    // M6: coeff_rate_est_lvl 1 (real PD0 coeff
                                    // rate, unchanged). M7/M8: 2 -> the C
                                    // perform_tx_pd0 `eob<th ? 6000+eob*500`
                                    // approximation that lowers the parent-NONE
                                    // cost and matches C's partition depth.
                                    pd0_refined_rate_lvl,
                                    pd0_refined_mode,
                                    pd0_refined_eexit_th,
                                    // The max-block variance cap, per ARM.
                                    // Allintra (`get_max_block_size_allintra`,
                                    // enc_mode_config.c:7042): fires at M8+
                                    // only, and stays at sb_size for
                                    // incomplete edge SBs. VIDEO
                                    // (`get_max_block_size_default`, :6991):
                                    // no cap at any preset.
                                    crate::part_arm::max_block_cap_active(
                                        sc_arm,
                                        speed_config.preset,
                                        x0 + 64 <= w && y0 + 64 <= h,
                                    ),
                                    // NSQ geometry, per ARM. Allintra
                                    // (`svt_aom_get_nsq_geom_level_allintra`,
                                    // enc_mode_config.c:8240): presets 0..=6 →
                                    // level 1/2/3 → enabled, presets 7+ → level 0
                                    // → disabled; when disabled a one-false
                                    // boundary node force-splits (no edge shape)
                                    // — the presets 7/8 partial-SB fix. VIDEO
                                    // (`svt_aom_get_nsq_geom_level_default`,
                                    // :8216) never returns 0, so geometry stays
                                    // on at every preset there.
                                    crate::part_arm::nsq_geom_enabled(sc_arm, speed_config.preset),
                                    // ALIGNED dims — the spec-5.11.4 edge grid.
                                    w,
                                    h,
                                    // This tile's pixel origin: the M6 PD0 leaf-cost
                                    // DC prediction must not read across a tile
                                    // boundary (C up/left_available respect tiles).
                                    // 0 for a single-tile frame → byte-inert.
                                    tile_sb_row_start * sb_size,
                                    tile_sb_col_start * sb_size,
                                    // Superres chunk B.4: this SB's STALE full-res variance entry.
                                    sb_stale_vars,
                                    // C `static_config.max_tx_size` (tune IQ sets 32 at qp<=45).
                                    max_tx_size,
                                    // C `pd0_use_src_samples` (video arm: recon).
                                    pd0_video_recon.then_some((&tile_frame_recon[..], w)),
                                );
                                #[cfg(feature = "std")]
                                if crate::dbgenv::pd0dbg()
                                    && crate::depth_refine::nsqdbg_here(x0, y0)
                                {
                                    fn walk(e: &crate::pd0::Pd0Eval, x: usize, y: usize) {
                                        eprintln!(
                                            "NSQDBG PD0 mi=({},{}) sq={} tested={} cost={} split={}",
                                            y / 4,
                                            x / 4,
                                            e.sq,
                                            e.tested,
                                            e.cost,
                                            e.split
                                        );
                                        if let Some(ch) = e.children.as_ref() {
                                            let h = e.sq / 2;
                                            walk(&ch[0], x, y);
                                            walk(&ch[1], x + h, y);
                                            walk(&ch[2], x, y + h);
                                            walk(&ch[3], x + h, y + h);
                                        }
                                    }
                                    walk(&eval, x0, y0);
                                }
                                let tree = eval.tree();
                                let sb_vars = sb_stale_vars.copied().unwrap_or_else(|| {
                                    crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0)
                                });
                                let mut funnel_ctx = if use_funnel {
                                    let (u_src, v_src) = chroma_src.unwrap();
                                    Some(crate::leaf_funnel::FunnelCtx {
                                        u_src,
                                        v_src,
                                        src10: funnel_src10,
                                        u_recon: &mut fun_u_recon,
                                        v_recon: &mut fun_v_recon,
                                        c_stride: cwid,
                                        ectx: fun_ectx.as_mut().unwrap(),
                                        rates: fun_rates.as_deref().unwrap(),
                                        frame: fun_frame.as_ref().unwrap(),
                                        y_recon10: if bd10_luma_funnel {
                                            Some(&mut tile_frame_recon10)
                                        } else {
                                            None
                                        },
                                        u_recon10: if bd10_full_rd {
                                            Some(&mut tile_frame_u_recon10)
                                        } else {
                                            None
                                        },
                                        v_recon10: if bd10_full_rd {
                                            Some(&mut tile_frame_v_recon10)
                                        } else {
                                            None
                                        },
                                        // bd10 post-pass: IBC is bd8-only (the injection
                                        // self-gates on bd10 too).
                                        ibc: None,
                                        inter: None,
                                        ibc_mvp: None,
                                        ibc_gate: Default::default(),
                                        full_rd10: bd10_full_rd,
                                    })
                                } else {
                                    None
                                };
                                crate::partition::encode_fixed_tree(
                                    &sb_input[y0 * in_stride + x0..],
                                    in_stride,
                                    &mut tile_frame_recon,
                                    w,
                                    &tree,
                                    unit_size,
                                    sb_qindex,
                                    &part_config,
                                    x0,
                                    y0,
                                    w,
                                    h,
                                    &sb_vars,
                                    (x0, y0),
                                    funnel_ctx.as_mut(),
                                )
                            }
                        }
                    } else {
                        crate::partition::partition_search_with_config(
                            &encode_input[y0 * w + x0..],
                            w,
                            &mut tile_frame_recon,
                            w,
                            cur_w,
                            cur_h,
                            sb_qindex,
                            sb_lambda,
                            speed_config.max_partition_depth as u32,
                            &part_config,
                            x0,
                            y0,
                            ref_ctx.as_ref(),
                        )
                    };
                    unit_results.push(sb_result);
                } // end per-b64 coding-unit loop

                // Merge the b64 units into this SUPERBLOCK's result. At SB64
                // there is exactly one unit and this is the identity (the
                // moved-out `PartitionResult`, byte-for-byte the old value).
                // At SB128 the four b64 quadrants become the children of a
                // PARTITION_SPLIT node rooted at the 128 square — which is
                // what C codes: an 8-symbol partition symbol at CDF row
                // bsl=4 (ctx 16..19), then the quadrants in Z-order.
                let sb_result =
                    merge_sb_units(unit_results, sb_size, unit_size, ref_frame_data.is_none());

                // Chain: evolve this SB's contexts by re-coding the decided
                // tree (throwaway arithmetic state; only the CDF updates
                // matter) and snapshot them for the following SBs.
                if funnel_chain {
                    let (mut fc, mut cfc) = chain_base.unwrap_or_else(|| {
                        (
                            crate::entropy::context::FrameContext::new_default(),
                            crate::entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                        )
                    });
                    // Issue #16: this is C's MD-side `ec_ctx_array[sb]`, not
                    // the bitstream's context. C's encode pass adapts an
                    // IntraBC txb's tx type on the INTRA DC row here
                    // (`svt_av1_cost_coeffs_txb`'s `is_inter_mode(mode)`
                    // ignores `use_intrabc`), so the per-SB MD rate tables
                    // rebuilt from this chain must see that adaptation — the
                    // same quirk `cost_coeffs_txb`'s `cost_dir` remap reads.
                    // Sticky across snapshots (the struct is cloned).
                    cfc.md_side_ibc_txt_update = true;
                    if let Some(tree) = sb_result.tree.as_ref() {
                        let se = sim_ectx.as_mut().unwrap();
                        if sb_row != sim_prev_sb_row {
                            se.reset_left_for_sb_row();
                            sim_prev_sb_row = sb_row;
                        }
                        let (u_src, v_src) = chroma_src.unwrap();
                        let mut sim_writer =
                            crate::entropy::writer::AomWriter::new(w * h * 2 + 256);
                        let mut sim_chroma = Some(ChromaPass {
                            u_src,
                            v_src,
                            u_recon: &mut sim_u,
                            v_recon: &mut sim_v,
                            stride: cwid,
                            qindex_u,
                            qindex_v,
                            qm_u: qm_levels[1],
                            qm_v: qm_levels[2],
                            c_quant: None,
                        });
                        encode_partition_tree(
                            tree,
                            &mut sim_writer,
                            &mut fc,
                            &mut cfc,
                            base_qindex,
                            se,
                            true,
                            sb_x0,
                            sb_y0,
                            &mut sim_chroma,
                            &mut sim_geom,
                        );
                    }
                    chain_snaps.push((fc, cfc));
                    debug_assert_eq!(chain_snaps.len(), local_sb_index + 1);
                }

                // Keep the per-SB recon list layout for downstream consumers.
                // Append this SB's rows straight from the tile canvas. The
                // staging `vec![0u8; sb_cur_w * sb_cur_h]` this replaces was
                // filled row by row and then copied wholesale into
                // `tile_recon` — an allocation, a zero-fill and a second pass
                // over every pixel of every superblock, for bytes that were
                // already contiguous per row. Same bytes, same order.
                for r in 0..sb_cur_h {
                    let src_off = (sb_y0 + r) * w + sb_x0;
                    tile_recon.extend_from_slice(&tile_frame_recon[src_off..src_off + sb_cur_w]);
                }
                if let Some(tree) = sb_result.tree {
                    tile_trees.push(tree);
                }
            }
        }
        // The bd10 FULL-RD canvases hold this tile's committed 10-bit
        // winner recon (`commit_leaf` writes `win_recon10` / `win_*_recon10`
        // into them per block). Outside that envelope they were never
        // allocated. Note `bd10_luma_funnel` alone is not enough: the eff-M9
        // band (p9..p13) allocates the LUMA canvas without the chroma ones,
        // so the complete 3-plane canvas exists exactly at `bd10_full_rd`.
        let tile_canvas10 = if bd10_full_rd {
            Some((
                tile_frame_recon10,
                tile_frame_u_recon10,
                tile_frame_v_recon10,
            ))
        } else {
            None
        };
        Ok((tile_recon, tile_trees, tile_canvas10))
    };

    // Parallel encoding with std::thread::scope when available, BOUNDED to
    // `thread_count` concurrent OS threads (Feature 4). Previously every tile
    // (up to 256) was spawned at once; now tiles run in fixed-size waves so a
    // heavily-tiled frame cannot oversubscribe the box. Order-preserving:
    // each wave's handles are joined and pushed in tile-index order, and the
    // waves themselves advance in order, so the assembled `Vec` is in exact
    // tile-index order — byte-identical to the old all-at-once collect for any
    // `thread_count`.
    #[cfg(feature = "std")]
    if tile_grid.num_tiles() > 1 {
        let num_tiles = tile_grid.num_tiles();
        let limit = match thread_count {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        }
        .clamp(1, num_tiles);
        return std::thread::scope(|s| {
            // Each tile's closure now yields an `EncodeResult`; collect them in
            // tile-index order and short-circuit to the FIRST error (in tile
            // order). On the success/default path every element is `Ok`, so the
            // collect is byte-identical to the previous `Vec` assembly.
            let mut results = Vec::with_capacity(num_tiles);
            let mut start = 0;
            while start < num_tiles {
                let end = (start + limit).min(num_tiles);
                let handles: Vec<_> = (start..end)
                    .map(|tile_idx| s.spawn(move || encode_one_tile(tile_idx)))
                    .collect();
                for h in handles {
                    results.push(h.join().unwrap());
                }
                start = end;
            }
            results.into_iter().collect()
        });
    }

    // Sequential fallback (single tile, or no-std build).
    (0..tile_grid.num_tiles()).map(encode_one_tile).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_control::RcMode;
    use alloc::vec;

    #[test]
    fn pipeline_encode_single_frame() {
        let mut pipeline = EncodePipeline::new(
            64,
            64,
            8,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 30,
                ..RcConfig::default()
            },
            4,
            64,
        );
        let y_plane = vec![128u8; 64 * 64];
        let bitstream = pipeline.encode_frame(&y_plane, 64);
        assert!(!bitstream.is_empty(), "should produce output");
        assert_eq!(pipeline.frame_count, 1);
    }

    /// Task #95 mono partial SBs at preset 6 (found by zenavif's seam
    /// canary, 2026-08-27). The M6 PD0 keeps NSQ geometry on, so a one-false
    /// edge node is TESTED (rect edge-shape cost) rather than force-split;
    /// the MONO fixed-tree walk then coded that leaf as a full PARTITION_NONE
    /// square — illegal at an edge (spec 5.11.4). Observed failure before the
    /// `encode_fixed_tree` fix, this exact test: `PARTITION_NONE leaf at a
    /// frame edge (64,0) 64x64: has_rows=true has_cols=false` (the pack's
    /// debug_assert; a release build emitted 18 dB garbage / undecodable
    /// streams instead). Presets >= 7 never reached the arm (NSQ geometry
    /// off -> forced SPLIT). Every geometry here is 8-aligned (mono has no
    /// TRUE->ALIGNED padding) and non-64-multiple on at least one axis, so
    /// each frame has a right-edge, bottom-edge and/or both-false SB. A
    /// GRADIENT plane is used on purpose: uniform content codes everything
    /// skip/NONE and would pass through the edge levels without a symbol.
    /// The decode round-trip (rav1d-safe + aom-rs, 56 dB at 96x80) is gated
    /// on the zenavif side (`svt_rs_direct_mono_partial_sb_preset6_roundtrips`).
    #[test]
    fn mono_partial_sb_preset6_edge_leaf_codes_the_edge_shape() {
        for (w, h) in [
            (96usize, 80usize),
            (64, 72),
            (72, 64),
            (16, 72),
            (128, 80),
            (96, 64),
            (200, 136),
        ] {
            let plane: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (((x + y) * 255) / (w + h)) as u8))
                .collect();
            let rc = RcConfig {
                mode: RcMode::Cqp,
                qp: 10,
                ..RcConfig::default()
            };
            let mut pipeline = EncodePipeline::new(w as u32, h as u32, 6, rc, 0, 1);
            let bitstream = pipeline
                .try_encode_frame(&plane, w)
                .unwrap_or_else(|e| panic!("mono {w}x{h} preset 6 must encode: {e}"));
            assert!(
                !bitstream.is_empty(),
                "mono {w}x{h} preset 6 produced no bytes"
            );
        }
    }

    /// Issue #5 chunk 2: QP 0 (base_qindex 0 = coded-lossless) ENCODES on the
    /// 4:2:0 still path, and the encoder's own reconstruction equals the
    /// source exactly — the property the frame header promises a decoder.
    /// (Byte-identity to the C oracle is `tests/lossless_fh_c_capture.rs`;
    /// this is the in-crate no-decoder witness.) Every arm outside the
    /// verified envelope stays a typed refusal, never a wrong stream.
    #[test]
    fn qp0_420_encodes_losslessly_and_out_of_envelope_arms_refuse() {
        let mk = |qp: u8| {
            EncodePipeline::new(
                64,
                64,
                7,
                RcConfig {
                    mode: RcMode::Cqp,
                    qp,
                    ..RcConfig::default()
                },
                0,
                1,
            )
            .with_chroma_420(true)
        };
        // Textured luma + textured chroma so the WHT/quant path carries real
        // residual (a flat frame would pass with the transform disabled).
        let y: Vec<u8> = (0..64 * 64)
            .map(|i| (((i / 64) * 255 / 64) ^ (((i % 64) * 3) & 0x3f)) as u8)
            .collect();
        let u: Vec<u8> = (0..32 * 32).map(|i| (60 + (i * 7) % 130) as u8).collect();
        let v: Vec<u8> = (0..32 * 32).map(|i| (200 - (i * 5) % 150) as u8).collect();

        let mut p0 = mk(0).with_recon_output(true);
        let obu = p0
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect("QP 0 is encodable on the 4:2:0 still path since chunk 2");
        assert!(!obu.is_empty());
        let (ry, ru, rv) = p0.last_recon.as_ref().expect("recon_output");
        assert_eq!(&ry[..], &y[..], "luma recon must equal the source at qp 0");
        assert_eq!(&ru[..], &u[..], "Cb recon must equal the source at qp 0");
        assert_eq!(&rv[..], &v[..], "Cr recon must equal the source at qp 0");
        // Anti-vacuity for the assertion above: the same content at QP 1 is
        // LOSSY (a recon == source check that also passes at qp 1 would prove
        // nothing about the lossless path).
        let mut p1 = mk(1).with_recon_output(true);
        let obu1 = p1
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect("QP 1 keeps encoding");
        assert!(!obu1.is_empty());
        let (ry1, _, _) = p1.last_recon.as_ref().expect("recon_output");
        assert_ne!(&ry1[..], &y[..], "qp 1 must be lossy on this content");

        // Out-of-envelope arms refuse with the typed error.
        let err = mk(0)
            .with_bit_depth(10)
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect_err("QP 0 at 10-bit is not in the verified envelope");
        assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));
        // Fork mode WITHOUT variance boost keeps base_q_idx at 0 while the
        // fork's chroma-q deltas leave the frame outside CodedLossless: that
        // is the refused arm. (With variance boost ON the fork re-signals the
        // frame base above 0 — C rc_aq.c:226 `readjust_base_q_idx` — so the
        // encode is an ordinary lossy one and never reaches the lossless path.)
        let mut fork = mk(0);
        fork.hdr = crate::hdr_mode::HdrForkConfig::hdr_fork();
        fork.hdr.enable_variance_boost = false;
        let err = fork
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect_err("QP 0 in fork mode (base_q_idx 0, chroma deltas) is not CodedLossless");
        assert!(matches!(err.error(), EncodeError::UnsupportedConfig(_)));
    }

    /// Issue #5, legacy surface: the MONOCHROME path has no lossless arm (and
    /// no C oracle), so the panicking `encode_frame` contract turns its QP-0
    /// refusal into a panic (never a silently-corrupt bitstream).
    #[test]
    #[should_panic(expected = "coded-lossless")]
    fn qp0_legacy_mono_encode_panics() {
        let mut pipeline = EncodePipeline::new(
            64,
            64,
            7,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 0,
                ..RcConfig::default()
            },
            0,
            1,
        );
        let y_plane = vec![128u8; 64 * 64];
        let _ = pipeline.encode_frame(&y_plane, 64);
    }

    /// Feature 1: a cooperative stop token that fires mid-frame makes
    /// `try_encode_frame` return `Err(Cancelled)` — no panic, no partial output,
    /// and the frame counter is not advanced (the pipeline stays consistent).
    #[test]
    fn try_encode_cancellation_mid_frame_is_clean_err() {
        use core::sync::atomic::{AtomicUsize, Ordering};

        // Allows the first `limit` checks, then cancels. The frame-entry check
        // plus at least one MD-search SB row pass before it trips — genuinely
        // mid-frame. (`may_stop()` is true, so the guarded in-loop checks run.)
        struct CancelAfter {
            count: AtomicUsize,
            limit: usize,
        }
        impl enough::Stop for CancelAfter {
            fn check(&self) -> core::result::Result<(), enough::StopReason> {
                if self.count.fetch_add(1, Ordering::Relaxed) >= self.limit {
                    Err(enough::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
            fn may_stop(&self) -> bool {
                true
            }
        }

        // 64x192 mono = 3 SB rows, so the per-SB-row stop-check has rows to trip.
        let (w, h) = (64u32, 192u32);
        let y_plane = vec![130u8; (w * h) as usize];
        let mut pipeline = EncodePipeline::new(
            w,
            h,
            8,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 30,
                ..RcConfig::default()
            },
            0,
            1,
        )
        .with_stop(CancelAfter {
            count: AtomicUsize::new(0),
            limit: 2,
        });

        let err = pipeline
            .try_encode_frame(&y_plane, w as usize)
            .expect_err("a fired stop token must yield Err, never Ok or a panic");
        assert!(
            matches!(
                err.error(),
                EncodeError::Cancelled(enough::StopReason::Cancelled)
            ),
            "expected EncodeError::Cancelled, got {err:?}"
        );
        // No partial output (the `Err` carries no bytes) and no state corruption:
        // the `?` fires before the post-encode bookkeeping, so `frame_count`
        // never advanced past 0.
        assert_eq!(
            pipeline.frame_count, 0,
            "a cancelled frame must not advance frame_count"
        );
    }

    /// Feature 3: under `fallible-alloc`, an oversized-dimensions request to a
    /// converted allocation site returns `Err(AllocFailed)` instead of aborting.
    /// `temporal_filter`'s first action is `try_vec![0u16; width * height]?`, so
    /// a `usize::MAX x 1` request fails the reservation (its byte count exceeds
    /// `isize::MAX` on both 32- and 64-bit) and returns before reading the input.
    #[cfg(feature = "fallible-alloc")]
    #[test]
    fn oversized_dims_return_alloc_failed_not_abort() {
        let tiny = [0u8; 4];
        let err = crate::temporal_filter::temporal_filter(
            &tiny,
            &[],
            usize::MAX,
            1,
            1,
            &crate::temporal_filter::TfConfig::default(),
        )
        .expect_err("an unsatisfiable reservation must be Err, not an abort");
        assert!(
            matches!(err.error(), EncodeError::AllocFailed { .. }),
            "expected EncodeError::AllocFailed, got {err:?}"
        );
    }

    #[test]
    fn pipeline_encode_sequence() {
        // 64x64: this test exercises the frame/RC state machine, not block
        // geometry, so it uses the smallest in-scope (full-SB) size.
        let mut pipeline = EncodePipeline::new(
            64,
            64,
            10,
            RcConfig {
                mode: RcMode::Crf,
                qp: 28,
                ..RcConfig::default()
            },
            3,
            16,
        );
        let y_plane = vec![100u8; 64 * 64];
        // The KEY frame encodes; the frame/RC state machine advances for it.
        let bitstream = pipeline
            .try_encode_frame(&y_plane, 64)
            .expect("key frame must encode");
        assert!(!bitstream.is_empty(), "key frame should produce output");
        assert_eq!(pipeline.frame_count, 1);
        assert_eq!(pipeline.rc_state.total_frames, 1);
        // Every following frame is an INTER frame, which `encode_frame_impl`
        // now refuses rather than emitting the undecodable bytes this loop used
        // to collect (measured: aomdec "Corrupt frame detected", dav1d "No data
        // decoded" -- see the refusal's comment). The counters must NOT advance
        // on a refusal, so the caller can retry a supported config.
        for i in 1..5 {
            let err = pipeline
                .try_encode_frame(&y_plane, 64)
                .expect_err("inter frame {i} must be refused");
            assert!(
                matches!(err.error(), crate::EncodeError::UnsupportedConfig(_)),
                "frame {i}: expected UnsupportedConfig, got {err:?}"
            );
        }
        assert_eq!(
            pipeline.frame_count, 1,
            "a refused frame must not advance the counter"
        );
        assert_eq!(pipeline.rc_state.total_frames, 1);
    }

    #[test]
    fn pipeline_key_frame_first() {
        let mut pipeline = EncodePipeline::new(64, 64, 8, RcConfig::default(), 4, 64);
        let y_plane = vec![128u8; 64 * 64];
        let bitstream = pipeline.encode_frame(&y_plane, 64);
        // First frame should be key frame with sequence header
        // OBU structure: TD + SH + Frame
        assert!(bitstream.len() > 10);
    }

    #[test]
    fn pipeline_dpb_updated() {
        let mut pipeline = EncodePipeline::new(64, 64, 8, RcConfig::default(), 4, 64);
        let y_plane = vec![128u8; 64 * 64];
        pipeline.encode_frame(&y_plane, 64);
        // After key frame, all DPB slots should be filled
        assert!(pipeline.dpb.occupied_slots() > 0);
    }

    #[test]
    fn pipeline_encode_420_single_frame() {
        let rc = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..RcConfig::default()
        };
        let mut pipeline = EncodePipeline::new(64, 64, 4, rc.clone(), 0, 1).with_chroma_420(true);
        let mut y = vec![0u8; 64 * 64];
        for (i, px) in y.iter_mut().enumerate() {
            *px = ((i / 64) * 4) as u8;
        }
        // Nontrivial chroma so u/v txbs actually carry coefficients.
        let mut u = vec![0u8; 32 * 32];
        let mut v = vec![0u8; 32 * 32];
        for i in 0..32 * 32 {
            u[i] = (64 + (i / 32) * 3) as u8;
            v[i] = (64 + (i % 32) * 5) as u8;
        }
        let bs_420 = pipeline.encode_frame_420(&y, &u, &v, 64);
        assert!(!bs_420.is_empty());
        assert_eq!(pipeline.frame_count, 1);

        // The mono stream for the same luma must differ (mono_chrome flag,
        // uv_mode symbols, chroma txbs) and the mono path must not require
        // the chroma flag.
        let mut mono = EncodePipeline::new(64, 64, 4, rc, 0, 1);
        let bs_mono = mono.encode_frame(&y, 64);
        assert_ne!(bs_420, bs_mono);
    }

    #[test]
    #[should_panic(expected = "with_chroma_420")]
    fn pipeline_encode_420_requires_flag() {
        let mut pipeline = EncodePipeline::new(64, 64, 4, RcConfig::default(), 0, 1);
        let y = vec![0u8; 64 * 64];
        let u = vec![128u8; 32 * 32];
        let v = vec![128u8; 32 * 32];
        let _ = pipeline.encode_frame_420(&y, &u, &v, 64);
    }

    /// Task #91: the partition alphabet the entropy ctx derives must agree
    /// with the square-size-keyed C rule (`svt_aom_partition_cdf_length`,
    /// entropy_coding.c:922). Before the `bsl` fix, width 128 folded into
    /// the 64 level and returned 10 symbols against the 64x64 CDF row.
    #[test]
    fn partition_ctx_alphabet_matches_c_rule_at_every_square_size() {
        let ectx = EntropyCtx::new(64, 64, true, true, false, 8);
        for sq in [8usize, 16, 32, 64, 128] {
            let (ctx, nsymbs) = ectx.partition_ctx(0, 0, sq);
            assert_eq!(
                nsymbs,
                crate::sb128_geom::partition_cdf_length(sq),
                "alphabet mismatch at square {sq} (ctx {ctx})"
            );
            // ctx rows are 4 per level, level = bsl; 128 must land in the
            // top group (16..=19) that carries the 8-symbol rows.
            let expect_group = match sq {
                8 => 0..=3,
                16 => 4..=7,
                32 => 8..=11,
                64 => 12..=15,
                _ => 16..=19,
            };
            assert!(expect_group.contains(&ctx), "square {sq} -> ctx {ctx}");
        }
    }

    /// The SB-size resolution honors the C derivation and the explicit
    /// override, and NEVER yields anything but 64/128.
    ///
    /// Task #91 chunk 3 flipped `sb128_encode_supported` on, so an SB128
    /// cell now RESOLVES to 128 and is genuinely coded at 128 (the
    /// `sb128_gate.sh` cells byte-match real SvtAv1EncApp). Before the
    /// chunk this test asserted the fallback-to-64 behaviour; the
    /// assertions below are the same contract with the capability gate
    /// open, plus a direct `resolve_sb_size` check so the fallback
    /// mechanism itself stays covered even though nothing triggers it.
    #[test]
    fn sb_size_resolution_and_fallback() {
        // Small frame -> C rule says 64, no fallback, so nothing about the
        // pre-SB128 gate cells changes.
        let p = EncodePipeline::new(64, 64, 0, RcConfig::default(), 0, 1);
        assert_eq!(p.sb_size, 64);
        assert_eq!(p.derived_sb_size, 64);
        assert!(!p.sb128_fallback);
        // A frame C codes at 128 (512x384 preset 0, MEASURED against the
        // real encoder's sequence header): the port now codes it at 128 too
        // and does NOT fall back.
        let p = EncodePipeline::new(512, 384, 0, RcConfig::default(), 0, 1);
        assert_eq!(p.sb_size, 128, "512x384 p0 is an SB128 cell in C");
        assert_eq!(p.derived_sb_size, 128);
        assert!(
            !p.sb128_fallback,
            "the SB128 encode path is capability-enabled"
        );
        // preset 2 at the same size is genuinely SB64 in C.
        let p = EncodePipeline::new(512, 384, 2, RcConfig::default(), 0, 1);
        assert_eq!(p.sb_size, 64);
        assert!(!p.sb128_fallback);
        // Explicit override asks for 128 on a frame the C rule puts at 64:
        // honoured, because the walk is size- (and preset-) agnostic.
        let p = EncodePipeline::new(64, 64, 0, RcConfig::default(), 0, 1).with_sb_size(Some(128));
        assert_eq!(p.sb_size, 128);
        assert_eq!(
            p.derived_sb_size, 64,
            "the C rule's own answer is preserved"
        );
        assert!(!p.sb128_fallback);
        // Explicit 64 on an SB128 cell pins 64 — the anti-vacuity witness's
        // "force the port to the wrong size" mode. Not a fallback: the
        // override chose it.
        let p = EncodePipeline::new(512, 384, 0, RcConfig::default(), 0, 1).with_sb_size(Some(64));
        assert_eq!(p.sb_size, 64);
        assert!(!p.sb128_fallback);
        assert_eq!(p.derived_sb_size, 128);
        // ... and re-resolving with None returns to the DERIVED value, not
        // to whatever the last override happened to be.
        let p = p.with_sb_size(None);
        assert_eq!(p.sb_size, 128);
        assert_eq!(p.derived_sb_size, 128);
        assert!(!p.sb128_fallback);
        // The fallback mechanism itself: still wired, and the ONLY thing
        // that can trigger it is `sb128_encode_supported` going false again
        // (e.g. if a future chunk re-gates a preset). Assert the contract
        // directly so the plumbing cannot rot while it is unreachable.
        assert_eq!(EncodePipeline::resolve_sb_size(128, None, 0), (128, false));
        assert_eq!(
            EncodePipeline::resolve_sb_size(128, Some(64), 0),
            (64, false)
        );
        assert_eq!(
            EncodePipeline::resolve_sb_size(64, Some(128), 0),
            (128, false)
        );
    }

    /// Task #91: the b64 coding units of one superblock, in C's coding
    /// order. SB64 must yield exactly the SB itself (this is what makes
    /// every SB64 path byte-identical by construction); SB128 must yield
    /// the Z-order quadrants with off-frame ones dropped (C
    /// `svt_aom_write_modes_sb`'s `mi_row + y_idx >= mi_rows` continue).
    #[test]
    fn sb_coding_units_match_c_walk_order() {
        use crate::sb128_geom::sb_coding_units;
        // SB64: always exactly one unit, whatever the frame extent.
        assert_eq!(sb_coding_units(0, 0, 64, 512, 384), alloc::vec![(0, 0)]);
        assert_eq!(
            sb_coding_units(448, 320, 64, 512, 384),
            alloc::vec![(448, 320)]
        );
        // SB128 interior: four quadrants, Z-order (raster within the SB).
        assert_eq!(
            sb_coding_units(0, 0, 128, 512, 384),
            alloc::vec![(0, 0), (64, 0), (0, 64), (64, 64)]
        );
        assert_eq!(
            sb_coding_units(256, 128, 128, 512, 384),
            alloc::vec![(256, 128), (320, 128), (256, 192), (320, 192)]
        );
        // Partial 128 COLUMN (448 = 3*128 + 64): the right quadrants are
        // off-frame and code nothing.
        assert_eq!(
            sb_coding_units(384, 0, 128, 448, 384),
            alloc::vec![(384, 0), (384, 64)]
        );
        // Partial 128 ROW (448 = 3*128 + 64 vertically).
        assert_eq!(
            sb_coding_units(0, 384, 128, 512, 448),
            alloc::vec![(0, 384), (64, 384)]
        );
        // Both partial: only the top-left quadrant survives.
        assert_eq!(
            sb_coding_units(384, 384, 128, 448, 448),
            alloc::vec![(384, 384)]
        );
    }
}

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
mod inter_tile_byte_gate {
    use super::*;
    use crate::entropy::writer::AomWriter;
    use crate::port_entropy_inter::Neighbors;
    use crate::port_entropy_inter::block::{
        InterFrameSyntax, InterModeInfo, write_inter_mode_info,
    };
    use crate::port_entropy_inter::modes::{MotionMode, TransformationType};
    use crate::port_entropy_inter::refframe::ReferenceMode;
    use crate::rate_control::RcMode;
    use alloc::vec;
    use svtav1_types::block::BlockSize;
    use svtav1_types::motion::Mv;
    use svtav1_types::prediction::PredictionMode;

    /// C's `gradient` plane, as `tools/identity_run` builds it.
    fn gradient(w: usize, h: usize) -> Vec<u8> {
        (0..h)
            .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
            .collect()
    }

    /// C's frame-1 TILE for the reference cell — the three bytes after the
    /// 2-byte temporal delimiter, the 2-byte OBU header and the 15-byte frame
    /// header of a 22-byte frame. Captured from
    /// `tools/identity_diff_inter.sh 64 64 40 6 2 gradient`'s `c.obu.pts1`:
    ///
    /// ```text
    /// 12 00 32 12 | 30 02 00 80 00 db 3b 40 00 00 04 04 e0 1c 00 | 94 9a b0
    /// ```
    ///
    /// The 15 header bytes are already byte-identical to the port's
    /// (`tools/inter_fh_gate.sh`, docs/INTER-ENCODE-PLAN.md §1r).
    const C_INTER_TILE: [u8; 3] = [0x94, 0x9a, 0xb0];

    #[test]
    fn the_inter_tile_matches_c_from_cs_measured_decision() {
        let (w, h) = (64usize, 64usize);
        let y = gradient(w, h);
        let uv = vec![128u8; (w / 2) * (h / 2)];

        // The reference cell's configuration, field for field with
        // `tools/identity_run`'s video arm: preset 6, CQP 40, flat GOP
        // (hierarchical_levels 0), intra_period 64, 4:2:0, 8-bit.
        let mut pipeline = EncodePipeline::new(
            w as u32,
            h as u32,
            6,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 40,
                ..RcConfig::default()
            },
            0,
            64,
        )
        .with_bit_depth(8)
        .with_chroma_420(true);

        let f0 = pipeline
            .try_encode_frame_420(&y, &uv, &uv, w)
            .expect("the video-mode key frame encodes");
        // ANTI-VACUITY, and the reason it is an assertion rather than a
        // comment: everything below is read out of the frame-0 encode's
        // state, so a configuration that silently produced a DIFFERENT
        // frame 0 would hand the tile writer plausible-looking CDFs from the
        // wrong encode. 961 B is C's byte count for this cell
        // (docs/INTER-ENCODE-PLAN.md §1q) and the port matches it exactly.
        assert_eq!(
            f0.len(),
            961,
            "this is not the reference cell — frame 0 must be C's 961 bytes"
        );

        // The CDF continuation under test: frame 1's header names
        // primary_ref_frame = 0, and ref_frame_idx[0] = 0, so the tile starts
        // from DPB slot 0's saved end-of-frame state.
        let saved = pipeline
            .dpb
            .get(0)
            .and_then(|r| r.frame_cdfs.clone())
            .expect("the key frame stored its end-of-frame CDFs on the DPB slot it refreshed");
        // The prologue and the mode-info group are assembled by `tile_from`
        // below, so the negative control at the end of this test runs the
        // IDENTICAL path with only the CDF state swapped.
        // --- the inter mode-info group ------------------------------------
        let nb = Neighbors {
            above: None,
            left: None,
            up_available: false,
            left_available: false,
        };
        let gm = [TransformationType::Identity; 8];
        let ref_order_hint = [0i32; 7];
        let frame = InterFrameSyntax {
            // frm_hdr reference_select = 1 (tools/fh_fields.py --index 1).
            reference_mode: ReferenceMode::Select,
            // is_filter_switchable = 1 -> SWITCHABLE, which is
            // SWITCHABLE_FILTERS + 1 = 4, NOT 3 (definitions.h:844-846).
            interpolation_filter: crate::port_enc_mode_config::md_config::SWITCHABLE,
            // The video arm never enables dual filter at any preset
            // (`speed_config.rs`, asserted there for M0..M13).
            enable_dual_filter: false,
            enable_interintra_compound: false,
            enable_masked_compound: false,
            enable_jnt_comp: false,
            enable_order_hint: true,
            order_hint_bits: crate::entropy::obu::ORDER_HINT_BITS,
            is_motion_mode_switchable: true,
            allow_warped_motion: true,
            allow_high_precision_mv: false,
            force_integer_mv: false,
            gm_wmtype: &gm,
            cur_order_hint: 1,
            ref_order_hint: &ref_order_hint,
        };
        // C's measured decision — see this module's doc comment.
        let blk = InterModeInfo {
            bsize: BlockSize::Block64x64,
            mode: PredictionMode::NewMv,
            ref_frame: [1, -1], // LAST_FRAME, NONE
            mv: [Mv { x: -24, y: 0 }, Mv { x: 0, y: 0 }],
            pred_mv: [Mv { x: 0, y: 0 }, Mv { x: 0, y: 0 }],
            inter_mode_ctx: 8,
            drl: crate::port_entropy_inter::modes::DrlBlock {
                drl_ctx: [-1, -1],
                drl_ctx_near: [0, 0],
                drl_index: 0,
            },
            interintra: None,
            motion_mode: MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            overlappable_neighbors: 0,
            compound: None,
            interp_filters: 0, // EIGHTTAP_REGULAR in both directions
            skip_mode: false,
        };
        // `FrameContext` carries `inter` and `nmvc` inline, and the writer
        // takes them as separate `&mut`s (C reaches them through one `fc`
        // pointer). Split the copies out, then write them BACK — a caller
        // that dropped them would silently code the next block against
        // unadapted inter CDFs.
        let tile = tile_from(&saved, &nb, &frame, &blk);
        assert_eq!(
            tile.as_slice(),
            &C_INTER_TILE[..],
            "the inter tile does not match C's; port {tile:02x?} vs C {C_INTER_TILE:02x?}"
        );

        // NEGATIVE CONTROL, and it is the reason this test is evidence about
        // CDF CONTINUATION rather than only about the writers: the SAME
        // decision coded from DEFAULT CDFs produces different bytes. If it
        // did not, the gate would pass with the restore deleted and would be
        // saying nothing about the feature it was built for.
        let from_defaults = tile_from(
            &crate::port_frame_cdf::FrameCdfs {
                fc: crate::entropy::context::FrameContext::new_default(),
                coeff: crate::entropy::coeff_c::CoeffFc::default_for_qindex(160),
            },
            &nb,
            &frame,
            &blk,
        );
        assert_ne!(
            from_defaults.as_slice(),
            &C_INTER_TILE[..],
            "default CDFs reproduced C's tile — this gate cannot see the restore"
        );
    }

    /// The tile assembly of the test above, factored so the negative control
    /// runs the IDENTICAL path with only the CDF state swapped. A second
    /// hand-written copy could differ somewhere else and hide the point.
    fn tile_from(
        cdfs: &crate::port_frame_cdf::FrameCdfs,
        nb: &Neighbors,
        frame: &InterFrameSyntax<'_>,
        blk: &InterModeInfo,
    ) -> Vec<u8> {
        let mut fc = cdfs.fc.clone();
        let mut w_ = AomWriter::new(256);
        let ectx = EntropyCtx::new(16, 16, false, true, false, 8);
        let (part_ctx, nsymbs) = ectx.partition_ctx(0, 0, 64);
        crate::entropy::context::write_partition_edge(
            &mut w_, &mut fc, part_ctx, 0, nsymbs, false, true, true,
        );
        crate::entropy::context::write_skip(&mut w_, &mut fc, 0, true);
        crate::entropy::context::write_intra_inter(&mut w_, &mut fc, 0, true);
        let mut ic = fc.inter.clone();
        let mut nmvc = fc.nmvc.clone();
        write_inter_mode_info(&mut w_, &mut fc, &mut ic, &mut nmvc, nb, frame, blk);
        w_.done().to_vec()
    }
}

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
mod inter_decision_probe {
    use super::*;
    use crate::inter_mvp::{
        InterMvpEnv, OrderHintInfo, TplMvRef, get_av1_mv_pred_drl, setup_ref_mv_list,
    };
    use crate::intrabc::TileMiBounds;
    use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
    use crate::port_md::drl::{ChooseDrlCtx, av1_drl_ctx, choose_best_av1_mv_pred};
    use crate::port_md::pme::MvCostTable;
    use alloc::vec;
    use svtav1_types::motion::{Mv, WarpedMotionParams};
    use svtav1_types::prediction::PredictionMode;

    /// C `BLOCK_64X64` (definitions.h block order) — the bsize
    /// `SVT_CINTER_OUT` printed (`bsize=12`).
    const BLOCK_64X64: usize = 12;
    /// C `LAST_FRAME`.
    const LAST_FRAME: i8 = 1;

    /// The reference cell's frame-1 MVP inputs, exactly as the frame header
    /// the port already writes byte-identically describes them
    /// (`tools/fh_fields.py --index 1`: `primary_ref_frame 0`,
    /// `allow_high_precision_mv 0`, `use_ref_frame_mvs 1`,
    /// `reference_select 1`, `order_hint 1`).
    ///
    /// `use_ref_frame_mvs = 1` is load-bearing and easy to get wrong: it is
    /// the ONLY path that sets the `GLOBALMV_OFFSET` bit on a block with no
    /// coded neighbours, and that bit is the whole of C's `imc = 8`. With
    /// the flag read as 0 the port would produce `inter_mode_ctx = 0`,
    /// which selects a different `newmv` CDF row and a different tile.
    #[test]
    fn the_ports_own_mvp_stack_reproduces_cs_pred_mv_mode_ctx_and_drl() {
        // 64x64 frame => 16x16 mi cells, one tile.
        let (mi_rows, mi_cols) = (16i32, 16i32);
        let tile = TileMiBounds {
            mi_col_start: 0,
            mi_col_end: mi_cols,
            mi_row_start: 0,
            mi_row_end: mi_rows,
        };
        // The MD mi grid at the FIRST block of the frame: every cell is the
        // default intra entry, because nothing has been committed yet. This
        // is `docs/INTER-ENCODE-PLAN.md` §1s item 2's grid — the type is
        // already the one `setup_ref_mv_list` reads.
        let entries = vec![MvpMiEntry::default(); (mi_rows * mi_cols) as usize];
        let grid = MvpGrid {
            entries: &entries,
            stride: mi_cols,
            base: 0,
        };
        let ctx = derive_block_ctx(0, 0, BLOCK_64X64, mi_rows, mi_cols, tile, 16);
        assert!(
            !ctx.up_available && !ctx.left_available,
            "the frame's first block has no coded neighbours — if this flips, \
             every number below is about a different block"
        );

        let gm = [WarpedMotionParams::default(); 8];
        // C's tpl_mvs after `av1_setup_motion_field`'s reset: INVALID_MV
        // everywhere, so every `add_tpl_ref_mv` returns 0.
        let tpl_stride = mi_cols >> 1;
        let tpl_mvs =
            vec![TplMvRef::default(); ((mi_rows >> 1) + 8) as usize * tpl_stride as usize];
        let env = InterMvpEnv {
            global_motion: &gm,
            ref_frame_sign_bias: [0; 8],
            allow_high_precision_mv: false,
            force_integer_mv: false,
            use_ref_frame_mvs: true,
            order_hint_info: OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
            },
            cur_order_hint: 1,
            ref_order_hint: [0; 8],
            tpl_mvs: &tpl_mvs,
            tpl_stride,
            sb64_sq_no4xn_geom: true,
            symmetric_refs: false,
        };

        // C `svt_aom_generate_av1_mvp_table`'s gm_mv for an IDENTITY global
        // motion model is the zero MV.
        let stack = setup_ref_mv_list(&grid, &ctx, &env, LAST_FRAME, [Mv::ZERO; 2]);

        // --- C `imc=8` -------------------------------------------------
        assert_eq!(
            stack.mode_context, 8,
            "inter_mode_ctx: C's SVT_CINTER_OUT prints imc=8"
        );
        // ... and it is the GLOBALMV bit, not an accident of another term.
        assert_eq!(
            stack.count, 0,
            "no coded neighbours and an INVALID_MV temporal field => empty stack"
        );

        // NEGATIVE CONTROL for the paragraph above: the ONLY term that can
        // set bit 3 on a neighbourless block is the temporal-MVP block's
        // `is_available == 0`, so reading `use_ref_frame_mvs` as 0 gives a
        // DIFFERENT mode context — and hence a different `newmv` CDF row and
        // a different tile. Without this the `== 8` assertion could not
        // distinguish "the port reproduces C" from "8 falls out anyway".
        let off = InterMvpEnv {
            use_ref_frame_mvs: false,
            ..env
        };
        let no_mfmv = setup_ref_mv_list(&grid, &ctx, &off, LAST_FRAME, [Mv::ZERO; 2]);
        assert_eq!(
            no_mfmv.mode_context, 0,
            "with use_ref_frame_mvs = 0 the GLOBALMV bit must NOT be set"
        );

        // --- C `drl=0`, `pmv0=0,0`, `drlctx=-1,-1` ---------------------
        // The cost table is unread on this path (`max_drl_index == 1`
        // short-circuits before any MV is priced); a zeroed one makes that
        // explicit instead of smuggling in a rate model the cell does not
        // exercise.
        let nmv_cost = MvCostTable {
            joint: [0; 4],
            comp: [
                vec![0i32; crate::port_md::pme::MV_VALS],
                vec![0i32; crate::port_md::pme::MV_VALS],
            ],
        };
        let drl_ctx = ChooseDrlCtx {
            shut_fast_rate: false,
            approx_inter_rate: 0,
            ref_mv_stack: &stack.stack,
            ref_mv_count: stack.count,
            nmv_cost: &nmv_cost,
            drl_mode_fac_bits: &[[0; 2]; 3],
        };
        let mut drl_index = 0xFFu8;
        let mut pred_mv = [Mv { x: 111, y: 222 }; 2];
        choose_best_av1_mv_pred(
            &drl_ctx,
            PredictionMode::NewMv,
            Mv { x: -24, y: 0 },
            Mv::ZERO,
            &mut drl_index,
            &mut pred_mv,
        );
        assert_eq!(drl_index, 0, "drl_index: C prints drl=0");
        assert_eq!(
            pred_mv[0],
            Mv::ZERO,
            "pred_mv: C prints pmv0=0,0 — this is what the writer differences the coded MV from"
        );

        // `get_av1_mv_pred_drl` is what MD calls to fill `nearestmv`; on a
        // single-ref NEWMV with an empty stack it must agree.
        let pred = get_av1_mv_pred_drl(
            &stack,
            false,
            PredictionMode::NewMv as u8,
            0,
            crate::inter_mvp::DrlMvPred::default(),
        );
        assert_eq!(pred.ref_mv[0], Mv::ZERO, "get_av1_mv_pred_drl's ref_mv[0]");

        // C's `drl_ctx[2] = {-1, -1}` is the "never computed" sentinel: MD
        // only fills it when `max_drl_index > 1`, which needs a stack with
        // more than one candidate. With `count == 0` the writer must not
        // emit a DRL symbol at all — corroborated independently by the
        // frame-context delta, which shows NO `drl` CDF moving on C's tile
        // (docs/INTER-ENCODE-PLAN.md §1s).
        assert_eq!(
            crate::port_md::predicates::get_max_drl_index(stack.count, PredictionMode::NewMv),
            1,
            "max_drl_index must be 1, i.e. no DRL symbol is coded"
        );
        // A POSITIVE CONTROL on that reasoning: with a two-candidate stack
        // the same code DOES ask for a DRL context, so the assertion above
        // is about this cell rather than about a function that always
        // returns 1.
        let mut two = stack;
        two.count = 2;
        two.stack[0].weight = 700;
        two.stack[1].weight = 700;
        assert!(
            crate::port_md::predicates::get_max_drl_index(two.count, PredictionMode::NewMv) > 1,
            "a 2-candidate stack must signal DRL — otherwise this test proves nothing"
        );
        assert_eq!(av1_drl_ctx(&two.stack, 0), 0);
    }

    /// **Does the port's OWN motion search find C's MV?**
    ///
    /// C's `SVT_CINTER_OUT` prints `mv0=0,-24` — eighth-pel, i.e. the
    /// full-pel `(-3, 0)` that the harness's 3-pixel right-shift makes
    /// exact. The port has two searches: the pre-campaign homegrown
    /// `crate::motion_est`, which lands on `-22` (a quarter pel short of an
    /// EXACT integer match — measured, `docs/INTER-ENCODE-PLAN.md` §1s), and
    /// the wholesale port of `motion_estimation.c` in `crate::inter_me`,
    /// which nothing calls. This drives the second one on the reference
    /// cell's actual planes.
    ///
    /// **Evidence tier: this is NOT a parity claim.** Most of
    /// `motion_estimation.c` is `static`, so the composed search can only
    /// reach tier 4 (`docs/WORKING-ON-THIS.md` §4); what a green run here
    /// says is "the ported search, configured by the ported
    /// `svt_aom_sig_deriv_me`, recovers this cell's MV" — a reachability and
    /// wiring result, not a bit-exactness one. The tier-1 kernels underneath
    /// it are gated in `tests/c_parity_inter_me.rs`.
    ///
    /// It doubles as the POSITIVE CONTROL for
    /// `port_enc_mode_config::me::apply_me_signals`: the search area it
    /// installs is what makes a `-3` MV reachable at all, so a bridge that
    /// silently wrote nothing would fail here rather than pass quietly.
    #[test]
    fn the_ports_own_svt_motion_search_finds_cs_mv_on_the_reference_cell() {
        use crate::inter_me::context::{
            MeB64Output, MeContext, MeDsRef, MePicParams, MeRefs, MeSrcBufs, Plane,
        };
        use crate::inter_me::context::{PU_8X8_0, PU_16X16_0, PU_32X32_0, PU_64X64};
        use crate::inter_me::motion_estimation_b64;
        use crate::port_enc_mode_config::ResolutionRange;
        use crate::port_enc_mode_config::me::{MeDerivInputs, apply_me_signals, sig_deriv_me};
        use crate::port_preanalysis::{downsample_2d, generate_padding};

        const W: usize = 64;
        const H: usize = 64;
        /// `tools/identity_run`'s `SVTAV1_FRAME_SHIFT` default.
        const SHIFT: usize = 3;

        // `tools/identity_run`'s `gradient` plane and its frame-1 translate.
        let f0: Vec<u8> = (0..H)
            .flat_map(|r| (0..W).map(move |c| (((r * 255) / H) as u8) ^ (((c * 3) & 0x3f) as u8)))
            .collect();
        let mut f1 = vec![0u8; W * H];
        for r in 0..H {
            for c in 0..W {
                f1[r * W + c] = f0[r * W + c.saturating_sub(SHIFT)];
            }
        }
        assert_ne!(f0, f1, "the translate must actually move the picture");

        // The REFERENCE picture, with C's replicated margin
        // (`svt_aom_generate_padding`, tier-1 gated in
        // `c_parity_preanalysis.rs`). The margin is not decoration here: at
        // MV -3 the block's left three columns read OUTSIDE the frame, and
        // the harness built frame 1 by replicating column 0 — so the match
        // is EXACT only against a replicated margin, and only then can the
        // residual be zero, which is what C's `skip = 1` records.
        const BORDER: usize = 64;
        let stride = W + 2 * BORDER;
        let rows = H + 2 * BORDER;
        let org = BORDER * stride + BORDER;
        let mut refbuf = vec![0u8; stride * rows];
        for r in 0..H {
            refbuf[org + r * stride..org + r * stride + W].copy_from_slice(&f0[r * W..r * W + W]);
        }
        generate_padding(&mut refbuf, org, stride, W, H, BORDER, BORDER);
        assert_eq!(
            refbuf[org - 3],
            f0[0],
            "the left margin must replicate column 0 — that is what makes the -3 match exact"
        );

        // The 1/4 and 1/16 luma pyramids HME levels 1 and 0 search, built
        // with the same `svt_aom_downsample_2d` C uses
        // (`svt_aom_downsample_filtering_input_picture`), each padded to its
        // own border.
        let mk_ds =
            |src: &[u8], s_org: usize, s_stride: usize, sw: usize, sh: usize, step: usize| {
                let (dw, dh) = (sw / step, sh / step);
                let db = BORDER / step;
                let dstride = dw + 2 * db;
                let dorg = db * dstride + db;
                let mut buf = vec![0u8; dstride * (dh + 2 * db)];
                downsample_2d(
                    &src[s_org..],
                    s_stride,
                    sw,
                    sh,
                    &mut buf[dorg..],
                    dstride,
                    step,
                );
                generate_padding(&mut buf, dorg, dstride, dw, dh, db, db);
                (buf, dorg, dstride, dw, dh, db)
            };
        let (qbuf, qorg, qstride, qw, qh, qb) = mk_ds(&refbuf, org, stride, W, H, 2);
        let (sbuf, sorg, sstride, sw_, sh_, sb_) = mk_ds(&qbuf, qorg, qstride, qw, qh, 2);

        let refs = MeRefs {
            arr: [
                [
                    Some(MeDsRef {
                        picture: Plane {
                            data: &refbuf,
                            org,
                            stride,
                            width: W as u16,
                            height: H as u16,
                            border: BORDER as u16,
                        },
                        quarter: Plane {
                            data: &qbuf,
                            org: qorg,
                            stride: qstride,
                            width: qw as u16,
                            height: qh as u16,
                            border: qb as u16,
                        },
                        sixteenth: Plane {
                            data: &sbuf,
                            org: sorg,
                            stride: sstride,
                            width: sw_ as u16,
                            height: sh_ as u16,
                            border: sb_ as u16,
                        },
                        picture_number: 0,
                    }),
                    None,
                    None,
                    None,
                ],
                [None, None, None, None],
            ],
        };

        // The SOURCE b64 and its two decimations (C `quarter_b64_buffer` /
        // `sixteenth_b64_buffer`).
        let mut src_q = vec![0u8; (W / 2) * (H / 2)];
        downsample_2d(&f1, W, W, H, &mut src_q, W / 2, 2);
        let mut src_s = vec![0u8; (W / 4) * (H / 4)];
        downsample_2d(&src_q, W / 2, W / 2, H / 2, &mut src_s, W / 4, 2);
        let src = MeSrcBufs {
            b64: &f1,
            b64_stride: W,
            quarter: &src_q,
            quarter_stride: W / 2,
            sixteenth: &src_s,
            sixteenth_stride: W / 4,
        };

        let pic = MePicParams {
            picture_number: 1,
            aligned_width: W as i16,
            aligned_height: H as i16,
            enhanced_width: W as u32,
            enhanced_height: H as u32,
            ahd_error: u32::MAX,
            input_resolution: 0,
            enable_me_8x8: true,
            enable_me_16x16: true,
            max_number_of_pus_per_sb: 85,
            hierarchical_levels: 0,
            similar_brightness_refs: false,
            frame_is_boosted: false,
            frame_is_leaf: false,
            gm_enabled: false,
            only_l_bwd: false,
            max_cand: 23,
            max_refs: 7,
            max_l0: 4,
            b64_geom_width: W as u32,
            b64_geom_height: H as u32,
            input_width: W as u16,
            input_height: H as u16,
        };

        // `frame_is_boosted` is the one derivation input this cell does not
        // pin from a dump, so BOTH values are swept: the answer must not
        // depend on a guess.
        for &boosted in &[false, true] {
            let signals = sig_deriv_me(MeDerivInputs {
                enc_mode: 6,
                sc_class5: 0,
                input_resolution: ResolutionRange::R240p,
                rtc_tune: false,
                is_base: boosted,
                hierarchical_levels: 0,
                // `enc_mode_config.c:1987-1999` sets all three
                // unconditionally (quoted in `port_preanalysis`).
                enable_hme_flag: 1,
                enable_hme_level0_flag: 1,
                enable_hme_level1_flag: 1,
                enable_hme_level2_flag: 1,
                use_best_me_unipred_cand_only: 0,
                me_qp_based_th_scaling: false,
                hme_qp_based_th_scaling: false,
                qp: 40,
                safe_limit_nref: 0,
                safe_limit_zz_th: 0,
            });
            let mut me = MeContext::default();
            apply_me_signals(&mut me, &signals);
            // POSITIVE CONTROL that the bridge wrote something: a default
            // `MeContext` has a ZERO search area, in which no MV but (0,0)
            // is reachable.
            assert!(
                me.me_sa.sa_max.width >= 8 && me.me_sa.sa_max.height >= 3,
                "apply_me_signals installed no search area (boosted={boosted})"
            );
            me.num_of_list_to_search = 1;
            me.num_of_ref_pic_to_search = [1, 0];
            me.me_type = crate::inter_me::context::MeType::OpenLoop;

            let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
            motion_estimation_b64(&pic, 0, 0, &mut me, &src, &refs, &mut out);

            assert_eq!(
                (out.me_mv_array[0].x, out.me_mv_array[0].y),
                (-(SHIFT as i16), 0),
                "the ported SVT search must recover the cell's full-pel MV \
                 (boosted={boosted}); C codes it as the eighth-pel {}",
                -(SHIFT as i32) * 8
            );
            assert_eq!(
                me.p_sb_best_sad[0][0][PU_64X64], 0,
                "an exact translation against a replicated margin has SAD 0 \
                 (boosted={boosted}) — a non-zero SAD here is what would make \
                 C's skip = 1 unreachable"
            );
            for i in [PU_32X32_0, PU_16X16_0, PU_8X8_0] {
                assert_eq!(me.p_sb_best_sad[0][0][i], 0, "sub-PU {i} SAD");
            }
        }
    }

    /// **The REAL pack walk writes C's tile**, not a hand-assembled
    /// composition of the same writers.
    ///
    /// `inter_tile_byte_gate` drives
    /// `port_entropy_inter::block::write_inter_mode_info` directly, with
    /// every field of C's measured decision spelled out at the call site —
    /// including `pred_mv`, `inter_mode_ctx` and `drl_ctx`. This test runs
    /// the same cell through `encode_block_syntax`, the function the frame's
    /// entropy walk actually calls, and hands it only what MODE DECISION
    /// decides (`partition::InterDecision`): the mode, the reference, the MV
    /// and `drl_index`. The three context fields are DERIVED inside the pack
    /// from `EntropyCtx`'s committed mode-info grid.
    ///
    /// So it gates two things the direct call cannot see:
    ///
    /// * the frame-level `InterSyntaxState` -> `InterFrameSyntax` plumbing,
    ///   and the neighbour derivation from the pack's own mi grid;
    /// * that the DERIVED `pred_mv` / `inter_mode_ctx` / `drl_ctx` equal
    ///   C's measured ones on a grid with nothing committed yet — i.e. that
    ///   moving them out of the MD payload did not change the bytes.
    ///
    /// It also pins the prologue this arm now shares with every other
    /// block: `write_skip` and `write_intra_inter` come from
    /// `encode_block_syntax` itself here, where the older gate wrote them by
    /// hand.
    #[test]
    fn the_real_pack_walk_writes_cs_inter_tile() {
        use crate::entropy::writer::AomWriter;
        use crate::partition::{BlockDecision, InterDecision, PartitionType};
        use crate::port_entropy_inter::modes::MotionMode;
        use crate::port_entropy_inter::refframe::ReferenceMode;
        use crate::rate_control::RcMode;
        use svtav1_types::prediction::PredictionMode;

        let (w, h) = (64usize, 64usize);
        let y: Vec<u8> = (0..h)
            .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
            .collect();
        let uv = vec![128u8; (w / 2) * (h / 2)];

        let mut pipeline = EncodePipeline::new(
            w as u32,
            h as u32,
            6,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 40,
                ..RcConfig::default()
            },
            0,
            64,
        )
        .with_bit_depth(8)
        .with_chroma_420(true);
        let f0 = pipeline
            .try_encode_frame_420(&y, &uv, &uv, w)
            .expect("the video-mode key frame encodes");
        assert_eq!(
            f0.len(),
            961,
            "this is not the reference cell — frame 0 must be C's 961 bytes"
        );
        let saved = pipeline
            .dpb
            .get(0)
            .and_then(|r| r.frame_cdfs.clone())
            .expect("the key frame stored its end-of-frame CDFs");

        let mut fc = saved.fc.clone();
        let mut coeff_fc = saved.coeff.clone();
        let mut writer = AomWriter::new(256);
        let mut ectx = EntropyCtx::new(w / 4, h / 4, false, true, false, 8);
        // The frame-1 header the port already writes byte-identically
        // (tools/inter_fh_gate.sh): reference_select 1, SWITCHABLE interp,
        // allow_high_precision_mv 0, use_ref_frame_mvs 1, order_hint 1.
        ectx.inter_syntax = Some(InterSyntaxState {
            reference_mode: ReferenceMode::Select,
            interpolation_filter: crate::port_enc_mode_config::md_config::SWITCHABLE,
            enable_dual_filter: false,
            enable_interintra_compound: false,
            enable_masked_compound: false,
            enable_jnt_comp: false,
            enable_order_hint: true,
            order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
            is_motion_mode_switchable: true,
            allow_warped_motion: true,
            allow_high_precision_mv: false,
            force_integer_mv: false,
            gm_wmtype: [crate::port_entropy_inter::modes::TransformationType::Identity; 8],
            cur_order_hint: 1,
            ref_order_hint: [0; 7],
            use_ref_frame_mvs: true,
        });
        let (mi_cols, mi_rows) = ((w / 4) as i32, (h / 4) as i32);
        let tpl_stride = (mi_cols + 1) >> 1;
        ectx.arm_inter_mvp(crate::partition::InterMdEnv {
            mi_stride: mi_cols,
            mi_rows,
            mi_cols,
            tile: crate::intrabc::TileMiBounds {
                mi_col_start: 0,
                mi_col_end: mi_cols,
                mi_row_start: 0,
                mi_row_end: mi_rows,
            },
            sb_mi_size: 16,
            global_motion: [svtav1_types::motion::WarpedMotionParams::default(); 8],
            allow_high_precision_mv: false,
            force_integer_mv: false,
            use_ref_frame_mvs: true,
            order_hint_info: crate::inter_mvp::OrderHintInfo {
                enable_order_hint: true,
                order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
            },
            cur_order_hint: 1,
            ref_order_hint: [0; 8],
            tpl_mvs: vec![
                crate::inter_mvp::TplMvRef::default();
                (((mi_rows + 32) >> 1) * tpl_stride) as usize
            ],
            tpl_stride,
            sb64_sq_no4xn_geom: true,
        });

        // C's measured decision, minus everything the pack now derives.
        let decision = BlockDecision {
            partition_type: PartitionType::None,
            is_inter: true,
            width: 64,
            height: 64,
            eob: 0,
            inter: Some(alloc::boxed::Box::new(InterDecision {
                mode: PredictionMode::NewMv,
                ref_frame: [1, -1],
                mv: [
                    svtav1_types::motion::Mv { x: -24, y: 0 },
                    svtav1_types::motion::Mv::ZERO,
                ],
                drl_index: 0,
                interp_filters: 0,
                motion_mode: MotionMode::SimpleTranslation,
                num_proj_ref: 0,
                overlappable_neighbors: 0,
                skip_mode: false,
            })),
            ..Default::default()
        };

        // The partition symbol is the WALK's, not the block writer's.
        let (part_ctx, nsymbs) = ectx.partition_ctx(0, 0, 64);
        crate::entropy::context::write_partition_edge(
            &mut writer,
            &mut fc,
            part_ctx,
            0,
            nsymbs,
            false,
            true,
            true,
        );
        let mut geom = crate::deblock::DeblockGeom::new(w, h, w, h);
        encode_block_syntax(
            &decision,
            &mut writer,
            &mut fc,
            &mut coeff_fc,
            160,
            &mut ectx,
            /*is_key=*/ false,
            0,
            0,
            &mut None,
            &mut geom,
        );
        let tile = writer.done().to_vec();
        assert_eq!(
            tile.as_slice(),
            &[0x94u8, 0x9a, 0xb0][..],
            "the real pack walk's inter tile does not match C's; port {tile:02x?}"
        );

        // The mi grid the walk stamped is what a NEXT block would scan.
        // Asserting it here is the positive control for `record_inter_mi`'s
        // grid half: without it the MVP walk would read an all-intra map and
        // this test would still pass, because the FIRST block of a frame has
        // no neighbours to read.
        let nb = ectx.inter_neighbors(0, 8);
        assert_eq!(
            nb.above.map(|a| a.ref_frame),
            Some([1, -1]),
            "the block below must see a LAST_FRAME inter neighbour"
        );
    }

    /// **The DPB's reference carries C's replicated margin** — §1s item 4.
    ///
    /// C pads a recon before it becomes a reference
    /// (`pad_ref_and_set_flags`, enc_dec_process.c:1072-1112, `border =
    /// BLOCK_SIZE_64 + 4`). The port stored bare planes, and the inter
    /// prediction filled every out-of-frame sample with the constant **128**
    /// — a value no decoder produces.
    ///
    /// §1t measured why that is a MODE-DECISION defect and not only a
    /// conformance one: the harness translates frame 1 right by 3 px WITH
    /// left-edge replication, so at the correct MV `-3` the block's first
    /// three columns read outside the reference and match EXACTLY only
    /// against a replicated margin. Against a 128 fill the residual is large
    /// and C's `skip = 1` is unreachable at any quantizer.
    ///
    /// This asserts the three things that break: the margin exists on the
    /// DPB slot, it replicates the edge in all four directions, and the
    /// prediction path reads it. The last is a POSITIVE CONTROL with teeth —
    /// it compares against the 128 the old path would have produced, so a
    /// padded plane that were built but never consulted fails here.
    #[test]
    fn the_dpb_reference_carries_cs_replicated_margin() {
        use crate::picture::REF_BORDER;
        use crate::rate_control::RcMode;

        let (w, h) = (64usize, 64usize);
        let y: Vec<u8> = (0..h)
            .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
            .collect();
        let uv = vec![128u8; (w / 2) * (h / 2)];
        let mut pipeline = EncodePipeline::new(
            w as u32,
            h as u32,
            6,
            RcConfig {
                mode: RcMode::Cqp,
                qp: 40,
                ..RcConfig::default()
            },
            0,
            64,
        )
        .with_bit_depth(8)
        .with_chroma_420(true);
        let f0 = pipeline
            .try_encode_frame_420(&y, &uv, &uv, w)
            .expect("the video-mode key frame encodes");
        assert_eq!(f0.len(), 961, "this is not the reference cell");

        let slot = pipeline.dpb.get(0).expect("the key frame refreshed slot 0");
        let padded = slot
            .padded
            .as_ref()
            .expect("a stored reference must carry its padded twin");
        assert_eq!(padded.y.border, REF_BORDER);
        assert_eq!(padded.y.width, w);
        assert_eq!(padded.y.height, h);
        let (cu, cv) = padded.uv.as_ref().expect("4:2:0 chroma is reconstructed");
        // C `(border + ss_x) >> ss_x` at 4:2:0 (enc_dec_process.c:1102).
        assert_eq!(cu.border, (REF_BORDER + 1) >> 1);
        assert_eq!(cv.border, (REF_BORDER + 1) >> 1);
        assert_eq!((cu.width, cu.height), (w / 2, h / 2));

        // The recon this margin replicates. Reading it back off the bare
        // plane keeps the two representations honest: a padded plane built
        // from the WRONG buffer would pass every size assertion above.
        let recon = &slot.y_plane;
        for r in 0..h {
            assert_eq!(padded.y.at(0, r as isize), recon[r * w]);
            for d in 1..=REF_BORDER as isize {
                assert_eq!(
                    padded.y.at(-d, r as isize),
                    recon[r * w],
                    "left margin at row {r}, depth {d} must replicate column 0"
                );
                assert_eq!(
                    padded.y.at(w as isize - 1 + d, r as isize),
                    recon[r * w + w - 1],
                    "right margin at row {r}, depth {d}"
                );
            }
        }
        for c in 0..w {
            for d in 1..=REF_BORDER as isize {
                assert_eq!(
                    padded.y.at(c as isize, -d),
                    recon[c],
                    "top margin at col {c}, depth {d}"
                );
                assert_eq!(
                    padded.y.at(c as isize, h as isize - 1 + d),
                    recon[(h - 1) * w + c],
                    "bottom margin at col {c}, depth {d}"
                );
            }
        }
        // The CORNERS are the part `generate_padding` gets right by
        // replicating the already-padded first and last ROWS, not by a
        // second horizontal pass — worth pinning, because a port that padded
        // vertically first leaves them zero.
        assert_eq!(padded.y.at(-1, -1), recon[0]);
        assert_eq!(padded.y.at(w as isize, h as isize), recon[h * w - 1]);

        // The prediction path reads it. `generate_inter_pred` is `pub(crate)`
        // only through `partition`, so drive it the way the search does: a
        // 8x8 block at the frame's left edge with a full-pel MV of -3, whose
        // first three columns are OUTSIDE the reference.
        let rfc = crate::partition::RefFrameCtx {
            y_plane: recon,
            stride: w,
            pic_width: w,
            pic_height: h,
            mv_map: None,
            mv_map_stride: 0,
            y_padded: Some(&padded.y),
            sb_size: 64,
        };
        // A FULL-PEL MV takes the convolve's COPY corner, so the prediction
        // is the reference sampled at the MV — including the three columns
        // that fall outside the frame, which is the whole point.
        let pred = crate::partition::generate_inter_pred_for_test(
            &rfc,
            svtav1_types::motion::Mv { x: -24, y: 0 },
            0,
            0,
            8,
            8,
        );
        for r in 0..8usize {
            for c in 0..8usize {
                let want = recon[r * w + (c as isize - 3).max(0) as usize];
                assert_eq!(
                    pred[r * 8 + c],
                    want,
                    "prediction at ({c}, {r}) must come from the replicated margin, not 128"
                );
            }
        }
        assert_ne!(
            pred[0], 128,
            "the out-of-frame column still reads the 128 fill this replaced"
        );

        // A SUB-PEL MV must not take the copy corner. This is the positive
        // control for the convolve swap itself (§1s item 5): the homegrown
        // BILINEAR this replaced is a 2-tap average of the two neighbouring
        // samples, so it can never leave the interval they bound — C's 8-tap
        // filter has negative taps and routinely does. Asserting only "the
        // result changed" would pass for any other bug; asserting it leaves
        // the bilinear interval proves an 8-tap filter ran.
        let sub = crate::partition::generate_inter_pred_for_test(
            &rfc,
            svtav1_types::motion::Mv { x: 4, y: 0 },
            8,
            8,
            8,
            8,
        );
        let mut outside_bilinear_interval = 0usize;
        for r in 0..8usize {
            for c in 0..8usize {
                let a = i32::from(recon[(8 + r) * w + 8 + c]);
                let b = i32::from(recon[(8 + r) * w + 9 + c]);
                let v = i32::from(sub[r * 8 + c]);
                if v < a.min(b) || v > a.max(b) {
                    outside_bilinear_interval += 1;
                }
            }
        }
        assert!(
            outside_bilinear_interval > 0,
            "every half-pel sample stayed inside the two-tap interval — an 8-tap \
             convolve did not run (bilinear cannot leave it, C's filter can)"
        );
    }
}

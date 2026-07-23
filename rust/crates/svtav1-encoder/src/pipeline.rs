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
    /// CICP color description.
    pub color_description: svtav1_entropy::obu::ColorDescription,
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
    /// CDEF evidence counters for the last encoded frame (non-vacuity
    /// reporting: how many pixels the signaled strengths actually touched).
    pub last_cdef_stats: crate::cdef::CdefStats,
    /// Loop-restoration evidence for the last encoded frame: per-plane
    /// frame types (0 NONE / 1 WIENER) + the number of RUs that signaled
    /// wiener. Zeroed when the search does not run.
    pub last_lr_stats: ([u8; 3], usize),
    /// Requested `TileRowsLog2` (C `static_config.tile_rows` —
    /// EbSvtAv1Enc.h:607-611: "0 means no tiling, 1 means split into 2").
    /// Default 0 = single tile row (unchanged pre-task-#86 behavior).
    /// The actually-encoded value is [`svtav1_entropy::obu::
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
            // C-matched default: CICP "unspecified" (cp/tc/mc = 2/2/2,
            // studio range) — the library defaults of enc_settings.c:1043.
            // The SH then carries color_description_present_flag=0 and
            // color_range=0, byte-matching C at matched configs. Callers
            // that know their color space (AVIF path) override via
            // with_color_description.
            color_description: svtav1_entropy::obu::ColorDescription::default(),
            chroma_420: false,
            last_recon: None,
            last_recon_unfiltered: None,
            last_recon_pre_cdef: None,
            last_recon10_y: None,
            last_recon10_uv: None,
            last_cdef_stats: crate::cdef::CdefStats::default(),
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
    fn resolve_sb_size(derived: usize, override_: Option<usize>, preset: u8) -> (usize, bool) {
        let want = override_.unwrap_or(derived);
        debug_assert!(want == 64 || want == 128, "sb_size must be 64 or 128, got {want}");
        if want == 128 && !Self::sb128_encode_supported(preset) {
            (64, true)
        } else {
            (want, false)
        }
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
    pub fn with_color_description(mut self, cd: svtav1_entropy::obu::ColorDescription) -> Self {
        self.color_description = cd;
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
            (self.width % 64 == 0 && self.height % 64 == 0) || self.speed_config.preset >= 6,
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
        let (tcw, tch) = ((tw + 1) / 2, (th + 1) / 2);
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
        let (tcw, tch) = ((tw + 1) / 2, (th + 1) / 2);
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
        if (self.width % 64 != 0 || self.height % 64 != 0) && self.speed_config.preset < 6 {
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
        if !self.gop.is_key_frame(self.frame_count) {
            return Err(whereat::at!(EncodeError::UnsupportedConfig(
                "chroma_420 pipeline supports still/key frames only (intra_period <= 1)",
            )));
        }
        let (tw, th) = (self.true_width as usize, self.true_height as usize);
        let (tcw, tch) = ((tw + 1) / 2, (th + 1) / 2);
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

    /// Shared frame encode body. `chroma = Some((u, v))` selects the 4:2:0
    /// path; `None` is the unchanged monochrome path.
    fn encode_frame_impl(
        &mut self,
        y_plane: &[u8],
        y_stride: usize,
        chroma: Option<(&[u8], &[u8])>,
    ) -> crate::EncodeResult<Vec<u8>> {
        let display_order = self.frame_count;
        // Feature 1: snapshot the cooperative-cancellation token once (a cheap
        // Arc clone; `Send + Sync`) so the per-SB loops here, the entropy-walk
        // closure, and `encode_tile_rows` all check the same token. The default
        // `Unstoppable` token's `may_stop()` is `false`, so every guarded check
        // below is a byte-inert false-branch.
        let stop = self.stop.clone();

        // Step 1: Determine frame type from GOP structure
        let is_key = self.gop.is_key_frame(display_order);
        // The 4:2:0 path is still-frame only: inter frames would need
        // chroma in the DPB and a chroma-aware inter frame header.
        assert!(
            chroma.is_none() || is_key,
            "chroma_420 pipeline supports still/key frames only (intra_period <= 1)"
        );
        let temporal_layer = if is_key {
            0
        } else {
            let pos = (display_order % self.gop.mini_gop_size as u64) as u32;
            self.gop.get_temporal_layer(pos)
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
                    if let Some(rf) = self.dpb.get(slot) {
                        if rf.y_plane.len() == n {
                            ref_frames.push(&rf.y_plane);
                        }
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
                    y_plane[..n].to_vec()
                }
            } else {
                y_plane[..n].to_vec()
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

        // Screen-content derivation (allintra): scm 3 auto-detect at
        // preset <= 7 (enc_handle.c:4514-4527), off at M8+; palette level
        // + FH allow_screen_content_tools from sc_class5
        // (enc_mode_config.c:2374-2393). Runs on the SOURCE luma (C
        // pcs->enhanced_pic) before everything downstream: the flag gates
        // the per-block no-palette flag coding in the tile pack, the MD
        // rates (via the tile driver's own identical derivation), and the
        // FH bits.
        let sc_derivation = crate::sc_detect::derive_allintra_sc(
            self.speed_config.preset,
            &encode_input,
            w,
            w,
            h,
        );

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
        #[allow(unused_mut)]
        let mut base_qindex = crate::rate_control::qp_to_qindex(tpl_adjusted_qp);
        // [SVT_HDR_MODE] fork Variance Boost: derive the per-SB qindex plan
        // (sb_qindex.rs = C variance_adjust_qp(readjust=true) chain). The
        // recentered base REPLACES base_qindex BEFORE every downstream
        // consumer (lambda, CDF bucket, deblock, FH) — C order: rc_aq runs
        // in rc_init_sb_qindex ahead of MD. picture_qp follows C's
        // (base+2)>>2 update.
        let sb_plan = if self.hdr.is_fork() && self.hdr.enable_variance_boost {
            let sb_cols_p = w.div_ceil(64);
            let sb_rows_p = h.div_ceil(64);
            let mut vars = svtav1_types::try_with_capacity![sb_cols_p * sb_rows_p]?;
            for r in 0..sb_rows_p {
                for c in 0..sb_cols_p {
                    vars.push(crate::sb_qindex::compute_sb_variances(
                        &encode_input, w, w, h, c * 64, r * 64,
                    ));
                }
            }
            let plan = crate::sb_qindex::variance_adjust_qp(
                base_qindex,
                &vars,
                self.hdr.variance_boost_strength,
                self.hdr.variance_octile,
                self.hdr.variance_boost_curve,
                tpl_adjusted_qp,
                self.bit_depth,
            );
            base_qindex = plan.base_qindex;
            tpl_adjusted_qp = ((i32::from(plan.base_qindex) + 2) >> 2).clamp(0, 63) as u8;
            Some(plan)
        } else {
            None
        };

        // Issue #5: `base_qindex == 0` signals CODED-LOSSLESS in the frame
        // header (WHT 4x4 transform, deblock/CDEF/LR forced off) — a path the
        // search/recon side does NOT implement yet. Encoding anyway emits a
        // valid-syntax, self-consistent bitstream of the WRONG image (decoders
        // reconstruct via the lossless rules the encoder never used; measured
        // ssim2 -200..-1100 vs source, and one 64x64 case rav1d rejects).
        // Zero-tolerance corruption class: reject with a typed error instead
        // of silently corrupting. QP 1 (qindex 4) is the verified floor.
        // Remove this gate only when the lossless envelope is ported +
        // byte-verified vs C (the aom-rs sibling's KB-5 closed this exact
        // class: forward WHT + lossless entropy-ctx + CfL-at-lossless).
        if base_qindex == 0 {
            return Err(whereat::at!(crate::EncodeError::UnsupportedConfig(
                "QP 0 (base_qindex 0 = coded-lossless) is not implemented; the \
                 emitted stream would decode to wrong pixels (issue #5). Use QP >= 1.",
            )));
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
        let mut c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>> =
            // Task #95 chunk 2: was gated on 64-aligned dims; the padded
            // `sb_input` now lets the per-b64 walk read C's replicated border
            // on partial SBs, so the still/PD0 coding quantizer is built for any
            // 8-aligned key frame. pic_avg_variance averages over the ALIGNED
            // b64 grid (sb_cols x sb_rows), matching C. Full-SB is unchanged.
            if is_key {
                let mut tot: u64 = 0;
                let mut cnt: u64 = 0;
                for sy in (0..h).step_by(64) {
                    for sx in (0..w).step_by(64) {
                        tot +=
                            crate::pd0::compute_b64_variance(sb_input, in_stride, sx, sy).0[0]
                                as u64;
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
                // C clamps allintra presets above M9 to M9 (enc_handle.c:4634).
                let eff_mode = self.speed_config.preset.min(9);
                let rdoq_level = crate::quant::rdoq_level_allintra(eff_mode, coeff_lvl);
                let lambda = crate::pd0::kf_full_lambda_8bit_tuned(
                    base_qindex,
                    tpl_adjusted_qp as u32,
                    self.hdr.is_fork() && self.hdr.alt_lambda_factors,
                    0,
                    (self.hdr.is_fork() && self.hdr.tune == crate::tune::TUNE_IQ)
                        .then(|| crate::tune::iq_lambda_weight(tpl_adjusted_qp as u32)),
                );
                Some(alloc::sync::Arc::new(crate::quant::CodingQuantCfg::new(
                    rdoq_level,
                    lambda,
                    base_qindex,
                )))
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
        let tile_grid = svtav1_entropy::obu::TileGrid::resolve(
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
            crate::chroma_q::ChromaQDeltas::default()
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
            if self.hdr.is_fork() && crate::tune::tune_uses_ssim_rdmult(self.hdr.tune) {
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
        let qm_levels: [u8; 3] = if self.hdr.is_fork() && self.hdr.enable_qm {
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
        let film_grain: Option<svtav1_entropy::obu::FilmGrainParams> =
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
                let mut seed = 7391u16.wrapping_add(
                    3381u16.wrapping_mul(self.frame_count as u16),
                );
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
        if self.hdr.is_fork() {
            if let Some(cq) = c_quant.as_mut() {
                let cfg = alloc::sync::Arc::get_mut(cq)
                    .expect("c_quant is unshared before tile encoding starts");
                cfg.hdr_fork = true;
                cfg.sharpness = self.hdr.sharpness;
                cfg.noise_norm_strength = self.hdr.noise_norm_strength;
                cfg.sharp_tx_active = sharp_tx_active;
                cfg.qm_levels = qm_levels;
            }
        }
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
            if self.hdr.is_fork() { self.hdr.noise_norm_strength } else { 0 },
            qm_levels,
            if self.hdr.is_fork() { self.hdr.tx_bias } else { 0 },
            self.hdr.is_fork() && self.hdr.complex_hvs == 1,
            self.hdr.is_fork() && self.hdr.alt_ssim_tuning,
            self.hdr.is_fork() && self.hdr.alt_lambda_factors,
            (self.hdr.is_fork() && self.hdr.tune == crate::tune::TUNE_IQ)
                .then(|| crate::tune::iq_lambda_weight(tpl_adjusted_qp as u32)),
            ssim_factors.as_ref(),
            base_qindex,
            tpl_adjusted_qp,
            self.hdr.sharpness,
            lambda,
            &self.speed_config,
            ref_frame_data.as_deref(),
            &mv_map,
            mv_map_stride,
            &sb_qp_offsets,
            chroma.is_some(),
            c_quant.clone(),
            sb_chroma_owned
                .as_ref()
                .map(|(u, v)| (u.as_slice(), v.as_slice())),
            self.bit_depth,
            self.thread_count,
            &stop,
        )?;

        let mut per_tile_decisions: Vec<Vec<crate::partition::BlockDecision>> = Vec::new();
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
        let sb95_ext_w = w.div_ceil(sb_size) * sb_size;
        let mut canvas10: Option<(Vec<u16>, Vec<u16>, Vec<u16>)> = tile_recons
            .first()
            .and_then(|t| t.3.as_ref())
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
                let Some((ty, tu, tv)) = t.3.as_ref() else {
                    continue;
                };
                let (r0, r1) = tile_grid.row_span(tile_idx / tile_grid.tile_cols);
                let (c0, c1) = tile_grid.col_span(tile_idx % tile_grid.tile_cols);
                let (y0, y1) = (r0 * sb_size, (r1 * sb_size).min(h));
                let (x0, x1) = (c0 * sb_size, (c1 * sb_size).min(w));
                for r in y0..y1 {
                    cy[r * w + x0..r * w + x1]
                        .copy_from_slice(&ty[r * sb95_ext_w + x0..r * sb95_ext_w + x1]);
                }
                let (cw, cxs, cxe) = (w / 2, x0 / 2, x1 / 2);
                let cst = sb95_ext_w / 2;
                for r in y0 / 2..y1 / 2 {
                    cu[r * cw + cxs..r * cw + cxe]
                        .copy_from_slice(&tu[r * cst + cxs..r * cst + cxe]);
                    cv[r * cw + cxs..r * cw + cxe]
                        .copy_from_slice(&tv[r * cst + cxs..r * cst + cxe]);
                }
            }
        }

        // Merge tile recons into frame buffer and update MV map
        for (tile_idx, (tile_recon, tile_decisions, tile_trees, _canvas10)) in
            tile_recons.iter().enumerate()
        {
            per_tile_decisions.push(tile_decisions.clone());
            let (tile_sb_row_start, tile_sb_row_end) =
                tile_grid.row_span(tile_idx / tile_grid.tile_cols);
            let (tile_sb_col_start, tile_sb_col_end) =
                tile_grid.col_span(tile_idx % tile_grid.tile_cols);
            let mut tree_k = 0usize;
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check (no-op
                // for the default `Unstoppable` token — `may_stop()` is false).
                if stop.may_stop() {
                    stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
                }
                for sb_col in tile_sb_col_start..tile_sb_col_end {
                    tree_slots[sb_row * sb_cols + sb_col] = Some(tile_trees[tree_k].clone());
                    tree_k += 1;
                }
            }
            let mut offset = 0;
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                // Feature 1: byte-inert cooperative-cancellation check.
                if stop.may_stop() {
                    stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
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
            // The bd10 re-encode (`tx_unit_hbd`) is not yet partial-SB-aware: an
            // edge/straddle block (frame dims not a multiple of 64) has an
            // out-of-envelope transform footprint the highbd tx unit can't map,
            // so a partial-SB bd10 frame must FALL BACK to the u8 output rather
            // than panic. Complete-SB frames (every current bd10 gate cell is
            // 64-aligned) are unaffected. Partial-SB bd10 is a documented
            // follow-up (docs/bd10-port-map.md).
            let bd10_frame_aligned = w % 64 == 0 && h % 64 == 0;
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
            let bd10_full_rd =
                bd10_full_rd_supported(self.bit_depth, self.speed_config.preset, w, h);
            let bd10_postpass_runs = !bd10_full_rd
                && bd10_frame_aligned
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
            if std::env::var_os("SVTAV1_BD10_POSTPASS").is_some() {
                let unsupported = all_trees
                    .iter()
                    .filter(|t| !bd10_tree_supported(t, bd10_edge_filter))
                    .count();
                eprintln!(
                    "BD10_POSTPASS runs={bd10_postpass_runs} aligned={bd10_frame_aligned} \
                     unsupported_sbs={unsupported}/{} edge_filter={bd10_edge_filter}",
                    all_trees.len()
                );
            }
            if let Some(cq) = c_quant.as_ref().filter(|_| bd10_postpass_runs) {
                let shift = (self.bit_depth - 8) as u32;
                let src10: alloc::vec::Vec<u16> =
                    encode_input.iter().map(|&s| (s as u16) << shift).collect();
                // bd10 full MD lambda (C full_lambda_md[1], md_process.c:725-759):
                // computed from the bd10 rdmult base (dc_qlookup_10 + ROUND_
                // POWER_OF_TWO(,4) + frame-type-factor 128 + the *16), NOT a
                // ×16 of the bd8 lambda — see kf_full_lambda_bd10.
                let lambda_bd10 =
                    u64::from(crate::pd0::kf_full_lambda_bd10(base_qindex, tpl_adjusted_qp as u32));
                let recon10 = bd10_reencode_luma(
                    &mut all_trees,
                    sb_cols,
                    sb_size,
                    w,
                    h,
                    &src10,
                    base_qindex,
                    cq.rdoq_level,
                    lambda_bd10,
                    bd10_edge_filter,
                    self.bit_depth,
                    qm_levels[0],
                    if self.hdr.is_fork() { self.hdr.sharpness } else { 0 },
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
                    let u10: alloc::vec::Vec<u16> =
                        u_src.iter().map(|&s| (s as u16) << shift).collect();
                    let v10: alloc::vec::Vec<u16> =
                        v_src.iter().map(|&s| (s as u16) << shift).collect();
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
                        bd10_edge_filter,
                        self.bit_depth,
                        [qm_levels[1], qm_levels[2]],
                        if self.hdr.is_fork() { self.hdr.sharpness } else { 0 },
                    )?;
                    self.last_recon10_uv = Some(uv10);
                }
                self.last_recon10_y = Some(recon10);
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
        // filter pass. sgrproj is never searched at the ported presets
        // (sg_filter_lvl = 0 — C enc_mode_config.c:2000) and stays
        // unported.

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
        if std::env::var_os("SVTAV1_DUMP_TREE").is_some() {
            for (sb_idx, tree) in all_trees.iter().enumerate() {
                let bx = (sb_idx % sb_cols) * sb_size;
                let by = (sb_idx / sb_cols) * sb_size;
                dump_tree_leaves(tree, bx, by);
            }
        }

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
            );
            // Task #91: C derives `use_128x128_superblock` at SH-write time
            // from `sb_size == BLOCK_128X128` (entropy_coding.c:2800). The
            // port's `sb_size` comes from the same rule
            // (sb128_geom::derive_super_block_size), so the bit follows it.
            t.use_128x128_superblock = self.sb_size == 128;
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
            let mut deblock_geom = crate::deblock::DeblockGeom::new(w, h);
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
            let mut tile_bitstreams: Vec<Vec<u8>> =
                Vec::with_capacity(tile_grid.num_tiles());
            for tile_idx in 0..tile_grid.num_tiles() {
                // Feature 1: byte-inert cooperative-cancellation check, once per
                // tile of each entropy re-walk (this closure runs up to 3x).
                if stop.may_stop() {
                    stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
                }
                let (tile_sb_row_start, tile_sb_row_end) =
                    tile_grid.row_span(tile_idx / tile_grid.tile_cols);
                let (tile_sb_col_start, tile_sb_col_end) =
                    tile_grid.col_span(tile_idx % tile_grid.tile_cols);

                let mut writer = svtav1_entropy::writer::AomWriter::new(n + 256);
                // CDF updates enabled — matches the frame header's disable_cdf_update=0
                let mut frame_ctx = svtav1_entropy::context::FrameContext::new_default();
                // C-exact coefficient CDFs for the base_q_idx bucket
                // (svt_av1_default_coef_probs semantics) — qindex domain.
                let mut coeff_fc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
                let mut ectx = EntropyCtx::new(
                    w4,
                    h4,
                    seq_tools.enable_filter_intra,
                    sc_derivation.allow_screen_content_tools,
                );
                // IBC chunk 1: arm the per-block use_intrabc flag coding
                // (C write_intrabc_info gate) from the same sc derivation
                // that set the FH bit — signaling and coding MUST agree or
                // the stream is undecodable.
                ectx.allow_intrabc = sc_derivation.allow_intrabc;
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
                        stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
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
                svtav1_entropy::obu::tile_size_bytes_minus_1_for(&non_last_lens);

            Ok((
                svtav1_entropy::obu::build_tile_group_multi(&tile_bitstreams, tile_size_bytes_minus_1),
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
        // IBC (chunk 1): C kills the deblock filter at SIGNAL-DERIVATION on
        // IntraBC frames — `dlf_level` stays 0 unless `enable_dlf_flag &&
        // frm_hdr->allow_intrabc == 0` (enc_mode_config.c:10117-10127), so
        // neither the level pick nor the frame apply runs and the FH codes
        // no loop-filter params (obu.rs suppresses them on the same flag).
        // Only sc_class5 presets <= 4 frames take this arm.
        let lf_levels = if sc_derivation.allow_intrabc {
            crate::deblock::LfLevels::default()
        } else if is_key {
            if is_single_frame && self.speed_config.preset <= 5 {
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let early_exit_convergence =
                    if self.speed_config.preset <= 3 { 0 } else { 1 };
                match recon10.as_ref() {
                    // bd10: search on the true 10-bit unfiltered recon
                    // against the true 10-bit source, with the highbd lpf
                    // kernels and `svt_full_distortion_kernel16_bits`
                    // (C `picture_sse_calculations` at is_16bit,
                    // deblocking_filter.c:768).
                    Some((y10, u10, v10)) => {
                        let sh = (self.bit_depth - 8) as u32;
                        let widen =
                            |p: &[u8]| -> Vec<u16> { p.iter().map(|&s| (s as u16) << sh).collect() };
                        let (sy10, su10, sv10) = (widen(&encode_input), widen(su), widen(sv));
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
                crate::deblock::pick_filter_levels_key_frame(base_qindex, self.bit_depth)
            }
        } else {
            crate::deblock::LfLevels::default()
        };
        self.last_recon_unfiltered = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        if let Some((y10, u10, v10)) = recon10.as_mut() {
            if lf_levels.any() {
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
        }
        if lf_levels.any() {
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
        let cdef_params = if sc_derivation.allow_intrabc {
            crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default())
        } else if is_key {
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
            if is_single_frame
                && crate::cdef::allintra_preset_uses_cdef_search(self.speed_config.preset)
            {
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
                    let cfg = crate::cdef::cdef_search_cfg_for_preset(self.speed_config.preset);
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
                            let (sy10, su10, sv10) =
                                (widen(&encode_input), widen(su), widen(sv));
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
            } else {
                crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_key_frame(
                    base_qindex,
                    self.bit_depth,
                ))
            }
        } else {
            crate::cdef::CdefPick::single(crate::cdef::CdefFrameParams::default())
        };
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
        self.last_recon_pre_cdef = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
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
        // bd10: apply CDEF to the 10-bit canvas too. Not for output — the u8
        // chain above still produces that — but because the Wiener LR search
        // reads the POST-CDEF recon, and at 10 bits that must be the 10-bit
        // one (C: rest_process runs after cdef_process on the same 16-bit
        // recon picture CDEF just filtered in place).
        if let Some((y10, u10, v10)) = recon10.as_mut() {
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
        let mut lr_signal = svtav1_entropy::obu::LrSignal::none(seq_tools.enable_restoration);
        // IBC (chunk 1): unlike DLF/CDEF, C suppresses loop restoration at
        // PIPELINE EXECUTION, not signal-derivation — `if (ppcs->
        // enable_restoration && frm_hdr->allow_intrabc == 0)` gates BOTH the
        // search (rest_process.c:262) and the apply/finish (:325, else-arm
        // forces all planes RESTORE_NONE). enable_restoration itself (and
        // the SH bit) stays UNCHANGED — do NOT fold this into the
        // derivation (docs/ibc-port-map.md §A.7).
        if is_key && seq_tools.enable_restoration && !sc_derivation.allow_intrabc {
            let ctrls = crate::restoration::wn_filter_ctrls_allintra(self.speed_config.preset);
            if ctrls.enabled {
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
                let (lr_tcw, lr_tch) = ((lr_true_w + 1) / 2, (lr_true_h + 1) / 2);
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
                        crate::restoration::search_restoration_still_bd(
                            &ctrls,
                            &widen_tight(&encode_input, w, lr_true_w, lr_true_h),
                            &widen_tight(su, cw, lr_tcw, lr_tch),
                            &widen_tight(sv, cw, lr_tcw, lr_tch),
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
                    None => crate::restoration::search_restoration_still(
                        &ctrls,
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
                    )?,
                };
                #[cfg(feature = "std")]
                if std::env::var_os("SVTAV1_DUMP_LR").is_some() {
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
                    let bounds = crate::restoration::save_lr_boundaries(
                        pre_y,
                        pre_u,
                        pre_v,
                        &recon,
                        &u_recon,
                        &v_recon,
                        w,
                        h,
                        chroma.is_some(),
                    );
                    crate::restoration::apply_restoration_frame(
                        &mut recon,
                        &mut u_recon,
                        &mut v_recon,
                        w,
                        h,
                        chroma.is_some(),
                        &rest_info,
                        &bounds,
                    );
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
                lr_signal = svtav1_entropy::obu::LrSignal {
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
        let sc_signal = svtav1_entropy::obu::ScSignal {
            allow_screen_content_tools: sc_derivation.allow_screen_content_tools,
            allow_intrabc: sc_derivation.allow_intrabc,
        };

        let bitstream = if is_key {
            let mut bs = alloc::vec::Vec::new();
            bs.extend_from_slice(&svtav1_entropy::obu::write_temporal_delimiter());
            bs.extend_from_slice(&svtav1_entropy::obu::write_sequence_header_ex(
                // TRUE (unaligned) dims flow to the sequence header:
                // max_frame_width/height_minus_1 carry the coded size, and
                // the level derivation keys off the real picture size (C
                // captures max_frame_width BEFORE 8-alignment,
                // enc_handle.c:4792). Everything else in the encode uses
                // the aligned self.width/height.
                self.true_width,
                self.true_height,
                is_single_frame,
                self.bit_depth,
                &self.color_description,
                chroma.is_none(), // mono_chrome unless the 4:2:0 path is active
                // seq_level_idx auto-derivation input (C: scs->frame_rate).
                self.rc_config.framerate,
                seq_tools,
            ));
            // Key frame header (raw bytes) + tile group with proper header.
            // base_qindex is the SAME value used for quantization, CDF
            // bucket selection and the deblock picker above — the decoder's
            // dequant/CDF init must match the encoder's exactly.
            let fh_bytes = svtav1_entropy::obu::write_key_frame_header_full_lr_sb(
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
                // [SVT_HDR_MODE] fork chroma-q deltas: the quantizer above
                // used qindex_u/qindex_v built from EXACTLY these deltas, so
                // signaling and application agree (chroma_q.rs). Mainline
                // passes None = the zero-delta bit pattern.
                if self.hdr.is_fork() {
                    Some([
                        chroma_deltas.u_dc,
                        chroma_deltas.u_ac,
                        chroma_deltas.v_dc,
                        chroma_deltas.v_ac,
                    ])
                } else {
                    None
                },
                // [SVT_HDR_MODE] per-SB delta-q res (variance boost). The
                // same value gates the walk's per-SB delta symbols.
                delta_q_res_signal,
                // [SVT_HDR_MODE] frame QM levels (fork enable_qm); None in
                // mainline mode. The quantizers used the SAME levels.
                if qm_levels == [15; 3] { None } else { Some(qm_levels) },
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
            bs.extend_from_slice(&svtav1_entropy::obu::write_obu(
                svtav1_entropy::obu::ObuType::Frame,
                &frame_payload,
            ));
            bs
        } else {
            // Inter frame: proper frame header with type, qindex, refresh
            // flags, ref indices.
            svtav1_entropy::obu::write_inter_frame(
                base_qindex,
                pcs.refresh_frame_flags,
                display_order as u8,
                &tile_data,
            )
        };

        // Step 7: Publish recon for the recon-parity gate, then update DPB.
        self.last_recon = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
        let ref_frame = ReferenceFrame {
            y_plane: recon,
            width: self.width,
            height: self.height,
            display_order,
            order_hint: display_order as u32,
        };
        self.dpb.refresh(pcs.refresh_frame_flags, &ref_frame);

        // Step 8: Update rate control state
        update_rc_state(&mut self.rc_state, bitstream.len() as u64 * 8, pcs.qp);

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
pub(crate) fn palette_cache(ectx: &EntropyCtx, block_x: usize, block_y: usize) -> alloc::vec::Vec<u16> {
    let x4 = block_x / 4;
    let y4 = block_y / 4;
    let mut above_n = if block_y % 64 != 0 && x4 < ectx.above_palette.len() {
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
    /// Left TXFM context at 4x4 granularity: the HEIGHT in pixels of the
    /// last coded TX in each mi row.
    left_txfm: Vec<u8>,
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
        allow_sct: bool,
    ) -> Self {
        let width_8x8 = (width_4x4 + 1) / 2;
        let height_8x8 = (height_4x4 + 1) / 2;
        // Chroma-plane 4x4 units: (w/2)/4 = width_4x4/2 (frames are
        // 64-aligned so this divides exactly; div_ceil for safety).
        let width_c4 = width_4x4.div_ceil(2);
        let height_c4 = height_4x4.div_ceil(2);
        Self {
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
            above_txfm: alloc::vec![0u8; width_4x4],
            left_txfm: alloc::vec![0u8; height_4x4],
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
            allow_sct,
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
            ctx.min(svtav1_entropy::context::PARTITION_CONTEXTS - 1),
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

    /// Record a block's luma palette (C's `mbmi->palette_mode_info`, read
    /// back by `svt_aom_get_palette_mode_ctx` / `svt_get_palette_cache_y`).
    /// `colors` is `None` for a non-palette block (palette_size 0 — every
    /// current leaf, until #71 chunk 3/4 injection wires a winning
    /// candidate through `BlockDecision.palette`). Stamped over the
    /// block's full mi span, exactly like [`Self::record_block`].
    pub(crate) fn record_palette(&mut self, x: usize, y: usize, w: usize, h: usize, colors: Option<&[u16]>) {
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
        let smooth = |m: u8| matches!(m, 9 | 10 | 11);
        let ab = y > 0 && smooth(self.above_mode[x / 4]);
        let le = x > self.tile_left_px && smooth(self.left_mode[y / 4]);
        i32::from(ab || le)
    }

    /// C `get_filt_type(xd, plane > 0)`: same over the neighbours' UV
    /// modes (chroma_above/left_mbmi; min-8x8 blocks make the +1-mi
    /// group offsets land in the same neighbour block).
    pub(crate) fn filt_type_uv(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9 | 10 | 11);
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
        svtav1_entropy::context::intra_mode_context(mode)
    }

    /// Get the left mode context at position (x, y) in pixel coordinates.
    pub(crate) fn left_mode_ctx(&self, y: usize) -> usize {
        let y4 = y / 4;
        let mode = if y4 < self.left_mode.len() {
            self.left_mode[y4]
        } else {
            0
        };
        svtav1_entropy::context::intra_mode_context(mode)
    }

    /// Get the skip context at position (x, y).
    pub(crate) fn skip_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = x4 < self.above_skip.len() && self.above_skip[x4];
        let left = y4 < self.left_skip.len() && self.left_skip[y4];
        svtav1_entropy::context::get_skip_context(above, left)
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
        let above = (self.above_txfm[x / 4] as usize >= w) as usize;
        let left = (self.left_txfm[y / 4] as usize >= h) as usize;
        match (has_above, has_left) {
            (true, true) => above + left,
            (true, false) => above,
            (false, true) => left,
            (false, false) => 0,
        }
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
    writer: &mut svtav1_entropy::writer::AomWriter,
    coeff_fc: &mut svtav1_entropy::coeff_c::CoeffFc,
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
    use svtav1_entropy::coeff_c;
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
    let scan = svtav1_entropy::scan_tables::scan(
        tx_size,
        svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tx_type] as usize,
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
#[allow(clippy::too_many_arguments)]
fn encode_block_syntax(
    decision: &crate::partition::BlockDecision,
    writer: &mut svtav1_entropy::writer::AomWriter,
    frame_ctx: &mut svtav1_entropy::context::FrameContext,
    coeff_fc: &mut svtav1_entropy::coeff_c::CoeffFc,
    base_q_idx: u8,
    ectx: &mut EntropyCtx,
    is_key: bool,
    block_x: usize,
    block_y: usize,
    chroma: &mut Option<ChromaPass<'_>>,
    geom: &mut crate::deblock::DeblockGeom,
) {
    // Diagnostic (SVTAV1_PACKTREE=<path>): one line per coded leaf — the
    // port's FINAL tree, file-only (no stderr noise; token-frugal drills).
    // tools/tree_diff.py joins it against the C-side CTREE dump (the
    // svt_aom_update_mi_map --wrap, valid at every preset) and prints only
    // the flips. Field domains mirror the C wrap: C BlockSize enum id via
    // block_size_index; fi 5 = none; uv 13 = CFL; skip is derived on the
    // diff side from yeob/ueob/veob (C dumps the all-plane skip bit).
    #[cfg(feature = "std")]
    if let Some(path) = std::env::var_os("SVTAV1_PACKTREE") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let (ueob, veob) = decision
                .chroma_dec
                .as_ref()
                .map(|c| (c.2, c.3))
                .unwrap_or((0, 0));
            let _ = writeln!(
                f,
                "PTREE mi=({},{}) bsize={} part={} mode={} uv={} fi={} ady={} aduv={} txd={} yeob={} ueob={} veob={} cflidx={} cflsgn={} pal={}",
                block_y / 4,
                block_x / 4,
                svtav1_entropy::context::block_size_index(
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
            );
        }
    }
    // Diagnostic (SVTAV1_PACKTREE_COEFF): the block's PACKED nonzero
    // luma+chroma levels as (raster_idx:level) pairs — the port counterpart
    // of the C QLEV/CCOEF wrap dumps (final coded levels). Two modes:
    //   * value contains a comma ("mi_row,mi_col") → pin ONE block, stderr.
    //   * value is a PATH (no comma) → append EVERY coded leaf to that file
    //     (coding order), for a whole-frame join vs the C SVT_QLEVELS_OUT
    //     dump. Backward-compatible: existing "r,c" callers are unchanged.
    #[cfg(feature = "std")]
    if let Ok(xy) = std::env::var("SVTAV1_PACKTREE_COEFF") {
        let is_pin = xy.contains(',');
        let want: alloc::vec::Vec<usize> =
            xy.split(',').filter_map(|s| s.trim().parse().ok()).collect();
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
                    .open(&xy)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }
    // Diagnostic (SVTAV1_PART_DUMP): every coded leaf's geometry + skip, to
    // diff the partition tree against the C entropy coder. No output change.
    #[cfg(feature = "std")]
    if std::env::var_os("SVTAV1_PART_DUMP").is_some() {
        eprintln!(
            "RSPART x{block_x} y{block_y} {}x{} skip={} ymode={} uvmode={} txd={}",
            decision.width,
            decision.height,
            decision.eob == 0,
            decision.intra_mode as u8,
            decision.uv_mode as u8,
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
        ((block_y / 4) % 2 == 1 || bh_mi % 2 == 0) && ((block_x / 4) % 2 == 1 || bw_mi % 2 == 0)
    };
    // Task #86: chroma-plane tile-row origin (exact halving — see
    // encode_chroma_block_dc's doc comment). Copied out of `ectx` before
    // the closure below so the closure doesn't need to borrow `ectx` too.
    let chroma_tile_top = ectx.tile_top_px / 2;
    let chroma_tile_left = ectx.tile_left_px / 2; // task #96, same halving rule
    let chroma_blocks = chroma.as_mut().filter(|_| blk_has_uv).map(|cp| {
        let cw = (decision.width as usize).max(8) / 2;
        let ch = (decision.height as usize).max(8) / 2;
        let cx = ((block_x >> 3) << 3) / 2 + if decision.width >= 8 { (block_x % 8) / 2 } else { 0 };
        let cy = ((block_y >> 3) << 3) / 2 + if decision.height >= 8 { (block_y % 8) / 2 } else { 0 };
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
                cp.u_src, cp.u_recon, cp.stride, cx, cy, cw, ch, cp.qindex_u, cp.c_quant,
                cp.qm_u, chroma_tile_top, chroma_tile_left,
            );
            let (v_q, v_eob) = crate::partition::encode_chroma_block_dc(
                cp.v_src, cp.v_recon, cp.stride, cx, cy, cw, ch, cp.qindex_v, cp.c_quant,
                cp.qm_v, chroma_tile_top, chroma_tile_left,
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
    svtav1_entropy::context::write_skip(writer, frame_ctx, skip_ctx, skip);

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
    if !skip {
        if let Some(st) = ectx.cdef_sb.as_mut() {
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
    }

    // [SVT_HDR_MODE] per-SB delta-q (C entropy_coding.c:4997, spec 5.11.41
    // mode_info -> read_delta_qindex): only at the SB's upper-left block,
    // and only when (bsize != sb_size || !skip). sb_size is 64 here.
    if let Some((res, prev)) = ectx.delta_q_state {
        let super_block_upper_left = block_x % 64 == 0 && block_y % 64 == 0;
        let is_sb_sized = decision.width == 64 && decision.height == 64;
        if super_block_upper_left && (!is_sb_sized || !skip) {
            let cur = ectx.delta_q_sb_qindex;
            let reduced = (cur - prev) / i32::from(res);
            svtav1_entropy::mv_coding::write_delta_q_index(
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
    // TODO(IBC chunk 9): thread the winner's real use_intrabc + DV from
    // BlockDecision through write_intrabc_info here; `use_intrabc` below
    // then steers the tx-type CDF rows + the var-tx tx_size writer.
    let use_intrabc = false;
    if is_key && ectx.allow_intrabc {
        writer.write_symbol(usize::from(use_intrabc), &mut frame_ctx.intrabc_cdf, 2);
    }

    // Mode syntax is ALWAYS coded — the skip flag only gates residuals
    // (AV1 intra_frame_mode_info reads y_mode regardless of skip).
    if !is_key {
        svtav1_entropy::context::write_intra_inter(writer, frame_ctx, 0, decision.is_inter);
    }

    if decision.is_inter {
        svtav1_entropy::mv_coding::write_mv(writer, decision.mv.x, decision.mv.y, true);
    } else if is_key {
        let above_ctx = ectx.above_mode_ctx(block_x);
        let left_ctx = ectx.left_mode_ctx(block_y);
        svtav1_entropy::context::write_intra_mode_kf(
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
            && svtav1_entropy::context::is_directional_mode(decision.intra_mode)
        {
            svtav1_entropy::context::write_angle_delta(
                writer,
                frame_ctx,
                decision.intra_mode,
                decision.angle_delta,
            );
        }
    } else {
        let bsize_group = svtav1_entropy::context::block_size_group(
            decision.width as usize,
            decision.height as usize,
        );
        svtav1_entropy::context::write_intra_mode_inter(
            writer,
            frame_ctx,
            bsize_group,
            decision.intra_mode,
        );
        if use_angle_delta(decision.width, decision.height)
            && svtav1_entropy::context::is_directional_mode(decision.intra_mode)
        {
            svtav1_entropy::context::write_angle_delta(
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
    if chroma_blocks.is_some() {
        debug_assert!(!decision.is_inter, "420 path is key/intra only");
        let cfl_allowed = decision.width <= 32 && decision.height <= 32;
        svtav1_entropy::context::write_uv_mode(
            writer,
            frame_ctx,
            cfl_allowed,
            decision.intra_mode,
            decision.uv_mode,
        );
        // CfL alphas follow a UV_CFL_PRED chroma mode (encode_intra_chroma_
        // mode_av1, entropy_coding.c:1181; decoder read_cfl_alphas). CFL is
        // never directional, so angle_delta_uv is skipped for it.
        if decision.uv_mode == svtav1_entropy::context::UV_CFL_PRED {
            svtav1_entropy::context::write_cfl_alphas(
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
            && svtav1_entropy::context::is_directional_mode(decision.uv_mode)
        {
            svtav1_entropy::context::write_angle_delta(
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
        pal_n_out = crate::palette::index_color_cache(&pal_cache, colors, &mut pal_found, &mut pal_out);
    }
    if !decision.is_inter
        && svtav1_entropy::context::allow_palette(
            ectx.allow_sct,
            decision.width as usize,
            decision.height as usize,
        )
    {
        let neighbor_ctx = ectx.palette_neighbor_ctx(block_x, block_y);
        let palette_arg = decision
            .palette
            .as_ref()
            .map(|(colors, _idx_map)| (colors.as_slice(), pal_found.as_slice(), &pal_out[..pal_n_out]));
        svtav1_entropy::context::write_palette_mode_info(
            writer,
            frame_ctx,
            decision.width as usize,
            decision.height as usize,
            decision.intra_mode,
            decision.uv_mode,
            chroma_blocks.is_some(),
            neighbor_ctx,
            palette_arg,
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
        && decision.intra_mode == 0 // DC_PRED
        && decision.palette.is_none() // palette_size == 0 (mode_decision.c:107)
        && decision.width <= 32
        && decision.height <= 32
    {
        let bsize_idx = svtav1_entropy::context::block_size_index(
            decision.width as usize,
            decision.height as usize,
        );
        let used = decision.filter_intra_mode != 5;
        svtav1_entropy::context::write_use_filter_intra(writer, frame_ctx, bsize_idx, used);
        if used {
            svtav1_entropy::context::write_filter_intra_mode(
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
        svtav1_entropy::context::write_palette_map_tokens(
            writer, frame_ctx, idx_map, w, h, w, colors.len(),
        );
    }

    // tx_size syntax — C av1_code_tx_size (entropy_coding.c:4697) called
    // from write_modes_b right after the uv/palette/filter_intra syntax
    // and before the residuals. Key frames signal TX_MODE_SELECT in the
    // FH (like C always does), so every INTRA block with bsize > 4x4
    // codes a tx_depth symbol (the ACTUAL `decision.tx_depth` from the
    // funnel's TXS search — 0/1/2, NOT hardcoded to largest); skip only
    // suppresses the symbol for inter blocks. The neighbor context update
    // (set_txfm_ctxs) runs for EVERY block, signaling or not. Inter frames
    // signal TX_MODE_LARGEST (no symbol), but keep their context arrays
    // maintained exactly like C's else-branch.
    {
        let w = decision.width as usize;
        let h = decision.height as usize;
        let depth = decision.tx_depth;
        if is_key && !(w == 4 && h == 4) {
            let ctx = ectx.tx_size_ctx(block_x, block_y, w, h);
            svtav1_entropy::context::write_tx_depth(writer, frame_ctx, w, h, ctx, depth as usize);
        }
        // set_txfm_ctxs records the CHOSEN tx dims (the C
        // tx_depth_to_tx_size chain — rect blocks halve the LONG dim
        // first) — the next blocks' tx_size contexts read them.
        let (txw, txh) = crate::leaf_funnel::txb_dims_at_depth(w, h, depth);
        ectx.record_txfm_dims(block_x, block_y, w, h, txw, txh);
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
        use svtav1_entropy::coeff_c;
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
            let scan = svtav1_entropy::scan_tables::scan(
                tx_size,
                svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
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
            if std::env::var_os("SVTAV1_CODED_EOB").is_some() {
                let nz = coeffs.iter().filter(|&&c| c != 0).count();
                eprintln!(
                    "CODED x{block_x} y{block_y} {w}x{h} tx{tx_type} scan_eob={eob} nz={nz}"
                );
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
                use_intrabc, // IntraBC blocks code tx types over the INTER rows
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
                let tx_x = block_x + (txb % cols) * txw;
                let tx_y = block_y + (txb / cols) * txh;
                let (above, left) = ectx.coeff_neighbors(tx_x, tx_y, txw, txh);
                let (txb_skip_ctx, dc_sign_ctx) =
                    coeff_c::get_txb_ctx(0, above, left, false, false);
                let tx_type = decision.txb_tx_types[txb] as usize;
                let coeffs = &decision.txb_qcoeffs[txb];
                let scan = svtav1_entropy::scan_tables::scan(
                    tx_size,
                    svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type] as usize,
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
                    use_intrabc, // IntraBC blocks code tx types over the INTER rows
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
            let uv_tt = crate::leaf_funnel::uv_tx_type(decision.uv_mode, cw, ch);
            #[cfg(feature = "std")]
            if std::env::var_os("SVTAV1_CODED_EOB").is_some() {
                let uv_ts = svtav1_entropy::coeff_c::tx_size_from_dims(cw, ch);
                let sidx =
                    svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[uv_tt as usize] as usize;
                let uv_scan = svtav1_entropy::scan_tables::scan(uv_ts, sidx);
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
            let cx =
                ((block_x >> 3) << 3) / 2 + if decision.width >= 8 { (block_x % 8) / 2 } else { 0 };
            let cy = ((block_y >> 3) << 3) / 2
                + if decision.height >= 8 { (block_y % 8) / 2 } else { 0 };
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
    // Palette neighbor state (C mbmi->palette_mode_info, stamped for
    // EVERY block — palette or not, matching record_block above).
    ectx.record_palette(
        block_x,
        block_y,
        decision.width as usize,
        decision.height as usize,
        decision.palette.as_ref().map(|(colors, _idx_map)| colors.as_slice()),
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
/// built from those same aligned dims (`DeblockGeom::new(w, h)`, ~:884) and is
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
/// against /root/svtav1/Source, 2026-07-19 — this supersedes the port map's
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
    writer: &mut svtav1_entropy::writer::AomWriter,
    frame_ctx: &mut svtav1_entropy::context::FrameContext,
    coeff_fc: &mut svtav1_entropy::coeff_c::CoeffFc,
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
                svtav1_entropy::context::write_partition_edge(
                    writer, frame_ctx, ctx, 0, nsymbs, // 0 = PARTITION_NONE
                    w == 128, has_rows, has_cols,
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
            svtav1_entropy::context::write_partition_edge(
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
                        (crate::partition::PartitionType::HorzA, 3) => {
                            &[(0, 0), (half_w, 0), (0, half_h)]
                        }
                        // full-width top + 2 bottoms
                        (crate::partition::PartitionType::HorzB, 3) => {
                            &[(0, 0), (0, half_h), (half_w, half_h)]
                        }
                        // 2 lefts (w/2 x h/2) + full-height right (w/2 x h)
                        (crate::partition::PartitionType::VertA, 3) => {
                            &[(0, 0), (0, half_h), (half_w, 0)]
                        }
                        // full-height left + 2 rights
                        (crate::partition::PartitionType::VertB, 3) => {
                            &[(0, 0), (half_w, 0), (half_w, half_h)]
                        }
                        (crate::partition::PartitionType::Horz4, 4) => &[
                            (0, 0),
                            (0, quarter_h),
                            (0, 2 * quarter_h),
                            (0, 3 * quarter_h),
                        ],
                        (crate::partition::PartitionType::Vert4, 4) => &[
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
            let uv_directional = matches!(d.uv_mode, 3..=8)
                || (matches!(d.uv_mode, 1 | 2) && d.uv_angle_delta != 0);
            let uv_ok = !uv_directional || !edge_filter;
            d.tx_depth == 0 && (!directional || !edge_filter) && uv_ok
        }
        crate::partition::PartitionTree::Split { children, .. } => {
            children.iter().all(|c| bd10_tree_supported(c, edge_filter))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn bd10_reencode_luma(
    all_trees: &mut [crate::partition::PartitionTree],
    sb_cols: usize,
    sb_size: usize,
    w: usize,
    h: usize,
    src10: &[u16],
    base_qindex: u8,
    rdoq_level: u8,
    lambda_bd10: u64,
    edge_filter: bool,
    bd: u8,
    qm_level: u8,
    // [SVT_HDR_MODE] fork loop_filter_sharpness (static_config.sharpness). 0 in
    // mainline → the quant table is byte-identical to build_quant_table_bd.
    sharpness: i8,
) -> crate::EncodeResult<alloc::vec::Vec<u16>> {
    let fc = svtav1_entropy::context::FrameContext::new_default();
    let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    let qt = crate::quant::build_quant_table_bd_sharp(base_qindex, bd, sharpness);
    let mut recon10 = svtav1_types::try_vec![0u16; w * h]?;
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
            &qt,
            rdoq_level,
            lambda_bd10,
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
    qt: &crate::quant::QuantTable,
    rdoq_level: u8,
    lambda: u64,
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
            let src_off = y * stride + x;
            // RDOQ contexts are 0/0 at eff-M9 (rate_est_level 0).
            let out = crate::leaf_funnel::tx_unit_hbd(
                src10,
                stride,
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
            for r in 0..bh {
                let drow = (y + r) * stride + x;
                recon10[drow..drow + bw].copy_from_slice(&out.recon[r * bw..(r + 1) * bw]);
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
            let offs: alloc::vec::Vec<(usize, usize)> = match (*partition_type, children.len()) {
                (PT::Split, 4) => alloc::vec![(0, 0), (hw, 0), (0, hh), (hw, hh)],
                (PT::Horz, 2) => alloc::vec![(0, 0), (0, hh)],
                (PT::Vert, 2) => alloc::vec![(0, 0), (hw, 0)],
                (PT::HorzA, 3) => alloc::vec![(0, 0), (hw, 0), (0, hh)],
                (PT::HorzB, 3) => alloc::vec![(0, 0), (0, hh), (hw, hh)],
                (PT::VertA, 3) => alloc::vec![(0, 0), (0, hh), (hw, 0)],
                (PT::VertB, 3) => alloc::vec![(0, 0), (hw, 0), (hw, hh)],
                (PT::Horz4, 4) => alloc::vec![(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                (PT::Vert4, 4) => alloc::vec![(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                other => panic!("bd10 reencode: unsupported partition {other:?}"),
            };
            for (child, (dx, dy)) in children.iter_mut().zip(offs) {
                bd10_reencode_node(
                    sb_mi_size,
                    child,
                    x + dx,
                    y + dy,
                    recon10,
                    stride,
                    src10,
                    qt,
                    rdoq_level,
                    lambda,
                    rates,
                    edge_filter,
                    frame_w,
                    frame_h,
                    bd,
                    qm_level,
                );
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
    u_src10: &[u16],
    v_src10: &[u16],
    cstride: usize,
    // The frame's 10-bit LUMA recon from `bd10_reencode_luma`, `w*h` at
    // stride `y_stride` — the CfL AC source for UV_CFL_PRED leaves.
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
    let fc = svtav1_entropy::context::FrameContext::new_default();
    let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(chroma_qindex);
    let rates = crate::leaf_funnel::build_md_rates(&fc, &cfc);
    // Per-plane chroma quant tables (== each other, and == the old single
    // base-qindex table, whenever the FH chroma deltas are 0 -> mainline inert).
    let qt_u = crate::quant::build_quant_table_bd_sharp(qindex_u, bd, sharpness);
    let qt_v = crate::quant::build_quant_table_bd_sharp(qindex_v, bd, sharpness);
    let (cframe_w, cframe_h) = (w / 2, h / 2);
    let mut recon10_u = svtav1_types::try_vec![0u16; cframe_w * cframe_h]?;
    let mut recon10_v = svtav1_types::try_vec![0u16; cframe_w * cframe_h]?;
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
        rates,
        rdoq_level != 0,
        bd,
        qm_level,
        None, // level-only re-encode: no RD terms
    );
    for r in 0..ch {
        let drow = (cy + r) * cstride + cx;
        recon10[drow..drow + cw].copy_from_slice(&out.recon[r * cw..(r + 1) * cw]);
    }
    let shift = (bd - 8) as u32;
    let rec_u8: alloc::vec::Vec<u8> = out.recon.iter().map(|&s| (s >> shift).min(255) as u8).collect();
    (out.qcoeff, out.eob, rec_u8)
}

#[allow(clippy::too_many_arguments)]
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
            let has_uv = ((y / 4) % 2 == 1 || bh_mi % 2 == 0) && ((x / 4) % 2 == 1 || bw_mi % 2 == 0);
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
                let mut ac =
                    alloc::vec![0i16; svtav1_dsp::intra_pred::CFL_BUF_LINE * ch.max(1)];
                crate::leaf_funnel::cfl_ac_from_frame_recon_hbd(
                    y_recon10, y_stride, x, y, bw, bh, cw, ch, &mut ac,
                );
                Some(ac)
            } else {
                None
            };
            let cfl_u = cfl_ac
                .as_ref()
                .map(|ac| (&ac[..], crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 0)));
            let cfl_v = cfl_ac
                .as_ref()
                .map(|ac| (&ac[..], crate::leaf_funnel::cfl_idx_to_alpha(d.cfl_alpha_idx, d.cfl_alpha_signs, 1)));
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
                recon10_u, u_src10, cstride, cx, cy, cw, ch, d.uv_mode, d.uv_angle_delta, uv_tt, &geom,
                edge_filter, qt_u, rdoq_level, lambda, rates, bd, qm_uv[0], cfl_u,
            );
            let (v_q, v_eob, v_rec) = bd10_reencode_chroma_plane(
                recon10_v, v_src10, cstride, cx, cy, cw, ch, d.uv_mode, d.uv_angle_delta, uv_tt, &geom,
                edge_filter, qt_v, rdoq_level, lambda, rates, bd, qm_uv[1], cfl_v,
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
            let offs: alloc::vec::Vec<(usize, usize)> = match (*partition_type, children.len()) {
                (PT::Split, 4) => alloc::vec![(0, 0), (hw, 0), (0, hh), (hw, hh)],
                (PT::Horz, 2) => alloc::vec![(0, 0), (0, hh)],
                (PT::Vert, 2) => alloc::vec![(0, 0), (hw, 0)],
                (PT::HorzA, 3) => alloc::vec![(0, 0), (hw, 0), (0, hh)],
                (PT::HorzB, 3) => alloc::vec![(0, 0), (0, hh), (hw, hh)],
                (PT::VertA, 3) => alloc::vec![(0, 0), (0, hh), (hw, 0)],
                (PT::VertB, 3) => alloc::vec![(0, 0), (hw, 0), (hw, hh)],
                (PT::Horz4, 4) => alloc::vec![(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                (PT::Vert4, 4) => alloc::vec![(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                other => panic!("bd10 chroma reencode: unsupported partition {other:?}"),
            };
            for (child, (dx, dy)) in children.iter_mut().zip(offs) {
                bd10_reencode_chroma_node(
                    sb_mi_size,
                    child,
                    x + dx,
                    y + dy,
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
                    rates,
                    edge_filter,
                    cframe_w,
                    cframe_h,
                    bd,
                    qm_uv,
                );
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
/// - **complete-SB only** — `tx_unit_hbd` is not partial-SB-aware. (The p0..=5
///   `refined` path is independently full-SB-gated.)
/// - **palette off** — a palette candidate has no 10-bit prediction here.
///
/// CfL is handled inside `evaluate_leaf` instead of here, because whether it is
/// reachable is a per-block runtime property (the chroma complexity detector),
/// not a config one: under the bd10 full-RD the CfL candidate is not offered,
/// which leaves a CfL block as a VISIBLE mode divergence rather than a
/// mixed-domain compare. See the comment at the `cfl_gate` site.
fn bd10_full_rd_supported(bit_depth: u8, preset: u8, w: usize, h: usize) -> bool {
    if bit_depth != 10 || preset > 8 || w % 64 != 0 || h % 64 != 0 {
        return false;
    }
    crate::leaf_funnel::FunnelCfg::for_preset(preset).palette_level == 0
}

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
    tile_grid: svtav1_entropy::obu::TileGrid,
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
    hdr_alt_ssim: bool,
    hdr_alt_lambda: bool,
    hdr_iq_lambda_weight: Option<u32>,
    ssim_factors: Option<&(alloc::vec::Vec<f64>, usize, usize)>,
    fh_base_qindex: u8,
    cli_qp: u8,
    hdr_sharpness: i8,
    _lambda: u64, // Per-SB lambda computed from sb_qp_offsets
    speed_config: &crate::speed_config::SpeedConfig,
    ref_frame_data: Option<&[u8]>,
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
) -> crate::EncodeResult<Vec<(
    Vec<u8>,
    Vec<crate::partition::BlockDecision>,
    Vec<crate::partition::PartitionTree>,
    // bd10 FULL-RD only: this tile's committed 10-bit winner recon, as
    // frame-extent (`ext_w` / `ext_w/2`-strided) Y/U/V canvases with only
    // this tile's SB region written. `None` outside the bd10 full-RD
    // envelope. See `Bd10Canvas` at the merge site.
    Option<(Vec<u16>, Vec<u16>, Vec<u16>)>,
)>> {
    let encode_one_tile = |tile_idx: usize| -> crate::EncodeResult<(
        Vec<u8>,
        Vec<crate::partition::BlockDecision>,
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
        // Presets 2/3 use update_cdf_level 1 and 4..=6 level 2 — for
        // I-slices the two are identical (only update_mv differs, forced
        // 0 on I-slices; set_cdf_controls, enc_mode_config.c:12047), so
        // the per-SB CDF chain gate below is 2..=6. 7/8/9+ use
        // update_cdf_level 0 (static default tables all frame).
        // eff-M9 (intra_level 8) arms the is_dc_only gate inside the funnel.
        let use_funnel = chroma_420
            && chroma_src.is_some()
            && ref_frame_data.is_none()
            && c_quant.is_some();
        // Same sc derivation as the pack side (identical inputs -> identical
        // result): the MD walk's rates + its per-SB CDF evolution must see
        // the same allow_sct as the real pack or the chains desync on
        // screen-content frames.
        let tile_sc = crate::sc_detect::derive_allintra_sc(
            speed_config.preset,
            encode_input,
            w,
            w,
            h,
        );
        let mut funnel_cfg = crate::leaf_funnel::FunnelCfg::for_preset(speed_config.preset);
        funnel_cfg.allow_sct = tile_sc.allow_screen_content_tools;
        // THE palette flip-on: with the level stamped, the funnel injects
        // palette candidates (chunk 4) and the pack codes the winners
        // (chunk 5). sc_derivation.palette_level is 0 on every non-sc
        // frame, so non-screen-content streams are untouched.
        funnel_cfg.palette_level = tile_sc.palette_level;
        // IBC chunk 3: the frame-level svt_aom_allow_intrabc (always
        // I-slice + sct on this path) — arms the per-candidate
        // intrabc_fac_bits[0] charge in the funnel. False on every
        // non-screen / p5+ frame (byte-inert there).
        funnel_cfg.allow_intrabc = tile_sc.allow_intrabc;
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
            let mut e = EntropyCtx::new(w / 4, h / 4, true, tile_sc.allow_screen_content_tools);
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
            let fc = svtav1_entropy::context::FrameContext::new_default();
            let cfc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
            Some(crate::leaf_funnel::build_md_rates(&fc, &cfc))
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
                base_qindex,
                bit_depth,
                qindex_u,
                qindex_v,
                ac_bias_eff,
                // IBC chunk 7: frame-constant DV RD tables (default ndvc at
                // MV_SUBPEL_NONE — `build_dv_cost_tables`'s cadence doc) +
                // the aligned frame height for the vartx bottom clip.
                dv_tables: crate::intrabc::build_dv_cost_tables(
                    &svtav1_entropy::mv_coding::NmvContext::default(),
                    funnel_cfg.allow_intrabc,
                    false, // approx_inter_rate: structurally 0 on allintra
                ),
                frame_h_px: h,
                cfg: funnel_cfg,
            })
        } else {
            None
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
        let funnel_chain = use_funnel && matches!(speed_config.preset, 0..=6) && multi_sb;
        let mut chain_snaps: Vec<(
            svtav1_entropy::context::FrameContext,
            alloc::boxed::Box<svtav1_entropy::coeff_c::CoeffFc>,
        )> = Vec::new();
        let mut sim_ectx = if funnel_chain {
            // The chain simulation re-codes each SB's symbols to evolve the
            // per-SB frame contexts — it must code the same no-palette
            // flags as the real pack or the palette CDF rows drift.
            let mut e = EntropyCtx::new(w / 4, h / 4, true, tile_sc.allow_screen_content_tools);
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
        let mut sim_geom = crate::deblock::DeblockGeom::new(w, h);
        let mut sim_u = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
        let mut sim_v = svtav1_types::try_vec![128u8; if funnel_chain { ext_cbuf } else { 0 }]?;
        let mut sim_prev_sb_row = usize::MAX;
        let mut fun_rates = fun_rates;
        let mut tile_decisions: Vec<crate::partition::BlockDecision> = Vec::new();
        let mut tile_trees: Vec<crate::partition::PartitionTree> = Vec::new();
        let mut tile_frame_recon = svtav1_types::try_vec![128u8; ext_w * ext_h]?;
        // bd10 LUMA mode funnel (task #94): a parallel TRUE 10-bit recon canvas,
        // maintained ONLY for complete-SB eff-M9 (preset ≥ 9) bd10 frames so the
        // per-block mode decision (evaluate_leaf MDS0) is made on the 10-bit
        // recon rather than the MSB-truncated u8 recon (which scales SATD ×4 on
        // `sample<<2` content and cannot flip the survivor). bd8 and every other
        // bd10 preset/partial-SB allocate NOTHING and pass `None` into FunnelCtx
        // → the funnel is byte-IDENTICAL. Frame-persistent (a block reads its
        // left/above SB's committed 10-bit recon); each SB's FunnelCtx borrows
        // it. On a 64-aligned frame ext_w==w, so this mirrors tile_frame_recon.
        let bd10_complete_sb = bit_depth == 10 && w % 64 == 0 && h % 64 == 0;
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
        let bd10_full_rd = bd10_full_rd_supported(bit_depth, speed_config.preset, w, h);
        let bd10_luma_funnel =
            bd10_complete_sb && (speed_config.preset >= 9 || bd10_full_rd);
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
            (svtav1_types::try_vec![512u16; n]?, svtav1_types::try_vec![512u16; n]?)
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
                stop.check().map_err(EncodeError::from).map_err(whereat::at)?;
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
                    f.qindex_u = (i32::from(sbq) + i32::from(chroma_ac_deltas.0))
                        .clamp(0, 255) as u8;
                    f.qindex_v = (i32::from(sbq) + i32::from(chroma_ac_deltas.1))
                        .clamp(0, 255) as u8;
                    // [SVT_HDR_MODE] per-SB lambda: alt KF factor (fork
                    // default) + the delta-q qdiff stats factor
                    // (rc_process.c:437-446; this path is fork-only).
                    #[cfg(feature = "std")]
                    if std::env::var("SVTAV1_LAMBDA_DBG").is_ok() {
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
                            hdr_iq_lambda_weight,
                        ));
                    }
                }

                let ref_ctx = ref_frame_data.map(|rf| crate::partition::RefFrameCtx {
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
                let sb_qindex = base_qindex;
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
                let use_pd0 = ref_ctx.is_none()
                    && (speed_config.preset >= 6
                        || (matches!(speed_config.preset, 0..=5) && use_funnel));
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
                let local_sb_index = (sb_row - tile_sb_row_start) * tile_sb_cols
                    + (sb_col - tile_sb_col_start);
                let chain_base = if funnel_chain {
                    // C `ec_ctx_array[sb]` neighbor rule for the rate-estimation
                    // CDF (enc_dec_process.c:3002-3022). `pic_based_rate_est` is
                    // only ever false (enc_handle.c), so the weighted-average
                    // branch always runs. Availability predicates match C for a
                    // single-tile SB-aligned frame: left = not tile-left column,
                    // top-right = not tile-top row AND the SB one to the right
                    // exists (so the last column has no top-right).
                    let left_avail = sb_col > tile_sb_col_start;
                    let topright_avail =
                        sb_row > tile_sb_row_start && sb_col + 1 < tile_sb_col_end;
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
                if funnel_chain && std::env::var_os("SVTAV1_CHAIN_DUMP").is_some() {
                    let dflt_cfc;
                    let cfc: &svtav1_entropy::coeff_c::CoeffFc = match &chain_base {
                        Some((_, cfc)) => cfc.as_ref(),
                        None => {
                            dflt_cfc =
                                svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
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
                if funnel_chain && std::env::var_os("SVTAV1_SEED_DUMP").is_some() {
                    let dflt;
                    let (fc, cfc): (
                        &svtav1_entropy::context::FrameContext,
                        &svtav1_entropy::coeff_c::CoeffFc,
                    ) = match &chain_base {
                        Some((fc, cfc)) => (fc, cfc.as_ref()),
                        None => {
                            dflt = (
                                svtav1_entropy::context::FrameContext::new_default(),
                                svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
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
                            let fc = svtav1_entropy::context::FrameContext::new_default();
                            let cfc =
                                svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
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
                let full_sb = cur_w == unit_size && cur_h == unit_size;
                let sb_result = if use_pd0 {
                    if speed_config.preset >= 9 {
                        let tree = if bit_depth == 10 {
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
                                // [SVT_HDR_MODE] Frame luma QM level. C forces
                                // PD0_LVL_0 at bd10 whose light encode applies
                                // the matrix when using_qmatrix (fork default);
                                // mainline/QM-off leave qm_levels = [15;3], so
                                // this is the non-QM (byte-inert) path there.
                                qm_levels[0],
                                crate::pd0::input_resolution_factor(w * h),
                                w,
                                h,
                            )
                        } else {
                            crate::pd0::pd0_pick_sb_partition(
                                sb_input,
                                in_stride,
                                x0,
                                y0,
                                cli_qp as u32,
                                sb_qindex,
                                // C `input_resolution_factor[input_resolution]`:
                                // per-picture coeff-rate addend keyed on w*h.
                                crate::pd0::input_resolution_factor(w * h),
                                // ALIGNED dims — the spec-5.11.4 edge predicate grid.
                                w,
                                h,
                            )
                        };
                        // The same per-SB variance map C's picture analysis
                        // feeds to is_dc_only_safe (pcs->ppcs->variance): the
                        // fixed-tree leaves use it to force the C-exact
                        // DC-only intra candidate set where the gate fires.
                        let sb_vars = crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0);
                        let mut funnel_ctx = if use_funnel {
                            let (u_src, v_src) = chroma_src.unwrap();
                            Some(crate::leaf_funnel::FunnelCtx {
                                u_src,
                                v_src,
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
                            None => m6_pd0_tables
                                .get_or_insert_with(|| crate::pd0::build_m6_pd0_tables(sb_qindex)),
                        };
                        // The PD1 depth-refinement path (depth_refine.rs) is not
                        // yet edge-aware, so partial SBs at presets 0..=5 fall
                        // back to the plain PD0 fixed tree below (which carries
                        // the forced edge splits). Full SBs are unaffected.
                        let refined = matches!(speed_config.preset, 0..=5) && use_funnel && full_sb;
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
                            let dr = crate::depth_refine::DrCtrls::for_preset(speed_config.preset);
                            let eval = crate::pd0::pd0_pick_sb_partition_m6_eval(
                                encode_input,
                                w,
                                x0,
                                y0,
                                cli_qp as u32,
                                sb_qindex,
                                tables,
                                if dr.disallow_4x4 { 8 } else { 4 },
                                // M4/M5: rate_est_level 1 -> coeff_rate_est_lvl 1
                                // (real PD0 coeff rate). M7/M8's level-2 PD0
                                // approximation only fires when this is >= 2.
                                funnel_cfg.coeff_rate_est_lvl,
                                // max-block variance cap: M8+ only
                                // (get_max_block_size_allintra base th ~0
                                // through M7) — never on this p<=5 branch.
                                false,
                                // NSQ enabled: this branch is preset 0..=5
                                // (nsq_geom_level 1/2/3), so a one-false node
                                // keeps its edge shape. Inert here (full-SB
                                // gated), but correct for the predicate.
                                true,
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
                                            encode_input,
                                            w,
                                            ux,
                                            uy,
                                            cli_qp as u32,
                                            sb_qindex,
                                            tables,
                                            if dr.disallow_4x4 { 8 } else { 4 },
                                            funnel_cfg.coeff_rate_est_lvl,
                                            false,
                                            true,
                                            w,
                                            h,
                                            tile_sb_row_start * sb_size,
                                            tile_sb_col_start * sb_size,
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
                            );
                            // Partition rates at the real contexts, from
                            // the same (possibly chained) frame context as
                            // the funnel's syntax rates.
                            let part_rates = match &chain_base {
                                Some((fc, _)) => crate::depth_refine::PartRates::from_fc(fc),
                                None => crate::depth_refine::PartRates::from_fc(
                                    &svtav1_entropy::context::FrameContext::new_default(),
                                ),
                            };
                            let (u_src, v_src) = chroma_src.unwrap();
                            let mut fx = crate::leaf_funnel::FunnelCtx {
                                u_src,
                                v_src,
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
                                full_rd10: bd10_full_rd,
                            };
                            let nsq = crate::depth_refine::NsqCfg::for_preset_qp(
                                speed_config.preset,
                                cli_qp as u32,
                            );
                            crate::depth_refine::decide_sb_refined(
                                &scan,
                                &mut fx,
                                encode_input,
                                w,
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
                                tables,
                                8,
                                // M6: coeff_rate_est_lvl 1 (real PD0 coeff
                                // rate, unchanged). M7/M8: 2 -> the C
                                // perform_tx_pd0 `eob<th ? 6000+eob*500`
                                // approximation that lowers the parent-NONE
                                // cost and matches C's partition depth.
                                funnel_cfg.coeff_rate_est_lvl,
                                // C get_max_block_size_allintra: the
                                // 64-variance cap fires at M8+ only, and
                                // stays at sb_size for incomplete edge SBs.
                                speed_config.preset >= 8
                                    && x0 + 64 <= w
                                    && y0 + 64 <= h,
                                // NSQ geom enabled iff enc_mode <= M6
                                // (svt_aom_get_nsq_geom_level_allintra: presets
                                // 0..=6 → level 1/2/3 → enabled; presets 7/8 →
                                // level 0 → disabled). When disabled, a
                                // one-false boundary node force-splits (no edge
                                // shape) — the presets 7/8 partial-SB fix.
                                speed_config.preset <= 6,
                                // ALIGNED dims — the spec-5.11.4 edge grid.
                                w,
                                h,
                                // This tile's pixel origin: the M6 PD0 leaf-cost
                                // DC prediction must not read across a tile
                                // boundary (C up/left_available respect tiles).
                                // 0 for a single-tile frame → byte-inert.
                                tile_sb_row_start * sb_size,
                                tile_sb_col_start * sb_size,
                            );
                            #[cfg(feature = "std")]
                            if std::env::var_os("SVTAV1_PD0DBG").is_some()
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
                            let sb_vars = crate::pd0::compute_b64_variance(sb_input, in_stride, x0, y0);
                            let mut funnel_ctx = if use_funnel {
                                let (u_src, v_src) = chroma_src.unwrap();
                                Some(crate::leaf_funnel::FunnelCtx {
                                    u_src,
                                    v_src,
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
                            svtav1_entropy::context::FrameContext::new_default(),
                            svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex),
                        )
                    });
                    if let Some(tree) = sb_result.tree.as_ref() {
                        let se = sim_ectx.as_mut().unwrap();
                        if sb_row != sim_prev_sb_row {
                            se.reset_left_for_sb_row();
                            sim_prev_sb_row = sb_row;
                        }
                        let (u_src, v_src) = chroma_src.unwrap();
                        let mut sim_writer =
                            svtav1_entropy::writer::AomWriter::new(w * h * 2 + 256);
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
                let mut sb_recon = alloc::vec![0u8; sb_cur_w * sb_cur_h];
                for r in 0..sb_cur_h {
                    let src_off = (sb_y0 + r) * w + sb_x0;
                    sb_recon[r * sb_cur_w..(r + 1) * sb_cur_w]
                        .copy_from_slice(&tile_frame_recon[src_off..src_off + sb_cur_w]);
                }
                tile_recon.extend_from_slice(&sb_recon);
                tile_decisions.extend(sb_result.decisions);
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
        Ok((tile_recon, tile_decisions, tile_trees, tile_canvas10))
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

    /// Issue #5: QP 0 derives base_qindex 0 = coded-lossless signaling, which
    /// the search/recon side does not implement — encoding would emit a
    /// valid-syntax stream of the WRONG image. The fallible entry must reject
    /// it with a typed error (never silent corruption), and QP 1 — the exact
    /// boundary the issue measured clean — must still encode.
    #[test]
    fn qp0_returns_unsupported_config_not_garbage() {
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
        let y = vec![128u8; 64 * 64];
        let u = vec![100u8; 32 * 32];
        let v = vec![150u8; 32 * 32];

        let err = mk(0)
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect_err("QP 0 must be rejected, not encoded to garbage (issue #5)");
        assert!(
            matches!(err.error(), EncodeError::UnsupportedConfig(_)),
            "expected EncodeError::UnsupportedConfig, got {err:?}"
        );

        // Anti-vacuity + the exact boundary: QP 1 (qindex 4) still encodes.
        let obu = mk(1)
            .try_encode_frame_420(&y, &u, &v, 64)
            .expect("QP 1 is the verified floor and must keep encoding");
        assert!(!obu.is_empty());
    }

    /// Issue #5, legacy surface: the panicking `encode_frame` contract turns
    /// the QP-0 rejection into a panic (never a silently-corrupt bitstream).
    #[test]
    #[should_panic(expected = "coded-lossless")]
    fn qp0_legacy_encode_panics() {
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
        for i in 0..5 {
            let bitstream = pipeline.encode_frame(&y_plane, 64);
            assert!(!bitstream.is_empty(), "frame {i} should produce output");
        }
        assert_eq!(pipeline.frame_count, 5);
        assert_eq!(pipeline.rc_state.total_frames, 5);
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
        let ectx = EntropyCtx::new(64, 64, true, false);
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
        assert!(!p.sb128_fallback, "the SB128 encode path is capability-enabled");
        // preset 2 at the same size is genuinely SB64 in C.
        let p = EncodePipeline::new(512, 384, 2, RcConfig::default(), 0, 1);
        assert_eq!(p.sb_size, 64);
        assert!(!p.sb128_fallback);
        // Explicit override asks for 128 on a frame the C rule puts at 64:
        // honoured, because the walk is size- (and preset-) agnostic.
        let p = EncodePipeline::new(64, 64, 0, RcConfig::default(), 0, 1).with_sb_size(Some(128));
        assert_eq!(p.sb_size, 128);
        assert_eq!(p.derived_sb_size, 64, "the C rule's own answer is preserved");
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
        assert_eq!(EncodePipeline::resolve_sb_size(128, Some(64), 0), (64, false));
        assert_eq!(EncodePipeline::resolve_sb_size(64, Some(128), 0), (128, false));
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
        assert_eq!(sb_coding_units(448, 320, 64, 512, 384), alloc::vec![(448, 320)]);
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
        assert_eq!(sb_coding_units(384, 384, 128, 448, 448), alloc::vec![(384, 384)]);
    }
}

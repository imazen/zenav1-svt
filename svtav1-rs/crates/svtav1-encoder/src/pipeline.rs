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
use alloc::vec::Vec;

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
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
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
    /// CDEF evidence counters for the last encoded frame (non-vacuity
    /// reporting: how many pixels the signaled strengths actually touched).
    pub last_cdef_stats: crate::cdef::CdefStats,
    /// Loop-restoration evidence for the last encoded frame: per-plane
    /// frame types (0 NONE / 1 WIENER) + the number of RUs that signaled
    /// wiener. Zeroed when the search does not run.
    pub last_lr_stats: ([u8; 3], usize),
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
        Self {
            hdr: crate::hdr_mode::HdrForkConfig::default(),
            speed_config: SpeedConfig::from_preset(preset),
            rc_config,
            rc_state: RcState::default(),
            dpb: DecodedPictureBuffer::new(),
            gop: GopStructure::new(hierarchical_levels, intra_period),
            frame_count: 0,
            width,
            height,
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
            last_cdef_stats: crate::cdef::CdefStats::default(),
            last_lr_stats: ([0; 3], 0),
        }
    }

    /// Set bit depth (8, 10, or 12).
    pub fn with_bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = depth;
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

    /// Encode a single frame through the full pipeline (monochrome).
    ///
    /// Returns the encoded bitstream data and updates internal state.
    pub fn encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> Vec<u8> {
        self.encode_frame_impl(y_plane, y_stride, None)
    }

    /// Encode a single 4:2:0 still/key frame (NumPlanes=3).
    ///
    /// `u`/`v` are (w/2 x h/2) planes tightly packed at stride w/2, where
    /// (w, h) are the pipeline frame dimensions (64-aligned in practice).
    /// Requires `chroma_420` to be enabled via [`Self::with_chroma_420`].
    pub fn encode_frame_420(&mut self, y: &[u8], u: &[u8], v: &[u8], y_stride: usize) -> Vec<u8> {
        assert!(
            self.chroma_420,
            "encode_frame_420 requires the pipeline to be built with with_chroma_420(true)"
        );
        let cn = (self.width as usize / 2) * (self.height as usize / 2);
        assert!(
            u.len() >= cn && v.len() >= cn,
            "u/v planes must be (w/2 x h/2)"
        );
        self.encode_frame_impl(y, y_stride, Some((u, v)))
    }

    /// Shared frame encode body. `chroma = Some((u, v))` selects the 4:2:0
    /// path; `None` is the unchanged monochrome path.
    fn encode_frame_impl(
        &mut self,
        y_plane: &[u8],
        y_stride: usize,
        chroma: Option<(&[u8], &[u8])>,
    ) -> Vec<u8> {
        let display_order = self.frame_count;

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
                    );
                    tf_result.filtered
                } else {
                    y_plane[..n].to_vec()
                }
            } else {
                y_plane[..n].to_vec()
            };

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
            let mut vars = alloc::vec::Vec::with_capacity(sb_cols_p * sb_rows_p);
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
            );
            base_qindex = plan.base_qindex;
            tpl_adjusted_qp = ((i32::from(plan.base_qindex) + 2) >> 2).clamp(0, 63) as u8;
            Some(plan)
        } else {
            None
        };

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
            if is_key && w % 64 == 0 && h % 64 == 0 {
                let mut tot: u64 = 0;
                let mut cnt: u64 = 0;
                for sy in (0..h).step_by(64) {
                    for sx in (0..w).step_by(64) {
                        tot +=
                            crate::pd0::compute_b64_variance(&encode_input, w, sx, sy).0[0] as u64;
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
                let lambda = crate::pd0::kf_full_lambda_8bit(base_qindex, tpl_adjusted_qp as u32);
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
        let mut recon = alloc::vec![128u8; n];
        // AV1 spec: use_128x128_superblock=0 in SH → sb_size=64.
        // The decoder always uses 64x64 SBs when this flag is 0.
        // The encoder's max_partition_depth controls how deep the
        // partition search goes WITHIN each 64x64 SB, not the SB size.
        let sb_size = 64;
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
        let mut mv_map = alloc::vec![svtav1_types::motion::Mv::ZERO; mv_map_size];

        // Compute per-SB TPL QP offsets for spatial bit allocation
        let sb_qp_offsets = if !is_key {
            if let Some(ref rf) = ref_frame_data {
                crate::rate_control::tpl_sb_qp_offsets(&encode_input, rf, w, h, w, sb_size)
            } else {
                alloc::vec![0i8; sb_cols * sb_rows]
            }
        } else {
            alloc::vec![0i8; sb_cols * sb_rows]
        };

        // Single tile row for bitstream conformance.
        // The decoder expects a single contiguous reconstruction buffer where
        // each SB's prediction reads from previously-encoded neighbors.
        // Tile-parallel encoding with separate recon buffers per tile row
        // breaks neighbor prediction continuity, producing different results
        // than what the decoder reconstructs.
        //
        // TODO: Implement proper multi-tile with per-tile entropy streams
        // and tile_info in the frame header. Until then, parallelism happens
        // at the SB level via partition search, not at the tile level.
        let tile_rows = 1;
        let rows_per_tile = sb_rows.div_ceil(tile_rows);

        // [SVT_HDR_MODE] fork chroma-q: derive the FH per-plane deltas and
        // the plane qindexes the quantizer must use. Mainline: all zero.
        let chroma_deltas = if self.hdr.is_fork() {
            crate::chroma_q::fork_chroma_q_deltas(base_qindex, &self.color_description)
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
        let sharp_tx_active = self.hdr.is_fork() && self.hdr.sharp_tx == 1 && sb_plan.is_some();
        // [SVT_HDR_MODE] frame QM levels (svt_av1_qm_init,
        // md_config_process.c:249): the linear qindex map (default tune =
        // PSNR in the fork); chroma levels derive from base + the FH
        // chroma AC deltas. [15;3] = QM off (identity).
        let qm_levels: [u8; 3] = if self.hdr.is_fork() && self.hdr.enable_qm {
            let lvl = |q: i32, lo: u8, hi: u8| {
                crate::qm::aom_get_qmlevel(q, i32::from(lo), i32::from(hi)) as u8
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
            w,
            h,
            sb_size,
            sb_cols,
            sb_rows,
            rows_per_tile,
            tile_rows,
            base_qindex,
            qindex_u,
            qindex_v,
            ac_bias_eff,
            sb_plan.as_ref().map(|p| p.sb_qindex.as_slice()),
            (chroma_deltas.u_ac, chroma_deltas.v_ac),
            sharp_tx_active,
            if self.hdr.is_fork() { self.hdr.noise_norm_strength } else { 0 },
            qm_levels,
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
            chroma.as_ref().map(|c| (c.0, c.1)),
        );

        let mut per_tile_decisions: Vec<Vec<crate::partition::BlockDecision>> = Vec::new();
        let mut all_trees: Vec<crate::partition::PartitionTree> = Vec::new();

        // Merge tile recons into frame buffer and update MV map
        for (tile_idx, (tile_recon, tile_decisions, tile_trees)) in tile_recons.iter().enumerate() {
            per_tile_decisions.push(tile_decisions.clone());
            all_trees.extend_from_slice(tile_trees);
            let tile_sb_row_start = tile_idx * rows_per_tile;
            let tile_sb_row_end = ((tile_idx + 1) * rows_per_tile).min(sb_rows);
            let mut offset = 0;
            for sb_row in tile_sb_row_start..tile_sb_row_end {
                for sb_col in 0..sb_cols {
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
        let chh = h / 2;
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
            // [SVT_HDR_MODE] the fork ALWAYS signals separate_uv_delta_q
            // (its FH writes independent U/V deltas — entropy_coding.c
            // fork block hardcodes both flags true).
            if self.hdr.is_fork() {
                t.separate_uv_delta_q = true;
            }
            // enable_intra_edge_filter's C-parity surface is still/420
            // (the C matched config). The mono extension keeps 0: C cannot
            // emit mono, and the mono leaf coder predicts without edge
            // filtering — signaling 0 keeps our recon decoder-exact on
            // that self-consistent surface.
            t.enable_intra_edge_filter &= self.chroma_420;
            t
        };

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
         -> (
            Vec<u8>,
            crate::deblock::DeblockGeom,
            Vec<u8>,
            Vec<u8>,
        ) {
            let (mut u_recon, mut v_recon) = if chroma.is_some() {
                (alloc::vec![128u8; cw * chh], alloc::vec![128u8; cw * chh])
            } else {
                (Vec::new(), Vec::new())
            };
            // Per-4x4 block/TX/skip geometry for the deblocking edge walk,
            // recorded in coding order (== the decoder's parse order).
            let mut deblock_geom = crate::deblock::DeblockGeom::new(w, h);
            let mut writer = svtav1_entropy::writer::AomWriter::new(n + 256);
            // CDF updates enabled — matches the frame header's disable_cdf_update=0
            let mut frame_ctx = svtav1_entropy::context::FrameContext::new_default();
            // C-exact coefficient CDFs for the base_q_idx bucket
            // (svt_av1_default_coef_probs semantics) — qindex domain.
            let mut coeff_fc = svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(base_qindex);
            // Mode/skip context tracking at 4x4 granularity
            let w4 = w.div_ceil(4);
            let h4 = h.div_ceil(4);
            let mut ectx = EntropyCtx::new(
                w4,
                h4,
                seq_tools.enable_filter_intra,
                sc_derivation.allow_screen_content_tools,
            );
            // [SVT_HDR_MODE] arm per-SB delta-q: prev starts at the FH base
            // (C prev_qindex tile-init); uniform plan = every SB at base.
            if let Some(res) = delta_q_res_signal {
                ectx.delta_q_state = Some((res, i32::from(base_qindex)));
                ectx.delta_q_sb_qindex = i32::from(base_qindex);
            }
            let mut chroma_pass = chroma.map(|(u_src, v_src)| ChromaPass {
                u_src,
                v_src,
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

            debug_assert_eq!(
                all_trees.len(),
                sb_cols * sb_rows,
                "tree count {} != SB count {}x{}={}",
                all_trees.len(),
                sb_cols,
                sb_rows,
                sb_cols * sb_rows,
            );
            let mut prev_sb_row = usize::MAX;
            for (sb_idx, tree) in all_trees.iter().enumerate() {
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
                let sb_col = sb_idx % sb_cols;
                let sb_row = sb_idx / sb_cols;
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
                ectx.cdef_pending = cdef_walk.and_then(|p| {
                    (p.bits > 0).then(|| (p.bits, p.fb_idx[sb_row * p.nhfb + sb_col]))
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
                        w,
                        h,
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

            (
                svtav1_entropy::obu::build_tile_group_single(writer.done()),
                deblock_geom,
                u_recon,
                v_recon,
            )
        };
        let (mut tile_data, deblock_geom, mut u_recon, mut v_recon) = run_entropy_walk(None, None);

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
        let lf_levels = if is_key {
            if is_single_frame && self.speed_config.preset <= 5 {
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let input = crate::deblock::DlfSearchInput {
                    sharpness: self.hdr.sharpness.clamp(0, 7) as u8,
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
                    early_exit_convergence: if self.speed_config.preset <= 3 { 0 } else { 1 },
                };
                crate::deblock::pick_filter_levels_full_search(&input)
            } else {
                crate::deblock::pick_filter_levels_key_frame(base_qindex)
            }
        } else {
            crate::deblock::LfLevels::default()
        };
        self.last_recon_unfiltered = Some((recon.clone(), u_recon.clone(), v_recon.clone()));
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
                self.hdr.sharpness.clamp(0, 7) as u8, // = signaled loop_filter_sharpness
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
        let cdef_params = if is_key {
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
                    match crate::cdef::cdef_search_still(
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
                    ) {
                        crate::cdef::CdefSearchPick::Picked(p) => p,
                        crate::cdef::CdefSearchPick::AllSkip => crate::cdef::CdefPick::single(
                            crate::cdef::pick_cdef_params_all_skip_search(base_qindex),
                        ),
                    }
                }
            } else {
                crate::cdef::CdefPick::single(crate::cdef::pick_cdef_params_key_frame(
                    base_qindex,
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
            let (tile_cdef, _geom_c, u_c, v_c) = run_entropy_walk(None, Some(&cdef_params));
            debug_assert_eq!(u_c, u_recon, "cdef re-walk chroma recon must be identical");
            debug_assert_eq!(v_c, v_recon, "cdef re-walk chroma recon must be identical");
            tile_data = tile_cdef;
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
        if is_key && seq_tools.enable_restoration {
            let ctrls = crate::restoration::wn_filter_ctrls_allintra(self.speed_config.preset);
            if ctrls.enabled {
                let rdmult = crate::pd0::kf_full_lambda_8bit_unweighted(base_qindex) as i64;
                let (su, sv) = chroma.unwrap_or((&[][..], &[][..]));
                let rest_info = crate::restoration::search_restoration_still(
                    &ctrls,
                    &encode_input,
                    su,
                    sv,
                    &recon,
                    &u_recon,
                    &v_recon,
                    w,
                    h,
                    chroma.is_some(),
                    rdmult,
                );
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
                    let (tile_lr, _geom2, u2, v2) =
                        run_entropy_walk(Some(&rest_info), cdef_walk_opt);
                    debug_assert_eq!(u2, u_recon, "re-walk chroma recon must be identical");
                    debug_assert_eq!(v2, v_recon, "re-walk chroma recon must be identical");
                    tile_data = tile_lr;

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
                self.width,
                self.height,
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
            let fh_bytes = svtav1_entropy::obu::write_key_frame_header_full_lr(
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
                self.hdr.sharpness.clamp(0, 7) as u8,
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
            );
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
        bitstream
    }
}

/// Encode tile rows, returning per-tile recon buffers.
///
/// When the `std` feature is enabled and there are multiple tile rows,
/// uses `std::thread::scope` for parallel encoding. Otherwise sequential.
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
    /// [SVT_HDR_MODE] per-SB delta-q emission state (C write_modes_b,
    /// entropy_coding.c:4997): `Some((delta_q_res, prev_qindex))` when the
    /// FH signaled delta_q_present. The walk arms `delta_q_pending` with
    /// the SB's target qindex at each SB start; the FIRST block whose
    /// origin is the SB corner (and bsize != SB size || !skip) emits
    /// `(cur - prev) / res` via av1_write_delta_q_index and updates prev.
    pub delta_q_state: Option<(u8, i32)>,
    /// The current SB's target qindex, set by the walk at SB start.
    pub delta_q_sb_qindex: i32,
    /// Pending per-SB `cdef_idx` emission (C write_cdef,
    /// entropy_coding.c:4034: `aom_write_literal(w, mbmi->cdef_strength,
    /// cdef_bits)` at the FIRST NON-SKIP block of each 64x64). Set at SB
    /// start by the walk when `cdef_bits > 0`; `take()`n at emission.
    cdef_pending: Option<(u8, u8)>,
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
            seq_filter_intra,
            allow_sct,
            delta_q_state: None,
            delta_q_sb_qindex: 0,
            cdef_pending: None,
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
    fn bsl(width: usize) -> usize {
        match width {
            w if w <= 8 => 0,
            w if w <= 16 => 1,
            w if w <= 32 => 2,
            _ => 3,
        }
    }

    /// Convert our bsl to rav1d BlockLevel.
    /// bsl=0 (8x8) → bl=4, bsl=1 (16x16) → bl=3, bsl=2 (32x32) → bl=2, bsl=3 (64x64) → bl=1.
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

    /// C `get_filt_type(xd, plane = 0)` (enc_intra_prediction.c:20): 1
    /// when the above OR left neighbour block's Y mode is smooth
    /// (SMOOTH/SMOOTH_V/SMOOTH_H), else 0. Neighbours are the blocks at
    /// (mi_row - 1, mi_col) / (mi_row, mi_col - 1); unavailable -> 0.
    pub(crate) fn filt_type_y(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9 | 10 | 11);
        let ab = y > 0 && smooth(self.above_mode[x / 4]);
        let le = x > 0 && smooth(self.left_mode[y / 4]);
        i32::from(ab || le)
    }

    /// C `get_filt_type(xd, plane > 0)`: same over the neighbours' UV
    /// modes (chroma_above/left_mbmi; min-8x8 blocks make the +1-mi
    /// group offsets land in the same neighbour block).
    pub(crate) fn filt_type_uv(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9 | 10 | 11);
        let ab = y > 0 && smooth(self.above_uv_mode[x / 4]);
        let le = x > 0 && smooth(self.left_uv_mode[y / 4]);
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
        // (set_mi_row_col: mi_row/col > tile start; single tile here).
        let has_above = y > 0;
        let has_left = x > 0;
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
                "PTREE mi=({},{}) bsize={} part={} mode={} uv={} fi={} ady={} aduv={} txd={} yeob={} ueob={} veob={} cflidx={} cflsgn={}",
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
            );
        }
    }
    // Diagnostic (SVTAV1_PACKTREE_COEFF="mi_row,mi_col"): the pinned
    // block's PACKED nonzero luma+chroma levels as (raster_idx:level)
    // pairs — the port counterpart of the C CCOEF wrap dump (final coded
    // levels), bounded to one stderr line per block.
    #[cfg(feature = "std")]
    if let Ok(xy) = std::env::var("SVTAV1_PACKTREE_COEFF") {
        let want: alloc::vec::Vec<usize> =
            xy.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if want.len() == 2 && want[0] == block_y / 4 && want[1] == block_x / 4 {
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
                .map(|c| (fmt_nz(&c.0, 12), fmt_nz(&c.1, 12)))
                .unwrap_or_default();
            eprintln!(
                "PCOEF mi=({},{}) yeob={} txt={} ynz=[{}] unz=[{}] vnz=[{}]",
                block_y / 4,
                block_x / 4,
                decision.eob,
                decision.tx_type,
                fmt_nz(&decision.qcoeffs, 24),
                unz,
                vnz
            );
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
                cp.qm_u,
            );
            let (v_q, v_eob) = crate::partition::encode_chroma_block_dc(
                cp.v_src, cp.v_recon, cp.stride, cx, cy, cw, ch, cp.qindex_v, cp.c_quant,
                cp.qm_v,
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

    // Per-SB cdef_idx (C write_cdef, entropy_coding.c:4034-4065; spec
    // read_cdef): at the FIRST NON-SKIP coded block of each 64x64,
    // `cdef_bits` raw literal bits carry the filter block's strength
    // index. Armed by the walk at SB start only when cdef_bits > 0
    // (aom_write_literal with 0 bits is a no-iteration loop).
    if !skip {
        if let Some((bits, idx)) = ectx.cdef_pending.take() {
            writer.write_literal(idx as u32, bits as u32);
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
    // The port codes no palette blocks yet, so both flags take the
    // symbol-0 arm and the neighbor ctx is 0 (no neighbor ever has
    // palette_size > 0); the CDF updates + per-SB avg chain still run,
    // which is what keeps the arithmetic stream aligned with C on
    // screen-content frames.
    if !decision.is_inter
        && svtav1_entropy::context::allow_palette(
            ectx.allow_sct,
            decision.width as usize,
            decision.height as usize,
        )
    {
        svtav1_entropy::context::write_no_palette_flags(
            writer,
            frame_ctx,
            decision.width as usize,
            decision.height as usize,
            decision.intra_mode,
            decision.uv_mode,
            chroma_blocks.is_some(),
            0,
        );
    }

    // use_filter_intra flag — C writes it right after the uv/palette
    // syntax (palette is never allowed for us:
    // allow_screen_content_tools = 0 -> svt_aom_allow_palette false) and
    // BEFORE code_tx_size, for every intra block passing
    // svt_aom_filter_intra_allowed (mode_decision.c:102-108): SH
    // filter_intra level != 0, mode == DC_PRED, palette_size == 0 (always
    // for us), and block_size_wide/high[bsize] <= 32. Write order:
    // entropy_coding.c:5098-5112 (key frames; the inter-frame intra path
    // :5231-5236 is identical — eligibility does not involve the frame
    // type or chroma presence, so this applies to mono streams too;
    // decoder mirror read_filter_intra_mode_info). We never PREDICT with
    // filter-intra, so the flag is always 0 — but when the SH signals the
    // tool the symbol MUST be coded or the decoder desyncs.
    if ectx.seq_filter_intra
        && !decision.is_inter
        && decision.intra_mode == 0 // DC_PRED
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

    // tx_size syntax — C av1_code_tx_size (entropy_coding.c:4697) called
    // from write_modes_b right after the uv/palette/filter_intra syntax
    // and before the residuals. Key frames signal TX_MODE_SELECT in the
    // FH (like C always does), so every INTRA block with bsize > 4x4
    // codes a tx_depth symbol — skip only suppresses it for inter
    // blocks, and our depth is always 0 (largest TX). The neighbor
    // context update (set_txfm_ctxs) runs for EVERY block, signaling or
    // not. Inter frames signal TX_MODE_LARGEST (no symbol), but keep
    // their context arrays maintained exactly like C's else-branch.
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
#[allow(clippy::too_many_arguments)]
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
                svtav1_entropy::context::write_partition(
                    writer, frame_ctx, ctx, 0, nsymbs, // 0 = PARTITION_NONE
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
            svtav1_entropy::context::write_partition(
                writer,
                frame_ctx,
                ctx,
                *partition_type as u8,
                nsymbs,
            );

            let half_w = w / 2;
            let half_h = h / 2;
            match (*partition_type, children.len()) {
                (crate::partition::PartitionType::Split, 4) => {
                    // PARTITION_SPLIT: 4 equal quarter-size children in Z-order.
                    // Don't update partition context here — children do it —
                    // EXCEPT the terminal 8x8 split (4x4 children write no
                    // partition bytes; the decoder sets the 8x8 cell to the
                    // SPLIT value, dav1d decode_sb BL_8X8).
                    if half_w == 4 {
                        ectx.update_partition_ctx(
                            block_x,
                            block_y,
                            w,
                            h,
                            crate::partition::PartitionType::Split,
                        );
                    }
                    encode_partition_tree(
                        &children[0],
                        writer,
                        frame_ctx,
                        coeff_fc,
                        base_q_idx,
                        ectx,
                        is_key,
                        block_x,
                        block_y,
                        chroma,
                        geom,
                    );
                    encode_partition_tree(
                        &children[1],
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
                    encode_partition_tree(
                        &children[2],
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
                    encode_partition_tree(
                        &children[3],
                        writer,
                        frame_ctx,
                        coeff_fc,
                        base_q_idx,
                        ectx,
                        is_key,
                        block_x + half_w,
                        block_y + half_h,
                        chroma,
                        geom,
                    );
                }
                (crate::partition::PartitionType::Horz, 2) => {
                    // PARTITION_HORZ: two children stacked vertically.
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
                    let bot = expect_leaf(&children[1]);
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
                (crate::partition::PartitionType::Vert, 2) => {
                    // PARTITION_VERT: two children side by side.
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
                    let right = expect_leaf(&children[1]);
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

fn encode_tile_rows(
    encode_input: &[u8],
    w: usize,
    h: usize,
    sb_size: usize,
    sb_cols: usize,
    sb_rows: usize,
    rows_per_tile: usize,
    tile_rows: usize,
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
) -> Vec<(
    Vec<u8>,
    Vec<crate::partition::BlockDecision>,
    Vec<crate::partition::PartitionTree>,
)> {
    let encode_one_tile = |tile_idx: usize| -> (
        Vec<u8>,
        Vec<crate::partition::BlockDecision>,
        Vec<crate::partition::PartitionTree>,
    ) {
        let tile_sb_row_start = tile_idx * rows_per_tile;
        let tile_sb_row_end = ((tile_idx + 1) * rows_per_tile).min(sb_rows);

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
        let cwid = w / 2;
        let chgt = h / 2;
        let mut fun_u_recon = alloc::vec![128u8; if use_funnel { cwid * chgt } else { 0 }];
        let mut fun_v_recon = alloc::vec![128u8; if use_funnel { cwid * chgt } else { 0 }];
        let mut fun_ectx = if use_funnel {
            Some(EntropyCtx::new(
                w / 4,
                h / 4,
                true,
                tile_sc.allow_screen_content_tools,
            ))
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
        let mut fun_frame = if use_funnel {
            let cq = c_quant.as_ref().unwrap();
            Some(crate::leaf_funnel::FunnelFrame {
                sharpness: hdr_sharpness,
                sharp_tx_active,
                noise_norm_strength: hdr_noise_norm,
                qm_levels,
                lambda: cq.lambda as u64,
                cli_qp: cli_qp as u32,
                rdoq_level: cq.rdoq_level,
                base_qindex,
                qindex_u,
                qindex_v,
                ac_bias_eff,
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
            Some(EntropyCtx::new(
                w / 4,
                h / 4,
                true,
                tile_sc.allow_screen_content_tools,
            ))
        } else {
            None
        };
        let mut sim_geom = crate::deblock::DeblockGeom::new(w, h);
        let mut sim_u = alloc::vec![128u8; if funnel_chain { cwid * chgt } else { 0 }];
        let mut sim_v = alloc::vec![128u8; if funnel_chain { cwid * chgt } else { 0 }];
        let mut sim_prev_sb_row = usize::MAX;
        let mut fun_rates = fun_rates;
        let mut tile_decisions: Vec<crate::partition::BlockDecision> = Vec::new();
        let mut tile_trees: Vec<crate::partition::PartitionTree> = Vec::new();
        let mut tile_frame_recon = alloc::vec![128u8; w * h];

        let mut part_config =
            crate::partition::PartitionSearchConfig::from_speed_config(speed_config);
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
            for sb_col in 0..sb_cols {
                let x0 = sb_col * sb_size;
                let y0 = sb_row * sb_size;
                let cur_w = sb_size.min(w - x0);
                let cur_h = sb_size.min(h - y0);

                // [SVT_HDR_MODE] variance boost: this SB searches/quantizes
                // at its PLANNED qindex (luma + per-plane chroma) with the
                // matching lambda (C per-SB svt_aom_lambda_assign). The
                // frame-level CDF bucket stays at the FH base (C behavior).
                if let (Some(plan), Some(f)) = (sb_qindex_plan, fun_frame.as_mut()) {
                    let sbq = plan[sb_row * sb_cols + sb_col];
                    f.base_qindex = sbq;
                    f.qindex_u = (i32::from(sbq) + i32::from(chroma_ac_deltas.0))
                        .clamp(0, 255) as u8;
                    f.qindex_v = (i32::from(sbq) + i32::from(chroma_ac_deltas.1))
                        .clamp(0, 255) as u8;
                    f.lambda = u64::from(crate::pd0::kf_full_lambda_8bit(
                        sbq,
                        u32::from(crate::rate_control::qindex_to_qp(sbq)),
                    ));
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
                // C-exact partition source gate (see the comment below);
                // computed here because the leaf lambda depends on it.
                let use_pd0 = ref_ctx.is_none()
                    && (speed_config.preset >= 6
                        || (matches!(speed_config.preset, 0..=5) && use_funnel))
                    && cur_w == sb_size
                    && cur_h == sb_size;
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
                let sb_index = sb_row * sb_cols + sb_col;
                let chain_base = if funnel_chain {
                    // C `ec_ctx_array[sb]` neighbor rule for the rate-estimation
                    // CDF (enc_dec_process.c:3002-3022). `pic_based_rate_est` is
                    // only ever false (enc_handle.c), so the weighted-average
                    // branch always runs. Availability predicates match C for a
                    // single-tile SB-aligned frame: left = not tile-left column,
                    // top-right = not tile-top row AND the SB one to the right
                    // exists (so the last column has no top-right).
                    let left_avail = sb_col > 0;
                    let topright_avail = sb_row > 0 && sb_col + 1 < sb_cols;
                    if left_avail && topright_avail {
                        // both -> copy left, then avg with top-right (3:1).
                        // C AVG_CDF_WEIGHT_LEFT / AVG_CDF_WEIGHT_TOP
                        // (enc_dec_process.c:2665-2666, :3016-3021).
                        const WT_LEFT: i32 = 3;
                        const WT_TOP: i32 = 1;
                        let mut base = chain_snaps[sb_index - 1].clone();
                        let tr = &chain_snaps[sb_index - sb_cols + 1];
                        base.0.avg_cdf_with(&tr.0, WT_LEFT, WT_TOP);
                        base.1.avg_cdf_with(tr.1.as_ref(), WT_LEFT, WT_TOP);
                        Some(base)
                    } else if left_avail {
                        // left only -> copy left (sb-1)
                        Some(chain_snaps[sb_index - 1].clone())
                    } else if topright_avail {
                        // top-right only -> copy top-right (sb - sb_cols + 1)
                        Some(chain_snaps[sb_index - sb_cols + 1].clone())
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
                let sb_result = if use_pd0 {
                    if speed_config.preset >= 9 {
                        let tree = crate::pd0::pd0_pick_sb_partition(
                            encode_input,
                            w,
                            x0,
                            y0,
                            cli_qp as u32,
                            sb_qindex,
                            // C `input_resolution_factor[input_resolution]`:
                            // per-picture coeff-rate addend keyed on w*h.
                            crate::pd0::input_resolution_factor(w * h),
                        );
                        // The same per-SB variance map C's picture analysis
                        // feeds to is_dc_only_safe (pcs->ppcs->variance): the
                        // fixed-tree leaves use it to force the C-exact
                        // DC-only intra candidate set where the gate fires.
                        let sb_vars = crate::pd0::compute_b64_variance(encode_input, w, x0, y0);
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
                            })
                        } else {
                            None
                        };
                        crate::partition::encode_fixed_tree(
                            &encode_input[y0 * w + x0..],
                            w,
                            &mut tile_frame_recon,
                            w,
                            &tree,
                            sb_size,
                            sb_qindex,
                            &part_config,
                            x0,
                            y0,
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
                        let refined = matches!(speed_config.preset, 0..=5) && use_funnel;
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
                            );
                            let cq = c_quant.as_ref().unwrap();
                            let scan = crate::depth_refine::build_refined_scan_at(
                                &eval,
                                &dr,
                                cq.lambda as u64,
                                tables,
                                x0,
                                y0,
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
                                cq.lambda as u64,
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
                                encode_input,
                                w,
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
                            let sb_vars = crate::pd0::compute_b64_variance(encode_input, w, x0, y0);
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
                                })
                            } else {
                                None
                            };
                            crate::partition::encode_fixed_tree(
                                &encode_input[y0 * w + x0..],
                                w,
                                &mut tile_frame_recon,
                                w,
                                &tree,
                                sb_size,
                                sb_qindex,
                                &part_config,
                                x0,
                                y0,
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
                            x0,
                            y0,
                            &mut sim_chroma,
                            &mut sim_geom,
                        );
                    }
                    chain_snaps.push((fc, cfc));
                    debug_assert_eq!(chain_snaps.len(), sb_index + 1);
                }

                // Keep the per-SB recon list layout for downstream consumers.
                let mut sb_recon = alloc::vec![0u8; cur_w * cur_h];
                for r in 0..cur_h {
                    let src_off = (y0 + r) * w + x0;
                    sb_recon[r * cur_w..(r + 1) * cur_w]
                        .copy_from_slice(&tile_frame_recon[src_off..src_off + cur_w]);
                }
                tile_recon.extend_from_slice(&sb_recon);
                tile_decisions.extend(sb_result.decisions);
                if let Some(tree) = sb_result.tree {
                    tile_trees.push(tree);
                }
            }
        }
        (tile_recon, tile_decisions, tile_trees)
    };

    // Parallel encoding with std::thread::scope when available
    #[cfg(feature = "std")]
    if tile_rows > 1 {
        return std::thread::scope(|s| {
            let handles: Vec<_> = (0..tile_rows)
                .map(|tile_idx| s.spawn(move || encode_one_tile(tile_idx)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
    }

    // Sequential fallback
    (0..tile_rows).map(encode_one_tile).collect()
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

    #[test]
    fn pipeline_encode_sequence() {
        let mut pipeline = EncodePipeline::new(
            32,
            32,
            10,
            RcConfig {
                mode: RcMode::Crf,
                qp: 28,
                ..RcConfig::default()
            },
            3,
            16,
        );
        let y_plane = vec![100u8; 32 * 32];
        for i in 0..5 {
            let bitstream = pipeline.encode_frame(&y_plane, 32);
            assert!(!bitstream.is_empty(), "frame {i} should produce output");
        }
        assert_eq!(pipeline.frame_count, 5);
        assert_eq!(pipeline.rc_state.total_frames, 5);
    }

    #[test]
    fn pipeline_key_frame_first() {
        let mut pipeline = EncodePipeline::new(16, 16, 8, RcConfig::default(), 4, 64);
        let y_plane = vec![128u8; 16 * 16];
        let bitstream = pipeline.encode_frame(&y_plane, 16);
        // First frame should be key frame with sequence header
        // OBU structure: TD + SH + Frame
        assert!(bitstream.len() > 10);
    }

    #[test]
    fn pipeline_dpb_updated() {
        let mut pipeline = EncodePipeline::new(16, 16, 8, RcConfig::default(), 4, 64);
        let y_plane = vec![128u8; 16 * 16];
        pipeline.encode_frame(&y_plane, 16);
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
}

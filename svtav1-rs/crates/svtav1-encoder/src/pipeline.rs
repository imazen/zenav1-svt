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
            speed_config: SpeedConfig::from_preset(preset),
            rc_config,
            rc_state: RcState::default(),
            dpb: DecodedPictureBuffer::new(),
            gop: GopStructure::new(hierarchical_levels, intra_period),
            frame_count: 0,
            width,
            height,
            bit_depth: 8,
            color_description: svtav1_entropy::obu::ColorDescription::srgb(),
            chroma_420: false,
            last_recon: None,
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
        assert!(u.len() >= cn && v.len() >= cn, "u/v planes must be (w/2 x h/2)");
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

        // Step 3c: Compute VAQ activity map for adaptive QP
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
        let tpl_adjusted_qp = if !is_key && self.dpb.occupied_slots() > 0 {
            if let Some(rf) = self.dpb.get(0) {
                let tpl_delta =
                    crate::rate_control::tpl_qp_adjustment(&encode_input, &rf.y_plane, w, h, w);
                (vaq_adjusted_qp as i16 + tpl_delta as i16).clamp(0, 63) as u8
            } else {
                vaq_adjusted_qp
            }
        } else {
            vaq_adjusted_qp
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

        let tile_recons = encode_tile_rows(
            &encode_input,
            w,
            h,
            sb_size,
            sb_cols,
            sb_rows,
            rows_per_tile,
            tile_rows,
            tpl_adjusted_qp,
            lambda,
            &self.speed_config,
            ref_frame_data.as_deref(),
            &mv_map,
            mv_map_stride,
            &sb_qp_offsets,
            chroma.is_some(),
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

        // Step 5: Loop filters — DISABLED until the C filter-search ports
        // land. The frame header currently signals loop_filter_level=0,
        // enable_cdef=0 (SH) and no restoration, so the decoder applies NO
        // filtering; applying them here would make the encoder's DPB recon
        // diverge from what any conforming decoder reconstructs (pixel
        // integrity rule: two paths producing different pixels is a bug).
        // The filter implementations below are kept for the faithful
        // deblock/CDEF/restoration ports, which must signal parameters and
        // filter identically to C.
        const APPLY_UNSIGNALED_FILTERS: bool = false;
        if APPLY_UNSIGNALED_FILTERS {
            // Step 5: Apply loop filters to reconstruction
            // 5a: Deblocking filter on block edges
            // Filter width (4/8/14-tap) and strength derived from QP and edge type.
            // (Spec 08, Section 7.14: filter size and strength per-edge)
            {
                let (strength, threshold) = svtav1_dsp::loop_filter::derive_deblock_strength(pcs.qp);
                // Apply deblocking on vertical edges (every 8 columns)
                for bx in 1..(w / 8) {
                    let edge_col = bx * 8;
                    let is_sb_edge = edge_col % sb_size == 0;
                    let filter_size =
                        svtav1_dsp::loop_filter::select_deblock_filter_size(is_sb_edge, pcs.qp);
                    match filter_size {
                        14 if edge_col >= 7 && edge_col + 7 <= w => {
                            svtav1_dsp::loop_filter::deblock_vert_14tap(
                                &mut recon, w, strength, threshold, edge_col, h,
                            );
                        }
                        8 if edge_col >= 4 && edge_col + 4 <= w => {
                            svtav1_dsp::loop_filter::deblock_vert_wide(
                                &mut recon, w, strength, threshold, edge_col, h,
                            );
                        }
                        _ => {
                            svtav1_dsp::loop_filter::deblock_vert(
                                &mut recon, w, strength, threshold, edge_col, h,
                            );
                        }
                    }
                }
                // Apply deblocking on horizontal edges (every 8 rows)
                for by in 1..(h / 8) {
                    let edge_row = by * 8;
                    let is_sb_edge = edge_row % sb_size == 0;
                    let filter_size =
                        svtav1_dsp::loop_filter::select_deblock_filter_size(is_sb_edge, pcs.qp);
                    match filter_size {
                        8 if edge_row >= 4 && edge_row + 4 <= h => {
                            svtav1_dsp::loop_filter::deblock_horz_wide(
                                &mut recon, w, strength, threshold, edge_row, w,
                            );
                        }
                        _ => {
                            svtav1_dsp::loop_filter::deblock_horz(
                                &mut recon, w, strength, threshold, edge_row, w,
                            );
                        }
                    }
                }
            }

            // 5b: CDEF
            if self.speed_config.enable_cdef {
                // Apply CDEF to each 8x8 block
                let mut filtered = recon.clone();
                let bw = 8usize;
                let blocks_x = w.div_ceil(bw);
                let blocks_y = h.div_ceil(bw);
                for by in 0..blocks_y {
                    for bx in 0..blocks_x {
                        let x0 = bx * bw;
                        let y0 = by * bw;
                        let cur_w = bw.min(w - x0);
                        let cur_h = bw.min(h - y0);
                        if cur_w == 8 && cur_h == 8 {
                            let (dir, _var) =
                                svtav1_dsp::loop_filter::cdef_find_dir(&recon[y0 * w + x0..], w);
                            // Light CDEF: pri_strength based on QP
                            let pri = (pcs.qp / 8).min(15);
                            let sec = (pcs.qp / 16).min(3);
                            svtav1_dsp::loop_filter::cdef_filter_block(
                                &recon[y0 * w + x0..],
                                w,
                                &mut filtered[y0 * w + x0..],
                                w,
                                dir,
                                pri as i32,
                                sec as i32,
                                3 + (pcs.qp / 16) as i32,
                                cur_w,
                                cur_h,
                            );
                        }
                    }
                }
                recon = filtered;
            }

            // 5c: Wiener restoration (if enabled)
            // Optimizes coefficients per-frame by searching for the set that
            // minimizes SSE between filtered reconstruction and source.
            if self.speed_config.enable_restoration {
                let mut restored = recon.clone();
                let (h_coeffs, v_coeffs) = svtav1_dsp::loop_filter::optimize_wiener_coefficients(
                    &encode_input,
                    w,
                    &recon,
                    w,
                    w,
                    h,
                );
                svtav1_dsp::loop_filter::wiener_filter(
                    &recon,
                    w,
                    &mut restored,
                    w,
                    w,
                    h,
                    h_coeffs,
                    v_coeffs,
                );
                recon = restored;
            }

            // 5d: Self-guided restoration (sgrproj) — applies variance-adaptive
            // denoising that preserves edges. (Spec 08, Section 7.17)
            // Only enabled at low presets where quality matters more than speed.
            if self.speed_config.enable_restoration && self.speed_config.preset <= 6 {
                let mut sgrproj_out = recon.clone();
                let params = svtav1_dsp::loop_filter::SgrprojParams {
                    r0: 2,
                    r1: 1,
                    s0: (10 + pcs.qp as i32 / 2).min(100),
                    s1: (5 + pcs.qp as i32 / 4).min(50),
                    xqd: [32, 32], // Equal blend of both passes with source
                };
                svtav1_dsp::loop_filter::sgrproj_filter(&recon, w, &mut sgrproj_out, w, w, h, &params);
                recon = sgrproj_out;
            }

        }

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
        let (mut u_recon, mut v_recon) = if chroma.is_some() {
            (alloc::vec![128u8; cw * chh], alloc::vec![128u8; cw * chh])
        } else {
            (Vec::new(), Vec::new())
        };
        let tile_data = {
            let mut writer = svtav1_entropy::writer::AomWriter::new(n + 256);
            // CDF updates enabled — matches the frame header's disable_cdf_update=0
            let mut frame_ctx = svtav1_entropy::context::FrameContext::new_default();
            // C-exact coefficient CDFs for the base_q_idx bucket
            // (svt_av1_default_coef_probs semantics).
            let mut coeff_fc =
                svtav1_entropy::coeff_c::CoeffFc::default_for_qindex(tpl_adjusted_qp);
            // Mode/skip context tracking at 4x4 granularity
            let w4 = w.div_ceil(4);
            let h4 = h.div_ceil(4);
            let mut ectx = EntropyCtx::new(w4, h4);
            let mut chroma_pass = chroma.map(|(u_src, v_src)| ChromaPass {
                u_src,
                v_src,
                u_recon: &mut u_recon,
                v_recon: &mut v_recon,
                stride: cw,
                qp: tpl_adjusted_qp,
            });

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

                encode_partition_tree(
                    tree,
                    &mut writer,
                    &mut frame_ctx,
                    &mut coeff_fc,
                    tpl_adjusted_qp,
                    &mut ectx,
                    is_key,
                    bx,
                    by,
                    &mut chroma_pass,
                );
            }

            svtav1_entropy::obu::build_tile_group_single(writer.done())
        };

        // Step 6b: Film grain estimation (compare source to reconstruction)
        let _grain_params = crate::film_grain::estimate_film_grain(&encode_input, &recon, w, h, w);
        // grain_params would be signaled in the frame header OBU
        // and used by the decoder to re-synthesize grain

        // Step 7: Build OBU bitstream
        // Use full (non-reduced) sequence header for multi-frame sequences,
        // still-picture header only for single-frame mode.
        let is_single_frame = self.gop.intra_period <= 1;
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
            ));
            // Key frame header (raw bytes) + tile group with proper header
            // Use tpl_adjusted_qp (same value used for CDF category selection)
            // so the decoder's CDF initialization matches the encoder's.
            let fh_bytes = svtav1_entropy::obu::write_key_frame_header_full(
                self.width,
                self.height,
                tpl_adjusted_qp,
                is_single_frame,
                chroma.is_none(),
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
            // Inter frame: proper frame header with type, QP, refresh flags, ref indices
            svtav1_entropy::obu::write_inter_frame(
                tpl_adjusted_qp,
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
struct EntropyCtx {
    /// Above row modes (at 4x4 granularity), indexed by column in 4x4 units.
    /// Updated after each block is encoded.
    above_mode: Vec<u8>,
    /// Left column modes (at 4x4 granularity), indexed by row in 4x4 units.
    left_mode: Vec<u8>,
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
    /// qindex for chroma quantization — same tables as luma, since the
    /// frame header signals DeltaQUDc = DeltaQUAc = 0.
    qp: u8,
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
    fn new(width_4x4: usize, height_4x4: usize) -> Self {
        let width_8x8 = (width_4x4 + 1) / 2;
        let height_8x8 = (height_4x4 + 1) / 2;
        // Chroma-plane 4x4 units: (w/2)/4 = width_4x4/2 (frames are
        // 64-aligned so this divides exactly; div_ceil for safety).
        let width_c4 = width_4x4.div_ceil(2);
        let height_c4 = height_4x4.div_ceil(2);
        Self {
            above_mode: alloc::vec![0u8; width_4x4], // DC_PRED = 0
            left_mode: alloc::vec![0u8; height_4x4],
            above_skip: alloc::vec![false; width_4x4],
            left_skip: alloc::vec![false; height_4x4],
            above_partition: alloc::vec![0u8; width_8x8],
            left_partition: alloc::vec![0u8; height_8x8],
            // 0xFF = INVALID_NEIGHBOR_DATA at frame edges, like C's
            // neighbor-array init.
            above_coeff: alloc::vec![0xFFu8; width_4x4],
            left_coeff: alloc::vec![0xFFu8; height_4x4],
            above_coeff_uv: [
                alloc::vec![0xFFu8; width_c4],
                alloc::vec![0xFFu8; width_c4],
            ],
            left_coeff_uv: [
                alloc::vec![0xFFu8; height_c4],
                alloc::vec![0xFFu8; height_c4],
            ],
        }
    }

    /// Coefficient neighbor spans for a transform at (x, y) of w x h pixels,
    /// in 4x4 units, clipped to the frame like C svt_aom_get_txb_ctx.
    fn coeff_neighbors(&self, x: usize, y: usize, w: usize, h: usize) -> (&[u8], &[u8]) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = (w / 4).min(self.above_coeff.len().saturating_sub(x4));
        let h4 = (h / 4).min(self.left_coeff.len().saturating_sub(y4));
        (&self.above_coeff[x4..x4 + w4], &self.left_coeff[y4..y4 + h4])
    }

    /// Record a coded transform block's `(dc_sign << 6) | cul_level` byte
    /// over its 4x4 span (C: neighbor array unit write after
    /// av1_write_coeffs_txb_1d).
    fn record_coeff(&mut self, x: usize, y: usize, w: usize, h: usize, val: u8) {
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
    fn coeff_neighbors_uv(&self, uv: usize, cx: usize, cy: usize, cw: usize, ch: usize) -> (&[u8], &[u8]) {
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
    fn record_coeff_uv(&mut self, uv: usize, cx: usize, cy: usize, cw: usize, ch: usize, val: u8) {
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
    fn reset_left_for_sb_row(&mut self) {
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
    fn partition_ctx(&self, x: usize, y: usize, width: usize) -> (usize, usize) {
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
    fn update_partition_ctx(
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
    fn record_block(&mut self, x: usize, y: usize, w: usize, h: usize, mode: u8, skip: bool) {
        let x4 = x / 4;
        let y4 = y / 4;
        let w4 = w / 4;
        let h4 = h / 4;
        // Fill above row with this block's mode
        for i in x4..(x4 + w4).min(self.above_mode.len()) {
            self.above_mode[i] = mode;
            self.above_skip[i] = skip;
        }
        // Fill left column with this block's mode
        for i in y4..(y4 + h4).min(self.left_mode.len()) {
            self.left_mode[i] = mode;
            self.left_skip[i] = skip;
        }
    }

    /// Get the above mode context at position (x, y) in pixel coordinates.
    fn above_mode_ctx(&self, x: usize) -> usize {
        let x4 = x / 4;
        let mode = if x4 < self.above_mode.len() {
            self.above_mode[x4]
        } else {
            0
        };
        svtav1_entropy::context::intra_mode_context(mode)
    }

    /// Get the left mode context at position (x, y) in pixel coordinates.
    fn left_mode_ctx(&self, y: usize) -> usize {
        let y4 = y / 4;
        let mode = if y4 < self.left_mode.len() {
            self.left_mode[y4]
        } else {
            0
        };
        svtav1_entropy::context::intra_mode_context(mode)
    }

    /// Get the skip context at position (x, y).
    fn skip_ctx(&self, x: usize, y: usize) -> usize {
        let x4 = x / 4;
        let y4 = y / 4;
        let above = x4 < self.above_skip.len() && self.above_skip[x4];
        let left = y4 < self.left_skip.len() && self.left_skip[y4];
        svtav1_entropy::context::get_skip_context(above, left)
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
    // eob relative to the DCT_DCT (default) scan for this tx size.
    let scan = svtav1_entropy::scan_tables::scan(
        tx_size,
        svtav1_entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[coeff_c::DCT_DCT] as usize,
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
        coeff_c::DCT_DCT,
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
) {
    // 4:2:0: encode this block's chroma pair FIRST (prediction reads the
    // live chroma recon written by previous blocks in coding order). The
    // min-8x8 luma policy guarantees the chroma block is exactly
    // (w/2, h/2) >= 4x4 and every block is a chroma reference.
    let chroma_blocks = chroma.as_mut().map(|cp| {
        let cw = decision.width as usize / 2;
        let ch = decision.height as usize / 2;
        let cx = block_x / 2;
        let cy = block_y / 2;
        let (u_q, u_eob) = crate::partition::encode_chroma_block_dc(
            cp.u_src, cp.u_recon, cp.stride, cx, cy, cw, ch, cp.qp,
        );
        let (v_q, v_eob) = crate::partition::encode_chroma_block_dc(
            cp.v_src, cp.v_recon, cp.stride, cx, cy, cw, ch, cp.qp,
        );
        (u_q, u_eob, v_q, v_eob)
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
            svtav1_entropy::context::write_angle_delta(writer, frame_ctx, decision.intra_mode, 0);
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
            svtav1_entropy::context::write_angle_delta(writer, frame_ctx, decision.intra_mode, 0);
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
            0, // UV_DC_PRED
        );
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
        let tx_size = coeff_c::tx_size_from_dims(w, h);
        let (above, left) = ectx.coeff_neighbors(block_x, block_y, w, h);
        let (txb_skip_ctx, dc_sign_ctx) = coeff_c::get_txb_ctx(0, above, left, true, false);
        // 64-dim transforms keep only the 32-capped low-frequency quadrant;
        // the C writer expects that quadrant packed at the adjusted stride.
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
        // The decision's eob was derived from the mode-decision scan; the
        // bitstream eob must be relative to the C scan order for this
        // (tx_size, tx_type).
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
            decision.intra_mode as usize,
            base_q_idx,
            false,
        );
        ectx.record_coeff(block_x, block_y, w, h, cul_level as u8);

        // Chroma txbs: plane 1 (U) then plane 2 (V), each one full-size
        // (w/2 x h/2) transform with its own neighbor context state.
        if let Some((u_q, _u_eob, v_q, _v_eob)) = chroma_blocks.as_ref() {
            let cw = w / 2;
            let ch = h / 2;
            let cx = block_x / 2;
            let cy = block_y / 2;
            write_chroma_txb(writer, coeff_fc, ectx, 0, cx, cy, cw, ch, u_q, base_q_idx);
            write_chroma_txb(writer, coeff_fc, ectx, 1, cx, cy, cw, ch, v_q, base_q_idx);
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
            let cw = decision.width as usize / 2;
            let ch = decision.height as usize / 2;
            let cx = block_x / 2;
            let cy = block_y / 2;
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
        skip,
    );
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
                chroma,
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
                    // Don't update partition context here — children do it.
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
                        top, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x, block_y,
                        chroma,
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
                        left, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x, block_y,
                        chroma,
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
                        (crate::partition::PartitionType::Horz4, 4) => {
                            &[(0, 0), (0, quarter_h), (0, 2 * quarter_h), (0, 3 * quarter_h)]
                        }
                        (crate::partition::PartitionType::Vert4, 4) => {
                            &[(0, 0), (quarter_w, 0), (2 * quarter_w, 0), (3 * quarter_w, 0)]
                        }
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
                        );
                    }
                }
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
    qp: u8,
    _lambda: u64, // Per-SB lambda computed from sb_qp_offsets
    speed_config: &crate::speed_config::SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    mv_map: &[svtav1_types::motion::Mv],
    mv_map_stride: usize,
    sb_qp_offsets: &[i8],
    chroma_420: bool,
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

        for sb_row in tile_sb_row_start..tile_sb_row_end {
            for sb_col in 0..sb_cols {
                let x0 = sb_col * sb_size;
                let y0 = sb_row * sb_size;
                let cur_w = sb_size.min(w - x0);
                let cur_h = sb_size.min(h - y0);

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
                let _ = (sb_row, sb_col, &sb_qp_offsets);
                let sb_qp_delta = 0i8;
                let sb_qp = (qp as i16 + sb_qp_delta as i16).clamp(0, 63) as u8;
                let sb_lambda =
                    (crate::rate_control::qp_to_lambda(sb_qp) * speed_config.lambda_scale()) as u64;

                // The search reads intra neighbors from — and reconstructs
                // directly into — the live frame buffer, exactly like the
                // decoder (fixes within-SB predictions that previously fell
                // back to 128).
                let sb_result = crate::partition::partition_search_with_config(
                    &encode_input[y0 * w + x0..],
                    w,
                    &mut tile_frame_recon,
                    w,
                    cur_w,
                    cur_h,
                    sb_qp,
                    sb_lambda,
                    speed_config.max_partition_depth as u32,
                    &part_config,
                    x0,
                    y0,
                    ref_ctx.as_ref(),
                );

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

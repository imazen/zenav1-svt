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
        }
    }

    /// Encode a single frame through the full pipeline.
    ///
    /// Returns the encoded bitstream data and updates internal state.
    pub fn encode_frame(&mut self, y_plane: &[u8], y_stride: usize) -> Vec<u8> {
        let display_order = self.frame_count;

        // Step 1: Determine frame type from GOP structure
        let is_key = self.gop.is_key_frame(display_order);
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
                let tpl_delta = crate::rate_control::tpl_qp_adjustment(
                    &encode_input,
                    &rf.y_plane,
                    w,
                    h,
                    w,
                );
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
        let sb_size = if self.speed_config.max_partition_depth >= 3 {
            64
        } else {
            32
        };
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

        // Tile-parallel encoding: split frame into tile rows, encode each in parallel.
        // Each tile row gets its own recon buffer; results are merged afterward.
        // Loop filters run on the full frame after all tiles complete.
        let tile_rows = if h >= 128 && cfg!(feature = "std") { 2 } else { 1 };
        let rows_per_tile = sb_rows.div_ceil(tile_rows);

        // Encode tile rows (parallel when std is available, sequential otherwise)
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
        );

        // Collect all block decisions from all tiles
        let mut all_decisions: Vec<crate::partition::BlockDecision> = Vec::new();

        // Merge tile recons into frame buffer and update MV map
        for (tile_idx, (tile_recon, tile_decisions)) in tile_recons.iter().enumerate() {
            all_decisions.extend_from_slice(tile_decisions);
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
                            w, rf, w,
                            x0 as i32, y0 as i32,
                            cur_w.min(16), cur_h.min(16),
                            svtav1_types::motion::Mv::ZERO, 8, 8, w, h,
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

        // Step 5: Apply loop filters to reconstruction
        // 5a: Deblocking filter on block edges
        // Filter width (4/8/14-tap) and strength derived from QP and edge type.
        // (Spec 08, Section 7.14: filter size and strength per-edge)
        {
            let (strength, threshold) =
                svtav1_dsp::loop_filter::derive_deblock_strength(pcs.qp);
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

        // Step 6: Entropy coding — encode per-block decisions from partition search
        // Uses the BlockDecision records collected during tile encoding instead of
        // re-encoding blocks from scratch. Encodes skip, mode, and coefficients
        // per block with CDF-based adaptive context.
        let mut writer = svtav1_entropy::writer::AomWriter::new(n);
        let mut coeff_ctx = svtav1_entropy::coeff::CoeffContext::default();
        let mut frame_ctx = svtav1_entropy::context::FrameContext::new_default();

        // Mode info grid for context derivation (8x8 block resolution)
        let mi_cols = w.div_ceil(8);
        let mi_rows = h.div_ceil(8);
        let mut mi_skip = alloc::vec![true; mi_cols * mi_rows]; // skip flags per block
        let mut mi_intra = alloc::vec![true; mi_cols * mi_rows]; // is_intra per block
        let mut mi_mode = alloc::vec![0u8; mi_cols * mi_rows]; // intra mode per block
        let mut mi_x = 0usize;
        let mut mi_y = 0usize;

        for decision in &all_decisions {
            let bw = (decision.width as usize).max(8) / 8;
            let bh = (decision.height as usize).max(8) / 8;

            // Derive context from above and left neighbors in the mode info grid
            let above_skip = if mi_y > 0 { mi_skip[(mi_y - 1) * mi_cols + mi_x] } else { true };
            let left_skip = if mi_x > 0 { mi_skip[mi_y * mi_cols + mi_x - 1] } else { true };
            let skip_ctx = svtav1_entropy::context::get_skip_context(above_skip, left_skip);

            let skip = decision.eob == 0;
            svtav1_entropy::context::write_skip(&mut writer, &mut frame_ctx, skip_ctx, skip);

            if !skip {
                if !is_key {
                    let above_intra = if mi_y > 0 {
                        mi_intra[(mi_y - 1) * mi_cols + mi_x]
                    } else {
                        true
                    };
                    let left_intra = if mi_x > 0 {
                        mi_intra[mi_y * mi_cols + mi_x - 1]
                    } else {
                        true
                    };
                    let ii_ctx =
                        svtav1_entropy::context::get_intra_inter_context(above_intra, left_intra);
                    svtav1_entropy::context::write_intra_inter(
                        &mut writer,
                        &mut frame_ctx,
                        ii_ctx,
                        decision.is_inter,
                    );
                }

                if !decision.is_inter {
                    if is_key {
                        let above_mode_ctx = if mi_y > 0 {
                            svtav1_entropy::context::intra_mode_context(
                                mi_mode[(mi_y - 1) * mi_cols + mi_x],
                            )
                        } else {
                            0
                        };
                        let left_mode_ctx = if mi_x > 0 {
                            svtav1_entropy::context::intra_mode_context(
                                mi_mode[mi_y * mi_cols + mi_x - 1],
                            )
                        } else {
                            0
                        };
                        svtav1_entropy::context::write_intra_mode_kf(
                            &mut writer,
                            &mut frame_ctx,
                            above_mode_ctx,
                            left_mode_ctx,
                            decision.intra_mode,
                        );
                    } else {
                        let bsize_group = svtav1_entropy::context::block_size_group(
                            decision.width as usize,
                            decision.height as usize,
                        );
                        svtav1_entropy::context::write_intra_mode_inter(
                            &mut writer,
                            &mut frame_ctx,
                            bsize_group,
                            decision.intra_mode,
                        );
                    }
                }

                svtav1_entropy::coeff::write_coefficients_ctx(
                    &mut writer,
                    &decision.qcoeffs,
                    decision.eob as usize,
                    &mut coeff_ctx,
                );
            }

            // Update mode info grid for context of subsequent blocks
            for dy in 0..bh.min(mi_rows - mi_y) {
                for dx in 0..bw.min(mi_cols - mi_x) {
                    let idx = (mi_y + dy) * mi_cols + (mi_x + dx);
                    if idx < mi_skip.len() {
                        mi_skip[idx] = skip;
                        mi_intra[idx] = !decision.is_inter;
                        mi_mode[idx] = decision.intra_mode;
                    }
                }
            }

            // Advance position in raster order
            mi_x += bw;
            if mi_x >= mi_cols {
                mi_x = 0;
                mi_y += bh;
            }
        }
        let tile_data = writer.done().to_vec();

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
            if is_single_frame {
                bs.extend_from_slice(&svtav1_entropy::obu::write_sequence_header(
                    self.width,
                    self.height,
                ));
            } else {
                bs.extend_from_slice(&svtav1_entropy::obu::write_sequence_header_full(
                    self.width,
                    self.height,
                ));
            }
            // Key frame header (raw bytes) + tile group with proper header
            let fh_bytes = svtav1_entropy::obu::write_key_frame_header_full(
                self.width,
                self.height,
                pcs.qp,
                is_single_frame,
            );
            let tg_bytes = svtav1_entropy::obu::build_tile_group_single(&tile_data);
            let mut frame_payload = alloc::vec::Vec::new();
            frame_payload.extend_from_slice(&fh_bytes);
            frame_payload.extend_from_slice(&tg_bytes);
            bs.extend_from_slice(&svtav1_entropy::obu::write_obu(
                svtav1_entropy::obu::ObuType::Frame,
                &frame_payload,
            ));
            bs
        } else {
            // Inter frame: proper frame header with type, QP, refresh flags, ref indices
            svtav1_entropy::obu::write_inter_frame(
                pcs.qp,
                pcs.refresh_frame_flags,
                display_order as u8,
                &tile_data,
            )
        };

        // Step 7: Update DPB with reconstructed frame
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
    lambda: u64,
    speed_config: &crate::speed_config::SpeedConfig,
    ref_frame_data: Option<&[u8]>,
    mv_map: &[svtav1_types::motion::Mv],
    mv_map_stride: usize,
) -> Vec<(Vec<u8>, Vec<crate::partition::BlockDecision>)> {
    let encode_one_tile = |tile_idx: usize| -> (Vec<u8>, Vec<crate::partition::BlockDecision>) {
        let tile_sb_row_start = tile_idx * rows_per_tile;
        let tile_sb_row_end = ((tile_idx + 1) * rows_per_tile).min(sb_rows);

        let mut tile_recon = Vec::new();
        let mut tile_decisions: Vec<crate::partition::BlockDecision> = Vec::new();
        let mut tile_frame_recon = alloc::vec![128u8; w * h];

        let part_config = crate::partition::PartitionSearchConfig::from_speed_config(speed_config);

        for sb_row in tile_sb_row_start..tile_sb_row_end {
            for sb_col in 0..sb_cols {
                let x0 = sb_col * sb_size;
                let y0 = sb_row * sb_size;
                let cur_w = sb_size.min(w - x0);
                let cur_h = sb_size.min(h - y0);

                let mut sb_recon = alloc::vec![0u8; cur_w * cur_h];

                let frame_ctx = crate::partition::FrameReconCtx {
                    buf: &tile_frame_recon,
                    stride: w,
                    sb_x: x0,
                    sb_y: y0,
                };
                let ref_ctx = ref_frame_data.map(|rf| crate::partition::RefFrameCtx {
                    y_plane: rf,
                    stride: w,
                    pic_width: w,
                    pic_height: h,
                    mv_map: Some(mv_map),
                    mv_map_stride,
                });
                let sb_result = crate::partition::partition_search_with_config(
                    &encode_input[y0 * w + x0..],
                    w,
                    &mut sb_recon,
                    cur_w,
                    cur_w,
                    cur_h,
                    qp,
                    lambda,
                    speed_config.max_partition_depth as u32,
                    &part_config,
                    Some(&frame_ctx),
                    x0,
                    y0,
                    ref_ctx.as_ref(),
                );

                // Update tile-local frame recon for neighbor context
                for r in 0..cur_h {
                    for c in 0..cur_w {
                        tile_frame_recon[(y0 + r) * w + x0 + c] = sb_recon[r * cur_w + c];
                    }
                }

                tile_recon.extend_from_slice(&sb_recon);
                tile_decisions.extend(sb_result.decisions);
            }
        }
        (tile_recon, tile_decisions)
    };

    // Parallel encoding with std::thread::scope when available
    #[cfg(feature = "std")]
    if tile_rows > 1 {
        return std::thread::scope(|s| {
            let handles: Vec<_> = (0..tile_rows)
                .map(|tile_idx| s.spawn(move || encode_one_tile(tile_idx)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
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
}

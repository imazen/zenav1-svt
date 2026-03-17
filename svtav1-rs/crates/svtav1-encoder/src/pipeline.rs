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
        // Low-activity frames get lower QP (more bits for smooth content)
        let vaq_adjusted_qp = if activity_map.frame_avg > 0.0 {
            let frame_activity_factor = (activity_map.frame_avg / 10.0).log2().clamp(-2.0, 2.0);
            (pcs.qp as f64 + frame_activity_factor).clamp(0.0, 63.0) as u8
        } else {
            pcs.qp
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
        let lambda = (crate::rate_control::qp_to_lambda(vaq_adjusted_qp)
            * self.speed_config.lambda_scale()) as u64;

        let sb_cols = w.div_ceil(sb_size);
        let sb_rows = h.div_ceil(sb_size);

        for sb_row in 0..sb_rows {
            for sb_col in 0..sb_cols {
                let x0 = sb_col * sb_size;
                let y0 = sb_row * sb_size;
                let cur_w = sb_size.min(w - x0);
                let cur_h = sb_size.min(h - y0);

                let mut sb_recon = alloc::vec![0u8; cur_w * cur_h];

                let part_config =
                    crate::partition::PartitionSearchConfig::from_speed_config(&self.speed_config);
                let frame_ctx = crate::partition::FrameReconCtx {
                    buf: &recon,
                    stride: w,
                    sb_x: x0,
                    sb_y: y0,
                };
                let _sb_result = crate::partition::partition_search_with_config(
                    &encode_input[y0 * w + x0..],
                    w,
                    &mut sb_recon,
                    cur_w,
                    cur_w,
                    cur_h,
                    vaq_adjusted_qp,
                    lambda,
                    self.speed_config.max_partition_depth as u32,
                    &part_config,
                    Some(&frame_ctx),
                    x0,
                    y0,
                );

                // Write SB recon to frame buffer
                for r in 0..cur_h {
                    for c in 0..cur_w {
                        recon[(y0 + r) * w + x0 + c] = sb_recon[r * cur_w + c];
                    }
                }
            }
        }

        // Step 5: Apply loop filters to reconstruction
        // 5a: Deblocking filter on block edges
        {
            let strength = (pcs.qp as i32 * 2).min(63);
            let threshold = 4 + pcs.qp as i32 / 4;
            // Apply deblocking on vertical edges (every 8 columns)
            for bx in 1..(w / 8) {
                let edge_col = bx * 8;
                svtav1_dsp::loop_filter::deblock_vert(
                    &mut recon, w, strength, threshold, edge_col, h,
                );
            }
            // Apply deblocking on horizontal edges (every 8 rows)
            for by in 1..(h / 8) {
                let edge_row = by * 8;
                svtav1_dsp::loop_filter::deblock_horz(
                    &mut recon, w, strength, threshold, edge_row, w,
                );
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
        if self.speed_config.enable_restoration {
            let mut restored = recon.clone();
            // Apply mild Wiener filter — coefficients tuned for the QP level
            let strength = (pcs.qp as i16 / 10).max(1);
            let h_coeffs = [strength, strength * 2, strength * 3];
            let v_coeffs = [strength, strength * 2, strength * 3];
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

        // Step 6: Entropy coding — write bitstream using real coefficient coding
        let mut writer = svtav1_entropy::writer::AomWriter::new(n);
        // Encode each block's coefficients
        let bw = 8usize;
        let blocks_x = w.div_ceil(bw);
        let blocks_y = h.div_ceil(bw);
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let x0 = bx * bw;
                let y0 = by * bw;
                let cur_w = bw.min(w - x0);
                let cur_h = bw.min(h - y0);

                // Re-encode to get coefficients for entropy coding
                let mut src_block = alloc::vec![0u8; cur_w * cur_h];
                let mut pred_block = alloc::vec![128u8; cur_w * cur_h];
                for r in 0..cur_h {
                    for c in 0..cur_w {
                        src_block[r * cur_w + c] = encode_input[(y0 + r) * w + x0 + c];
                        pred_block[r * cur_w + c] = 128; // DC prediction
                    }
                }
                let enc = crate::encode_loop::encode_block(
                    &src_block,
                    cur_w,
                    &pred_block,
                    cur_w,
                    cur_w,
                    cur_h,
                    pcs.qp,
                );
                // Write using real coefficient coding
                svtav1_entropy::coeff::write_coefficients(
                    &mut writer,
                    &enc.qcoeffs,
                    enc.eob as usize,
                    0,
                    0,
                );
            }
        }
        let tile_data = writer.done().to_vec();

        // Step 6b: Film grain estimation (compare source to reconstruction)
        let _grain_params = crate::film_grain::estimate_film_grain(&encode_input, &recon, w, h, w);
        // grain_params would be signaled in the frame header OBU
        // and used by the decoder to re-synthesize grain

        // Step 7: Build OBU bitstream
        let bitstream = if is_key {
            svtav1_entropy::obu::write_still_frame(self.width, self.height, pcs.qp, &tile_data)
        } else {
            // For inter frames, just write tile group OBU
            svtav1_entropy::obu::write_obu(svtav1_entropy::obu::ObuType::Frame, &tile_data)
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

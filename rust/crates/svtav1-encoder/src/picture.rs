//! Picture management — PCS lifecycle, reference frame buffer, DPB.
//!
//! Spec 11 (picture-management.md): PCS, DPB, GOP.
//!
//! Manages the flow of pictures through the encoding pipeline:
//! input → analysis → mode decision → encode → output.
//!
//! Ported from SVT-AV1's pcs.h, sequence_control_set.h, and
//! sys_resource_manager.c.

use alloc::vec::Vec;
use svtav1_types::frame::FrameType;
use svtav1_types::reference::REF_FRAMES;

/// Picture Control Set — per-picture encoding state.
///
/// This is the central data structure that flows through the pipeline.
/// Each picture gets a PCS that tracks its encoding parameters,
/// reference frame assignments, and output status.
#[derive(Debug)]
pub struct PictureControlSet {
    /// Frame number in display order.
    pub display_order: u64,
    /// Frame number in decode order.
    pub decode_order: u64,
    /// Frame type (key, inter, intra-only, switch).
    pub frame_type: FrameType,
    /// Whether this frame is shown (vs. hidden alt-ref).
    pub show_frame: bool,
    /// Temporal layer index (0 = base layer).
    pub temporal_layer: u8,
    /// Hierarchical level within the mini-GOP.
    pub hierarchical_level: u8,
    /// Base QP for this picture.
    pub qp: u8,
    /// Reference frame indices into the DPB.
    pub ref_frame_idx: [i8; REF_FRAMES],
    /// Whether this frame refreshes a reference slot.
    pub refresh_frame_flags: u8,
    /// Picture width.
    pub width: u32,
    /// Picture height.
    pub height: u32,
}

impl PictureControlSet {
    pub fn new_key_frame(width: u32, height: u32, display_order: u64) -> Self {
        Self {
            display_order,
            decode_order: display_order,
            frame_type: FrameType::Key,
            show_frame: true,
            temporal_layer: 0,
            hierarchical_level: 0,
            qp: 30,
            ref_frame_idx: [-1; REF_FRAMES],
            refresh_frame_flags: 0xFF, // Refresh all slots
            width,
            height,
        }
    }

    pub fn new_inter_frame(
        width: u32,
        height: u32,
        display_order: u64,
        decode_order: u64,
        temporal_layer: u8,
    ) -> Self {
        Self {
            display_order,
            decode_order,
            frame_type: FrameType::Inter,
            show_frame: true,
            temporal_layer,
            hierarchical_level: temporal_layer,
            qp: 30,
            ref_frame_idx: [-1; REF_FRAMES],
            refresh_frame_flags: 0,
            width,
            height,
        }
    }
}

/// Decoded Picture Buffer — stores reference frames for inter prediction.
///
/// The DPB has REF_FRAMES (8) slots. Each slot can hold one decoded
/// frame that other frames can reference.
#[derive(Debug)]
pub struct DecodedPictureBuffer {
    /// Reference frame slots. None = empty slot.
    ///
    /// `Arc` because a KEY frame's `refresh_frame_flags` is 0xFF: every slot
    /// receives the SAME picture, and deep-cloning a full Y plane eight times
    /// per frame was 4.2 % of the port's `memmove`/`memset` self time. The
    /// slots are read-only once stored (`get` hands out `&ReferenceFrame`), so
    /// sharing one allocation is observationally identical to eight copies.
    /// The type stays private — `store`/`get`/`refresh` are unchanged.
    slots: [Option<alloc::sync::Arc<ReferenceFrame>>; REF_FRAMES],
}

/// C `scs->border` (`Globals/enc_handle.c:4256`): `BLOCK_SIZE_64 + 4`, the
/// margin every REFERENCE picture buffer carries.
///
/// It is not decoration. AV1 clamps a motion vector so the predicted block
/// plus its filter taps stays inside the frame PLUS this margin, and the MC
/// then indexes NEGATIVE offsets from pixel (0, 0) — so a reference stored
/// without it cannot answer a legal MV at all. `docs/INTER-ENCODE-PLAN.md`
/// §1t measured the consequence on the campaign's own cell: the harness
/// translates frame 1 right by 3 px with left-edge replication, so at the
/// correct MV `-3` the block's first three columns read outside the frame and
/// match EXACTLY only against a replicated margin. With a zero- or
/// 128-filled margin the residual is non-zero and C's `skip = 1` is not
/// reachable, which makes this a MODE-DECISION requirement and not only a
/// decoder-conformance one.
pub const REF_BORDER: usize = 64 + 4;

/// One reference plane with C's replicated margin applied
/// (`svt_aom_generate_padding`, driven from `pad_ref_and_set_flags`,
/// enc_dec_process.c:1088-1112).
#[derive(Debug, Clone)]
pub struct PaddedPlane {
    /// The whole allocation, `stride * (height + 2 * border)` bytes.
    pub buf: alloc::vec::Vec<u8>,
    /// Index of pixel (0, 0) — C's `y_buffer - buffer_y`.
    pub origin: usize,
    pub stride: usize,
    /// The plane's own dims, NOT counting the margin.
    pub width: usize,
    pub height: usize,
    pub border: usize,
}

impl PaddedPlane {
    /// Copy a bare `width x height` plane into a bordered allocation and
    /// replicate its edges, exactly as C pads a reference picture.
    ///
    /// `border` is [`REF_BORDER`] for luma and `(REF_BORDER + 1) >> 1` for
    /// 4:2:0 chroma — C's `(ref_pic_ptr->border + ss_x) >> ss_x`
    /// (enc_dec_process.c:1098-1112).
    #[must_use]
    pub fn from_plane(src: &[u8], width: usize, height: usize, border: usize) -> Self {
        let stride = width + 2 * border;
        let origin = border * stride + border;
        let mut buf = alloc::vec![0u8; stride * (height + 2 * border)];
        for r in 0..height {
            buf[origin + r * stride..origin + r * stride + width]
                .copy_from_slice(&src[r * width..r * width + width]);
        }
        crate::port_preanalysis::generate_padding(
            &mut buf, origin, stride, width, height, border, border,
        );
        Self {
            buf,
            origin,
            stride,
            width,
            height,
            border,
        }
    }

    /// The sample at `(x, y)` in PLANE coordinates, where negative values and
    /// values past the plane's own dims read the replicated margin.
    ///
    /// Out-of-margin coordinates PANIC rather than clamp: a legal MV cannot
    /// produce one (that is what the clamp in `compute_subpel_params` is
    /// for), so silently clamping would hide an MV-clamp defect as a
    /// pixel divergence.
    #[must_use]
    pub fn at(&self, x: isize, y: isize) -> u8 {
        let b = self.border as isize;
        assert!(
            x >= -b
                && y >= -b
                && x < (self.width + self.border) as isize
                && y < (self.height + self.border) as isize,
            "({x}, {y}) is outside the reference's {b}-pixel margin"
        );
        self.buf[(self.origin as isize + y * self.stride as isize + x) as usize]
    }
}

/// The three padded planes of one reference picture.
#[derive(Debug, Clone)]
pub struct PaddedRef {
    pub y: PaddedPlane,
    /// 4:2:0 chroma. `None` on a monochrome encode, where there is none.
    pub uv: Option<(PaddedPlane, PaddedPlane)>,
}

/// A reference frame stored in the DPB.
#[derive(Debug, Clone)]
pub struct ReferenceFrame {
    /// Reconstructed luma pixels.
    pub y_plane: Vec<u8>,
    /// Reconstructed CHROMA pixels, `(width/2) * (height/2)` each on the
    /// 4:2:0 path. EMPTY on the monochrome path and for any encode whose
    /// chroma the pipeline did not reconstruct.
    ///
    /// Inter prediction reads all three planes, so a luma-only DPB can only
    /// ever produce a luma-correct inter frame — that missing chroma is one of
    /// the defects the inter refusal in `pipeline.rs` names. Storing it is
    /// byte-inert for every still/key encode (nothing reads the DPB on a key
    /// frame); it costs one `w*h/2` clone per coded frame in a video encode.
    pub u_plane: Vec<u8>,
    /// See [`Self::u_plane`].
    pub v_plane: Vec<u8>,
    /// C `EbReferenceObject::ref_cdef_strengths[0][..num]`
    /// (`reference_object.h:52`, written by `rest_process.c:207-210`) — the
    /// PACKED luma CDEF strengths this picture's frame header signalled.
    ///
    /// A later frame's CDEF candidate set is rewritten from these
    /// (`update_cdef_filters_on_ref_info`), so a DPB that does not carry them
    /// cannot reproduce C's inter-frame CDEF at all.
    pub cdef_y_strengths: Vec<u8>,
    /// C `EbReferenceObject::frame_context` (`reference_object.h:39`), written
    /// at `packetization_process.c:741-744` — the END-OF-FRAME entropy state,
    /// counters already reset.
    ///
    /// A later frame whose header names this slot in `primary_ref_frame` starts
    /// its tile CDFs from exactly these tables (`ec_process.c:101-112`). That
    /// is a CONFORMANCE requirement, not a compression choice: a decoder reads
    /// `primary_ref_frame` and does the same restore, so an encoder that coded
    /// against different probabilities produces a stream that decodes to
    /// garbage rather than one that merely differs in size.
    ///
    /// `None` on any frame whose entropy walk did not run (there is none
    /// today) — a restore from `None` falls back to the spec defaults, which is
    /// what `PRIMARY_REF_NONE` means, so the failure mode is a wrong stream,
    /// never a panic. See [`crate::port_frame_cdf`].
    pub frame_cdfs: Option<alloc::sync::Arc<crate::port_frame_cdf::FrameCdfs>>,
    /// C `EbReferenceObject::sb_min_sq_size[sb]` — the minimum
    /// `blk_geom->sq_size` this picture CODED in each superblock, in raster SB
    /// order (`coding_loop.c:1640`; init 128 at `enc_dec_process.c:3101`).
    ///
    /// Read by the NEXT frame's `set_depth_removal_level_controls`
    /// (`enc_mode_config.c:3173-3196`), which raises `dev_32x32_to_16x16_th`
    /// and `dev_16x16_to_8x8_th` when the value is >= 64 or >= 32. EMPTY on a
    /// picture whose trees were not folded, which the reader treats as C's
    /// `(uint8_t)~0` "no usable reference" sentinel — the arm that makes NO
    /// adjustment, so an empty vector is inert rather than wrong.
    pub sb_min_sq_size: alloc::vec::Vec<u8>,
    /// The same recon with C's replicated reference margin
    /// ([`PaddedRef`]), which is the form INTER PREDICTION indexes — see
    /// [`REF_BORDER`] for why the margin is load-bearing for the DECISION
    /// and not only for conformance.
    ///
    /// `None` only on a frame whose recon was not stored (there is none
    /// today); the bare planes above stay because every non-MC reader —
    /// TPL's SB qp offsets, the open-loop ME's own pyramid — indexes them
    /// at frame stride.
    pub padded: Option<alloc::boxed::Box<PaddedRef>>,
    /// The chroma twin of [`Self::cdef_y_strengths`]
    /// (`ref_cdef_strengths[1][..num]`).
    pub cdef_uv_strengths: Vec<u8>,
    /// C `EbReferenceObject::filter_level[0..2]` + `filter_level_u` +
    /// `filter_level_v` (`reference_object.h:44-47`, written by
    /// `rest_process.c:200-203`) — the loop-filter levels this picture's
    /// FRAME HEADER signalled, as `[y_vert, y_horz, u, v]`.
    ///
    /// A later frame's deblock level is derived from these on BOTH pickers:
    /// the by-q one takes their per-plane MIN and shuts its own filter off
    /// when any is zero, and the full-image one takes their MEAN as the
    /// level it copies (see [`crate::dlf_arm`]). A DPB that does not carry
    /// them cannot reproduce C's inter-frame deblocking at all — which is
    /// what made 20 of the inter campaign's 40 residual cells differ FIRST
    /// at `loop_filter_level[0]`.
    pub lf_levels: [u8; 4],
    /// C `EbReferenceObject::dlf_dist_dev` (`reference_object.h:49`, written
    /// by `rest_process.c:204`) — `1000 - 1000 * best_sse / zero_sse` for
    /// this picture's own deblock, i.e. the per-mille SSE improvement the
    /// filter actually bought.
    ///
    /// **-1 means "never computed"**, and readers must SKIP it rather than
    /// average it in: `dlf_process.c:92` seeds it there and only the
    /// non-SB-based path overwrites it, so a picture coded with
    /// `sb_based_dlf` genuinely has no measurement. Treating -1 as a small
    /// number would trip the `prev_dlf_dist < 5` shut-off on every frame
    /// that follows a fast-path one.
    pub dlf_dist_dev: i32,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Display order of this frame.
    pub display_order: u64,
    /// Order hint for temporal distance computation.
    pub order_hint: u32,
}

impl DecodedPictureBuffer {
    pub fn new() -> Self {
        Self {
            slots: Default::default(),
        }
    }

    /// Store a reference frame in the specified slot.
    pub fn store(&mut self, slot: usize, frame: ReferenceFrame) {
        if slot < REF_FRAMES {
            self.slots[slot] = Some(alloc::sync::Arc::new(frame));
        }
    }

    /// Get a reference to a frame in the specified slot.
    pub fn get(&self, slot: usize) -> Option<&ReferenceFrame> {
        if slot < REF_FRAMES {
            self.slots[slot].as_deref()
        } else {
            None
        }
    }

    /// Refresh slots based on the refresh_frame_flags bitmask.
    pub fn refresh(&mut self, flags: u8, frame: &ReferenceFrame) {
        if flags == 0 {
            return;
        }
        // ONE clone, shared by every refreshed slot (see the `slots` doc).
        let shared = alloc::sync::Arc::new(frame.clone());
        for i in 0..REF_FRAMES {
            if flags & (1 << i) != 0 {
                self.slots[i] = Some(alloc::sync::Arc::clone(&shared));
            }
        }
    }

    /// Clear all slots.
    pub fn clear(&mut self) {
        self.slots.fill(None);
    }

    /// Count occupied slots.
    pub fn occupied_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

impl Default for DecodedPictureBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// GOP (Group of Pictures) structure for hierarchical coding.
#[derive(Debug, Clone)]
pub struct GopStructure {
    /// Number of hierarchical levels (1-6).
    pub hierarchical_levels: u8,
    /// Mini-GOP size (e.g., 16 for 4-level hierarchy).
    pub mini_gop_size: u32,
    /// Intra period (key frame interval). 0 = single key frame.
    pub intra_period: u32,
}

impl GopStructure {
    pub fn new(hierarchical_levels: u8, intra_period: u32) -> Self {
        let mini_gop_size = 1u32 << hierarchical_levels;
        Self {
            hierarchical_levels,
            mini_gop_size,
            intra_period,
        }
    }

    /// Get the temporal layer for a given position within a mini-GOP.
    pub fn get_temporal_layer(&self, pos_in_gop: u32) -> u8 {
        if pos_in_gop == 0 {
            return 0; // Base layer
        }
        // The temporal layer is determined by the position's factor of 2
        let mut layer = self.hierarchical_levels;
        let mut step = 1u32;
        while step < self.mini_gop_size {
            // The divisor cannot be 0: the loop invariant `step <
            // mini_gop_size` makes `mini_gop_size / step >= 1`, so this is the
            // same value the `%` form computed. (`is_multiple_of` DOES differ
            // from `% n == 0` at n == 0 — it answers `self == 0` where `%`
            // panics — so the invariant is what makes the swap behaviour-
            // preserving here, not the rewrite itself.)
            if pos_in_gop.is_multiple_of(self.mini_gop_size / step) {
                return layer.saturating_sub(1);
            }
            layer = layer.saturating_sub(1);
            step *= 2;
        }
        self.hierarchical_levels
    }

    /// Determine if a frame at this position should be a key frame.
    pub fn is_key_frame(&self, display_order: u64) -> bool {
        // Kept explicit: `intra_period == 0` reaches the `%` as a zero divisor.
        // `is_multiple_of(0)` happens to answer `display_order == 0` — the same
        // thing — but the guard states the intent and predates the rewrite.
        if self.intra_period == 0 {
            return display_order == 0;
        }
        display_order.is_multiple_of(self.intra_period as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcs_key_frame() {
        let pcs = PictureControlSet::new_key_frame(1920, 1080, 0);
        assert_eq!(pcs.frame_type, FrameType::Key);
        assert!(pcs.show_frame);
        assert_eq!(pcs.refresh_frame_flags, 0xFF);
    }

    #[test]
    fn pcs_inter_frame() {
        let pcs = PictureControlSet::new_inter_frame(1920, 1080, 5, 3, 2);
        assert_eq!(pcs.frame_type, FrameType::Inter);
        assert_eq!(pcs.temporal_layer, 2);
    }

    #[test]
    fn dpb_store_and_get() {
        let mut dpb = DecodedPictureBuffer::new();
        assert_eq!(dpb.occupied_slots(), 0);

        let frame = ReferenceFrame {
            padded: None,
            y_plane: alloc::vec![128u8; 64 * 64],
            u_plane: alloc::vec![],
            v_plane: alloc::vec![],
            cdef_y_strengths: alloc::vec![],
            cdef_uv_strengths: alloc::vec![],
            frame_cdfs: None,
            sb_min_sq_size: alloc::vec![],
            lf_levels: [0; 4],
            dlf_dist_dev: -1,
            width: 64,
            height: 64,
            display_order: 0,
            order_hint: 0,
        };
        dpb.store(0, frame);
        assert_eq!(dpb.occupied_slots(), 1);
        assert!(dpb.get(0).is_some());
        assert!(dpb.get(1).is_none());
    }

    #[test]
    fn dpb_refresh() {
        let mut dpb = DecodedPictureBuffer::new();
        let frame = ReferenceFrame {
            padded: None,
            y_plane: alloc::vec![128u8; 16],
            u_plane: alloc::vec![],
            v_plane: alloc::vec![],
            cdef_y_strengths: alloc::vec![],
            cdef_uv_strengths: alloc::vec![],
            frame_cdfs: None,
            sb_min_sq_size: alloc::vec![],
            lf_levels: [0; 4],
            dlf_dist_dev: -1,
            width: 4,
            height: 4,
            display_order: 0,
            order_hint: 0,
        };
        // Refresh slots 0, 2, 4 (flags = 0b00010101 = 0x15)
        dpb.refresh(0x15, &frame);
        assert_eq!(dpb.occupied_slots(), 3);
        assert!(dpb.get(0).is_some());
        assert!(dpb.get(1).is_none());
        assert!(dpb.get(2).is_some());
    }

    #[test]
    fn gop_key_frame_detection() {
        let gop = GopStructure::new(4, 64);
        assert!(gop.is_key_frame(0));
        assert!(!gop.is_key_frame(1));
        assert!(gop.is_key_frame(64));
        assert!(!gop.is_key_frame(63));
    }

    #[test]
    fn gop_temporal_layers() {
        let gop = GopStructure::new(3, 64);
        assert_eq!(gop.get_temporal_layer(0), 0); // base
        assert_eq!(gop.mini_gop_size, 8);
    }

    #[test]
    fn gop_single_key() {
        let gop = GopStructure::new(4, 0);
        assert!(gop.is_key_frame(0));
        assert!(!gop.is_key_frame(1));
        assert!(!gop.is_key_frame(1000));
    }
}

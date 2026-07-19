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
    slots: [Option<ReferenceFrame>; REF_FRAMES],
}

/// A reference frame stored in the DPB.
#[derive(Debug, Clone)]
pub struct ReferenceFrame {
    /// Reconstructed luma pixels.
    pub y_plane: Vec<u8>,
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
            self.slots[slot] = Some(frame);
        }
    }

    /// Get a reference to a frame in the specified slot.
    pub fn get(&self, slot: usize) -> Option<&ReferenceFrame> {
        if slot < REF_FRAMES {
            self.slots[slot].as_ref()
        } else {
            None
        }
    }

    /// Refresh slots based on the refresh_frame_flags bitmask.
    pub fn refresh(&mut self, flags: u8, frame: &ReferenceFrame) {
        for i in 0..REF_FRAMES {
            if flags & (1 << i) != 0 {
                self.slots[i] = Some(frame.clone());
            }
        }
    }

    /// Clear all slots.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
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
            if pos_in_gop % (self.mini_gop_size / step) == 0 {
                return layer.saturating_sub(1);
            }
            layer = layer.saturating_sub(1);
            step *= 2;
        }
        self.hierarchical_levels
    }

    /// Determine if a frame at this position should be a key frame.
    pub fn is_key_frame(&self, display_order: u64) -> bool {
        if self.intra_period == 0 {
            return display_order == 0;
        }
        display_order % self.intra_period as u64 == 0
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
            y_plane: alloc::vec![128u8; 64 * 64],
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
            y_plane: alloc::vec![128u8; 16],
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

//! Arbitrary frame dimensions — chunk 1 geometry plumbing (task #95).
//!
//! Source-to-source translation of the C geometry model per
//! docs/arbitrary-dims-port-map.md. UNWIRED (add `pub mod frame_geom;` to
//! lib.rs when integration starts); written under the 2026-07-17
//! bulk-write directive — no build/test run yet.
//!
//! THE model (map §0): TWO boundary systems coexist.
//! - ALIGNED (mi grid): true dims rounded UP to a multiple of 8
//!   (MIN_BLOCK_SIZE, definitions.h:2034; spec 7.2.6 compute_image_size).
//!   Consumers: SB/mi grid, partition has_rows/has_cols, cropped-tx RDO
//!   bound, CDEF fb grid, tile info.
//! - TRUE/CODED (crop): the user dims, possibly ODD. Consumers: seq/frame
//!   header size fields, DLF plane bounds, LR unit sizing, recon output.
//! - SB-grid extent (ceil(aligned/sb)*sb) is a loop trip-count only.

/// C MIN_BLOCK_SIZE (definitions.h:2034).
pub const MIN_BLOCK_SIZE: usize = 8;

/// Frame geometry carrying BOTH boundary systems. Every consumer must
/// pick the correct one — see the module doc table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDims {
    /// TRUE/CODED luma dims (header-signaled; can be odd).
    pub true_w: usize,
    pub true_h: usize,
    /// ALIGNED luma dims (multiple of 8; == true dims when 8-aligned).
    pub aligned_w: usize,
    pub aligned_h: usize,
}

impl FrameDims {
    /// C set_param_based_on_input (enc_handle.c:3918-3930; the SAME
    /// arithmetic is duplicated at resource_coordination_process.c:717-729
    /// — ported ONCE here on purpose).
    pub fn new(true_w: usize, true_h: usize) -> Self {
        let pad_r = if true_w % MIN_BLOCK_SIZE != 0 {
            MIN_BLOCK_SIZE - (true_w % MIN_BLOCK_SIZE)
        } else {
            0
        };
        let pad_b = if true_h % MIN_BLOCK_SIZE != 0 {
            MIN_BLOCK_SIZE - (true_h % MIN_BLOCK_SIZE)
        } else {
            0
        };
        FrameDims {
            true_w,
            true_h,
            aligned_w: true_w + pad_r,
            aligned_h: true_h + pad_b,
        }
    }

    /// Right/bottom pad amounts (ALIGNED - TRUE).
    pub fn pad_right(&self) -> usize {
        self.aligned_w - self.true_w
    }
    pub fn pad_bottom(&self) -> usize {
        self.aligned_h - self.true_h
    }

    /// TRUE chroma dims, CEILING rounding — the convention used by
    /// pic_buffer_desc.c:567/:619, restoration.c:1534/:1579/:1604 and the
    /// recon output crop (app_context.c:123). (w+1)>>1 at 4:2:0.
    pub fn true_chroma_ceil(&self) -> (usize, usize) {
        ((self.true_w + 1) >> 1, (self.true_h + 1) >> 1)
    }

    /// TRUE chroma dims, FLOOR rounding — DLF's convention
    /// (deblocking_filter.c:167-168 uses a plain >>1 on true dims).
    /// PORT-NOTE(unverified): at ODD true widths this DISAGREES with the
    /// ceiling convention used everywhere else in C — DLF treats the last
    /// valid chroma column as out of bounds (under-filters). REPLICATE
    /// per-consumer; verify with the 65x65 odd-width differential vs
    /// SvtAv1EncApp before trusting either convention at odd dims.
    pub fn true_chroma_floor_dlf(&self) -> (usize, usize) {
        (self.true_w >> 1, self.true_h >> 1)
    }

    /// ALIGNED chroma dims (aligned dims are even — no ambiguity).
    pub fn aligned_chroma(&self) -> (usize, usize) {
        (self.aligned_w >> 1, self.aligned_h >> 1)
    }

    /// mi grid extent in 4x4 units (ALIGNED-based, spec MiCols/MiRows).
    pub fn mi_cols(&self) -> usize {
        self.aligned_w >> 2
    }
    pub fn mi_rows(&self) -> usize {
        self.aligned_h >> 2
    }

    /// SB counts for a given sb size (loop trip-counts; each SB clamps
    /// back to ALIGNED via [`sb_geom`]).
    pub fn sb_cols(&self, sb: usize) -> usize {
        self.aligned_w.div_ceil(sb)
    }
    pub fn sb_rows(&self, sb: usize) -> usize {
        self.aligned_h.div_ceil(sb)
    }
}

/// Per-SB clamped geometry — C sb_geom_init (pcs.c:1535-1555):
/// `width = MIN(aligned - org, sb)`. Because org is a multiple of sb and
/// aligned is a multiple of 8, partial sizes are ALWAYS multiples of 8.
pub fn sb_geom(dims: &FrameDims, sb: usize, sb_x: usize, sb_y: usize) -> (usize, usize) {
    (
        (dims.aligned_w - sb_x).min(sb),
        (dims.aligned_h - sb_y).min(sb),
    )
}

/// Spec 5.11.4 partition-edge predicates against the ALIGNED grid — the
/// single rule mirrored at C entropy_coding.c:932-981 (writer) and the
/// three search sites (product_coding_loop.c:10538/:10618/:10896 +
/// enc_dec_process.c:1394). `half` = half the square's pixel size.
/// - both true  -> full partition alphabet
/// - !has_cols  -> binary SPLIT-vs-VERT (shape must be PART_V)
/// - !has_rows  -> binary SPLIT-vs-HORZ (shape must be PART_H)
/// - both false -> forced SPLIT, NO symbol coded
/// PORT-NOTE(unverified): verify on the 96x80 milestone cell; the port's
/// current walk/writer assume has_rows == has_cols == true everywhere
/// (64-aligned harness).
pub fn edge_has_rows_cols(
    dims: &FrameDims,
    blk_x: usize,
    blk_y: usize,
    half: usize,
) -> (bool, bool) {
    (
        blk_y + half < dims.aligned_h,
        blk_x + half < dims.aligned_w,
    )
}

/// C pad_input_picture (pic_operators.c:561-604): replicate the last real
/// column into the right pad, THEN memcpy the last real row (INCLUDING
/// the just-written right pad) into the bottom pad — order matters for
/// the bottom-right corner. Operates on an aligned_w-strided plane whose
/// left-top true_w x true_h region holds source pixels.
/// PORT-NOTE(unverified): byte-compare against the C pad on a non-aligned
/// cell (the sc detector's pad-to-8 already ported independently in
/// sc_detect.rs uses the same replication rule).
pub fn pad_input_plane(plane: &mut [u8], dims: &FrameDims) {
    let (tw, th, aw, ah) = (dims.true_w, dims.true_h, dims.aligned_w, dims.aligned_h);
    debug_assert!(plane.len() >= aw * ah);
    if tw < aw {
        for r in 0..th {
            let edge = plane[r * aw + tw - 1];
            for c in tw..aw {
                plane[r * aw + c] = edge;
            }
        }
    }
    if th < ah {
        let (last_real, rest) = plane.split_at_mut(th * aw);
        let src_row = &last_real[(th - 1) * aw..th * aw];
        for r in 0..(ah - th) {
            rest[r * aw..(r + 1) * aw].copy_from_slice(src_row);
        }
    }
}

/// C cropped-tx RDO bound (product_coding_loop.c:4664, full_loop.c:2228):
/// the DISTORTION metric (only — never the coded residual/tx) crops to
/// the ALIGNED boundary. Returns the (w, h) the distortion kernels see.
pub fn cropped_tx_dims(
    dims: &FrameDims,
    tx_x: usize,
    tx_y: usize,
    txw: usize,
    txh: usize,
) -> (usize, usize) {
    (
        txw.min(dims.aligned_w.saturating_sub(tx_x)),
        txh.min(dims.aligned_h.saturating_sub(tx_y)),
    )
}

/// Config side effects (enc_settings.c:214-233): frames with EITHER dim
/// < 64 force-disable restoration (and AQ) — an SVT implementation limit,
/// replicated for parity, not a spec rule.
pub fn small_frame_disables_restoration(dims: &FrameDims) -> bool {
    dims.true_w < 64 || dims.true_h < 64
}

/// LR unit-count collapse (restoration.c:71-73): units = max(round-to-
/// nearest(size/256), 1) — any plane <= 384px gets exactly ONE unit.
pub fn lr_units_in_dim(plane_size: usize) -> usize {
    const UNIT: usize = 256;
    ((plane_size + UNIT / 2) / UNIT).max(1)
}

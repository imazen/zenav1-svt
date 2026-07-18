//! Arbitrary frame dimensions — chunk 1 geometry plumbing (task #95).
//!
//! Source-to-source translation of the C geometry model per
//! docs/arbitrary-dims-port-map.md.
//!
//! WIRING STATUS: [`FrameDims::new`] backs the pipeline's true-vs-aligned dims
//! (chunk 1) and [`edge_has_rows_cols`] backs `pipeline::partition_edge_flags`
//! (chunk 2 pack side), both covered by tests below. [`sb_geom`],
//! [`cropped_tx_dims`], [`pad_input_plane`] and the `mi_*`/`sb_*` accessors are
//! still unwired — the pipeline re-derives those inline; route them through
//! here as the chunk-2 search restructure lands, so the frame extent has ONE
//! definition.
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
/// Takes the ALIGNED extent directly (rather than a [`FrameDims`]) so the
/// pack walk — which knows the frame only through the deblock geometry — can
/// share this one rule instead of re-deriving it; `pipeline::
/// partition_edge_flags` delegates here.
///
/// Note a size invariant that keeps the search's base case safe: while the
/// aligned dims are a multiple of 8, an 8x8 node can NEVER be an edge node
/// (`half` = 4, and an 8-node sits at a multiple of 8, so `blk + 4` is at most
/// `aligned - 4 < aligned`). Edge handling therefore only ever applies to
/// nodes of 16 pixels and up.
///
/// PORT-NOTE(unverified at the STREAM level): the flag algebra is unit-tested
/// below against hand-derived 96x80 vectors, but end-to-end byte-identity at a
/// partial-SB cell still needs the search restructure (#95 chunk 2).
pub fn edge_has_rows_cols(
    aligned_w: usize,
    aligned_h: usize,
    blk_x: usize,
    blk_y: usize,
    half: usize,
) -> (bool, bool) {
    (blk_y + half < aligned_h, blk_x + half < aligned_w)
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

/// Seq-header max-frame-size bit derivation (C entropy_coding.c:2760-2783):
/// `bits = floor_log2(max); if max > 1<<bits { bits += 1 }`, then the
/// writer emits `bits-1` as a 4-bit literal followed by `max-1` in `bits`
/// bits. Operates on TRUE dims (captured pre-alignment,
/// enc_handle.c:4792-4799). Returns (bits, minus_1_value).
/// PORT-NOTE(unverified): the port's SH writer currently derives width
/// bits its own way for 64-aligned dims — swap to this at #95 chunk 2
/// and byte-compare the SH on a non-aligned cell.
pub fn seq_size_bits(max_dim: usize) -> (u32, u32) {
    debug_assert!(max_dim >= 1);
    let mut bits = usize::BITS - 1 - max_dim.leading_zeros(); // floor log2
    if max_dim > (1usize << bits) {
        bits += 1;
    }
    (bits.max(1), (max_dim - 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-derived vectors for the spec-5.11.4 edge predicates, taken from the
    /// 96x80 milestone cell (docs/arbitrary-dims-port-map.md): aligned 96x80
    /// gives a 2x2 SB grid where SB(0,0) is interior, SB(0,1) hits the RIGHT
    /// edge, SB(1,0) the BOTTOM edge, and SB(1,1) both — so one frame exercises
    /// all three non-interior branches.
    #[test]
    fn edge_flags_match_c_rule_on_the_96x80_milestone() {
        let (aw, ah) = (96, 80);
        // has_rows = y + half < aligned_h ; has_cols = x + half < aligned_w
        // SB(0,0): 0+32<80 and 0+32<96 -> interior, full alphabet.
        assert_eq!(edge_has_rows_cols(aw, ah, 0, 0, 32), (true, true));
        // SB(0,1) at x=64: 64+32 == 96, NOT < 96 -> right edge (binary
        // SPLIT-vs-VERT).
        assert_eq!(edge_has_rows_cols(aw, ah, 64, 0, 32), (true, false));
        // SB(1,0) at y=64: 64+32 == 96, NOT < 80 -> bottom edge (binary
        // SPLIT-vs-HORZ).
        assert_eq!(edge_has_rows_cols(aw, ah, 0, 64, 32), (false, true));
        // SB(1,1): both -> forced SPLIT, no symbol coded.
        assert_eq!(edge_has_rows_cols(aw, ah, 64, 64, 32), (false, false));
        // Descending into SB(1,1): the 32-node at (64,64) is still a BOTTOM
        // edge (64+16 == 80, not < 80) but no longer a right edge (80 < 96).
        assert_eq!(edge_has_rows_cols(aw, ah, 64, 64, 16), (false, true));
        // Its 16-node is fully interior (72 < 80, 72 < 96) — the recursion
        // terminates on in-frame leaves, which is why nothing codes half a
        // block.
        assert_eq!(edge_has_rows_cols(aw, ah, 64, 64, 8), (true, true));
    }

    /// While the aligned dims are a multiple of 8, an 8x8 node can never be an
    /// edge node — the invariant that keeps the search's `MIN_BLOCK_SIZE` base
    /// case (which can only emit PARTITION_NONE) legal at the frame border.
    #[test]
    fn eight_pixel_nodes_are_never_edge_nodes() {
        for (aw, ah) in [(96usize, 80usize), (72, 40), (200, 104), (64, 64)] {
            let mut x = 0;
            while x < aw {
                let mut y = 0;
                while y < ah {
                    // half = 4 for an 8x8 node
                    assert_eq!(
                        edge_has_rows_cols(aw, ah, x, y, 4),
                        (true, true),
                        "8x8 node at ({x},{y}) in {aw}x{ah} must be interior"
                    );
                    y += 8;
                }
                x += 8;
            }
        }
    }

    /// A 64-aligned frame has no edge nodes at any partition size — this is the
    /// property that makes the edge coding byte-neutral on every currently
    /// gated cell (identity matrix 54/54).
    #[test]
    fn sixty_four_aligned_frames_have_no_edge_nodes() {
        for (aw, ah) in [(64usize, 64usize), (128, 128), (192, 64), (256, 192)] {
            for half in [4usize, 8, 16, 32] {
                let node = half * 2;
                let mut x = 0;
                while x < aw {
                    let mut y = 0;
                    while y < ah {
                        assert_eq!(
                            edge_has_rows_cols(aw, ah, x, y, half),
                            (true, true),
                            "{node}x{node} node at ({x},{y}) in {aw}x{ah} must be interior"
                        );
                        y += node;
                    }
                    x += node;
                }
            }
        }
    }

    /// sb_geom clamps the per-SB extent to the aligned frame (C sb_geom_init).
    #[test]
    fn sb_geom_clamps_partial_superblocks() {
        let dims = FrameDims::new(96, 80);
        assert_eq!((dims.aligned_w, dims.aligned_h), (96, 80));
        assert_eq!(sb_geom(&dims, 64, 0, 0), (64, 64));
        assert_eq!(sb_geom(&dims, 64, 64, 0), (32, 64)); // right column
        assert_eq!(sb_geom(&dims, 64, 0, 64), (64, 16)); // bottom row
        assert_eq!(sb_geom(&dims, 64, 64, 64), (32, 16)); // corner
    }
}

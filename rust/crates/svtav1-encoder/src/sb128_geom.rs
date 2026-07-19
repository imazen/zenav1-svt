//! SB128 — chunk 1 geometry tables (task #91, dead code at SB64).
//!
//! Source translation per docs/sb128-port-map.md. UNWIRED (add
//! `pub mod sb128_geom;` when integration starts); bulk-write directive
//! 2026-07-17, no build run yet. Every item is inert until the
//! Globals/enc_handle.c:4071 selection gate is ported (chunk 11).
//!
//! THE architectural fact: SVT keeps TWO grids — a b64 grid ALWAYS 64x64
//! (ME/variance/per-64 stats) and the sb grid following super_block_size.
//! SB128 code exists to bridge them.

/// C ns_blk_offset_md (common_utils.c:269) — per-shape mds block-index
/// offsets for sub-128 squares. Shape order: N,H,V,H4,V4,HA,HB,VA,VB.
pub const NS_BLK_OFFSET_MD: [u32; 9] = [0, 1, 3, 5, 9, 13, 16, 19, 22];

/// C ns_blk_offset_128_md (common_utils.c:270) — the 128-square variant:
/// H4/V4 are GEOMETRICALLY impossible at 128 (no 128x32 BlockSize), so
/// their slots are 0 and the AB shapes pack tighter. This is a TABLE
/// SWAP in C (consumer product_coding_loop.c:10893 picks by
/// bsize==BLOCK_128X128), not a generic skip — replicate as a swap.
pub const NS_BLK_OFFSET_128_MD: [u32; 9] = [0, 1, 3, 0, 0, 5, 8, 11, 14];

/// Partition-symbol alphabet length (C svt_aom_partition_cdf_length,
/// entropy_coding.c:922-930): 4 at <=8x8, 8 at 128x128 (EXT minus
/// H4/V4), 10 otherwise.
pub fn partition_cdf_length(sq: usize) -> usize {
    if sq <= 8 {
        4
    } else if sq == 128 {
        8
    } else {
        10
    }
}

/// Whether a shape is legal at a given square size. At 128: H4/V4
/// excluded by geometry; everything else (incl. HA/HB/VA/VB) legal —
/// M0/M1's picture-wide allow_HVA_HVB=0 is a SEPARATE gate applied at
/// every depth, not a 128 rule (map §2). Shape indices as
/// NS_BLK_OFFSET order: 0=N 1=H 2=V 3=H4 4=V4 5=HA 6=HB 7=VA 8=VB.
pub fn shape_legal_at(sq: usize, shape: usize) -> bool {
    match shape {
        3 | 4 => sq != 128 && sq >= 16, // H4/V4: no 128x32; C also bars <16
        5..=8 => sq >= 16,
        _ => true,
    }
}

/// C get_sb128_variance (enc_mode_config.c:119-142): AVERAGE of up to 4
/// b64 variance cells with an edge-clamped divisor. `b64` is the always-
/// 64 grid; (bx, by) the SB's first b64 coords.
pub fn sb128_variance(
    b64_var: &[u16],
    b64_cols: usize,
    b64_rows: usize,
    bx: usize,
    by: usize,
) -> u16 {
    let mut sum = u32::from(b64_var[by * b64_cols + bx]);
    let mut count = 1u32;
    if bx + 1 < b64_cols {
        sum += u32::from(b64_var[by * b64_cols + bx + 1]);
        count += 1;
    }
    if by + 1 < b64_rows {
        sum += u32::from(b64_var[(by + 1) * b64_cols + bx]);
        count += 1;
        if bx + 1 < b64_cols {
            sum += u32::from(b64_var[(by + 1) * b64_cols + bx + 1]);
            count += 1;
        }
    }
    (sum / count) as u16
}

/// C get_sb128_me_data (enc_mode_config.c:62-114): the distortion fields
/// AVERAGE like variance, BUT me_8x8_cost_var takes the MAX of the
/// covered cells (:83/:91/:100) — the asymmetry the map flags as the
/// easy mis-port. Generic helpers so callers can't mix them up.
pub fn sb128_bridge_avg(vals: [Option<u32>; 4]) -> u32 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for v in vals.into_iter().flatten() {
        sum += u64::from(v);
        n += 1;
    }
    debug_assert!(n > 0);
    (sum / n.max(1)) as u32
}
pub fn sb128_bridge_max(vals: [Option<u32>; 4]) -> u32 {
    vals.into_iter().flatten().max().unwrap_or(0)
}

/// CDEF three-phase contract at SB128 (map §5; the aom-rs KB-1 #2 bug
/// class). fb unit = 64x64 ALWAYS. This helper answers phase-1/phase-3's
/// shared question: is fb (fbc, fbr) a NON-top-left quadrant of a
/// 128-variant block (given that block's bsize at the fb's own mi)?
/// - Phase 1 (search): if yes -> SKIP the fb entirely (stats stay stale).
/// - Phase 3 (apply): if yes -> dirinit forced fresh (cached dir stale).
/// bsize codes: 0 = not 128-variant, 1 = 128x128, 2 = 128x64, 3 = 64x128.
/// PORT-NOTE(unverified): phase 2 (explicit cdef_strength fan-out to the
/// covered quadrant slots keyed by bsize, enc_cdef.c:874-893) and the
/// write_cdef 64-mask quadrant indexing (entropy_coding.c:3986-4017)
/// must land IN THE SAME CHUNK as the consumers; a synthetic unit test
/// vs a filter-every-64-independently reference is required before the
/// SB128 gate flips (map chunk 8).
pub fn cdef_fb_is_stale_quadrant(fbc: usize, fbr: usize, bsize128: u8) -> bool {
    match bsize128 {
        1 => (fbc & 1 != 0) || (fbr & 1 != 0),
        2 => fbc & 1 != 0, // 128x64: right quadrant stale
        3 => fbr & 1 != 0, // 64x128: below quadrant stale
        _ => false,
    }
}

/// Seq-header bit: C derives use_128x128_superblock from
/// sb_size == BLOCK_128X128 at write time (entropy_coding.c:2800); the
/// struct field is dead. sb_mi_size = 32, MI-domain sb_size_log2 = 5,
/// PIXEL log2 = 7 (keep both representations distinct — the tile-limit
/// formulas use the PIXEL one: max_tile_width_sb = 4096 >> pixel_log2,
/// HALVED at SB128, entropy_coding.c:2450-2467).
pub fn sb_header_params(sb: usize) -> (bool, usize, u32, u32) {
    debug_assert!(sb == 64 || sb == 128);
    let is128 = sb == 128;
    (
        is128,
        if is128 { 32 } else { 16 },  // sb_mi_size
        if is128 { 5 } else { 4 },    // MI-domain log2
        if is128 { 7 } else { 6 },    // PIXEL-domain log2
    )
}

/// C write_cdef (entropy_coding.c:3986-4017), translated exactly — the
/// phase-4 signaling side of the CDEF contract. Per coded block:
/// - lossless/intrabc frames: no cdef syntax at all (caller gate).
/// - the mbmi whose cdef_strength is written is read at the mi rounded
///   DOWN to 64-alignment: `m = ~((1<<(6-MI_SIZE_LOG2))-1)` = ~15,
///   i.e. (mi_row & !15, mi_col & !15) — always 64-based, even at SB128.
/// - `cdef_transmitted[4]` resets at each SB TOP-LEFT
///   (`!(mi & (sb_mi_size-1))`, sb_mi_size 32 at SB128 / 16 at SB64).
/// - the quadrant slot uses BIT 4 ONLY (`mask = 1<<(6-MI_SIZE_LOG2)` =
///   16): `index = sb128 ? ((mi_col>>4)&1) + 2*((mi_row>>4)&1) : 0` —
///   NOT the ~15 rounding mask (two different masks in one function;
///   easy to conflate).
/// - the literal is emitted at the FIRST NON-SKIP block of the quadrant
///   (cdef_bits wide), then the slot latches.
/// PORT-NOTE(unverified): the port's current writer emits cdef_idx once
/// per 64-SB via its own path; at SB128 wiring, replace with this state
/// machine + a synthetic 4-quadrant unit test.
pub struct CdefTransmit {
    transmitted: [bool; 4],
}

impl CdefTransmit {
    pub fn new() -> Self {
        CdefTransmit {
            transmitted: [false; 4],
        }
    }

    /// Call per coded block, in coding order. `sb_mi_size` 16/32.
    /// Returns Some(mbmi_mi) — the 64-aligned mi whose cdef_strength to
    /// write — when the cdef literal must be emitted for this block.
    pub fn on_block(
        &mut self,
        mi_row: usize,
        mi_col: usize,
        sb_mi_size: usize,
        sb128: bool,
        skip: bool,
    ) -> Option<(usize, usize)> {
        if mi_row & (sb_mi_size - 1) == 0 && mi_col & (sb_mi_size - 1) == 0 {
            self.transmitted = [false; 4];
        }
        let index = if sb128 {
            ((mi_col >> 4) & 1) + 2 * ((mi_row >> 4) & 1)
        } else {
            0
        };
        if !self.transmitted[index] && !skip {
            self.transmitted[index] = true;
            Some((mi_row & !15usize, mi_col & !15usize))
        } else {
            None
        }
    }
}

impl Default for CdefTransmit {
    fn default() -> Self {
        Self::new()
    }
}

/// CDEF phase-2 strength fan-out (C propagate_cdef_strength,
/// enc_cdef.c:874-893): the single searched strength for a 128-variant
/// block must be EXPLICITLY written to every covered 64-quadrant grid
/// slot — mi-grid aliasing does NOT cover it (cdef_strength is assigned
/// post-MD). Returns the (mi_row, mi_col) offsets (in mi units, 16 = one
/// 64-quadrant) of the EXTRA slots beyond the block's own top-left.
/// bsize128 codes as in [`cdef_fb_is_stale_quadrant`].
pub fn cdef_strength_fanout_offsets(bsize128: u8) -> &'static [(usize, usize)] {
    match bsize128 {
        1 => &[(0, 16), (16, 0), (16, 16)], // BLOCK_128X128
        2 => &[(0, 16)],                    // BLOCK_128X64: right
        3 => &[(16, 0)],                    // BLOCK_64X128: below
        _ => &[],
    }
}

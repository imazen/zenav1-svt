//! SB128 — geometry tables + the `super_block_size` selection rule
//! (task #91).
//!
//! Source translation per docs/sb128-port-map.md. Wired at `lib.rs`; the
//! geometry helpers below still have no encode-path consumers (chunk 4+),
//! but [`derive_super_block_size`] IS live — `pipeline::EncodePipeline`
//! calls it to decide the SB grid, exactly like C.
//!
//! THE architectural fact: SVT keeps TWO grids — a b64 grid ALWAYS 64x64
//! (ME/variance/per-64 stats) and the sb grid following super_block_size.
//! SB128 code exists to bridge them.

// ---------------------------------------------------------------------------
// super_block_size selection (C Globals/enc_handle.c:4071-4111)
// ---------------------------------------------------------------------------

/// C `INPUT_SIZE_240p_TH` (Codec/definitions.h:1834) — 0x28500 = 165,120
/// luma samples. Frames strictly below this are `INPUT_SIZE_240p_RANGE`.
pub const INPUT_SIZE_240P_TH: u64 = 0x28500;

/// C `INPUT_SIZE_360p_TH` (Codec/definitions.h:1835) — 0x4CE00 = 315,392.
pub const INPUT_SIZE_360P_TH: u64 = 0x4CE00;

/// C `ResolutionRange` (Codec/definitions.h:1824-1832), only the two
/// buckets the SB-size rule reads. `svt_aom_derive_input_resolution`
/// (Codec/sequence_control_set.c:120-136) classifies on `max_input_luma_
/// width * max_input_luma_height` — the **8-ALIGNED** dims, because
/// enc_handle.c:3920 folds `max_input_pad_right/bottom` into those fields
/// before the resolution is derived at :3992. Verified empirically: a
/// 404x404 request (true 163,216 < TH) encodes with SB128 because it pads
/// to 408x408 = 166,464 >= TH, while 512x320 = 163,840 (already 8-aligned)
/// stays SB64.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolutionRange {
    P240,
    P360,
    /// 480p and above — the rule only needs "> 360p".
    Above360,
}

/// C `svt_aom_derive_input_resolution` (Codec/sequence_control_set.c:120),
/// collapsed to the three buckets the SB-size rule distinguishes.
/// `aligned_w`/`aligned_h` are the 8-rounded encode dims.
pub fn derive_input_resolution(aligned_w: usize, aligned_h: usize) -> ResolutionRange {
    let n = (aligned_w as u64) * (aligned_h as u64);
    if n < INPUT_SIZE_240P_TH {
        ResolutionRange::P240
    } else if n < INPUT_SIZE_360P_TH {
        ResolutionRange::P360
    } else {
        ResolutionRange::Above360
    }
}

/// The non-preset inputs to the SB-size rule that the port does not model
/// as first-class config yet. Every field defaults to the value the
/// identity harness's C oracle actually uses (`capture_c_trace.c:149-173`
/// leaves them at `svt_av1_enc_init_handle` defaults), so
/// `SbSizeInputs::default()` reproduces the oracle exactly.
#[derive(Clone, Copy, Debug)]
pub struct SbSizeInputs {
    /// C `static_config.fast_decode` (default 0).
    pub fast_decode: bool,
    /// C `static_config.qp` — CLI domain 0..63.
    pub qp: u8,
    /// C `static_config.resize_mode > RESIZE_NONE` (default false).
    pub resize: bool,
    /// C `static_config.rtc` (default false).
    pub rtc: bool,
    /// C `static_config.enable_variance_boost`. **mainline v4.2 default is
    /// false** (enc_settings.c:1152); the HDR fork defaults it TRUE
    /// (:1150), which forces SB64 — so an sb128 x fork cell needs
    /// `SVT_FORK_ENABLE_VARIANCE_BOOST=0`.
    pub variance_boost: bool,
    /// C `static_config.sframe_dist != 0 || sframe_posi.sframe_posis`.
    pub sframe: bool,
    /// C `scs->allintra` (enc_handle.c:507-518) — set by `avif = true`,
    /// which the identity oracle passes.
    pub allintra: bool,
}

impl Default for SbSizeInputs {
    fn default() -> Self {
        SbSizeInputs {
            fast_decode: false,
            qp: 32,
            resize: false,
            rtc: false,
            variance_boost: false,
            sframe: false,
            allintra: true,
        }
    }
}

/// C `super_block_size` derivation, Globals/enc_handle.c:4071-4111 —
/// transcribed branch for branch. Returns 64 or 128 (PIXELS).
///
/// `enc_mode` is the SIGNED preset (C `ENC_MR = -1`, `ENC_M0 = 0`, ...,
/// EbSvtAv1Enc.h:44-56), so the `<= ENC_M1` / `<= ENC_MR` comparisons keep
/// their C meaning for the research modes.
///
/// **The two facts that decide every current gate cell:**
/// 1. `INPUT_SIZE_240p_RANGE` (aligned area < 165,120) forces 64
///    UNCONDITIONALLY — so every existing harness cell (largest is
///    256x256 = 65,536) is SB64 no matter the preset. SB128 needs a frame
///    of at least ~165,120 aligned luma samples (e.g. 512x384 = 196,608).
/// 2. In the `allintra` branch only `enc_mode <= ENC_M1` picks 128 — so
///    presets 2..13 are SB64 at every frame size. Only presets 0 and 1
///    (and the negative research modes) can reach SB128 in allintra.
pub fn derive_super_block_size(
    aligned_w: usize,
    aligned_h: usize,
    enc_mode: i8,
    inputs: &SbSizeInputs,
) -> usize {
    const ENC_MR: i8 = -1;
    const ENC_M1: i8 = 1;
    const ENC_M5: i8 = 5;

    let res = derive_input_resolution(aligned_w, aligned_h);
    let mut sb = if (inputs.fast_decode && inputs.qp <= 56 && res == ResolutionRange::Above360)
        || inputs.resize
        || inputs.rtc
        || res == ResolutionRange::P240
        || inputs.variance_boost
    {
        64
    } else if inputs.allintra {
        if enc_mode <= ENC_M1 { 128 } else { 64 }
    } else if enc_mode <= ENC_MR {
        128
    } else if enc_mode <= ENC_M5 {
        if inputs.qp <= 57 { 64 } else { 128 }
    } else {
        64
    };
    // "When switch frame is on, all renditions must have same super block
    // size. See spec 5.5.1, 5.9.15." (enc_handle.c:4095-4098)
    if inputs.sframe {
        sb = 64;
    }
    sb
}

/// The **b64 coding units** of one superblock, in C's coding order.
///
/// THE architectural fact (map §1) made operational: SVT's b64 grid is
/// ALWAYS 64x64 while the sb grid follows `super_block_size`, so the
/// per-64 machinery (variance map, PD0 tree, leaf funnel, recon) is the
/// same at both sizes — only the *visiting order* and the extra root
/// partition symbol differ.
///
/// - SB64: exactly one unit, the SB itself. Callers are byte-identical to
///   the pre-SB128 code by construction.
/// - SB128: up to four b64 quadrants in **Z-order** (the AV1
///   PARTITION_SPLIT child order, spec 5.11.4 / C `svt_aom_write_modes_sb`),
///   with quadrants whose top-left is at/after the ALIGNED frame extent
///   DROPPED — exactly C's `mi_row + y_idx >= mi_rows || mi_col + x_idx >=
///   mi_cols` `continue`. Those quadrants code nothing at all.
///
/// `aligned_w`/`aligned_h` are the 8-aligned encode dims (the spec-5.11.4
/// predicate grid), NOT the true dims.
pub fn sb_coding_units(
    sb_x: usize,
    sb_y: usize,
    sb_size: usize,
    aligned_w: usize,
    aligned_h: usize,
) -> alloc::vec::Vec<(usize, usize)> {
    debug_assert!(sb_size == 64 || sb_size == 128);
    if sb_size == 64 {
        return alloc::vec![(sb_x, sb_y)];
    }
    let mut out = alloc::vec::Vec::with_capacity(4);
    for i in 0..4usize {
        let x = sb_x + (i & 1) * 64;
        let y = sb_y + (i >> 1) * 64;
        if x >= aligned_w || y >= aligned_h {
            continue;
        }
        out.push((x, y));
    }
    out
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value below is a MEASURED verdict from the real C encoder
    /// (`Bin/Release/SvtAv1EncApp`, v4.2.0, mainline defaults), read back
    /// out of the emitted sequence header's `use_128x128_superblock` bit —
    /// not a transcription guess. Harness config = the identity oracle's
    /// (`avif = true` -> allintra, `rate_control_mode = 0`, variance boost
    /// off, no resize/rtc/sframe/fast-decode).
    #[test]
    fn derive_super_block_size_matches_measured_c() {
        let d = SbSizeInputs::default();
        // MEASURED: 512x384 = 196,608 px (>= 240p TH) at preset 0 and 1
        // emits use_128x128_superblock=1; preset 2 and 3 emit 0.
        assert_eq!(derive_super_block_size(512, 384, 0, &d), 128);
        assert_eq!(derive_super_block_size(512, 384, 1, &d), 128);
        assert_eq!(derive_super_block_size(512, 384, 2, &d), 64);
        assert_eq!(derive_super_block_size(512, 384, 3, &d), 64);
        // MEASURED: 256x256 = 65,536 px at preset 0 emits 0 — the 240p
        // clause wins over the allintra M0 clause. This is why every
        // pre-existing harness cell is SB64.
        assert_eq!(derive_super_block_size(256, 256, 0, &d), 64);
        // MEASURED threshold pair, both at preset 0:
        //   512x320 = 163,840 (< 165,120) -> 0
        //   512x336 = 172,032 (>= 165,120) -> 1
        assert_eq!(derive_super_block_size(512, 320, 0, &d), 64);
        assert_eq!(derive_super_block_size(512, 336, 0, &d), 128);
        // MEASURED: a 404x404 REQUEST emits 1 even though 404*404 =
        // 163,216 < TH — C classifies on the 8-ALIGNED dims (408x408 =
        // 166,464). Callers must pass aligned dims; this asserts the
        // aligned value is the one that flips.
        assert_eq!(derive_super_block_size(408, 408, 0, &d), 128);
        assert_eq!(derive_super_block_size(404, 404, 0, &d), 64);
    }

    #[test]
    fn derive_super_block_size_force_64_clauses() {
        let base = SbSizeInputs::default();
        // Each force-64 clause independently overrides the allintra-M0 128.
        for f in [
            SbSizeInputs { variance_boost: true, ..base },
            SbSizeInputs { rtc: true, ..base },
            SbSizeInputs { resize: true, ..base },
            SbSizeInputs { sframe: true, ..base },
        ] {
            assert_eq!(derive_super_block_size(512, 384, 0, &f), 64, "{f:?}");
        }
        // fast_decode only forces 64 ABOVE 360p — at 512x384 (360p range)
        // the `!(res <= 360p)` guard makes it inert.
        let fd = SbSizeInputs { fast_decode: true, qp: 32, ..base };
        assert_eq!(derive_super_block_size(512, 384, 0, &fd), 128);
        // 640x512 = 327,680 >= 360p TH -> the fast_decode clause fires.
        assert_eq!(derive_super_block_size(640, 512, 0, &fd), 64);
        // ... but only while qp <= 56.
        let fd57 = SbSizeInputs { fast_decode: true, qp: 57, ..base };
        assert_eq!(derive_super_block_size(640, 512, 0, &fd57), 128);
    }

    #[test]
    fn derive_super_block_size_non_allintra_branches() {
        let inter = SbSizeInputs { allintra: false, ..SbSizeInputs::default() };
        // enc_mode <= ENC_MR (-1) -> 128
        assert_eq!(derive_super_block_size(512, 384, -1, &inter), 128);
        // M0..M5 -> qp-dependent (<= 57 -> 64)
        assert_eq!(derive_super_block_size(512, 384, 0, &inter), 64);
        let hiq = SbSizeInputs { qp: 58, ..inter };
        assert_eq!(derive_super_block_size(512, 384, 5, &hiq), 128);
        // > M5 -> 64
        assert_eq!(derive_super_block_size(512, 384, 6, &hiq), 64);
    }

    #[test]
    fn resolution_buckets_match_c_thresholds() {
        assert_eq!(derive_input_resolution(512, 320), ResolutionRange::P240);
        assert_eq!(derive_input_resolution(512, 336), ResolutionRange::P360);
        // 315,392 is the 360p TH: 640x496 = 317,440 is above it.
        assert_eq!(derive_input_resolution(640, 496), ResolutionRange::Above360);
    }

    #[test]
    fn sb_header_params_match_c() {
        assert_eq!(sb_header_params(64), (false, 16, 4, 6));
        assert_eq!(sb_header_params(128), (true, 32, 5, 7));
    }

    #[test]
    fn partition_alphabet_len_matches_c() {
        assert_eq!(partition_cdf_length(8), 4);
        assert_eq!(partition_cdf_length(16), 10);
        assert_eq!(partition_cdf_length(64), 10);
        assert_eq!(partition_cdf_length(128), 8);
    }
}

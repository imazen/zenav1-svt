//! Segmentation: aq-mode-1 variance binning and ROI-map segment assignment.
//!
//! Wholesale translation of `Source/Lib/Codec/segmentation.c` (all 315 lines)
//! plus the three consumers that read `SegmentationParams` outside that file:
//! the per-block quantizer offset (`full_loop.c:1671-1673`,
//! `coding_loop.c:330-332`), the per-segment loop-filter level
//! (`deblocking_common.c:118-131`), and the lossless/`coded_lossless`
//! derivation (`md_config_process.c:992-1009`).
//!
//! # Reachability (why this is a capability gap, not dead code)
//!
//! C reaches enabled segmentation on a still KEY frame two ways:
//! * `svt_aom_setup_segmentation` (segmentation.c:228) sets
//!   `segmentation_enabled = (aq_mode == 1)` with **no slice-type gate**, and
//!   is called from the AOM_Q (CRF/CQP) branch of the rate controller
//!   (`rc_process.c:851-852`);
//! * `roi_map_setup_segmentation` (segmentation.c:160) enables it
//!   unconditionally whenever an ROI-map event is attached
//!   (`resource_coordination_process.c:918` binds `ppcs->roi_map_evt`).
//!
//! # Wiring status
//!
//! **NOT wired into the encoder.** The five frame-header writer sites still
//! emit a hardcoded `segmentation_enabled = 0`, and the port has no config
//! surface for `aq-mode 1` or an ROI map. Flipping those on before the
//! decision layer consumes `segment_id` would emit syntax nothing decides,
//! which is strictly worse than leaving it off. This module is the
//! translation those sites will consume once the config surface lands.
//!
//! # C quirks reproduced bug-for-bug (each is commented at its site)
//!
//! * `ROUND(a)` (utility.h:110) expands to `(a >= 0) ? (a + 1/2) : (a - 1/2)`
//!   — `1/2` is INTEGER division, so the macro is the identity.
//! * `find_segment_qps` accumulates `avg_var` in a `uint16_t` (wrapping) and
//!   stores `POW2(bin_edge)` into an `int16_t` (truncating).
//! * `get_variance_for_cu`'s 2:1 cases pick their second sample with a
//!   dimension-swapped / origin-scaled index.

use svtav1_types::block::BlockSize;
use svtav1_types::restoration::{MAX_LOOP_FILTER, MAX_SEGMENTS};
use svtav1_types::segmentation::{
    SEG_LVL_ALT_LF_U, SEG_LVL_ALT_LF_V, SEG_LVL_ALT_LF_Y_H, SEG_LVL_ALT_LF_Y_V, SEG_LVL_ALT_Q,
    SEG_LVL_LF_LUT, SEG_LVL_MAX, SEG_LVL_REF_FRAME, SegmentationParams,
};

/// C `ME_TIER_ZERO_PU_64x64` (me_context.h:47).
pub const ME_TIER_ZERO_PU_64X64: usize = 0;
/// C `ME_TIER_ZERO_PU_32x32_0` (me_context.h:48).
pub const ME_TIER_ZERO_PU_32X32_0: usize = 1;
/// C `ME_TIER_ZERO_PU_16x16_0` (me_context.h:52).
pub const ME_TIER_ZERO_PU_16X16_0: usize = 5;
/// C `ME_TIER_ZERO_PU_8x8_0` (me_context.h:68).
pub const ME_TIER_ZERO_PU_8X8_0: usize = 21;
/// C `ME_TIER_ZERO_PU_8x8_63` (me_context.h:131).
pub const ME_TIER_ZERO_PU_8X8_63: usize = 84;

/// Per-b64 variance-array length C allocates when aq-mode 1 (or all-intra, or
/// variance-octile) is on: `block_count = 85` (`pcs.c:1273-1280`), i.e.
/// exactly `ME_TIER_ZERO_PU_8x8_63 + 1`.
pub const VARIANCE_BLOCK_COUNT: usize = ME_TIER_ZERO_PU_8X8_63 + 1;

/// C `block_size_wide[BLOCK_SIZES_ALL]` (common_utils.c:286-287), in pixels.
const BLOCK_SIZE_WIDE: [i32; BlockSize::SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
/// C `block_size_high[BLOCK_SIZES_ALL]` (common_utils.c:289-290), in pixels.
const BLOCK_SIZE_HIGH: [i32; BlockSize::SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// C `svt_log2f_safe(x)` = `get_msb((x) | 1)` (definitions.h:612, :617-620):
/// the index of the most significant set bit, with the `| 1` making `x == 0`
/// yield 0 instead of the undefined `clz(0)`.
#[inline]
pub fn log2f_safe(x: u32) -> u32 {
    31 - (x | 1).leading_zeros()
}

/// C `SVT_VAR_AVG2(a, b)` — the MAINLINE (`SVT_HDR_MODE == 0`) spelling
/// `((a) + (b)) >> 1` on `SvtVarType = uint16_t` (definitions.h:240-242).
///
/// The sum is computed in `int` in C (integer promotion), so it cannot
/// overflow; the result is stored back into a `uint16_t`.
#[inline]
fn var_avg2(a: u16, b: u16) -> u16 {
    ((u32::from(a) + u32::from(b)) >> 1) as u16
}

// ============================================================================
// segmentation.c:23-84 — get_variance_for_cu
// ============================================================================

/// C `get_variance_for_cu` (segmentation.c:23-84).
///
/// `org_x`/`org_y` are the block origin **relative to the SB origin**.
/// The comment in C says "Assumes max CU size is 64".
///
/// # `variance` is a PLANE slice, not one 85-entry row
///
/// C's `variance_ptr` is `pcs->ppcs->variance[sb_index]`, which points INTO
/// a single contiguous allocation: `EB_MALLOC_2D` (svt_malloc.h:275-281) does
/// one `EB_MALLOC_ARRAY(p2d[0], width * height)` and then
/// `p2d[w] = p2d[0] + w * height`. That matters because C's `BLOCK_16X8` arm
/// indexes PAST the 85-entry row (see below), landing in the NEXT b64's
/// samples. So the caller must pass the contiguous plane **from this b64's
/// row onward** (`&plane[sb_index * VARIANCE_BLOCK_COUNT..]`), not a copy of
/// the single row — anything else silently changes the value C reads.
///
/// For the LAST b64 in the frame that same C read runs past the end of the
/// whole allocation: a genuine heap over-read (undefined behaviour) in C,
/// not reproducible by construction. This port indexes a bounds-checked
/// slice, so the equivalent call PANICS instead of returning garbage —
/// loudly wrong beats silently wrong, and see the PORT-NOTE below.
///
/// PORT-NOTE(unverified): the last-b64 BLOCK_16X8 over-read
/// (segmentation.c:41, `index1 = index0 + org_y`) is UB in C and therefore
/// has no defined value to match. It is unreachable today (nothing calls
/// this), and a wiring pass must decide the policy — the honest options are
/// to over-allocate the variance plane by one row (matching what C's
/// allocator happens to give it in-bounds) or to clamp with a comment. To
/// verify what C actually reads there, instrument `get_variance_for_cu` in
/// a scratch C build and dump `index1` for every 16x8 leaf of the last b64.
///
/// # Index derivations
///
/// For the SQUARE cases the origin scaling is correct — 8x8 uses
/// `(org_x >> 3) + org_y`, and since `org_y` is a multiple of 8 that equals
/// `(org_x >> 3) + 8 * (org_y >> 3)`, the right raster index into the 8-wide
/// 8x8 grid. The 2:1 cases are NOT: `BLOCK_8X16` (8 wide, 16 high — two 8x8s
/// stacked VERTICALLY) takes `index1 = index0 + 1`, its HORIZONTAL
/// neighbour, while `BLOCK_16X8` (two 8x8s side by side) takes
/// `index1 = index0 + org_y`, i.e. `index0` itself at the top SB row and an
/// out-of-row index below it. The 16x32/32x16 and 32x64/64x32 pairs have the
/// same swap (16x32 → `+1`, 32x16 → `+(org_y >> 2)`, 32x64 → `+1`, 64x32 →
/// `+(org_y >> 4)`); of those only 32x16 and 64x32 stay inside the row.
/// Reproduced verbatim — C's behaviour is what an oracle comparison must
/// match.
pub fn get_variance_for_cu(bsize: BlockSize, org_x: i32, org_y: i32, variance: &[u16]) -> u16 {
    use BlockSize::*;
    let (index0, index1): (i32, i32) = match bsize {
        Block4x4 | Block4x8 | Block8x4 | Block8x8 => {
            let i = ME_TIER_ZERO_PU_8X8_0 as i32 + ((org_x >> 3) + org_y);
            (i, i)
        }
        Block8x16 => {
            let i = ME_TIER_ZERO_PU_8X8_0 as i32 + ((org_x >> 3) + org_y);
            (i, i + 1)
        }
        Block16x8 => {
            let i = ME_TIER_ZERO_PU_8X8_0 as i32 + ((org_x >> 3) + org_y);
            (i, i + org_y)
        }
        Block4x16 | Block16x4 | Block16x16 => {
            let i = ME_TIER_ZERO_PU_16X16_0 as i32 + ((org_x >> 4) + (org_y >> 2));
            (i, i)
        }
        Block16x32 => {
            let i = ME_TIER_ZERO_PU_16X16_0 as i32 + ((org_x >> 4) + (org_y >> 2));
            (i, i + 1)
        }
        Block32x16 => {
            let i = ME_TIER_ZERO_PU_16X16_0 as i32 + ((org_x >> 4) + (org_y >> 2));
            (i, i + (org_y >> 2))
        }
        Block8x32 | Block32x8 | Block32x32 => {
            let i = ME_TIER_ZERO_PU_32X32_0 as i32 + ((org_x >> 5) + (org_y >> 4));
            (i, i)
        }
        Block32x64 => {
            let i = ME_TIER_ZERO_PU_32X32_0 as i32 + ((org_x >> 5) + (org_y >> 4));
            (i, i + 1)
        }
        Block64x32 => {
            let i = ME_TIER_ZERO_PU_32X32_0 as i32 + ((org_x >> 5) + (org_y >> 4));
            (i, i + (org_y >> 4))
        }
        // C's `case BLOCK_64X64: case BLOCK_16X64: case BLOCK_64X16: default:`
        // — index 0 is ME_TIER_ZERO_PU_64x64, the whole-b64 variance. The
        // 128-wide sizes land here too (they are unreachable at sb64).
        _ => (0, 0),
    };
    var_avg2(variance[index0 as usize], variance[index1 as usize])
}

// ============================================================================
// segmentation.c:249-260 — calculate_segmentation_data
// ============================================================================

/// C `calculate_segmentation_data` (segmentation.c:249-260).
///
/// Derives `last_active_seg_id` (the highest segment id with ANY enabled
/// feature) and `seg_id_pre_skip` (set once any feature at or above
/// `SEG_LVL_REF_FRAME` is enabled — those change how `skip` is parsed, so the
/// segment id has to come first).
///
/// Bug-for-bug: neither output is CLEARED first, so this accumulates over
/// repeated calls on the same struct. C relies on the frame header being
/// zeroed per picture.
pub fn calculate_segmentation_data(seg: &mut SegmentationParams) {
    for i in 0..MAX_SEGMENTS {
        for j in 0..SEG_LVL_MAX {
            if seg.feature_enabled[i][j] != 0 {
                seg.last_active_seg_id = i as u8;
                if j >= SEG_LVL_REF_FRAME {
                    seg.seg_id_pre_skip = 1;
                }
            }
        }
    }
}

// ============================================================================
// segmentation.c:262-315 — find_segment_qps
// ============================================================================

/// C `find_segment_qps` (segmentation.c:262-315).
///
/// Bins the frame's per-8x8 ME variances (log2 domain) into `MAX_SEGMENTS`
/// buckets and assigns each bucket a qindex OFFSET proportional to its
/// distance from the frame's average log-variance. `variance` is
/// `pcs->ppcs->variance` — one [`VARIANCE_BLOCK_COUNT`]-entry row per b64,
/// `b64_total_count` rows.
///
/// Four C behaviours are load-bearing and reproduced verbatim:
/// 1. `avg_var` is a `uint16_t` accumulating `local_avg >> 6` per b64, so it
///    WRAPS mod 65536 on large frames.
/// 2. `strength` is `const float strength = 2` cast to `uint16_t` — the
///    multiply is exact integer 2.
/// 3. `ROUND(a)` (utility.h:110) is `(a >= 0) ? (a + 1/2) : (a - 1/2)`, and
///    `1/2` is integer division — the macro is the IDENTITY. It is applied to
///    both `step_size` and the per-segment offset, and rounds nothing.
/// 4. `variance_bin_edge` is `int16_t` while `POW2(bin_edge)` is `1 <<
///    bin_edge` in `int` — bin edges at or above bit 15 TRUNCATE (and can go
///    negative), which then makes the `variance <= edge` test in
///    [`apply_segmentation_based_quantization`] fail for that bucket.
///
/// Panics if `b64_total_count == 0` (C would divide by zero).
pub fn find_segment_qps(
    seg: &mut SegmentationParams,
    variance: &[[u16; VARIANCE_BLOCK_COUNT]],
    b64_total_count: u32,
) {
    assert!(b64_total_count > 0, "C divides avg_var by b64_total_count");
    debug_assert!(variance.len() >= b64_total_count as usize);

    // C: uint16_t min_var = UINT16_MAX, max_var = MIN_UNSIGNED_VALUE (= 0,
    // utility.h:162), avg_var = 0.
    let mut min_var: u16 = u16::MAX;
    let mut max_var: u16 = 0;
    let mut avg_var: u16 = 0;

    for sb_idx in 0..b64_total_count as usize {
        let variance_ptr = &variance[sb_idx];
        // C: uint32_t local_avg = 0; loop over the 64 8x8 variances.
        let mut local_avg: u32 = 0;
        for var_index in ME_TIER_ZERO_PU_8X8_0..=ME_TIER_ZERO_PU_8X8_63 {
            let v = variance_ptr[var_index];
            max_var = max_var.max(v);
            min_var = min_var.min(v);
            local_avg += u32::from(v);
        }
        // C: `avg_var += (local_avg >> 6);` on a uint16_t — WRAPS.
        avg_var = avg_var.wrapping_add((local_avg >> 6) as u16);
    }
    avg_var /= b64_total_count as u16;
    avg_var = log2f_safe(u32::from(avg_var)) as u16;

    let min_var_log = log2f_safe(u32::from(min_var)) as u16;
    let max_var_log = log2f_safe(u32::from(max_var)) as u16;

    // C: `(uint16_t)(max_var_log - min_var_log) <= MAX_SEGMENTS ? 1
    //     : ROUND((max_var_log - min_var_log) / MAX_SEGMENTS)`.
    // The subtraction happens after integer promotion to `int`, then the cast
    // back to uint16_t wraps if max < min (it never is: max_var >= min_var).
    // ROUND is the identity (see the doc comment), so this is a plain /8.
    let diff = (max_var_log as i32 - min_var_log as i32) as u16;
    let step_size: u16 = if diff <= MAX_SEGMENTS as u16 {
        1
    } else {
        diff / MAX_SEGMENTS as u16
    };

    let mut bin_edge: u16 = min_var_log.wrapping_add(step_size);
    let mut bin_center: u16 = bin_edge >> 1;

    // C: `for (int i = MAX_SEGMENTS - 1; i >= 0; i--)` — segment 7 gets the
    // LOWEST bin edge, segment 0 the highest.
    for i in (0..MAX_SEGMENTS).rev() {
        // C: `POW2(bin_edge)` = `1 << bin_edge` in `int`, stored to int16_t.
        // `bin_edge` can exceed 15 (min_var_log <= 15 plus 8 steps), so this
        // truncates — reproduced with a wrapping shift + `as i16`.
        let pow2 = 1i32.wrapping_shl(u32::from(bin_edge));
        seg.variance_bin_edge[i] = pow2 as i16;
        // C: `ROUND((uint16_t)strength * (MAX(1, bin_center) - avg_var))`
        // with strength == 2. Both operands promote to `int`; the product can
        // be negative (a low-variance segment gets a negative qindex delta).
        let center = core::cmp::max(1i32, i32::from(bin_center));
        let offset = 2i32 * (center - i32::from(avg_var));
        seg.feature_data[i][SEG_LVL_ALT_Q] = offset as i16;
        bin_edge = bin_edge.wrapping_add(step_size);
        bin_center = bin_center.wrapping_add(step_size);
    }
    // C: segment 0 carries the largest positive offset; if even THAT is
    // negative the frame would go lossless there, which SVT cannot encode.
    if seg.feature_data[0][SEG_LVL_ALT_Q] < 0 {
        seg.feature_data[0][SEG_LVL_ALT_Q] = 0;
    }
}

// ============================================================================
// segmentation.c:228-247 — svt_aom_setup_segmentation (non-ROI arm)
// ============================================================================

/// C `svt_aom_setup_segmentation` (segmentation.c:228-247), the arm taken
/// when `ppcs->roi_map_evt == NULL`.
///
/// `aq_mode` is `scs->static_config.aq_mode`; segmentation is enabled iff it
/// is exactly 1. Note there is NO slice-type gate — a still KEY frame at
/// `--aq-mode 1` takes this path.
///
/// When enabled C always signals both updates ("always updating for now",
/// segmentation.c:236-237) and never the temporal one (the `//!frame_is_
/// intra_only(...)` in C is commented out — see
/// [`crate::segmentation`]'s note on the temporal arm).
pub fn setup_segmentation(
    seg: &mut SegmentationParams,
    aq_mode: u8,
    variance: &[[u16; VARIANCE_BLOCK_COUNT]],
    b64_total_count: u32,
) {
    seg.segmentation_enabled = aq_mode == 1;
    if seg.segmentation_enabled {
        seg.segmentation_update_data = true;
        seg.segmentation_update_map = true;
        seg.segmentation_temporal_update = false;
        find_segment_qps(seg, variance, b64_total_count);
        for i in 0..MAX_SEGMENTS {
            seg.feature_enabled[i][SEG_LVL_ALT_Q] = 1;
        }
        calculate_segmentation_data(seg);
    }
}

// ============================================================================
// segmentation.c:136-158 — svt_aom_apply_segmentation_based_quantization
// ============================================================================

/// C `svt_aom_apply_segmentation_based_quantization` (segmentation.c:136-158),
/// the arm taken when `ppcs->roi_map_evt == NULL`. Returns the block's
/// `segment_id`.
///
/// Walks segments from 7 down to 0 and takes the first whose bin edge covers
/// the block variance AND whose resulting qindex stays > 0 ("Avoid lossless
/// since SVT-AV1 doesn't support it"). Falls through to 0 if none qualifies —
/// C initializes `blk_ptr->segment_id = 0` before the loop.
///
/// `base_q_idx` is `frm_hdr.quantization_params.base_q_idx`. `variance` is
/// the contiguous variance plane FROM this b64's row onward — see
/// [`get_variance_for_cu`]'s doc for why a single-row copy is not
/// equivalent.
pub fn apply_segmentation_based_quantization(
    seg: &SegmentationParams,
    variance: &[u16],
    bsize: BlockSize,
    org_x: i32,
    org_y: i32,
    base_q_idx: i32,
) -> u8 {
    let variance_val = get_variance_for_cu(bsize, org_x, org_y, variance);
    let mut segment_id: u8 = 0;
    for i in (0..MAX_SEGMENTS).rev() {
        // C compares `uint16_t variance <= int16_t variance_bin_edge[i]`;
        // both promote to `int`, so a TRUNCATED-NEGATIVE bin edge (see
        // find_segment_qps) can never match.
        if i32::from(variance_val) <= i32::from(seg.variance_bin_edge[i]) {
            let q_index = base_q_idx + i32::from(seg.feature_data[i][SEG_LVL_ALT_Q]);
            if q_index > 0 {
                segment_id = i as u8;
                break;
            }
        }
    }
    segment_id
}

// ============================================================================
// segmentation.c:87-134 — roi_map_apply_segmentation_based_quantization
// ============================================================================

/// A frame's attached ROI-map event — C `SvtAv1RoiMapEvt`
/// (`Source/API/EbSvtAv1.h:268-274`), minus the list linkage and the
/// picture-number trigger (both are scheduling state, not segmentation math).
#[derive(Clone, Copy, Debug)]
pub struct RoiMapEvt<'a> {
    /// Per-b64 segment ids, row-major with stride
    /// `(scs->max_input_luma_width + 63) / 64`.
    pub b64_seg_map: &'a [u8],
    /// Per-segment qindex OFFSETS (C `seg_qp[MAX_SEGMENTS]`).
    pub seg_qp: [i16; MAX_SEGMENTS],
    /// Highest segment id the map uses (C `max_seg_id`, an `int8_t`).
    pub max_seg_id: i8,
}

/// C `roi_map_apply_segmentation_based_quantization` (segmentation.c:87-134).
/// Returns the block's `segment_id`.
///
/// `sb_org_x`/`sb_org_y` are the SB origin in the frame; `org_x`/`org_y` are
/// the block origin **relative to the SB**; `stride_b64` is
/// `(scs->max_input_luma_width + 63) / 64`.
///
/// At sb64 the SB maps to exactly one b64 cell. At sb128 C intersects the
/// block rectangle against the SB's FOUR b64 quadrants and takes the MINIMUM
/// segment id over every quadrant the block overlaps. Note the quadrant
/// origins are built from the SB origin (`sb_org_x`, `+64`, …), NOT from the
/// block, and the intersection test uses the block's absolute rectangle.
///
/// C then asserts the id changed from the `MAX_SEGMENTS` sentinel and, in
/// release builds, falls back to segment 0 — reproduced here (a block that
/// intersects nothing gets 0).
///
/// The final loop is a DOWNWARD walk from the mapped id looking for the first
/// segment whose qindex stays > 0; if segment 0 itself would be lossless the
/// loop exits without ever assigning and the block keeps the incoming
/// `segment_id` (C keeps whatever `blk_ptr->segment_id` already held — the
/// caller's previous block, which is why C's trailing assert exists). This
/// port returns 0 in that case and flags it below.
pub fn roi_map_apply_segmentation_based_quantization(
    seg: &SegmentationParams,
    roi: &RoiMapEvt<'_>,
    stride_b64: i32,
    sb_size_is_128: bool,
    sb_org_x: i32,
    sb_org_y: i32,
    bsize: BlockSize,
    org_x: i32,
    org_y: i32,
    base_q_idx: i32,
    // C's `blk_ptr->segment_id` ON ENTRY. C's downward walk only WRITES
    // `blk_ptr->segment_id` when it finds a non-lossless segment; if none
    // qualifies it falls through leaving the incoming value in place
    // (segmentation.c:121-129). Taking it as a parameter is what lets this
    // function reproduce that fall-through instead of inventing a 0.
    incoming_segment_id: u8,
) -> u8 {
    // C: `uint8_t segment_id = MAX_SEGMENTS;` — the "no intersection" sentinel.
    let mut segment_id: u8 = MAX_SEGMENTS as u8;
    if !sb_size_is_128 {
        let column_b64 = sb_org_x >> 6;
        let row_b64 = sb_org_y >> 6;
        segment_id = roi.b64_seg_map[(row_b64 * stride_b64 + column_b64) as usize];
    } else {
        // 4 b64 blocks to check intersection (segmentation.c:100-114).
        let b64_seg_columns = [sb_org_x, sb_org_x + 64, sb_org_x, sb_org_x + 64];
        let b64_seg_rows = [sb_org_y, sb_org_y, sb_org_y + 64, sb_org_y + 64];
        let blk_org_x = sb_org_x + org_x;
        let blk_org_y = sb_org_y + org_y;
        let bwidth = BLOCK_SIZE_WIDE[bsize as usize];
        let bheight = BLOCK_SIZE_HIGH[bsize as usize];
        for i in 0..4 {
            if blk_org_x < b64_seg_columns[i] + 64
                && blk_org_x + bwidth > b64_seg_columns[i]
                && blk_org_y < b64_seg_rows[i] + 64
                && blk_org_y + bheight > b64_seg_rows[i]
            {
                let column_b64 = b64_seg_columns[i] >> 6;
                let row_b64 = b64_seg_rows[i] >> 6;
                segment_id =
                    segment_id.min(roi.b64_seg_map[(row_b64 * stride_b64 + column_b64) as usize]);
            }
        }
    }
    debug_assert_ne!(
        segment_id, MAX_SEGMENTS as u8,
        "C asserts the block intersected at least one b64 cell"
    );
    if segment_id == MAX_SEGMENTS as u8 {
        // No intersection with any segment, assign to segment 0.
        segment_id = 0;
    }

    // C: `for (int i = segment_id; i >= 0; i--)` — first non-lossless wins.
    //
    // If EVERY i down to 0 is lossless, C's loop never assigns and
    // `blk_ptr->segment_id` KEEPS ITS INCOMING VALUE (segmentation.c:121-129),
    // then the trailing assert at :131-133 fires in a debug build. An earlier
    // revision of this port returned 0 there, calling it "the only defensible
    // total answer" — but C's answer is observable (release builds do not
    // assert), so the faithful total answer is the incoming id, which is what
    // this now returns.
    let mut out: u8 = incoming_segment_id;
    for i in (0..=segment_id as usize).rev() {
        let q_index = base_q_idx + i32::from(seg.feature_data[i][SEG_LVL_ALT_Q]);
        if q_index > 0 {
            out = i as u8;
            break;
        }
    }
    debug_assert!(
        base_q_idx + i32::from(seg.feature_data[out as usize][SEG_LVL_ALT_Q]) > 0,
        "segmentation.c:131-133 asserts the chosen segment is not lossless"
    );
    out
}

// ============================================================================
// segmentation.c:160-226 — roi_map_setup_segmentation
// ============================================================================

/// C `roi_map_setup_segmentation` (segmentation.c:160-226).
///
/// Enables segmentation UNCONDITIONALLY (no aq-mode gate), copies the ROI
/// map's per-segment qindex offsets into `SEG_LVL_ALT_Q`, and then derives
/// per-segment loop-filter DELTAS by re-running the q-based filter-level
/// picker at each segment's own clamped qindex and subtracting the frame's
/// levels.
///
/// `base_q_idx` is `frm_hdr.quantization_params.base_q_idx` (C reads it into
/// a `uint8_t`); `bit_depth` selects the picker's per-depth linear fit.
///
/// C calls `svt_av1_pick_filter_level_by_q(pcs, qindex, filter_level)`, whose
/// KEY-frame closed form is already ported as
/// [`crate::deblock::pick_filter_levels_key_frame`]. That port's `LfLevels`
/// is `[y_vert, y_horz, u, v]`, matching C's `filter_level[0..3]` ordering
/// here (C's `svt_av1_pick_filter_level_by_q` writes `filter_level[0]`,
/// `[1]`, `[2]=u`, `[3]=v`).
///
/// PORT-NOTE(unverified): the LF-delta half is only exercised on KEY frames
/// here, because `pick_filter_levels_key_frame` is the KEY specialization.
/// C's picker also has an inter arm (`deblocking_filter.c:1067-1096`) that
/// this port does not carry. Verify by adding an inter picker and re-running
/// this function on a non-key frame against a C dump of
/// `segmentation_params->feature_data[i][SEG_LVL_ALT_LF_*]`.
pub fn roi_map_setup_segmentation(
    seg: &mut SegmentationParams,
    roi: &RoiMapEvt<'_>,
    base_q_idx: u8,
    bit_depth: u8,
) {
    seg.segmentation_enabled = true;
    seg.segmentation_update_data = true;
    seg.segmentation_update_map = true;
    seg.segmentation_temporal_update = false;

    // C: `for (int i = 0; i <= roi_map->max_seg_id; i++)`. max_seg_id is an
    // int8_t; a negative value makes the loop body never run.
    let seg_count = if roi.max_seg_id < 0 {
        0
    } else {
        (roi.max_seg_id as usize + 1).min(MAX_SEGMENTS)
    };
    for i in 0..seg_count {
        seg.feature_enabled[i][SEG_LVL_ALT_Q] = 1;
        seg.feature_data[i][SEG_LVL_ALT_Q] = roi.seg_qp[i];
        seg.feature_enabled[i][SEG_LVL_ALT_LF_Y_V] = 1;
        seg.feature_enabled[i][SEG_LVL_ALT_LF_Y_H] = 1;
        seg.feature_enabled[i][SEG_LVL_ALT_LF_U] = 1;
        seg.feature_enabled[i][SEG_LVL_ALT_LF_V] = 1;
    }

    // setup loop filter data (segmentation.c:178-198)
    let filter_level = crate::deblock::pick_filter_levels_key_frame(base_q_idx, bit_depth).levels;
    for i in 0..seg_count {
        // C: `uint8_t qindex_seg = CLIP3(0, 255, qindex + feature_data[i][ALT_Q])`
        // — the sum is computed in `int` then narrowed after the clamp.
        let qindex_seg =
            (i32::from(base_q_idx) + i32::from(seg.feature_data[i][SEG_LVL_ALT_Q])).clamp(0, 255);
        let filter_level_seg =
            crate::deblock::pick_filter_levels_key_frame(qindex_seg as u8, bit_depth).levels;
        // Each guard re-tests feature_enabled, which the loop above just set —
        // kept as-is so the shape matches C.
        for (feat, idx) in [
            (SEG_LVL_ALT_LF_Y_V, 0usize),
            (SEG_LVL_ALT_LF_Y_H, 1),
            (SEG_LVL_ALT_LF_U, 2),
            (SEG_LVL_ALT_LF_V, 3),
        ] {
            if seg.feature_enabled[i][feat] != 0 {
                // C: `filter_level_seg[idx] - filter_level[idx]` on int32_t,
                // stored to int16_t. Both levels are 0..63, so no narrowing.
                seg.feature_data[i][feat] =
                    i16::from(filter_level_seg[idx]) - i16::from(filter_level[idx]);
            }
        }
    }
    calculate_segmentation_data(seg);
}

// ============================================================================
// Consumers outside segmentation.c
// ============================================================================

/// The per-block quantizer offset C threads into
/// `svt_aom_quantize_inv_quantize` as `segmentation_qp_offset`
/// (`coding_loop.c:330-332`, `full_loop.c:2292`, `product_coding_loop.c:4587`
/// and :5629 — every site is the same ternary).
#[inline]
pub fn seg_qp_offset(seg: &SegmentationParams, segment_id: u8) -> i32 {
    if seg.segmentation_enabled {
        i32::from(seg.feature_data[segment_id as usize][SEG_LVL_ALT_Q])
    } else {
        0
    }
}

/// C `svt_aom_quantize_inv_quantize`'s segmentation step
/// (`full_loop.c:1671-1673`): apply the offset and clamp back into
/// `[0, 255]`, but ONLY when the offset is nonzero (a zero offset must not
/// clamp an out-of-range incoming `q_index` — the guard is load-bearing).
#[inline]
pub fn apply_seg_qindex(q_index: i32, segmentation_qp_offset: i32) -> i32 {
    if segmentation_qp_offset != 0 {
        (q_index + segmentation_qp_offset).clamp(0, 255)
    } else {
        q_index
    }
}

/// C `svt_av1_loop_filter_frame_init`'s per-segment level derivation
/// (`deblocking_common.c:118-131`): start from the frame level for
/// `[plane][dir]`, add the segment's LF delta when that feature is active,
/// clamp to `[0, MAX_LOOP_FILTER]`.
///
/// `frame_level` is `filt_lvl[plane]` for `dir == 0` and `filt_lvl_r[plane]`
/// for `dir == 1` (`deblocking_common.c:101-107`: luma splits vert/horz,
/// chroma uses the same level for both directions).
///
/// The `mode_ref_delta_enabled` arm (:132-145) is NOT applied here: the C
/// encoder runs with `mode_ref_delta_enabled = 0`
/// (`resource_coordination_process.c:393`), which is also why the frame
/// header writer emits `loop_filter_delta_enabled = 0`.
pub fn segment_filter_level(
    seg: &SegmentationParams,
    plane: usize,
    dir: usize,
    frame_level: i32,
    segment_id: usize,
) -> i32 {
    debug_assert!(plane < 3 && dir < 2);
    let mut lvl_seg = frame_level;
    let feature_id = SEG_LVL_LF_LUT[plane][dir];
    if seg.feature_active(segment_id, feature_id) {
        lvl_seg = (lvl_seg + seg.seg_data(segment_id, feature_id)).clamp(0, MAX_LOOP_FILTER as i32);
    }
    lvl_seg
}

/// C `md_config_process.c:992-1017`: per-segment lossless flags, the
/// frame-level `coded_lossless` they imply, AND the two consequences C draws
/// from them. Takes `seg` by `&mut` because one of those consequences is that
/// C DISABLES segmentation.
///
/// Returns `(lossless[MAX_SEGMENTS], coded_lossless)`.
///
/// Three parts, all of which the first revision of this port got wrong or
/// omitted (found by the adversarial verification pass):
///
/// 1. **Per-segment flags** (`:994-1000`). Bug-for-bug: the sum is computed and
///    narrowed through `int16_t` TWICE in C
///    (`(int16_t)((int16_t)base_q_idx + feature_data[...])`) before the `<= 0`
///    test, so a base + offset overflowing `int16_t` wraps. `base_q_idx` is
///    0..255 and offsets are ±2·log2-range, so the wrap is unreachable in
///    practice — reproduced anyway.
///
/// 2. **The mixed-lossless auto-disable** (`:1010-1013`):
///    ```c
///    // To Do: fix the case of lossy and lossless segments in the same frame
///    if (!frm_hdr->coded_lossless && has_lossless_segment)
///        frm_hdr->segmentation_params.segmentation_enabled = 0;
///    ```
///    A frame carrying BOTH lossless and lossy segments cannot be coded, so C
///    drops segmentation for the whole frame. Omitting this left the port
///    signalling segmentation on a frame C would have turned it off for.
///
/// 3. **The segmentation-disabled arm** (`:1015-1017`):
///    ```c
///    if (!frm_hdr->segmentation_params.segmentation_enabled)
///        frm_hdr->coded_lossless = pcs->lossless[0] = !base_q_idx;
///    ```
///    NOT "leave both untouched", which is what this used to claim and do. At
///    `base_q_idx == 0` with segmentation off, C sets BOTH `coded_lossless` and
///    `lossless[0]` to TRUE — that is precisely how a plain `--qp 0` still
///    reaches the lossless path. Returning `false` there would have made a
///    future lossless wiring silently skip its own envelope. Note this arm runs
///    AFTER (2), so a frame disabled by the mixed-lossless rule falls through
///    into it, exactly as in C.
pub fn derive_lossless(
    seg: &mut SegmentationParams,
    base_q_idx: i32,
) -> ([bool; MAX_SEGMENTS], bool) {
    let mut lossless = [false; MAX_SEGMENTS];
    let mut coded_lossless = false;
    if seg.segmentation_enabled {
        let mut has_lossless_segment = false;
        for segment_id in 0..MAX_SEGMENTS {
            let sum = (base_q_idx as i16).wrapping_add(seg.feature_data[segment_id][SEG_LVL_ALT_Q]);
            lossless[segment_id] = sum <= 0;
            has_lossless_segment = has_lossless_segment || lossless[segment_id];
        }
        coded_lossless = true;
        for &l in lossless.iter() {
            if !l {
                coded_lossless = false;
                break;
            }
        }
        // md_config_process.c:1011-1013.
        if !coded_lossless && has_lossless_segment {
            seg.segmentation_enabled = false;
        }
    }
    // md_config_process.c:1015-1017 — reached both when segmentation was off to
    // begin with AND when the clause above just turned it off.
    if !seg.segmentation_enabled {
        let l0 = base_q_idx == 0;
        lossless = [false; MAX_SEGMENTS];
        lossless[0] = l0;
        coded_lossless = l0;
    }
    (lossless, coded_lossless)
}

#[cfg(test)]
mod tests;

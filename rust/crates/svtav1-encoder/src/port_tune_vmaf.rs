//! The `--tune vmaf` luma preprocessing chain — an unsharp mask applied to the
//! source picture before anything else in the encoder sees it.
//!
//! Two C files, one feature. The six leaf kernels live in
//! `Source/Lib/Codec/temporal_filtering.c:3636-3746` (RTCD-dispatched, with
//! `_c` scalar references that are EXPORTED); the nine helpers that assemble
//! them live in `Source/Lib/Codec/pic_analysis_process.c:1642-1899` and are all
//! `static`. They are kept together here because splitting a mask across its
//! own kernels helps nobody.
//!
//! ## Reachability
//!
//! `svt_aom_picture_analysis_kernel` calls `vmaf_preprocess_frame` under
//! `scs->static_config.tune == TUNE_VMAF` (pic_analysis_process.c:1957-1959)
//! and nothing else calls it. So this is live exactly when the user asks for
//! `--tune vmaf`, and byte-inert otherwise. Unlike the screen-content mode-2
//! detector next door, nothing remaps the tune value away — checked in
//! `enc_settings.c` / `enc_handle.c`: `tune` is validated (`<= 4`) and stored.
//!
//! It REWRITES THE SOURCE PLANE in place, so every later stage — statistics,
//! ME, mode decision, the residual — sees the sharpened pixels. There is no
//! way to be approximately right here.
//!
//! ## Evidence
//!
//! The six kernels are gated at tier 1 against the real exported `_c` symbols
//! (`c_parity_tune_vmaf.rs`). The nine `static` helpers are pure functions of
//! scalars or of those kernels' outputs, and are covered by driving the
//! assembled chain; where a helper is a bare threshold ladder its constants are
//! pinned and the C line is cited (tier 4, and said so).
//!
//! ## Floating point
//!
//! `gradient_coherence` is the only `f32`/`f64` arithmetic in the chain, and
//! the only library call in it is `sqrt`, which IEEE-754 requires to be
//! correctly rounded — unlike `expf`/`logf`, it cannot differ between libms.
//! (`docs/WORKING-ON-THIS.md` §5c: cross-ISA float questions need an emulator,
//! not an argument. This one needs neither, because the operation is exact.)
//! The C spells it `sqrtf((float)(double_expr))`, so the double expression is
//! rounded to `f32` BEFORE the square root; doing the root in `f64` and
//! narrowing afterwards is a different function and is not what this does.

use alloc::vec;
use alloc::vec::Vec;

use crate::temporal_filter::estimate_noise_fp16;

/// `steps_x` / `steps_y` in `vmaf_box_blur_frame` (pic_analysis_process.c:1774).
/// Both are 2, and the ring is `2 * steps_y + 1` rows deep.
pub const VMAF_STEPS: usize = 2;
/// The number of rows `vmaf_box_blur_frame` keeps live.
pub const VMAF_RING_ROWS: usize = 2 * VMAF_STEPS + 1;

// ---------------------------------------------------------------------------
// The six leaf kernels (temporal_filtering.c, EXPORTED as `_c`)
// ---------------------------------------------------------------------------

/// `svt_vmaf_compute_avg_mad_c` (temporal_filtering.c:3636).
///
/// Mean absolute deviation from the block mean, averaged over whole 8x8 blocks
/// and per pixel. Partial edge blocks are skipped (`by + 8 <= height`).
///
/// The mean is `sum >> 6`, i.e. truncated, not rounded — that is C's, and it
/// biases `mad` upward by up to 0.5 per pixel.
pub fn compute_avg_mad(src: &[u8], width: usize, height: usize, stride: usize) -> u32 {
    let mut total_activity: u64 = 0;
    let mut block_count: u64 = 0;
    let mut by = 0usize;
    while by + 8 <= height {
        let mut bx = 0usize;
        while bx + 8 <= width {
            let block = |r: usize, c: usize| u32::from(src[(by + r) * stride + bx + c]);
            let mut sum = 0u32;
            for r in 0..8 {
                for c in 0..8 {
                    sum += block(r, c);
                }
            }
            let mean = sum >> 6;
            let mut mad = 0u32;
            for r in 0..8 {
                for c in 0..8 {
                    mad += block(r, c).abs_diff(mean);
                }
            }
            total_activity += u64::from(mad);
            block_count += 1;
            bx += 8;
        }
        by += 8;
    }
    if block_count == 0 {
        return 0;
    }
    (total_activity / (block_count * 64)) as u32
}

/// `svt_vmaf_apply_unsharp_row_c` (temporal_filtering.c:3664).
///
/// `dst[j] = clamp(src[j] + ((clamp(src[j] - blur[j], +/-max_delta) * amount) >> 15))`.
/// The `>> 15` is an arithmetic shift of a possibly-negative `i32`, so it
/// rounds toward negative infinity — `wrapping_shr` on an unsigned would not
/// match.
pub fn apply_unsharp_row(
    src: &[u8],
    blur: &[u8],
    dst: &mut [u8],
    width: usize,
    amount: i32,
    max_delta: i32,
) {
    for j in 0..width {
        let detail = (i32::from(src[j]) - i32::from(blur[j])).clamp(-max_delta, max_delta);
        dst[j] = (i32::from(src[j]) + ((detail * amount) >> 15)).clamp(0, 255) as u8;
    }
}

/// `svt_vmaf_vpass_row_c` (temporal_filtering.c:3674).
///
/// The vertical half of the separable blur: a `[1, 4, 6, 4, 1]` binomial over
/// the five live `hpass` rows, offset by `2 * steps_x` because `hpass_row`
/// writes its own left margin first.
///
/// The `>> 8` (not `>> 4`) is the combined normaliser: `hpass_row` already
/// carries a factor of 16 from its own `[1, 4, 6, 4, 1]`.
///
/// C reads each `int16_t` through `(uint32_t)`, which sign-extends first. The
/// values cannot be negative — the horizontal filter's maximum is
/// `16 * 255 = 4080`, well inside `i16` — so the cast is inert, and this port
/// keeps the arithmetic in `u32` after an `i16 -> u32` conversion that would
/// reproduce the wrap if the bound were ever violated.
pub fn vpass_row(rows: [&[i16]; 5], blur_row: &mut [u8], width: usize, steps_x: usize) {
    let blur_start = 2 * steps_x;
    for (x, out) in blur_row.iter_mut().take(width).enumerate() {
        let j = x + blur_start;
        let at = |k: usize| rows[k][j] as u32;
        let v = at(0) + at(4) + 4 * (at(1) + at(3)) + 6 * at(2);
        *out = ((v + 128) >> 8) as u8;
    }
}

/// `svt_vmaf_hpass_row_c` (temporal_filtering.c:3734).
///
/// The horizontal half: two cascaded `[1, 2, 1]` stages, which compose to
/// `[1, 4, 6, 4, 1]` (gain 16). Written as a running two-tap accumulator pair
/// per stage rather than a convolution, which is what makes it one add per tap.
///
/// Edge handling is CLAMP-TO-EDGE, spelled `x <= 0 ? 0 : x >= width ? width - 1
/// : x`. The `<=` is redundant — at `x == 0` both arms read `src_row[0]` — and
/// that is not an inference: changing it to `<` is the ONE mutation in this
/// module's battery that the differential does not catch, which is the
/// measurement that it cannot matter. Transcribed as written anyway rather
/// than "simplified" to `clamp`, because C's form still reads `src_row[0]` at
/// `width == 0` where a clamp would underflow.
///
/// `h_row` must hold `width + 2 * steps_x` entries; the caller's ring rows are
/// sized for that.
pub fn hpass_row(src_row: &[u8], width: usize, h_row: &mut [i16]) {
    const STEPS_X: i32 = VMAF_STEPS as i32;
    let mut h_acc = [0u32; 4];
    for x in -STEPS_X..width as i32 + STEPS_X {
        let mut tmp1 = if x <= 0 {
            u32::from(src_row[0])
        } else if x >= width as i32 {
            u32::from(src_row[width - 1])
        } else {
            u32::from(src_row[x as usize])
        };
        for s in (0..(STEPS_X as usize) * 2).step_by(2) {
            let tmp2 = h_acc[s] + tmp1;
            h_acc[s] = tmp1;
            tmp1 = h_acc[s + 1] + tmp2;
            h_acc[s + 1] = tmp2;
        }
        h_row[(x + STEPS_X) as usize] = tmp1 as i16;
    }
}

/// `svt_vmaf_compute_gradient_coherence_c` (temporal_filtering.c:3685).
///
/// A structure-tensor coherence over 16x16 tiles, weighted by gradient energy:
/// `sum(sqrt((xx - yy)^2 + 4 xy^2)) / sum(xx + yy)`. Near 1 for oriented
/// structure (edges, text), near 0 for isotropic noise or grain.
///
/// The tile grid starts at 1 and stops at `height - 1` / `width - 1` because
/// the central-difference gradients need a one-pixel ring — so a frame under
/// 3 pixels in either dimension contributes nothing and the function returns
/// its `weight_sum <= 0` fallback of 1.0.
///
/// Accumulation is `i64` in C and stays `i64` here: at 4K a tile's `sum_xx`
/// can reach `16 * 16 * 510^2`, which fits `i32`, but the C type is what the
/// contract is.
pub fn compute_gradient_coherence(src: &[u8], width: usize, height: usize, stride: usize) -> f32 {
    let mut weighted_coh = 0.0f64;
    let mut weight_sum = 0.0f64;
    if width < 3 || height < 3 {
        return 1.0;
    }
    for by in (1..height - 1).step_by(16) {
        for bx in (1..width - 1).step_by(16) {
            let (mut sum_xx, mut sum_yy, mut sum_xy) = (0i64, 0i64, 0i64);
            let y_end = (by + 16).min(height - 1);
            let x_end = (bx + 16).min(width - 1);
            for y in by..y_end {
                for x in bx..x_end {
                    let at = |r: usize, c: usize| i32::from(src[r * stride + c]);
                    let grad_x = i64::from(at(y, x + 1) - at(y, x - 1));
                    let grad_y = i64::from(at(y + 1, x) - at(y - 1, x));
                    sum_xx += grad_x * grad_x;
                    sum_yy += grad_y * grad_y;
                    sum_xy += grad_x * grad_y;
                }
            }
            let (xx, yy, xy) = (sum_xx as f64, sum_yy as f64, sum_xy as f64);
            // C: `(double)sqrtf((float)((xx - yy) * (xx - yy) + 4.0 * xy * xy))`
            // — the f64 expression is narrowed to f32 BEFORE the root.
            weighted_coh += f64::from((((xx - yy) * (xx - yy) + 4.0 * xy * xy) as f32).sqrt());
            weight_sum += xx + yy;
        }
    }
    if weight_sum <= 0.0 {
        return 1.0;
    }
    (weighted_coh / weight_sum) as f32
}

/// `svt_vmaf_count_detail_le_c` (temporal_filtering.c:3718).
///
/// How many pixels differ from the blur by no more than `thresh`. Note the two
/// different strides: `src` uses the picture stride, `blur` is tightly packed
/// at `width`, which is how `vmaf_box_blur_frame` writes it.
pub fn count_detail_le(
    src: &[u8],
    blur: &[u8],
    width: usize,
    height: usize,
    src_stride: usize,
    thresh: i32,
) -> u32 {
    let mut match_count = 0u32;
    for y in 0..height {
        let src_row = &src[y * src_stride..];
        let blur_row = &blur[y * width..];
        for x in 0..width {
            if i32::from(src_row[x].abs_diff(blur_row[x])) <= thresh {
                match_count += 1;
            }
        }
    }
    match_count
}

// ---------------------------------------------------------------------------
// The strength ladder (pic_analysis_process.c, all `static`)
// ---------------------------------------------------------------------------

/// `vmaf_get_spatial_amount` (pic_analysis_process.c:1642). TIER 4 — `static`
/// with no exported caller that isolates it; the constants are transcribed and
/// cited.
pub fn spatial_amount(avg_mad: u32) -> f32 {
    match avg_mad {
        0..=1 => 0.15,
        2..=4 => 0.22,
        5..=11 => 0.28,
        _ => 0.30,
    }
}

/// `vmaf_get_qp_amount` (pic_analysis_process.c:1654). TIER 4.
///
/// Full strength at qp 0, easing linearly to the 0.3 floor at qp 35 and held
/// there above. C divides by the literal `35.0f`, so the whole expression is
/// `f32`; computing it in `f64` and narrowing would round differently.
pub fn qp_amount(base_qp: u32) -> f32 {
    if base_qp >= 35 {
        return 0.3;
    }
    0.5 - (base_qp as f32 / 35.0) * (0.5 - 0.3)
}

/// `vmaf_get_coherence_factor` (pic_analysis_process.c:1661). TIER 4.
///
/// Noise and grain (low coherence) get less sharpening; they cost the most
/// PSNR per unit of VMAF.
pub fn coherence_factor(gcoh: f32) -> f32 {
    if gcoh < 0.40 {
        0.80
    } else if gcoh < 0.60 {
        0.9
    } else {
        1.0
    }
}

/// `vmaf_compute_combined_amount` (pic_analysis_process.c:1671). TIER 4.
pub fn combined_amount(base_qp: u32, avg_mad: u32, gcoh: f32) -> f32 {
    let combined = (qp_amount(base_qp) + spatial_amount(avg_mad)) / 2.0;
    combined * coherence_factor(gcoh)
}

/// `vmaf_get_noise_gate` (pic_analysis_process.c:1701). TIER 4 for the gate
/// ladder; the two estimators it composes are each tier 1 elsewhere
/// (`c_parity_temporal::estimate_noise_fp16`,
/// `c_parity_temporal_filtering::noise_log1p_fp16_matches_c`).
///
/// A negative noise estimate means "too few smooth pixels to be reliable"
/// (`svt_estimate_noise_fp16_c` returns `-65536` there), and C treats that as
/// clean rather than as maximally noisy.
pub fn noise_gate(y: &[u8], width: usize, height: usize, stride: usize) -> f32 {
    let noise_fp16 = estimate_noise_fp16(y, width, height, stride);
    if noise_fp16 < 0 {
        return 1.0;
    }
    let noise_log1p = crate::port_temporal_filtering::noise_log1p_fp16(noise_fp16);

    const GATE_START: i32 = 40000;
    const GATE_END: i32 = 80000;
    const GATE_FLOOR: f32 = 0.3;

    if noise_log1p <= GATE_START {
        return 1.0;
    }
    if noise_log1p >= GATE_END {
        return GATE_FLOOR;
    }
    let t = (noise_log1p - GATE_START) as f32 / (GATE_END - GATE_START) as f32;
    1.0 - t * (1.0 - GATE_FLOOR)
}

/// `vmaf_get_delta_clip` (pic_analysis_process.c:1741). TIER 4.
///
/// The per-pixel cap on how far the mask may move a sample. Higher qp tolerates
/// more; a busy frame takes 4 less, because strong edges are where the mask
/// costs the most PSNR.
pub fn delta_clip(base_qp: i32, busy_frame: bool) -> i32 {
    let qp_delta = if base_qp <= 42 {
        8
    } else if base_qp <= 51 {
        9
    } else if base_qp <= 57 {
        10
    } else {
        12
    };
    if busy_frame { qp_delta - 4 } else { qp_delta }
}

// ---------------------------------------------------------------------------
// The frame passes
// ---------------------------------------------------------------------------

/// The five-row scratch ring `vmaf_box_blur_frame` rotates through.
///
/// C carries it as `int16_t* const hring[5]` on the analysis context, grown on
/// demand and rotated by swapping pointers. Owning it lets the rotation be a
/// slice `rotate_left` and removes the "did I resize all five?" question that
/// C's `EB_MALLOC_ARRAY_NO_CHECK` loop has to answer by hand.
#[derive(Debug, Clone)]
pub struct VmafRing {
    rows: Vec<Vec<i16>>,
}

impl VmafRing {
    /// Rows sized for `width + 2 * VMAF_STEPS`, which is what `hpass_row`
    /// writes (`padded_width` at pic_analysis_process.c:1856).
    pub fn new(width: usize) -> Self {
        Self {
            rows: vec![vec![0i16; width + 2 * VMAF_STEPS]; VMAF_RING_ROWS],
        }
    }
}

/// `vmaf_box_blur_frame` (pic_analysis_process.c:1773).
///
/// Separable, two cascaded box stages per direction. `blur` is written tightly
/// packed at `width`, NOT at the picture stride — `count_detail_le` and
/// `unsharp_apply_frame` both rely on that.
///
/// The row rotation is C's pointer shuffle expressed as `rotate_left(1)`: after
/// it, `rows[4]` is the buffer that held the oldest row and is the one the next
/// iteration overwrites, exactly as C's `oldest` is.
pub fn box_blur_frame(
    luma: &[u8],
    stride: usize,
    blur: &mut [u8],
    width: usize,
    height: usize,
    ring: &mut VmafRing,
) {
    if height == 0 || width == 0 {
        return;
    }
    let clamp_row = |row: i32| row.clamp(0, height as i32 - 1) as usize;

    for k in 0..VMAF_RING_ROWS - 1 {
        let row = clamp_row(k as i32 - VMAF_STEPS as i32);
        hpass_row(&luma[row * stride..], width, &mut ring.rows[k]);
    }

    for m in 0..height {
        let row = (m + VMAF_STEPS).min(height - 1);
        hpass_row(&luma[row * stride..], width, &mut ring.rows[4]);
        {
            let r = &ring.rows;
            vpass_row(
                [&r[0], &r[1], &r[2], &r[3], &r[4]],
                &mut blur[m * width..],
                width,
                VMAF_STEPS,
            );
        }
        ring.rows.rotate_left(1);
    }
}

/// `vmaf_unsharp_apply_frame` (pic_analysis_process.c:1815).
///
/// In C `src` and `dst` are the SAME plane at the one call site, and the row
/// kernel reads and writes each sample once at the same index, so an in-place
/// row is well defined. This signature takes one mutable plane for that reason
/// rather than pretending the two can differ.
pub fn unsharp_apply_frame(
    plane: &mut [u8],
    blur: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    sharp_amount: i32,
    delta_clip: i32,
) {
    for y in 0..height {
        let row = &mut plane[y * stride..y * stride + width];
        let blur_row = &blur[y * width..y * width + width];
        for j in 0..width {
            let detail =
                (i32::from(row[j]) - i32::from(blur_row[j])).clamp(-delta_clip, delta_clip);
            row[j] = (i32::from(row[j]) + ((detail * sharp_amount) >> 15)).clamp(0, 255) as u8;
        }
    }
}

/// What `vmaf_preprocess_frame` records on the PCS besides rewriting the plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmafPreprocess {
    /// `pcs->vmaf_sharpening_amount`, Q15.
    pub sharpening_amount: i32,
    /// `pcs->vmaf_max_delta`.
    pub max_delta: i32,
}

/// `vmaf_preprocess_frame` (pic_analysis_process.c:1842) — the whole chain.
///
/// Rewrites `luma` IN PLACE and returns the two values C stores on the PCS.
///
/// C bails out early (leaving the plane untouched and `vmaf_sharpening_amount`
/// already assigned) if either scratch allocation fails; this port owns its
/// scratch, so the only way out is the successful one. The `sharp_amount`
/// assignment happens BEFORE those allocations in C, which is why it is
/// returned even though the plane may not have been touched — that ordering is
/// preserved here for the values, not for the failure mode.
///
/// The two `(int)` casts on a `float` are C truncation toward zero. `as i32`
/// in Rust is the same for in-range values and saturates rather than being UB
/// out of range; the products here are bounded by `0.30 * 32768` so neither
/// applies.
pub fn preprocess_frame(
    luma: &mut [u8],
    stride: usize,
    width: usize,
    height: usize,
    base_qp: u32,
) -> VmafPreprocess {
    // Step 1: the per-frame sharpening amount, gated down on noisy frames.
    let avg_mad = compute_avg_mad(luma, width, height, stride);
    let gcoh = compute_gradient_coherence(luma, width, height, stride);
    let mut sharp_amount = (combined_amount(base_qp, avg_mad, gcoh) * 32768.0) as i32;
    sharp_amount = (sharp_amount as f32 * noise_gate(luma, width, height, stride)) as i32;

    // Step 2: the low-pass reference.
    let mut ring = VmafRing::new(width);
    let mut blur = vec![0u8; width * height];
    box_blur_frame(luma, stride, &mut blur, width, height, &mut ring);

    // Step 3: busy-frame flag (under 85% flat pixels) and the delta clip.
    const FLAT_DETAIL_THR: i32 = 12;
    let pixel_count = (width * height) as u32;
    // C: `pixel_count * 85 / 100` in `uint32_t`, which wraps above ~50.5 MP.
    // Reproduced rather than widened: the wrap is part of the contract, and a
    // `u64` here would disagree with the oracle on a 8K-class frame.
    let flat_pixel_target = pixel_count.wrapping_mul(85) / 100;
    let flat_pixel_count = count_detail_le(luma, &blur, width, height, stride, FLAT_DETAIL_THR);
    let is_busy_frame = flat_pixel_count < flat_pixel_target;
    let max_delta = delta_clip(base_qp as i32, is_busy_frame);

    // Step 4: the mask, in place.
    unsharp_apply_frame(luma, &blur, width, height, stride, sharp_amount, max_delta);

    VmafPreprocess {
        sharpening_amount: sharp_amount,
        max_delta,
    }
}

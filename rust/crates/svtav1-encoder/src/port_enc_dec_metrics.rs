//! Port of `Codec/enc_dec_process.c`'s **frame-quality metrics** (SSIM, PSNR)
//! and its **recode decision**.
//!
//! `enc_dec_process.c` is 3277 lines and 42 functions, and most of them are
//! not translation work at all — they are the encoder's threading, allocation
//! and segment plumbing, which this port replaces by design rather than
//! translates. [`untranslated`] is a per-function ledger of exactly which ones
//! and why, so the file's queue can be closed honestly instead of left as a
//! standing "38 missing".
//!
//! What IS translation work, and is here:
//!
//! | C function | line | here |
//! |---|---|---|
//! | `svt_aom_ssim_parms_8x8_c` | 607 | [`ssim_parms_8x8`] |
//! | `svt_aom_highbd_ssim_parms_8x8_c` | 621 | [`highbd_ssim_parms_8x8`] |
//! | `ssim_8x8` | 673 | [`ssim_8x8`] |
//! | `highbd_ssim_8x8` | 679 | [`highbd_ssim_8x8`] |
//! | `aom_ssim2` | 695 | [`ssim2`] |
//! | `aom_highbd_ssim2` | 719 | [`highbd_ssim2`] |
//! | `get_sse_10bit` | 909 | [`get_sse_10bit`] |
//! | `recode_loop_decision_maker` | 1986 | [`recode_loop_decision_maker`] |
//!
//! `svt_aom_similarity` (:645) is EXPORTED and already ported in
//! [`crate::ssim_md`]; this file's 8x8 kernels feed it.
//!
//! **EVIDENCE.** `aom_ssim2` and `aom_highbd_ssim2` survive the Release build
//! as local (`t`) symbols with their source ABI intact (prologues
//! disassembled — see `link_globalized_enc_dec_statics` in
//! `svtav1-cref/build.rs`), so they are reachable at **tier 1** through the
//! same `--globalize-symbol` promotion the rest of this lane uses, and
//! `tests/c_parity_enc_dec_metrics.rs` drives them. Each pins the whole chain
//! below it — the 8x8 parameter accumulators, `ssim_8x8` / `highbd_ssim_8x8`
//! and `svt_aom_similarity`.
//!
//! `get_sse_10bit` and `recode_loop_decision_maker` were inlined away and have
//! no symbol; they are **tier 4**, hand-derived vectors, and say so.
//!
//! **Preprocessor check** (`docs/WORKING-ON-THIS.md` §5 trap #1):
//! `grep -c 'SVT_HDR_MODE' enc_dec_process.c` is 0, so nothing here has a
//! second fork definition.

use crate::port_rc_vbr_cbr_state::{RateControl, SeqRc};

/// C `cc1` / `cc2` and their 10- and 12-bit siblings
/// (enc_dec_process.c:637-642): `(64^2 * (.01 * peak)^2)` and
/// `(64^2 * (.03 * peak)^2)`, pre-multiplied by the 64-sample window.
const CC1: i64 = 26_634;
const CC2: i64 = 239_708;
const CC1_10: i64 = 428_658;
const CC2_10: i64 = 3_857_925;
const CC1_12: i64 = 6_868_593;
const CC2_12: i64 = 61_817_334;

/// C `svt_aom_similarity` (enc_dec_process.c:645) — **EXPORTED**.
///
/// [`crate::ssim_md`] has a private 8-bit-only copy of this for the
/// tune-SSIM MD distortion; that one is deliberately specialised and is not
/// reused here, because THIS caller needs the 10- and 12-bit constant sets
/// and the `bd`-selected arm is the whole difference between them.
///
/// C's `else` arm sets `c1 = c2 = 0` and `assert(0)`; the port panics instead
/// of returning a silently meaningless score, because a zeroed stabiliser is
/// not a defined SSIM.
///
/// # Panics
/// On a bit depth outside {8, 10, 12}.
#[must_use]
pub fn similarity(
    sum_s: u32,
    sum_r: u32,
    sum_sq_s: u32,
    sum_sq_r: u32,
    sum_sxr: u32,
    count: i64,
    bd: u32,
) -> f64 {
    let (cc1, cc2) = match bd {
        8 => (CC1, CC2),
        10 => (CC1_10, CC2_10),
        12 => (CC1_12, CC2_12),
        other => panic!("svt_aom_similarity: unsupported bit depth {other}"),
    };
    // C scales the constants by the pixel count BEFORE converting to double,
    // so the `>> 12` is an integer shift and its truncation is part of the
    // result.
    let c1 = ((cc1 * count * count) >> 12) as f64;
    let c2 = ((cc2 * count * count) >> 12) as f64;
    let (sum_s, sum_r) = (f64::from(sum_s), f64::from(sum_r));
    let (sum_sq_s, sum_sq_r, sum_sxr) =
        (f64::from(sum_sq_s), f64::from(sum_sq_r), f64::from(sum_sxr));
    let count = count as f64;
    let ssim_n = (2.0 * sum_s * sum_r + c1) * (2.0 * count * sum_sxr - 2.0 * sum_s * sum_r + c2);
    let ssim_d = (sum_s * sum_s + sum_r * sum_r + c1)
        * (count * sum_sq_s - sum_s * sum_s + count * sum_sq_r - sum_r * sum_r + c2);
    ssim_n / ssim_d
}

/// C `svt_aom_ssim_parms_8x8_c` (enc_dec_process.c:607) — the five sums an
/// 8x8 SSIM window needs.
///
/// **The accumulators are `uint32_t` and C ADDS INTO them without clearing**,
/// so the caller owns the initialisation. Both callers zero them first; the
/// port takes `&mut` for the same reason rather than returning fresh values,
/// because `highbd_ssim_8x8` relies on the accumulate-into shape.
///
/// `s[j] * s[j]` is `int` arithmetic on values under 256, so it cannot
/// overflow; the sum over 64 of them cannot either (64 * 255² = 4.1e6).
pub fn ssim_parms_8x8(s: &[u8], sp: usize, r: &[u8], rp: usize, sums: &mut SsimSums) {
    for i in 0..8 {
        let srow = &s[i * sp..][..8];
        let rrow = &r[i * rp..][..8];
        for j in 0..8 {
            let sv = u32::from(srow[j]);
            let rv = u32::from(rrow[j]);
            sums.sum_s += sv;
            sums.sum_r += rv;
            sums.sum_sq_s += sv * sv;
            sums.sum_sq_r += rv * rv;
            sums.sum_sxr += sv * rv;
        }
    }
}

/// The five `uint32_t` accumulators `svt_aom_similarity` consumes. C passes
/// them as five `uint32_t*` out-params; grouping them makes a transposed pair
/// impossible at a call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SsimSums {
    pub sum_s: u32,
    pub sum_r: u32,
    pub sum_sq_s: u32,
    pub sum_sq_r: u32,
    pub sum_sxr: u32,
}

/// C `svt_aom_highbd_ssim_parms_8x8_c` (enc_dec_process.c:621).
///
/// The source is SVT's **split 10-bit layout**: `s` holds the high 8 bits and
/// `sinc` the low 2 bits packed into the TOP of a byte, so a sample is
/// `(s[j] << 2) | ((sinc[j] >> 6) & 3)`. The reference `r` is already
/// `uint16_t`.
///
/// C writes that expression as `(int64_t)(s[j] << 2) + ((sinc[j] >> 6) & 0x3)`
/// and immediately narrows it to a `uint32_t ss` — the `int64_t` cast is
/// therefore inert, and `ss * ss` is done in **32 bits**. With a maximum `ss`
/// of 1023 the square is 1.0e6 and the 64-sample sum is 6.7e7, so it fits;
/// but the cast makes it look like 64-bit arithmetic and it is not.
pub fn highbd_ssim_parms_8x8(
    s: &[u8],
    sp: usize,
    sinc: &[u8],
    spinc: usize,
    r: &[u16],
    rp: usize,
    sums: &mut SsimSums,
) {
    for i in 0..8 {
        let srow = &s[i * sp..][..8];
        let irow = &sinc[i * spinc..][..8];
        let rrow = &r[i * rp..][..8];
        for j in 0..8 {
            let ss = (u32::from(srow[j]) << 2) + u32::from((irow[j] >> 6) & 0x3);
            let rv = u32::from(rrow[j]);
            sums.sum_s += ss;
            sums.sum_r += rv;
            sums.sum_sq_s += ss * ss;
            sums.sum_sq_r += rv * rv;
            sums.sum_sxr += ss * rv;
        }
    }
}

/// C `ssim_8x8` (enc_dec_process.c:673).
#[must_use]
pub fn ssim_8x8(s: &[u8], sp: usize, r: &[u8], rp: usize) -> f64 {
    let mut sums = SsimSums::default();
    ssim_parms_8x8(s, sp, r, rp, &mut sums);
    similarity(
        sums.sum_s,
        sums.sum_r,
        sums.sum_sq_s,
        sums.sum_sq_r,
        sums.sum_sxr,
        64,
        8,
    )
}

/// C `highbd_ssim_8x8` (enc_dec_process.c:679).
///
/// The shift is applied AFTER accumulation and asymmetrically: the linear sums
/// shift by `shift`, the quadratic ones by `2 * shift`. Shifting the samples
/// before summing would give a different (and wrong) answer.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn highbd_ssim_8x8(
    s: &[u8],
    sp: usize,
    sinc: &[u8],
    spinc: usize,
    r: &[u16],
    rp: usize,
    bd: u32,
    shift: u32,
) -> f64 {
    let mut sums = SsimSums::default();
    highbd_ssim_parms_8x8(s, sp, sinc, spinc, r, rp, &mut sums);
    similarity(
        sums.sum_s >> shift,
        sums.sum_r >> shift,
        sums.sum_sq_s >> (2 * shift),
        sums.sum_sq_r >> (2 * shift),
        sums.sum_sxr >> (2 * shift),
        64,
        bd,
    )
}

/// C `aom_ssim2` (enc_dec_process.c:695).
///
/// An 8x8 window stepped on the **4x4** grid, so windows overlap and straddle
/// block boundaries — that overlap is what makes the score penalise blocking
/// artifacts, and stepping by 8 instead would be a different metric.
///
/// Returns `NaN` for a region of 8 or fewer pixels in either dimension, as C
/// does (C then `assert(samples > 0)`, which the early return makes
/// unreachable).
#[must_use]
pub fn ssim2(
    img1: &[u8],
    stride_img1: usize,
    img2: &[u8],
    stride_img2: usize,
    width: usize,
    height: usize,
) -> f64 {
    if width <= 8 || height <= 8 {
        return f64::NAN;
    }
    let mut samples = 0_u32;
    let mut ssim_total = 0.0_f64;
    let mut i = 0_usize;
    while i + 8 <= height {
        let row1 = &img1[i * stride_img1..];
        let row2 = &img2[i * stride_img2..];
        let mut j = 0_usize;
        while j + 8 <= width {
            ssim_total += ssim_8x8(&row1[j..], stride_img1, &row2[j..], stride_img2);
            samples += 1;
            j += 4;
        }
        i += 4;
    }
    ssim_total / f64::from(samples)
}

/// C `aom_highbd_ssim2` (enc_dec_process.c:719). Same 4x4-stepped window as
/// [`ssim2`], over the split 10-bit layout.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn highbd_ssim2(
    img1: &[u8],
    stride_img1: usize,
    img1inc: &[u8],
    stride_img1inc: usize,
    img2: &[u16],
    stride_img2: usize,
    width: usize,
    height: usize,
    bd: u32,
    shift: u32,
) -> f64 {
    if width <= 8 || height <= 8 {
        return f64::NAN;
    }
    let mut samples = 0_u32;
    let mut ssim_total = 0.0_f64;
    let mut i = 0_usize;
    while i + 8 <= height {
        let row1 = &img1[i * stride_img1..];
        let rowi = &img1inc[i * stride_img1inc..];
        let row2 = &img2[i * stride_img2..];
        let mut j = 0_usize;
        while j + 8 <= width {
            ssim_total += highbd_ssim_8x8(
                &row1[j..],
                stride_img1,
                &rowi[j..],
                stride_img1inc,
                &row2[j..],
                stride_img2,
                bd,
                shift,
            );
            samples += 1;
            j += 4;
        }
        i += 4;
    }
    ssim_total / f64::from(samples)
}

/// C `get_sse_10bit` (enc_dec_process.c:909). `static` and inlined — tier 4.
///
/// The same split 10-bit layout as [`highbd_ssim_parms_8x8`] but assembled
/// with a DIFFERENT expression: `(a_hi[i] << 2) | (a_lo[i] >> 6)` — an OR with
/// no `& 3` mask. That is equivalent only because `a_lo[i] >> 6` on a `uint8_t`
/// promoted to `int` is already 0..=3; the mask in the SSIM twin is redundant
/// for the same reason. Both are transcribed as written.
///
/// The difference is computed in `int` (a signed value in `-1023..=1023`) and
/// squared into an `int64_t` accumulator.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn get_sse_10bit(
    a_hi: &[u8],
    a_hi_stride: usize,
    a_lo: &[u8],
    a_lo_stride: usize,
    b: &[u16],
    b_stride: usize,
    width: usize,
    height: usize,
) -> i64 {
    let mut sse = 0_i64;
    for j in 0..height {
        let hi = &a_hi[j * a_hi_stride..][..width];
        let lo = &a_lo[j * a_lo_stride..][..width];
        let bb = &b[j * b_stride..][..width];
        for i in 0..width {
            let a = (i32::from(hi[i]) << 2) | (i32::from(lo[i]) >> 6);
            let d = a - i32::from(bb[i]);
            sse += i64::from(d) * i64::from(d);
        }
    }
    sse
}

// ---------------------------------------------------------------------------
// The recode decision
// ---------------------------------------------------------------------------

/// What [`recode_loop_decision_maker`] decided, as data rather than as five
/// scattered writes through a `PictureControlSet*`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecodeDecision {
    /// C `*do_recode`.
    pub do_recode: bool,
    /// The new `frm_hdr.quantization_params.base_q_idx`, when recoding.
    pub base_q_idx: i32,
    /// The new `ppcs->picture_qp`, when recoding.
    pub picture_qp: u8,
    /// The new `ppcs->loop_count` (incremented on a recode, reset to 0
    /// otherwise).
    pub loop_count: i32,
    /// Whether the caller must clear `delta_q_present` and re-seed every
    /// superblock's `qindex` from `base_q_idx`.
    pub reseed_sb_qindex: bool,
}

/// C `recode_loop_decision_maker` (enc_dec_process.c:1986). `static` and
/// inlined — tier 4.
///
/// Two disjoint paths, and the RTC-CBR one **returns early**: under
/// `--rc cbr --rtc` the decision comes from `svt_av1_rc_recode_decision_rtc_cbr`
/// (`rc_rtc_cbr.c`, a different file) and the whole VBR bisection below is
/// skipped. That is why `rtc_cbr_wants_recode` is a parameter rather than
/// something this function computes.
///
/// The overlay special case is easy to miss: an overlay frame that came in
/// UNDER `max_frame_bandwidth` cancels the recode even when the bisection
/// asked for one.
///
/// Everything the C tail does after setting `base_q_idx` —
/// `svt_av1_variance_adjust_qp`, `svt_aom_sb_qp_derivation_tpl_la`,
/// `svt_av1_normalize_sb_delta_q` — lives in `rc_aq.c`, not in this file, so
/// it is the caller's to run off [`RecodeDecision::reseed_sb_qindex`].
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn recode_loop_decision_maker(
    scs: &SeqRc,
    rc: &RateControl,
    is_rtc_cbr: bool,
    rtc_cbr_wants_recode: bool,
    base_q_idx: i32,
    loop_count: i32,
    is_overlay: bool,
    projected_frame_size: i32,
    recode_loop_q: i32,
    recode_loop_wants_recode: bool,
) -> RecodeDecision {
    let picture_qp = |qindex: i32| -> u8 {
        i32::from(scs.min_qp_allowed)
            .max(i32::from(scs.max_qp_allowed).min((qindex + 2) >> 2))
            .clamp(i32::from(scs.min_qp_allowed), i32::from(scs.max_qp_allowed)) as u8
    };

    if is_rtc_cbr {
        if rtc_cbr_wants_recode {
            return RecodeDecision {
                do_recode: true,
                base_q_idx,
                picture_qp: picture_qp(base_q_idx),
                loop_count: loop_count + 1,
                reseed_sb_qindex: true,
            };
        }
        // C returns without touching `*do_recode`, which the caller
        // initialised to false, and WITHOUT resetting loop_count.
        return RecodeDecision {
            do_recode: false,
            base_q_idx,
            picture_qp: picture_qp(base_q_idx),
            loop_count,
            reseed_sb_qindex: false,
        };
    }

    // Special case for an overlay frame that is already under the burst cap.
    let mut loop_again = recode_loop_wants_recode;
    if loop_again && is_overlay && projected_frame_size < rc.max_frame_bandwidth {
        loop_again = false;
    }

    if !loop_again {
        return RecodeDecision {
            do_recode: false,
            base_q_idx,
            picture_qp: picture_qp(base_q_idx),
            loop_count: 0,
            reseed_sb_qindex: false,
        };
    }

    let new_q = crate::port_rc_vbr_cbr_state::clamp_qindex(scs, recode_loop_q);
    RecodeDecision {
        do_recode: true,
        base_q_idx: new_q,
        picture_qp: picture_qp(new_q),
        loop_count: loop_count + 1,
        reseed_sb_qindex: true,
    }
}

/// Why the rest of `enc_dec_process.c` is not translated.
///
/// The lane's brief says to count non-translatable work OUT of the queue with
/// a reason rather than leave it reported as missing. This is that ledger. It
/// is a doc item, not code, so that the reasons live next to the port and move
/// with it.
///
/// **Threading, allocation and lifecycle — replaced by design, not ported.**
/// The port has no thread contexts, no dctors and no object pools; it owns its
/// buffers with Rust lifetimes.
/// * `svt_aom_enc_dec_context_ctor` (:214), `enc_dec_context_dctor` (:100) —
///   context allocation / teardown.
/// * `svt_aom_mode_decision_kernel` (:2900), `svt_aom_mode_decision_kernel_iter`
///   — the encode-decode THREAD body. `EncodePipeline` is the port's
///   equivalent and is tile-parallel by construction.
/// * `assign_enc_dec_segments` (:319) — hands SB rows to worker threads
///   through a mutexed segment queue.
/// * `reset_enc_dec` (:530), `reset_encode_pass_neighbor_arrays` (:496),
///   `reset_segmentation_map` (:488) — per-picture buffer resets for objects
///   the port allocates per encode.
/// * `rtime_alloc_palette_search_buffers` (:2860) — a lazy allocation.
/// * `free_temporal_filtering_buffer` (:749) — frees `saved_src_pic`.
/// * `prepare_input_picture` (:2760), `pad_ref_and_set_flags` (:2680) —
///   reference-buffer padding and DPB flag plumbing.
/// * `copy_neighbour_arrays_pd0` (:1300) — copies between the pipeline's
///   neighbour-array objects.
/// * `svt_aom_recon_output` (:560) — pushes a recon buffer into an output
///   fifo.
/// * `svt_av1_add_film_grain` (:2820) — a thin dispatch into
///   `grainsynthesis.c`; the port's film-grain synthesis is
///   [`crate::film_grain`].
///
/// **Debug-only.**
/// * `exaustive_light_pd1_features` (:2075) — its own comment says "for
///   debug/documentation purposes: list all features assumed off for light
///   pd1". It asserts; it computes nothing.
///
/// **Genuinely missing, and NOT counted out — the honest remainder.** These
/// are real algorithms this lane has not ported:
/// * `init_md_scan` (:1441), `set_blocks_to_test` (:1394),
///   `set_blocks_to_be_tested` (:1482), `set_child_to_be_tested` (:1513) —
///   the PD1 scan construction. [`crate::depth_refine`] builds its scan a
///   different way; these are the C shape.
/// * `update_pred_th_offset` (:1536),
///   `is_parent_to_current_deviation_small` (:1634),
///   `is_child_to_current_deviation_small` (:1693),
///   `get_max_min_pd0_depths` (:1943),
///   `perform_pred_depth_refinement` (:1969) — **PARTIALLY ported**, inside
///   [`crate::depth_refine::set_start_end_depth`], for the all-intra path
///   only. The arms that only an INTER frame reaches are absent there and are
///   named in that module: `use_ref_info` (reference-block-size agreement),
///   `q_weight` (`svt_aom_get_qp_based_th_scaling_factors`) and
///   `coeff_lvl_modulation`. Reporting these as "ported" on the strength of a
///   name match would be wrong in exactly the direction the inventory tool
///   warns about.
/// * `pd0_detector` (:2406), `pd0_detector_allintra` (:2341),
///   `lpd1_detector_post_pd0` (:2105), `lpd1_detector_skip_pd0` (:2209) — the
///   preset speed detectors that choose the light-PD1 path.
/// * `avg_cdf_symbol` (:2543), `avg_cdf_symbols` (:2585), `avg_nmv` (:2567),
///   `copy_mv_rate` (:36) — the SB-level CDF averaging. NOTE for whoever picks
///   these up: `avg_cdf_symbol` and `avg_cdf_symbols` DO have surviving `t`
///   symbols, but LLVM constant-propagated their weight parameters, so the
///   compiled ABI does not match the source signature and they are NOT
///   directly bindable (the same trap as
///   `calc_active_worst_quality_no_stats_cbr`; disassemble before trying).
pub mod untranslated {}

#[cfg(test)]
mod tests {
    use super::*;

    /// **EVIDENCE TIERS in this module, stated per test.** [`ssim2`] and
    /// everything under it are pinned at TIER 1 by
    /// `tests/c_parity_enc_dec_metrics.rs`. The 10-bit chain, `get_sse_10bit`
    /// and [`recode_loop_decision_maker`] are TIER 4 — hand-derived vectors
    /// traced against the C source — because the 10-bit walker's symbol has a
    /// constant-folded ABI and the other two were inlined away entirely. Each
    /// test below says which it is.
    const _: () = ();

    fn seq(seq: &mut SeqRc) -> &mut SeqRc {
        seq
    }

    /// TIER 4. The 8x8 accumulators, hand-computed for a uniform window.
    #[test]
    fn ssim_parms_8x8_accumulates_into_the_caller_s_sums() {
        let s = vec![10u8; 64];
        let r = vec![20u8; 64];
        let mut sums = SsimSums::default();
        ssim_parms_8x8(&s, 8, &r, 8, &mut sums);
        assert_eq!(sums.sum_s, 64 * 10);
        assert_eq!(sums.sum_r, 64 * 20);
        assert_eq!(sums.sum_sq_s, 64 * 100);
        assert_eq!(sums.sum_sq_r, 64 * 400);
        assert_eq!(sums.sum_sxr, 64 * 200);
        // C ADDS INTO the out-params without clearing them; a second call must
        // double, not replace.
        ssim_parms_8x8(&s, 8, &r, 8, &mut sums);
        assert_eq!(sums.sum_s, 2 * 64 * 10);
    }

    /// TIER 4. The split 10-bit sample assembly:
    /// `(hi << 2) | ((lo >> 6) & 3)`.
    #[test]
    fn highbd_ssim_parms_8x8_assembles_the_split_sample() {
        let hi = vec![128u8; 64];
        // 0b1000_0000 >> 6 == 0b10 == 2.
        let lo = vec![128u8; 64];
        let r = vec![511u16; 64];
        let mut sums = SsimSums::default();
        highbd_ssim_parms_8x8(&hi, 8, &lo, 8, &r, 8, &mut sums);
        let ss = (128u32 << 2) + 2; // 514
        assert_eq!(sums.sum_s, 64 * ss);
        assert_eq!(sums.sum_sq_s, 64 * ss * ss);
        assert_eq!(sums.sum_sxr, 64 * ss * 511);
    }

    /// TIER 4, and the vector is the one that CAUGHT the constant-folded C
    /// symbol: a uniform 8x8 window at `ss = 514`, `r = 511`, `bd = 10`,
    /// `shift = 2`. Computed independently from the C source arithmetic
    /// (the sums shift by `shift` / `2 * shift` AFTER accumulation, and the
    /// bd-10 constants are `(428658 * 64 * 64) >> 12` and
    /// `(3857925 * 64 * 64) >> 12`).
    ///
    /// The promoted `aom_highbd_ssim2` returns 0.9999828709000638 for the same
    /// input, which is the `shift = 0` answer — that is the specialisation,
    /// not a port bug. See `link_globalized_enc_dec_statics` in
    /// `svtav1-cref/build.rs`.
    #[test]
    fn highbd_ssim_8x8_applies_the_shift_after_accumulation() {
        let hi = vec![128u8; 64];
        let lo = vec![128u8; 64];
        let r = vec![511u16; 64];
        let got = highbd_ssim_8x8(&hi, 8, &lo, 8, &r, 8, 10, 2);
        assert_eq!(got.to_bits(), 0.999_982_921_923_913_5_f64.to_bits());
        // shift = 0 is the value the specialised C symbol returns.
        let unshifted = highbd_ssim_8x8(&hi, 8, &lo, 8, &r, 8, 10, 0);
        assert_eq!(unshifted.to_bits(), 0.999_982_870_900_063_8_f64.to_bits());
        assert_ne!(got.to_bits(), unshifted.to_bits());
    }

    /// TIER 4. The `<= 8` guard, on BOTH dimensions, returns NaN — and 8 is
    /// inside the guard, not outside it.
    #[test]
    fn ssim2_returns_nan_for_a_too_small_region() {
        let a = vec![0u8; 16 * 16];
        assert!(ssim2(&a, 16, &a, 16, 8, 16).is_nan());
        assert!(ssim2(&a, 16, &a, 16, 16, 8).is_nan());
        assert!(ssim2(&a, 16, &a, 16, 4, 4).is_nan());
        assert!(!ssim2(&a, 16, &a, 16, 9, 9).is_nan());
        let hi = vec![0u8; 16 * 16];
        let r = vec![0u16; 16 * 16];
        assert!(highbd_ssim2(&hi, 16, &hi, 16, &r, 16, 8, 16, 10, 2).is_nan());
    }

    /// TIER 4. The window steps by **4**, not 8, so a 16x16 region yields
    /// 3x3 = 9 windows rather than 2x2 = 4. Stepping by 8 would be a
    /// different metric that still "works".
    #[test]
    fn ssim2_steps_the_window_by_four() {
        // Two planes that differ only in one 4x4 quadrant: an 8-step walk
        // would visit it once, a 4-step walk four times, so the scores differ.
        let mut a = vec![100u8; 16 * 16];
        let b = vec![100u8; 16 * 16];
        for y in 4..8 {
            for x in 4..8 {
                a[y * 16 + x] = 200;
            }
        }
        let four_step = ssim2(&a, 16, &b, 16, 16, 16);
        // Recompute with an 8-step walk by hand to show it is a different
        // number, i.e. that this test can actually fail.
        let mut total = 0.0;
        let mut n = 0u32;
        let mut i = 0;
        while i + 8 <= 16 {
            let mut j = 0;
            while j + 8 <= 16 {
                total += ssim_8x8(&a[i * 16 + j..], 16, &b[i * 16 + j..], 16);
                n += 1;
                j += 8;
            }
            i += 8;
        }
        let eight_step = total / f64::from(n);
        assert_eq!(n, 4);
        assert_ne!(four_step.to_bits(), eight_step.to_bits());
    }

    /// TIER 4. `svt_aom_similarity` selects its stabilisers on `bd`, and the
    /// three arms give three different scores for the same sums.
    #[test]
    fn similarity_selects_constants_on_bit_depth() {
        let (a, b, c, d, e) = (8224u32, 8176u32, 1_056_784u32, 1_044_484u32, 1_050_616u32);
        let v8 = similarity(a, b, c, d, e, 64, 8);
        let v10 = similarity(a, b, c, d, e, 64, 10);
        let v12 = similarity(a, b, c, d, e, 64, 12);
        assert_ne!(v8.to_bits(), v10.to_bits());
        assert_ne!(v10.to_bits(), v12.to_bits());
    }

    /// TIER 4. C's `else` arm zeroes c1/c2 and asserts; the port panics.
    #[test]
    #[should_panic(expected = "unsupported bit depth")]
    fn similarity_refuses_an_unknown_bit_depth() {
        let _ = similarity(1, 1, 1, 1, 1, 64, 9);
    }

    /// TIER 4. `get_sse_10bit` assembles its sample with an OR and no mask,
    /// and squares a SIGNED difference.
    #[test]
    fn get_sse_10bit_squares_a_signed_difference() {
        let hi = vec![128u8; 16];
        let lo = vec![128u8; 16]; // >> 6 == 2
        // sample = (128 << 2) | 2 == 514
        let b = vec![500u16; 16];
        let sse = get_sse_10bit(&hi, 4, &lo, 4, &b, 4, 4, 4);
        assert_eq!(sse, 16 * (514i64 - 500) * (514 - 500));
        // A reference ABOVE the source gives the same magnitude.
        let b_hi = vec![528u16; 16];
        let sse2 = get_sse_10bit(&hi, 4, &lo, 4, &b_hi, 4, 4, 4);
        assert_eq!(sse2, 16 * 14 * 14);
    }

    fn rc_with_cap(cap: i32) -> RateControl {
        RateControl {
            max_frame_bandwidth: cap,
            ..Default::default()
        }
    }

    /// TIER 4. The RTC-CBR path RETURNS EARLY: it never runs the VBR
    /// bisection, and when it declines a recode it does NOT reset
    /// `loop_count` (the VBR path does).
    #[test]
    fn recode_loop_decision_maker_rtc_cbr_returns_early() {
        let mut s = SeqRc::default();
        let scs = seq(&mut s);
        let rc = rc_with_cap(1_000_000);

        let yes = recode_loop_decision_maker(scs, &rc, true, true, 120, 3, false, 0, 200, true);
        assert!(yes.do_recode);
        assert_eq!(yes.base_q_idx, 120, "the RTC path keeps the qindex it had");
        assert_eq!(yes.loop_count, 4);
        assert!(yes.reseed_sb_qindex);

        let no = recode_loop_decision_maker(scs, &rc, true, false, 120, 3, false, 0, 200, true);
        assert!(!no.do_recode);
        assert_eq!(
            no.loop_count, 3,
            "the RTC early return must NOT reset loop_count"
        );
        assert!(!no.reseed_sb_qindex);
    }

    /// TIER 4. An overlay frame already under the burst cap CANCELS a recode
    /// the bisection asked for — and the cancel path resets `loop_count`.
    #[test]
    fn recode_loop_decision_maker_overlay_under_cap_cancels() {
        let mut s = SeqRc::default();
        let scs = seq(&mut s);
        let rc = rc_with_cap(1_000_000);

        let cancelled =
            recode_loop_decision_maker(scs, &rc, false, false, 120, 2, true, 500_000, 200, true);
        assert!(!cancelled.do_recode);
        assert_eq!(cancelled.loop_count, 0);

        // At or above the cap the cancel does not apply.
        let kept =
            recode_loop_decision_maker(scs, &rc, false, false, 120, 2, true, 1_000_000, 200, true);
        assert!(kept.do_recode);
        assert_eq!(kept.loop_count, 3);

        // A non-overlay frame is never cancelled.
        let inter =
            recode_loop_decision_maker(scs, &rc, false, false, 120, 2, false, 500_000, 200, true);
        assert!(inter.do_recode);
    }

    /// TIER 4. On a recode the new qindex is clamped through
    /// `quantizer_to_qindex[min/max_qp_allowed]` and `picture_qp` is
    /// re-derived as `(base_q_idx + 2) >> 2` clamped to the QP domain — the
    /// two clamps are in DIFFERENT domains and using one for both is the easy
    /// mistake.
    #[test]
    fn recode_loop_decision_maker_clamps_qindex_and_qp_in_their_own_domains() {
        let mut s = SeqRc {
            min_qp_allowed: 10,
            max_qp_allowed: 40,
            ..Default::default()
        };
        let scs = seq(&mut s);
        let rc = rc_with_cap(1_000_000);

        // A wildly high q clamps to quantizer_to_qindex[40].
        let d = recode_loop_decision_maker(scs, &rc, false, false, 100, 0, false, 0, 250, true);
        let qmax = i32::from(crate::rate_control::qp_to_qindex(40));
        assert_eq!(d.base_q_idx, qmax);
        assert_eq!(d.picture_qp, 40);

        // A wildly low q clamps to quantizer_to_qindex[10].
        let d = recode_loop_decision_maker(scs, &rc, false, false, 100, 0, false, 0, -5, true);
        let qmin = i32::from(crate::rate_control::qp_to_qindex(10));
        assert_eq!(d.base_q_idx, qmin);
        assert_eq!(d.picture_qp, 10);
    }
}

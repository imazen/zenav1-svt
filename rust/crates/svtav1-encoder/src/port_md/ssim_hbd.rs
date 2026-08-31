//! The HIGH-BIT-DEPTH half of the tune-SSIM MD distortion —
//! `mode_decision.c:4220-4408`.
//!
//! [`crate::ssim_md`] ports the 8-bit arm of
//! `svt_spatial_full_distortion_ssim_kernel` (multiplier `m = 1`,
//! `svt_aom_similarity` at `bd = 8`). C's `hbd` arm is a DIFFERENT
//! computation: it calls `ssim_hbd`, whose tiles use
//! `svt_aom_similarity` at **bd = 10** (different `c1`/`c2` constants),
//! and it sets the distortion multiplier **`m = 8`**. Neither of those
//! falls out of the 8-bit code, so this is a separate module rather than
//! a widened one.
//!
//! | this module | C |
//! |---|---|
//! | [`similarity`] | `enc_dec_process.c:645-676` (EXPORTED, all three bit depths) |
//! | [`ssim_4x4_hbd`] | `mode_decision.c:4220-4241` (EXPORTED) |
//! | [`ssim_8x8_hbd`] | `mode_decision.c:4245-4266` (EXPORTED) |
//! | [`ssim_8x8_blocks_hbd`] | `mode_decision.c:4321-4340` |
//! | [`ssim_4x4_blocks_hbd`] | `mode_decision.c:4342-4361` |
//! | [`ssim_hbd`] | `mode_decision.c:4363-4370` |
//! | [`spatial_full_distortion_ssim_hbd`] | `mode_decision.c:4372-4408`, hbd arm |
//!
//! # Evidence
//!
//! Tier 1 for the three EXPORTED symbols (`svt_aom_similarity`,
//! `svt_ssim_4x4_hbd_c`, `svt_ssim_8x8_hbd_c`) plus the whole kernel
//! through `svt_spatial_full_distortion_ssim_kernel`, in
//! `tests/c_parity_md_ssim_hbd.rs`. The `_c` suffix on the two tile
//! kernels is deliberate: those are the scalar references this port
//! transcribes, not the RTCD pointers.
//!
//! # An UNVERIFIED premise, flagged rather than asserted
//!
//! The group's triage could not confirm that the fork's
//! `--alt-ssim-tuning` is reachable at 10 bit. `mds3.rs` calls
//! [`crate::ssim_md::spatial_full_distortion_ssim`] on an 8-bit recon
//! whenever `frame.tune_ssim`, with no hbd argument, while C branches on
//! `ctx->hbd_md`. `tune_ssim` comes from `hdr.is_fork() &&
//! hdr.alt_ssim_tuning` and bd10 full-RD MD exists at preset <= 8, and
//! nothing was found gating the two apart — **but no cell was run to
//! confirm it**. This module is therefore a faithful translation whose
//! REACHABILITY is open, which `docs/WORKING-ON-THIS.md` §7 says to keep
//! and document rather than skip. If a measurement later shows the hbd
//! arm is unreachable, that belongs here as a correction, not as a
//! deletion.
//!
//! # Delegated
//!
//! `svt_psy_distortion_hbd` (ac_bias.c:103) needs the high-bit-depth
//! Hadamard kernels and is itself under `#if
//! CONFIG_ENABLE_HIGH_BIT_DEPTH`. [`spatial_full_distortion_ssim_hbd`]
//! takes its result as a parameter rather than stubbing it, so a caller
//! without it must pass 0 knowingly.

/// C `cc1` / `cc2` (enc_dec_process.c) for bd 8: `64^2*(.01*255)^2` and
/// `64^2*(.03*255)^2`.
pub const CC1_8: i64 = 26634;
pub const CC2_8: i64 = 239708;
/// C `cc1_10` / `cc2_10` (enc_dec_process.c:640-641).
pub const CC1_10: i64 = 428658;
pub const CC2_10: i64 = 3857925;
/// C `cc1_12` / `cc2_12` (enc_dec_process.c:642-643).
pub const CC1_12: i64 = 6868593;
pub const CC2_12: i64 = 61817334;

/// C `svt_aom_similarity` (enc_dec_process.c:645-676, EXPORTED), all
/// three bit depths.
///
/// C's `else` arm sets `c1 = c2 = 0` and asserts; a release build
/// silently returns the degenerate ratio. Reproduced with `_ => (0, 0)`
/// rather than a panic, because a panic is a different behaviour from
/// C's.
///
/// The numerator's first factor is `2.0 * sum_s * sum_r + c1` evaluated
/// in `double` — note C promotes `sum_s`/`sum_r` (both `uint32_t`)
/// through the leading `2.0`, while the DENOMINATOR casts each to
/// `double` explicitly. Both end up as `f64` products, so the port uses
/// `f64` throughout.
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
        8 => (CC1_8, CC2_8),
        10 => (CC1_10, CC2_10),
        12 => (CC1_12, CC2_12),
        _ => (0, 0),
    };
    let c1 = ((cc1 * count * count) >> 12) as f64;
    let c2 = ((cc2 * count * count) >> 12) as f64;
    let sum_s = f64::from(sum_s);
    let sum_r = f64::from(sum_r);
    let sum_sq_s = f64::from(sum_sq_s);
    let sum_sq_r = f64::from(sum_sq_r);
    let sum_sxr = f64::from(sum_sxr);
    let count = count as f64;
    let ssim_n = (2.0 * sum_s * sum_r + c1) * (2.0 * count * sum_sxr - 2.0 * sum_s * sum_r + c2);
    let ssim_d = (sum_s * sum_s + sum_r * sum_r + c1)
        * (count * sum_sq_s - sum_s * sum_s + count * sum_sq_r - sum_r * sum_r + c2);
    ssim_n / ssim_d
}

/// The five accumulators C's tile kernels build. Kept separate so the
/// 4x4 and 8x8 kernels share exactly the code C shares (the loop) and
/// nothing else.
fn tile_sums(s: &[u16], sp: usize, r: &[u16], rp: usize, n: usize) -> (u32, u32, u32, u32, u32) {
    let (mut sum_s, mut sum_r, mut sum_sq_s, mut sum_sq_r, mut sum_sxr) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    for i in 0..n {
        for j in 0..n {
            let sv = u32::from(s[i * sp + j]);
            let rv = u32::from(r[i * rp + j]);
            // C accumulates into uint32_t and WRAPS on overflow; at
            // 12-bit the worst case is 4095^2 * 64 = 1.07e9, still inside
            // u32, so wrapping is unreachable — but it is what C does.
            sum_s = sum_s.wrapping_add(sv);
            sum_r = sum_r.wrapping_add(rv);
            sum_sq_s = sum_sq_s.wrapping_add(sv.wrapping_mul(sv));
            sum_sq_r = sum_sq_r.wrapping_add(rv.wrapping_mul(rv));
            sum_sxr = sum_sxr.wrapping_add(sv.wrapping_mul(rv));
        }
    }
    (sum_s, sum_r, sum_sq_s, sum_sq_r, sum_sxr)
}

/// C `svt_ssim_4x4_hbd_c` (mode_decision.c:4220-4241, EXPORTED).
///
/// **Its `bd` argument to `svt_aom_similarity` is a hardwired 10**, not
/// the caller's bit depth — so a 12-bit encode still uses the 10-bit
/// stabilizers here.
pub fn ssim_4x4_hbd(s: &[u16], sp: usize, r: &[u16], rp: usize) -> f64 {
    let (a, b, c, d, e) = tile_sums(s, sp, r, rp, 4);
    similarity(a, b, c, d, e, 4 * 4, 10)
}

/// C `svt_ssim_8x8_hbd_c` (mode_decision.c:4245-4266, EXPORTED). Same
/// hardwired `bd = 10`.
pub fn ssim_8x8_hbd(s: &[u16], sp: usize, r: &[u16], rp: usize) -> f64 {
    let (a, b, c, d, e) = tile_sums(s, sp, r, rp, 8);
    similarity(a, b, c, d, e, 8 * 8, 10)
}

/// C `ssim_8x8_blocks_hbd` (mode_decision.c:4321-4340).
pub fn ssim_8x8_blocks_hbd(
    s: &[u16],
    sp: usize,
    r: &[u16],
    rp: usize,
    width: usize,
    height: usize,
) -> f64 {
    tiling_walk(s, sp, r, rp, width, height, 8)
}

/// C `ssim_4x4_blocks_hbd` (mode_decision.c:4342-4361).
pub fn ssim_4x4_blocks_hbd(
    s: &[u16],
    sp: usize,
    r: &[u16],
    rp: usize,
    width: usize,
    height: usize,
) -> f64 {
    tiling_walk(s, sp, r, rp, width, height, 4)
}

fn tiling_walk(
    s: &[u16],
    sp: usize,
    r: &[u16],
    rp: usize,
    width: usize,
    height: usize,
    n: usize,
) -> f64 {
    let mut total = 0.0f64;
    let mut samples = 0u32;
    let mut i = 0usize;
    while i + n <= height {
        let mut j = 0usize;
        while j + n <= width {
            let v = if n == 8 {
                ssim_8x8_hbd(&s[i * sp + j..], sp, &r[i * rp + j..], rp)
            } else {
                ssim_4x4_hbd(&s[i * sp + j..], sp, &r[i * rp + j..], rp)
            };
            // C `CLIP3(0, 1, v)`.
            total += v.clamp(0.0, 1.0);
            samples += 1;
            j += n;
        }
        i += n;
    }
    debug_assert!(samples > 0);
    total / f64::from(samples)
}

/// C `ssim_hbd` (mode_decision.c:4363-4370): 8x8 tiling when BOTH
/// dimensions are multiples of 8, else 4x4.
pub fn ssim_hbd(s: &[u16], sp: usize, r: &[u16], rp: usize, width: usize, height: usize) -> f64 {
    debug_assert!(width.is_multiple_of(4) && height.is_multiple_of(4));
    if width.is_multiple_of(8) && height.is_multiple_of(8) {
        ssim_8x8_blocks_hbd(s, sp, r, rp, width, height)
    } else {
        ssim_4x4_blocks_hbd(s, sp, r, rp, width, height)
    }
}

/// C `SSIM_DISTORTION_M_HBD` — the `m = 8` the hbd arm of
/// `svt_spatial_full_distortion_ssim_kernel` sets
/// (mode_decision.c:4396). The 8-bit arm leaves `m = 1`.
pub const SSIM_DISTORTION_M_HBD: u64 = 8;

/// C `svt_spatial_full_distortion_ssim_kernel`
/// (mode_decision.c:4372-4408), **hbd arm**.
///
/// `psy_ac_distortion` is C's `svt_psy_distortion_hbd(...)` result, which
/// this port does not compute (see the module doc); pass 0 when
/// `ac_bias == 0`, which is what C does by never evaluating it.
///
/// The final expression is
/// `(uint64_t)((1 - ssim) * count * 100 * 7 * m) + psy`, with the cast
/// TRUNCATING toward zero and `m = 8` — four times the 8-bit arm's
/// scale is *not* an accident of bit depth, it is a different constant.
#[allow(clippy::too_many_arguments)]
pub fn spatial_full_distortion_ssim_hbd(
    input: &[u16],
    input_offset: usize,
    input_stride: usize,
    recon: &[u16],
    recon_offset: usize,
    recon_stride: usize,
    area_width: usize,
    area_height: usize,
    ac_bias: f64,
    psy_ac_distortion: u64,
) -> u64 {
    let count = (area_width * area_height) as f64;
    let ssim_score = ssim_hbd(
        &input[input_offset..],
        input_stride,
        &recon[recon_offset..],
        recon_stride,
        area_width,
        area_height,
    );
    let psy = if ac_bias != 0.0 {
        (psy_ac_distortion as f64 * ac_bias) as u64
    } else {
        0
    };
    let spatial = ((1.0 - ssim_score) * count * 100.0 * 7.0 * SSIM_DISTORTION_M_HBD as f64) as u64;
    spatial + psy
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity case: a plane against itself scores 1, so the
    /// distortion is 0 regardless of `m`.
    #[test]
    fn identical_planes_score_one() {
        let img: Vec<u16> = (0..64 * 64).map(|i| ((i * 7) % 1024) as u16).collect();
        let s = ssim_hbd(&img, 64, &img, 64, 64, 64);
        assert!(
            (s - 1.0).abs() < 1e-9,
            "ssim_hbd of a plane with itself: {s}"
        );
        assert_eq!(
            spatial_full_distortion_ssim_hbd(&img, 0, 64, &img, 0, 64, 64, 64, 0.0, 0),
            0
        );
    }

    /// The hbd multiplier is 8, and it multiplies the SPATIAL term only —
    /// the psy term is added after.
    #[test]
    fn hbd_multiplier_scales_only_the_spatial_term() {
        assert_eq!(SSIM_DISTORTION_M_HBD, 8);
        let a: Vec<u16> = vec![100; 64 * 64];
        let mut b = a.clone();
        b[0] = 900;
        let no_psy = spatial_full_distortion_ssim_hbd(&a, 0, 64, &b, 0, 64, 64, 64, 0.0, 0);
        let with_psy = spatial_full_distortion_ssim_hbd(&a, 0, 64, &b, 0, 64, 64, 64, 2.0, 10);
        assert_eq!(with_psy, no_psy + 20);
        // ac_bias == 0 must NOT add the psy term even when one is given,
        // because C never evaluates it on that branch.
        assert_eq!(
            spatial_full_distortion_ssim_hbd(&a, 0, 64, &b, 0, 64, 64, 64, 0.0, 10),
            no_psy
        );
    }

    /// `ssim_hbd` picks 4x4 tiling when EITHER dimension is not a
    /// multiple of 8 — an AND over both, not an OR.
    #[test]
    fn tiling_choice_needs_both_dims_multiple_of_eight() {
        let img: Vec<u16> = (0..64 * 64).map(|i| ((i * 13) % 1024) as u16).collect();
        let mut noisy = img.clone();
        for (i, v) in noisy.iter_mut().enumerate() {
            if i % 5 == 0 {
                *v = v.wrapping_add(37);
            }
        }
        let by_8 = ssim_8x8_blocks_hbd(&img, 64, &noisy, 64, 16, 16);
        let by_4 = ssim_4x4_blocks_hbd(&img, 64, &noisy, 64, 16, 16);
        assert!((by_8 - by_4).abs() > 1e-12, "the two tilings must differ");
        assert_eq!(ssim_hbd(&img, 64, &noisy, 64, 16, 16), by_8);
        // 12 is a multiple of 4 but not 8 -> the 4x4 walker.
        assert_eq!(
            ssim_hbd(&img, 64, &noisy, 64, 12, 16),
            ssim_4x4_blocks_hbd(&img, 64, &noisy, 64, 12, 16)
        );
        assert_eq!(
            ssim_hbd(&img, 64, &noisy, 64, 16, 12),
            ssim_4x4_blocks_hbd(&img, 64, &noisy, 64, 16, 12)
        );
    }

    /// The two tile kernels hardwire `bd = 10` regardless of the content's
    /// actual depth — pinned so a later "fix" to pass the real depth is
    /// caught.
    #[test]
    fn tile_kernels_hardwire_bd_10() {
        let s: Vec<u16> = (0..64).map(|i| (i * 17) as u16).collect();
        let r: Vec<u16> = (0..64).map(|i| (i * 19) as u16).collect();
        let sums = tile_sums(&s, 8, &r, 8, 8);
        assert_eq!(
            ssim_8x8_hbd(&s, 8, &r, 8),
            similarity(sums.0, sums.1, sums.2, sums.3, sums.4, 64, 10)
        );
        assert_ne!(
            ssim_8x8_hbd(&s, 8, &r, 8),
            similarity(sums.0, sums.1, sums.2, sums.3, sums.4, 64, 8)
        );
    }
}

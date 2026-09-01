//! High-bit-depth resize — the 10-bit half of `Codec/resize.c`.
//!
//! # What this closes
//!
//! `docs/REFUSED-CONFIGS.md` carries a CAPABILITY refusal: *"superres is
//! 8-bit only so far (the u16 source downscale is unported)"*. The 8-bit twin
//! (`resize.rs`) is complete and tier-1 gated by `c_parity_resize.rs`; this is
//! the mechanical widening of it, gated the same way. It blocks no inter work
//! — it retires a ledgered refusal, which `WORKING-ON-THIS.md` §6b says to
//! read as a backlog rather than a solution.
//!
//! **The refusal is not yet removed.** `docs/REFUSED-CONFIGS.md` is GENERATED
//! (`tools/refusal_inventory.sh`) from the `Err` sites themselves, and the
//! site for this one is in `crates/svtav1-encoder/src/pipeline.rs`, which this
//! lane does not own. What is closed here is the KERNEL gap — the `u16` source
//! downscale now exists and is C-gated. Removing the refusal is one wiring
//! change in `pipeline.rs` plus a superres bd10 gate cell, and until that
//! lands the config still (correctly) refuses.
//!
//! # Not a copy of the 8-bit path with a wider type
//!
//! Two things genuinely differ and are ported as written:
//! * every write goes through `clip_pixel_highbd(v, bd)`, so the clamp ceiling
//!   is `(1 << bd) - 1`, not 255;
//! * `bd` is threaded through every level of the ladder
//!   (`resize_plane_horizontal` -> `resize_multistep` -> `down2_sym*` /
//!   `interpolate`), because the clamp happens at the LEAF, not once at the
//!   end.
//!
//! Everything else — the filter banks, `choose_interp_filter`,
//! `get_down2_steps` / `get_down2_length`, the phase accumulator — is shared
//! with the 8-bit path and is re-used from [`crate::resize`] rather than
//! duplicated, so the two cannot drift.
//!
//! # Evidence
//!
//! Tier 1 — `tests/c_parity_resize_hbd.rs` drives the real exported
//! `svt_av1_highbd_interpolate_core_c`, `svt_av1_highbd_down2_symeven_c` and
//! `svt_av1_highbd_resize_plane_horizontal`. The statics
//! (`highbd_interpolate`, `highbd_down2_symodd`, `highbd_resize_multistep`)
//! have no exported symbol and are covered TRANSITIVELY through
//! `svt_av1_highbd_resize_plane_horizontal`, which drives all three.

use alloc::vec;
use alloc::vec::Vec;

use crate::resize::{
    DOWN2_SYMEVEN_HALF_FILTER, DOWN2_SYMODD_HALF_FILTER, SUBPEL_TAPS, choose_interp_filter,
    down2_length, down2_steps,
};
use crate::superres::{
    RS_SCALE_EXTRA_BITS, RS_SCALE_EXTRA_OFF, RS_SCALE_SUBPEL_BITS, RS_SUBPEL_BITS, RS_SUBPEL_MASK,
};

/// C `FILTER_BITS`. `resize.rs` keeps its own private copy; this is the same
/// value from the same header (filter.h:22) rather than a second definition
/// with a different meaning.
const FILTER_BITS: i32 = 7;

/// `clip_pixel_highbd(v, bd)`.
#[inline]
fn clip_pixel_highbd(v: i32, bd: i32) -> u16 {
    v.clamp(0, (1i32 << bd) - 1) as u16
}

/// Port of `svt_av1_highbd_interpolate_core_c` (resize.c:489) — polyphase
/// resample of one 1-D line from `in_length` to `out_length`.
///
/// C splits the output into initial / middle / end parts so the middle can
/// skip the edge clamps; this port runs ONE loop with the full clamp, which is
/// bit-identical (C computes `x1`/`x2` precisely so the middle part's taps are
/// already in range, making the clamp a no-op there) and cannot silently
/// under- or over-read. The same shape the 8-bit port takes, and
/// `c_parity_resize_hbd` covers both regimes including the `x1 > x2`
/// short-input case.
pub fn highbd_interpolate_core(
    input: &[u16],
    in_length: usize,
    output: &mut [u16],
    out_length: usize,
    bd: i32,
    filters: &[[i16; SUBPEL_TAPS]; 1 << RS_SUBPEL_BITS],
) {
    debug_assert!(in_length > 0 && out_length > 0);
    let (inl, outl) = (in_length as i32, out_length as i32);
    let delta = (((in_length as u32) << RS_SCALE_SUBPEL_BITS) as i32 + outl / 2) / outl;
    let offset = if inl > outl {
        (((inl - outl) << (RS_SCALE_SUBPEL_BITS - 1)) + outl / 2) / outl
    } else {
        -((((outl - inl) << (RS_SCALE_SUBPEL_BITS - 1)) + outl / 2) / outl)
    };
    let mut y = offset + RS_SCALE_EXTRA_OFF;
    for out in output.iter_mut().take(out_length) {
        let int_pel = y >> RS_SCALE_SUBPEL_BITS;
        let sub_pel = ((y >> RS_SCALE_EXTRA_BITS) & RS_SUBPEL_MASK) as usize;
        let filter = &filters[sub_pel];
        let mut sum: i32 = 0;
        for (k, &tap) in filter.iter().enumerate() {
            let pk = int_pel - SUBPEL_TAPS as i32 / 2 + 1 + k as i32;
            let idx = pk.clamp(0, inl - 1) as usize;
            sum += i32::from(tap) * i32::from(input[idx]);
        }
        *out = clip_pixel_highbd((sum + (1 << (FILTER_BITS - 1))) >> FILTER_BITS, bd);
        y += delta;
    }
}

/// Port of `highbd_interpolate` (resize.c:562, `static`) — the filter-bank
/// selection wrapper. The bank depends only on the length RATIO, so it is the
/// same `choose_interp_filter` the 8-bit path uses.
pub fn highbd_interpolate(
    input: &[u16],
    in_length: usize,
    output: &mut [u16],
    out_length: usize,
    bd: i32,
) {
    highbd_interpolate_core(
        input,
        in_length,
        output,
        out_length,
        bd,
        choose_interp_filter(in_length as i32, out_length as i32),
    );
}

/// Port of `svt_av1_highbd_down2_symeven_c` (resize.c:568) — exact 2:1
/// decimation with the even-symmetric half filter.
pub fn highbd_down2_symeven(input: &[u16], length: usize, output: &mut [u16], bd: i32) {
    let filter = &DOWN2_SYMEVEN_HALF_FILTER;
    let len = length as i32;
    let mut o = 0usize;
    let mut i = 0i32;
    while i < len {
        let mut sum: i32 = 1 << (FILTER_BITS - 1);
        for (j, &tap) in filter.iter().enumerate() {
            let a = input[(i - j as i32).max(0) as usize];
            let b = input[(i + 1 + j as i32).min(len - 1) as usize];
            sum += (i32::from(a) + i32::from(b)) * i32::from(tap);
        }
        output[o] = clip_pixel_highbd(sum >> FILTER_BITS, bd);
        o += 1;
        i += 2;
    }
}

/// Port of `highbd_down2_symodd` (resize.c:619, `static`) — the odd-phase
/// twin, used when the current length is odd.
pub fn highbd_down2_symodd(input: &[u16], length: usize, output: &mut [u16], bd: i32) {
    let filter = &DOWN2_SYMODD_HALF_FILTER;
    let len = length as i32;
    let mut o = 0usize;
    let mut i = 0i32;
    while i < len {
        let mut sum: i32 =
            (1 << (FILTER_BITS - 1)) + i32::from(input[i as usize]) * i32::from(filter[0]);
        for (j, &tap) in filter.iter().enumerate().skip(1) {
            let a = input[(i - j as i32).max(0) as usize];
            let b = input[(i + j as i32).min(len - 1) as usize];
            sum += (i32::from(a) + i32::from(b)) * i32::from(tap);
        }
        output[o] = clip_pixel_highbd(sum >> FILTER_BITS, bd);
        o += 1;
        i += 2;
    }
}

/// Port of `highbd_resize_multistep` (resize.c:670, `static`) — one 1-D line,
/// `length` -> `olength`: exact 2:1 steps while they fit, then the polyphase
/// interpolate.
///
/// The odd/even choice is on the CURRENT filtered length, re-tested every
/// step, not on the original.
pub fn highbd_resize_multistep(
    input: &[u16],
    length: usize,
    output: &mut [u16],
    olength: usize,
    bd: i32,
) {
    if length == olength {
        output[..length].copy_from_slice(&input[..length]);
        return;
    }
    let steps = down2_steps(length, olength);
    if steps == 0 {
        highbd_interpolate(input, length, output, olength, bd);
        return;
    }
    let mut cur: Vec<u16> = input[..length].to_vec();
    let mut filtered_length = length;
    for _ in 0..steps {
        let proj = down2_length(filtered_length, 1);
        let mut next = vec![0u16; proj];
        if filtered_length & 1 != 0 {
            highbd_down2_symodd(&cur, filtered_length, &mut next, bd);
        } else {
            highbd_down2_symeven(&cur, filtered_length, &mut next, bd);
        }
        cur = next;
        filtered_length = proj;
    }
    if filtered_length == olength {
        output[..olength].copy_from_slice(&cur[..olength]);
    } else {
        highbd_interpolate(&cur, filtered_length, output, olength, bd);
    }
}

/// Port of `svt_av1_highbd_resize_plane_horizontal` (resize.c:761) — the
/// superres SOURCE downscale at high bit depth: `width` -> `width2` at
/// unchanged height.
#[allow(clippy::too_many_arguments)]
pub fn highbd_resize_plane_horizontal(
    input: &[u16],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u16],
    width2: usize,
    out_stride: usize,
    bd: i32,
) {
    debug_assert!(width > 0 && height > 0 && width2 > 0);
    for r in 0..height {
        highbd_resize_multistep(
            &input[r * in_stride..],
            width,
            &mut output[r * out_stride..],
            width2,
            bd,
        );
    }
}

/// C `highbd_fill_col_to_arr` (`resize.c:707`) — static.
pub fn highbd_fill_col_to_arr(img: &[u16], stride: usize, len: usize, arr: &mut [u16]) {
    for (i, dst) in arr.iter_mut().take(len).enumerate() {
        *dst = img[i * stride];
    }
}

/// C `highbd_fill_arr_to_col` (`resize.c:716`) — static.
pub fn highbd_fill_arr_to_col(img: &mut [u16], stride: usize, len: usize, arr: &[u16]) {
    for (i, src) in arr.iter().take(len).enumerate() {
        img[i * stride] = *src;
    }
}

/// C `svt_av1_highbd_resize_plane_c` (`resize.c:725`) — the 10-bit
/// two-dimensional plane resize.
///
/// Identical in shape to the 8-bit
/// [`crate::resize::resize_plane`]: rows first into a `width2`-stride
/// intermediate, then each of the `width2` columns gathered, resized and
/// scattered. `bd` reaches the clamp inside the polyphase interpolator, so it
/// is threaded rather than fixed.
///
/// # Panics
///
/// If either buffer is too small for the strides and dimensions given.
#[allow(clippy::too_many_arguments)]
pub fn highbd_resize_plane(
    input: &[u16],
    height: usize,
    width: usize,
    in_stride: usize,
    output: &mut [u16],
    height2: usize,
    width2: usize,
    out_stride: usize,
    bd: i32,
) {
    assert!(width > 0 && height > 0 && width2 > 0 && height2 > 0);
    assert!(input.len() >= (height - 1) * in_stride + width);
    assert!(output.len() >= (height2 - 1) * out_stride + width2);

    let mut intbuf = alloc::vec![0u16; width2 * height];
    let mut arrbuf = alloc::vec![0u16; height];
    let mut arrbuf2 = alloc::vec![0u16; height2];

    for r in 0..height {
        highbd_resize_multistep(
            &input[r * in_stride..],
            width,
            &mut intbuf[r * width2..],
            width2,
            bd,
        );
    }
    for c in 0..width2 {
        highbd_fill_col_to_arr(&intbuf[c..], width2, height, &mut arrbuf);
        highbd_resize_multistep(&arrbuf, height, &mut arrbuf2, height2, bd);
        highbd_fill_arr_to_col(&mut output[c..], out_stride, height2, &arrbuf2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_length_is_a_copy() {
        let input: Vec<u16> = (0..32u16).map(|i| i * 31).collect();
        let mut out = vec![0u16; 32];
        highbd_resize_multistep(&input, 32, &mut out, 32, 10);
        assert_eq!(out, input);
    }

    #[test]
    fn clamp_ceiling_follows_bit_depth() {
        // A flat plane at the bd maximum must survive every stage unchanged:
        // all the filters sum to 1<<FILTER_BITS, so a flat input is a fixed
        // point, and a ceiling of 255 would crush it.
        for bd in [10i32, 12] {
            let maxv = ((1u32 << bd) - 1) as u16;
            let input = vec![maxv; 64];
            let mut out = vec![0u16; 32];
            highbd_down2_symeven(&input, 64, &mut out, bd);
            assert!(
                out.iter().all(|&v| v == maxv),
                "bd {bd}: flat max plane was not preserved: {:?}",
                &out[..4]
            );
        }
    }
}

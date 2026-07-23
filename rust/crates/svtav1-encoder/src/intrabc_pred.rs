//! IntraBC prediction (compensation) — the RECON-domain block copy + the
//! chroma half-pel bilinear (IBC chunk 7, docs/ibc-port-map.md §A.6/§D.7).
//!
//! C reference (SVT-AV1 v4.2.0): `svt_aom_enc_make_inter_predictor`
//! (enc_inter_prediction.c:2515) with the intrabc identity scale factors →
//! `compute_subpel_params` non-scaled arm (:2400-2452: `mv_q4 =
//! clamp_mv_to_umv_border_sb(xd, mv, bw, bh, ss_x, ss_y)` =
//! `mv.{x,y} * (1 << (1 - ss))` clamped to the UMV border; `subpel =
//! (mv_q4 & SUBPEL_MASK) << SCALE_EXTRA_BITS`; `pos = pre + (mv_q4 >>
//! SUBPEL_BITS)`) → `svt_inter_predictor` (inter_prediction.c:1386-1442):
//! full-pel → the `svt_aom_convolve[0][0][0]` copy slot; any subpel →
//! `convolve_2d_for_intrabc` (:1195-1236), which hardcodes BILINEAR
//! params and passes a LITERAL `8` (exact half-pel) as the kernel
//! row-select — the real Q4 fraction is only a zero/nonzero gate.
//!
//! Semantics under the luma-integer-DV invariant (`is_dv_valid` rejects
//! sub-pel DVs, so `dv & 7 == 0` always):
//! - LUMA (ss=0): `mv_q4 = dv * 2` — always a multiple of 16 → subpel 0 →
//!   plain copy from `(x + dv.x/8, y + dv.y/8)`.
//! - CHROMA 4:2:0 (ss=1): `mv_q4 = dv` (the eighth-pel luma DV read
//!   directly as a Q4 chroma vector). `pos = c_org + (dv >> 4)`
//!   (arithmetic shift — floors toward -inf for negative DVs, exactly C's
//!   `>>`), `subpel = dv & 15` ∈ {0, 8}. Odd full-pel luma components
//!   give the half-pel case.
//!
//! Half-pel bilinear arithmetic, derived from the C convolve chain with
//! the BILINEAR kernel at subpel 8 = {0,0,0,64,64,0,0,0} (inter_
//! prediction.c:1161-1177), FILTER_BITS=7, round_0=3, round_1=11 (8-bit
//! `get_conv_params_no_round`):
//! - x-only (`svt_av1_convolve_x_sr_c`, :402): `res = 64*(a+b)`;
//!   `ROUND_POWER_OF_TWO(res, 3)` = `8*(a+b)` exactly; then
//!   `ROUND_POWER_OF_TWO(·, FILTER_BITS-round_0=4)` = `(a+b+1)>>1`.
//! - y-only (`svt_av1_convolve_y_sr_c`, :374): `ROUND_POWER_OF_TWO(
//!   64*(a+b), 7)` = `(a+b+1)>>1`.
//! - 2d (`svt_av1_convolve_2d_sr_c`, :329): horiz `im = 2048 + 8*(a+b)`
//!   (offset `1<<(bd+FILTER_BITS-1)`), vert `sum = (1<<19) + 64*(im0+im1)`,
//!   `res = ROUND_POWER_OF_TWO(sum, 11) - (256+128)`, `bits = 0` → net
//!   `(a+b+c+d+2)>>2` exactly (worked in the IBC chunk-7 landing notes;
//!   the zero taps of the 8-tap kernel touch pixels that are multiplied
//!   by 0 in C — not read at all here, keeping every access in-bounds).
//!
//! The UMV-border clamp (`clamp_mv_to_umv_border_sb`) NEVER binds for a
//! valid DV: `is_dv_valid` keeps the whole referenced block inside the
//! tile, and the clamp bounds sit `AOM_INTERP_EXTEND + block` PIXELS
//! outside the frame — debug-asserted below rather than modeled.
//!
//! Bounds note (chroma): for an odd DV the bilinear reads one chroma
//! column/row past the referenced block. `is_dv_valid`'s sub-8x8
//! chroma-ref margin keeps the left/top side legal; on the right/bottom
//! the read can touch at most the pixel at the source block's outer edge
//! (source right edge ≤ tile right edge). At the FRAME right/bottom edge
//! that pixel sits outside the visible frame in C too, where the recon
//! picture's allocated border padding is read. The port's canvases are
//! unpadded, so those reads clamp to the last in-frame pixel — a
//! PORT-NOTE(unverified) divergence risk only for a DV whose source
//! region abuts the frame edge AND has an odd component; flagged for the
//! chunk-10 localization pass if a cell first-diverges at such a block.

use svtav1_types::motion::Mv;

/// Luma IntraBC predictor: plain copy from the in-progress recon at
/// `(abs_x + dv.x/8, abs_y + dv.y/8)`. `dst` is `w`-strided tightly
/// packed (the funnel's `Cand::pred` convention).
pub fn predict_intrabc_luma(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    dv: Mv,
    dst: &mut [u8],
) {
    debug_assert_eq!(dv.x & 7, 0, "IntraBC DV must be whole-pel");
    debug_assert_eq!(dv.y & 7, 0, "IntraBC DV must be whole-pel");
    let sx = (abs_x as i64 + i64::from(dv.x >> 3)) as usize;
    let sy = (abs_y as i64 + i64::from(dv.y >> 3)) as usize;
    for r in 0..h {
        let src_row = (sy + r) * stride + sx;
        dst[r * w..r * w + w].copy_from_slice(&recon[src_row..src_row + w]);
    }
}

/// Chroma-plane IntraBC predictor (4:2:0): the C `compute_subpel_params`
/// ss=1 derivation + copy / half-pel bilinear dispatch. `c_org_x/y` are
/// the block's CHROMA-plane origin (the caller's `ccx`/`ccy`, already
/// chroma-ref adjusted); `cw`/`ch` the chroma dims; `plane` is one full
/// chroma canvas (`c_stride`-strided); `dst` is `cw`-strided.
///
/// `frame_cw`/`frame_ch` bound the clamped reads (the canvas' chroma
/// dims) — see the module bounds note.
#[allow(clippy::too_many_arguments)]
pub fn predict_intrabc_chroma(
    plane: &[u8],
    c_stride: usize,
    c_org_x: usize,
    c_org_y: usize,
    cw: usize,
    ch: usize,
    frame_cw: usize,
    frame_ch: usize,
    dv: Mv,
    dst: &mut [u8],
) {
    debug_assert_eq!(dv.x & 7, 0);
    debug_assert_eq!(dv.y & 7, 0);
    // mv_q4 = dv (ss=1: `mv.x * (1 << (1-1))`); pos = org + (mv_q4 >> 4)
    // with C's arithmetic shift; subpel = (mv_q4 & 15) ∈ {0, 8}.
    let pos_x = c_org_x as i64 + i64::from(dv.x >> 4);
    let pos_y = c_org_y as i64 + i64::from(dv.y >> 4);
    let half_x = (dv.x & 15) != 0;
    let half_y = (dv.y & 15) != 0;

    // Clamped sampler (edge replication past the frame — C reads its
    // padded border there; see the module bounds note).
    let sample = |x: i64, y: i64| -> u16 {
        let xc = x.clamp(0, frame_cw as i64 - 1) as usize;
        let yc = y.clamp(0, frame_ch as i64 - 1) as usize;
        u16::from(plane[yc * c_stride + xc])
    };

    match (half_x, half_y) {
        (false, false) => {
            for r in 0..ch {
                for c in 0..cw {
                    dst[r * cw + c] = sample(pos_x + c as i64, pos_y + r as i64) as u8;
                }
            }
        }
        (true, false) => {
            // Horizontal half-pel: (a + b + 1) >> 1 over columns x, x+1.
            for r in 0..ch {
                for c in 0..cw {
                    let a = sample(pos_x + c as i64, pos_y + r as i64);
                    let b = sample(pos_x + c as i64 + 1, pos_y + r as i64);
                    dst[r * cw + c] = ((a + b + 1) >> 1) as u8;
                }
            }
        }
        (false, true) => {
            // Vertical half-pel: (a + b + 1) >> 1 over rows y, y+1.
            for r in 0..ch {
                for c in 0..cw {
                    let a = sample(pos_x + c as i64, pos_y + r as i64);
                    let b = sample(pos_x + c as i64, pos_y + r as i64 + 1);
                    dst[r * cw + c] = ((a + b + 1) >> 1) as u8;
                }
            }
        }
        (true, true) => {
            // 2D half-pel: (a + b + c + d + 2) >> 2 over the 2x2.
            for r in 0..ch {
                for c in 0..cw {
                    let (x, y) = (pos_x + c as i64, pos_y + r as i64);
                    let s = sample(x, y) + sample(x + 1, y) + sample(x, y + 1) + sample(x + 1, y + 1);
                    dst[r * cw + c] = ((s + 2) >> 2) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A literal transcription of C `svt_av1_convolve_2d_sr_c` restricted
    /// to the BILINEAR kernel at subpel {0,8} — the oracle the shortcut
    /// arithmetic above must match bit-for-bit. 8-tap loop with the real
    /// zero taps, real offsets (fo=3), real two-stage rounding.
    fn convolve_oracle(
        src: &[u8],
        stride: usize,
        sx: usize,
        sy: usize,
        w: usize,
        h: usize,
        subpel_x: bool,
        subpel_y: bool,
    ) -> vec::Vec<u8> {
        const FILTER_BITS: i32 = 7;
        const ROUND_0: i32 = 3;
        const ROUND_1: i32 = 11;
        let taps_at = |half: bool| -> [i32; 8] {
            if half {
                [0, 0, 0, 64, 64, 0, 0, 0]
            } else {
                [0, 0, 0, 128, 0, 0, 0, 0]
            }
        };
        let round_pot = |v: i32, n: i32| -> i32 { (v + (1 << (n - 1))) >> n };
        let xf = taps_at(subpel_x);
        let yf = taps_at(subpel_y);
        let px = |x: i64, y: i64| -> i32 { i32::from(src[y as usize * stride + x as usize]) };
        let mut out = vec![0u8; w * h];
        if subpel_x && subpel_y {
            // 2D: horizontal into im (h + 7 rows from sy - 3), then vertical.
            let bd = 8;
            let im_h = h + 7;
            let mut im = vec![0i32; im_h * w];
            for y in 0..im_h {
                for x in 0..w {
                    let mut sum = 1 << (bd + FILTER_BITS - 1);
                    for (k, &t) in xf.iter().enumerate() {
                        if t != 0 {
                            sum += t * px(sx as i64 + x as i64 - 3 + k as i64, sy as i64 + y as i64 - 3);
                        }
                    }
                    im[y * w + x] = round_pot(sum, ROUND_0);
                }
            }
            let offset_bits = bd + 2 * FILTER_BITS - ROUND_0;
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 1 << offset_bits;
                    for (k, &t) in yf.iter().enumerate() {
                        if t != 0 {
                            sum += t * im[(y + k) * w + x];
                        }
                    }
                    let res = round_pot(sum, ROUND_1)
                        - ((1 << (offset_bits - ROUND_1)) + (1 << (offset_bits - ROUND_1 - 1)));
                    // bits = 2*FILTER_BITS - ROUND_0 - ROUND_1 = 0.
                    out[y * w + x] = res.clamp(0, 255) as u8;
                }
            }
        } else if subpel_x {
            for y in 0..h {
                for x in 0..w {
                    let mut res = 0i32;
                    for (k, &t) in xf.iter().enumerate() {
                        if t != 0 {
                            res += t * px(sx as i64 + x as i64 - 3 + k as i64, sy as i64 + y as i64);
                        }
                    }
                    let res = round_pot(res, ROUND_0);
                    out[y * w + x] = round_pot(res, FILTER_BITS - ROUND_0).clamp(0, 255) as u8;
                }
            }
        } else if subpel_y {
            for y in 0..h {
                for x in 0..w {
                    let mut res = 0i32;
                    for (k, &t) in yf.iter().enumerate() {
                        if t != 0 {
                            res += t * px(sx as i64 + x as i64, sy as i64 + y as i64 - 3 + k as i64);
                        }
                    }
                    out[y * w + x] = round_pot(res, FILTER_BITS).clamp(0, 255) as u8;
                }
            }
        } else {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = px(sx as i64 + x as i64, sy as i64 + y as i64) as u8;
                }
            }
        }
        out
    }

    fn lcg_frame(seed: &mut u32, n: usize) -> vec::Vec<u8> {
        (0..n)
            .map(|_| {
                *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (*seed >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn luma_copy_exact() {
        let mut seed = 7u32;
        let stride = 64;
        let frame = lcg_frame(&mut seed, stride * 64);
        let mut dst = vec![0u8; 8 * 8];
        // Block at (32, 32), DV (-128, -64) eighth-pel = (-16, -8) px.
        predict_intrabc_luma(&frame, stride, 32, 32, 8, 8, Mv { x: -128, y: -64 }, &mut dst);
        for r in 0..8 {
            for c in 0..8 {
                assert_eq!(dst[r * 8 + c], frame[(24 + r) * stride + 16 + c]);
            }
        }
    }

    /// The chunk-7 pin for map §F.12: every subpel case of the chroma
    /// predictor must match the literal C convolve transcription over
    /// randomized content — including the "odd DV" (half-pel) cases whose
    /// kernel row-select C hardcodes to 8.
    #[test]
    fn chroma_halfpel_matches_convolve_oracle() {
        let mut seed = 42u32;
        let c_stride = 128;
        let (fcw, fch) = (128usize, 128usize);
        let plane = lcg_frame(&mut seed, c_stride * fch);
        // (dv.x, dv.y) eighth-pel; odd/even full-pel combinations.
        let cases: [(i16, i16); 6] = [
            (-64, -32),  // even/even -> copy
            (-56, -32),  // odd/even  -> x half-pel
            (-64, -40),  // even/odd  -> y half-pel
            (-56, -40),  // odd/odd   -> 2d
            (-8, -8),    // minimal odd/odd
            (40, -104),  // positive x odd, negative y odd
        ];
        for (dvx, dvy) in cases {
            let dv = Mv { x: dvx, y: dvy };
            let (cw, ch) = (8usize, 8usize);
            let (ccx, ccy) = (32usize, 32usize);
            let pos_x = (ccx as i64 + i64::from(dvx >> 4)) as usize;
            let pos_y = (ccy as i64 + i64::from(dvy >> 4)) as usize;
            let mut dst = vec![0u8; cw * ch];
            predict_intrabc_chroma(&plane, c_stride, ccx, ccy, cw, ch, fcw, fch, dv, &mut dst);
            let oracle = convolve_oracle(
                &plane,
                c_stride,
                pos_x,
                pos_y,
                cw,
                ch,
                (dvx & 15) != 0,
                (dvy & 15) != 0,
            );
            assert_eq!(dst, oracle, "dv=({dvx},{dvy})");
        }
    }

    #[test]
    fn chroma_neg_dv_floors_like_c_shift() {
        // dv.x = -8 (one odd luma pel left): C `-8 >> 4` = -1 (arithmetic
        // floor), subpel 8 — i.e. sample columns (org-1, org) averaged.
        let c_stride = 32;
        let mut plane = vec![0u8; c_stride * 32];
        for (i, p) in plane.iter_mut().enumerate() {
            *p = (i % 251) as u8;
        }
        let mut dst = vec![0u8; 4 * 4];
        predict_intrabc_chroma(&plane, c_stride, 8, 8, 4, 4, 32, 32, Mv { x: -8, y: 0 }, &mut dst);
        let a = u16::from(plane[8 * c_stride + 7]);
        let b = u16::from(plane[8 * c_stride + 8]);
        assert_eq!(dst[0], ((a + b + 1) >> 1) as u8);
    }
}

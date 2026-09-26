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

/// A reconstruction sample: `u8` at 8 bits, `u16` at 10.
///
/// The IntraBC compensation arithmetic is BIT-DEPTH INDEPENDENT for the
/// depths this encoder accepts, so both predictors below are generic over
/// this rather than duplicated. Derivation (from C
/// `get_conv_params_no_round`, convolve.h:41-66, and the highbd kernels at
/// inter_prediction.c:731-826):
///
/// `round_0 = ROUND0_BITS = 3` and `round_1 = 2*FILTER_BITS - round_0 = 11`
/// at BOTH depths — the `intbufrange > 16` correction that would change them
/// needs `bd + FILTER_BITS - round_0 + 2 > 16`, i.e. `bd > 10`. Feeding those
/// through `svt_av1_highbd_convolve_{x,y,2d}_sr_c` with the BILINEAR kernel at
/// subpel 8 gives, for every `bd <= 10`:
///
/// - x-only: `ROUND(ROUND(64*(a+b), 3), 4)` = `(a+b+1)>>1`
/// - y-only: `ROUND(64*(a+b), 7)` = `(a+b+1)>>1`
/// - 2d: the `1<<(bd+FILTER_BITS-1)` horizontal offset and the
///   `1<<offset_bits` vertical offset are cancelled EXACTLY by the
///   `(1<<(offset_bits-round_1)) + (1<<(offset_bits-round_1-1))` subtraction,
///   leaving `(a+b+c+d+2)>>2` with `bits = 2*FILTER_BITS - round_0 - round_1
///   = 0`.
///
/// i.e. the same three closed forms the 8-bit path already used. The only
/// depth-dependent term is `clip_pixel_highbd`, which cannot bind: an average
/// of in-range samples is in range. **This stops being true at bd12** (round_0
/// becomes 5 and round_1 9), which is why `bit_depth_config_error`
/// (pipeline.rs) refuses 12-bit rather than letting it reach here.
pub trait ReconSample: Copy {
    /// Widen for the intermediate arithmetic.
    fn to_i64(self) -> i64;
    /// Narrow back after averaging (never clips — see the trait docs).
    fn from_i64(v: i64) -> Self;
}

impl ReconSample for u8 {
    #[inline]
    fn to_i64(self) -> i64 {
        i64::from(self)
    }
    #[inline]
    fn from_i64(v: i64) -> Self {
        v as u8
    }
}

impl ReconSample for u16 {
    #[inline]
    fn to_i64(self) -> i64 {
        i64::from(self)
    }
    #[inline]
    fn from_i64(v: i64) -> Self {
        v as u16
    }
}

/// Luma IntraBC predictor: plain copy from the in-progress recon at
/// `(abs_x + dv.x/8, abs_y + dv.y/8)`. `dst` is `w`-strided tightly
/// packed (the funnel's `Cand::pred` convention).
///
/// Generic over [`ReconSample`]: at bd10 the caller passes the 10-bit recon
/// canvas and gets the 10-bit prediction. A whole-pel copy has no arithmetic
/// to be depth-sensitive about.
pub fn predict_intrabc_luma<S: ReconSample>(
    recon: &[S],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    w: usize,
    h: usize,
    dv: Mv,
    dst: &mut [S],
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
pub fn predict_intrabc_chroma<S: ReconSample>(
    plane: &[S],
    c_stride: usize,
    c_org_x: usize,
    c_org_y: usize,
    cw: usize,
    ch: usize,
    frame_cw: usize,
    frame_ch: usize,
    dv: Mv,
    dst: &mut [S],
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
    let sample = |x: i64, y: i64| -> i64 {
        let xc = x.clamp(0, frame_cw as i64 - 1) as usize;
        let yc = y.clamp(0, frame_ch as i64 - 1) as usize;
        plane[yc * c_stride + xc].to_i64()
    };

    match (half_x, half_y) {
        (false, false) => {
            for r in 0..ch {
                for c in 0..cw {
                    dst[r * cw + c] = S::from_i64(sample(pos_x + c as i64, pos_y + r as i64));
                }
            }
        }
        (true, false) => {
            // Horizontal half-pel: (a + b + 1) >> 1 over columns x, x+1.
            for r in 0..ch {
                for c in 0..cw {
                    let a = sample(pos_x + c as i64, pos_y + r as i64);
                    let b = sample(pos_x + c as i64 + 1, pos_y + r as i64);
                    dst[r * cw + c] = S::from_i64((a + b + 1) >> 1);
                }
            }
        }
        (false, true) => {
            // Vertical half-pel: (a + b + 1) >> 1 over rows y, y+1.
            for r in 0..ch {
                for c in 0..cw {
                    let a = sample(pos_x + c as i64, pos_y + r as i64);
                    let b = sample(pos_x + c as i64, pos_y + r as i64 + 1);
                    dst[r * cw + c] = S::from_i64((a + b + 1) >> 1);
                }
            }
        }
        (true, true) => {
            // 2D half-pel: (a + b + c + d + 2) >> 2 over the 2x2.
            for r in 0..ch {
                for c in 0..cw {
                    let (x, y) = (pos_x + c as i64, pos_y + r as i64);
                    let s =
                        sample(x, y) + sample(x + 1, y) + sample(x, y + 1) + sample(x + 1, y + 1);
                    dst[r * cw + c] = S::from_i64((s + 2) >> 2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

use super::*;

/// One row of the C `Quants`/`Dequants` tables for a single qindex,
/// luma, 8-bit: C `svt_av1_build_quantizer` (md_config_process.c:97) with
/// all delta-q 0 and sharpness 0 (`qzbin_factor` per
/// `svt_aom_get_qzbin_factor`, `qrounding_factor = 48` for q > 0).
/// `[0]` = DC, `[1]` = AC.
pub(super) struct QuantEntry {
    pub(super) zbin: [i32; 2],
    pub(super) round: [i32; 2],
    pub(super) quant: [i32; 2],
    pub(super) quant_shift: [i32; 2],
    pub(super) dequant: [i32; 2],
}

/// C `svt_aom_invert_quant` (inv_transforms.c:3507).
pub(super) fn invert_quant(d: i32) -> (i32, i32) {
    let mut t = d as u32;
    let mut l = 0i32;
    while t > 1 {
        t >>= 1;
        l += 1;
    }
    let m = 1i64 + (1i64 << (16 + l)) / d as i64;
    ((m - (1 << 16)) as i32, 1 << (16 - l))
}

pub(super) fn build_quant_entry(qindex: u8) -> QuantEntry {
    let q = qindex as usize;
    let dc = svtav1_dsp::quant_tables::DC_QLOOKUP_8[q] as i32;
    let ac = svtav1_dsp::quant_tables::AC_QLOOKUP_8[q] as i32;
    // svt_aom_get_qzbin_factor (inv_transforms.c:3492), 8-bit.
    let qzbin_factor = if q == 0 {
        64
    } else if dc < 148 {
        84
    } else {
        80
    };
    let qrounding_factor = if q == 0 { 64 } else { 48 };
    let mut e = QuantEntry {
        zbin: [0; 2],
        round: [0; 2],
        quant: [0; 2],
        quant_shift: [0; 2],
        dequant: [0; 2],
    };
    for (i, quant_qtx) in [dc, ac].into_iter().enumerate() {
        let (quant, shift) = invert_quant(quant_qtx);
        e.quant[i] = quant;
        e.quant_shift[i] = shift;
        e.zbin[i] = (qzbin_factor * quant_qtx + 64) >> 7; // ROUND_POWER_OF_TWO(x, 7)
        e.round[i] = (qrounding_factor * quant_qtx) >> 7;
        e.dequant[i] = quant_qtx;
    }
    e
}

/// C `av1_get_tx_scale_tab[TX_SIZES_ALL]` (full_loop.c:22), indexed by the
/// C TxSize value.
pub(super) const TX_SCALE_TAB: [i32; 19] =
    [0, 0, 0, 1, 2, 0, 0, 0, 0, 1, 1, 2, 2, 0, 0, 0, 0, 1, 1];

/// C `svt_aom_quantize_b_c` (full_loop.c:31) without quant matrices
/// (`q_matrix == NULL`): returns (eob, packed qcoeff, packed dqcoeff).
/// `coeffs` is the packed coefficient buffer (row stride = packed width),
/// `scan` the DCT_DCT scan for the tx size, `log_scale` = tx scale.
#[cfg(test)]
pub(super) fn quantize_b(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
) -> (u16, Vec<i32>, Vec<i32>) {
    let mut qcoeff = vec![0i32; coeffs.len()];
    let mut dqcoeff = vec![0i32; coeffs.len()];
    let eob = quantize_b_into(coeffs, scan, e, log_scale, &mut qcoeff, &mut dqcoeff);
    (eob, qcoeff, dqcoeff)
}

/// [`quantize_b`]'s body writing caller-owned output slices — the hot
/// block-cost paths reuse [`Pd0Scratch`] instead of allocating per block.
pub(super) fn quantize_b_into(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    let zbins = e.zbin.map(|v| (v + ((1 << log_scale) >> 1)) >> log_scale);
    let round = e.round.map(|v| (v + ((1 << log_scale) >> 1)) >> log_scale);
    // PD0 quantizes 8-bit residuals even for a high-bit-depth input frame.
    // Reuse the coding path's bd8 raster kernel: the prescan only excluded
    // coefficients inside the same dead zone, and EOB is recovered in scan
    // order after the independent per-coefficient arithmetic.
    svtav1_dsp::quant_coding::quantize_b_raster(
        coeffs,
        qcoeff,
        dqcoeff,
        &zbins,
        &round,
        &e.quant,
        &e.quant_shift,
        &e.dequant,
        log_scale,
    );
    crate::quant::eob_from_qcoeff(scan, qcoeff)
}

/// The former PD0 scan-order body, retained as an independent regression
/// reference in addition to the real scalar/dispatched C parity comparisons.
#[cfg(test)]
pub(super) fn quantize_b_scan_reference(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
) -> (u16, Vec<i32>, Vec<i32>) {
    let n_coeffs = scan.len();
    let zbins = [
        (e.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (e.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];
    let mut qcoeff = vec![0i32; coeffs.len()];
    let mut dqcoeff = vec![0i32; coeffs.len()];

    // Pre-scan pass: find the last scan position outside the zbin dead zone.
    let mut non_zero_count = n_coeffs;
    for i in (0..n_coeffs).rev() {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        if coeff < zbins[iz] && coeff > -zbins[iz] {
            non_zero_count -= 1;
        } else {
            break;
        }
    }

    let mut eob: i64 = -1;
    for i in 0..non_zero_count {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = (coeff ^ coeff_sign) - coeff_sign;
        if abs_coeff >= zbins[iz] {
            let round = (e.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let tmp = (abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32) as i64;
            let tmp32 = (((((tmp * e.quant[iz] as i64) >> 16) + tmp) * e.quant_shift[iz] as i64)
                >> (16 - log_scale)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let abs_dq = ((tmp32 as i64 * e.dequant[iz] as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if tmp32 != 0 {
                eob = i as i64;
            }
        }
    }
    ((eob + 1) as u16, qcoeff, dqcoeff)
}

/// C `svt_av1_quantize_b_qm` — the QM arm of `svt_aom_quantize_inv_quantize_
/// light` (full_loop.c:1346, 8-bit) — i.e. [`quantize_b`] with the frame luma
/// quantization matrix applied. `wt`/`iwt` are the raster-indexed matrix
/// slices from [`crate::qm::qm_slices`]. Mirrors the differentially C-tested
/// [`crate::qm::quantize_b_qm`] (tests/c_parity_qm.rs) on PD0's [`QuantEntry`]
/// (whose zbin/round/quant/quant_shift/dequant fields are identical to
/// `QuantTable`'s). Keeps the bd8-domain INT16 clamp (C's 8-bit kernel clamps
/// `INT16_MIN..INT16_MAX`, av1_quantize.c) — PD0 quantizes 8-bit residuals
/// even at bd10.
/// [`quantize_b_into`]'s QM twin (C `svt_av1_quantize_b_qm`). Writes only
/// the positions that pass the weighted dead zone — `qcoeff`/`dqcoeff` must
/// be zeroed by the caller first.
pub(super) fn quantize_b_qm_into(
    coeffs: &[i32],
    scan: &[u16],
    e: &QuantEntry,
    log_scale: i32,
    wt: &[u8],
    iwt: &[u8],
    qcoeff: &mut [i32],
    dqcoeff: &mut [i32],
) -> u16 {
    const AOM_QM_BITS: i32 = 5;
    let n_coeffs = scan.len();
    let zbins = [
        (e.zbin[0] + ((1 << log_scale) >> 1)) >> log_scale,
        (e.zbin[1] + ((1 << log_scale) >> 1)) >> log_scale,
    ];

    // Pre-scan pass (weighted zbin dead zone).
    let mut non_zero_count = n_coeffs;
    for i in (0..n_coeffs).rev() {
        let rc = scan[i] as usize;
        let w = i32::from(wt[rc]);
        let coeff = coeffs[rc] * w;
        let iz = usize::from(rc != 0);
        if coeff < zbins[iz] * (1 << AOM_QM_BITS) && coeff > -zbins[iz] * (1 << AOM_QM_BITS) {
            non_zero_count -= 1;
        } else {
            break;
        }
    }

    let mut eob: i64 = -1;
    for i in 0..non_zero_count {
        let rc = scan[i] as usize;
        let coeff = coeffs[rc];
        let iz = usize::from(rc != 0);
        let coeff_sign: i32 = if coeff < 0 { -1 } else { 0 };
        let abs_coeff = (coeff ^ coeff_sign) - coeff_sign;
        let w = i64::from(wt[rc]);
        if i64::from(abs_coeff) * w >= i64::from(zbins[iz]) << AOM_QM_BITS {
            let round = (e.round[iz] + ((1 << log_scale) >> 1)) >> log_scale;
            let mut tmp = i64::from((abs_coeff + round).clamp(i16::MIN as i32, i16::MAX as i32));
            tmp *= w;
            let tmp32 = (((((tmp * e.quant[iz] as i64) >> 16) + tmp) * e.quant_shift[iz] as i64)
                >> (16 - log_scale + AOM_QM_BITS)) as i32;
            qcoeff[rc] = (tmp32 ^ coeff_sign) - coeff_sign;
            let dequant =
                (e.dequant[iz] * i32::from(iwt[rc]) + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS;
            let abs_dq = ((tmp32 as i64 * dequant as i64) >> log_scale) as i32;
            dqcoeff[rc] = (abs_dq ^ coeff_sign) - coeff_sign;
            if tmp32 != 0 {
                eob = i as i64;
            }
        }
    }
    (eob + 1) as u16
}

/// C `energy_computation` (transforms.c:3095): sum of squared
/// coefficients over an area.
pub(super) fn energy(coeff: &[i32], stride: usize, w: usize, h: usize) -> u64 {
    if stride == w {
        return svtav1_dsp::residual::sq_sum_i32(&coeff[..w * h]);
    }
    let mut e = 0u64;
    for r in 0..h {
        // The right 32x32 quadrant of a 64-wide transform is strided;
        // exclude the other quadrant and any row padding from its energy.
        e = e.wrapping_add(svtav1_dsp::residual::sq_sum_i32(
            &coeff[r * stride..r * stride + w],
        ));
    }
    e
}

/// The shared perform_tx_pd0 transform+quant+distortion core: forward
/// DCT_DCT at the (possibly subres-halved) max-square tx size, 64-dim
/// energy fold + pack (svt_handle_transform64x64/64x32), quantize at
/// `qindex_off` (the caller applies `rate_est_ctrls.lpd0_qp_offset`),
/// frequency-domain SSE + three_quad_energy, and the dist shift.
///
/// C's PD0 RECON neighbour state for one superblock — the port's model of
/// `ctx->recon_neigh_y` while `pd0_use_src_samples` is FALSE.
///
/// **Why it exists.** `svt_aom_sig_deriv_enc_dec_pd0` sets
/// `ctx->pd0_use_src_samples = allintra || pcs->hbd_md`
/// (enc_mode_config.c:7309), so on a VIDEO frame PD0 does NOT copy the source
/// row/column into the recon-neighbour arrays. Instead every PD0 block
/// predicts from the RECON that PD0 itself generates
/// (`av1_perform_inverse_transform_recon`, product_coding_loop.c:8438) and
/// writes back through `mode_decision_update_neighbor_arrays_pd0` (:121) at
/// the points where a node's partition is DECIDED. MEASURED with the
/// `SVT_PD0COST_OUT` interposer on `gradient 64x64 q40 p6` video: with the
/// level and subres right but the prediction still from source, C and the port
/// agree to the unit on every block that has NO neighbour and diverge on every
/// block that has one.
///
/// **Why a pixel canvas and not the 1-D arrays.** C's neighbour array keeps,
/// per column, the bottom row of the last block written there, and per row the
/// right column. PD0's decided blocks TILE the superblock, so for every read
/// the C array holds exactly the canvas pixel at `(x, y-1)` / `(x-1, y)` — the
/// canvas is equivalent and it lets the existing
/// [`crate::partition::extract_neighbors_tiled`] supply C's `n_top_px` /
/// `n_left_px` clamp and edge replication unchanged.
///
/// The canvas covers rows `sb_y - 1 ..= sb_y + 64` at the frame's aligned
/// stride, seeded from the MD recon of the already-coded superblocks — which
/// is what C's arrays hold at SB entry, since `copy_neighbour_arrays_pd0`
/// snapshots the live MD arrays (enc_dec_process.c:2980) rather than clearing
/// them.
pub(super) struct Pd0ReconCanvas {
    pub(super) buf: alloc::vec::Vec<u8>,
    pub(super) stride: usize,
    /// Frame row the canvas's row 0 corresponds to (`sb_y - 1`, or 0).
    pub(super) y0: usize,
}

/// The number of canvas rows: 1 above + 64 SB + 1 so a 64-tall block's left
/// column read (`abs_y .. abs_y + 64` at canvas row `abs_y - y0`) stays in
/// bounds instead of tripping `extract_neighbors_tiled`'s length guard.
pub(super) const PD0_CANVAS_ROWS: usize = 66;

impl Pd0ReconCanvas {
    /// Seed from the frame's MD recon plane at this SB's origin.
    pub(super) fn new(recon: &[u8], stride: usize, sb_y: usize) -> Self {
        let y0 = sb_y.saturating_sub(1);
        let mut buf = alloc::vec![128u8; stride * PD0_CANVAS_ROWS];
        for r in 0..PD0_CANVAS_ROWS {
            let src = (y0 + r) * stride;
            if src + stride <= recon.len() {
                buf[r * stride..r * stride + stride].copy_from_slice(&recon[src..src + stride]);
            }
        }
        Self { buf, stride, y0 }
    }

    /// C `svt_aom_update_recon_neighbor_array` for one decided node: the
    /// block's recon becomes the neighbour reference for everything below and
    /// to the right of it. The straddle clip is `commit_leaf`'s — a block
    /// reaching past the aligned stride must not wrap into the next row.
    pub(super) fn write(&mut self, abs_x: usize, abs_y: usize, bw: usize, bh: usize, recon: &[u8]) {
        let wr = bw.min(self.stride.saturating_sub(abs_x));
        for r in 0..bh {
            let row = (abs_y + r).saturating_sub(self.y0);
            if row >= PD0_CANVAS_ROWS || wr == 0 {
                continue;
            }
            let dst = row * self.stride + abs_x;
            self.buf[dst..dst + wr].copy_from_slice(&recon[r * bw..r * bw + wr]);
        }
    }
}

/// C `perform_tx_pd0`'s tx size after the subres remap
/// (product_coding_loop.c:4318-4344): the residual is `bw x tx_h` with
/// `tx_h = bh >> mds_subres_step`. Returns the port enum and C's TxSize index.
pub(super) fn pd0_tx_size(bw: usize, tx_h: usize) -> (svtav1_types::transform::TxSize, usize) {
    use svtav1_types::transform::TxSize;
    match (bw, tx_h) {
        (64, 64) => (TxSize::Tx64x64, 4usize),
        (64, 32) => (TxSize::Tx64x32, 12),
        (32, 32) => (TxSize::Tx32x32, 3),
        (32, 16) => (TxSize::Tx32x16, 10),
        (16, 16) => (TxSize::Tx16x16, 2),
        (16, 8) => (TxSize::Tx16x8, 8),
        (8, 8) => (TxSize::Tx8x8, 1),
        (8, 4) => (TxSize::Tx8x4, 6),
        (4, 4) => (TxSize::Tx4x4, 0),
        // Task #95 chunk 2: "tall" rect TX for the PARTITION_VERT boundary
        // block (`sq/2 x sq`) of a right-edge partial-SB node. The AV1 enum
        // indices mirror the "wide" ones (TX_8X16=7, TX_16X32=9, TX_32X64=11).
        (32, 64) => (TxSize::Tx32x64, 11),
        (16, 32) => (TxSize::Tx16x32, 9),
        (8, 16) => (TxSize::Tx8x16, 7),
        // `mds_subres_step == 2` remaps (product_coding_loop.c:4324-4338):
        // TX_64X64 -> TX_64X16, TX_32X32 -> TX_32X8, TX_16X16 -> TX_16X4.
        // (8x8 at step 2 would be TX_8X2, which does not exist — C asserts;
        // a PD0 level that sets step 2 always disallows blocks below 16.)
        (64, 16) => (TxSize::Tx64x16, 18),
        (32, 8) => (TxSize::Tx32x8, 16),
        (16, 4) => (TxSize::Tx16x4, 14),
        _ => unreachable!("PD0 tx {bw}x{tx_h}"),
    }
}

/// Returns (eob, dist, packed qcoeff, packed C TxSize, packed dqcoeff).
///
/// The DEQUANTIZED coefficients come back too because C's video PD0 needs
/// them: with `pd0_use_src_samples = false` the block's RECON feeds the next
/// block's intra prediction, and recon is `pred + inverse_transform(dqcoeff)`.
/// The allintra paths ignore the extra value.
/// The residual is `s.residual[..sq_size * tx_h]`; the packed
/// `qcoeff`/`dqcoeff` outputs land in `s.qcoeff[..used]` /
/// `s.dqcoeff[..used]` where `used = min(sq_size,32) * min(tx_h,32)`.
/// Returns `(eob, dist, c_tx_size)`.
pub(super) fn tx_quant_core(
    s: &mut Pd0Scratch,
    sq_size: usize,
    tx_h: usize,
    qindex_off: u8,
    qm_level: u8,
    subres_step: u32,
) -> (u16, u64, usize) {
    use svtav1_types::transform::TxType;
    let (tx_size, c_tx_size) = pd0_tx_size(sq_size, tx_h);

    let n = sq_size * tx_h;
    let residual_len = n;
    let coeffs = scratch_i32(&mut s.coeffs, n);
    if qindex_off == 0 && sq_size == 4 && tx_h == 4 {
        // C svt_av1_estimate_transform's lossless TX_4X4 branch, including
        // its transposed store. Larger PD0 transforms still use DCT.
        let mut res = [0i16; 16];
        res.copy_from_slice(&s.residual[..16]);
        let mut wht = [0i32; 16];
        svtav1_dsp::fwd_txfm::fwht4x4(&res, &mut wht, 4);
        for r in 0..4 {
            for c in 0..4 {
                coeffs[c * 4 + r] = wht[r * 4 + c];
            }
        }
    } else {
        svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
            &s.residual[..residual_len],
            coeffs,
            sq_size,
            tx_size,
            TxType::DctDct,
        );
    }

    // 64-dim fold + pack (svt_handle_transform64x64 / 64x32 / 32x64).
    let mut three_quad_energy = 0u64;
    if sq_size == 64 {
        if tx_h == 64 {
            three_quad_energy =
                energy(&s.coeffs[32..], 64, 32, 32) + energy(&s.coeffs[32 * 64..], 64, 64, 32);
        } else {
            // svt_handle_transform64x32 (transforms.c:3184) / 64x16 (:3223):
            // the top-right 32-wide quadrant over the transform's own height
            // — 32 rows at subres step 1, 16 at step 2.
            three_quad_energy = energy(&s.coeffs[32..], 64, 32, tx_h);
        }
        let pack_h = tx_h.min(32);
        for row in 1..pack_h {
            for c in 0..32 {
                s.coeffs[row * 32 + c] = s.coeffs[row * 64 + c];
            }
        }
    } else if tx_h == 64 {
        // Tall 32x64 (svt_handle_transform32x64): the block is 32 wide (no
        // width fold), so the top 32 rows are already contiguous — keep them
        // and route the bottom 32 rows' energy to three_quad_energy.
        three_quad_energy = energy(&s.coeffs[sq_size * 32..], sq_size, sq_size, 32);
    }

    let packed_w = sq_size.min(32);
    let packed_h = tx_h.min(32);
    let log_scale = TX_SCALE_TAB[c_tx_size];
    let entry = build_quant_entry(qindex_off);
    let scan = crate::entropy::scan_tables::scan(c_tx_size, 0);
    debug_assert_eq!(scan.len(), packed_w * packed_h);
    // [SVT_HDR_MODE] Quantization matrices in PD0. C's md_encode_block_pd0
    // quantize (`svt_aom_quantize_inv_quantize_light`, full_loop.c:1263)
    // applies the frame's luma QM whenever `frm_hdr.quantization_params.
    // using_qmatrix` is set (fork default ON) — the QM arm calls
    // `svt_av1_quantize_b_qm`. PD0 always transforms DCT_DCT, which
    // IS_2D_TRANSFORM, so the matrix applies whenever the frame luma
    // `qm_level < 15`. The matrix LEVEL is the frame value derived from
    // base_qindex (`frm_hdr.quantization_params.qm[PLANE_Y]`,
    // md_config_process.c:270), NOT the `qindex_off` quant step. C passes
    // `bit_depth = EB_EIGHT_BIT` to the PD0 quantize (product_coding_loop.c:
    // 4397/4471), so even at bd10 it is the 8-bit QM kernel over the 8-bit
    // `quants_8bit` tables `build_quant_entry` already models — this fix is
    // 8-bit-domain and carries no highbd term. Without it PD0 dequantized
    // WITHOUT matrices, so a QM-tipped partition near-tie (top-left 32x32 of
    // a smooth SB) coded SPLIT where C keeps NONE (fork x bd10 Class A).
    let used = packed_w * packed_h;
    let eob = match (qm_level < 15)
        .then(|| crate::qm::qm_slices(usize::from(qm_level), false, c_tx_size))
        .flatten()
    {
        Some((wt, iwt)) => {
            // `quantize_b_qm` writes only the positions that pass the
            // weighted dead zone — the reused buffers must start at zero.
            let q = scratch_i32(&mut s.qcoeff, used);
            let dq = scratch_i32(&mut s.dqcoeff, used);
            q.fill(0);
            dq.fill(0);
            quantize_b_qm_into(&s.coeffs[..used], scan, &entry, log_scale, wt, iwt, q, dq)
        }
        None => {
            let q = scratch_i32(&mut s.qcoeff, used);
            let dq = scratch_i32(&mut s.dqcoeff, used);
            quantize_b_into(&s.coeffs[..used], scan, &entry, log_scale, q, dq)
        }
    };

    // svt_aom_picture_full_distortion32_bits_single: freq-domain SSE
    // (or plain coeff energy when eob == 0) over the packed region.
    let mut dist = if eob > 0 {
        svtav1_dsp::residual::sse_i32(&s.coeffs[..used], &s.dqcoeff[..used])
    } else {
        energy(&s.coeffs, packed_w, packed_w, packed_h)
    };
    dist += three_quad_energy;
    // RIGHT_SIGNED_SHIFT(dist, (MAX_TX_SCALE=1 - tx_scale) * 2) << subres
    let shift = (1 - log_scale) * 2;
    dist = if shift < 0 {
        dist << (-shift)
    } else {
        dist >> shift
    };
    dist <<= subres_step;

    (eob, dist, c_tx_size)
}

// ---------------------------------------------------------------------------
// PD0_LVL_1 coefficient rate (svt_av1_cost_coeffs_txb, contexts 0)
// ---------------------------------------------------------------------------

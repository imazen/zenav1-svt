use super::*;

/// C's SUB-8 chroma arm of `av1_inter_prediction` (`enc_inter_prediction.c:3374`),
/// for one committed-or-candidate 4xN / Nx4 block.
///
/// A sub-8 luma block's chroma covers the PARENT 8x8, so C does not predict it
/// with one MV: `inter_chroma_4xn_pred` stitches it from the covered mode-info
/// cells' own references, MVs and filters, and falls back to this block's own
/// MV over the whole area when one of those cells is INTRA.
///
/// It lives here rather than in `inter_pred_arm` because it needs both the mi
/// grid and the per-reference DPB table, and it is shared by MDS0's candidate
/// prediction and the MDS3 interpolation-filter rebuild — which used to size
/// chroma as `w / 2` and so wrote a 4xN block's chroma at the wrong stride.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_inter_chroma_sub8(
    padded_by_ref: &[Option<&crate::picture::PaddedRef>; 8],
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    ref_frame: i8,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    u_out: &mut [u8],
    v_out: &mut [u8],
    uv_stride: usize,
) {
    let padded = padded_by_ref[ref_frame.max(0) as usize]
        .expect("a sub-8 inter block names a reference with no DPB picture");
    let Some((refu, refv)) = padded.uv.as_ref() else {
        return;
    };
    let mut refs_by_frame: [Option<(&crate::picture::PaddedPlane, _)>; 8] = [None; 8];
    for (i, slot) in refs_by_frame.iter_mut().enumerate() {
        *slot = padded_by_ref[i].and_then(|p| p.uv.as_ref().map(|(u, v)| (u, v)));
    }
    // C fills `xd->mi[0]` from the block being predicted (:3036-3043); every
    // other covered cell comes from the mi grid. Only the cells C's
    // `row_start` / `col_start` walk reaches are read — a (-1, *) cell for a
    // block that is not 4 HIGH would index above the frame on the first mi row.
    let mut mis = [[crate::inter_pred_arm::Sub8ChromaMi::default(); 2]; 2];
    let row_start: i32 = if bh == 4 { -1 } else { 0 };
    let col_start: i32 = if bw == 4 { -1 } else { 0 };
    for dr in row_start..=0 {
        for dc in col_start..=0 {
            mis[(dr + 1) as usize][(dc + 1) as usize] = if dr == 0 && dc == 0 {
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: true,
                    ref_frame,
                    mv,
                    interp_filters,
                }
            } else {
                let idx = ((org_y / 4) as i32 + dr) * grid_stride + (org_x / 4) as i32 + dc;
                let e = &grid[idx as usize];
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: e.use_intrabc || e.ref_frame[0] > 0,
                    ref_frame: e.ref_frame[0],
                    mv: e.mv[0],
                    interp_filters: e.interp_filters,
                }
            };
        }
    }
    let stitched = crate::inter_pred_arm::predict_inter_chroma_sub8x8(
        &refs_by_frame,
        &mis,
        org_x,
        org_y,
        bw,
        bh,
        sb_size,
        frame_w,
        frame_h,
        u_out,
        v_out,
        uv_stride,
    );
    if !stitched {
        crate::inter_pred_arm::predict_inter_chroma_whole(
            refu,
            refv,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            u_out,
            v_out,
            uv_stride,
            1,
            1,
        );
    }
}

/// The 10-bit twin of [`predict_inter_chroma_sub8`]: same covered-cell walk
/// (`inter_chroma_4xn_pred` reads `is16bit` and runs the identical walk on
/// the 16-bit picture), sourcing each cell's reference from the `hbd` DPB
/// twin instead of the 8-bit plane.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_inter_chroma_sub8_hbd(
    padded_by_ref: &[Option<&crate::picture::PaddedRef>; 8],
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
    ref_frame: i8,
    mv: Mv,
    interp_filters: u32,
    sb_size: usize,
    frame_w: usize,
    frame_h: usize,
    bit_depth: u8,
    u_out: &mut [u16],
    v_out: &mut [u16],
    uv_stride: usize,
) {
    let padded = padded_by_ref[ref_frame.max(0) as usize]
        .expect("a sub-8 inter block names a reference with no DPB picture");
    let Some(hbd) = padded.hbd.as_ref() else {
        return;
    };
    let Some((refu, refv)) = hbd.uv.as_ref() else {
        return;
    };
    let mut refs_by_frame: [Option<(
        &crate::picture::PaddedPlaneHbd,
        &crate::picture::PaddedPlaneHbd,
    )>; 8] = [None; 8];
    for (i, slot) in refs_by_frame.iter_mut().enumerate() {
        *slot = padded_by_ref[i]
            .and_then(|p| p.hbd.as_ref())
            .and_then(|h| h.uv.as_ref().map(|(u, v)| (u, v)));
    }
    let mut mis = [[crate::inter_pred_arm::Sub8ChromaMi::default(); 2]; 2];
    let row_start: i32 = if bh == 4 { -1 } else { 0 };
    let col_start: i32 = if bw == 4 { -1 } else { 0 };
    for dr in row_start..=0 {
        for dc in col_start..=0 {
            mis[(dr + 1) as usize][(dc + 1) as usize] = if dr == 0 && dc == 0 {
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: true,
                    ref_frame,
                    mv,
                    interp_filters,
                }
            } else {
                let idx = ((org_y / 4) as i32 + dr) * grid_stride + (org_x / 4) as i32 + dc;
                let e = &grid[idx as usize];
                crate::inter_pred_arm::Sub8ChromaMi {
                    is_inter: e.use_intrabc || e.ref_frame[0] > 0,
                    ref_frame: e.ref_frame[0],
                    mv: e.mv[0],
                    interp_filters: e.interp_filters,
                }
            };
        }
    }
    let stitched = crate::inter_pred_arm::predict_inter_chroma_sub8x8_hbd(
        &refs_by_frame,
        &mis,
        org_x,
        org_y,
        bw,
        bh,
        sb_size,
        frame_w,
        frame_h,
        bit_depth,
        u_out,
        v_out,
        uv_stride,
    );
    if !stitched {
        crate::inter_pred_arm::predict_inter_chroma_whole_hbd(
            refu,
            refv,
            org_x,
            org_y,
            bw,
            bh,
            mv,
            interp_filters,
            sb_size,
            frame_w,
            frame_h,
            bit_depth,
            u_out,
            v_out,
            uv_stride,
            1,
            1,
        );
    }
}

/// The ABOVE row and LEFT column the OBMC walk reads, projected out of the mi
/// grid into caller-owned arrays.
///
/// C reads `xd->mi` in place; this is the same span, copied so the borrow is
/// local. Both are bounded by one superblock edge in mi units plus the cell
/// the 4-wide pairing rule reaches past it, so neither allocates.
pub(crate) struct ObmcNbSpans {
    pub above: [crate::obmc_pred_arm::ObmcNbCell; OBMC_NB_SPAN],
    pub n_above: usize,
    pub left: [crate::obmc_pred_arm::ObmcNbCell; OBMC_NB_SPAN],
    pub n_left: usize,
}

/// One superblock edge in mi units (128 / 4) plus the pairing cell.
pub(crate) const OBMC_NB_SPAN: usize = 33;

pub(super) const OBMC_NB_INTRA: crate::obmc_pred_arm::ObmcNbCell =
    crate::obmc_pred_arm::ObmcNbCell {
        bsize: svtav1_types::block::BlockSize::Block4x4,
        overlappable: false,
        ref_frame: 0,
        mv: Mv::ZERO,
        interp_filters: 0,
    };

/// Project the two mi spans the OBMC walks read.
///
/// `mi_row == 0` has no ABOVE row and `mi_col == 0` no LEFT column; C gates
/// those with `xd->up_available` / `left_available`, which the caller passes
/// on rather than inferring from an empty span.
pub(crate) fn obmc_nb_spans(
    grid: &[crate::intrabc_mvp::MvpMiEntry],
    grid_stride: i32,
    mi_rows: i32,
    mi_cols: i32,
    org_x: usize,
    org_y: usize,
    bw: usize,
    bh: usize,
) -> ObmcNbSpans {
    let (mi_row, mi_col) = ((org_y / 4) as i32, (org_x / 4) as i32);
    let (n4_w, n4_h) = (bw / 4, bh / 4);
    let mut out = ObmcNbSpans {
        above: [OBMC_NB_INTRA; OBMC_NB_SPAN],
        n_above: 0,
        left: [OBMC_NB_INTRA; OBMC_NB_SPAN],
        n_left: 0,
    };
    let cell = |idx: i32| -> crate::obmc_pred_arm::ObmcNbCell {
        let e = &grid[idx as usize];
        crate::obmc_pred_arm::ObmcNbCell {
            bsize: svtav1_types::block::BlockSize::from_u8(e.bsize)
                .unwrap_or(svtav1_types::block::BlockSize::Block4x4),
            // C `is_neighbor_overlappable`: `ref_frame[0] > INTRA_FRAME`.
            // IntraBC is NOT overlappable — its `ref_frame[0]` is INTRA_FRAME.
            overlappable: e.ref_frame[0] > 0,
            ref_frame: e.ref_frame[0],
            mv: e.mv[0],
            interp_filters: e.interp_filters,
        }
    };
    if mi_row > 0 {
        // One past `n4_w`, because the pairing rule can read `idx + 1`.
        let n = ((n4_w + 1).min(OBMC_NB_SPAN)).min((mi_cols - mi_col).max(0) as usize + 1);
        for k in 0..n {
            let c = mi_col + k as i32;
            if c >= mi_cols {
                break;
            }
            out.above[k] = cell((mi_row - 1) * grid_stride + c);
            out.n_above = k + 1;
        }
    }
    if mi_col > 0 {
        let n = ((n4_h + 1).min(OBMC_NB_SPAN)).min((mi_rows - mi_row).max(0) as usize + 1);
        for k in 0..n {
            let r = mi_row + k as i32;
            if r >= mi_rows {
                break;
            }
            out.left[k] = cell(r * grid_stride + mi_col - 1);
            out.n_left = k + 1;
        }
    }
    out
}

/// `block_mi.interinter_comp` → the masked arm's `InterInterCompoundData`
/// + the DIFFWTD `mask_type`, and `dist_wtd_comp_weight_assign`'s outputs
/// for ref 1's convolve (enc_inter_prediction.c:3253-3264 — ref 0 is
/// `bck_frame_index`, ref 1 `fwd_frame_index`, `order_idx` 0). Shared by
/// the inject-time prediction and the IFS/MDS3 rebuild: C's driver runs
/// this setup once per `svt_aom_inter_prediction` call, so each port call
/// site re-derives it from the same candidate fields.
#[allow(clippy::type_complexity)]
pub(crate) fn compound_md_args(
    f: &InterMdFrame<'_>,
    ref_frame: [i8; 2],
    interinter_comp_type: u8,
    interinter_mask_type: u8,
    interinter_wedge_index: i8,
    interinter_wedge_sign: bool,
    compound_idx: u8,
) -> (
    Option<(
        svtav1_dsp::port_masked_blend::InterInterCompoundData,
        svtav1_dsp::port_masked_compound::DiffwtdMaskType,
    )>,
    Option<svtav1_dsp::port_scale_factors::DistWtdWeights>,
) {
    use svtav1_dsp::port_masked_compound::{CompoundType, DiffwtdMaskType};
    let is_compound = ref_frame[1] > crate::inter_mvp::INTRA_FRAME;
    let comp_type = match interinter_comp_type {
        0 => CompoundType::Average,
        1 => CompoundType::DistWtd,
        2 => CompoundType::Wedge,
        3 => CompoundType::DiffWtd,
        t => panic!("an injected compound candidate carries interinter_comp_type {t}"),
    };
    let masked =
        is_compound && svtav1_dsp::port_masked_compound::is_masked_compound_type(comp_type);
    let comp_data = masked.then(|| {
        (
            svtav1_dsp::port_masked_blend::InterInterCompoundData {
                compound_type: comp_type,
                wedge_index: interinter_wedge_index.max(0) as usize,
                wedge_sign: usize::from(interinter_wedge_sign),
            },
            match interinter_mask_type {
                0 => DiffwtdMaskType::D38,
                1 => DiffwtdMaskType::D38Inv,
                t => panic!("an injected DIFFWTD candidate carries mask_type {t}"),
            },
        )
    });
    // C's call is inside the `is_compound` arm — a single-reference or
    // inter-intra candidate never reaches the weight assign.
    let distwtd = is_compound.then(|| {
        svtav1_dsp::port_scale_factors::dist_wtd_comp_weight_assign(
            f.order_hint.enable_order_hint,
            f.order_hint.order_hint_bits as i32,
            f.order_hint.cur_order_hint,
            f.order_hint.ref_order_hint[(ref_frame[0] - 1) as usize],
            f.order_hint.ref_order_hint[(ref_frame[1] - 1) as usize],
            i32::from(compound_idx),
            0,
            true,
            svtav1_dsp::port_scale_factors::DistWtdWeights {
                fwd_offset: 0,
                bck_offset: 0,
                use_dist_wtd_comp_avg: 0,
            },
        )
    });
    (comp_data, distwtd)
}

/// C `av1_use_angle_delta(bsize)` (reconintra.h:59): `bsize >= BLOCK_8X8` in
/// enum order — true for every block size except BLOCK_4X4, BLOCK_4X8 and
/// BLOCK_8X4 (the 4:1 rects 4x16/16x4 come AFTER BLOCK_128X128 in the enum).
/// C `get_ref_obj` (rc_process.c:61) narrowed to the four fields the three
/// `get_ref_*_percentage` readers use: the DPB slot's `slice_type` and its
/// three coded-area percentages.
///
/// `None` for an empty slot, which C cannot express — it would dereference a
/// null `object_ptr` — so `None` is this port refusing rather than reproducing
/// UB, the same choice `port_rc_process::get_ref_obj` documents.
pub(super) fn ref_obj_stats(
    dpb: &crate::picture::DecodedPictureBuffer,
    slot: usize,
) -> Option<crate::port_rc_process::RefObjStats> {
    let rf = dpb.get(slot)?;
    Some(crate::port_rc_process::RefObjStats {
        slice_type: if rf.is_islice {
            crate::port_rc_process::SliceType::I
        } else {
            crate::port_rc_process::SliceType::B
        },
        intra_coded_area: rf.intra_coded_area,
        skip_coded_area: rf.skip_coded_area,
        hp_coded_area: rf.hp_coded_area,
    })
}

/// The FRAME-level inputs `resolve_sb_lpd1` reads — everything it needs that
/// is constant across the picture's superblocks. Built once in
/// `encode_frame_impl` where [`MdConfigSignals`] / `PipelineMdInputs` /
/// `pic_decision` are in scope, and threaded into `encode_tile_rows`; the
/// per-superblock fields (`qp_index`, the lambda, `disallow_below_64x64`,
/// the PD0 root eval) are supplied by the caller inside the SB loop.
#[derive(Clone, Copy)]
pub(crate) struct Lpd1FrameIn {
    /// `pcs->lpd1_lvl` — the picture Light-PD1 level `sig_deriv_enc_dec_common`
    /// resolved (`MdConfigSignals::pic_lpd1_lvl`).
    pub pic_lpd1_lvl: u8,
    /// `pcs->enc_mode` — `eff_enc_mode`.
    pub enc_mode: i8,
    /// `pcs->slice_type == B_SLICE`.
    pub is_b_slice: bool,
    /// `ppcs->input_resolution`.
    pub input_resolution: crate::port_enc_mode_config::ResolutionRange,
    /// `ppcs->picture_qp`.
    pub picture_qp: u32,
    /// `ppcs->ref_list0_count_try` / `ref_list1_count_try`.
    pub ref_list0_count_try: u32,
    pub ref_list1_count_try: u32,
    /// The `SvtReference` the frame is being encoded against — selects the
    /// fork arms inside `set_lpd1_ctrls` (`!subsampling_x` forces level 0 on
    /// Ghost Robot's `f67a0f747`).
    pub reference: crate::reference::SvtReference,
    /// `pcs->mimic_only_tx_4x4` — C's `frm_hdr.coded_lossless` post-step
    /// (`md_config_process.c:1086`). Ghost Robot's `a74cfb9ec` reads it to
    /// force `lpd1_lvl`/`pd1_lvl_refinement` to 0; on the older references
    /// it is unread here.
    pub coded_lossless: bool,
    /// `pcs->ref_skip_percentage`.
    pub ref_skip_percentage: u8,
    /// `ppcs->use_best_me_unipred_cand_only`.
    pub use_best_me_unipred_cand_only: u8,
    /// `ctx->nic_pruning_ctrls.merge_inter_cands_mult` — `4` at nic level >= 6.
    pub merge_inter_cands_mult: u8,
    /// The `MdConfigSignals` ladder fields `sig_deriv_enc_dec_light_pd1_default`
    /// reads back off `pcs`.
    pub cand_reduction_level: u8,
    pub rdoq_level: u8,
    pub coeff_shaving_level: u8,
    pub me_subpel_level: u8,
    pub rate_est_level: u8,
    pub approx_inter_rate: u8,
    pub intra_level: u32,
}

/// C `md_encode_block`'s Light-PD1 dispatch for ONE superblock
/// (`enc_dec_process.c:3036-3090`): the picture's `set_lpd1_ctrls` level,
/// refined per-SB by `lpd1_detector_*` off the just-computed PD0 result. A
/// level above `REGULAR_PD1` routes the leaf walk to
/// `md_encode_block_light_pd1` — the light candidate set + light MDS0 +
/// light full loop the funnel reads off [`FunnelCtx::lpd1`].
///
/// `root` is the PD0 eval's ROOT node — C's `pc_tree->rdc.rd_cost` +
/// `block_data[PART_N][0]`. `pic_pd0_lvl` is the POST-`pd0_detector`
/// `Pd0Level` (0..=6). Returns `None` on the regular path (the overwhelming
/// majority of superblocks — `pic_lpd1_lvl` is nonzero only for non-base
/// inter pictures at M7..M13).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_sb_lpd1(
    i: &Lpd1FrameIn,
    sb: &crate::port_pd0_detector::Pd0SbInput,
    frame: &crate::part_arm::Pd0DetFrame<'_>,
    root: Option<&crate::pd0::Pd0RootDet>,
    pic_pd0_lvl: u8,
    sb_index: usize,
    qp_index: u32,
    lambda_8bit: u64,
    disallow_below_64x64: bool,
) -> Option<crate::leaf_funnel::light::Lpd1Leaf> {
    use crate::port_enc_mode_config::common::{REGULAR_PD1, set_lpd1_ctrls};
    use crate::port_enc_mode_config::enc_mode as em;
    // `sig_deriv_enc_dec_common`'s non-rtc `lpd1_lvl` arm
    // (enc_mode_config.c:7163-7175): `pic_lpd1_lvl` through M10, an ME bump
    // above it.
    let mut lpd1_lvl = i32::from(i.pic_lpd1_lvl);
    // Ghost Robot `a74cfb9ec` (`enc_mode_config.c:7241`): `mimic_only_tx_4x4`
    // takes the WHOLE ladder off before it runs — "SB-level adaptation must
    // not re-enable the light path". Collapsed here to `lpd1_lvl = 0`, which
    // `set_lpd1_ctrls` maps to REGULAR_PD1 → the `None` return below.
    if i.reference == crate::reference::SvtReference::GhostRobot && i.coded_lossless {
        lpd1_lvl = 0;
    }
    if i.enc_mode > em::M10 && !sb.slice_type_is_intra {
        let me_8x8 = sb.me_8x8_cost_variance;
        // `3 * ctx->qp_index` at <= M8, a flat 3000 above — and this arm only
        // runs above M10, so the threshold is always 3000.
        let th = if i.enc_mode <= em::M8 {
            3 * qp_index
        } else {
            3000
        };
        if lpd1_lvl == 0 {
            if me_8x8 < th {
                lpd1_lvl += 3;
            }
        } else if me_8x8 < th {
            lpd1_lvl += 2;
        }
    }
    let lpd1_lvl = lpd1_lvl.clamp(0, 7);
    let dbg = crate::dbgenv::lpd1dbg();
    if dbg {
        eprintln!("RLPD1 sb={sb_index} lvl={lpd1_lvl} pic={}", i.pic_lpd1_lvl);
    }
    // `ctx->subsampling_x` is always 1 on this path — the port's envelope is
    // 4:2:0, and a monochrome frame never reaches the LPD1 detector — so the
    // fork's `!subsampling_x` collapse cannot fire here regardless.
    let mut ctrls = set_lpd1_ctrls(u8::try_from(lpd1_lvl).ok()?, 1, i.reference)?;
    // A level that resolves to `REGULAR_PD1` (-1) cannot be raised by the
    // detector — it only demotes — so the regular path wins by construction.
    if ctrls.pd1_level <= REGULAR_PD1 {
        return None;
    }
    // Non-rtc `pd1_lvl_refinement` (enc_mode_config.c:7179-7183): 0 through
    // M10, 2 above. Ghost Robot `a74cfb9ec` then collapses it to 0 when
    // `!ctx->subsampling_x || pcs->mimic_only_tx_4x4` (`:7288`) — the
    // `subsampling_x` half cannot fire here (4:2:0 envelope, always 1), the
    // `coded_lossless` half can.
    let pd1_lvl_refinement = if i.enc_mode <= em::M10
        || (i.reference == crate::reference::SvtReference::GhostRobot && i.coded_lossless)
    {
        0
    } else {
        2
    };
    // `skip_pd_pass_0` (enc_dec_process.c:2959): `disallow_below_64x64 &&
    // (sb_size==64 || max_block_size==64) || (disallow_below_32x32 &&
    // max_block_size==32)`. On the 64x64 video arm `max_block_size` is 64, so
    // this reduces to `disallow_below_64x64`.
    let skip_pd_pass_0 = disallow_below_64x64;
    if dbg {
        eprintln!(
            "RLPD1 sb={sb_index} pre={} pic={} pd0={pic_pd0_lvl} skip0={skip_pd_pass_0} \
             root={:?}",
            ctrls.pd1_level,
            i.pic_lpd1_lvl,
            root.map(|r| (
                r.rd_cost,
                r.blk.map(|b| (b.nz, b.is_inter, b.mv0.x, b.mv0.y))
            )),
        );
    }
    if skip_pd_pass_0 || pic_pd0_lvl == 6 {
        crate::port_pd0_detector::lpd1_detector_skip_pd0(&mut ctrls, pd1_lvl_refinement, sb);
    } else {
        // `post_pd0` reads the PD0 root's `block_data[PART_N][0]`; a missing
        // capture (the `None` arm) cannot reproduce C's read, so fall back to
        // the regular path rather than guess at a demotion.
        let root = root?;
        crate::port_pd0_detector::lpd1_detector_post_pd0(
            &mut ctrls,
            pd1_lvl_refinement,
            sb,
            root,
            u32::try_from(lambda_8bit).unwrap_or(u32::MAX),
        );
    }
    if dbg {
        eprintln!("RLPD1 sb={sb_index} post={}", ctrls.pd1_level);
    }
    if ctrls.pd1_level <= REGULAR_PD1 {
        return None;
    }
    // `svt_aom_sig_deriv_enc_dec_light_pd1_default` (enc_mode_config.c:7378)
    // — the per-SB signal set the light lane's gates read.
    let sig = crate::port_enc_mode_config::light_pd1::sig_deriv_enc_dec_light_pd1_default(
        crate::port_enc_mode_config::light_pd1::LightPd1Inputs {
            lpd1_level: ctrls.pd1_level,
            enc_mode: i.enc_mode,
            input_resolution: i.input_resolution,
            is_b_slice: i.is_b_slice,
            picture_qp: i.picture_qp,
            is_ref_l0_avail: frame.l0.avail,
            is_ref_l1_avail: frame.l1.avail,
            ref_list1_count_try: i.ref_list1_count_try,
            me_8x8_cost_variance: sb.me_8x8_cost_variance,
            me_64x64_distortion: sb.me_64x64_distortion,
            l0_sb_skip: frame
                .l0
                .sb_skip
                .and_then(|v| v.get(sb_index).copied())
                .unwrap_or(0),
            l1_sb_skip: frame
                .l1
                .sb_skip
                .and_then(|v| v.get(sb_index).copied())
                .unwrap_or(0),
            l0_sb_64x64_mvp: frame
                .l0
                .mvp_64x64
                .and_then(|v| v.get(sb_index).copied())
                .unwrap_or(0),
            l1_sb_64x64_mvp: frame
                .l1
                .mvp_64x64
                .and_then(|v| v.get(sb_index).copied())
                .unwrap_or(0),
            ref_skip_percentage: i.ref_skip_percentage,
            cand_reduction_level: i.cand_reduction_level,
            rdoq_level: i.rdoq_level,
            coeff_shaving_level: i.coeff_shaving_level,
            me_subpel_level: i.me_subpel_level,
            rate_est_level: i.rate_est_level,
            approx_inter_rate: i.approx_inter_rate,
            intra_level: u8::try_from(i.intra_level).unwrap_or(u8::MAX),
            ref_list0_count_try: i.ref_list0_count_try,
            use_best_me_unipred_cand_only: i.use_best_me_unipred_cand_only,
            // `scs->static_config.rtc && ppcs->hierarchical_levels == 0` —
            // false on the video (non-rtc) arm by definition.
            use_flat_ipp: false,
            is_not_last_layer: frame.is_not_last_layer,
        },
    )?;
    Some(crate::leaf_funnel::light::Lpd1Leaf {
        sig,
        lambda: lambda_8bit,
        qp_index,
        merge_inter_cands_mult: i.merge_inter_cands_mult,
    })
}

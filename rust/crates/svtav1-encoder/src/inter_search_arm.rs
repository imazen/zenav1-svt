//! C's per-block MD motion-search driver — the loop over
//! `ctx->ref_frame_type_arr` that `product_coding_loop.c:9425-9447` runs
//! before `generate_md_stage_0_cand`.
//!
//! ```text
//! build_single_ref_mvp_array(pcs, ctx);   // :9429, gated
//! read_refine_me_mvs(pcs, ctx, pc_tree);  // :9431
//! ... reset pme_res ...                   // :9433-9438
//! perform_md_reference_pruning(pcs, ctx); // :9441, gated
//! pme_search(pcs, ctx, input_pic);        // :9445, gated on updated_enable_pme
//! ```
//!
//! # Why this is a module and not three calls in `inter_md_arm`
//!
//! Every leaf here was already ported with no caller
//! ([`crate::port_md::md_search`]'s `build_single_ref_mvp_list`,
//! `best_mvp_by_distortion`, `refine_me_mv_for_ref`, `pme_search_for_ref`,
//! `md_subpel_search`, `md_nsq_motion_search`, plus
//! [`crate::md_subpel`]'s two tree searches). What was missing is the
//! LOOP: each iteration needs a different reference picture, a different
//! MVP stack, a different MVP list and its own `mv_cost_params`. That
//! loop is this file; `inter_md_arm` keeps the candidate half.
//!
//! # Why it is atomic with the reference set
//!
//! MEASURED 2026-09-02 (`docs/INTER-ENCODE-PLAN.md` §1z¹⁴). On
//! `gradient 64x64 q40 p8` frame 1, C's `me_candidate_array` for the coded
//! 64x64 is `[dir=1, dir=2]` — a LIST-1 unipred and a BI_PRED, and no
//! list-0 entry at all. `inject_new_candidates` can therefore only produce
//! a BWDREF NEWMV, `inject_mvp_candidates_ii` never produces a NEWMV, so
//! **the LAST_FRAME NEWMV that C codes on that cell exists only because
//! `inject_pme_candidates` ran.** Widening `ref_frame_type_arr` without
//! this driver hands MD a BWDREF candidate and no LAST one; that was
//! implemented and measured at `inter_byte_gate` 23 of 36 FAILING.
//!
//! # Evidence tier
//!
//! Tier 4 for the driver (`read_refine_me_mvs`, `pme_search` and
//! `build_single_ref_mvp_array` are all `static` with no exported symbol),
//! on top of tier-1 leaves: `svt_av1_find_best_sub_pixel_tree_pruned`,
//! `svt_aom_choose_best_av1_mv_pred`, `svt_pme_sad_loop_kernel`,
//! `svt_aom_fp_mv_err_cost` and `clip_mv_on_pic_boundary` are exported and
//! driven by `c_parity_*` tests rather than re-transcribed here.
//!
//! The per-block JOIN against C is `SVT_SUBPEL_OUT`
//! (`tools/capture_c_trace/wrap_recon.c`), which fires INSIDE
//! `svt_av1_find_best_sub_pixel_tree_pruned` — once per
//! `(block, list_idx, ref_idx, search_stage)` — and prints C's
//! `mvp_array` / `fp_me_mv` / `fp_me_dist` / start and result MVs at that
//! instant. **Do not use `SVT_INJCFG_OUT`'s `PMEST` line for those
//! fields**: it reads them at neighbour-array-update time, which is after
//! the whole depth has been searched, so they belong to whatever block MD
//! processed last.

use crate::inter_mvp::{InterMvpStack, av1_set_ref_frame, get_list_idx, get_ref_frame_idx};
use crate::picture::PaddedRef;
use crate::port_md::md_search::{
    DistortionType, FullPelCtx, MdPmeCtrls, PlaneDistortion, RefPicGeom, RefineMeIn,
    SubpelBlockGeom, best_mvp_by_distortion, build_single_ref_mvp_list, md_subpel_search,
    pme_search_for_ref, refine_me_mv_for_ref,
};
use crate::port_md::pme::{MvCostParams, MvCostTable, MvCostType};
use alloc::vec::Vec;
use svtav1_types::motion::Mv;

/// C `REF_LIST_MAX_DEPTH`.
pub const REF_LIST_MAX_DEPTH: usize = 4;

/// The frame-constant halves of C's `ModeDecisionContext` that the two
/// searches read.
///
/// Every field is a picture-level signal on this port: no per-SB delta-q is
/// signalled, so `full_lambda_md[EB_8_BIT_MD]` / `fast_lambda_md[..]` do not
/// vary by superblock, and the four control structs come from
/// `svt_aom_sig_deriv_enc_dec_default`, which takes no per-SB input.
pub struct SearchFrameCfg {
    /// C `ctx->md_pme_ctrls`.
    pub md_pme: MdPmeCtrls,
    /// C `ctx->md_pme_ctrls.dist_type` — carried separately because the
    /// ported [`MdPmeCtrls`] holds only the fields the PREDICATES read.
    pub pme_dist_type: DistortionType,
    /// C's qp-modulated `full_pel_search_{width,height}`
    /// (product_coding_loop.c:3203-3211) — applied once at the frame level
    /// because `svt_aom_get_qp_based_th_scaling_factors` reads
    /// `static_config.qp`, which is frame-constant.
    pub pme_full_pel_w: u8,
    pub pme_full_pel_h: u8,
    /// C `ctx->md_subpel_me_ctrls`.
    pub md_subpel_me: crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls,
    /// C `ctx->md_subpel_pme_ctrls`.
    pub md_subpel_pme: crate::port_enc_mode_config::encdec::MdSubPelSearchCtrls,
    /// C `ctx->md_nsq_me_ctrls.enabled`.
    pub md_nsq_me_enabled: bool,
    /// C `ctx->ref_pruning_ctrls.enabled`.
    pub ref_pruning_enabled: bool,
    /// C `ctx->updated_enable_pme` (product_coding_loop.c:9418-9422).
    pub updated_enable_pme: bool,
    /// C `ctx->full_lambda_md[EB_8_BIT_MD]`.
    pub full_lambda_8bit: u32,
    /// C `ctx->fast_lambda_md[EB_8_BIT_MD]`.
    pub fast_lambda_8bit: u32,
    /// C `frm_hdr->quantization_params.base_q_idx`.
    pub base_q_idx: u8,
    /// C `svt_aom_get_sad_per_bit(base_q_idx, 0)`.
    pub sad_per_bit: i32,
    pub allow_high_precision_mv: bool,
    /// C `ctx->shut_fast_rate`.
    pub shut_fast_rate: bool,
    /// C `ctx->approx_inter_rate`.
    pub approx_inter_rate: u8,
    /// C `pcs->ppcs->enhanced_pic->width` / `height` — the PME early
    /// MVP-vs-ME check's resolution term.
    pub pic_width: u32,
    pub pic_height: u32,
}

/// The per-block inputs, all of which the caller already has.
pub struct BlockSearchIn<'a> {
    pub org_x: usize,
    pub org_y: usize,
    pub bw: usize,
    pub bh: usize,
    pub bsize: u8,
    /// C `ctx->blk_geom->sq_size`.
    pub sq_size: u16,
    pub mi_rows: i32,
    pub mi_cols: i32,
    /// C `pcs->ppcs->enhanced_pic` luma and its stride.
    pub src: &'a [u8],
    pub src_stride: usize,
    /// C `ctx->ref_frame_type_arr[0..tot_ref_frame_types]`.
    pub ref_frame_type_arr: &'a [i8],
    /// The padded DPB picture per `MvReferenceFrame` (index 1..=7).
    pub padded_by_ref: &'a [Option<&'a PaddedRef>; 8],
    /// C `ctx->ref_mv_stack[frame_type]`, one per `MvReferenceFrame`.
    pub stacks: &'a [InterMvpStack],
    /// C `blk_ptr->av1xd->ref_mv_count[frame_type]`.
    pub ref_mv_count: &'a [u8; 8],
    /// C `md_rate_est_ctx->nmv_vec_cost` + `nmvcoststack`.
    pub nmv: &'a MvCostTable,
    /// C `md_rate_est_ctx->drl_mode_fac_bits`.
    pub drl_mode_fac_bits: &'a [[i32; 2]; crate::port_md::drl::DRL_MODE_CONTEXTS],
    /// The same tables in the shape `md_subpel` wants.
    pub search_tables: &'a crate::intrabc::MvCostTables,
    /// This frame's open-loop ME.
    pub me: &'a crate::inter_me_arm::FrameMe,
}

/// What the driver leaves for `inject_inter_candidates`.
#[derive(Debug, Clone)]
pub struct BlockSearchOut {
    /// C `ctx->sb_me_mv[list][ref]`.
    pub sb_me_mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->post_subpel_me_mv_cost[list][ref]`.
    pub post_subpel_me_mv_cost: [[u32; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->valid_pme_mv[list][ref]`.
    pub valid_pme_mv: [[bool; REF_LIST_MAX_DEPTH]; 2],
    /// C `ctx->best_pme_mv[list][ref]`.
    pub best_pme_mv: [[Mv; REF_LIST_MAX_DEPTH]; 2],
    /// Which of C's four exits produced each entry, `None` where
    /// `pme_search` never looked at that pair.
    ///
    /// Not a C field — C distinguishes the exits only by control flow. It is
    /// here for the same reason [`crate::port_md::md_search::PmeExit`]
    /// exists: `valid_pme_mv = 1` says nothing about whether a SEARCH
    /// happened, so without it a test cannot tell "PME ran" from "PME handed
    /// back the ME MV", and the positive control below would pass with the
    /// search deleted.
    pub pme_exit: [[Option<crate::port_md::md_search::PmeExit>; REF_LIST_MAX_DEPTH]; 2],
}

impl Default for BlockSearchOut {
    fn default() -> Self {
        Self {
            sb_me_mv: [[Mv::ZERO; REF_LIST_MAX_DEPTH]; 2],
            // C's `(int32_t)~0` initialisation.
            post_subpel_me_mv_cost: [[u32::MAX; REF_LIST_MAX_DEPTH]; 2],
            valid_pme_mv: [[false; REF_LIST_MAX_DEPTH]; 2],
            best_pme_mv: [[Mv::ZERO; REF_LIST_MAX_DEPTH]; 2],
            pme_exit: [[None; REF_LIST_MAX_DEPTH]; 2],
        }
    }
}

/// The per-reference state C keeps in `ModeDecisionContext` between the
/// three passes.
#[derive(Clone, Default)]
struct RefState {
    mvps: Vec<Mv>,
    best_fp_mvp_idx: usize,
    best_fp_mvp_dist: u32,
    fp_me_mv: Mv,
    sub_me_mv: Mv,
    fp_me_dist: u32,
    post_subpel_me_mv_cost: u32,
    me_data_present: bool,
}

fn ref_geom(p: &PaddedRef) -> RefPicGeom {
    RefPicGeom {
        border: p.y.border as i32,
        max_width: p.y.width as i32,
        max_height: p.y.height as i32,
        y_stride: p.y.stride,
    }
}

/// C's `product_coding_loop.c:9425-9447` block, single-reference arm.
///
/// The compound entries of `ref_frame_type_arr` are skipped exactly as C
/// skips them: all three functions guard on `rf[1] == NONE_FRAME`.
#[must_use]
pub fn run_block_searches(cfg: &SearchFrameCfg, b: &BlockSearchIn<'_>) -> BlockSearchOut {
    let mut out = BlockSearchOut::default();
    let mut st: [[RefState; REF_LIST_MAX_DEPTH]; 2] = Default::default();

    let input_origin_index = b.org_y * b.src_stride + b.org_x;
    let mi_row = (b.org_y / 4) as i32;
    let mi_col = (b.org_x / 4) as i32;
    let bsize = svtav1_types::block::BlockSize::from_u8(b.bsize)
        .expect("an inter block always has a real BlockSize");

    // C `build_single_ref_mvp_array`'s gate (product_coding_loop.c:9425).
    let build_mvps = (cfg.md_subpel_me.enabled != 0
        && cfg.md_subpel_me.subpel_search_method
            == crate::port_enc_mode_config::encdec::subpel_search_method::SUBPEL_TREE_PRUNED
        && cfg.md_subpel_me.mvp_th != 0)
        || cfg.updated_enable_pme
        || cfg.ref_pruning_enabled;

    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };
        let s = &mut st[li][ri];
        s.me_data_present = b.me.me_data_present(b.org_x, b.org_y, b.bsize, li, ri);
        if build_mvps {
            s.mvps = build_single_ref_mvp_list(
                cfg.shut_fast_rate,
                &stack_this_mvs(&b.stacks[rf[0].max(0) as usize]),
                b.ref_mv_count[rf[0].max(0) as usize],
                b.org_x as i32,
                b.org_y as i32,
                b.bw as i32,
                b.bh as i32,
                &r,
            );
            let (idx, d) = best_mvp_by_distortion(
                &s.mvps,
                &mut dist,
                b.org_x as i32,
                b.org_y as i32,
                r.y_stride,
                input_origin_index,
            );
            s.best_fp_mvp_idx = idx;
            s.best_fp_mvp_dist = d;
        }
    }

    // ---- read_refine_me_mvs (product_coding_loop.c:2815-2936) ----
    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH || !st[li][ri].me_data_present {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let Some(raw) = b.me.mv_for(b.org_x, b.org_y, b.bsize, li, ri, b.me.max_l0) else {
            continue;
        };

        // C sets `ctx->ref_mv` from `choose_best_av1_mv_pred(NEWMV, me_mv)`
        // AFTER the clip; `refine_me_mv_for_ref`'s doc states that contract
        // and does the clip itself, so the centre it uses and the centre
        // this prediction is measured against are derived the same way.
        let centre = clipped_me_centre(b, &r, raw);
        let ref_mv = choose_pred_mv(b, cfg, rf[0], centre);
        let mvcp = full_pel_mv_cost_params(cfg, b, ref_mv, DistortionType::Sad);

        let fp_ctx = FullPelCtx {
            blk_org_x: b.org_x as i32,
            blk_org_y: b.org_y as i32,
            bwidth: b.bw as i32,
            bheight: b.bh as i32,
            enable_psad: false,
            hbd_md: false,
            sprs_lev0_start_x: 0,
            sprs_lev0_end_x: 0,
            sprs_lev0_start_y: 0,
            sprs_lev0_end_y: 0,
        };
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };

        let geom = subpel_geom(b, mi_row, mi_col);
        let mut sub_ctx = crate::md_subpel::SubpelMdContext {
            pd_pass: 1,
            mvp_th: i32::from(cfg.md_subpel_me.mvp_th),
            hp_mv_th: cfg.md_subpel_me.hp_mv_th,
            best_fp_mvp_dist: st[li][ri].best_fp_mvp_dist,
            best_fp_mvp: st[li][ri]
                .mvps
                .get(st[li][ri].best_fp_mvp_idx)
                .copied()
                .unwrap_or(Mv::ZERO),
            fp_me_dist: 0,
        };
        let mut subpel = |mv: &mut Mv| -> u32 {
            md_subpel_search(
                crate::md_subpel::SPEL_ME,
                &cfg.md_subpel_me,
                geom,
                bsize,
                li,
                ri,
                cfg.allow_high_precision_mv,
                ref_mv,
                usize::from(cfg.base_q_idx),
                cfg.full_lambda_8bit,
                cfg.md_subpel_me.skip_diag_refinement,
                Some(b.search_tables),
                b.src,
                input_origin_index,
                b.src_stride,
                &p.y.buf,
                (p.y.origin + b.org_y * p.y.stride + b.org_x) as i64,
                p.y.stride,
                Some(&mut sub_ctx),
                mv,
            )
        };
        // C's no-subpel arm: the full-pel ME cost, computed only when
        // somebody downstream needs it.
        let mut fp_dist = |mv: Mv| -> u32 {
            let idx = (b.org_x as i32 + (i32::from(mv.x) >> 3))
                + (b.org_y as i32 + (i32::from(mv.y) >> 3)) * r.y_stride as i32;
            let mut d2 = PlaneDistortion {
                src: b.src,
                src_stride: b.src_stride,
                ref_plane: &p.y.buf,
                ref_org: p.y.origin,
                ref_stride: p.y.stride,
                bwidth: b.bw,
                bheight: b.bh,
            };
            use crate::port_md::md_search::DistortionSource as _;
            let v = d2.variance(idx, input_origin_index);
            let full = full_pel_mv_cost_params(cfg, b, ref_mv, DistortionType::Var);
            v.wrapping_add(crate::port_md::pme::fp_mv_err_cost(mv, &full) as u32)
        };

        let do_subpel = cfg.md_subpel_me.enabled != 0;
        let res = refine_me_mv_for_ref(
            RefineMeIn {
                // The port has no NSQ MV inheritance yet (see the module
                // note below), so the square-block path is the only one.
                blk_avail_sqi: false,
                bsize_is_64x128_or_128x64: false,
                bsize_is_4x4: b.bw == 4 && b.bh == 4,
                parent_tested: false,
                sq_sb_me_mv: Mv::ZERO,
                raw_me_mv_full_pel: raw,
                md_nsq_me_enabled: false,
                do_subpel,
                subpel_fixed_stage: false,
                needs_fp_me_dist: cfg.updated_enable_pme || cfg.ref_pruning_enabled,
                shape_is_part_n: b.bw == b.bh,
            },
            &fp_ctx,
            &r,
            None,
            if do_subpel { Some(&mut subpel) } else { None },
            if do_subpel { None } else { Some(&mut fp_dist) },
            &mut dist,
            &mvcp,
            input_origin_index,
        );

        let s = &mut st[li][ri];
        s.fp_me_mv = res.fp_me_mv;
        s.sub_me_mv = res.sub_me_mv;
        s.post_subpel_me_mv_cost = res.post_subpel_me_mv_cost;
        // C `svt_av1_find_best_sub_pixel_tree_pruned` writes `fp_me_dist`
        // from INSIDE the subpel search when `search_stage == SPEL_ME`
        // (mcomp.c:616-618); the no-subpel arm writes it here instead.
        s.fp_me_dist = if do_subpel {
            sub_ctx.fp_me_dist
        } else {
            res.fp_me_dist.unwrap_or(u32::MAX)
        };
        out.sb_me_mv[li][ri] = res.sb_me_mv;
        out.post_subpel_me_mv_cost[li][ri] = res.post_subpel_me_mv_cost;
    }

    if !cfg.updated_enable_pme {
        return out;
    }

    // ---- pme_search (product_coding_loop.c:3197-3372) ----
    for &pair in b.ref_frame_type_arr {
        let rf = av1_set_ref_frame(pair);
        if rf[1] != crate::inter_mvp::NONE_FRAME {
            continue;
        }
        let (li, ri) = (get_list_idx(rf[0]), get_ref_frame_idx(rf[0]));
        if ri >= REF_LIST_MAX_DEPTH {
            continue;
        }
        let Some(p) = b.padded_by_ref[rf[0].max(0) as usize] else {
            continue;
        };
        let r = ref_geom(p);
        let s = st[li][ri].clone();
        if s.mvps.is_empty() {
            continue;
        }
        let best_mvp = s.mvps[s.best_fp_mvp_idx];
        let ref_mv = choose_pred_mv(b, cfg, rf[0], best_mvp);
        let mvcp = full_pel_mv_cost_params(cfg, b, ref_mv, cfg.pme_dist_type);

        let fp_ctx = FullPelCtx {
            blk_org_x: b.org_x as i32,
            blk_org_y: b.org_y as i32,
            bwidth: b.bw as i32,
            bheight: b.bh as i32,
            enable_psad: false,
            hbd_md: false,
            sprs_lev0_start_x: 0,
            sprs_lev0_end_x: 0,
            sprs_lev0_start_y: 0,
            sprs_lev0_end_y: 0,
        };
        let mut dist = PlaneDistortion {
            src: b.src,
            src_stride: b.src_stride,
            ref_plane: &p.y.buf,
            ref_org: p.y.origin,
            ref_stride: p.y.stride,
            bwidth: b.bw,
            bheight: b.bh,
        };
        let geom = subpel_geom(b, mi_row, mi_col);
        let mut sub_ctx = crate::md_subpel::SubpelMdContext {
            pd_pass: 1,
            mvp_th: i32::from(cfg.md_subpel_pme.mvp_th),
            hp_mv_th: cfg.md_subpel_pme.hp_mv_th,
            best_fp_mvp_dist: s.best_fp_mvp_dist,
            best_fp_mvp: best_mvp,
            fp_me_dist: 0,
        };
        let mut subpel = |mv: &mut Mv| -> u32 {
            md_subpel_search(
                crate::md_subpel::SPEL_PME,
                &cfg.md_subpel_pme,
                geom,
                bsize,
                li,
                ri,
                cfg.allow_high_precision_mv,
                ref_mv,
                usize::from(cfg.base_q_idx),
                cfg.full_lambda_8bit,
                // C reads the ME controls' `skip_diag_refinement` even on a
                // PME call (`svt_init_mv_cost_params`, :1906).
                cfg.md_subpel_me.skip_diag_refinement,
                Some(b.search_tables),
                b.src,
                input_origin_index,
                b.src_stride,
                &p.y.buf,
                (p.y.origin + b.org_y * p.y.stride + b.org_x) as i64,
                p.y.stride,
                Some(&mut sub_ctx),
                mv,
            )
        };

        let res = pme_search_for_ref(
            &cfg.md_pme,
            &fp_ctx,
            &r,
            &mut dist,
            &mvcp,
            input_origin_index,
            cfg.pme_dist_type,
            /* skipped */ false,
            s.me_data_present,
            s.fp_me_mv,
            s.sub_me_mv,
            s.fp_me_dist,
            s.post_subpel_me_mv_cost,
            &s.mvps,
            cfg.pme_full_pel_w,
            cfg.pme_full_pel_h,
            cfg.pic_width,
            cfg.pic_height,
            if cfg.md_subpel_pme.enabled != 0 {
                Some(&mut subpel)
            } else {
                None
            },
        );
        out.valid_pme_mv[li][ri] = res.valid;
        out.best_pme_mv[li][ri] = res.best_pme_mv;
        out.pme_exit[li][ri] = Some(res.exit);

        #[cfg(feature = "std")]
        if crate::dbgenv::canddbg() && crate::depth_refine::nsqdbg_here(b.org_x, b.org_y) {
            std::eprintln!(
                "PMEDBG org=({},{}) {}x{} li={li} ri={ri} mvpn={} bestidx={} bestdist={} \
                 fpme=({},{}) subme=({},{}) fpdist={} pscost={} exit={:?} pme=({},{}) valid={} \
                 flam={} fastlam={} epb={}",
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                s.mvps.len(),
                s.best_fp_mvp_idx,
                s.best_fp_mvp_dist,
                s.fp_me_mv.y,
                s.fp_me_mv.x,
                s.sub_me_mv.y,
                s.sub_me_mv.x,
                s.fp_me_dist,
                s.post_subpel_me_mv_cost,
                res.exit,
                res.best_pme_mv.y,
                res.best_pme_mv.x,
                res.valid,
                cfg.full_lambda_8bit,
                cfg.fast_lambda_8bit,
                (cfg.full_lambda_8bit >> 6).max(1),
            );
        }
    }
    out
}

fn stack_this_mvs(stack: &InterMvpStack) -> Vec<Mv> {
    stack.stack.iter().map(|c| c.this_mv).collect()
}

fn subpel_geom(b: &BlockSearchIn<'_>, mi_row: i32, mi_col: i32) -> SubpelBlockGeom {
    SubpelBlockGeom {
        mi_row,
        mi_col,
        mi_width: (b.bw / 4) as i32,
        mi_height: (b.bh / 4) as i32,
        mi_rows: b.mi_rows,
        mi_cols: b.mi_cols,
        bwidth: b.bw,
        bheight: b.bh,
        sq_size: b.sq_size,
    }
}

/// C's ME centre after `clip_mv_on_pic_boundary`
/// (product_coding_loop.c:2870-2871), which runs BEFORE
/// `choose_best_av1_mv_pred`.
fn clipped_me_centre(b: &BlockSearchIn<'_>, r: &RefPicGeom, raw_full_pel: Mv) -> Mv {
    let mut mv = Mv {
        x: raw_full_pel.x.wrapping_mul(8),
        y: raw_full_pel.y.wrapping_mul(8),
    };
    crate::port_md::coding_loop::clip_mv_on_pic_boundary(
        b.org_x as i32,
        b.org_y as i32,
        b.bw as i32,
        b.bh as i32,
        r.max_width,
        r.max_height,
        r.border,
        &mut mv.x,
        &mut mv.y,
    );
    mv
}

/// C `svt_aom_choose_best_av1_mv_pred(ctx, ref_pair, NEWMV, mv, 0, ...)`
/// -> `ctx->ref_mv` (the `best_pred_mv[0]` it writes).
fn choose_pred_mv(b: &BlockSearchIn<'_>, cfg: &SearchFrameCfg, frame_type: i8, mv: Mv) -> Mv {
    let mut drl_index = 0u8;
    let mut pred = [Mv::ZERO; 2];
    crate::port_md::drl::choose_best_av1_mv_pred(
        &crate::port_md::drl::ChooseDrlCtx {
            shut_fast_rate: cfg.shut_fast_rate,
            approx_inter_rate: cfg.approx_inter_rate,
            ref_mv_stack: &b.stacks[frame_type.max(0) as usize].stack,
            ref_mv_count: b.ref_mv_count[frame_type.max(0) as usize],
            nmv_cost: b.nmv,
            drl_mode_fac_bits: b.drl_mode_fac_bits,
        },
        svtav1_types::prediction::PredictionMode::NewMv,
        mv,
        Mv::ZERO,
        &mut drl_index,
        &mut pred,
    );
    pred[0]
}

/// C `svt_init_mv_cost_params` restricted to the members
/// `md_full_pel_search` reads, with C's own lambda choice
/// (`dist_type != SAD ? full_lambda : fast_lambda`, :1920-1922).
fn full_pel_mv_cost_params<'a>(
    cfg: &SearchFrameCfg,
    b: &'a BlockSearchIn<'a>,
    ref_mv: Mv,
    dist_type: DistortionType,
) -> MvCostParams<'a> {
    let rdmult = if dist_type == DistortionType::Sad {
        cfg.fast_lambda_8bit
    } else {
        cfg.full_lambda_8bit
    };
    MvCostParams {
        ref_mv,
        full_ref_mv: Mv {
            x: ref_mv.x >> 3,
            y: ref_mv.y >> 3,
        },
        mv_cost_type: if cfg.md_subpel_me.skip_diag_refinement >= 3 {
            MvCostType::Opt
        } else {
            MvCostType::Entropy
        },
        mv_cost_tables: Some(b.nmv),
        error_per_bit: ((rdmult >> 6).max(1)) as i32,
        early_exit_th: 1020 - (i32::from(b.sq_size) >> 2),
        sad_per_bit: cfg.sad_per_bit,
    }
}

/// The PICTURE-level signals the frame configuration is derived from —
/// the four `enc_mode_config.c` levels plus the lambdas and the qp.
pub struct SearchFrameInputs {
    /// `pcs->md_pme_level`
    pub md_pme_level: u8,
    /// `pcs->me_subpel_level`
    pub me_subpel_level: u8,
    /// `pcs->pme_subpel_level`
    pub pme_subpel_level: u8,
    /// `pcs->md_nsq_mv_search_level`
    pub md_nsq_mv_search_level: u8,
    /// `pcs->dist_based_ref_pruning`
    pub dist_based_ref_pruning: u8,
    /// `scs->static_config.qp` — the CLI qp the PME search-area scaling
    /// reads (NOT `base_q_idx`).
    pub cli_qp: u32,
    /// `scs->qp_based_th_scaling_ctrls.pme_qp_based_th_scaling`
    /// (`enc_handle.c:3812`: 1 on the `_default` arm above `ENC_MR`).
    pub pme_qp_based_th_scaling: bool,
    pub full_lambda_8bit: u32,
    pub fast_lambda_8bit: u32,
    pub base_q_idx: u8,
    pub allow_high_precision_mv: bool,
    /// `ctx->approx_inter_rate`
    pub approx_inter_rate: u8,
    pub pic_width: u32,
    pub pic_height: u32,
}

/// Resolve C's four control rows and the qp-modulated PME extents once per
/// frame.
///
/// `None` when any level is outside the range its C table accepts (where C
/// would `assert(0)`), which is the same contract
/// `port_enc_mode_config::encdec::sig_deriv_enc_dec_default` has.
#[must_use]
pub fn frame_cfg(i: &SearchFrameInputs) -> Option<SearchFrameCfg> {
    use crate::port_enc_mode_config::ctrls;
    use crate::port_enc_mode_config::encdec;

    let pme = ctrls::md_pme_search_controls(i.md_pme_level)?;
    let subpel_me = encdec::md_subpel_me_controls(i.me_subpel_level)?;
    let subpel_pme = encdec::md_subpel_pme_controls(i.pme_subpel_level)?;
    let nsq = encdec::md_nsq_motion_search_controls(i.md_nsq_mv_search_level)?;
    let pruning = ctrls::set_dist_based_ref_pruning_controls(i.dist_based_ref_pruning)?;

    let (qw, qwd) = crate::port_enc_mode_config::me::get_qp_based_th_scaling_factors(
        i.pme_qp_based_th_scaling,
        i.cli_qp,
    );
    let (w, h) = crate::port_md::md_search::pme_search_extents(
        pme.full_pel_search_width,
        pme.full_pel_search_height,
        pme.sa_q_weight != 0,
        qw,
        qwd,
    );

    Some(SearchFrameCfg {
        md_pme: MdPmeCtrls {
            enabled: pme.enabled != 0,
            full_pel_search_width: pme.full_pel_search_width,
            full_pel_search_height: pme.full_pel_search_height,
            sa_q_weight: pme.sa_q_weight != 0,
            enable_psad: pme.enable_psad != 0,
            early_check_mv_th_multiplier: pme.early_check_mv_th_multiplier,
            pre_fp_pme_to_me_mv_th: pme.pre_fp_pme_to_me_mv_th,
            pre_fp_pme_to_me_cost_th: i64::from(pme.pre_fp_pme_to_me_cost_th),
            post_fp_pme_to_me_mv_th: pme.post_fp_pme_to_me_mv_th,
            post_fp_pme_to_me_cost_th: i64::from(pme.post_fp_pme_to_me_cost_th),
        },
        pme_dist_type: match pme.dist_type {
            ctrls::DistortionType::Sad => DistortionType::Sad,
            ctrls::DistortionType::Var => DistortionType::Var,
            ctrls::DistortionType::Ssd => DistortionType::Ssd,
        },
        pme_full_pel_w: w,
        pme_full_pel_h: h,
        md_subpel_me: subpel_me,
        md_subpel_pme: subpel_pme,
        md_nsq_me_enabled: nsq.enabled != 0,
        ref_pruning_enabled: pruning.enabled != 0,
        // C `product_coding_loop.c:9418-9422`. The second assignment zeroes
        // it when `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`;
        // this port hands the injector `use_neighbouring_mode_ctrls_enabled
        // = false` and `is_intra_bordered = false`, so that arm cannot fire
        // and the value is the control's own. C agrees on the campaign's
        // cells (`SVT_INJCFG_OUT`: `ibord=0 uepme=1`).
        updated_enable_pme: pme.enabled != 0,
        full_lambda_8bit: i.full_lambda_8bit,
        fast_lambda_8bit: i.fast_lambda_8bit,
        base_q_idx: i.base_q_idx,
        sad_per_bit: crate::port_md::pme::get_sad_per_bit(usize::from(i.base_q_idx), false),
        allow_high_precision_mv: i.allow_high_precision_mv,
        shut_fast_rate: false,
        approx_inter_rate: i.approx_inter_rate,
        pic_width: i.pic_width,
        pic_height: i.pic_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inter_me::context::{MeB64Output, MeCandidate};
    use crate::inter_me_arm::FrameMe;
    use crate::picture::{PaddedPlane, PaddedRef};
    use crate::port_md::drl::DRL_MODE_CONTEXTS;
    use crate::port_md::md_search::PmeExit;
    use crate::port_md::pme::MV_VALS;

    const W: usize = 128;
    const H: usize = 128;
    /// The true displacement, in full pels.
    ///
    /// **2, not the campaign's 3, and that is load-bearing.** C's PME
    /// full-pel window here is `+-(full_pel_search_width >> 1)` after the qp
    /// modulation — `MAX(3, ROUND(7 * 40 / 63)) = 4`, so `+-2` FULL PELS
    /// around the MVP. At a 3-pel displacement the full-pel search cannot
    /// reach the truth at all and only the sub-pel stage's two-iteration
    /// overshoot gets there, which makes the landing point a property of the
    /// interpolator on this fixture rather than of the search. At 2 the
    /// truth is inside the window and the assertion is about the search.
    const SHIFT: usize = 2;

    /// A TRIANGLE wave in `2x + y`, so a wrong MV costs real variance in
    /// BOTH components (a pure horizontal ramp is invariant to vertical
    /// error and would let a y-axis defect pass).
    ///
    /// **Triangle, not `% 256`.** A sawtooth has a one-pixel cliff every
    /// 256 samples, and the sub-pel search interpolates ACROSS it: the
    /// first version of this fixture used `(2x + y) % 256` and the
    /// refinement walked off the true `-3` full-pel match to `-2.25`,
    /// because the interpolated cliff scored better than the exact match.
    /// That is a property of the fixture, not of the port — pick content
    /// the interpolator cannot beat the truth on.
    fn source() -> alloc::vec::Vec<u8> {
        (0..W * H)
            .map(|i| {
                let (x, y) = (i % W, i / W);
                let t = (x * 2 + y) % 510;
                let base = if t < 255 { t } else { 509 - t };
                // A deterministic per-pixel DITHER on top. Without it the
                // triangle wave is locally linear, the interpolator
                // reproduces it EXACTLY at every sub-pel offset, and the
                // search is left minimising MV RATE alone — which walks off
                // the true match to a cheaper MV and makes the assertion
                // below a statement about the cost table rather than about
                // the search. The dither shifts WITH the content (it is a
                // function of the source position), so the true full-pel
                // match still scores zero variance and everything else does
                // not.
                let d = (x.wrapping_mul(37) ^ y.wrapping_mul(101)) % 19;
                (base.saturating_add(d).min(255)) as u8
            })
            .collect()
    }

    /// The reference is the source shifted LEFT by [`SHIFT`] pixels, so the
    /// exact match sits at full-pel MV `x = -SHIFT`.
    ///
    /// The sign is the whole point and it is easy to get backwards: the
    /// prediction reads `ref[x + mv]`, so a NEGATIVE mv must find the
    /// source's content at a SMALLER reference x — i.e. the reference is the
    /// source moved toward x = 0. Building it the other way makes `-SHIFT`
    /// the WORST full-pel offset, and then a search that walks away from it
    /// is behaving correctly while the assertion fails.
    fn reference() -> alloc::vec::Vec<u8> {
        let src = source();
        let mut r = alloc::vec![0u8; W * H];
        for y in 0..H {
            for x in 0..W {
                r[y * W + x] = src[y * W + (x + SHIFT).min(W - 1)];
            }
        }
        r
    }

    fn zero_cost() -> MvCostTable {
        MvCostTable {
            joint: [0; 4],
            comp: [alloc::vec![0i32; MV_VALS], alloc::vec![0i32; MV_VALS]],
        }
    }

    /// One b64 whose ONLY ME candidate is LIST 1's — C's measured shape on
    /// `gradient 128x128 q40 p8` (`SVT_HME_OUT`: `n=1 c=[0:dir=1 ...]`,
    /// `mv0=(0,0) mvl1=(-3,0)`).
    fn frame_me_list1_only() -> FrameMe {
        let (max_cand, max_refs, max_l0) = (13usize, 5usize, 3usize);
        let mut b = MeB64Output::new(max_cand, max_refs);
        // Every 64x64/32x32/... pu of the b64 gets the same single list-1
        // candidate, so the driver reaches the same state at any block size.
        for pu in 0..b.total_me_candidate_index.len() {
            b.total_me_candidate_index[pu] = 1;
            b.me_candidate_array[pu * max_cand] = MeCandidate {
                direction: 1,
                ref_idx_l0: 0,
                ref_idx_l1: 0,
                ref0_list: 0,
                ref1_list: 1,
            };
            // list 0's slot is left at (0,0) — C never writes it when the
            // list-0 search is pruned, and reading it as if it were a result
            // is the defect §1z¹³ measured.
            b.me_mv_array[pu * max_refs + max_l0] = Mv {
                x: -(SHIFT as i16),
                y: 0,
            };
        }
        FrameMe {
            per_b64: alloc::vec![b; 4],
            b64_cols: 2,
            b64_rows: 2,
            max_refs,
            max_cand,
            max_l0,
            enable_me_8x8: true,
            enable_me_16x16: true,
        }
    }

    fn cfg() -> SearchFrameCfg {
        frame_cfg(&SearchFrameInputs {
            // C's resolved levels on the campaign's `p8` video cells,
            // read back off C's own context through `SVT_INJCFG_OUT`'s
            // `PMEST` line and matched to the control tables:
            //   `pme=1/0/7/5/MIN/25/16/50/32/1/1`  -> md_pme level 4
            //   `sme=1/2/0/1/2/0/0/MAX/0/104/4`    -> me_subpel level 4
            //   `spme=1/3/0/1/2/0/0/MAX/0/104/0`   -> pme_subpel level 2
            md_pme_level: 4,
            me_subpel_level: 4,
            pme_subpel_level: 2,
            md_nsq_mv_search_level: 2,
            dist_based_ref_pruning: 0,
            cli_qp: 40,
            pme_qp_based_th_scaling: true,
            full_lambda_8bit: 241_378,
            fast_lambda_8bit: 6_633,
            base_q_idx: 160,
            allow_high_precision_mv: false,
            approx_inter_rate: 0,
            pic_width: W as u32,
            pic_height: H as u32,
        })
        .expect("the campaign's levels are in-domain for every C control table")
    }

    /// POSITIVE CONTROL — the whole point of this module, and it fails if
    /// either half is deleted.
    ///
    /// With C's measured ME shape (a LIST-1 candidate and nothing for list
    /// 0) the two references reach `pme_search` in DIFFERENT states, and
    /// that difference is what makes the reference set and PME one
    /// mechanism:
    ///
    /// * `BWDREF` has ME data, so `read_refine_me_mvs` refines its MV and
    ///   `pme_search` takes a BAIL-TO-ME exit — the PME MV is an ECHO of the
    ///   ME MV, not a search result;
    /// * `LAST` has NO ME data, so the full-pel search actually RUNS
    ///   (`PmeExit::Searched`) and is the ONLY producer of a LAST_FRAME
    ///   NEWMV candidate. Delete the `pme_search` half and `valid_pme_mv[0]`
    ///   is false; delete the reference-set half and the loop never visits
    ///   list 0 at all. Either way this assertion fails.
    #[test]
    fn pme_is_the_only_producer_of_a_last_frame_mv_when_me_has_only_list_1() {
        let src = source();
        let refp = PaddedRef {
            y: PaddedPlane::from_plane(&reference(), W, H, 64),
            uv: None,
        };
        let padded_by_ref: [Option<&PaddedRef>; 8] =
            [None, Some(&refp), None, None, None, Some(&refp), None, None];
        let stacks = alloc::vec![crate::inter_mvp::InterMvpStack::default(); 8];
        let nmv = zero_cost();
        let fac = [[0i32; 2]; DRL_MODE_CONTEXTS];
        let tables = crate::intrabc::build_nmv_cost_table(
            &crate::entropy::context::FrameContext::new_default().nmvc,
            crate::entropy::mv_coding::MvSubpelPrecision::Low,
        );
        let me = frame_me_list1_only();
        let out = run_block_searches(
            &cfg(),
            &BlockSearchIn {
                org_x: 0,
                org_y: 0,
                bw: 64,
                bh: 64,
                // BLOCK_64X64
                bsize: 12,
                sq_size: 64,
                mi_rows: (H / 4) as i32,
                mi_cols: (W / 4) as i32,
                src: &src,
                src_stride: W,
                ref_frame_type_arr: &[1, 5],
                padded_by_ref: &padded_by_ref,
                stacks: &stacks,
                ref_mv_count: &[0; 8],
                nmv: &nmv,
                drl_mode_fac_bits: &fac,
                search_tables: &tables,
                me: &me,
            },
        );

        let summary = alloc::format!(
            "l0: sbme={:?} pme={:?} valid={} exit={:?} | l1: sbme={:?} pme={:?} valid={} exit={:?}",
            out.sb_me_mv[0][0],
            out.best_pme_mv[0][0],
            out.valid_pme_mv[0][0],
            out.pme_exit[0][0],
            out.sb_me_mv[1][0],
            out.best_pme_mv[1][0],
            out.valid_pme_mv[1][0],
            out.pme_exit[1][0],
        );

        // BWDREF: the ME MV was refined and PME echoed it.
        assert_eq!(
            out.sb_me_mv[1][0],
            Mv {
                x: -(SHIFT as i16) * 8,
                y: 0
            },
            "list 1's ME MV is the one C's candidate array names"
        );
        assert!(out.valid_pme_mv[1][0]);
        assert!(
            matches!(
                out.pme_exit[1][0],
                Some(PmeExit::EarlyMvpCheck | PmeExit::PreFullPel | PmeExit::PostFullPel)
            ),
            "list 1 has ME data, so PME must take a bail-to-ME exit, not run a \
             search — {summary}"
        );

        // LAST: no ME data at all, so the SEARCH is what produces the MV.
        assert_eq!(
            out.sb_me_mv[0][0],
            Mv::ZERO,
            "list 0 has no ME data, so read_refine_me_mvs must leave sb_me_mv alone"
        );
        assert_eq!(
            out.pme_exit[0][0],
            Some(PmeExit::Searched),
            "list 0's PME must RUN its full-pel search — a bail exit here would \
             mean the driver found ME data C does not have"
        );
        assert!(out.valid_pme_mv[0][0]);
        assert_eq!(
            out.best_pme_mv[0][0].x,
            -(SHIFT as i16) * 8,
            "the PME search must land on the true displacement, not on the \
             zero MVP it started from"
        );
    }
}

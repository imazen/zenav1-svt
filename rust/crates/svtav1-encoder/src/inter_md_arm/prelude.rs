use super::*;

/// The block-setup half of C's `md_product_coding_loop`: the per-reference
/// MVP stacks and the ME/PME searches (`svt_aom_generate_av1_mvp_table` ->
/// `read_refine_me_mvs` -> `pme_search`, product_coding_loop.c:9393-9447).
///
/// C runs this BEFORE `generate_md_stage_0_cand`, so `ctx->md_me_dist` /
/// `md_pme_dist` already exist when `inject_intra_candidates` resolves
/// `dc_cand_only_flag` via `eliminate_candidate_based_on_pme_me_results`
/// (mode_decision.c:3576-3579). The funnel needs the same ordering: this
/// runs before the intra candidate set is fixed, and
/// [`build_inter_candidates`] consumes the result rather than re-searching.
pub struct BlockPrelude<'a> {
    /// C `ctx->ref_mv_stack[MODE_CTX_REF_FRAMES]`.
    pub stacks: Vec<crate::inter_mvp::InterMvpStack>,
    /// C `ctx->ref_mv_count[MODE_CTX_REF_FRAMES]`.
    pub ref_mv_count: [u8; crate::inter_mvp::MODE_CTX_REF_FRAMES],
    /// The search output — `md_me_dist()`/`md_pme_dist()` included.
    pub search: crate::inter_search_arm::BlockSearchOut,
    /// C `ctx->ref_frame_type_arr[0..tot_ref_frame_types]` at INJECTION
    /// time — `determine_best_references`' rebuild of the picture-level
    /// list from this block's own ME candidates when `use_best_me` fired,
    /// else the picture-level list unchanged.
    pub ref_arr: alloc::borrow::Cow<'a, [i8]>,
    /// This block's ME candidates — `determine_best_references` consumed
    /// them in `block_prelude`; `build_inter_candidates` hands the same
    /// slice to the injectors.
    pub me_cands: Vec<crate::port_md::predicates::MeCandidateRef>,
}

/// Build [`BlockPrelude`] for one block (see its doc for C's ordering).
///
/// `light` is `Some((sig, is_intra_bordered))` on the light-PD1 lane — the
/// per-SB `svt_aom_sig_deriv_enc_dec_light_pd1_default` signals and
/// `ctx->is_intra_bordered` already gated on
/// `use_neighbouring_mode_ctrls.enabled` (product_coding_loop.c:9115). It
/// swaps C's `read_refine_me_mvs` + `pme_search` for
/// `read_refine_me_mvs_light_pd1` (:2737) — a strictly smaller search with
/// its own seeding and skip gates.
pub fn block_prelude<'a>(
    f: &InterMdFrame<'a>,
    b: &mut InterBlockCtx<'_>,
    lambda: u64,
    fast_lambda: u32,
    // C `ctx->is_intra_bordered` (product_coding_loop.c:9417) — the
    // `use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0`
    // product, computed by the caller. The regular lane resolves
    // `ctx->updated_enable_pme` off it (:9418-9422); the light lane
    // carries it inside `light`'s tuple for `run_block_searches_light`.
    is_intra_bordered: bool,
    light: Option<(
        &crate::port_enc_mode_config::light_pd1::LightPd1Signals,
        bool,
    )>,
) -> BlockPrelude<'a> {
    use crate::port_md::predicates::MeCandidateRef;
    // --- The reference-MV stack, PER REFERENCE TYPE. C calls
    //     `svt_aom_generate_av1_mvp_table(ctx, ..., ctx->ref_frame_type_arr,
    //     ctx->tot_ref_frame_types, pcs)` (product_coding_loop.c:9393), i.e.
    //     one stack per entry — not one for LAST.
    let ctx = derive_block_ctx(
        (b.org_y / 4) as i32,
        (b.org_x / 4) as i32,
        b.bsize as usize,
        f.mi_rows,
        f.mi_cols,
        f.tile,
        f.sb_mi_size,
    );
    let grid = MvpGrid {
        entries: b.grid,
        stride: b.grid_stride,
        base: (b.org_y / 4) as i32 * b.grid_stride + (b.org_x / 4) as i32,
    };
    // C `svt_aom_generate_av1_mvp_table`'s `gm_mv`
    // (adaptive_mv_pred.c:1372-1394): the block-centre projection of THIS
    // reference's global-motion model. It is the zero MV for an IDENTITY
    // model, which is what this used to hardcode; with the search's real
    // models threaded through `InterMdFrame::global_motion` it is the value
    // `setup_ref_mv_list` substitutes for a GLOBALMV neighbour, so a wrong
    // one desyncs the DRL against the decoder's own scan.
    // C's `ctx->ref_mv_stack[MODE_CTX_REF_FRAMES]` — indexed by REFERENCE
    // TYPE, so a compound pair (LAST_LAST2 = 20, LAST_BWD = 8) has its own
    // entry, and `inject_mvp_candidates_ii`'s `ref_mv_stack[ref_pair]` reads
    // the stack that pair's constituents built together.
    let mut stacks = alloc::vec![
        crate::inter_mvp::InterMvpStack::default();
        crate::inter_mvp::MODE_CTX_REF_FRAMES
    ];
    let mut ref_mv_count = [0u8; crate::inter_mvp::MODE_CTX_REF_FRAMES];
    // C's `ctx->sb64_sq_no4xn_geom` is set in the MD block setup, so it is a
    // property of THIS block, not of the picture.
    let mvp_env = f.mvp_env.for_block(f.sb_size, b.bw as usize, b.bh as usize);
    // --- C's ME candidate array for this block, verbatim: the injectors
    //     read each candidate's own `direction` and resolve it to a
    //     reference frame (`mode_decision.c:2320-2326`), and
    //     `determine_best_references` below rebuilds the ref list from it.
    let me_cands: Vec<MeCandidateRef> =
        f.me.cands_for(b.org_x, b.org_y, b.bsize)
            .iter()
            .map(|c| MeCandidateRef {
                direction: c.direction(),
                ref_idx_l0: c.ref_idx_l0(),
                ref_idx_l1: c.ref_idx_l1(),
                ref0_list: c.ref0_list(),
                ref1_list: c.ref1_list(),
            })
            .collect();
    // C `determine_best_references` (product_coding_loop.c:65-116), run at
    // the top of `md_encode_block`/`md_encode_block_light_pd1` when the
    // lane's own gate says so — the light lane's is `use_best_references
    // == 3 && temporal_layer_index > 0` (:9074), the regular lane's is
    // `get_enable_use_best_me` (:9379), which levels 1 and 3 also resolve
    // without TPL. In both, `ctx->ref_frame_type_arr` is REBUILT per block
    // from this block's own ME candidate array — a reference ME never
    // searched (`do_ref == 0`) drops out entirely — and that list, not
    // `ppcs`' picture-level one, then drives the MVP table, the searches,
    // and every injector below. `use_best_references == 2` needs
    // `get_sb_tpl_inter_stats` (TPL, unported); `None` folds to false,
    // which is C's own result whenever `tpl_ctrls.enable` is 0.
    let use_best_me = !f.sframe_ref_pruned
        && if light.is_some() {
            f.use_best_references == 3 && f.temporal_layer_index > 0
        } else {
            crate::port_md::coding_loop::get_enable_use_best_me(
                f.use_best_references,
                u32::from(f.temporal_layer_index),
                f.me.per_b64
                    .get((b.org_y / 64) * f.me.b64_cols + (b.org_x / 64))
                    .map_or(0, |o| o.me_8x8_distortion),
            )
            .unwrap_or(false)
        };
    let block_ref_arr: alloc::borrow::Cow<'_, [i8]> = if use_best_me {
        alloc::borrow::Cow::Owned(crate::port_md::coding_loop::determine_best_references(
            &me_cands,
            me_cands.len(),
            // `pcs->slice_type == B_SLICE` — this frame is inter (the
            // funnel only builds `InterMdFrame` on non-I slices) and the
            // port's `SliceType` makes every inter frame B.
            true,
            f.ref_list0_count_try,
            f.ref_list1_count_try,
        ))
    } else {
        alloc::borrow::Cow::Borrowed(f.ref_frame_type_arr)
    };
    // C `svt_aom_generate_av1_mvp_table` (product_coding_loop.c:9393 ->
    // adaptive_mv_pred.c:1329; the light lane's `!shut_fast_rate`-guarded
    // call is at :9114): ONE driver over `ref_frame_type_arr`, single AND
    // compound entries, with the `mv_ref0` scratch shared across the loop
    // the way C shares its local — the `symteric_refs` shortcut reads what
    // the LAST pass left in it. `shut_fast_rate` is false on every lane
    // this reaches, so the guard is a transcription, not a fork — it keeps
    // C's skip reachable rather than baking in today's value.
    if light.is_none_or(|(sig, _)| !sig.shut_fast_rate) {
        for (&rt, st) in block_ref_arr
            .iter()
            .zip(crate::inter_mvp::generate_av1_mvp_table(
                &grid,
                &ctx,
                &mvp_env,
                b.bsize as usize,
                &block_ref_arr,
            ))
        {
            ref_mv_count[rt.max(0) as usize] = st.count;
            stacks[rt.max(0) as usize] = st;
        }
    }

    // --- C's per-block MD motion searches, in C's own order. The regular
    //     lane runs `build_single_ref_mvp_array` -> `read_refine_me_mvs` ->
    //     `pme_search` (product_coding_loop.c:9425-9447); the light lane
    //     runs `read_refine_me_mvs_light_pd1` (:2737) instead — no MVP
    //     array, no PME, no ref pruning — which is what
    //     [`crate::inter_search_arm::run_block_searches_light`] ports. See
    //     [`crate::inter_search_arm`] for why the reference set and PME are
    //     one mechanism.
    let search_in = crate::inter_search_arm::BlockSearchIn {
        // C `ctx->full_lambda_md[0]` / `fast_lambda_md[0]` as
        // `svt_aom_mode_decision_configure_sb` set them for THIS
        // superblock. `lambda` is the funnel's own per-SB MD lambda,
        // which is the SAME quantity the search used to re-derive at
        // frame level -- one value, one derivation.
        full_lambda_8bit: u32::try_from(lambda).unwrap_or(u32::MAX),
        fast_lambda_8bit: fast_lambda,
        org_x: b.org_x,
        org_y: b.org_y,
        bw: b.bw,
        bh: b.bh,
        bsize: b.bsize,
        // C `blk_geom->sq_size` — the SQUARE this shape came from. The
        // funnel has no NSQ parent link here, so a square block's own
        // size is used; for an NSQ shape that is the larger side, which
        // is what `svt_init_mv_cost_params`' `early_exit_th` reads.
        sq_size: b.bw.max(b.bh) as u16,
        mi_rows: f.mi_rows,
        mi_cols: f.mi_cols,
        src: f.src,
        src_stride: f.src_stride,
        // The BLOCK-level list `determine_best_references` produced (or the
        // picture-level one when its gate is off) — what C's
        // `ctx->ref_frame_type_arr` holds at this point in
        // `md_encode_block`.
        ref_frame_type_arr: &block_ref_arr,
        padded_by_ref: &f.padded_by_ref,
        stacks: &stacks,
        ref_mv_count: &ref_mv_count,
        nmv: &f.nmv,
        drl_mode_fac_bits: &f.fac.drl_mode,
        search_tables: &f.search_tables,
        me: f.me,
        // C `ctx->sq_sb_me_mv` + `pc_tree->tested_blk[PART_N][0]`, which
        // live ACROSS blocks. `None` here means the caller has no
        // square-parent state, which makes every shape take C's
        // `me_mv_array` seed — the behaviour this module had before the
        // state existed. The funnel supplies it.
        sq_me: b.sq_me.as_deref().copied(),
        // C `ctx->updated_enable_pme` (product_coding_loop.c:9418-9422):
        // `md_pme_ctrls.enabled`, zeroed when this block's
        // `is_intra_bordered && use_neighbouring_mode_ctrls.enabled`. The
        // caller's `is_intra_bordered` is already the `enabled`-gated
        // product, so the conjunction collapses to `!is_intra_bordered`.
        // Skipping `pme_search` here is what keeps an intra-bordered
        // block's `inject_pme_candidates` from adding candidates C never
        // had — MEASURED on `vidyo1 256x256 p8` frame 1, mi=(10,8),
        // where the port's `sb_pme_mv` (20,6) injected three extra
        // NEWMV/NEWNEWMV candidates C's `updated_enable_pme = 0`
        // suppressed (`SVT_INJCFG_OUT`: `ibord=1 uepme=0`).
        updated_enable_pme: f.search.md_pme_enabled && !is_intra_bordered,
    };
    let search = if let Some((sig, sig_ibord)) = light {
        crate::inter_search_arm::run_block_searches_light(
            &f.search,
            &search_in,
            &crate::inter_search_arm::LightSearchSig {
                md_subpel_me: &sig.md_subpel_me,
                is_intra_bordered: sig_ibord,
                use_neighbouring_mode_enabled: sig.cand_reduction.use_neighbouring_mode_enabled
                    != 0,
                shut_fast_rate: sig.shut_fast_rate,
                approx_inter_rate: sig.approx_inter_rate,
            },
        )
    } else {
        crate::inter_search_arm::run_block_searches(&f.search, &search_in)
    };
    // C `if (ctx->shape == PART_N) ctx->sq_sb_me_mv = ctx->sb_me_mv`
    // (product_coding_loop.c:2932-2934), and the `tested_blk[PART_N][0]` that
    // guards its reader. The write is HERE and not in `inter_search_arm`
    // because the state is the caller's — C's is one slot on the
    // mode-decision context, and the funnel is what owns the block walk that
    // gives it its meaning.
    if search.is_square_shape
        && let Some(q) = b.sq_me.as_deref_mut()
    {
        q.record_square(b.org_x, b.org_y, b.bw, search.sb_me_mv);
    }
    BlockPrelude {
        stacks,
        ref_mv_count,
        search,
        ref_arr: block_ref_arr,
        me_cands,
    }
}

/// `port_md::inject::inject_inter_candidates` (C `mode_decision.c:2836`)
/// decides WHICH candidates exist; this fills its `InjectCtx` and turns each
/// one into a motion-compensated prediction plus C's real
/// `svt_aom_inter_fast_cost`. The returned order is the injector's, which is
/// load-bearing — each stage sees the injected-MV log the previous ones
/// filled, so `NEARESTMV` at the same MV suppresses the `NEWMV` duplicate.
///
/// `prelude` is this block's [`block_prelude`] output — C runs the MVP/ME
/// searches at block setup, BEFORE the intra candidate set is decided
/// (`eliminate_candidate_based_on_pme_me_results` reads `md_me_dist`), so the
/// caller owns when they run.
#[must_use]
pub fn build_inter_candidates(
    f: &InterMdFrame<'_>,
    b: &mut InterBlockCtx<'_>,
    lambda: u64,
    // C `nic_pruning_ctrls->merge_inter_cands_mult` — the
    // `generate_md_stage_0_cand_light_pd1` class-merge control
    // (mode_decision.c:3638-3643).
    merge_inter_cands_mult: u8,
    prelude: BlockPrelude<'_>,
    warp_out: &mut WarpRefineBlock,
    // C `ctx->is_intra_bordered` — the
    // `use_neighbouring_mode_ctrls.enabled ? is_intra_bordered(ctx) : 0`
    // product BOTH lanes compute (product_coding_loop.c:9115 / :9451).
    is_intra_bordered: bool,
    // `Some(sig)` on the light-PD1 lane — the per-SB
    // `svt_aom_sig_deriv_enc_dec_light_pd1_default` output. When the per-SB
    // `pd1_level` is above `REGULAR_PD1` the inter candidate set is the
    // STRICT subset `inject_inter_candidates_light_pd1` emits (MVP +
    // ME-NEWMV only — no global, no bipred-3x3, no unipred-3x3, no PME, no
    // non-simple/compound expansion; mode_decision.c:3526-3562), and the
    // cand-reduction controls and `approx_inter_rate` come from the sig,
    // not the picture-level rows.
    light: Option<&crate::port_enc_mode_config::light_pd1::LightPd1Signals>,
) -> Vec<InterCandOut> {
    use crate::port_md::inject::{
        CandArray, InjectCtx, WmCtrls, inject_inter_candidates, inject_inter_candidates_light_pd1,
    };
    use crate::port_md::predicates::InjectedMvLog;

    let BlockPrelude {
        stacks,
        ref_mv_count,
        search,
        ref_arr,
        me_cands,
    } = prelude;
    let ctx = derive_block_ctx(
        (b.org_y / 4) as i32,
        (b.org_x / 4) as i32,
        b.bsize as usize,
        f.mi_rows,
        f.mi_cols,
        f.tile,
        f.sb_mi_size,
    );
    let grid = MvpGrid {
        entries: b.grid,
        stride: b.grid_stride,
        base: (b.org_y / 4) as i32 * b.grid_stride + (b.org_x / 4) as i32,
    };

    // C `merge_inter_cands` (mode_decision.c:3638-3643), computed once per
    // block in `generate_md_stage_0_cand_light_pd1`: when the best
    // post-subpel ME/PME variance per pixel is under the nic-level
    // threshold, EVERY inter candidate is CAND_CLASS_2 — the MVP and MV
    // lanes share one MDS0 pool instead of competing for separate caps.
    let merge_inter_cands = merge_inter_cands_mult != u8::MAX && {
        // C `uint16_t th = (mult * (63 - scs->static_config.qp)) >> 1`
        // and `(MIN(md_me_dist, md_pme_dist) / (bw * bh)) < th` — C's
        // integer division, with `th` widened for the compare.
        let th = (u32::from(merge_inter_cands_mult) * 63u32.saturating_sub(f.search.cli_qp)) >> 1;
        let me_d = search.md_me_dist();
        let pme_d = search.md_pme_dist();
        let r = me_d.min(pme_d) / ((b.bw * b.bh) as u32) < th;
        #[cfg(feature = "std")]
        if crate::dbgenv::mrgdbg() {
            std::eprintln!(
                "MRGDBG blk=({},{}) {}x{} mult={} th={} me_d={} pme_d={} -> {r}",
                b.org_x,
                b.org_y,
                b.bw,
                b.bh,
                merge_inter_cands_mult,
                th,
                me_d,
                pme_d
            );
        }
        r
    };

    let me_totals = [me_cands.len() as u8];
    let sb_me_mv = search.sb_me_mv;

    // C `ctx->ref_pruning_ctrls` + `ctx->ref_filtering_res`, computed per
    // block by `inter_search_arm::run_block_searches`
    // (`perform_md_reference_pruning`, product_coding_loop.c:9441). A
    // `default()` here silently made every ref valid — which let a pruned
    // LAST2's PME MV in and let it edge a real C candidate out of MDS1's
    // survivor set.
    let ref_pruning = search.ref_pruning.clone();
    // C `svt_aom_init_wm_samples` (adaptive_mv_pred.c:1752) -> the injector's
    // `num_proj_ref`. This was `[0u8; 8]`, and a zero here is not a
    // conservative default: `motion_mode_allowed` promotes a block to
    // WARPED_CAUSAL — and with it the THREE-symbol MOTION_MODES alphabet
    // instead of the two-symbol OBMC one — exactly when this count is >= 1
    // and the frame allows warped motion. The DECODER runs the same scan, so
    // a wrong count is an arithmetic-coder DESYNC, not a quality choice:
    // `docs/INTER-ENCODE-PLAN.md` §1z¹⁸ measured `aomdec` REJECTING 22 of the
    // campaign's 96 cells for this, every one at the preset where
    // `allow_warped_motion` is 1.
    //
    // C's three-part gate is reproduced exactly; the `else` arm zeroes every
    // entry, which is what the old constant happened to be right about on the
    // frames where the gate is false.
    //
    // The SAMPLES are kept, not only the count: C's
    // `svt_aom_init_wm_samples` precomputes both once per block into
    // `ctx->wm_sample_info[ref]`, and `svt_aom_warped_motion_parameters`
    // reads them for EVERY candidate MV rather than re-scanning. Discarding
    // them here would have meant a second, partial transcription of the same
    // four-way scan at every warp derivation.
    let mut wm_sample_num = [0u8; 8];
    let mut wm_samples: [WarpSamples; 8] = Default::default();
    if f.allow_warped_motion
        && crate::port_entropy_inter::modes::is_motion_variation_allowed_bsize(
            svtav1_types::block::BlockSize::from_u8(b.bsize)
                .expect("an injected inter block must have a real BlockSize"),
        )
        && b.overlappable_neighbors != 0
    {
        for &rt in f.ref_frame_type_arr {
            let rf = crate::inter_mvp::av1_set_ref_frame(rt);
            if rf[1] != NONE_FRAME {
                continue;
            }
            let (n, pts, pts_inref) = crate::inter_mvp::find_warp_samples(&grid, &ctx, rf[0]);
            let slot = rf[0].max(0) as usize;
            wm_sample_num[slot] = n;
            wm_samples[slot] = WarpSamples { n, pts, pts_inref };
        }
    }
    // C `ctx->wm_ctrls` = `svt_aom_set_wm_controls(pcs->wm_level)`
    // (enc_mode_config.c:4397). The port derived `wm_level` in the wired
    // picture-level sig-deriv all along and threw it away here.
    //
    // MEASURED (benchmarks/c_motion_mode_census_2026-09-10.meta): C codes 88
    // of 1158 inter blocks WARPED_CAUSAL over the 24-cell video gate, all at
    // preset 6 and none at preset 8 -- which is exactly what this derivation
    // predicts (`wm_level` is 3 at M6 and 0 at M8 for a flat GOP at <= 720p).
    let wmc = crate::port_enc_mode_config::ctrls::set_wm_controls(f.wm_level)
        .expect("set_wm_controls answers None only for a level outside C's switch");
    // `refine_level == 0` (wm_level 1) means C refines the MV AT INJECTION
    // (`svt_aom_wm_motion_refinement` inside `inj_non_simple_modes`), but ONLY
    // on `NEWMV` clones (mode_decision.c:924-926) — non-NEWMV clones skip the
    // refinement and go straight to `svt_aom_warped_motion_parameters`. The
    // refinement is wired (`port_md::mv_refine::wm_motion_refinement`), so the
    // controls reach the injector unmodified at every level.
    let wm_enabled = wmc.enabled != 0;
    // C `ctx->cand_reduction_ctrls` — the light lane reads the per-SB
    // `sig.cand_reduction` (its `cand_reduction_level` is raised above the
    // picture's), the regular lane the picture-level row.
    let cand_red = light.map_or(&f.cand_reduction, |s| &s.cand_reduction);
    let inj = InjectCtx {
        bsize: b.bsize,
        bwidth: b.bw as u16,
        bheight: b.bh as u16,
        blk_org_x: b.org_x as u32,
        blk_org_y: b.org_y as u32,
        // C `ctx->shape == PART_N` — the inter-intra ctrls pick their
        // wedge mode between `wedge_mode_sq` and `wedge_mode_nsq` on it
        // (mode_decision.c:468). It was hardcoded true, which sent every
        // NSQ shape down the square path.
        shape_is_part_n: b.is_part_n,
        // C `frm_hdr->reference_mode == SINGLE_REFERENCE` — the value
        // `allow_bipred` reads. `REFERENCE_SELECT` frames (every inter
        // frame of a complete mini-GOP) get bipred, which is what the
        // compound entries of `ref_frame_type_arr` are for.
        reference_mode_is_single: !f.reference_mode_is_select,
        allow_high_precision_mv: f.allow_high_precision_mv,
        is_motion_mode_switchable: f.is_motion_mode_switchable,
        force_integer_mv: u8::from(f.force_integer_mv),
        // C `frm_hdr->skip_mode_params.skip_mode_flag`. The injector reads it
        // in its NEAREST_NEAREST arm to mark a COMPOUND candidate
        // `skip_mode_allowed`.
        skip_mode_flag: f.skip_mode_flag,
        // C `frm_hdr->skip_mode_params.ref_frame_idx_{0,1}` — the pair the
        // NEAREST_NEARESTMV arm compares a candidate's refs against, derived
        // by `setup_skip_mode_allowed` at picture decision.
        skip_mode_ref_frame_idx_0: f.skip_mode_ref_frame_idx_0,
        skip_mode_ref_frame_idx_1: f.skip_mode_ref_frame_idx_1,
        is_lossless_segment: false,
        // The same block-level list the MVP table and searches used —
        // `determine_best_references`' rebuild when `use_best_me` fired in
        // `block_prelude`, else the picture-level list.
        ref_frame_type_arr: &ref_arr,
        global_motion: &f.global_motion,
        // C `pcs->ppcs->gm_ctrls.skip_identity`, which
        // `svt_aom_set_gm_controls` sets ONLY at gm_level 4: with it set and a
        // reference's model IDENTITY, `inject_global_candidates` `continue`s.
        // This was hardcoded `true`, which suppressed the GLOBALMV candidate C
        // injects at every other level.
        gm_skip_identity: f.gm_skip_identity,
        // C `ctx->global_mv_injection = ppcs->gm_ctrls.enabled`
        // (enc_mode_config.c:7847/:7964). It was hardcoded `true`, which
        // injected GLOBALMV candidates at presets (>= M5) where C's gm
        // level is 0 and the whole section is skipped.
        global_mv_injection: f.gm_enabled,
        wm_sample_num: &wm_sample_num,
        ref_mv_stack: &stacks,
        ref_mv_count: &ref_mv_count,
        nmv_cost: &f.nmv,
        drl_mode_fac_bits: &f.fac.drl_mode,
        // C `ctx->shut_fast_rate` — false on both lanes this reaches
        // (enc_mode_config.c:7565 light / :7908 regular), read off the sig
        // anyway so the transcription survives a level that sets it.
        shut_fast_rate: light.map_or(false, |s| s.shut_fast_rate),
        // C `ctx->approx_inter_rate` — the regular arm writes it from
        // `pcs->approx_inter_rate` (`sig_deriv_enc_dec_default`,
        // enc_mode_config.c:7906); the light-PD1 arm's `MAX(1, ·)` differs
        // exactly where `pcs` is 0, so the sig's own field reads here.
        approx_inter_rate: light.map_or(f.search.approx_inter_rate, |s| s.approx_inter_rate),
        total_me_cnt: me_cands.len(),
        me_cands: &me_cands,
        me_totals: &me_totals,
        me_block_offset: 0,
        sb_me_mv: &sb_me_mv,
        post_subpel_me_mv_cost: &search.post_subpel_me_mv_cost,
        valid_pme_mv: &search.valid_pme_mv,
        best_pme_mv: &search.best_pme_mv,
        ref_pruning: &ref_pruning,
        // C `ctx->corrupted_mv_check`: the `is_valid_mv_diff` guard. On with
        // a real cost table, which is what this module supplies.
        corrupted_mv_check: true,
        // C `ctx->cand_reduction_ctrls.redundant_cand_ctrls`. `score_th` is 0
        // at levels 0..3, i.e. everywhere this port's `cand_reduction_level`
        // can land, so this is inert TODAY and would not be if a level 4+
        // ever became reachable.
        redundant_cand_ctrls: crate::port_md::predicates::RedundantCandCtrls {
            score_th: cand_red.redundant_cand_ctrls.score_th,
            mag_th: cand_red.redundant_cand_ctrls.mag_th,
        },
        // C `set_inter_comp_controls(ctx, pcs->inter_compound_mode)`
        // (enc_mode_config.c:7856/:7973) — `get_inter_compound_level`'s
        // ladder: 0 = "AVG only" (MD_COMP_DIST, every `do_*` off) at
        // M3+, 3 at M0, 4 at M1..M2.
        inter_comp_ctrls: crate::port_enc_mode_config::ctrls::set_inter_comp_controls(
            f.inter_compound_mode,
        )
        .unwrap_or_default(),
        // C `set_inter_intra_ctrls(ctx->inter_intra_comp_ctrls,
        // pcs->inter_intra_level)` (mode_decision.c:420-470 / enc_mode_
        // config.c:5385) — level 2 on the reachable ladder: enabled, no
        // RD model, wedge search on NSQ shapes only.
        inter_intra_comp_ctrls: crate::port_enc_mode_config::ctrls::set_inter_intra_ctrls(
            f.inter_intra_level,
        )
        .map(|c| crate::port_md::inject::InterIntraCompCtrls {
            enabled: c.enabled != 0,
            use_rd_model: c.use_rd_model != 0,
            wedge_mode_sq: c.wedge_mode_sq,
            wedge_mode_nsq: c.wedge_mode_nsq,
        })
        .unwrap_or_default(),
        wm_ctrls: WmCtrls {
            enabled: wm_enabled,
            use_wm_for_mvp: wm_enabled && wmc.use_wm_for_mvp != 0,
            refinement_iterations: wmc.refinement_iterations,
            refine_level: wmc.refine_level,
        },
        // C `ctx->obmc_ctrls`, as `svt_aom_set_obmc_controls` expands
        // `pcs->ppcs->pic_obmc_level`. The injector reads `refine_level` (which
        // MD stage refines the MV, if any) and `enabled`.
        obmc_ctrls: {
            let c = crate::port_enc_mode_config::ctrls::set_obmc_controls(f.pic_obmc_level);
            crate::port_md::inject::ObmcCtrls {
                enabled: c.enabled != 0,
                max_blk_size: c.max_blk_size,
                trans_face_off: c.trans_face_off != 0,
                refine_level: c.refine_level,
            }
        },
        // C `ctx->cand_reduction_ctrls.near_count_ctrls`, and the ONE field
        // of that struct this envelope is not inert in: it is
        // `{enabled 1, near_count 3, near_near_count 3}` at every level the
        // default arm reaches, which is up to three `NEARMV` candidates per
        // single reference. See the module header for the measurement.
        near_count_ctrls: crate::port_md::inject::NearCountCtrls {
            enabled: cand_red.near_count_ctrls.enabled != 0,
            near_count: cand_red.near_count_ctrls.near_count,
            near_near_count: cand_red.near_count_ctrls.near_near_count,
        },
        // C `ctx->bipred3x3_ctrls` — `svt_aom_set_bipred3x3_controls`
        // (`pcs->bipred3x3_injection`) (enc_mode_config.c:5869/:7855).
        // This is what arms `bipred_3x3_candidates_injection`'s +-2
        // per-list NEW_NEWMV ring (mode_decision.c:2911) — the search that
        // lets a compound candidate win mid-walk, so the mi grid carries
        // `{LAST,BWDREF}` stamps later blocks' `setup_ref_mv_list` reads.
        bipred3x3_ctrls: crate::port_enc_mode_config::ctrls::set_bipred3x3_controls(
            f.bipred3x3_injection,
        )
        .map_or_else(crate::port_md::inject::Bipred3x3Ctrls::default, |c| {
            crate::port_md::inject::Bipred3x3Ctrls {
                enabled: c.enabled != 0,
                search_diag: c.search_diag != 0,
                use_best_list: c.use_best_list != 0,
                use_l0_l1_dev: c.use_l0_l1_dev,
            }
        }),
        unipred3x3_injection: 0,
        // C `ctx->new_nearest_injection` — 1 in both sig derivations this
        // lane reaches (enc_mode_config.c:7570/:7848/:7965); the light sig's
        // copy reads here so a level that clears it stays reachable.
        new_nearest_injection: light.map_or(true, |s| s.new_nearest_injection != 0),
        new_nearest_near_comb_injection: 0,
        inject_new_me: true,
        inject_new_pme: true,
        // The same per-block `ctx->updated_enable_pme` resolution
        // `search_in` got in `block_prelude` — the injector and the
        // search read ONE ctx field in C.
        updated_enable_pme: f.search.md_pme_enabled && !is_intra_bordered,
        // C `ctx->cand_reduction_ctrls.reduce_unipred_candidates` — 0 at
        // levels 0..2, so inert on this envelope for the same reason.
        reduce_unipred_candidates: cand_red.reduce_unipred_candidates,
        // C `ctx->cand_reduction_ctrls.use_neighbouring_mode_ctrls.enabled`,
        // which is 1 from level 2 up — read in conjunction with the now-real
        // `is_intra_bordered` below.
        use_neighbouring_mode_ctrls_enabled: cand_red.use_neighbouring_mode_enabled != 0,
        lpd1_mvp_best_me_list: cand_red.lpd1_mvp_best_me_list != 0,
        is_intra_bordered,
        has_overlappable_candidates: b.overlappable_neighbors != 0,
        allow_warped_motion: f.allow_warped_motion,
        left_available: b.neighbors.left_available,
        up_available: b.neighbors.up_available,
        left_mi: b
            .neighbors
            .left_avail()
            .and_then(|m| mode_from_u8(m.mode).map(|md| (md, m.ref_frame))),
        above_mi: b
            .neighbors
            .above_avail()
            .and_then(|m| mode_from_u8(m.mode).map(|md| (md, m.ref_frame))),
    };

    // C allocates `ctx->fast_cand_array` to `pcs->ppcs->max_can_count`
    // (md_process.c:386) and `INC_MD_CAND_CNT` saturates at the same
    // value — the two must agree or a saturated push leaves
    // `inj_comp_modes` reading a stale last candidate.
    let mut cands = CandArray::new(usize::from(f.max_can_count));
    let mut log = InjectedMvLog::default();
    // The block-scoped half of what the MDS1 refinement needs, filled here
    // because this is where the samples, the stacks and the derived controls
    // already exist. `enabled` false makes the refinement a no-op without the
    // funnel having to know why.
    *warp_out = WarpRefineBlock {
        mvp_stacks: core::mem::take(&mut warp_out.mvp_stacks),
        enabled: wm_enabled,
        refinement_iterations: wmc.refinement_iterations,
        refine_diag: wmc.refine_diag != 0,
        shut_approx_if_not_mds0: wmc.shut_approx_if_not_mds0 != 0,
        lower_band_th: wmc.lower_band_th,
        upper_band_th: wmc.upper_band_th,
        refine_level: wmc.refine_level,
        samples: wm_samples,
        stacks: stacks.clone(),
        ref_mv_count,
        mi_row: (b.org_y / 4) as i32,
        mi_col: (b.org_x / 4) as i32,
        bsize: svtav1_types::block::BlockSize::from_u8(b.bsize)
            .expect("an injected inter block must have a real BlockSize"),
        bwidth: b.bw,
        bheight: b.bh,
    };
    // Hand the block's MV stacks to the funnel: the OBMC refinement's DRL
    // re-pick must use the SAME stack the injector priced against.
    warp_out.mvp_stacks.clear();
    warp_out.mvp_stacks.extend_from_slice(&stacks);
    if light.is_some() {
        // C `inject_inter_candidates_light_pd1` takes no warp hooks — the
        // light path never refines an MV at injection (`read_refine_me_mvs_
        // light_pd1` runs the light refine, not the MDS1 warp lane).
        inject_inter_candidates_light_pd1(&inj, &mut cands, &mut log);
    } else {
        let mut hooks = WarpHooks {
            blk: warp_out,
            f,
            b,
            ii_ctrls: inj.inter_intra_comp_ctrls,
            comp_ctrls: inj.inter_comp_ctrls,
            cmp: CmpStore::default(),
        };
        inject_inter_candidates(&inj, &mut cands, &mut log, &mut hooks);
    }

    let mut out = Vec::new();
    for c in cands.as_slice() {
        // Every motion mode C's regular lane can stamp is predictable
        // (`predict_inter_yuv_warped` / the OBMC blend tail), and
        // inter-intra compounds are predicted by the `combine_interintra`
        // tail in `predict_and_price`. The assertion stays because a
        // silently dropped candidate is a mode decision nobody made.
        assert!(
            matches!(
                c.motion_mode,
                crate::port_md::predicates::MotionMode::SimpleTranslation
                    | crate::port_md::predicates::MotionMode::WarpedCausal
                    | crate::port_md::predicates::MotionMode::ObmcCausal
            ),
            "the inter candidate set produced a candidate this port cannot PREDICT \
             (motion_mode {:?}, interintra {}, ref_frame {:?}). Its control was supposed \
             to be off — see `inter_md_arm`'s header. Refusing rather than dropping it, \
             because a silently dropped candidate is a mode decision nobody made.",
            c.motion_mode,
            c.is_interintra_used,
            c.ref_frame,
        );
        // C `blk_ptr->inter_mode_ctx[ref_frame_type]` — the mode context of
        // the candidate's OWN reference TYPE, which for a compound pair is
        // the compound index (>= 8), not `ref_frame[0]`.
        let imc =
            stacks[crate::inter_mvp::av1_ref_frame_type(c.ref_frame).max(0) as usize].mode_context;
        let mut o = predict_and_price(f, b, c, imc, &stacks, lambda);
        // C `cand->cand_class` (mode_decision.c:3662-3669): `NEWMV` /
        // `NEW_NEWMV` — or ANY inter candidate when `merge_inter_cands`
        // fired — is class 2; the remaining inter modes are class 1.
        o.cand_class = if merge_inter_cands
            || matches!(o.mode, PredictionMode::NewMv | PredictionMode::NewNewMv)
        {
            2
        } else {
            1
        };
        out.push(o);
    }
    out
}

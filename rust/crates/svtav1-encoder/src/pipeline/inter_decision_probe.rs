use super::*;
use crate::inter_mvp::{
    InterMvpEnv, OrderHintInfo, TplMvRef, get_av1_mv_pred_drl, setup_ref_mv_list,
};
use crate::intrabc::TileMiBounds;
use crate::intrabc_mvp::{MvpGrid, MvpMiEntry, derive_block_ctx};
use crate::port_md::drl::{ChooseDrlCtx, av1_drl_ctx, choose_best_av1_mv_pred};
use crate::port_md::pme::MvCostTable;
use alloc::vec;
use svtav1_types::motion::{Mv, WarpedMotionParams};
use svtav1_types::prediction::PredictionMode;

/// C `BLOCK_64X64` (definitions.h block order) — the bsize
/// `SVT_CINTER_OUT` printed (`bsize=12`).
const BLOCK_64X64: usize = 12;
use svtav1_types::reference::LAST_FRAME;

/// The reference cell's frame-1 MVP inputs, exactly as the frame header
/// the port already writes byte-identically describes them
/// (`tools/fh_fields.py --index 1`: `primary_ref_frame 0`,
/// `allow_high_precision_mv 0`, `use_ref_frame_mvs 1`,
/// `reference_select 1`, `order_hint 1`).
///
/// `use_ref_frame_mvs = 1` is load-bearing and easy to get wrong: it is
/// the ONLY path that sets the `GLOBALMV_OFFSET` bit on a block with no
/// coded neighbours, and that bit is the whole of C's `imc = 8`. With
/// the flag read as 0 the port would produce `inter_mode_ctx = 0`,
/// which selects a different `newmv` CDF row and a different tile.
#[test]
fn the_ports_own_mvp_stack_reproduces_cs_pred_mv_mode_ctx_and_drl() {
    // 64x64 frame => 16x16 mi cells, one tile.
    let (mi_rows, mi_cols) = (16i32, 16i32);
    let tile = TileMiBounds {
        mi_col_start: 0,
        mi_col_end: mi_cols,
        mi_row_start: 0,
        mi_row_end: mi_rows,
    };
    // The MD mi grid at the FIRST block of the frame: every cell is the
    // default intra entry, because nothing has been committed yet. This
    // is `docs/INTER-ENCODE-PLAN.md` §1s item 2's grid — the type is
    // already the one `setup_ref_mv_list` reads.
    let entries = vec![MvpMiEntry::default(); (mi_rows * mi_cols) as usize];
    let grid = MvpGrid {
        entries: &entries,
        stride: mi_cols,
        base: 0,
    };
    let ctx = derive_block_ctx(0, 0, BLOCK_64X64, mi_rows, mi_cols, tile, 16);
    assert!(
        !ctx.up_available && !ctx.left_available,
        "the frame's first block has no coded neighbours — if this flips, \
             every number below is about a different block"
    );

    let gm = [WarpedMotionParams::default(); 8];
    // C's tpl_mvs after `av1_setup_motion_field`'s reset: INVALID_MV
    // everywhere, so every `add_tpl_ref_mv` returns 0.
    let tpl_stride = mi_cols >> 1;
    let tpl_mvs = vec![TplMvRef::default(); ((mi_rows >> 1) + 8) as usize * tpl_stride as usize];
    let env = InterMvpEnv {
        global_motion: &gm,
        ref_frame_sign_bias: [0; 8],
        allow_high_precision_mv: false,
        force_integer_mv: false,
        use_ref_frame_mvs: true,
        order_hint_info: OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
        },
        cur_order_hint: 1,
        ref_order_hint: [0; 8],
        tpl_mvs: &tpl_mvs,
        tpl_stride,
        sb64_sq_no4xn_geom: true,
        symmetric_refs_eligible: false,
        symmetric_refs: false,
    };

    // C `svt_aom_generate_av1_mvp_table`'s gm_mv for an IDENTITY global
    // motion model is the zero MV.
    let stack = setup_ref_mv_list(&grid, &ctx, &env, LAST_FRAME, [Mv::ZERO; 2]);

    // --- C `imc=8` -------------------------------------------------
    assert_eq!(
        stack.mode_context, 8,
        "inter_mode_ctx: C's SVT_CINTER_OUT prints imc=8"
    );
    // ... and it is the GLOBALMV bit, not an accident of another term.
    assert_eq!(
        stack.count, 0,
        "no coded neighbours and an INVALID_MV temporal field => empty stack"
    );

    // NEGATIVE CONTROL for the paragraph above: the ONLY term that can
    // set bit 3 on a neighbourless block is the temporal-MVP block's
    // `is_available == 0`, so reading `use_ref_frame_mvs` as 0 gives a
    // DIFFERENT mode context — and hence a different `newmv` CDF row and
    // a different tile. Without this the `== 8` assertion could not
    // distinguish "the port reproduces C" from "8 falls out anyway".
    let off = InterMvpEnv {
        use_ref_frame_mvs: false,
        ..env
    };
    let no_mfmv = setup_ref_mv_list(&grid, &ctx, &off, LAST_FRAME, [Mv::ZERO; 2]);
    assert_eq!(
        no_mfmv.mode_context, 0,
        "with use_ref_frame_mvs = 0 the GLOBALMV bit must NOT be set"
    );

    // --- C `drl=0`, `pmv0=0,0`, `drlctx=-1,-1` ---------------------
    // The cost table is unread on this path (`max_drl_index == 1`
    // short-circuits before any MV is priced); a zeroed one makes that
    // explicit instead of smuggling in a rate model the cell does not
    // exercise.
    let nmv_cost = MvCostTable::zeroed();
    let drl_ctx = ChooseDrlCtx {
        shut_fast_rate: false,
        approx_inter_rate: 0,
        ref_mv_stack: &stack.stack,
        ref_mv_count: stack.count,
        nmv_cost: &nmv_cost,
        drl_mode_fac_bits: &[[0; 2]; 3],
    };
    let mut drl_index = 0xFFu8;
    let mut pred_mv = [Mv { x: 111, y: 222 }; 2];
    choose_best_av1_mv_pred(
        &drl_ctx,
        PredictionMode::NewMv,
        Mv { x: -24, y: 0 },
        Mv::ZERO,
        &mut drl_index,
        &mut pred_mv,
    );
    assert_eq!(drl_index, 0, "drl_index: C prints drl=0");
    assert_eq!(
        pred_mv[0],
        Mv::ZERO,
        "pred_mv: C prints pmv0=0,0 — this is what the writer differences the coded MV from"
    );

    // `get_av1_mv_pred_drl` is what MD calls to fill `nearestmv`; on a
    // single-ref NEWMV with an empty stack it must agree.
    let pred = get_av1_mv_pred_drl(
        &stack,
        false,
        PredictionMode::NewMv as u8,
        0,
        crate::inter_mvp::DrlMvPred::default(),
    );
    assert_eq!(pred.ref_mv[0], Mv::ZERO, "get_av1_mv_pred_drl's ref_mv[0]");

    // C's `drl_ctx[2] = {-1, -1}` is the "never computed" sentinel: MD
    // only fills it when `max_drl_index > 1`, which needs a stack with
    // more than one candidate. With `count == 0` the writer must not
    // emit a DRL symbol at all — corroborated independently by the
    // frame-context delta, which shows NO `drl` CDF moving on C's tile
    // (docs/INTER-ENCODE-PLAN.md §1s).
    assert_eq!(
        crate::port_md::predicates::get_max_drl_index(stack.count, PredictionMode::NewMv),
        1,
        "max_drl_index must be 1, i.e. no DRL symbol is coded"
    );
    // A POSITIVE CONTROL on that reasoning: with a two-candidate stack
    // the same code DOES ask for a DRL context, so the assertion above
    // is about this cell rather than about a function that always
    // returns 1.
    let mut two = stack;
    two.count = 2;
    two.stack[0].weight = 700;
    two.stack[1].weight = 700;
    assert!(
        crate::port_md::predicates::get_max_drl_index(two.count, PredictionMode::NewMv) > 1,
        "a 2-candidate stack must signal DRL — otherwise this test proves nothing"
    );
    assert_eq!(av1_drl_ctx(&two.stack, 0), 0);
}

/// **Does the port's OWN motion search find C's MV?**
///
/// C's `SVT_CINTER_OUT` prints `mv0=0,-24` — eighth-pel, i.e. the
/// full-pel `(-3, 0)` that the harness's 3-pixel right-shift makes
/// exact. The port has two searches: the pre-campaign homegrown
/// `crate::motion_est`, which lands on `-22` (a quarter pel short of an
/// EXACT integer match — measured, `docs/INTER-ENCODE-PLAN.md` §1s), and
/// the wholesale port of `motion_estimation.c` in `crate::inter_me`,
/// which nothing calls. This drives the second one on the reference
/// cell's actual planes.
///
/// **Evidence tier: this is NOT a parity claim.** Most of
/// `motion_estimation.c` is `static`, so the composed search can only
/// reach tier 4 (`docs/WORKING-ON-THIS.md` §4); what a green run here
/// says is "the ported search, configured by the ported
/// `svt_aom_sig_deriv_me`, recovers this cell's MV" — a reachability and
/// wiring result, not a bit-exactness one. The tier-1 kernels underneath
/// it are gated in `tests/c_parity_inter_me.rs`.
///
/// It doubles as the POSITIVE CONTROL for
/// `port_enc_mode_config::me::apply_me_signals`: the search area it
/// installs is what makes a `-3` MV reachable at all, so a bridge that
/// silently wrote nothing would fail here rather than pass quietly.
#[test]
fn the_ports_own_svt_motion_search_finds_cs_mv_on_the_reference_cell() {
    use crate::inter_me::context::{
        MeB64Output, MeContext, MeDsRef, MePicParams, MeRefs, MeSrcBufs, Plane,
    };
    use crate::inter_me::context::{PU_8X8_0, PU_16X16_0, PU_32X32_0, PU_64X64};
    use crate::inter_me::motion_estimation_b64;
    use crate::port_enc_mode_config::ResolutionRange;
    use crate::port_enc_mode_config::me::{MeDerivInputs, apply_me_signals, sig_deriv_me};
    use crate::port_preanalysis::{downsample_2d, generate_padding};

    const W: usize = 64;
    const H: usize = 64;
    /// `tools/identity_run`'s `SVTAV1_FRAME_SHIFT` default.
    const SHIFT: usize = 3;

    // `tools/identity_run`'s `gradient` plane and its frame-1 translate.
    let f0: Vec<u8> = (0..H)
        .flat_map(|r| (0..W).map(move |c| (((r * 255) / H) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect();
    let mut f1 = vec![0u8; W * H];
    for r in 0..H {
        for c in 0..W {
            f1[r * W + c] = f0[r * W + c.saturating_sub(SHIFT)];
        }
    }
    assert_ne!(f0, f1, "the translate must actually move the picture");

    // The REFERENCE picture, with C's replicated margin
    // (`svt_aom_generate_padding`, tier-1 gated in
    // `c_parity_preanalysis.rs`). The margin is not decoration here: at
    // MV -3 the block's left three columns read OUTSIDE the frame, and
    // the harness built frame 1 by replicating column 0 — so the match
    // is EXACT only against a replicated margin, and only then can the
    // residual be zero, which is what C's `skip = 1` records.
    const BORDER: usize = 64;
    let stride = W + 2 * BORDER;
    let rows = H + 2 * BORDER;
    let org = BORDER * stride + BORDER;
    let mut refbuf = vec![0u8; stride * rows];
    for r in 0..H {
        refbuf[org + r * stride..org + r * stride + W].copy_from_slice(&f0[r * W..r * W + W]);
    }
    generate_padding(&mut refbuf, org, stride, W, H, BORDER, BORDER);
    assert_eq!(
        refbuf[org - 3],
        f0[0],
        "the left margin must replicate column 0 — that is what makes the -3 match exact"
    );

    // The 1/4 and 1/16 luma pyramids HME levels 1 and 0 search, built
    // with the same `svt_aom_downsample_2d` C uses
    // (`svt_aom_downsample_filtering_input_picture`), each padded to its
    // own border.
    let mk_ds = |src: &[u8], s_org: usize, s_stride: usize, sw: usize, sh: usize, step: usize| {
        let (dw, dh) = (sw / step, sh / step);
        let db = BORDER / step;
        let dstride = dw + 2 * db;
        let dorg = db * dstride + db;
        let mut buf = vec![0u8; dstride * (dh + 2 * db)];
        downsample_2d(
            &src[s_org..],
            s_stride,
            sw,
            sh,
            &mut buf[dorg..],
            dstride,
            step,
        );
        generate_padding(&mut buf, dorg, dstride, dw, dh, db, db);
        (buf, dorg, dstride, dw, dh, db)
    };
    let (qbuf, qorg, qstride, qw, qh, qb) = mk_ds(&refbuf, org, stride, W, H, 2);
    let (sbuf, sorg, sstride, sw_, sh_, sb_) = mk_ds(&qbuf, qorg, qstride, qw, qh, 2);

    let refs = MeRefs {
        arr: [
            [
                Some(MeDsRef {
                    picture: Plane {
                        data: &refbuf,
                        org,
                        stride,
                        width: W as u16,
                        height: H as u16,
                        border: BORDER as u16,
                    },
                    quarter: Plane {
                        data: &qbuf,
                        org: qorg,
                        stride: qstride,
                        width: qw as u16,
                        height: qh as u16,
                        border: qb as u16,
                    },
                    sixteenth: Plane {
                        data: &sbuf,
                        org: sorg,
                        stride: sstride,
                        width: sw_ as u16,
                        height: sh_ as u16,
                        border: sb_ as u16,
                    },
                    picture_number: 0,
                }),
                None,
                None,
                None,
            ],
            [None, None, None, None],
        ],
    };

    // The SOURCE b64 and its two decimations (C `quarter_b64_buffer` /
    // `sixteenth_b64_buffer`).
    let mut src_q = vec![0u8; (W / 2) * (H / 2)];
    downsample_2d(&f1, W, W, H, &mut src_q, W / 2, 2);
    let mut src_s = vec![0u8; (W / 4) * (H / 4)];
    downsample_2d(&src_q, W / 2, W / 2, H / 2, &mut src_s, W / 4, 2);
    let src = MeSrcBufs {
        b64: &f1,
        b64_stride: W,
        quarter: &src_q,
        quarter_stride: W / 2,
        sixteenth: &src_s,
        sixteenth_stride: W / 4,
    };

    let pic = MePicParams {
        picture_number: 1,
        aligned_width: W as i16,
        aligned_height: H as i16,
        enhanced_width: W as u32,
        enhanced_height: H as u32,
        ahd_error: u32::MAX,
        input_resolution: 0,
        enable_me_8x8: true,
        enable_me_16x16: true,
        max_number_of_pus_per_sb: 85,
        hierarchical_levels: 0,
        similar_brightness_refs: false,
        frame_is_boosted: false,
        frame_is_leaf: false,
        gm_enabled: false,
        only_l_bwd: false,
        max_cand: 23,
        max_refs: 7,
        max_l0: 4,
        b64_geom_width: W as u32,
        b64_geom_height: H as u32,
        input_width: W as u16,
        input_height: H as u16,
    };

    // `frame_is_boosted` is the one derivation input this cell does not
    // pin from a dump, so BOTH values are swept: the answer must not
    // depend on a guess.
    for &boosted in &[false, true] {
        let signals = sig_deriv_me(
            MeDerivInputs {
                enc_mode: 6,
                sc_class5: 0,
                input_resolution: ResolutionRange::R240p,
                rtc_tune: false,
                is_base: boosted,
                hierarchical_levels: 0,
                // `enc_mode_config.c:1987-1999` sets all three
                // unconditionally (quoted in `port_preanalysis`).
                enable_hme_flag: 1,
                enable_hme_level0_flag: 1,
                enable_hme_level1_flag: 1,
                enable_hme_level2_flag: 1,
                use_best_me_unipred_cand_only: 0,
                me_qp_based_th_scaling: false,
                hme_qp_based_th_scaling: false,
                qp: 40,
                safe_limit_nref: 0,
                safe_limit_zz_th: 0,
            },
            crate::reference::SvtReference::Hybrid3115,
        );
        let mut me = MeContext::default();
        apply_me_signals(&mut me, &signals);
        // POSITIVE CONTROL that the bridge wrote something: a default
        // `MeContext` has a ZERO search area, in which no MV but (0,0)
        // is reachable.
        assert!(
            me.me_sa.sa_max.width >= 8 && me.me_sa.sa_max.height >= 3,
            "apply_me_signals installed no search area (boosted={boosted})"
        );
        me.num_of_list_to_search = 1;
        me.num_of_ref_pic_to_search = [1, 0];
        me.me_type = crate::inter_me::context::MeType::OpenLoop;

        let mut out = MeB64Output::new(pic.max_cand, pic.max_refs);
        motion_estimation_b64(&pic, 0, 0, &mut me, &src, &refs, &mut out);

        assert_eq!(
            (out.me_mv_array[0].x, out.me_mv_array[0].y),
            (-(SHIFT as i16), 0),
            "the ported SVT search must recover the cell's full-pel MV \
                 (boosted={boosted}); C codes it as the eighth-pel {}",
            -(SHIFT as i32) * 8
        );
        assert_eq!(
            me.p_sb_best_sad[0][0][PU_64X64], 0,
            "an exact translation against a replicated margin has SAD 0 \
                 (boosted={boosted}) — a non-zero SAD here is what would make \
                 C's skip = 1 unreachable"
        );
        for i in [PU_32X32_0, PU_16X16_0, PU_8X8_0] {
            assert_eq!(me.p_sb_best_sad[0][0][i], 0, "sub-PU {i} SAD");
        }
    }
}

/// **The REAL pack walk writes C's tile**, not a hand-assembled
/// composition of the same writers.
///
/// `inter_tile_byte_gate` drives
/// `port_entropy_inter::block::write_inter_mode_info` directly, with
/// every field of C's measured decision spelled out at the call site —
/// including `pred_mv`, `inter_mode_ctx` and `drl_ctx`. This test runs
/// the same cell through `encode_block_syntax`, the function the frame's
/// entropy walk actually calls, and hands it only what MODE DECISION
/// decides (`partition::InterDecision`): the mode, the reference, the MV
/// and `drl_index`. The three context fields are DERIVED inside the pack
/// from `EntropyCtx`'s committed mode-info grid.
///
/// So it gates two things the direct call cannot see:
///
/// * the frame-level `InterSyntaxState` -> `InterFrameSyntax` plumbing,
///   and the neighbour derivation from the pack's own mi grid;
/// * that the DERIVED `pred_mv` / `inter_mode_ctx` / `drl_ctx` equal
///   C's measured ones on a grid with nothing committed yet — i.e. that
///   moving them out of the MD payload did not change the bytes.
///
/// It also pins the prologue this arm now shares with every other
/// block: `write_skip` and `write_intra_inter` come from
/// `encode_block_syntax` itself here, where the older gate wrote them by
/// hand.
#[test]
fn the_real_pack_walk_writes_cs_inter_tile() {
    use crate::entropy::writer::AomWriter;
    use crate::partition::{BlockDecision, InterDecision, PartitionType};
    use crate::port_entropy_inter::modes::MotionMode;
    use crate::port_entropy_inter::refframe::ReferenceMode;
    use crate::rate_control::RcMode;
    use svtav1_types::prediction::PredictionMode;

    let (w, h) = (64usize, 64usize);
    let y: Vec<u8> = (0..h)
        .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect();
    let uv = vec![128u8; (w / 2) * (h / 2)];

    let mut pipeline = EncodePipeline::new(
        w as u32,
        h as u32,
        6,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..RcConfig::default()
        },
        0,
        64,
    )
    .with_bit_depth(8)
    .with_chroma_420(true);
    let f0 = pipeline
        .try_encode_frame_420(&y, &uv, &uv, w)
        .expect("the video-mode key frame encodes");
    assert_eq!(
        f0.len(),
        961,
        "this is not the reference cell — frame 0 must be C's 961 bytes"
    );
    let saved = pipeline
        .dpb
        .get(0)
        .and_then(|r| r.frame_cdfs.clone())
        .expect("the key frame stored its end-of-frame CDFs");

    let mut fc = saved.fc.clone();
    let mut coeff_fc = saved.coeff.clone();
    let mut writer = AomWriter::new(256);
    let mut ectx = EntropyCtx::new(
        w / 4,
        h / 4,
        false,
        true,
        false,
        8,
        svtav1_types::chroma::ChromaFormat::Yuv420,
    );
    // The frame-1 header the port already writes byte-identically
    // (tools/inter_fh_gate.sh): reference_select 1, SWITCHABLE interp,
    // allow_high_precision_mv 0, use_ref_frame_mvs 1, order_hint 1.
    ectx.inter_syntax = Some(InterSyntaxState {
        // This unit test drives the inter pack arm directly; C's frame-2
        // skip-mode bit is off on the GOP it models.
        skip_mode_flag: false,
        skip_mode_ref_frame_idx_0: -1,
        skip_mode_ref_frame_idx_1: -1,
        reference_mode: ReferenceMode::Select,
        interpolation_filter: crate::port_enc_mode_config::md_config::SWITCHABLE,
        enable_dual_filter: false,
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_jnt_comp: false,
        enable_order_hint: true,
        order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
        is_motion_mode_switchable: true,
        allow_warped_motion: true,
        allow_high_precision_mv: false,
        force_integer_mv: false,
        gm_wmtype: [crate::port_entropy_inter::modes::TransformationType::Identity; 8],
        cur_order_hint: 1,
        ref_order_hint: [0; 7],
        use_ref_frame_mvs: true,
    });
    let (mi_cols, mi_rows) = ((w / 4) as i32, (h / 4) as i32);
    let tpl_stride = (mi_cols + 1) >> 1;
    ectx.arm_inter_mvp(crate::partition::InterMdEnv {
        mi_stride: mi_cols,
        mi_rows,
        mi_cols,
        tile: crate::intrabc::TileMiBounds {
            mi_col_start: 0,
            mi_col_end: mi_cols,
            mi_row_start: 0,
            mi_row_end: mi_rows,
        },
        sb_mi_size: 16,
        global_motion: [svtav1_types::motion::WarpedMotionParams::default(); 8],
        allow_high_precision_mv: false,
        force_integer_mv: false,
        use_ref_frame_mvs: true,
        order_hint_info: crate::inter_mvp::OrderHintInfo {
            enable_order_hint: true,
            order_hint_bits: u32::from(crate::entropy::obu::ORDER_HINT_BITS),
        },
        cur_order_hint: 1,
        ref_order_hint: [0; 8],
        ref_frame_sign_bias: [0; 8],
        symmetric_refs_eligible: false,
        tpl_mvs: vec![
            crate::inter_mvp::TplMvRef::default();
            (((mi_rows + 32) >> 1) * tpl_stride) as usize
        ],
        tpl_stride,
        sb_size_64: true,
    });

    // C's measured decision, minus everything the pack now derives.
    let decision = BlockDecision {
        partition_type: PartitionType::None,
        is_inter: true,
        width: 64,
        height: 64,
        eob: 0,
        inter: Some(alloc::boxed::Box::new(InterDecision {
            mode: PredictionMode::NewMv,
            ref_frame: [1, -1],
            mv: [
                svtav1_types::motion::Mv { x: -24, y: 0 },
                svtav1_types::motion::Mv::ZERO,
            ],
            drl_index: 0,
            interp_filters: 0,
            motion_mode: MotionMode::SimpleTranslation,
            num_proj_ref: 0,
            overlappable_neighbors: 0,
            skip_mode: false,
            comp_group_idx: 0,
            compound_idx: 0,
            interinter_comp_type: 0,
            interinter_mask_type: 0,
            interinter_wedge_index: 0,
            interinter_wedge_sign: false,
            is_interintra_used: false,
            interintra_mode: 0,
            use_wedge_interintra: false,
            interintra_wedge_index: 0,
            wm_params: Default::default(),
            wm_params_l1: Default::default(),
        })),
        ..Default::default()
    };

    // The partition symbol is the WALK's, not the block writer's.
    let (part_ctx, nsymbs) = ectx.partition_ctx(0, 0, 64);
    crate::entropy::context::write_partition_edge(
        &mut writer,
        &mut fc,
        part_ctx,
        0,
        nsymbs,
        false,
        true,
        true,
    );
    let mut geom = crate::deblock::DeblockGeom::new(w, h, w, h);
    encode_block_syntax(
        &decision,
        &mut writer,
        &mut fc,
        &mut coeff_fc,
        160,
        &mut ectx,
        /*is_key=*/ false,
        0,
        0,
        &mut None,
        &mut geom,
        false,
    );
    let tile = writer.done().to_vec();
    assert_eq!(
        tile.as_slice(),
        &[0x94u8, 0x9a, 0xb0][..],
        "the real pack walk's inter tile does not match C's; port {tile:02x?}"
    );

    // The mi grid the walk stamped is what a NEXT block would scan.
    // Asserting it here is the positive control for `record_inter_mi`'s
    // grid half: without it the MVP walk would read an all-intra map and
    // this test would still pass, because the FIRST block of a frame has
    // no neighbours to read.
    let nb = ectx.inter_neighbors(0, 8);
    assert_eq!(
        nb.above.map(|a| a.ref_frame),
        Some([1, -1]),
        "the block below must see a LAST_FRAME inter neighbour"
    );
}

/// **LAST is a slot the RPS names, and from poc 2 it is NOT slot 0.**
///
/// Three pipeline sites read the reference picture — `ref_frame_data`
/// (the ME source), `ref_padded_luma` (what motion compensation indexes)
/// and PD0's `sb_min_sq_size` read — and all three took `self.dpb.get(0)`
/// until 2026-09-03, a hard-coded DPB SLOT. C resolves LAST through
/// `pcs->ppcs->ref_pic_ptr_array[REF_LIST_0][0]`, i.e. the picture's own
/// `rps.ref_dpb_index[LAST]`.
///
/// The two agree on every frame this repo's gates cover — poc 1's LAST
/// IS slot 0 — and diverge at poc 2, because frame 1 refreshes slot 1.
/// MEASURED before the fix on `gradient 64x64 q32 p8 frames=3`: the
/// port's frame-2 MD searched `mv=(2,-36)` against poc 0 where the true
/// poc-1 displacement is `(0,-24)`, and coded 100 % intra at 466 B
/// against C's 21; after, 22 B against 21.
///
/// **No byte cell can witness this**, because the frame-2 refusal fires
/// before frame 2 is coded and lifting it needs an env var this crate
/// resolves once per process. So this pins the PREMISE instead: LAST
/// walks 0 -> 0 -> 1 while frame 1's `refresh_frame_mask` leaves slot 0
/// holding the key frame, i.e. LAST and slot 0 name DIFFERENT pictures
/// from poc 2 on. Without that the constant would have been harmless.
#[test]
fn last_is_not_dpb_slot_zero_from_poc_two() {
    use crate::rate_control::RcMode;
    let mut pipeline = EncodePipeline::new(
        64,
        64,
        8,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 32,
            ..RcConfig::default()
        },
        0,
        64,
    )
    .with_bit_depth(8)
    .with_chroma_420(true);

    let p0 = pipeline
        .run_picture_decision(0, true)
        .expect("the key frame's picture decision");
    let p1 = pipeline
        .run_picture_decision(1, false)
        .expect("poc 1's picture decision");
    let p2 = pipeline
        .run_picture_decision(2, false)
        .expect("poc 2's picture decision");

    let last = crate::port_picstruct::LAST;
    assert_eq!(
        (p1.rps.ref_dpb_index[last], p2.rps.ref_dpb_index[last]),
        (0, 1),
        "C's LAST is DPB slot 0 at poc 1 and slot 1 at poc 2"
    );
    // The other half of the defect: slot 0 is NOT overwritten by frame 1,
    // so at poc 2 the hard-coded slot named the KEY frame, two
    // displacements away instead of one.
    assert_eq!(
        p0.rps.refresh_frame_mask & 0x01,
        0x01,
        "the key frame refreshes slot 0"
    );
    assert_eq!(
        p1.rps.refresh_frame_mask & 0x01,
        0,
        "frame 1 must NOT refresh slot 0 — if it did, get(0) would have \
             been right by accident and this defect would be unreachable"
    );
}

/// **The DPB's reference carries C's replicated margin** — §1s item 4.
///
/// C pads a recon before it becomes a reference
/// (`pad_ref_and_set_flags`, enc_dec_process.c:1072-1112, `border =
/// BLOCK_SIZE_64 + 4`). The port stored bare planes, and the inter
/// prediction filled every out-of-frame sample with the constant **128**
/// — a value no decoder produces.
///
/// §1t measured why that is a MODE-DECISION defect and not only a
/// conformance one: the harness translates frame 1 right by 3 px WITH
/// left-edge replication, so at the correct MV `-3` the block's first
/// three columns read outside the reference and match EXACTLY only
/// against a replicated margin. Against a 128 fill the residual is large
/// and C's `skip = 1` is unreachable at any quantizer.
///
/// This asserts the three things that break: the margin exists on the
/// DPB slot, it replicates the edge in all four directions, and the
/// prediction path reads it. The last is a POSITIVE CONTROL with teeth —
/// it compares against the 128 the old path would have produced, so a
/// padded plane that were built but never consulted fails here.
#[test]
fn the_dpb_reference_carries_cs_replicated_margin() {
    use crate::picture::ref_pic_border;
    use crate::rate_control::RcMode;

    let (w, h) = (64usize, 64usize);
    let y: Vec<u8> = (0..h)
        .flat_map(|r| (0..w).map(move |c| (((r * 255) / h) as u8) ^ (((c * 3) & 0x3f) as u8)))
        .collect();
    let uv = vec![128u8; (w / 2) * (h / 2)];
    let mut pipeline = EncodePipeline::new(
        w as u32,
        h as u32,
        6,
        RcConfig {
            mode: RcMode::Cqp,
            qp: 40,
            ..RcConfig::default()
        },
        0,
        64,
    )
    .with_bit_depth(8)
    .with_chroma_420(true);
    let f0 = pipeline
        .try_encode_frame_420(&y, &uv, &uv, w)
        .expect("the video-mode key frame encodes");
    assert_eq!(f0.len(), 961, "this is not the reference cell");

    let slot = pipeline.dpb.get(0).expect("the key frame refreshed slot 0");
    let padded = slot
        .padded
        .as_ref()
        .expect("a stored reference must carry its padded twin");
    // The reference-picture border is `sb_size + 32`
    // (enc_handle.c:1212-1217), not the input-picture `scs->border`.
    let ref_border = ref_pic_border(pipeline.sb_size, false);
    assert_eq!(padded.y.border, ref_border);
    assert_eq!(padded.y.width, w);
    assert_eq!(padded.y.height, h);
    let (cu, cv) = padded.uv.as_ref().expect("4:2:0 chroma is reconstructed");
    // C `(border + ss_x) >> ss_x` at 4:2:0 (enc_dec_process.c:1102).
    assert_eq!(cu.border, (ref_border + 1) >> 1);
    assert_eq!(cv.border, (ref_border + 1) >> 1);
    assert_eq!((cu.width, cu.height), (w / 2, h / 2));

    // The recon this margin replicates. Reading it back off the bare
    // plane keeps the two representations honest: a padded plane built
    // from the WRONG buffer would pass every size assertion above.
    let recon = &slot.y_plane;
    for r in 0..h {
        assert_eq!(padded.y.at(0, r as isize), recon[r * w]);
        for d in 1..=ref_border as isize {
            assert_eq!(
                padded.y.at(-d, r as isize),
                recon[r * w],
                "left margin at row {r}, depth {d} must replicate column 0"
            );
            assert_eq!(
                padded.y.at(w as isize - 1 + d, r as isize),
                recon[r * w + w - 1],
                "right margin at row {r}, depth {d}"
            );
        }
    }
    for c in 0..w {
        for d in 1..=ref_border as isize {
            assert_eq!(
                padded.y.at(c as isize, -d),
                recon[c],
                "top margin at col {c}, depth {d}"
            );
            assert_eq!(
                padded.y.at(c as isize, h as isize - 1 + d),
                recon[(h - 1) * w + c],
                "bottom margin at col {c}, depth {d}"
            );
        }
    }
    // The CORNERS are the part `generate_padding` gets right by
    // replicating the already-padded first and last ROWS, not by a
    // second horizontal pass — worth pinning, because a port that padded
    // vertically first leaves them zero.
    assert_eq!(padded.y.at(-1, -1), recon[0]);
    assert_eq!(padded.y.at(w as isize, h as isize), recon[h * w - 1]);

    // The prediction path reads it. `generate_inter_pred` is `pub(crate)`
    // only through `partition`, so drive it the way the search does: a
    // 8x8 block at the frame's left edge with a full-pel MV of -3, whose
    // first three columns are OUTSIDE the reference.
    let rfc = crate::partition::RefFrameCtx {
        y_plane: recon,
        stride: w,
        pic_width: w,
        pic_height: h,
        mv_map: None,
        mv_map_stride: 0,
        y_padded: Some(&padded.y),
        uv_padded: padded.uv.as_ref(),
        ss_x: 1,
        ss_y: 1,
        sb_size: 64,
    };
    // A FULL-PEL MV takes the convolve's COPY corner, so the prediction
    // is the reference sampled at the MV — including the three columns
    // that fall outside the frame, which is the whole point.
    let pred = crate::partition::generate_inter_pred_for_test(
        &rfc,
        svtav1_types::motion::Mv { x: -24, y: 0 },
        0,
        0,
        8,
        8,
    );
    for r in 0..8usize {
        for c in 0..8usize {
            let want = recon[r * w + (c as isize - 3).max(0) as usize];
            assert_eq!(
                pred[r * 8 + c],
                want,
                "prediction at ({c}, {r}) must come from the replicated margin, not 128"
            );
        }
    }
    assert_ne!(
        pred[0], 128,
        "the out-of-frame column still reads the 128 fill this replaced"
    );

    // A SUB-PEL MV must not take the copy corner. This is the positive
    // control for the convolve swap itself (§1s item 5): the homegrown
    // BILINEAR this replaced is a 2-tap average of the two neighbouring
    // samples, so it can never leave the interval they bound — C's 8-tap
    // filter has negative taps and routinely does. Asserting only "the
    // result changed" would pass for any other bug; asserting it leaves
    // the bilinear interval proves an 8-tap filter ran.
    let sub = crate::partition::generate_inter_pred_for_test(
        &rfc,
        svtav1_types::motion::Mv { x: 4, y: 0 },
        8,
        8,
        8,
        8,
    );
    let mut outside_bilinear_interval = 0usize;
    for r in 0..8usize {
        for c in 0..8usize {
            let a = i32::from(recon[(8 + r) * w + 8 + c]);
            let b = i32::from(recon[(8 + r) * w + 9 + c]);
            let v = i32::from(sub[r * 8 + c]);
            if v < a.min(b) || v > a.max(b) {
                outside_bilinear_interval += 1;
            }
        }
    }
    assert!(
        outside_bilinear_interval > 0,
        "every half-pel sample stayed inside the two-tap interval — an 8-tap \
             convolve did not run (bilinear cannot leave it, C's filter can)"
    );
}

use super::*;

/// Encode a superblock with a FIXED square partition tree (from the
/// C-exact PD0 decision, `crate::pd0`): no shape search happens here —
/// the tree is walked in coding order and every leaf is coded with the
/// same mode-decision block coder the search path uses
/// (`encode_with_neighbors`), reconstructing into the live frame buffer.
///
/// This mirrors C's `fixed_partition` PD1 pass at allintra effective-M9:
/// the partition structure comes from PD0 (pred_depth_only, nsq search
/// off), and only per-block mode/coeff decisions remain.
#[allow(clippy::too_many_arguments)]
/// Build the walk/entropy-pass [`BlockDecision`] for a funnel leaf choice
/// (shared by the fixed-tree path and the depth-refined walk).
pub(crate) fn funnel_block_decision(
    choice: crate::leaf_funnel::LeafChoice,
    w: usize,
    h: usize,
) -> BlockDecision {
    let mut choice = choice;
    let (qcoeffs, eob, tx_type) = if choice.tx_depth == 0 {
        // Unpack the packed (<= 32-capped) txb into the full
        // w x h raster the depth-0 walk path expects.
        let (pw, ph) = (w.min(32), h.min(32));
        let tx_type = choice.txb_tx_types[0];
        // AV1 eob is a SCAN-ORDER quantity. The previous raster-order
        // last-nonzero was only ever consumed as `== 0` (correct either way),
        // but it made the SVTAV1_DUMP_TREE `eob` field wildly misleading — a
        // 32x32 leaf whose true scan-order eob is 299 showed as ~706 (the
        // raster index of the last retained diagonal coeff). The tx pipeline
        // already stores that true scan-order eob per txb (`txb_eobs`), so
        // read it rather than re-scanning the packed raster (the coder
        // re-derives it identically at pipeline.rs `write_coeffs_txb_1d`).
        let eob = choice.txb_eobs.first().copied().unwrap_or_else(|| {
            // Defensive fallback (txb_qcoeffs is populated but txb_eobs is
            // not — unreachable in practice; see the parallel construction
            // at leaf_funnel/types.rs): the scan-order re-derivation.
            let tx_size = crate::entropy::coeff_c::tx_size_from_dims(pw, ph);
            let sidx =
                crate::entropy::scan_tables::TX_TYPE_TO_SCAN_INDEX[tx_type as usize] as usize;
            let scan = crate::entropy::scan_tables::scan(tx_size, sidx);
            let mut e = 0u16;
            for (i, &pos) in scan.iter().enumerate() {
                if choice.txb_qcoeffs[0][pos as usize] != 0 {
                    e = (i + 1) as u16;
                }
            }
            e
        });
        // No 64-dim fold on this block: the "packed" txb IS the full w x h
        // raster (`pw == w && ph == h`, and `tx_unit` sizes the buffer
        // `pw * ph`), so the unpack above was a byte-for-byte copy of it into a
        // freshly allocated `Vec` that was then dropped along with the source.
        // Take it instead. Blocks with a 64-dim side still unpack, because
        // there the raster is genuinely larger than the packed corner and the
        // rows/columns past 32 must stay zero.
        let full = if pw == w && ph == h {
            core::mem::take(&mut choice.txb_qcoeffs[0])
        } else {
            let mut full = alloc::vec![0i32; w * h];
            for r in 0..ph {
                full[r * w..r * w + pw]
                    .copy_from_slice(&choice.txb_qcoeffs[0][r * pw..r * pw + pw]);
            }
            full
        };
        (full, eob, tx_type)
    } else {
        let total: u32 = choice.txb_eobs.iter().map(|&e| e as u32).sum();
        (alloc::vec::Vec::new(), total.min(u16::MAX as u32) as u16, 0)
    };
    BlockDecision {
        partition_type: PartitionType::None,
        intra_mode: choice.mode,
        tx_type,
        qcoeffs,
        eob,
        width: w as u16,
        height: h as u16,
        filter_intra_mode: choice.fi_mode,
        uv_mode: choice.uv_mode,
        cfl_alpha_idx: choice.cfl_alpha_idx,
        cfl_alpha_signs: choice.cfl_alpha_signs,
        palette: choice.palette,
        angle_delta: choice.angle_delta,
        uv_angle_delta: choice.uv_angle_delta,
        tx_depth: choice.tx_depth,
        txb_qcoeffs: if choice.tx_depth > 0 {
            choice.txb_qcoeffs
        } else {
            alloc::vec::Vec::new()
        },
        txb_eobs: if choice.tx_depth > 0 {
            choice.txb_eobs
        } else {
            alloc::vec::Vec::new()
        },
        txb_tx_types: if choice.tx_depth > 0 {
            choice.txb_tx_types
        } else {
            alloc::vec::Vec::new()
        },
        chroma_dec: Some((
            choice.u_qcoeffs,
            choice.v_qcoeffs,
            choice.u_eob,
            choice.v_eob,
            choice.u_recon,
            choice.v_recon,
        )),
        use_intrabc: choice.ibc.is_some(),
        dv: choice.ibc.map(|(dv, _)| dv).unwrap_or_default(),
        dv_ref: choice.ibc.map(|(_, r)| r).unwrap_or_default(),
        // The INTER winner (`docs/INTER-ENCODE-PLAN.md` §1s items 1b + 7).
        // `is_inter` and `inter` are set TOGETHER: `encode_block_syntax`'s
        // inter arm panics on `is_inter` without a payload rather than
        // falling back, because a quiet fallback turns an undecodable stream
        // back into a byte divergence (§1u).
        is_inter: choice.inter.is_some(),
        inter: choice.inter.map(|i| {
            alloc::boxed::Box::new(InterDecision {
                mode: i.mode,
                ref_frame: i.ref_frame,
                mv: i.mv,
                drl_index: i.drl_index,
                interp_filters: i.interp_filters,
                motion_mode: i.motion_mode,
                num_proj_ref: u16::from(i.num_proj_ref),
                overlappable_neighbors: u32::from(i.overlappable_neighbors),
                // C `block_mi.skip_mode` — the full-cost skip-mode
                // arbitration's decision (rd_cost.c:1443), real now that
                // the frame's skip-mode pair reaches the injector.
                skip_mode: i.skip_mode,
                comp_group_idx: i.comp_group_idx,
                compound_idx: i.compound_idx,
                interinter_comp_type: i.interinter_comp_type,
                interinter_mask_type: i.interinter_mask_type,
                interinter_wedge_index: i.interinter_wedge_index,
                interinter_wedge_sign: i.interinter_wedge_sign,
                is_interintra_used: i.is_interintra_used,
                interintra_mode: i.interintra_mode,
                use_wedge_interintra: i.use_wedge_interintra,
                interintra_wedge_index: i.interintra_wedge_index,
                wm_params: i.wm_params,
                wm_params_l1: i.wm_params_l1,
            })
        }),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_fixed_tree(
    // Full frame-origin source: IntraBC searches absolute coordinates.
    src: &[u8],
    src_stride: usize,
    recon: &mut [u8],
    recon_stride: usize,
    tree: &crate::pd0::Pd0Tree,
    size: usize,
    qindex: u8,
    config: &PartitionSearchConfig,
    abs_x: usize,
    abs_y: usize,
    // ALIGNED frame dims — a PD0-leaf node that is a single-edge (one-false)
    // block against this grid is coded as PARTITION_HORZ / PARTITION_VERT
    // (its single in-frame block), matching C's `set_blocks_to_test` edge
    // shape (task #95 chunk 2). On a 64-aligned frame every leaf is complete,
    // so this is byte-neutral.
    aligned_w: usize,
    aligned_h: usize,
    sb_vars: &crate::pd0::SbVariance,
    sb_org: (usize, usize),
    mut funnel: Option<&mut crate::leaf_funnel::FunnelCtx<'_>>,
    // The reference context the non-funnel leaf arm's inter candidate reads.
    // `None` on every key frame (still/mono) — the leaf is intra-only then.
    // On a 4:4:4 inter frame the funnel never arms (`use_funnel` is
    // 4:2:0-only), so this is the arm that carries inter there.
    ref_ctx: Option<&RefFrameCtx>,
) -> PartitionResult {
    match tree {
        crate::pd0::Pd0Tree::Leaf(leaf_size) => {
            let src_off = abs_y * src_stride + abs_x;
            debug_assert_eq!(*leaf_size, size, "PD0 leaf size must match node size");
            // C-exact leaf funnel (presets 6/7/8/eff-M9, 4:2:0 still): the
            // MDS0/MDS1/MDS3 mode decision replaces the homegrown leaf
            // coder; the walk codes exactly what it decided.
            if let Some(fx) = funnel.as_deref_mut() {
                // eff-M9 (intra_level 8) arms the is_dc_only variance gate;
                // when it fires the funnel injects only DC. Dead at M6/M7/M8
                // (dc_only_gate false -> full {DC,V,H,SMOOTH} candidate set).
                let dc_only = fx.frame.cfg.dc_only_gate
                    && crate::pd0::is_dc_only_safe(
                        sb_vars,
                        size,
                        abs_x - sb_org.0,
                        abs_y - sb_org.1,
                    );
                // eff-M9 per-SB TXS gate (FTR_COUPLE_VLPD0_TXS_PER_SB): C
                // only turns the VLPD0 txs bump on for SBs the pd0 detector
                // leaves at PD0_LVL_6 (undemoted). Recompute the exact same
                // decision the tree build used (compute_b64_variance +
                // pd0_detector_allintra_demotes at the CLI qp) so demoted
                // PD0_LVL_5 SBs keep TXS off. Ignored unless the funnel
                // config sets `txs_lvl6_gate` (eff-M9 only).
                //
                // bd10: C forces `pd0_ctrls.pd0_level = PD0_LVL_0`
                // (`set_pd0_ctrls`, enc_mode_config.c:5416) at every preset,
                // so the coupling's `pd0_level == PD0_LVL_6` predicate
                // (enc_mode_config.c:8116) is FALSE for every SB — the txs
                // bump never fires, TXS stays off (tx_depth 0). Mirror that by
                // forcing `sb_is_lvl6 = false` at bd10; the pd0 detector is
                // irrelevant there (the SB is at LVL_0, the LVL_0 partition
                // path). bd8 unchanged.
                let sb_is_lvl6 = fx.frame.bit_depth != 10
                    && !crate::pd0::pd0_detector_allintra_demotes(sb_vars, fx.frame.cli_qp);
                // Task #95 chunk 2: a PD0-leaf node that is a SINGLE-EDGE
                // (one-false) block is coded as PARTITION_HORZ (`!has_rows`)
                // or PARTITION_VERT (`!has_cols`) — its single in-frame block
                // (`size x size/2` for HORZ, `size/2 x size` for VERT), the
                // other half being off-frame. `set_blocks_to_test` injects
                // exactly this shape at the allintra fixed-tree presets
                // (md_disallow_nsq_search). Byte-neutral on 64-aligned frames.
                let half = size / 2;
                let has_rows = abs_y + half < aligned_h;
                let has_cols = abs_x + half < aligned_w;
                if !has_rows || !has_cols {
                    let (bw, bh, ptype) = if !has_rows {
                        (size, half, PartitionType::Horz)
                    } else {
                        (half, size, PartitionType::Vert)
                    };
                    let choice = crate::leaf_funnel::decide_leaf_rect(
                        fx,
                        src,
                        src_stride,
                        src_off,
                        recon,
                        recon_stride,
                        abs_x,
                        abs_y,
                        bw,
                        bh,
                        dc_only,
                        sb_is_lvl6,
                    );
                    let decision = funnel_block_decision(choice, bw, bh);
                    let tree = PartitionTree::Split {
                        partition_type: ptype,
                        width: size as u16,
                        height: size as u16,
                        children: alloc::vec![PartitionTree::Leaf(decision)],
                    };
                    return PartitionResult {
                        partition_type: ptype,
                        rd_cost: 0,
                        distortion: 0,
                        rate: 0,
                        // The tree is the only copy — see `PartitionResult::decisions`.
                        decisions: alloc::vec::Vec::new(),
                        tree: Some(tree),
                        num_blocks: 1,
                    };
                }
                let choice = crate::leaf_funnel::decide_leaf(
                    fx,
                    src,
                    src_stride,
                    src_off,
                    recon,
                    recon_stride,
                    abs_x,
                    abs_y,
                    size,
                    dc_only,
                    sb_is_lvl6,
                );
                let decision = funnel_block_decision(choice, size, size);
                let tree = PartitionTree::Leaf(decision);
                return PartitionResult {
                    partition_type: PartitionType::None,
                    rd_cost: 0,
                    distortion: 0,
                    rate: 0,
                    // The tree is the only copy — see `PartitionResult::decisions`.
                    decisions: alloc::vec::Vec::new(),
                    tree: Some(tree),
                    num_blocks: 1,
                };
            }
            // C-exact leaf intra candidate set: at allintra effective-M9
            // the PD1 pass (REGULAR PD1 with the allintra signals —
            // enc_mode_config.c:11294; intra_level 8 arms
            // prune_using_edge_info) forces {DC_PRED} whenever the
            // variance-map gate `is_dc_only_safe` (mode_decision.c:845)
            // fires for the block. The fixed tree is exactly the C
            // context for the gate: PART_N squares 8..64, 64x64 SB.
            // Allintra semantics only — an inter frame (ref_ctx present)
            // keeps the full intra set beside its inter candidates.
            let dc_only = ref_ctx.is_none()
                && crate::pd0::is_dc_only_safe(sb_vars, size, abs_x - sb_org.0, abs_y - sb_org.1);
            // The same spec-5.11.4 single-edge rule as the funnel arm above,
            // for the MONOCHROME fixed-tree path (no funnel). A PD0 leaf that
            // is a one-false node (the M6 PD0 keeps NSQ geometry on, so it
            // TESTS such a node with the rect edge-shape cost instead of
            // force-splitting it) may only be coded as PARTITION_HORZ
            // (`!has_rows`) / PARTITION_VERT (`!has_cols`) with its single
            // in-frame `size x size/2` / `size/2 x size` block — never as a
            // PARTITION_NONE square: the pack refuses that (illegal per
            // spec 5.11.4, `encode_partition_tree`'s debug_assert) and a
            // release build would emit an undecodable / garbage stream. This
            // arm is exactly what zenavif measured on every 8-aligned
            // non-64-multiple mono cell at preset 6 (96x80 -> 18 dB garbage,
            // 128x80 / 96x64 / 200x136 undecodable) while presets >= 7 (NSQ
            // geometry off -> forced SPLIT in PD0) were clean. Byte-neutral on
            // 64-aligned frames (both flags always true) and on 4:2:0 (the
            // funnel arm returns first).
            let half = size / 2;
            let has_rows = abs_y + half < aligned_h;
            let has_cols = abs_x + half < aligned_w;
            if !has_rows || !has_cols {
                let (bw, bh, ptype, tptype) = if !has_rows {
                    (
                        size,
                        half,
                        PartitionType::Horz,
                        svtav1_types::partition::PartitionType::Horz,
                    )
                } else {
                    (
                        half,
                        size,
                        PartitionType::Vert,
                        svtav1_types::partition::PartitionType::Vert,
                    )
                };
                let block = encode_with_neighbors(
                    &src[src_off..],
                    src_stride,
                    recon,
                    recon_stride,
                    bw,
                    bh,
                    qindex,
                    config,
                    abs_x,
                    abs_y,
                    ref_ctx,
                    tptype,
                    dc_only,
                );
                let children: alloc::vec::Vec<PartitionTree> = block.tree.into_iter().collect();
                return PartitionResult {
                    partition_type: ptype,
                    rd_cost: block.rd_cost,
                    distortion: block.distortion,
                    rate: block.rate,
                    num_blocks: block.num_blocks,
                    decisions: block.decisions,
                    tree: Some(PartitionTree::Split {
                        partition_type: ptype,
                        width: size as u16,
                        height: size as u16,
                        children,
                    }),
                };
            }
            encode_with_neighbors(
                &src[src_off..],
                src_stride,
                recon,
                recon_stride,
                size,
                size,
                qindex,
                config,
                abs_x,
                abs_y,
                ref_ctx,
                svtav1_types::partition::PartitionType::None,
                dc_only,
            )
        }
        crate::pd0::Pd0Tree::Split(children) => {
            let half = size / 2;
            let mut result = PartitionResult {
                partition_type: PartitionType::Split,
                rd_cost: 0,
                distortion: 0,
                rate: 0,
                num_blocks: 0,
                decisions: alloc::vec::Vec::new(),
                tree: None,
            };
            let mut child_trees = alloc::vec::Vec::with_capacity(4);
            for (i, child) in children.iter().enumerate() {
                // Off-frame quadrant (partial SB): codes nothing, exactly like
                // C `svt_aom_write_modes_sb`'s SPLIT-loop `continue`. Skipping
                // the recursion keeps the in-frame children packed in quadrant
                // order — the same order the entropy walk replays them (which
                // recomputes each quadrant's position and skips the off-frame
                // ones itself). Never taken on a 64-aligned frame.
                if matches!(child, crate::pd0::Pd0Tree::Off) {
                    continue;
                }
                let x0 = (i & 1) * half;
                let y0 = (i >> 1) * half;
                let sub = encode_fixed_tree(
                    src,
                    src_stride,
                    recon,
                    recon_stride,
                    child,
                    half,
                    qindex,
                    config,
                    abs_x + x0,
                    abs_y + y0,
                    aligned_w,
                    aligned_h,
                    sb_vars,
                    sb_org,
                    funnel.as_deref_mut(),
                    ref_ctx,
                );
                result.distortion += sub.distortion;
                result.rate += sub.rate;
                result.num_blocks += sub.num_blocks;
                result.decisions.extend(sub.decisions);
                if let Some(t) = sub.tree {
                    child_trees.push(t);
                }
            }
            result.rd_cost = result.distortion;
            result.tree = Some(PartitionTree::Split {
                partition_type: PartitionType::Split,
                width: size as u16,
                height: size as u16,
                children: child_trees,
            });
            result
        }
        // Reached only if a caller hands an off-frame quadrant directly; the
        // Split arm above already skips them, and the SB root is always
        // in-frame. Defensive: an off-frame node reconstructs/codes nothing.
        crate::pd0::Pd0Tree::Off => PartitionResult {
            partition_type: PartitionType::None,
            rd_cost: 0,
            distortion: 0,
            rate: 0,
            decisions: alloc::vec::Vec::new(),
            tree: None,
            num_blocks: 0,
        },
    }
}

use super::*;

/// Extract the leaf decision from a partition tree node.
/// Panics if the node is not a Leaf (HORZ/VERT children must always be leaves).
pub(super) fn expect_leaf(
    tree: &crate::partition::PartitionTree,
) -> &crate::partition::BlockDecision {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => d,
        crate::partition::PartitionTree::Split { .. } => {
            panic!("HORZ/VERT children must be leaf blocks, not split nodes")
        }
    }
}

/// Recursively encode a partition tree to the bitstream in AV1 spec order.
///
/// AV1 spec: for each SB, write partition_type, then:
/// - PARTITION_NONE: write partition symbol + block syntax
/// - PARTITION_SPLIT: write partition symbol, recurse into 4 children
/// - PARTITION_HORZ/VERT: write partition symbol, then block syntax for
///   each child directly (NO partition symbols for children — the decoder
///   reads them as leaf blocks without expecting a partition symbol)
///
/// Partition context is derived from tracked above/left partition arrays,
/// matching the rav1d decoder's context derivation exactly.
/// Frame-edge partition flags for a SQUARE partition node — C
/// `encode_partition_av1` (entropy_coding.c:941-943):
/// `hbs` = HALF the node width in pixels, then
/// `has_rows = (y + hbs) < aligned_height`, `has_cols = (x + hbs) < aligned_width`.
///
/// The ALIGNED frame extent is recovered from the deblock geometry, which is
/// built from those same aligned dims (`DeblockGeom::new(w, h, ..)`, ~:884) and is
/// already threaded through this whole walk — so the partition edge rules and
/// the deblock walk can never disagree about where the frame ends. Aligned dims
/// are always a multiple of 8, so `mi * 4` recovers the pixel extent exactly.
///
/// On a 64-aligned frame every node lies wholly inside the frame, so both flags
/// are always `true` and the callers below stay bit-identical to the pre-edge
/// port.
#[inline]
pub(super) fn partition_edge_flags(
    geom: &crate::deblock::DeblockGeom,
    block_x: usize,
    block_y: usize,
    node_w: usize,
) -> (bool, bool) {
    crate::frame_geom::edge_has_rows_cols(
        geom.mi_cols * 4,
        geom.mi_rows * 4,
        block_x,
        block_y,
        node_w / 2,
    )
}

#[allow(clippy::too_many_arguments)]
/// Fold the per-b64 coding-unit results of ONE superblock into the SB's
/// result (task #91, SB128).
///
/// SB64 (`units.len() == 1`, `unit_size == sb_size`): the identity — the
/// single `PartitionResult` is moved out unchanged, so every SB64 caller is
/// byte-identical by construction.
///
/// SB128: the up-to-4 b64 quadrants become the children of a
/// `PARTITION_SPLIT` node rooted at the 128 square. That is exactly what C
/// codes — `encode_partition_av1` writes one partition symbol for the 128
/// node against the 8-symbol alphabet at CDF row `bsl = 4` (ctx 16..19,
/// `svt_aom_partition_cdf_length`, entropy_coding.c:922), then
/// `svt_aom_write_modes_sb` recurses into the quadrants in Z-order. The
/// entropy walk ([`encode_partition_tree`]) already handles a 128-wide
/// `Split` node: it derives ctx/nsymbs from the node width via
/// `EntropyCtx::partition_ctx` and passes `is_128 = w == 128` to
/// `write_partition_edge`, which is what selects the H4/V4-free gathers at
/// a frame edge.
///
/// Off-frame quadrants are already absent from `units`
/// (`sb128_geom::sb_coding_units` drops them, C's `mi_row + y_idx >=
/// mi_rows` `continue`), so `children` holds only the in-frame quadrants —
/// the packed layout the walk's Split arm expects.
///
/// WHY FORCED-SPLIT IS CORRECT HERE, NOT A HEURISTIC (verified first-hand
/// against reference/svt-av1/Source, 2026-07-19 — this supersedes the port map's
/// "UNVERIFIED for textured content" caveat):
///
/// C `set_blocks_to_be_tested` (Codec/enc_dec_process.c:1483-1499) computes
/// the MD scan's largest square candidate as
///
/// ```text
/// int max_sq_size = ctx->max_block_size;
/// if (pcs->mimic_only_tx_4x4)             max_sq_size = MIN(.., 8);
/// else if (static_config.max_tx_size==32) max_sq_size = MIN(.., 32);
/// else if (pcs->slice_type == I_SLICE)    max_sq_size = MIN(.., 64);
/// ```
///
/// — so on a KEY frame the largest square ever ENTERED INTO THE SCAN is
/// 64x64, whatever the superblock size. A BLOCK_128X128 is never an MD
/// candidate on an I_SLICE, so the 128 root has no codable outcome except
/// PARTITION_SPLIT. (`ctx->max_block_size` itself is `super_block_size`
/// unconditionally at M0..M7 — `get_max_block_size_allintra`,
/// enc_mode_config.c:7055-7080, sets `base_var_th_cap = (uint16_t)~0`, so
/// the `variance <= var_th_cap` test on a `uint16_t` variance is a
/// tautology; the clamp above is what actually decides.)
///
/// SCOPE OF THAT PROOF: it covers I_SLICE frames — which is the port's
/// target (ALLINTRA single-frame KEY, docs/ACCEPTANCE-CRITERIA.md). On an
/// INTER frame `max_sq_size` is NOT clamped to 64 and a genuine 128-level
/// NONE/HORZ/VERT RD search would be required; inter is unported
/// throughout, so this path is consistent with the rest of the encoder
/// rather than a new limitation. `debug_assert`ed below.
pub(super) fn merge_sb_units(
    mut units: Vec<crate::partition::PartitionResult>,
    sb_size: usize,
    unit_size: usize,
    is_key: bool,
) -> crate::partition::PartitionResult {
    if sb_size == unit_size {
        debug_assert_eq!(units.len(), 1, "SB64 must have exactly one coding unit");
        return units.remove(0);
    }
    debug_assert_eq!((sb_size, unit_size), (128, 64));
    debug_assert!(
        is_key,
        "the forced-SPLIT 128 root is only PROVEN on an I_SLICE (C clamps the MD \
         scan's max square to 64 there, enc_dec_process.c:1497); an INTER frame \
         needs a real 128-level NONE/HORZ/VERT RD search"
    );
    let mut out = crate::partition::PartitionResult {
        partition_type: crate::partition::PartitionType::Split,
        rd_cost: 0,
        distortion: 0,
        rate: 0,
        num_blocks: 0,
        decisions: alloc::vec::Vec::new(),
        tree: None,
    };
    let mut children = alloc::vec::Vec::with_capacity(units.len());
    for u in units {
        out.distortion += u.distortion;
        out.rate += u.rate;
        out.num_blocks += u.num_blocks;
        out.decisions.extend(u.decisions);
        if let Some(t) = u.tree {
            children.push(t);
        }
    }
    out.rd_cost = out.distortion;
    out.tree = Some(crate::partition::PartitionTree::Split {
        partition_type: crate::partition::PartitionType::Split,
        width: sb_size as u16,
        height: sb_size as u16,
        children,
    });
    out
}

use crate::dbgenv::psym as psym_dbg;

pub(super) fn encode_partition_tree(
    tree: &crate::partition::PartitionTree,
    writer: &mut crate::entropy::writer::AomWriter,
    frame_ctx: &mut crate::entropy::context::FrameContext,
    coeff_fc: &mut crate::entropy::coeff_c::CoeffFc,
    base_q_idx: u8,
    ectx: &mut EntropyCtx,
    is_key: bool,
    block_x: usize,
    block_y: usize,
    chroma: &mut Option<ChromaPass<'_>>,
    geom: &mut crate::deblock::DeblockGeom,
    recon_only: bool,
) {
    match tree {
        crate::partition::PartitionTree::Leaf(decision) => {
            let w = decision.width as usize;
            let h = decision.height as usize;
            if w > 4 || h > 4 {
                let (has_rows, has_cols) = partition_edge_flags(geom, block_x, block_y, w);
                // A PARTITION_NONE leaf is only legal where the node lies wholly
                // inside the frame: at an edge the non-SPLIT outcome is VERT
                // (right edge) or HORZ (bottom edge), never NONE, and with BOTH
                // flags false the partition is forced to SPLIT. The edge-aware
                // search must therefore never hand us a NONE leaf at an edge.
                debug_assert!(
                    has_rows && has_cols,
                    "PARTITION_NONE leaf at a frame edge ({block_x},{block_y}) {w}x{h}: \
                     has_rows={has_rows} has_cols={has_cols} — illegal per spec 5.11.4"
                );
                if !recon_only {
                    let (ctx, nsymbs) = ectx.partition_ctx(block_x, block_y, w);
                    if psym_dbg() {
                        eprintln!(
                            "PSYM sim={} mi=({},{}) bsize={}x{} part=0 ctx={} hr={} hc={}",
                            writer.cdf_only as u8,
                            block_y / 4,
                            block_x / 4,
                            w,
                            h,
                            ctx,
                            has_rows as u8,
                            has_cols as u8
                        );
                    }
                    crate::entropy::context::write_partition_edge(
                        writer,
                        frame_ctx,
                        ctx,
                        0,
                        nsymbs, // 0 = PARTITION_NONE
                        w == 128,
                        has_rows,
                        has_cols,
                    );
                }
            }

            // Update partition context for PARTITION_NONE
            if !recon_only {
                ectx.update_partition_ctx(
                    block_x,
                    block_y,
                    w,
                    h,
                    crate::partition::PartitionType::None,
                );
            }

            encode_block_syntax(
                decision, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x, block_y,
                chroma, geom, recon_only,
            );
        }
        crate::partition::PartitionTree::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let w = *width as usize;
            let h = *height as usize;
            if !recon_only {
                let (ctx, nsymbs) = ectx.partition_ctx(block_x, block_y, w);
                let (has_rows, has_cols) = partition_edge_flags(geom, block_x, block_y, w);
                if psym_dbg() {
                    eprintln!(
                        "PSYM sim={} mi=({},{}) bsize={}x{} part={} ctx={} hr={} hc={}",
                        writer.cdf_only as u8,
                        block_y / 4,
                        block_x / 4,
                        w,
                        h,
                        *partition_type as u8,
                        ctx,
                        has_rows as u8,
                        has_cols as u8
                    );
                }
                crate::entropy::context::write_partition_edge(
                    writer,
                    frame_ctx,
                    ctx,
                    *partition_type as u8,
                    nsymbs,
                    w == 128,
                    has_rows,
                    has_cols,
                );
            }

            let half_w = w / 2;
            let half_h = h / 2;
            match (*partition_type, children.len()) {
                (crate::partition::PartitionType::Split, _) => {
                    // PARTITION_SPLIT: up to 4 quarter-size children in Z-order.
                    // On a partial SB the off-frame quadrants were pruned from
                    // `children` by encode_fixed_tree, so walk the 4 quadrant
                    // SLOTS, skip the off-frame ones by absolute position (C
                    // svt_aom_write_modes_sb's `mi_row+y_idx >= mi_rows ||
                    // mi_col+x_idx >= mi_cols` continue, entropy_coding.c:5498),
                    // and pull the packed in-frame children in order. A
                    // 64-aligned frame keeps all four in-frame → byte-identical.
                    // Don't update partition context here — children do it —
                    // EXCEPT the terminal 8x8 split (4x4 children write no
                    // partition bytes; the decoder sets the 8x8 cell to the
                    // SPLIT value, dav1d decode_sb BL_8X8). An 8x8 node is never
                    // a frame edge, so all four 4x4 quadrants are in-frame.
                    if half_w == 4 && !recon_only {
                        ectx.update_partition_ctx(
                            block_x,
                            block_y,
                            w,
                            h,
                            crate::partition::PartitionType::Split,
                        );
                    }
                    let aligned_w = geom.mi_cols * 4;
                    let aligned_h = geom.mi_rows * 4;
                    let mut ci = 0usize;
                    for i in 0..4usize {
                        let cx = block_x + (i & 1) * half_w;
                        let cy = block_y + (i >> 1) * half_h;
                        if cx >= aligned_w || cy >= aligned_h {
                            continue;
                        }
                        encode_partition_tree(
                            &children[ci],
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            cx,
                            cy,
                            chroma,
                            geom,
                            recon_only,
                        );
                        ci += 1;
                    }
                    debug_assert_eq!(
                        ci,
                        children.len(),
                        "packed in-frame child count must equal the in-frame quadrant count"
                    );
                }
                (crate::partition::PartitionType::Horz, _) => {
                    // PARTITION_HORZ: two children stacked vertically — OR, on
                    // a partial SB (task #95 chunk 2), a single in-frame top
                    // block (`children.len() == 1`), the bottom half being
                    // off-frame (C write_modes_sb codes block 1 only if
                    // `mi_row + hbs < mi_rows`, entropy_coding.c:5490).
                    // Update partition context for HORZ (children don't do it).
                    if !recon_only {
                        ectx.update_partition_ctx(
                            block_x,
                            block_y,
                            w,
                            h,
                            crate::partition::PartitionType::Horz,
                        );
                    }

                    // Children are leaf blocks — encode directly without
                    // partition symbols (decoder reads them as direct blocks).
                    let top = expect_leaf(&children[0]);
                    encode_block_syntax(
                        top, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x,
                        block_y, chroma, geom, recon_only,
                    );
                    if let Some(bot_tree) = children.get(1) {
                        let bot = expect_leaf(bot_tree);
                        encode_block_syntax(
                            bot,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x,
                            block_y + half_h,
                            chroma,
                            geom,
                            recon_only,
                        );
                    }
                }
                (crate::partition::PartitionType::Vert, _) => {
                    // PARTITION_VERT: two children side by side — OR a single
                    // in-frame left block on a partial SB (task #95 chunk 2),
                    // the right half being off-frame.
                    // Update partition context for VERT.
                    if !recon_only {
                        ectx.update_partition_ctx(
                            block_x,
                            block_y,
                            w,
                            h,
                            crate::partition::PartitionType::Vert,
                        );
                    }

                    let left = expect_leaf(&children[0]);
                    encode_block_syntax(
                        left, writer, frame_ctx, coeff_fc, base_q_idx, ectx, is_key, block_x,
                        block_y, chroma, geom, recon_only,
                    );
                    if let Some(right_tree) = children.get(1) {
                        let right = expect_leaf(right_tree);
                        encode_block_syntax(
                            right,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x + half_w,
                            block_y,
                            chroma,
                            geom,
                            recon_only,
                        );
                    }
                }
                (ptype, n) => {
                    // Extended partitions: children are DIRECT leaf blocks at
                    // spec-defined offsets — no partition symbols of their own.
                    let quarter_w = w / 4;
                    let quarter_h = h / 4;
                    let offsets: &[(usize, usize)] = match (ptype, n) {
                        // 2 tops (w/2 x h/2) + full-width bottom (w x h/2)
                        (crate::partition::PartitionType::HorzA, 1..=3) => {
                            &[(0, 0), (half_w, 0), (0, half_h)]
                        }
                        // full-width top + 2 bottoms
                        (crate::partition::PartitionType::HorzB, 1..=3) => {
                            &[(0, 0), (0, half_h), (half_w, half_h)]
                        }
                        // 2 lefts (w/2 x h/2) + full-height right (w/2 x h)
                        (crate::partition::PartitionType::VertA, 1..=3) => {
                            &[(0, 0), (0, half_h), (half_w, 0)]
                        }
                        // full-height left + 2 rights
                        (crate::partition::PartitionType::VertB, 1..=3) => {
                            &[(0, 0), (half_w, 0), (half_w, half_h)]
                        }
                        // FEWER THAN 4 IS LEGAL AT A FRAME BOUNDARY. A node
                        // that is not itself a boundary node (has_rows &&
                        // has_cols both true) can still STRADDLE the aligned
                        // extent, and then its H4/V4 sub-blocks at the tail
                        // start outside the frame and code nothing
                        // (`svt_aom_write_modes_sb` early return). Children are
                        // dropped from the TAIL — highest y for H4, highest x
                        // for V4 — so zipping the surviving children against
                        // the full offset list pairs them correctly; a middle
                        // child cannot be dropped while a later one survives.
                        //
                        // This panicked as `unsupported partition shape
                        // (Horz4, 3)` on a 512x481 crop of gb82-sc/graph.png at
                        // preset 2. It went unnoticed because the identity
                        // harness rejects odd dims for `crop:` ("I420 needs
                        // even dims"), so no gate could reach an odd-height
                        // real-content frame — the panic was found by the new
                        // IntraBC tier-invariance test, which builds its own
                        // planes and therefore does not go through that check.
                        (crate::partition::PartitionType::Horz4, 1..=4) => &[
                            (0, 0),
                            (0, quarter_h),
                            (0, 2 * quarter_h),
                            (0, 3 * quarter_h),
                        ],
                        (crate::partition::PartitionType::Vert4, 1..=4) => &[
                            (0, 0),
                            (quarter_w, 0),
                            (2 * quarter_w, 0),
                            (3 * quarter_w, 0),
                        ],
                        other => panic!("unsupported partition shape {other:?}"),
                    };
                    if !recon_only {
                        ectx.update_partition_ctx(block_x, block_y, w, h, ptype);
                    }
                    for (child, &(dx, dy)) in children.iter().zip(offsets) {
                        let leaf = expect_leaf(child);
                        encode_block_syntax(
                            leaf,
                            writer,
                            frame_ctx,
                            coeff_fc,
                            base_q_idx,
                            ectx,
                            is_key,
                            block_x + dx,
                            block_y + dy,
                            chroma,
                            geom,
                            recon_only,
                        );
                    }
                }
            }
        }
    }
}

/// Check the native luma re-encode envelope before mutating the tree.
/// Lossy monochrome and high-preset color use depth-zero transforms. Lossless
/// monochrome uses four raster 4x4 WHTs per 8x8 leaf, predicting each unit from
/// native reconstructed neighbors. Color lossless uses the native full-RD
/// funnel at every preset and does not need this post-pass.
///
/// Directional prediction here requires the signaled edge filter to be off;
/// palette and IntraBC need the full-RD funnel. An unsupported native-input
/// tree is rejected by the caller's source-consumption check.
// `coded_lossless` is only forwarded into the recursion today — the allow is
// on the function because the param-level form does not reach this lint.
#[allow(clippy::only_used_in_recursion)]
pub(super) fn bd10_tree_supported(
    tree: &crate::partition::PartitionTree,
    edge_filter: bool,
    // No leaf clause reads this any more: the depth-1 8x8 lossless special
    // case it used to admit is subsumed by general `tx_depth` support
    // (`bd10_reencode_leaf_txs`). Kept on the signature because the recursion
    // passes it down and because a future clause that needs it should not have
    // to re-thread it.
    #[allow(unused_variables)] coded_lossless: bool,
) -> bool {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => {
            // Filter-intra IS ported (predict_filter_intra_hbd) and directional
            // intra IS ported (dr_predict_hbd) — but the re-encode passes
            // filt_type=0, valid only when the SH edge filter is off. So a
            // directional leaf is in-envelope ONLY when !edge_filter; with
            // edge_filter on this post-pass cannot encode the leaf (filt_type
            // would need the live per-block smooth-neighbour derivation).
            let directional = matches!(d.intra_mode, 3..=8)
                || (matches!(d.intra_mode, 1 | 2) && d.angle_delta != 0);
            // Chroma re-encode (task #94): the bd10 chroma pass predicts via
            // predict_unit_hbd, which supports DC/V/H/SMOOTH/PAETH + directional
            // (edge_filter off). UV_CFL_PRED (13) is NOT a predict_unit_hbd mode
            // — it is handled separately in `bd10_reencode_chroma_node`, which
            // rebuilds the CfL prediction from the 10-bit LUMA recon the luma
            // pass just produced (`cfl_luma_subsampling_420_hbd` +
            // `cfl_predict_hbd` on a DC base). That support MUST stay in
            // lockstep with the search's `cfl_gate`: a leaf the search can pick
            // but the post-pass rejects silently drops the WHOLE FRAME out of
            // the re-encode, which is a far worse (invisible) failure than a
            // visible mode divergence.
            let uv_directional =
                matches!(d.uv_mode, 3..=8) || (matches!(d.uv_mode, 1 | 2) && d.uv_angle_delta != 0);
            let uv_ok = !uv_directional || !edge_filter;
            // A palette or IntraBC leaf is NOT re-encodable by this post-pass:
            // `bd10_reencode_node` predicts every leaf with
            // `predict_unit_hbd(d.intra_mode, d.angle_delta, d.filter_intra)`
            // and never consults `d.palette` / `d.ibc`, so it would code
            // DC-based levels under palette syntax — a decoder desync, not a
            // quality loss.
            //
            // The color post-pass runs only at preset >= 9, where `sc_detect`
            // yields palette_level = 0 and intrabc_level = 0. Monochrome's
            // legacy search does not inject these tools. Historically palette
            // was also gated out of the bd10 funnel; that gate is gone now.
            // Enforce
            // it structurally instead: a leaf the post-pass cannot code drops
            // the frame back to the u8 output, which is the same
            // fall-back-don't-miscode contract every other clause here has.
            let paletted = d.palette.is_some() || d.use_intrabc;
            // `tx_depth > 0` IS supported now (`bd10_reencode_leaf_txs`), and
            // it had to be: the transform-size search is on at preset 6, so a
            // single depth-1 leaf in one superblock dropped a whole 10-bit
            // VIDEO frame out of the re-encode.
            // AN INTER LEAF codes no intra `y_mode` and no `uv_mode` at all
            // (docs/INTER-ENCODE-PLAN.md §1x defects 2 and 6), so the two
            // directional clauses above are about a mode it does not carry.
            // What it needs instead is a motion mode the post-pass can rebuild:
            // `predict_inter_leaf_hbd` handles SimpleTranslation and
            // WarpedCausal, and OBMC is never injected.
            //
            // `edge_filter` NO LONGER REJECTS A DIRECTIONAL LEAF. The post-pass
            // now derives `get_filt_type` per block from a neighbour mode grid
            // (`Bd10ModeNeighbors`) instead of passing 0, so the condition the
            // two clauses were guarding is gone. `directional` / `uv_directional`
            // are kept as bindings because the ARGUMENT is what the next reader
            // needs: it was never that directional prediction is unported, only
            // that its `filt_type` input was faked.
            let _ = (directional, uv_ok);
            #[cfg(feature = "std")]
            if crate::dbgenv::bd10_postpass() {
                let ok = !paletted;
                if !ok {
                    eprintln!(
                        "BD10_TREE reject {}x{} tx_depth={} palette={} ibc={} inter={}",
                        d.width,
                        d.height,
                        d.tx_depth,
                        d.palette.is_some(),
                        d.use_intrabc,
                        d.inter.is_some()
                    );
                }
            }
            // Every committed inter leaf is in-envelope here: a SUB-8 leaf's
            // chroma covers the parent 8x8 and C stitches it from the covered
            // cells' own MVs (`inter_chroma_4xn_pred`), which the post-pass
            // rebuilds via `stamp_inter_mi_grid` +
            // `predict_inter_chroma_sub8_hbd`; OBMC reads the same grid
            // through `obmc_nb_spans` and the blend is applied in place over
            // the leaf's own base translation (`apply_obmc_hbd_postpass`).
            !paletted
        }
        crate::partition::PartitionTree::Split { children, .. } => children
            .iter()
            .all(|c| bd10_tree_supported(c, edge_filter, coded_lossless)),
    }
}

/// Coefficient neighbors for the native level pass. Keep the bytes produced
/// by the 10-bit quantizer, never the provisional 8-bit mode-search levels.
/// SB raster order preserves in-tile neighbors; each tile boundary clears only
/// the entering SB span so adjacent tiles cannot contribute coefficient signs.
pub(super) struct Bd10CoeffNeighbors {
    pub(super) above: Vec<u8>,
    pub(super) left: Vec<u8>,
}

impl Bd10CoeffNeighbors {
    pub(super) fn new(w: usize, h: usize) -> crate::EncodeResult<Self> {
        Ok(Self {
            above: svtav1_types::try_vec![0u8; w.div_ceil(4)]?,
            left: svtav1_types::try_vec![0u8; h.div_ceil(4)]?,
        })
    }

    pub(super) fn enter_sb(
        &mut self,
        x: usize,
        y: usize,
        size: usize,
        tile: crate::intra_edge::TileMi,
    ) {
        let (mx, my) = (x / 4, y / 4);
        if my == tile.mi_row_start {
            let end = (mx + size / 4).min(self.above.len());
            self.above[mx..end].fill(0);
        }
        if mx == tile.mi_col_start {
            let end = (my + size / 4).min(self.left.len());
            self.left[my..end].fill(0);
        }
    }

    /// `(txb_skip_ctx, dc_sign_ctx)` for the TU at (x,y,w,h) inside a leaf of
    /// `leaf_w`x`leaf_h` pixels. `leaf == tu` is C's `plane_bsize ==
    /// txsize_to_bsize[tx_size]` (entropy_coding.c:284): a tx-split leaf's TU
    /// does NOT span the block, so `txb_skip_ctx` comes from the
    /// `skip_contexts` table, not the 0 shortcut. `plane` selects C's
    /// `plane != 0` arm (:310-314): `ctx_base + (is_chroma_larger ? 10 : 7)`
    /// where `is_chroma_larger` is `num_pels_log2(plane_bsize) >
    /// num_pels_log2(tx_bsize)` — leaf pels > TU pels. Chroma callers index
    /// this grid in CHROMA coordinates (x,y,w,h in chroma px).
    pub(super) fn contexts(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        leaf_w: usize,
        leaf_h: usize,
        plane: usize,
    ) -> (usize, usize) {
        let (mx, my) = (x / 4, y / 4);
        crate::entropy::coeff_c::get_txb_ctx(
            plane,
            &self.above[mx..(mx + w / 4).min(self.above.len())],
            &self.left[my..(my + h / 4).min(self.left.len())],
            w == leaf_w && h == leaf_h,
            leaf_w * leaf_h > w * h,
        )
    }

    pub(super) fn record(&mut self, x: usize, y: usize, w: usize, h: usize, cul: u8) {
        let (mx, my) = (x / 4, y / 4);
        let right = (mx + w / 4).min(self.above.len());
        let bottom = (my + h / 4).min(self.left.len());
        self.above[mx..right].fill(cul);
        self.left[my..bottom].fill(cul);
    }
}

impl EntropyCtx {
    /// Overwrite `self` with `src`, REUSING every vector's allocation.
    ///
    /// NOT `*self = src.clone()`, and not `Clone::clone_from` either:
    /// `#[derive(Clone)]` does NOT override `clone_from`, so the trait's
    /// default body is literally `*self = source.clone()` — which allocates all
    /// twenty-three of this struct's vectors and frees the twenty-three it
    /// replaces. MEASURED: the NSQ walk's snapshot restore was 67,872
    /// allocating calls on the canonical alloc cell, and switching the call
    /// from `= clone()` to `clone_from` moved NOTHING, which is what exposed
    /// the derive's behaviour. `Vec::clone_from` DOES reuse its destination,
    /// so doing it field by field is the version that pays.
    pub(crate) fn restore_from(&mut self, src: &Self) {
        self.above_mode.clone_from(&src.above_mode);
        self.left_mode.clone_from(&src.left_mode);
        self.above_uv_mode.clone_from(&src.above_uv_mode);
        self.left_uv_mode.clone_from(&src.left_uv_mode);
        self.above_skip.clone_from(&src.above_skip);
        self.left_skip.clone_from(&src.left_skip);
        self.above_partition.clone_from(&src.above_partition);
        self.left_partition.clone_from(&src.left_partition);
        self.above_coeff.clone_from(&src.above_coeff);
        self.left_coeff.clone_from(&src.left_coeff);
        self.above_txfm.clone_from(&src.above_txfm);
        self.above_inter_bw.clone_from(&src.above_inter_bw);
        self.left_txfm.clone_from(&src.left_txfm);
        self.left_inter_bh.clone_from(&src.left_inter_bh);
        self.above_palette.clone_from(&src.above_palette);
        self.left_palette.clone_from(&src.left_palette);
        self.above_palette_colors
            .clone_from(&src.above_palette_colors);
        self.left_palette_colors
            .clone_from(&src.left_palette_colors);
        self.above_nmi.clone_from(&src.above_nmi);
        self.left_nmi.clone_from(&src.left_nmi);
        self.mvp_grid.clone_from(&src.mvp_grid);
        self.above_coeff_uv[0].clone_from(&src.above_coeff_uv[0]);
        self.above_coeff_uv[1].clone_from(&src.above_coeff_uv[1]);
        self.left_coeff_uv[0].clone_from(&src.left_coeff_uv[0]);
        self.left_coeff_uv[1].clone_from(&src.left_coeff_uv[1]);
        self.seq_filter_intra = src.seq_filter_intra;
        self.tx_mode_select = src.tx_mode_select;
        self.allow_sct = src.allow_sct;
        self.allow_intrabc = src.allow_intrabc;
        self.aligned_w_px = src.aligned_w_px;
        self.aligned_h_px = src.aligned_h_px;
        self.bit_depth = src.bit_depth;
        self.cdef_sb = src.cdef_sb;
        self.tile_top_px = src.tile_top_px;
        self.tile_left_px = src.tile_left_px;
        self.tile_mi = src.tile_mi;
    }
}

/// The bd10 re-encode's LUMA/CHROMA neighbour MODE grid — the twin of
/// [`Bd10CoeffNeighbors`], for `get_filt_type`.
///
/// The post-pass used to pass `filt_type = 0` unconditionally, which is only
/// correct when the sequence header's intra edge filter is OFF; the frame gate
/// therefore rejected any directional leaf whenever it was on, and that
/// rejection drops the WHOLE FRAME out of the re-encode. On a VIDEO frame the
/// edge filter is on at preset 6, so one directional intra leaf in one
/// superblock was enough to make a 10-bit inter frame unencodable.
///
/// C `get_filt_type(xd, plane > 0)` (intra_prediction.c) is
/// `is_smooth(above) || is_smooth(left)` over the 4x4 neighbour grid, with the
/// tile edges reading as unavailable — the same derivation `EntropyCtx::
/// filt_type_y` / `filt_type_uv` already do for the real encode. This
/// reproduces it from the committed decisions the post-pass is walking.
pub(crate) struct Bd10ModeNeighbors {
    pub(super) above: Vec<u8>,
    pub(super) left: Vec<u8>,
    pub(super) above_uv: Vec<u8>,
    pub(super) left_uv: Vec<u8>,
    pub(super) tile_top_px: usize,
    pub(super) tile_left_px: usize,
}

impl Bd10ModeNeighbors {
    pub(super) fn new(w: usize, h: usize) -> crate::EncodeResult<Self> {
        Ok(Self {
            above: svtav1_types::try_vec![0u8; w.div_ceil(4)]?,
            left: svtav1_types::try_vec![0u8; h.div_ceil(4)]?,
            above_uv: svtav1_types::try_vec![0u8; w.div_ceil(4)]?,
            left_uv: svtav1_types::try_vec![0u8; h.div_ceil(4)]?,
            tile_top_px: 0,
            tile_left_px: 0,
        })
    }

    /// Same contract as [`Bd10CoeffNeighbors::enter_sb`]: a tile's own first
    /// SB row / column clears the span it is entering, so no neighbour is read
    /// across a tile edge a decoder cannot see.
    pub(super) fn enter_sb(
        &mut self,
        x: usize,
        y: usize,
        size: usize,
        tile: crate::intra_edge::TileMi,
    ) {
        self.tile_top_px = tile.mi_row_start * 4;
        self.tile_left_px = tile.mi_col_start * 4;
        let (mx, my) = (x / 4, y / 4);
        if my == tile.mi_row_start {
            let end = (mx + size / 4).min(self.above.len());
            self.above[mx..end].fill(0);
            self.above_uv[mx..end].fill(0);
        }
        if mx == tile.mi_col_start {
            let end = (my + size / 4).min(self.left.len());
            self.left[my..end].fill(0);
            self.left_uv[my..end].fill(0);
        }
    }

    /// C `get_filt_type(xd, 0)` — `EntropyCtx::filt_type_y`'s derivation.
    pub(super) fn filt_type_y(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let ab = y > self.tile_top_px && smooth(self.above[(x / 4).min(self.above.len() - 1)]);
        let le = x > self.tile_left_px && smooth(self.left[(y / 4).min(self.left.len() - 1)]);
        i32::from(ab || le)
    }

    /// C `get_filt_type(xd, 1)` — `EntropyCtx::filt_type_uv`'s derivation,
    /// including its 8x8-group rounding and bottom-right-owner selection.
    pub(super) fn filt_type_uv(&self, x: usize, y: usize) -> i32 {
        let smooth = |m: u8| matches!(m, 9..=11);
        let ai = ((x / 4) | 1).min(self.above_uv.len() - 1);
        let li = ((y / 4) | 1).min(self.left_uv.len() - 1);
        let ab = (y & !7) > self.tile_top_px && smooth(self.above_uv[ai]);
        let le = (x & !7) > self.tile_left_px && smooth(self.left_uv[li]);
        i32::from(ab || le)
    }

    /// Stamp a committed leaf. An INTER leaf codes no intra `y_mode` and no
    /// `uv_mode`, and C's `svt_aom_is_smooth` answers false for both on one —
    /// so a non-smooth sentinel is the faithful stamp, not a lie about DC.
    pub(super) fn record(&mut self, x: usize, y: usize, w: usize, h: usize, mode: u8, uv_mode: u8) {
        let (mx, my) = (x / 4, y / 4);
        let right = (mx + w / 4).min(self.above.len());
        let bottom = (my + h / 4).min(self.left.len());
        self.above[mx..right].fill(mode);
        self.left[my..bottom].fill(mode);
        self.above_uv[mx..right].fill(uv_mode);
        self.left_uv[my..bottom].fill(uv_mode);
    }
}

/// Recursive leaf printer for `SVTAV1_DUMP_TREE` (coding order).
#[cfg(feature = "std")]
pub(super) fn dump_tree_leaves(tree: &crate::partition::PartitionTree, x: usize, y: usize) {
    match tree {
        crate::partition::PartitionTree::Leaf(d) => {
            if let Some(i) = d.inter.as_deref() {
                eprintln!(
                    "LEAF x{:4} y{:4} {}x{} mode {:?} uv {:2} tx {} eob {} txd {} rf=[{},{}] mv=[{},{}|{},{}] drl={}",
                    x,
                    y,
                    d.width,
                    d.height,
                    i.mode,
                    d.uv_mode,
                    d.tx_type,
                    d.eob,
                    d.tx_depth,
                    i.ref_frame[0],
                    i.ref_frame[1],
                    i.mv[0].y,
                    i.mv[0].x,
                    i.mv[1].y,
                    i.mv[1].x,
                    i.drl_index,
                );
            } else {
                eprintln!(
                    "LEAF x{:4} y{:4} {}x{} mode {:2} uv {:2} tx {} eob {} txd {}",
                    x, y, d.width, d.height, d.intra_mode, d.uv_mode, d.tx_type, d.eob, d.tx_depth
                );
            }
        }
        crate::partition::PartitionTree::Split {
            partition_type,
            width,
            height,
            children,
        } => {
            let (w, h) = (*width as usize, *height as usize);
            let (hw, hh, qw, qh) = (w / 2, h / 2, w / 4, h / 4);
            use crate::partition::PartitionType as P;
            let offs: alloc::vec::Vec<(usize, usize)> = match partition_type {
                P::Split => alloc::vec![(0, 0), (hw, 0), (0, hh), (hw, hh)],
                P::Horz => alloc::vec![(0, 0), (0, hh)],
                P::Vert => alloc::vec![(0, 0), (hw, 0)],
                P::HorzA => alloc::vec![(0, 0), (hw, 0), (0, hh)],
                P::HorzB => alloc::vec![(0, 0), (0, hh), (hw, hh)],
                P::VertA => alloc::vec![(0, 0), (0, hh), (hw, 0)],
                P::VertB => alloc::vec![(0, 0), (hw, 0), (hw, hh)],
                P::Horz4 => alloc::vec![(0, 0), (0, qh), (0, 2 * qh), (0, 3 * qh)],
                P::Vert4 => alloc::vec![(0, 0), (qw, 0), (2 * qw, 0), (3 * qw, 0)],
                P::None => alloc::vec![(0, 0)],
            };
            eprintln!("SPLIT x{x:4} y{y:4} {w}x{h} {partition_type:?}");
            for (child, (dx, dy)) in children.iter().zip(offs) {
                dump_tree_leaves(child, x + dx, y + dy);
            }
        }
    }
}

/// Is the bd10 FULL-RD mode funnel (MDS1 + MDS3 at true depth) usable for this
/// frame? (task #94, MODE axis — docs/bd10-port-map.md.)
///
/// Below eff-M9 the coded mode is the MDS1/MDS3 full-RD winner rather than the
/// MDS0 survivor, so a bd10 MDS0 alone closes nothing (measured). When this is
/// on, `evaluate_leaf` runs the whole full-RD chain — luma depth loop with
/// TXS/TXT, and the chroma loop — on 10-bit pixels with the bd10 quant tables
/// and `full_lambda_md[EB_10_BIT_MD]`, and the winner's 10-bit levels ARE the
/// coded ones, so the level-only re-encode post-pass is skipped.
///
/// Scope, deliberately narrow:
/// - **presets 0..=8**. p6..=8 was the MODE axis (landed first). p0..=5 take the
///   PD1 depth-refine + NSQ walk (`decide_sb_refined`), which is the **PART**
///   axis: C's PD1 runs at `hbd_md = 2`, so `test_depth` /
///   `test_split_partition` sum 10-bit MDS3 leaf costs when choosing the shape
///   and the depth. Feeding that walk 8-bit leaf costs picked C's *bd8*
///   geometry. LOCALIZED (docs/bd10-port-map.md): at p0..p2 C's PD0 pass is
///   bit-depth-IDENTICAL (bd8 `pic_pd0_lvl == 0` and bd10 forces `PD0_LVL_0`,
///   both run at `hbd_md = 0` on the MSB-truncated plane — measured
///   byte-identical `SVT_PD0COST_OUT` dumps), and the depth-refinement gates
///   also run inside C's `hbd_md = 0` window (enc_dec_process.c:2965 forces 0,
///   :3023 restores AFTER the :3017 refinement call), so the ONLY bit-depth
///   input to the geometry is the PD1 leaf cost.
///   Lossy eff-M9 (p9..p13) uses the MDS0 funnel + level post-pass.
///   Coded-lossless uses this native full-RD path at every preset, so each
///   4x4 WHT prediction consumes the preceding unit's native reconstruction.
/// - **any SB geometry, complete or partial** (2026-08-04). This used to be
///   complete-SB-only on the stated grounds that `tx_unit_hbd` is not
///   partial-SB-aware; that was MEASURED WRONG. `tx_unit_hbd` takes explicit
///   `(w, h, stride, off)` and its only geometry-sensitive term is
///   `TxRdArgs::crop`, which is the bd10 TWIN of the u8 cropped-TX distortion
///   and is already fed the same `blk_crop`/`uv_crop` (leaf_funnel.rs). The
///   real partial-SB machinery — the PD0 edge predicates, the edge-aware PD1
///   depth-refinement walk, the one-false shape injection, the SB-extent recon
///   canvases and `commit_leaf`'s straddle clip — is SHARED with the u8 path,
///   which is 36/36 at partial SB. So the bd10 full-RD funnel inherits it.
/// - Native palette and CfL candidates are evaluated inside the funnel.
pub(super) fn bd10_full_rd_supported(
    coded_lossless: bool,
    bit_depth: u8,
    preset: i8,
    chroma_420: bool,
    // C `pcs->slice_type == I_SLICE`. It is the term this gate was MISSING,
    // and the reason 10-bit VIDEO diverged while 10-bit stills did not.
    is_islice: bool,
    _w: usize,
    _h: usize,
) -> bool {
    // `chroma_420` is load-bearing, not decoration: the funnel this gate
    // enables is only ever constructed when `use_funnel` holds, and that
    // requires 4:2:0. Without this term the gate returned TRUE for a
    // monochrome bd10 frame at preset <= 8 — which then suppressed the level
    // post-pass (`bd10_postpass_runs = !bd10_full_rd`) while the funnel it
    // claimed to be deferring to never ran, leaving the whole encode in the
    // 8-bit domain under a 10-bit sequence header.
    //
    // The former `FunnelCfg::for_preset(preset).palette_level == 0` term is
    // gone: `for_preset` returns the constant 0 (leaf_funnel.rs), the real
    // per-frame level being stamped later from `sc_detect`, so the clause was
    // a compile-time tautology that read like a screen-content precondition.
    // Palette at bd10 is now handled inside the funnel (see
    // `search_palette_luma_hbd`), so no such precondition is needed.
    //
    // THE FRAME-TYPE TERM. C derives `pcs->hbd_md` in
    // `svt_aom_sig_deriv_multi_processes_default` (enc_mode_config.c:2151-2164):
    //
    //     enc_mode <= ENC_MR  ->  1
    //     enc_mode <= ENC_M5  ->  is_base ? 2 : 0
    //     else                ->  is_islice ? 2 : 0
    //
    // so at preset 6 and above C's mode decision is **8-BIT on every non-I
    // frame**. This gate had no frame-type term at all, so a 10-bit INTER
    // frame ran the port's 10-bit full-RD funnel where C runs 8-bit MD — which
    // is why bd10 STILLS on real content are 16/16 byte-identical while every
    // one of the 24 bd10 VIDEO cells diverged
    // (benchmarks/bd10_video_2026-09-10.meta).
    //
    // `is_base` does not appear because it is ALWAYS TRUE on this port: a
    // hierarchical GOP is refused (`gop_config_error`), so every picture is
    // temporal layer 0. When that refusal lifts, this line needs the real
    // `is_base` for the `<= ENC_M5` arm.
    //
    // The `preset <= 8` bound is the PORT's, not C's: C's `hbd_md` is non-zero
    // for an I-slice at every preset, and the port serves presets >= 9 with the
    // MDS0 funnel plus the level re-encode post-pass instead of full RD.
    // `is_base` does not appear because it is ALWAYS TRUE on this port: a
    // hierarchical GOP is refused (`gop_config_error`), so every picture is
    // temporal layer 0. When that refusal lifts, the `<= ENC_M5` arm needs the
    // real `is_base`.
    let hbd_md_nonzero = preset <= 5 || is_islice;
    bit_depth == 10 && (preset <= 8 || coded_lossless) && chroma_420 && hbd_md_nonzero
}

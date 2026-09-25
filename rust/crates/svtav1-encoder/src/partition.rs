//! Partition search — recursive block splitting for optimal RD.
//!
//! Spec 10 (encoding-loop.md): Recursive partition search.
//!
//! AV1 uses a quadtree+extended partition structure starting from 64x64
//! (or 128x128) superblocks, recursively splitting into smaller blocks.
//! Each split decision compares RD cost of encoding at current size vs
//! splitting. (Spec 10: "partition search evaluates NONE, SPLIT, HORZ,
//! VERT, and extended partition types")
//!
//! All 10 AV1 partition types supported:
//! NONE, HORZ, VERT, SPLIT, HORZ_A, HORZ_B, VERT_A, VERT_B, HORZ_4, VERT_4
//! (Spec 16: PartitionType enum, definitions.h:858-872)

/// Minimum block size for partition search (4x4 per AV1 spec).
pub const MIN_BLOCK_SIZE: usize = 4;

/// Configuration for partition search, derived from SpeedConfig.
/// Controls which tools are enabled during mode decision within
/// the partition search loop.
#[derive(Debug, Clone)]
pub struct PartitionSearchConfig {
    /// Maximum number of intra candidates to evaluate.
    /// (Spec 03: NIC = Number of Intra Candidates per MDS stage)
    pub max_intra_candidates: usize,
    /// Whether to try directional intra modes (D45..D203).
    /// (Spec 05: "directional modes are between V_PRED and D67_PRED")
    pub enable_directional: bool,
    /// Whether to try T-shape partitions (HORZ_A/B, VERT_A/B).
    /// (Spec 10: "extended partition types for improved RD at boundaries")
    pub enable_ext_partitions: bool,
    /// Whether to try 4:1 partitions (HORZ_4, VERT_4).
    pub enable_4to1_partitions: bool,
    /// Whether to enable ADST transform types in RDO.
    /// (Spec 04: "ADST captures asymmetric energy from directional prediction")
    pub enable_adst: bool,
    /// Whether to use RDO for transform type selection (try multiple TX types).
    /// When false, always uses DCT-DCT.
    pub rdo_tx_decision: bool,
    /// Whether to try filter-intra prediction modes.
    /// (Spec 05: "filter-intra for blocks <= 32x32")
    pub enable_filter_intra: bool,
    /// Minimum luma block dimension the partition search may produce.
    /// 4 = full AV1 partition ladder (mono default). 8 = 4:2:0 policy:
    /// every coded block keeps min(width, height) >= 8, so every luma block
    /// is a chroma reference with chroma dims exactly (w/2, h/2) >= 4 —
    /// AV1's sub-8x8 is_chroma_ref / last-block chroma rules are deferred.
    pub min_block_dim: usize,
    /// Frame-level C-exact coding quantizer (still path, presets >= 9):
    /// when set, every luma quantization in this search runs C's
    /// MDS3/still quantize path (`quant.rs`) instead of the legacy
    /// dead-zone quantizer. None everywhere else.
    pub c_quant: Option<alloc::sync::Arc<crate::quant::CodingQuantCfg>>,
    /// Task #86: the Y-origin (luma pixel domain) of the CURRENT TILE's
    /// own top row. `extract_neighbors` (via [`encode_with_neighbors`])
    /// uses this instead of the frame's absolute y=0 to decide "above"
    /// availability, matching AV1's per-tile prediction independence
    /// (spec: intra prediction never crosses a tile boundary). 0 = single
    /// tile row (unchanged pre-#86 behavior — the frame's top row IS the
    /// only tile's top row).
    pub tile_top_px: usize,
    /// Task #96: the X-origin (luma pixel domain) of the CURRENT TILE's
    /// own left column — the column analogue of [`Self::tile_top_px`].
    /// 0 = single tile column (unchanged pre-#96 behavior).
    pub tile_left_px: usize,
    /// C `seq_header.sb_mi_size` — superblock size in MI (4px) units, 16 at
    /// SB64 and 32 at SB128 (task #91). Feeds the intra availability tables
    /// (`intra_edge::has_top_right` / `has_bottom_left`), which index blocks
    /// by `mi & (sb_mi_size - 1)`. Defaults to 16 so every pre-SB128 caller
    /// is byte-identical by construction; `pipeline` overrides it from the
    /// derived SB size.
    pub sb_mi_size: usize,
    /// The ALIGNED luma frame extent (C `pcs->ppcs->aligned_width` /
    /// `aligned_height`). Reference-sample reads clamp to it — see
    /// [`extract_neighbors_tiled`]'s `plane_w`/`plane_h`. `usize::MAX` means
    /// "no clamp", which is what every 64-aligned caller effectively gets;
    /// the pipeline sets the real extent so a partial superblock is correct.
    pub aligned_w: usize,
    pub aligned_h: usize,
}

impl PartitionSearchConfig {
    /// Create from a SpeedConfig.
    pub fn from_speed_config(sc: &crate::speed_config::SpeedConfig) -> Self {
        Self {
            max_intra_candidates: sc.max_intra_candidates as usize,
            enable_directional: sc.enable_directional_modes,
            enable_ext_partitions: sc.preset <= 8,
            enable_4to1_partitions: sc.preset <= 6,
            enable_adst: sc.enable_adst,
            rdo_tx_decision: sc.rdo_tx_decision,
            enable_filter_intra: sc.enable_filter_intra,
            min_block_dim: MIN_BLOCK_SIZE,
            c_quant: None,
            tile_top_px: 0,
            tile_left_px: 0,
            sb_mi_size: 16,
            aligned_w: usize::MAX,
            aligned_h: usize::MAX,
        }
    }

    /// Default config (all features enabled).
    pub fn full() -> Self {
        Self {
            max_intra_candidates: 13,
            enable_directional: true,
            enable_ext_partitions: true,
            enable_4to1_partitions: true,
            enable_adst: true,
            rdo_tx_decision: true,
            enable_filter_intra: true,
            min_block_dim: MIN_BLOCK_SIZE,
            c_quant: None,
            tile_top_px: 0,
            tile_left_px: 0,
            sb_mi_size: 16,
            aligned_w: usize::MAX,
            aligned_h: usize::MAX,
        }
    }
}

/// Reference frame context for inter prediction within partition search.
///
/// When provided, `encode_single_block` tries inter prediction in addition
/// to intra modes, comparing RD cost to pick the winner.
#[derive(Clone, Copy)]
pub struct RefFrameCtx<'a> {
    /// Reference Y plane pixels.
    pub y_plane: &'a [u8],
    /// Reference stride.
    pub stride: usize,
    /// Reference picture width.
    pub pic_width: usize,
    /// Reference picture height.
    pub pic_height: usize,
    /// Frame-level MV map for spatial MV prediction (8x8 block grid).
    /// Index: (block_y / 8) * mv_map_stride + (block_x / 8).
    /// When None, searches around Mv::ZERO.
    pub mv_map: Option<&'a [svtav1_types::motion::Mv]>,
    /// Stride of the MV map (= frame_width / 8).
    pub mv_map_stride: usize,
    /// The SAME reference luma plane with C's replicated margin
    /// (`picture::PaddedPlane`, `docs/INTER-ENCODE-PLAN.md` §1s item 4).
    ///
    /// `y_plane` above is the bare recon at frame stride, which every
    /// non-MC reader indexes; this is the form INTER PREDICTION needs,
    /// because a legal MV puts the predicted block partly OUTSIDE the frame
    /// and the samples there are the replicated edge, not a constant.
    pub y_padded: Option<&'a crate::picture::PaddedPlane>,
    /// The reference's padded CHROMA planes (u, v) at the frame's chroma
    /// resolution — full-resolution at 4:4:4. `None` on monochrome and on
    /// every encode whose chroma never became a reference.
    pub uv_padded: Option<&'a (crate::picture::PaddedPlane, crate::picture::PaddedPlane)>,
    /// The frame's chroma subsampling (`ChromaFormat::subsampling_x/y`) —
    /// 1/1 at 4:2:0, 0/0 at 4:4:4. The chroma MC geometry derives from it.
    pub ss_x: usize,
    pub ss_y: usize,
    /// `scs->super_block_size` — read by `compute_subpel_params`'s MV clamp
    /// (`clamp_mv_to_umv_border_sb`), which bounds the MV against the SB, not
    /// the block.
    pub sb_size: usize,
}

/// The frame-constant half of the inter MVP environment — everything
/// `inter_mvp::setup_ref_mv_list` reads that is NOT the mode-info grid.
///
/// The grid itself lives on the walk that stamps it
/// (`EntropyCtx::mvp_grid`), because C stamps its mi map **mid-walk** from
/// inside `svt_aom_update_mi_map` and the values coded into the bitstream
/// must come from the COMMITTED map — the one a decoder rebuilds — not from
/// a search-time copy.
#[derive(Clone)]
pub struct InterMdEnv {
    pub mi_stride: i32,
    pub mi_rows: i32,
    pub mi_cols: i32,
    pub tile: crate::intrabc::TileMiBounds,
    /// `mi_size_wide[seq_header.sb_size]` (16 at SB64, 32 at SB128).
    pub sb_mi_size: i32,
    /// C `pcs->ppcs->global_motion` — the models
    /// `crate::port_global_me::set_global_motion_field` published for this
    /// frame, the same array the frame header codes.
    pub global_motion: [svtav1_types::motion::WarpedMotionParams; 8],
    pub allow_high_precision_mv: bool,
    pub force_integer_mv: bool,
    pub use_ref_frame_mvs: bool,
    pub order_hint_info: crate::inter_mvp::OrderHintInfo,
    pub cur_order_hint: i32,
    pub ref_order_hint: [i32; 8],
    /// C `pcs->av1_cm->ref_frame_sign_bias[8]` —
    /// `port_picstruct::set_ref_frame_sign_bias` (`pd_process.c:4894`). 1
    /// marks a BACKWARD reference, which is how `scan_blk_mbmi` knows to
    /// sign-flip a neighbour candidate whose ref is on the opposite
    /// temporal side. All-zero under low delay; live under random access.
    pub ref_frame_sign_bias: [i32; 8],
    /// C's picture-level `symteric_refs` preconditions —
    /// `pcs->temporal_layer_index > 0 && pred_structure == RANDOM_ACCESS`
    /// (adaptive_mv_pred.c:1339-1341). The list half of C's gate is
    /// evaluated per call inside `inter_mvp::generate_av1_mvp_table` on
    /// the block's own `ref_frame_type_arr` — `determine_best_references`
    /// may reorder it away from {LAST, BWDREF, LAST_BWD} — so a
    /// picture-level stamp of the whole gate is wrong. Stamped by the
    /// pipeline; `inter_mvp_fields` reads the same eligibility so the
    /// coded stack matches MD's.
    pub symmetric_refs_eligible: bool,
    /// C `pcs->tpl_mvs` — every cell `INVALID_MV` (the state
    /// `av1_setup_motion_field`'s reset leaves) until the temporal MV field
    /// is wired. Allocated rather than empty because `add_tpl_ref_mv`
    /// indexes it unconditionally.
    pub tpl_mvs: alloc::vec::Vec<crate::inter_mvp::TplMvRef>,
    pub tpl_stride: i32,
    /// The PICTURE-level half of C `ctx->sb64_sq_no4xn_geom`
    /// (`product_coding_loop.c:10256`): `scs->super_block_size == 64`. The
    /// block-shape half is stamped per block by
    /// [`crate::inter_mvp::InterMvpEnv::for_block`], because C's is a per-block
    /// field and a picture-level approximation of it is wrong on any picture
    /// that mixes block shapes.
    pub sb_size_64: bool,
}

impl InterMdEnv {
    /// The borrowed view `inter_mvp` takes.
    pub fn mvp_env(&self) -> crate::inter_mvp::InterMvpEnv<'_> {
        crate::inter_mvp::InterMvpEnv {
            global_motion: &self.global_motion,
            // C derives this in `svt_av1_setup_frame_sign_bias` from the
            // order hints — a backward reference flips the sign of a
            // spatial candidate's MV before it enters the stack. Computed
            // once per picture by `port_picstruct::set_ref_frame_sign_bias`
            // and carried here; all-zero under low delay by that same
            // derivation.
            ref_frame_sign_bias: self.ref_frame_sign_bias.map(|v| v as u32),
            allow_high_precision_mv: self.allow_high_precision_mv,
            force_integer_mv: self.force_integer_mv,
            use_ref_frame_mvs: self.use_ref_frame_mvs,
            order_hint_info: self.order_hint_info,
            cur_order_hint: self.cur_order_hint,
            ref_order_hint: self.ref_order_hint,
            tpl_mvs: &self.tpl_mvs,
            tpl_stride: self.tpl_stride,
            // Deliberately FALSE here: every consumer must stamp it for the
            // block it is about, with `InterMvpEnv::for_block`. Carrying the
            // picture-level bool through would look like an answer.
            sb64_sq_no4xn_geom: false,
            symmetric_refs_eligible: self.symmetric_refs_eligible,
            // The per-call gate result — `generate_av1_mvp_table` stamps it
            // on its own env copy from `symmetric_refs_eligible` and the
            // ref list it is driven with; callers that need the shortcut
            // outside that driver (the entropy-side single-ref rebuild)
            // stamp it on their own env explicitly.
            symmetric_refs: false,
        }
    }
}

impl<'a> RefFrameCtx<'a> {
    /// Get the spatial MV predictor for a block at (abs_x, abs_y).
    /// Returns the median of above and left MVs if available.
    pub fn get_mv_predictor(&self, abs_x: usize, abs_y: usize) -> svtav1_types::motion::Mv {
        let Some(map) = self.mv_map else {
            return svtav1_types::motion::Mv::ZERO;
        };
        let bx = abs_x / 8;
        let by = abs_y / 8;
        let stride = self.mv_map_stride;
        if stride == 0 {
            return svtav1_types::motion::Mv::ZERO;
        }

        // Collect available spatial neighbors
        let mut mvs = alloc::vec::Vec::new();
        if by > 0 {
            let above = map[(by - 1) * stride + bx];
            if above != svtav1_types::motion::Mv::ZERO {
                mvs.push(above);
            }
        }
        if bx > 0 {
            let left = map[by * stride + bx - 1];
            if left != svtav1_types::motion::Mv::ZERO {
                mvs.push(left);
            }
        }
        if by > 0 && bx > 0 {
            let diag = map[(by - 1) * stride + bx - 1];
            if diag != svtav1_types::motion::Mv::ZERO {
                mvs.push(diag);
            }
        }

        match mvs.len() {
            0 => svtav1_types::motion::Mv::ZERO,
            1 => mvs[0],
            2 => svtav1_types::motion::Mv {
                x: (mvs[0].x + mvs[1].x) / 2,
                y: (mvs[0].y + mvs[1].y) / 2,
            },
            _ => {
                // Median of 3: sort and take middle
                let mut xs: [i16; 3] = [mvs[0].x, mvs[1].x, mvs[2].x];
                let mut ys: [i16; 3] = [mvs[0].y, mvs[1].y, mvs[2].y];
                xs.sort_unstable();
                ys.sort_unstable();
                svtav1_types::motion::Mv { x: xs[1], y: ys[1] }
            }
        }
    }
}

/// The longest intra reference edge this port can be asked for.
///
/// Every [`extract_neighbors_tiled`] caller passes a coding-block or TX-unit
/// dimension, and AV1's largest of either is `BLOCK_128X128` — C's
/// `MAX_SB_SIZE` (`EbSvtAv1Enc.h`), which is also the port's `sb_size` ceiling.
/// The extractor asserts the bound rather than clamping: a clamp would hand the
/// predictor fewer reference samples than it indexes, which is the silent
/// wrong-pixel failure `CLAUDE.md`'s conformance mandate forbids.
pub(crate) const MAX_EDGE_PX: usize = 128;

/// One block's intra reference edges, on the stack.
///
/// MEASURED 2026-09-05 (`benchmarks/percall_layout_2026-09-05`): the two
/// `Vec<u8>`s this replaces were **44,502 of the port's 244,967 heap blocks per
/// photo_cid 512x512 p6 encode — 18.2 % of every allocator call the encoder
/// makes — for 435,872 bytes, i.e. an average payload of 9.8 BYTES per
/// `malloc`/`free` pair.** The C encoder makes 2,627 allocator calls for the
/// whole frame and reads its reference samples straight out of the recon buffer
/// into `MacroBlockD`-owned storage. This is pure per-call overhead with almost
/// no byte payload, which is why it is a stack struct and not a scratch pool.
pub(crate) struct NeighborEdges {
    above: [u8; MAX_EDGE_PX],
    left: [u8; MAX_EDGE_PX],
    /// C `above_ref[-1]` / the unavailable-edge fills — see
    /// [`extract_neighbors_tiled`].
    pub(crate) top_left: u8,
    pub(crate) has_above: bool,
    pub(crate) has_left: bool,
    w: usize,
    h: usize,
}

impl NeighborEdges {
    /// The above row (`width` samples) and left column (`height` samples),
    /// exactly the slices the pre-2026-09-05 `(Vec<u8>, Vec<u8>, ...)` tuple
    /// carried.
    #[inline]
    pub(crate) fn parts(&self) -> (&[u8], &[u8], u8, bool, bool) {
        (
            &self.above[..self.w],
            &self.left[..self.h],
            self.top_left,
            self.has_above,
            self.has_left,
        )
    }
}

/// Single-tile-row-equivalent form of [`extract_neighbors_tiled`]
/// (`tile_top = 0`) — kept at the original signature because
/// `leaf_funnel.rs` (a separate, off-limits workstream file, task #86
/// scope) calls this exact form.
///
/// PORT-NOTE(unverified): `leaf_funnel.rs`'s own intra-edge/filter-intra
/// prediction is therefore NOT tile-row-aware yet — it inherits the same
/// "treats a tile's own top row as having a real above neighbor" gap
/// [`extract_neighbors_tiled`]'s doc describes, for any leaf the M-preset
/// funnel handles. Verify via: re-run the task #86 identity cells once the
/// funnel gets its own `tile_top` threading (or once a funnel-covering
/// preset's 2-tile-row identity cell is added) and confirm the divergence
/// moves past leaf-funnel-covered blocks.
pub(crate) fn extract_neighbors(
    recon: &[u8],
    stride: usize,
    abs_x: usize,
    abs_y: usize,
    width: usize,
    height: usize,
    plane_w: usize,
    plane_h: usize,
) -> NeighborEdges {
    extract_neighbors_tiled(
        recon, stride, abs_x, abs_y, width, height, 0, 0, plane_w, plane_h,
    )
}

impl PartitionTree {
    /// Number of leaf blocks in the tree — `collect_decisions().len()`
    /// without deep-cloning every [`BlockDecision`] (each owns up to nine
    /// `Vec`s) just to throw the clones away.
    pub fn count_leaves(&self) -> usize {
        match self {
            PartitionTree::Leaf(_) => 1,
            PartitionTree::Split { children, .. } => {
                children.iter().map(PartitionTree::count_leaves).sum()
            }
        }
    }

    /// C `pcs->sb_min_sq_size[sb]` for one superblock — the MINIMUM
    /// `blk_geom->sq_size` over every block this tree codes
    /// (`coding_loop.c:1640`, `MIN(blk_geom->sq_size, ...)` folded over the
    /// coded blocks, initialised to 128 at `enc_dec_process.c:3101`).
    ///
    /// `node_sq` is the SQUARE this node partitions, which is what
    /// `blk_geom->sq_size` means for an NSQ shape: a 64x32 block produced by
    /// `PARTITION_HORZ` of a 64x64 has `sq_size == 64`, not 32. So only
    /// `PARTITION_SPLIT` halves it; every other partition type hands its
    /// children the same square.
    ///
    /// Read by the NEXT frame: `set_depth_removal_level_controls`
    /// (`enc_mode_config.c:3173-3196`) raises two dev thresholds when the
    /// list-0 reference's value for this superblock is >= 32 or >= 64.
    #[must_use]
    pub fn min_sq_size(&self, node_sq: usize) -> usize {
        match self {
            PartitionTree::Leaf(_) => node_sq,
            PartitionTree::Split {
                partition_type,
                children,
                ..
            } => {
                let child_sq = if *partition_type == PartitionType::Split {
                    node_sq / 2
                } else {
                    node_sq
                };
                children
                    .iter()
                    .map(|c| c.min_sq_size(child_sq))
                    .min()
                    .unwrap_or(node_sq)
            }
        }
    }

    /// C `pcs->sb_max_sq_size[sb]` — the MAXIMUM `blk_geom->sq_size` over the
    /// coded blocks (`coding_loop.c:1641`, the `MAX` twin of
    /// [`Self::min_sq_size`]'s `MIN`; initialised to 0 at
    /// `enc_dec_process.c:3102`). Read by the next frame's `use_ref_info`
    /// arm in `update_pred_th_offset` (enc_dec_process.c:1614-1630).
    #[must_use]
    pub fn max_sq_size(&self, node_sq: usize) -> usize {
        match self {
            PartitionTree::Leaf(_) => node_sq,
            PartitionTree::Split {
                partition_type,
                children,
                ..
            } => {
                let child_sq = if *partition_type == PartitionType::Split {
                    node_sq / 2
                } else {
                    node_sq
                };
                children
                    .iter()
                    .map(|c| c.max_sq_size(child_sq))
                    .max()
                    .unwrap_or(node_sq)
            }
        }
    }

    /// Collect all leaf decisions in tree order (depth-first).
    pub fn collect_decisions(&self) -> alloc::vec::Vec<BlockDecision> {
        match self {
            PartitionTree::Leaf(d) => alloc::vec![d.clone()],
            PartitionTree::Split { children, .. } => {
                let mut decisions = alloc::vec::Vec::new();
                for child in children {
                    decisions.extend(child.collect_decisions());
                }
                decisions
            }
        }
    }

    /// C `pcs->sb_intra[sb]` / `pcs->sb_skip[sb]` for the superblock this
    /// tree codes, as `update_b` accumulates them (coding_loop.c:1606/1643):
    /// `sb_intra` is 1 when ANY leaf is intra (`!is_inter` — IntraBC codes
    /// DC_PRED and counts as intra, matching `add_block`), and `sb_skip`
    /// stays 1 only while EVERY leaf codes no coefficient.
    ///
    /// `sb_skip` under-approximates C's `block_has_coeff`: a leaf whose
    /// `chroma_dec` is `None` gets its UV_DC residuals only in the entropy
    /// walk, so a luma-zero leaf is counted skip here even where that later
    /// chroma has a coefficient. `pd0_detector` reads these flags for the
    /// LEFT and TOP superblocks only — already-coded SBs — so the result is
    /// exact whenever every leaf's chroma is decided (funnel paths) and a
    /// strict subset of C's non-skip set otherwise.
    pub(crate) fn sb_intra_skip(&self) -> (bool, bool) {
        match self {
            PartitionTree::Leaf(d) => {
                let intra = !d.is_inter;
                let skip = d.eob == 0
                    && d.chroma_dec
                        .as_ref()
                        .is_none_or(|(_, _, u_eob, v_eob, _, _)| *u_eob == 0 && *v_eob == 0);
                (intra, skip)
            }
            PartitionTree::Split { children, .. } => children
                .iter()
                .map(PartitionTree::sb_intra_skip)
                .fold((false, true), |(ai, ask), (i, s)| (ai || i, ask && s)),
        }
    }
}

/// Per-block encoding decision record for bitstream encoding.
#[derive(Debug, Clone)]
pub struct BlockDecision {
    /// Partition type that produced this block.
    pub partition_type: PartitionType,
    /// Whether this block uses inter prediction.
    pub is_inter: bool,
    /// IBC chunk 9: IntraBC block (KEY frame, screen content). The block
    /// codes use_intrabc=1 + the DV diff, suppresses every intra mode
    /// syntax slice, and routes tx_size through the inter var-tx writer.
    pub use_intrabc: bool,
    /// The IntraBC displacement vector (eighth-pel; whole-pel by
    /// is_dv_valid). Dead unless `use_intrabc`.
    pub dv: svtav1_types::motion::Mv,
    /// The DV predictor `svt_av1_encode_dv` diffs against
    /// (ref_mv_stack[INTRA_FRAME][0].this_mv at injection).
    pub dv_ref: svtav1_types::motion::Mv,
    /// Intra prediction mode index (0-12 for AV1 modes).
    pub intra_mode: u8,
    /// Transform type used for the residual (C TxType index; 0 = DCT_DCT).
    /// MUST match what the bitstream signals or the decoder inverse-
    /// transforms with the wrong basis.
    pub tx_type: u8,
    /// Motion vector (for inter blocks).
    pub mv: svtav1_types::motion::Mv,
    /// What MODE DECISION chose for an inter block
    /// (`docs/INTER-ENCODE-PLAN.md` §1s item 7).
    ///
    /// `Some` exactly when [`Self::is_inter`]; the pack REFUSES an
    /// `is_inter` block without it rather than falling back, because the
    /// fallback it replaced — a bare `write_mv` with a fresh `NmvContext`,
    /// no reference-frame / mode / DRL / interp-filter symbols and a RAW
    /// (undifferenced) MV — is not a decodable bitstream. A quiet fallback
    /// would turn that into a byte divergence instead of a break.
    ///
    /// Boxed because it is `None` on every intra block, which is every
    /// block of every still-picture cell in the 1,100-cell envelope, and
    /// `BlockDecision` is cloned per candidate.
    pub inter: Option<alloc::boxed::Box<InterDecision>>,
    /// Quantized coefficients.
    pub qcoeffs: alloc::vec::Vec<i32>,
    /// End of block position.
    pub eob: u16,
    /// Block width.
    pub width: u16,
    /// Block height.
    pub height: u16,
    /// Filter-intra mode (0..4) or 5 = not used — C block_mi
    /// filter_intra_mode (FILTER_INTRA_MODES sentinel). When used, the
    /// block codes y_mode DC + use_filter_intra=1 + the CDF5 mode symbol.
    pub filter_intra_mode: u8,
    /// UV prediction mode (0 = UV_DC; follows luma on M6 funnel leaves;
    /// 13 = UV_CFL_PRED when the CfL search won the chroma decision).
    pub uv_mode: u8,
    /// CfL alpha idx/signs — coded by write_cfl_alphas when uv_mode == 13.
    pub cfl_alpha_idx: u8,
    pub cfl_alpha_signs: u8,
    /// Luma palette (screen content, #71): the deduped ascending colors
    /// (2..=8) + the full nominal-size color index map. None = no palette
    /// (every non-DC / non-palette-winner block). The pack writes the
    /// n>0 mode-info arm + colors + map tokens when Some; MD carries it
    /// from the winning palette candidate.
    pub palette: Option<(alloc::vec::Vec<u16>, alloc::vec::Vec<u8>)>,
    /// Luma angle delta (directional modes on >= 8x8 blocks; 0 elsewhere).
    pub angle_delta: i8,
    /// Chroma angle delta (directional uv modes on >= 8x8 blocks).
    pub uv_angle_delta: i8,
    /// TX depth (0 = block-sized TX, 1 = quartered). Depth > 0 blocks
    /// carry per-txb data in `txb_qcoeffs`/`txb_eobs`/`txb_tx_types`.
    pub tx_depth: u8,
    /// Per-txb packed qcoeffs at tx_depth > 0, raster txb order.
    pub txb_qcoeffs: alloc::vec::Vec<alloc::vec::Vec<i32>>,
    /// Per-txb eobs (raster-domain nonzero indicator) at tx_depth > 0.
    pub txb_eobs: alloc::vec::Vec<u16>,
    /// Per-txb C TxType indices at tx_depth > 0.
    pub txb_tx_types: alloc::vec::Vec<u8>,
    /// Funnel-decided chroma: (u_q, v_q, u_eob, v_eob, u_recon, v_recon)
    /// — packed cw x ch rasters + the decision-phase reconstructions the
    /// walk copies into its chroma planes. None on non-funnel paths (the
    /// walk derives UV_DC chroma itself).
    #[allow(clippy::type_complexity)]
    pub chroma_dec: Option<(
        alloc::vec::Vec<i32>,
        alloc::vec::Vec<i32>,
        u16,
        u16,
        alloc::vec::Vec<u8>,
        alloc::vec::Vec<u8>,
    )>,
}

impl Default for BlockDecision {
    fn default() -> Self {
        Self {
            partition_type: PartitionType::None,
            is_inter: false,
            use_intrabc: false,
            dv: svtav1_types::motion::Mv::default(),
            dv_ref: svtav1_types::motion::Mv::default(),
            intra_mode: 0,
            tx_type: 0,
            mv: svtav1_types::motion::Mv::ZERO,
            inter: None,
            qcoeffs: alloc::vec::Vec::new(),
            eob: 0,
            width: 0,
            height: 0,
            filter_intra_mode: 5,
            uv_mode: 0,
            cfl_alpha_idx: 0,
            cfl_alpha_signs: 0,
            palette: None,
            angle_delta: 0,
            uv_angle_delta: 0,
            tx_depth: 0,
            txb_qcoeffs: alloc::vec::Vec::new(),
            txb_eobs: alloc::vec::Vec::new(),
            txb_tx_types: alloc::vec::Vec::new(),
            chroma_dec: None,
        }
    }
}

/// C `PartitionType` — unified: the single definition lives in `svtav1_types::partition`.
pub use svtav1_types::partition::PartitionType;

/// Result of encoding a single partition block.
#[derive(Debug, Clone)]
pub struct PartitionResult {
    /// The partition type chosen at this level.
    pub partition_type: PartitionType,
    /// Total RD cost for this partition decision.
    pub rd_cost: u64,
    /// Total distortion (SSE).
    pub distortion: u64,
    /// Total rate (estimated bits).
    pub rate: u32,
    /// Per-block encoding decisions (flat list, for backward compat).
    ///
    /// Populated ONLY by the legacy [`partition_search`] path. The funnel
    /// paths (`encode_fixed_tree`, `depth_refine::decide_sb`) leave it
    /// empty and carry their decisions solely in `tree`: the duplicate was
    /// a leaf-by-leaf deep clone of every [`BlockDecision`] (each owns up
    /// to nine `Vec`s) that nothing downstream ever read. Use
    /// [`PartitionTree::collect_decisions`] if you need the flat list, or
    /// [`PartitionTree::count_leaves`] if you only need its length.
    pub decisions: alloc::vec::Vec<BlockDecision>,
    /// Recursive partition tree for spec-conformant bitstream encoding.
    pub tree: Option<PartitionTree>,
    /// Number of coded blocks.
    pub num_blocks: u32,
}

/// Encode a superblock with recursive partition search.
/// Rate (1/256-bit units, the scale of every other rate in this module)
/// of coding partition symbol `sym` at a square node of `width`.
///
/// Real entropy cost from the DEFAULT partition CDFs via the C cost model
/// (av1_prob_cost / av1_cost_symbol, 1/512-bit units, halved with
/// rounding into this module's 1/256 scale) — replacing the old
/// hardcoded 48/56/64 constants that priced every partition equally and
/// NONE at zero. Neighbor sub-context 0 is used: the search runs before
/// the entropy pass, so the write-time above/left partition bits aren't
/// known here; row selection by block-size class carries the dominant
/// asymmetry (e.g. 64x64: NONE ~0.7 bits vs HORZ ~4.5 bits). Threading
/// the live neighbor context (C's md partition_context) is the next step
/// toward C's md RDO.
fn partition_rate_256(width: usize, sym: PartitionType) -> u32 {
    (crate::entropy::context::partition_symbol_cost(width, 0, sym as usize) + 1) >> 1
}

/// Add the PARTITION_NONE symbol cost to a leaf result at a SQUARE node
/// and rescore its rd with this search's lambda (the same
/// `dist + (lambda * rate) >> 8` formula every candidate below uses).
/// Non-square and 4x4 leaves code no partition symbol (the tile writer
/// only emits one for square tree leaves with dim > 4).
fn add_none_node_cost(result: &mut PartitionResult, width: usize, height: usize, lambda: u64) {
    if width == height && width > 4 {
        result.rate += partition_rate_256(width, PartitionType::None);
        result.rd_cost = result.distortion + ((lambda * result.rate as u64) >> 8);
    }
}

#[cfg(test)]
mod tests;
/// What MODE DECISION chooses for an inter block, as distinct from what is
/// DERIVED from the coded mode-info map.
///
/// C stores both on `BlkStruct` — `predmv`, `inter_mode_ctx` and `drl_ctx`
/// alongside the mode and the MV — with the comment "Store drl_ctx in blk to
/// avoid storing final_ref_mv_stack for EC" (mode_decision.c:3708). That is a
/// caching decision, not a semantic one: those three are a pure function of
/// the reference-MV stack, which is a pure function of the mode-info map the
/// pack walk already maintains and the decoder rebuilds. So they are derived
/// in the pack (`EntropyCtx::inter_mvp_fields`) rather than carried here, and
/// what this struct holds is only what MD actually DECIDES.
///
/// The split is not cosmetic. Deriving them in the pack makes the coded
/// contexts a function of the COMMITTED map, which is what a decoder sees —
/// so an MD path whose own grid lags (the pre-campaign recursion stamps no
/// mi map at all) cannot silently write a context no decoder can reproduce.
#[derive(Clone, Debug)]
pub struct InterDecision {
    /// C `block_mi.mode` — an inter mode (`NEWMV` and up).
    pub mode: svtav1_types::prediction::PredictionMode,
    /// C `block_mi.ref_frame`.
    pub ref_frame: [i8; 2],
    /// C `block_mi.mv`, eighth-pel.
    pub mv: [svtav1_types::motion::Mv; 2],
    /// C `blk_ptr->drl_index` — which ref-MV-stack entry this candidate was
    /// injected against. A DECISION (`svt_aom_choose_best_av1_mv_pred` picks
    /// it by rate), not a derivation.
    pub drl_index: u8,
    /// C `block_mi.interp_filters`, the packed `(y) | (x << 16)` pair.
    pub interp_filters: u32,
    /// C `block_mi.motion_mode`.
    pub motion_mode: crate::port_entropy_inter::modes::MotionMode,
    /// C `block_mi.num_proj_ref`.
    pub num_proj_ref: u16,
    /// C `blk_ptr->overlappable_neighbors`.
    pub overlappable_neighbors: u32,
    /// C `block_mi.skip_mode`.
    pub skip_mode: bool,
    /// C `block_mi.comp_group_idx` / `compound_idx` /
    /// `interinter_comp.type` — the CODED compound symbols, carried from the
    /// winning candidate. `comp_group_idx == 0, compound_idx == 1,
    /// COMPOUND_AVERAGE` (the `MD_COMP_AVG` row of `determine_compound_mode`)
    /// is what every reachable compound candidate carries at the ported
    /// `inter_compound_mode` levels.
    pub comp_group_idx: u8,
    pub compound_idx: u8,
    pub interinter_comp_type: u8,
    /// C `block_mi.interinter_comp.mask_type` / `wedge_index` /
    /// `wedge_sign` — the masked-compound mask the winner's
    /// `search_compound_diff_wedge` picked. `mask_type` is a coded symbol;
    /// the wedge pair is coded syntax and the DIFFWTD rebuild seed.
    pub interinter_mask_type: u8,
    pub interinter_wedge_index: i8,
    pub interinter_wedge_sign: bool,
    /// C `block_mi.is_interintra_used` / `interintra_mode` /
    /// `use_wedge_interintra` / `interintra_wedge_index` — the inter-intra
    /// blend the winner's `inter_intra_search` picked. The committed
    /// reconstruction re-runs the intra predictor and blends with these.
    pub is_interintra_used: bool,
    pub interintra_mode: u8,
    pub use_wedge_interintra: bool,
    pub interintra_wedge_index: i8,
    /// C `cand->wm_params_l0` — the local-warp affine model. For a GLOBALMV /
    /// GLOBAL_GLOBALMV leaf it is reference 0's GLOBAL model. Carried on the
    /// DECISION because the bd10 level re-encode has to rebuild this leaf's
    /// prediction from it: the warp parameters are not in the bitstream, and
    /// re-deriving them there would be a second transcription of the
    /// neighbour scan.
    pub wm_params: svtav1_types::motion::WarpedMotionParams,
    /// C `cand->wm_params_l1` — reference 1's model for a compound leaf. The
    /// bd10 re-encode's compound arm warps each reference by its own model,
    /// exactly like the 8-bit path.
    pub wm_params_l1: svtav1_types::motion::WarpedMotionParams,
}

mod neighbors;
pub use neighbors::*;

mod search;
pub use search::*;

mod fixed_tree;
pub(crate) use fixed_tree::*;

mod block_encode;
pub use block_encode::*;
